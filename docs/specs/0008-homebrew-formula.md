# Feature Delta Spec: Homebrew formula

**Date:** 2026-11-02
**Status:** done
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 (17) · `docs/adr/0008-distribute-on-macos-with-the-homebrew-tools-tap.md` · `docs/specs/0005-ci-and-macos-release.md`

---

## 1. Objective & Context

**Problem:** A manual install of the macOS binary needs a download, a checksum check, and the removal of the quarantine attribute.
**Today:** Spec 0005 defines the release tarball. No Homebrew formula exists.
**After this change:** `brew install aheissenberger/tools/pseudofy` installs the release binary on an Apple Silicon Mac.

This spec starts only after spec 0005 is done and the release `v0.1.0` exists.

## 2. Scope

### Added

- `Formula/pseudofy.rb` in the repository `aheissenberger/homebrew-tools`. It downloads `pseudofy-<version>-aarch64-apple-darwin.tar.gz` from the pseudofy GitHub release.
- Section "pseudofy" in `README.md` of `aheissenberger/homebrew-tools`. It gives the install command and the steps to update the formula.

### Modified

- `README.md` (pseudofy) — section "Install on macOS": put the Homebrew install first. Keep the manual download as the second option.

### Removed

- None

### Out of scope (explicitly)

- Intel Macs and Linux (Homebrew on Linux).
- A workflow that updates the tap automatically.
- A Homebrew cask, a source build, and a submission to `homebrew/core`.
- Changes to `.github/workflows/release.yml`.

## 3. Architectural Rules & Invariants

- [x] The formula has `depends_on arch: :arm64` and `depends_on :macos`.
- [x] The formula `version`, the release tag, and the `Cargo.toml` version are equal.
- [x] The formula `sha256` is equal to the value in the release `.sha256` file.
- [x] The formula `license` is `"Apache-2.0"`.
- [x] The `test do` block runs only `pseudofy --version`. It uses no real data and no project key.
- [x] No workflow and no repository uses a new secret (ADR-0008).

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] `brew install aheissenberger/tools/pseudofy` installs `pseudofy` without a quarantine attribute.
- [x] `pseudofy --version` prints `pseudofy 0.1.0`.
- [x] `brew test aheissenberger/tools/pseudofy` passes.
- [x] On an Intel Mac, `brew install` stops with an architecture error.

**Tests to add/update:**

- [x] `Formula/pseudofy.rb` — `test do` block asserts the version output.

**Commands that must pass:**

```sh
./scripts/docs-check
brew style Formula/pseudofy.rb
brew audit --strict aheissenberger/tools/pseudofy
brew install aheissenberger/tools/pseudofy
brew test aheissenberger/tools/pseudofy
xattr "$(brew --prefix)/bin/pseudofy"
```

**Manual verification (only if unavoidable):**

- Run the `brew` commands on an Apple Silicon Mac. The devcontainer has no Homebrew for macOS.
- Run `xattr` on the installed binary — the output does not contain `com.apple.quarantine`.

## 5. Rollback / Notes (optional)

Verified on 2026-11-02 with Homebrew 57cac9d: `brew style` and `brew audit --strict --online` pass, `brew install` and `brew test` pass on an Apple Silicon Mac, `pseudofy --version` prints `pseudofy 0.1.0`, and the binary has no quarantine attribute. The Intel case is covered by `depends_on arch: :arm64`; no Intel Mac was tested. Current Homebrew trusts the formula on `brew install aheissenberger/tools/pseudofy`.

To roll back, delete `Formula/pseudofy.rb` from the tap. The GitHub release stays.
The formula depends on the tarball layout of spec 0005: the binary `pseudofy` is at the root of the tarball.
