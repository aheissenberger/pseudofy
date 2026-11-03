# Feature Delta Spec: Two last names

**Date:** 2026-11-03
**Status:** active
**Author:** Claude (agent)
**Related:** `docs/specs/0022-fewer-pseudonym-collisions.md` · `docs/adr/0003-derive-pseudonyms-with-a-keyed-hash.md`

---

## 1. Objective & Context

**Problem:** A project with 10 000 persons still gets about 15 person collisions with `two_first_names = true`. One real first name always gets one fake first name. Two persons with the same real first name collide if their two real last names get one fake last name. The last-name list has 1 887 entries, so this happens for 1 of 1 887 such pairs.
Spec 0022 and the README say that `two_first_names = true` gives about 0.01 person collisions. This is not correct.
**Today:** a fake last-name token is one entry of the last-name list.
**After this change:** the setting `two_last_names` gives each fake last-name token a second last name: `Berger-Wagner`. With `two_first_names` and `two_last_names`, person collisions drop to about 0.02 for 10 000 persons.

## 2. Scope

### Added

- `src/config.rs` — the setting `two_last_names`. The default is `false`. The `init` config shows it as a commented default with an explanation.
- `src/pseudonym.rs` — with `two_last_names = true`, each fake last-name token is two different entries of the last-name list without `-`, joined with `-`. The HMAC output selects both entries. Neither entry is the real last name. An initial stays one name.

### Modified

- `src/pseudonym.rs` — `double_first_name` becomes `double_name` for both lists.
- `src/run.rs` — pass `two_last_names` to the pseudonymizer.
- `README.md` — the collision section: the two causes of person collisions, and the effect of each setting. Release notes for 0.4.0.
- `docs/specs/0022-fewer-pseudonym-collisions.md` — section 5 links this spec for the corrected numbers.
- `tests/csv.rs`, `tests/cli.rs` — the tests of section 4.
- `Cargo.toml` — version `0.4.0`.

### Removed

- None

### Out of scope (explicitly)

- Larger name lists.
- A fake last name that depends on the first name.

## 3. Architectural Rules & Invariants

- [x] With `two_last_names = false`, each fake name of 0.3.0 stays unchanged. `tests/pseudonym_stability.rs` passes unchanged.
- [x] Each pseudonym comes from the HMAC output only (ADR-0003).
- [x] No new crate. No `unwrap()` or `expect()` outside tests.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] With `two_last_names = true`, `Anna Zorbel` gives `X Y-Z`: one fake first name and two different fake last names joined with `-`.
- [x] No German last name of the list appears as one part of its own fake double last name.
- [x] 300 persons with one first name and 300 different last names: without `two_last_names`, 5 or more person collisions. With `two_last_names`, 0 (unit test with a fixed key).
- [x] With `two_last_names = true`, one real last name gets the same fake double last name in a last-name column, a full-name column, and free text.

**Tests to add/update:**

- [x] Unit tests in `src/pseudonym.rs` and `src/config.rs`, and a test in `tests/csv.rs`, for the behaviors above.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

Expected person collisions for 10 000 persons with the German lists (1 644 first names, 1 887 last names), and uniform names. Measured with the binary on invented data, after review:

| `two_first_names` | `two_last_names` | Person collisions |
| --- | --- | --- |
| false | false | 45 |
| false | true | 14 |
| true | false | 15 |
| true | true | 0 |

The three causes are: the same first name (about 15), the same last name (about 15), and both names different with both fake parts equal (about 16). Each setting removes two of the three causes.

A change of `two_last_names` changes each fake last name of the project.
