//! Project key in `secrets/pseudofy.key` (ADR-0003).

use std::path::Path;

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
pub fn create(_project: &Path) -> anyhow::Result<()> {
    todo!("task io")
}

/// Load `secrets/pseudofy.key`. If the file is missing, the error tells the
/// user to run `pseudofy init`. No error message contains the key.
pub fn load(_project: &Path) -> anyhow::Result<ProjectKey> {
    todo!("task io")
}
