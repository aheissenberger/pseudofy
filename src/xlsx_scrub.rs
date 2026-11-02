//! Scrub the written XLSX package (ADR-0009).
//!
//! `umya-spreadsheet` keeps the old shared strings of a changed cell. The
//! scrub blanks each entry of `xl/sharedStrings.xml` that no worksheet uses.
//! Error texts never contain part contents.

use std::collections::HashSet;
use std::io::{Cursor, Read, Write};

use anyhow::anyhow;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const SHARED_STRINGS: &str = "xl/sharedStrings.xml";

/// Return `package` with each unused shared string replaced by an empty one.
pub fn scrub(package: &[u8]) -> anyhow::Result<Vec<u8>> {
    let bad = || anyhow!("cannot scrub the workbook package");
    let mut archive = ZipArchive::new(Cursor::new(package)).map_err(|_| bad())?;

    let mut used: HashSet<usize> = HashSet::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|_| bad())?;
        let name = file.name().to_owned();
        if name.starts_with("xl/worksheets/") && is_xml(&name) {
            let mut xml = Vec::new();
            file.read_to_end(&mut xml).map_err(|_| bad())?;
            collect_used(&xml, &mut used).map_err(|_| bad())?;
        }
    }

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let is_shared = {
            let file = archive.by_index_raw(index).map_err(|_| bad())?;
            file.name() == SHARED_STRINGS
        };
        if is_shared {
            let mut xml = Vec::new();
            archive
                .by_index(index)
                .map_err(|_| bad())?
                .read_to_end(&mut xml)
                .map_err(|_| bad())?;
            let cleaned = blank_unused(&xml, &used).map_err(|_| bad())?;
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            writer
                .start_file(SHARED_STRINGS, options)
                .map_err(|_| bad())?;
            writer.write_all(&cleaned).map_err(|_| bad())?;
        } else {
            let file = archive.by_index_raw(index).map_err(|_| bad())?;
            writer.raw_copy_file(file).map_err(|_| bad())?;
        }
    }
    let cursor = writer.finish().map_err(|_| bad())?;
    Ok(cursor.into_inner())
}

/// What `inspect` finds in an input package.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Inspection {
    /// The layout cannot be read without data loss: a shared-string table
    /// with a namespace prefix, or an inline string with several runs.
    pub unsupported: bool,
    /// Kinds of parts that pseudofy does not pseudonymize.
    pub kinds: Vec<&'static str>,
}

#[derive(Default)]
struct SheetFlags {
    validations: bool,
    headers: bool,
    conditional: bool,
}

/// Inspect the input package: unsupported layouts and unprocessed parts.
/// Error texts never contain part contents.
pub fn inspect(package: &[u8]) -> anyhow::Result<Inspection> {
    let bad = || anyhow!("cannot inspect the workbook package");
    let mut archive = ZipArchive::new(Cursor::new(package)).map_err(|_| bad())?;
    let mut comments = false;
    let mut charts = false;
    let mut flags = SheetFlags::default();
    let mut unsupported = false;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|_| bad())?;
        let name = file.name().to_owned();
        if name.starts_with("xl/threadedComments/")
            || (name.starts_with("xl/comments") && is_xml(&name))
        {
            comments = true;
        }
        if name.starts_with("xl/charts/") {
            charts = true;
        }
        if name == SHARED_STRINGS {
            let mut xml = Vec::new();
            file.read_to_end(&mut xml).map_err(|_| bad())?;
            if root_has_prefix(&xml).map_err(|_| bad())? {
                unsupported = true;
            }
        } else if name.starts_with("xl/worksheets/") && is_xml(&name) {
            let mut xml = Vec::new();
            file.read_to_end(&mut xml).map_err(|_| bad())?;
            unsupported |= scan_sheet(&xml, &mut flags).map_err(|_| bad())?;
        }
    }
    let mut kinds = Vec::new();
    for (present, kind) in [
        (comments, "comments"),
        (charts, "charts"),
        (flags.validations, "data validations"),
        (flags.headers, "headers/footers"),
        (flags.conditional, "conditional formatting"),
    ] {
        if present {
            kinds.push(kind);
        }
    }
    Ok(Inspection { unsupported, kinds })
}

/// True if the root element has a namespace prefix.
fn root_has_prefix(xml: &[u8]) -> Result<bool, quick_xml::Error> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) | Event::Empty(start) => {
                return Ok(start.name().prefix().is_some());
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buf.clear();
    }
}

const HEADER_FOOTER: [&[u8]; 6] = [
    b"oddHeader",
    b"oddFooter",
    b"evenHeader",
    b"evenFooter",
    b"firstHeader",
    b"firstFooter",
];

