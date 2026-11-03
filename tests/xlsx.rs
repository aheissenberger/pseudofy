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

/// Copies the entries of an XLSX package into a new one. `edit` gets the
/// name and text of each part and returns new text to replace it. `extra`
/// parts are appended.
fn rewrite_package(
    bytes: &[u8],
    edit: impl Fn(&str, &str) -> Option<String>,
    extra: &[(&str, &str)],
) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, text) in package_parts(bytes) {
        let text = edit(&name, &text).unwrap_or(text);
        writer.start_file(name, options).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    for (name, text) in extra {
        writer.start_file(*name, options).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// Fails if an XML part of the package at `path` is not well-formed.
fn assert_well_formed(path: &Path) {
    let parts = package_parts(&fs::read(path).unwrap());
    assert!(!parts.is_empty(), "package has no parts");
    for (name, text) in parts {
        let extension = Path::new(&name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !["xml", "rels", "vml"].contains(&extension.as_str()) {
            continue;
        }
        let mut reader = quick_xml::Reader::from_str(&text);
        let mut depth: i64 = 0;
        loop {
            let event = reader
                .read_event()
                .unwrap_or_else(|_| panic!("package part {name} is not well-formed"));
            match event {
                quick_xml::events::Event::Eof => break,
                quick_xml::events::Event::Start(_) => depth += 1,
                quick_xml::events::Event::End(_) => depth -= 1,
                _ => {}
            }
        }
        assert_eq!(depth, 0, "package part {name} has unbalanced elements");
    }
}

const MAIN_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

/// Runs on a project whose only failing file is `file`. Checks the
/// "unsupported workbook layout" error and that no copy exists.
fn run_refused(dir: &TempDir, file: &str) -> String {
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path()).failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains(file), "stderr: {stderr}");
    assert!(
        stderr.contains("unsupported workbook layout"),
        "stderr: {stderr}"
    );
    assert_no_leak(&stderr, "stderr");
    assert!(!out.path().join(file).exists(), "a copy was written");
    stderr
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
    // Each name occurs in one sheet only. A name in a name column is a known
    // name, and spec 0004 replaces known names in every other text cell too.
    for (title, name) in [
        ("Kunden 2025", "Zorbina Quenzel"),
        ("Notizen", "Pirmox Hulbert"),
    ] {
        let sheet = book.new_sheet(title).unwrap();
        set(sheet, "A1", "name");
        set(sheet, "A2", name);
    }
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    assert_ne!(value(&copy, "Kunden 2025", "A2"), "Zorbina Quenzel");
    assert_eq!(value(&copy, "Notizen", "A2"), "Pirmox Hulbert");
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
    let (out, stdout, stderr) = run_ok(dir.path());
    assert!(
        stderr.contains("warning: mappe.xlsx: 1 formula cells unchanged"),
        "stderr: {stderr}"
    );
    assert!(!stdout.contains("formula cells"), "stdout: {stdout}");
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
    assert_well_formed(&out.path().join("mappe.xlsx"));
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
    assert_well_formed(&out.path().join("mappe.xlsx"));
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
    assert_well_formed(&out.path().join("mappe.xlsx"));
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
    assert!(stderr.contains("1 rich-text cells"), "stderr: {stderr}");
    assert_package_clean(&out.path().join("mappe.xlsx"));
    assert_well_formed(&out.path().join("mappe.xlsx"));
    let copy = open(out.path(), "mappe.xlsx");
    let replaced = value(&copy, "Daten", "A3");
    assert!(!replaced.is_empty(), "rich-text cell is empty");
    assert_ne!(replaced, "Zorbina Quenzel");
    assert_no_leak(&replaced, "rich-text cell");
}

#[test]
fn sheets_glob_without_a_matching_sheet_fails_the_file() {
    let dir = project(Some("[\"Kunden*\"]"));
    save(
        &simple_book("Zorbina Quenzel", "zorbina@acme.test"),
        dir.path(),
        "mappe.xlsx",
    );
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path()).failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    let expected = "error: mappe.xlsx: rule \"**/*.xlsx\" matches no sheet";
    assert!(
        stderr.lines().any(|line| line.trim_end() == expected),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("Kunden"), "stderr names a sheet: {stderr}");
    assert!(!stderr.contains("Daten"), "stderr names a sheet: {stderr}");
    assert!(!out.path().join("mappe.xlsx").exists());
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

fn sheet_with(cells: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><worksheet xmlns=\"{MAIN_NS}\"><sheetData>{cells}</sheetData></worksheet>"
    )
}

#[test]
fn unprocessed_parts_give_one_warning_without_content() {
    let dir = project(None);
    let bytes = book_bytes(&simple_book("Tamsorin Velgrad", "tamsorin@acme.test"));
    let edited = rewrite_package(
        &bytes,
        |name, text| {
            if name != "xl/worksheets/sheet1.xml" {
                return None;
            }
            let validations = "</sheetData><dataValidations count=\"1\"><dataValidation type=\"list\" sqref=\"A2\"><formula1>\"x,y\"</formula1></dataValidation></dataValidations>";
            let header = "<headerFooter><oddHeader>&amp;LZorbina Quenzel</oddHeader></headerFooter></worksheet>";
            Some(
                text.replacen("</sheetData>", validations, 1)
                    .replacen("</worksheet>", header, 1),
            )
        },
        &[],
    );
    fs::write(dir.path().join("mappe.xlsx"), edited).unwrap();
    let (out, _, stderr) = run_ok(dir.path());
    assert!(out.path().join("mappe.xlsx").exists());
    let warnings: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("not pseudonymized"))
        .collect();
    assert_eq!(warnings.len(), 1, "stderr: {stderr}");
    let line = warnings[0];
    assert!(line.contains("warning:"), "line: {line}");
    assert!(line.contains("mappe.xlsx"), "line: {line}");
    assert!(line.contains("data validations"), "line: {line}");
    assert!(line.contains("headers/footers"), "line: {line}");
    assert_no_leak(&stderr, "stderr");
    assert_well_formed(&out.path().join("mappe.xlsx"));
}

#[test]
fn comments_part_gives_a_comments_warning() {
    let dir = project(None);
    let bytes = book_bytes(&simple_book("Tamsorin Velgrad", "tamsorin@acme.test"));
    let comments = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><comments xmlns=\"{MAIN_NS}\"><authors><author>Autor</author></authors><commentList><comment ref=\"A2\" authorId=\"0\"><text><r><t>Hinweis</t></r></text></comment></commentList></comments>"
    );
    // The comments part must hang on the sheet: the warning comes from the
    // package that the crate writes, and the crate drops loose parts.
    let rels = rels_xml(&[("comments", "../comments1.xml".to_owned())]);
    let types = "<Override PartName=\"/xl/comments1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml\"/></Types>";
    let edited = repackage(
        &bytes,
        str::to_owned,
        |name, text| (name == "[Content_Types].xml").then(|| text.replacen("</Types>", types, 1)),
        &[
            ("xl/comments1.xml".to_owned(), comments),
            ("xl/worksheets/_rels/sheet1.xml.rels".to_owned(), rels),
        ],
    );
    fs::write(dir.path().join("mappe.xlsx"), edited).unwrap();
    let (_out, _, stderr) = run_ok(dir.path());
    let line = stderr
        .lines()
        .find(|line| line.contains("not pseudonymized"))
        .unwrap_or_default();
    assert!(line.contains("mappe.xlsx"), "stderr: {stderr}");
    assert!(line.contains("comments"), "stderr: {stderr}");
    assert_no_leak(&stderr, "stderr");
}

#[test]
fn plain_workbook_gives_no_unprocessed_warning() {
    let dir = project(None);
    save(
        &simple_book("Tamsorin Velgrad", "tamsorin@acme.test"),
        dir.path(),
        "mappe.xlsx",
    );
    let (_out, _, stderr) = run_ok(dir.path());
    assert!(!stderr.contains("not pseudonymized"), "stderr: {stderr}");
}

#[test]
fn hyperlink_tooltip_is_cleared() {
    let dir = project(None);
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let mut link = umya_spreadsheet::Hyperlink::default();
    link.set_url("https://example.com/seite");
    link.set_tooltip("Mail Zorbina Quenzel");
    book.sheet_by_name_mut("Daten")
        .unwrap()
        .cell_mut("A2")
        .set_hyperlink(link);
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    assert_package_clean(&out.path().join("mappe.xlsx"));
    assert_well_formed(&out.path().join("mappe.xlsx"));
    let copy = open(out.path(), "mappe.xlsx");
    let sheet = copy.sheet_by_name("Daten").unwrap();
    let link = sheet.cell("A2").unwrap().hyperlink().cloned();
    let tooltip = link.map(|h| h.tooltip().to_string()).unwrap_or_default();
    assert_eq!(tooltip, "");
}

#[test]
fn percent_encoded_mailto_gets_the_fake_address() {
    let dir = project(None);
    let mut book = simple_book("Zorbina Quenzel", "tamsorin@acme.test");
    let mut link = umya_spreadsheet::Hyperlink::default();
    link.set_url("mailto:tamsorin%40acme.test");
    book.sheet_by_name_mut("Daten")
        .unwrap()
        .cell_mut("B2")
        .set_hyperlink(link);
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    assert_package_clean(&out.path().join("mappe.xlsx"));
    assert_well_formed(&out.path().join("mappe.xlsx"));
    let copy = open(out.path(), "mappe.xlsx");
    let sheet = copy.sheet_by_name("Daten").unwrap();
    let cell = sheet.cell("B2").unwrap();
    let fake = cell.value().to_string();
    assert!(fake.contains('@'), "cell: {fake}");
    let url = cell.hyperlink().map(|h| h.url().to_string());
    assert_eq!(url, Some(format!("mailto:{fake}")));
}

#[test]
fn mailto_with_query_part_is_reduced_to_the_fake_address() {
    let dir = project(None);
    let mut book = simple_book("Zorbina Quenzel", "Pirmox.Hulbert@acme.test");
    let mut link = umya_spreadsheet::Hyperlink::default();
    link.set_url("mailto:Pirmox.Hulbert@acme.test?subject=Hallo%20Pirmox");
    book.sheet_by_name_mut("Daten")
        .unwrap()
        .cell_mut("B2")
        .set_hyperlink(link);
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    assert_package_clean(&out.path().join("mappe.xlsx"));
    assert_well_formed(&out.path().join("mappe.xlsx"));
    for (name, text) in package_parts(&fs::read(out.path().join("mappe.xlsx")).unwrap()) {
        assert!(
            !text.to_lowercase().contains("pirmox"),
            "part {name} leaks pirmox"
        );
    }
    let copy = open(out.path(), "mappe.xlsx");
    let sheet = copy.sheet_by_name("Daten").unwrap();
    let cell = sheet.cell("B2").unwrap();
    let fake = cell.value().to_string();
    assert!(fake.contains('@'), "cell: {fake}");
    let url = cell.hyperlink().map(|h| h.url().to_string());
    assert_eq!(url, Some(format!("mailto:{fake}")));
}

