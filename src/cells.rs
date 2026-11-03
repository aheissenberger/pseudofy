//! Column roles, header matching, cell replacement, and the safe output
//! write. `csv_file` and `xlsx_file` use the same functions.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, bail};
use regex::Regex;

use crate::config::Rule;
use crate::known_names::{KnownNames, Matcher};
use crate::text::{self, NameRole, Part};

/// Name replacement function (the signature of `text::replace_name`).
pub(crate) type NameFn = fn(&str, NameRole, &mut dyn FnMut(Part, &str) -> String) -> String;

/// Email replacement function (the signature of `text::replace_emails`).
pub(crate) type EmailFn = fn(&str, &Regex, &mut dyn FnMut(&str) -> String) -> String;

/// Kind of value that the replace callback gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    First,
    Last,
    Email,
}

/// Role of a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColRole {
    Name(NameRole),
    Email,
}

/// Header texts (trimmed, lowercase), each with the glob of its rule.
#[derive(Debug, Default)]
pub(crate) struct Columns {
    pub(crate) first: Vec<(String, String)>,
    pub(crate) last: Vec<(String, String)>,
    pub(crate) full: Vec<(String, String)>,
    pub(crate) email: Vec<(String, String)>,
}

/// Globs of the rules that name at least one header and find none of them
/// in the file. A rule that finds some of its headers gives no warning.
pub(crate) fn rules_without_headers(rules: &[&Rule], missing: &[(String, String)]) -> Vec<String> {
    rules
        .iter()
        .filter(|rule| {
            let named: Vec<&String> = rule
                .first_name
                .iter()
                .chain(&rule.last_name)
                .chain(&rule.full_name)
                .chain(&rule.email)
                .collect();
            !named.is_empty()
                && named.iter().all(|header| {
                    let header = header.trim().to_lowercase();
                    missing
                        .iter()
                        .any(|(glob, miss)| *glob == rule.label() && *miss == header)
                })
        })
        .map(|rule| rule.label())
        .collect()
}

pub(crate) fn is_not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

/// Name of the temp file in the output directory.
const TEMP_NAME: &str = ".pseudofy-out.tmp";

/// Write `bytes` to `output` without following a symlink: check each directory
/// below `out_root`, refuse a symlink target, write a temp file, rename it.
/// The copy gets the permission bits of `input` (a private input gives a
/// private copy).
pub(crate) fn write_output(
    out_root: &Path,
    output: &Path,
    bytes: &[u8],
    rel: &str,
    input: &Path,
) -> anyhow::Result<()> {
    let parent = output.parent().unwrap_or(out_root);
    let below = parent
        .strip_prefix(out_root)
        .unwrap_or_else(|_| Path::new(""));
    let mut current = out_root.to_path_buf();
    for part in below.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                bail!("{rel}: an output directory is a symlink");
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => bail!("{rel}: an output path is not a directory"),
            Err(error) if is_not_found(&error) => {
                fs::create_dir(&current)
                    .with_context(|| format!("cannot create directory for {rel}"))?;
            }
            Err(_) => bail!("cannot check the output directory for {rel}"),
        }
    }
    match fs::symlink_metadata(output) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("{rel}: the output file is a symlink");
        }
        Ok(meta) if meta.is_dir() => bail!("{rel}: the output path is a directory"),
        Ok(_) => {}
        Err(error) if is_not_found(&error) => {}
        Err(_) => bail!("cannot check the output file for {rel}"),
    }
    // A short fixed name: a long but legal file name still fits.
    let temp = parent.join(TEMP_NAME);
    let permissions = fs::metadata(input).ok().map(|meta| meta.permissions());
    match fs::remove_file(&temp) {
        Ok(()) => {}
        Err(error) if is_not_found(&error) => {}
        Err(_) => bail!("cannot write {rel}"),
    }
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .and_then(|mut file| {
            if let Some(permissions) = permissions {
                file.set_permissions(permissions)?;
            }
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temp, output));
    if written.is_err() {
        // Best effort: the temp file holds pseudonymized data only.
        drop(fs::remove_file(&temp));
    }
    written.with_context(|| format!("cannot write {rel}"))
}

