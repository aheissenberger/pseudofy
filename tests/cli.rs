use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;

fn pseudofy() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("pseudofy")
}

#[test]
fn version_prints_name_and_version() {
    pseudofy()
        .arg("--version")
        .assert()
        .success()
        .stdout("pseudofy 0.1.0\n");
}

#[test]
fn help_lists_subcommands() {
    pseudofy()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("init").and(contains("run")));
}

#[test]
fn unknown_subcommand_fails() {
    pseudofy().arg("unknown").assert().failure();
}

fn init_command(project: &std::path::Path) -> Command {
    let mut command = pseudofy();
    command
        .args(["init", "--project"])
        .arg(project)
        .env_remove("LC_ALL")
        .env_remove("LC_MESSAGES")
        .env_remove("LANGUAGE")
        .env_remove("LANG");
    command
}

#[test]
fn init_with_language_de_ignores_the_system_language() {
    let dir = tempfile::TempDir::new().unwrap();
    init_command(dir.path())
        .args(["--language", "de"])
        .env("LANG", "fr_FR.UTF-8")
        .env("LC_ALL", "fr_FR.UTF-8")
        .assert()
        .success();
    let config = std::fs::read_to_string(dir.path().join("config/pseudofy.toml")).unwrap();
    assert!(config.lines().any(|l| l == "language = \"de\""), "{config}");
    assert!(
        config.lines().any(|l| l == "email_subdomains = true"),
        "{config}"
    );
    let key = std::fs::read_to_string(dir.path().join("secrets/pseudofy.key")).unwrap();
    let digits = key.strip_suffix('\n').unwrap();
    assert_eq!(digits.len(), 64);
    assert!(digits.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn init_with_unsupported_system_language_writes_en_and_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    init_command(dir.path())
        .env("LC_ALL", "fr_FR.UTF-8")
        .env("LC_MESSAGES", "fr_FR.UTF-8")
        .env("LANG", "fr_FR.UTF-8")
        .assert()
        .success()
        .stderr(contains("warning"));
    let config = std::fs::read_to_string(dir.path().join("config/pseudofy.toml")).unwrap();
    assert!(config.lines().any(|l| l == "language = \"en\""), "{config}");
}

#[test]
fn init_twice_fails_and_keeps_the_key() {
    let dir = tempfile::TempDir::new().unwrap();
    init_command(dir.path())
        .args(["--language", "en"])
        .assert()
        .success();
    let key_path = dir.path().join("secrets/pseudofy.key");
    let key = std::fs::read_to_string(&key_path).unwrap();
    init_command(dir.path())
        .args(["--language", "en"])
        .assert()
        .failure();
    assert_eq!(key, std::fs::read_to_string(&key_path).unwrap());
}
