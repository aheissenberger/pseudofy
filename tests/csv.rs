//! End-to-end tests for `pseudofy run` on CSV files (spec 0002).
//! All data is invented. Each test works in a temporary directory.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions may panic"
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;
use tempfile::TempDir;

const KEY_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const KEY_B: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

const REAL_TOKENS: [&str; 4] = ["zorbina", "quenzel", "tamsorin", "velgrad"];
const REAL_EMAIL_PARTS: [&str; 5] = ["acme.test", "other.test", "a@", "b@", "c@"];

fn pseudofy() -> Command {
    Command::cargo_bin("pseudofy").unwrap()
}

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read_to_string(path).unwrap()
}

fn config(email_subdomains: Option<bool>) -> String {
    let mut text = String::from("language = \"de\"\n");
    if let Some(value) = email_subdomains {
        text.push_str("email_subdomains = ");
        text.push_str(&value.to_string());
        text.push('\n');
    }
    text.push_str(
        "\n[[rule]]\nfiles = \"**/*.csv\"\nfirst_name = [\"first_name\"]\n\
         last_name = [\"last_name\"]\nfull_name = [\"name\"]\nemail = [\"email\"]\n",
    );
    text
}

/// Creates a project with a key and a config and copies the fixtures into it.
fn project(key: &str, email_subdomains: Option<bool>, fixtures: &[&str]) -> TempDir {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "secrets/pseudofy.key", &format!("{key}\n"));
    write_file(
        dir.path(),
        "config/pseudofy.toml",
        &config(email_subdomains),
    );
    for name in fixtures {
        write_file(dir.path(), name, &fixture(name));
    }
    dir
}

fn write_file(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn run(project: &Path, out: &Path) -> assert_cmd::assert::Assert {
    pseudofy()
        .args(["run", "--project"])
        .arg(project)
        .arg("--out")
        .arg(out)
        .assert()
}

/// Runs successfully into a fresh output directory and returns it with stdout.
fn run_ok(project: &Path) -> (TempDir, String) {
    let out = TempDir::new().unwrap();
    let assert = run(project, out.path()).success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    (out, stdout)
}

fn read(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative)).unwrap()
}

/// Splits CSV text without quoted delimiters into rows of cells.
fn rows(text: &str, delimiter: char) -> Vec<Vec<String>> {
    text.lines()
        .map(|line| {
            line.split(delimiter)
                .map(|cell| cell.trim_matches('"').to_string())
                .collect()
        })
        .collect()
}

fn column(table: &[Vec<String>], header: &str) -> Vec<String> {
    let index = table[0].iter().position(|h| h == header).unwrap();
    table[1..].iter().map(|row| row[index].clone()).collect()
}

/// Returns the words of `text` that contain an at sign.
fn emails_in(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '"')
        .filter(|word| word.contains('@'))
        .map(str::to_string)
        .collect()
}

