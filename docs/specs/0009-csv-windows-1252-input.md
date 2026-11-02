# Feature Delta Spec: CSV files in Windows-1252

**Date:** 2026-11-02
**Status:** done
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/specs/0002-pseudonym-engine-and-csv.md` · `docs/adr/0009-decode-csv-with-encoding-rs.md`

---

## 1. Objective & Context

**Problem:** German Excel writes CSV files in Windows-1252. Requirement 18 needs these files.
**Today:** `run` reports "line N is not valid UTF-8" and skips the file.
**After this change:** `run` reads UTF-8 and Windows-1252 CSV files. Each copy has the encoding of its input file.

## 2. Scope

### Added

- Encoding detection in `src/csv_file.rs`. If the bytes after an optional UTF-8 BOM are valid UTF-8, the file is UTF-8. Else the file is Windows-1252, except in the reject cases below.
- Reject cases: a UTF-8 BOM before invalid UTF-8, a UTF-16 BOM, a NUL byte, or invalid UTF-8 that contains a valid UTF-8 multi-byte sequence. `run` reports the file as failed with `<rel>: unsupported or damaged encoding` and writes no copy.
- `tests/fixtures/windows-1252.csv` — invented names with `ä`, `ö`, `ü`, `ß`, delimiter `;`, line ending `\r\n`.

### Modified

- `src/csv_file.rs` — decode a Windows-1252 file to UTF-8 before the transform. Encode the result to Windows-1252 before the write.
- `src/names.rs` — a unit test makes sure that each name list entry has only Windows-1252 characters.
- `tests/csv.rs` — tests for Windows-1252 files.
- `Cargo.toml` — add `encoding_rs = "=0.8.44"` (ADR-0009).

### Removed

- None

### Out of scope (explicitly)

- UTF-16 files (the Excel export "Unicode Text").
- Other single-byte encodings (ISO-8859-15, Mac Roman).
- A config key for the encoding.
- XLSX files.

## 3. Architectural Rules & Invariants

- [x] All rules and invariants of spec 0002, section 3, apply to Windows-1252 files.
- [x] A UTF-8 file gives the same output as before this change.
- [x] A Windows-1252 copy is Windows-1252. pseudofy never adds a BOM.
- [x] A byte that the transform does not change stays byte-equal in the copy.
- [x] If the encoder cannot encode a pseudonym, `run` reports the file and continues with the other files. The message contains no real value and no pseudonym.
- [x] The only new direct crate is `encoding_rs` (ADR-0009).
- [x] pseudofy never decodes a file in a reject case. It writes no copy of it.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] A Windows-1252 file with `Jörg Müller` in a name column gives a Windows-1252 copy with a pseudonym.
- [x] `Jörg` in a UTF-8 file and in a Windows-1252 file of one project gets the same pseudonym.
- [x] The Windows-1252 copy keeps row count, column count, header bytes, delimiter, and line ending.
- [x] A Windows-1252 file without a value to replace gives a byte-equal copy.
- [x] Each entry of the `de` and `en` name lists encodes to Windows-1252 without loss.
- [x] A UTF-8 file with one invalid byte, a UTF-8 BOM before Windows-1252 text, and a UTF-16 file each give a failed file without a copy. The message contains no real value.

**Tests to add/update:**

- [x] `tests/csv.rs` — all behaviors above, except the name list check.
- [x] `src/names.rs` unit test — the name list check.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

A Windows-1252 file with a byte pair like `Ã©` looks like mixed UTF-8. pseudofy refuses it. Such pairs are rare in real text.

A Windows-1252 file that is also valid UTF-8 is read as UTF-8. Only ASCII text and rare byte sequences are valid in both. For ASCII text, the output is equal in both encodings.
