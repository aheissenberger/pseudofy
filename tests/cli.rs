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
        .stdout(format!("pseudofy {}\n", env!("CARGO_PKG_VERSION")));
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
        "files = [\"**/*.csv\", \"**/*.xlsx\"]",
        "# files_excluded = []",
    ] {
        assert!(config.lines().any(|l| l == line), "{line}: {config}");
    }
    let key = std::fs::read_to_string(dir.path().join("secrets/pseudofy.key")).unwrap();
    let digits = key.strip_suffix('\n').unwrap();
    assert_eq!(digits.len(), 64);
    assert!(digits.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "sys-locale reads the macOS system language, not LANG or LC_ALL"
)]
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
        "language = \"en\"\n{setting}\n[[rule]]\nfiles = [\"**/*.csv\"]\nfull_name = [\"name\"]\n"
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

#[test]
fn run_with_copy_other_files_skips_secrets_and_config_in_any_case() {
    // On a case-insensitive file system `Secrets/` is `secrets/`. The test
    // creates both spellings, so it also works on a case-sensitive one.
    let dir = other_files_project("copy_other_files = true\n");
    write_at(dir.path(), "Secrets/other.key", b"secret");
    write_at(dir.path(), "CONFIG/other.toml", b"config");
    let out = tempfile::TempDir::new().unwrap();
    run_project(dir.path(), out.path());
    assert!(!out.path().join("Secrets").exists());
    assert!(!out.path().join("CONFIG").exists());
}