fn is_slug(part: &str) -> bool {
    !part.is_empty()
        && part
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Checks a fake email and returns the subdomain label, if any.
fn check_fake_email(email: &str, subdomains: bool) -> Option<String> {
    let (local, domain) = email.split_once('@').unwrap();
    let (first, last) = local.split_once('.').unwrap();
    assert!(is_slug(first) && is_slug(last), "bad local part: {email}");
    if subdomains {
        let label = domain.strip_suffix(".example.com").unwrap();
        assert!(
            (6..=10).contains(&label.len()) && label.chars().all(|c| c.is_ascii_lowercase()),
            "bad label: {email}"
        );
        Some(label.to_string())
    } else {
        assert_eq!(domain, "example.com", "bad domain: {email}");
        None
    }
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn snapshot_relative(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    snapshot(root)
        .into_iter()
        .map(|(path, data)| (path.strip_prefix(root).unwrap().to_path_buf(), data))
        .collect()
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

// --- Names ---------------------------------------------------------------

#[test]
fn same_name_in_two_files_gets_the_same_pseudonym() {
    let dir = project(KEY_A, None, &["people_a.csv", "people_b.csv"]);
    let (out, _) = run_ok(dir.path());
    let a = rows(&read(out.path(), "people_a.csv"), ',');
    let b = rows(&read(out.path(), "people_b.csv"), ',');
    assert_eq!(column(&a, "first_name")[0], column(&b, "first_name")[0]);
    assert_eq!(column(&a, "last_name")[0], column(&b, "last_name")[0]);
    assert_eq!(column(&a, "name")[0], column(&b, "name")[0]);
}

#[test]
fn first_name_and_full_name_share_the_first_name_pseudonym() {
    let dir = project(KEY_A, None, &["people_a.csv"]);
    let (out, _) = run_ok(dir.path());
    let table = rows(&read(out.path(), "people_a.csv"), ',');
    for i in 0..2 {
        let full = &column(&table, "name")[i];
        let parts: Vec<&str> = full.split(' ').collect();
        assert_eq!(parts.len(), 2, "full name keeps two tokens: {full}");
        assert_eq!(parts[0], column(&table, "first_name")[i]);
        assert_eq!(parts[1], column(&table, "last_name")[i]);
    }
}

#[test]
fn last_comma_first_is_supported_and_shares_pseudonyms() {
    let dir = project(KEY_A, None, &["people_a.csv", "last_first.csv"]);
    let (out, _) = run_ok(dir.path());
    let people = rows(&read(out.path(), "people_a.csv"), ',');
    let table = rows(&read(out.path(), "last_first.csv"), ';');
    let full = &column(&table, "name")[0];
    let (last, first) = full.split_once(", ").unwrap();
    assert_eq!(first, column(&people, "first_name")[0]);
    assert_eq!(last, column(&people, "last_name")[0]);
}

#[test]
fn name_case_style_is_kept() {
    let dir = project(KEY_A, None, &["people_a.csv", "upper.csv"]);
    let (out, _) = run_ok(dir.path());
    let people = rows(&read(out.path(), "people_a.csv"), ',');
    let upper = rows(&read(out.path(), "upper.csv"), ',');
    let first = &column(&upper, "first_name")[0];
    let last = &column(&upper, "last_name")[0];
    assert_eq!(first, &first.to_uppercase());
    assert_eq!(last, &last.to_uppercase());
    assert_eq!(
        first.to_lowercase(),
        column(&people, "first_name")[0].to_lowercase()
    );
    assert_eq!(
        last.to_lowercase(),
        column(&people, "last_name")[0].to_lowercase()
    );
}

// --- Determinism and keys ------------------------------------------------

#[test]
fn two_runs_with_one_key_give_byte_equal_output() {
    let dir = project(KEY_A, None, &["people_a.csv", "people_b.csv"]);
    let (first, _) = run_ok(dir.path());
    let (second, _) = run_ok(dir.path());
    assert_eq!(
        snapshot_relative(first.path()),
        snapshot_relative(second.path())
    );
    assert!(!snapshot_relative(first.path()).is_empty());
}

#[test]
fn two_keys_give_different_output() {
    let one = project(KEY_A, None, &["people_a.csv"]);
    let two = project(KEY_B, None, &["people_a.csv"]);
    let (out_one, _) = run_ok(one.path());
    let (out_two, _) = run_ok(two.path());
    assert_ne!(
        read(out_one.path(), "people_a.csv"),
        read(out_two.path(), "people_a.csv")
    );
}

// --- Emails ----------------------------------------------------------------

#[test]
fn without_subdomains_every_email_ends_in_example_com() {
    let dir = project(KEY_A, Some(false), &["people_a.csv"]);
    let (out, _) = run_ok(dir.path());
    let text = read(out.path(), "people_a.csv");
    let table = rows(&text, ',');
    let emails = column(&table, "email");
    assert!(emails.iter().all(|e| e.ends_with("@example.com")));
    let found = emails_in(&text);
    assert_eq!(found.len(), 3, "a, b and the one in the free text");
    for email in found {
        check_fake_email(&email, false);
    }
}

#[test]
fn default_gives_one_subdomain_per_real_domain_ignoring_case() {
    let dir = project(KEY_A, None, &["people_a.csv"]);
    let (out, _) = run_ok(dir.path());
    let table = rows(&read(out.path(), "people_a.csv"), ',');
    let emails = column(&table, "email");
    let label_a = check_fake_email(&emails[0], true).unwrap();
    let label_b = check_fake_email(&emails[1], true).unwrap();
    let in_text = emails_in(&column(&table, "note")[0]);
    assert_eq!(in_text.len(), 1);
    let label_c = check_fake_email(&in_text[0], true).unwrap();
    assert_eq!(label_a, label_b, "acme.test and ACME.test");
    assert_ne!(label_a, label_c, "acme.test and other.test");
}

#[test]
fn explicit_subdomains_true_behaves_like_the_default() {
    let dir = project(KEY_A, Some(true), &["people_a.csv"]);
    let (out, _) = run_ok(dir.path());
    let text = read(out.path(), "people_a.csv");
    let found = emails_in(&text);
    assert_eq!(found.len(), 3);
    for email in found {
        check_fake_email(&email, true);
    }
}

#[test]
fn same_email_in_two_files_gets_the_same_fake_email() {
    let dir = project(KEY_A, None, &["people_a.csv", "people_b.csv"]);
    let (out, _) = run_ok(dir.path());
    let a = rows(&read(out.path(), "people_a.csv"), ',');
    let b = rows(&read(out.path(), "people_b.csv"), ',');
    let free_text = emails_in(&column(&a, "note")[0]);
    assert_eq!(free_text[0], column(&b, "email")[0], "c@other.test");
}

#[test]
fn emails_in_free_text_columns_are_replaced() {
    let dir = project(KEY_A, None, &["people_a.csv"]);
    let (out, _) = run_ok(dir.path());
    let table = rows(&read(out.path(), "people_a.csv"), ',');
    let note = &column(&table, "note")[0];
    assert!(note.starts_with("write to "));
    assert!(note.ends_with(" today"));
    assert!(!note.contains("other.test"));
}

// --- No leaks ----------------------------------------------------------------

#[test]
fn no_real_name_or_email_appears_in_the_output() {
    let dir = project(KEY_A, None, &["people_a.csv", "people_b.csv", "upper.csv"]);
    let (out, stdout) = run_ok(dir.path());
    for name in ["people_a.csv", "people_b.csv", "upper.csv"] {
        let text = read(out.path(), name);
        let lower = text.to_lowercase();
        for part in REAL_EMAIL_PARTS {
            assert!(!lower.contains(part), "{name} leaks {part}");
        }
        for token in tokens(&text) {
            assert!(
                !REAL_TOKENS.contains(&token.as_str()),
                "{name} leaks {token}"
            );
        }
    }
    let lower = stdout.to_lowercase();
    for token in REAL_TOKENS.iter().chain(REAL_EMAIL_PARTS.iter()) {
        assert!(!lower.contains(token), "stdout leaks {token}");
    }
}

#[test]
fn output_never_contains_the_project_key() {
    let dir = project(KEY_A, None, &["people_a.csv"]);
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path()).success();
    let output = assert.get_output();
    assert!(!String::from_utf8_lossy(&output.stdout).contains(KEY_A));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(KEY_A));
    assert!(!read(out.path(), "people_a.csv").contains(KEY_A));
}

#[test]
fn pseudonym_differs_from_the_real_value_in_every_name_cell() {
    let dir = project(KEY_A, None, &["people_a.csv"]);
    let (out, _) = run_ok(dir.path());
    let before = rows(&fixture("people_a.csv"), ',');
    let after = rows(&read(out.path(), "people_a.csv"), ',');
    for header in ["first_name", "last_name", "name", "email"] {
        for (real, fake) in column(&before, header).iter().zip(column(&after, header)) {
            assert_ne!(real.to_lowercase(), fake.to_lowercase(), "{header}");
        }
    }
}

// --- Structure -------------------------------------------------------------

fn variant(base: &str, delimiter: char, line_ending: &str) -> String {
    let mut text = String::new();
    for line in base.lines() {
        text.push_str(&line.replace(',', &delimiter.to_string()));
        text.push_str(line_ending);
    }
    text
}

#[test]
fn output_keeps_shape_header_delimiter_and_line_ending() {
    for delimiter in [',', ';', '\t'] {
        for line_ending in ["\n", "\r\n"] {
            let input = variant(&fixture("people_a.csv"), delimiter, line_ending);
            let dir = project(KEY_A, None, &[]);
            write_file(dir.path(), "data.csv", &input);
            let (out, _) = run_ok(dir.path());
            let output = read(out.path(), "data.csv");
            let label = format!("delimiter {delimiter:?}, ending {line_ending:?}");

            let before = rows(&input, delimiter);
            let after = rows(&output, delimiter);
            assert_eq!(before.len(), after.len(), "rows, {label}");
            for (b, a) in before.iter().zip(&after) {
                assert_eq!(b.len(), a.len(), "columns, {label}");
            }
            assert_eq!(before[0], after[0], "header, {label}");
            assert_eq!(
                input.lines().next(),
                output.lines().next(),
                "header line, {label}"
            );
            assert_eq!(
                input.matches("\r\n").count(),
                output.matches("\r\n").count(),
                "CRLF count, {label}"
            );
            assert_eq!(
                input.matches('\n').count(),
                output.matches('\n').count(),
                "LF count, {label}"
            );
        }
    }
}

#[test]
fn input_files_are_unchanged_after_run() {
    let dir = project(
        KEY_A,
        None,
        &["people_a.csv", "people_b.csv", "last_first.csv"],
    );
    let before = snapshot(dir.path());
    let (_out, _) = run_ok(dir.path());
    assert_eq!(before, snapshot(dir.path()));
}

#[test]
fn files_in_subdirectories_are_written_to_the_same_relative_path() {
    let dir = project(KEY_A, None, &[]);
    write_file(dir.path(), "team/east/people.csv", &fixture("people_a.csv"));
    let (out, _) = run_ok(dir.path());
    let table = rows(&read(out.path(), "team/east/people.csv"), ',');
    assert_eq!(table.len(), 3);
}

// --- Summary and errors ------------------------------------------------------

#[test]
fn summary_line_reports_files_replaced_values_and_collisions() {
    let dir = project(KEY_A, None, &["people_a.csv", "people_b.csv"]);
    let (_out, stdout) = run_ok(dir.path());
    let line = stdout
        .lines()
        .find(|l| l.starts_with("files: "))
        .unwrap_or_else(|| panic!("no summary line in {stdout:?}"));
    let parts: Vec<&str> = line.split(", ").collect();
    assert_eq!(parts.len(), 3, "{line}");
    assert_eq!(parts[0], "files: 2");
    let replaced: u32 = parts[1]
        .strip_prefix("replaced values: ")
        .unwrap()
        .parse()
        .unwrap();
    assert!(replaced > 0);
    let _collisions: u32 = parts[2]
        .strip_prefix("collisions: ")
        .unwrap()
        .parse()
        .unwrap();
}

#[test]
fn missing_key_fails_and_points_to_init() {
    let dir = project(KEY_A, None, &["people_a.csv"]);
    fs::remove_file(dir.path().join("secrets/pseudofy.key")).unwrap();
    let out = TempDir::new().unwrap();
    run(dir.path(), out.path())
        .failure()
        .stderr(contains("pseudofy init"));
}

#[test]
fn out_equal_to_project_fails() {
    let dir = project(KEY_A, None, &["people_a.csv"]);
    let before = snapshot(dir.path());
    run(dir.path(), dir.path()).failure();
    assert_eq!(before, snapshot(dir.path()));
}

#[test]
fn out_inside_project_works_and_is_not_read_again() {
    let dir = project(KEY_A, None, &["people_a.csv", "people_b.csv"]);
    let out = dir.path().join("out");
    let first = run(dir.path(), &out).success();
    let first_stdout = String::from_utf8(first.get_output().stdout.clone()).unwrap();
    assert!(first_stdout.contains("files: 2,"), "{first_stdout}");
    let written = snapshot(&out);
    assert_eq!(written.len(), 2);

    let second = run(dir.path(), &out).success();
    let second_stdout = String::from_utf8(second.get_output().stdout.clone()).unwrap();
    assert!(second_stdout.contains("files: 2,"), "{second_stdout}");
    assert!(!out.join("out").exists(), "out was read as input");
    assert_eq!(written, snapshot(&out));
}

#[test]
fn files_in_secrets_and_config_are_not_processed() {
    let dir = project(KEY_A, None, &["people_a.csv"]);
    write_file(dir.path(), "secrets/extra.csv", &fixture("people_a.csv"));
    write_file(dir.path(), "config/extra.csv", &fixture("people_a.csv"));
    let (out, stdout) = run_ok(dir.path());
    assert!(stdout.contains("files: 1,"), "{stdout}");
    assert!(!out.path().join("secrets").exists());
    assert!(!out.path().join("config").exists());
    assert!(out.path().join("people_a.csv").exists());
}

#[test]
fn run_does_not_print_real_values_on_stderr() {
    let dir = project(KEY_A, None, &["people_a.csv"]);
    let out = TempDir::new().unwrap();
    run(dir.path(), out.path())
        .success()
        .stderr(contains("Zorbina").not().and(contains("acme").not()));
}

// --- Regression tests for review findings ------------------------------------

const LEAK_WORDS: [&str; 8] = [
    "zorbel",
    "bäckerei",
    "müller",
    "anna",
    "jürgen",
    "localhost",
    "quenzel",
    "acme",
];

fn stderr_text(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stderr).to_lowercase()
}

