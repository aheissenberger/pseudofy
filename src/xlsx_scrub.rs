//! Scrub the written XLSX package (ADR-0010).
//!
//! `umya-spreadsheet` keeps the old shared strings of a changed cell. The
//! scrub blanks each entry of `xl/sharedStrings.xml` that no worksheet uses.
//! Error texts never contain part contents.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Write};

use anyhow::anyhow;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const SHARED_STRINGS: &str = "xl/sharedStrings.xml";

/// Return `output` with each unused shared string replaced by an empty one
/// and with the external links of `input` restored (ADR-0011).
pub fn scrub(input: &[u8], output: &[u8]) -> anyhow::Result<Vec<u8>> {
    let bad = || anyhow!("cannot scrub the workbook package");
    let mut archive = ZipArchive::new(Cursor::new(output)).map_err(|_| bad())?;

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

    // Parts to write instead of, or in addition to, the parts of `output`.
    let mut restored = restore_external_links(input, &mut archive).map_err(|_| bad())?;

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for index in 0..archive.len() {
        let name = {
            let file = archive.by_index_raw(index).map_err(|_| bad())?;
            file.name().to_owned()
        };
        let replacement = restored.remove(&name);
        if name == SHARED_STRINGS {
            let mut xml = Vec::new();
            archive
                .by_index(index)
                .map_err(|_| bad())?
                .read_to_end(&mut xml)
                .map_err(|_| bad())?;
            let cleaned = blank_unused(&xml, &used).map_err(|_| bad())?;
            writer
                .start_file(SHARED_STRINGS, options)
                .map_err(|_| bad())?;
            writer.write_all(&cleaned).map_err(|_| bad())?;
        } else if let Some(bytes) = replacement {
            writer.start_file(name, options).map_err(|_| bad())?;
            writer.write_all(&bytes).map_err(|_| bad())?;
        } else {
            let file = archive.by_index_raw(index).map_err(|_| bad())?;
            writer.raw_copy_file(file).map_err(|_| bad())?;
        }
    }
    let mut added: Vec<_> = restored.into_iter().collect();
    added.sort();
    for (name, bytes) in added {
        writer.start_file(name, options).map_err(|_| bad())?;
        writer.write_all(&bytes).map_err(|_| bad())?;
    }
    let cursor = writer.finish().map_err(|_| bad())?;
    Ok(cursor.into_inner())
}

const WORKBOOK: &str = "xl/workbook.xml";
const WORKBOOK_RELS: &str = "xl/_rels/workbook.xml.rels";
const CONTENT_TYPES: &str = "[Content_Types].xml";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const EXTERNAL_LINK_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink";
const EXTERNAL_LINK_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml";

/// Elements of `CT_Workbook` that follow `externalReferences`.
const AFTER_EXTERNAL: [&[u8]; 11] = [
    b"definedNames",
    b"calcPr",
    b"oleSize",
    b"customWorkbookViews",
    b"pivotCaches",
    b"smartTagPr",
    b"smartTagTypes",
    b"webPublishing",
    b"fileRecoveryPr",
    b"webPublishObjects",
    b"extLst",
];

type Failure = Box<dyn std::error::Error>;

fn read_part<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<Option<Vec<u8>>, Failure> {
    match archive.by_name(name) {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(Some(bytes))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn attr(start: &BytesStart<'_>, local: &[u8]) -> Result<Option<String>, Failure> {
    for item in start.attributes() {
        let item = item?;
        if item.key.local_name().as_ref() == local {
            return Ok(Some(
                item.normalized_value(quick_xml::XmlVersion::Explicit1_0)?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

/// The relationship ids of the `<externalReference>` entries, in order.
fn external_ids(workbook: &[u8]) -> Result<Vec<String>, Failure> {
    let mut reader = Reader::from_reader(workbook);
    let mut buf = Vec::new();
    let mut ids = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) | Event::Empty(start) if is_name(&start, b"externalReference") => {
                ids.push(attr(&start, b"id")?.ok_or("missing id")?);
            }
            Event::Eof => return Ok(ids),
            _ => {}
        }
        buf.clear();
    }
}

/// All relationships as `(Id, Target)`.
fn relationships(rels: &[u8]) -> Result<Vec<(String, String)>, Failure> {
    let mut reader = Reader::from_reader(rels);
    let mut buf = Vec::new();
    let mut found = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) | Event::Empty(start) if is_name(&start, b"Relationship") => {
                let id = attr(&start, b"Id")?.ok_or("missing Id")?;
                found.push((id, attr(&start, b"Target")?.unwrap_or_default()));
            }
            Event::Eof => return Ok(found),
            _ => {}
        }
        buf.clear();
    }
}

fn has_element(xml: &[u8], name: &[u8]) -> Result<bool, Failure> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) | Event::Empty(start) if is_name(&start, name) => {
                return Ok(true);
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buf.clear();
    }
}

