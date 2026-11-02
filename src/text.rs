//! Name tokens, case styles, email search, and transliteration.

use regex::Regex;

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

/// Replace each name token in `value`.
///
/// `replace(part, token)` gives the pseudonym of one token. The result gets the
/// case style of the token (`Anna`, `ANNA`, `anna`). All separators stay.
pub fn replace_name(
    _value: &str,
    _role: NameRole,
    _replace: &mut dyn FnMut(Part, &str) -> String,
) -> String {
    todo!("task core")
}

/// Regex that finds email addresses in a cell.
pub fn email_regex() -> anyhow::Result<Regex> {
    todo!("task core")
}

/// Replace each email address in `value` with `replace(address)`.
pub fn replace_emails(
    _value: &str,
    _regex: &Regex,
    _replace: &mut dyn FnMut(&str) -> String,
) -> String {
    todo!("task core")
}
