//! Known names: the real names from the name columns of all project files
//! (spec 0004). They stay in memory only and are never printed.
//!
//! Matching rules:
//! - Whole word: a name token is a maximal run of letters, combining marks and
//!   `'` (as in `text.rs`). "Annabelle" and "Anna's" do not match "Anna".
//! - Case-insensitive; the replacement keeps the case style of the match.
//! - A longer match has priority over a shorter one.
//! - One-letter tokens (initials) never match on their own and are not part
//!   of a full-name form.
//! - A full-name match gets its parts from the split rule of
//!   `text::replace_name` with `NameRole::Full`.

use std::collections::BTreeSet;

use regex::{Regex, RegexBuilder};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::config::FreeText;
use crate::text::{NameRole, Part, replace_name};

/// Size limit for the compiled matcher, so thousands of names compile.
const REGEX_LIMIT: usize = 512 * 1024 * 1024;

/// Character class body of a name character, for the word boundary.
const WORD: &str = r"\p{L}\p{M}'";

/// The known names of one project. Pass 1 of `run` fills it.
#[derive(Default)]
pub struct KnownNames {
    firsts: BTreeSet<String>,
    lasts: BTreeSet<String>,
    persons: BTreeSet<(Vec<String>, Vec<String>)>,
}

fn is_token_char(c: char) -> bool {
    c.is_alphabetic() || c == '\'' || is_combining_mark(c)
}

/// Normalized name tokens of `value` (NFC, lowercase), without initials.
fn tokens(value: &str) -> Vec<String> {
    let nfc: String = value.nfc().collect();
    nfc.split(|c: char| !is_token_char(c))
        .filter(|t| t.chars().any(char::is_alphabetic) && t.chars().count() > 1)
        .map(str::to_lowercase)
        .collect()
}

fn join_pattern(parts: &[String]) -> String {
    parts
        .iter()
        .map(|t| regex::escape(t))
        .collect::<Vec<_>>()
        .join(r"\s+")
}

fn size_of(parts: &[String]) -> usize {
    parts.iter().map(|t| t.chars().count() + 1).sum()
}

impl KnownNames {
    /// Add one person from one row: the first-name and last-name cell values
    /// (each may hold several tokens, e.g. "Anna Lena"). Either may be empty.
    pub fn add_person(&mut self, first: &str, last: &str) {
        let f = tokens(first);
        let l = tokens(last);
        self.firsts.extend(f.iter().cloned());
        self.lasts.extend(l.iter().cloned());
        if !f.is_empty() && !l.is_empty() {
            self.persons.insert((f, l));
        }
    }

    /// Add the value of one `full_name` cell ("First Middle Last" or "Last, First").
    pub fn add_full(&mut self, value: &str) {
        let nfc: String = value.nfc().collect();
        let (first, last) = if let Some((before, after)) = nfc.split_once(',') {
            (after.to_string(), before.to_string())
        } else {
            let all: Vec<&str> = nfc
                .split(|c: char| !is_token_char(c))
                .filter(|t| t.chars().any(char::is_alphabetic))
                .collect();
            match all.split_last() {
                Some((last, rest)) => (rest.join(" "), (*last).to_string()),
                None => return,
            }
        };
        self.add_person(&first, &last);
    }