/// Resolve the target of a workbook relationship to a part name below
/// `xl/externalLinks/`.
fn link_part(target: &str) -> Result<String, Failure> {
    let part = match target.strip_prefix('/') {
        Some(absolute) => absolute.to_owned(),
        None => format!("xl/{target}"),
    };
    let plain = part
        .strip_prefix("xl/externalLinks/")
        .is_some_and(|rest| !rest.is_empty() && !rest.contains('/') && !rest.contains(".."));
    if plain && is_xml(&part) {
        Ok(part)
    } else {
        Err("unexpected external link target".into())
    }
}

/// Name of the relationships part of `part`.
fn rels_part(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// Build the parts to write for the external links of `input`: changed parts
/// of `output` and new parts. Empty if `input` has no external link or
/// `output` has `<externalReferences>`.
fn restore_external_links<R: Read + std::io::Seek>(
    input: &[u8],
    output: &mut ZipArchive<R>,
) -> Result<HashMap<String, Vec<u8>>, Failure> {
    let mut parts = HashMap::new();
    let mut source = ZipArchive::new(Cursor::new(input))?;
    let Some(in_workbook) = read_part(&mut source, WORKBOOK)? else {
        return Ok(parts);
    };
    let ids = external_ids(&in_workbook)?;
    if ids.is_empty() {
        return Ok(parts);
    }
    let out_workbook = read_part(output, WORKBOOK)?.ok_or("no workbook part")?;
    if has_element(&out_workbook, b"externalReferences")? {
        return Ok(parts);
    }
    let in_rels = read_part(&mut source, WORKBOOK_RELS)?.ok_or("no relationships")?;
    let in_rels = relationships(&in_rels)?;

    // The link parts, in the order of the input.
    let mut link_parts = Vec::new();
    for id in &ids {
        let (_, target) = in_rels
            .iter()
            .find(|(candidate, _)| candidate == id)
            .ok_or("unknown relationship")?;
        link_parts.push(link_part(target)?);
    }
    for part in &link_parts {
        let xml = read_part(&mut source, part)?.ok_or("missing link part")?;
        parts.insert(part.clone(), strip_cached_rows(&xml)?);
        let rels = rels_part(part);
        if let Some(bytes) = read_part(&mut source, &rels)? {
            parts.insert(rels, bytes);
        }
    }

    // Fresh relationship ids.
    let out_rels = read_part(output, WORKBOOK_RELS)?.ok_or("no relationships")?;
    let mut taken: HashSet<String> = relationships(&out_rels)?
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let types = read_part(output, CONTENT_TYPES)?.ok_or("no content types")?;
    let declared = override_names(&types)?;

    let mut references: Vec<String> = Vec::new();
    let mut relations: Vec<String> = Vec::new();
    let mut overrides: Vec<String> = Vec::new();
    let mut counter = 1usize;
    for part in &link_parts {
        let id = loop {
            let candidate = format!("rIdExt{counter}");
            counter += 1;
            if taken.insert(candidate.clone()) {
                break candidate;
            }
        };
        let file = part.strip_prefix("xl/").unwrap_or(part);
        references.push(format!(
            r#"<externalReference xmlns:r="{REL_NS}" r:id="{id}"/>"#
        ));
        relations.push(format!(
            r#"<Relationship Id="{id}" Type="{EXTERNAL_LINK_REL}" Target="{file}"/>"#
        ));
        if !declared.contains(&format!("/{part}")) {
            overrides.push(format!(
                r#"<Override PartName="/{part}" ContentType="{EXTERNAL_LINK_TYPE}"/>"#
            ));
        }
    }
    let references = format!(
        "<externalReferences>{}</externalReferences>",
        references.concat()
    );
    let relations = relations.concat();
    let overrides = overrides.concat();

    parts.insert(
        WORKBOOK.to_owned(),
        insert_xml(&out_workbook, &references, &AFTER_EXTERNAL)?,
    );
    parts.insert(
        WORKBOOK_RELS.to_owned(),
        insert_xml(&out_rels, &relations, &[])?,
    );
    parts.insert(
        CONTENT_TYPES.to_owned(),
        insert_xml(&types, &overrides, &[])?,
    );
    Ok(parts)
}

/// The `PartName` of each `<Override>` in `[Content_Types].xml`.
fn override_names(types: &[u8]) -> Result<HashSet<String>, Failure> {
    let mut reader = Reader::from_reader(types);
    let mut buf = Vec::new();
    let mut present = HashSet::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) | Event::Empty(start) if is_name(&start, b"Override") => {
                if let Some(name) = attr(&start, b"PartName")? {
                    present.insert(name);
                }
            }
            Event::Eof => return Ok(present),
            _ => {}
        }
        buf.clear();
    }
}

