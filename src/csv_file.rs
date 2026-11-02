//! Pseudonymize one CSV file.

use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, anyhow};
use csv::{ByteRecord, ReaderBuilder, Terminator, WriterBuilder};
use encoding_rs::WINDOWS_1252;

use crate::cells::{
    CellFns, ColRole, Collector, Columns, HEADER_SEARCH_ROWS, HeaderPick, Kind, collect_columns,
    failing_rule, header_count, pick_header, role_of, transform_cell, transform_text_cell,
    write_output,
};
use crate::config::Rule;
use crate::known_names::KnownNames;
use crate::pseudonym::Pseudonymizer;

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The largest CSV file that pseudofy reads (the file is held in memory).
const MAX_BYTES: u64 = 256 * 1024 * 1024;

/// Read `input`. A file above `MAX_BYTES` is refused before and while it is
/// read, so a file that grows during the read cannot pass.
fn read_input(input: &Path, rel: &str) -> anyhow::Result<Vec<u8>> {
    let unreadable = || format!("cannot read {rel}");
    let too_large = || anyhow!("{rel}: file is larger than 256 MiB");
    let file = fs::File::open(input).with_context(unreadable)?;
    let size = file.metadata().with_context(unreadable)?.len();
    if size > MAX_BYTES {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(unreadable)?;
    if u64::try_from(bytes.len()).is_ok_and(|len| len > MAX_BYTES) {
        return Err(too_large());
    }
    Ok(bytes)
}

/// Result of the transformation of one file.
struct Transformed {
    bytes: Vec<u8>,
    replaced: usize,
    /// Known names replaced in text cells.
    free_text: usize,
}

/// Counts for one processed CSV file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CsvStats {
    /// Replaced values (including the free-text replacements).
    pub replaced: usize,
    /// Known names replaced in text cells.
    pub free_text: usize,
}

/// Pass 1: read the name columns of `input` and add the known names to
/// `known`. Nothing is written. On an error `known` stays unchanged.
pub fn collect(
    input: &Path,
    rel: &str,
    rules: &[&Rule],
    cells: &CellFns,
    known: &mut KnownNames,
) -> anyhow::Result<()> {
    let bytes = read_input(input, rel)?;
    let columns = collect_columns(rules);
    let mut collector = Collector::default();
    transform_encoded(
        &bytes,
        &columns,
        rules,
        cells,
        &mut |_, _| String::new(),
        rel,
        Some(&mut collector),
    )?;
    collector.apply(known);
    Ok(())
}

/// Read `input`, replace names and emails, write the copy to `output`.
/// `out_root` is the output directory, `rel` the relative path for messages.
/// `rules` are the rules that match the file.
pub fn process(
    input: &Path,
    out_root: &Path,
    output: &Path,
    rel: &str,
    rules: &[&Rule],
    pseudonymizer: &mut Pseudonymizer,
    cells: &CellFns,
) -> anyhow::Result<CsvStats> {
    let bytes = read_input(input, rel)?;
    let columns = collect_columns(rules);
    let mut replace = |kind: Kind, value: &str| match kind {
        Kind::First => pseudonymizer.first_name(value),
        Kind::Last => pseudonymizer.last_name(value),
        Kind::Email => pseudonymizer.email(value),
    };
    let result = transform_encoded(&bytes, &columns, rules, cells, &mut replace, rel, None)?;
    write_output(out_root, output, &result.bytes, rel)?;
    Ok(CsvStats {
        replaced: result.replaced,
        free_text: result.free_text,
    })
}

/// Transform a UTF-8 or Windows-1252 file. The copy has the encoding of the input.
fn transform_encoded(
    input: &[u8],
    columns: &Columns,
    rules: &[&Rule],
    cells: &CellFns,
    replace: &mut dyn FnMut(Kind, &str) -> String,
    rel: &str,
    collect: Option<&mut Collector>,
) -> anyhow::Result<Transformed> {
    let damaged = || anyhow!("{rel}: unsupported or damaged encoding");
    if input.starts_with(b"\xFF\xFE") || input.starts_with(b"\xFE\xFF") || input.contains(&0) {
        return Err(damaged());
    }
    let rest = input.strip_prefix(BOM);
    if std::str::from_utf8(rest.unwrap_or(input)).is_ok() {
        return transform(input, columns, rules, cells, replace, rel, collect);
    }
    // Invalid UTF-8 after a UTF-8 BOM, or next to valid UTF-8 sequences, is a
    // damaged or mixed file. Decoding it as Windows-1252 would hide real names.
    if rest.is_some() || has_utf8_multibyte(input) {
        return Err(damaged());
    }
    let (decoded, _) = WINDOWS_1252.decode_without_bom_handling(input);
    let mut result = transform(
        decoded.as_bytes(),
        columns,
        rules,
        cells,
        replace,
        rel,
        collect,
    )?;
    let text = std::str::from_utf8(&result.bytes)
        .map_err(|_| anyhow!("{rel}: cannot encode the copy in Windows-1252"))?;
    let (encoded, _, unmappable) = WINDOWS_1252.encode(text);
    if unmappable {
        return Err(anyhow!("{rel}: cannot encode the copy in Windows-1252"));
    }
    result.bytes = encoded.into_owned();
    Ok(result)
}

