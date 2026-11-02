# Feature Delta Spec: XLSX support

**Date:** 2026-11-02
**Status:** active
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/specs/0002-pseudonym-engine-and-csv.md` · `docs/adr/0004-initial-crate-set.md` · `docs/adr/0009-scrub-the-written-xlsx-package.md`

---

## 1. Objective & Context

**Problem:** Projects also contain XLSX files with real names and email addresses.
**Today:** `pseudofy run` processes only CSV files.
**After this change:** `pseudofy run` also writes pseudonymized copies of XLSX files. pseudofy processes each sheet of a workbook. The copies keep sheets and styles.

## 2. Scope

### Added

- `src/xlsx_file.rs` — open a workbook with `umya-spreadsheet`, replace cell values, save the copy.
- `src/xlsx_scrub.rs` — blank each unused entry of `xl/sharedStrings.xml` in the written package (ADR-0009).
- `src/panic_guard.rs` — the panic hook, and a guard that keeps the hook silent for a panic that pseudofy catches.
- `src/cells.rs` — column roles, header matching, cell replacement, and the safe output write. `src/csv_file.rs` and `src/xlsx_file.rs` use it.
- `tests/xlsx.rs` — XLSX tests. The tests create the input workbooks in a temporary directory.

### Modified

- `src/run.rs` — select `.xlsx` files and call `src/xlsx_file.rs`.
- `src/csv_file.rs` — move the shared code to `src/cells.rs`.
- `src/main.rs` — install the panic hook from `src/panic_guard.rs`. The hook prints only the code location, never the panic message. A crate can panic on a damaged workbook.
- `src/config.rs` — a rule gets the optional field `sheets` (list of sheet-name globs, for example `"Kunden*"`). Without `sheets`, the rule applies to each sheet.
- `Cargo.toml` — add `umya-spreadsheet`, `zip`, and `quick-xml` (ADR-0009).

### Removed

- None

### Out of scope (explicitly)

- `.xls`, `.xlsm`, `.ods` files.
- Formulas, comments, document properties (author, last modified by), defined names, charts. pseudofy keeps each formula but clears its cached string result.
- Pivot tables. pseudofy refuses a workbook with a pivot cache and reports it as a failed file.
- Rich-text runs inside one cell. pseudofy writes the cell as plain text and reports it.

## 3. Architectural Rules & Invariants

- [ ] All rules and invariants of spec 0002, section 3, apply to XLSX files.
- [ ] pseudofy processes each sheet, also hidden sheets.
- [ ] Each sheet has its own header row: the first non-empty row of that sheet.
- [ ] A rule column that a sheet does not have is skipped for that sheet, without an error.
- [ ] One real value gets one pseudonym in all sheets and all files.
- [ ] pseudofy changes only string cells. Number, date, boolean, and formula cells stay unchanged.
- [ ] No part of the output package contains a real value that pseudofy replaced. This includes `xl/sharedStrings.xml`, cached formula results, and `mailto:` hyperlinks.
- [ ] The copy keeps sheet names, sheet order, cell styles, column widths, and merged cells.
- [ ] `src/xlsx_file.rs` and `src/csv_file.rs` use the same replacement function from `src/cells.rs`.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [ ] A name in a CSV file and in an XLSX file of one project gets the same pseudonym.
- [ ] A workbook with three sheets gives a copy with three sheets in the same order and with the same names.
- [ ] Two sheets with different header rows get their names replaced in the correct columns.
- [ ] A name in sheet 1 and in sheet 2 gets the same pseudonym.
- [ ] A rule with `sheets = ["Kunden*"]` changes the sheet `Kunden 2025` and does not change the sheet `Notizen`.
- [ ] A hidden sheet is processed and stays hidden.
- [ ] A bold cell stays bold after the replacement.
- [ ] A formula cell keeps its formula. `run` reports the formula cell count for each file.
- [ ] A damaged `.xlsx` file makes `run` report the file and continue with the other files. The exit code is non-zero.
- [ ] A `.xlsx` file that makes the XLSX crate panic gives one error line without a real value.
- [ ] A workbook with a pivot cache is reported as a failed file. pseudofy writes no copy of it.
- [ ] A `mailto:` hyperlink of a replaced email cell gets the fake address.
- [ ] Column widths, merged cells, and rich-text cells survive a run. `run` reports the rich-text cell count.

**Tests to add/update:**

- [ ] `tests/xlsx.rs` — all behaviors above.

**Commands that must pass:**

```sh
devcontainer exec --workspace-folder . ./scripts/check
```

**Manual verification (only if unavoidable):**

- Open one output workbook in Excel or Numbers — the file opens without a repair message.

## 5. Rollback / Notes (optional)

Document properties can contain real names. A later spec can clear them.
