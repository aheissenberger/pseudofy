//! HMAC-SHA256 pseudonyms (ADR-0003).

use crate::key::ProjectKey;
use crate::locale::Language;

/// Maps real values to pseudonyms and counts the collisions of one run.
pub struct Pseudonymizer {
    _todo: (),
}

impl Pseudonymizer {
    pub fn new(_key: &ProjectKey, _language: Language, _email_subdomains: bool) -> Self {
        todo!("task core")
    }

    /// Pseudonym of one first-name token, as written in the name list.
    pub fn first_name(&mut self, _real: &str) -> String {
        todo!("task core")
    }

    /// Pseudonym of one last-name token, as written in the name list.
    pub fn last_name(&mut self, _real: &str) -> String {
        todo!("task core")
    }

    /// Fake email address for one real email address.
    pub fn email(&mut self, _real: &str) -> String {
        todo!("task core")
    }

    /// Number of collisions detected so far.
    pub fn collisions(&self) -> usize {
        todo!("task core")
    }
}
