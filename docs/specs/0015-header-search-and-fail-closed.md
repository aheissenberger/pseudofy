# Feature Delta Spec: Header search and fail closed

**Date:** 2026-11-02
**Status:** active
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/specs/0002-pseudonym-engine-and-csv.md` · `docs/specs/0003-xlsx-support.md` · `docs/specs/0004-known-names-in-free-text.md`

---

## 1. Objective & Context

**Problem:** A review found silent leaks. A CSV without a header line keeps all names. A sheet without headers keeps all names, and its warning disappears if another sheet has the headers. A title row above the header hides each name column. The header line is never pseudonymized. A CSV with an unclosed quote is copied with its real values.
**Today:** The header is the first record of a CSV and the first non-empty row of a sheet. If a rule finds none of its headers, pseudofy warns and writes the copy.
**After this change:** pseudofy searches the header. If a rule matches a file or a sheet and finds none of its headers, the file fails and pseudofy writes no copy. Each row above the header and the header itself get email and known-name replacement.

## 2. Scope

### Added

- Header search: among the first 20 non-empty rows (CSV records, sheet rows), the header is the row with the most cells that are exactly a header text of a rule that applies. If two rows have this highest count, the file fails: `<path>: header row is ambiguous` (XLSX: `<path>: sheet <n>: header row is ambiguous`). Each row above the header is a title row.
- CSV delimiter: pseudofy tries `,`, `;`, and tab. It takes the delimiter whose header row has the most header texts.
- Fail closed:
  - CSV: if a rule matches the file and the file has none of the header texts of that rule, the file fails: `<path>: no header of rule "<glob>" in the first 20 rows`.
  - XLSX: the same check runs for each sheet that a rule applies to (after the `sheets` globs). The message names the sheet: `<path>: sheet <n>: no header of rule "<glob>" in the first 20 rows`. `<n>` is the sheet position (1-based), not the sheet name.
  - XLSX: if a rule matches the file and its `sheets` globs match no sheet, the file fails: `<path>: rule "<glob>" matches no sheet`.
  - If a file fails, pseudofy writes no copy of the file. The run continues with the next file and exits with an error.
- Title rows and the header: pseudofy replaces each email and each known name in them (as in free text, spec 0004). A header text that is a configured header stays unchanged.
- CSV: if a quoted field is still open at the end of the file, the file fails: `<path>: unclosed quote`. An escaped quote `""` inside a quoted field does not close the field.
- CSV: a changed record keeps the bytes of its unchanged fields, also their quotes. Only a changed field is written again.

### Modified

- `src/csv_file.rs`, `src/xlsx_file.rs`, `src/cells.rs` — header search, fail closed, replacement in title rows and the header.
- `src/run.rs` — no change to the warning for a file that no rule matches. That file still gets email replacement in each row, now also in the first row.
- `tests/csv.rs`, `tests/xlsx.rs`, `tests/xlsx_fidelity.rs` — the tests of section 4. `tests/xlsx_fidelity.rs` uses the header texts of the fixture (`First Name`, `Last Name`, `Email`) and checks that the copy has no first name of the input.
- `README.md` — describe the header search and the fail-closed rule.

### Removed

- The warning `rule "<glob>" matches <path>, but the file has none of its headers`. The error replaces it.

### Out of scope (explicitly)

- More than 20 rows for the search, or a config setting for the header row.
- Files that no rule matches.

## 3. Architectural Rules & Invariants

- [ ] A failed file has no copy in the output directory. A stale copy from an earlier run stays as it was.
- [ ] No error message contains a cell value or a header text. A message contains the path, the sheet position, and the rule glob only.
- [ ] Data rows below the header are processed as before.
- [ ] A CSV keeps its bytes outside the changed fields: delimiter, quoting, line ends, encoding, BOM.
- [ ] No new crate. No `unwrap()` or `expect()` outside tests.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [ ] A CSV `Zorbina Quenzel,zorbina.quenzel@acme.test` without a header fails. The output has no copy.
- [ ] A CSV with a title line `Kontakte von zorbina.quenzel@acme.test` above the header `name,email` gets a copy. The title line has a fake email. Each name below the header is replaced.
- [ ] A workbook with two sheets fails if sheet 2 has no header. The message names `sheet 2`.
- [ ] A workbook with `sheets = ["Daten"]` and an info sheet without headers gets a copy.
- [ ] A sheet with a title in row 1 and the header in row 2 gets its names replaced. The title text gets email and known-name replacement.
- [ ] An email in a CSV header line or in a sheet header row is replaced.
- [ ] A CSV with an unclosed quote fails with `unclosed quote`, also when the open field contains `""`. A valid field `"x"" y,"` does not fail.
- [ ] A title line `Export der Felder Name, Email` above `name,email` does not hide the name column. Two rows with the same highest header count make the file fail.
- [ ] A semicolon file with the title line `Liste: Name, Email` uses `;`.
- [ ] A rule with `sheets = ["Kunden"]` on a workbook without that sheet makes the file fail.
- [ ] A changed header line keeps the quotes of its unchanged fields.
- [ ] The copy of `tests/fixtures/fidelity.xlsx` contains no first name of the input.

**Tests to add/update:**

- [ ] `tests/csv.rs`, `tests/xlsx.rs`, `tests/xlsx_fidelity.rs` — the behaviors above.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

This change breaks runs that worked before: a file without headers now fails. The README tells the user to limit a rule with `files` or `sheets`.
