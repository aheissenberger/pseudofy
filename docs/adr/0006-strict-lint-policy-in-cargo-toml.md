# ADR-0006: Strict lint policy in Cargo.toml

**Date:** 2026-11-02
**Status:** proposed
**Deciders:** Andreas Heissenberger
**Related:** `docs/specs/0006-strict-lint-policy.md` · `docs/adr/0001-use-rust-for-the-cli.md` · `docs/specs/0002-pseudonym-engine-and-csv.md` §3

## Context

pseudofy reads real person names and email addresses.
Spec 0002 forbids pseudofy to print, log, or write a real value.
A panic from `unwrap()` or `expect()` prints its message to stderr. That message can contain a real value.
Clippy uses only its default lints today. The lint levels are flags in `scripts/check`.
rust-analyzer and a plain `cargo clippy` run do not see these flags.

## Decision

We will define the lint policy in the `[lints]` table of `Cargo.toml`.

- `unsafe_code` is `forbid`.
- The Clippy group `pedantic` is `warn`, with priority `-1`.
- `unwrap_used`, `expect_used`, and `dbg_macro` are `warn`.
- `allow_attributes` and `allow_attributes_without_reason` are `warn`. A lint exception uses `#[expect(lint, reason = "...")]`.
- `clippy.toml` permits `unwrap()` and `expect()` in tests.
- `rust-version` in `Cargo.toml` is the pinned toolchain version without the patch number.
- `scripts/check` makes each warning an error with `-D warnings`.

## Consequences

### Positive

- The editor, a plain `cargo clippy` run, and `scripts/check` use the same rules.
- Production code must handle each error. A panic message cannot leak a real value from `unwrap()`.
- `pedantic` finds casting errors and unclear APIs early.
- A coding agent cannot silence a lint without a visible reason. `#[expect]` fails when the exception is no longer necessary.

### Negative / Accepted costs

- `pedantic` gives some warnings without value. Each exception needs an `#[expect]` with a reason.
- Error handling needs more code than `unwrap()`.
- A panic from indexing or from a dependency can still occur. The lints do not prevent it.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Default Clippy lints only | `unwrap()` can leak a real value in a panic message. |
| Lint flags in `scripts/check` only | The editor and a plain `cargo clippy` run do not see the flags. |
| Clippy group `restriction` at `warn` | The Clippy documentation advises against it. Many lints in the group contradict each other. |
| `print_stdout` and `print_stderr` at `warn` now | No output module exists yet. Spec 0002 decides about it. |

## Compliance & verification (optional)

`scripts/check` runs `cargo clippy --locked --all-targets -- -D warnings`.
A review checks that each `#[expect]` reason is true.
