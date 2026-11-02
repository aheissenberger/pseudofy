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
fn mode_off_replaces_no_known_name_outside_the_name_columns() {
    let dir = project(Some("off"));
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}Zorbina,Quenzel,Zorbina Quenzel said hi to zq@acme.test\n"),
    );
    let (out, stdout, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    assert_ne!(t[1][0], "Zorbina");
    assert_ne!(t[1][1], "Quenzel");
    let note = &t[1][2];
    assert!(note.starts_with("Zorbina Quenzel said hi to "), "{note}");
    assert!(!note.contains("acme.test"), "{note}");
    assert!(!stdout.contains("free-text"), "{stdout}");
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

/// Creates a project with a key and one CSV rule given as TOML lines.
fn project_with_rule(free_text: &str, rule_lines: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "secrets/pseudofy.key", &format!("{KEY}\n"));
    let text = format!(
        "language = \"de\"\nfree_text = \"{free_text}\"\n\n[[rule]]\nfiles = \"**/*.csv\"\n{rule_lines}"
    );
    write_file(dir.path(), "config/pseudofy.toml", &text);
    dir
}

fn assert_absent(text: &str, words: &[&str], what: &str) {
    let lower = text.to_lowercase();
    for word in words {
        assert!(!lower.contains(&word.to_lowercase()), "{what} leaks {word}");
    }
}

#[test]
fn quotes_and_possessives_stay_around_the_pseudonyms() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}Zorbina,Quenzel,c8 'Zorbina Quenzel' and Zorbina's\n"),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    let (first, last) = fakes(&t, 1);
    assert_eq!(t[1][2], format!("c8 '{first} {last}' and {first}'s"));
    assert_absent(&t[1][2], &["Zorbina", "Quenzel"], "note");
}

#[test]
fn two_first_name_and_two_last_name_columns_all_count_as_known_names() {
    let dir = project_with_rule(
        "tokens",
        "first_name = [\"first\", \"first2\"]\nlast_name = [\"last\", \"last2\"]\n",
    );
    write_file(
        dir.path(),
        "people.csv",
        "first,last,first2,last2,note\nZorbina,Quenzel,Pirmox,Hulbert,Zorbina / Pirmox / Hulbert\n",
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    let row = &t[1];
    assert_eq!(row[4], format!("{} / {} / {}", row[0], row[2], row[3]));
    assert_absent(&row[4], &["Zorbina", "Pirmox", "Hulbert"], "note");
}

#[test]
fn full_name_with_an_email_suffix_is_replaced_and_email_parts_stay() {
    let dir = project_with_rule("tokens", "full_name = [\"name\"]\n");
    write_file(
        dir.path(),
        "people.csv",
        "name,note\nOttilie Wenzhart <ottilie.wenzhart@firma.test>,Ottilie Wenzhart / firma / test\n",
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    let fake_name = t[1][0].split(" <").next().unwrap().to_string();
    assert_ne!(fake_name, "Ottilie Wenzhart");
    assert_eq!(t[1][1], format!("{fake_name} / firma / test"));
}

#[test]
fn mode_full_replaces_hyphenated_names_and_full_names_with_initials() {
    let dir = project_with_rule(
        "full",
        "first_name = [\"first\"]\nlast_name = [\"last\"]\nfull_name = [\"name\"]\n",
    );
    write_file(
        dir.path(),
        "people.csv",
        "first,last,name,note\nLisandra-Mae,Kroft,Berta C. Kunzl,Lisandra-Mae Kroft\n\
         Pirmox,Hulbert,Berta C. Kunzl,Berta C. Kunzl\n",
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    assert_eq!(t[1][3], format!("{} {}", t[1][0], t[1][1]));
    assert_absent(&t[1][3], &["Lisandra", "Kroft"], "note");
    assert_eq!(t[2][3], t[2][2]);
    assert_absent(&t[2][3], &["Berta", "Kunzl"], "note");
}

#[test]
fn case_folding_matches_special_letters() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}İlkay,Strauß,İlkay / İLKAY\nPirmox,Hulbert,STRAUSS\n"),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    assert_eq!(t[1][2], format!("{} / {}", t[1][0], t[1][0].to_uppercase()));
    assert_eq!(t[2][2], t[1][1].to_uppercase());
}

#[test]
fn multi_token_last_name_is_replaced_as_a_whole_and_parts_alone_stay() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!(
            "{HEADER}Tamsorin,de Velgrad,Tamsorin de Velgrad\nPirmox,Hulbert,firma.de and de facto\n"
        ),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    assert_eq!(t[1][2], format!("{} {}", t[1][0], t[1][1]));
    assert_eq!(t[2][2], "firma.de and de facto");
}

#[test]
fn a_note_without_a_known_name_is_byte_equal() {
    let dir = project(None);
    let note = "Mu\u{308}hle offen";
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}Zorbina,Quenzel,{note}\n"),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    assert_eq!(t[1][2].as_bytes(), note.as_bytes());
}

