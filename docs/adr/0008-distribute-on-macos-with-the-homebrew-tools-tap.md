# ADR-0008: Distribute on macOS with the homebrew-tools tap

**Date:** 2026-11-02
**Status:** proposed
**Deciders:** Andreas Heissenberger
**Related:** `docs/plan/pseudonymization.md` §2 (14, 17) · `docs/specs/0008-homebrew-formula.md` · `docs/adr/0005-build-releases-in-github-actions.md`

## Context

Spec 0005 publishes a macOS arm64 tarball on each GitHub release.
A manual download needs a checksum check and the removal of the quarantine attribute.
The user wants to install and upgrade pseudofy with Homebrew.
The user already has the public tap `aheissenberger/homebrew-tools`. It holds the formulas `gitvault` and `tackler`.
Homebrew does not set the quarantine attribute on files that a formula installs.

## Decision

We will publish pseudofy as the formula `Formula/pseudofy.rb` in the tap `aheissenberger/homebrew-tools`.
The formula installs the release binary from spec 0005. It does not build from source.
The maintainer updates the formula manually for each release. No workflow writes to the tap.

## Consequences

### Positive

- The install command is `brew install aheissenberger/tools/pseudofy`.
- `brew upgrade` updates pseudofy. Homebrew checks the SHA-256 of the tarball.
- The user does not need to remove the quarantine attribute.
- No repository secret is necessary. Spec 0005's "no secret" rule stays valid.
- The tap uses one pattern for all tools of the maintainer.

### Negative / Accepted costs

- Each release needs one manual commit in the tap: `version`, `url`, and `sha256`.
- The formula supports only Apple Silicon, the same as spec 0005.
- The formula is not in `homebrew/core`. The user must name the tap.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| New tap `aheissenberger/homebrew-pseudofy` | A second tap repository for one formula. The existing tap already fits. |
| Formula in the pseudofy repository | Not a standard tap name. The user must give the tap URL explicitly. |
| `release.yml` pushes the formula to the tap | Needs a token secret with write access to another repository. Spec 0005 forbids secrets. |
| Formula builds from source | Needs Rust on the user's Mac. Slow install. The release binary already exists. |
| Submit to `homebrew/core` | The project is new and has few users. `homebrew/core` requires notable projects. |

## Compliance & verification (optional)

Spec 0008 defines the formula. `brew audit --strict` and `brew test` run on an Apple Silicon Mac before each tap commit.
