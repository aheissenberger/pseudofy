//! Round-trip fidelity of Excel-made workbooks (spec 0010).
//! The fixtures contain only invented data. The test compares ZIP part names only.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions may panic"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// The Excel-made fixtures. `fidelity.xlsx` links to `fidelity_reference.xlsx`.
const FIXTURES: [&str; 2] = ["fidelity.xlsx", "fidelity_reference.xlsx"];

/// Parts that the copy may lose. Keep this list equal to the list in ADR-0004.
const ALLOWED_LOST: [&str; 1] = [
    // umya-spreadsheet does not write the calculation chain. Excel rebuilds it
    // on the next calculation.
    "xl/calcChain.xml",
];

fn part_names(path: &Path) -> BTreeSet<String> {
    let file = fs::File::open(path).unwrap();
    let archive = zip::ZipArchive::new(file).unwrap();
    archive.file_names().map(str::to_owned).collect()
}

/// Returns each part of `input` that is missing in `output` and not in `allowed`.
fn unexpected_lost(
    input: &BTreeSet<String>,
    output: &BTreeSet<String>,
    allowed: &[&str],
) -> Vec<String> {
    input
        .iter()
        .filter(|name| !output.contains(*name) && !allowed.contains(&name.as_str()))
        .cloned()
        .collect()
}

fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for name in FIXTURES {
        fs::copy(fixtures.join(name), dir.path().join(name)).unwrap();
    }
    fs::create_dir_all(dir.path().join("secrets")).unwrap();
    fs::write(dir.path().join("secrets/pseudofy.key"), format!("{KEY}\n")).unwrap();
    fs::create_dir_all(dir.path().join("config")).unwrap();
    fs::write(
        dir.path().join("config/pseudofy.toml"),
        "language = \"de\"\n\n[[rule]]\nfiles = \"*.xlsx\"\n\
         first_name = [\"First Name\"]\nlast_name = [\"Last Name\"]\nemail = [\"Email\"]\n",
    )
    .unwrap();
    dir
}

#[test]
fn copy_keeps_each_part_that_is_not_on_the_list() {
    let project = project();
    let out = TempDir::new().unwrap();
    Command::cargo_bin("pseudofy")
        .unwrap()
        .args(["run", "--project"])
        .arg(project.path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success();

    let mut failures = Vec::new();
    let mut lost_anywhere = BTreeSet::new();
    for name in FIXTURES {
        let input = part_names(&project.path().join(name));
        let output = part_names(&out.path().join(name));
        lost_anywhere.extend(input.difference(&output).cloned());
        for part in unexpected_lost(&input, &output, &ALLOWED_LOST) {
            failures.push(format!("{name}: {part}"));
        }
    }
    assert!(
        failures.is_empty(),
        "the copy loses parts that are not on the list:\n{}",
        failures.join("\n")
    );

    // An entry that no fixture loses any more must leave the list and ADR-0004.
    let stale: Vec<&str> = ALLOWED_LOST
        .iter()
        .copied()
        .filter(|part| !lost_anywhere.contains(*part))
        .collect();
    assert!(
        stale.is_empty(),
        "the copy keeps these parts; remove them from the list and from ADR-0004: {stale:?}"
    );
}

#[test]
fn unexpected_lost_names_each_missing_part_that_is_not_allowed() {
    let set = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect();
    let input = set(&["a.xml", "b.xml", "c.xml", "d.xml"]);
    let output = set(&["a.xml"]);
    assert_eq!(
        unexpected_lost(&input, &output, &["c.xml"]),
        vec!["b.xml".to_owned(), "d.xml".to_owned()]
    );
    assert_eq!(unexpected_lost(&input, &input, &[]), Vec::<String>::new());
}

// ---- Sparklines (spec 0013) ----

fn part_text(path: &Path, name: &str) -> Option<String> {
    let file = fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut entry = archive.by_name(name).ok()?;
    let mut text = String::new();
    entry.read_to_string(&mut text).unwrap();
    Some(text)
}

/// The attributes of every start tag with local name `local`.
fn tags(xml: &str, local: &str) -> Vec<BTreeMap<String, String>> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut found = Vec::new();
    loop {
        match reader.read_event().unwrap() {
            quick_xml::events::Event::Eof => break,
            quick_xml::events::Event::Start(e) | quick_xml::events::Event::Empty(e)
                if e.local_name().as_ref() == local.as_bytes() =>
            {
                let attributes = e
                    .attributes()
                    .map(|a| {
                        let a = a.unwrap();
                        (
                            String::from_utf8_lossy(a.key.as_ref()).into_owned(),
                            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .unwrap()
                                .into_owned(),
                        )
                    })
                    .collect();
                found.push(attributes);
            }
            _ => {}
        }
    }
    found
}

/// The number of `sparkline` elements of each sheet, by sheet name.
fn sparkline_counts(path: &Path) -> BTreeMap<String, usize> {
    let workbook = part_text(path, "xl/workbook.xml").unwrap();
    let rels = part_text(path, "xl/_rels/workbook.xml.rels").unwrap();
    let targets: BTreeMap<String, String> = tags(&rels, "Relationship")
        .into_iter()
        .map(|r| (r["Id"].clone(), r["Target"].clone()))
        .collect();
    let mut counts = BTreeMap::new();
    for sheet in tags(&workbook, "sheet") {
        let target = &targets[&sheet["r:id"]];
        let part = match target.strip_prefix('/') {
            Some(absolute) => absolute.to_owned(),
            None => format!("xl/{target}"),
        };
        let xml = part_text(path, &part).unwrap();
        counts.insert(sheet["name"].clone(), tags(&xml, "sparkline").len());
    }
    counts
}

#[test]
fn copy_keeps_the_sparkline_count_of_each_sheet() {
    let project = project();
    let out = TempDir::new().unwrap();
    Command::cargo_bin("pseudofy")
        .unwrap()
        .args(["run", "--project"])
        .arg(project.path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success();

    let mut total = 0;
    for name in FIXTURES {
        let input = sparkline_counts(&project.path().join(name));
        let output = sparkline_counts(&out.path().join(name));
        assert_eq!(input, output, "{name}: sparkline count per sheet differs");
        total += input.values().sum::<usize>();
    }
    assert!(total >= 10, "the fixtures must contain sparklines");
}