fn assert_no_leak(text: &str, what: &str) {
    let lower = text.to_lowercase();
    for word in LEAK_WORDS {
        assert!(!lower.contains(word), "{what} leaks {word}");
    }
}

/// Runs successfully, checks stderr for leaks, returns the output directory.
fn run_clean(project: &Path) -> TempDir {
    let out = TempDir::new().unwrap();
    let assert = run(project, out.path()).success();
    assert_no_leak(&stderr_text(&assert), "stderr");
    out
}

fn strip_angle(word: &str) -> &str {
    word.trim_matches(|c| c == '<' || c == '>')
}

#[test]
fn email_column_values_the_pattern_may_not_match_are_replaced() {
    let dir = project(KEY_A, None, &[]);
    write_file(
        dir.path(),
        "data.csv",
        "name,email\nPerson One,info@bäckerei-zorbel.test\nPerson Two,zorbel@localhost\n",
    );
    let out = run_clean(dir.path());
    let text = read(out.path(), "data.csv");
    assert_no_leak(&text, "output");
    let table = rows(&text, ',');
    for (real, fake) in ["info@bäckerei-zorbel.test", "zorbel@localhost"]
        .iter()
        .zip(column(&table, "email"))
    {
        assert_ne!(*real, fake);
        assert!(fake.ends_with("example.com"), "{fake}");
        assert!(fake.is_ascii(), "{fake}");
    }
}

