//! End-to-end tests for `pseudofy run` on XLSX files (spec 0003).
//! All data is invented. Each test creates its workbooks in a temporary directory.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions may panic"
)]

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;
use umya_spreadsheet::{Workbook, Worksheet};

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const LEAK_WORDS: [&str; 5] = ["zorbina", "quenzel", "tamsorin", "velgrad", "acme"];

fn pseudofy() -> Command {
    Command::cargo_bin("pseudofy").unwrap()
}

fn write_file(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Creates a project with a key and a config. `sheets` is an optional
/// TOML value for the `sheets` field of the XLSX rule.
fn project(sheets: Option<&str>) -> TempDir {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "secrets/pseudofy.key", &format!("{KEY}\n"));
    let fields = "first_name = [\"first_name\"]\nlast_name = [\"last_name\"]\n\
                  full_name = [\"name\"]\nemail = [\"email\"]\n";
    let mut text = String::from("language = \"de\"\n");
    text.push_str("\n[[rule]]\nfiles = \"**/*.csv\"\n");
    text.push_str(fields);
    text.push_str("\n[[rule]]\nfiles = \"**/*.xlsx\"\n");
    if let Some(sheets) = sheets {
        text.push_str("sheets = ");
        text.push_str(sheets);
        text.push('\n');
    }
    text.push_str(fields);
    write_file(dir.path(), "config/pseudofy.toml", &text);
    dir
}

fn run(project: &Path, out: &Path) -> assert_cmd::assert::Assert {
    pseudofy()
        .args(["run", "--project"])
        .arg(project)
        .arg("--out")
        .arg(out)
        .assert()
}

/// Runs successfully into a fresh output directory. Returns it with stdout and stderr.
fn run_ok(project: &Path) -> (TempDir, String, String) {
    let out = TempDir::new().unwrap();
    let assert = run(project, out.path()).success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    (out, stdout, stderr)
}

fn set(sheet: &mut Worksheet, coordinate: &str, value: &str) {
    sheet.cell_mut(coordinate).set_value_string(value);
}

fn save(book: &Workbook, project: &Path, relative: &str) {
    let path = project.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    umya_spreadsheet::writer::xlsx::write(book, path).unwrap();
}

fn open(root: &Path, relative: &str) -> Workbook {
    umya_spreadsheet::reader::xlsx::read(root.join(relative)).unwrap()
}

fn value(book: &Workbook, sheet: &str, coordinate: &str) -> String {
    book.sheet_by_name(sheet).unwrap().value(coordinate).clone()
}

/// A workbook with one sheet "Daten": header name,email in row 1.
fn simple_book(name: &str, email: &str) -> Workbook {
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    let sheet = book.new_sheet("Daten").unwrap();
    set(sheet, "A1", "name");
    set(sheet, "B1", "email");
    set(sheet, "A2", name);
    set(sheet, "B2", email);
    book
}

fn assert_no_leak(text: &str, what: &str) {
    let lower = text.to_lowercase();
    for word in LEAK_WORDS {
        assert!(!lower.contains(word), "{what} leaks {word}");
    }
}

#[test]
fn name_in_csv_and_xlsx_gets_the_same_pseudonym() {
    let dir = project(None);
    write_file(
        dir.path(),
        "kunden.csv",
        "name,email\nZorbina Quenzel,zorbina@acme.test\n",
    );
    save(
        &simple_book("Zorbina Quenzel", "zorbina@acme.test"),
        dir.path(),
        "kunden.xlsx",
    );
    let (out, _, _) = run_ok(dir.path());
    let csv = fs::read_to_string(out.path().join("kunden.csv")).unwrap();
    let csv_row = csv.lines().nth(1).unwrap().to_string();
    let mut cells = csv_row.split(',').map(|c| c.trim_matches('"').to_string());
    let csv_name = cells.next().unwrap();
    let csv_email = cells.next().unwrap();
    let book = open(out.path(), "kunden.xlsx");
    let xlsx_name = value(&book, "Daten", "A2");
    let xlsx_email = value(&book, "Daten", "B2");
    assert_ne!(xlsx_name, "Zorbina Quenzel");
    assert_eq!(xlsx_name, csv_name);
    assert_eq!(xlsx_email, csv_email);
}

