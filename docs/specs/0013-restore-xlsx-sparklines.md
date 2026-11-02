# Feature Delta Spec: Restore XLSX sparklines

**Date:** 2026-11-02
**Status:** active
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/adr/0011-restore-dropped-xlsx-parts-from-the-input.md` · `docs/specs/0012-restore-xlsx-external-links.md`

---

## 1. Objective & Context

**Problem:** `umya-spreadsheet` drops the sparkline groups from each sheet of the copy. Requirement 9 needs the file structure in the copy.
**Today:** The copy of `tests/fixtures/fidelity.xlsx` has 0 of the 10 sparklines of the input. The part-name test of spec 0010 does not detect this loss.
**After this change:** The scrub copies the sparkline extension of each input sheet to the same sheet of the copy.

## 2. Scope

### Added

- `src/xlsx_scrub.rs` — a restore step for sparklines (ADR-0011):
  - Find each input sheet with an `<ext>` element whose `uri` is `{05C60535-1F16-4fd2-B633-F4F36F0B64E0}`.
  - Find the sheet of the copy with the same sheet name through `xl/workbook.xml` and its relationships.
  - Add the `<ext>` element to the `<extLst>` element of that sheet. If the sheet has no `<extLst>`, add one as the last child of `<worksheet>`.
  - Declare on the `<ext>` element each namespace prefix that it uses.

### Modified

- `tests/xlsx_fidelity.rs` — compare the number of `<x14:sparkline>` elements of each sheet in the input and the copy.
- `docs/adr/0004-initial-crate-set.md` — remove sparklines from the list of losses.

### Removed

- None

### Out of scope (explicitly)

- Other `extLst` extensions, for example x14 data validations or x14 conditional formatting.
- A change to the sparkline ranges. A range contains only a sheet name and cell references.

## 3. Architectural Rules & Invariants

- [ ] No new crate. The restore step uses `zip` and `quick-xml` (ADR-0010).
- [ ] The scrub copies only the sparkline `<ext>` element from an input sheet.
- [ ] The scrub rebuilds the `<ext>` element from the sparkline elements of ECMA-376 and MS-XLSX: `sparklineGroups`, `sparklineGroup`, the color elements, `sparklines`, `sparkline`, `f`, and `sqref`. It drops each other element, comment, processing instruction, and CDATA section.
- [ ] The scrub reads the namespace of a prefix only from the input `<worksheet>` and `<extLst>` elements.
- [ ] If an input sheet has no sparklines, a fault in another part of the input does not stop the file.
- [ ] If the copy already has a sparkline `<ext>` element in that sheet, the scrub does not add a second one.
- [ ] If the scrub cannot find the sheet of the input or of the copy, pseudofy fails for that file. The error message contains no real value.
- [ ] No `unwrap()` or `expect()` outside tests.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [ ] Each sheet of the copy of `tests/fixtures/fidelity.xlsx` has the same number of sparklines as the input sheet.
- [ ] Each restored sparkline has the same `<xm:f>` and `<xm:sqref>` values as the input.
- [ ] A workbook without sparklines gives the same sheet XML as before this change.

**Tests to add/update:**

- [ ] `tests/xlsx_fidelity.rs` — the sparkline count of each sheet.
- [ ] `tests/xlsx.rs` — the ranges of the restored sparklines.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

**Manual verification (only if unavoidable):**

- Open the copy of `tests/fixtures/fidelity.xlsx` in Excel. The file opens without a repair message. The column Trend shows the sparklines.

## 5. Rollback / Notes (optional)

Start this spec after spec 0012 is merged. This spec uses the input package argument of `scrub` from spec 0012.
