# Feature Delta Spec: Fewer pseudonym collisions

**Date:** 2026-11-03
**Status:** done
**Author:** Claude (agent)
**Related:** `docs/specs/0024-two-last-names.md` (corrected collision numbers) · `docs/adr/0003-derive-pseudonyms-with-a-keyed-hash.md` · `docs/adr/0015-add-two-digits-to-fake-emails.md` · `docs/specs/0019-text-normalization.md` · `docs/specs/0021-rule-file-lists-and-exclusions.md`

---

## 1. Objective & Context

**Problem:** A project with 10 000 persons gets about 16 shared fake full names and 16 shared fake emails. The name pool has about 3.1 million full names. The warning `two or more real values share one pseudonym` does not say which kind collides, or that the cause is chance.
**Today:** a fake email is `first.last@label.example.com`. A fake first name is one entry of the first-name list. The collision count is one number.
**After this change:** a fake email ends with two digits. The setting `two_first_names` gives each fake first name a second first name. The warning gives the count for each kind and names the cause.

## 2. Scope

### Added

- `src/pseudonym.rs` — the local part of a fake email ends with two digits from 10 to 99, taken from the HMAC output: `sophie.wagner42@label.example.com`.
- `src/config.rs` — the setting `two_first_names`. The default is `false`. The `init` config shows it as a commented default with an explanation.
- `src/pseudonym.rs` — with `two_first_names = true`, each fake first-name token is two different entries of the first-name list, joined with `-`: `Lukas-Maximilian`. The HMAC output selects both entries. An initial stays one letter.
- `src/pseudonym.rs` — the collision count for each kind: persons, emails, email domains.

### Modified

- `src/run.rs`, `src/main.rs` — the summary line keeps `collisions: N`. The warning becomes `warning: N collisions (persons: A, emails: B, email domains: C): different real values got one pseudonym by chance`.
- `tests/pseudonym_stability.rs` — the golden fake emails and the checksums of the email values. The golden fake names stay unchanged.
- `tests/csv.rs`, `tests/xlsx.rs`, `tests/free_text.rs`, `tests/cli.rs` — expected fake emails, and tests of section 4.
- `README.md` — a section about collisions and `two_first_names`. Release notes for 0.3.0: each fake email changes.
- `docs/adr/0015-add-two-digits-to-fake-emails.md` — new ADR for the change of each fake email.

### Removed

- None

### Out of scope (explicitly)

- A saved table of pseudonyms.
- Larger name lists.
- A second first name in the local part of a fake email.

## 3. Architectural Rules & Invariants

- [x] Each pseudonym comes from the HMAC output only (ADR-0003). The order of files and rows does not change a pseudonym.
- [x] With `two_first_names = false`, each fake name of 0.2.0 stays unchanged.
- [x] A warning names counts and kinds, never a value.
- [x] No new crate. No `unwrap()` or `expect()` outside tests.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] Each fake email has a local part `x.y` plus two digits from 10 to 99. One real email gets the same fake email in each run.
- [x] With 25 name pairs, 200 different real emails give fewer than 40 collisions. Without digits, they give 175 or more (unit test with a fixed key).
- [x] With `two_first_names = true`, `Anna Zorbel` gives `X-Y Z`: two different fake first names joined with `-`, and one fake last name. `A. Zorbel` keeps one initial.
- [x] With `two_first_names = true`, one real first name gets the same two fake first names in a first-name column, a full-name column, and free text.
- [x] The warning gives the count for each kind. The three counts add up to the summary count.

**Tests to add/update:**

- [x] Unit tests in `src/pseudonym.rs` and `src/config.rs`, and tests in `tests/csv.rs` and `tests/cli.rs`, for the behaviors above.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

Expected collisions for 10 000 persons: emails about 16 to about 0.2.
The estimate "persons with `two_first_names = true`: about 0.01" is wrong. `two_first_names` removes only collisions of persons with the same last name. Spec 0024 gives the corrected numbers and adds `two_last_names`.
A change of `two_first_names` changes each fake first name of the project.
