//! Scrub the written XLSX package (ADR-0010).
//!
//! `umya-spreadsheet` keeps the old shared strings of a changed cell. The
//! scrub blanks each entry of `xl/sharedStrings.xml` that no worksheet uses.
//! Error texts never contain part contents.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use anyhow::anyhow;
use quick_xml::events::{BytesEnd, BytesStart, Event};
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
            let xml = read_limited(&mut file).map_err(|_| bad())?;
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
            let xml =
                read_limited(archive.by_index(index).map_err(|_| bad())?).map_err(|_| bad())?;
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
const STYLES: &str = "xl/styles.xml";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PACKAGE_REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
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
        Ok(file) => Ok(Some(read_limited(file)?)),
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
    if ids.len() > MAX_RELATIONSHIPS {
        return Err("too many external references".into());
    }
    let out_workbook = read_part(output, WORKBOOK)?.ok_or("no workbook part")?;
    if has_element(&out_workbook, b"externalReferences")? {
        return Ok(parts);
    }
    let in_rels = read_part(&mut source, WORKBOOK_RELS)?.ok_or("no relationships")?;
    let in_rels = rel_map(&in_rels)?;

    // The link parts, in the order of the input.
    let mut link_parts = Vec::new();
    for id in &ids {
        let target = in_rels.get(id).ok_or("unknown relationship")?;
        link_parts.push(link_part(target)?);
    }
    for part in &link_parts {
        // Many references can share one part: read it once.
        if parts.contains_key(part) {
            continue;
        }
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

/// Resolve the target of a relationship of `xl/workbook.xml` to a part name.
/// A leading `/` is the package root; `.` and `..` segments are resolved.
fn resolve_target(target: &str) -> Result<String, Failure> {
    let (mut segments, rest) = match target.strip_prefix('/') {
        Some(absolute) => (Vec::new(), absolute),
        None => (vec!["xl"], target),
    };
    for segment in rest.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop().ok_or("bad target")?;
            }
            other => segments.push(other),
        }
    }
    Ok(segments.join("/"))
}

/// The relationship targets by id. The first of two equal ids wins.
fn rel_map(rels: &[u8]) -> Result<HashMap<String, String>, Failure> {
    let mut map = HashMap::new();
    for (id, target) in relationships(rels)? {
        map.entry(id).or_insert(target);
    }
    Ok(map)
}

/// The `<sheet>` entries of a workbook as `(name, relationship id)`.
fn sheet_refs(workbook: &[u8]) -> Result<Vec<(String, String)>, Failure> {
    let mut reader = Reader::from_reader(workbook);
    let mut buf = Vec::new();
    let mut sheets = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) | Event::Empty(start) if is_name(&start, b"sheet") => {
                let name = attr(&start, b"name")?.ok_or("missing name")?;
                // The id as the crate reads it (exact `r:id` first).
                let id = crate_sheet_id(&start).ok_or("missing id")?;
                sheets.push((name, id));
            }
            Event::Eof => return Ok(sheets),
            _ => {}
        }
        buf.clear();
    }
}

/// The sheets of a workbook as `(name, part)`, through its relationships.
fn sheet_parts(workbook: &[u8], rels: &[u8]) -> Result<Vec<(String, String)>, Failure> {
    let rels = rel_map(rels)?;
    let mut sheets = Vec::new();
    for (name, id) in sheet_refs(workbook)? {
        let target = rels.get(&id).ok_or("unknown relationship")?;
        sheets.push((name, resolve_target(target)?));
    }
    Ok(sheets)
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

/// Whether `child` may be a child of `parent` in a sparkline `<ext>`
/// (ECMA-376 and MS-XLSX, local names).
fn allowed_child(parent: &[u8], child: &[u8]) -> bool {
    match parent {
        b"ext" => child == b"sparklineGroups",
        b"sparklineGroups" => child == b"sparklineGroup",
        b"sparklineGroup" => matches!(
            child,
            b"colorSeries"
                | b"colorNegative"
                | b"colorAxis"
                | b"colorMarkers"
                | b"colorFirst"
                | b"colorLast"
                | b"colorHigh"
                | b"colorLow"
                | b"sparklines"
                | b"f"
        ),
        b"sparklines" => child == b"sparkline",
        b"sparkline" => matches!(child, b"f" | b"sqref"),
        _ => false,
    }
}

/// The sparkline `<ext>` elements of a worksheet, rebuilt from the known
/// sparkline elements only. `qual` is the prefix of the main namespace of the
/// copy with a colon, or empty. Each `<ext>` declares every namespace prefix
/// that it uses. Prefixes come only from the root and the `<extLst>`.
fn sparkline_exts(sheet: &[u8], qual: &str) -> Result<Vec<String>, Failure> {
    let mut reader = Reader::from_reader(sheet);
    let mut buf = Vec::new();
    let mut known: Vec<(String, String)> = Vec::new();
    let mut depth = 0usize;
    let mut in_list = false;
    let mut exts = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buf)?;
        match event {
            Event::Start(start) => {
                if depth == 0 {
                    if !is_name(&start, b"worksheet") {
                        return Ok(exts);
                    }
                    known.extend(declarations(&start)?);
                } else if depth == 1 && is_name(&start, b"extLst") {
                    known.extend(declarations(&start)?);
                    in_list = true;
                } else if depth == 2 && in_list && is_sparkline_ext(&start)? {
                    let rebuilt = rebuild_ext(&mut reader, &start, qual)?;
                    let mut declared = String::new();
                    for prefix in undeclared_prefixes(rebuilt.as_bytes())? {
                        if qual.strip_suffix(':') == Some(prefix.as_str()) {
                            continue;
                        }
                        let (_, uri) = known
                            .iter()
                            .rev()
                            .find(|(candidate, _)| *candidate == prefix)
                            .ok_or("undeclared prefix")?;
                        declared.extend([
                            " xmlns:",
                            &prefix,
                            "=\"",
                            &quick_xml::escape::escape(uri),
                            "\"",
                        ]);
                    }
                    let split = 1 + qual.len() + "ext".len();
                    let head = rebuilt.get(..split).ok_or("bad range")?;
                    let tail = rebuilt.get(split..).ok_or("bad range")?;
                    exts.push(format!("{head}{declared}{tail}"));
                    buf.clear();
                    continue;
                }
                depth += 1;
            }
            Event::Empty(start) => {
                if depth == 0 {
                    if !is_name(&start, b"worksheet") {
                        return Ok(exts);
                    }
                    known.extend(declarations(&start)?);
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 1 {
                    in_list = false;
                }
            }
            Event::Eof => return Ok(exts),
            _ => {}
        }
        buf.clear();
    }
}

/// Rebuild the sparkline `<ext>` whose start tag `ext` was just read: keep the
/// known elements in their positions with their attributes, and the text of
/// `f` and `sqref`. Drop all else. The root is named with `qual`.
fn rebuild_ext(
    reader: &mut Reader<&[u8]>,
    ext: &BytesStart<'_>,
    qual: &str,
) -> Result<String, Failure> {
    let root = format!("{qual}ext");
    let mut start = BytesStart::new(root.clone());
    for item in ext.attributes() {
        start.push_attribute(item?);
    }
    let mut writer = Writer::new(Vec::new());
    writer.write_event(Event::Start(start))?;
    let mut stack: Vec<Vec<u8>> = vec![b"ext".to_vec()];
    let mut skip = 0usize;
    let mut buf = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buf)?;
        match event {
            Event::Start(start) => {
                let local = start.local_name().as_ref().to_vec();
                let top = stack.last().ok_or("empty stack")?;
                if skip == 0 && allowed_child(top, &local) {
                    writer.write_event(Event::Start(start))?;
                    stack.push(local);
                } else {
                    skip += 1;
                }
            }
            Event::Empty(start) => {
                let top = stack.last().ok_or("empty stack")?;
                if skip == 0 && allowed_child(top, start.local_name().as_ref()) {
                    writer.write_event(Event::Empty(start))?;
                }
            }
            Event::End(end) => {
                if skip > 0 {
                    skip -= 1;
                } else {
                    stack.pop();
                    if stack.is_empty() {
                        writer.write_event(Event::End(BytesEnd::new(root)))?;
                        break;
                    }
                    writer.write_event(Event::End(end))?;
                }
            }
            Event::Text(_) | Event::GeneralRef(_) => {
                let top = stack.last().ok_or("empty stack")?;
                if skip == 0 && (top == b"f" || top == b"sqref") {
                    writer.write_event(event)?;
                }
            }
            Event::Eof => return Err("unclosed ext".into()),
            _ => {}
        }
        buf.clear();
    }
    Ok(String::from_utf8(writer.into_inner())?)
}

/// The prefix of the root element of `xml` with a colon, or an empty string.
fn root_qualifier(xml: &[u8]) -> Result<String, Failure> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) | Event::Empty(start) => {
                return Ok(match start.name().prefix() {
                    Some(prefix) => format!("{}:", std::str::from_utf8(prefix.as_ref())?),
                    None => String::new(),
                });
            }
            Event::Eof => return Err("no root".into()),
            _ => {}
        }
        buf.clear();
    }
}

