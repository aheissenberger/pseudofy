# Feature Delta Spec: Claude Code project setup

**Date:** 2026-11-02
**Status:** proposed
**Author:** Claude (agent)
**Related:** `docs/adr/0007-split-agent-work-by-file-ownership.md` · `docs/adr/0006-strict-lint-policy-in-cargo-toml.md` · `docs/adr/0002-develop-in-a-devcontainer.md`

---

## 1. Objective & Context

**Problem:** Claude Code in the devcontainer has no project rules. The global user rules and skills are not in the container. Nothing makes Claude run `scripts/check` before it stops.
**Today:** The repository has no `CLAUDE.md` and no `.claude/` directory.
**After this change:** Claude Code reads the project rules. A Stop hook blocks a finish while a check fails. Five subagents with task-specific models do bounded work. The main session splits work into tasks with disjoint write sets (ADR-0007).

## 2. Scope

### Added

- `CLAUDE.md` — project rules: spec-driven workflow, secrets rules, "never print a real value", `scripts/check` before a finish, the orchestration protocol from ADR-0007.
- `.claude/settings.json` — permissions and the Stop hook.
  - `deny`: `Read`, `Edit`, `Write` on `./secrets/**` and `./**/.private_*/**`.
  - `allow`: `cargo fmt`, `cargo clippy`, `cargo test`, `cargo build`, `cargo check`, `./scripts/check`, `./scripts/docs-check`, read-only `git` commands.
- `.claude/hooks/stop-check` — Stop hook. If the working tree has no change, exit 0. Else run `./scripts/check`. If `cargo` is not on the `PATH`, run it with `devcontainer exec`. If the check fails, exit 2 and write the last 60 output lines to stderr.
- `.claude/agents/rust-implementer.md` — `model: sonnet`, `effort: medium`, `isolation: worktree`. Implements one task. Writes only the files of its write set. Runs `./scripts/check`. Commits in its worktree.
- `.claude/agents/rust-test-writer.md` — `model: sonnet`, `effort: medium`, `isolation: worktree`. Writes tests in `tests/` from the spec acceptance criteria only. Does not read the implementation of the task.
- `.claude/agents/rust-reviewer.md` — `model: opus`, `effort: high`, no `Write` and no `Edit`. Reviews a diff against the spec, the invariants, and real-value leaks.
- `.claude/agents/check-runner.md` — `model: haiku`, `effort: low`, tools `Bash`, `Read`. Runs `./scripts/check`. Returns only the failed checks and their first errors.
- `.claude/agents/crate-researcher.md` — `model: haiku`, `effort: low`, no `Write` and no `Edit`. Answers API questions for the pinned crate versions from the sources in the Cargo registry.

### Modified

- `.gitignore` — add `.claude/worktrees/` and `.claude/settings.local.json`.

### Removed

- None

### Out of scope (explicitly)

- A project setting for the main session model. The user selects it with `/model`.
- A hook that enforces a write set per tool call. The main session checks write sets after each task.
- CI integration (spec 0005).
- A copy of the user skill `spec-driven-development` in the repository. The user copies it once into the volume `pseudofy-claude-config`.

## 3. Architectural Rules & Invariants

- [ ] Each agent file uses only fields from the Claude Code subagent documentation.
- [ ] No agent except the main session edits `Cargo.toml`, `Cargo.lock`, `src/main.rs`, or `docs/`.
- [ ] No two agents that run at the same time have a common file in their write sets.
- [ ] The Stop hook never prints a file from `secrets/` or `.private_*`.
- [ ] The Stop hook works in the devcontainer and on the host.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [ ] `.claude/hooks/stop-check` exits 0 on a clean working tree.
- [ ] `.claude/hooks/stop-check` exits 2 if `src/main.rs` contains `unwrap()`.
- [ ] `/agents` in Claude Code lists the five project agents.
- [ ] A `Read` of `secrets/x` is denied.

**Tests to add/update:**

- [ ] None. The hook behavior is checked with the commands below.

**Commands that must pass:**

```sh
devcontainer exec --workspace-folder . ./scripts/check
devcontainer exec --workspace-folder . bash -c 'echo "{}" | .claude/hooks/stop-check; echo "exit=$?"'
python3 -m json.tool .claude/settings.json
```

**Manual verification (only if unavoidable):**

- Run `/agents` in Claude Code in the devcontainer — the list shows the five agents with their models.
- Ask Claude to read `secrets/x` — Claude Code denies the read.

## 5. Rollback / Notes (optional)

Each worktree has its own `target/` directory. The first build in a worktree takes longer.