/// `true` if `bytes` contain a valid UTF-8 sequence of two to four bytes.
fn has_utf8_multibyte(bytes: &[u8]) -> bool {
    bytes.iter().enumerate().any(|(index, &byte)| {
        let len = match byte {
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => return false,
        };
        bytes
            .get(index..index + len)
            .is_some_and(|window| std::str::from_utf8(window).is_ok())
    })
}

fn detect_delimiter(header_line: &[u8]) -> u8 {
    let mut counts = [0_usize; 3];
    let mut quoted = false;
    for &byte in header_line {
        match byte {
            b'"' => quoted = !quoted,
            b',' if !quoted => counts[0] += 1,
            b';' if !quoted => counts[1] += 1,
            b'\t' if !quoted => counts[2] += 1,
            _ => {}
        }
    }
    let max = counts.iter().copied().max().unwrap_or(0);
    let winners = counts.iter().filter(|&&count| count == max).count();
    if max == 0 || winners != 1 {
        return b',';
    }
    match counts.iter().position(|&count| count == max) {
        Some(1) => b';',
        Some(2) => b'\t',
        _ => b',',
    }
}

/// The first record without its terminator, and the terminator style
/// (`\r\n`, lone `\r`, or `\n` as default). Line breaks inside quotes do not end it.
fn first_record(body: &[u8]) -> (&[u8], &'static [u8]) {
    let mut quoted = false;
    for (index, &byte) in body.iter().enumerate() {
        match byte {
            b'"' => quoted = !quoted,
            b'\n' if !quoted => return (body.get(..index).unwrap_or(body), b"\n"),
            b'\r' if !quoted => {
                let head = body.get(..index).unwrap_or(body);
                return if body.get(index + 1) == Some(&b'\n') {
                    (head, b"\r\n")
                } else {
                    (head, b"\r")
                };
            }
            _ => {}
        }
    }
    (body, b"\n")
}

/// Header counts of the first 20 non-empty records under `delimiter`.
fn header_counts(body: &[u8], delimiter: u8, columns: &Columns) -> Vec<usize> {
    let mut reader = ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(body);
    let mut record = ByteRecord::new();
    let mut counts = Vec::new();
    while counts.len() < HEADER_SEARCH_ROWS
        && matches!(reader.read_byte_record(&mut record), Ok(true))
    {
        let Some(texts) = record_texts(&record) else {
            continue;
        };
        if texts.iter().all(String::is_empty) {
            continue;
        }
        counts.push(header_count(&texts, columns));
    }
    counts
}

/// Delimiter and header row of a file.
struct Layout {
    delimiter: u8,
    pick: HeaderPick,
    /// Non-empty records among the first 20.
    searched: usize,
}

/// The delimiter is the one of `,`, `;` and tab whose best header row has the
/// most header texts. Ties go to the guess from the first record, then to the
/// order `,` `;` tab.
fn find_layout(body: &[u8], columns: &Columns) -> Layout {
    let fallback = detect_delimiter(first_record(body).0);
    let mut candidates = vec![fallback];
    candidates.extend(b",;\t".iter().copied().filter(|&d| d != fallback));
    let mut best: Option<(usize, Layout)> = None;
    for delimiter in candidates {
        let counts = header_counts(body, delimiter, columns);
        let score = counts.iter().copied().max().unwrap_or(0);
        if best.as_ref().is_none_or(|(top, _)| score > *top) {
            let layout = Layout {
                delimiter,
                pick: pick_header(&counts),
                searched: counts.len(),
            };
            best = Some((score, layout));
        }
    }
    best.map_or(
        Layout {
            delimiter: fallback,
            pick: HeaderPick::None,
            searched: 0,
        },
        |(_, layout)| layout,
    )
}

/// State of the quote scanner (the states of csv-core).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scan {
    FieldStart,
    Plain,
    Quoted,
    /// A quote inside a quoted field: closes it, or starts `""`.
    QuoteInQuoted,
}

fn scan_step(state: Scan, byte: u8, delimiter: u8) -> Scan {
    let boundary = byte == delimiter || byte == b'\n' || byte == b'\r';
    match state {
        Scan::Quoted if byte == b'"' => Scan::QuoteInQuoted,
        Scan::Quoted => Scan::Quoted,
        Scan::QuoteInQuoted | Scan::FieldStart if byte == b'"' => Scan::Quoted,
        _ if boundary => Scan::FieldStart,
        _ => Scan::Plain,
    }
}

/// `true` if a quoted field is still open at the end of `body`. A quote opens
/// a field only at the start of a field; `""` in a quoted field is a quote.
fn has_unclosed_quote(body: &[u8], delimiter: u8) -> bool {
    body.iter().fold(Scan::FieldStart, |state, &byte| {
        scan_step(state, byte, delimiter)
    }) == Scan::Quoted
}