#[test]
fn three_sheets_keep_their_order_and_names() {
    let dir = project(None);
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    for title in ["Zeta", "Alpha", "Mitte"] {
        let sheet = book.new_sheet(title).unwrap();
        set(sheet, "A1", "name");
        set(sheet, "A2", "Zorbina Quenzel");
    }
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    let names: Vec<&str> = copy
        .sheet_collection()
        .iter()
        .map(umya_spreadsheet::Worksheet::name)
        .collect();
    assert_eq!(names, ["Zeta", "Alpha", "Mitte"]);
}

#[test]
fn sheets_with_different_header_rows_replace_the_right_columns() {
    let dir = project(None);
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    let first = book.new_sheet("Eins").unwrap();
    set(first, "A1", "id");
    set(first, "B1", "name");
    set(first, "C1", "note");
    set(first, "A2", "1");
    set(first, "B2", "Zorbina Quenzel");
    set(first, "C2", "keep one");
    let second = book.new_sheet("Zwei").unwrap();
    set(second, "A3", "note");
    set(second, "B3", "email");
    set(second, "C3", "name");
    set(second, "A4", "keep two");
    set(second, "B4", "tamsorin@acme.test");
    set(second, "C4", "Tamsorin Velgrad");
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    assert_ne!(value(&copy, "Eins", "B2"), "Zorbina Quenzel");
    assert_ne!(value(&copy, "Eins", "B2"), "");
    assert_eq!(value(&copy, "Eins", "A2"), "1");
    assert_eq!(value(&copy, "Eins", "C2"), "keep one");
    assert_eq!(value(&copy, "Zwei", "A4"), "keep two");
    assert_ne!(value(&copy, "Zwei", "B4"), "tamsorin@acme.test");
    assert!(value(&copy, "Zwei", "B4").contains('@'));
    assert_ne!(value(&copy, "Zwei", "C4"), "Tamsorin Velgrad");
    assert_ne!(value(&copy, "Zwei", "C4"), "");
}

#[test]
fn name_in_two_sheets_gets_the_same_pseudonym() {
    let dir = project(None);
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    for title in ["Eins", "Zwei"] {
        let sheet = book.new_sheet(title).unwrap();
        set(sheet, "A1", "name");
        set(sheet, "A2", "Zorbina Quenzel");
    }
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    let one = value(&copy, "Eins", "A2");
    assert_ne!(one, "Zorbina Quenzel");
    assert_eq!(one, value(&copy, "Zwei", "A2"));
}

#[test]
fn sheets_glob_changes_only_the_matching_sheet() {
    let dir = project(Some("[\"Kunden*\"]"));
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    for title in ["Kunden 2025", "Notizen"] {
        let sheet = book.new_sheet(title).unwrap();
        set(sheet, "A1", "name");
        set(sheet, "A2", "Zorbina Quenzel");
    }
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    assert_ne!(value(&copy, "Kunden 2025", "A2"), "Zorbina Quenzel");
    assert_eq!(value(&copy, "Notizen", "A2"), "Zorbina Quenzel");
}

#[test]
fn hidden_sheet_is_processed_and_stays_hidden() {
    let dir = project(None);
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    let visible = book.new_sheet("Sichtbar").unwrap();
    set(visible, "A1", "name");
    set(visible, "A2", "Tamsorin Velgrad");
    let hidden = book.new_sheet("Versteckt").unwrap();
    set(hidden, "A1", "name");
    set(hidden, "A2", "Zorbina Quenzel");
    hidden.set_sheet_state("hidden".to_string());
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    assert_ne!(value(&copy, "Versteckt", "A2"), "Zorbina Quenzel");
    assert_ne!(value(&copy, "Versteckt", "A2"), "");
    let sheet = copy.sheet_by_name("Versteckt").unwrap();
    assert_eq!(sheet.sheet_state(), "hidden");
    let sheet = copy.sheet_by_name("Sichtbar").unwrap();
    assert_ne!(sheet.sheet_state(), "hidden");
}

#[test]
fn bold_cell_stays_bold_after_replacement() {
    let dir = project(None);
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    book.sheet_by_name_mut("Daten")
        .unwrap()
        .style_mut("A2")
        .font_mut()
        .set_bold(true);
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    assert_ne!(value(&copy, "Daten", "A2"), "Zorbina Quenzel");
    let sheet = copy.sheet_by_name("Daten").unwrap();
    let bold = sheet.style("A2").font().map(umya_spreadsheet::Font::bold);
    assert_eq!(bold, Some(true));
}

