# ADR-0001: Use Rust for the CLI

**Date:** 2026-11-02
**Status:** proposed
**Deciders:** Andreas Heissenberger
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/specs/0001-rust-cli-scaffold-and-devcontainer.md`

## Context

pseudofy is a new command-line app. It reads and writes CSV and XLSX files.
It must run without a language runtime on the target machine.
The user requires Rust.

## Decision

We will write pseudofy in Rust, as one binary crate.
The file `rust-toolchain.toml` pins one stable Rust version.
The crate uses the Rust 2024 edition.

## Consequences

### Positive

- The release is one static binary for each platform.
- The type system and `cargo clippy` find many errors before run time.

### Negative / Accepted costs

- Rust has fewer libraries for XLSX than Python. ADR-0004 selects the XLSX crate.
- Compile times are longer than for a script language.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Python with openpyxl | The user requires Rust. The target machine needs a Python runtime. |
| Go | The user requires Rust. |

## Compliance & verification (optional)

`scripts/check` runs `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test`.
