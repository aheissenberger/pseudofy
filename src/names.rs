//! Frozen name lists (ADR-0003). A change to a list changes all pseudonyms.

use crate::locale::Language;

const DE_FIRST: &str = include_str!("../data/de/first-names.txt");
const DE_LAST: &str = include_str!("../data/de/last-names.txt");
const EN_FIRST: &str = include_str!("../data/en/first-names.txt");
const EN_LAST: &str = include_str!("../data/en/last-names.txt");

/// First names and last names of one language.
pub struct NameLists {
    pub first: Vec<&'static str>,
    pub last: Vec<&'static str>,
}

/// Name lists of `language`.
pub fn lists(language: Language) -> NameLists {
    let (first, last) = match language {
        Language::De => (DE_FIRST, DE_LAST),
        Language::En => (EN_FIRST, EN_LAST),
    };
    NameLists {
        first: parse(first),
        last: parse(last),
    }
}

/// One name for each line. Empty lines and lines that start with `#` are skipped.
fn parse(text: &'static str) -> Vec<&'static str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn check(list: &[&str]) {
        assert!(list.len() >= 1000, "list has {} entries", list.len());
        let mut seen = HashSet::new();
        for name in list {
            assert!(
                !name.contains(char::is_whitespace) && !name.contains(','),
                "bad entry {name}"
            );
            assert!(seen.insert(name.to_lowercase()), "duplicate entry {name}");
        }
    }

    #[test]
    fn each_list_has_1000_unique_entries() {
        for language in [Language::De, Language::En] {
            let lists = lists(language);
            check(&lists.first);
            check(&lists.last);
        }
    }

    #[test]
    fn names_encode_to_windows_1252_without_loss() {
        for language in [Language::De, Language::En] {
            let lists = lists(language);
            for name in lists.first.iter().chain(lists.last.iter()) {
                let (bytes, _, unmappable) = encoding_rs::WINDOWS_1252.encode(name);
                assert!(!unmappable, "entry {name} is not Windows-1252");
                let (back, _) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(&bytes);
                assert_eq!(&*back, *name);
            }
        }
    }

    #[test]
    fn parse_skips_comments_and_empty_lines() {
        assert_eq!(parse("# c\n\nAnna\n  Ben \n"), vec!["Anna", "Ben"]);
    }
}
