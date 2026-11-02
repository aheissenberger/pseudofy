# Feature Delta Spec: XLSX support

**Date:** 2026-11-02
**Status:** done
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/specs/0002-pseudonym-engine-and-csv.md` · `docs/adr/0004-initial-crate-set.md` · `docs/adr/0010-scrub-the-written-xlsx-package.md`

---

## 1. Objective & Context

**Problem:** Projects also contain XLSX files with real names and email addresses.
**Today:** `pseudofy run` processes only CSV files.
**After this change:** `pseudofy run` also writes pseudonymized copies of XLSX files. pseudofy processes each sheet of a workbook. The copies keep sheets and styles.

## 2. Scope

### Added

- `src/xlsx_file.rs` — open a workbook with `umya-spreadsheet`, replace cell values, save the copy.
- `src/xlsx_scrub.rs` — blank each unused entry of `xl/sharedStrings.xml` in the written package (ADR-0010).
- `src/panic_guard.rs` — the panic hook, and a guard that keeps the hook silent for a panic that pseudofy catches.
- `src/cells.rs` — column roles, header matching, cell replacement, and the safe output write. `src/csv_file.rs` and `src/xlsx_file.rs` use it.
- `tests/xlsx.rs` — XLSX tests. The tests create the input workbooks in a temporary directory.

### Modified

- `src/run.rs` — select `.xlsx` files and call `src/xlsx_file.rs`.
- `src/csv_file.rs` — move the shared code to `src/cells.rs`.
- `src/main.rs` — install the panic hook from `src/panic_guard.rs`. The hook prints only the code location, never the panic message. A crate can panic on a damaged workbook.
- `src/config.rs` — a rule gets the optional field `sheets` (list of sheet-name globs, for example `"Kunden*"`). Without `sheets`, the rule applies to each sheet.
- `Cargo.toml` — add `umya-spreadsheet`, `zip`, and `quick-xml` (ADR-0010).
- `README.md` — state which XLSX parts pseudofy does not pseudonymize.

### Removed

- None

### Out of scope (explicitly)

- `.xls`, `.xlsm`, `.ods` files.
- Formulas, comments, document properties (author, last modified by), defined names, charts. pseudofy keeps each formula but clears its cached string result.
- Data validations, headers and footers, conditional formatting, sheet names. pseudofy copies them unchanged.
- Text in a column that no rule names, except email addresses.
- Pivot tables. pseudofy refuses a workbook with a pivot cache and reports it as a failed file.
- Rich-text runs inside one cell. pseudofy writes the cell as plain text and reports it.
- Workbooks that the XLSX crate cannot read without data loss: a shared-string table with a namespace prefix, and inline strings with more than one run. pseudofy refuses them and reports a failed file.
- Hyperlinks with a cell range reference. The XLSX crate fails on them. pseudofy reports a failed file.

## 3. Architectural Rules & Invariants

- [x] All rules and invariants of spec 0002, section 3, apply to XLSX files.
- [x] pseudofy processes each sheet, also hidden sheets.
- [x] Each sheet has its own header row: the first non-empty row of that sheet.
- [x] A rule column that a sheet does not have is skipped for that sheet, without an error.
- [x] One real value gets one pseudonym in all sheets and all files.
- [x] pseudofy changes only string cells. Number, date, boolean, and formula cells stay unchanged.
- [x] No stale copy of a replaced cell value stays in the output package. This includes `xl/sharedStrings.xml`, cached formula results, and `mailto:` hyperlinks.
- [x] pseudofy prints one warning for each workbook with comments, charts, data validations, headers or footers, or conditional formatting. The warning names the kinds, never their content.
- [x] pseudofy clears each hyperlink tooltip. In a `mailto:` URL, pseudofy decodes percent escapes, replaces the address, and removes the query part.
- [x] The copy keeps sheet names, sheet order, cell styles, column widths, and merged cells.
- [x] `src/xlsx_file.rs` and `src/csv_file.rs` use the same replacement function from `src/cells.rs`.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] A name in a CSV file and in an XLSX file of one project gets the same pseudonym.
- [x] A workbook with three sheets gives a copy with three sheets in the same order and with the same names.
- [x] Two sheets with different header rows get their names replaced in the correct columns.
- [x] A name in sheet 1 and in sheet 2 gets the same pseudonym.
- [x] A rule with `sheets = ["Kunden*"]` changes the sheet `Kunden 2025` and does not change the sheet `Notizen`.
- [x] A hidden sheet is processed and stays hidden.
- [x] A bold cell stays bold after the replacement.
- [x] A formula cell keeps its formula. `run` reports the formula cell count for each file.
- [x] A damaged `.xlsx` file makes `run` report the file and continue with the other files. The exit code is non-zero.
- [x] A `.xlsx` file that makes the XLSX crate panic gives one error line without a real value.
- [x] A workbook with a pivot cache is reported as a failed file. pseudofy writes no copy of it.
- [x] A `mailto:` hyperlink of a replaced email cell gets the fake address.
- [x] Column widths, merged cells, and rich-text cells survive a run. `run` reports the rich-text cell count.

**Tests to add/update:**

- [x] `tests/xlsx.rs` — all behaviors above.

**Commands that must pass:**

```sh
devcontainer exec --workspace-folder . ./scripts/check
```

**Manual verification (only if unavoidable):**

- Open one output workbook in Excel or Numbers — the file opens without a repair message.

## 5. Rollback / Notes (optional)

Document properties can contain real names. A later spec can clear them.