#[test]
fn unicode_email_in_free_text_is_replaced_without_leaking_the_name() {
    let dir = project(KEY_A, None, &[]);
    write_file(
        dir.path(),
        "data.csv",
        "id,note\n1,please write to anna.müller@acme.test soon\n",
    );
    let out = run_clean(dir.path());
    let text = read(out.path(), "data.csv");
    assert_no_leak(&text, "output");
    let found = emails_in(&text);
    assert_eq!(found.len(), 1, "{text}");
    assert!(found[0].ends_with("example.com"), "{}", found[0]);
}

#[test]
fn email_inside_a_full_name_cell_matches_the_email_column() {
    let dir = project(KEY_A, None, &[]);
    write_file(
        dir.path(),
        "data.csv",
        "name,email\nZorbel Quenzel <zorbel@acme.test>,other@other.test\nPerson Two,zorbel@acme.test\n",
    );
    let out = run_clean(dir.path());
    let text = read(out.path(), "data.csv");
    assert_no_leak(&text, "output");
    let table = rows(&text, ',');
    let in_name = emails_in(&column(&table, "name")[0]);
    assert_eq!(in_name.len(), 1, "{text}");
    assert_eq!(strip_angle(&in_name[0]), column(&table, "email")[1]);
}

#[test]
fn nfd_and_nfc_forms_of_a_name_give_the_same_pseudonym() {
    let dir = project(KEY_A, None, &[]);
    let nfd = "Ju\u{308}rgen";
    let nfc = "J\u{fc}rgen";
    assert_ne!(nfd, nfc);
    write_file(
        dir.path(),
        "data.csv",
        &format!("first_name,last_name\n{nfd},Quenzel\n{nfc},Quenzel\n"),
    );
    let out = run_clean(dir.path());
    let table = rows(&read(out.path(), "data.csv"), ',');
    let first = column(&table, "first_name");
    assert_eq!(first[0], first[1]);
    assert_ne!(first[0].to_lowercase(), nfc.to_lowercase());
}

