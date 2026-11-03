//! Pseudonymize one XLSX file.

use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{Context, anyhow};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use umya_spreadsheet::{CellRawValue, Workbook, reader, writer};

use crate::cells::{
    CellFns, ColRole, Collector, Columns, HEADER_SEARCH_ROWS, HeaderPick, Kind, collect_columns,
    failing_rule, header_count, name_kind, pick_header, role_of, transform_cell,
    transform_text_cell, write_output,
};
use crate::config::Rule;
use crate::known_names::KnownNames;
use crate::panic_guard;
use crate::pseudonym::Pseudonymizer;
use crate::text::{self, NameRole};
use crate::xlsx_scrub;

/// Counts for one processed workbook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct XlsxStats {
    /// Replaced values (including the free-text replacements).
    pub replaced: usize,
    /// Known names replaced in text cells.
    pub free_text: usize,
    /// Formula cells (left unchanged).
    pub formulas: usize,
    /// Rich-text cells that were written as plain text.
    pub rich_text: usize,
}

/// A rule with the sheet-name globs of its `sheets` field (`None`: each sheet).
type SheetRule<'a> = (Option<GlobSet>, &'a Rule);

/// The largest XLSX file that pseudofy reads (the file is held in memory).
const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Read `input`. A file above `MAX_BYTES` is refused before and while it is
/// read, so a file that grows during the read cannot pass.
fn read_input(input: &Path, rel: &str) -> anyhow::Result<Vec<u8>> {
    let unreadable = || format!("cannot read {rel}");
    let too_large = || anyhow!("{rel}: file is larger than 512 MiB");
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

/// Read `input` once. Return the workbook and the package bytes it came from.
/// The package check runs before `umya-spreadsheet` reads a byte.
fn open_book(input: &Path, rel: &str) -> anyhow::Result<(Workbook, Vec<u8>)> {
    let bytes = read_input(input, rel)?;
    let unreadable = || anyhow!("{rel}: cannot read the workbook");
    // From here on the canonical package is the only one that is read: the
    // check and `umya-spreadsheet` see the same names and relationships.
    let bytes = xlsx_scrub::check_package(&bytes).map_err(|reason| anyhow!("{rel}: {reason}"))?;
    let found = xlsx_scrub::inspect(&bytes).map_err(|_| unreadable())?;
    if found.unsupported {
        return Err(anyhow!("{rel}: unsupported workbook layout"));
    }
    // The crate can panic on a damaged workbook; its error texts are not used.
    let book =
        panic_guard::catch(|| reader::xlsx::read_reader(Cursor::new(bytes.as_slice()), true))
            .ok_or_else(unreadable)?
            .map_err(|_| unreadable())?;
    Ok((book, bytes))
}

/// Pass 1: read the name columns of `input` and add the known names to
/// `known`. Nothing is written or printed. On an error `known` stays unchanged.
pub fn collect(
    input: &Path,
    rel: &str,
    rules: &[&Rule],
    cells: &CellFns,
    known: &mut KnownNames,
) -> anyhow::Result<()> {
    let (mut book, _) = open_book(input, rel)?;
    let mut collector = Collector::default();
    transform(
        &mut book,
        rules,
        cells,
        &mut |_, _| String::new(),
        Some(&mut collector),
        rel,
    )?;
    collector.apply(known);
    Ok(())
}

/// Read `input`, replace names and emails in each sheet, write the copy to
/// `output`. `out_root` is the output directory, `rel` the relative path for
/// messages. `rules` are the rules that match the file.
pub fn process(
    input: &Path,
    out_root: &Path,
    output: &Path,
    rel: &str,
    rules: &[&Rule],
    pseudonymizer: &mut Pseudonymizer,
    cells: &CellFns,
) -> anyhow::Result<XlsxStats> {
    // The scrub restores parts from the same bytes that were inspected and read.
    let (mut book, original) = open_book(input, rel)?;

    let mut replace = |kind: Kind, value: &str| match kind {
        Kind::First => pseudonymizer.first_name(value),
        Kind::Last => pseudonymizer.last_name(value),
        Kind::Email => pseudonymizer.email(value),
    };
    let result = transform(&mut book, rules, cells, &mut replace, None, rel)?;

    let mut out: Vec<u8> = Vec::new();
    let unwritable = || anyhow!("{rel}: cannot write the workbook");
    panic_guard::catch(|| writer::xlsx::write_writer(&book, &mut out))
        .ok_or_else(unwritable)?
        .map_err(|_| unwritable())?;
    // Defense in depth: no pivot part, whatever its name was in the input.
    if xlsx_scrub::has_pivot_entries(&out) {
        return Err(anyhow!("{rel}: workbook has pivot tables; not supported"));
    }
    let out = xlsx_scrub::scrub(&original, &out).map_err(|_| unwritable())?;
    // The warnings come from the written package, where part names are canonical.
    let headers: Vec<String> = rules
        .iter()
        .flat_map(|rule| {
            [
                &rule.first_name,
                &rule.last_name,
                &rule.full_name,
                &rule.email,
            ]
            .into_iter()
            .flatten()
        })
        .map(|header| header.trim().to_lowercase())
        .collect();
    let written = xlsx_scrub::inspect_with_headers(&out, &headers).map_err(|_| unwritable())?;
    write_output(out_root, output, &out, rel, input)?;
    if !written.kinds.is_empty() {
        eprintln!(
            "warning: {rel}: not pseudonymized: {}",
            written.kinds.join(", ")
        );
    }
    Ok(result.stats)
}

/// The first char of the private-use range that stands for a byte of an
/// escape that is no UTF-8 (`\u{F700}` + byte). The decoded text keeps such an
/// escape, and `percent_encode` writes it again, so it is never encoded twice.
const BYTE_BASE: u32 = 0xF700;

/// Percent-decode `text`, byte by byte. A valid UTF-8 run of decoded bytes
/// becomes text. A byte that is no UTF-8 stays as an escape (a private-use
/// char), and the decode goes on. An invalid escape stays as it is. With
/// `query`, `+` is a space.
fn percent_decode(text: &str, query: bool) -> String {
    fn hex(byte: u8) -> Option<u8> {
        char::from(byte)
            .to_digit(16)
            .and_then(|digit| u8::try_from(digit).ok())
    }
    fn flush(pending: &mut Vec<u8>, out: &mut String) {
        let mut rest: &[u8] = pending;
        while !rest.is_empty() {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    out.push_str(valid);
                    break;
                }
                Err(error) => {
                    let (valid, tail) = rest.split_at(error.valid_up_to());
                    out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                    let bad = error.error_len().unwrap_or(tail.len()).min(tail.len());
                    for &byte in &tail[..bad] {
                        out.push(
                            char::from_u32(BYTE_BASE + u32::from(byte))
                                .unwrap_or(char::REPLACEMENT_CHARACTER),
                        );
                    }
                    rest = &tail[bad..];
                }
            }
        }
        pending.clear();
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut pending: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(&high), Some(&low)) = (bytes.get(i + 1), bytes.get(i + 2))
            && let (Some(high), Some(low)) = (hex(high), hex(low))
        {
            pending.push(high * 16 + low);
            i += 3;
            continue;
        }
        flush(&mut pending, &mut out);
        // Copy one char (the text is valid UTF-8, `i` is on a boundary).
        let Some(c) = text.get(i..).and_then(|rest| rest.chars().next()) else {
            break;
        };
        out.push(if query && c == '+' { ' ' } else { c });
        i += c.len_utf8();
    }
    flush(&mut pending, &mut out);
    out
}

