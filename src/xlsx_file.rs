//! Pseudonymize one XLSX file.

use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, anyhow};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::Regex;
use umya_spreadsheet::{CellRawValue, Workbook, reader, writer};

use crate::cells::{
    ColRole, EmailFn, Kind, NameFn, collect_columns, missing_headers, role_of,
    rules_without_headers, transform_cell, write_output,
};
use crate::config::Rule;
use crate::panic_guard;
use crate::pseudonym::Pseudonymizer;
use crate::text;
use crate::xlsx_scrub;

/// Counts for one processed workbook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct XlsxStats {
    /// Replaced values.
    pub replaced: usize,
    /// Formula cells (left unchanged).
    pub formulas: usize,
    /// Rich-text cells that were written as plain text.
    pub rich_text: usize,
}

/// A rule with the sheet-name globs of its `sheets` field (`None`: each sheet).
type SheetRule<'a> = (Option<GlobSet>, &'a Rule);

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
    email_regex: &Regex,
) -> anyhow::Result<XlsxStats> {
    let bytes = fs::read(input).with_context(|| format!("cannot read {rel}"))?;
    let unreadable = || anyhow!("{rel}: cannot read the workbook");
    if has_pivot_parts(&bytes) {
        return Err(anyhow!("{rel}: workbook has pivot tables; not supported"));
    }
    let found = xlsx_scrub::inspect(&bytes).map_err(|_| unreadable())?;
    if found.unsupported {
        return Err(anyhow!("{rel}: unsupported workbook layout"));
    }
    if !found.kinds.is_empty() {
        eprintln!(
            "warning: {rel}: not pseudonymized: {}",
            found.kinds.join(", ")
        );
    }
    // The crate can panic on a damaged workbook; its error texts are not used.
    let mut book = panic_guard::catch(|| reader::xlsx::read_reader(Cursor::new(bytes), true))
        .ok_or_else(unreadable)?
        .map_err(|_| unreadable())?;

    let mut replace = |kind: Kind, value: &str| match kind {
        Kind::First => pseudonymizer.first_name(value),
        Kind::Last => pseudonymizer.last_name(value),
        Kind::Email => pseudonymizer.email(value),
    };
    let result = transform(
        &mut book,
        rules,
        email_regex,
        text::replace_name,
        text::replace_emails,
        &mut replace,
    )?;
    for glob in &result.no_sheet {
        eprintln!("warning: rule \"{glob}\" matches {rel}, but no sheet matches its sheets globs");
    }
    for glob in rules_without_headers(rules, &result.missing) {
        if !result.no_sheet.iter().any(|g| g == glob) {
            eprintln!(
                "warning: rule \"{glob}\" matches {rel}, but the file has none of its headers"
            );
        }
    }

    let mut out: Vec<u8> = Vec::new();
    let unwritable = || anyhow!("{rel}: cannot write the workbook");
    panic_guard::catch(|| writer::xlsx::write_writer(&book, &mut out))
        .ok_or_else(unwritable)?
        .map_err(|_| unwritable())?;
    let out = xlsx_scrub::scrub(&out).map_err(|_| unwritable())?;
    write_output(out_root, output, &out, rel)?;
    Ok(result.stats)
}

/// True if the package has a pivot cache or a pivot table part.
fn has_pivot_parts(bytes: &[u8]) -> bool {
    zip::ZipArchive::new(Cursor::new(bytes)).is_ok_and(|archive| {
        archive
            .file_names()
            .any(|name| name.starts_with("xl/pivotCache/") || name.starts_with("xl/pivotTables/"))
    })
}

/// Replace each email in `value` through the email pseudonym.
fn replace_in_text(
    value: &str,
    email_regex: &Regex,
    emails: EmailFn,
    replace: &mut dyn FnMut(Kind, &str) -> String,
) -> String {
    emails(value, email_regex, &mut |address| {
        replace(Kind::Email, address)
    })
}

