# Feature Delta Spec: Known names in free text

**Date:** 2026-11-02
**Status:** done
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
- `src/text.rs` — the curly apostrophe (U+2019) and U+02BC are name characters like `'`. `O’Brien` is one token in name columns and in free text.
- `tests/xlsx.rs` — the `sheets` glob test uses a name that occurs in no name column. Known names now change the other sheets too.

### Removed

- None

### Out of scope (explicitly)

- Names that occur in no name column of the project.
- Name detection with heuristics or language models.
- Formula cells, comments, and document properties.

## 3. Architectural Rules & Invariants

- [x] All rules and invariants of spec 0002, section 3, apply.
- [x] A known name in free text gets the same pseudonym as in its name column.
- [x] The match is whole-word and case-insensitive. The replacement keeps the case style of the matched text.
- [x] `full` replaces only complete full names ("Anna Muster", "Muster, Anna").
- [x] `tokens` also replaces each single first-name and last-name token. A single token matches only if the word in the text is not all lowercase. `Mark` and `MARK` match, `mark` does not. Full-name matches stay case-insensitive.
- [x] `off` replaces no known name outside the name columns. Pass 1 does not run. Emails are still replaced in each cell.
- [x] A longer match has priority over a shorter match.
- [x] An uppercase one-letter initial after a matched name is replaced only if a name column had an initial after the same name. It gets the role it had in that column. Other one-letter words stay.
- [x] Quotes and apostrophes at the start or end of a name stay. `‘Bob’` gives `‘<fake>’` in name columns and in free text.
- [x] Known names stay in memory only. pseudofy never writes them to a file or a log.
- [x] Header rows are not changed.
- [x] If two column spellings fold to one key (`Strauss`, `Strauß`), each spelling in free text gets the pseudonym of the same spelling in its column. The result does not depend on file names or file order.
- [x] A cell segment that contains a match is written in NFC. A segment without a match stays byte-equal.
- [x] A file that fails in pass 1 adds no known names. Pass 2 reports it.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] "Call Anna Muster" becomes "Call `<fake first> <fake last>`". The fake names are equal to those in the name column.
- [x] A known name from a CSV file is replaced in a text cell of an XLSX file in the same project.
- [x] With `free_text = "full"`, "Anna said" stays unchanged. With `tokens`, "Anna" is replaced.
- [x] With `free_text = "off"`, "Anna Muster said hi" in a text cell stays unchanged. An email in that cell is replaced.
- [x] "Annabelle" stays unchanged if only "Anna" is known.
- [x] `run` prints `<rel>: <N> free-text replacements` to stdout for each file with N > 0.

**Tests to add/update:**

- [x] `src/known_names.rs` unit tests — whole-word match, priority, case style.
- [x] `tests/free_text.rs` — the five behaviors above.

**Commands that must pass:**

```sh
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

The mode `tokens` can replace normal words that are also names, for example "Mark" or "Rose" at the start of a sentence. A lowercase word like "mark" is not replaced. A known name written only in lowercase, for example "anna", stays in free text unless it is part of a full name.
If the user sees too many wrong replacements, the user sets `free_text = "full"`.
