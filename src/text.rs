//! Name tokens, case styles, email search, and transliteration.

use regex::Regex;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Role of a name column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRole {
    /// `first_name` column: each token is a first name.
    First,
    /// `last_name` column: each token is a last name.
    Last,
    /// `full_name` column: "First Middle Last" or "Last, First".
    Full,
}

/// Part of a name that one token has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    First,
    Last,
}

/// A name token: byte range in the value and its part.
struct Token {
    start: usize,
    end: usize,
    part: Part,
    /// Title, suffix, or particle: stays unchanged.
    keep: bool,
}

/// `true` for a Unicode format character (category Cf), for example the soft
/// hyphen U+00AD or the zero-width space U+200B. These characters are
/// invisible. They must neither split a name nor make two spellings of one
/// name differ (ADR-0013).
///
/// The `unicode-normalization` crate has no category data, so the ranges of
/// category Cf (Unicode 15) are listed here.
pub(crate) fn is_format_char(c: char) -> bool {
    matches!(
        u32::from(c),
        0x00AD // soft hyphen
            | 0x0600..=0x0605 // Arabic number signs
            | 0x061C // Arabic letter mark
            | 0x06DD // Arabic end of ayah
            | 0x070F // Syriac abbreviation mark
            | 0x0890..=0x0891 // Arabic pound and piastre marks
            | 0x08E2 // Arabic disputed end of ayah
            | 0x180E // Mongolian vowel separator
            | 0x200B..=0x200F // zero-width space, joiners, direction marks
            | 0x202A..=0x202E // bidi embedding and override
            | 0x2060..=0x2064 // word joiner, invisible operators
            | 0x2066..=0x206F // bidi isolates, deprecated format characters
            | 0xFEFF // zero-width no-break space (BOM)
            | 0xFFF9..=0xFFFB // interlinear annotation
            | 0x110BD | 0x110CD // Kaithi number signs
            | 0x13430..=0x1343F // Egyptian hieroglyph format controls
            | 0x1BCA0..=0x1BCA3 // shorthand format controls
            | 0x1D173..=0x1D17A // musical format controls
            | 0xE0001 // language tag
            | 0xE0020..=0xE007F // tag characters
    )
}

/// `'`, the curly apostrophe U+2019 and the modifier letter U+02BC.
pub(crate) fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '\u{2bc}')
}

/// The one normalization of name columns, free text, and email addresses
/// (ADR-0013): remove format characters, NFC, lowercase, U+2019 and U+02BC to
/// `'`, `ß` to `ss`, no U+0307 after `i` (so `İ` equals `i`), trimmed.
pub(crate) fn normalize(text: &str) -> String {
    let visible: String = text.chars().filter(|c| !is_format_char(*c)).collect();
    let mut out = String::with_capacity(visible.len());
    for c in visible.nfc() {
        for l in c.to_lowercase() {
            match l {
                'ß' => out.push_str("ss"),
                '\u{2019}' | '\u{2bc}' => out.push('\''),
                '\u{307}' if out.ends_with('i') => {}
                _ => out.push(l),
            }
        }
    }
    out.trim().to_string()
}

fn is_token_char(c: char) -> bool {
    c.is_alphabetic() || is_apostrophe(c) || is_combining_mark(c) || is_format_char(c)
}

/// Titles and salutations: not names, never replaced. Case-insensitive; the
/// dot is no token character, so `Dr.` and `Dr` are the same. `DI` counts only
/// in capitals, because `di` is a particle and `Di` can be a name.
fn is_title(token: &str) -> bool {
    const TITLES: [&str; 13] = [
        "dr", "prof", "mag", "mmag", "ing", "dipl", "univ", "herr", "frau", "mr", "mrs", "ms", "mx",
    ];
    token == "DI" || TITLES.contains(&token.to_lowercase().as_str())
}

/// Suffixes: not names, never replaced. Roman numerals only in capitals.
fn is_suffix(token: &str) -> bool {
    const SUFFIXES: [&str; 6] = ["jr", "sr", "msc", "bsc", "mba", "phd"];
    matches!(token, "II" | "III" | "IV") || SUFFIXES.contains(&token.to_lowercase().as_str())
}

