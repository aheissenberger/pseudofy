# ADR-0010: Scrub the written XLSX package

**Date:** 2026-11-02
**Status:** proposed
**Deciders:** Andreas Heissenberger
**Related:** `docs/specs/0003-xlsx-support.md` · `docs/adr/0004-initial-crate-set.md`

## Context

`umya-spreadsheet` 3.1.1 keeps the shared-string table of the input workbook.
A changed cell gets a new entry. The old entry stays in `xl/sharedStrings.xml`.
Thus each replaced name and email address stays in the output file.
The crate has no public function that clears or rebuilds the table.
An XLSX file is a ZIP package of XML parts. `zip` and `quick-xml` are already in `Cargo.lock` through `umya-spreadsheet`.

## Decision

We will scrub the XLSX package after `umya-spreadsheet` writes it, before pseudofy writes the file to `--out`.

- The scrub reads the package with `zip` and the XML parts with `quick-xml`.
- The scrub collects each shared-string index that a cell of a worksheet uses.
- The scrub replaces each other `<si>` entry with an empty entry. Each index stays valid.
- The scrub copies all other parts unchanged.
- `Cargo.toml` adds `zip` and `quick-xml` with the versions and features that `umya-spreadsheet` uses. Each version is pinned with `=`.

## Consequences

### Positive

- No replaced value stays in the shared-string table.
- No new crate enters `Cargo.lock`.
- A test can read each part of the output package and search it for real values.

### Negative / Accepted costs

- pseudofy reads and writes each XLSX package twice.
- The scrub depends on the part names and the XML structure of the shared-string table (ECMA-376).
- A version change of `umya-spreadsheet` can change the versions of `zip` and `quick-xml`. Both pins then need an update.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Copy the sheets into a new `umya-spreadsheet` workbook | Workbook parts like the theme, defined names, and workbook views can change or get lost. Excel can show a repair message. |
| Patch or fork `umya-spreadsheet` | A fork needs maintenance for each release. |
| Accept the old entries | The output contains real values. This breaks the main rule of pseudofy. |

## Compliance & verification (optional)

A test unzips each output workbook and searches each part for the real values of the input.
