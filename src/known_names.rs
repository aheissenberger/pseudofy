//! Known names: the real names from the name columns of all project files
//! (spec 0004). They stay in memory only and are never printed.
//!
//! The matcher works on tokens, not on a regex: the text is split into name
//! tokens once, and hash lookups over token windows find the names. The time
//! is linear in the text length (a window has at most `MAX_WORDS` tokens).
//!
//! Matching rules:
//! - Token: a maximal run of letters, combining marks and `'`. Leading and
//!   trailing `'` and a trailing possessive `'s` are not part of the token, so
//!   `'Anna Muster'` and `Anna's` match, and `O'Brien` is one token. A curly
//!   apostrophe (U+2019, U+02BC) is an apostrophe like `'`; a hyphen separates
//!   tokens, as in `text.rs`.
//! - Whole word: only complete tokens match. "Annabelle" does not match "Anna".
//! - Folding: stored and text tokens are compared after NFC, lowercase,
//!   `ß` to `ss`, and removal of U+0307 after `i`. The replacement keeps the
//!   case style of the matched text (`text::replace_name`).
//! - A person (first tokens and last tokens of one row) matches as the
//!   sequence `First Last` or `Last, First`. Inside the sequence the tokens
//!   are separated by whitespace or hyphens. One one-letter initial (`C` or
//!   `C.`) may stand between the first and the last part. The tokens of a
//!   matched person get the split of that person (also for multi-token last
//!   names). If two persons give the same sequence with different splits,
//!   the rule of `NameRole::Full` applies (only the last token is Last).
//! - A longer match has priority over a shorter one; a person sequence wins
//!   over single tokens. A full name counts as one replacement.
//! - Single tokens (mode `tokens`): a token known as first and as last name
//!   gets the role that it has more often in the columns; a tie gives First.
//!   A last-name token that is all lowercase and not the final token
//!   in every last-name cell (a particle like `de`, `von`) does not match alone.
//! - One-letter tokens never match alone.

use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet};

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::{UnicodeNormalization, is_nfc};

use crate::config::FreeText;
use crate::text::{NameRole, Part, replace_name};

/// Longest person sequence in tokens. A longer person is ignored.
const MAX_WORDS: usize = 12;

/// Separator of the tokens in a lookup key.
const KEY_SEP: char = '\u{1}';

/// The known names of one project. Pass 1 of `run` fills it.
#[derive(Default)]
pub struct KnownNames {
    first_count: HashMap<String, usize>,
    last_count: HashMap<String, usize>,
    /// Last-name tokens that were all lowercase in some cell.
    lower_last: HashSet<String>,
    /// Last-name tokens that were not all lowercase in some cell.
    cased_last: HashSet<String>,
    persons: HashSet<(Vec<String>, Vec<String>)>,
    /// Folded token → the spellings seen in name columns (NFC, lowercase).
    /// A free-text token with a column spelling gets that spelling; any other
    /// token gets the smallest one. Independent of the file order.
    canon: HashMap<String, BTreeSet<String>>,
}

/// `'`, the curly apostrophe U+2019 and the modifier letter U+02BC.
fn is_apos(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '\u{2bc}')
}

fn is_token_char(c: char) -> bool {
    c.is_alphabetic() || is_apos(c) || is_combining_mark(c)
}

/// A name token: byte range in the (NFC) text.
struct Tok {
    start: usize,
    end: usize,
    /// One letter: an initial.
    initial: bool,
}

fn push_run(text: &str, from: usize, to: usize, out: &mut Vec<Tok>) {
    let Some(run) = text.get(from..to) else {
        return;
    };
    let start = from + (run.len() - run.trim_start_matches(is_apos).len());
    let trim =
        |s: usize, e: usize| s + text.get(s..e).unwrap_or("").trim_end_matches(is_apos).len();
    let mut end = trim(start, to);
    let mut rev = text.get(start..end).unwrap_or("").char_indices().rev();
    if let (Some((_, last)), Some((at, apos))) = (rev.next(), rev.next())
        && matches!(last, 's' | 'S')
        && is_apos(apos)
        && at > 0
    {
        end = trim(start, start + at);
    }
    let Some(token) = text.get(start..end) else {
        return;
    };
    if !token.chars().any(char::is_alphabetic) {
        return;
    }
    out.push(Tok {
        start,
        end,
        initial: token.chars().count() == 1,
    });
}