    /// Build the whole-word matcher for `mode`.
    pub fn matcher(&self, mode: FreeText) -> anyhow::Result<Matcher> {
        // (sort length, pattern)
        let mut alts: BTreeSet<(usize, String)> = BTreeSet::new();
        let persons = self.persons.iter().filter(|_| mode != FreeText::Off);
        for (f, l) in persons {
            let total = size_of(f) + size_of(l);
            let (fp, lp) = (join_pattern(f), join_pattern(l));
            alts.insert((total, format!(r"{fp}\s+{lp}")));
            alts.insert((total, format!(r"{lp}\s*,\s*{fp}")));
        }
        if mode == FreeText::Tokens {
            for t in self.firsts.iter().chain(self.lasts.iter()) {
                alts.insert((size_of(std::slice::from_ref(t)), regex::escape(t)));
            }
        }
        if alts.is_empty() {
            return Ok(Matcher {
                regex: None,
                firsts: BTreeSet::new(),
                lasts: BTreeSet::new(),
            });
        }
        // Longest first: regex alternation is leftmost-first.
        let body = alts
            .iter()
            .rev()
            .map(|(_, p)| p.as_str())
            .collect::<Vec<_>>()
            .join("|");
        let pattern = format!(r"(?:^|[^{WORD}])({body})(?:$|[^{WORD}])");
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(true)
            .unicode(true)
            .size_limit(REGEX_LIMIT)
            .dfa_size_limit(REGEX_LIMIT)
            .build()
            .map_err(|_| anyhow::anyhow!("cannot build the known-name matcher"))?;
        Ok(Matcher {
            regex: Some(regex),
            firsts: self.firsts.clone(),
            lasts: self.lasts.clone(),
        })
    }
}

/// Whole-word, case-insensitive matcher for the known names.
pub struct Matcher {
    regex: Option<Regex>,
    firsts: BTreeSet<String>,
    lasts: BTreeSet<String>,
}