#[test]
fn a_file_that_fails_in_pass_one_adds_no_names() {
    let dir = project(None);
    // UTF-8 `ü` next to a lone 0xff byte: mixed encoding, rejected (spec 0009).
    let mut bad = format!("{HEADER}Zorbina,Quenzel,Grüße").into_bytes();
    bad.extend_from_slice(&[0xff, b'\n']);
    fs::write(dir.path().join("bad.csv"), bad).unwrap();
    write_file(
        dir.path(),
        "good.csv",
        &format!("{HEADER}Pirmox,Hulbert,Zorbina\n"),
    );
    let out = TempDir::new().unwrap();
    let assert = pseudofy()
        .args(["run", "--project"])
        .arg(dir.path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert_eq!(stderr.matches("bad.csv").count(), 1, "stderr was: {stderr}");
    let t = table(out.path(), "good.csv");
    assert_eq!(t[1][2], "Zorbina");
}

#[test]
fn xlsx_header_equal_to_a_known_name_stays_unchanged() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}Zorbina,Quenzel,x\n"),
    );
    let mut book = umya_spreadsheet::new_file_empty_worksheet();
    let sheet = book.new_sheet("Daten").unwrap();
    sheet.cell_mut("A1").set_value_string("Zorbina");
    sheet.cell_mut("A2").set_value_string("Zorbina");
    save(&book, dir.path(), "notes.xlsx");
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    let book = umya_spreadsheet::reader::xlsx::read(out.path().join("notes.xlsx")).unwrap();
    let sheet = book.sheet_by_name("Daten").unwrap();
    assert_eq!(sheet.value("A1"), "Zorbina");
    assert_eq!(sheet.value("A2"), t[1][0]);
}

#[test]
fn a_token_that_is_mostly_a_last_name_gets_the_last_name_pseudonym() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!(
            "{HEADER}Zorbina,Quenzel,x\nPirmox,Quenzel,x\nQuenzel,Hulbert,x\nTamsorin,Velgrad,Quenzel arrived\n"
        ),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    assert_ne!(t[1][1], t[3][0], "test needs distinct pseudonyms");
    assert_eq!(t[4][2], format!("{} arrived", t[1][1]));
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(head) => head.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[test]
fn all_lowercase_name_columns_match_each_case_form_in_free_text() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}zorbina,quenzel,Ruf Quenzel an | quenzel | QUENZEL\n"),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    let (_, last) = fakes(&t, 1);
    let parts: Vec<&str> = t[1][2].split(" | ").collect();
    assert_eq!(parts.len(), 3, "{:?}", t[1][2]);
    let head = parts[0].strip_prefix("Ruf ").unwrap();
    let head = head.strip_suffix(" an").unwrap();
    assert_eq!(head, capitalized(&last.to_lowercase()));
    // An all-lowercase single word does not match (spec 0004, tokens mode).
    assert_eq!(parts[1], "quenzel");
    assert_eq!(parts[2], last.to_uppercase());
}

#[test]
fn lowercase_common_words_are_not_replaced_but_capitalized_names_are() {
    let dir = project_with_rule(
        "tokens",
        "first_name = [\"first\"]\nlast_name = [\"last\"]\n",
    );
    write_file(
        dir.path(),
        "people.csv",
        "first,last,note\nMark,Will,\"We will mark it. Mark said: WILL\"\n",
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    let (first, last) = fakes(&t, 1);
    assert_eq!(
        t[1][2],
        format!("We will mark it. {first} said: {}", last.to_uppercase())
    );
}

fn dunmor_project(mode: &str, note: &str) -> (TempDir, Vec<Vec<String>>) {
    let dir = project_with_rule(mode, "first_name = [\"first\"]\nlast_name = [\"last\"]\n");
    write_file(
        dir.path(),
        "people.csv",
        &format!("first,last,note\nVelra,O’Dunmor,{note}\n"),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    (out, t)
}

#[test]
fn curly_apostrophe_name_is_replaced_as_a_full_name_in_full_mode() {
    let (_out, t) = dunmor_project("full", "Velra O’Dunmor");
    assert_eq!(t[1][2], format!("{} {}", t[1][0], t[1][1]));
    assert_absent(&t[1][2], &["Velra", "Dunmor"], "note");
}

#[test]
fn curly_apostrophe_last_name_is_replaced_as_one_token_in_tokens_mode() {
    let (_out, t) = dunmor_project("tokens", "O’Dunmor");
    assert_eq!(t[1][2], t[1][1]);
}

#[test]
fn straight_apostrophe_column_matches_curly_apostrophe_in_free_text() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}Velra,O'Dunmor,Velra O’Dunmor\n"),
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    assert_absent(&t[1][2], &["Dunmor", "Velra"], "note");
}