/// Record the features of one worksheet in `flags`. Returns true if an inline
/// string has several runs.
fn scan_sheet(xml: &[u8], flags: &mut SheetFlags) -> Result<bool, quick_xml::Error> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut multi_run = false;
    let mut in_inline = false;
    let mut runs = 0usize;
    let mut in_header = false;
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) => {
                let local = start.local_name();
                let local = local.as_ref();
                if local == b"dataValidation" {
                    flags.validations = true;
                } else if local == b"cfRule" {
                    flags.conditional = true;
                } else if local == b"is" {
                    in_inline = true;
                    runs = 0;
                } else if local == b"r" && in_inline {
                    runs += 1;
                    multi_run |= runs > 1;
                } else if HEADER_FOOTER.contains(&local) {
                    in_header = true;
                }
            }
            Event::Empty(start) => {
                let local = start.local_name();
                let local = local.as_ref();
                if local == b"dataValidation" {
                    flags.validations = true;
                } else if local == b"cfRule" {
                    flags.conditional = true;
                } else if local == b"r" && in_inline {
                    runs += 1;
                    multi_run |= runs > 1;
                }
            }
            Event::End(end) => {
                let local = end.local_name();
                let local = local.as_ref();
                if local == b"is" {
                    in_inline = false;
                } else if HEADER_FOOTER.contains(&local) {
                    in_header = false;
                }
            }
            Event::Text(text) if in_header => {
                let empty = text.decode().map_or(true, |t| t.trim().is_empty());
                flags.headers |= !empty;
            }
            Event::CData(_) if in_header => flags.headers = true,
            Event::Eof => return Ok(multi_run),
            _ => {}
        }
        buf.clear();
    }
}

fn is_xml(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("xml"))
}

fn is_name(start: &BytesStart<'_>, name: &[u8]) -> bool {
    start.local_name().as_ref() == name
}

/// Add each shared-string index that a cell `t="s"` uses to `used`.
fn collect_used(xml: &[u8], used: &mut HashSet<usize>) -> Result<(), quick_xml::Error> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut in_shared_cell = false;
    let mut in_value = false;
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) if is_name(&start, b"c") => {
                in_shared_cell = start
                    .attributes()
                    .flatten()
                    .any(|attr| attr.key.as_ref() == b"t" && attr.value.as_ref() == b"s");
            }
            Event::End(end) if end.local_name().as_ref() == b"c" => in_shared_cell = false,
            Event::Start(start) if in_shared_cell && is_name(&start, b"v") => in_value = true,
            Event::End(end) if end.local_name().as_ref() == b"v" => in_value = false,
            Event::Text(text) if in_value => {
                if let Ok(decoded) = text.decode()
                    && let Ok(index) = decoded.trim().parse::<usize>()
                {
                    used.insert(index);
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
        buf.clear();
    }
}