/// Lowercase particles belong to the last name (`van Beethoven`). They stay
/// unchanged in the output; only the core last name is replaced. A capitalized
/// word (`Van`, `De`) is a name and is replaced.
pub(crate) fn is_particle(token: &str) -> bool {
    const PARTICLES: [&str; 13] = [
        "von", "van", "de", "der", "den", "du", "la", "le", "zu", "di", "da", "ten", "ter",
    ];
    PARTICLES.contains(&token)
}

/// What a token of a `full_name` value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// Title or suffix: not a name, unchanged.
    Skip,
    First,
    Last,
    /// Lowercase particle of the last name: unchanged.
    Particle,
}

/// Slots of the name tokens (byte ranges in `value`) of a `full_name` value.
///
/// - Titles and suffixes are `Skip`.
/// - Exactly one comma between name tokens (a comma with only titles or
///   suffixes on one side does not count) means `Last, First`: the tokens
///   before it are the last name, those after it the first name.
/// - Otherwise the final token is the last name, and the lowercase particles
///   right before it belong to it. All other tokens are first names.
/// - A single word is a last name.
pub(crate) fn full_slots(value: &str, ranges: &[(usize, usize)]) -> Vec<Slot> {
    let raw = |i: usize| {
        ranges
            .get(i)
            .and_then(|r| value.get(r.0..r.1))
            .unwrap_or("")
    };
    let eligible: Vec<usize> = (0..ranges.len())
        .filter(|i| !is_title(raw(*i)) && !is_suffix(raw(*i)))
        .collect();
    let first_end = eligible.first().and_then(|i| ranges.get(*i)).map(|r| r.1);
    let last_start = eligible.last().and_then(|i| ranges.get(*i)).map(|r| r.0);
    let mut commas = value.match_indices(',').map(|(pos, _)| pos).filter(|pos| {
        first_end.is_some_and(|e| e <= *pos) && last_start.is_some_and(|s| s >= *pos)
    });
    let comma = match (commas.next(), commas.next()) {
        (Some(pos), None) => Some(pos),
        _ => None,
    };
    let mut slots = vec![Slot::Skip; ranges.len()];
    let mut set = |i: usize, slot: Slot| {
        if let Some(s) = slots.get_mut(i) {
            *s = slot;
        }
    };
    if let Some(pos) = comma {
        let (before, after): (Vec<usize>, Vec<usize>) = eligible
            .iter()
            .partition(|i| ranges.get(**i).is_some_and(|r| r.0 < pos));
        for (k, i) in before.iter().enumerate() {
            let particle = k + 1 < before.len() && is_particle(raw(*i));
            set(*i, if particle { Slot::Particle } else { Slot::Last });
        }
        for i in after {
            set(i, Slot::First);
        }
    } else if let Some((core, rest)) = eligible.split_last() {
        set(*core, Slot::Last);
        let mut chain = true;
        for i in rest.iter().rev() {
            chain = chain && is_particle(raw(*i));
            set(*i, if chain { Slot::Particle } else { Slot::First });
        }
    }
    slots
}

/// Replace each name token in `value`.
///
/// `replace(part, token)` gives the pseudonym of one token. The result gets the
/// case style of the token (`Anna`, `ANNA`, `anna`). All separators stay. In a
/// `full_name` value, titles, suffixes, and lowercase particles stay too.
pub fn replace_name(
    value: &str,
    role: NameRole,
    replace: &mut dyn FnMut(Part, &str) -> String,
) -> String {
    let normalized: String = value.nfc().collect();
    let value = normalized.as_str();
    let mut tokens = split_tokens(value);
    assign_parts(value, role, &mut tokens);
    let mut out = String::with_capacity(value.len());
    let mut pos = 0;
    for token in &tokens {
        out.push_str(value.get(pos..token.start).unwrap_or(""));
        let text = value.get(token.start..token.end).unwrap_or("");
        if token.keep {
            out.push_str(text);
        } else {
            let pseudonym = replace(token.part, text);
            out.push_str(&apply_case(text, &pseudonym));
        }
        pos = token.end;
    }
    out.push_str(value.get(pos..).unwrap_or(""));
    out
}

