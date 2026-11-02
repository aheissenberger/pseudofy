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
/// and with the external links and sparklines of `input` restored (ADR-0011).
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
    restored.extend(restore_sparklines(input, &mut archive).map_err(|_| bad())?);

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
        parts.insert(part.clone(), strip_cached_values(&xml)?);
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
    let mut declared = override_names(&types)?;

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
        // `insert` also skips a part that two references share.
        if declared.insert(format!("/{part}")) {
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

const SPARKLINE_URI: &str = "{05C60535-1F16-4fd2-B633-F4F36F0B64E0}";

/// The sheets of a workbook as `(name, part)`, through its relationships.
fn sheet_parts(workbook: &[u8], rels: &[u8]) -> Result<Vec<(String, String)>, Failure> {
    let rels = relationships(rels)?;
    let mut reader = Reader::from_reader(workbook);
    let mut buf = Vec::new();
    let mut sheets = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) | Event::Empty(start) if is_name(&start, b"sheet") => {
                let name = attr(&start, b"name")?.ok_or("missing name")?;
                let id = attr(&start, b"id")?.ok_or("missing id")?;
                let (_, target) = rels
                    .iter()
                    .find(|(candidate, _)| *candidate == id)
                    .ok_or("unknown relationship")?;
                let part = match target.strip_prefix('/') {
                    Some(absolute) => absolute.to_owned(),
                    None => format!("xl/{target}"),
                };
                sheets.push((name, part));
            }
            Event::Eof => return Ok(sheets),
            _ => {}
        }
        buf.clear();
    }
}

/// The `xmlns:prefix` declarations of a start tag.
fn declarations(start: &BytesStart<'_>) -> Result<Vec<(String, String)>, Failure> {
    let mut found = Vec::new();
    for item in start.attributes() {
        let item = item?;
        let key = item.key.as_ref();
        if let Some(prefix) = key.strip_prefix(b"xmlns:") {
            let value = item.normalized_value(quick_xml::XmlVersion::Explicit1_0)?;
            found.push((String::from_utf8(prefix.to_vec())?, value.into_owned()));
        }
    }
    Ok(found)
}

fn is_sparkline_ext(start: &BytesStart<'_>) -> Result<bool, Failure> {
    Ok(is_name(start, b"ext") && attr(start, b"uri")?.as_deref() == Some(SPARKLINE_URI))
}

/// Prefixes that the element `xml` uses and does not declare itself.
fn undeclared_prefixes(xml: &[u8]) -> Result<Vec<String>, Failure> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut scopes: Vec<Vec<String>> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buf)?;
        match &event {
            Event::Start(start) | Event::Empty(start) => {
                let own: Vec<String> = declarations(start)?.into_iter().map(|(p, _)| p).collect();
                let mut used: Vec<String> = Vec::new();
                if let Some(prefix) = start.name().prefix() {
                    used.push(String::from_utf8(prefix.as_ref().to_vec())?);
                }
                for item in start.attributes() {
                    if let Some(prefix) = item?.key.prefix() {
                        used.push(String::from_utf8(prefix.as_ref().to_vec())?);
                    }
                }
                for prefix in used {
                    let bound = prefix == "xml"
                        || prefix == "xmlns"
                        || own.contains(&prefix)
                        || scopes.iter().any(|scope| scope.contains(&prefix));
                    if !bound && !missing.contains(&prefix) {
                        missing.push(prefix);
                    }
                }
                if matches!(event, Event::Start(_)) {
                    scopes.push(own);
                }
            }
            Event::End(_) => {
                scopes.pop();
            }
            Event::Eof => return Ok(missing),
            _ => {}
        }
        buf.clear();
    }
}