/// Rewrite the shared-string table: each `<si>` whose index is not in `used`
/// becomes `<si><t/></si>`.
fn blank_unused(xml: &[u8], used: &HashSet<usize>) -> Result<Vec<u8>, quick_xml::Error> {
    let mut reader = Reader::from_reader(xml);
    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    let mut index = 0usize;
    // Depth inside an `<si>` that is being dropped (0: not dropping).
    let mut skipping = 0usize;
    loop {
        let event = reader.read_event_into(&mut buf)?;
        if skipping > 0 {
            match event {
                Event::Start(_) => skipping += 1,
                Event::End(_) => skipping -= 1,
                Event::Eof => return Ok(writer.into_inner()),
                _ => {}
            }
            buf.clear();
            continue;
        }
        match &event {
            Event::Eof => return Ok(writer.into_inner()),
            Event::Start(start) if is_name(start, b"si") => {
                let keep = used.contains(&index);
                index += 1;
                if keep {
                    writer.write_event(event.borrow())?;
                } else {
                    writer.write_event(Event::Start(start.to_owned()))?;
                    writer.write_event(Event::Empty(BytesStart::new("t")))?;
                    writer.write_event(Event::End(start.to_end().into_owned()))?;
                    skipping = 1;
                }
            }
            Event::Empty(start) if is_name(start, b"si") => {
                index += 1;
                writer.write_event(event.borrow())?;
            }
            _ => writer.write_event(event.borrow())?,
        }
        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(sheet: &str, shared: &str) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, body) in [
            ("[Content_Types].xml", "<Types/>"),
            ("xl/worksheets/sheet1.xml", sheet),
            ("xl/sharedStrings.xml", shared),
            ("docProps/core.xml", "<cp/>"),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn part(bytes: &[u8], name: &str) -> String {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut text = String::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    fn names(bytes: &[u8]) -> Vec<String> {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_owned())
            .collect()
    }

    const SHEET: &str = r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>2</v></c><c r="B1"><v>1</v></c><c r="C1" t="str"><v>0</v></c></row></sheetData></worksheet>"#;
    const SHARED: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="urn:x" count="3" uniqueCount="3"><si><t>Old Alpha</t></si><si><r><t>Old</t></r><r><t> Beta</t></r></si><si><t xml:space="preserve">Keep &amp; Gamma</t></si></sst>"#;

    #[test]
    fn blanks_only_unused_entries() {
        let out = scrub(&package(SHEET, SHARED)).unwrap();
        let shared = part(&out, SHARED_STRINGS);
        assert!(!shared.contains("Old"));
        assert!(!shared.contains("Beta"));
        assert!(shared.contains("Keep &amp; Gamma"));
        assert!(shared.contains(r#"<sst xmlns="urn:x" count="3" uniqueCount="3">"#));
        assert_eq!(shared.matches("<si>").count(), 3);
        assert_eq!(shared.matches("<si><t/></si>").count(), 2);
    }

    #[test]
    fn other_parts_and_order_stay() {
        let input = package(SHEET, SHARED);
        let out = scrub(&input).unwrap();
        assert_eq!(names(&out), names(&input));
        assert_eq!(part(&out, "xl/worksheets/sheet1.xml"), SHEET);
        assert_eq!(part(&out, "docProps/core.xml"), "<cp/>");
    }

    fn plain_package(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        for (name, body) in parts {
            writer.start_file(*name, options).unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn plain_workbook_has_nothing_to_report() {
        let found = inspect(&package(SHEET, SHARED)).unwrap();
        assert_eq!(found, Inspection::default());
    }

    #[test]
    fn comments_and_charts_are_detected() {
        let found = inspect(&plain_package(&[
            ("xl/comments1.xml", "<comments/>"),
            ("xl/charts/chart1.xml", "<c/>"),
        ]))
        .unwrap();
        assert_eq!(found.kinds, ["comments", "charts"]);
        let found = inspect(&plain_package(&[("xl/threadedComments/t1.xml", "<t/>")])).unwrap();
        assert_eq!(found.kinds, ["comments"]);
    }

    #[test]
    fn sheet_features_are_detected_with_prefixes() {
        let sheet = r#"<x:worksheet xmlns:x="urn:x"><x:conditionalFormatting><x:cfRule type="x"/></x:conditionalFormatting><x:dataValidations><x:dataValidation/></x:dataValidations><x:headerFooter><x:oddHeader>&amp;LInvented</x:oddHeader></x:headerFooter></x:worksheet>"#;
        let found = inspect(&plain_package(&[("xl/worksheets/sheet1.xml", sheet)])).unwrap();
        assert_eq!(
            found.kinds,
            [
                "data validations",
                "headers/footers",
                "conditional formatting"
            ]
        );
        assert!(!found.unsupported);
    }

    #[test]
    fn empty_header_is_not_reported() {
        let sheet = "<worksheet><headerFooter><oddHeader> </oddHeader><oddFooter/></headerFooter></worksheet>";
        let found = inspect(&plain_package(&[("xl/worksheets/sheet1.xml", sheet)])).unwrap();
        assert_eq!(found, Inspection::default());
    }

    #[test]
    fn prefixed_shared_strings_are_unsupported() {
        let shared = r#"<x:sst xmlns:x="urn:x"><x:si><x:t>A</x:t></x:si></x:sst>"#;
        let found = inspect(&plain_package(&[(SHARED_STRINGS, shared)])).unwrap();
        assert!(found.unsupported);
        let found = inspect(&plain_package(&[(SHARED_STRINGS, SHARED)])).unwrap();
        assert!(!found.unsupported);
    }

    #[test]
    fn inline_string_with_several_runs_is_unsupported() {
        let two = r#"<worksheet><sheetData><row><c t="inlineStr"><is><r><t>A</t></r><r><t>B</t></r></is></c></row></sheetData></worksheet>"#;
        let one = r#"<worksheet><sheetData><row><c t="inlineStr"><is><r><t>A</t></r></is></c><c t="inlineStr"><is><r><t>B</t></r></is></c></row></sheetData></worksheet>"#;
        let found = inspect(&plain_package(&[("xl/worksheets/sheet1.xml", two)])).unwrap();
        assert!(found.unsupported);
        let found = inspect(&plain_package(&[("xl/worksheets/sheet1.xml", one)])).unwrap();
        assert!(!found.unsupported);
    }

    #[test]
    fn inspect_of_invalid_package_gives_a_plain_error() {
        let error = inspect(b"not a zip").unwrap_err();
        assert!(!format!("{error:#}").contains("not a zip"));
    }

    #[test]
    fn invalid_package_gives_a_plain_error() {
        let error = scrub(b"not a zip").unwrap_err();
        assert!(!format!("{error:#}").contains("not a zip"));
    }
}
