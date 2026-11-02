# Feature Delta Spec: Restore XLSX external links

**Date:** 2026-11-02
**Status:** done
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/adr/0011-restore-dropped-xlsx-parts-from-the-input.md` · `docs/adr/0010-scrub-the-written-xlsx-package.md` · `docs/specs/0010-xlsx-round-trip-fidelity.md`

---

## 1. Objective & Context

**Problem:** `umya-spreadsheet` drops external links from the copy. A formula like `[1]!ContactsTable[...]` then points to nothing. Requirement 9 needs the file structure in the copy.
**Today:** `tests/xlsx_fidelity.rs` lists `xl/externalLinks/externalLink1.xml` and its relationships part as lost.
**After this change:** The scrub copies each external link from the input to the copy. The copy has no cached value of a linked workbook.

## 2. Scope

### Added

- `src/xlsx_scrub.rs` — a restore step for external links (ADR-0011):
  - Copy each `xl/externalLinks/externalLinkN.xml` part and its `_rels` part from the input.
  - Remove all content of each `<sheetData>` element and each `<values>` element (DDE links) of a restored external link part.
  - Add an `<externalReferences>` element to `xl/workbook.xml`. Keep the order of the input, because a formula uses the position `[N]`.
  - Add one relationship for each link to `xl/_rels/workbook.xml.rels`. Use relationship ids that the copy does not use.
  - Add one `<Override>` for each link to `[Content_Types].xml`.

### Modified

- `src/xlsx_scrub.rs` — `scrub` gets the input package as a second argument.
- `src/xlsx_file.rs` — pass the input package to `scrub`. `open_book` returns the bytes that it read.
- `tests/xlsx_fidelity.rs` — remove the two external link parts from the list of lost parts.
- `tests/xlsx.rs` — add the tests of section 4.
- `docs/adr/0004-initial-crate-set.md` — remove external links from the list of lost parts.

### Removed

- None

### Out of scope (explicitly)

- Sparklines (spec 0013).
- A change to the target path of an external link (ADR-0011).
- Sheet names and defined names inside an external link part.
- Pseudonymization of cached values. The restore step removes them.

## 3. Architectural Rules & Invariants

- [x] No new crate. The restore step uses `zip` and `quick-xml` (ADR-0010).
- [x] The scrub copies only the parts that section 2 names from the input.
- [x] No restored part contains a `<cell>`, `<v>`, or `<values>` element.
- [x] The scrub uses the input bytes that pseudofy inspected and read. It does not read the input file again.
- [x] If the copy already has an `<externalReferences>` element, the scrub restores no external link.
- [x] If the scrub cannot restore an external link, pseudofy fails for that file. The error message contains no real value.
- [x] No `unwrap()` or `expect()` outside tests.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] The copy of `tests/fixtures/fidelity.xlsx` has each external link part of the input.
- [x] `xl/workbook.xml` of the copy has `<externalReferences>` with one entry for each link, in the order of the input.
- [x] Each new relationship of `xl/workbook.xml.rels` points to its link part. No relationship id occurs twice.
- [x] `[Content_Types].xml` of the copy has an `<Override>` for each link part.
- [x] A cached cell with an invented name in a link part of the input does not occur in any part of the copy.
- [x] A workbook without external links gives the same parts as before this change.

**Tests to add/update:**

- [x] `tests/xlsx_fidelity.rs` — the list of lost parts has only `xl/calcChain.xml`.
- [x] `tests/xlsx.rs` — the test adds a cached cell with an invented name to a copy of the fixture, runs pseudofy, and searches each part of the copy.
- [x] `tests/xlsx.rs` — the workbook, relationship, and content-type behaviors above.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

**Manual verification (only if unavoidable):**

- Open the copy of `tests/fixtures/fidelity.xlsx` in Excel. The file opens without a repair message. The formula in E13 refers to `fidelity_reference.xlsx`.
  Result (2026-11-02): Excel opens the copy without a repair message. E13 shows an invalid-reference error.
  E13 uses a structured reference to a table of the linked workbook. Excel resolves it only while that workbook is open.
  The input shows its cached result. The copy has no cached result (ADR-0011).

## 5. Rollback / Notes (optional)

Start this spec after spec 0010 is merged. Spec 0013 uses the input package argument of `scrub` from this spec.