/// Add `snippet` to the `<extLst>` of a worksheet, or add an `<extLst>` as the
/// last child of the root. Returns `None` if the sheet has a sparkline `<ext>`.
/// `qual` is the prefix of the main namespace with a colon, or empty.
fn add_to_ext_list(sheet: &[u8], snippet: &str, qual: &str) -> Result<Option<Vec<u8>>, Failure> {
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
                let list = format!("<{qual}extLst>{snippet}</{qual}extLst>");
                writer.get_mut().extend_from_slice(list.as_bytes());
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

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Build the worksheet parts of `output` that get the sparklines of the
/// sheet of `input` with the same name. Empty if `input` has no sparkline.
fn restore_sparklines<R: Read + std::io::Seek>(
    input: &[u8],
    output: &mut ZipArchive<R>,
) -> Result<HashMap<String, Vec<u8>>, Failure> {
    let mut parts = HashMap::new();
    let mut source = ZipArchive::new(Cursor::new(input))?;
    // Input parts that have a sparkline `<ext>`, by normalized name, and the
    // normalized names of all input parts.
    let mut found: HashMap<String, Vec<u8>> = HashMap::new();
    let mut present: HashSet<String> = HashSet::new();
    for index in 0..source.len() {
        let file = source.by_index(index)?;
        let name = fold(file.name());
        if file.is_dir() || !is_xml(&name) {
            present.insert(name);
            continue;
        }
        let bytes = read_limited(file)?;
        if contains(&bytes, SPARKLINE_URI.as_bytes()) && !sparkline_exts(&bytes, "")?.is_empty() {
            found.insert(name.clone(), bytes);
        }
        present.insert(name);
    }
    if found.is_empty() {
        return Ok(parts);
    }
    let in_workbook = read_part(&mut source, WORKBOOK)?.ok_or("no workbook part")?;
    let in_rels = read_part(&mut source, WORKBOOK_RELS)?.ok_or("no relationships")?;
    let out_workbook = read_part(output, WORKBOOK)?.ok_or("no workbook part")?;
    let out_rels = read_part(output, WORKBOOK_RELS)?.ok_or("no relationships")?;
    let mut targets: HashMap<String, String> = HashMap::new();
    for (name, part) in sheet_parts(&out_workbook, &out_rels)? {
        targets.entry(name).or_insert(part);
    }
    for (name, part) in sheet_parts(&in_workbook, &in_rels)? {
        let part = fold(&part);
        let Some(sheet) = found.get(&part) else {
            if !present.contains(&part) {
                return Err("missing sheet part".into());
            }
            continue;
        };
        let out_part = targets.get(&name).ok_or("no sheet in the copy")?;
        let out_sheet = read_part(output, out_part)?.ok_or("missing sheet part")?;
        let qual = root_qualifier(&out_sheet)?;
        let exts = sparkline_exts(sheet, &qual)?;
        if let Some(changed) = add_to_ext_list(&out_sheet, &exts.concat(), &qual)? {
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

// ---- Package check (spec 0017) ----

const MAX_ENTRIES: usize = 10_000;
const MAX_RELATIONSHIPS: usize = 10_000;
const MAX_DEPTH: usize = 16;
const MAX_SHEETS: usize = 4_096;
const MAX_BYTES: u64 = 512 * 1024 * 1024;
/// Limit for the work of the case-insensitive part lookups of the crate
/// (name comparisons in total).
const MAX_LOOKUPS: u64 = 10_000_000;

const UNREADABLE: &str = "cannot read the workbook";
const PIVOT: &str = "workbook has pivot tables; not supported";
const TOO_LARGE: &str = "workbook is too large";
const CYCLE: &str = "workbook has a relationship cycle";
const TOO_DEEP: &str = "workbook has relationships nested too deeply";
const NAMES: &str = "workbook has ambiguous part names";
const TOO_MANY_SHEETS: &str = "workbook has too many sheets";
const SHEET_IDS: &str = "workbook lists one relationship for two sheets";

/// Normalized part name: `\` becomes `/`, ASCII lowercase.
fn fold(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

/// Read `reader` up to `MAX_BYTES`. A longer stream is an error.
fn read_limited(reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).is_ok_and(|len| len > MAX_BYTES) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "part is too large",
        ));
    }
    Ok(bytes)
}

/// True if the name (normalized) is below the pivot cache or pivot table folder.
fn is_pivot_name(name: &str) -> bool {
    let name = fold(name);
    let name = name.trim_start_matches(['/', '.']);
    name.starts_with("xl/pivotcache/") || name.starts_with("xl/pivottables/")
}

/// True if the package has a pivot cache or a pivot table part.
pub fn has_pivot_entries(package: &[u8]) -> bool {
    ZipArchive::new(Cursor::new(package))
        .is_ok_and(|archive| archive.file_names().any(is_pivot_name))
}

fn pivot_type(rel_type: &str) -> bool {
    let lower = rel_type.to_ascii_lowercase();
    ["/pivottable", "/pivotcachedefinition", "/pivotcacherecords"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn pivot_content_type(content_type: &str) -> bool {
    let lower = content_type.to_ascii_lowercase();
    ["pivottable+", "pivotcachedefinition+", "pivotcacherecords+"]
        .iter()
        .any(|word| lower.contains(word))
}

/// `umya-spreadsheet` normalization of a path (`.` and `..` resolved).
fn normalize_path(path: &str) -> String {
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                out.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
        }
    }
    out.to_str().unwrap_or("").replace('\\', "/")
}

/// The join of `umya-spreadsheet`: `target` below `base`, or from the root
/// if it starts with `/`.
fn join_paths(base: &str, target: &str) -> String {
    match target.split_once('/') {
        Some(("", rest)) => normalize_path(rest),
        _ => normalize_path(&format!("{base}/{target}")),
    }
}

/// The folder part and the file part of a part name.
fn split_part(part: &str) -> (&str, &str) {
    part.rsplit_once('/').unwrap_or(("", part))
}

/// One relationship of a `.rels` part, as pseudofy and the crate read it.
struct Rel {
    id: String,
    kind: String,
    target: String,
    external: bool,
}

/// True if the crate, which reads attribute values without unescaping, reads
/// the same text as the parser: no character that the written escape changes.
/// A `>` is refused too: the crate reads an entity of it (`&gt;`) in a part
/// name as written, so a name could point to two different parts.
fn plain_text(text: &str) -> bool {
    !text
        .chars()
        .any(|c| matches!(c, '&' | '<' | '>' | '"') || c.is_control())
}

/// The values of one `<Relationship>`: only the exact, unprefixed names count
/// (the crate reads no other). A repeated name is an error.
fn read_relationship(start: &BytesStart<'_>) -> Result<Rel, Failure> {
    let mut fields: [Option<String>; 4] = [None, None, None, None];
    for item in start.attributes() {
        let item = item?;
        let slot = match item.key.as_ref() {
            b"Id" => 0,
            b"Type" => 1,
            b"Target" => 2,
            b"TargetMode" => 3,
            _ => continue,
        };
        // No entity in the mode: the crate compares the raw text.
        if slot == 3 && item.value.contains(&b'&') {
            return Err("unusual relationship".into());
        }
        let value = item.normalized_value(quick_xml::XmlVersion::Explicit1_0)?;
        fields[slot] = Some(value.into_owned());
    }
    let [Some(id), Some(kind), Some(target), mode] = fields else {
        return Err("incomplete relationship".into());
    };
    let external = mode.as_deref() == Some("External");
    if id.is_empty() || !plain_text(&id) || (!external && !plain_text(&target)) {
        return Err("unusual relationship".into());
    }
    Ok(Rel {
        id,
        kind,
        target,
        external,
    })
}

/// The relationships of a `.rels` part. Only a `Relationships` root with
/// `Relationship` children is accepted. More than `limit` is an error.
fn rel_entries(xml: &[u8], limit: usize) -> Result<Vec<Rel>, &'static str> {
    let parse = || -> Result<Vec<Rel>, Failure> {
        let mut reader = Reader::from_reader(xml);
        let mut buf = Vec::new();
        let mut found: Vec<Rel> = Vec::new();
        let mut ids: HashSet<String> = HashSet::new();
        let mut depth = 0usize;
        let mut roots = 0usize;
        loop {
            let event = reader.read_event_into(&mut buf)?;
            if let Event::Start(ref start) | Event::Empty(ref start) = event {
                match (depth, start.name().as_ref()) {
                    (0, b"Relationships") => roots += 1,
                    (1, b"Relationship") => {
                        if found.len() >= limit {
                            return Err(TOO_LARGE.into());
                        }
                        let rel = read_relationship(start)?;
                        if !ids.insert(rel.id.clone()) {
                            return Err("repeated relationship id".into());
                        }
                        found.push(rel);
                    }
                    _ => return Err("unexpected element".into()),
                }
                if roots > 1 {
                    return Err("unexpected element".into());
                }
            }
            match event {
                Event::Start(_) => depth += 1,
                Event::End(_) => depth = depth.saturating_sub(1),
                Event::DocType(_) => return Err("unexpected doctype".into()),
                Event::Eof => return Ok(found),
                _ => {}
            }
            buf.clear();
        }
    };
    parse().map_err(|error| {
        if error.to_string() == TOO_LARGE {
            TOO_LARGE
        } else {
            UNREADABLE
        }
    })
}

fn escape_attr(text: &str) -> String {
    // A bare `>` is legal in an attribute value, and the crate reads the
    // value as written: no `&gt;` here.
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

/// The canonical `.rels` part for `list`: only `Id`, `Type`, `Target` and
/// (for an External target) `TargetMode`, escaped. The crate reads the
/// values of this part as written, so it reads what the check saw.
fn canonical_rels(list: &[Rel]) -> Vec<u8> {
    let mut text = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"{PACKAGE_REL_NS}\">"
    );
    for rel in list {
        text.push_str("<Relationship");
        for (name, value) in [
            ("Id", &rel.id),
            ("Type", &rel.kind),
            ("Target", &rel.target),
        ] {
            text.push(' ');
            text.push_str(name);
            text.push_str("=\"");
            text.push_str(&escape_attr(value));
            text.push('"');
        }
        if rel.external {
            text.push_str(" TargetMode=\"External\"");
        }
        text.push_str("/>");
    }
    text.push_str("</Relationships>");
    text.into_bytes()
}

/// The `r:id` of a `<sheet>` as the crate reads it: the exact key first,
/// then the first key with the local name `id`. The raw value.
fn crate_sheet_id(start: &BytesStart<'_>) -> Option<String> {
    let mut by_local = None;
    for item in start.attributes().with_checks(false).flatten() {
        let key = item.key.into_inner();
        if key == b"r:id" {
            return Some(String::from_utf8_lossy(&item.value).into_owned());
        }
        let local = key.rsplit(|byte| *byte == b':').next().unwrap_or(key);
        if by_local.is_none() && local == b"id" {
            by_local = Some(String::from_utf8_lossy(&item.value).into_owned());
        }
    }
    by_local
}

