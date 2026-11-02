# Feature Delta Spec: Known names in free text

**Date:** 2026-11-02
**Status:** active
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 (11) · `docs/specs/0002-pseudonym-engine-and-csv.md` · `docs/specs/0003-xlsx-support.md` · `docs/adr/0003-derive-pseudonyms-with-a-keyed-hash.md`

---

## 1. Objective & Context

**Problem:** Text cells outside the name columns can contain real names, for example "Call Anna Muster".
**Today:** `pseudofy run` replaces names only in name columns.
**After this change:** `pseudofy run` also replaces each known name in all other text cells of the project.

## 2. Scope

### Added

- `src/known_names.rs` — collect the known names from all name columns of all project files. Build one whole-word matcher.
- `tests/free_text.rs` — CSV and XLSX tests for known names in text cells.

### Modified

- `src/run.rs` — two passes. Pass 1 collects the known names from all files. Pass 2 writes the copies.
- `src/config.rs` — the optional key `free_text` with the values `off`, `full`, and `tokens`. The default is `tokens`.
- `src/csv_file.rs`, `src/xlsx_file.rs` — pass 1 reads the name columns of each file. Pass 2 gives the matcher to the cell replacement.
- `src/cells.rs` — replace known names in each text cell outside the name and email columns, after the emails.
- `src/config.rs` template from `pseudofy init` — a commented `free_text` line.
- `tests/xlsx.rs` — the `sheets` glob test uses a name that occurs in no name column. Known names now change the other sheets too.

### Removed

- None

### Out of scope (explicitly)

- Names that occur in no name column of the project.
- Name detection with heuristics or language models.
- Formula cells, comments, and document properties.

## 3. Architectural Rules & Invariants

- [ ] All rules and invariants of spec 0002, section 3, apply.
- [ ] A known name in free text gets the same pseudonym as in its name column.
- [ ] The match is whole-word and case-insensitive. The replacement keeps the case style of the matched text.
- [ ] `full` replaces only complete full names ("Anna Muster", "Muster, Anna").
- [ ] `tokens` also replaces each single first-name and last-name token.
- [ ] `off` replaces no known name outside the name columns. Pass 1 does not run. Emails are still replaced in each cell.
- [ ] A longer match has priority over a shorter match.
- [ ] Known names stay in memory only. pseudofy never writes them to a file or a log.
- [ ] Header rows are not changed.
- [ ] A file that fails in pass 1 adds no known names. Pass 2 reports it.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [ ] "Call Anna Muster" becomes "Call `<fake first> <fake last>`". The fake names are equal to those in the name column.
- [ ] A known name from a CSV file is replaced in a text cell of an XLSX file in the same project.
- [ ] With `free_text = "full"`, "Anna said" stays unchanged. With `tokens`, "Anna" is replaced.
- [ ] With `free_text = "off"`, "Anna Muster said hi" in a text cell stays unchanged. An email in that cell is replaced.
- [ ] "Annabelle" stays unchanged if only "Anna" is known.
- [ ] `run` prints `<rel>: <N> free-text replacements` to stdout for each file with N > 0.

**Tests to add/update:**

- [ ] `src/known_names.rs` unit tests — whole-word match, priority, case style.
- [ ] `tests/free_text.rs` — the five behaviors above.

**Commands that must pass:**

```sh
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

The mode `tokens` can replace normal words that are also names, for example "Mark" or "Rose".
If the user sees too many wrong replacements, the user sets `free_text = "full"`.
