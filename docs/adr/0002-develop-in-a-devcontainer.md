# ADR-0002: Develop in a devcontainer

**Date:** 2026-11-02
**Status:** proposed
**Deciders:** Andreas Heissenberger
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/specs/0001-rust-cli-scaffold-and-devcontainer.md`

## Context

The user develops on macOS. The user requires a devcontainer.
Each developer and each agent must get the same toolchain.
The host must not need a local Rust installation.

## Decision

We will define the development environment in `.devcontainer/devcontainer.json`.
The devcontainer uses the image `mcr.microsoft.com/devcontainers/rust` with a pinned version tag.
`rust-toolchain.toml` controls the Rust version inside the container.
All checks run inside the devcontainer.

## Consequences

### Positive

- The toolchain is the same for each developer, for agents, and for CI.
- VS Code opens the project in the container with one command.

### Negative / Accepted costs

- The host needs Docker.
- The command-line check needs the devcontainer CLI (`@devcontainers/cli`) on the host.
- The first container build takes several minutes.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Local `rustup` on the host | The user requires a devcontainer. Versions drift between machines. |
| Nix flake | The user requires a devcontainer. |
| Own `Dockerfile` | The prebuilt image is sufficient. An own file adds maintenance. |

## Compliance & verification (optional)

`devcontainer exec --workspace-folder . ./scripts/check` must pass.
