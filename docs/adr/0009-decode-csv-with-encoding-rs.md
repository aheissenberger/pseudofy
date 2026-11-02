# ADR-0009: Decode CSV with encoding_rs

**Date:** 2026-11-02
**Status:** proposed
**Deciders:** Andreas Heissenberger
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/specs/0009-csv-windows-1252-input.md` · `docs/adr/0004-initial-crate-set.md`

## Context

German Excel writes "CSV (Trennzeichen-getrennt)" files in Windows-1252 with the delimiter `;`.
Today `run` stops on each line that is not valid UTF-8.
Requirement 18 adds Windows-1252 input. The copy must keep the encoding of the input file.
ADR-0004 does not permit a new runtime crate without an ADR.

## Decision

We will decode and encode Windows-1252 with `encoding_rs`, pinned to `=0.8.44`.

`encoding_rs` is already in `Cargo.lock` as a dependency of `umya-spreadsheet`.
The direct dependency adds no new crate to the build.

## Consequences

### Positive

- `encoding_rs` implements the WHATWG Encoding Standard. Its Windows-1252 decoder and encoder map all 256 bytes in both directions.
- Unchanged bytes of a Windows-1252 file stay byte-equal in the copy.
- The build gets no new crate.

### Negative / Accepted costs

- `Cargo.toml` gets one more direct runtime crate.
- A pseudonym character that Windows-1252 cannot encode stops the file. The name lists must contain only Windows-1252 characters.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Own Windows-1252 table in `src/csv_file.rs` | `encoding_rs` is already in the build and is tested. An own table adds code to maintain. |
| `encoding` | Unmaintained. |
| `chardetng` for encoding detection | A new crate. A UTF-8 validity test is sufficient for two encodings. |
| A config key for the encoding | One project can contain files in both encodings. |

## Compliance & verification (optional)

Spec 0009 tests a Windows-1252 file end to end.