#[test]
fn prefixed_shared_string_table_is_refused() {
    let dir = project(None);
    let bytes = book_bytes(&simple_book("Tamsorin Velgrad", "tamsorin@acme.test"));
    let strings = ["name", "email", "Tamsorin Velgrad", "tamsorin@acme.test"];
    let mut items = String::new();
    for text in strings.iter().chain(strings.iter()) {
        items += "<x:si><x:t>";
        items += text;
        items += "</x:t></x:si>";
    }
    let sst = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><x:sst xmlns:x=\"{MAIN_NS}\">{items}</x:sst>"
    );
    let edited = rewrite_package(
        &bytes,
        |name, _| (name == "xl/sharedStrings.xml").then(|| sst.clone()),
        &[],
    );
    fs::write(dir.path().join("mappe.xlsx"), edited).unwrap();
    run_refused(&dir, "mappe.xlsx");
}

#[test]
fn inline_string_with_two_runs_is_refused() {
    let dir = project(None);
    let bytes = book_bytes(&simple_book("Tamsorin Velgrad", "tamsorin@acme.test"));
    let sheet = sheet_with(concat!(
        "<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>name</t></is></c></row>",
        "<row r=\"2\"><c r=\"A2\" t=\"inlineStr\"><is><r><t>Tamsorin</t></r><r><t> Velgrad</t></r></is></c></row>"
    ));
    let edited = rewrite_package(
        &bytes,
        |name, _| (name == "xl/worksheets/sheet1.xml").then(|| sheet.clone()),
        &[],
    );
    fs::write(dir.path().join("mappe.xlsx"), edited).unwrap();
    run_refused(&dir, "mappe.xlsx");
}

// ---- External links (spec 0012, ADR-0011) ----

const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
const LINK_PART: &str = "xl/externalLinks/externalLink1.xml";
const LINK_RELS_PART: &str = "xl/externalLinks/_rels/externalLink1.xml.rels";
const CACHED_NAME: &str = "Zorbina Quenzel";

fn fixture_bytes(name: &str) -> Vec<u8> {
    fs::read(Path::new(FIXTURE_DIR).join(name)).unwrap()
}

/// Runs pseudofy on the bytes, saved as `mappe.xlsx` in a project whose
/// rule matches the headers of the fidelity fixture. Returns the copy's parts.
fn copy_parts(bytes: &[u8]) -> Vec<(String, String)> {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "secrets/pseudofy.key", &format!("{KEY}\n"));
    write_file(
        dir.path(),
        "config/pseudofy.toml",
        "language = \"de\"\n\n[[rule]]\nfiles = \"*.xlsx\"\n\
         first_name = [\"First Name\"]\nlast_name = [\"Last Name\"]\n\
         full_name = [\"name\"]\nemail = [\"Email\", \"email\"]\n",
    );
    fs::write(dir.path().join("mappe.xlsx"), bytes).unwrap();
    let (out, _, _) = run_ok(dir.path());
    package_parts(&fs::read(out.path().join("mappe.xlsx")).unwrap())
}

fn part_text(parts: &[(String, String)], name: &str) -> String {
    parts
        .iter()
        .find(|(part, _)| part == name)
        .unwrap_or_else(|| panic!("the package has no part {name}"))
        .1
        .clone()
}

fn has_part(parts: &[(String, String)], name: &str) -> bool {
    parts.iter().any(|(part, _)| part == name)
}

/// The start tags `<tag ...>` of an XML text, without the angle brackets.
fn start_tags<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag} ");
    let mut tags = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + 1..];
        tags.push(&rest[..rest.find('>').unwrap()]);
    }
    tags
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    let at = tag.find(&key)? + key.len();
    let value = &tag[at..];
    Some(value[..value.find('"').unwrap()].to_string())
}

fn has_external_link_parts(parts: &[(String, String)]) -> bool {
    parts
        .iter()
        .any(|(name, _)| name.starts_with("xl/externalLinks/"))
}

#[test]
fn copy_keeps_each_external_link_part_of_the_input() {
    let input = package_parts(&fixture_bytes("fidelity.xlsx"));
    assert!(has_part(&input, LINK_PART));
    let copy = copy_parts(&fixture_bytes("fidelity.xlsx"));
    for (name, _) in input
        .iter()
        .filter(|(name, _)| name.starts_with("xl/externalLinks/"))
    {
        assert!(has_part(&copy, name), "the copy lost {name}");
    }
    assert!(has_part(&copy, LINK_RELS_PART));
}

#[test]
fn copy_lists_one_external_reference_per_link() {
    let input = package_parts(&fixture_bytes("fidelity.xlsx"));
    let links = input
        .iter()
        .filter(|(name, _)| name.starts_with("xl/externalLinks/externalLink"))
        .count();
    let copy = copy_parts(&fixture_bytes("fidelity.xlsx"));
    let workbook = part_text(&copy, "xl/workbook.xml");
    assert!(workbook.contains("<externalReferences>"));
    assert_eq!(start_tags(&workbook, "externalReference").len(), links);
}

#[test]
fn external_reference_id_resolves_to_the_external_link_part() {
    let copy = copy_parts(&fixture_bytes("fidelity.xlsx"));
    let workbook = part_text(&copy, "xl/workbook.xml");
    let rels = part_text(&copy, "xl/_rels/workbook.xml.rels");
    let references = start_tags(&workbook, "externalReference");
    assert_eq!(references.len(), 1);
    let id = attribute(references[0], "r:id").unwrap();

    let relationships = start_tags(&rels, "Relationship");
    let mut ids: Vec<String> = relationships
        .iter()
        .map(|tag| attribute(tag, "Id").unwrap())
        .collect();
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), total, "a relationship Id occurs twice");

    let matching: Vec<&&str> = relationships
        .iter()
        .filter(|tag| attribute(tag, "Id").as_deref() == Some(id.as_str()))
        .collect();
    assert_eq!(matching.len(), 1, "r:id {id} is not in workbook.xml.rels");
    let kind = attribute(matching[0], "Type").unwrap();
    let target = attribute(matching[0], "Target").unwrap();
    assert!(kind.ends_with("/externalLink"), "type: {kind}");
    assert!(
        target.ends_with("externalLinks/externalLink1.xml"),
        "target: {target}"
    );
}

#[test]
fn content_types_has_an_override_for_the_external_link() {
    let copy = copy_parts(&fixture_bytes("fidelity.xlsx"));
    let types = part_text(&copy, "[Content_Types].xml");
    assert!(
        types.contains("PartName=\"/xl/externalLinks/externalLink1.xml\""),
        "no Override for the external link"
    );
}

#[test]
fn cached_external_values_do_not_reach_the_copy() {
    let marker = "<sheetData sheetId=\"0\"/>";
    let cached = format!(
        "<sheetData sheetId=\"0\"><row r=\"1\"><cell r=\"A1\" t=\"str\">\
         <v>{CACHED_NAME}</v></cell></row></sheetData>"
    );
    let input = rewrite_package(
        &fixture_bytes("fidelity.xlsx"),
        |name, text| (name == LINK_PART).then(|| text.replacen(marker, &cached, 1)),
        &[],
    );
    let parts = package_parts(&input);
    assert!(part_text(&parts, LINK_PART).contains(CACHED_NAME));

    let copy = copy_parts(&input);
    assert!(has_part(&copy, LINK_PART), "the link part is missing");
    for (name, text) in &copy {
        assert!(
            !text.contains(CACHED_NAME) && !text.contains("Quenzel"),
            "the cached name occurs in {name}"
        );
        if name.starts_with("xl/externalLinks/") {
            assert!(
                !text.contains("<cell") && !text.contains("<v>"),
                "{name} keeps cached cells"
            );
        }
    }
}

#[test]
fn workbook_without_external_links_gets_none() {
    let copy = copy_parts(&fixture_bytes("fidelity_reference.xlsx"));
    assert!(!has_external_link_parts(&copy));
    assert!(!part_text(&copy, "xl/workbook.xml").contains("<externalReferences"));
}

#[test]
fn generated_workbook_gets_no_external_links() {
    let bytes = book_bytes(&simple_book("Tamsorin Velgrad", "tamsorin@acme.test"));
    let copy = copy_parts(&bytes);
    assert!(!has_external_link_parts(&copy));
    assert!(!part_text(&copy, "xl/workbook.xml").contains("<externalReferences"));
}

// ---- Sparklines (spec 0013, ADR-0011) ----

const SPARKLINE_URI: &str = "{05C60535-1F16-4fd2-B633-F4F36F0B64E0}";

/// Sheet name and part text of each worksheet, mapped through workbook.xml.
fn sheets_by_name(parts: &[(String, String)]) -> Vec<(String, String)> {
    let workbook = part_text(parts, "xl/workbook.xml");
    let rels = part_text(parts, "xl/_rels/workbook.xml.rels");
    let relationships = start_tags(&rels, "Relationship");
    start_tags(&workbook, "sheet")
        .iter()
        .map(|sheet| {
            let id = attribute(sheet, "r:id").unwrap();
            let tag = relationships
                .iter()
                .find(|t| attribute(t, "Id").as_deref() == Some(id.as_str()))
                .unwrap();
            let target = attribute(tag, "Target").unwrap();
            let part = match target.strip_prefix('/') {
                Some(absolute) => absolute.to_owned(),
                None => format!("xl/{target}"),
            };
            (attribute(sheet, "name").unwrap(), part_text(parts, &part))
        })
        .collect()
}

/// The `(xm:f, xm:sqref)` text of each sparkline element, in document order.
fn sparkline_ranges(xml: &str) -> Vec<(String, String)> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut ranges = Vec::new();
    let mut current: Option<(String, String)> = None;
    let mut field = String::new();
    loop {
        match reader.read_event().unwrap() {
            Event::Eof => break,
            Event::Start(e) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if local == "sparkline" {
                    current = Some((String::new(), String::new()));
                }
                field = local;
            }
            Event::Text(t) => {
                if let Some((f, sqref)) = current.as_mut() {
                    let text = t.decode().unwrap().into_owned();
                    match field.as_str() {
                        "f" => f.push_str(&text),
                        "sqref" => sqref.push_str(&text),
                        _ => {}
                    }
                }
            }
            Event::End(e) => {
                if e.local_name().as_ref() == b"sparkline" {
                    ranges.extend(current.take());
                }
                field.clear();
            }
            _ => {}
        }
    }
    ranges
}