/// The relationship ids of the `<sheet>` elements of a workbook.
fn sheet_rids(workbook: &[u8]) -> Result<Vec<String>, &'static str> {
    let mut reader = Reader::from_reader(workbook);
    let mut buf = Vec::new();
    let mut ids = Vec::new();
    let mut count = 0usize;
    loop {
        match reader.read_event_into(&mut buf).map_err(|_| UNREADABLE)? {
            Event::Start(start) | Event::Empty(start) if start.name().as_ref() == b"sheet" => {
                count += 1;
                if count > MAX_SHEETS {
                    return Err(TOO_MANY_SHEETS);
                }
                ids.extend(crate_sheet_id(&start));
            }
            Event::Eof => return Ok(ids),
            _ => {}
        }
        buf.clear();
    }
}

/// True if a content type of `[Content_Types].xml` is a pivot type.
fn has_pivot_content_type(xml: &[u8]) -> Result<bool, &'static str> {
    let scan = || -> Result<bool, Failure> {
        let mut reader = Reader::from_reader(xml);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(start) | Event::Empty(start) => {
                    if let Some(kind) = attr(&start, b"ContentType")?
                        && pivot_content_type(&kind)
                    {
                        return Ok(true);
                    }
                }
                Event::Eof => return Ok(false),
                _ => {}
            }
            buf.clear();
        }
    };
    scan().map_err(|_| UNREADABLE)
}

/// The package with the part lookup of `umya-spreadsheet`.
struct Package<'a> {
    archive: ZipArchive<Cursor<&'a [u8]>>,
    /// Part names in ASCII lowercase; unique after `open`.
    folded: HashMap<String, usize>,
    sizes: Vec<u64>,
    /// Name comparisons of the lookups that need the slow path of the crate.
    lookups: u64,
    /// Parts that were counted once.
    seen: HashSet<usize>,
    /// Bytes of all parts, plus one more time per repeated reference.
    total: u64,
    rels: HashMap<usize, Rc<Vec<Rel>>>,
    parsed: usize,
    visited: usize,
}

impl<'a> Package<'a> {
    /// Open the package. The names must be canonical: each raw name finds
    /// itself, none has a `\` or a leading `/`, and no two are equal after
    /// ASCII lowercase. Then the crate and this check find the same entry.
    fn open(package: &'a [u8]) -> Result<(Self, Vec<String>), &'static str> {
        let mut archive = ZipArchive::new(Cursor::new(package)).map_err(|_| UNREADABLE)?;
        if archive.len() > MAX_ENTRIES {
            return Err(TOO_LARGE);
        }
        if archive.file_names().any(is_pivot_name) {
            return Err(PIVOT);
        }
        let mut folded = HashMap::new();
        let mut names = Vec::new();
        for index in 0..archive.len() {
            let file = archive.by_index_raw(index).map_err(|_| UNREADABLE)?;
            if file.size() > MAX_BYTES {
                return Err(TOO_LARGE);
            }
            let name = file.name().to_owned();
            drop(file);
            if archive.index_for_name(&name) != Some(index)
                || name.contains('\\')
                || name.starts_with('/')
                || folded.insert(name.to_ascii_lowercase(), index).is_some()
            {
                return Err(NAMES);
            }
            names.push(name);
        }
        let mut sizes = Vec::new();
        let mut budget = MAX_BYTES;
        for index in 0..archive.len() {
            let file = archive.by_index(index).map_err(|_| UNREADABLE)?;
            // The real size counts, not the declared one.
            let used = std::io::copy(&mut file.take(budget + 1), &mut std::io::sink())
                .map_err(|_| UNREADABLE)?;
            if used > budget {
                return Err(TOO_LARGE);
            }
            budget -= used;
            sizes.push(used);
        }
        let package = Package {
            archive,
            folded,
            sizes,
            lookups: 0,
            seen: HashSet::new(),
            total: MAX_BYTES - budget,
            rels: HashMap::new(),
            parsed: 0,
            visited: 0,
        };
        Ok((package, names))
    }

    /// Count `cost` name comparisons of the crate.
    fn charge(&mut self, cost: usize) -> Result<(), &'static str> {
        self.lookups = self
            .lookups
            .saturating_add(u64::try_from(cost).unwrap_or(u64::MAX));
        if self.lookups > MAX_LOOKUPS {
            return Err(TOO_LARGE);
        }
        Ok(())
    }

    /// The entry that `umya-spreadsheet` finds for `name`: the exact name, or
    /// else a scan over the names in the order of the archive.
    fn find(&mut self, name: &str) -> Result<Option<usize>, &'static str> {
        if let Some(index) = self.archive.index_for_name(name) {
            return Ok(Some(index));
        }
        let found = self
            .folded
            .get(&name.replace('\\', "/").to_ascii_lowercase())
            .copied();
        self.charge(found.map_or(self.sizes.len(), |index| index + 1))?;
        Ok(found)
    }

    /// Count a reference to the part. A repeated reference adds its size again.
    fn count(&mut self, index: usize) -> Result<(), &'static str> {
        if !self.seen.insert(index) {
            self.total += self.sizes.get(index).copied().unwrap_or(0);
            if self.total > MAX_BYTES {
                return Err(TOO_LARGE);
            }
        }
        Ok(())
    }

    fn read(&mut self, index: usize) -> Result<Vec<u8>, &'static str> {
        let file = self.archive.by_index(index).map_err(|_| UNREADABLE)?;
        read_limited(file).map_err(|_| TOO_LARGE)
    }

    fn rels_of(&mut self, index: usize) -> Result<Rc<Vec<Rel>>, &'static str> {
        if let Some(list) = self.rels.get(&index) {
            return Ok(Rc::clone(list));
        }
        let xml = self.read(index)?;
        let list = rel_entries(&xml, MAX_RELATIONSHIPS - self.parsed)?;
        self.parsed += list.len();
        let list = Rc::new(list);
        self.rels.insert(index, Rc::clone(&list));
        Ok(list)
    }

    /// Count one relationship that the crate follows.
    fn touch(&mut self) -> Result<(), &'static str> {
        self.visited += 1;
        if self.visited > MAX_RELATIONSHIPS {
            return Err(TOO_LARGE);
        }
        Ok(())
    }

    /// Follow the relationships of `part` like the crate does: the rels part
    /// is `<folder>/_rels/<file>.rels`, a target is joined to the folder of
    /// `part`. Each reference counts, as the crate has no visited set. An
    /// External relationship makes the crate look up the empty part.
    fn walk(
        &mut self,
        part: &str,
        depth: usize,
        on_path: &mut HashSet<String>,
    ) -> Result<(), &'static str> {
        let (folder, file) = split_part(part);
        let rels_name = join_paths(folder, &format!("_rels/{file}.rels"));
        let Some(rels_index) = self.find(&rels_name)? else {
            return Ok(());
        };
        if depth > MAX_DEPTH {
            return Err(TOO_DEEP);
        }
        self.count(rels_index)?;
        let list = self.rels_of(rels_index)?;
        let key = fold(part);
        on_path.insert(key.clone());
        for rel in list.iter() {
            if rel.external {
                self.touch()?;
                self.charge(self.sizes.len())?;
            } else {
                self.visit(folder, &rel.target, depth + 1, on_path)?;
            }
        }
        on_path.remove(&key);
        Ok(())
    }

    fn visit(
        &mut self,
        folder: &str,
        target: &str,
        depth: usize,
        on_path: &mut HashSet<String>,
    ) -> Result<(), &'static str> {
        self.touch()?;
        let child = join_paths(folder, target);
        if let Some(index) = self.find(&child)? {
            self.count(index)?;
        }
        if on_path.contains(&fold(&child)) {
            return Err(CYCLE);
        }
        self.walk(&child, depth, on_path)
    }

    /// Estimate the memory of the crate for the shared strings, the styles,
    /// and the sheets that it reads. Refuses more than `MAX_MEMORY`. The
    /// sheets were counted by the walk of the relationships already.
    fn estimate(&mut self) -> Result<(), &'static str> {
        let Some(workbook) = self.find(WORKBOOK)? else {
            return Ok(());
        };
        let xml = self.read(workbook)?;
        let ids = sheet_rids(&xml)?;
        drop(xml);
        let rels_index = self.find(WORKBOOK_RELS)?.ok_or(UNREADABLE)?;
        let rels = self.rels_of(rels_index)?;

        let strings = match self.find(SHARED_STRINGS)? {
            Some(index) => {
                let xml = self.read(index)?;
                shared_string_sizes(&xml)?
            }
            None => Vec::new(),
        };
        let styles = match self.find(STYLES)? {
            Some(index) => {
                let xml = self.read(index)?;
                style_cost(&xml)?
            }
            None => StyleCost {
                formats: Vec::new(),
                shared: 0,
            },
        };
        // The table, the style lists, and the expanded cell formats.
        let mut total = u64::try_from(strings.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(SHARED_ITEM_OVERHEAD)
            .saturating_add(strings.iter().map(|size| u64::from(*size)).sum::<u64>())
            .saturating_add(styles.shared)
            .saturating_add(styles.formats.iter().sum::<u64>());
        if total > MAX_MEMORY {
            return Err(TOO_LARGE);
        }
        let by_id: HashMap<&str, &Rel> = rels.iter().map(|rel| (rel.id.as_str(), rel)).collect();
        for id in ids {
            // The crate reads the target of the relationship with the id.
            if let Some(rel) = by_id.get(id.as_str()) {
                let target = rel.target.strip_prefix("/xl/").unwrap_or(&rel.target);
                if let Some(index) = self.find(&join_paths("xl", target))? {
                    let xml = self.read(index)?;
                    sheet_cost(&xml, &strings, &styles, &mut total)?;
                }
            }
        }
        Ok(())
    }

    /// The package again, with each `.rels` part in its canonical form.
    fn canonical(&mut self) -> Result<Vec<u8>, &'static str> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for index in 0..self.archive.len() {
            if let Some(list) = self.rels.get(&index) {
                let name = {
                    let file = self.archive.by_index_raw(index).map_err(|_| UNREADABLE)?;
                    file.name().to_owned()
                };
                writer.start_file(name, options).map_err(|_| UNREADABLE)?;
                writer
                    .write_all(&canonical_rels(list))
                    .map_err(|_| UNREADABLE)?;
            } else {
                let file = self.archive.by_index_raw(index).map_err(|_| UNREADABLE)?;
                writer.raw_copy_file(file).map_err(|_| UNREADABLE)?;
            }
        }
        let cursor = writer.finish().map_err(|_| UNREADABLE)?;
        Ok(cursor.into_inner())
    }
}

