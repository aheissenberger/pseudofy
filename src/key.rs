//! Project key in `secrets/pseudofy.key` (ADR-0003).

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use anyhow::{Context, anyhow, bail};

/// Path of the key file, relative to the project directory.
pub const KEY_FILE: &str = "secrets/pseudofy.key";

/// 32 secret bytes. The type has no `Debug` or `Display`: never print it.
pub struct ProjectKey([u8; 32]);

impl ProjectKey {
    /// Key bytes for the HMAC.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Key from fixed bytes. Only for tests.
    #[cfg(test)]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        ProjectKey(bytes)
    }
}

/// Create `secrets/pseudofy.key` with 32 random bytes as 64 hex digits.
/// Fail if the file exists.
pub fn create(project: &Path) -> anyhow::Result<()> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| anyhow!("cannot get random bytes"))?;
    let mut text = String::with_capacity(65);
    for byte in bytes {
        for nibble in [byte >> 4, byte & 0x0f] {
            text.push(char::from_digit(u32::from(nibble), 16).unwrap_or('0'));
        }
    }
    text.push('\n');

    let path = project.join(KEY_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create directory for {KEY_FILE}"))?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .with_context(|| format!("cannot create {KEY_FILE}"))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("cannot write {KEY_FILE}"))?;
    Ok(())
}

/// Load `secrets/pseudofy.key`. If the file is missing, the error tells the
/// user to run `pseudofy init`. No error message contains the key.
pub fn load(project: &Path) -> anyhow::Result<ProjectKey> {
    let path = project.join(KEY_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            bail!("{KEY_FILE} not found; run `pseudofy init --project <DIR>` first");
        }
        Err(_) => bail!("cannot read {KEY_FILE}"),
    };
    parse(text.trim()).ok_or_else(|| anyhow!("{KEY_FILE} is not 64 hex digits"))
}

fn parse(text: &str) -> Option<ProjectKey> {
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(ProjectKey(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path()).unwrap();
        let text = fs::read_to_string(dir.path().join(KEY_FILE)).unwrap();
        assert_eq!(text.len(), 65);
        let key = load(dir.path()).unwrap();
        assert_eq!(key.as_bytes().len(), 32);
        assert!(create(dir.path()).is_err());
    }

    #[test]
    fn missing_key_tells_user_to_init() {
        let dir = tempfile::tempdir().unwrap();
        let message = load(dir.path()).err().unwrap().to_string();
        assert!(message.contains("run `pseudofy init --project <DIR>` first"));
    }

    #[test]
    fn bad_key_error_hides_content() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("secrets")).unwrap();
        fs::write(dir.path().join(KEY_FILE), "zzsecretcontent\n").unwrap();
        let message = load(dir.path()).err().unwrap().to_string();
        assert!(!message.contains("secretcontent"));
    }
}
