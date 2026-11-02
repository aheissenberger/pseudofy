//! Project config in `config/pseudofy.toml`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::Context;

use serde::Deserialize;

use crate::locale::Language;

/// Path of the config file, relative to the project directory.
pub const CONFIG_FILE: &str = "config/pseudofy.toml";

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub language: Language,
    #[serde(default = "default_email_subdomains")]
    pub email_subdomains: bool,
    /// How known names are replaced in text cells outside the name columns.
    #[serde(default)]
    pub free_text: FreeText,
    /// Copy other files (not CSV or XLSX) unchanged to the output (spec 0014).
    #[serde(default)]
    pub copy_other_files: bool,
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
}

/// Mode of the known-name replacement in free text (spec 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FreeText {
    /// No known-name replacement outside the name columns. Emails are still
    /// replaced in each cell.
    Off,
    /// Replace only complete full names ("Anna Muster", "Muster, Anna").
    Full,
    /// Also replace each single first-name and last-name token.
    #[default]
    Tokens,
}

/// One `[[rule]]`: a file glob and the header texts of the name and email columns.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub files: String,
    #[serde(default)]
    pub first_name: Vec<String>,
    #[serde(default)]
    pub last_name: Vec<String>,
    #[serde(default)]
    pub full_name: Vec<String>,
    #[serde(default)]
    pub email: Vec<String>,
    /// Sheet-name globs for XLSX files. Empty: the rule applies to each sheet.
    #[serde(default)]
    pub sheets: Vec<String>,
}

fn default_email_subdomains() -> bool {
    true
}

/// Load `config/pseudofy.toml`.
pub fn load(project: &Path) -> anyhow::Result<Config> {
    let text = fs::read_to_string(project.join(CONFIG_FILE))
        .with_context(|| format!("cannot read {CONFIG_FILE}"))?;
    toml::from_str(&text).with_context(|| format!("invalid {CONFIG_FILE}"))
}

/// Write `config/pseudofy.toml` with the language, one example rule, and the
/// other settings as commented defaults. Fail if the file exists.
pub fn create(project: &Path, language: Language) -> anyhow::Result<()> {
    let path = project.join(CONFIG_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("cannot create the config directory")?;
    }
    let text = format!(
        r##"# pseudofy config
#
# A setting with "#" in front shows its default value.
# Remove the "#" to change the value.

# Language of the fake names: "de" or "en".
language = "{}"

# Fake email domains.
# true:  each real domain gets one fake subdomain of example.com.
# false: each fake email uses example.com.
# email_subdomains = true

# Known names in other text cells (not in the name columns).
# "tokens": replace full names and single first or last names.
# "full":   replace only complete full names.
# "off":    replace no names in other text cells.
# free_text = "tokens"

# Other files (not .csv or .xlsx).
# false: do not copy them.
# true:  copy them unchanged to the output directory. pseudofy does not
#        check them. Check each copied file before you share it.
#        Hidden files and directories (".*") are never copied.
# copy_other_files = false

# Rules. Each rule selects files and names the header texts of the name
# and email columns. Add one [[rule]] block for each group of files.
#
# files:      glob relative to the project directory (case is ignored).
# first_name: header texts of first-name columns.
# last_name:  header texts of last-name columns.
# full_name:  header texts of full-name columns ("First Last").
# email:      header texts of email columns.
# sheets:     XLSX only. Sheet-name globs. Default: each sheet.
#
# In XLSX files, each sheet uses its first non-empty row as the header.
[[rule]]
files = "**/*.{{csv,xlsx}}"
first_name = ["first_name"]
last_name = ["last_name"]
full_name = ["name"]
email = ["email"]
# sheets = ["*"]
"##,
        language.code()
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("cannot create {CONFIG_FILE}"))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("cannot write {CONFIG_FILE}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn created_config_loads() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), Language::De).unwrap();
        let config = load(dir.path()).unwrap();
        assert_eq!(config.language, Language::De);
        assert!(config.email_subdomains);
        assert_eq!(config.rules.len(), 1);
        assert_eq!(config.rules[0].files, "**/*.{csv,xlsx}");
        assert!(create(dir.path(), Language::En).is_err());
    }

    fn template(language: Language) -> String {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), language).unwrap();
        fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap()
    }

    fn expected(language: Language) -> Config {
        Config {
            language,
            email_subdomains: true,
            free_text: FreeText::Tokens,
            copy_other_files: false,
            rules: vec![Rule {
                files: "**/*.{csv,xlsx}".to_owned(),
                first_name: vec!["first_name".to_owned()],
                last_name: vec!["last_name".to_owned()],
                full_name: vec!["name".to_owned()],
                email: vec!["email".to_owned()],
                sheets: Vec::new(),
            }],
        }
    }

    #[test]
    fn copy_other_files_defaults_to_false_and_parses() {
        let config: Config = toml::from_str("language = \"en\"").unwrap();
        assert!(!config.copy_other_files);
        let config: Config = toml::from_str("language = \"en\"\ncopy_other_files = true").unwrap();
        assert!(config.copy_other_files);
    }

    #[test]
    fn template_parses_to_the_defaults_plus_one_rule() {
        for language in [Language::De, Language::En] {
            let config: Config = toml::from_str(&template(language)).unwrap();
            assert_eq!(config, expected(language));
        }
    }

    #[test]
    fn template_mentions_each_field() {
        let text = template(Language::En);
        for name in [
            "language",
            "email_subdomains",
            "free_text",
            "copy_other_files",
            "files",
            "first_name",
            "last_name",
            "full_name",
            "email",
            "sheets",
        ] {
            assert!(text.contains(name), "{name}");
        }
    }

    #[test]
    fn commented_defaults_are_true_defaults() {
        let text: String = template(Language::En)
            .lines()
            .map(|line| match line.strip_prefix("# ") {
                Some(rest) if is_setting(rest) => rest,
                _ => line,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let config: Config = toml::from_str(&text).unwrap();
        // `sheets = ["*"]` matches each sheet, like the empty default.
        let mut want = expected(Language::En);
        want.rules[0].sheets = vec!["*".to_owned()];
        assert_eq!(config, want);
    }

    /// A commented `key = value` line (the key is a lowercase word).
    fn is_setting(line: &str) -> bool {
        line.split_once(" = ").is_some_and(|(key, _)| {
            !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_')
        })
    }
}
