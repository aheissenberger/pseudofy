# Feature Delta Spec: XLSX input hardening

**Date:** 2026-11-02
**Status:** active
**Author:** Claude (agent)
**Related:** `docs/specs/0003-xlsx-support.md` · `docs/adr/0010-scrub-the-written-xlsx-package.md` · `docs/adr/0011-restore-dropped-xlsx-parts-from-the-input.md`

---

## 1. Objective & Context

**Problem:** A review found these faults with hostile or damaged input:
- `umya-spreadsheet` finds parts by relationship and without case. pseudofy checks exact part names. A pivot cache named `xl/PivotCache/…` gets past the refusal, and its real values reach the copy. The warnings for comments and headers miss parts with other names.
- A relationship cycle makes `umya-spreadsheet` overflow the stack. The whole run aborts.
- A relationship fan-out runs without end.
- A small zip bomb needs gigabytes of memory.
- Two loops are quadratic: trailing initials in `src/known_names.rs`, and external references in `src/xlsx_scrub.rs`.
- A CSV needs about eight times its size in memory.

**Today:** `has_pivot_parts` and `inspect` compare exact names. No check limits relationships, part sizes, or file sizes.
**After this change:** pseudofy checks the input package before `umya-spreadsheet` reads it. It runs the warnings on the written package, where part names are canonical. It refuses a file above the limits.

## 2. Scope

### Added

- `src/xlsx_scrub.rs` — a package check before `umya-spreadsheet`:
  - Compare part names after this normalization: `\` becomes `/`, ASCII lowercase.
  - Refuse a pivot part by normalized name, by relationship type, or by content type.
  - Follow the relationships from `xl/_rels/workbook.xml.rels` with the rules of `umya-spreadsheet` (path join, case-insensitive lookup). Refuse a cycle, a depth above 16, or more than 10 000 relationships.
  - Refuse more than 10 000 zip entries, or a total uncompressed size above 512 MiB. Count a part once for each reference to it. Read each part with a limit, so a wrong declared size does not get past the check.
- `src/xlsx_scrub.rs` — a canonical package for `umya-spreadsheet` (the check and the crate must see the same package):
  - Refuse an entry whose raw name does not find itself (two names that decode to the same text), a name with `\\` or a leading `/`, and two names that are equal after ASCII lowercase.
  - Parse each `.rels` part with exact attribute names. Accept only unprefixed `Relationship` elements. Refuse a duplicate attribute, a duplicate `Id`, a relationship without `Id`, `Type`, or `Target`, an entity in `TargetMode`, and `&`, `<`, `"`, or a control character in a non-External `Target` or an `Id`. Write each `.rels` part again in a canonical form (only `Id`, `Type`, `Target`, `TargetMode`) before `umya-spreadsheet` reads the package.
  - Refuse two `<sheet>` elements with the same `r:id` in `xl/workbook.xml`, and more than 4 096 `<sheet>` elements. Read a sheet id with the rule of `umya-spreadsheet` (exact `r:id` first).
  - Refuse `>` in a non-External `Target` or an `Id` (the crate reads attribute values without unescape).
  - Estimate the memory that the crate builds: the length of the shared string of each `t="s"` cell, the font, fill, border, and number format of each cell format, and the cell format of each cell, row, and column with an `s` attribute. Refuse more than 1 GiB. Refuse a shared string longer than 32 767 characters, a font name longer than 31 characters, and a number format longer than 255 characters (the limits of Excel).
  - Count each case-insensitive lookup that `umya-spreadsheet` makes, also for each External relationship. Refuse more than 10 000 000 name comparisons (about 4 seconds).
- `src/xlsx_scrub.rs` — the external-link restore reads each link part once.
- `src/xlsx_file.rs` — refuse an XLSX file larger than 512 MiB: `<path>: file is larger than 512 MiB`.
- `src/xlsx_scrub.rs` — run the warning scan (comments, charts, headers and footers, data validations, conditional formatting) on the package that `umya-spreadsheet` wrote. Keep the checks for data loss (prefixed shared strings, multi-run inline strings) on the input, with normalized names.
- `src/csv_file.rs` — refuse a CSV file above 256 MiB: `<path>: file is larger than 256 MiB`.

### Modified

- `src/xlsx_file.rs` — call the package check. Remove `has_pivot_parts` or make it use the package check.
- `src/known_names.rs` — keep a set of token positions with an edit (no linear scan per match).
- `src/xlsx_scrub.rs` — a map from relationship id to target in the external-link restore and in `sheet_parts`.
- `tests/xlsx.rs`, `tests/csv.rs` — the tests of section 4.
- `README.md` — the size limits.

### Removed

- None

### Out of scope (explicitly)

- Streaming CSV processing.
- A config setting for the limits.

## 3. Architectural Rules & Invariants

- [ ] A refused file gets no copy. The run continues with the next file.
- [ ] No hostile workbook aborts the process. Each limit is checked before `umya-spreadsheet` reads the package.
- [ ] An error message contains the path and the reason, never part contents.
- [ ] No new crate. No `unwrap()` or `expect()` outside tests.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [ ] A workbook with pivot parts under `xl/PivotCache/` or `xl\pivotCache\` is refused.
- [ ] A worksheet header with a name under `xl/Worksheets/` gets the warning `headers/footers`.
- [ ] A relationship cycle makes the file fail. A second, valid file in the same project gets its copy.
- [ ] A fan-out chain of 40 parts fails within 5 seconds.
- [ ] A package with a total uncompressed size above 512 MiB fails without a large memory use.
- [ ] 100 000 repeats of a name with a trailing initial take less than 1 second.
- [ ] Each hostile package of the review (2026-11-03) fails without an abort: External self-loops through `/_rels/.rels`, an entity in `TargetMode` or `Target`, a prefixed `Target` attribute, two entry names that decode to the same text, 300 sheets with one `r:id`, 10 000 external references to one part.
- [ ] A drawing with 2 500 images gets a copy.
- [ ] A cycle hidden behind `>` in a `Target`, one 4 MiB shared string in 2 000 cells, one 4 MiB font name in 2 000 cell formats, and 100 000 `<sheet>` elements each fail without an abort and within 10 seconds.
- [ ] A package with a 600 MiB entry fails at once.

**Tests to add/update:**

- [ ] `tests/xlsx.rs`, `tests/csv.rs`, and unit tests in `src/xlsx_scrub.rs` and `src/known_names.rs` — the behaviors above. Tests build the hostile packages in memory with `zip`.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

The limits are constants. A later spec can make them config settings.

`umya-spreadsheet` can need about 12 times the uncompressed size of the worksheets in memory. The README says so.
