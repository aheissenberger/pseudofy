# Feature Delta Spec: CI and macOS release

**Date:** 2026-11-02
**Status:** proposed
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 (14, 15) · `docs/adr/0002-develop-in-a-devcontainer.md` · `docs/adr/0005-build-releases-in-github-actions.md`

---

## 1. Objective & Context

**Problem:** The devcontainer builds only Linux binaries. The user needs a macOS binary.
**Today:** No CI exists. Checks run only in the local devcontainer.
**After this change:** GitHub Actions runs all checks for each push and pull request. A version tag creates a GitHub release with a macOS binary for Apple Silicon.

## 2. Scope

### Added

- `.github/workflows/ci.yml` — on push and pull request, run `./scripts/check` in the devcontainer with `devcontainers/ci`.
- `.github/workflows/release.yml` — on tag `v*`, run on a macOS arm64 runner. Build `cargo build --release --locked --target aarch64-apple-darwin`.
- Release asset `pseudofy-<version>-aarch64-apple-darwin.tar.gz` and its `.sha256` file. The binary `pseudofy` is at the root of the tarball.

### Modified

- `README.md` — section "Install on macOS": download, checksum check, quarantine removal.

### Removed

- None

### Out of scope (explicitly)

- Intel Macs (`x86_64-apple-darwin`), Linux and Windows release binaries.
- Apple Developer ID signing and notarization.
- Homebrew formula (`docs/specs/0008-homebrew-formula.md`).

## 3. Architectural Rules & Invariants

- [ ] Each third-party action is pinned to a full commit SHA.
- [ ] Each workflow has `permissions:` with the minimum rights. Only `release.yml` gets `contents: write`.
- [ ] The release job reads the Rust version from `rust-toolchain.toml`.
- [ ] The tag version must be equal to the version in `Cargo.toml`. If not, the release job fails.
- [ ] No workflow uses a repository secret.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [ ] A push to `main` runs `ci.yml`. The run is green.
- [ ] The tag `v0.1.0` creates a release with the `.tar.gz` file and the `.sha256` file.
- [ ] `file pseudofy` on the extracted binary prints `Mach-O 64-bit executable arm64`.
- [ ] A tag that is not equal to the `Cargo.toml` version makes the release job fail.

**Tests to add/update:**

- [ ] No new Rust tests. The workflows run the existing tests.

**Commands that must pass:**

```sh
devcontainer exec --workspace-folder . ./scripts/check
gh workflow view ci.yml
gh run list --workflow ci.yml --limit 1
```

**Manual verification (only if unavoidable):**

- Download the release asset on an Apple Silicon Mac — `shasum -a 256 -c` passes.
- Run `xattr -d com.apple.quarantine pseudofy`, then `./pseudofy --version` — the output is `pseudofy 0.1.0`.

## 5. Rollback / Notes (optional)

The binary has only the ad-hoc signature of the Rust linker. Gatekeeper blocks a downloaded binary until the user removes the quarantine attribute.
A later spec can add Developer ID signing and notarization.
