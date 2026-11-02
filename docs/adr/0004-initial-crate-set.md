# ADR-0004: Initial crate set

**Date:** 2026-11-02
**Status:** proposed
**Deciders:** Andreas Heissenberger
**Related:** `docs/specs/0001-rust-cli-scaffold-and-devcontainer.md` · `docs/specs/0002-pseudonym-engine-and-csv.md` · `docs/specs/0003-xlsx-support.md` · `docs/specs/0004-known-names-in-free-text.md` · `docs/specs/0010-xlsx-round-trip-fidelity.md` · `docs/adr/0010-scrub-the-written-xlsx-package.md`

## Context

pseudofy must parse arguments, read config, read and write CSV and XLSX, and compute HMAC values.
The XLSX output must keep sheets, styles, and column widths.
`calamine` only reads XLSX. `rust_xlsxwriter` only writes XLSX. Neither keeps the styles of an existing file.

## Decision

We will use these crates and no other runtime crates without a new ADR:

| Crate | Purpose |
| --- | --- |
| `clap` (derive) | Command-line arguments |
| `serde`, `toml` | Config file `config/pseudofy.toml` |
| `csv` | CSV read and write |
| `umya-spreadsheet` | XLSX read and write with styles kept |
| `hmac`, `sha2` | Keyed hash (ADR-0003) |
| `getrandom` | Project key generation |
| `regex` | Email detection and whole-word name search |
| `sys-locale` | System language for `pseudofy init` |
| `unicode-normalization` | NFC normalization of real values |
| `globset`, `walkdir` | File selection in the project |
| `anyhow`, `thiserror` | Error handling |

Test-only crates: `assert_cmd`, `predicates`, `tempfile`.
ADR-0010 adds `zip` and `quick-xml` as runtime crates. Spec 0010 also uses `zip` in a test.
`Cargo.lock` is committed. Each version is pinned in `Cargo.toml`.

## Consequences

### Positive

- Each crate is widely used and maintained.
- `umya-spreadsheet` keeps styles, column widths, merged cells, hidden sheets, comments, data validation, conditional formatting, pivot tables, and VBA.

### Negative / Accepted costs

- `umya-spreadsheet` does not edit cells in place. It reads the full workbook into its own model and writes a new file from the model.
- `umya-spreadsheet` 3.1.1 has no model for external links, `customXml`, sparklines, timelines, query tables, and `richData`. The copy can lose these parts.
- `umya-spreadsheet` can panic on damaged input. pseudofy catches the panic (spec 0003). A release profile must not set `panic = "abort"`.
- `umya-spreadsheet` adds approximately 89 crates to the build.
- `umya-spreadsheet` is slower than `calamine` and uses more memory on large workbooks.
- Fifteen direct runtime crates increase compile time.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| `calamine` plus `rust_xlsxwriter` | The output loses styles, formulas, and column widths. |
| Own XLSX XML editing with `zip` and `quick-xml` | High effort. Shared strings and rich text are error-prone. |
| `fake` for names | Output can change between releases (ADR-0003). |

## Compliance & verification (optional)

Review each change to `Cargo.toml`. A new runtime crate needs a new ADR.

Spec 0010 compares the parts of an Excel-made workbook with the parts of its copy.
If the copy loses a part that users need, or if Excel shows a repair message, write a new ADR.
That ADR replaces `umya-spreadsheet` with a direct XML edit of the workbook with `zip` and `quick-xml`.
