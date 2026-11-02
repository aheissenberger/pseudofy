# Feature Delta Spec: actionlint in the devcontainer

**Date:** 2026-11-02
**Status:** proposed
**Author:** Claude (agent)
**Related:** `docs/specs/0005-ci-and-macos-release.md` · `docs/specs/0007-claude-code-project-setup.md` · `docs/adr/0005-build-releases-in-github-actions.md`

---

## 1. Objective & Context

**Problem:** Spec 0005 adds GitHub Actions workflows. An error in a workflow file is found only after a push.
**Today:** The devcontainer has `shellcheck` but no workflow linter.
**After this change:** `actionlint` exists in the devcontainer. It checks workflow files locally. It uses `shellcheck` for the `run:` scripts.

## 2. Scope

### Added

- None

### Modified

- `.devcontainer/post-create.sh` — download `actionlint` 1.7.12 for the container architecture from the GitHub release. Verify the SHA-256 checksum. Install the binary to `/usr/local/bin/actionlint`.

### Removed

- None

### Out of scope (explicitly)

- `actionlint` in `scripts/check`. Spec 0005 adds it together with the first workflow file.
- Workflow files (spec 0005).

## 3. Architectural Rules & Invariants

- [ ] The version and the SHA-256 checksum of each architecture are pinned in `.devcontainer/post-create.sh`.
- [ ] If the checksum does not match, `.devcontainer/post-create.sh` fails and installs nothing.
- [ ] The script supports the architectures `arm64` and `amd64`.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [ ] `actionlint -version` prints `1.7.12` in the devcontainer.
- [ ] `actionlint` reports an error for a workflow file with an unknown key.

**Tests to add/update:**

- [ ] None. The commands below check the behavior.

**Commands that must pass:**

```sh
devcontainer up --workspace-folder . --remove-existing-container
devcontainer exec --workspace-folder . actionlint -version
devcontainer exec --workspace-folder . shellcheck .devcontainer/post-create.sh
```

**Manual verification (only if unavoidable):**

- None

## 5. Rollback / Notes (optional)

To update `actionlint`, change the version and both checksums from `actionlint_<version>_checksums.txt` of the release.
