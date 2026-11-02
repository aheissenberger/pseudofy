//! End-to-end tests for known names in free text (spec 0004).
//! All data is invented. Each test works in a temporary directory.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions may panic"
)]

use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;
use umya_spreadsheet::Workbook;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const LEAK_WORDS: [&str; 4] = ["zorbina", "quenzel", "tamsorin", "velgrad"];

const HEADER: &str = "first_name,last_name,note\n";

fn pseudofy() -> Command {
    Command::cargo_bin("pseudofy").unwrap()
}

fn write_file(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Creates a project with a key and a config. `free_text` is the value of
/// the optional key of the same name.
fn project(free_text: Option<&str>) -> TempDir {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "secrets/pseudofy.key", &format!("{KEY}\n"));
    let fields = "first_name = [\"first_name\"]\nlast_name = [\"last_name\"]\n\
                  full_name = [\"name\"]\nemail = [\"email\"]\n";
    let mut text = String::from("language = \"de\"\n");
    if let Some(mode) = free_text {
        text.push_str("free_text = \"");
        text.push_str(mode);
        text.push_str("\"\n");
    }
    text.push_str("\n[[rule]]\nfiles = \"**/*.csv\"\n");
    text.push_str(fields);
    text.push_str("\n[[rule]]\nfiles = \"**/*.xlsx\"\n");
    text.push_str(fields);
    write_file(dir.path(), "config/pseudofy.toml", &text);
    dir
}

/// Runs successfully into a fresh output directory. Returns it with stdout and stderr.
fn run_ok(project: &Path) -> (TempDir, String, String) {
    let out = TempDir::new().unwrap();
    let assert = pseudofy()
        .args(["run", "--project"])
        .arg(project)
        .arg("--out")
        .arg(out.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    (out, stdout, stderr)
}

/// Reads a CSV file into rows of cells.
fn table(root: &Path, relative: &str) -> Vec<Vec<String>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_path(root.join(relative))
        .unwrap();
    reader
        .records()
        .map(|record| record.unwrap().iter().map(str::to_string).collect())
        .collect()
}

/// The first and last name cells (columns 0 and 1) of a row.
fn fakes(table: &[Vec<String>], row: usize) -> (String, String) {
    (table[row][0].clone(), table[row][1].clone())
}

fn assert_no_leak(text: &str, what: &str) {
    let lower = text.to_lowercase();
    for word in LEAK_WORDS {
        assert!(!lower.contains(word), "{what} leaks {word}");
    }
}

/// A workbook with sheet "Daten": header `note` in A1 and the given notes below.
fn notes_book(notes: &[&str]) -> Workbook {
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    let sheet = book.new_sheet("Daten").unwrap();
    sheet.cell_mut("A1").set_value_string("note");
    for (index, note) in notes.iter().enumerate() {
        sheet
            .cell_mut(format!("A{}", index + 2).as_str())
            .set_value_string(*note);
    }
    book
}

fn save(book: &Workbook, project: &Path, relative: &str) {
    let path = project.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    umya_spreadsheet::writer::xlsx::write(book, path).unwrap();
}

/// The name and text of every part of an XLSX package.
fn package_parts(path: &Path) -> Vec<(String, String)> {
    let bytes = fs::read(path).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut parts = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        if entry.is_dir() {
            continue;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        parts.push((
            entry.name().to_string(),
            String::from_utf8_lossy(&data).into_owned(),
        ));
    }
    parts
}

#[test]
fn full_name_in_a_text_cell_gets_the_pseudonyms_of_the_name_columns() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}Zorbina,Quenzel,Call Zorbina Quenzel\n"),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    let (first, last) = fakes(&t, 1);
    assert_eq!(t[1][2], format!("Call {first} {last}"));
}

#[test]
fn csv_name_is_replaced_in_a_text_cell_of_an_xlsx_file() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}Zorbina,Quenzel,x\n"),
    );
    save(
        &notes_book(&["Meeting with Zorbina Quenzel"]),
        dir.path(),
        "notes.xlsx",
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    let (first, last) = fakes(&t, 1);
    let book = umya_spreadsheet::reader::xlsx::read(out.path().join("notes.xlsx")).unwrap();
    let cell = book.sheet_by_name("Daten").unwrap().value("A2").clone();
    assert_eq!(cell, format!("Meeting with {first} {last}"));
}

