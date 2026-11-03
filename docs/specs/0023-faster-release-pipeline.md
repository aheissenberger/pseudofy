# Feature Delta Spec: Faster release pipeline

**Date:** 2026-11-03
**Status:** done
**Author:** Claude (agent)
**Related:** `docs/specs/0011-checks-only-on-release-tags.md` · `docs/specs/0020-release-hardening.md` · `docs/adr/0005-build-releases-in-github-actions.md`

---

## 1. Objective & Context

**Problem:** A release run takes about 6.5 minutes. The jobs `check` (176 s), `build` (180 s), and `publish` (28 s) run one after the other. In `check`, `tests/xlsx.rs` takes 46 s, because the XLSX dependencies run without optimization.
**Today:** `build` needs `check`. `publish` needs `build`. Debug and test builds compile each dependency with `opt-level = 0`.
**After this change:** `check` and `build` run at the same time. `publish` needs both. Debug and test builds optimize the dependencies. A release run takes about 3.5 minutes.

## 2. Scope

### Added

- `Cargo.toml` — `[profile.dev.package."*"]` with `opt-level = 2`. pseudofy itself stays at `opt-level = 0` in debug builds.

### Modified

- `.github/workflows/release.yml` — `build` does not need `check`. `publish` needs `check` and `build`.
- `.github/workflows/release.yml` — the header comment describes the new job order.
- `docs/specs/0011-checks-only-on-release-tags.md`, `docs/specs/0020-release-hardening.md` — link this spec in `**Related:**`. The publish of a release still needs the checks.

### Removed

- None

### Out of scope (explicitly)

- A cache of the cargo registry or the target directory.
- A prebuilt devcontainer image.
- A separate macOS test job.

## 3. Architectural Rules & Invariants

- [x] A release is published only if `check` and `build` pass.
- [x] `build` keeps `contents: read`. Only `publish` has write permissions.
- [x] The release binary does not change: `[profile.release]` does not change.
- [x] No new crate. No new action.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] `actionlint` passes for `release.yml`.
- [x] In `release.yml`, `publish` has `needs: [check, build]`, and `build` has no `needs`.
- [x] `cargo test --test xlsx` takes less time with the new profile than without it (measured in the devcontainer).
- [ ] The next release tag: `check` and `build` start at the same time, and `publish` starts after both. This criterion is checked at the next release.

**Tests to add/update:**

- [x] None. `./scripts/check` covers the profile change.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

If `check` fails, `build` has used a macOS runner for nothing. The repository is public, so the runner is free.
Measured in the devcontainer: `tests/xlsx.rs` runs in 6.6 s instead of 11.9 s. On the GitHub runner, it ran in 46 s without the profile.