#[test]
fn one_letter_initials_never_stay_equal_to_themselves() {
    for key in [KEY_A, KEY_B] {
        let dir = project(key, None, &[]);
        let mut input = String::from("first_name,last_name\n");
        for initial in ["A", "Z", "Q", "J", "M", "T"] {
            input.push_str(initial);
            input.push(',');
            input.push_str(initial);
            input.push('\n');
        }
        write_file(dir.path(), "data.csv", &input);
        let out = run_clean(dir.path());
        let before = rows(&input, ',');
        let after = rows(&read(out.path(), "data.csv"), ',');
        for header in ["first_name", "last_name"] {
            for (real, fake) in column(&before, header).iter().zip(column(&after, header)) {
                assert_ne!(real.to_lowercase(), fake.to_lowercase(), "{header}");
            }
        }
    }
}

#[test]
fn uppercase_csv_extension_matches_the_default_rule() {
    let dir = project(KEY_A, None, &[]);
    write_file(
        dir.path(),
        "PEOPLE.CSV",
        "first_name,last_name\nZorbina,Quenzel\n",
    );
    let out = run_clean(dir.path());
    let files = snapshot_relative(out.path());
    assert_eq!(files.len(), 1, "{:?}", files.keys());
    let text = String::from_utf8(files.into_values().next().unwrap()).unwrap();
    assert_no_leak(&text, "output");
    assert_ne!(text, "first_name,last_name\nZorbina,Quenzel\n");
}

#[test]
fn blank_lines_are_kept_at_their_position() {
    let dir = project(KEY_A, None, &[]);
    let input = "first_name,last_name\nZorbina,Quenzel\n\nTamsorin,Velgrad\n";
    write_file(dir.path(), "data.csv", input);
    let out = run_clean(dir.path());
    let output = read(out.path(), "data.csv");
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), input.lines().count(), "{output:?}");
    assert_eq!(lines[2], "");
    assert!(!lines[1].is_empty() && !lines[3].is_empty());
}

#[test]
fn header_bytes_and_missing_final_newline_are_kept() {
    let dir = project(KEY_A, None, &[]);
    let input = "\"first_name\",\"last_name\"\nZorbina,Quenzel";
    write_file(dir.path(), "data.csv", input);
    let out = run_clean(dir.path());
    let output = read(out.path(), "data.csv");
    assert_eq!(
        output.lines().next(),
        Some("\"first_name\",\"last_name\""),
        "{output:?}"
    );
    assert!(!output.ends_with('\n'), "{output:?}");
    assert!(!output.to_lowercase().contains("zorbina"), "{output:?}");
}