/// Split one record (without its terminator) into the raw bytes of its fields.
fn raw_fields(record: &[u8], delimiter: u8) -> Vec<&[u8]> {
    let mut fields = Vec::new();
    let mut state = Scan::FieldStart;
    let mut begin = 0;
    for (index, &byte) in record.iter().enumerate() {
        let quoted = matches!(state, Scan::Quoted);
        if byte == delimiter && !quoted {
            fields.push(record.get(begin..index).unwrap_or(&[]));
            begin = index + 1;
        }
        state = scan_step(state, byte, delimiter);
    }
    fields.push(record.get(begin..).unwrap_or(&[]));
    fields
}

/// Trimmed lowercase text of each field. `None` if a field is not valid UTF-8.
fn record_texts(record: &ByteRecord) -> Option<Vec<String>> {
    record
        .iter()
        .map(|field| {
            std::str::from_utf8(field)
                .ok()
                .map(|text| text.trim().to_lowercase())
        })
        .collect()
}

/// The place of a record relative to the header row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Title,
    Header,
    Data,
}

/// Walks the records and tells where each is relative to the header row
/// that `find_layout` picked.
struct HeaderSearch {
    /// Index of the header among the non-empty records.
    header_at: Option<usize>,
    /// Non-empty records seen.
    seen: usize,
    passed: bool,
    /// Normalized texts of the header record, once reached.
    header: Option<Vec<String>>,
    /// Column roles from the header.
    roles: Vec<Option<ColRole>>,
}

impl HeaderSearch {
    fn new(pick: HeaderPick) -> Self {
        Self {
            header_at: match pick {
                HeaderPick::At(index) => Some(index),
                _ => None,
            },
            seen: 0,
            passed: false,
            header: None,
            roles: Vec::new(),
        }
    }

    fn place(&mut self, record: &ByteRecord, columns: &Columns) -> Place {
        if self.passed {
            return Place::Data;
        }
        let Some(texts) = record_texts(record) else {
            return Place::Title;
        };
        if texts.iter().all(String::is_empty) {
            return Place::Title;
        }
        let index = self.seen;
        self.seen += 1;
        if self.header_at != Some(index) {
            return Place::Title;
        }
        self.roles = texts.iter().map(|h| role_of(h, columns)).collect();
        self.header = Some(texts);
        self.passed = true;
        Place::Header
    }
}

/// Bytes of a changed record. Unchanged fields keep their original bytes
/// (also their quotes). A changed field is quoted if the original was quoted
/// or if the CSV writer needs quotes. `None` on a write error.
fn render_changed(
    raw: &[u8],
    original: &ByteRecord,
    out: &ByteRecord,
    delimiter: u8,
    builder: &WriterBuilder,
    line_end: &[u8],
) -> Option<Vec<u8>> {
    let raws = raw_fields(raw, delimiter);
    if raws.len() != original.len() {
        let mut bytes = Vec::new();
        render(builder, out, &mut bytes)?;
        return Some(bytes);
    }
    let mut bytes = Vec::with_capacity(raw.len() + line_end.len());
    for (index, (field, new)) in raws.iter().zip(out.iter()).enumerate() {
        if index > 0 {
            bytes.push(delimiter);
        }
        if original.get(index) == Some(new) {
            bytes.extend_from_slice(field);
        } else if field.first() == Some(&b'"') {
            bytes.push(b'"');
            for &byte in new {
                if byte == b'"' {
                    bytes.push(b'"');
                }
                bytes.push(byte);
            }
            bytes.push(b'"');
        } else if new.is_empty() {
            // Nothing to write; a lone empty field needs no quotes here.
        } else {
            let mut one = ByteRecord::new();
            one.push_field(new);
            let mut rendered = Vec::new();
            render(builder, &one, &mut rendered)?;
            rendered.truncate(rendered.len().saturating_sub(line_end.len()));
            bytes.extend_from_slice(&rendered);
        }
    }
    bytes.extend_from_slice(line_end);
    Some(bytes)
}

