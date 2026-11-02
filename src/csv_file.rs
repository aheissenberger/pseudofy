//! Pseudonymize one CSV file.

use std::fs;
use std::path::Path;

use anyhow::{Context, anyhow};
use csv::{ByteRecord, ReaderBuilder, Terminator, WriterBuilder};
use regex::Regex;

use crate::config::Rule;
use crate::pseudonym::Pseudonymizer;
use crate::text::{self, NameRole, Part};

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Name replacement function (the signature of `text::replace_name`).
type NameFn = fn(&str, NameRole, &mut dyn FnMut(Part, &str) -> String) -> String;
/// Email replacement function (the signature of `text::replace_emails`).
type EmailFn = fn(&str, &Regex, &mut dyn FnMut(&str) -> String) -> String;

/// Kind of value that the replace callback gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    First,
    Last,
    Email,
}

/// Header texts (trimmed, lowercase), each with the glob of its rule.
#[derive(Debug, Default)]
struct Columns {
    first: Vec<(String, String)>,
    last: Vec<(String, String)>,
    full: Vec<(String, String)>,
    email: Vec<(String, String)>,
}

/// Result of the transformation of one file.
struct Transformed {
    bytes: Vec<u8>,
    replaced: usize,
    /// (rule glob, header text) pairs that the file does not have.
    missing: Vec<(String, String)>,
}

/// Read `input`, replace names and emails, write the copy to `output`.
/// `rel` is the relative path for messages. `rules` are the rules that match
/// the file. Returns the number of replaced values.
pub fn process(
    input: &Path,
    output: &Path,
    rel: &str,
    rules: &[&Rule],
    pseudonymizer: &mut Pseudonymizer,
    email_regex: &Regex,
) -> anyhow::Result<usize> {
    let bytes = fs::read(input).with_context(|| format!("cannot read {rel}"))?;
    let columns = collect_columns(rules);
    let mut replace = |kind: Kind, value: &str| match kind {
        Kind::First => pseudonymizer.first_name(value),
        Kind::Last => pseudonymizer.last_name(value),
        Kind::Email => pseudonymizer.email(value),
    };
    let result = transform(
        &bytes,
        &columns,
        email_regex,
        text::replace_name,
        text::replace_emails,
        &mut replace,
        rel,
    )?;
    for (glob, header) in &result.missing {
        eprintln!(
            "warning: rule \"{glob}\" names the header \"{header}\", which {rel} does not have"
        );
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).with_context(|| format!("cannot create directory for {rel}"))?;
    }
    fs::write(output, &result.bytes).with_context(|| format!("cannot write {rel}"))?;
    Ok(result.replaced)
}