pub(crate) fn collect_columns(rules: &[&Rule]) -> Columns {
    let mut columns = Columns::default();
    for rule in rules {
        let add = |target: &mut Vec<(String, String)>, headers: &[String]| {
            for header in headers {
                target.push((rule.label(), header.trim().to_lowercase()));
            }
        };
        add(&mut columns.first, &rule.first_name);
        add(&mut columns.last, &rule.last_name);
        add(&mut columns.full, &rule.full_name);
        add(&mut columns.email, &rule.email);
    }
    columns
}

pub(crate) fn name_kind(part: Part) -> Kind {
    match part {
        Part::First => Kind::First,
        Part::Last => Kind::Last,
    }
}

const MAILTO: &str = "mailto:";

/// `text` split before a trailing `mailto:` (any case); the tail is empty
/// without one.
fn split_mailto(text: &str) -> (&str, &str) {
    let at = text.len().saturating_sub(MAILTO.len());
    match (text.get(..at), text.get(at..)) {
        (Some(head), Some(tail)) if tail.eq_ignore_ascii_case(MAILTO) => (head, tail),
        _ => (text, ""),
    }
}

/// Replace the addresses at `spans`; run the text outside them through `names`.
/// With `keep_mailto`, a `mailto:` right before an address stays as it is.
pub(crate) fn replace_spans(
    cell: &str,
    spans: &[std::ops::Range<usize>],
    names: NameFn,
    name_role: NameRole,
    keep_mailto: bool,
    replace: &mut dyn FnMut(Kind, &str) -> String,
) -> String {
    let mut out = String::with_capacity(cell.len());
    let mut pos = 0;
    for span in spans {
        let before = cell.get(pos..span.start).unwrap_or("");
        let (before, tag) = if keep_mailto {
            split_mailto(before)
        } else {
            (before, "")
        };
        out.push_str(&names(before, name_role, &mut |part, token| {
            replace(name_kind(part), token)
        }));
        out.push_str(tag);
        out.push_str(&replace(Kind::Email, cell.get(span.clone()).unwrap_or("")));
        pos = span.end;
    }
    let rest = cell.get(pos..).unwrap_or("");
    out.push_str(&names(rest, name_role, &mut |part, token| {
        replace(name_kind(part), token)
    }));
    out
}

/// What `transform_cell` needs besides the cell.
pub(crate) struct CellFns<'a> {
    pub(crate) email_regex: &'a Regex,
    pub(crate) names: NameFn,
    pub(crate) emails: EmailFn,
    /// Known names for text cells (pass 2 of spec 0004); `None`: no free-text pass.
    pub(crate) matcher: Option<&'a Matcher>,
}

/// `full_name` value without email addresses (they are no name).
fn without_emails(value: &str) -> String {
    static REGEX: OnceLock<Option<Regex>> = OnceLock::new();
    let Some(regex) = REGEX.get_or_init(|| text::email_regex().ok()).as_ref() else {
        return value.to_owned();
    };
    let mut out = String::with_capacity(value.len());
    let mut pos = 0;
    for span in text::email_spans(value, regex) {
        out.push_str(value.get(pos..span.start).unwrap_or(""));
        out.push(' ');
        pos = span.end;
    }
    out.push_str(value.get(pos..).unwrap_or(""));
    out
}

/// Known names found in the name columns of one file (pass 1). They reach
/// `KnownNames` only through `apply`, after the whole file was read.
///
/// A row can have several first-name and last-name cells. All of them give
/// tokens. If a row has as many first-name as last-name cells, the i-th first
/// value and the i-th last value form one person; otherwise there is no person.
#[derive(Default)]
pub(crate) struct Collector {
    rows: Vec<(Vec<String>, Vec<String>)>,
    fulls: Vec<String>,
    firsts: Vec<String>,
    lasts: Vec<String>,
}

impl Collector {
    /// One data cell. Call `end_row` after the last cell of each row.
    pub(crate) fn cell(&mut self, role: Option<ColRole>, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        match role {
            Some(ColRole::Name(NameRole::First)) => self.firsts.push(value.to_owned()),
            Some(ColRole::Name(NameRole::Last)) => self.lasts.push(value.to_owned()),
            Some(ColRole::Name(NameRole::Full)) => self.fulls.push(without_emails(value)),
            _ => {}
        }
    }

    /// End of a data row: the first-name and last-name cells form persons.
    pub(crate) fn end_row(&mut self) {
        if !self.firsts.is_empty() || !self.lasts.is_empty() {
            self.rows.push((
                std::mem::take(&mut self.firsts),
                std::mem::take(&mut self.lasts),
            ));
        }
    }

