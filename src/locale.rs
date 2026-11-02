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
pub fn from_tag(tag: Option<&str>) -> Detected {
    let subtag = tag
        .and_then(|t| t.split(['-', '_', '.', '@']).next())
        .unwrap_or("")
        .to_ascii_lowercase();
    match subtag.as_str() {
        "de" => Detected {
            language: Language::De,
            fallback: false,
        },
        "en" => Detected {
            language: Language::En,
            fallback: false,
        },
        _ => Detected {
            language: Language::En,
            fallback: true,
        },
    }
}

/// Read the system language with `sys-locale`.
pub fn system_language() -> Detected {
    from_tag(sys_locale::get_locale().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_tags_map_without_fallback() {
        for (tag, language) in [
            ("de", Language::De),
            ("de-AT", Language::De),
            ("de_AT.UTF-8", Language::De),
            ("DE_de@euro", Language::De),
            ("en-US", Language::En),
            ("en", Language::En),
        ] {
            let d = from_tag(Some(tag));
            assert_eq!(d.language, language, "{tag}");
            assert!(!d.fallback, "{tag}");
        }
    }

    #[test]
    fn other_tags_fall_back_to_en() {
        for tag in [
            Some("fr"),
            Some("fr-FR"),
            Some("C"),
            Some("POSIX"),
            Some(""),
            None,
        ] {
            let d = from_tag(tag);
            assert_eq!(d.language, Language::En);
            assert!(d.fallback);
        }
    }
}