/// The record bytes without the terminator.
fn without_terminator(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

/// Parse the file and transform each row. Rows above the header and the header
/// get email and known-name replacement; the data rows get the column roles.
/// With `collect`, no copy is built: the name cells of each data row go to the
/// collector instead.
fn transform(
    input: &[u8],
    columns: &Columns,
    rules: &[&Rule],
    cells: &CellFns,
    replace: &mut dyn FnMut(Kind, &str) -> String,
    rel: &str,
    mut collect: Option<&mut Collector>,
) -> anyhow::Result<Transformed> {
    let (bom, body) = match input.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, input),
    };
    let (_, line_end) = first_record(body);
    let layout = find_layout(body, columns);
    let delimiter = layout.delimiter;
    if has_unclosed_quote(body, delimiter) {
        return Err(anyhow!("{rel}: unclosed quote"));
    }
    if layout.pick == HeaderPick::Ambiguous {
        return Err(anyhow!("{rel}: header row is ambiguous"));
    }
    let ends_with_newline = body.ends_with(b"\n") || body.ends_with(b"\r");

    let mut reader = ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(body);
    let mut writer_builder = WriterBuilder::new();
    writer_builder
        .delimiter(delimiter)
        .terminator(terminator_of(line_end))
        .flexible(true);
    let mut out_bytes: Vec<u8> = Vec::new();

    let mut search = HeaderSearch::new(layout.pick);
    let mut replaced = 0_usize;
    let mut free_text = 0_usize;
    let mut record = ByteRecord::new();
    let mut any_record = false;
    // End byte of the previous record and the physical line number there.
    let mut prev_end = 0_usize;
    let mut running_line: u64 = 1;
    loop {
        let more = reader
            .read_byte_record(&mut record)
            .map_err(|_| anyhow!("{rel}: cannot parse the record after line {running_line}"))?;
        if !more {
            break;
        }
        any_record = true;
        let start = record
            .position()
            .and_then(|pos| usize::try_from(pos.byte()).ok())
            .unwrap_or(prev_end);
        // The reader puts the skipped empty lines at the start of the record.
        let start = skip_breaks(body, start);
        let end = usize::try_from(reader.position().byte())
            .map_or(body.len(), |pos| after_terminator(body, pos));
        let gap = body.get(prev_end..start).unwrap_or(&[]);
        let blanks = count_breaks(gap);
        let line = running_line + blanks;
        running_line = line + count_breaks(body.get(start..end).unwrap_or(&[]));
        let verbatim = body.get(prev_end..end).unwrap_or(&[]);
        prev_end = end;

        let data = search.place(&record, columns) == Place::Data;
        if let Some(collector) = collect.as_deref_mut() {
            if data {
                collect_record(&record, &search.roles, collector)
                    .ok_or_else(|| anyhow!("{rel}: line {line} is not valid UTF-8"))?;
            }
            continue;
        }
        let mut count = |kind: Kind, value: &str| {
            replaced += 1;
            replace(kind, value)
        };
        let out = if data {
            transform_record(&record, &search.roles, cells, &mut count, &mut free_text)
        } else {
            transform_text_record(&record, columns, cells, &mut count, &mut free_text)
        }
        .ok_or_else(|| anyhow!("{rel}: line {line} is not valid UTF-8"))?;
        if !data && out == record {
            // A title row or header without a change keeps its bytes.
            out_bytes.extend_from_slice(verbatim);
            continue;
        }
        out_bytes.extend(line_end.repeat(usize::try_from(blanks).unwrap_or(0)));
        let raw = without_terminator(body.get(start..end).unwrap_or(&[]));
        let bytes = render_changed(raw, &record, &out, delimiter, &writer_builder, line_end)
            .ok_or_else(|| anyhow!("{rel}: cannot write line {line}"))?;
        out_bytes.extend_from_slice(&bytes);
    }
    // Fail closed: a rule that finds none of its headers in the header row.
    // A file without a non-empty record (`searched == 0`) has nothing to leak.
    let failing = failing_rule(rules, search.header.as_deref(), columns);
    if let Some(glob) = failing.filter(|_| layout.searched > 0) {
        return Err(anyhow!(
            "{rel}: no header of rule \"{glob}\" in the first {HEADER_SEARCH_ROWS} rows"
        ));
    }
    if ends_with_newline && any_record {
        let rest = body.get(prev_end..).unwrap_or(&[]);
        out_bytes.extend(line_end.repeat(usize::try_from(count_breaks(rest)).unwrap_or(0)));
    }

    Ok(Transformed {
        bytes: assemble(&out_bytes, bom, ends_with_newline, line_end),
        replaced,
        free_text,
    })
}

/// Add the BOM and drop a final terminator that the input did not have.
fn assemble(out_bytes: &[u8], bom: bool, ends_with_newline: bool, line_end: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(out_bytes.len() + BOM.len());
    if bom {
        bytes.extend_from_slice(BOM);
    }
    bytes.extend_from_slice(out_bytes);
    if !ends_with_newline && bytes.ends_with(line_end) {
        bytes.truncate(bytes.len() - line_end.len());
    }
    bytes
}

/// Transform each field of a title row or header record. `None` if a field is
/// not valid UTF-8.
fn transform_text_record(
    record: &ByteRecord,
    columns: &Columns,
    cells: &CellFns,
    replace: &mut dyn FnMut(Kind, &str) -> String,
    free: &mut usize,
) -> Option<ByteRecord> {
    let mut out = ByteRecord::new();
    for field in record {
        let cell = std::str::from_utf8(field).ok()?;
        let value = transform_text_cell(cell, columns, cells, replace, free);
        out.push_field(value.as_bytes());
    }
    Some(out)
}

fn terminator_of(line_end: &[u8]) -> Terminator {
    match line_end {
        b"\r\n" => Terminator::CRLF,
        b"\r" => Terminator::Any(b'\r'),
        _ => Terminator::Any(b'\n'),
    }
}

/// Position of the first byte at or after `pos` that is not a line break.
fn skip_breaks(body: &[u8], pos: usize) -> usize {
    pos + body
        .get(pos..)
        .unwrap_or(&[])
        .iter()
        .take_while(|&&byte| byte == b'\r' || byte == b'\n')
        .count()
}

/// Append `record` as one CSV line to `target`.
fn render(builder: &WriterBuilder, record: &ByteRecord, target: &mut Vec<u8>) -> Option<()> {
    let mut writer = builder.from_writer(Vec::new());
    writer.write_byte_record(record).ok()?;
    target.extend_from_slice(&writer.into_inner().ok()?);
    Some(())
}