fn lex(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut run: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if is_token_char(c) {
            if run.is_none() {
                run = Some(i);
            }
        } else if let Some(s) = run.take() {
            push_run(text, s, i, &mut out);
        }
    }
    if let Some(s) = run {
        push_run(text, s, text.len(), &mut out);
    }
    out
}

/// Fold a token: NFC, lowercase, `ß` to `ss`, no U+0307 after `i`.
fn fold(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    for c in token.nfc() {
        for l in c.to_lowercase() {
            match l {
                'ß' => out.push_str("ss"),
                '\u{2019}' | '\u{2bc}' => out.push('\''),
                '\u{307}' if out.ends_with('i') => {}
                _ => out.push(l),
            }
        }
    }
    out
}

fn is_sep_char(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '\u{2010}' | '\u{2011}' | '\u{2013}' | '\u{2014}')
}

enum Gap {
    /// Whitespace and hyphens.
    Sep,
    /// Whitespace around exactly one comma.
    Comma,
    Bad,
}

fn classify(gap: &str) -> Gap {
    if !gap.is_empty() && gap.chars().all(is_sep_char) {
        Gap::Sep
    } else if gap.chars().all(|c| c == ',' || c.is_whitespace()) && gap.matches(',').count() == 1 {
        Gap::Comma
    } else {
        Gap::Bad
    }
}

/// Gap after an initial: an optional `.`, then whitespace or hyphens.
fn initial_gap(gap: &str) -> bool {
    match gap.strip_prefix('.') {
        Some(rest) => rest.chars().all(is_sep_char),
        None => !gap.is_empty() && gap.chars().all(is_sep_char),
    }
}

/// Folded tokens of `value` without initials, each with the flag whether it
/// is all lowercase in the value and its spelling in the value.
fn name_tokens(value: &str) -> Vec<(String, bool, String)> {
    let nfc: String = value.nfc().collect();
    lex(&nfc)
        .iter()
        .filter(|t| !t.initial)
        .filter_map(|t| nfc.get(t.start..t.end))
        .map(|raw| {
            let lower = raw
                .chars()
                .filter(|c| c.is_alphabetic())
                .all(char::is_lowercase);
            (fold(raw), lower, raw.to_string())
        })
        .collect()
}

