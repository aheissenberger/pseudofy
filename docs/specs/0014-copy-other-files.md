# Feature Delta Spec: Copy other files and explain each config setting

**Date:** 2026-11-02
**Status:** done
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/adr/0012-copy-other-project-files-unchanged.md`

---

## 1. Objective & Context

**Problem:** The output directory has only the CSV and XLSX copies. The user copies each other project file by hand. The config that `init` writes does not explain each setting.
**Today:** `pseudofy run` ignores each file that is not a `.csv` or `.xlsx` file.
**After this change:** If the config setting `copy_other_files` is `true`, `pseudofy run` copies each other file unchanged to the same relative path in the output directory. The default is `false`.
The config that `init` writes explains each setting. Each setting with a default value is commented out and shows that default.

## 2. Scope

### Added

- `src/config.rs` — the setting `copy_other_files` (bool, default `false`).
- `src/config.rs` — a new config template for `init`:
  - A comment above each setting explains it and each permitted value.
  - `language` is active. `init` sets it.
  - `email_subdomains`, `free_text`, and `copy_other_files` are commented out with their default values.
  - The example `[[rule]]` is active. A comment explains each rule field: `files`, `first_name`, `last_name`, `full_name`, `email`, `sheets`.
  - `sheets` is commented out with its default.
- `src/run.rs` — the copy step (ADR-0012):
  - If `copy_other_files` is `true`, copy each other file of the walk to the same relative path in the output directory.
  - Skip a symlink, an Excel lock file (`~$*`), and a file with a path component that starts with `.`.
  - Print `<path>: copied unchanged` for each copied file.
  - If a copy fails, print an error with the path and count the file as failed.
- `tests/cli.rs` — the tests of section 4.

### Modified

- `tests/cli.rs` — the `init` test expects `# email_subdomains = true`.
- `src/main.rs` — if `copy_other_files` is `true`, the summary line gets `, copied unchanged: N`.
- `README.md` — describe the setting. Tell the user to check each copied file before sharing.
- `docs/plan/pseudonymization.md` — add a requirement: a config setting makes `run` copy each other file unchanged.

### Removed

- None

### Out of scope (explicitly)

- A list of file extensions to copy or to skip.
- Empty directories.
- A check of the content of a copied file.

## 3. Architectural Rules & Invariants

- [x] The walk keeps its exclusions: `secrets/`, `config/`, and the output directory. The names `secrets` and `config` are compared without case, because macOS file systems ignore case by default.
- [x] A copy keeps the permission bits of the input file.
- [x] pseudofy never copies a `.csv` or `.xlsx` file unchanged. The extension check ignores case.
- [x] pseudofy never copies a file with a path component that starts with `.`, for example `.private_key` or `.git/config`.
- [x] The copy uses the same safe write as the other outputs: no write through a symlink in the output directory.
- [x] The copy streams the file. It does not read the full file into memory.
- [x] No new crate. No `unwrap()` or `expect()` outside tests.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] Without the setting, `run` copies no other file. The summary line is the same as before this change.
- [x] With `copy_other_files = true`, `run` copies `notes.txt` and `sub/image.png` byte-identical to the output directory.
- [x] With `copy_other_files = true`, `run` does not copy a file in `secrets/` or `config/`, a `.private_x` file, a file in `.git/`, `.DS_Store`, `~$book.xlsx`, or a symlink.
- [x] With `copy_other_files = true`, `run` prints `notes.txt: copied unchanged`. The summary line gives the number of copied files.
- [x] `init` writes `# copy_other_files = false`, `# email_subdomains = true`, and `# free_text = "tokens"`.
- [x] The config that `init` writes gives the same settings as the defaults, with the example rule.
- [x] The config that `init` writes names each field of `Config` and `Rule`.

**Tests to add/update:**

- [x] `tests/cli.rs` — the behaviors above.
- [x] `src/config.rs` — the default and the parse of `copy_other_files`.
- [x] `src/config.rs` — the template parses to the defaults and names each field.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

Without the setting, `run` behaves as before this change.