/// Percent-encode `text`. Letters, digits, and the characters that a url
/// keeps in a path, a query, or a mailto address stay as they are. A private-use
/// char of `percent_decode` becomes its escape again. With `query`, a space
/// is `+` and a plus is `%2B`.
fn percent_encode(text: &str, query: bool) -> String {
    const KEEP: &str = "-._~!$'()*,;:@";
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(text.len());
    let push_escape = |out: &mut String, byte: u8| {
        out.push('%');
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 15)]));
    };
    let mut utf8 = [0u8; 4];
    for c in text.chars() {
        let code = u32::from(c);
        if (BYTE_BASE..BYTE_BASE + 256).contains(&code) {
            push_escape(&mut out, u8::try_from(code - BYTE_BASE).unwrap_or(0));
        } else if (c == ' ' && query) || (c == '+' && !query) {
            out.push('+');
        } else if c.is_ascii_alphanumeric() || (c.is_ascii() && KEEP.contains(c)) {
            out.push(c);
        } else {
            for &byte in c.encode_utf8(&mut utf8).as_bytes() {
                push_escape(&mut out, byte);
            }
        }
    }
    out
}

/// A `mailto:` url: cut at the first `?`, decoded. The display name gets the
/// full-name pseudonym, the addresses the email pseudonym; the result is
/// encoded. The text between two addresses is cut at `,` and `;` outside
/// quotes, and each piece is a name of its own.
fn new_mailto(rest: &str, fns: &CellFns, replace: &mut dyn FnMut(Kind, &str) -> String) -> String {
    let decoded = percent_decode(rest.split('?').next().unwrap_or_default(), false);
    let spans = text::email_spans(&decoded, fns.email_regex);
    let names = |run: &str, replace: &mut dyn FnMut(Kind, &str) -> String| {
        let mut out = String::with_capacity(run.len());
        let mut piece = String::new();
        let mut quoted = false;
        let mut flush = |piece: &mut String, out: &mut String| {
            out.push_str(&(fns.names)(piece, NameRole::Full, &mut |part, token| {
                replace(name_kind(part), token)
            }));
            piece.clear();
        };
        for c in run.chars() {
            if c == '"' {
                quoted = !quoted;
            }
            if matches!(c, ',' | ';') && !quoted {
                flush(&mut piece, &mut out);
                out.push(c);
            } else {
                piece.push(c);
            }
        }
        flush(&mut piece, &mut out);
        out
    };
    let mut out = String::with_capacity(decoded.len());
    let mut pos = 0;
    for span in &spans {
        out.push_str(&names(decoded.get(pos..span.start).unwrap_or(""), replace));
        out.push_str(&replace(
            Kind::Email,
            decoded.get(span.clone()).unwrap_or(""),
        ));
        pos = span.end;
    }
    out.push_str(&names(decoded.get(pos..).unwrap_or(""), replace));
    format!("mailto:{}", percent_encode(&out, false))
}

