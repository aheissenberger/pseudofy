# Feature Delta Spec: Output and key safety

**Date:** 2026-11-02
**Status:** active
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 (requirement 8) · `docs/specs/0002-pseudonym-engine-and-csv.md` · `docs/adr/0012-copy-other-project-files-unchanged.md`

---

## 1. Objective & Context

**Problem:** A review found these faults:
- `--out` can be an existing directory with input files inside the project. pseudofy then overwrites input files (requirement 8).
- Through a symlink or another letter case, `--out` can point into `secrets/`. The project key can then be overwritten.
- A failed key write leaves an empty key file.
- A FIFO in place of the key or the config blocks `run`.
- CSV and XLSX files in hidden directories are processed.
- CSV and XLSX copies lose the permission bits of the input.
- A long file name fails because of the temp name.
- The output tree has a race window if another user can write to it.

**Today:** `resolve_dirs` in `src/run.rs` compares paths that are not canonical and are case-sensitive. `src/cells.rs` `write_output` uses the temp name `.<name>.pseudofy-tmp`.
**After this change:** pseudofy never writes into a project directory with input files. It never writes into `secrets/` or `config/`. It writes the key safely. It reads only regular files of a limited size for the key and the config.

## 2. Scope

### Added

- `src/run.rs` — the output check:
  - If `--out` is inside the project, it must not exist, or it must be empty, or it must contain the marker file `.pseudofy-out`. pseudofy writes the marker when it creates the output directory. Only a regular file with the marker text counts.
  - pseudofy creates a new output directory with mode 0700 (unix).
  - Before each write, pseudofy refuses an output path that is the same file (device and inode) as a walked input file.
  - pseudofy refuses an output directory that is, or is inside, the canonical `secrets/` or `config/` of the project. It compares canonical paths and the device and inode of each ancestor, so a symlink and another letter case do not get past the check. It checks again after it creates the directory.
  - pseudofy warns if an existing output directory is writable by other users (`mode & 0o022`).
- `src/run.rs` — the walk skips each path with a component that starts with `.` (also for CSV and XLSX files).

### Modified

- `src/key.rs` — write the key to a temp file with a random name in `secrets/`, call `sync_all`, then link it to `pseudofy.key` with `fs::hard_link` (fails if the key exists), then remove the temp file. If the file system has no hard links, write `pseudofy.key` with `create_new`. If a step fails, remove the files that this call made. Read the key only from a regular file, at most 1 KiB.
- `src/config.rs` — read the config only from a regular file, at most 1 MiB.
- `src/cells.rs` — `write_output`: use a short fixed temp name `.pseudofy-out.tmp`, and set the permission bits of the input file on the copy.
- `tests/cli.rs` — the tests of section 4. `tests/csv.rs` counts the marker file in an output directory inside the project.
- `README.md` — the rules for `--out`, hidden files, and the output permissions.

### Removed

- None

### Out of scope (explicitly)

- Directory-handle based writes (`openat`). They need a new crate (ADR-0004). The warning and the README cover the race window.

## 3. Architectural Rules & Invariants

- [ ] pseudofy never changes an input file (requirement 8).
- [ ] pseudofy never writes into `secrets/` or `config/` of the project.
- [ ] An error message contains a path, never file contents.
- [ ] No new crate. No `unwrap()` or `expect()` outside tests.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [ ] `--out P/data` fails if `P/data` has a file and no marker. No input file changes.
- [ ] A second run to the same output directory inside the project works (marker).
- [ ] `--out` through a symlink to `P/secrets` fails. `--out P/Secrets` fails if `P/secrets` exists.
- [ ] A failed key write leaves no key file. A second `init` then works.
- [ ] A FIFO at `secrets/pseudofy.key` makes `run` fail at once.
- [ ] `P/.hidden/a.csv` gets no copy.
- [ ] A CSV copy of an input with mode 0600 has mode 0600.
- [ ] A CSV with a 245-character name gets a copy.

**Tests to add/update:**

- [ ] `tests/cli.rs` and unit tests in `src/run.rs`, `src/key.rs`, `src/config.rs`, `src/cells.rs` — the behaviors above.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

An existing output directory inside the project without the marker now fails. Delete it or add the marker file.