    pub(crate) fn apply(mut self, known: &mut KnownNames) {
        self.end_row();
        for (firsts, lasts) in &self.rows {
            if firsts.len() == lasts.len() {
                for (first, last) in firsts.iter().zip(lasts) {
                    known.add_person(first, last);
                }
            } else {
                for first in firsts {
                    known.add_first(first);
                }
                for last in lasts {
                    known.add_last(last);
                }
            }
        }
        for value in &self.fulls {
            known.add_full(value);
        }
    }
}

/// Replace the known names in `text` (which holds no address).
fn free_segment(
    matcher: &Matcher,
    text: &str,
    replace: &mut dyn FnMut(Kind, &str) -> String,
    free: &mut usize,
) -> String {
    let (out, count) = matcher.replace(text, &mut |part, token| replace(name_kind(part), token));
    *free += count;
    out
}

/// Replace the addresses of a text cell; replace the known names in the text
/// outside the addresses.
fn replace_free_text(
    cell: &str,
    fns: &CellFns,
    matcher: &Matcher,
    replace: &mut dyn FnMut(Kind, &str) -> String,
    free: &mut usize,
) -> String {
    let mut out = String::with_capacity(cell.len());
    let mut pos = 0;
    for span in text::email_spans(cell, fns.email_regex) {
        let before = cell.get(pos..span.start).unwrap_or("");
        out.push_str(&free_segment(matcher, before, replace, free));
        out.push_str(&replace(Kind::Email, cell.get(span.clone()).unwrap_or("")));
        pos = span.end;
    }
    out.push_str(&free_segment(
        matcher,
        cell.get(pos..).unwrap_or(""),
        replace,
        free,
    ));
    out
}

/// Replace names and emails in one cell, depending on the column role. In a
/// cell without role, the known names count in `free`.
pub(crate) fn transform_cell(
    cell: &str,
    role: Option<ColRole>,
    fns: &CellFns,
    replace: &mut dyn FnMut(Kind, &str) -> String,
    free: &mut usize,
) -> String {
    let (email_regex, names, emails) = (fns.email_regex, fns.names, fns.emails);
    let trimmed = cell.trim();
    if trimmed.is_empty() {
        return cell.to_owned();
    }
    match role {
        Some(ColRole::Name(name_role)) => {
            // Emails first; names only in the text outside the matches.
            replace_spans(
                cell,
                &text::email_spans(cell, email_regex),
                names,
                name_role,
                false,
                replace,
            )
        }
        Some(ColRole::Email) => {
            let spans = text::email_spans(cell, email_regex);
            if !spans.is_empty() {
                return replace_spans(cell, &spans, names, NameRole::Full, true, replace);
            }
            // No address found: the whole trimmed cell is the address. A
            // `mailto:` prefix stays.
            let lead = cell.len() - cell.trim_start().len();
            let tail = cell.len() - cell.trim_end().len();
            let has_mailto = trimmed
                .get(..MAILTO.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(MAILTO));
            let (scheme, address) = trimmed.split_at(if has_mailto { MAILTO.len() } else { 0 });
            let mut out = String::with_capacity(cell.len());
            out.push_str(cell.get(..lead).unwrap_or(""));
            out.push_str(scheme);
            out.push_str(&replace(Kind::Email, address));
            out.push_str(cell.get(cell.len() - tail..).unwrap_or(""));
            out
        }
        None => match fns.matcher {
            Some(matcher) => replace_free_text(cell, fns, matcher, replace, free),
            None => emails(cell, email_regex, &mut |address| {
                replace(Kind::Email, address)
            }),
        },
    }
}

pub(crate) fn role_of(header: &str, columns: &Columns) -> Option<ColRole> {
    let has = |list: &[(String, String)]| list.iter().any(|(_, text)| text == header);
    if has(&columns.first) {
        Some(ColRole::Name(NameRole::First))
    } else if has(&columns.last) {
        Some(ColRole::Name(NameRole::Last))
    } else if has(&columns.full) {
        Some(ColRole::Name(NameRole::Full))
    } else if has(&columns.email) {
        Some(ColRole::Email)
    } else {
        None
    }
}

/// Number of non-empty rows (CSV records, sheet rows) in which the header is searched.
pub(crate) const HEADER_SEARCH_ROWS: usize = 20;

