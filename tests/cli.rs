#![expect(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions may panic"
)]

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
    for line in [
        "# email_subdomains = true",
        "# copy_other_files = false",
        "# free_text = \"tokens\"",
        "[[rule]]",
    ] {
        assert!(config.lines().any(|l| l == line), "{line}: {config}");
    }
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

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const IMAGE_BYTES: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0xff, 0xfe, 0x00, 0x80, 0xc3, 0x28, 0x0a,
];

fn write_at(root: &std::path::Path, relative: &str, content: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// Project with a CSV, a rule and many other files. `setting` is a line for
/// the top level of the config, or empty.
fn other_files_project(setting: &str) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    write_at(root, "secrets/pseudofy.key", format!("{KEY}\n").as_bytes());
    write_at(root, "secrets/extra.txt", b"extra secret file");
    let config = format!(
        "language = \"en\"\n{setting}\n[[rule]]\nfiles = \"**/*.csv\"\nfull_name = [\"name\"]\n"
    );
    write_at(root, "config/pseudofy.toml", config.as_bytes());
    write_at(root, "config/other.toml", b"other config");
    write_at(root, "data.csv", b"name\nZorbina Quenzel\n");
    write_at(root, "notes.txt", b"some notes\n");
    write_at(root, "sub/image.png", IMAGE_BYTES);
    write_at(root, ".private_x", b"hidden");
    write_at(root, ".git/config", b"vcs");
    write_at(root, ".DS_Store", b"ds");
    write_at(root, "sub/.hidden", b"hidden");
    write_at(root, "~$book.xlsx", b"lock");
    #[cfg(unix)]
    std::os::unix::fs::symlink("notes.txt", root.join("link.txt")).unwrap();
    dir
}

fn run_project(project: &std::path::Path, out: &std::path::Path) -> assert_cmd::assert::Assert {
    pseudofy()
        .args(["run", "--project"])
        .arg(project)
        .arg("--out")
        .arg(out)
        .assert()
        .success()
}

const EXCLUDED: [&str; 8] = [
    ".private_x",
    ".git/config",
    ".DS_Store",
    "sub/.hidden",
    "~$book.xlsx",
    "secrets/extra.txt",
    "config/other.toml",
    "link.txt",
];

fn assert_only_csv_copied(out: &std::path::Path) {
    let csv = std::fs::read_to_string(out.join("data.csv")).unwrap();
    assert!(
        !csv.contains("Zorbina") && !csv.contains("Quenzel"),
        "{csv}"
    );
    assert!(!out.join("notes.txt").exists());
    assert!(!out.join("sub/image.png").exists());
    for name in EXCLUDED {
        assert!(!out.join(name).exists(), "{name}");
    }
}

#[test]
fn run_without_copy_other_files_copies_only_the_csv() {
    let dir = other_files_project("");
    let out = tempfile::TempDir::new().unwrap();
    let assert = run_project(dir.path(), out.path());
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(!stdout.contains("copied unchanged"), "{stdout}");
    assert_only_csv_copied(out.path());
}

#[test]
fn run_with_copy_other_files_false_behaves_like_the_default() {
    let dir = other_files_project("copy_other_files = false\n");
    let out = tempfile::TempDir::new().unwrap();
    let assert = run_project(dir.path(), out.path());
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(!stdout.contains("copied unchanged"), "{stdout}");
    assert_only_csv_copied(out.path());
}

#[test]
fn run_with_copy_other_files_copies_plain_files_byte_identical() {
    let dir = other_files_project("copy_other_files = true\n");
    let out = tempfile::TempDir::new().unwrap();
    let assert = run_project(dir.path(), out.path());
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert_eq!(
        std::fs::read(out.path().join("notes.txt")).unwrap(),
        b"some notes\n"
    );
    assert_eq!(
        std::fs::read(out.path().join("sub/image.png")).unwrap(),
        IMAGE_BYTES
    );
    assert!(
        stdout.lines().any(|l| l == "notes.txt: copied unchanged"),
        "{stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l == "sub/image.png: copied unchanged"),
        "{stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("files:") && l.ends_with(", copied unchanged: 2")),
        "{stdout}"
    );
}

#[test]
fn run_with_copy_other_files_skips_excluded_files_and_pseudonymizes_the_csv() {
    let dir = other_files_project("copy_other_files = true\n");
    let out = tempfile::TempDir::new().unwrap();
    run_project(dir.path(), out.path());
    for name in EXCLUDED {
        assert!(!out.path().join(name).exists(), "{name}");
    }
    let csv = std::fs::read_to_string(out.path().join("data.csv")).unwrap();
    assert!(
        !csv.contains("Zorbina") && !csv.contains("Quenzel"),
        "{csv}"
    );
}

#[test]
fn run_with_copy_other_files_never_copies_uppercase_csv_or_xlsx_unchanged() {
    let dir = other_files_project("copy_other_files = true\n");
    write_at(dir.path(), "DATA.CSV", b"name\nZorbina Quenzel\n");
    write_at(dir.path(), "Book.XLSX", b"Zorbina Quenzel not a workbook");
    let out = tempfile::TempDir::new().unwrap();
    // The run may fail for these files; only the outputs matter.
    let _ = pseudofy()
        .args(["run", "--project"])
        .arg(dir.path())
        .arg("--out")
        .arg(out.path())
        .assert();
    for name in ["DATA.CSV", "Book.XLSX"] {
        if let Ok(bytes) = std::fs::read(out.path().join(name)) {
            let text = String::from_utf8_lossy(&bytes);
            assert!(!text.contains("Zorbina"), "{name}: {text}");
        }
    }
}