// ---- Memory estimate (spec 0017) ----
//
// `umya-spreadsheet` copies a shared string into every `t="s"` cell and the
// whole expanded style (font, fill, border, number format) into every cell,
// row, and column with an `s` attribute, and into every cell format. A small
// package can need gigabytes. The estimate walks the same data as the crate:
// it streams the shared strings, the styles, and each sheet that the crate
// reads, and sums the bytes of the copies plus a constant per copy.
//
// The constants are measured on a release build of pseudofy (peak RSS of a
// whole run, difference between a workbook with and without the copies;
// invented workbooks with 100 000 to 500 000 objects):
// - 5 000 `t="s"` cells of one 30 000 character string: 157 MB, so the copy
//   costs its bytes (factor 1);
// - 500 000 `t="s"` cells of a 10 character string: +17 MB, 34 bytes a cell;
// - 100 000 cells with `s` (font name of 5 or 31 characters): +45 MB, 450
//   bytes a cell, almost independent of the font name;
// - 90 000 more cell formats: +119 MB, 1.3 KB a cell format;
// - 480 000 more shared string items: +75 MB, 156 bytes an item.
// The copied bytes (raw XML of font, fill, border, and number format; the
// text of the string) are added with factor 1 to the overhead of the object.

/// Refuse a package whose estimate is above 1 GiB.
const MAX_MEMORY: u64 = 1 << 30;
/// Limits of Excel.
const MAX_SHARED_CHARS: usize = 32_767;
const MAX_FONT_NAME: usize = 31;
const MAX_FORMAT_CHARS: usize = 255;
/// Items of one list of the styles (fonts, fills, borders, formats, cell formats).
const MAX_STYLE_ITEMS: usize = 1 << 20;
/// Bytes of the crate per shared string item in the table.
const SHARED_ITEM_OVERHEAD: u64 = 128;
/// Bytes of the crate per cell with a shared string.
const STRING_COPY_OVERHEAD: u64 = 48;
/// Bytes of the crate per cell, row, or column with a copy of a style.
const STYLE_COPY_OVERHEAD: u64 = 480;
/// Bytes of the crate per expanded cell format.
const STYLE_OVERHEAD: u64 = 1280;
const LONG_TEXT: &str = "workbook has a text longer than Excel allows";

/// The value of the attribute `key` (exact name, raw value), if any.
fn exact_attr(start: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    start
        .attributes()
        .with_checks(false)
        .flatten()
        .find(|item| item.key.into_inner() == key)
        .map(|item| String::from_utf8_lossy(&item.value).into_owned())
}

/// The size of an event in bytes, as a `u32`.
fn span(from: u64, to: u64) -> u32 {
    u32::try_from(to.saturating_sub(from)).unwrap_or(u32::MAX)
}

/// The bytes of each `<si>` of the shared strings. Refuses an item above
/// 32 767 characters, and a table whose items alone are above the limit. A
/// parse error ends the scan (the crate fails on it as well).
fn shared_string_sizes(xml: &[u8]) -> Result<Vec<u32>, &'static str> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut sizes: Vec<u32> = Vec::new();
    let mut depth = 0usize;
    let mut begin = 0u64;
    let mut in_text = false;
    let mut chars = 0usize;
    loop {
        let before = reader.buffer_position();
        let Ok(event) = reader.read_event_into(&mut buf) else {
            return Ok(sizes);
        };
        let after = reader.buffer_position();
        match &event {
            Event::Start(start) => {
                if depth == 1 && is_name(start, b"si") {
                    begin = before;
                    chars = 0;
                } else if depth >= 2 && is_name(start, b"t") {
                    in_text = true;
                }
                depth += 1;
            }
            Event::Empty(start) if depth == 1 && is_name(start, b"si") => {
                sizes.push(span(before, after));
            }
            Event::End(end) => {
                depth = depth.saturating_sub(1);
                if depth == 1 && end.local_name().as_ref() == b"si" {
                    sizes.push(span(begin, after));
                }
                if end.local_name().as_ref() == b"t" {
                    in_text = false;
                }
            }
            Event::Text(text) if in_text => {
                chars += text.decode().map_or(text.len(), |s| s.chars().count());
            }
            Event::CData(data) if in_text => chars += data.len(),
            Event::GeneralRef(_) if in_text => chars += 1,
            Event::Eof => return Ok(sizes),
            _ => {}
        }
        if chars > MAX_SHARED_CHARS {
            return Err(LONG_TEXT);
        }
        if u64::try_from(sizes.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(SHARED_ITEM_OVERHEAD)
            > MAX_MEMORY
        {
            return Err(TOO_LARGE);
        }
        buf.clear();
    }
}

/// What the styles cost in the crate.
struct StyleCost {
    /// The expanded size of each cell format (a copy per cell with `s`).
    formats: Vec<u64>,
    /// The bytes of the fonts, fills, borders, and number formats, once.
    shared: u64,
}

/// Which list of the styles an element belongs to.
#[derive(Clone, Copy, PartialEq)]
enum Section {
    None,
    NumFmts,
    Fonts,
    Fills,
    Borders,
    CellXfs,
}

/// The id of `key` in `start`, 0 if missing or not a number.
fn style_id(start: &BytesStart<'_>, key: &[u8]) -> usize {
    exact_attr(start, key)
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// The lists of the styles with the bytes of each item.
#[derive(Default)]
struct StyleLists {
    section: Option<Section>,
    /// Start of the font, fill, or border that is open.
    begin: u64,
    formats: HashMap<usize, u64>,
    fonts: Vec<u64>,
    fills: Vec<u64>,
    borders: Vec<u64>,
    xfs: Vec<[usize; 4]>,
}

impl StyleLists {
    fn list(&mut self) -> Option<&mut Vec<u64>> {
        match self.section? {
            Section::Fonts => Some(&mut self.fonts),
            Section::Fills => Some(&mut self.fills),
            Section::Borders => Some(&mut self.borders),
            _ => None,
        }
    }

    /// An element at `depth` (the number of open elements); `size` is its
    /// start tag in bytes, `before` its offset.
    fn element(
        &mut self,
        depth: usize,
        start: &BytesStart<'_>,
        (before, size, empty): (u64, u64, bool),
    ) -> Result<(), &'static str> {
        let name = start.local_name();
        let name = name.as_ref();
        match (depth, self.section) {
            (1, _) => {
                self.section = Some(match name {
                    b"numFmts" => Section::NumFmts,
                    b"fonts" => Section::Fonts,
                    b"fills" => Section::Fills,
                    b"borders" => Section::Borders,
                    b"cellXfs" => Section::CellXfs,
                    _ => Section::None,
                });
            }
            (2, Some(Section::NumFmts)) if name == b"numFmt" => {
                let code = exact_attr(start, b"formatCode").unwrap_or_default();
                if code.chars().count() > MAX_FORMAT_CHARS {
                    return Err(LONG_TEXT);
                }
                self.formats.insert(style_id(start, b"numFmtId"), size);
            }
            (2, Some(Section::CellXfs)) if name == b"xf" => {
                self.xfs.push([
                    style_id(start, b"numFmtId"),
                    style_id(start, b"fontId"),
                    style_id(start, b"fillId"),
                    style_id(start, b"borderId"),
                ]);
            }
            (2, Some(Section::Fonts | Section::Fills | Section::Borders)) => {
                self.begin = before;
                if empty && let Some(list) = self.list() {
                    list.push(size);
                }
            }
            (3, Some(Section::Fonts)) if name == b"name" => {
                let value = exact_attr(start, b"val").unwrap_or_default();
                if value.chars().count() > MAX_FONT_NAME {
                    return Err(LONG_TEXT);
                }
            }
            _ => {}
        }
        if empty && depth == 1 {
            self.section = None;
        }
        Ok(())
    }

    /// An end tag; `depth` is the number of elements that stay open.
    fn close(&mut self, depth: usize, after: u64) {
        let size = after.saturating_sub(self.begin);
        if depth == 2 {
            if let Some(list) = self.list() {
                list.push(size);
            }
        } else if depth == 1 {
            self.section = None;
        }
    }

    fn too_long(&self) -> bool {
        [
            self.fonts.len(),
            self.fills.len(),
            self.borders.len(),
            self.xfs.len(),
            self.formats.len(),
        ]
        .iter()
        .any(|len| *len > MAX_STYLE_ITEMS)
    }

    fn finish(self) -> StyleCost {
        let at = |list: &[u64], index: usize| list.get(index).copied().unwrap_or(0);
        let shared = [&self.fonts, &self.fills, &self.borders]
            .iter()
            .flat_map(|list| list.iter())
            .sum::<u64>()
            + self.formats.values().sum::<u64>();
        let formats = self
            .xfs
            .iter()
            .map(|[format, font, fill, border]| {
                STYLE_OVERHEAD
                    + self.formats.get(format).copied().unwrap_or(0)
                    + at(&self.fonts, *font)
                    + at(&self.fills, *fill)
                    + at(&self.borders, *border)
            })
            .collect();
        StyleCost { formats, shared }
    }
}

