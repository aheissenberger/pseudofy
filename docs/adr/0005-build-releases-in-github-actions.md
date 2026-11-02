# ADR-0005: Build releases in GitHub Actions

**Date:** 2026-11-02
**Status:** proposed
**Deciders:** Andreas Heissenberger
**Related:** `docs/plan/pseudonymization.md` §2 (14, 15) · `docs/specs/0005-ci-and-macos-release.md` · `docs/adr/0002-develop-in-a-devcontainer.md`

## Context

The devcontainer runs Linux. It builds Linux binaries only.
The user needs a macOS binary for Apple Silicon.
A cross-compilation from Linux to macOS needs the Apple SDK. That setup is fragile.
The source is in a public GitHub repository.

## Decision

We will build release binaries in GitHub Actions on a macOS arm64 runner.
A tag `v*` starts the release workflow.
All other checks run in the devcontainer, locally and in CI (ADR-0002).

## Consequences

### Positive

- The binary comes from a native macOS build. No SDK setup is necessary.
- Public repositories get GitHub-hosted macOS runners at no cost.
- Each release is repeatable from one tag.

### Negative / Accepted costs

- A release needs GitHub. A local release build is not defined.
- The binary has no Developer ID signature. The user must remove the quarantine attribute.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Build on the Mac host with `rustup` | The host must not need Rust (ADR-0002). The build is not repeatable. |
| `cargo-zigbuild` in the devcontainer | Not verified for macOS targets. Some crates need the Apple SDK. |
| Self-hosted macOS runner | Maintenance effort. Security risk on a public repository. |

## Compliance & verification (optional)

Spec 0005 defines the workflows. A review checks that each action is pinned to a commit SHA.
