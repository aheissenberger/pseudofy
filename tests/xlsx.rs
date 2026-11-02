//! End-to-end tests for `pseudofy run` on XLSX files (spec 0003).
//! All data is invented. Each test creates its workbooks in a temporary directory.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions may panic"
)]

use std::fs;
use std::io::{Cursor, Read, Write};
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

/// Serializes a workbook to XLSX bytes.
fn book_bytes(book: &Workbook) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    umya_spreadsheet::writer::xlsx::write_writer(book, &mut buffer).unwrap();
    buffer.into_inner()
}

/// The name and text of every part of an XLSX package.
fn package_parts(bytes: &[u8]) -> Vec<(String, String)> {
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

/// Fails if any part of the package at `path` contains a real value.
fn assert_package_clean(path: &Path) {
    let parts = package_parts(&fs::read(path).unwrap());
    assert!(!parts.is_empty(), "package has no parts");
    for (name, text) in parts {
        assert_no_leak(&text, &format!("package part {name}"));
    }
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
    hidden.set_state(umya_spreadsheet::SheetStateValues::Hidden);
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    assert_ne!(value(&copy, "Versteckt", "A2"), "Zorbina Quenzel");
    assert_ne!(value(&copy, "Versteckt", "A2"), "");
    let sheet = copy.sheet_by_name("Versteckt").unwrap();
    assert!(matches!(
        sheet.state(),
        umya_spreadsheet::SheetStateValues::Hidden
    ));
    let sheet = copy.sheet_by_name("Sichtbar").unwrap();
    assert!(matches!(
        sheet.state(),
        umya_spreadsheet::SheetStateValues::Visible
    ));
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
    assert_package_clean(&out.path().join("mappe.xlsx"));
}

/// The input must really keep the names in the shared-string table,
/// otherwise the package check proves nothing.
#[test]
fn input_package_keeps_names_in_shared_strings() {
    let dir = project(None);
    save(
        &simple_book("Zorbina Quenzel", "zorbina@acme.test"),
        dir.path(),
        "mappe.xlsx",
    );
    let parts = package_parts(&fs::read(dir.path().join("mappe.xlsx")).unwrap());
    let shared = parts
        .iter()
        .find(|(name, _)| name == "xl/sharedStrings.xml")
        .map(|(_, text)| text.clone())
        .unwrap_or_default();
    assert!(
        shared.contains("Zorbina Quenzel"),
        "shared strings: {shared}"
    );
}

#[test]
fn mailto_hyperlink_gets_the_fake_address() {
    let dir = project(None);
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let mut link = umya_spreadsheet::Hyperlink::default();
    link.set_url("mailto:zorbina@acme.test");
    book.sheet_by_name_mut("Daten")
        .unwrap()
        .cell_mut("B2")
        .set_hyperlink(link);
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    assert_package_clean(&out.path().join("mappe.xlsx"));
    let copy = open(out.path(), "mappe.xlsx");
    let sheet = copy.sheet_by_name("Daten").unwrap();
    let cell = sheet.cell("B2").unwrap();
    let fake = cell.value().to_string();
    assert!(fake.contains('@'), "cell: {fake}");
    let url = cell.hyperlink().map(|h| h.url().to_string());
    assert_eq!(url, Some(format!("mailto:{fake}")));
}

#[test]
fn cached_formula_result_does_not_leak_a_real_name() {
    let dir = project(None);
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let sheet = book.sheet_by_name_mut("Daten").unwrap();
    sheet
        .cell_mut("A3")
        .set_formula("A2")
        .set_formula_result_string("Zorbina Quenzel");
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    assert_package_clean(&out.path().join("mappe.xlsx"));
    let copy = open(out.path(), "mappe.xlsx");
    let sheet = copy.sheet_by_name("Daten").unwrap();
    assert_eq!(sheet.cell("A3").unwrap().formula(), "A2");
}

#[test]
fn workbook_with_a_pivot_cache_is_refused_and_the_others_are_written() {
    let dir = project(None);
    let bytes = book_bytes(&simple_book("Zorbina Quenzel", "zorbina@acme.test"));
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        if entry.is_dir() {
            continue;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        writer
            .start_file(entry.name(), zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&data).unwrap();
    }
    writer
        .start_file(
            "xl/pivotCache/pivotCacheDefinition1.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    writer.write_all(b"<pivotCacheDefinition/>").unwrap();
    let pivot = writer.finish().unwrap().into_inner();
    fs::write(dir.path().join("pivot.xlsx"), pivot).unwrap();
    write_file(
        dir.path(),
        "gut.csv",
        "name,email\nZorbina Quenzel,zorbina@acme.test\n",
    );
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path()).failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("pivot.xlsx"), "stderr: {stderr}");
    assert!(stderr.to_lowercase().contains("pivot"), "stderr: {stderr}");
    assert!(stderr.contains("1 file(s) failed"), "stderr: {stderr}");
    assert_no_leak(&stderr, "stderr");
    assert!(!out.path().join("pivot.xlsx").exists());
    assert!(out.path().join("gut.csv").exists());
}

