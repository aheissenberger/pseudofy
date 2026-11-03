# Feature Delta Spec: XLSX warnings and hyperlinks

**Date:** 2026-11-02
**Status:** active
**Author:** Claude (agent)
**Related:** `docs/specs/0003-xlsx-support.md` · `docs/specs/0017-xlsx-input-hardening.md` · `docs/adr/0012-copy-other-project-files-unchanged.md`

---

## 1. Objective & Context

**Problem:** A review found XLSX text that pseudofy keeps with no warning:
- formula text, for example `="Max "&"Muster"`;
- text in drawing shapes and text boxes;
- names in hyperlink targets;
- table total labels;
- number formats with literal text;
- embedded objects.

The README does not list these parts. It also does not explain the limits of the free-text replacement.
**Today:** `inspect` warns for comments, charts, data validations, headers and footers, and conditional formatting. A plain stdout line reports formula cells. Hyperlink targets get email replacement only.
**After this change:** pseudofy warns for each part that can keep a real value. It replaces names in hyperlink targets. The README lists each part that pseudofy does not change.

## 2. Scope

### Added

- `src/xlsx_scrub.rs` — the warning scan also reports these kinds:
  - `formulas` — a cell has a formula.
  - `drawings` — a drawing part has text (`a:t`), or a shape or picture has alt text (`descr`, `title`) or a name other than the default.
  - `tables` — a table has a total label, a total formula, or a column name that is not a configured header text.
  - `number formats` — a number format has literal text (`"…"`, `\`, or `[$…]`).
  - `embedded objects` — the package has an embedded object.
- `src/xlsx_file.rs` — hyperlink targets:
  - `mailto:` — replace the display name before the address with the pseudonym of the name (full-name role). Replace the address as today.
  - Other URLs — percent-decode the target, replace emails and known names as in free text (spec 0004), and encode the replaced parts again. Keep each unchanged byte. In the query, `+` is a space. An invalid escape stays as it is and does not stop the decode of the other bytes. `\` in a `file:` link stays.
  - A link to a place in the workbook (`location`) stays unchanged. It names a sheet, and sheet names stay unchanged.
  - A `mailto:` with more than one display name gets each name replaced on its own.

### Modified

- `src/run.rs` — the formula line becomes `warning: <path>: N formula cells unchanged`.
- `tests/xlsx.rs` — the tests of section 4.
- `README.md` — list each XLSX part that pseudofy does not change. Explain the limits of the free-text replacement: names in no name column, all-lowercase names, spelling variants.

### Removed

- None

### Out of scope (explicitly)

- Replacement of names inside formulas, drawings, tables, number formats, or embedded objects.

## 3. Architectural Rules & Invariants

- [ ] A warning names the kind, never the text.
- [ ] A hyperlink target in the copy is a valid URL.
- [ ] No new crate. No `unwrap()` or `expect()` outside tests.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [ ] A workbook with `="Tamsorin "&"Velgrad"` gets the warning `formulas`.
- [ ] A workbook with a text box gets the warning `drawings`.
- [ ] A table with `totalsRowLabel` gets the warning `tables`.
- [ ] A number format `"Quenzel-Format "0` gets the warning `number formats`.
- [ ] `mailto:"Zorbina Quenzel" <zorbina.quenzel@acme.test>` gets a fake display name and a fake address.
- [ ] `https://intranet.acme.test/people/zorbina-quenzel?u=Zorbina%20Quenzel` gets the known name replaced in the query.
- [ ] `?u=Zorbina+Quenzel` with `free_text = "full"` gets the name replaced. `%2B` stays `%2B`.
- [ ] A `location` link `'Zorbina Quenzel'!A1` stays unchanged.
- [ ] A picture with `descr` and a table with a column name that is not a header text get a warning.

**Tests to add/update:**

- [ ] `tests/xlsx.rs` — the behaviors above. Tests build the parts in memory with `zip`.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

A lowercase name in a host name (`zorbina.quenzel.example.org`) is not replaced (spec 0004: all-lowercase words). The README names this limit.
