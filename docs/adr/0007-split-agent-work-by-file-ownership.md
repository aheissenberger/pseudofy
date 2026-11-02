# ADR-0007: Split agent work by file ownership

**Date:** 2026-11-02
**Status:** proposed
**Deciders:** Andreas Heissenberger
**Related:** `docs/specs/0007-claude-code-project-setup.md` · `docs/adr/0002-develop-in-a-devcontainer.md` · `docs/adr/0006-strict-lint-policy-in-cargo-toml.md`

## Context

Claude Code can run subagents in parallel. Each subagent can use a different model.
Two agents that write the same file at the same time overwrite each other's work.
Cargo compiles the full crate. A half-finished file of one agent breaks the build of another agent in the same directory.
`Cargo.toml`, `Cargo.lock`, and `src/main.rs` change in almost each spec. They are a merge conflict risk.
A model with more capability costs more and runs slower. Mechanical tasks do not need it.

## Decision

We will split the work of each spec into tasks with disjoint write sets.

- The main session is the orchestrator. It owns `Cargo.toml`, `Cargo.lock`, `src/main.rs`, and `docs/`.
- Before it starts tasks, the orchestrator adds each dependency and each module stub. It commits them on the feature branch.
- Each task has one write set: a list of files. Two tasks that run at the same time have no common file.
- Each writing agent runs in its own git worktree (`isolation: worktree`).
- After a task, the orchestrator compares `git diff --name-only` with the write set. It rejects a task that writes outside its write set.
- The orchestrator merges the tasks one by one. It runs `check-runner` after each merge, then `rust-reviewer` once.

We will select the model by task:

| Agent | Model | Reason |
| --- | --- | --- |
| Main session | user selection (Opus recommended) | Planning, specs, merges |
| `rust-implementer`, `rust-test-writer` | Sonnet | Bounded code tasks with a clear spec |
| `rust-reviewer` | Opus | Errors in review are expensive |
| `check-runner`, `crate-researcher` | Haiku | Mechanical runs and lookups |

## Consequences

### Positive

- Parallel tasks cannot overwrite each other. Merges have no conflicts.
- Each worktree compiles a consistent state.
- Mechanical work uses a fast and cheap model.
- The tests come from the spec and not from the implementation.

### Negative / Accepted costs

- Each worktree has its own `target/` directory. The first build in each worktree is slow.
- The orchestrator must plan the write sets before work starts.
- A task that needs a file of another task must wait for the merge of that task.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| One agent does all work | No parallel work. The main context fills with build output. |
| Parallel agents in one directory | One agent's half-finished file breaks the other agent's build. |
| One model for all agents | Opus for mechanical work is slow and expensive. Haiku for review misses errors. |
| Shared `CARGO_TARGET_DIR` for all worktrees | Cargo locks the directory. Builds wait for each other and invalidate each other's cache. |

## Compliance & verification (optional)

The orchestrator compares each task diff with its write set before the merge.
`rust-reviewer` checks the merged diff against the spec.