/// Returns each namespace prefix that the sparkline extension uses and that is
/// not in scope there.
fn undeclared_prefixes_in_sparkline_ext(xml: &str) -> Vec<String> {
    use quick_xml::events::{BytesStart, Event};
    fn declared(e: &BytesStart) -> Vec<String> {
        e.attributes()
            .map(|a| String::from_utf8_lossy(a.unwrap().key.as_ref()).into_owned())
            .filter_map(|k| k.strip_prefix("xmlns:").map(str::to_owned))
            .collect()
    }
    fn used(e: &BytesStart) -> Vec<String> {
        let mut names = vec![String::from_utf8_lossy(e.name().as_ref()).into_owned()];
        names.extend(
            e.attributes()
                .map(|a| String::from_utf8_lossy(a.unwrap().key.as_ref()).into_owned()),
        );
        names
            .iter()
            .filter(|n| !n.starts_with("xmlns"))
            .filter_map(|n| n.split_once(':').map(|(p, _)| p.to_owned()))
            .filter(|p| p != "xml")
            .collect()
    }
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut scopes: Vec<Vec<String>> = Vec::new();
    let mut ext_depth: Option<usize> = None;
    let mut missing = Vec::new();
    loop {
        let event = reader.read_event().unwrap();
        match &event {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                scopes.push(declared(e));
                let is_ext = e.local_name().as_ref() == b"ext"
                    && e.attributes()
                        .any(|a| a.unwrap().value.as_ref() == SPARKLINE_URI.as_bytes());
                if is_ext && ext_depth.is_none() {
                    ext_depth = Some(scopes.len());
                }
                if ext_depth.is_some() {
                    for prefix in used(e) {
                        if !scopes.iter().flatten().any(|d| *d == prefix) {
                            missing.push(prefix);
                        }
                    }
                }
                if matches!(event, Event::Empty(_)) {
                    scopes.pop();
                    if ext_depth.is_some_and(|d| scopes.len() < d) {
                        ext_depth = None;
                    }
                }
            }
            Event::End(_) => {
                scopes.pop();
                if ext_depth.is_some_and(|d| scopes.len() < d) {
                    ext_depth = None;
                }
            }
            _ => {}
        }
    }
    missing.sort();
    missing.dedup();
    missing
}

#[test]
fn restored_sparklines_keep_formula_and_cell_of_the_input_sheet() {
    let input = sheets_by_name(&package_parts(&fixture_bytes("fidelity.xlsx")));
    let copy = sheets_by_name(&copy_parts(&fixture_bytes("fidelity.xlsx")));
    let mut total = 0;
    for (name, xml) in &input {
        let expected = sparkline_ranges(xml);
        total += expected.len();
        let (_, copied) = copy
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("the copy has no sheet {name}"));
        assert_eq!(
            sparkline_ranges(copied),
            expected,
            "sparkline ranges of sheet {name} differ"
        );
    }
    assert_eq!(total, 10, "the fixture has 10 sparklines");
}

#[test]
fn sheet_with_sparklines_is_well_formed_and_declares_each_prefix() {
    let copy = sheets_by_name(&copy_parts(&fixture_bytes("fidelity.xlsx")));
    let mut checked = 0;
    for (name, xml) in &copy {
        let mut reader = quick_xml::Reader::from_str(xml);
        let mut depth: i64 = 0;
        loop {
            match reader
                .read_event()
                .unwrap_or_else(|_| panic!("sheet {name} is not well-formed"))
            {
                quick_xml::events::Event::Eof => break,
                quick_xml::events::Event::Start(_) => depth += 1,
                quick_xml::events::Event::End(_) => depth -= 1,
                _ => {}
            }
        }
        assert_eq!(depth, 0, "sheet {name} has unbalanced elements");
        if xml.contains(SPARKLINE_URI) {
            checked += 1;
            let missing = undeclared_prefixes_in_sparkline_ext(xml);
            assert!(
                missing.is_empty(),
                "sheet {name}: prefixes not declared: {missing:?}"
            );
        }
    }
    assert_eq!(checked, 1, "one sheet of the copy has the sparkline ext");
}

#[test]
fn undeclared_prefix_in_sparkline_ext_is_detected() {
    let xml = format!(
        "<worksheet xmlns=\"{MAIN_NS}\"><extLst><ext uri=\"{SPARKLINE_URI}\">\
         <x14:sparklines><xm:f>A1</xm:f></x14:sparklines></ext></extLst></worksheet>"
    );
    assert_eq!(
        undeclared_prefixes_in_sparkline_ext(&xml),
        vec!["x14".to_owned(), "xm".to_owned()]
    );
}

#[test]
fn workbook_without_sparklines_gets_no_sparkline_extension() {
    let bytes = book_bytes(&simple_book("Tamsorin Velgrad", "tamsorin@acme.test"));
    let copy = copy_parts(&bytes);
    let sheets = sheets_by_name(&copy);
    assert_eq!(sheets.len(), 1);
    for (name, xml) in &sheets {
        assert!(!xml.contains("x14:sparkline"), "sheet {name}");
        assert!(!xml.contains(SPARKLINE_URI), "sheet {name}");
    }
}

// ---- Header search and fail closed (spec 0015) ----

/// Runs a project that must fail. Returns a fresh output directory and stderr.
fn run_failing(project: &Path) -> (TempDir, String) {
    let out = TempDir::new().unwrap();
    let assert = run(project, out.path()).failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    (out, stderr)
}

/// Checks the line `error: <message>` and that stderr has no value or header text.
fn assert_error_line(stderr: &str, message: &str) {
    let expected = format!("error: {message}");
    assert!(
        stderr.lines().any(|line| line.trim_end() == expected),
        "no line {expected:?} in stderr: {stderr}"
    );
    assert_no_leak(stderr, "stderr");
    for header in ["first_name", "last_name", "email"] {
        assert!(!stderr.contains(header), "stderr names a header: {stderr}");
    }
}

const NO_HEADER_RULE: &str = "no header of rule \"**/*.xlsx\" in the first 20 rows";

#[test]
fn workbook_fails_when_sheet_2_has_no_header_and_names_the_sheet_position() {
    let dir = project(None);
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    let first = book.new_sheet("Daten").unwrap();
    set(first, "A1", "name");
    set(first, "B1", "email");
    set(first, "A2", "Zorbina Quenzel");
    set(first, "B2", "zorbina@acme.test");
    let second = book.new_sheet("Info").unwrap();
    set(second, "A1", "Tamsorin Velgrad");
    set(second, "B1", "tamsorin@acme.test");
    save(&book, dir.path(), "mappe.xlsx");
    let (out, stderr) = run_failing(dir.path());
    assert_error_line(&stderr, &format!("mappe.xlsx: sheet 2: {NO_HEADER_RULE}"));
    assert!(!stderr.contains("Info"), "stderr names the sheet: {stderr}");
    assert!(!out.path().join("mappe.xlsx").exists());
}

#[test]
fn workbook_without_any_header_fails_with_sheet_1() {
    let dir = project(None);
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    let sheet = book.new_sheet("Daten").unwrap();
    set(sheet, "A1", "Zorbina Quenzel");
    set(sheet, "B1", "zorbina@acme.test");
    save(&book, dir.path(), "mappe.xlsx");
    let (out, stderr) = run_failing(dir.path());
    assert_error_line(&stderr, &format!("mappe.xlsx: sheet 1: {NO_HEADER_RULE}"));
    assert!(!out.path().join("mappe.xlsx").exists());
}

#[test]
fn sheets_glob_lets_an_info_sheet_without_headers_pass() {
    let dir = project(Some("[\"Daten\"]"));
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let info = book.new_sheet("Info").unwrap();
    set(info, "A1", "Nur fuer den internen Gebrauch");
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    assert_ne!(value(&copy, "Daten", "A2"), "Zorbina Quenzel");
    assert_ne!(value(&copy, "Daten", "B2"), "zorbina@acme.test");
    assert_eq!(value(&copy, "Info", "A1"), "Nur fuer den internen Gebrauch");
}

#[test]
fn title_in_row_1_and_header_in_row_2_get_names_replaced_and_the_title_cleaned() {
    let dir = project(None);
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    let sheet = book.new_sheet("Daten").unwrap();
    set(sheet, "A1", "Kontakte von zorbina@acme.test fuer Zorbina");
    set(sheet, "A2", "name");
    set(sheet, "B2", "email");
    set(sheet, "A3", "Zorbina Quenzel");
    set(sheet, "B3", "zorbina@acme.test");
    set(sheet, "A4", "Tamsorin Velgrad");
    set(sheet, "B4", "tamsorin@acme.test");
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    let title = value(&copy, "Daten", "A1");
    assert_no_leak(&title, "title");
    assert!(title.starts_with("Kontakte von "), "title: {title}");
    assert!(
        title.contains('@') && title.contains("example.com"),
        "{title}"
    );
    assert!(title.contains(" fuer "), "title: {title}");
    assert_eq!(value(&copy, "Daten", "A2"), "name");
    assert_eq!(value(&copy, "Daten", "B2"), "email");
    assert_ne!(value(&copy, "Daten", "A3"), "Zorbina Quenzel");
    assert_ne!(value(&copy, "Daten", "A4"), "Tamsorin Velgrad");
    assert_ne!(value(&copy, "Daten", "A4"), "");
    assert!(value(&copy, "Daten", "B3").contains("example.com"));
    assert_package_clean(&out.path().join("mappe.xlsx"));
}

#[test]
fn email_in_the_header_row_is_replaced_and_configured_headers_stay() {
    let dir = project(None);
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    let sheet = book.new_sheet("Daten").unwrap();
    set(sheet, "A1", "name");
    set(sheet, "B1", "email");
    set(sheet, "C1", "Kontakt zorbina@acme.test");
    set(sheet, "A2", "Zorbina Quenzel");
    set(sheet, "B2", "zorbina@acme.test");
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    assert_eq!(value(&copy, "Daten", "A1"), "name");
    assert_eq!(value(&copy, "Daten", "B1"), "email");
    let cell = value(&copy, "Daten", "C1");
    assert_no_leak(&cell, "header cell");
    assert!(cell.starts_with("Kontakt "), "{cell}");
    assert!(cell.contains("example.com"), "{cell}");
    assert_package_clean(&out.path().join("mappe.xlsx"));
}

/// A workbook whose header row is row `header_row`. Each row above has a title.
fn book_with_header_in_row(header_row: u32) -> Workbook {
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    let sheet = book.new_sheet("Daten").unwrap();
    for row in 1..header_row {
        set(sheet, &format!("A{row}"), &format!("Zeile {row}"));
    }
    set(sheet, &format!("A{header_row}"), "name");
    set(sheet, &format!("A{}", header_row + 1), "Zorbina Quenzel");
    book
}