impl Matcher {
    /// Replace each known name in `text`. `replace(part, token)` gives the
    /// pseudonym of one real token; the result keeps the case style of the
    /// matched text. Returns the new text and the number of replaced names.
    pub fn replace(
        &self,
        text: &str,
        replace: &mut dyn FnMut(Part, &str) -> String,
    ) -> (String, usize) {
        let text: String = text.nfc().collect();
        let Some(regex) = &self.regex else {
            return (text, 0);
        };
        let mut out = String::with_capacity(text.len());
        let mut pos = 0;
        let mut count = 0;
        while pos <= text.len() {
            let Some(caps) = regex.captures_at(&text, pos) else {
                break;
            };
            let Some(m) = caps.get(1) else { break };
            out.push_str(text.get(pos..m.start()).unwrap_or(""));
            let matched = m.as_str();
            pos = m.end();
            if matched.contains(|c: char| c.is_whitespace() || c == ',') {
                out.push_str(&replace_name(matched, NameRole::Full, replace));
                count += 1;
                continue;
            }
            let lower = matched.to_lowercase();
            let role = if self.firsts.contains(&lower) {
                NameRole::First
            } else if self.lasts.contains(&lower) {
                NameRole::Last
            } else {
                // Not reachable: every single-token alternative is known.
                out.push_str(matched);
                continue;
            };
            out.push_str(&replace_name(matched, role, replace));
            count += 1;
        }
        out.push_str(text.get(pos..).unwrap_or(""));
        (out, count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(part: Part, token: &str) -> String {
        let tag = match part {
            Part::First => "F",
            Part::Last => "L",
        };
        format!("{tag}{}", token.chars().count())
    }

    fn run(names: &KnownNames, mode: FreeText, text: &str) -> (String, usize) {
        let m = names.matcher(mode).unwrap();
        m.replace(text, &mut fake)
    }

    fn sample() -> KnownNames {
        let mut k = KnownNames::default();
        k.add_person("Anna", "Muster");
        k
    }

    #[test]
    fn whole_word_only() {
        let k = sample();
        assert_eq!(
            run(&k, FreeText::Tokens, "Annabelle Mustering"),
            ("Annabelle Mustering".to_string(), 0)
        );
        // An apostrophe is a name character: "Anna's" is another token.
        assert_eq!(run(&k, FreeText::Tokens, "Anna's").1, 0);
        assert_eq!(run(&k, FreeText::Tokens, "(Anna), ok").0, "(F4), ok");
    }

    #[test]
    fn case_style_kept() {
        let k = sample();
        let mut seen = Vec::new();
        let m = k.matcher(FreeText::Tokens).unwrap();
        let (out, n) = m.replace("ANNA MUSTER and anna", &mut |p, t| {
            seen.push((p, t.to_string()));
            match p {
                Part::First => "Berta".to_string(),
                Part::Last => "Kunz".to_string(),
            }
        });
        assert_eq!(out, "BERTA KUNZ and berta");
        assert_eq!(n, 2);
        assert_eq!(seen[0], (Part::First, "ANNA".to_string()));
    }

    #[test]
    fn longest_match_wins() {
        let k = sample();
        let mut seen = Vec::new();
        let m = k.matcher(FreeText::Tokens).unwrap();
        let (out, n) = m.replace("Call Anna Muster now", &mut |p, t| {
            seen.push((p, t.to_string()));
            "X".to_string()
        });
        assert_eq!(n, 1);
        assert_eq!(out, "Call X X now");
        assert_eq!(
            seen,
            vec![
                (Part::First, "Anna".to_string()),
                (Part::Last, "Muster".to_string())
            ]
        );
    }

    #[test]
    fn comma_form() {
        let k = sample();
        let mut seen = Vec::new();
        let m = k.matcher(FreeText::Full).unwrap();
        let (out, n) = m.replace("See Muster, Anna.", &mut |p, t| {
            seen.push((p, t.to_string()));
            "Zed".to_string()
        });
        assert_eq!(out, "See Zed, Zed.");
        assert_eq!(n, 1);
        assert_eq!(seen[0], (Part::Last, "Muster".to_string()));
        assert_eq!(seen[1], (Part::First, "Anna".to_string()));
    }

    #[test]
    fn full_mode_vs_tokens_mode() {
        let k = sample();
        assert_eq!(
            run(&k, FreeText::Full, "Anna said"),
            ("Anna said".into(), 0)
        );
        assert_eq!(
            run(&k, FreeText::Tokens, "Anna said"),
            ("F4 said".into(), 1)
        );
        assert_eq!(run(&k, FreeText::Full, "Anna  Muster").1, 1);
    }

    #[test]
    fn adjacent_matches() {
        let k = sample();
        assert_eq!(run(&k, FreeText::Tokens, "Anna,Muster").0, "F4,L6");
    }

    #[test]
    fn add_full_forms() {
        let mut k = KnownNames::default();
        k.add_full("Müller, Jörg");
        k.add_full("Berta C. Kunz");
        assert_eq!(run(&k, FreeText::Full, "Jörg Müller").1, 1);
        assert_eq!(run(&k, FreeText::Full, "Berta Kunz").1, 1);
        assert_eq!(run(&k, FreeText::Tokens, "C. said").1, 0);
    }

    #[test]
    fn umlauts_and_nfd() {
        let mut k = KnownNames::default();
        k.add_person("Jörg", "Müller");
        assert_eq!(run(&k, FreeText::Tokens, "Hi JÖRG Müller!").1, 1);
        assert_eq!(run(&k, FreeText::Tokens, "Jörgen Müllers").1, 0);
        let nfd = "Jo\u{308}rg Mu\u{308}ller";
        let (out, n) = run(&k, FreeText::Tokens, nfd);
        assert_eq!((out.as_str(), n), ("F4 L6", 1));
    }

    #[test]
    fn empty_set_matches_nothing() {
        let k = KnownNames::default();
        assert_eq!(
            run(&k, FreeText::Tokens, "Anna Muster"),
            ("Anna Muster".into(), 0)
        );
    }

    #[test]
    fn many_names_compile() {
        let mut k = KnownNames::default();
        for i in 0..2000 {
            let code: String = i
                .to_string()
                .chars()
                .map(|d| (b'a' + (d as u8 - b'0')) as char)
                .collect();
            k.add_person(&format!("Vorname{code}"), &format!("Nachname{code}"));
        }
        k.add_person("Anna", "Muster");
        assert_eq!(run(&k, FreeText::Tokens, "Call Anna Muster").1, 1);
        assert_eq!(
            run(&k, FreeText::Off, "Call Anna Muster"),
            ("Call Anna Muster".to_string(), 0)
        );
    }
}