/// Number of the normalized `texts` that are exactly a configured header text.
pub(crate) fn header_count(texts: &[String], columns: &Columns) -> usize {
    texts
        .iter()
        .filter(|text| role_of(text, columns).is_some())
        .count()
}

/// Result of the header search over the header counts of the first rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderPick {
    /// No row has a header text.
    None,
    /// Index (among the searched rows) of the row with the most header texts.
    At(usize),
    /// Two rows share the highest count.
    Ambiguous,
}

/// The header is the row with the most header texts. `counts` has one entry per
/// non-empty row; only the first `HEADER_SEARCH_ROWS` count.
pub(crate) fn pick_header(counts: &[usize]) -> HeaderPick {
    let window = counts.get(..HEADER_SEARCH_ROWS).unwrap_or(counts);
    let max = window.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return HeaderPick::None;
    }
    let mut best = window
        .iter()
        .enumerate()
        .filter(|&(_, &count)| count == max)
        .map(|(index, _)| index);
    match (best.next(), best.next()) {
        (Some(index), None) => HeaderPick::At(index),
        _ => HeaderPick::Ambiguous,
    }
}

/// Replace emails and known names in a title row or header cell. A cell whose
/// text is a configured header text stays unchanged.
pub(crate) fn transform_text_cell(
    cell: &str,
    columns: &Columns,
    fns: &CellFns,
    replace: &mut dyn FnMut(Kind, &str) -> String,
    free: &mut usize,
) -> String {
    if role_of(&cell.trim().to_lowercase(), columns).is_some() {
        return cell.to_owned();
    }
    transform_cell(cell, None, fns, replace, free)
}

/// Glob of the first rule that names headers and has none of them in the
/// header row. `header` is `None` if the search found no header row.
pub(crate) fn failing_rule(
    rules: &[&Rule],
    header: Option<&[String]>,
    columns: &Columns,
) -> Option<String> {
    let missing = missing_headers(header.unwrap_or(&[]), columns);
    rules_without_headers(rules, &missing).into_iter().next()
}