#[cfg(unix)]
#[test]
fn run_with_copy_other_files_keeps_permissions_and_skips_directory_symlinks() {
    use std::os::unix::fs::PermissionsExt;
    let dir = other_files_project("copy_other_files = true\n");
    let private = dir.path().join("private.txt");
    std::fs::write(&private, b"private notes").unwrap();
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink("sub", dir.path().join("linkdir")).unwrap();
    let out = tempfile::TempDir::new().unwrap();
    run_project(dir.path(), out.path());
    let mode = std::fs::metadata(out.path().join("private.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert!(!out.path().join("linkdir").exists());
}

#[test]
fn run_with_copy_other_files_copies_a_long_file_name() {
    let dir = other_files_project("copy_other_files = true\n");
    let name = format!("{}.txt", "a".repeat(245));
    write_at(dir.path(), &name, b"long");
    let out = tempfile::TempDir::new().unwrap();
    run_project(dir.path(), out.path());
    assert_eq!(std::fs::read(out.path().join(&name)).unwrap(), b"long");
}

#[test]
fn run_with_copy_other_files_and_out_inside_the_project_does_not_copy_the_output() {
    let dir = other_files_project("copy_other_files = true\n");
    let out = dir.path().join("out");
    run_project(dir.path(), &out);
    run_project(dir.path(), &out);
    assert!(out.join("notes.txt").exists());
    assert!(!out.join("out").exists());
}

#[test]
fn run_with_copy_other_files_reports_a_failed_copy_by_path() {
    let dir = other_files_project("copy_other_files = true\n");
    let out = tempfile::TempDir::new().unwrap();
    // A directory in place of the target makes the copy fail.
    std::fs::create_dir_all(out.path().join("notes.txt")).unwrap();
    let assert = pseudofy()
        .args(["run", "--project"])
        .arg(dir.path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        stderr.lines().any(|l| l == "error: cannot copy notes.txt"),
        "{stderr}"
    );
    assert!(!stderr.contains("some notes"), "{stderr}");
}

#[test]
fn run_names_the_files_glob_when_no_rule_matches() {
    let dir = other_files_project("");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.sheet_mut(0).unwrap();
    sheet.cell_mut("A1").set_value_string("name");
    sheet.cell_mut("A2").set_value_string("Zorbina Quenzel");
    umya_spreadsheet::writer::xlsx::write(&book, dir.path().join("list.xlsx")).unwrap();
    let out = tempfile::TempDir::new().unwrap();
    let stderr = stderr_of(&run_output(dir.path(), out.path()));
    assert!(
        stderr.contains(
            "warning: list.xlsx: no rule matches this path (check \"files\" in the config); only emails are replaced"
        ),
        "{stderr}"
    );
}

fn run_output(project: &std::path::Path, out: &std::path::Path) -> std::process::Output {
    pseudofy()
        .args(["run", "--project"])
        .arg(project)
        .arg("--out")
        .arg(out)
        .output()
        .unwrap()
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn run_refuses_out_inside_the_project_with_files_and_no_marker() {
    let dir = other_files_project("");
    write_at(dir.path(), "data/a.csv", b"name\nZorbina Quenzel\n");
    write_at(dir.path(), "data/b.csv", b"name\nVelmar Drossel\n");
    let output = run_output(dir.path(), &dir.path().join("data"));
    assert!(!output.status.success());
    assert!(!stderr_of(&output).contains("Zorbina"));
    assert_eq!(
        std::fs::read(dir.path().join("data/a.csv")).unwrap(),
        b"name\nZorbina Quenzel\n"
    );
    assert_eq!(
        std::fs::read(dir.path().join("data/b.csv")).unwrap(),
        b"name\nVelmar Drossel\n"
    );
    assert!(!dir.path().join("data/.pseudofy-out").exists());
}

#[test]
fn run_twice_to_the_same_out_inside_the_project_works_with_the_marker() {
    let dir = other_files_project("");
    let out = dir.path().join("out");
    run_project(dir.path(), &out);
    assert!(out.join(".pseudofy-out").exists());
    run_project(dir.path(), &out);
    assert!(out.join("data.csv").exists());
    assert!(!out.join("out").exists());
}

#[test]
fn run_uses_an_empty_out_inside_the_project_and_marks_it() {
    let dir = other_files_project("");
    let out = dir.path().join("empty");
    std::fs::create_dir(&out).unwrap();
    run_project(dir.path(), &out);
    assert!(out.join(".pseudofy-out").exists());
    assert!(out.join("data.csv").exists());
}

#[cfg(unix)]
#[test]
fn run_refuses_out_in_secrets_through_a_symlink_or_another_case() {
    let dir = other_files_project("");
    let key_before = std::fs::read(dir.path().join("secrets/pseudofy.key")).unwrap();
    std::os::unix::fs::symlink("secrets", dir.path().join("sneaky")).unwrap();
    let config_link = dir.path().join("cfglink");
    std::os::unix::fs::symlink(dir.path().join("config"), &config_link).unwrap();
    for out in [
        dir.path().join("sneaky"),
        dir.path().join("sneaky/deep/out"),
        dir.path().join("Secrets"),
        dir.path().join("CONFIG/x"),
        config_link.join("out"),
    ] {
        let output = run_output(dir.path(), &out);
        assert!(!output.status.success(), "{}", out.display());
    }
    assert_eq!(
        std::fs::read(dir.path().join("secrets/pseudofy.key")).unwrap(),
        key_before
    );
    assert!(!dir.path().join("secrets/data.csv").exists());
    assert!(!dir.path().join("secrets/deep").exists());
    // A case-insensitive file system (APFS) finds `secrets` for `Secrets`.
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(
        !names.iter().any(|n| n == "Secrets" || n == "CONFIG"),
        "{names:?}"
    );
}

#[cfg(unix)]
#[test]
fn failed_key_write_leaves_no_key_and_a_second_init_works() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::TempDir::new().unwrap();
    let secrets = dir.path().join("secrets");
    std::fs::create_dir(&secrets).unwrap();
    std::fs::set_permissions(&secrets, std::fs::Permissions::from_mode(0o500)).unwrap();
    let first = init_command(dir.path())
        .args(["--language", "en"])
        .output()
        .unwrap();
    std::fs::set_permissions(&secrets, std::fs::Permissions::from_mode(0o700)).unwrap();
    if first.status.success() {
        // Root ignores the mode: the failure cannot be simulated.
        return;
    }
    assert!(!secrets.join("pseudofy.key").exists());
    assert_eq!(std::fs::read_dir(&secrets).unwrap().count(), 0);
    init_command(dir.path())
        .args(["--language", "en"])
        .assert()
        .success();
    assert!(secrets.join("pseudofy.key").exists());
}

#[cfg(unix)]
fn run_with_fifo_at(relative: &str) {
    let dir = other_files_project("");
    let path = dir.path().join(relative);
    std::fs::remove_file(&path).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap();
    assert!(made.success());
    let out = tempfile::TempDir::new().unwrap();
    pseudofy()
        .args(["run", "--project"])
        .arg(dir.path())
        .arg("--out")
        .arg(out.path())
        .timeout(std::time::Duration::from_secs(10))
        .assert()
        .failure()
        .stderr(contains(format!("cannot read {relative}")));
}

#[cfg(unix)]
#[test]
fn run_fails_at_once_when_the_key_is_a_fifo() {
    run_with_fifo_at("secrets/pseudofy.key");
}

#[cfg(unix)]
#[test]
fn run_fails_at_once_when_the_config_is_a_fifo() {
    run_with_fifo_at("config/pseudofy.toml");
}

#[test]
fn run_gives_no_copy_for_csv_in_hidden_directories_or_hidden_files() {
    let dir = other_files_project("");
    write_at(dir.path(), ".hidden/a.csv", b"name\nZorbina Quenzel\n");
    write_at(dir.path(), "sub/.deep/b.csv", b"name\nZorbina Quenzel\n");
    write_at(dir.path(), ".dot.csv", b"name\nZorbina Quenzel\n");
    let out = tempfile::TempDir::new().unwrap();
    let output = run_output(dir.path(), out.path());
    assert!(output.status.success());
    assert!(out.path().join("data.csv").exists());
    assert!(!out.path().join(".hidden").exists());
    assert!(!out.path().join("sub/.deep").exists());
    assert!(!out.path().join(".dot.csv").exists());
}

#[cfg(unix)]
#[test]
fn run_keeps_the_permissions_of_a_csv() {
    use std::os::unix::fs::PermissionsExt;
    let dir = other_files_project("");
    let private = dir.path().join("data.csv");
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600)).unwrap();
    let out = tempfile::TempDir::new().unwrap();
    run_project(dir.path(), out.path());
    let mode = std::fs::metadata(out.path().join("data.csv"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn run_copies_a_csv_with_a_245_character_name() {
    let dir = other_files_project("");
    let name = format!("{}.csv", "c".repeat(245));
    write_at(dir.path(), &name, b"name\nZorbina Quenzel\n");
    let out = tempfile::TempDir::new().unwrap();
    run_project(dir.path(), out.path());
    let copy = std::fs::read_to_string(out.path().join(&name)).unwrap();
    assert!(!copy.contains("Zorbina"), "{copy}");
}

#[cfg(unix)]
#[test]
fn run_warns_if_out_is_writable_by_other_users() {
    use std::os::unix::fs::PermissionsExt;
    let dir = other_files_project("");
    let out = tempfile::TempDir::new().unwrap();
    std::fs::set_permissions(out.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    let output = run_output(dir.path(), out.path());
    assert!(output.status.success());
    assert!(
        stderr_of(&output)
            .lines()
            .any(|l| l == "warning: --out is writable by other users")
    );
    std::fs::set_permissions(out.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = run_output(dir.path(), out.path());
    assert!(!stderr_of(&output).contains("writable by other users"));
}

/// A project with `data.csv`, `archive/old.csv`, and `archive/keep/a.csv`.
/// Rule 1 excludes `archive/**`; `rules` adds more rule lines.
fn excluded_project(setting: &str, rules: &str) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    write_at(root, "secrets/pseudofy.key", format!("{KEY}\n").as_bytes());
    let config = format!(
        "language = \"en\"\n{setting}\n[[rule]]\nfiles = [\"**/*.csv\", \"**/*.xlsx\"]\n\
         files_excluded = [\"archive/**\"]\nfull_name = [\"name\"]\n{rules}"
    );
    write_at(root, "config/pseudofy.toml", config.as_bytes());
    write_at(root, "data.csv", b"name\nZorbina Quenzel\n");
    write_at(root, "archive/old.csv", b"name\nTamsorin Velgrad\n");
    write_at(root, "archive/keep/a.csv", b"label\nQuorvan Brindle\n");
    dir
}

#[test]
fn excluded_files_get_no_copy_by_default_and_are_counted() {
    let dir = excluded_project("", "");
    let out = tempfile::TempDir::new().unwrap();
    let output = run_output(dir.path(), out.path());
    assert!(output.status.success(), "{}", stderr_of(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("excluded: 2"), "{stdout}");
    assert!(!out.path().join("archive").exists());
    let csv = std::fs::read_to_string(out.path().join("data.csv")).unwrap();
    assert!(!csv.contains("Zorbina"), "{csv}");
}

#[test]
fn excluded_files_are_copied_unchanged_with_copy_other_files() {
    let dir = excluded_project("copy_other_files = true\n", "");
    let out = tempfile::TempDir::new().unwrap();
    let output = run_output(dir.path(), out.path());
    assert!(output.status.success(), "{}", stderr_of(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("archive/old.csv: copied unchanged"),
        "{stdout}"
    );
    assert!(stdout.contains("excluded: 2"), "{stdout}");
    assert_eq!(
        std::fs::read(out.path().join("archive/old.csv")).unwrap(),
        b"name\nTamsorin Velgrad\n"
    );
    let csv = std::fs::read_to_string(out.path().join("data.csv")).unwrap();
    assert!(!csv.contains("Zorbina"), "{csv}");
}

#[test]
fn a_file_excluded_by_one_rule_gets_the_columns_of_another_rule() {
    let rule = "[[rule]]\nfiles = [\"archive/keep/*.csv\"]\nfull_name = [\"label\"]\n";
    let dir = excluded_project("", rule);
    let out = tempfile::TempDir::new().unwrap();
    let output = run_output(dir.path(), out.path());
    assert!(output.status.success(), "{}", stderr_of(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("excluded: 1"), "{stdout}");
    let csv = std::fs::read_to_string(out.path().join("archive/keep/a.csv")).unwrap();
    assert!(csv.starts_with("label\n"), "{csv}");
    assert!(!csv.contains("Quorvan"), "{csv}");
}

#[test]
fn files_as_a_string_or_an_empty_array_fails_with_a_hint() {
    for (files, message) in [
        (
            "\"**/*.csv\"",
            "files must be an array, for example files = [\"**/*.csv\"]",
        ),
        ("[]", "files must have one or more globs"),
    ] {
        let dir = other_files_project("");
        let config =
            format!("language = \"en\"\n[[rule]]\nfiles = {files}\nfull_name = [\"name\"]\n");
        write_at(dir.path(), "config/pseudofy.toml", config.as_bytes());
        let out = tempfile::TempDir::new().unwrap();
        let output = run_output(dir.path(), out.path());
        assert!(!output.status.success());
        let stderr = stderr_of(&output);
        assert!(stderr.contains(message), "{stderr}");
    }
}

#[test]
fn an_invalid_excluded_glob_fails_and_names_the_glob() {
    let dir = other_files_project("");
    let config = "language = \"en\"\n[[rule]]\nfiles = [\"**/*.csv\"]\nfiles_excluded = [\"a/[b\"]\nfull_name = [\"name\"]\n";
    write_at(dir.path(), "config/pseudofy.toml", config.as_bytes());
    let out = tempfile::TempDir::new().unwrap();
    let output = run_output(dir.path(), out.path());
    assert!(!output.status.success());
    let stderr = stderr_of(&output);
    assert!(stderr.contains("invalid glob \"a/[b\""), "{stderr}");
}

#[cfg(unix)]
#[test]
fn an_excluded_file_is_never_overwritten_by_an_output() {
    // `out/data.csv` is the same file as the excluded `archive/old.csv`.
    let dir = excluded_project("", "");
    let out = tempfile::TempDir::new().unwrap();
    std::fs::hard_link(
        dir.path().join("archive/old.csv"),
        out.path().join("data.csv"),
    )
    .unwrap();
    let output = run_output(dir.path(), out.path());
    assert!(!output.status.success());
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("data.csv: the output path is an input file"),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read(dir.path().join("archive/old.csv")).unwrap(),
        b"name\nTamsorin Velgrad\n"
    );
}