#[cfg(unix)]
#[test]
fn output_symlink_to_an_input_file_never_changes_the_input() {
    let dir = project(KEY_A, None, &[]);
    let input = "first_name,last_name\nZorbina,Quenzel\n";
    write_file(dir.path(), "data.csv", input);
    let out = TempDir::new().unwrap();
    std::os::unix::fs::symlink(dir.path().join("data.csv"), out.path().join("data.csv")).unwrap();
    let assert = run(dir.path(), out.path());
    assert_no_leak(&stderr_text(&assert), "stderr");
    assert_eq!(read(dir.path(), "data.csv"), input);
}

#[test]
fn out_inside_secrets_fails() {
    let dir = project(KEY_A, None, &["people_a.csv"]);
    let out = dir.path().join("secrets/out");
    let assert = run(dir.path(), &out).failure();
    assert_no_leak(&stderr_text(&assert), "stderr");
    assert!(!out.join("people_a.csv").exists());
}

#[test]
fn file_without_a_matching_rule_warns_and_still_replaces_emails() {
    let dir = project(KEY_A, None, &[]);
    write_file(
        dir.path(),
        "config/pseudofy.toml",
        "language = \"de\"\n\n[[rule]]\nfiles = \"only/*.csv\"\nfirst_name = [\"first_name\"]\n",
    );
    write_file(
        dir.path(),
        "other/data.csv",
        "first_name,email\nZorbina,zorbel@acme.test\n",
    );
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path()).success();
    let stderr = stderr_text(&assert);
    assert!(stderr.contains("warning"), "{stderr}");
    assert_no_leak(&stderr, "stderr");
    let text = read(out.path(), "other/data.csv");
    assert!(!text.to_lowercase().contains("acme"), "{text}");
    assert!(!text.to_lowercase().contains("zorbel"), "{text}");
    assert!(text.contains("example.com"), "{text}");
}

#[test]
fn stderr_never_contains_real_values_from_the_fixtures() {
    let dir = project(KEY_A, None, &["people_a.csv", "people_b.csv", "upper.csv"]);
    write_file(
        dir.path(),
        "extra.csv",
        "name,email\nZorbel Quenzel <zorbel@acme.test>,info@bäckerei-zorbel.test\n",
    );
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path());
    let stderr = stderr_text(&assert);
    assert_no_leak(&stderr, "stderr");
    for part in REAL_TOKENS.iter().chain(REAL_EMAIL_PARTS.iter()) {
        assert!(!stderr.contains(part), "stderr leaks {part}");
    }
}

#[test]
fn apostrophe_email_gives_one_fake_address_in_column_and_free_text() {
    let dir = project(KEY_A, None, &[]);
    write_file(
        dir.path(),
        "data.csv",
        "email,notes\nmary.o'neil@acme.test,Write to mary.o'neil@acme.test today\n",
    );
    let out = run_clean(dir.path());
    let text = read(out.path(), "data.csv");
    let lower = text.to_lowercase();
    for part in ["mary", "neil", "o'n"] {
        assert!(!lower.contains(part), "output leaks {part}: {text}");
    }
    assert_no_leak(&text, "output");
    let table = rows(&text, ',');
    let in_column = column(&table, "email");
    let in_text = emails_in(&column(&table, "notes")[0]);
    assert_eq!(in_text.len(), 1, "{text}");
    assert!(in_column[0].ends_with("example.com"), "{text}");
    assert!(in_text[0].ends_with("example.com"), "{text}");
    assert_eq!(in_column[0], in_text[0]);
}

#[test]
fn display_name_with_address_in_email_column_keeps_its_shape() {
    let dir = project(KEY_A, None, &[]);
    write_file(
        dir.path(),
        "data.csv",
        "email\nZorbel Quenzel <zorbel@acme.test>\n",
    );
    let out = run_clean(dir.path());
    let text = read(out.path(), "data.csv");
    let lower = text.to_lowercase();
    for part in ["zorbel", "quenzel"] {
        assert!(!lower.contains(part), "output leaks {part}: {text}");
    }
    assert_no_leak(&text, "output");
    let table = rows(&text, ',');
    let cell = &column(&table, "email")[0];
    let words: Vec<&str> = cell.split(' ').collect();
    assert_eq!(words.len(), 3, "{cell}");
    assert!(words[0].chars().all(char::is_alphabetic), "{cell}");
    assert!(words[1].chars().all(char::is_alphabetic), "{cell}");
    assert!(
        words[2].starts_with('<') && words[2].ends_with('>'),
        "{cell}"
    );
    assert!(words[2].contains('@'), "{cell}");
    assert!(strip_angle(words[2]).ends_with("example.com"), "{cell}");
}

