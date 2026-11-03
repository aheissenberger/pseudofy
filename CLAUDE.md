# pseudofy

pseudofy is a Rust command-line app. It replaces person names and email addresses in CSV and XLSX files with stable pseudonyms.
Read `docs/plan/pseudonymization.md` for the requirements.

## Environment

- Work in the devcontainer (ADR-0002). `cargo` exists only in the devcontainer.
- Run subagents with `isolation: worktree` only in the devcontainer.

## Workflow

- Use the skill `spec-driven-development`. If the skill is not available, follow these rules.
- Do not write code for a non-trivial change without a spec in `docs/specs/`.
- Implement only the Added, Modified, and Removed lists of the active spec.
- If necessary work is outside the scope, stop. Tell the user. Update the spec first.
- Do one spec on one branch `feat/NNNN-kebab-case-title`.
- Run `./scripts/docs-check` after each change to a spec or an ADR.
- Before you finish, run `./scripts/check`. The Stop hook blocks a finish while a check fails.

## Code rules

- pseudofy never prints, logs, or writes a real value or the project key. This includes error and panic messages.
  Exception: with `copy_other_files = true`, pseudofy copies other files and excluded CSV and XLSX files unchanged (ADR-0014).
- Do not use `unwrap()` or `expect()` outside tests (ADR-0006).
- Do not add `#[allow]`. If a lint exception is necessary, use `#[expect(lint, reason = "...")]`.
- Do not add a runtime crate without a new ADR (ADR-0004). Pin each version with `=`.
- Use only invented data in tests and fixtures.

## Secrets

- `secrets/` and `.private_*` contain only keys, passwords, and certificates. Each value in them is secret.
- Do not read, copy, print, or hash a file in these directories. Use `ls` on the directory only.
- Do not write to these directories. If a value is missing, give the user instructions.
- Put hosts, paths, and other settings in `config/`. Never put them in `secrets/`.

## Orchestration (ADR-0007)

The main session is the orchestrator. It splits the work of a spec into tasks.

1. Write the task list. Give each task one write set: a list of files.
2. Make sure that no two parallel tasks have a common file in their write sets.
3. Edit `Cargo.toml`, `Cargo.lock`, `src/main.rs`, and `docs/` only in the main session.
4. Add each dependency and each module stub before you start the tasks.
5. Commit on the feature branch. Each worktree starts from this commit.
6. Start one agent for each task. Give it the spec path, the task, and the write set.
7. After each task, compare `git diff --name-only` with the write set. Reject a task that writes outside it.
8. Merge the tasks one by one. Run `check-runner` after each merge.
9. Run `rust-reviewer` once on the full diff of the feature branch.

| Agent | Use it for |
| --- | --- |
| `rust-implementer` | One code task with a write set |
| `rust-test-writer` | Tests from the acceptance criteria of a spec |
| `rust-reviewer` | Review of a diff against the spec |
| `check-runner` | `./scripts/check` with a short result |
| `crate-researcher` | API questions about a pinned crate version |

Do a small task in the main session. Start an agent only if the task is independent or if its output is large.