/// Estimate the styles. Refuses a font name above 31 characters and a number
/// format above 255 characters. A parse error ends the scan.
fn style_cost(xml: &[u8]) -> Result<StyleCost, &'static str> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut lists = StyleLists::default();
    let mut depth = 0usize;
    loop {
        let before = reader.buffer_position();
        let Ok(event) = reader.read_event_into(&mut buf) else {
            break;
        };
        let after = reader.buffer_position();
        match &event {
            Event::Start(start) | Event::Empty(start) => {
                let empty = matches!(event, Event::Empty(_));
                lists.element(depth, start, (before, after.saturating_sub(before), empty))?;
                if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                lists.close(depth, after);
            }
            Event::Eof => break,
            _ => {}
        }
        if lists.too_long() {
            return Err(TOO_LARGE);
        }
        buf.clear();
    }
    Ok(lists.finish())
}

/// Add the copies that the crate makes for the cells, rows, and columns of
/// one sheet to `total`. A parse error ends the scan.
fn sheet_cost(
    xml: &[u8],
    strings: &[u32],
    styles: &StyleCost,
    total: &mut u64,
) -> Result<(), &'static str> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut shared_cell = false;
    let mut in_value = false;
    let add = |bytes: u64, total: &mut u64| -> Result<(), &'static str> {
        *total = total.saturating_add(bytes);
        if *total > MAX_MEMORY {
            return Err(TOO_LARGE);
        }
        Ok(())
    };
    let style_of = |start: &BytesStart<'_>, key: &[u8]| -> Option<u64> {
        let index = exact_attr(start, key)?.trim().parse::<usize>().ok()?;
        Some(styles.formats.get(index).copied().unwrap_or(0))
    };
    loop {
        let Ok(event) = reader.read_event_into(&mut buf) else {
            return Ok(());
        };
        match &event {
            Event::Start(start) | Event::Empty(start) => {
                let opens = matches!(event, Event::Start(_));
                let key: &[u8] = match start.name().as_ref() {
                    b"c" | b"row" => b"s",
                    b"col" => b"style",
                    b"v" => {
                        in_value = opens;
                        buf.clear();
                        continue;
                    }
                    _ => {
                        buf.clear();
                        continue;
                    }
                };
                if let Some(bytes) = style_of(start, key) {
                    add(STYLE_COPY_OVERHEAD + bytes, total)?;
                }
                if start.name().as_ref() == b"c" {
                    shared_cell = opens && exact_attr(start, b"t").as_deref() == Some("s");
                }
            }
            Event::End(end) => {
                in_value = false;
                if end.name().as_ref() == b"c" {
                    shared_cell = false;
                }
            }
            Event::Text(text) if in_value && shared_cell => {
                let index = text
                    .decode()
                    .ok()
                    .and_then(|text| text.trim().parse::<usize>().ok());
                if let Some(bytes) = index.and_then(|index| strings.get(index)) {
                    add(STRING_COPY_OVERHEAD + u64::from(*bytes), total)?;
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
        buf.clear();
    }
}

fn check(package: &[u8]) -> Result<Vec<u8>, &'static str> {
    let (mut pkg, names) = Package::open(package)?;
    // Every `.rels` part: its strict form, and the pivot relationship types.
    for (index, name) in names.iter().enumerate() {
        if !name.ends_with('/')
            && Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("rels"))
        {
            let list = pkg.rels_of(index)?;
            if list.iter().any(|rel| pivot_type(&rel.kind)) {
                return Err(PIVOT);
            }
        }
    }
    if let Some(index) = pkg.find(CONTENT_TYPES)? {
        let xml = pkg.read(index)?;
        if has_pivot_content_type(&xml)? {
            return Err(PIVOT);
        }
    }
    // The crate reads the full subtree of a sheet for each `<sheet>`.
    if let Some(index) = pkg.find(WORKBOOK)? {
        let xml = pkg.read(index)?;
        let mut ids = HashSet::new();
        for id in sheet_rids(&xml)? {
            if !ids.insert(id) {
                return Err(SHEET_IDS);
            }
        }
    }
    // The relationship graph, from the workbook.
    let rels_index = pkg.find(WORKBOOK_RELS)?.ok_or(UNREADABLE)?;
    let list = pkg.rels_of(rels_index)?;
    let mut on_path = HashSet::new();
    for rel in list.iter() {
        if rel.external {
            pkg.touch()?;
            pkg.charge(pkg.sizes.len())?;
        } else {
            let target = rel.target.strip_prefix("/xl/").unwrap_or(&rel.target);
            pkg.visit("xl", target, 1, &mut on_path)?;
        }
    }
    pkg.estimate()?;
    pkg.canonical()
}

/// Check the input package before `umya-spreadsheet` reads it and return it
/// in a canonical form: part names that the crate and this check resolve to
/// the same entry, and each `.rels` part written again with only the values
/// that were checked. The check covers pivot parts (also under other names),
/// the number of parts, their real sizes, and the relationship graph (no
/// cycle, depth, count, lookup work). Error texts name the reason only,
/// never part contents.
pub fn check_package(package: &[u8]) -> anyhow::Result<Vec<u8>> {
    check(package).map_err(|reason| anyhow!(reason))
}

/// The normalized part names of the sheets that the workbook lists.
/// Empty if the workbook or its relationships are missing. `None` if they
/// cannot be parsed.
fn listed_sheets(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Option<HashSet<String>> {
    let mut workbook = None;
    let mut rels = None;
    for index in 0..archive.len() {
        let Ok(file) = archive.by_index_raw(index) else {
            continue;
        };
        match fold(file.name()).as_str() {
            "xl/workbook.xml" => workbook = workbook.or(Some(index)),
            "xl/_rels/workbook.xml.rels" => rels = rels.or(Some(index)),
            _ => {}
        }
    }
    let mut read = |index: Option<usize>| -> Option<Vec<u8>> {
        read_limited(archive.by_index(index?).ok()?).ok()
    };
    let (Some(workbook), Some(rels)) = (read(workbook), read(rels)) else {
        return Some(HashSet::new());
    };
    let listed = || -> Result<HashSet<String>, Failure> {
        let targets = rel_map(&rels)?;
        let mut parts = HashSet::new();
        for (_, id) in sheet_refs(&workbook)? {
            if let Some(target) = targets.get(&id) {
                let target = target.strip_prefix("/xl/").unwrap_or(target);
                parts.insert(fold(&join_paths("xl", target)));
            }
        }
        Ok(parts)
    };
    listed().ok()
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
    parts: SheetParts,
}

/// Cell formulas and embedded objects of a worksheet.
#[derive(Default)]
struct SheetParts {
    formulas: bool,
    ole: bool,
}

/// Inspect a package: unsupported layouts and unprocessed parts. Part names
/// are compared by their normalized form. The checks for data loss run on the
/// input package, the warnings on the package that `umya-spreadsheet` wrote
/// (canonical part names). Error texts never contain part contents.
pub fn inspect(package: &[u8]) -> anyhow::Result<Inspection> {
    inspect_with_headers(package, &[])
}

/// Like `inspect`. `headers` are the configured header texts: a table column
/// with another name can hold a real value and gets the `tables` warning.
/// With no headers, each table column is such a name.
pub fn inspect_with_headers(package: &[u8], headers: &[String]) -> anyhow::Result<Inspection> {
    let bad = || anyhow!("cannot inspect the workbook package");
    let mut archive = ZipArchive::new(Cursor::new(package)).map_err(|_| bad())?;
    let mut comments = false;
    let mut charts = false;
    let mut drawings = false;
    let mut tables = false;
    let mut formats = false;
    let mut embedded = false;
    let mut flags = SheetFlags::default();
    // A workbook that cannot be parsed hides its sheets: unsupported.
    let mut unsupported = false;
    let listed = listed_sheets(&mut archive).unwrap_or_else(|| {
        unsupported = true;
        HashSet::new()
    });
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|_| bad())?;
        let name = fold(file.name());
        if name.starts_with("xl/threadedcomments/")
            || (name.starts_with("xl/comments") && is_xml(&name))
        {
            comments = true;
        }
        if name.starts_with("xl/charts/") {
            charts = true;
        }
        if name.starts_with("xl/embeddings/") {
            embedded = true;
        }
        if name.starts_with("xl/drawings/") && is_xml(&name) {
            let xml = read_limited(&mut file).map_err(|_| bad())?;
            drawings |= drawing_has_text(&xml).map_err(|_| bad())?;
        } else if name.starts_with("xl/tables/") && is_xml(&name) {
            let xml = read_limited(&mut file).map_err(|_| bad())?;
            tables |= table_has_totals(&xml, headers).map_err(|_| bad())?;
        } else if name == STYLES {
            let xml = read_limited(&mut file).map_err(|_| bad())?;
            formats |= styles_have_literal_text(&xml).map_err(|_| bad())?;
        }
        if name == SHARED_STRINGS.to_ascii_lowercase() {
            let xml = read_limited(&mut file).map_err(|_| bad())?;
            if root_has_prefix(&xml).map_err(|_| bad())? {
                unsupported = true;
            }
        } else if (name.starts_with("xl/worksheets/") && is_xml(&name)) || listed.contains(&name) {
            let xml = read_limited(&mut file).map_err(|_| bad())?;
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
        (flags.parts.formulas, "formulas"),
        (drawings, "drawings"),
        (tables, "tables"),
        (formats, "number formats"),
        (embedded || flags.parts.ole, "embedded objects"),
    ] {
        if present {
            kinds.push(kind);
        }
    }
    Ok(Inspection { unsupported, kinds })
}

/// True if `name` is a default shape name: one or more letter words and a
/// number ("Picture 3", "Text Box 12", "Grafik 1").
fn is_default_shape_name(name: &str) -> bool {
    let Some((words, number)) = name.trim().rsplit_once(' ') else {
        return false;
    };
    !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
        && !words.is_empty()
        && words
            .split(' ')
            .all(|word| !word.is_empty() && word.chars().all(char::is_alphabetic))
}