#[test]
fn trailing_dot_after_address_in_name_cell_is_kept_and_address_matches_email_column() {
    let dir = project(KEY_A, None, &[]);
    write_file(
        dir.path(),
        "data.csv",
        "name,email\nZorbel Quenzel zorbel@acme.test.,zorbel@acme.test\n",
    );
    let out = run_clean(dir.path());
    let text = read(out.path(), "data.csv");
    assert_no_leak(&text, "output");
    let table = rows(&text, ',');
    let name_cell = &column(&table, "name")[0];
    assert!(name_cell.ends_with('.'), "{name_cell}");
    let in_name = emails_in(name_cell);
    assert_eq!(in_name.len(), 1, "{text}");
    assert_eq!(
        in_name[0].trim_end_matches('.'),
        column(&table, "email")[0],
        "{text}"
    );
}

#[test]
fn cr_only_line_endings_are_kept_and_names_are_replaced() {
    let dir = project(KEY_A, None, &[]);
    let input = "first_name,last_name\rZorbina,Quenzel\r\rTamsorin,Velgrad\r";
    write_file(dir.path(), "data.csv", input);
    let out = run_clean(dir.path());
    let output = read(out.path(), "data.csv");
    assert!(!output.contains('\n'), "{output:?}");
    assert_eq!(
        output.matches('\r').count(),
        input.matches('\r').count(),
        "{output:?}"
    );
    assert!(output.contains("\r\r"), "{output:?}");
    assert_no_leak(&output, "output");
}

#[test]
fn quoted_newline_header_and_semicolon_delimiter_are_kept() {
    let dir = project(KEY_A, None, &[]);
    let header = "\"Kunden\nNr\";first_name;last_name\n";
    write_file(
        dir.path(),
        "data.csv",
        &format!("{header}1;Zorbina;Quenzel\n"),
    );
    let out = run_clean(dir.path());
    let output = read(out.path(), "data.csv");
    assert!(output.starts_with(header), "{output:?}");
    assert_no_leak(&output, "output");
    let data_line = output[header.len()..].trim_end();
    assert_eq!(data_line.split(';').count(), 3, "{output:?}");
    assert!(!data_line.contains(','), "{output:?}");
    assert!(data_line.starts_with("1;"), "{output:?}");
}

#[test]
fn partial_headers_give_no_warning() {
    let dir = project(KEY_A, None, &[]);
    write_file(
        dir.path(),
        "data.csv",
        "first_name,last_name\nZorbina,Quenzel\n",
    );
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path()).success();
    let stderr = stderr_text(&assert);
    assert!(!stderr.contains("warning"), "{stderr}");
}

#[test]
fn rule_without_any_header_warns_once() {
    let dir = project(KEY_A, None, &[]);
    write_file(dir.path(), "data.csv", "id,note\n1,x\n");
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path()).success();
    let stderr = stderr_text(&assert);
    assert_eq!(stderr.matches("warning").count(), 1, "{stderr}");
    assert!(stderr.contains("data.csv"), "{stderr}");
}

#[test]
fn a_bad_file_is_reported_and_the_other_files_are_written() {
    let dir = project(KEY_A, None, &[]);
    let bad = dir.path().join("a_bad.csv");
    fs::write(&bad, b"first_name\nZorbina\xff\n").unwrap();
    write_file(dir.path(), "b_good.csv", "first_name\nTamsorin\n");
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path()).failure();
    let stderr = stderr_text(&assert);
    assert!(stderr.contains("a_bad.csv"), "{stderr}");
    assert!(stderr.contains("1 file(s) failed"), "{stderr}");
    assert_no_leak(&stderr, "stderr");
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("files: 1,"), "{stdout}");
    assert!(!out.path().join("a_bad.csv").exists());
    let good = read(out.path(), "b_good.csv");
    assert!(!good.to_lowercase().contains("tamsorin"), "{good}");
}

#[cfg(unix)]
#[test]
fn symlinked_input_is_skipped_with_a_warning() {
    let dir = project(KEY_A, None, &[]);
    let target = TempDir::new().unwrap();
    write_file(target.path(), "linked.csv", "first_name\nZorbina\n");
    std::os::unix::fs::symlink(
        target.path().join("linked.csv"),
        dir.path().join("linked.csv"),
    )
    .unwrap();
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path()).success();
    let stderr = stderr_text(&assert);
    assert!(stderr.contains("linked.csv: symlink skipped"), "{stderr}");
    assert!(!out.path().join("linked.csv").exists());
}

// ---- Windows-1252 input (spec 0009) ----

const WIN_FIXTURE: &str = "windows-1252.csv";
const WIN_REAL_VALUES: [&str; 8] = [
    "Jörg",
    "Müller",
    "Bärbel",
    "Weiß",
    "jm@acme.test",
    "bw@acme.test",
    "acme.test",
    "Grüße",
];