/// Any other url: each segment (between `/ ? & = # ; \`) is decoded and gets
/// the free-text replacement. A changed segment is encoded again; an
/// unchanged one stays byte for byte. In the query (after `?`, before `#`),
/// `+` is a space.
fn new_plain_url(
    url: &str,
    fns: &CellFns,
    replace: &mut dyn FnMut(Kind, &str) -> String,
) -> String {
    let is_delimiter = |c: char| matches!(c, '/' | '?' | '&' | '=' | '#' | ';' | '\\');
    let mut out = String::with_capacity(url.len());
    let mut segment = String::new();
    let mut in_query = false;
    let mut flush = |segment: &mut String, out: &mut String, query: bool| {
        if segment.is_empty() {
            return;
        }
        let decoded = percent_decode(segment, query);
        let mut free = 0;
        let new = transform_cell(&decoded, None, fns, replace, &mut free);
        if new == decoded {
            out.push_str(segment);
        } else {
            out.push_str(&percent_encode(&new, query));
        }
        segment.clear();
    };
    for c in url.chars() {
        if is_delimiter(c) {
            flush(&mut segment, &mut out, in_query);
            out.push(c);
            match c {
                '?' => in_query = true,
                '#' => in_query = false,
                _ => {}
            }
        } else {
            segment.push(c);
        }
    }
    flush(&mut segment, &mut out, in_query);
    out
}

/// The new url of a hyperlink.
fn new_url(url: &str, fns: &CellFns, replace: &mut dyn FnMut(Kind, &str) -> String) -> String {
    const SCHEME: &str = "mailto:";
    if url.len() >= SCHEME.len()
        && url.is_char_boundary(SCHEME.len())
        && url[..SCHEME.len()].eq_ignore_ascii_case(SCHEME)
    {
        new_mailto(&url[SCHEME.len()..], fns, replace)
    } else {
        new_plain_url(url, fns, replace)
    }
}

/// Replace the names and emails in the url of the hyperlink of `cell` and
/// clear its tooltip.
fn scrub_hyperlink(
    cell: &mut umya_spreadsheet::Cell,
    fns: &CellFns,
    replace: &mut dyn FnMut(Kind, &str) -> String,
) {
    let Some(link) = cell.hyperlink() else {
        return;
    };
    // A link to a place in the workbook names a sheet; sheet names stay.
    let url = if link.location() {
        link.url().to_owned()
    } else {
        new_url(link.url(), fns, replace)
    };
    if url != link.url() || !link.tooltip().is_empty() {
        let link = cell.hyperlink_mut();
        link.set_url(url);
        link.set_tooltip("");
    }
}

/// The cached string result of a formula can hold a real value; the formula
/// stays.
fn clear_cached_text(cell: &mut umya_spreadsheet::Cell) {
    if matches!(
        cell.raw_value(),
        CellRawValue::String(_) | CellRawValue::RichText(_) | CellRawValue::Lazy(_)
    ) {
        cell.set_formula_result_blank();
    }
}

/// Result of the transformation of one workbook.
struct Transformed {
    stats: XlsxStats,
}

fn sheet_rules<'a>(rules: &[&'a Rule]) -> anyhow::Result<Vec<SheetRule<'a>>> {
    rules
        .iter()
        .map(|rule| {
            if rule.sheets.is_empty() {
                return Ok((None, *rule));
            }
            let mut builder = GlobSetBuilder::new();
            for pattern in &rule.sheets {
                let glob = GlobBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .with_context(|| format!("invalid sheet glob \"{pattern}\" in the config"))?;
                builder.add(glob);
            }
            let set = builder
                .build()
                .context("invalid sheet globs in the config")?;
            Ok((Some(set), *rule))
        })
        .collect()
}