/// Insert `snippet` as a child of the root element: before the first child
/// named in `before`, or else at the end of the root.
fn insert_xml(xml: &[u8], snippet: &str, before: &[&[u8]]) -> Result<Vec<u8>, Failure> {
    let mut reader = Reader::from_reader(xml);
    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut done = false;
    loop {
        let event = reader.read_event_into(&mut buf)?;
        let insert_here = !done
            && depth == 1
            && match &event {
                Event::Start(start) | Event::Empty(start) => {
                    before.contains(&start.local_name().as_ref())
                }
                Event::End(_) => true,
                _ => false,
            };
        if insert_here {
            writer.get_mut().extend_from_slice(snippet.as_bytes());
            done = true;
        }
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => {
                return if done {
                    Ok(writer.into_inner())
                } else {
                    Err("no insertion point".into())
                };
            }
            _ => {}
        }
        writer.write_event(event.borrow())?;
        buf.clear();
    }
}

/// Remove each `<row>` inside a `<sheetData>` of an external link part.
fn strip_cached_rows(xml: &[u8]) -> Result<Vec<u8>, Failure> {
    let mut reader = Reader::from_reader(xml);
    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    let mut in_data = false;
    let mut skipping = 0usize;
    loop {
        let event = reader.read_event_into(&mut buf)?;
        if skipping > 0 {
            match event {
                Event::Start(_) => skipping += 1,
                Event::End(_) => skipping -= 1,
                Event::Eof => return Err("unclosed row".into()),
                _ => {}
            }
            buf.clear();
            continue;
        }
        match &event {
            Event::Eof => return Ok(writer.into_inner()),
            Event::Start(start) if is_name(start, b"sheetData") => in_data = true,
            Event::End(end) if end.local_name().as_ref() == b"sheetData" => in_data = false,
            Event::Start(start) if in_data && is_name(start, b"row") => {
                skipping = 1;
                buf.clear();
                continue;
            }
            Event::Empty(start) if in_data && is_name(start, b"row") => {
                buf.clear();
                continue;
            }
            _ => {}
        }
        writer.write_event(event.borrow())?;
        buf.clear();
    }
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
        let both = package(SHEET, SHARED);
        let out = scrub(&both, &both).unwrap();
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
        let out = scrub(&input, &input).unwrap();
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

    const OUT_WORKBOOK: &str = r#"<workbook xmlns="urn:m"><sheets><sheet name="A" sheetId="1" r:id="rId1"/></sheets><definedNames><definedName name="n">A!$A$1</definedName></definedNames><calcPr calcId="1"/></workbook>"#;
    const OUT_RELS: &str = r#"<Relationships xmlns="urn:r"><Relationship Id="rId1" Type="t" Target="worksheets/sheet1.xml"/><Relationship Id="rIdExt1" Type="t" Target="x"/></Relationships>"#;
    const OUT_TYPES: &str =
        r#"<Types xmlns="urn:t"><Override PartName="/xl/workbook.xml" ContentType="c"/></Types>"#;
    const IN_WORKBOOK: &str = r#"<workbook xmlns="urn:m" xmlns:r="urn:r"><sheets/><externalReferences><externalReference r:id="rId9"/><externalReference r:id="rId4"/></externalReferences></workbook>"#;
    const IN_RELS: &str = r#"<Relationships xmlns="urn:r"><Relationship Id="rId4" Type="t" Target="externalLinks/externalLink1.xml"/><Relationship Id="rId9" Type="t" Target="externalLinks/externalLink2.xml"/></Relationships>"#;
    const LINK1: &str = r#"<externalLink xmlns="urn:m"><externalBook><sheetNames><sheetName val="S"/></sheetNames><sheetDataSet><sheetData sheetId="0"><row r="1"><cell r="A1" t="str"><v>Invented Name</v></cell></row><row r="2"/></sheetData></sheetDataSet></externalBook></externalLink>"#;
    const LINK_RELS: &str = r#"<Relationships xmlns="urn:r"><Relationship Id="rId1" Type="x" Target="file.xlsx" TargetMode="External"/></Relationships>"#;

    fn linked_input() -> Vec<u8> {
        plain_package(&[
            (CONTENT_TYPES, OUT_TYPES),
            (WORKBOOK, IN_WORKBOOK),
            (WORKBOOK_RELS, IN_RELS),
            ("xl/externalLinks/externalLink1.xml", LINK1),
            ("xl/externalLinks/_rels/externalLink1.xml.rels", LINK_RELS),
            ("xl/externalLinks/externalLink2.xml", "<externalLink/>"),
        ])
    }

    fn plain_output(workbook: &str) -> Vec<u8> {
        plain_package(&[
            (CONTENT_TYPES, OUT_TYPES),
            (WORKBOOK, workbook),
            (WORKBOOK_RELS, OUT_RELS),
        ])
    }

    #[test]
    fn external_links_are_restored_in_input_order() {
        let out = scrub(&linked_input(), &plain_output(OUT_WORKBOOK)).unwrap();
        let workbook = part(&out, WORKBOOK);
        let first = workbook.find(r#"r:id="rIdExt2""#).unwrap();
        let second = workbook.find(r#"r:id="rIdExt3""#).unwrap();
        // Input order: externalLink2 (rId9) first, then externalLink1 (rId4).
        let rels = part(&out, WORKBOOK_RELS);
        assert!(rels.contains(r#"Id="rIdExt1" Type="t" Target="x""#));
        assert!(rels.contains(r#"Id="rIdExt3""#));
        assert!(rels.contains(r#"Id="rIdExt2""#));
        assert!(first < second);
        assert!(
            workbook.find("<externalReferences>").unwrap() > workbook.find("</sheets>").unwrap()
        );
        assert!(
            workbook.find("</externalReferences>").unwrap()
                < workbook.find("<definedNames>").unwrap()
        );
        let types = part(&out, CONTENT_TYPES);
        assert!(types.contains(r#"PartName="/xl/externalLinks/externalLink1.xml""#));
        assert!(types.contains(r#"PartName="/xl/externalLinks/externalLink2.xml""#));
        let link = part(&out, "xl/externalLinks/externalLink1.xml");
        assert!(!link.contains("Invented") && !link.contains("<row") && !link.contains("<cell"));
        assert!(link.contains(r#"<sheetData sheetId="0">"#));
        assert_eq!(
            part(&out, "xl/externalLinks/_rels/externalLink1.xml.rels"),
            LINK_RELS
        );
    }

    #[test]
    fn relationship_ids_point_to_their_links() {
        let out = scrub(&linked_input(), &plain_output(OUT_WORKBOOK)).unwrap();
        let rels = part(&out, WORKBOOK_RELS);
        // rIdExt1 is taken; the first link (externalLink2) gets rIdExt2.
        assert!(rels.contains(r#"Id="rIdExt2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink" Target="externalLinks/externalLink2.xml""#));
        assert!(rels.contains(r#"Id="rIdExt3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink" Target="externalLinks/externalLink1.xml""#));
    }

    #[test]
    fn existing_external_references_restore_nothing() {
        let workbook =
            OUT_WORKBOOK.replace("<definedNames>", "<externalReferences/><definedNames>");
        let output = plain_output(&workbook);
        let out = scrub(&linked_input(), &output).unwrap();
        assert_eq!(names(&out), names(&output));
        assert_eq!(part(&out, WORKBOOK), workbook);
    }

    #[test]
    fn input_without_links_changes_no_part() {
        let input = plain_package(&[(WORKBOOK, "<workbook/>")]);
        let output = plain_output(OUT_WORKBOOK);
        let out = scrub(&input, &output).unwrap();
        assert_eq!(names(&out), names(&output));
        assert_eq!(part(&out, WORKBOOK), OUT_WORKBOOK);
        assert_eq!(part(&out, WORKBOOK_RELS), OUT_RELS);
    }

    #[test]
    fn missing_link_part_is_an_error() {
        let input = plain_package(&[
            (WORKBOOK, IN_WORKBOOK),
            (WORKBOOK_RELS, IN_RELS),
            ("xl/externalLinks/externalLink1.xml", LINK1),
        ]);
        let error = scrub(&input, &plain_output(OUT_WORKBOOK)).unwrap_err();
        assert!(!format!("{error:#}").contains("Invented"));
    }

    #[test]
    fn declared_override_is_not_repeated() {
        let types = OUT_TYPES.replace(
            "</Types>",
            r#"<Override PartName="/xl/externalLinks/externalLink1.xml" ContentType="c"/></Types>"#,
        );
        let output = plain_package(&[
            (CONTENT_TYPES, &types),
            (WORKBOOK, OUT_WORKBOOK),
            (WORKBOOK_RELS, OUT_RELS),
        ]);
        let out = scrub(&linked_input(), &output).unwrap();
        let text = part(&out, CONTENT_TYPES);
        assert_eq!(text.matches("externalLink1.xml").count(), 1);
        assert_eq!(text.matches("externalLink2.xml").count(), 1);
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
        let error = scrub(b"not a zip", b"not a zip").unwrap_err();
        assert!(!format!("{error:#}").contains("not a zip"));
    }
}