/// The sparkline `<ext>` elements of a worksheet, each with a declaration
/// for every namespace prefix that it uses.
fn sparkline_exts(sheet: &[u8]) -> Result<Vec<String>, Failure> {
    let mut reader = Reader::from_reader(sheet);
    let mut buf = Vec::new();
    let mut known: Vec<(String, String)> = Vec::new();
    let mut depth = 0usize;
    let mut in_list = false;
    let mut open = false;
    // Start, end, and name length of each sparkline ext.
    let mut found: Vec<(usize, usize, usize)> = Vec::new();
    loop {
        let before = usize::try_from(reader.buffer_position())?;
        let event = reader.read_event_into(&mut buf)?;
        match &event {
            Event::Start(start) => {
                depth += 1;
                if depth <= 2 {
                    known.extend(declarations(start)?);
                }
                if depth == 2 && is_name(start, b"extLst") {
                    in_list = true;
                } else if in_list && depth == 3 && !open && is_sparkline_ext(start)? {
                    open = true;
                    found.push((before, 0, start.name().as_ref().len()));
                }
            }
            Event::End(_) => {
                if open && depth == 3 {
                    let after = usize::try_from(reader.buffer_position())?;
                    if let Some(last) = found.last_mut() {
                        last.1 = after;
                    }
                    open = false;
                }
                if depth == 2 {
                    in_list = false;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    let mut exts = Vec::new();
    for (from, to, name_len) in found {
        let raw = sheet.get(from..to).ok_or("bad range")?;
        let mut declared: Vec<String> = Vec::new();
        for prefix in undeclared_prefixes(raw)? {
            let (_, uri) = known
                .iter()
                .rev()
                .find(|(candidate, _)| *candidate == prefix)
                .ok_or("undeclared prefix")?;
            declared.push(format!(
                r#" xmlns:{prefix}="{}""#,
                quick_xml::escape::escape(uri)
            ));
        }
        let text = std::str::from_utf8(raw)?;
        let split = 1 + name_len;
        let head = text.get(..split).ok_or("bad range")?;
        let tail = text.get(split..).ok_or("bad range")?;
        exts.push(format!("{head}{}{tail}", declared.concat()));
    }
    Ok(exts)
}

/// Add `snippet` to the `<extLst>` of a worksheet, or add an `<extLst>` as the
/// last child of the root. Returns `None` if the sheet has a sparkline `<ext>`.
fn add_to_ext_list(sheet: &[u8], snippet: &str) -> Result<Option<Vec<u8>>, Failure> {
    let mut reader = Reader::from_reader(sheet);
    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut done = false;
    loop {
        let event = reader.read_event_into(&mut buf)?;
        match &event {
            Event::Start(start) | Event::Empty(start) if is_sparkline_ext(start)? => {
                return Ok(None);
            }
            Event::Empty(start) if depth == 1 && !done && is_name(start, b"extLst") => {
                writer.write_event(Event::Start(start.to_owned()))?;
                writer.get_mut().extend_from_slice(snippet.as_bytes());
                writer.write_event(Event::End(start.to_end().into_owned()))?;
                done = true;
                buf.clear();
                continue;
            }
            Event::End(end) if !done && depth == 2 && end.local_name().as_ref() == b"extLst" => {
                writer.get_mut().extend_from_slice(snippet.as_bytes());
                done = true;
            }
            Event::End(_) if !done && depth == 1 => {
                writer.get_mut().extend_from_slice(b"<extLst>");
                writer.get_mut().extend_from_slice(snippet.as_bytes());
                writer.get_mut().extend_from_slice(b"</extLst>");
                done = true;
            }
            _ => {}
        }
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => {
                return if done {
                    Ok(Some(writer.into_inner()))
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

/// Build the worksheet parts of `output` that get the sparklines of the
/// sheet of `input` with the same name. Empty if `input` has no sparkline.
fn restore_sparklines<R: Read + std::io::Seek>(
    input: &[u8],
    output: &mut ZipArchive<R>,
) -> Result<HashMap<String, Vec<u8>>, Failure> {
    let mut parts = HashMap::new();
    let mut source = ZipArchive::new(Cursor::new(input))?;
    let (Some(in_workbook), Some(in_rels)) = (
        read_part(&mut source, WORKBOOK)?,
        read_part(&mut source, WORKBOOK_RELS)?,
    ) else {
        return Ok(parts);
    };
    let mut targets: Option<Vec<(String, String)>> = None;
    for (name, part) in sheet_parts(&in_workbook, &in_rels)? {
        let Some(sheet) = read_part(&mut source, &part)? else {
            continue;
        };
        let exts = sparkline_exts(&sheet)?;
        if exts.is_empty() {
            continue;
        }
        if targets.is_none() {
            let workbook = read_part(output, WORKBOOK)?.ok_or("no workbook part")?;
            let rels = read_part(output, WORKBOOK_RELS)?.ok_or("no relationships")?;
            targets = Some(sheet_parts(&workbook, &rels)?);
        }
        let (_, out_part) = targets
            .iter()
            .flatten()
            .find(|(candidate, _)| *candidate == name)
            .ok_or("no sheet in the copy")?;
        let out_sheet = read_part(output, out_part)?.ok_or("missing sheet part")?;
        if let Some(changed) = add_to_ext_list(&out_sheet, &exts.concat())? {
            parts.insert(out_part.clone(), changed);
        }
    }
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

/// Remove the cached values of an external link part: all content of each
/// `<sheetData>` (external workbook links) and each `<values>` element (DDE
/// links). The elements that hold no value stay.
fn strip_cached_values(xml: &[u8]) -> Result<Vec<u8>, Failure> {
    let mut reader = Reader::from_reader(xml);
    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buf)?;
        match &event {
            Event::Eof => return Ok(writer.into_inner()),
            Event::Start(start) if is_name(start, b"sheetData") => {
                let end = start.to_end().into_owned();
                writer.write_event(event.borrow())?;
                skip_to_end(&mut reader)?;
                writer.write_event(Event::End(end))?;
            }
            Event::Start(start) if is_name(start, b"values") => skip_to_end(&mut reader)?,
            Event::Empty(start) if is_name(start, b"values") => {}
            _ => writer.write_event(event.borrow())?,
        }
        buf.clear();
    }
}

/// Skip all events up to and including the end of the element just started.
fn skip_to_end(reader: &mut Reader<&[u8]>) -> Result<(), Failure> {
    let mut buf = Vec::new();
    let mut depth = 1usize;
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            Event::Eof => return Err("unclosed element".into()),
            _ => {}
        }
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
    fn dde_values_and_any_sheet_data_content_are_removed() {
        let dde = r#"<externalLink xmlns="urn:m"><ddeLink ddeService="Excel" ddeTopic="Book1"><ddeItems><ddeItem name="R1C1" advise="1"><values rows="1" cols="1"><value t="str"><val>Invented Dde</val></value></values></ddeItem><ddeItem name="R2C1"><values/></ddeItem></ddeItems></ddeLink></externalLink>"#;
        let stripped = String::from_utf8(strip_cached_values(dde.as_bytes()).unwrap()).unwrap();
        assert!(!stripped.contains("Invented") && !stripped.contains("<value"));
        assert!(stripped.contains(r#"<ddeItem name="R1C1" advise="1"></ddeItem>"#));

        let odd = r#"<externalLink><externalBook><sheetDataSet><sheetData sheetId="0"><cell r="A1"><v>Invented NoRow</v></cell></sheetData><x:sheetData sheetId="1"><x:row/></x:sheetData></sheetDataSet></externalBook></externalLink>"#;
        let stripped = String::from_utf8(strip_cached_values(odd.as_bytes()).unwrap()).unwrap();
        assert!(!stripped.contains("Invented") && !stripped.contains("<cell"));
        assert!(stripped.contains(r#"<sheetData sheetId="0"></sheetData>"#));
        assert!(stripped.contains(r#"<x:sheetData sheetId="1"></x:sheetData>"#));
    }

    #[test]
    fn two_references_to_one_part_declare_it_once() {
        let rels = IN_RELS.replace(
            "externalLinks/externalLink2.xml",
            "/xl/externalLinks/externalLink1.xml",
        );
        let input = plain_package(&[
            (CONTENT_TYPES, OUT_TYPES),
            (WORKBOOK, IN_WORKBOOK),
            (WORKBOOK_RELS, &rels),
            ("xl/externalLinks/externalLink1.xml", LINK1),
        ]);
        let out = scrub(&input, &plain_output(OUT_WORKBOOK)).unwrap();
        let text = part(&out, CONTENT_TYPES);
        assert_eq!(text.matches("externalLink1.xml").count(), 1);
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

    const SPARK_URI: &str = "{05C60535-1F16-4fd2-B633-F4F36F0B64E0}";

    fn spark_ext(extra_ns: &str) -> String {
        format!(
            r#"<ext uri="{SPARK_URI}" xmlns:x14="urn:x14"><x14:sparklineGroups xmlns:xm="urn:xm"><x14:sparklineGroup{extra_ns}><x14:sparklines><x14:sparkline><xm:f>Data!A1:C1</xm:f><xm:sqref>D1</xm:sqref></x14:sparkline></x14:sparklines></x14:sparklineGroup></x14:sparklineGroups></ext>"#
        )
    }

    fn book(sheets: &[(&str, &str, &str)]) -> Vec<(String, String)> {
        let mut workbook = Vec::new();
        let mut rels = Vec::new();
        for (index, (name, part, _)) in sheets.iter().enumerate() {
            workbook.push(format!(
                r#"<sheet name="{name}" sheetId="{}" r:id="rId{}"/>"#,
                index + 7,
                index + 1
            ));
            rels.push(format!(
                r#"<Relationship Id="rId{}" Type="t" Target="{part}"/>"#,
                index + 1
            ));
        }
        let workbook = format!(
            r#"<workbook xmlns:r="urn:r"><sheets>{}</sheets></workbook>"#,
            workbook.concat()
        );
        let rels = format!("<Relationships>{}</Relationships>", rels.concat());
        let mut parts = vec![
            (WORKBOOK.to_owned(), workbook),
            (WORKBOOK_RELS.to_owned(), rels),
        ];
        for (_, part, body) in sheets {
            let name = match part.strip_prefix('/') {
                Some(absolute) => absolute.to_owned(),
                None => format!("xl/{part}"),
            };
            parts.push((name, (*body).to_owned()));
        }
        parts
    }

    fn book_package(sheets: &[(&str, &str, &str)]) -> Vec<u8> {
        let parts = book(sheets);
        let refs: Vec<(&str, &str)> = parts
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_str()))
            .collect();
        plain_package(&refs)
    }

    fn spark_sheet(ext: &str) -> String {
        format!(
            r#"<worksheet xmlns="urn:m" xmlns:xr2="urn:xr2"><sheetData/><extLst>{ext}</extLst></worksheet>"#
        )
    }

    #[test]
    fn sparklines_go_into_a_sheet_without_ext_list() {
        let input = book_package(&[("A", "worksheets/sheet1.xml", &spark_sheet(&spark_ext("")))]);
        let output = book_package(&[(
            "A",
            "worksheets/sheet1.xml",
            r#"<worksheet xmlns="urn:m"><sheetData/><pageMargins/></worksheet>"#,
        )]);
        let out = scrub(&input, &output).unwrap();
        let sheet = part(&out, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains("<pageMargins/><extLst><ext uri="));
        assert!(sheet.ends_with("</ext></extLst></worksheet>"));
        assert_eq!(sheet.matches("<x14:sparkline>").count(), 1);
        assert!(sheet.contains("<xm:f>Data!A1:C1</xm:f><xm:sqref>D1</xm:sqref>"));
    }

    #[test]
    fn sparklines_are_appended_to_an_existing_ext_list() {
        let input = book_package(&[("A", "worksheets/sheet1.xml", &spark_sheet(&spark_ext("")))]);
        let other = r#"<ext uri="{other}"><o/></ext>"#;
        let output = book_package(&[(
            "A",
            "worksheets/sheet1.xml",
            &format!(
                r#"<worksheet xmlns="urn:m"><sheetData/><extLst>{other}</extLst></worksheet>"#
            ),
        )]);
        let out = scrub(&input, &output).unwrap();
        let sheet = part(&out, "xl/worksheets/sheet1.xml");
        assert_eq!(sheet.matches("<extLst>").count(), 1);
        assert!(sheet.contains(&format!("{other}<ext uri=")));
        assert!(sheet.ends_with("</ext></extLst></worksheet>"));

        let empty = book_package(&[(
            "A",
            "worksheets/sheet1.xml",
            r#"<worksheet xmlns="urn:m"><sheetData/><extLst/></worksheet>"#,
        )]);
        let out = scrub(&input, &empty).unwrap();
        let sheet = part(&out, "xl/worksheets/sheet1.xml");
        assert_eq!(sheet.matches("<extLst>").count(), 1);
        assert_eq!(sheet.matches("<x14:sparkline>").count(), 1);
    }

    #[test]
    fn existing_sparklines_are_not_duplicated() {
        let sheet = spark_sheet(&spark_ext(""));
        let input = book_package(&[("A", "worksheets/sheet1.xml", &sheet)]);
        let output = book_package(&[("A", "worksheets/sheet1.xml", &sheet)]);
        let out = scrub(&input, &output).unwrap();
        assert_eq!(part(&out, "xl/worksheets/sheet1.xml"), sheet);
    }

    #[test]
    fn sheets_are_matched_by_name() {
        let plain = r#"<worksheet xmlns="urn:m"><sheetData/></worksheet>"#;
        let input = book_package(&[
            ("A", "worksheets/sheet1.xml", plain),
            ("B", "worksheets/sheet2.xml", &spark_sheet(&spark_ext(""))),
        ]);
        let output = book_package(&[
            ("B", "/xl/worksheets/sheet1.xml", plain),
            ("A", "worksheets/sheet2.xml", plain),
        ]);
        let out = scrub(&input, &output).unwrap();
        assert!(part(&out, "xl/worksheets/sheet1.xml").contains("<x14:sparkline>"));
        assert_eq!(part(&out, "xl/worksheets/sheet2.xml"), plain);
    }

    #[test]
    fn missing_prefix_declaration_is_added() {
        let ext = spark_ext(r#" xr2:uid="{1}""#);
        let input = book_package(&[("A", "worksheets/sheet1.xml", &spark_sheet(&ext))]);
        let output = book_package(&[(
            "A",
            "worksheets/sheet1.xml",
            r#"<worksheet xmlns="urn:m"><sheetData/></worksheet>"#,
        )]);
        let out = scrub(&input, &output).unwrap();
        let sheet = part(&out, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains(r#"<ext xmlns:xr2="urn:xr2" uri="#));
        assert_eq!(sheet.matches("xmlns:x14=").count(), 1);
        assert_eq!(sheet.matches("xmlns:xm=").count(), 1);
    }

    #[test]
    fn input_without_sparklines_changes_nothing() {
        let plain = r#"<worksheet xmlns="urn:m"><sheetData/></worksheet>"#;
        let other = spark_sheet(r#"<ext uri="{o}"/>"#);
        let input = book_package(&[("A", "worksheets/sheet1.xml", &other)]);
        let output = book_package(&[("A", "worksheets/sheet1.xml", plain)]);
        let out = scrub(&input, &output).unwrap();
        assert_eq!(names(&out), names(&output));
        assert_eq!(part(&out, "xl/worksheets/sheet1.xml"), plain);
        assert_eq!(part(&out, WORKBOOK), part(&output, WORKBOOK));
    }

    #[test]
    fn unknown_sheet_is_an_error_without_content() {
        let ext = spark_ext("").replace("Data!A1:C1", "Invented!A1:C1");
        let input = book_package(&[("Invented", "worksheets/sheet1.xml", &spark_sheet(&ext))]);
        let output = book_package(&[("Other", "worksheets/sheet1.xml", "<worksheet/>")]);
        let error = scrub(&input, &output).unwrap_err();
        assert!(!format!("{error:#}").contains("Invented"));
    }

    #[test]
    fn sparklines_and_external_links_compose() {
        let mut parts = book(&[("A", "worksheets/sheet1.xml", &spark_sheet(&spark_ext("")))]);
        parts[0].1 = IN_WORKBOOK.replace(
            "<sheets/>",
            r#"<sheets><sheet name="A" sheetId="1" r:id="rId1"/></sheets>"#,
        );
        parts[1].1 = IN_RELS.replace(
            "</Relationships>",
            r#"<Relationship Id="rId1" Type="t" Target="worksheets/sheet1.xml"/></Relationships>"#,
        );
        parts.push((
            "xl/externalLinks/externalLink1.xml".to_owned(),
            LINK1.to_owned(),
        ));
        parts.push((
            "xl/externalLinks/externalLink2.xml".to_owned(),
            "<externalLink/>".to_owned(),
        ));
        parts.push((CONTENT_TYPES.to_owned(), OUT_TYPES.to_owned()));
        let refs: Vec<(&str, &str)> = parts
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_str()))
            .collect();
        let input = plain_package(&refs);
        let out_rels = OUT_RELS.to_owned();
        let out_workbook = OUT_WORKBOOK.to_owned();
        let output = plain_package(&[
            (CONTENT_TYPES, OUT_TYPES),
            (WORKBOOK, &out_workbook),
            (WORKBOOK_RELS, &out_rels),
            (
                "xl/worksheets/sheet1.xml",
                "<worksheet><sheetData/></worksheet>",
            ),
        ]);
        let out = scrub(&input, &output).unwrap();
        assert!(part(&out, WORKBOOK).contains("<externalReferences>"));
        assert!(part(&out, "xl/worksheets/sheet1.xml").contains("<x14:sparkline>"));
    }
}