/// Number of line breaks (`\n`, `\r\n`, lone `\r`) in `bytes`.
fn count_breaks(bytes: &[u8]) -> u64 {
    let mut count = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' || (byte == b'\r' && bytes.get(index + 1) != Some(&b'\n')) {
            count += 1;
        }
    }
    count
}

/// The CSV reader may stop between the `\r` and the `\n` of a CRLF terminator.
fn after_terminator(body: &[u8], pos: usize) -> usize {
    if pos > 0 && body.get(pos - 1) == Some(&b'\r') && body.get(pos) == Some(&b'\n') {
        pos + 1
    } else {
        pos
    }
}

/// Give the name cells of a record to the collector. `None` if a field is
/// not valid UTF-8.
fn collect_record(
    record: &ByteRecord,
    roles: &[Option<ColRole>],
    collector: &mut Collector,
) -> Option<()> {
    for (index, field) in record.iter().enumerate() {
        let cell = std::str::from_utf8(field).ok()?;
        collector.cell(roles.get(index).copied().flatten(), cell);
    }
    collector.end_row();
    Some(())
}

/// Transform each field of a record. `None` if a field is not valid UTF-8.
fn transform_record(
    record: &ByteRecord,
    roles: &[Option<ColRole>],
    cells: &CellFns,
    replace: &mut dyn FnMut(Kind, &str) -> String,
    free: &mut usize,
) -> Option<ByteRecord> {
    let mut out = ByteRecord::new();
    for (index, field) in record.iter().enumerate() {
        let cell = std::str::from_utf8(field).ok()?;
        let role = roles.get(index).copied().flatten();
        let value = transform_cell(cell, role, cells, replace, free);
        out.push_field(value.as_bytes());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::{EmailFn, NameFn};
    use crate::text;
    use crate::text::{NameRole, Part};
    use regex::Regex;

    fn fake_names(
        value: &str,
        _role: NameRole,
        replace: &mut dyn FnMut(Part, &str) -> String,
    ) -> String {
        value
            .split(' ')
            .map(|token| {
                if token.is_empty() {
                    String::new()
                } else {
                    replace(Part::First, token)
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn fake_emails(value: &str, regex: &Regex, replace: &mut dyn FnMut(&str) -> String) -> String {
        regex
            .replace_all(value, |caps: &regex::Captures| replace(&caps[0]))
            .into_owned()
    }

    fn columns() -> Columns {
        Columns {
            first: vec![("*.csv".into(), "first_name".into())],
            full: vec![("*.csv".into(), "name".into())],
            email: vec![("*.csv".into(), "email".into())],
            ..Columns::default()
        }
    }

    fn fns(regex: &Regex, names: NameFn, emails: EmailFn) -> CellFns<'_> {
        CellFns {
            email_regex: regex,
            names,
            emails,
            matcher: None,
        }
    }

    fn run(input: &[u8], columns: &Columns) -> Transformed {
        let regex = Regex::new(r"[a-z0-9._-]+@[a-z0-9.-]+\.[a-z]+").unwrap();
        transform(
            input,
            columns,
            &[],
            &fns(&regex, fake_names, fake_emails),
            &mut |kind, _value| match kind {
                Kind::First | Kind::Last => "N".to_owned(),
                Kind::Email => "x@example.com".to_owned(),
            },
            "t.csv",
            None,
        )
        .unwrap()
    }

    fn text(out: Transformed) -> String {
        String::from_utf8(out.bytes).unwrap()
    }

    fn run_enc(input: &[u8], columns: &Columns) -> anyhow::Result<Transformed> {
        let regex = text::email_regex().unwrap();
        transform_encoded(
            input,
            columns,
            &[],
            &fns(&regex, text::replace_name, text::replace_emails),
            &mut |kind, _| match kind {
                Kind::First => "Berta".to_owned(),
                Kind::Last => "Muster".to_owned(),
                Kind::Email => "f@x.test".to_owned(),
            },
            "t.csv",
            None,
        )
    }

    #[test]
    fn windows_1252_round_trips_all_bytes() {
        let all: Vec<u8> = (0..=255).collect();
        let (decoded, _) = WINDOWS_1252.decode_without_bom_handling(&all);
        let (encoded, _, unmappable) = WINDOWS_1252.encode(&decoded);
        assert!(!unmappable);
        assert_eq!(&*encoded, all.as_slice());
    }

    fn rejected(input: &[u8]) -> String {
        match run_enc(input, &columns()) {
            Ok(_) => String::from("accepted"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn damaged_or_mixed_encodings_are_rejected() {
        let message = "t.csv: unsupported or damaged encoding";
        // UTF-8 with one invalid byte.
        assert_eq!(rejected(b"name\nJ\xc3\xb6rg M\xc3\xbcller\xff\n"), message);
        // UTF-8 BOM before Windows-1252 text.
        assert_eq!(rejected(b"\xEF\xBB\xBFname\nJ\xf6rg\n"), message);
        // UTF-16 with and without BOM.
        assert_eq!(rejected(b"\xFF\xFEn\0a\0m\0e\0"), message);
        assert_eq!(rejected(b"\xFE\xFF\0n\0a\0m\0e"), message);
        assert_eq!(rejected(b"n\0a\0m\0e\0\n\0"), message);
    }

    #[test]
    fn utf8_multibyte_detection() {
        assert!(has_utf8_multibyte(b"a\xc3\xa4b"));
        assert!(has_utf8_multibyte(b"\xe2\x82\xac"));
        assert!(!has_utf8_multibyte(b"J\xf6rg M\xfcller \x80 \xc3"));
        assert!(!has_utf8_multibyte(b"\xe2\x82"));
    }

    #[test]
    fn windows_1252_file_gets_windows_1252_copy() {
        let input = b"name;note\r\nJ\xf6rg M\xfcller;caf\xe9 \x80\r\n";
        let out = run_enc(input, &columns()).unwrap();
        assert_eq!(out.replaced, 2);
        assert_eq!(out.bytes, b"name;note\r\nBerta Muster;caf\xe9 \x80\r\n");
    }

    #[test]
    fn windows_1252_file_without_values_is_byte_equal() {
        let input = b"id;ort\r\n1;M\xfcnchen \x81\x8d\x8f\x90\x9d\r\n2;Stra\xdfe";
        let out = run_enc(input, &columns()).unwrap();
        assert_eq!(out.replaced, 0);
        assert_eq!(out.bytes, input);
    }

    #[test]
    fn windows_1252_blank_lines_stay() {
        let input = b"id;m\r\n\r\n1;\xf6\r\n";
        let out = run_enc(input, &Columns::default()).unwrap();
        assert_eq!(out.bytes, input);
    }

    #[test]
    fn unencodable_copy_is_reported_without_values() {
        let regex = text::email_regex().unwrap();
        let error = transform_encoded(
            b"name\nJ\xf6rg\n",
            &columns(),
            &[],
            &fns(&regex, text::replace_name, text::replace_emails),
            &mut |_, _| "\u{141}".to_owned(),
            "t.csv",
            None,
        )
        .err()
        .map(|e| e.to_string())
        .unwrap();
        assert_eq!(error, "t.csv: cannot encode the copy in Windows-1252");
    }

    #[test]
    fn utf8_file_is_unchanged_in_behavior() {
        let input = "name\nJ\u{f6}rg\n".as_bytes();
        let out = run_enc(input, &columns()).unwrap();
        assert_eq!(out.bytes, b"name\nMuster\n");
        let bom = b"\xEF\xBB\xBFname\nAb\n";
        let out = run_enc(bom, &columns()).unwrap();
        assert_eq!(out.bytes, b"\xEF\xBB\xBFname\nMuster\n");
    }

    #[test]
    fn delimiter_detection() {
        assert_eq!(detect_delimiter(b"a;b;c"), b';');
        assert_eq!(detect_delimiter(b"a\tb\tc"), b'\t');
        assert_eq!(detect_delimiter(b"a,b;c"), b',');
        assert_eq!(detect_delimiter(b"abc"), b',');
        assert_eq!(detect_delimiter(b"\"a;b;c\",d"), b',');
    }

    fn rule(files: &str, first: &[&str], email: &[&str]) -> Rule {
        Rule {
            files: files.to_string(),
            first_name: first.iter().map(|s| (*s).to_string()).collect(),
            last_name: Vec::new(),
            full_name: Vec::new(),
            email: email.iter().map(|s| (*s).to_string()).collect(),
            sheets: Vec::new(),
        }
    }

    #[test]
    fn keeps_bom_delimiter_and_crlf() {
        let input = b"\xEF\xBB\xBFid;mail\r\n1;a@b.test\r\n2;\r\n";
        let out = run(input, &Columns::default());
        assert_eq!(
            out.bytes,
            b"\xEF\xBB\xBFid;mail\r\n1;x@example.com\r\n2;\r\n"
        );
        assert_eq!(out.replaced, 1);
    }

    #[test]
    fn replaces_names_and_emails() {
        let input = b"Name , note\nab cd,mail a@b.test now\n";
        let out = run(input, &columns());
        assert_eq!(out.replaced, 3);
        assert_eq!(
            String::from_utf8(out.bytes).unwrap(),
            "Name , note\nN N,mail x@example.com now\n"
        );
    }

    #[test]
    fn email_column_replaces_matches_or_whole_cell() {
        let input = b"email,x\n a@b.test ,1\n  not an address ,2\nsee a@b.test ok,3\n";
        let out = run(input, &columns());
        assert_eq!(out.replaced, 5);
        assert_eq!(
            text(out),
            "email,x\n x@example.com ,1\n  x@example.com ,2\nN x@example.com N,3\n"
        );
    }

    #[test]
    fn email_inside_name_column_is_not_split_into_names() {
        let input = b"name,x\nab a@b.test cd,1\nab cd,2\n";
        let out = run(input, &columns());
        assert_eq!(out.replaced, 5);
        assert_eq!(text(out), "name,x\nN x@example.com N,1\nN N,2\n");
    }

    #[test]
    fn keeps_blank_lines_and_final_newline_state() {
        let input = b"id,m\n\n1,a@b.test\n\n\n2,c\n";
        let out = run(input, &Columns::default());
        assert_eq!(text(out), "id,m\n\n1,x@example.com\n\n\n2,c\n");
        let input = b"id,m\r\n1,a\r\n\r\n2,b";
        let out = run(input, &Columns::default());
        assert_eq!(text(out), "id,m\r\n1,a\r\n\r\n2,b");
        let input = b"id,m\n1,a\n\n";
        let out = run(input, &Columns::default());
        assert_eq!(text(out), "id,m\n1,a\n\n");
    }

    #[test]
    fn header_bytes_stay_unchanged() {
        let input = b"\xEF\xBB\xBF\"id\" , m\r\n1,a\r\n";
        let out = run(input, &Columns::default());
        assert_eq!(out.bytes, input);
        let out = run(b"id,m", &Columns::default());
        assert_eq!(out.bytes, b"id,m");
    }

    #[test]
    fn error_names_the_real_line_number() {
        let regex = Regex::new("x").unwrap();
        let input = b"id,m\n\n\n1,\xFF\n";
        let error = transform(
            input,
            &Columns::default(),
            &[],
            &fns(&regex, fake_names, fake_emails),
            &mut |_, _| String::new(),
            "t.csv",
            None,
        )
        .err()
        .map(|e| e.to_string())
        .unwrap();
        assert_eq!(error, "t.csv: line 4 is not valid UTF-8");
    }

    fn run_real(input: &[u8], columns: &Columns) -> String {
        let regex = text::email_regex().unwrap();
        let out = transform(
            input,
            columns,
            &[],
            &fns(&regex, text::replace_name, text::replace_emails),
            &mut |kind, value| match kind {
                Kind::First => "Berta".to_owned(),
                Kind::Last => "Muster".to_owned(),
                Kind::Email => format!("f{}@x.test", value.len()),
            },
            "t.csv",
            None,
        )
        .unwrap();
        String::from_utf8(out.bytes).unwrap()
    }

    #[test]
    fn trailing_dot_address_is_same_in_name_and_email_cells() {
        let input = b"name,email\nzorbel@acme.test.,zorbel@acme.test\n";
        let out = run_real(input, &columns());
        assert_eq!(out, "name,email\nf16@x.test.,f16@x.test\n");
    }

    #[test]
    fn display_name_in_email_column_is_replaced() {
        let input = b"email\n\"\"\"Zorbel Quenzel\"\" <zorbel@acme.test>\"\n";
        let out = run_real(input, &columns());
        assert_eq!(out, "email\n\"\"\"Berta Muster\"\" <f16@x.test>\"\n");
    }

    #[test]
    fn cr_only_line_endings_are_kept() {
        let input = b"name,note\rZorbel Quenzel,a\r\rTamsorin Velgrad,b\r";
        let out = run_real(input, &columns());
        assert_eq!(out, "name,note\rBerta Muster,a\r\rBerta Muster,b\r");
    }

    #[test]
    fn delimiter_comes_from_the_first_record() {
        assert_eq!(first_record(b"\"a\nb\";c\n1;2\n").0, b"\"a\nb\";c");
        let input = "\"Kunden\nNr\";name\n1;Zorbel Quenzel\n";
        let out = run_real(input.as_bytes(), &columns());
        assert_eq!(out, "\"Kunden\nNr\";name\n1;Berta Muster\n");
    }

    #[test]
    fn writes_by_rename_and_refuses_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("out");
        fs::create_dir(&root).unwrap();
        let file = root.join("sub").join("a.csv");
        write_output(&root, &file, b"one", "sub/a.csv").unwrap();
        write_output(&root, &file, b"two", "sub/a.csv").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"two");
        assert!(!root.join("sub").join(".a.csv.pseudofy-tmp").exists());

        #[cfg(unix)]
        {
            let victim = dir.path().join("victim.txt");
            fs::write(&victim, b"keep").unwrap();
            let link = root.join("b.csv");
            std::os::unix::fs::symlink(&victim, &link).unwrap();
            let error = write_output(&root, &link, b"x", "b.csv").unwrap_err();
            assert!(error.to_string().contains("b.csv"));
            assert_eq!(fs::read(&victim).unwrap(), b"keep");

            let target_dir = dir.path().join("elsewhere");
            fs::create_dir(&target_dir).unwrap();
            std::os::unix::fs::symlink(&target_dir, root.join("linked")).unwrap();
            let inner = root.join("linked").join("c.csv");
            assert!(write_output(&root, &inner, b"x", "linked/c.csv").is_err());
            assert!(!target_dir.join("c.csv").exists());
        }
    }

    fn strict(input: &[u8]) -> anyhow::Result<String> {
        let rule = rule("*.csv", &[], &["email"]);
        let rules = [&rule];
        let columns = collect_columns(&rules);
        let regex = text::email_regex().unwrap();
        transform_encoded(
            input,
            &columns,
            &rules,
            &fns(&regex, text::replace_name, text::replace_emails),
            &mut |_, value| format!("f{}@x.test", value.len()),
            "t.csv",
            None,
        )
        .map(|out| String::from_utf8(out.bytes).unwrap())
    }

    #[test]
    fn file_without_header_fails_without_values_in_the_message() {
        let error = strict(b"zorbina.quenzel@acme.test,Zorbina Quenzel\n")
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "t.csv: no header of rule \"*.csv\" in the first 20 rows"
        );
        // A file without a non-empty record has nothing to leak.
        assert!(strict(b"").is_ok());
    }

    #[test]
    fn title_rows_are_searched_past_and_scrubbed() {
        let input =
            b"Kontakte von zorbina.quenzel@acme.test\n\nname,Email\nZ,zorbina.quenzel@acme.test\n";
        let out = strict(input).unwrap();
        assert_eq!(out, "Kontakte von f25@x.test\n\nname,Email\nZ,f25@x.test\n");
    }

    #[test]
    fn delimiter_comes_from_the_header_not_the_title() {
        let input = b"Liste, intern\nname;email\nZ;a@b.test\n";
        let out = strict(input).unwrap();
        assert_eq!(out, "Liste, intern\nname;email\nZ;f8@x.test\n");
    }

    #[test]
    fn email_in_the_header_line_is_replaced_and_header_texts_stay() {
        let input = b"\xEF\xBB\xBFemail,\"a@b.test\"\r\nx@y.test,1\r\n";
        let out = strict(input).unwrap();
        assert_eq!(out, "\u{feff}email,\"f8@x.test\"\r\nf8@x.test,1\r\n");
    }

    #[test]
    fn header_after_the_20th_row_is_not_found() {
        let mut input = String::new();
        for _ in 0..20 {
            input.push_str("titel\n");
        }
        input.push_str("email\n");
        assert!(strict(input.as_bytes()).is_err());
        let ok = input.replacen("titel", "email", 1);
        assert!(strict(ok.as_bytes()).is_ok());
    }

    #[test]
    fn unclosed_quote_fails() {
        let error = strict(b"email,x\na@b.test,\"open\nmore\n")
            .unwrap_err()
            .to_string();
        assert_eq!(error, "t.csv: unclosed quote");
        assert!(has_unclosed_quote(b"a,\"b", b','));
        assert!(!has_unclosed_quote(b"a,\"b\"\"c\",d\n", b','));
        assert!(!has_unclosed_quote(b"a\"b,c\n", b','));
        assert!(!has_unclosed_quote(b"a;\"b,c\"", b';'));
    }

    #[test]
    fn unchanged_title_and_header_bytes_stay() {
        let input = b"\"Titel\" ,x\r\n\r\n\"email\",\"y\"\r\na@b.test,1\r\n";
        let out = strict(input).unwrap();
        assert_eq!(
            out,
            "\"Titel\" ,x\r\n\r\n\"email\",\"y\"\r\nf8@x.test,1\r\n"
        );
    }

    #[test]
    fn escaped_quote_keeps_the_field_open() {
        let open = b"id,name\n\"1\"\"a,Z\nnext,row\n";
        assert!(has_unclosed_quote(open, b','));
        assert_eq!(
            strict(b"email,x\n\"1\"\"a,Z\n2,Y\n")
                .unwrap_err()
                .to_string(),
            "t.csv: unclosed quote"
        );
        assert!(!has_unclosed_quote(b"a,\"x\"\" y,\"\n", b','));
        assert!(!has_unclosed_quote(b"a;\"x\"\" y;\"\n", b';'));
        assert!(strict(b"email,note\na@b.test,\"x\"\" y,\"\n").is_ok());
    }

    #[test]
    fn raw_fields_split_at_unquoted_delimiters_only() {
        let fields = raw_fields(b"\"a,b\",\"x\"\"y\",,z", b',');
        assert_eq!(
            fields,
            [&b"\"a,b\""[..], &b"\"x\"\"y\""[..], &b""[..], &b"z"[..]]
        );
    }

    #[test]
    fn header_with_most_texts_wins_and_ties_fail() {
        let out = strict(b"Felder Name, email\nx,email\nq,a@b.test\n").unwrap_err();
        assert_eq!(out.to_string(), "t.csv: header row is ambiguous");
        let out = strict(b"Felder x, email\nemail,\"y\"\nq,a@b.test\n");
        assert!(out.is_err());
    }

    #[test]
    fn delimiter_with_the_best_header_row_wins() {
        let cols = columns();
        let layout = find_layout(b"Liste: Name, Email\nname;email\nZ;a@b.test\n", &cols);
        assert_eq!(layout.delimiter, b';');
        assert_eq!(layout.pick, HeaderPick::At(1));
        let layout = find_layout(b"name\temail\nZ\ta@b.test\n", &cols);
        assert_eq!(layout.delimiter, b'\t');
    }

    #[test]
    fn changed_record_keeps_the_original_bytes_of_unchanged_fields() {
        let out = strict(b"\"email\",\"x a@b.test\"\n\"a@b.test\",\"keep\"\n").unwrap();
        assert_eq!(out, "\"email\",\"x f8@x.test\"\n\"f8@x.test\",\"keep\"\n");
        // A changed unquoted field is quoted only if it needs quotes.
        let out = strict(b"email,x\na@b.test,1\n").unwrap();
        assert_eq!(out, "email,x\nf8@x.test,1\n");
    }
}