/// True if a drawing part has text in an `a:t` element, or a shape or picture
/// (`cNvPr`) has alt text (`descr`, `title`) or a name that is not a default.
fn drawing_has_text(xml: &[u8]) -> Result<bool, Failure> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut in_text = false;
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) => {
                in_text = start.local_name().as_ref() == b"t";
                if cnvpr_has_text(&start)? {
                    return Ok(true);
                }
            }
            Event::Empty(start) => {
                in_text = false;
                if cnvpr_has_text(&start)? {
                    return Ok(true);
                }
            }
            Event::End(_) => in_text = false,
            Event::Text(text) if in_text => {
                if text.decode().map_or(true, |t| !t.trim().is_empty()) {
                    return Ok(true);
                }
            }
            Event::CData(_) if in_text => return Ok(true),
            Event::Eof => return Ok(false),
            _ => {}
        }
        buf.clear();
    }
}

/// True if `start` is a `cNvPr` with alt text or a non-default name.
fn cnvpr_has_text(start: &BytesStart<'_>) -> Result<bool, Failure> {
    if start.local_name().as_ref() != b"cNvPr" {
        return Ok(false);
    }
    for key in [&b"descr"[..], b"title"] {
        if attr(start, key)?.is_some_and(|value| !value.trim().is_empty()) {
            return Ok(true);
        }
    }
    Ok(attr(start, b"name")?
        .is_some_and(|name| !name.trim().is_empty() && !is_default_shape_name(&name)))
}

/// True if a table part has a total label, a total or calculated formula, or
/// a column name that is not one of the configured `headers`.
fn table_has_totals(xml: &[u8], headers: &[String]) -> Result<bool, Failure> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) | Event::Empty(start) => {
                let local = start.local_name();
                let local = local.as_ref();
                if local == b"totalsRowFormula" || local == b"calculatedColumnFormula" {
                    return Ok(true);
                }
                if local == b"tableColumn"
                    && attr(&start, b"totalsRowLabel")?.is_some_and(|label| !label.is_empty())
                {
                    return Ok(true);
                }
                if local == b"tableColumn"
                    && attr(&start, b"name")?
                        .is_some_and(|name| !headers.contains(&name.trim().to_lowercase()))
                {
                    return Ok(true);
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buf.clear();
    }
}

