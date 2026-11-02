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

#[test]
fn default_config_covers_csv_and_xlsx() {
    let dir = tempfile::TempDir::new().unwrap();
    pseudofy()
        .args(["init", "--language", "en", "--project"])
        .arg(dir.path())
        .assert()
        .success();
    let workbook_dir = dir.path().join("sub");
    std::fs::create_dir_all(&workbook_dir).unwrap();
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.sheet_mut(0).unwrap();
    sheet.cell_mut("A1").set_value_string("name");
    sheet.cell_mut("A2").set_value_string("Zorbina Quenzel");
    umya_spreadsheet::writer::xlsx::write(&book, workbook_dir.join("people.xlsx")).unwrap();
    std::fs::write(dir.path().join("people.csv"), "name\nZorbina Quenzel\n").unwrap();
    let out = tempfile::TempDir::new().unwrap();
    let assert = pseudofy()
        .args(["run", "--project"])
        .arg(dir.path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(!stderr.contains("no rule matches"), "{stderr}");
    let csv = std::fs::read_to_string(out.path().join("people.csv")).unwrap();
    assert!(!csv.contains("Zorbina"), "{csv}");
    let copy = umya_spreadsheet::reader::xlsx::read(out.path().join("sub/people.xlsx")).unwrap();
    let value = copy.sheet(0).unwrap().value("A2");
    assert!(!value.is_empty() && !value.contains("Zorbina"), "{value}");
}
