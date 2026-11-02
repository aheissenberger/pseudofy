//! HMAC-SHA256 pseudonyms (ADR-0003).

use std::collections::{HashMap, HashSet};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use unicode_normalization::UnicodeNormalization;

use crate::key::ProjectKey;
use crate::locale::Language;
use crate::names::{self, NameLists};
use crate::text::ascii_local_part;

type HmacSha256 = Hmac<Sha256>;

/// HMAC-SHA256 block size. A key shorter than a block is zero-padded by HMAC,
/// so a padded block gives the same MAC and the constructor cannot fail.
const BLOCK: usize = 64;
const SEP: u8 = 0x1f;
const BASE_DOMAIN: &str = "example.com";
const CONSONANTS: &[u8] = b"bcdfghjklmnprstvwz";
const VOWELS: &[u8] = b"aeiou";

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    First,
    Last,
    Email,
    Domain,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::First => "first",
            Kind::Last => "last",
            Kind::Email => "email",
            Kind::Domain => "domain",
        }
    }
}

/// Maps real values to pseudonyms and counts the collisions of one run.
pub struct Pseudonymizer {
    key: [u8; BLOCK],
    lists: NameLists,
    email_subdomains: bool,
    /// Per kind: pseudonym (lowercase) -> normalized real value.
    seen: HashMap<Kind, HashMap<String, String>>,
    /// Real values (per kind) that already count as a collision.
    counted: HashSet<(Kind, String)>,
    collisions: usize,
}

/// NFC, trim, lowercase.
fn normalize(real: &str) -> String {
    real.nfc().collect::<String>().trim().to_lowercase()
}

/// Big-endian u64 from 8 bytes of `bytes`, starting at `offset`.
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    bytes
        .iter()
        .skip(offset)
        .take(8)
        .fold(0u64, |acc, &b| (acc << 8) | u64::from(b))
}