#[test]
fn header_in_row_20_is_found() {
    let dir = project(None);
    save(&book_with_header_in_row(20), dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    assert_ne!(value(&copy, "Daten", "A21"), "Zorbina Quenzel");
    assert_ne!(value(&copy, "Daten", "A21"), "");
}

#[test]
fn header_in_row_21_is_not_found_and_the_file_fails() {
    let dir = project(None);
    save(&book_with_header_in_row(21), dir.path(), "mappe.xlsx");
    let (out, stderr) = run_failing(dir.path());
    assert_error_line(&stderr, &format!("mappe.xlsx: sheet 1: {NO_HEADER_RULE}"));
    assert!(!out.path().join("mappe.xlsx").exists());
}

#[test]
fn failed_workbook_keeps_a_stale_copy_and_the_other_files_are_written() {
    let dir = project(None);
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    set(book.new_sheet("Daten").unwrap(), "A1", "Zorbina Quenzel");
    save(&book, dir.path(), "a_bad.xlsx");
    save(
        &simple_book("Tamsorin Velgrad", "tamsorin@acme.test"),
        dir.path(),
        "b_good.xlsx",
    );
    let out = TempDir::new().unwrap();
    write_file(out.path(), "a_bad.xlsx", "stale copy");
    let assert = run(dir.path(), out.path()).failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert_error_line(&stderr, &format!("a_bad.xlsx: sheet 1: {NO_HEADER_RULE}"));
    assert!(stderr.contains("1 file(s) failed"), "stderr: {stderr}");
    assert_eq!(
        fs::read(out.path().join("a_bad.xlsx")).unwrap(),
        b"stale copy"
    );
    let copy = open(out.path(), "b_good.xlsx");
    assert_ne!(value(&copy, "Daten", "A2"), "Tamsorin Velgrad");
}

#[test]
fn header_row_with_the_most_header_texts_wins_over_a_title_row() {
    let dir = project(None);
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    let sheet = book.new_sheet("Daten").unwrap();
    set(sheet, "A1", "Bericht");
    set(sheet, "C1", "email");
    set(sheet, "A2", "name");
    set(sheet, "B2", "email");
    set(sheet, "A3", "Zorbina Quenzel");
    set(sheet, "B3", "zorbina@acme.test");
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let copy = open(out.path(), "mappe.xlsx");
    assert_ne!(value(&copy, "Daten", "A3"), "Zorbina Quenzel");
    assert_ne!(value(&copy, "Daten", "B3"), "zorbina@acme.test");
    assert_eq!(value(&copy, "Daten", "A2"), "name");
}

#[test]
fn key_value_layout_has_two_equal_header_rows_and_fails() {
    let dir = project(None);
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    let sheet = book.new_sheet("Daten").unwrap();
    set(sheet, "A1", "name");
    set(sheet, "B1", "Zorbina Quenzel");
    set(sheet, "A2", "email");
    set(sheet, "B2", "Tamsorin Velgrad");
    save(&book, dir.path(), "mappe.xlsx");
    let (out, stderr) = run_failing(dir.path());
    assert_error_line(&stderr, "mappe.xlsx: sheet 1: header row is ambiguous");
    assert!(!out.path().join("mappe.xlsx").exists());
}

// ---- Input hardening (spec 0017) ----

const REL_BASE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const RELS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

/// A `.rels` part with `(type suffix, target)` relationships.
fn rels_xml(items: &[(&str, String)]) -> String {
    use std::fmt::Write as _;
    let mut text =
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"{RELS_NS}\">");
    for (index, (kind, target)) in items.iter().enumerate() {
        write!(
            text,
            "<Relationship Id=\"rId{}\" Type=\"{REL_BASE}/{kind}\" Target=\"{target}\"/>",
            index + 1
        )
        .unwrap();
    }
    text + "</Relationships>"
}