/// Remove `<...>` parts (an unclosed `<` runs to the end).
fn strip_angle(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut inside = false;
    for c in value.chars() {
        match c {
            '<' => inside = true,
            '>' if inside => {
                inside = false;
                out.push(' ');
            }
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

impl KnownNames {
    fn add_spelling(&mut self, folded: &str, raw: &str) {
        self.canon
            .entry(folded.to_string())
            .or_default()
            .insert(raw.to_lowercase());
    }

    /// Add the tokens of a first-name cell value. Returns the folded tokens.
    pub fn add_first(&mut self, value: &str) -> Vec<String> {
        let tokens = name_tokens(value);
        for (t, _, raw) in &tokens {
            *self.first_count.entry(t.clone()).or_insert(0) += 1;
            self.add_spelling(t, raw);
        }
        tokens.into_iter().map(|(t, _, _)| t).collect()
    }

    /// Add the tokens of a last-name cell value. Returns the folded tokens.
    pub fn add_last(&mut self, value: &str) -> Vec<String> {
        let tokens = name_tokens(value);
        let n = tokens.len();
        for (i, (t, lower, raw)) in tokens.iter().enumerate() {
            *self.last_count.entry(t.clone()).or_insert(0) += 1;
            self.add_spelling(t, raw);
            // A particle is lowercase and not the final token of its cell.
            let set = if *lower && i + 1 != n {
                &mut self.lower_last
            } else {
                &mut self.cased_last
            };
            set.insert(t.clone());
        }
        tokens.into_iter().map(|(t, _, _)| t).collect()
    }

    /// Add one person from one row: the first-name and last-name cell values
    /// (each may hold several tokens, e.g. "Anna Lena"). Either may be empty.
    pub fn add_person(&mut self, first: &str, last: &str) {
        let f = self.add_first(first);
        let l = self.add_last(last);
        if !f.is_empty() && !l.is_empty() && f.len() + l.len() <= MAX_WORDS {
            self.persons.insert((f, l));
        }
    }

    /// Add the value of one `full_name` cell ("First Middle Last" or
    /// "Last, First"). Angle-bracket parts (`<...>`) are ignored.
    pub fn add_full(&mut self, value: &str) {
        let clean = strip_angle(value);
        let nfc: String = clean.nfc().collect();
        if let Some((before, after)) = nfc.split_once(',') {
            self.add_person(after, before);
            return;
        }
        // Same assignment as `NameRole::Full`: the final token is Last, an
        // initial included. A final initial leaves no last name.
        let toks = lex(&nfc);
        let Some((last, rest)) = toks.split_last() else {
            return;
        };
        if last.initial {
            self.add_first(nfc.get(..last.start).unwrap_or(""));
            return;
        }
        let first = match rest.first() {
            Some(a) => nfc.get(a.start..last.start).unwrap_or(""),
            None => "",
        };
        let last = nfc.get(last.start..last.end).unwrap_or("");
        self.add_person(first, last);
    }

    /// Build the whole-word matcher for `mode`.
    pub fn matcher(&self, mode: FreeText) -> Matcher {
        let mut m = Matcher::default();
        if mode == FreeText::Off {
            return m;
        }
        m.canon.clone_from(&self.canon);
        for (f, l) in &self.persons {
            let seq: Vec<&String> = f.iter().chain(l).collect();
            insert_split(&mut m.persons, &seq, Split::At(f.len()));
            let rev: Vec<&String> = l.iter().chain(f).collect();
            insert_split(&mut m.commas, &rev, Split::At(l.len()));
            for s in [&seq, &rev] {
                if let Some(head) = s.first() {
                    m.heads.insert((*head).clone());
                }
            }
            m.max_words = m.max_words.max(seq.len());
        }
        if mode == FreeText::Tokens {
            let all: HashSet<&String> = self
                .first_count
                .keys()
                .chain(self.last_count.keys())
                .collect();
            for t in all {
                let firsts = self.first_count.get(t).copied().unwrap_or(0);
                let particle = self.lower_last.contains(t) && !self.cased_last.contains(t);
                let lasts = if particle {
                    0
                } else {
                    self.last_count.get(t).copied().unwrap_or(0)
                };
                if firsts == 0 && lasts == 0 {
                    continue;
                }
                let part = if firsts >= lasts {
                    Part::First
                } else {
                    Part::Last
                };
                m.singles.insert(t.clone(), part);
            }
        }
        m
    }
}

/// Split of a person sequence.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Split {
    /// The first n tokens are First, the rest Last. In the comma form the
    /// first n tokens are the last name.
    At(usize),
    /// Persons disagree: the rule of `NameRole::Full`.
    Full,
}

fn insert_split(map: &mut HashMap<String, Split>, seq: &[&String], split: Split) {
    let key = seq
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(&KEY_SEP.to_string());
    match map.entry(key) {
        Entry::Occupied(mut e) => {
            if *e.get() != split {
                e.insert(Split::Full);
            }
        }
        Entry::Vacant(v) => {
            v.insert(split);
        }
    }
}

/// A person match: token indexes and the split.
struct Found {
    words: Vec<usize>,
    init: Option<usize>,
    comma: Option<usize>,
    split: Split,
}

/// Whole-word, case-insensitive matcher for the known names.
#[derive(Default)]
pub struct Matcher {
    persons: HashMap<String, Split>,
    commas: HashMap<String, Split>,
    heads: HashSet<String>,
    singles: HashMap<String, Part>,
    max_words: usize,
    canon: HashMap<String, BTreeSet<String>>,
}

/// A one-letter initial right after a matched name (`Kesta Wolmar J.`) gets
/// the last-name role, as the final token of a full name in a name column.
fn trailing_initial(text: &str, toks: &[Tok], last: usize, edits: &mut Vec<(usize, Part)>) {
    let (Some(prev), Some(next)) = (toks.get(last), toks.get(last + 1)) else {
        return;
    };
    let gap = text.get(prev.end..next.start).unwrap_or("");
    let spaced = !gap.is_empty() && gap.chars().all(char::is_whitespace);
    if next.initial && spaced && !edits.iter().any(|(idx, _)| *idx == last + 1) {
        edits.push((last + 1, Part::Last));
    }
}

fn role(part: Part) -> NameRole {
    match part {
        Part::First => NameRole::First,
        Part::Last => NameRole::Last,
    }
}

impl Matcher {
    fn is_empty(&self) -> bool {
        self.persons.is_empty() && self.singles.is_empty()
    }

    /// Longest person sequence that starts at token `i`.
    fn find_window(
        &self,
        text: &str,
        toks: &[Tok],
        folded: &[String],
        from: usize,
    ) -> Option<Found> {
        let head = folded.get(from)?;
        if !self.heads.contains(head.as_str()) {
            return None;
        }
        let mut key = head.clone();
        let mut words = vec![from];
        let mut init: Option<(usize, usize)> = None; // (words before, token)
        let mut comma: Option<usize> = None; // words before the comma
        let (mut prev, mut prev_init) = (from, false);
        let mut best: Option<Found> = None;
        let mut next = from + 1;
        while next < toks.len()
            && (words.len() < self.max_words || toks.get(next).is_some_and(|t| t.initial))
        {
            let (before, after) = (toks.get(prev)?, toks.get(next)?);
            let Some(gap) = text.get(before.end..after.start) else {
                break;
            };
            if after.initial {
                if comma.is_some() {
                    // `Last, First M.`: the initial follows the first-name part.
                    if !prev_init
                        && matches!(classify(gap), Gap::Sep)
                        && let Some(b) = best.as_mut()
                        && b.comma.is_some()
                        && b.words.len() == words.len()
                    {
                        b.init = Some(next);
                    }
                    break;
                }
                let ok = !prev_init
                    && init.is_none()
                    && comma.is_none()
                    && matches!(classify(gap), Gap::Sep);
                if !ok {
                    break;
                }
                init = Some((words.len(), next));
                (prev, prev_init) = (next, true);
                next += 1;
                continue;
            }
            if prev_init {
                if !initial_gap(gap) {
                    break;
                }
            } else {
                match classify(gap) {
                    Gap::Sep => {}
                    Gap::Comma if comma.is_none() && init.is_none() => comma = Some(words.len()),
                    Gap::Comma | Gap::Bad => break,
                }
            }
            words.push(next);
            key.push(KEY_SEP);
            key.push_str(folded.get(next)?);
            (prev, prev_init) = (next, false);
            next += 1;
            let hit = match comma {
                None => self.persons.get(key.as_str()).copied().filter(|split| {
                    let head_len = match split {
                        Split::At(head_len) => *head_len,
                        Split::Full => words.len() - 1,
                    };
                    init.is_none_or(|(pos, _)| pos == head_len)
                }),
                Some(at) => self
                    .commas
                    .get(key.as_str())
                    .copied()
                    .filter(|split| match split {
                        Split::At(want) => *want == at,
                        Split::Full => true,
                    }),
            };
            if let Some(split) = hit {
                best = Some(Found {
                    words: words.clone(),
                    init: init.map(|(_, tok)| tok),
                    comma,
                    split,
                });
            }
        }
        best
    }

    /// Replace each known name in `text`. `replace(part, token)` gives the
    /// pseudonym of one real token; the result keeps the case style of the
    /// matched text. Returns the new text and the number of replaced names.
    /// Without a match the text is returned unchanged.
    pub fn replace(
        &self,
        input: &str,
        replace: &mut dyn FnMut(Part, &str) -> String,
    ) -> (String, usize) {
        if self.is_empty() {
            return (input.to_string(), 0);
        }
        let nfc: Cow<str> = if is_nfc(input) {
            Cow::Borrowed(input)
        } else {
            Cow::Owned(input.nfc().collect())
        };
        let text = nfc.as_ref();
        let toks = lex(text);
        let folded: Vec<String> = toks
            .iter()
            .map(|t| {
                if t.initial {
                    String::new()
                } else {
                    fold(text.get(t.start..t.end).unwrap_or(""))
                }
            })
            .collect();
        // (token index, part), and the number of names.
        let mut edits: Vec<(usize, Part)> = Vec::new();
        let mut count = 0;
        let mut i = 0;
        while i < toks.len() {
            if toks.get(i).is_some_and(|t| t.initial) {
                i += 1;
                continue;
            }
            if let Some(found) = self.find_window(text, &toks, &folded, i) {
                let n = found.words.len();
                for (k, &w) in found.words.iter().enumerate() {
                    let first = match (found.comma, found.split) {
                        (Some(c), _) => k >= c,
                        (None, Split::At(m)) => k < m,
                        (None, Split::Full) => k + 1 != n,
                    };
                    edits.push((w, if first { Part::First } else { Part::Last }));
                }
                if let Some(tok) = found.init {
                    edits.push((tok, Part::First));
                }
                count += 1;
                let last = found.words.last().map_or(i, |w| *w);
                trailing_initial(text, &toks, last, &mut edits);
                i = last + 1;
                continue;
            }
            // A single token matches only a word that is not all lowercase
            // (spec 0004): `Mark` and `MARK` match, `mark` does not.
            let word = toks.get(i).and_then(|t| text.get(t.start..t.end));
            let lowercase = word.is_some_and(|w| {
                w.chars()
                    .filter(|c| c.is_alphabetic())
                    .all(char::is_lowercase)
            });
            let part = folded.get(i).and_then(|f| self.singles.get(f.as_str()));
            if let (Some(part), false) = (part, lowercase) {
                edits.push((i, *part));
                count += 1;
                trailing_initial(text, &toks, i, &mut edits);
            }
            i += 1;
        }
        if count == 0 {
            return (input.to_string(), 0);
        }
        edits.sort_by_key(|(idx, _)| *idx);
        let mut out = String::with_capacity(text.len());
        let mut pos = 0;
        for (idx, part) in edits {
            let Some(t) = toks.get(idx) else { continue };
            out.push_str(text.get(pos..t.start).unwrap_or(""));
            let token = text.get(t.start..t.end).unwrap_or("");
            // A token with a column spelling is passed as it is; any other
            // spelling gets the smallest column spelling. The case style
            // comes from the text.
            let lower = token.to_lowercase();
            let real = folded
                .get(idx)
                .and_then(|f| self.canon.get(f))
                .filter(|set| !set.contains(&lower))
                .and_then(|set| set.first())
                .map_or(token, String::as_str);
            out.push_str(&replace_name(token, role(part), &mut |p, _| {
                replace(p, real)
            }));
            pos = t.end;
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
        let m = names.matcher(mode);
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
        assert_eq!(run(&k, FreeText::Tokens, "(Anna), ok").0, "(F4), ok");
    }

    #[test]
    fn apostrophes() {
        let mut k = KnownNames::default();
        k.add_person("Zorbina", "Quenzel");
        k.add_person("Tamsorin", "O'Brien");
        assert_eq!(
            run(&k, FreeText::Full, "Hi 'Zorbina Quenzel'!"),
            ("Hi 'F7 L7'!".to_string(), 1)
        );
        assert_eq!(
            run(&k, FreeText::Tokens, "Zorbina's and Zorbina\u{2019}s"),
            ("F7's and F7\u{2019}s".to_string(), 2)
        );
        assert_eq!(
            run(&k, FreeText::Tokens, "Mr O'Brien, O'Brien's"),
            ("Mr L7, L7's".to_string(), 2)
        );
        assert_eq!(run(&k, FreeText::Tokens, "O'Brienx").1, 0);
    }

    #[test]
    fn case_style_kept() {
        let k = sample();
        let mut seen = Vec::new();
        let m = k.matcher(FreeText::Tokens);
        let (out, n) = m.replace("ANNA MUSTER and Anna", &mut |p, t| {
            seen.push((p, t.to_string()));
            match p {
                Part::First => "Berta".to_string(),
                Part::Last => "Kunz".to_string(),
            }
        });
        assert_eq!(out, "BERTA KUNZ and Berta");
        assert_eq!(n, 2);
        // The callback gets the token itself (a column spelling).
        assert_eq!(seen[0], (Part::First, "ANNA".to_string()));
    }

    #[test]
    fn folded_match_uses_the_column_spelling() {
        let mut k = KnownNames::default();
        k.add_person("Ilse", "Strauß");
        let m = k.matcher(FreeText::Tokens);
        let mut seen = Vec::new();
        let (out, n) = m.replace("STRAUSS", &mut |p, t| {
            seen.push((p, t.to_string()));
            "Kunz".to_string()
        });
        assert_eq!((out.as_str(), n), ("KUNZ", 1));
        assert_eq!(seen, [(Part::Last, "strauß".to_string())]);
    }

    #[test]
    fn longest_match_wins() {
        let k = sample();
        let mut seen = Vec::new();
        let m = k.matcher(FreeText::Tokens);
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
        let m = k.matcher(FreeText::Full);
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
        assert_eq!(run(&k, FreeText::Off, "Anna Muster").1, 0);
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
    fn add_full_ignores_angle_brackets() {
        let mut k = KnownNames::default();
        k.add_full("Ottilie Wenzhart <ottilie.wenzhart@firma.test>");
        assert_eq!(run(&k, FreeText::Full, "Ottilie Wenzhart").1, 1);
        assert_eq!(run(&k, FreeText::Tokens, "firma test").1, 0);
        assert_eq!(run(&k, FreeText::Tokens, "Ottilie").1, 1);
        // An all-lowercase single word does not match.
        assert_eq!(run(&k, FreeText::Tokens, "ottilie").1, 0);
    }

    #[test]
    fn hyphen_and_initial() {
        let mut k = KnownNames::default();
        k.add_person("Lisandra-Mae", "Kroft");
        k.add_full("Berta C. Kunzl");
        assert_eq!(
            run(&k, FreeText::Full, "Hi Lisandra-Mae Kroft"),
            ("Hi F8-F3 L5".to_string(), 1)
        );
        assert_eq!(run(&k, FreeText::Full, "Lisandra Mae  Kroft").1, 1);
        assert_eq!(
            run(&k, FreeText::Full, "Berta C. Kunzl"),
            ("F5 F. L5".to_string(), 1)
        );
        assert_eq!(run(&k, FreeText::Full, "Berta C Kunzl").1, 1);
        assert_eq!(run(&k, FreeText::Full, "Berta Kunzl").1, 1);
        assert_eq!(run(&k, FreeText::Full, "Berta C. D. Kunzl").1, 0);
    }

    #[test]
    fn case_folding() {
        let mut k = KnownNames::default();
        k.add_person("İlkay", "Strauß");
        for text in ["ilkay Strauss", "İLKAY STRAUSS", "I\u{307}lkay STRAUSS"] {
            assert_eq!(run(&k, FreeText::Full, text).1, 1, "{text}");
        }
        assert_eq!(run(&k, FreeText::Tokens, "STRAUSS").1, 1);
        let mut k2 = KnownNames::default();
        k2.add_person("ilkay", "STRAUSS");
        assert_eq!(run(&k2, FreeText::Full, "İlkay Strauß").1, 1);
    }

    #[test]
    fn multi_token_last_name_split() {
        let mut k = KnownNames::default();
        k.add_person("Jorvik", "van der Hallen");
        assert_eq!(
            run(&k, FreeText::Full, "Jorvik van der Hallen"),
            ("F6 l3 l3 L6".to_string(), 1)
        );
        assert_eq!(
            run(&k, FreeText::Full, "van der Hallen, Jorvik"),
            ("l3 l3 L6, F6".to_string(), 1)
        );
        // Particles do not match alone; the capitalized part does.
        assert_eq!(
            run(&k, FreeText::Tokens, "van der Hallen said"),
            ("van der L6 said".to_string(), 1)
        );
    }

    #[test]
    fn conflicting_splits_use_full_rule() {
        let mut k = KnownNames::default();
        k.add_person("Anna Maria", "Muster");
        k.add_person("Anna", "Maria Muster");
        assert_eq!(
            run(&k, FreeText::Full, "Anna Maria Muster"),
            ("F4 F5 L6".to_string(), 1)
        );
    }

    #[test]
    fn untouched_text_is_returned_as_is() {
        let k = sample();
        let nfd = "Jo\u{308}rg said";
        assert_eq!(run(&k, FreeText::Tokens, nfd), (nfd.to_string(), 0));
    }

    #[test]
    fn token_role_by_frequency() {
        let mut k = KnownNames::default();
        k.add_person("Renvik", "Dalmora");
        k.add_person("Alva", "Renvik");
        k.add_person("Alva", "Renvik");
        // Renvik: once first, twice last.
        assert_eq!(run(&k, FreeText::Tokens, "Renvik").0, "L6");
        // Alva: twice first; Dalmora: once last.
        assert_eq!(run(&k, FreeText::Tokens, "Alva").0, "F4");
        // Tie: First.
        let mut t = KnownNames::default();
        t.add_person("Renvik", "Dalmora");
        t.add_person("Alva", "Renvik");
        assert_eq!(run(&t, FreeText::Tokens, "Renvik").0, "F6");
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
    fn many_names_are_fast() {
        let code = |i: usize| -> String {
            i.to_string()
                .chars()
                .map(|d| (b'a' + (d as u8 - b'0')) as char)
                .collect()
        };
        let mut k = KnownNames::default();
        for i in 0..5000 {
            k.add_person(
                &format!("Vorname{}", code(i)),
                &format!("Nachname{}", code(i)),
            );
        }
        k.add_person("Anna", "Muster");
        let texts: Vec<String> = (0..10_000)
            .map(|i| {
                format!(
                    "Please call Vorname{} Nachname{} or ask Anna Muster today",
                    code(i % 6000),
                    code(i % 5000)
                )
            })
            .collect();
        let start = std::time::Instant::now();
        let m = k.matcher(FreeText::Tokens);
        let mut total = 0;
        for text in &texts {
            total += m.replace(text, &mut fake).1;
        }
        assert!(total >= 10_000);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(
            run(&k, FreeText::Off, "Call Anna Muster"),
            ("Call Anna Muster".to_string(), 0)
        );
    }

    fn seen(k: &KnownNames, mode: FreeText, text: &str) -> Vec<String> {
        let mut seen = Vec::new();
        k.matcher(mode).replace(text, &mut |_, t| {
            seen.push(t.to_string());
            "X".to_string()
        });
        seen
    }

    #[test]
    fn lowercase_last_name_column() {
        let mut k = KnownNames::default();
        k.add_person("zorbina", "quenzel");
        k.add_person("Jorvik", "de Velgrad");
        for t in ["Quenzel", "QUENZEL"] {
            assert_eq!(run(&k, FreeText::Tokens, t).1, 1, "{t}");
        }
        // An all-lowercase single word does not match.
        assert_eq!(run(&k, FreeText::Tokens, "quenzel").1, 0);
        assert_eq!(run(&k, FreeText::Tokens, "zorbina quenzel").1, 1);
        assert_eq!(run(&k, FreeText::Tokens, "de said").1, 0);
        assert_eq!(run(&k, FreeText::Tokens, "Velgrad").1, 1);
    }

    #[test]
    fn curly_apostrophe_is_one_key() {
        let mut k = KnownNames::default();
        k.add_person("Tamsorin", "O\u{2019}Brien");
        for t in [
            "O'Brien",
            "O\u{2019}Brien",
            "O\u{2bc}Brien",
            "O\u{2019}Brien\u{2019}s",
        ] {
            assert_eq!(run(&k, FreeText::Tokens, t).1, 1, "{t}");
        }
        assert_eq!(run(&k, FreeText::Tokens, "O\u{2019}Brienx").1, 0);
        // The column spelling is passed as it is.
        assert_eq!(
            seen(&k, FreeText::Tokens, "O\u{2019}Brien"),
            ["O\u{2019}Brien"]
        );
        // An unseen spelling gets the column spelling.
        assert_eq!(seen(&k, FreeText::Tokens, "O'Brien"), ["o\u{2019}brien"]);
        let mut k2 = KnownNames::default();
        k2.add_person("A", "O'Brien");
        k2.add_person("B", "O\u{2019}Brien");
        assert_eq!(seen(&k2, FreeText::Tokens, "O'Brien"), ["O'Brien"]);
        assert_eq!(seen(&k2, FreeText::Tokens, "O\u{2bc}BRIEN"), ["o'brien"]);
    }

    #[test]
    fn several_spellings_do_not_depend_on_order() {
        let mut a = KnownNames::default();
        a.add_person("Ilse", "Strauss");
        a.add_person("Karl", "Strauß");
        let mut b = KnownNames::default();
        b.add_person("Karl", "Strauß");
        b.add_person("Ilse", "Strauss");
        for k in [&a, &b] {
            let got = seen(k, FreeText::Tokens, "Strauss STRAUSS Strauß STRAUß");
            assert_eq!(got, ["Strauss", "STRAUSS", "Strauß", "STRAUß"]);
        }
    }

    #[test]
    fn full_name_with_trailing_initial() {
        let mut k = KnownNames::default();
        k.add_full("Kesta Wolmar J.");
        assert_eq!(run(&k, FreeText::Full, "Kesta Wolmar").1, 0);
        assert_eq!(run(&k, FreeText::Tokens, "Kesta").1, 1);
        // The tokens before the final initial are first names.
        assert_eq!(run(&k, FreeText::Tokens, "Wolmar").1, 1);
        assert_eq!(run(&k, FreeText::Tokens, "J. said").1, 0);
    }

    #[test]
    fn comma_form_with_trailing_initial() {
        let mut k = KnownNames::default();
        k.add_full("Draven, Ilsa M.");
        assert_eq!(
            run(&k, FreeText::Full, "Draven, Ilsa M. said"),
            ("L6, F4 F. said".to_string(), 1)
        );
        assert_eq!(
            run(&k, FreeText::Full, "Draven, Ilsa said").0,
            "L6, F4 said"
        );
    }

    #[test]
    fn dashes_separate_inside_a_name() {
        let mut k = KnownNames::default();
        k.add_person("Lisandra\u{2013}Mae", "Kroft");
        assert_eq!(run(&k, FreeText::Full, "Lisandra\u{2013}Mae Kroft").1, 1);
        assert_eq!(run(&k, FreeText::Full, "Lisandra\u{2014}Mae Kroft").1, 1);
    }
}