pub(crate) fn missing_headers(headers: &[String], columns: &Columns) -> Vec<(String, String)> {
    columns
        .first
        .iter()
        .chain(&columns.last)
        .chain(&columns.full)
        .chain(&columns.email)
        .filter(|(_, text)| !headers.contains(text))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_output_uses_a_short_temp_name_for_a_long_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let name = format!("{}.csv", "n".repeat(245));
        let input = dir.path().join("in.csv");
        fs::write(&input, "x").unwrap();
        let out = dir.path().join("out");
        fs::create_dir(&out).unwrap();
        write_output(&out, &out.join(&name), b"x", &name, &input).unwrap();
        assert_eq!(fs::read(out.join(&name)).unwrap(), b"x");
        assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn write_output_gives_the_copy_the_input_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.csv");
        fs::write(&input, "x").unwrap();
        fs::set_permissions(&input, fs::Permissions::from_mode(0o600)).unwrap();
        let out = dir.path().join("out");
        fs::create_dir(&out).unwrap();
        write_output(&out, &out.join("a.csv"), b"x", "a.csv", &input).unwrap();
        let mode = fs::metadata(out.join("a.csv"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    use crate::config::FreeText;

    fn count(known: &KnownNames, mode: FreeText, text: &str) -> usize {
        let matcher = known.matcher(mode);
        matcher.replace(text, &mut |_, _| "X".to_owned()).1
    }

    fn first() -> ColRole {
        ColRole::Name(NameRole::First)
    }

    fn last() -> ColRole {
        ColRole::Name(NameRole::Last)
    }

    #[test]
    fn several_name_columns_pair_by_position() {
        let mut c = Collector::default();
        c.cell(Some(first()), "Zorbina");
        c.cell(Some(first()), "Tamsorin");
        c.cell(Some(last()), "Quenzel");
        c.cell(Some(last()), "Velgrad");
        c.end_row();
        // Unequal counts: tokens only, no person.
        c.cell(Some(first()), "Mirwen");
        c.cell(Some(first()), "Ostrel");
        c.cell(Some(last()), "Dunhaldt");
        c.end_row();
        let mut known = KnownNames::default();
        c.apply(&mut known);
        assert_eq!(count(&known, FreeText::Full, "Zorbina Quenzel"), 1);
        assert_eq!(count(&known, FreeText::Full, "Tamsorin Velgrad"), 1);
        assert_eq!(count(&known, FreeText::Full, "Zorbina Velgrad"), 0);
        assert_eq!(count(&known, FreeText::Full, "Mirwen Dunhaldt"), 0);
        for name in ["Tamsorin", "Velgrad", "Mirwen", "Ostrel", "Dunhaldt"] {
            assert_eq!(count(&known, FreeText::Tokens, name), 1, "{name}");
        }
    }

    fn columns() -> Columns {
        Columns {
            full: vec![("*.csv".into(), "name".into())],
            email: vec![("*.csv".into(), "email".into())],
            ..Columns::default()
        }
    }

    fn rule() -> Rule {
        Rule {
            files: vec!["*.csv".to_owned()],
            files_excluded: Vec::new(),
            first_name: vec![],
            last_name: vec![],
            full_name: vec!["Name".to_owned()],
            email: vec!["Email".to_owned()],
            sheets: vec![],
        }
    }

    #[test]
    fn header_count_counts_exact_header_texts() {
        let yes = vec!["name".to_owned(), "email".to_owned(), "x".to_owned()];
        let no = vec!["titel".to_owned(), "datum".to_owned()];
        assert_eq!(header_count(&yes, &columns()), 2);
        assert_eq!(header_count(&no, &columns()), 0);
        assert_eq!(header_count(&[], &columns()), 0);
    }

    #[test]
    fn pick_header_takes_the_highest_count_and_detects_ties() {
        assert_eq!(pick_header(&[]), HeaderPick::None);
        assert_eq!(pick_header(&[0, 0]), HeaderPick::None);
        assert_eq!(pick_header(&[1, 2, 1]), HeaderPick::At(1));
        assert_eq!(pick_header(&[2, 1, 2]), HeaderPick::Ambiguous);
        assert_eq!(pick_header(&[1, 1]), HeaderPick::Ambiguous);
        // A tie after the first 20 rows does not count.
        let mut late = vec![0; 25];
        late[3] = 1;
        late[22] = 1;
        assert_eq!(pick_header(&late), HeaderPick::At(3));
    }

    #[test]
    fn failing_rule_names_the_glob_only() {
        let rule = rule();
        let rules = [&rule];
        let columns = collect_columns(&rules);
        let none = vec!["x".to_owned()];
        assert_eq!(
            failing_rule(&rules, Some(&none), &columns).as_deref(),
            Some("*.csv")
        );
        assert_eq!(
            failing_rule(&rules, None, &columns).as_deref(),
            Some("*.csv")
        );
        let some = vec!["x".to_owned(), "name".to_owned()];
        assert_eq!(failing_rule(&rules, Some(&some), &columns), None);
        let empty = Columns::default();
        assert_eq!(failing_rule(&[], None, &empty), None);
    }

    #[test]
    fn header_text_cell_stays_and_others_get_emails_replaced() {
        let regex = text::email_regex().unwrap();
        let fns = CellFns {
            email_regex: &regex,
            names: text::replace_name,
            emails: text::replace_emails,
            matcher: None,
        };
        let mut free = 0;
        let mut replace = |_: Kind, _: &str| "x@y.test".to_owned();
        let cols = columns();
        assert_eq!(
            transform_text_cell(" Email ", &cols, &fns, &mut replace, &mut free),
            " Email "
        );
        assert_eq!(
            transform_text_cell("Liste von a.b@c.test", &cols, &fns, &mut replace, &mut free),
            "Liste von x@y.test"
        );
    }

    #[test]
    fn full_name_without_email() {
        let mut c = Collector::default();
        c.cell(
            Some(ColRole::Name(NameRole::Full)),
            "Ottilie Wenzhart <ottilie.wenzhart@firma.test>",
        );
        c.cell(
            Some(ColRole::Name(NameRole::Full)),
            "Berta Kunzl ottilie.x@firma.test",
        );
        c.end_row();
        let mut known = KnownNames::default();
        c.apply(&mut known);
        assert_eq!(count(&known, FreeText::Full, "Ottilie Wenzhart"), 1);
        assert_eq!(count(&known, FreeText::Full, "Berta Kunzl"), 1);
        for word in ["firma", "test", "x"] {
            assert_eq!(count(&known, FreeText::Tokens, word), 0, "{word}");
        }
    }
}