fn win_fixture_bytes() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(WIN_FIXTURE);
    fs::read(path).unwrap()
}

/// Creates a project with a key, a config, and the given raw files.
fn raw_project(files: &[(&str, &[u8])]) -> TempDir {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "secrets/pseudofy.key", &format!("{KEY_A}\n"));
    write_file(dir.path(), "config/pseudofy.toml", &config(None));
    for (name, bytes) in files {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    dir
}

fn decode_1252(bytes: &[u8]) -> String {
    let (text, had_errors) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes);
    assert!(!had_errors, "copy is not clean Windows-1252");
    text.into_owned()
}

fn encode_1252(text: &str) -> Vec<u8> {
    let (bytes, _, had_errors) = encoding_rs::WINDOWS_1252.encode(text);
    assert!(!had_errors, "text is not Windows-1252");
    bytes.into_owned()
}

fn win_copy() -> Vec<u8> {
    let project = raw_project(&[(WIN_FIXTURE, &win_fixture_bytes())]);
    let (out, _) = run_ok(project.path());
    fs::read(out.path().join(WIN_FIXTURE)).unwrap()
}

#[test]
fn windows_1252_file_gets_a_windows_1252_copy_with_pseudonyms() {
    let copy = win_copy();
    assert!(!copy.starts_with(&[0xEF, 0xBB, 0xBF]), "copy has a BOM");
    let text = decode_1252(&copy);
    let table = rows(&text, ';');
    for header in ["first_name", "last_name", "name"] {
        for cell in column(&table, header) {
            for real in ["Jörg", "Müller", "Bärbel", "Weiß"] {
                assert!(!cell.contains(real), "real value left in {header}");
            }
        }
    }
    assert!(!text.contains("jm@acme.test") && !text.contains("bw@acme.test"));
}

#[test]
fn same_first_name_in_utf8_and_windows_1252_gets_the_same_pseudonym() {
    let utf8 = "first_name,last_name,name,email,note\nJörg,Müller,Jörg Müller,jm@acme.test,x\n";
    let project = raw_project(&[
        (WIN_FIXTURE, &win_fixture_bytes()),
        ("utf8.csv", utf8.as_bytes()),
    ]);
    let (out, _) = run_ok(project.path());
    let utf8_table = rows(&read(out.path(), "utf8.csv"), ',');
    let win_text = decode_1252(&fs::read(out.path().join(WIN_FIXTURE)).unwrap());
    let win_table = rows(&win_text, ';');
    assert_eq!(
        column(&utf8_table, "first_name")[0],
        column(&win_table, "first_name")[0]
    );
}

#[test]
fn windows_1252_copy_keeps_shape_header_delimiter_and_line_ending() {
    let input = win_fixture_bytes();
    let copy = win_copy();
    let lines = |bytes: &[u8]| -> Vec<Vec<u8>> {
        bytes
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(<[u8]>::to_vec)
            .collect()
    };
    let (in_lines, out_lines) = (lines(&input), lines(&copy));
    assert_eq!(in_lines.len(), out_lines.len(), "row count");
    assert_eq!(in_lines[0], out_lines[0], "header bytes");
    for (a, b) in in_lines.iter().zip(&out_lines) {
        let count = |l: &[u8]| l.split(|x| *x == b';').count();
        assert_eq!(count(a), count(b), "column count");
        assert!(b.ends_with(b"\r"), "line ending is not CRLF");
    }
    let newlines = |bytes: &[u8]| bytes.split(|b| *b == b'\n').count();
    assert_eq!(newlines(&input), newlines(&copy));
}

#[test]
fn windows_1252_file_without_a_value_to_replace_is_byte_equal() {
    let bytes = encode_1252("id;note\r\n1;Grüße aus Köln\r\n");
    let project = raw_project(&[("plain.csv", &bytes)]);
    let (out, _) = run_ok(project.path());
    assert_eq!(fs::read(out.path().join("plain.csv")).unwrap(), bytes);
}

#[test]
fn windows_1252_run_prints_no_real_value() {
    let project = raw_project(&[(WIN_FIXTURE, &win_fixture_bytes())]);
    let out = TempDir::new().unwrap();
    let assert = run(project.path(), out.path()).success();
    let output = assert.get_output();
    for stream in [&output.stderr, &output.stdout] {
        let text = String::from_utf8_lossy(stream);
        for real in WIN_REAL_VALUES {
            assert!(!text.contains(real), "real value in output stream");
            let encoded = encode_1252(real);
            assert!(
                !stream.windows(encoded.len()).any(|w| w == encoded),
                "real value (1252) in output stream"
            );
        }
    }
}
