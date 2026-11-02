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

    #[test]
    fn invalid_package_gives_a_plain_error() {
        let error = scrub(b"not a zip").unwrap_err();
        assert!(!format!("{error:#}").contains("not a zip"));
    }
}
