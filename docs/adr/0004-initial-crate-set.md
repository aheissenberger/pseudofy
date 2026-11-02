# ADR-0004: Initial crate set

**Date:** 2026-11-02
**Status:** proposed
**Deciders:** Andreas Heissenberger
**Related:** `docs/specs/0001-rust-cli-scaffold-and-devcontainer.md` · `docs/specs/0002-pseudonym-engine-and-csv.md` · `docs/specs/0003-xlsx-support.md` · `docs/specs/0004-known-names-in-free-text.md`

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
`Cargo.lock` is committed. Each version is pinned in `Cargo.toml`.

## Consequences

### Positive

- Each crate is widely used and maintained.
- `umya-spreadsheet` edits cells in place and keeps the workbook layout.

### Negative / Accepted costs

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
