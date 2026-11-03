# Feature Delta Spec: Rule file lists and exclusions

**Date:** 2026-11-03
**Status:** active
**Author:** Claude (agent)
**Related:** `docs/adr/0014-copy-excluded-and-other-files-unchanged.md` · `docs/specs/0002-pseudonym-engine-and-csv.md` · `docs/specs/0014-copy-other-files.md` · `docs/specs/0015-header-search-and-fail-closed.md`

---

## 1. Objective & Context

**Problem:** A rule has one `files` glob. The user cannot exclude files from a rule, for example an archive folder or files that are already pseudonymized. One glob for many folders is hard to read.
**Today:** `files` is one string. A CSV or XLSX file that no rule matches gets a copy with emails replaced and a warning.
**After this change:** `files` and `files_excluded` are arrays of globs. pseudofy handles an excluded file as an other file (ADR-0014). This is a breaking change of the config format (release 0.3.0).

## 2. Scope

### Added

- `src/config.rs` — `files_excluded` in each `[[rule]]`: an array of globs. The default is an empty array.
- `src/run.rs` — a rule matches a path if one glob of `files` matches and no glob of `files_excluded` matches.
- `src/run.rs` — a CSV or XLSX file is excluded if one or more rules have a matching `files` glob, and each of these rules has a matching `files_excluded` glob.
- `src/run.rs` — pseudofy handles an excluded file as an other file (ADR-0014). With `copy_other_files = false`, it writes no copy. With `copy_other_files = true`, it copies the file unchanged and prints `<path>: copied unchanged`.
- `src/main.rs` — the summary adds `, excluded: N` if N is more than 0.

### Modified

- `src/config.rs` — `files` is an array of one or more globs. A string gives the error `files must be an array, for example files = ["**/*.csv"]`. An empty array is an error.
- `src/config.rs` — the `init` config uses `files = ["**/*.csv", "**/*.xlsx"]` and shows `# files_excluded = []` with a comment. The comment about the header row describes the search in the first 20 non-empty rows (spec 0015).
- `src/run.rs` — each glob of `files` and `files_excluded` uses the options of today: case is ignored, and `*` does not match `/`.
- `tests/cli.rs`, `tests/csv.rs`, `tests/xlsx.rs`, `tests/xlsx_fidelity.rs`, `tests/free_text.rs` — the configs of the tests use the array form.
- `tests/pseudonym_stability.rs` — only the `files` line of its config uses the array form.
- `Cargo.toml` — version `0.3.0`.
- `README.md` — the rule settings, a recommended pattern, the risk of a wrong glob in `files_excluded` with `copy_other_files = true`, and release notes for 0.3.0 with the migration of `files`.
- `docs/adr/0012-copy-other-project-files-unchanged.md` — status `superseded by ADR-0014`.
- `CLAUDE.md` — the exception names ADR-0014 and excluded files.

### Removed

- The string form of `files`.

### Out of scope (explicitly)

- A global exclusion list outside the rules.
- Negated globs (`!pattern`) inside `files`.

## 3. Architectural Rules & Invariants

- [x] With `copy_other_files = false`, an excluded file produces no file in `--out`.
- [x] A CSV or XLSX file that no rule excludes is never copied unchanged.
- [x] A warning or an error names the relative path or the glob, never a cell value.
- [x] `tests/pseudonym_stability.rs` passes with unchanged golden values and checksums.
- [x] No new crate. No `unwrap()` or `expect()` outside tests.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] `files = ["**/*.csv", "**/*.xlsx"]` matches `a.csv`, `x/a.csv`, and `x/y/a.xlsx`.
- [x] `files_excluded = ["archive/**"]` gives no copy of `archive/a.csv` and the summary `excluded: 1`.
- [x] With `copy_other_files = true`, `archive/a.csv` is copied byte-identical and the line `archive/a.csv: copied unchanged` is printed.
- [x] A file that rule A excludes and rule B matches gets the columns of rule B.
- [x] A file that matches no `files` glob still gets the warning `no rule matches this path` and email replacement.
- [x] `files = "**/*.csv"` fails with the migration error. `files = []` fails.
- [x] An invalid glob in `files_excluded` fails and names the glob.

**Tests to add/update:**

- [x] Unit tests in `src/config.rs` and `src/run.rs`, and tests in `tests/cli.rs`, for the behaviors above.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

A config from 0.2.0 fails with the migration error. The user changes `files = "x"` to `files = ["x"]`.