/// Maximal runs of alphabetic chars and apostrophes, without apostrophes at
/// either end (`‘Bob’` gives `Bob`). Runs without a letter are separators.
fn split_tokens(value: &str) -> Vec<Token> {
    fn push(value: &str, start: usize, end: usize, tokens: &mut Vec<Token>) {
        let run = value.get(start..end).unwrap_or("");
        let start = start + (run.len() - run.trim_start_matches(is_apostrophe).len());
        let end = start
            + value
                .get(start..end)
                .unwrap_or("")
                .trim_end_matches(is_apostrophe)
                .len();
        if value
            .get(start..end)
            .is_some_and(|t| t.chars().any(char::is_alphabetic))
        {
            tokens.push(Token {
                start,
                end,
                part: Part::Last,
                keep: false,
            });
        }
    }

    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in value.char_indices() {
        if is_token_char(c) {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            push(value, s, i, &mut tokens);
        }
    }
    if let Some(s) = start {
        push(value, s, value.len(), &mut tokens);
    }
    tokens
}

fn assign_parts(value: &str, role: NameRole, tokens: &mut [Token]) {
    match role {
        NameRole::First => {
            for t in tokens.iter_mut() {
                t.part = Part::First;
            }
        }
        NameRole::Last => {
            // A lowercase particle before another token stays (`van Berg`).
            let n = tokens.len();
            for (i, t) in tokens.iter_mut().enumerate() {
                t.part = Part::Last;
                t.keep = i + 1 < n && is_particle(value.get(t.start..t.end).unwrap_or(""));
            }
        }
        NameRole::Full => {
            let ranges: Vec<(usize, usize)> = tokens.iter().map(|t| (t.start, t.end)).collect();
            for (t, slot) in tokens.iter_mut().zip(full_slots(value, &ranges)) {
                t.part = if slot == Slot::First {
                    Part::First
                } else {
                    Part::Last
                };
                t.keep = matches!(slot, Slot::Skip | Slot::Particle);
            }
        }
    }
}

/// Give `pseudonym` the case style of `token`.
fn apply_case(token: &str, pseudonym: &str) -> String {
    let letters: Vec<char> = token.chars().filter(|c| c.is_alphabetic()).collect();
    let upper = letters.iter().all(|c| c.is_uppercase());
    let lower = letters.iter().all(|c| c.is_lowercase());
    if letters.len() == 1 {
        let initial: String = pseudonym.chars().take(1).collect();
        return if upper {
            initial.to_uppercase()
        } else if lower {
            initial.to_lowercase()
        } else {
            initial
        };
    }
    if upper {
        pseudonym.to_uppercase()
    } else if lower {
        pseudonym.to_lowercase()
    } else {
        pseudonym.to_string()
    }
}

/// Characters of an unquoted local part: RFC 5322 `atext` without
/// `! / ? & =` (text or URL punctuation around an address), plus Unicode
/// letters, digits, marks, and format characters. A dot is allowed only
/// between two such words.
const LOCAL_CHARS: &str = r"[\p{L}\p{N}\p{M}\p{Cf}_%+*#$^`{|}~'-]";

/// Regex that finds email addresses in a cell.
///
/// Local part: words of `LOCAL_CHARS` joined by single dots (the first
/// character is a letter, digit, or `_`), or a quoted string. Domain: labels
/// of letters, digits, and `-`, joined by dots. One label is enough
/// (`localhost`). `email_spans` checks the rest.
pub fn email_regex() -> anyhow::Result<Regex> {
    let label = r"[\p{L}\p{N}](?:[\p{L}\p{N}\p{M}\p{Cf}-]*[\p{L}\p{N}\p{M}])?";
    let pattern = [
        r"(?:[\p{L}\p{N}_]",
        LOCAL_CHARS,
        r"*(?:\.",
        LOCAL_CHARS,
        r#"+)*|"(?:[^"\\\r\n]|\\[^\r\n])+")@"#,
        label,
        r"(?:\.",
        label,
        ")*",
    ]
    .concat();
    Ok(Regex::new(&pattern)?)
}

/// `true` if the address at `start` is inside a URL and not a `user@host`
/// after `?`, `&`, `=`, or `mailto:` (then it is the userinfo or the path).
fn is_url_part(value: &str, start: usize) -> bool {
    let before = value.get(..start).unwrap_or("");
    let word = before
        .rfind(char::is_whitespace)
        .map_or(before, |i| before.get(i..).unwrap_or(""));
    let lower = word.trim_start().to_ascii_lowercase();
    let in_url = lower.contains("://") || lower.starts_with("www.");
    in_url && !(lower.ends_with(['?', '&', '=']) || lower.ends_with("mailto:"))
}