/// True if a number format of the styles part has literal text: quoted text
/// a backslash-escaped character, or a `[$` block (it can hold any text, for
/// example `[$Name]0`; each `[$` is flagged).
fn styles_have_literal_text(xml: &[u8]) -> Result<bool, Failure> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) | Event::Empty(start) => {
                if start.local_name().as_ref() == b"numFmt"
                    && attr(&start, b"formatCode")?
                        .is_some_and(|code| code.contains(['"', '\\']) || code.contains("[$"))
                {
                    return Ok(true);
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buf.clear();
    }
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
                } else if local == b"f" {
                    flags.parts.formulas = true;
                } else if local == b"oleObject" {
                    flags.parts.ole = true;
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
                } else if local == b"f" {
                    flags.parts.formulas = true;
                } else if local == b"oleObject" {
                    flags.parts.ole = true;
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
    fn drawing_alt_text_and_custom_names_are_detected() {
        let drawing = |props: &str| {
            format!(
                r#"<xdr:wsDr xmlns:xdr="urn:x"><xdr:pic><xdr:nvPicPr><xdr:cNvPr id="2" {props}/></xdr:nvPicPr></xdr:pic></xdr:wsDr>"#
            )
        };
        let kinds = |props: &str| {
            let xml = drawing(props);
            inspect(&plain_package(&[("xl/drawings/drawing1.xml", &xml)]))
                .unwrap()
                .kinds
        };
        for props in [
            r#"name="Picture 1" descr="Invented alt text""#,
            r#"name="Picture 1" title="Invented title""#,
            r#"name="Quenzel-Foto""#,
            r#"name="Picture""#,
            r#"name="Picture 1a""#,
        ] {
            assert_eq!(kinds(props), ["drawings"], "{props}");
        }
        for props in [
            r#"name="Picture 1""#,
            "name=\"Text Box 12\" descr=\"\" title=\" \"",
            r#"name="Grafik 3""#,
            r#"name="Diagramm 7""#,
            r#"hidden="0""#,
        ] {
            assert!(kinds(props).is_empty(), "{props}");
        }
    }

    #[test]
    fn table_columns_must_be_configured_headers() {
        let table = r#"<table xmlns="urn:x"><tableColumns count="2"><tableColumn id="1" name="Name"/><tableColumn id="2" name="E-Mail"/></tableColumns></table>"#;
        let package = plain_package(&[("xl/tables/table1.xml", table)]);
        let headers = ["name".to_owned(), "e-mail".to_owned()];
        let found = inspect_with_headers(&package, &headers).unwrap();
        assert_eq!(found, Inspection::default());
        let headers = ["name".to_owned()];
        assert_eq!(
            inspect_with_headers(&package, &headers).unwrap().kinds,
            ["tables"]
        );
        assert_eq!(inspect(&package).unwrap().kinds, ["tables"]);
    }

    #[test]
    fn currency_block_number_format_is_detected() {
        let styles = r#"<styleSheet xmlns="urn:x"><numFmts count="1"><numFmt numFmtId="164" formatCode="[$Name]0"/></numFmts></styleSheet>"#;
        let found = inspect(&plain_package(&[("xl/styles.xml", styles)])).unwrap();
        assert_eq!(found.kinds, ["number formats"]);
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

    const PLAIN_SHEET: &str = r#"<worksheet xmlns="urn:m"><sheetData/></worksheet>"#;

    #[test]
    fn only_known_elements_and_their_text_are_copied() {
        let ext = format!(
            r#"<ext uri="{SPARK_URI}" xmlns:x14="urn:x14"><!-- Max Mustermann --><foo>Erika Musterfrau</foo><![CDATA[secret]]><?pi Hans?><x14:sparklineGroups xmlns:xm="urn:xm"><x14:sparklineGroup><!-- Max Mustermann --><foo><x14:sparklines/></foo><x14:colorSeries rgb="FF000000"/><x14:sparklines><x14:sparkline><![CDATA[secret]]><xm:f>Data!A1:C1</xm:f><xm:sqref>D1</xm:sqref><bar>Erika Musterfrau</bar></x14:sparkline></x14:sparklines></x14:sparklineGroup><x14:sparklineGroup/></x14:sparklineGroups><xm:f>stray</xm:f></ext>"#
        );
        let input = book_package(&[("A", "worksheets/sheet1.xml", &spark_sheet(&ext))]);
        let output = book_package(&[("A", "worksheets/sheet1.xml", PLAIN_SHEET)]);
        let out = scrub(&input, &output).unwrap();
        let sheet = part(&out, "xl/worksheets/sheet1.xml");
        for dropped in [
            "Max", "Erika", "secret", "Hans", "foo", "bar", "stray", "CDATA", "<?pi", "<!--",
        ] {
            assert!(!sheet.contains(dropped), "{dropped}");
        }
        assert!(!sheet.contains("<x14:sparklines/>"));
        assert!(sheet.contains(r#"<x14:colorSeries rgb="FF000000"/>"#));
        assert_eq!(sheet.matches("<x14:sparkline>").count(), 1);
        assert!(sheet.contains("<xm:f>Data!A1:C1</xm:f><xm:sqref>D1</xm:sqref>"));
    }

    #[test]
    fn byte_order_mark_does_not_shift_the_copy() {
        let bom_sheet = format!("\u{feff}{}", spark_sheet(&spark_ext("")));
        let input = book_package(&[("A", "worksheets/sheet1.xml", &bom_sheet)]);
        let bom_plain = format!("\u{feff}{PLAIN_SHEET}");
        for plain in [PLAIN_SHEET, bom_plain.as_str()] {
            let output = book_package(&[("A", "worksheets/sheet1.xml", plain)]);
            let out = scrub(&input, &output).unwrap();
            let sheet = part(&out, "xl/worksheets/sheet1.xml");
            assert_eq!(sheet.matches("<x14:sparkline>").count(), 1);
            assert!(sheet.contains(&format!(r#"<ext uri="{SPARK_URI}" xmlns:x14="urn:x14">"#)));
            assert!(sheet.contains("<xm:f>Data!A1:C1</xm:f><xm:sqref>D1</xm:sqref>"));
            assert!(sheet.ends_with("</ext></extLst></worksheet>"));
        }
    }

    #[test]
    fn targets_are_normalized() {
        assert_eq!(
            resolve_target("../xl/worksheets/sheet1.xml").unwrap(),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(
            resolve_target("./worksheets//sheet1.xml").unwrap(),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(
            resolve_target("/xl/worksheets/sheet1.xml").unwrap(),
            "xl/worksheets/sheet1.xml"
        );
        assert!(resolve_target("../../x.xml").is_err());

        let mut parts = book(&[("A", "worksheets/sheet1.xml", &spark_sheet(&spark_ext("")))]);
        parts[1].1 = parts[1]
            .1
            .replace("worksheets/sheet1.xml", "../xl/worksheets/sheet1.xml");
        let refs: Vec<(&str, &str)> = parts
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_str()))
            .collect();
        let input = plain_package(&refs);
        let output = book_package(&[("A", "worksheets/sheet1.xml", PLAIN_SHEET)]);
        let out = scrub(&input, &output).unwrap();
        assert!(part(&out, "xl/worksheets/sheet1.xml").contains("<x14:sparkline>"));
    }

    #[test]
    fn prefixes_come_only_from_root_and_ext_list() {
        let ext = spark_ext("").replace(r#" xmlns:x14="urn:x14""#, "");
        let sheet = format!(
            r#"<worksheet xmlns="urn:m" xmlns:x14="urn:x14"><sheetData xmlns:x14="urn:WRONG"/><extLst>{ext}</extLst></worksheet>"#
        );
        let input = book_package(&[("A", "worksheets/sheet1.xml", &sheet)]);
        let output = book_package(&[("A", "worksheets/sheet1.xml", PLAIN_SHEET)]);
        let out = scrub(&input, &output).unwrap();
        let sheet = part(&out, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains(r#"xmlns:x14="urn:x14""#));
        assert!(!sheet.contains("WRONG"));

        let only_wrong = format!(
            r#"<worksheet xmlns="urn:m"><sheetData xmlns:x14="urn:x14"/><extLst>{ext}</extLst></worksheet>"#
        );
        let input = book_package(&[("A", "worksheets/sheet1.xml", &only_wrong)]);
        assert!(scrub(&input, &output).is_err());
    }

    #[test]
    fn workbook_faults_are_ignored_without_sparklines() {
        let workbook = r#"<workbook xmlns:r="urn:r"><sheets><sheet name="A" sheetId="1" r:id="rId9"/></sheets></workbook>"#;
        let input = plain_package(&[
            (WORKBOOK, workbook),
            (WORKBOOK_RELS, "<Relationships/>"),
            ("xl/worksheets/sheet1.xml", PLAIN_SHEET),
        ]);
        let output = book_package(&[("A", "worksheets/sheet1.xml", PLAIN_SHEET)]);
        let out = scrub(&input, &output).unwrap();
        assert_eq!(names(&out), names(&output));
        assert_eq!(part(&out, "xl/worksheets/sheet1.xml"), PLAIN_SHEET);

        // With sparklines, the same faults are errors.
        let input = plain_package(&[
            (WORKBOOK, workbook),
            (WORKBOOK_RELS, "<Relationships/>"),
            ("xl/worksheets/sheet1.xml", &spark_sheet(&spark_ext(""))),
        ]);
        assert!(scrub(&input, &output).is_err());
    }

    #[test]
    fn listed_sheet_missing_from_the_input_is_an_error() {
        let mut parts = book(&[
            ("A", "worksheets/sheet1.xml", PLAIN_SHEET),
            ("B", "worksheets/sheet2.xml", &spark_sheet(&spark_ext(""))),
        ]);
        parts.remove(2);
        let refs: Vec<(&str, &str)> = parts
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_str()))
            .collect();
        let input = plain_package(&refs);
        let output = book_package(&[
            ("A", "worksheets/sheet1.xml", PLAIN_SHEET),
            ("B", "worksheets/sheet2.xml", PLAIN_SHEET),
        ]);
        assert!(scrub(&input, &output).is_err());
    }

    #[test]
    fn prefixed_main_namespace_gets_prefixed_elements() {
        let input = book_package(&[("A", "worksheets/sheet1.xml", &spark_sheet(&spark_ext("")))]);
        let prefixed = r#"<x:worksheet xmlns:x="urn:m"><x:sheetData/></x:worksheet>"#;
        let output = book_package(&[("A", "worksheets/sheet1.xml", prefixed)]);
        let out = scrub(&input, &output).unwrap();
        let sheet = part(&out, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains("<x:sheetData/><x:extLst><x:ext uri="));
        assert!(sheet.ends_with("</x:ext></x:extLst></x:worksheet>"));
        assert!(!sheet.contains("<ext"));
        assert!(!sheet.contains("<extLst"));
        assert_eq!(sheet.matches("<x14:sparkline>").count(), 1);
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

    // ---- Package check (spec 0017) ----

    fn rels(items: &[(&str, &str)]) -> String {
        use std::fmt::Write as _;
        let mut text = String::from(r#"<Relationships xmlns="urn:r">"#);
        for (index, (kind, target)) in items.iter().enumerate() {
            write!(
                text,
                r#"<Relationship Id="rId{}" Type="http://x/relationships/{kind}" Target="{target}"/>"#,
                index + 1
            )
            .unwrap();
        }
        text + "</Relationships>"
    }

    /// A package with a workbook relationship part for `items` and `more` parts.
    fn graph(items: &[(&str, &str)], more: &[(String, String)]) -> Vec<u8> {
        let workbook_rels = rels(items);
        let mut parts: Vec<(&str, &str)> = vec![
            (CONTENT_TYPES, OUT_TYPES),
            (WORKBOOK, "<workbook/>"),
            (WORKBOOK_RELS, &workbook_rels),
        ];
        parts.extend(more.iter().map(|(n, b)| (n.as_str(), b.as_str())));
        plain_package(&parts)
    }

    fn refused(package: &[u8]) -> String {
        check_package(package).unwrap_err().to_string()
    }

    #[test]
    fn ordinary_package_passes_the_check() {
        let sheet_rels = rels(&[("drawing", "../drawings/d1.xml"), ("hyperlink", "mailto:x")]);
        let drawing_rels = rels(&[("chart", "../charts/c1.xml")]);
        let more = [
            ("xl/worksheets/sheet1.xml".to_owned(), "<w/>".to_owned()),
            ("xl/worksheets/_rels/sheet1.xml.rels".to_owned(), sheet_rels),
            ("xl/drawings/d1.xml".to_owned(), "<d/>".to_owned()),
            ("xl/drawings/_rels/d1.xml.rels".to_owned(), drawing_rels),
            ("xl/charts/c1.xml".to_owned(), "<c/>".to_owned()),
        ];
        let package = graph(&[("worksheet", "worksheets/sheet1.xml")], &more);
        check_package(&package).unwrap();
    }

    #[test]
    fn pivot_names_are_found_after_normalization() {
        for name in [
            "xl/pivotCache/a.xml",
            "xl/PivotCache/a.xml",
            "xl\\pivotCache\\a.xml",
            "XL/PIVOTTABLES/a.xml",
        ] {
            let package = graph(&[], &[(name.to_owned(), "<x/>".to_owned())]);
            assert_eq!(refused(&package), PIVOT, "{name}");
            assert!(has_pivot_entries(&package), "{name}");
        }
        assert!(!has_pivot_entries(&graph(&[], &[])));
        assert!(!has_pivot_entries(b"not a zip"));
    }

    #[test]
    fn pivot_relationship_and_content_types_are_found() {
        for kind in ["pivotTable", "pivotCacheDefinition", "pivotCacheRecords"] {
            let package = graph(&[(kind, "data/p.xml")], &[]);
            assert_eq!(refused(&package), PIVOT, "{kind}");
        }
        let types = r#"<Types xmlns="urn:t"><Override PartName="/xl/p.xml" ContentType="application/vnd.x.pivotCacheRecords+xml"/></Types>"#;
        let package = plain_package(&[
            (CONTENT_TYPES, types),
            (WORKBOOK_RELS, &rels(&[("worksheet", "w.xml")])),
        ]);
        assert_eq!(refused(&package), PIVOT);
    }

    #[test]
    fn cycles_are_found_with_the_lookup_of_the_crate() {
        let own = rels(&[("worksheet", "sheet1.xml")]);
        let more = [
            ("xl/worksheets/sheet1.xml".to_owned(), "<w/>".to_owned()),
            ("xl/worksheets/_rels/sheet1.xml.rels".to_owned(), own),
        ];
        let package = graph(&[("worksheet", "worksheets/sheet1.xml")], &more);
        assert_eq!(refused(&package), CYCLE);

        // Other case and other folder spelling: the same part for the crate.
        let own = rels(&[("worksheet", "../Worksheets/./SHEET1.xml")]);
        let more = [
            ("xl/worksheets/sheet1.xml".to_owned(), "<w/>".to_owned()),
            ("xl/worksheets/_rels/sheet1.xml.rels".to_owned(), own),
        ];
        let package = graph(&[("worksheet", "worksheets/sheet1.xml")], &more);
        assert_eq!(refused(&package), CYCLE);
    }

    /// Parts p0..p{count-1}; each has two relationships to the next one.
    fn fan_out(count: usize) -> Vec<(String, String)> {
        let mut parts = Vec::new();
        for index in 0..count {
            parts.push((format!("xl/p{index}.xml"), "<x/>".to_owned()));
            let next = format!("p{}.xml", index + 1);
            parts.push((
                format!("xl/_rels/p{index}.xml.rels"),
                rels(&[("worksheet", &next), ("worksheet", &next)]),
            ));
        }
        parts
    }

    #[test]
    fn fan_out_is_limited_by_depth_and_by_count() {
        let start = std::time::Instant::now();
        let package = graph(&[("worksheet", "p0.xml")], &fan_out(40));
        assert_eq!(refused(&package), TOO_DEEP);
        let package = graph(&[("worksheet", "p0.xml")], &fan_out(14));
        assert_eq!(refused(&package), TOO_LARGE);
        let package = graph(&[("worksheet", "p0.xml")], &fan_out(8));
        check_package(&package).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn too_many_entries_and_relationships_are_refused() {
        let more: Vec<(String, String)> = (0..MAX_ENTRIES)
            .map(|index| (format!("xl/m{index}.bin"), String::new()))
            .collect();
        assert_eq!(refused(&graph(&[], &more)), TOO_LARGE);

        let many: Vec<(&str, &str)> = (0..=MAX_RELATIONSHIPS)
            .map(|_| ("image", "a.png"))
            .collect();
        assert_eq!(refused(&graph(&many, &[])), TOO_LARGE);
    }

    #[test]
    fn each_reference_to_a_part_counts_its_size() {
        let big = ("xl/big.bin".to_owned(), " ".repeat(1024 * 1024));
        let many: Vec<(&str, &str)> = (0..600).map(|_| ("image", "big.bin")).collect();
        let package = graph(&many, std::slice::from_ref(&big));
        assert!(package.len() < 100_000, "the test package is small");
        assert_eq!(refused(&package), TOO_LARGE);
        let few: Vec<(&str, &str)> = (0..3).map(|_| ("image", "big.bin")).collect();
        check_package(&graph(&few, &[big])).unwrap();
    }

    #[test]
    fn errors_name_the_reason_only() {
        assert_eq!(refused(b"not a zip"), UNREADABLE);
        // No workbook relationships: the crate cannot read it either.
        assert_eq!(refused(&plain_package(&[(WORKBOOK, "<w/>")])), UNREADABLE);
    }

    #[test]
    fn inspect_compares_normalized_names() {
        let sheet = "<worksheet><headerFooter><oddHeader>&amp;LInvented</oddHeader></headerFooter></worksheet>";
        let multi = r#"<worksheet><sheetData><row><c t="inlineStr"><is><r><t>A</t></r><r><t>B</t></r></is></c></row></sheetData></worksheet>"#;
        for name in ["xl/Worksheets/sheet1.xml", "xl\\worksheets\\sheet1.xml"] {
            let found = inspect(&plain_package(&[(name, sheet)])).unwrap();
            assert_eq!(found.kinds, ["headers/footers"], "{name}");
            let found = inspect(&plain_package(&[(name, multi)])).unwrap();
            assert!(found.unsupported, "{name}");
        }
        let prefixed = r#"<x:sst xmlns:x="urn:x"><x:si><x:t>A</x:t></x:si></x:sst>"#;
        let found = inspect(&plain_package(&[("xl/SharedStrings.xml", prefixed)])).unwrap();
        assert!(found.unsupported);
        let found = inspect(&plain_package(&[("XL/Comments1.xml", "<c/>")])).unwrap();
        assert_eq!(found.kinds, ["comments"]);
    }

    #[test]
    fn inspect_finds_sheets_through_the_workbook_relationships() {
        let workbook = r#"<workbook xmlns:r="urn:r"><sheets><sheet name="A" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
        let sheet = "<worksheet><headerFooter><oddFooter>&amp;LInvented</oddFooter></headerFooter></worksheet>";
        let package = plain_package(&[
            (WORKBOOK, workbook),
            (WORKBOOK_RELS, &rels(&[("worksheet", "data/s.xml")])),
            ("xl/data/s.xml", sheet),
        ]);
        let found = inspect(&package).unwrap();
        assert_eq!(found.kinds, ["headers/footers"]);
    }

    #[test]
    fn many_external_references_are_refused() {
        let references = r#"<externalReference r:id="rId1"/>"#.repeat(MAX_RELATIONSHIPS + 1);
        let workbook = format!(
            r#"<workbook xmlns="urn:m" xmlns:r="urn:r"><sheets/><externalReferences>{references}</externalReferences></workbook>"#
        );
        let input = plain_package(&[(WORKBOOK, &workbook), (WORKBOOK_RELS, IN_RELS)]);
        assert!(scrub(&input, &plain_output(OUT_WORKBOOK)).is_err());
    }

    // ---- Canonical package (review of 2026-11-03) ----

    fn external(id: &str) -> String {
        format!(
            r#"<Relationship Id="{id}" Type="http://x/hyperlink" Target="http://e.test/?a=1&amp;b=2" TargetMode="External" foo="bar"/>"#
        )
    }

    fn rels_with(body: &str) -> String {
        format!(r#"<Relationships xmlns="urn:r">{body}</Relationships>"#)
    }

    #[test]
    fn rels_parts_are_written_again_in_a_canonical_form() {
        let sheet_rels = rels_with(&format!(
            r#"{}<Relationship xmlns:q="urn:q" q:x="1" Id="rId2" Type="http://x/drawing" Target="../d.xml" TargetMode="Internal"></Relationship>"#,
            external("rId1")
        ));
        let more = [
            ("xl/worksheets/sheet1.xml".to_owned(), "<w/>".to_owned()),
            ("xl/worksheets/_rels/sheet1.xml.rels".to_owned(), sheet_rels),
            ("xl/d.xml".to_owned(), "<d/>".to_owned()),
        ];
        let package = graph(&[("worksheet", "worksheets/sheet1.xml")], &more);
        let canonical = check_package(&package).unwrap();
        let text = part(&canonical, "xl/worksheets/_rels/sheet1.xml.rels");
        assert!(
            text.contains(
                r#"xmlns="http://schemas.openxmlformats.org/package/2006/relationships""#
            )
        );
        assert!(text.contains(
            r#"<Relationship Id="rId1" Type="http://x/hyperlink" Target="http://e.test/?a=1&amp;b=2" TargetMode="External"/>"#
        ));
        assert!(
            text.contains(r#"<Relationship Id="rId2" Type="http://x/drawing" Target="../d.xml"/>"#)
        );
        assert!(!text.contains("foo") && !text.contains("q:x"));
        // The other parts stay as they were.
        assert_eq!(part(&canonical, "xl/d.xml"), "<d/>");
        assert_eq!(names(&canonical), names(&package));
    }

    #[test]
    fn part_names_must_be_canonical() {
        for name in ["xl\\media\\a.bin", "/xl/media/a.bin"] {
            let package = graph(&[], &[(name.to_owned(), String::new())]);
            assert_eq!(refused(&package), NAMES, "{name}");
        }
        let more = [
            ("xl/media/a.bin".to_owned(), String::new()),
            ("xl/Media/A.bin".to_owned(), String::new()),
        ];
        assert_eq!(refused(&graph(&[], &more)), NAMES);
    }

    #[test]
    fn strict_rels_reader_refuses_unusual_parts() {
        let bodies = [
            r#"<Relationship Id="a" Type="t" Target="x&amp;y"/>"#,
            r#"<Relationship Id="a" Type="t" Target="x" TargetMode="&#69;xternal"/>"#,
            r#"<Relationship Id="a" Type="t" Target="x" Target="y"/>"#,
            r#"<Relationship Id="a" Type="t"/>"#,
            r#"<Relationship Id="a" Type="t" Target="x"/><Relationship Id="a" Type="t" Target="y"/>"#,
            r#"<Relationship xmlns:q="urn:q" Id="a" Type="t" q:Target="x"/>"#,
            r#"<Other Id="a" Type="t" Target="x"/>"#,
            r#"<p:Relationship xmlns:p="urn:q" Id="a" Type="t" Target="x"/>"#,
            r#"<Relationship Id="a" Type="t" Target="x"><Relationship Id="b" Type="t" Target="y"/></Relationship>"#,
        ];
        for body in bodies {
            let package = plain_package(&[
                (CONTENT_TYPES, OUT_TYPES),
                (WORKBOOK, "<workbook/>"),
                (WORKBOOK_RELS, &rels_with(body)),
            ]);
            assert_eq!(refused(&package), UNREADABLE, "{body}");
        }
    }

    #[test]
    fn external_relationships_count_a_scan_of_all_names() {
        // 9 900 External relationships and 9 900 other parts: 98 million
        // comparisons for the crate.
        let body: String = (0..9_900)
            .map(|index| external(&format!("r{index}")))
            .collect();
        let mut more: Vec<(String, String)> = (0..9_000)
            .map(|index| (format!("xl/media/m{index}.bin"), String::new()))
            .collect();
        more.push(("xl/worksheets/sheet1.xml".to_owned(), "<w/>".to_owned()));
        more.push((
            "xl/worksheets/_rels/sheet1.xml.rels".to_owned(),
            rels_with(&body),
        ));
        let package = graph(&[("worksheet", "worksheets/sheet1.xml")], &more);
        assert_eq!(refused(&package), TOO_LARGE);
        // A few hundred are fine.
        let body: String = (0..300)
            .map(|index| external(&format!("r{index}")))
            .collect();
        more.pop();
        more.push((
            "xl/worksheets/_rels/sheet1.xml.rels".to_owned(),
            rels_with(&body),
        ));
        check_package(&graph(&[("worksheet", "worksheets/sheet1.xml")], &more)).unwrap();
    }

    #[test]
    fn drawing_with_2500_images_passes_the_check() {
        let images: Vec<(&str, String)> = (0..2_500)
            .map(|index| ("image", format!("../media/i{index}.png")))
            .collect();
        let mut more = vec![
            ("xl/worksheets/sheet1.xml".to_owned(), "<w/>".to_owned()),
            (
                "xl/worksheets/_rels/sheet1.xml.rels".to_owned(),
                rels(&[("drawing", "../drawings/d.xml")]),
            ),
            ("xl/drawings/d.xml".to_owned(), "<d/>".to_owned()),
            (
                "xl/drawings/_rels/d.xml.rels".to_owned(),
                rels(
                    &images
                        .iter()
                        .map(|(k, t)| (*k, t.as_str()))
                        .collect::<Vec<_>>(),
                ),
            ),
        ];
        more.extend((0..2_500).map(|index| (format!("xl/media/i{index}.png"), String::new())));
        let package = graph(&[("worksheet", "worksheets/sheet1.xml")], &more);
        check_package(&package).unwrap();
    }

    #[test]
    fn sheets_must_have_distinct_relationship_ids() {
        let workbook = r#"<workbook xmlns:r="urn:r"><sheets><sheet name="A" r:id="rId1"/><sheet name="B" r:id="rId2"/><sheet name="C" d3p1:id="rId1"/></sheets></workbook>"#;
        let package = plain_package(&[
            (CONTENT_TYPES, OUT_TYPES),
            (WORKBOOK, workbook),
            (WORKBOOK_RELS, &rels(&[])),
        ]);
        assert_eq!(refused(&package), SHEET_IDS);
        let workbook = workbook.replace("d3p1:id", "d3p1:other");
        let package = plain_package(&[
            (CONTENT_TYPES, OUT_TYPES),
            (WORKBOOK, &workbook),
            (WORKBOOK_RELS, &rels(&[])),
        ]);
        check_package(&package).unwrap();
    }

    #[test]
    fn many_references_to_one_link_part_are_read_once() {
        let references = r#"<externalReference r:id="rId4"/>"#.repeat(MAX_RELATIONSHIPS);
        let workbook = format!(
            r#"<workbook xmlns="urn:m" xmlns:r="urn:r"><sheets/><externalReferences>{references}</externalReferences></workbook>"#
        );
        let rels = r#"<Relationships xmlns="urn:r"><Relationship Id="rId4" Type="t" Target="externalLinks/externalLink1.xml"/></Relationships>"#;
        let link = format!(
            r#"<externalLink xmlns="urn:m"><externalBook/>{}</externalLink>"#,
            " ".repeat(8 * 1024 * 1024)
        );
        let input = plain_package(&[
            (WORKBOOK, &workbook),
            (WORKBOOK_RELS, rels),
            ("xl/externalLinks/externalLink1.xml", &link),
        ]);
        let start = std::time::Instant::now();
        let out = scrub(&input, &plain_output(OUT_WORKBOOK)).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
        assert!(part(&out, WORKBOOK).contains("<externalReferences>"));
    }
}
