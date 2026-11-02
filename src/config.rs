//! Project config in `config/pseudofy.toml`.

use std::path::Path;

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
}

fn default_email_subdomains() -> bool {
    true
}

/// Load `config/pseudofy.toml`.
pub fn load(_project: &Path) -> anyhow::Result<Config> {
    todo!("task io")
}

/// Write `config/pseudofy.toml` with the language, `email_subdomains = true`,
/// and one example rule. Fail if the file exists.
pub fn create(_project: &Path, _language: Language) -> anyhow::Result<()> {
    todo!("task io")
}
