//! Pseudonymize one CSV file.

use std::fs;
use std::path::Path;

use anyhow::{Context, anyhow};
use csv::{ByteRecord, ReaderBuilder, Terminator, WriterBuilder};
use encoding_rs::WINDOWS_1252;
use regex::Regex;

use crate::cells::{
    ColRole, Columns, EmailFn, Kind, NameFn, collect_columns, missing_headers, role_of,
    rules_without_headers, transform_cell, write_output,
};
use crate::config::Rule;
use crate::pseudonym::Pseudonymizer;
use crate::text;

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Result of the transformation of one file.
struct Transformed {
    bytes: Vec<u8>,
    replaced: usize,
    /// (rule glob, header text) pairs that the file does not have.
    missing: Vec<(String, String)>,
}

/// Read `input`, replace names and emails, write the copy to `output`.
/// `out_root` is the output directory, `rel` the relative path for messages.
/// `rules` are the rules that match the file. Returns the number of replaced
/// values.
pub fn process(
    input: &Path,
    out_root: &Path,
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
    let result = transform_encoded(
        &bytes,
        &columns,
        email_regex,
        text::replace_name,
        text::replace_emails,
        &mut replace,
        rel,
    )?;
    for glob in rules_without_headers(rules, &result.missing) {
        eprintln!("warning: rule \"{glob}\" matches {rel}, but the file has none of its headers");
    }
    write_output(out_root, output, &result.bytes, rel)?;
    Ok(result.replaced)
}

/// Transform a UTF-8 or Windows-1252 file. The copy has the encoding of the input.
fn transform_encoded(
    input: &[u8],
    columns: &Columns,
    email_regex: &Regex,
    names: NameFn,
    emails: EmailFn,
    replace: &mut dyn FnMut(Kind, &str) -> String,
    rel: &str,
) -> anyhow::Result<Transformed> {
    let damaged = || anyhow!("{rel}: unsupported or damaged encoding");
    if input.starts_with(b"\xFF\xFE") || input.starts_with(b"\xFE\xFF") || input.contains(&0) {
        return Err(damaged());
    }
    let rest = input.strip_prefix(BOM);
    if std::str::from_utf8(rest.unwrap_or(input)).is_ok() {
        return transform(input, columns, email_regex, names, emails, replace, rel);
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
        email_regex,
        names,
        emails,
        replace,
        rel,
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
    let (header_line, line_end) = first_record(body);
    let delimiter = detect_delimiter(header_line);
    let crlf = line_end == b"\r\n";
    let ends_with_newline = body.ends_with(b"\n") || body.ends_with(b"\r");

    let mut reader = ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(body);
    let terminator = if crlf {
        Terminator::CRLF
    } else if line_end == b"\r" {
        Terminator::Any(b'\r')
    } else {
        Terminator::Any(b'\n')
    };
    let mut writer_builder = WriterBuilder::new();
    writer_builder
        .delimiter(delimiter)
        .terminator(terminator)
        .flexible(true);
    let mut body_out: Vec<u8> = Vec::new();

    let mut roles: Vec<Option<ColRole>> = Vec::new();
    let mut missing = Vec::new();
    let mut replaced = 0_usize;
    let mut record = ByteRecord::new();
    let cells = CellFns {
        email_regex,
        names,
        emails,
    };
    let mut header_bytes: &[u8] = &[];
    let mut first = true;
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
        prev_end = end;
        if first {
            first = false;
            header_bytes = body.get(..end).unwrap_or(&[]);
            let headers = header_texts(&record, rel)?;
            roles = headers.iter().map(|h| role_of(h, columns)).collect();
            missing = missing_headers(&headers, columns);
            continue;
        }
        for _ in 0..blanks {
            body_out.extend_from_slice(line_end);
        }
        let out = transform_record(&record, &roles, &cells, &mut |kind, value| {
            replaced += 1;
            replace(kind, value)
        })
        .ok_or_else(|| anyhow!("{rel}: line {line} is not valid UTF-8"))?;
        render(&writer_builder, &out, &mut body_out)
            .ok_or_else(|| anyhow!("{rel}: cannot write line {line}"))?;
    }
    if ends_with_newline && !first {
        let rest = body.get(prev_end..).unwrap_or(&[]);
        for _ in 0..count_breaks(rest) {
            body_out.extend_from_slice(line_end);
        }
    }

    let mut bytes = Vec::with_capacity(body_out.len() + header_bytes.len() + BOM.len());
    if bom {
        bytes.extend_from_slice(BOM);
    }
    bytes.extend_from_slice(header_bytes);
    bytes.extend_from_slice(&body_out);
    if !ends_with_newline && bytes.ends_with(line_end) {
        bytes.truncate(bytes.len() - line_end.len());
    }
    Ok(Transformed {
        bytes,
        replaced,
        missing,
    })
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

/// What `transform_cell` needs besides the cell.
struct CellFns<'a> {
    email_regex: &'a Regex,
    names: NameFn,
    emails: EmailFn,
}

/// Transform each field of a record. `None` if a field is not valid UTF-8.
fn transform_record(
    record: &ByteRecord,
    roles: &[Option<ColRole>],
    cells: &CellFns,
    replace: &mut dyn FnMut(Kind, &str) -> String,
) -> Option<ByteRecord> {
    let mut out = ByteRecord::new();
    for (index, field) in record.iter().enumerate() {
        let cell = std::str::from_utf8(field).ok()?;
        let role = roles.get(index).copied().flatten();
        let value = transform_cell(
            cell,
            role,
            cells.email_regex,
            cells.names,
            cells.emails,
            replace,
        );
        out.push_field(value.as_bytes());
    }
    Some(out)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{NameRole, Part};

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

    fn text(out: Transformed) -> String {
        String::from_utf8(out.bytes).unwrap()
    }

    fn run_enc(input: &[u8], columns: &Columns) -> anyhow::Result<Transformed> {
        let regex = text::email_regex().unwrap();
        transform_encoded(
            input,
            columns,
            &regex,
            text::replace_name,
            text::replace_emails,
            &mut |kind, _| match kind {
                Kind::First => "Berta".to_owned(),
                Kind::Last => "Muster".to_owned(),
                Kind::Email => "f@x.test".to_owned(),
            },
            "t.csv",
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
            &regex,
            text::replace_name,
            text::replace_emails,
            &mut |_, _| "\u{141}".to_owned(),
            "t.csv",
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
    fn warns_only_for_a_rule_without_any_header() {
        let partial = rule("a/*.csv", &["First_Name"], &["email"]);
        let none = rule("b/*.csv", &["vorname"], &[]);
        let empty = rule("c/*.csv", &[], &[]);
        let missing = vec![
            ("a/*.csv".to_string(), "email".to_string()),
            ("b/*.csv".to_string(), "vorname".to_string()),
        ];
        assert_eq!(
            rules_without_headers(&[&partial, &none, &empty], &missing),
            ["b/*.csv"]
        );
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
            &regex,
            fake_names,
            fake_emails,
            &mut |_, _| String::new(),
            "t.csv",
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
            &regex,
            text::replace_name,
            text::replace_emails,
            &mut |kind, value| match kind {
                Kind::First => "Berta".to_owned(),
                Kind::Last => "Muster".to_owned(),
                Kind::Email => format!("f{}@x.test", value.len()),
            },
            "t.csv",
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
}
