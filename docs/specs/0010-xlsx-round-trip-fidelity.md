# Feature Delta Spec: XLSX round-trip fidelity

**Date:** 2026-11-02
**Status:** done
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/specs/0003-xlsx-support.md` · `docs/adr/0004-initial-crate-set.md` · `docs/adr/0010-scrub-the-written-xlsx-package.md`

---

## 1. Objective & Context

**Problem:** `umya-spreadsheet` writes a new workbook from its own model. It has no model for some workbook parts (ADR-0004). Requirement 9 needs the file structure in the copy.
**Today:** The XLSX tests use workbooks that `umya-spreadsheet` makes. No test uses a workbook that Excel makes.
**After this change:** A test runs pseudofy on an Excel-made workbook. The test fails if the copy loses a part that is not on a documented list.

## 2. Scope

### Added

- `tests/fixtures/fidelity.xlsx` — the user makes this workbook in Excel with invented data only. It has tables, conditional formatting, data validation, a sparkline, a comment, a hidden sheet, a merged cell, and a link to a second workbook.
- `tests/fixtures/fidelity_reference.xlsx` — the second workbook that `fidelity.xlsx` links to. The user makes it in Excel with invented data only.
- `tests/xlsx_fidelity.rs` — run `pseudofy run` on the fixture. Compare the ZIP part names of the input and the copy.

### Modified

- `docs/adr/0004-initial-crate-set.md` — update the list of lost parts with the result of the test.

### Removed

- None

### Out of scope (explicitly)

- A change to `src/xlsx_file.rs`.
- A replacement of `umya-spreadsheet`. If the result needs a replacement, a new ADR and a new spec decide it.
- Document properties and comments that contain real names.

## 3. Architectural Rules & Invariants

- [x] The fixture contains only invented data.
- [x] `tests/xlsx_fidelity.rs` has a constant list of the part names that the copy may lose. Each entry has a comment with the reason.
- [x] The test never compares cell values of name columns. It compares only part names.
- [x] The test adds no crate. It uses `zip`, a runtime crate since ADR-0010.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] Each ZIP part of the fixture exists in the copy, or is on the list of lost parts.
- [x] The test fails with the name of each part that is lost and not on the list.
- [x] ADR-0004 lists the same lost parts as the test.

**Tests to add/update:**

- [x] `tests/xlsx_fidelity.rs` — the behaviors above.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

**Manual verification (only if unavoidable):**

- Open the copy of `tests/fixtures/fidelity.xlsx` in Excel — the file opens without a repair message.
  Result (2026-11-02): Excel shows a repair message. The copy of `tests/fixtures/fidelity_reference.xlsx` opens without one.
  The cause is the lost external link. ADR-0011 and spec 0012 restore it.

## 5. Rollback / Notes (optional)

Start this spec after spec 0003 is merged. The test needs `.xlsx` support in `run`.

If the copy loses a part that users need, or if Excel shows a repair message, write a new ADR.
That ADR replaces `umya-spreadsheet` with a direct XML edit of the workbook with `zip` and `quick-xml`.
