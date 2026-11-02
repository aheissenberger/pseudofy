# Feature Delta Spec: Rust CLI scaffold and devcontainer

**Date:** 2026-11-02
**Status:** done
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2, §4 · `docs/adr/0001-use-rust-for-the-cli.md` · `docs/adr/0002-develop-in-a-devcontainer.md` · `docs/adr/0004-initial-crate-set.md`

---

## 1. Objective & Context

**Problem:** pseudofy has no code and no development environment.
**Today:** The repository contains only `docs/` and `scripts/docs-check`.
**After this change:** The devcontainer builds. A `pseudofy` binary prints help and its version. One script runs all checks.

## 2. Scope

### Added

- `.devcontainer/devcontainer.json` — Rust image with a pinned tag, VS Code extensions `rust-analyzer`, `tamasfe.even-better-toml`, and `anthropic.claude-code`. A named volume keeps `~/.claude` across container rebuilds.
- `rust-toolchain.toml` — one pinned stable Rust version, components `rustfmt` and `clippy`.
- `Cargo.toml`, `Cargo.lock` — binary crate `pseudofy`, edition 2024, `license = "Apache-2.0"`, dependency `clap` only.
- `LICENSE` — the full Apache License 2.0 text from apache.org, unchanged.
- `src/main.rs` — `clap` parser with `--help` and `--version`. Subcommands `init` and `run` exist and exit with code 2 and the message "not implemented".
- `tests/cli.rs` — CLI tests for `--help`, `--version`, and the two subcommands.
- `scripts/check` — runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `./scripts/docs-check`.
- `.gitignore` — `target/`, `secrets/`, `.private_*`, `out/`.
- `README.md` — purpose, devcontainer start, check command, license section.

### Modified

- None

### Removed

- None

### Out of scope (explicitly)

- Pseudonymization logic, config file, CSV and XLSX handling (specs 0002 and 0003).
- Release builds and CI pipeline (spec 0005).

## 3. Architectural Rules & Invariants

- [x] `src/main.rs` contains only argument parsing and dispatch.
- [x] This change adds only `clap` (runtime) and `assert_cmd`, `predicates` (test). ADR-0004 covers them.
- [x] The devcontainer image tag and the Rust version are pinned. No `latest` tag.
- [x] `scripts/check` exits with a non-zero code if one check fails.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] `pseudofy --version` prints `pseudofy 0.1.0`.
- [x] `pseudofy --help` lists the subcommands `init` and `run`.
- [x] `pseudofy run` exits with code 2 and prints "not implemented" to stderr.
- [x] `pseudofy` with an unknown subcommand exits with a non-zero code.

**Tests to add/update:**

- [x] `tests/cli.rs` — the four behaviors above.

**Commands that must pass:**

```sh
devcontainer up --workspace-folder .
devcontainer exec --workspace-folder . ./scripts/check
```

**Manual verification (only if unavoidable):**

- Open the folder in VS Code and select "Reopen in Container" — `rust-analyzer` shows no errors in `src/main.rs`.

## 5. Rollback / Notes (optional)

The host needs Docker and `npm install -g @devcontainers/cli` for the command-line check.