/// Rebuilds the package: `rename` gives the new name of each part, `edit` the
/// new text, `extra` parts are appended.
fn repackage(
    bytes: &[u8],
    rename: impl Fn(&str) -> String,
    edit: impl Fn(&str, &str) -> Option<String>,
    extra: &[(String, String)],
) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, text) in package_parts(bytes) {
        let text = edit(&name, &text).unwrap_or(text);
        writer.start_file(rename(&name), options).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    for (name, text) in extra {
        writer.start_file(name.as_str(), options).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn real_book_bytes() -> Vec<u8> {
    book_bytes(&simple_book("Zorbina Quenzel", "zorbina@acme.test"))
}

/// Puts `hostile` as `mappe.xlsx` and a valid `gut.xlsx` into a project and
/// runs. The run must fail for `mappe.xlsx` only, with exit code 1 (no abort).
/// Returns stderr; checks that `gut.xlsx` has its copy and `mappe.xlsx` none.
fn run_hostile(hostile: &[u8]) -> String {
    let dir = project(None);
    fs::write(dir.path().join("mappe.xlsx"), hostile).unwrap();
    run_hostile_project(&dir)
}

/// `run_hostile` for a project that has `mappe.xlsx` already.
fn run_hostile_project(dir: &TempDir) -> String {
    save(
        &simple_book("Tamsorin Velgrad", "tamsorin@acme.test"),
        dir.path(),
        "gut.xlsx",
    );
    let out = TempDir::new().unwrap();
    let assert = run(dir.path(), out.path()).failure();
    let output = assert.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(stderr.contains("mappe.xlsx"), "stderr: {stderr}");
    assert!(stderr.contains("1 file(s) failed"), "stderr: {stderr}");
    assert_no_leak(&stderr, "stderr");
    assert!(
        !out.path().join("mappe.xlsx").exists(),
        "a copy was written"
    );
    assert!(
        out.path().join("gut.xlsx").exists(),
        "no copy of the valid file"
    );
    stderr
}

/// The package plus a relationship part for `sheet1` that lists `items`.
fn with_sheet_rels(bytes: &[u8], items: &[(&str, String)], extra: &[(String, String)]) -> Vec<u8> {
    let mut parts = vec![(
        "xl/worksheets/_rels/sheet1.xml.rels".to_owned(),
        rels_xml(items),
    )];
    parts.extend(extra.iter().cloned());
    repackage(bytes, str::to_owned, |_, _| None, &parts)
}

#[test]
fn pivot_parts_under_other_names_are_refused() {
    let pivot = |name: &str, target: &str| {
        with_sheet_rels(
            &real_book_bytes(),
            &[("pivotTable", target.to_owned())],
            &[(name.to_owned(), "<pivotTableDefinition/>".to_owned())],
        )
    };
    let cases = [
        pivot(
            "xl/PivotCache/pivotCacheRecords1.xml",
            "../pivotCache/pivotCacheRecords1.xml",
        ),
        pivot(
            "xl\\pivotCache\\pivotCacheRecords1.xml",
            "../pivotCache/pivotCacheRecords1.xml",
        ),
        pivot(
            "xl/PIVOTTABLES/pivotTable1.xml",
            "../pivotTables/pivotTable1.xml",
        ),
        // Only the relationship type says pivot.
        pivot("xl/data/p.xml", "../data/p.xml"),
    ];
    for (index, case) in cases.iter().enumerate() {
        let stderr = run_hostile(case);
        assert!(stderr.contains("pivot"), "case {index}, stderr: {stderr}");
    }
}

#[test]
fn pivot_content_type_alone_is_refused() {
    let types = "<Override PartName=\"/xl/data/p.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.pivotTable+xml\"/></Types>";
    let bytes = repackage(
        &real_book_bytes(),
        str::to_owned,
        |name, text| (name == "[Content_Types].xml").then(|| text.replacen("</Types>", types, 1)),
        &[("xl/data/p.xml".to_owned(), "<x/>".to_owned())],
    );
    let stderr = run_hostile(&bytes);
    assert!(stderr.contains("pivot"), "stderr: {stderr}");
}

/// A package where the sheet part has the name `part`, the workbook
/// relationship points to `target`, and the sheet text is `sheet`.
fn moved_sheet(part: &str, target: &str, sheet: &str) -> Vec<u8> {
    repackage(
        &real_book_bytes(),
        |name| {
            if name == "xl/worksheets/sheet1.xml" {
                part.to_owned()
            } else {
                name.to_owned()
            }
        },
        |name, text| match name {
            "xl/_rels/workbook.xml.rels" => Some(text.replace("worksheets/sheet1.xml", target)),
            "xl/worksheets/sheet1.xml" => Some(sheet.to_owned()),
            _ => None,
        },
        &[],
    )
}

fn sheet_with_header() -> String {
    let header = "<headerFooter><oddHeader>&amp;LZorbina Quenzel</oddHeader></headerFooter>";
    let sheet = package_parts(&real_book_bytes())
        .into_iter()
        .find(|(name, _)| name == "xl/worksheets/sheet1.xml")
        .unwrap()
        .1;
    sheet.replacen("</worksheet>", &format!("{header}</worksheet>"), 1)
}

#[test]
fn header_in_a_sheet_with_another_name_gets_the_warning() {
    let sheet = sheet_with_header();
    let cases = [
        ("xl/Worksheets/sheet1.xml", "worksheets/sheet1.xml"),
        ("xl/data/s.xml", "data/s.xml"),
    ];
    for (part, target) in cases {
        let dir = project(None);
        fs::write(
            dir.path().join("mappe.xlsx"),
            moved_sheet(part, target, &sheet),
        )
        .unwrap();
        let (out, _, stderr) = run_ok(dir.path());
        assert!(out.path().join("mappe.xlsx").exists(), "{part}");
        let line = stderr
            .lines()
            .find(|line| line.contains("not pseudonymized"))
            .unwrap_or_default();
        assert!(line.contains("headers/footers"), "{part}, stderr: {stderr}");
        assert_no_leak(&stderr, "stderr");
    }
}

#[test]
fn prefixed_shared_strings_under_another_name_are_refused() {
    let strings = ["name", "email", "Zorbina Quenzel", "zorbina@acme.test"];
    let items = strings
        .map(|text| format!("<x:si><x:t>{text}</x:t></x:si>"))
        .concat();
    let sst = format!("<x:sst xmlns:x=\"{MAIN_NS}\">{items}</x:sst>");
    let bytes = repackage(
        &real_book_bytes(),
        |name| {
            if name == "xl/sharedStrings.xml" {
                "xl/SharedStrings.xml".to_owned()
            } else {
                name.to_owned()
            }
        },
        |name, _| (name == "xl/sharedStrings.xml").then(|| sst.clone()),
        &[],
    );
    let dir = project(None);
    fs::write(dir.path().join("mappe.xlsx"), bytes).unwrap();
    run_refused(&dir, "mappe.xlsx");
}

#[test]
fn inline_string_with_two_runs_under_another_name_is_refused() {
    let sheet = sheet_with(concat!(
        "<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>name</t></is></c></row>",
        "<row r=\"2\"><c r=\"A2\" t=\"inlineStr\"><is><r><t>Zorbina</t></r><r><t> Quenzel</t></r></is></c></row>"
    ));
    for (part, target) in [
        ("xl/Worksheets/sheet1.xml", "worksheets/sheet1.xml"),
        ("xl/data/s.xml", "data/s.xml"),
    ] {
        let dir = project(None);
        fs::write(
            dir.path().join("mappe.xlsx"),
            moved_sheet(part, target, &sheet),
        )
        .unwrap();
        run_refused(&dir, "mappe.xlsx");
    }
}

#[test]
fn relationship_cycle_fails_the_file_and_the_run_goes_on() {
    // sheet1 -> sheet1 overflowed the stack of the crate.
    let cycle = with_sheet_rels(
        &real_book_bytes(),
        &[("worksheet", "sheet1.xml".to_owned())],
        &[],
    );
    let stderr = run_hostile(&cycle);
    assert!(stderr.contains("relationship cycle"), "stderr: {stderr}");

    // A longer loop: sheet1 -> a -> b -> a.
    let parts: Vec<(String, String)> = vec![
        ("xl/worksheets/a.xml".into(), "<x/>".into()),
        ("xl/worksheets/b.xml".into(), "<x/>".into()),
        (
            "xl/worksheets/_rels/a.xml.rels".into(),
            rels_xml(&[("worksheet", "b.xml".into())]),
        ),
        (
            "xl/worksheets/_rels/b.xml.rels".into(),
            rels_xml(&[("worksheet", "a.xml".into())]),
        ),
    ];
    let looped = with_sheet_rels(
        &real_book_bytes(),
        &[("worksheet", "a.xml".to_owned())],
        &parts,
    );
    let stderr = run_hostile(&looped);
    assert!(stderr.contains("relationship cycle"), "stderr: {stderr}");
}

/// Parts p0..p{count-1}; each has two relationships to the next one.
fn fan_out_parts(count: usize) -> Vec<(String, String)> {
    let mut parts = Vec::new();
    for index in 0..count {
        parts.push((format!("xl/worksheets/p{index}.xml"), "<x/>".to_owned()));
        let next = format!("p{}.xml", index + 1);
        parts.push((
            format!("xl/worksheets/_rels/p{index}.xml.rels"),
            rels_xml(&[("worksheet", next.clone()), ("worksheet", next)]),
        ));
    }
    parts
}

#[test]
fn fan_out_chain_fails_fast() {
    let bytes = with_sheet_rels(
        &real_book_bytes(),
        &[("worksheet", "p0.xml".to_owned())],
        &fan_out_parts(40),
    );
    let start = std::time::Instant::now();
    let stderr = run_hostile(&bytes);
    assert!(start.elapsed() < std::time::Duration::from_secs(60));
    assert!(stderr.contains("nested too deeply"), "stderr: {stderr}");

    // 14 levels pass the depth limit; 2^14 references pass the count limit.
    let bytes = with_sheet_rels(
        &real_book_bytes(),
        &[("worksheet", "p0.xml".to_owned())],
        &fan_out_parts(14),
    );
    let start = std::time::Instant::now();
    let stderr = run_hostile(&bytes);
    assert!(start.elapsed() < std::time::Duration::from_secs(60));
    assert!(stderr.contains("too large"), "stderr: {stderr}");
}

#[test]
fn deep_relationship_chain_fails() {
    // 20 parts in a line, one relationship each.
    let mut parts = Vec::new();
    for index in 0..20 {
        parts.push((format!("xl/worksheets/p{index}.xml"), "<x/>".to_owned()));
        parts.push((
            format!("xl/worksheets/_rels/p{index}.xml.rels"),
            rels_xml(&[("worksheet", format!("p{}.xml", index + 1))]),
        ));
    }
    let bytes = with_sheet_rels(
        &real_book_bytes(),
        &[("worksheet", "p0.xml".to_owned())],
        &parts,
    );
    let stderr = run_hostile(&bytes);
    assert!(stderr.contains("nested too deeply"), "stderr: {stderr}");
}

#[test]
fn many_missing_targets_fail_without_a_long_scan() {
    // 9 000 relationships to parts that do not exist, 9 000 other parts.
    let missing: Vec<(&str, String)> = (0..9_000)
        .map(|index| ("image", format!("missing{index}.png")))
        .collect();
    let extra: Vec<(String, String)> = (0..9_000)
        .map(|index| (format!("xl/media/m{index}.bin"), String::new()))
        .collect();
    let bytes = with_sheet_rels(&real_book_bytes(), &missing, &extra);
    let start = std::time::Instant::now();
    let stderr = run_hostile(&bytes);
    assert!(start.elapsed() < std::time::Duration::from_secs(60));
    assert!(stderr.contains("too large"), "stderr: {stderr}");
}

#[test]
fn too_many_zip_entries_fail() {
    let extra: Vec<(String, String)> = (0..10_001)
        .map(|index| (format!("xl/media/m{index}.bin"), String::new()))
        .collect();
    let bytes = repackage(&real_book_bytes(), str::to_owned, |_, _| None, &extra);
    let stderr = run_hostile(&bytes);
    assert!(stderr.contains("too large"), "stderr: {stderr}");
}

#[test]
fn shared_references_to_one_big_part_count_each_time() {
    // 1 MiB of blanks (a few hundred bytes in the file), referenced 600 times:
    // 600 MiB for the crate, which reads the part for each reference.
    let big = ("xl/media/big.bin".to_owned(), " ".repeat(1024 * 1024));
    let many: Vec<(&str, String)> = (0..600)
        .map(|_| ("image", "../media/big.bin".to_owned()))
        .collect();
    let bytes = with_sheet_rels(&real_book_bytes(), &many, std::slice::from_ref(&big));
    let stderr = run_hostile(&bytes);
    assert!(stderr.contains("too large"), "stderr: {stderr}");

    // The same part with a few references is fine.
    let few: Vec<(&str, String)> = (0..3)
        .map(|_| ("image", "../media/big.bin".to_owned()))
        .collect();
    let bytes = with_sheet_rels(&real_book_bytes(), &few, &[big]);
    let dir = project(None);
    fs::write(dir.path().join("mappe.xlsx"), bytes).unwrap();
    let (out, _, _) = run_ok(dir.path());
    assert!(out.path().join("mappe.xlsx").exists());
}

// ---- Canonical package (spec 0017, review of 2026-11-03) ----

/// A `.rels` part with `body` as the text between the root tags.
fn rels_body(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"{RELS_NS}\">{body}</Relationships>"
    )
}

fn external_rel(id: &str) -> String {
    format!(
        "<Relationship Id=\"{id}\" Type=\"{REL_BASE}/hyperlink\" Target=\"https://example.test/\" TargetMode=\"External\"/>"
    )
}

/// The package with `rels` as the relationships of `sheet1` and `extra` parts.
fn with_sheet_rels_text(rels: &str) -> Vec<u8> {
    repackage(
        &real_book_bytes(),
        str::to_owned,
        |_, _| None,
        &[(
            "xl/worksheets/_rels/sheet1.xml.rels".to_owned(),
            rels_body(rels),
        )],
    )
}

#[test]
fn external_loops_through_the_root_rels_name_are_refused() {
    // The crate looks up `/_rels/.rels` for an External relationship.
    for root in ["/_rels/.rels", "\\_rels\\.rels"] {
        let rels = rels_body(&external_rel("rId1"));
        let bytes = repackage(
            &real_book_bytes(),
            str::to_owned,
            |_, _| None,
            &[
                (
                    "xl/worksheets/_rels/sheet1.xml.rels".to_owned(),
                    rels.clone(),
                ),
                (root.to_owned(), rels),
            ],
        );
        let stderr = run_hostile(&bytes);
        assert!(stderr.contains("ambiguous part names"), "{root}: {stderr}");
    }
}

#[test]
fn backslash_and_case_variant_names_are_refused() {
    let sheet = sheet_with_header();
    let bytes = moved_sheet(
        "xl\\worksheets\\sheet1.xml",
        "worksheets/sheet1.xml",
        &sheet,
    );
    let stderr = run_hostile(&bytes);
    assert!(stderr.contains("ambiguous part names"), "{stderr}");

    // Two names that differ only in ASCII case.
    let bytes = repackage(
        &real_book_bytes(),
        str::to_owned,
        |_, _| None,
        &[("xl/Workbook.xml".to_owned(), "<workbook/>".to_owned())],
    );
    let stderr = run_hostile(&bytes);
    assert!(stderr.contains("ambiguous part names"), "{stderr}");
}

#[test]
fn two_entry_names_that_decode_to_the_same_text_are_refused() {
    // `sX` is patched to the CP437 byte 0x82 (e acute): the same text as the
    // UTF-8 name of the other entry, so the crate cannot tell them apart.
    let cyclic = rels_xml(&[("worksheet", "s\u{e9}.xml".to_owned())]);
    let mut bytes = repackage(
        &real_book_bytes(),
        |name| {
            if name == "xl/worksheets/sheet1.xml" {
                "xl/worksheets/s\u{e9}.xml".to_owned()
            } else {
                name.to_owned()
            }
        },
        |name, text| {
            (name == "xl/_rels/workbook.xml.rels")
                .then(|| text.replace("worksheets/sheet1.xml", "worksheets/s\u{e9}.xml"))
        },
        &[
            ("xl/worksheets/_rels/sX.xml.rels".to_owned(), rels_body("")),
            ("xl/worksheets/_rels/s\u{e9}.xml.rels".to_owned(), cyclic),
        ],
    );
    let from = b"_rels/sX.xml.rels";
    let to = b"_rels/s\x82.xml.rels";
    let mut at = 0;
    while let Some(found) = bytes[at..].windows(from.len()).position(|w| w == from) {
        let start = at + found;
        bytes[start..start + to.len()].copy_from_slice(to);
        at = start + to.len();
    }
    let stderr = run_hostile(&bytes);
    assert!(stderr.contains("ambiguous part names"), "{stderr}");
}

#[test]
fn unusual_relationship_parts_are_refused() {
    let worksheet = format!("Type=\"{REL_BASE}/worksheet\"");
    let cases = [
        // An entity in the mode or in an internal target.
        format!(
            "<Relationship Id=\"rId1\" {worksheet} Target=\"sheet1.xml\" TargetMode=\"&#69;xternal\"/>"
        ),
        format!("<Relationship Id=\"rId1\" {worksheet} Target=\"s&amp;1.xml\"/>"),
        // The only Target has a prefix: the crate reads none.
        format!(
            "<Relationship xmlns:q=\"urn:q\" Id=\"rId1\" {worksheet} q:Target=\"sheet1.xml\"/>"
        ),
        // A repeated attribute, a repeated id, a missing attribute.
        format!("<Relationship Id=\"rId1\" {worksheet} Target=\"a.xml\" Target=\"b.xml\"/>"),
        format!(
            "<Relationship Id=\"rId1\" {worksheet} Target=\"a.xml\"/><Relationship Id=\"rId1\" {worksheet} Target=\"b.xml\"/>"
        ),
        format!("<Relationship Id=\"rId1\" {worksheet}/>"),
        // Another element name or a prefixed one.
        format!("<Other Id=\"rId1\" {worksheet} Target=\"a.xml\"/>"),
        format!("<x:Relationship xmlns:x=\"{RELS_NS}\" Id=\"rId1\" {worksheet} Target=\"a.xml\"/>"),
    ];
    for (index, body) in cases.iter().enumerate() {
        let stderr = run_hostile(&with_sheet_rels_text(body));
        assert!(
            stderr.contains("cannot read the workbook"),
            "case {index}: {stderr}"
        );
    }
}

#[test]
fn relationships_are_read_as_the_crate_reads_them() {
    // The crate reads `Target`, not `q:Target`: the cycle through sheet1 is real.
    let body = format!(
        "<Relationship xmlns:q=\"urn:q\" Id=\"rId1\" Type=\"{REL_BASE}/worksheet\" q:Target=\"none.xml\" Target=\"sheet1.xml\"/>"
    );
    let stderr = run_hostile(&with_sheet_rels_text(&body));
    assert!(stderr.contains("relationship cycle"), "{stderr}");

    // The crate skips `<Relationship>..</Relationship>`; the copy of the
    // package has it as an empty element, so the check sees it too.
    let body = format!(
        "<Relationship Id=\"rId1\" Type=\"{REL_BASE}/worksheet\" Target=\"sheet1.xml\"></Relationship>"
    );
    let stderr = run_hostile(&with_sheet_rels_text(&body));
    assert!(stderr.contains("relationship cycle"), "{stderr}");
}

#[test]
fn sheets_with_one_relationship_id_are_refused() {
    let sheets = (0..300)
        .map(|index| {
            format!(
                "<sheet name=\"S{index}\" sheetId=\"{}\" r:id=\"rId1\"/>",
                index + 1
            )
        })
        .collect::<Vec<_>>()
        .concat();
    let bytes = repackage(
        &real_book_bytes(),
        str::to_owned,
        |name, text| {
            (name == "xl/workbook.xml").then(|| {
                let start = text.find("<sheets>").unwrap();
                let end = text.find("</sheets>").unwrap();
                format!("{}<sheets>{sheets}{}", &text[..start], &text[end..])
            })
        },
        &[],
    );
    let start = std::time::Instant::now();
    let stderr = run_hostile(&bytes);
    assert!(start.elapsed() < std::time::Duration::from_secs(60));
    assert!(
        stderr.contains("one relationship for two sheets"),
        "{stderr}"
    );
}

#[test]
fn drawing_with_2500_images_gets_a_copy() {
    let images: Vec<(&str, String)> = (1..=2_500)
        .map(|index| ("image", format!("../media/image{index}.png")))
        .collect();
    let drawing = "<xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\"/>";
    let mut parts = vec![
        ("xl/drawings/drawing1.xml".to_owned(), drawing.to_owned()),
        (
            "xl/drawings/_rels/drawing1.xml.rels".to_owned(),
            rels_xml(&images),
        ),
    ];
    parts.extend((1..=2_500).map(|index| (format!("xl/media/image{index}.png"), "PNG".to_owned())));
    let bytes = with_sheet_rels(
        &real_book_bytes(),
        &[("drawing", "../drawings/drawing1.xml".to_owned())],
        &parts,
    );
    // Stored entries: the debug build of the crate is slow to open deflated ones.
    let bytes = stored(&bytes);
    let dir = project(None);
    fs::write(dir.path().join("mappe.xlsx"), bytes).unwrap();
    let (out, _, _) = run_ok(dir.path());
    assert!(out.path().join("mappe.xlsx").exists());
}

/// The same package with stored (not compressed) entries.
fn stored(bytes: &[u8]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, text) in package_parts(bytes) {
        writer.start_file(name, options).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn many_external_relationships_and_many_entries_fail_fast() {
    // 9 900 External relationships: the crate scans all names for each one.
    let rels: String = (1..=9_900)
        .map(|index| external_rel(&format!("rId{index}")))
        .collect();
    let mut extra = vec![(
        "xl/worksheets/_rels/sheet1.xml.rels".to_owned(),
        rels_body(&rels),
    )];
    extra.extend((0..9_900).map(|index| (format!("xl/media/m{index}.bin"), String::new())));
    let bytes = repackage(&real_book_bytes(), str::to_owned, |_, _| None, &extra);
    let start = std::time::Instant::now();
    let stderr = run_hostile(&bytes);
    assert!(start.elapsed() < std::time::Duration::from_secs(60));
    assert!(stderr.contains("too large"), "{stderr}");
}

#[test]
fn external_hyperlink_with_an_entity_and_foreign_attributes_gets_a_copy() {
    let body = format!(
        "<Relationship Id=\"rId1\" Type=\"{REL_BASE}/hyperlink\" Target=\"https://example.test/?a=1&amp;b=2\" TargetMode=\"External\" xmlns:q=\"urn:q\" q:Note=\"x\"/>"
    );
    let dir = project(None);
    fs::write(dir.path().join("mappe.xlsx"), with_sheet_rels_text(&body)).unwrap();
    let (out, _, _) = run_ok(dir.path());
    assert!(out.path().join("mappe.xlsx").exists());
}

#[test]
fn many_references_to_one_external_link_part_finish_fast() {
    let references = "<externalReference r:id=\"rId99\"/>".repeat(10_000);
    let link = format!(
        "<externalLink xmlns=\"{MAIN_NS}\"><externalBook/>{}</externalLink>",
        " ".repeat(4 * 1024 * 1024)
    );
    let link_rel = format!(
        "<Relationship Id=\"rId99\" Type=\"{REL_BASE}/externalLink\" Target=\"externalLinks/externalLink1.xml\"/></Relationships>"
    );
    let bytes = repackage(
        &real_book_bytes(),
        str::to_owned,
        |name, text| match name {
            "xl/workbook.xml" => Some(text.replacen(
                "</sheets>",
                &format!("</sheets><externalReferences>{references}</externalReferences>"),
                1,
            )),
            "xl/_rels/workbook.xml.rels" => Some(text.replacen("</Relationships>", &link_rel, 1)),
            _ => None,
        },
        &[("xl/externalLinks/externalLink1.xml".to_owned(), link)],
    );
    let dir = project(None);
    fs::write(dir.path().join("mappe.xlsx"), bytes).unwrap();
    let start = std::time::Instant::now();
    let out = TempDir::new().unwrap();
    let output = pseudofy()
        .args(["run", "--project"])
        .arg(dir.path())
        .arg("--out")
        .arg(out.path())
        .output()
        .unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(60));
    // Either a copy or a plain refusal, never an abort.
    assert!(matches!(output.status.code(), Some(0 | 1)));
}

#[test]
fn xlsx_file_above_512_mib_is_refused_before_it_is_read() {
    let dir = project(None);
    // A sparse file: it takes no disk space and is never read.
    let file = fs::File::create(dir.path().join("mappe.xlsx")).unwrap();
    file.set_len(512 * 1024 * 1024 + 1).unwrap();
    drop(file);
    let stderr = run_hostile_project(&dir);
    assert!(
        stderr.contains("mappe.xlsx: file is larger than 512 MiB"),
        "{stderr}"
    );
}

#[test]
fn package_with_one_600_mib_entry_fails_at_once() {
    // Streamed zeros: some hundred KiB in the file, 600 MiB for the crate.
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().large_file(true);
    for (name, text) in package_parts(&real_book_bytes()) {
        writer.start_file(name, options).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.start_file("xl/media/big.bin", options).unwrap();
    let chunk = vec![0u8; 1024 * 1024];
    for _ in 0..600 {
        writer.write_all(&chunk).unwrap();
    }
    let bytes = writer.finish().unwrap().into_inner();
    assert!(bytes.len() < 4 * 1024 * 1024);
    let start = std::time::Instant::now();
    let stderr = run_hostile(&bytes);
    assert!(start.elapsed() < std::time::Duration::from_secs(60));
    assert!(stderr.contains("too large"), "{stderr}");
}

// ---- Third review (spec 0017, 2026-11-03) ----

/// The valid book with `edit` applied to the text of its parts.
fn edited_book(edit: impl Fn(&str, &str) -> Option<String>) -> Vec<u8> {
    repackage(&real_book_bytes(), str::to_owned, edit, &[])
}

/// `run_hostile`, and the run must end within `seconds`.
fn run_hostile_within(hostile: &[u8], seconds: u64) -> String {
    let start = std::time::Instant::now();
    let stderr = run_hostile(hostile);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(seconds),
        "took {:?}",
        start.elapsed()
    );
    stderr
}

#[test]
fn greater_than_in_a_target_cannot_hide_a_cycle() {
    // The sheet is `s&gt;1.xml` in the archive. The package writer escaped
    // `>` as `&gt;` and the crate read it raw: it walked another graph than
    // the check and overflowed its stack on the loop of this sheet.
    for (part, rels_part) in [
        (
            "xl/worksheets/s&gt;1.xml",
            "xl/worksheets/_rels/s&gt;1.xml.rels",
        ),
        ("xl/worksheets/s>1.xml", "xl/worksheets/_rels/s>1.xml.rels"),
    ] {
        let bytes = repackage(
            &real_book_bytes(),
            |name| {
                if name == "xl/worksheets/sheet1.xml" {
                    part.to_owned()
                } else {
                    name.to_owned()
                }
            },
            |name, text| {
                (name == "xl/_rels/workbook.xml.rels")
                    .then(|| text.replace("worksheets/sheet1.xml", "worksheets/s>1.xml"))
            },
            &[(
                rels_part.to_owned(),
                rels_xml(&[("drawing", "s>1.xml".to_owned())]),
            )],
        );
        let stderr = run_hostile_within(&bytes, 60);
        assert!(!stderr.contains("overflow"), "{part}: {stderr}");
    }
}

/// The book with the shared strings `sst`, the styles `styles` and the rows
/// `rows` (the content of `<sheetData>`).
fn custom_book(sst: &str, styles: &str, rows: &str) -> Vec<u8> {
    edited_book(|name, text| match name {
        "xl/sharedStrings.xml" => Some(sst.to_owned()),
        "xl/styles.xml" => Some(styles.to_owned()),
        "xl/worksheets/sheet1.xml" => {
            let start = text.find("<sheetData>")? + "<sheetData>".len();
            let end = text.find("</sheetData>")?;
            Some(format!("{}{rows}{}", &text[..start], &text[end..]))
        }
        _ => None,
    })
}

const STYLES_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">";

fn sst_of(text: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><si><t>{text}</t></si></sst>"
    )
}

