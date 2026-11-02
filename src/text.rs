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
}

fn is_token_char(c: char) -> bool {
    c.is_alphabetic() || matches!(c, '\'' | '\u{2019}' | '\u{2bc}') || is_combining_mark(c)
}

/// Replace each name token in `value`.
///
/// `replace(part, token)` gives the pseudonym of one token. The result gets the
/// case style of the token (`Anna`, `ANNA`, `anna`). All separators stay.
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
        let pseudonym = replace(token.part, text);
        out.push_str(&apply_case(text, &pseudonym));
        pos = token.end;
    }
    out.push_str(value.get(pos..).unwrap_or(""));
    out
}

/// Maximal runs of alphabetic chars and apostrophes, without apostrophes at
/// either end (`‘Bob’` gives `Bob`). Runs without a letter are separators.
fn split_tokens(value: &str) -> Vec<Token> {
    fn push(value: &str, start: usize, end: usize, tokens: &mut Vec<Token>) {
        let is_apos = |c: char| matches!(c, '\'' | '\u{2019}' | '\u{2bc}');
        let run = value.get(start..end).unwrap_or("");
        let start = start + (run.len() - run.trim_start_matches(is_apos).len());
        let end = start
            + value
                .get(start..end)
                .unwrap_or("")
                .trim_end_matches(is_apos)
                .len();
        if value
            .get(start..end)
            .is_some_and(|t| t.chars().any(char::is_alphabetic))
        {
            tokens.push(Token {
                start,
                end,
                part: Part::Last,
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
            for t in tokens.iter_mut() {
                t.part = Part::Last;
            }
        }
        NameRole::Full => {
            if let Some(comma) = value.find(',') {
                for t in tokens.iter_mut() {
                    t.part = if t.start < comma {
                        Part::Last
                    } else {
                        Part::First
                    };
                }
            } else {
                let n = tokens.len();
                for (i, t) in tokens.iter_mut().enumerate() {
                    t.part = if i + 1 == n { Part::Last } else { Part::First };
                }
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

/// Regex that finds email addresses in a cell.
pub fn email_regex() -> anyhow::Result<Regex> {
    Ok(Regex::new(
        r#"[^\s@<>(),;:"\[\]]+@[^\s@<>(),;:"'\[\]]+\.[^\s@<>(),;:"'\[\]]+"#,
    )?)
}

/// Byte ranges of the email addresses in `value`.
///
/// A match loses leading and trailing `'` and trailing `.`; they stay plain
/// text outside the range. A match without a domain with a dot is skipped.
pub fn email_spans(value: &str, regex: &Regex) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    for m in regex.find_iter(value) {
        let no_tail = m.as_str().trim_end_matches(['.', '\'']);
        let trimmed = no_tail.trim_start_matches('\'');
        let lead = no_tail.len() - trimmed.len();
        let has_domain = trimmed.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.contains('.') && !domain.starts_with('.')
        });
        if has_domain {
            let start = m.start() + lead;
            spans.push(start..start + trimmed.len());
        }
    }
    spans
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
}
