# ADR-0016: Update the tap formula with a manual workflow

**Date:** 2026-11-03
**Status:** accepted
**Deciders:** Andreas Heissenberger
**Related:** `docs/adr/0008-distribute-on-macos-with-the-homebrew-tools-tap.md` · `docs/specs/0008-homebrew-formula.md` · `docs/specs/0020-release-hardening.md`

## Context

ADR-0008 publishes pseudofy as `Formula/pseudofy.rb` in the tap `aheissenberger/homebrew-tools`. The maintainer updates the formula by hand for each release. No workflow writes to the tap.
After release 0.2.0, the formula stayed at 0.1.0 until a manual commit. `brew upgrade` gave the old version.
A push from `release.yml` to the tap needs a token secret with write access to a second repository. Spec 0005 and spec 0020 forbid secrets in the release workflow.

## Decision

We will update the formula with a workflow in the tap repository that the maintainer starts by hand. This ADR replaces ADR-0008 and keeps each of its other rules.

- The workflow is `.github/workflows/update-pseudofy.yml` in `aheissenberger/homebrew-tools`. Its only trigger is `workflow_dispatch`, with an optional version. The default is the latest pseudofy release.
- The workflow uses the `GITHUB_TOKEN` of the tap repository. It has `contents: write` for the tap only. No secret and no token for another repository exist.
- Before it changes the formula, the workflow checks the SHA-256 of the release asset and its build attestation from `aheissenberger/pseudofy/.github/workflows/release.yml` at the tag of the version.
- The workflow runs `brew style`, `brew audit --strict`, `brew install`, and `brew test` on macOS. It pushes the commit `pseudofy X.Y.Z` to `main` only if each check passes.
- `release.yml` in pseudofy does not change. It does not write to the tap.

## Consequences

### Positive

- One command updates the formula: `gh workflow run update-pseudofy.yml -R aheissenberger/homebrew-tools -f version=X.Y.Z`.
- The formula changes only after a check of the attestation and an install test.
- The pseudofy release workflow stays free of secrets.

### Negative / Accepted costs

- The maintainer must start the workflow after each release. Until then, `brew upgrade` gives the old version.
- A workflow in the tap can write to the tap. A person with write access to the tap can change the workflow.
- The workflow uses one macOS runner for each update.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Manual commit for each release (ADR-0008) | The formula stayed at the old version after release 0.2.0. Each update needs four manual checks. |
| `release.yml` pushes the formula to the tap | Needs a token secret with write access to another repository. Spec 0005 and spec 0020 forbid secrets. |
| A scheduled workflow in the tap | It changes the tap without a decision of the maintainer. |
| The workflow opens a pull request | Needs the repository setting that lets GitHub Actions create pull requests. The manual start is already the decision of the maintainer. |

## Compliance & verification (optional)

The workflow passes `actionlint`. The first run after release 0.3.0 checks the attestation, the install, and `pseudofy --version`.
