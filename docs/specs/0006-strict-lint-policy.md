# Feature Delta Spec: Strict lint policy

**Date:** 2026-11-02
**Status:** done
**Author:** Claude (agent)
**Related:** `docs/adr/0006-strict-lint-policy-in-cargo-toml.md` · `docs/specs/0001-rust-cli-scaffold-and-devcontainer.md` · `docs/specs/0002-pseudonym-engine-and-csv.md` §3

---

## 1. Objective & Context

**Problem:** Clippy uses only its default lints. A panic from `unwrap()` or `expect()` can print a real value to stderr. Spec 0002 forbids this.
**Today:** `scripts/check` runs `cargo clippy --all-targets -- -D warnings`. `Cargo.toml` has no lint table. rust-analyzer and a plain `cargo clippy` run see fewer rules than the check.
**After this change:** `Cargo.toml` defines the lint policy from ADR-0006. Each `cargo` command in `scripts/check` uses `--locked`. The current code passes all checks.

## 2. Scope

### Added

- `clippy.toml` — `allow-unwrap-in-tests = true`, `allow-expect-in-tests = true`.

### Modified

- `Cargo.toml` — add `rust-version = "1.99"`.
- `Cargo.toml` — add `[lints.rust]` with `unsafe_code = "forbid"`.
- `Cargo.toml` — add `[lints.clippy]` with `pedantic` at `warn` (priority `-1`), and `unwrap_used`, `expect_used`, `dbg_macro`, `allow_attributes`, `allow_attributes_without_reason` at `warn`.
- `scripts/check` — add `--locked` to `cargo clippy` and `cargo test`.
- `tests/cli.rs` — replace `Command::cargo_bin("pseudofy").unwrap()` with `assert_cmd::cargo::cargo_bin_cmd!("pseudofy")`.

### Removed

- None

### Out of scope (explicitly)

- The `print_stdout` and `print_stderr` lints. Spec 0002 decides about one output module.
- `#[expect]` entries for single pedantic lints. Add one only when code triggers it, with `reason = "..."`.
- Claude Code hooks and a `CLAUDE.md` file for the project.
- CI integration (spec 0005).

## 3. Architectural Rules & Invariants

- [x] This change adds no crate.
- [x] Lint levels are only in `Cargo.toml` and `clippy.toml`. `scripts/check` adds only `-D warnings`.
- [x] No `#[allow]` or `#[expect]` attribute is added to pass the check.
- [x] The CLI behavior from spec 0001 does not change.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] `./scripts/check` passes on the current code.
- [x] An `unwrap()` call in `src/` makes `./scripts/check` fail.
- [x] An `unsafe` block in `src/` makes `./scripts/check` fail.
- [x] If `Cargo.lock` does not match `Cargo.toml`, `./scripts/check` fails.
- [x] An `#[allow]` attribute, or an `#[expect]` attribute without `reason`, in `src/` makes `./scripts/check` fail.

**Tests to add/update:**

- [x] `tests/cli.rs` — the four tests from spec 0001 pass without `unwrap()`.

**Commands that must pass:**

```sh
devcontainer exec --workspace-folder . ./scripts/check
```

**Manual verification (only if unavoidable):**

- Add `let _ = "1".parse::<u8>().unwrap();` to `main()` — `./scripts/check` fails with `clippy::unwrap_used`. Remove the line.
- Add `unsafe {}` to `main()` — `./scripts/check` fails with `unsafe_code`. Remove the line.

## 5. Rollback / Notes (optional)

A trial run on a copy of the code passed with this policy. `pedantic` gave no warning on the current code.
