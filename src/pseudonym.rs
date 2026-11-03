//! HMAC-SHA256 pseudonyms (ADR-0003).

use std::collections::{HashMap, HashSet};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::key::ProjectKey;
use crate::locale::Language;
use crate::names::{self, NameLists};
use crate::text::{ascii_local_part, normalize, normalize_domain};

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

/// The name tokens of the person that the last calls belong to.
///
/// The engine sees tokens, not rows. Calls for one person come together
/// (`Hans Peter Meier`: first, first, last), so a new group starts when a
/// token follows a group that has first and last tokens and the kind changes.
#[derive(Default)]
struct Group {
    /// (normalized real, fake) per token. An initial is recorded under its
    /// letter: real `j` and fake `b`.
    firsts: Vec<(String, String)>,
    lasts: Vec<(String, String)>,
    last_kind: Option<Kind>,
}

impl Group {
    /// (real key, fake key) of the person; `None` without first or last name.
    fn keys(&self) -> Option<(String, String)> {
        if self.firsts.is_empty() || self.lasts.is_empty() {
            return None;
        }
        let join = |parts: &[(String, String)], real: bool| {
            let words: Vec<&str> = parts
                .iter()
                .map(|(r, f)| if real { r.as_str() } else { f.as_str() })
                .collect();
            words.join(" ")
        };
        let key = |real: bool| {
            format!(
                "{}{}{}",
                join(&self.firsts, real),
                char::from(SEP),
                join(&self.lasts, real)
            )
        };
        Some((key(true), key(false)))
    }
}

/// Maps real values to pseudonyms and counts the collisions of one run.
///
/// A collision is two different real persons (first and last name together)
/// with one fake full name, or two different real emails (domains) with one
/// fake email (label). Two first names with one fake first name are no
/// collision: the output shows the fake full name.
pub struct Pseudonymizer {
    key: [u8; BLOCK],
    lists: NameLists,
    email_subdomains: bool,
    /// Per kind: pseudonym (lowercase) -> normalized real value.
    seen: HashMap<Kind, HashMap<String, String>>,
    /// Real values (per kind) that already count as a collision.
    counted: HashSet<(Kind, String)>,
    collisions: usize,
    group: Group,
    /// Fake full name (lowercase) -> the real persons that got it.
    persons: HashMap<String, HashSet<String>>,
}

/// `normalize` of an email address, with each `xn--` label of the domain
/// converted to Unicode (`bäckerei.test` = `xn--bckerei-5wa.test`).
fn normalize_email(real: &str) -> String {
    let Some((local, domain)) = real.rsplit_once('@') else {
        return normalize(real);
    };
    let labels: Vec<String> = normalize_domain(domain)
        .split('.')
        .map(|label| {
            match label
                .get(..4)
                .filter(|head| head.eq_ignore_ascii_case("xn--"))
                .and(label.get(4..))
                .filter(|_| label.len() <= MAX_LABEL)
                .and_then(punycode_decode)
                .filter(|decoded| !decoded.is_empty())
            {
                Some(decoded) => normalize_domain(&decoded),
                None => label.to_string(),
            }
        })
        .collect();
    format!("{}@{}", normalize(local), labels.join("."))
}

/// Longest label (in bytes) whose `xn--` form is decoded (RFC 1035). A longer
/// label stays as it is, so the decoding costs little.
const MAX_LABEL: usize = 63;

/// Value of a punycode digit: `a`-`z` are 0-25, `0`-`9` are 26-35.
fn punycode_digit(byte: u8) -> Option<u32> {
    match byte {
        b'a'..=b'z' => Some(u32::from(byte - b'a')),
        b'A'..=b'Z' => Some(u32::from(byte - b'A')),
        b'0'..=b'9' => Some(u32::from(byte - b'0') + 26),
        _ => None,
    }
}