#[test]
fn mode_full_replaces_only_complete_full_names() {
    let dir = project(Some("full"));
    write_file(
        dir.path(),
        "people.csv",
        &format!(
            "{HEADER}Zorbina,Quenzel,Zorbina said hi\nZorbina,Quenzel,Zorbina Quenzel said hi\n"
        ),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    assert_eq!(t[1][2], "Zorbina said hi");
    let (first, last) = fakes(&t, 2);
    assert_eq!(t[2][2], format!("{first} {last} said hi"));
}

#[test]
fn mode_tokens_is_the_default_and_replaces_a_single_first_name() {
    for mode in [None, Some("tokens")] {
        let dir = project(mode);
        write_file(
            dir.path(),
            "people.csv",
            &format!("{HEADER}Zorbina,Quenzel,Zorbina said hi\n"),
        );
        let (out, _, _) = run_ok(dir.path());
        let t = table(out.path(), "people.csv");
        let (first, _) = fakes(&t, 1);
        assert_eq!(t[1][2], format!("{first} said hi"));
    }
}

#[test]
fn a_longer_word_that_contains_a_known_name_stays_unchanged() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}Zorbina,Quenzel,Zorbinabelle\n"),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    assert_eq!(t[1][2], "Zorbinabelle");
}

#[test]
fn stdout_reports_free_text_replacements_for_files_with_matches_only() {
    let dir = project(None);
    write_file(
        dir.path(),
        "a.csv",
        &format!("{HEADER}Zorbina,Quenzel,Zorbina called\nTamsorin,Velgrad,ping Quenzel\n"),
    );
    write_file(
        dir.path(),
        "b.csv",
        &format!("{HEADER}Tamsorin,Velgrad,nothing here\n"),
    );
    let (_, stdout, _) = run_ok(dir.path());
    assert!(
        stdout
            .lines()
            .any(|line| line == "a.csv: 2 free-text replacements"),
        "stdout was: {stdout}"
    );
    assert!(
        !stdout
            .lines()
            .any(|line| line.starts_with("b.csv:") && line.contains("free-text")),
        "stdout was: {stdout}"
    );
}

#[test]
fn last_comma_first_in_a_text_cell_keeps_its_order() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}Zorbina,Quenzel,\"Quenzel, Zorbina\"\n"),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    let (first, last) = fakes(&t, 1);
    assert_eq!(t[1][2], format!("{last}, {first}"));
}

#[test]
fn uppercase_name_in_a_text_cell_gets_uppercase_pseudonyms() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}Zorbina,Quenzel,ZORBINA QUENZEL\n"),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    let (first, last) = fakes(&t, 1);
    let expected = format!("{first} {last}").to_uppercase();
    assert_eq!(t[1][2], expected);
}

#[test]
fn header_text_equal_to_a_known_name_stays_unchanged() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        "first_name,last_name,Zorbina\nZorbina,Quenzel,Zorbina\n",
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    assert_eq!(t[0][2], "Zorbina");
    let (first, _) = fakes(&t, 1);
    assert_eq!(t[1][2], first);
}

#[test]
fn no_real_name_appears_in_any_output_file_or_in_stdout_and_stderr() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!(
            "{HEADER}Zorbina,Quenzel,Call Zorbina Quenzel\nTamsorin,Velgrad,\"Velgrad, Tamsorin\"\n\
             Zorbina,Quenzel,TAMSORIN VELGRAD wrote\n"
        ),
    );
    save(
        &notes_book(&["Meeting with Zorbina Quenzel", "Tamsorin", "VELGRAD"]),
        dir.path(),
        "notes.xlsx",
    );
    let (out, stdout, stderr) = run_ok(dir.path());
    let csv_text = fs::read_to_string(out.path().join("people.csv")).unwrap();
    assert_no_leak(&csv_text, "csv");
    let parts = package_parts(&out.path().join("notes.xlsx"));
    assert!(!parts.is_empty(), "package has no parts");
    for (name, text) in parts {
        assert_no_leak(&text, &format!("package part {name}"));
    }
    assert_no_leak(&stdout, "stdout");
    assert_no_leak(&stderr, "stderr");
}
