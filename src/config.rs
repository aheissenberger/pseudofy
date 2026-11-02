//! Project config in `config/pseudofy.toml`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::Context;

use serde::Deserialize;

use crate::locale::Language;

/// Path of the config file, relative to the project directory.
pub const CONFIG_FILE: &str = "config/pseudofy.toml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub language: Language,
    #[serde(default = "default_email_subdomains")]
    pub email_subdomains: bool,
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
}

/// One `[[rule]]`: a file glob and the header texts of the name and email columns.
#[derive(Debug, Deserialize)]
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

/// Write `config/pseudofy.toml` with the language, `email_subdomains = true`,
/// and one example rule. Fail if the file exists.
pub fn create(project: &Path, language: Language) -> anyhow::Result<()> {
    let path = project.join(CONFIG_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("cannot create the config directory")?;
    }
    let text = format!(
        r#"# pseudofy config
language = "{}"
# false: each fake email uses example.com. true: each real domain gets one fake subdomain.
email_subdomains = true

# Each rule selects files with a glob (relative to the project) and names the
# header texts of the name and email columns. In XLSX files, each sheet uses
# its first non-empty row as the header. `sheets = ["Kunden*"]` limits a rule
# to the sheets with matching names.
[[rule]]
files = "**/*.{{csv,xlsx}}"
first_name = ["first_name"]
last_name = ["last_name"]
full_name = ["name"]
email = ["email"]
"#,
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
}