/// Bias adaptation of RFC 3492 section 6.1.
fn punycode_adapt(delta: u32, count: u32, first: bool) -> u32 {
    let mut delta = if first { delta / 700 } else { delta / 2 };
    delta += delta / count.max(1);
    let mut k = 0;
    while delta > 455 {
        delta /= 35;
        k += 36;
    }
    k + 36 * delta / (delta + 38)
}

/// Decode the punycode of a label without the `xn--` prefix (RFC 3492).
/// `None` for an invalid or too large input.
fn punycode_decode(input: &str) -> Option<String> {
    let (basic, extended) = input.rsplit_once('-').unwrap_or(("", input));
    if !basic.is_ascii() {
        return None;
    }
    let mut out: Vec<char> = basic.chars().collect();
    let (mut code, mut pos, mut bias) = (128u32, 0u32, 72u32);
    let mut digits = extended.bytes().peekable();
    while digits.peek().is_some() {
        let old_pos = pos;
        let (mut weight, mut step) = (1u32, 36u32);
        loop {
            let digit = punycode_digit(digits.next()?)?;
            pos = pos.checked_add(digit.checked_mul(weight)?)?;
            let limit = if step <= bias {
                1
            } else if step >= bias + 26 {
                26
            } else {
                step - bias
            };
            if digit < limit {
                break;
            }
            weight = weight.checked_mul(36 - limit)?;
            step = step.checked_add(36)?;
        }
        let len = u32::try_from(out.len()).ok()?.checked_add(1)?;
        bias = punycode_adapt(pos - old_pos, len, old_pos == 0);
        code = code.checked_add(pos / len)?;
        pos %= len;
        let at = usize::try_from(pos).ok()?;
        if at > out.len() {
            return None;
        }
        out.insert(at, char::from_u32(code)?);
        pos += 1;
    }
    Some(out.into_iter().collect())
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
            group: Group::default(),
            persons: HashMap::new(),
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

    /// Add a name token to the current person; a finished person is recorded.
    fn note_token(&mut self, kind: Kind, real: String, fake: String) {
        let complete = !self.group.firsts.is_empty() && !self.group.lasts.is_empty();
        if complete && self.group.last_kind != Some(kind) {
            self.commit_group();
        }
        let list = if kind == Kind::Last {
            &mut self.group.lasts
        } else {
            &mut self.group.firsts
        };
        list.push((real, fake));
        self.group.last_kind = Some(kind);
    }

    /// Record the current person and count a collision with another person.
    fn commit_group(&mut self) {
        if let Some((real, fake)) = std::mem::take(&mut self.group).keys() {
            let set = self.persons.entry(fake).or_default();
            if !set.is_empty() && !set.contains(&real) {
                self.collisions += 1;
            }
            set.insert(real);
        }
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
        let single = normalized.chars().count() == 1;
        let mut attempt = 0u32;
        let result = loop {
            let bytes = self.digest(kind, &normalized, attempt);
            let list = match kind {
                Kind::Last => &self.lists.last,
                _ => &self.lists.first,
            };
            let candidate = pick(list, u64_at(&bytes, 0));
            let same_initial = single
                && candidate
                    .chars()
                    .next()
                    .and_then(|c| c.to_lowercase().next())
                    == normalized.chars().next();
            if (candidate.to_lowercase() != normalized && !same_initial)
                || attempt >= 64
                || list.len() < 2
            {
                break candidate;
            }
            attempt += 1;
        };
        let fake = if single {
            result.chars().take(1).collect::<String>().to_lowercase()
        } else {
            result.to_lowercase()
        };
        self.note_token(kind, normalized, fake);
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
        let normalized = normalize_email(real);
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
        let open = self.group.keys().map_or(0, |(real, fake)| {
            self.persons.get(&fake).map_or(0, |set| {
                usize::from(!set.is_empty() && !set.contains(&real))
            })
        });
        self.collisions + open
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
    fn one_letter_values_never_keep_their_letter() {
        let mut p = engine(10, false);
        for c in ['a', 'b', 'c', 'd', 'e', 'h', 'l', 'w', 'A', 'W'] {
            let real = c.to_string();
            let lower = c.to_lowercase().next();
            assert_ne!(
                p.first_name(&real)
                    .chars()
                    .next()
                    .and_then(|x| x.to_lowercase().next()),
                lower
            );
            assert_ne!(
                p.last_name(&real)
                    .chars()
                    .next()
                    .and_then(|x| x.to_lowercase().next()),
                lower
            );
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
    fn collisions_count_persons_once() {
        let mut p = engine(9, false);
        // 5 first and 5 last names give 25 fake full names: 60 distinct
        // persons must collide.
        for i in 0..60 {
            let real = format!("person{i}");
            p.first_name(&real);
            p.last_name(&real);
            p.first_name(&real);
            p.last_name(&real);
        }
        let c = p.collisions();
        assert!((35..=59).contains(&c), "{c}");
        let before = p.collisions();
        p.first_name("person0");
        p.last_name("person0");
        assert_eq!(p.collisions(), before);
    }

    #[test]
    fn first_name_only_collisions_are_not_counted() {
        let mut p = engine(9, false);
        for i in 0..60 {
            p.first_name(&format!("person{i}"));
        }
        for i in 0..60 {
            p.last_name(&format!("person{i}"));
        }
        assert_eq!(p.collisions(), 0);
    }

    #[test]
    fn initial_is_recorded_under_its_letter() {
        let mut p = engine(9, false);
        // `J` and `K` may get fake names with one letter; the person is
        // recorded as the letter and the last name.
        p.first_name("j");
        p.last_name("meier");
        p.first_name("k");
        p.last_name("meier");
        p.first_name("j");
        p.last_name("meier");
        assert!(p.collisions() <= 1);
        let mut q = engine(9, false);
        q.first_name("j");
        q.last_name("meier");
        q.first_name("j");
        q.last_name("meier");
        assert_eq!(q.collisions(), 0);
    }

    #[test]
    fn two_hundred_persons_do_not_collide() {
        let mut p = Pseudonymizer::new(&ProjectKey::from_bytes([11; 32]), Language::De, false);
        let letters = |mut n: usize| {
            let mut s = String::new();
            loop {
                s.push(char::from(b'a' + u8::try_from(n % 26).unwrap_or(0)));
                n /= 26;
                if n == 0 {
                    break s;
                }
            }
        };
        for i in 0..200 {
            p.first_name(&format!("vor{}", letters(i)));
            p.last_name(&format!("nach{}", letters(i)));
        }
        assert_eq!(p.collisions(), 0);
    }

    #[test]
    fn colliding_emails_are_counted() {
        let mut p = Pseudonymizer::with_lists(
            &ProjectKey::from_bytes([1; 32]),
            NameLists {
                first: vec!["Anton"],
                last: vec!["Bauer"],
            },
            false,
        );
        p.email("a@x.test");
        assert_eq!(p.collisions(), 0);
        p.email("b@x.test");
        assert_eq!(p.collisions(), 1);
        p.email("b@x.test");
        assert_eq!(p.collisions(), 1);
    }

    #[test]
    fn normalization_pairs_get_one_pseudonym() {
        let mut p = engine(1, true);
        for (a, b) in [
            ("O\u{2019}Brien", "O'Brien"),
            ("O\u{2bc}Brien", "o'brien"),
            ("Strau\u{df}", "STRAUSS"),
            ("Strau\u{df}", "Strauss"),
            ("\u{130}lker", "Ilker"),
            ("\u{130}lker", "ilker"),
            ("I\u{307}lker", "ILKER"),
            ("M\u{fc}\u{ad}ller", "M\u{fc}ller"),
            ("Kes\u{200b}ta", "Kesta"),
            ("\u{feff}Kesta", "kesta"),
        ] {
            assert_eq!(p.first_name(a), p.first_name(b), "{a} {b}");
            assert_eq!(p.last_name(a), p.last_name(b), "{a} {b}");
        }
        // A domain keeps `ß` (IDNA2008).
        assert_ne!(p.email("a@Stra\u{df}e.test"), p.email("a@STRASSE.test"));
        assert_eq!(p.email("a@Stra\u{df}e.test"), p.email("a@STRA\u{df}E.test"));
        assert_eq!(p.email("Stra\u{df}e@x.test"), p.email("STRASSE@x.test"));
        assert_eq!(
            p.last_name("\u{39f}\u{394}\u{3a5}\u{3a3}\u{3a3}\u{395}\u{3a5}\u{3a3}"),
            p.last_name("\u{39f}\u{3b4}\u{3c5}\u{3c3}\u{3c3}\u{3b5}\u{3c5}\u{3c2}")
        );
    }

    #[test]
    fn idn_domains_get_one_label() {
        let mut p = engine(1, true);
        assert_eq!(
            p.email("anna@b\u{e4}ckerei.test"),
            p.email("anna@xn--bckerei-5wa.test")
        );
        assert_eq!(
            p.email("anna@B\u{c4}CKEREI.test"),
            p.email("anna@XN--bckerei-5wa.test")
        );
        assert_ne!(
            p.email("anna@xn--bckerei-5wa.test"),
            p.email("anna@other.test")
        );
    }

    #[test]
    fn punycode_decodes_rfc_samples() {
        assert_eq!(
            punycode_decode("bckerei-5wa").as_deref(),
            Some("b\u{e4}ckerei")
        );
        assert_eq!(
            punycode_decode("mnchen-3ya").as_deref(),
            Some("m\u{fc}nchen")
        );
        assert_eq!(punycode_decode("maana-pta").as_deref(), Some("ma\u{f1}ana"));
        // RFC 3492 7.1 (A) Arabic (Egyptian).
        assert_eq!(
            punycode_decode("egbpdaj6bu4bxfgehfvwxn").as_deref(),
            Some(
                "\u{644}\u{64a}\u{647}\u{645}\u{627}\u{628}\u{62a}\u{643}\u{644}\u{645}\u{648}\u{634}\u{639}\u{631}\u{628}\u{64a}\u{61f}"
            )
        );
        // RFC 3492 7.1 (L) 3<nen>B<gumi><kinpachi><sensei>.
        assert_eq!(
            punycode_decode("3B-ww4c5e180e575a65lsy2b").as_deref(),
            Some("3\u{5e74}B\u{7d44}\u{91d1}\u{516b}\u{5148}\u{751f}")
        );

        assert_eq!(punycode_decode("-").as_deref(), Some(""));
        assert_eq!(punycode_decode("a-").as_deref(), Some("a"));
    }

    #[test]
    fn invalid_punycode_stays_as_it_is() {
        assert_eq!(punycode_decode("!!"), None);
        assert_eq!(punycode_decode("zzzzzzzzzzzzzzzzzzzzzzzz"), None);
        assert_eq!(punycode_decode("\u{e4}-abc"), None);
        assert_eq!(normalize_email("a@xn--!!.test"), "a@xn--!!.test");
        assert_eq!(normalize_email("a@xn--.test"), "a@xn--.test");
    }

    #[test]
    fn long_xn_labels_stay_as_they_are_and_cost_little() {
        let label = format!("xn--{}", "a".repeat(400_000));
        let email = format!("a@{label}.test");
        let start = std::time::Instant::now();
        assert_eq!(normalize_email(&email), email);
        assert!(start.elapsed() < std::time::Duration::from_secs(60));
        // A label of 63 bytes is still decoded.
        assert_eq!(
            normalize_email("a@xn--bckerei-5wa.test"),
            "a@b\u{e4}ckerei.test"
        );
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
