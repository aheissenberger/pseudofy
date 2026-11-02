# Feature Delta Spec: Checks only on release tags

**Date:** 2026-11-02
**Status:** active
**Author:** Claude (agent)
**Related:** `docs/specs/0005-ci-and-macos-release.md` · `docs/adr/0005-build-releases-in-github-actions.md` · `docs/adr/0002-develop-in-a-devcontainer.md`

---

## 1. Objective & Context

**Problem:** GitHub Actions runs the checks for each branch push and each pull request. The user wants GitHub Actions to run only for a release.
**Today:** `ci.yml` runs `./scripts/check` on each branch push and pull request. `release.yml` builds the macOS binary on a tag `v*`.
**After this change:** Only a tag `v*` starts a workflow. The workflow runs `./scripts/check` in the devcontainer first. The release job starts only if the checks pass.

## 2. Scope

### Added

- Job `check` in `.github/workflows/release.yml` — run `./scripts/check` in the devcontainer with `devcontainers/ci`.

### Modified

- `.github/workflows/release.yml` — the job `release` needs the job `check`.
- `docs/specs/0005-ci-and-macos-release.md` — a note that spec 0011 replaces the CI trigger.

### Removed

- `.github/workflows/ci.yml`.

### Out of scope (explicitly)

- A change to `./scripts/check`.
- Branch protection rules.

## 3. Architectural Rules & Invariants

- [x] No workflow runs on a branch push or a pull request.
- [ ] A tag `v*` runs the checks before the release build. A failed check stops the release.
- [x] All rules of spec 0005 §3 apply: actions pinned to a full commit SHA, minimum `permissions:`, only the release job gets `contents: write`, no repository secret.
- [ ] Developers run `./scripts/check` in the devcontainer before a merge (ADR-0002).

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] A push to `main` starts no workflow run.
- [ ] A tag `v*` shows the job `check` before the job `release` in one workflow run.
- [ ] If the job `check` fails, the job `release` does not run.

**Tests to add/update:**

- [x] No Rust tests. The workflow runs the existing tests.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
gh workflow list
```

## 5. Rollback / Notes (optional)

Without CI on `main`, a merge with a failed check is possible. The next release tag finds it before the build.