/// Styles with one font, `fill`, one border, and `xfs` cell formats; format 1
/// uses the fill 1.
fn styles_of(font_name: &str, fill: &str, format_code: &str, xfs: usize) -> String {
    let formats = if format_code.is_empty() {
        String::new()
    } else {
        format!(
            "<numFmts count=\"1\"><numFmt numFmtId=\"164\" formatCode=\"{format_code}\"/></numFmts>"
        )
    };
    format!(
        "{STYLES_HEAD}{formats}<fonts count=\"1\"><font><sz val=\"11\"/><name val=\"{font_name}\"/></font></fonts>\
         <fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill>{fill}</fill></fills>\
         <borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders>\
         <cellXfs count=\"{xfs}\">{}</cellXfs></styleSheet>",
        "<xf numFmtId=\"0\" fontId=\"0\" fillId=\"1\" borderId=\"0\" xfId=\"0\"/>".repeat(xfs)
    )
}

/// `count` rows with one cell each; `attrs` goes into the cell.
fn cell_rows(count: usize, attrs: &str, value: &str) -> String {
    use std::fmt::Write as _;
    (1..=count).fold(String::new(), |mut text, row| {
        write!(
            text,
            "<row r=\"{row}\"><c r=\"A{row}\"{attrs}><v>{value}</v></c></row>"
        )
        .unwrap();
        text
    })
}

const PLAIN_FILL: &str = "<patternFill patternType=\"gray125\"/>";

/// A header row and a data row (inline strings), then `extra` cells in row 3.
fn valid_rows(extra: &str) -> String {
    format!(
        "<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>name</t></is></c><c r=\"B1\" t=\"inlineStr\"><is><t>email</t></is></c></row>\
         <row r=\"2\"><c r=\"A2\" t=\"inlineStr\"><is><t>Zorbina Quenzel</t></is></c><c r=\"B2\" t=\"inlineStr\"><is><t>zorbina@acme.test</t></is></c></row>\
         <row r=\"3\">{extra}</row>"
    )
}

#[test]
fn four_mib_shared_string_in_2000_cells_is_refused_without_an_abort() {
    // The crate copies the text into each cell: 8 GB.
    let text = "q".repeat(4 * 1024 * 1024);
    let bytes = custom_book(
        &sst_of(&text),
        &styles_of("Calibri", PLAIN_FILL, "", 2),
        &cell_rows(2_000, " t=\"s\"", "0"),
    );
    let stderr = run_hostile_within(&bytes, 60);
    assert!(stderr.contains("longer than Excel allows"), "{stderr}");
}