/// Give the text of a cell to the collector.
fn collect_cell(collector: &mut Collector, cell: &umya_spreadsheet::Cell, role: Option<ColRole>) {
    match cell.raw_value() {
        CellRawValue::String(text) | CellRawValue::Lazy(text) => collector.cell(role, text),
        CellRawValue::RichText(rich) => collector.cell(role, rich.text().as_ref()),
        _ => {}
    }
}

/// Transform each sheet. With `collect`, the workbook stays unchanged: the
/// name cells of each row go to the collector instead.
fn transform(
    book: &mut Workbook,
    rules: &[&Rule],
    fns: &CellFns,
    replace: &mut dyn FnMut(Kind, &str) -> String,
    mut collect: Option<&mut Collector>,
    rel: &str,
) -> anyhow::Result<Transformed> {
    let matchers = sheet_rules(rules)?;
    let mut stats = XlsxStats::default();
    let names: Vec<String> = book
        .sheet_collection()
        .iter()
        .map(|sheet| sheet.name().to_owned())
        .collect();
    if let Some(glob) = unmatched_sheet_rule(&matchers, &names) {
        return Err(anyhow!("{rel}: rule \"{glob}\" matches no sheet"));
    }

    for (position, sheet) in book.sheet_collection_mut().iter_mut().enumerate() {
        let name = sheet.name().to_owned();
        let mut applicable: Vec<&Rule> = Vec::new();
        for (set, rule) in &matchers {
            if set.as_ref().is_none_or(|set| set.is_match(&name)) {
                applicable.push(rule);
            }
        }
        let columns = collect_columns(&applicable);
        let mut cells = sheet.cells_mut();
        cells.sort_by_key(|cell| {
            let at = cell.coordinate();
            (at.row_num(), at.col_num())
        });
        let Some(first_row) = cells
            .iter()
            .find(|cell| !cell.value().trim().is_empty())
            .map(|cell| cell.coordinate().row_num())
        else {
            continue;
        };
        let (header_row, header_texts) =
            sheet_header(&cells, &columns, &applicable, position + 1, rel)?;
        let by_col: HashMap<u32, ColRole> = header_texts
            .iter()
            .filter_map(|(col, text)| role_of(text, &columns).map(|role| (*col, role)))
            .collect();
        // Rows up to the header (all rows without a header) are text rows.
        let is_data = |row: u32| header_row.is_some_and(|header| row > header);

        let mut current_row = first_row;
        for cell in &mut cells {
            let at = cell.coordinate();
            let (row, col) = (at.row_num(), at.col_num());
            if let Some(collector) = collect.as_deref_mut() {
                if row != current_row {
                    collector.end_row();
                    current_row = row;
                }
                if is_data(row) && !cell.is_formula() {
                    collect_cell(collector, cell, by_col.get(&col).copied());
                }
                continue;
            }
            scrub_hyperlink(cell, fns, replace);
            if cell.is_formula() {
                if header_row.is_none_or(|header| row > header) {
                    stats.formulas += 1;
                }
                clear_cached_text(cell);
                continue;
            }
            let Some((old, rich)) = string_value(cell) else {
                continue;
            };
            let mut count = |kind: Kind, value: &str| {
                stats.replaced += 1;
                replace(kind, value)
            };
            let new = if is_data(row) {
                transform_cell(
                    &old,
                    by_col.get(&col).copied(),
                    fns,
                    &mut count,
                    &mut stats.free_text,
                )
            } else {
                transform_text_cell(&old, &columns, fns, &mut count, &mut stats.free_text)
            };
            if new != old {
                cell.set_value_string(new);
                if rich {
                    stats.rich_text += 1;
                }
            }
        }
    }
    if let Some(collector) = collect {
        collector.end_row();
    }
    Ok(Transformed { stats })
}

/// Header search: the row with the most header texts among the first 20
/// non-empty rows. Returns its row number and its texts; rows above it are
/// title rows. Fails if the row is ambiguous or if a rule finds none of its
/// headers. `cells` are sorted by row. `position` is the 1-based sheet position.
fn sheet_header(
    cells: &[&mut umya_spreadsheet::Cell],
    columns: &Columns,
    applicable: &[&Rule],
    position: usize,
    rel: &str,
) -> anyhow::Result<(Option<u32>, HashMap<u32, String>)> {
    let rows: Vec<u32> = non_empty_rows(cells)
        .into_iter()
        .take(HEADER_SEARCH_ROWS)
        .collect();
    let texts: Vec<HashMap<u32, String>> = rows.iter().map(|&row| row_texts(cells, row)).collect();
    let counts: Vec<usize> = texts
        .iter()
        .map(|row| header_count(&row.values().cloned().collect::<Vec<_>>(), columns))
        .collect();
    let (row, header) = match pick_header(&counts) {
        HeaderPick::Ambiguous => {
            return Err(anyhow!("{rel}: sheet {position}: header row is ambiguous"));
        }
        HeaderPick::At(index) => (rows.get(index).copied(), texts.into_iter().nth(index)),
        HeaderPick::None => (None, None),
    };
    let header = header.unwrap_or_default();
    let values: Vec<String> = header.values().cloned().collect();
    if let Some(glob) = failing_rule(applicable, row.map(|_| values.as_slice()), columns) {
        return Err(anyhow!(
            "{rel}: sheet {position}: no header of rule \"{glob}\" in the first {HEADER_SEARCH_ROWS} rows"
        ));
    }
    Ok((row, header))
}