fn collect_columns(rules: &[&Rule]) -> Columns {
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

fn transform(
    input: &[u8],
    columns: &Columns,
    email_regex: &Regex,
    names: NameFn,
    emails: EmailFn,
    replace: &mut dyn FnMut(Kind, &str) -> String,
    rel: &str,
) -> anyhow::Result<Transformed> {
    let (bom, body) = match input.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, input),
    };
    let header_line = body.split(|&b| b == b'\n').next().unwrap_or(&[]);
    let crlf = body.contains(&b'\n') && header_line.ends_with(b"\r");
    let delimiter = detect_delimiter(header_line);

    let mut reader = ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(body);
    let terminator = if crlf {
        Terminator::CRLF
    } else {
        Terminator::Any(b'\n')
    };
    let mut writer = WriterBuilder::new()
        .delimiter(delimiter)
        .terminator(terminator)
        .flexible(true)
        .from_writer(Vec::new());

    let mut roles: Vec<Option<NameRole>> = Vec::new();
    let mut missing = Vec::new();
    let mut replaced = 0_usize;
    let mut record = ByteRecord::new();
    let mut row = 0_usize;
    loop {
        let more = reader
            .read_byte_record(&mut record)
            .map_err(|_| anyhow!("{rel}: cannot parse row {}", row + 1))?;
        if !more {
            break;
        }
        row += 1;
        if row == 1 {
            let headers = header_texts(&record, rel)?;
            roles = headers.iter().map(|h| role_of(h, columns)).collect();
            missing = missing_headers(&headers, columns);
            writer
                .write_byte_record(&record)
                .map_err(|_| anyhow!("{rel}: cannot write row 1"))?;
            continue;
        }
        let mut out = ByteRecord::new();
        for (index, field) in record.iter().enumerate() {
            let cell = std::str::from_utf8(field)
                .map_err(|_| anyhow!("{rel}: row {row} is not valid UTF-8"))?;
            let mut value = cell.to_owned();
            if !value.trim().is_empty() {
                if let Some(Some(role)) = roles.get(index) {
                    value = names(&value, *role, &mut |part, token| {
                        replaced += 1;
                        match part {
                            Part::First => replace(Kind::First, token),
                            Part::Last => replace(Kind::Last, token),
                        }
                    });
                }
                value = emails(&value, email_regex, &mut |address| {
                    replaced += 1;
                    replace(Kind::Email, address)
                });
            }
            out.push_field(value.as_bytes());
        }
        writer
            .write_byte_record(&out)
            .map_err(|_| anyhow!("{rel}: cannot write row {row}"))?;
    }

    let body_out = writer
        .into_inner()
        .map_err(|_| anyhow!("{rel}: cannot write the output"))?;
    let mut bytes = Vec::with_capacity(body_out.len() + BOM.len());
    if bom {
        bytes.extend_from_slice(BOM);
    }
    bytes.extend_from_slice(&body_out);
    Ok(Transformed {
        bytes,
        replaced,
        missing,
    })
}

fn header_texts(record: &ByteRecord, rel: &str) -> anyhow::Result<Vec<String>> {
    record
        .iter()
        .map(|field| {
            std::str::from_utf8(field)
                .map(|text| text.trim().to_lowercase())
                .map_err(|_| anyhow!("{rel}: header is not valid UTF-8"))
        })
        .collect()
}

fn role_of(header: &str, columns: &Columns) -> Option<NameRole> {
    let has = |list: &[(String, String)]| list.iter().any(|(_, text)| text == header);
    if has(&columns.first) {
        Some(NameRole::First)
    } else if has(&columns.last) {
        Some(NameRole::Last)
    } else if has(&columns.full) {
        Some(NameRole::Full)
    } else {
        None
    }
}

fn missing_headers(headers: &[String], columns: &Columns) -> Vec<(String, String)> {
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

    fn fake_names(
        value: &str,
        _role: NameRole,
        replace: &mut dyn FnMut(Part, &str) -> String,
    ) -> String {
        value
            .split(' ')
            .map(|token| replace(Part::First, token))
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

    fn run(input: &[u8], columns: &Columns) -> Transformed {
        let regex = Regex::new(r"[a-z0-9._-]+@[a-z0-9.-]+\.[a-z]+").unwrap();
        transform(
            input,
            columns,
            &regex,
            fake_names,
            fake_emails,
            &mut |kind, _value| match kind {
                Kind::First | Kind::Last => "N".to_owned(),
                Kind::Email => "x@example.com".to_owned(),
            },
            "t.csv",
        )
        .unwrap()
    }

    #[test]
    fn delimiter_detection() {
        assert_eq!(detect_delimiter(b"a;b;c"), b';');
        assert_eq!(detect_delimiter(b"a\tb\tc"), b'\t');
        assert_eq!(detect_delimiter(b"a,b;c"), b',');
        assert_eq!(detect_delimiter(b"abc"), b',');
        assert_eq!(detect_delimiter(b"\"a;b;c\",d"), b',');
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
    fn replaces_names_and_emails_and_reports_missing() {
        let input = b"Name , note\nab cd,mail a@b.test now\n";
        let out = run(input, &columns());
        assert_eq!(out.replaced, 3);
        assert_eq!(
            String::from_utf8(out.bytes).unwrap(),
            "Name , note\nN N,mail x@example.com now\n"
        );
        let heads: Vec<_> = out.missing.iter().map(|(_, h)| h.as_str()).collect();
        assert_eq!(heads, ["first_name", "email"]);
    }
}