#[test]
fn shared_string_above_32767_characters_is_refused_and_32767_is_not() {
    let rows = cell_rows(1, " t=\"s\"", "0");
    let styles = styles_of("Calibri", PLAIN_FILL, "", 2);
    let bytes = custom_book(&sst_of(&"q".repeat(32_768)), &styles, &rows);
    let stderr = run_hostile(&bytes);
    assert!(stderr.contains("longer than Excel allows"), "{stderr}");

    // A character that the text writes as an entity counts once.
    let entities = "&amp;".repeat(32_768);
    let bytes = custom_book(&sst_of(&entities), &styles, &rows);
    let stderr = run_hostile(&bytes);
    assert!(stderr.contains("longer than Excel allows"), "{stderr}");

    let dir = project(None);
    fs::write(
        dir.path().join("mappe.xlsx"),
        custom_book(
            &sst_of(&"q".repeat(32_767)),
            &styles,
            &valid_rows("<c r=\"C3\" t=\"s\"><v>0</v></c>"),
        ),
    )
    .unwrap();
    let (out, _, _) = run_ok(dir.path());
    assert!(out.path().join("mappe.xlsx").exists());
}

#[test]
fn shared_string_copies_above_one_gib_are_refused() {
    // 40 000 cells with a 32 000 character string: 1.3 GB of copies, from a
    // package of a few hundred KiB.
    let bytes = custom_book(
        &sst_of(&"q".repeat(32_000)),
        &styles_of("Calibri", PLAIN_FILL, "", 2),
        &cell_rows(40_000, " t=\"s\"", "0"),
    );
    assert!(bytes.len() < 1024 * 1024, "{}", bytes.len());
    let stderr = run_hostile_within(&bytes, 60);
    assert!(stderr.contains("workbook is too large"), "{stderr}");
}

#[test]
fn four_mib_font_name_in_2000_cell_formats_is_refused_without_an_abort() {
    let text = "f".repeat(4 * 1024 * 1024);
    let bytes = custom_book(
        &sst_of("x"),
        &styles_of(&text, PLAIN_FILL, "", 2_000),
        &cell_rows(2_000, " s=\"1\"", "1"),
    );
    let stderr = run_hostile_within(&bytes, 60);
    assert!(stderr.contains("longer than Excel allows"), "{stderr}");
}

#[test]
fn font_name_and_number_format_limits_of_excel_are_refused() {
    let rows = cell_rows(1, " s=\"1\"", "1");
    for (font, format_code) in [
        ("f".repeat(32), String::new()),
        ("Calibri".to_owned(), "0".repeat(256)),
    ] {
        let bytes = custom_book(
            &sst_of("x"),
            &styles_of(&font, PLAIN_FILL, &format_code, 2),
            &rows,
        );
        let stderr = run_hostile(&bytes);
        assert!(stderr.contains("longer than Excel allows"), "{stderr}");
    }
    // The limits themselves are valid.
    let dir = project(None);
    fs::write(
        dir.path().join("mappe.xlsx"),
        custom_book(
            &sst_of("x"),
            &styles_of(&"f".repeat(31), PLAIN_FILL, &"0".repeat(255), 2),
            &valid_rows("<c r=\"C3\" s=\"1\"><v>1</v></c>"),
        ),
    )
    .unwrap();
    let (out, _, _) = run_ok(dir.path());
    assert!(out.path().join("mappe.xlsx").exists());
}

#[test]
fn cell_formats_that_expand_above_one_gib_are_refused() {
    // A 1 MiB fill in the format of 1 200 cells: 1.2 GB of copies.
    let fill = format!(
        "<patternFill patternType=\"gray125\"/><!--{}-->",
        "a".repeat(1 << 20)
    );
    let bytes = custom_book(
        &sst_of("x"),
        &styles_of("Calibri", &fill, "", 2),
        &cell_rows(1_200, " s=\"1\"", "1"),
    );
    assert!(bytes.len() < 1024 * 1024, "{}", bytes.len());
    let stderr = run_hostile_within(&bytes, 60);
    assert!(stderr.contains("workbook is too large"), "{stderr}");
}

#[test]
fn many_sheet_elements_fail_fast() {
    use std::fmt::Write as _;
    let sheets = (0..100_000).fold(String::new(), |mut text, index| {
        write!(
            text,
            "<sheet name=\"S{index}\" sheetId=\"{}\" r:id=\"x{index}\"/>",
            index + 2
        )
        .unwrap();
        text
    });
    let bytes = edited_book(|name, text| {
        (name == "xl/workbook.xml")
            .then(|| text.replace("</sheets>", &format!("{sheets}</sheets>")))
    });
    let stderr = run_hostile_within(&bytes, 60);
    assert!(stderr.contains("workbook has too many sheets"), "{stderr}");
}

#[test]
fn decoy_id_attribute_does_not_hide_a_sheet_from_the_layout_check() {
    // The crate reads `r:id`; `id="rId9"` before it must not change which
    // sheet pseudofy scans for the two-run inline string.
    let sheet = sheet_with(concat!(
        "<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>name</t></is></c></row>",
        "<row r=\"2\"><c r=\"A2\" t=\"inlineStr\"><is><r><t>Zorbina</t></r><r><t> Quenzel</t></r></is></c></row>"
    ));
    for decoy in ["", " id=\"rId9\""] {
        let moved = moved_sheet("xl/data/s.xml", "data/s.xml", &sheet);
        let bytes = repackage(
            &moved,
            str::to_owned,
            |name, text| {
                (name == "xl/workbook.xml")
                    .then(|| text.replace("r:id=\"rId1\"", &format!("{decoy} r:id=\"rId1\"")))
            },
            &[],
        );
        let dir = project(None);
        fs::write(dir.path().join("mappe.xlsx"), bytes).unwrap();
        run_refused(&dir, "mappe.xlsx");
    }
}

// ---- Spec 0018: warnings for parts that keep text, and hyperlink targets ----

/// The `not pseudonymized` warning line of a run.
fn kinds_line(stderr: &str) -> String {
    stderr
        .lines()
        .find(|line| line.contains("not pseudonymized"))
        .unwrap_or_default()
        .to_owned()
}

/// Runs a project with `mappe.xlsx` made from `book`; returns the output
/// directory and the warning line.
fn warning_of(book: &Workbook) -> (TempDir, String) {
    let dir = project(None);
    save(book, dir.path(), "mappe.xlsx");
    let (out, _, stderr) = run_ok(dir.path());
    assert_no_leak(&stderr, "stderr");
    assert_well_formed(&out.path().join("mappe.xlsx"));
    (out, kinds_line(&stderr))
}

#[test]
fn formula_with_text_gets_the_formulas_warning() {
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    book.sheet_by_name_mut("Daten")
        .unwrap()
        .cell_mut("C2")
        .set_formula("\"Tamsorin \"&\"Velgrad\"");
    let (out, line) = warning_of(&book);
    assert!(line.contains("mappe.xlsx"), "line: {line}");
    assert!(line.contains("formulas"), "line: {line}");
    // The formula text stays (out of scope); the warning is the only note.
    let copy = open(out.path(), "mappe.xlsx");
    let sheet = copy.sheet_by_name("Daten").unwrap();
    assert!(sheet.cell("C2").unwrap().formula().contains("Velgrad"));
}

#[test]
fn text_box_in_a_drawing_gets_the_drawings_warning() {
    let bytes = real_book_bytes();
    let drawing = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><xdr:twoCellAnchor><xdr:from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>6</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>5</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"2\" name=\"Box 1\"/><xdr:cNvSpPr txBox=\"1\"/></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang=\"de-DE\"/><a:t>Ansprechpartner Tamsorin Velgrad</a:t></a:r></a:p></xdr:txBody></xdr:sp><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>".to_owned();
    let rels = rels_xml(&[("drawing", "../drawings/drawing1.xml".to_owned())]);
    let types = "<Override PartName=\"/xl/drawings/drawing1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawing+xml\"/></Types>";
    let edited = repackage(
        &bytes,
        str::to_owned,
        |name, text| {
            match name {
            "[Content_Types].xml" => Some(text.replacen("</Types>", types, 1)),
            "xl/worksheets/sheet1.xml" => Some(text.replacen(
                "</worksheet>",
                "<drawing xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" r:id=\"rId1\"/></worksheet>",
                1,
            )),
            _ => None,
        }
        },
        &[
            ("xl/drawings/drawing1.xml".to_owned(), drawing),
            ("xl/worksheets/_rels/sheet1.xml.rels".to_owned(), rels),
        ],
    );
    let dir = project(None);
    fs::write(dir.path().join("mappe.xlsx"), edited).unwrap();
    let (out, _, stderr) = run_ok(dir.path());
    assert_well_formed(&out.path().join("mappe.xlsx"));
    let line = kinds_line(&stderr);
    assert!(line.contains("drawings"), "stderr: {stderr}");
    assert_no_leak(&stderr, "stderr");
}

#[test]
fn table_with_a_total_label_gets_the_tables_warning() {
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let sheet = book.sheet_by_name_mut("Daten").unwrap();
    let mut table = umya_spreadsheet::Table::new("Tabelle1", ("A1", "B3"));
    let mut name = umya_spreadsheet::TableColumn::new("name");
    name.set_totals_row_label("Summe Quenzel");
    table.add_column(name);
    table.add_column(umya_spreadsheet::TableColumn::new("email"));
    table.set_totals_row_count(1);
    sheet.add_table(table);
    let (_out, line) = warning_of(&book);
    assert!(line.contains("tables"), "line: {line}");
}

#[test]
fn number_format_with_literal_text_gets_the_number_formats_warning() {
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let sheet = book.sheet_by_name_mut("Daten").unwrap();
    sheet.cell_mut("C2").set_value_number(5);
    sheet
        .cell_mut("C2")
        .style_mut()
        .number_format_mut()
        .set_format_code("\"Quenzel-Format \"0");
    let (_out, line) = warning_of(&book);
    assert!(line.contains("number formats"), "line: {line}");
}

#[test]
fn plain_number_format_gives_no_number_formats_warning() {
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let sheet = book.sheet_by_name_mut("Daten").unwrap();
    sheet.cell_mut("C2").set_value_number(5);
    sheet
        .cell_mut("C2")
        .style_mut()
        .number_format_mut()
        .set_format_code("#,##0.00");
    let (_out, line) = warning_of(&book);
    assert!(!line.contains("number formats"), "line: {line}");
}

#[test]
fn embedded_object_gets_the_embedded_objects_warning() {
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let sheet = book.sheet_by_name_mut("Daten").unwrap();
    let mut object = umya_spreadsheet::OleObject::default();
    object.set_prog_id("Package");
    object.set_object_extension("bin");
    object.set_object_data(b"Tamsorin Velgrad".to_vec());
    sheet.ole_objects_mut().set_ole_object(object);
    let dir = project(None);
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, stderr) = run_ok(dir.path());
    let parts = package_parts(&fs::read(out.path().join("mappe.xlsx")).unwrap());
    let kept = parts
        .iter()
        .any(|(name, _)| name.starts_with("xl/embeddings/"));
    // The warning follows the written package: it names the kind exactly
    // when the copy still holds the object.
    let line = kinds_line(&stderr);
    assert_eq!(line.contains("embedded objects"), kept, "stderr: {stderr}");
    assert!(kept, "umya dropped the embedded object");
}