/// Row numbers (ascending) of the rows with a non-blank cell value.
fn non_empty_rows(cells: &[&mut umya_spreadsheet::Cell]) -> Vec<u32> {
    let mut rows: Vec<u32> = cells
        .iter()
        .filter(|cell| !cell.value().trim().is_empty())
        .map(|cell| cell.coordinate().row_num())
        .collect();
    rows.dedup();
    rows
}

/// Texts (trimmed, lowercase) of the cells of one row by column number.
fn row_texts(cells: &[&mut umya_spreadsheet::Cell], row: u32) -> HashMap<u32, String> {
    cells
        .iter()
        .filter(|cell| cell.coordinate().row_num() == row)
        .map(|cell| {
            (
                cell.coordinate().col_num(),
                cell.value().trim().to_lowercase(),
            )
        })
        .collect()
}

/// The text of a string cell and whether it is rich text.
fn string_value(cell: &umya_spreadsheet::Cell) -> Option<(String, bool)> {
    match cell.raw_value() {
        CellRawValue::String(text) | CellRawValue::Lazy(text) => Some((text.to_string(), false)),
        CellRawValue::RichText(rich) => Some((rich.text().to_string(), true)),
        _ => None,
    }
}

/// Glob of the first rule with `sheets` that matches none of the sheet `names`.
fn unmatched_sheet_rule<'a>(matchers: &[SheetRule<'a>], names: &[String]) -> Option<&'a str> {
    matchers
        .iter()
        .find(|(set, _)| {
            set.as_ref()
                .is_some_and(|set| !names.iter().any(|name| set.is_match(name)))
        })
        .map(|(_, rule)| rule.files.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text;

    fn rule(sheets: &[&str]) -> Rule {
        Rule {
            files: "*.xlsx".to_owned(),
            first_name: vec![],
            last_name: vec![],
            full_name: vec!["Name".to_owned()],
            email: vec!["Mail".to_owned()],
            sheets: sheets.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn fake(kind: Kind, value: &str) -> String {
        let tag = match kind {
            Kind::First => "F",
            Kind::Last => "L",
            Kind::Email => "E",
        };
        format!("{tag}-{}", value.len())
    }

    fn run(book: &mut Workbook, rules: &[&Rule]) -> Transformed {
        try_run(book, rules).unwrap()
    }

    fn try_run(book: &mut Workbook, rules: &[&Rule]) -> anyhow::Result<Transformed> {
        let regex = text::email_regex().unwrap();
        let fns = CellFns {
            email_regex: &regex,
            names: text::replace_name,
            emails: text::replace_emails,
            matcher: None,
        };
        transform(book, rules, &fns, &mut fake, None, "t.xlsx")
    }

    fn book() -> Workbook {
        let mut book = umya_spreadsheet::new_file_empty_worksheet();
        let one = book.new_sheet("Kunden 2025").unwrap();
        one.cell_mut("A2").set_value_string("Name");
        one.cell_mut("B2").set_value_string("Mail");
        one.cell_mut("C2").set_value_string("Amount");
        one.cell_mut("A3").set_value_string("Anna Beispiel");
        one.cell_mut("B3").set_value_string("anna@example.org");
        one.cell_mut("C3").set_value_number(12);
        one.cell_mut("C4").set_formula("C3*2");
        one.cell_mut("A3").style_mut().font_mut().set_bold(true);
        let two = book.new_sheet("Notizen").unwrap();
        two.cell_mut("A1").set_value_string("Amount");
        two.cell_mut("B1").set_value_string("Name");
        two.cell_mut("B2").set_value_string("Berta Muster");
        two.cell_mut("A2").set_value_string("Berta Muster");
        let hidden = book.new_sheet("Hidden").unwrap();
        hidden.set_state(umya_spreadsheet::SheetStateValues::Hidden);
        hidden.cell_mut("A1").set_value_string("Name");
        hidden.cell_mut("A2").set_value_string("Carl Probe");
        book
    }

    fn text_of(book: &Workbook, sheet: &str, at: &str) -> String {
        book.sheet_by_name(sheet)
            .unwrap()
            .cell(at)
            .map(|cell| cell.value().to_string())
            .unwrap_or_default()
    }

    #[test]
    fn replaces_strings_in_columns_per_sheet() {
        let mut book = book();
        let rule = rule(&[]);
        let result = run(&mut book, &[&rule]);
        assert_ne!(text_of(&book, "Kunden 2025", "A3"), "Anna Beispiel");
        assert!(text_of(&book, "Kunden 2025", "A3").starts_with("F-"));
        assert!(text_of(&book, "Kunden 2025", "B3").starts_with("E-"));
        assert_eq!(text_of(&book, "Kunden 2025", "A2"), "Name");
        assert_eq!(text_of(&book, "Kunden 2025", "C3"), "12");
        // Header in row 1 of the second sheet, other column order.
        assert!(text_of(&book, "Notizen", "B2").starts_with("F-"));
        assert_eq!(text_of(&book, "Notizen", "A2"), "Berta Muster");
        assert_eq!(result.stats.formulas, 1);
    }

    #[test]
    fn formula_stays() {
        let mut book = book();
        let rule = rule(&[]);
        run(&mut book, &[&rule]);
        let sheet = book.sheet_by_name("Kunden 2025").unwrap();
        assert_eq!(sheet.cell("C4").unwrap().formula(), "C3*2");
    }

    #[test]
    fn sheets_filter_limits_the_rule() {
        let mut book = book();
        let rule = rule(&["kunden*"]);
        run(&mut book, &[&rule]);
        assert!(text_of(&book, "Kunden 2025", "A3").starts_with("F-"));
        assert_eq!(text_of(&book, "Notizen", "B2"), "Berta Muster");
    }

    #[test]
    fn hidden_sheet_is_processed_and_stays_hidden() {
        let mut book = book();
        let rule = rule(&[]);
        run(&mut book, &[&rule]);
        assert!(text_of(&book, "Hidden", "A2").starts_with("F-"));
        let hidden = book.sheet_by_name("Hidden").unwrap();
        assert!(matches!(
            hidden.state(),
            umya_spreadsheet::SheetStateValues::Hidden
        ));
    }

    #[test]
    fn bold_style_stays() {
        let mut book = book();
        let rule = rule(&[]);
        run(&mut book, &[&rule]);
        let sheet = book.sheet_by_name("Kunden 2025").unwrap();
        assert!(
            sheet
                .cell("A3")
                .unwrap()
                .style()
                .font()
                .is_some_and(umya_spreadsheet::Font::bold)
        );
    }

    #[test]
    fn rule_without_matching_header_fails_with_the_sheet_position() {
        let mut book = book();
        let mut rule = rule(&[]);
        rule.full_name = vec!["Nope".to_owned()];
        rule.email = vec![];
        let error = try_run(&mut book, &[&rule]).err().map(|e| e.to_string());
        assert_eq!(
            error.as_deref(),
            Some("t.xlsx: sheet 1: no header of rule \"*.xlsx\" in the first 20 rows")
        );
    }

    #[test]
    fn second_sheet_without_header_fails_and_sheets_glob_avoids_it() {
        let mut book = umya_spreadsheet::new_file_empty_worksheet();
        let one = book.new_sheet("Daten").unwrap();
        one.cell_mut("A1").set_value_string("Name");
        one.cell_mut("A2").set_value_string("Anna Beispiel");
        let two = book.new_sheet("Info").unwrap();
        two.cell_mut("A1").set_value_string("Anna Beispiel");
        let rule_all = rule(&[]);
        let error = try_run(&mut book, &[&rule_all])
            .err()
            .map(|e| e.to_string());
        assert_eq!(
            error.as_deref(),
            Some("t.xlsx: sheet 2: no header of rule \"*.xlsx\" in the first 20 rows")
        );
        let rule_daten = rule(&["Daten"]);
        run(&mut book, &[&rule_daten]);
        assert!(text_of(&book, "Daten", "A2").starts_with("F-"));
        assert_eq!(text_of(&book, "Info", "A1"), "Anna Beispiel");
    }

    #[test]
    fn title_row_above_the_header_is_searched_past_and_scrubbed() {
        let mut book = umya_spreadsheet::new_file_empty_worksheet();
        let sheet = book.new_sheet("Daten").unwrap();
        sheet
            .cell_mut("A1")
            .set_value_string("Liste von anna@example.org");
        sheet.cell_mut("A2").set_value_string("Name");
        sheet.cell_mut("B2").set_value_string("zorbina@example.org");
        sheet.cell_mut("A3").set_value_string("Anna Beispiel");
        let rule = rule(&[]);
        run(&mut book, &[&rule]);
        assert!(text_of(&book, "Daten", "A1").starts_with("Liste von E-"));
        assert_eq!(text_of(&book, "Daten", "A2"), "Name");
        assert!(text_of(&book, "Daten", "B2").starts_with("E-"));
        assert!(text_of(&book, "Daten", "A3").starts_with("F-"));
    }

    #[test]
    fn header_is_searched_only_in_the_first_20_rows() {
        let mut book = umya_spreadsheet::new_file_empty_worksheet();
        let sheet = book.new_sheet("Daten").unwrap();
        for row in 1..=20 {
            sheet.cell_mut(format!("A{row}")).set_value_string("Titel");
        }
        sheet.cell_mut("A21").set_value_string("Name");
        let rule = rule(&[]);
        assert!(try_run(&mut book, &[&rule]).is_err());
        sheet_cell_fix(&mut book);
        assert!(try_run(&mut book, &[&rule]).is_ok());
    }

    fn sheet_cell_fix(book: &mut Workbook) {
        if let Ok(sheet) = book.sheet_by_name_mut("Daten") {
            sheet.cell_mut("A20").set_value_string("Name");
        }
    }

    #[test]
    fn sheet_without_rules_scrubs_each_row_including_the_first() {
        let mut book = umya_spreadsheet::new_file_empty_worksheet();
        let sheet = book.new_sheet("Daten").unwrap();
        sheet.cell_mut("A1").set_value_string("anna@example.org");
        sheet.cell_mut("A2").set_value_string("berta@example.org");
        run(&mut book, &[]);
        assert!(text_of(&book, "Daten", "A1").starts_with("E-"));
        assert!(text_of(&book, "Daten", "A2").starts_with("E-"));
    }

    #[test]
    fn workbook_round_trips_with_sheet_order() {
        let book = book();
        let mut bytes = Vec::new();
        writer::xlsx::write_writer(&book, &mut bytes).unwrap();
        let book = reader::xlsx::read_reader(Cursor::new(bytes), true).unwrap();
        let names: Vec<&str> = book
            .sheet_collection()
            .iter()
            .map(umya_spreadsheet::Worksheet::name)
            .collect();
        assert_eq!(names, ["Kunden 2025", "Notizen", "Hidden"]);
    }

    #[test]
    fn rich_text_cell_becomes_plain_and_is_counted() {
        let mut book = book();
        let mut rich = umya_spreadsheet::RichText::default();
        rich.set_text("Dora Muster");
        book.sheet_by_name_mut("Kunden 2025")
            .unwrap()
            .cell_mut("A5")
            .set_rich_text(rich);
        let rule = rule(&[]);
        let result = run(&mut book, &[&rule]);
        assert_eq!(result.stats.rich_text, 1);
        assert!(text_of(&book, "Kunden 2025", "A5").starts_with("F-"));
        let cell = book
            .sheet_by_name("Kunden 2025")
            .unwrap()
            .cell("A5")
            .unwrap();
        assert!(matches!(cell.raw_value(), CellRawValue::String(_)));
    }

    #[test]
    fn hyperlink_email_gets_the_same_fake_as_the_cell() {
        let mut book = book();
        let sheet = book.sheet_by_name_mut("Kunden 2025").unwrap();
        let cell = sheet.cell_mut("B4");
        cell.set_value_string("zorbina@acme.test");
        let link = cell.hyperlink_mut();
        link.set_url("mailto:zorbina@acme.test");
        link.set_tooltip("Write to zorbina@acme.test");
        let rule = rule(&[]);
        run(&mut book, &[&rule]);
        let sheet = book.sheet_by_name("Kunden 2025").unwrap();
        let cell = sheet.cell("B4").unwrap();
        let fake = cell.value().to_string();
        assert!(fake.starts_with("E-"));
        let link = cell.hyperlink().unwrap();
        assert_eq!(link.url(), format!("mailto:{fake}"));
        assert_eq!(link.tooltip(), "");
    }

    #[test]
    fn mailto_is_decoded_cut_and_replaced() {
        let mut book = book();
        let sheet = book.sheet_by_name_mut("Kunden 2025").unwrap();
        let cell = sheet.cell_mut("B4");
        cell.set_value_string("zorbina@acme.test");
        cell.hyperlink_mut()
            .set_url("mailto:zorbina%40acme.test?subject=Hi%20Zorbina&body=x");
        let cell = sheet.cell_mut("B5");
        cell.set_value_string("zorbina@acme.test");
        cell.hyperlink_mut().set_url("MAILTO:zorbina@acme.test");
        let rule = rule(&[]);
        run(&mut book, &[&rule]);
        let sheet = book.sheet_by_name("Kunden 2025").unwrap();
        let fake = sheet.cell("B4").unwrap().value().to_string();
        assert_eq!(
            sheet.cell("B4").unwrap().hyperlink().unwrap().url(),
            format!("mailto:{fake}")
        );
        assert_eq!(
            sheet.cell("B5").unwrap().hyperlink().unwrap().url(),
            format!("mailto:{fake}")
        );
    }

    #[test]
    fn percent_decode_keeps_invalid_escapes() {
        assert_eq!(percent_decode("a%40b", false), "a@b");
        assert_eq!(percent_decode("a%4", false), "a%4");
        assert_eq!(percent_decode("a%zzb%", false), "a%zzb%");
        assert_eq!(percent_decode("%C3%A4", false), "\u{e4}");
        assert_eq!(percent_decode("a+b", true), "a b");
        assert_eq!(percent_decode("a+b", false), "a+b");
        // A byte that is no UTF-8 stays an escape, and the rest is decoded.
        assert_eq!(percent_encode(&percent_decode("%FF", false), false), "%FF");
        assert_eq!(
            percent_encode(&percent_decode("%2CZ%C3%A4%FFx%20y", false), false),
            ",Z%C3%A4%FFx%20y"
        );
        assert_eq!(percent_decode("%2CZ%FF", false).chars().count(), 3);
        assert_eq!(percent_encode("a b+c", true), "a+b%2Bc");
        assert_eq!(percent_encode("a b+c", false), "a%20b+c");
    }

    #[test]
    fn other_url_keeps_the_email_replacement_and_tooltip_is_cleared() {
        let mut book = book();
        let sheet = book.sheet_by_name_mut("Kunden 2025").unwrap();
        let link = sheet.cell_mut("D5").hyperlink_mut();
        link.set_url("https://example.test/?u=zorbina@acme.test");
        link.set_tooltip("Invented tip");
        let rule = rule(&[]);
        run(&mut book, &[&rule]);
        let sheet = book.sheet_by_name("Kunden 2025").unwrap();
        let link = sheet.cell("D5").unwrap().hyperlink().unwrap();
        assert!(!link.url().contains("zorbina"));
        assert_eq!(link.tooltip(), "");
    }

    #[test]
    fn cached_string_result_of_a_formula_is_cleared() {
        let mut book = book();
        let cell = book
            .sheet_by_name_mut("Kunden 2025")
            .unwrap()
            .cell_mut("D3");
        cell.set_formula("A3");
        cell.set_formula_result_string("Anna Beispiel");
        let rule = rule(&[]);
        let result = run(&mut book, &[&rule]);
        let cell = book
            .sheet_by_name("Kunden 2025")
            .unwrap()
            .cell("D3")
            .unwrap();
        assert_eq!(cell.formula(), "A3");
        assert!(matches!(cell.raw_value(), CellRawValue::Empty));
        assert_eq!(result.stats.formulas, 2);
    }

    #[test]
    fn rule_with_sheets_that_match_no_sheet_fails() {
        let mut book = book();
        let rule = rule(&["nothing*"]);
        let error = try_run(&mut book, &[&rule]).err().map(|e| e.to_string());
        assert_eq!(
            error.as_deref(),
            Some("t.xlsx: rule \"*.xlsx\" matches no sheet")
        );
        assert_eq!(text_of(&book, "Kunden 2025", "A3"), "Anna Beispiel");
    }

    #[test]
    fn header_row_with_most_header_texts_wins() {
        let mut book = umya_spreadsheet::new_file_empty_worksheet();
        let sheet = book.new_sheet("Daten").unwrap();
        sheet.cell_mut("A1").set_value_string("Bericht");
        sheet.cell_mut("C1").set_value_string("Mail");
        sheet.cell_mut("A2").set_value_string("Name");
        sheet.cell_mut("B2").set_value_string("Mail");
        sheet.cell_mut("A3").set_value_string("Zorbina Quenzel");
        let rule = rule(&[]);
        run(&mut book, &[&rule]);
        assert!(text_of(&book, "Daten", "A3").starts_with("F-"));
    }

    #[test]
    fn key_value_layout_with_equal_counts_is_ambiguous() {
        let mut book = umya_spreadsheet::new_file_empty_worksheet();
        let sheet = book.new_sheet("Daten").unwrap();
        sheet.cell_mut("A1").set_value_string("Name");
        sheet.cell_mut("B1").set_value_string("Zorbina Quenzel");
        sheet.cell_mut("A2").set_value_string("Mail");
        sheet.cell_mut("B2").set_value_string("Tamsorin Velgrad");
        let rule = rule(&[]);
        let error = try_run(&mut book, &[&rule]).err().map(|e| e.to_string());
        assert_eq!(
            error.as_deref(),
            Some("t.xlsx: sheet 1: header row is ambiguous")
        );
    }
}