#[test]
fn panicking_input_gives_one_error_line_without_a_real_value() {
    let dir = project(None);
    let bytes = book_bytes(&simple_book("Tamsorin Velgrad", "tamsorin@acme.test"));
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        if entry.is_dir() {
            continue;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if entry.name() == "xl/worksheets/sheet1.xml" {
            data = concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">",
                "<sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>name</t></is></c></row>",
                "<row r=\"2\"><c r=\"A2\" t=\"inlineStr\"><is><t>Zorbina Quen"
            )
            .as_bytes()
            .to_vec();
        }
        writer
            .start_file(entry.name(), zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&data).unwrap();
    }
    let cut = writer.finish().unwrap().into_inner();
    fs::write(dir.path().join("abgeschnitten.xlsx"), cut).unwrap();
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path()).failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    let mentioning = stderr
        .lines()
        .filter(|line| line.contains("abgeschnitten.xlsx"))
        .count();
    assert_eq!(mentioning, 1, "stderr: {stderr}");
    assert_no_leak(&stderr, "stderr");
    assert!(!stderr.contains("internal error"), "stderr: {stderr}");
}

#[test]
fn column_widths_and_merged_cells_survive_a_run() {
    let dir = project(None);
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let sheet = book.sheet_by_name_mut("Daten").unwrap();
    set(sheet, "D1", "titel");
    sheet.column_dimension_mut("A").set_width(37.0);
    sheet.add_merge_cells("D1:E1");
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    let sheet = copy.sheet_by_name("Daten").unwrap();
    let width = sheet
        .column_dimension("A")
        .map(umya_spreadsheet::Column::width);
    assert_eq!(width, Some(37.0));
    let merged: Vec<String> = sheet
        .merge_cells()
        .iter()
        .map(umya_spreadsheet::Range::range)
        .collect();
    assert_eq!(merged, ["D1:E1"]);
}

#[test]
fn rich_text_cell_in_a_name_column_is_replaced_and_reported() {
    let dir = project(None);
    let mut book = simple_book("Tamsorin Velgrad", "tamsorin@acme.test");
    let mut first = umya_spreadsheet::TextElement::default();
    first.set_text("Zorbina ");
    first.font_mut().set_bold(true);
    let mut second = umya_spreadsheet::TextElement::default();
    second.set_text("Quenzel");
    let mut rich = umya_spreadsheet::RichText::default();
    rich.add_rich_text_elements(first)
        .add_rich_text_elements(second);
    book.sheet_by_name_mut("Daten")
        .unwrap()
        .cell_mut("A3")
        .set_rich_text(rich);
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, stderr) = run_ok(dir.path());
    assert!(stderr.contains("rich-text"), "stderr: {stderr}");
    assert_package_clean(&out.path().join("mappe.xlsx"));
}

#[test]
fn sheets_glob_without_a_matching_sheet_warns_about_the_sheet_name() {
    let dir = project(Some("[\"Kunden*\"]"));
    save(
        &simple_book("Zorbina Quenzel", "zorbina@acme.test"),
        dir.path(),
        "mappe.xlsx",
    );
    let (_out, _, stderr) = run_ok(dir.path());
    assert!(stderr.contains("no sheet"), "stderr: {stderr}");
    assert!(!stderr.contains("none of its headers"), "stderr: {stderr}");
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
