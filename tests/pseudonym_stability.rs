//! Stability (golden) tests for requirement 7 (docs/plan/pseudonymization.md):
//! one real value gets the same fake value in each later run with the same
//! project key, also across releases. ADR-0003 freezes the name lists.
//!
//! These tests protect two things: the SHA-256 of each frozen name list, and
//! the exact pseudonyms that a fixed key produces for fixed invented input.
//! A change of any value here is a BREAKING change. It needs a new ADR and a
//! release note. Never update the expected values just to make a test pass.
//! All data is invented.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions may panic"
)]

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const LIST_HASHES: [(&str, &str); 4] = [
    (
        "data/de/first-names.txt",
        "df13439cee5e326d86769db7155babe7104225363e0cc89a4acaa9c880e96dc6",
    ),
    (
        "data/de/last-names.txt",
        "e15ce2912a2bbe43e9edd85b53155983329dd9ea631e415ca63611efbe942704",
    ),
    (
        "data/en/first-names.txt",
        "c2449907ab062a6be5e7c65d867d531db4d538753ae824d352c5db1acb859e27",
    ),
    (
        "data/en/last-names.txt",
        "874dd474e81aa880d555396c86ca7fa8bcded3d771658b7d5761bf9f48bc4c3c",
    ),
];

const INPUT: &str = "first_name,last_name,name,email\n\
Zorbina,Quenzel,Tamsorin Velgrad,zorbina.quenzel@acme.test\n\
Tamsorin,Velgrad,Anna B.,tamsorin.velgrad@other.test\n\
Zorbina,Velgrad,Zorbina Quenzel,zorbina.velgrad@acme.test\n";

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        write!(out, "{b:02x}").unwrap();
        out
    })
}

#[test]
fn name_lists_match_their_frozen_checksums() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (path, expected) in LIST_HASHES {
        let bytes = fs::read(root.join(path)).unwrap();
        let actual = hex(&Sha256::digest(&bytes));
        assert_eq!(
            actual, expected,
            "{path}: a name list changed; this changes existing pseudonyms (requirement 7, ADR-0003)"
        );
    }
}

/// Runs pseudofy on the invented input and returns the output CSV.
fn pseudonymize(language: &str, email_subdomains: bool) -> String {
    let dir = TempDir::new().unwrap();
    let write = |relative: &str, content: &str| {
        let path = dir.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    };
    write("secrets/pseudofy.key", &format!("{KEY}\n"));
    write(
        "config/pseudofy.toml",
        &format!(
            "language = \"{language}\"\nemail_subdomains = {email_subdomains}\n\n\
             [[rule]]\nfiles = \"**/*.csv\"\nfirst_name = [\"first_name\"]\n\
             last_name = [\"last_name\"]\nfull_name = [\"name\"]\nemail = [\"email\"]\n"
        ),
    );
    write("people.csv", INPUT);
    let out = TempDir::new().unwrap();
    Command::cargo_bin("pseudofy")
        .unwrap()
        .args(["run", "--project"])
        .arg(dir.path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success();
    fs::read_to_string(out.path().join("people.csv")).unwrap()
}

fn check(language: &str, email_subdomains: bool, expected: &str) {
    let actual = pseudonymize(language, email_subdomains);
    assert_eq!(
        actual, expected,
        "pseudonyms changed ({language}, email_subdomains={email_subdomains}); \
         this breaks requirement 7 and needs an ADR and a release note"
    );
}

#[test]
fn german_pseudonyms_with_email_subdomains_are_stable() {
    check(
        "de",
        true,
        "first_name,last_name,name,email\nNora,Wuest,Burkhart Herde,juli.langer@vuralahego.example.com\nBurkhart,Herde,Rolf G.,jan-hendrik.link@datosizazu.example.com\nNora,Herde,Nora Wuest,jean.grossmann@vuralahego.example.com\n",
    );
}

#[test]
fn german_pseudonyms_without_email_subdomains_are_stable() {
    check(
        "de",
        false,
        "first_name,last_name,name,email\nNora,Wuest,Burkhart Herde,juli.langer@example.com\nBurkhart,Herde,Rolf G.,jan-hendrik.link@example.com\nNora,Herde,Nora Wuest,jean.grossmann@example.com\n",
    );
}

#[test]
fn english_pseudonyms_with_email_subdomains_are_stable() {
    check(
        "en",
        true,
        "first_name,last_name,name,email\nMona,Salter,Bradley Vaughn,booker.rader@vuralahego.example.com\nBradley,Vaughn,Orville G.,jerome.cameron@datosizazu.example.com\nMona,Vaughn,Mona Salter,shelley.donnelly@vuralahego.example.com\n",
    );
}

#[test]
fn english_pseudonyms_without_email_subdomains_are_stable() {
    check(
        "en",
        false,
        "first_name,last_name,name,email\nMona,Salter,Bradley Vaughn,booker.rader@example.com\nBradley,Vaughn,Orville G.,jerome.cameron@example.com\nMona,Vaughn,Mona Salter,shelley.donnelly@example.com\n",
    );
}