fn pick(list: &[&'static str], value: u64) -> &'static str {
    let len = list.len() as u64;
    if len == 0 {
        return "";
    }
    let index = usize::try_from(value % len).unwrap_or(0);
    list.get(index).copied().unwrap_or("")
}

impl Pseudonymizer {
    pub fn new(key: &ProjectKey, language: Language, email_subdomains: bool) -> Self {
        Self::build(key, names::lists(language), email_subdomains)
    }

    #[cfg(test)]
    fn with_lists(key: &ProjectKey, lists: NameLists, email_subdomains: bool) -> Self {
        Self::build(key, lists, email_subdomains)
    }

    fn build(key: &ProjectKey, lists: NameLists, email_subdomains: bool) -> Self {
        let mut block = [0u8; BLOCK];
        for (slot, byte) in block.iter_mut().zip(key.as_bytes()) {
            *slot = *byte;
        }
        Self {
            key: block,
            lists,
            email_subdomains,
            seen: HashMap::new(),
            counted: HashSet::new(),
            collisions: 0,
        }
    }

    /// HMAC of `kind`, normalized value and attempt number.
    fn digest(&self, kind: Kind, normalized: &str, attempt: u32) -> [u8; 32] {
        let mut mac = <HmacSha256 as KeyInit>::new(&self.key.into());
        mac.update(kind.tag().as_bytes());
        mac.update(&[SEP]);
        mac.update(normalized.as_bytes());
        if attempt > 0 {
            mac.update(&[SEP]);
            mac.update(attempt.to_string().as_bytes());
        }
        mac.finalize().into_bytes().into()
    }

    /// Record the pseudonym and count a collision with another real value.
    fn record(&mut self, kind: Kind, pseudonym: &str, normalized_real: &str) {
        let map = self.seen.entry(kind).or_default();
        match map.get(&pseudonym.to_lowercase()) {
            Some(existing) if existing != normalized_real => {
                if self.counted.insert((kind, normalized_real.to_string())) {
                    self.collisions += 1;
                }
            }
            Some(_) => {}
            None => {
                map.insert(pseudonym.to_lowercase(), normalized_real.to_string());
            }
        }
    }

    fn name(&mut self, kind: Kind, real: &str) -> String {
        let normalized = normalize(real);
        let mut attempt = 0u32;
        let result = loop {
            let bytes = self.digest(kind, &normalized, attempt);
            let list = match kind {
                Kind::Last => &self.lists.last,
                _ => &self.lists.first,
            };
            let candidate = pick(list, u64_at(&bytes, 0));
            if candidate.to_lowercase() != normalized || attempt >= 64 || list.len() < 2 {
                break candidate;
            }
            attempt += 1;
        };
        self.record(kind, result, &normalized);
        result.to_string()
    }

    /// Pseudonym of one first-name token, as written in the name list.
    pub fn first_name(&mut self, real: &str) -> String {
        self.name(Kind::First, real)
    }

    /// Pseudonym of one last-name token, as written in the name list.
    pub fn last_name(&mut self, real: &str) -> String {
        self.name(Kind::Last, real)
    }

    /// Fake email address for one real email address.
    pub fn email(&mut self, real: &str) -> String {
        let normalized = normalize(real);
        let domain_part = normalized
            .rsplit_once('@')
            .map_or(normalized.as_str(), |(_, d)| d)
            .to_string();
        let domain = if self.email_subdomains {
            format!("{}.{BASE_DOMAIN}", self.domain_label(&domain_part))
        } else {
            BASE_DOMAIN.to_string()
        };
        let mut attempt = 0u32;
        let address = loop {
            let bytes = self.digest(Kind::Email, &normalized, attempt);
            let first = pick(&self.lists.first, u64_at(&bytes, 0));
            let last = pick(&self.lists.last, u64_at(&bytes, 8));
            let local = match (ascii_local_part(first), ascii_local_part(last)) {
                (f, l) if f.is_empty() && l.is_empty() => "user".to_string(),
                (f, l) if f.is_empty() => l,
                (f, l) if l.is_empty() => f,
                (f, l) => format!("{f}.{l}"),
            };
            let candidate = format!("{local}@{domain}");
            if candidate != normalized || attempt >= 64 {
                break candidate;
            }
            attempt += 1;
        };
        self.record(Kind::Email, &address, &normalized);
        address
    }

    /// Pronounceable label of 6 to 10 lowercase letters for a mail domain.
    fn domain_label(&mut self, normalized_domain: &str) -> String {
        let mut attempt = 0u32;
        let label = loop {
            let bytes = self.digest(Kind::Domain, normalized_domain, attempt);
            let candidate = label_from(&bytes);
            let full = format!("{candidate}.{BASE_DOMAIN}");
            if full != normalized_domain || attempt >= 64 {
                break candidate;
            }
            attempt += 1;
        };
        self.record(Kind::Domain, &label, normalized_domain);
        label
    }

    /// Number of collisions detected so far.
    pub fn collisions(&self) -> usize {
        self.collisions
    }
}

fn label_from(bytes: &[u8; 32]) -> String {
    let first = bytes.first().copied().unwrap_or(0);
    let length = 6 + usize::from(first % 5);
    let mut label = String::with_capacity(length);
    for (i, byte) in bytes.iter().skip(1).take(length).enumerate() {
        let set = if i % 2 == 0 { CONSONANTS } else { VOWELS };
        let index = usize::from(*byte) % set.len();
        label.push(char::from(set.get(index).copied().unwrap_or(b'a')));
    }
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: [&str; 5] = ["Anton", "Berta", "Cäcilie", "Dora", "Emil"];
    const LAST: [&str; 5] = ["Bauer", "Größer", "Hofer", "Lenz", "Weiß"];

    fn lists() -> NameLists {
        NameLists {
            first: FIRST.to_vec(),
            last: LAST.to_vec(),
        }
    }

    fn engine(seed: u8, subdomains: bool) -> Pseudonymizer {
        Pseudonymizer::with_lists(&ProjectKey::from_bytes([seed; 32]), lists(), subdomains)
    }

    #[test]
    fn results_are_stable_and_ignore_order() {
        let mut a = engine(1, true);
        let mut b = engine(1, true);
        let a1 = a.first_name("Karl");
        let a2 = a.last_name("Karl");
        let a3 = a.email("karl@firma.test");
        let b3 = b.email("karl@firma.test");
        let b2 = b.last_name("Karl");
        let b1 = b.first_name("Karl");
        assert_eq!((a1, a2, a3), (b1, b2, b3));
    }

    #[test]
    fn equal_normalized_values_give_equal_results() {
        let mut p = engine(1, true);
        assert_eq!(p.first_name("karl"), p.first_name("  KARL "));
        assert_eq!(p.email("Karl@Firma.Test"), p.email(" karl@firma.test"));
    }

    #[test]
    fn results_depend_on_the_key() {
        let mut a = engine(1, true);
        let mut b = engine(2, true);
        let names: Vec<String> = (0..40).map(|i| format!("person{i}")).collect();
        let differs = names.iter().any(|n| a.first_name(n) != b.first_name(n));
        assert!(differs);
        let differs = names
            .iter()
            .any(|n| a.email(&format!("{n}@x.test")) != b.email(&format!("{n}@x.test")));
        assert!(differs);
    }

    #[test]
    fn names_come_from_the_lists() {
        let mut p = engine(3, false);
        assert!(FIRST.contains(&p.first_name("Karl").as_str()));
        assert!(LAST.contains(&p.last_name("Karl").as_str()));
    }

    #[test]
    fn no_identity_mapping() {
        let mut p = engine(4, true);
        for real in FIRST.iter().chain(LAST.iter()) {
            assert_ne!(p.first_name(real).to_lowercase(), real.to_lowercase());
            assert_ne!(p.last_name(real).to_lowercase(), real.to_lowercase());
        }
    }

    #[test]
    fn email_format_without_subdomains() {
        let mut p = engine(5, false);
        for i in 0..20 {
            let e = p.email(&format!("user{i}@firma{i}.test"));
            let (local, domain) = e.split_once('@').unwrap_or(("", ""));
            assert_eq!(domain, "example.com");
            assert_ne!(local, "");
            assert!(
                local
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
            );
            assert!(e.is_ascii());
        }
    }

    #[test]
    fn email_local_part_is_transliterated() {
        let mut p = Pseudonymizer::with_lists(
            &ProjectKey::from_bytes([6; 32]),
            NameLists {
                first: vec!["Jürgen"],
                last: vec!["Größer"],
            },
            false,
        );
        assert_eq!(p.email("a@b.test"), "juergen.groesser@example.com");
    }

    #[test]
    fn domain_label_format() {
        let mut p = engine(7, true);
        for i in 0..50 {
            let e = p.email(&format!("a@firma{i}.test"));
            let domain = e.split_once('@').map_or("", |(_, d)| d);
            let label = domain.strip_suffix(".example.com").unwrap_or("");
            assert!((6..=10).contains(&label.len()), "{label}");
            assert!(label.bytes().all(|b| b.is_ascii_lowercase()));
            for (i, b) in label.bytes().enumerate() {
                assert_eq!(VOWELS.contains(&b), i % 2 == 1);
            }
        }
    }

    #[test]
    fn same_domain_one_label_different_domains_different_labels() {
        let mut p = engine(8, true);
        let domain = |e: &str| e.split_once('@').map(|(_, d)| d.to_string());
        let a = domain(&p.email("anna@Firma.Test"));
        let b = domain(&p.email("bernd@firma.test"));
        let c = domain(&p.email("carl@andere.test"));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn collisions_are_counted_once_per_real_value() {
        let mut p = engine(9, false);
        // Only 5 first names exist: 30 distinct reals must collide.
        for i in 0..30 {
            let real = format!("person{i}");
            p.first_name(&real);
            p.first_name(&real);
        }
        let c = p.collisions();
        assert!(c >= 25, "{c}");
        assert!(c <= 29, "{c}");
        let before = p.collisions();
        p.first_name("person0");
        assert_eq!(p.collisions(), before);
    }

    #[test]
    fn empty_lists_do_not_panic() {
        let mut p = Pseudonymizer::with_lists(
            &ProjectKey::from_bytes([0; 32]),
            NameLists {
                first: vec![],
                last: vec![],
            },
            true,
        );
        let _ = p.first_name("x");
        let _ = p.last_name("x");
        let _ = p.email("x@y.test");
    }
}
