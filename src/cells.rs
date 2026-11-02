//! Column roles, header matching, cell replacement, and the safe output
//! write. `csv_file` and `xlsx_file` use the same functions.

use std::fs;
use std::io::Write;
use std::path::Path;

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
pub(crate) fn rules_without_headers<'a>(
    rules: &[&'a Rule],
    missing: &[(String, String)],
) -> Vec<&'a str> {
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
                        .any(|(glob, miss)| *glob == rule.files && *miss == header)
                })
        })
        .map(|rule| rule.files.as_str())
        .collect()
}

pub(crate) fn is_not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

/// Write `bytes` to `output` without following a symlink: check each directory
/// below `out_root`, refuse a symlink target, write a temp file, rename it.
pub(crate) fn write_output(
    out_root: &Path,
    output: &Path,
    bytes: &[u8],
    rel: &str,
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
    let name = output
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = parent.join(format!(".{name}.pseudofy-tmp"));
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
                target.push((rule.files.clone(), header.trim().to_lowercase()));
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

/// Replace the addresses at `spans`; run the text outside them through `names`.
pub(crate) fn replace_spans(
    cell: &str,
    spans: &[std::ops::Range<usize>],
    names: NameFn,
    name_role: NameRole,
    replace: &mut dyn FnMut(Kind, &str) -> String,
) -> String {
    let mut out = String::with_capacity(cell.len());
    let mut pos = 0;
    for span in spans {
        let before = cell.get(pos..span.start).unwrap_or("");
        out.push_str(&names(before, name_role, &mut |part, token| {
            replace(name_kind(part), token)
        }));
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

/// Known names found in the name columns of one file (pass 1). They reach
/// `KnownNames` only through `apply`, after the whole file was read.
#[derive(Default)]
pub(crate) struct Collector {
    persons: Vec<(String, String)>,
    fulls: Vec<String>,
    first: String,
    last: String,
}

impl Collector {
    /// One data cell. Call `end_row` after the last cell of each row.
    pub(crate) fn cell(&mut self, role: Option<ColRole>, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        match role {
            Some(ColRole::Name(NameRole::First)) if self.first.is_empty() => {
                value.clone_into(&mut self.first);
            }
            Some(ColRole::Name(NameRole::Last)) if self.last.is_empty() => {
                value.clone_into(&mut self.last);
            }
            Some(ColRole::Name(NameRole::Full)) => self.fulls.push(value.to_owned()),
            _ => {}
        }
    }

    /// End of a data row: the first-name and last-name cells form one person.
    pub(crate) fn end_row(&mut self) {
        if !self.first.is_empty() || !self.last.is_empty() {
            self.persons.push((
                std::mem::take(&mut self.first),
                std::mem::take(&mut self.last),
            ));
        }
    }

    pub(crate) fn apply(mut self, known: &mut KnownNames) {
        self.end_row();
        for (first, last) in &self.persons {
            known.add_person(first, last);
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
                replace,
            )
        }
        Some(ColRole::Email) => {
            let spans = text::email_spans(cell, email_regex);
            if !spans.is_empty() {
                return replace_spans(cell, &spans, names, NameRole::Full, replace);
            }
            // No address found: the whole trimmed cell is the address.
            let lead = cell.len() - cell.trim_start().len();
            let tail = cell.len() - cell.trim_end().len();
            let mut out = String::with_capacity(cell.len());
            out.push_str(cell.get(..lead).unwrap_or(""));
            out.push_str(&replace(Kind::Email, trimmed));
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