#[test]
fn two_spellings_get_the_pseudonym_of_the_same_spelling_independent_of_file_names() {
    let build = |first_file: &str| {
        let dir = project(None);
        write_file(
            dir.path(),
            first_file,
            &format!("{HEADER}Helmar,Strauss,x\n"),
        );
        write_file(
            dir.path(),
            "b.csv",
            &format!("{HEADER}Gerda,Strauß,Strauss | Strauß\n"),
        );
        let (out, _, _) = run_ok(dir.path());
        let a = table(out.path(), first_file);
        let b = table(out.path(), "b.csv");
        (a, b)
    };
    let (a, b) = build("a.csv");
    let parts: Vec<&str> = b[1][2].split(" | ").collect();
    assert_eq!(parts.len(), 2, "{:?}", b[1][2]);
    assert_eq!(parts[0], a[1][1]);
    assert_eq!(parts[1], b[1][1]);
    let (a2, b2) = build("z.csv");
    assert_eq!(a2[1], a[1]);
    assert_eq!(b2[1], b[1]);
}

#[test]
fn full_name_with_initial_is_replaced_as_the_name_cell() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        "name,note\nKesta Wolmar J.,Kesta Wolmar J.\n",
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    assert_eq!(t[1][1], t[1][0]);
    assert_absent(&t[1][1], &["Kesta", "Wolmar"], "note");
}

#[test]
fn last_comma_first_with_initial_is_replaced_as_the_name_cell() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        "name,note\n\"Draven, Ilsa M.\",\"Draven, Ilsa M.\"\n",
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    assert_eq!(t[1][1], t[1][0]);
    assert_absent(&t[1][1], &["Draven", "Ilsa"], "note");
}

#[test]
fn en_dash_and_em_dash_replace_a_hyphenated_first_name_in_full_mode() {
    let dir = project_with_rule("full", "first_name = [\"first\"]\nlast_name = [\"last\"]\n");
    write_file(
        dir.path(),
        "people.csv",
        "first,last,note\nLisandra-Mae,Kroft,Lisandra–Mae Kroft\n\
         Lisandra-Mae,Kroft,Lisandra—Mae Kroft\n",
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    for cells in &t[1..] {
        assert_absent(&cells[2], &["Lisandra", "Kroft"], "note");
        // The note keeps its own dash; only the name parts change.
        let note = cells[2].replace(['\u{2013}', '\u{2014}'], "-");
        assert_eq!(note, format!("{} {}", cells[0], cells[1]));
    }
}

#[test]
fn rich_text_cell_without_a_known_name_stays_rich_text_without_warning() {
    let dir = project(None);
    write_file(
        dir.path(),
        "people.csv",
        &format!("{HEADER}Zorbina,Quenzel,x\n"),
    );
    let mut book = notes_book(&["plain"]);
    let mut first = umya_spreadsheet::TextElement::default();
    first.set_text("Hello ");
    first.font_mut().set_bold(true);
    let mut second = umya_spreadsheet::TextElement::default();
    second.set_text("world");
    let mut rich = umya_spreadsheet::RichText::default();
    rich.add_rich_text_elements(first)
        .add_rich_text_elements(second);
    book.sheet_by_name_mut("Daten")
        .unwrap()
        .cell_mut("A3")
        .set_rich_text(rich);
    save(&book, dir.path(), "notes.xlsx");
    let (out, _, stderr) = run_ok(dir.path());
    assert!(!stderr.contains("rich-text"), "stderr: {stderr}");
    let parts = package_parts(&out.path().join("notes.xlsx"));
    let shared = parts
        .iter()
        .find(|(name, _)| name.ends_with("sharedStrings.xml"))
        .map(|(_, text)| text.as_str())
        .unwrap_or_default();
    assert!(shared.contains("Hello "), "{shared}");
    assert_eq!(shared.matches("<r>").count(), 2, "{shared}");
}

#[test]
fn quoted_nickname_keeps_its_quotes_and_matches_the_column() {
    let dir = project_with_rule(
        "tokens",
        "first_name = [\"first\"]\nlast_name = [\"last\"]\n",
    );
    write_file(
        dir.path(),
        "people.csv",
        "first,last,note\n\u{2018}Bob\u{2019},Kroft,Bob / \u{2018}Bob\u{2019}\n",
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    let fake = t[1][0]
        .strip_prefix('\u{2018}')
        .and_then(|f| f.strip_suffix('\u{2019}'))
        .unwrap()
        .to_string();
    assert_eq!(t[1][2], format!("{fake} / \u{2018}{fake}\u{2019}"));
    assert_absent(&t[1][2], &["Bob"], "note");
}

#[test]
fn one_letter_words_after_a_name_stay() {
    let dir = project_with_rule(
        "tokens",
        "first_name = [\"first\"]\nlast_name = [\"last\"]\n",
    );
    write_file(
        dir.path(),
        "people.csv",
        "first,last,note\nKesta,Wolmar,\"Kesta u. Karl; Thanks Kesta I will; Wolmar e. V.\"\n",
    );
    let (out, _, _) = run_ok(dir.path());
    let t = table(out.path(), "people.csv");
    let (first, last) = fakes(&t, 1);
    assert_eq!(
        t[1][2],
        format!("{first} u. Karl; Thanks {first} I will; {last} e. V.")
    );
}
