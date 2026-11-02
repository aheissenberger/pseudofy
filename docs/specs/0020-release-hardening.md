# Feature Delta Spec: Release hardening

**Date:** 2026-11-02
**Status:** active
**Author:** Claude (agent)
**Related:** `docs/adr/0005-build-releases-in-github-actions.md` · `docs/specs/0011-checks-only-on-release-tags.md` · `docs/specs/0008-actionlint-in-the-devcontainer.md`

---

## 1. Objective & Context

**Problem:** A review of the build and the release found these gaps:
- The release job runs third-party build code and holds the write token.
- A tag on any commit gives a release.
- The shipped macOS binary is never tested.
- No check looks for crate advisories.
- The build is not reproducible, and the release has no provenance.
- The devcontainer image is pinned by tag only.
- Workflow and shell files are not linted.

**Today:** `.github/workflows/release.yml` has a `check` job and a `release` job. The `release` job builds and publishes in one job.
**After this change:** Three jobs: `check`, `build` (read token), `publish` (write token, no build). The tagged commit must be on `main`. The release has a provenance attestation.

## 2. Scope

### Added

- `deny.toml` — `cargo deny` config: advisories, sources (crates.io only), bans, licenses.
- `scripts/check` — run `cargo deny --locked check`, `actionlint`, and `shellcheck` on `scripts/*`, `.devcontainer/*.sh`, and `.claude/hooks/*`.
- `.devcontainer/post-create.sh` — install `cargo-deny` and `actionlint` at pinned versions with a SHA-256 check (spec 0008-actionlint).

### Modified

- `.github/workflows/release.yml`:
  - `check` job: also fail if the tagged commit is not an ancestor of `origin/main`.
  - `build` job (`contents: read`, `needs: check`): build with `--locked`, run `cargo test --locked --release` for the target, run `pseudofy --version`, make a reproducible tarball (owner 0:0, fixed mtime from `SOURCE_DATE_EPOCH`), upload it as an artifact.
  - `publish` job (`contents: write`, `id-token: write`, `attestations: write`, `needs: build`): download the artifact, write the `.sha256` file, attest the build provenance, create the release as a draft, upload the assets, publish the release.
  - Pin each new action by its full commit SHA.
- `Cargo.toml` — `[profile.release] strip = true`. Do not set `panic = "abort"` (ADR-0004).
- `.devcontainer/devcontainer.json` — pin the image by digest.
- `README.md` — tell the user how to check the provenance with `gh attestation verify`.
- `docs/adr/0004-initial-crate-set.md` — fix the count of direct runtime crates.

### Removed

- The `release` job (replaced by `build` and `publish`).

### Out of scope (explicitly)

- The gh login in the devcontainer. The user keeps it (decision 2026-11-02). The risk: build code in the container can read the token.
- Repository settings (tag ruleset, immutable releases, SHA pinning). The user applies them on GitHub.
- An Apple Developer ID signature.

## 3. Architectural Rules & Invariants

- [ ] Top-level `permissions: {}`. Each job has only the permissions it needs.
- [ ] No job with a write token runs `cargo` or a third-party build step.
- [ ] No `${{ }}` expression in a `run:` script. Each value comes through `env:`.
- [ ] Each action is pinned by a full commit SHA.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [ ] `actionlint` reports no error for `.github/workflows/release.yml`.
- [ ] `./scripts/check` runs `cargo deny`, `actionlint`, and `shellcheck`, and passes.
- [ ] A tag on a commit that is not on `main` fails the `check` job. No release is created.

**Tests to add/update:**

- [ ] None in Rust. The checks above run in `./scripts/check`.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

**Manual verification (only if unavoidable):**

- Push the next release tag. The `build` job runs only after `check`. The `publish` job runs only after `build`. `gh attestation verify` passes for the tarball.

## 5. Rollback / Notes (optional)

The next release tag is the first run of the new jobs (spec 0011).
