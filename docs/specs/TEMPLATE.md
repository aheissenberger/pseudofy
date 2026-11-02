# Feature Delta Spec: <short title>

**Date:** YYYY-MM-DD
**Status:** proposed | active | done | superseded
**Author:** <name or "Claude (agent)">
**Related:** `docs/plan/<plan>.md` §<section> · `docs/adr/NNNN-kebab-case-title.md` · #issue

> Delta specs describe one bounded change. Keep each spec to approximately one page.
> Delete the guidance blockquotes before you set the status to `active`.
> Status lifecycle: `proposed` = planned, no branch; `active` = work in progress on a feature
> branch; `done` = merged into the main branch (set it in the merged change set);
> `superseded` = a later spec replaces this spec.
> Write in the spirit of ASD-STE100: short direct sentences, one action per step, one term per
> concept.

---

## 1. Objective & Context

<!--
Why does this change exist? Write 2 to 4 sentences. Link the requirement (plan section, bug,
user need). Describe the current state and the state after the change.
-->

**Problem:**
**Today:**
**After this change:**

## 2. Scope

<!--
Explicit boundaries. Work that is not in Added, Modified, or Removed is out of scope.
Use repository-relative paths.
-->

### Added

- <new module, package, file, script, document, or capability>

### Modified

- <existing module, file, script, or document, and the type of change>

### Removed

- <deleted code, deprecated behavior, or "None">

### Out of scope (explicitly)

- <adjacent work that this change must NOT do>

## 3. Architectural Rules & Invariants

<!--
Rules that the implementation must obey. Take them from the project instructions, the
architecture documents, and the related ADRs. Replace the examples with the applicable rules.
-->

- [ ] Business logic stays in the domain or application layer. The UI and the adapters stay thin.
- [ ] No new dependency without an ADR (or: this change adds `<dependency>` because <reason>).
- [ ] Public interfaces stay backward-compatible (or: this change breaks `<interface>` because <reason>).
- [ ] Change-specific invariant: <rule>

## 4. Acceptance Criteria & Verification

<!--
Exact, runnable verification. Use automated tests where possible.
-->

**Behavior:**

- [ ] <observable result>
- [ ] <error case and its expected result>

**Tests to add/update:**

- [ ] `<path to test file>` — <what the tests cover>

**Commands that must pass:**

```sh
./scripts/docs-check
<format check command>
<lint command>
<test command>
```

**Manual verification (only if unavoidable):**

- <step> — <expected result>

## 5. Rollback / Notes (optional)

<Migration concerns, feature flags, follow-up specs, open questions.>