/// Byte ranges of the email addresses in `value`.
///
/// Punctuation around an address is not part of it. A match is skipped if
/// the last label of its domain has no letter (`3@4.50`), or if it is the
/// userinfo or the path of a URL.
pub fn email_spans(value: &str, regex: &Regex) -> Vec<std::ops::Range<usize>> {
    regex
        .find_iter(value)
        .filter(|m| {
            let domain = m.as_str().rsplit_once('@').map_or("", |(_, d)| d);
            let tld = domain.rsplit('.').next().unwrap_or("");
            tld.chars().any(char::is_alphabetic) && !is_url_part(value, m.start())
        })
        .map(|m| m.range())
        .collect()
}

/// Replace each email address in `value` with `replace(address)`.
pub fn replace_emails(
    value: &str,
    regex: &Regex,
    replace: &mut dyn FnMut(&str) -> String,
) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pos = 0;
    for span in email_spans(value, regex) {
        out.push_str(value.get(pos..span.start).unwrap_or(""));
        out.push_str(&replace(value.get(span.clone()).unwrap_or("")));
        pos = span.end;
    }
    out.push_str(value.get(pos..).unwrap_or(""));
    out
}

/// Transliterate `ä ö ü ß` (also uppercase) and lowercase, then keep `[a-z0-9-]`.
pub(crate) fn ascii_local_part(name: &str) -> String {
    let mut out = String::new();
    for c in name.nfc() {
        match c {
            'ä' | 'Ä' => out.push_str("ae"),
            'ö' | 'Ö' => out.push_str("oe"),
            'ü' | 'Ü' => out.push_str("ue"),
            'ß' | 'ẞ' => out.push_str("ss"),
            _ => {
                for d in std::iter::once(c).nfd().filter(|d| !is_combining_mark(*d)) {
                    let l = d.to_ascii_lowercase();
                    if l.is_ascii_lowercase() || l.is_ascii_digit() || l == '-' {
                        out.push(l);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(value: &str, role: NameRole) -> String {
        replace_name(value, role, &mut |part, _| match part {
            Part::First => "Berta".to_string(),
            Part::Last => "Muster".to_string(),
        })
    }

    #[test]
    fn tokens_split_on_separators() {
        assert_eq!(run("Anna-Lena Maria", NameRole::First), "Berta-Berta Berta");
        assert_eq!(run("Anna2, x", NameRole::First), "Berta2, b");
        assert_eq!(run("O'Neil", NameRole::Last), "Muster");
        assert_eq!(run(" 12 - ", NameRole::Full), " 12 - ");
    }

    #[test]
    fn quotes_around_a_name_stay() {
        assert_eq!(
            run("\u{2018}Bob\u{2019}", NameRole::First),
            "\u{2018}Berta\u{2019}"
        );
        assert_eq!(run("'Bob'", NameRole::First), "'Berta'");
        assert_eq!(run("Kunz\u{2019}", NameRole::Last), "Muster\u{2019}");
        assert_eq!(run("O\u{2019}Brien", NameRole::Last), "Muster");
        let mut seen = Vec::new();
        replace_name(
            "Robert \u{2018}Bob\u{2019} Smith",
            NameRole::Full,
            &mut |_, t| {
                seen.push(t.to_string());
                String::from("X")
            },
        );
        assert_eq!(seen, ["Robert", "Bob", "Smith"]);
    }

    #[test]
    fn full_name_roles() {
        assert_eq!(
            run("Hans Peter Meier", NameRole::Full),
            "Berta Berta Muster"
        );
        assert_eq!(run("Meier", NameRole::Full), "Muster");
        assert_eq!(
            run("Meier Schulz, Hans Peter", NameRole::Full),
            "Muster Muster, Berta Berta"
        );
        assert_eq!(run("Meier,Hans", NameRole::Full), "Muster,Berta");
        assert_eq!(run("Meier", NameRole::Last), "Muster");
    }

    #[test]
    fn case_styles() {
        assert_eq!(run("MEIER, hans", NameRole::Full), "MUSTER, berta");
        assert_eq!(run("Hans MEIER", NameRole::Full), "Berta MUSTER");
        assert_eq!(run("hans meier", NameRole::Full), "berta muster");
        assert_eq!(run("HaNs", NameRole::First), "Berta");
    }

    #[test]
    fn initials() {
        assert_eq!(run("J. Meier", NameRole::Full), "B. Muster");
        assert_eq!(run("j. meier", NameRole::Full), "b. muster");
        assert_eq!(run("Meier, J", NameRole::Full), "Muster, B");
    }

    #[test]
    fn transliteration() {
        assert_eq!(ascii_local_part("Jürgen"), "juergen");
        assert_eq!(ascii_local_part("Groß"), "gross");
        assert_eq!(ascii_local_part("ÄÖÜ Ä"), "aeoeueae");
        assert_eq!(ascii_local_part("Anne-Marie O'Neil 2"), "anne-marieoneil2");
    }

    #[test]
    fn nfd_and_nfc_names_are_equal() {
        let nfd = "Ju\u{308}rgen-Lena";
        let nfc = "J\u{fc}rgen-Lena";
        let mut seen = Vec::new();
        let mut f = |_: Part, t: &str| {
            seen.push(t.to_string());
            "Berta".to_string()
        };
        let a = replace_name(nfd, NameRole::First, &mut f);
        let b = replace_name(nfc, NameRole::First, &mut f);
        assert_eq!(a, "Berta-Berta");
        assert_eq!(a, b);
        assert_eq!(seen, ["J\u{fc}rgen", "Lena", "J\u{fc}rgen", "Lena"]);
    }

    #[test]
    fn unicode_emails_match_whole() -> anyhow::Result<()> {
        let re = email_regex()?;
        let mut seen = Vec::new();
        let mut rep = |m: &str| {
            seen.push(m.to_string());
            "x@y.z".to_string()
        };
        assert_eq!(
            replace_emails("anna.m\u{fc}ller@acme.test", &re, &mut rep),
            "x@y.z"
        );
        assert_eq!(
            replace_emails("info@b\u{e4}ckerei-zorbel.at", &re, &mut rep),
            "x@y.z"
        );
        assert_eq!(
            replace_emails("Zorbel <zorbel@acme.test>", &re, &mut rep),
            "Zorbel <x@y.z>"
        );
        assert_eq!(
            replace_emails("mail a@b.test.", &re, &mut rep),
            "mail x@y.z."
        );
        assert_eq!(
            seen,
            [
                "anna.m\u{fc}ller@acme.test",
                "info@b\u{e4}ckerei-zorbel.at",
                "zorbel@acme.test",
                "a@b.test"
            ]
        );
        Ok(())
    }

    #[test]
    fn accents_are_dropped_in_local_part() {
        assert_eq!(ascii_local_part("D\u{e9}sir\u{e9}e"), "desiree");
        assert_eq!(ascii_local_part("Se\u{301}bastien"), "sebastien");
    }

    #[test]
    fn emails_are_replaced() -> anyhow::Result<()> {
        let re = email_regex()?;
        let mut seen = Vec::new();
        let out = replace_emails("Mail a@b.org, c.d+x@e.co.uk. Bye", &re, &mut |m| {
            seen.push(m.to_string());
            "x@y.z".to_string()
        });
        assert_eq!(out, "Mail x@y.z, x@y.z. Bye");
        assert_eq!(seen, ["a@b.org", "c.d+x@e.co.uk"]);
        assert_eq!(
            replace_emails("no mail @ here", &re, &mut |_| "!".into()),
            "no mail @ here"
        );
        Ok(())
    }

    #[test]
    fn apostrophe_in_local_part() -> anyhow::Result<()> {
        let re = email_regex()?;
        let mut rep = |_: &str| "x@y.test".to_string();
        assert_eq!(
            replace_emails("mary.o'neil@acme.test", &re, &mut rep),
            "x@y.test"
        );
        assert_eq!(
            replace_emails("'mary@acme.test'.", &re, &mut rep),
            "'x@y.test'."
        );
        let value = "see 'a@b.test'.";
        let spans = email_spans(value, &re);
        assert_eq!(spans.len(), 1);
        assert_eq!(value.get(spans[0].clone()), Some("a@b.test"));
        Ok(())
    }

    fn tagged(value: &str, role: NameRole) -> String {
        replace_name(value, role, &mut |part, token| match part {
            Part::First => format!("F[{token}]"),
            Part::Last => format!("L[{token}]"),
        })
    }

    #[test]
    fn titles_and_suffixes_stay() {
        assert_eq!(run("Dr. Hans Zorbel", NameRole::Full), "Dr. Berta Muster");
        assert_eq!(
            run("Prof. Dr. Hans Zorbel", NameRole::Full),
            "Prof. Dr. Berta Muster"
        );
        assert_eq!(run("DR HANS ZORBEL", NameRole::Full), "DR BERTA MUSTER");
        assert_eq!(run("Herr Zorbel", NameRole::Full), "Herr Muster");
        assert_eq!(
            run("Frau Mag. Anna Zorbel", NameRole::Full),
            "Frau Mag. Berta Muster"
        );
        assert_eq!(run("Mr Hans Zorbel", NameRole::Full), "Mr Berta Muster");
        assert_eq!(run("Mrs. Zorbel", NameRole::Full), "Mrs. Muster");
        assert_eq!(run("Ms. Anna Zorbel", NameRole::Full), "Ms. Berta Muster");
        assert_eq!(
            run("Dipl.-Ing. DI Hans Zorbel", NameRole::Full),
            "Dipl.-Ing. DI Berta Muster"
        );
        assert_eq!(run("Hans Zorbel Jr.", NameRole::Full), "Berta Muster Jr.");
        assert_eq!(run("Hans Zorbel III", NameRole::Full), "Berta Muster III");
        assert_eq!(run("Hans Zorbel, MBA", NameRole::Full), "Berta Muster, MBA");
        assert_eq!(
            run("Hans Zorbel MSc PhD", NameRole::Full),
            "Berta Muster MSc PhD"
        );
        assert_eq!(
            run("Zorbel, Hans, Jr.", NameRole::Full),
            "Muster, Berta, Jr."
        );
        assert_eq!(run("Dr.", NameRole::Full), "Dr.");
        // `Di` is a name, `di` a particle.
        assert_eq!(run("Di Zorbel", NameRole::Full), "Berta Muster");
    }

    #[test]
    fn particles_belong_to_the_last_name() {
        assert_eq!(
            tagged("Ludwig van Beethoven", NameRole::Full),
            "F[Ludwig] van L[Beethoven]"
        );
        assert_eq!(
            tagged("Hans de la Cruz", NameRole::Full),
            "F[Hans] de la L[Cruz]"
        );
        assert_eq!(
            tagged("van der Berg, Hans", NameRole::Full),
            "van der L[Berg], F[Hans]"
        );
        // Capitalized: a name. A final particle is the last name.
        assert_eq!(
            tagged("Hans Van Berg", NameRole::Full),
            "F[Hans] F[Van] L[Berg]"
        );
        assert_eq!(tagged("Hans de", NameRole::Full), "F[Hans] l[de]");
        assert_eq!(tagged("von Berg", NameRole::Last), "von L[Berg]");
        assert_eq!(tagged("von Berg", NameRole::First), "f[von] F[Berg]");
        assert_eq!(
            tagged("Dr. Hans von Zorbel", NameRole::Full),
            "Dr. F[Hans] von L[Zorbel]"
        );
    }

    #[test]
    fn comma_forms() {
        assert_eq!(tagged("Zorbel, Hans", NameRole::Full), "L[Zorbel], F[Hans]");
        assert_eq!(
            tagged("Zorbel, Dr. Hans Peter", NameRole::Full),
            "L[Zorbel], Dr. F[Hans] F[Peter]"
        );
        // Two commas between names: not `Last, First`.
        assert_eq!(
            tagged("Zorbel, Hans, Peter", NameRole::Full),
            "F[Zorbel], F[Hans], L[Peter]"
        );
        assert_eq!(tagged("Hans Zorbel", NameRole::Full), "F[Hans] L[Zorbel]");
        assert_eq!(tagged("Zorbel", NameRole::Full), "L[Zorbel]");
    }

    #[test]
    fn format_characters_stay_in_one_token() {
        assert_eq!(
            tagged("M\u{fc}\u{ad}ller", NameRole::Last),
            "L[M\u{fc}\u{ad}ller]"
        );
        assert_eq!(
            tagged("Kes\u{200b}ta Wol\u{200d}mar", NameRole::Full),
            "F[Kes\u{200b}ta] L[Wol\u{200d}mar]"
        );
        assert!(is_format_char('\u{ad}'));
        assert!(is_format_char('\u{200b}'));
        assert!(is_format_char('\u{feff}'));
        assert!(!is_format_char('a'));
        assert!(!is_format_char('\u{2019}'));
    }

    #[test]
    fn normalize_merges_spellings() {
        for (a, b) in [
            ("O\u{2019}Brien", "O'Brien"),
            ("O\u{2bc}BRIEN", "o'brien"),
            ("Strau\u{df}", "STRAUSS"),
            ("Stra\u{1e9e}e", "strasse"),
            ("\u{130}lker", "Ilker"),
            ("I\u{307}lker", "ilker"),
            ("Mu\u{308}ller", "M\u{fc}ller"),
            ("M\u{fc}\u{ad}ller", "M\u{fc}ller"),
            ("\u{feff} Kes\u{200b}ta ", "kesta"),
        ] {
            assert_eq!(normalize(a), normalize(b), "{a:?} {b:?}");
        }
        assert_ne!(normalize("M\u{fc}ller"), normalize("Muller"));
        assert_eq!(normalize("Karl"), "karl");
    }

    fn found(value: &str) -> anyhow::Result<Vec<String>> {
        let re = email_regex()?;
        Ok(email_spans(value, &re)
            .into_iter()
            .filter_map(|r| value.get(r).map(str::to_string))
            .collect())
    }

    #[test]
    fn punctuation_around_an_address_is_not_part_of_it() -> anyhow::Result<()> {
        for value in [
            "Schreib an anna.zorbel@firma.test!",
            "anna.zorbel@firma.test?",
            "\u{bb}anna.zorbel@firma.test\u{ab}",
            "/anna.zorbel@firma.test/",
            "anna.zorbel@firma.test\u{2026}",
            "anna.zorbel@firma.test\u{3002}",
            "!anna.zorbel@firma.test",
            "x/anna.zorbel@firma.test",
            "(anna.zorbel@firma.test)",
        ] {
            assert_eq!(found(value)?, ["anna.zorbel@firma.test"], "{value}");
        }
        Ok(())
    }

    #[test]
    fn urls_keep_their_shape() -> anyhow::Result<()> {
        let re = email_regex()?;
        let url = "https://firma.test/x?u=anna.zorbel@firma.test&y=1";
        let out = replace_emails(url, &re, &mut |_| "x@y.test".to_string());
        assert_eq!(out, "https://firma.test/x?u=x@y.test&y=1");
        assert_eq!(found("https://anna@firma.test/p")?, Vec::<String>::new());
        assert_eq!(found("https://user:pw@firma.test/p")?, Vec::<String>::new());
        assert_eq!(
            found("https://firma.test/a.b@c.test")?,
            Vec::<String>::new()
        );
        assert_eq!(found("https://f.test/?a=1&m=b@c.test")?, ["b@c.test"]);
        assert_eq!(
            found("mailto:anna@firma.test?subject=x")?,
            ["anna@firma.test"]
        );
        assert_eq!(found("see MAILTO:anna@firma.test")?, ["anna@firma.test"]);
        Ok(())
    }

    #[test]
    fn domain_needs_a_letter_in_the_last_label() -> anyhow::Result<()> {
        assert_eq!(found("Preis 3@4.50 EUR")?, Vec::<String>::new());
        assert_eq!(found("a@1.2.3.4")?, Vec::<String>::new());
        assert_eq!(found("a@b.c1")?, ["a@b.c1"]);
        Ok(())
    }

    #[test]
    fn localhost_quoted_and_idn_addresses() -> anyhow::Result<()> {
        assert_eq!(found("anna.zorbel@localhost")?, ["anna.zorbel@localhost"]);
        assert_eq!(
            found("Mail \"anna zorbel\"@firma.test.")?,
            ["\"anna zorbel\"@firma.test"]
        );
        assert_eq!(found("a@xn--bckerei-5wa.test")?, ["a@xn--bckerei-5wa.test"]);
        assert_eq!(found("a@b\u{e4}ckerei.test")?, ["a@b\u{e4}ckerei.test"]);
        assert_eq!(found("a@b.test@c.test")?.len(), 1);
        Ok(())
    }

    #[test]
    fn local_part_dots_are_strict() -> anyhow::Result<()> {
        assert_eq!(found(".a@b.test")?, ["a@b.test"]);
        assert_eq!(found("a..b@c.test")?, ["b@c.test"]);
        assert_eq!(found("a.@c.test")?, Vec::<String>::new());
        assert_eq!(found("a@b..test")?, ["a@b"]);
        assert_eq!(found("a@-b.test")?, Vec::<String>::new());
        assert_eq!(found("a+tag_1%x@b.test")?, ["a+tag_1%x@b.test"]);
        Ok(())
    }
}
