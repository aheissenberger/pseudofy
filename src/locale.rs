//! System language for `pseudofy init`.

use serde::Deserialize;

/// Language of the bundled name lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    De,
    En,
}

impl Language {
    /// Config value of the language: `de` or `en`.
    pub fn code(self) -> &'static str {
        match self {
            Language::De => "de",
            Language::En => "en",
        }
    }
}

/// Result of the language detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    pub language: Language,
    /// `true` if no name list exists for the tag and `en` is the fallback.
    pub fallback: bool,
}

/// Map a locale tag (`de-AT`, `de_AT.UTF-8`, `en-US`, `fr`, `C`) to a language.
/// `None` and each unsupported tag give `en` with `fallback = true`.
pub fn from_tag(_tag: Option<&str>) -> Detected {
    todo!("task core")
}

/// Read the system language with `sys-locale`.
pub fn system_language() -> Detected {
    from_tag(sys_locale::get_locale().as_deref())
}