/// Percent-decode `text` as UTF-8. An invalid escape stays as it is; if the
/// result is not UTF-8, `text` is returned unchanged.
fn percent_decode(text: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        char::from(byte)
            .to_digit(16)
            .and_then(|digit| u8::try_from(digit).ok())
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(high * 16 + low);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

/// The new url of a hyperlink. A `mailto:` url is decoded, cut at the first
/// `?`, and its addresses are replaced; other urls only get the replacement.
fn new_url(
    url: &str,
    email_regex: &Regex,
    emails: EmailFn,
    replace: &mut dyn FnMut(Kind, &str) -> String,
) -> String {
    const SCHEME: &str = "mailto:";
    if url.len() >= SCHEME.len()
        && url.is_char_boundary(SCHEME.len())
        && url[..SCHEME.len()].eq_ignore_ascii_case(SCHEME)
    {
        let decoded = percent_decode(&url[SCHEME.len()..]);
        let address = decoded.split('?').next().unwrap_or_default();
        format!(
            "{SCHEME}{}",
            replace_in_text(address, email_regex, emails, replace)
        )
    } else {
        replace_in_text(url, email_regex, emails, replace)
    }
}

/// Replace the emails in the url of the hyperlink of `cell` and clear its
/// tooltip.
fn scrub_hyperlink(
    cell: &mut umya_spreadsheet::Cell,
    email_regex: &Regex,
    emails: EmailFn,
    replace: &mut dyn FnMut(Kind, &str) -> String,
) {
    let Some(link) = cell.hyperlink() else {
        return;
    };
    let url = new_url(link.url(), email_regex, emails, replace);
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
    /// (rule glob, header text) pairs that no applicable sheet has.
    missing: Vec<(String, String)>,
    /// Globs of rules with `sheets` that match no sheet of the workbook.
    no_sheet: Vec<String>,
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

fn transform(
    book: &mut Workbook,
    rules: &[&Rule],
    email_regex: &Regex,
    names: NameFn,
    emails: EmailFn,
    replace: &mut dyn FnMut(Kind, &str) -> String,
) -> anyhow::Result<Transformed> {
    let matchers = sheet_rules(rules)?;
    let all_pairs = missing_headers(&[], &collect_columns(rules));
    let mut found: Vec<(String, String)> = Vec::new();
    let mut stats = XlsxStats::default();
    let mut sheet_matched = vec![false; matchers.len()];

    for sheet in book.sheet_collection_mut() {
        let name = sheet.name().to_owned();
        let mut applicable: Vec<&Rule> = Vec::new();
        for (i, (set, rule)) in matchers.iter().enumerate() {
            if set.as_ref().is_none_or(|set| set.is_match(&name)) {
                sheet_matched[i] = true;
                applicable.push(rule);
            }
        }
        let columns = collect_columns(&applicable);
        let mut cells = sheet.cells_mut();
        cells.sort_by_key(|cell| {
            let at = cell.coordinate();
            (at.row_num(), at.col_num())
        });
        let Some(header_row) = cells
            .iter()
            .find(|cell| !cell.value().trim().is_empty())
            .map(|cell| cell.coordinate().row_num())
        else {
            continue;
        };
        let mut header_texts: HashMap<u32, String> = HashMap::new();
        for cell in &cells {
            let at = cell.coordinate();
            if at.row_num() == header_row {
                header_texts.insert(at.col_num(), cell.value().trim().to_lowercase());
            }
        }
        let texts: Vec<String> = header_texts.values().cloned().collect();
        let sheet_missing = missing_headers(&texts, &columns);
        for pair in missing_headers(&[], &columns) {
            if !sheet_missing.contains(&pair) {
                found.push(pair);
            }
        }
        let by_col: HashMap<u32, ColRole> = header_texts
            .iter()
            .filter_map(|(col, text)| role_of(text, &columns).map(|role| (*col, role)))
            .collect();

        for cell in &mut cells {
            let at = cell.coordinate();
            let (row, col) = (at.row_num(), at.col_num());
            scrub_hyperlink(cell, email_regex, emails, replace);
            if cell.is_formula() {
                if row > header_row {
                    stats.formulas += 1;
                }
                clear_cached_text(cell);
                continue;
            }
            if row <= header_row {
                continue;
            }
            let (old, rich) = match cell.raw_value() {
                CellRawValue::String(text) | CellRawValue::Lazy(text) => (text.to_string(), false),
                CellRawValue::RichText(rich) => (rich.text().to_string(), true),
                _ => continue,
            };
            let new = transform_cell(
                &old,
                by_col.get(&col).copied(),
                email_regex,
                names,
                emails,
                &mut |kind, value| {
                    stats.replaced += 1;
                    replace(kind, value)
                },
            );
            if new != old {
                cell.set_value_string(new);
                if rich {
                    stats.rich_text += 1;
                }
            }
        }
    }
    let missing = all_pairs
        .into_iter()
        .filter(|pair| !found.contains(pair))
        .collect();
    let no_sheet = matchers
        .iter()
        .zip(&sheet_matched)
        .filter(|((set, _), matched)| set.is_some() && !**matched)
        .map(|((_, rule), _)| rule.files.clone())
        .collect();
    Ok(Transformed {
        stats,
        missing,
        no_sheet,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let regex = text::email_regex().unwrap();
        transform(
            book,
            rules,
            &regex,
            text::replace_name,
            text::replace_emails,
            &mut fake,
        )
        .unwrap()
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
        assert_eq!(result.missing.len(), 0);
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
        let result = run(&mut book, &[&rule]);
        assert!(text_of(&book, "Kunden 2025", "A3").starts_with("F-"));
        assert_eq!(text_of(&book, "Notizen", "B2"), "Berta Muster");
        assert_eq!(result.missing.len(), 0);
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
    fn rule_without_matching_header_is_reported_as_missing() {
        let mut book = book();
        let mut rule = rule(&[]);
        rule.full_name = vec!["Nope".to_owned()];
        rule.email = vec![];
        let result = run(&mut book, &[&rule]);
        assert_eq!(rules_without_headers(&[&rule], &result.missing).len(), 1);
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
        assert_eq!(percent_decode("a%40b"), "a@b");
        assert_eq!(percent_decode("a%4"), "a%4");
        assert_eq!(percent_decode("a%zzb%"), "a%zzb%");
        assert_eq!(percent_decode("%C3%A4"), "\u{e4}");
        assert_eq!(percent_decode("%FF"), "%FF");
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
    fn rule_with_sheets_that_match_no_sheet_is_reported() {
        let mut book = book();
        let rule = rule(&["nothing*"]);
        let result = run(&mut book, &[&rule]);
        assert_eq!(result.no_sheet, ["*.xlsx"]);
        assert_eq!(text_of(&book, "Kunden 2025", "A3"), "Anna Beispiel");
    }

    #[test]
    fn pivot_parts_are_detected() {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("xl/pivotCache/pivotCacheDefinition1.xml", options)
            .unwrap();
        writer.write_all(b"<x/>").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert!(has_pivot_parts(&bytes));
        assert!(!has_pivot_parts(b"not a zip"));
    }
}