#[test]
fn formula_and_number_cells_stay_and_run_reports_the_formula_count() {
    let dir = project(None);
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let sheet = book.sheet_by_name_mut("Daten").unwrap();
    set(sheet, "C1", "summe");
    sheet.cell_mut("C2").set_value_number(40);
    sheet.cell_mut("C3").set_value_number(2);
    sheet.cell_mut("C4").set_formula("SUM(C2:C3)");
    save(&book, dir.path(), "mappe.xlsx");
    let (out, stdout, _) = run_ok(dir.path());
    assert!(
        stdout.contains("mappe.xlsx: 1 formula cells unchanged"),
        "stdout: {stdout}"
    );
    let copy = open(out.path(), "mappe.xlsx");
    let sheet = copy.sheet_by_name("Daten").unwrap();
    assert_eq!(sheet.cell("C4").unwrap().formula(), "SUM(C2:C3)");
    assert_eq!(sheet.cell("C2").unwrap().value_number(), Some(40.0));
    assert_eq!(sheet.cell("C3").unwrap().value_number(), Some(2.0));
}

#[test]
fn damaged_xlsx_is_reported_and_the_other_files_are_written() {
    let dir = project(None);
    let junk: Vec<u8> = (0..512u32)
        .map(|i| u8::try_from((i * 7 + 13) % 251).unwrap())
        .collect();
    fs::write(dir.path().join("kaputt.xlsx"), junk).unwrap();
    write_file(
        dir.path(),
        "gut.csv",
        "name,email\nZorbina Quenzel,zorbina@acme.test\n",
    );
    save(
        &simple_book("Tamsorin Velgrad", "tamsorin@acme.test"),
        dir.path(),
        "gut.xlsx",
    );
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path()).failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stderr.contains("error:"), "stderr: {stderr}");
    assert!(stderr.contains("kaputt.xlsx"), "stderr: {stderr}");
    assert!(stderr.contains("1 file(s) failed"), "stderr: {stderr}");
    assert_no_leak(&stderr, "stderr");
    assert_no_leak(&stdout, "stdout");
    assert!(out.path().join("gut.csv").exists());
    let copy = open(out.path(), "gut.xlsx");
    assert_ne!(value(&copy, "Daten", "A2"), "Tamsorin Velgrad");
}

#[test]
fn output_workbooks_and_streams_contain_no_real_value() {
    let dir = project(None);
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    for title in ["Eins", "Zwei"] {
        let sheet = book.new_sheet(title).unwrap();
        set(sheet, "A1", "name");
        set(sheet, "B1", "email");
        set(sheet, "A2", "Zorbina Quenzel");
        set(sheet, "B2", "zorbina@acme.test");
        set(sheet, "A3", "Tamsorin Velgrad");
        set(sheet, "B3", "tamsorin@acme.test");
    }
    save(&book, dir.path(), "mappe.xlsx");
    let (out, stdout, stderr) = run_ok(dir.path());
    assert_no_leak(&stdout, "stdout");
    assert_no_leak(&stderr, "stderr");
    let copy = open(out.path(), "mappe.xlsx");
    let mut seen = 0;
    for sheet in copy.sheet_collection() {
        for cell in sheet.cells() {
            assert_no_leak(&cell.value(), "output cell");
            seen += 1;
        }
    }
    assert!(seen >= 8, "expected cells in the output, found {seen}");
}

#[test]
fn input_files_stay_unchanged() {
    let dir = project(None);
    write_file(
        dir.path(),
        "kunden.csv",
        "name,email\nZorbina Quenzel,zorbina@acme.test\n",
    );
    save(
        &simple_book("Zorbina Quenzel", "zorbina@acme.test"),
        dir.path(),
        "mappe.xlsx",
    );
    let before_xlsx = fs::read(dir.path().join("mappe.xlsx")).unwrap();
    let before_csv = fs::read(dir.path().join("kunden.csv")).unwrap();
    let (_out, _, _) = run_ok(dir.path());
    assert_eq!(
        fs::read(dir.path().join("mappe.xlsx")).unwrap(),
        before_xlsx
    );
    assert_eq!(fs::read(dir.path().join("kunden.csv")).unwrap(), before_csv);
}