/// Runs a workbook whose cell B2 has the hyperlink `url`; returns the url of
/// the copy and the text of the cell.
fn hyperlink_copy(url: &str) -> (String, String) {
    let dir = project(None);
    let mut book = simple_book("Zorbina Quenzel", "zorbina.quenzel@acme.test");
    let mut link = umya_spreadsheet::Hyperlink::default();
    link.set_url(url);
    book.sheet_by_name_mut("Daten")
        .unwrap()
        .cell_mut("B2")
        .set_hyperlink(link);
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, stderr) = run_ok(dir.path());
    assert_no_leak(&stderr, "stderr");
    assert_well_formed(&out.path().join("mappe.xlsx"));
    let copy = open(out.path(), "mappe.xlsx");
    let cell = copy.sheet_by_name("Daten").unwrap().cell("B2").unwrap();
    let target = cell
        .hyperlink()
        .map(|link| link.url().to_string())
        .unwrap_or_default();
    (target, cell.value().to_string())
}

fn assert_valid_url(url: &str) {
    assert!(
        url.bytes().all(|b| b.is_ascii_graphic()) && !url.contains(['"', '<', '>']),
        "url: {url}"
    );
}

#[test]
fn mailto_with_display_name_gets_a_fake_name_and_address() {
    for url in [
        "mailto:\"Zorbina Quenzel\" <zorbina.quenzel@acme.test>",
        "mailto:%22Zorbina%20Quenzel%22%20%3Czorbina.quenzel%40acme.test%3E",
    ] {
        let (target, fake) = hyperlink_copy(url);
        assert_valid_url(&target);
        assert_no_leak(&target, "url");
        assert!(target.starts_with("mailto:%22"), "url: {target}");
        assert!(target.contains("%3C"), "url: {target}");
        assert!(target.contains(&fake), "url: {target}");
    }
}

#[test]
fn url_with_a_name_in_the_query_gets_the_known_name_replaced() {
    let (target, _) =
        hyperlink_copy("https://intranet.acme.test/people/zorbina-quenzel?u=Zorbina%20Quenzel");
    assert_valid_url(&target);
    assert!(
        target.starts_with("https://intranet.acme.test/people/"),
        "url: {target}"
    );
    assert!(target.contains("?u="), "url: {target}");
    assert!(!target.contains("Quenzel"), "url: {target}");
    assert!(!target.contains("Zorbina"), "url: {target}");
}

#[test]
fn url_with_an_email_in_the_path_gets_the_address_replaced() {
    let (target, fake) = hyperlink_copy("https://x.example.org/user/zorbina.quenzel%40acme.test/");
    assert_valid_url(&target);
    assert!(target.contains(&fake), "url: {target}");
    assert!(target.ends_with('/'), "url: {target}");
}

#[test]
fn url_without_a_name_or_email_stays_byte_identical() {
    for url in [
        "https://example.org/a%20b/c?x=1&y=%C3%A4#frag",
        "https://example.org/plain",
    ] {
        let (target, _) = hyperlink_copy(url);
        assert_eq!(target, url);
    }
}

// ---- Spec 0018 review fixes: alt text, table columns, query, location ----

/// Runs a workbook with one picture named `name`; returns the warning line.
fn picture_line(name: &str) -> String {
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let mut marker = umya_spreadsheet::structs::drawing::spreadsheet::MarkerType::default();
    marker.set_coordinate("D2");
    let mut image = umya_spreadsheet::structs::Image::default();
    image.new_image_with_dimensions(10, 10, name, b"not a real image".to_vec(), marker);
    book.sheet_by_name_mut("Daten").unwrap().add_image(image);
    let (_out, line) = warning_of(&book);
    line
}

#[test]
fn picture_with_a_custom_name_gets_the_drawings_warning() {
    for name in ["Tamsorin Velgrad", "Foto Velgrad", "velgrad.png"] {
        let line = picture_line(name);
        assert!(line.contains("drawings"), "name: {name}, line: {line}");
    }
}

#[test]
fn picture_with_a_default_name_gives_no_drawings_warning() {
    for name in ["Picture 1", "Grafik 3", "Text Box 12"] {
        let line = picture_line(name);
        assert!(!line.contains("drawings"), "name: {name}, line: {line}");
    }
}

fn table_book(columns: [&str; 2]) -> Workbook {
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let sheet = book.sheet_by_name_mut("Daten").unwrap();
    let mut table = umya_spreadsheet::Table::new("Tabelle1", ("A1", "B3"));
    for column in columns {
        table.add_column(umya_spreadsheet::TableColumn::new(column));
    }
    sheet.add_table(table);
    book
}

#[test]
fn table_column_that_is_no_header_text_gets_the_tables_warning() {
    let (_out, line) = warning_of(&table_book(["name", "Quenzel-Spalte"]));
    assert!(line.contains("tables"), "line: {line}");
}

#[test]
fn table_with_configured_header_columns_gives_no_tables_warning() {
    let (_out, line) = warning_of(&table_book(["name", "Email"]));
    assert!(!line.contains("tables"), "line: {line}");
}

#[test]
fn currency_block_in_a_number_format_gets_the_number_formats_warning() {
    let mut book = simple_book("Zorbina Quenzel", "zorbina@acme.test");
    let sheet = book.sheet_by_name_mut("Daten").unwrap();
    sheet.cell_mut("C2").set_value_number(5);
    sheet
        .cell_mut("C2")
        .style_mut()
        .number_format_mut()
        .set_format_code("[$Quenzel]0");
    let (_out, line) = warning_of(&book);
    assert!(line.contains("number formats"), "line: {line}");
}

/// Like `hyperlink_copy`, with `free_text`, a `location` flag, and a second
/// person in row 3. Returns the url of the copy and the copy.
fn link_run(free_text: &str, url: &str, location: bool) -> (String, Workbook) {
    let dir = project(None);
    let config = dir.path().join("config/pseudofy.toml");
    let text = fs::read_to_string(&config).unwrap();
    let text = text.replacen(
        "language = \"de\"\n",
        &format!("language = \"de\"\nfree_text = \"{free_text}\"\n"),
        1,
    );
    fs::write(&config, text).unwrap();
    let mut book = simple_book("Zorbina Quenzel", "zorbina.quenzel@acme.test");
    let sheet = book.sheet_by_name_mut("Daten").unwrap();
    set(sheet, "A3", "Tamsorin Velgrad");
    set(sheet, "B3", "tamsorin@acme.test");
    let mut link = umya_spreadsheet::Hyperlink::default();
    link.set_url(url);
    link.set_location(location);
    sheet.cell_mut("C2").set_hyperlink(link);
    set(sheet, "C2", "x");
    save(&book, dir.path(), "mappe.xlsx");
    let (out, _, stderr) = run_ok(dir.path());
    assert_no_leak(&stderr, "stderr");
    assert_well_formed(&out.path().join("mappe.xlsx"));
    let copy = open(out.path(), "mappe.xlsx");
    let target = copy
        .sheet_by_name("Daten")
        .unwrap()
        .cell("C2")
        .unwrap()
        .hyperlink()
        .map(|link| link.url().to_string())
        .unwrap_or_default();
    (target, copy)
}

/// Percent-decodes `text` (test helper).
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn plus_in_the_query_is_a_space_and_percent_2b_stays() {
    let (target, _) = link_run("full", "https://x.example.org/p?u=Zorbina+Quenzel", false);
    assert_valid_url(&target);
    assert_no_leak(&target, "url");
    assert!(target.starts_with("https://x.example.org/p?u="), "{target}");
    let (target, _) = link_run(
        "full",
        "https://x.example.org/p?u=Zorbina%20Quenzel&t=a%2Bb",
        false,
    );
    assert_valid_url(&target);
    assert_no_leak(&target, "url");
    assert!(target.ends_with("&t=a%2Bb"), "{target}");
    // A segment that does not change stays byte for byte, `+` included.
    let (target, _) = link_run("full", "https://x.example.org/p?a=b+c&d=%2B", false);
    assert_eq!(target, "https://x.example.org/p?a=b+c&d=%2B");
    // A plus in the path is a plus.
    let (target, _) = link_run("full", "https://x.example.org/a+b/Zorbina%20Quenzel", false);
    assert_valid_url(&target);
    assert!(target.contains("/a+b/"), "{target}");
    assert_no_leak(&target, "url");
}

#[test]
fn location_link_stays_unchanged() {
    for url in ["'Zorbina Quenzel'!A1", "Daten!A1"] {
        let (target, _) = link_run("tokens", url, true);
        assert_eq!(target, url);
    }
}

#[test]
fn invalid_utf8_escape_keeps_its_escape_and_the_rest_is_replaced() {
    let (target, _) = link_run("tokens", "https://x.example.org/p?q=%2CZorbina%FF", false);
    assert_valid_url(&target);
    assert_no_leak(&target, "url");
    assert!(target.ends_with("%FF"), "{target}");
    assert!(!target.contains("%25"), "{target}");
    for mode in ["full", "tokens"] {
        let (target, _) = link_run(
            mode,
            "https://x.example.org/p?u=Zorbina%20Quenzel%FF",
            false,
        );
        assert_valid_url(&target);
        assert_no_leak(&target, "url");
        assert!(target.ends_with("%FF"), "{target}");
        assert!(!target.contains("%25"), "{target}");
    }
    let (target, _) = link_run(
        "tokens",
        "mailto:%22Zorbina%20Quenzel%22%20%3Czorbina.quenzel%40acme.test%3E%FF",
        false,
    );
    assert_valid_url(&target);
    assert_no_leak(&target, "url");
    assert!(!target.contains("%25"), "{target}");
}

#[test]
fn backslash_in_a_file_link_stays_unencoded() {
    let (target, _) = link_run(
        "tokens",
        "file:///\\\\server\\share\\Zorbina Quenzel.docx",
        false,
    );
    assert_valid_url(&target);
    assert_no_leak(&target, "url");
    assert!(target.contains("\\\\server\\share\\"), "{target}");
    assert!(!target.contains("%5C"), "{target}");
}

#[test]
fn mailto_with_two_display_names_gets_each_name_replaced() {
    let (target, copy) = link_run(
        "tokens",
        "mailto:\"Zorbina Quenzel\" <zorbina.quenzel@acme.test>, \"Tamsorin Velgrad\" <tamsorin@acme.test>",
        false,
    );
    assert_valid_url(&target);
    assert_no_leak(&target, "url");
    let sheet = copy.sheet_by_name("Daten").unwrap();
    let first = sheet.cell("A2").unwrap().value().to_string();
    let second = sheet.cell("A3").unwrap().value().to_string();
    let decoded = unescape(&target);
    assert!(decoded.contains(&format!("\"{first}\"")), "{decoded}");
    assert!(decoded.contains(&format!("\"{second}\"")), "{decoded}");
}
