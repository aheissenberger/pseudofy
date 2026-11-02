//! Pseudonymize one XLSX file.

use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
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
use crate::pseudonym::Pseudonymizer;
use crate::text;

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
    // The crate can panic on a damaged workbook; its error texts are not used.
    let mut book = catch_unwind(AssertUnwindSafe(|| {
        reader::xlsx::read_reader(Cursor::new(bytes), true)
    }))
    .map_err(|_| unreadable())?
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
    for glob in rules_without_headers(rules, &result.missing) {
        eprintln!("warning: rule \"{glob}\" matches {rel}, but the file has none of its headers");
    }

    let mut out: Vec<u8> = Vec::new();
    let unwritable = || anyhow!("{rel}: cannot write the workbook");
    catch_unwind(AssertUnwindSafe(|| {
        writer::xlsx::write_writer(&book, &mut out)
    }))
    .map_err(|_| unwritable())?
    .map_err(|_| unwritable())?;
    write_output(out_root, output, &out, rel)?;
    Ok(result.stats)
}

/// Result of the transformation of one workbook.
struct Transformed {
    stats: XlsxStats,
    /// (rule glob, header text) pairs that no applicable sheet has.
    missing: Vec<(String, String)>,
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

    for sheet in book.sheet_collection_mut() {
        let name = sheet.name().to_owned();
        let applicable: Vec<&Rule> = matchers
            .iter()
            .filter(|(set, _)| set.as_ref().is_none_or(|set| set.is_match(&name)))
            .map(|(_, rule)| *rule)
            .collect();
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
            if row <= header_row {
                continue;
            }
            if cell.is_formula() {
                stats.formulas += 1;
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
    Ok(Transformed { stats, missing })
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
        hidden.set_sheet_state("hidden".to_owned());
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
        assert_eq!(hidden.sheet_state(), "hidden");
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
}
