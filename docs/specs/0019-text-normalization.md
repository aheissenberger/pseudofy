# Feature Delta Spec: Text normalization and name parsing

**Date:** 2026-11-02
**Status:** done
**Author:** Claude (agent)
**Related:** `docs/adr/0013-normalize-text-before-hashing.md` · `docs/adr/0003-derive-pseudonyms-with-a-keyed-hash.md` · `docs/specs/0004-known-names-in-free-text.md`

---

## 1. Objective & Context

**Problem:** A review found these faults:
- One real value gets two pseudonyms (apostrophe forms, `ß`, `İ`, IDN domains).
- Invisible characters let a name through.
- The email pattern takes in punctuation and parts of URLs. It misses `@localhost` and quoted local parts.
- A full name with a title, a particle, or a suffix is split wrongly. A title gets a fake word, so title holders can be grouped.
- `mailto:` in an email column is replaced as a name.
- The collision warning fires on almost every project, and it misses two persons with the same fake full name.

**Today:** `pseudonym::normalize` and `known_names::fold` differ. `text::email_regex` uses broad character classes.
**After this change:** one normalization (ADR-0013), a strict email pattern, and a full-name parser that knows titles, particles, and suffixes. The collision count reports persons.

## 2. Scope

### Added

- `src/text.rs` — a shared `normalize` (ADR-0013), used by `src/pseudonym.rs` and `src/known_names.rs`. Format characters (Cf) are token characters, so they never split a name.
- `src/text.rs` — the email pattern:
  - The local part is RFC 5322 `atext` and `.`, or a quoted string.
  - The domain is labels of letters, digits, and `-`, joined by `.`. The last label has a letter. The only single label is `localhost`.
  - Characters after the address (`!`, `?`, `»`, `«`, `…`, `.`) are not part of it.
  - pseudofy replaces each address, also in a URL. In a match, only the text after the last `/`, `:`, `;`, `,`, `|`, `?`, `&`, `=`, or quote is the local part, so the rest of the URL stays. A URL userinfo (`https://user@host/`) is an address.
  - The scan is linear in the length of the text.
- `src/text.rs` — the full-name parser (a title counts only before the first name, a suffix only after the last name; the last name token is never a title or a suffix):
  - Titles and salutations stay unchanged and are not names: `Dr.`, `Prof.`, `Mag.`, `Ing.`, `DI`, `Herr`, `Frau`, `Mr.`, `Mrs.`, `Ms.`.
  - Suffixes stay unchanged: `Jr.`, `Sr.`, `II`, `III`, `MBA`, `MSc`, `BSc`, `PhD`.
  - Lowercase particles belong to the last name: `von`, `van`, `de`, `der`, `den`, `du`, `la`, `le`, `zu`, `di`, `da`, `ten`, `ter`.
  - `Last, First` (one comma) is read as last name, then first name. The order stays in the output.
  - A single word is a last name, as today.
- `src/cells.rs` — in an email column, `mailto:` before the address stays unchanged.
- `src/pseudonym.rs` — the collision count reports two different real persons (first and last name together) or two real emails with one pseudonym. Two first names with one fake first name are not counted. An initial is recorded under its letter.

### Modified

- `src/known_names.rs`, `src/pseudonym.rs` — use the shared `normalize`.
- `src/pseudonym.rs` — convert each `xn--` label of at most 63 bytes to Unicode before the hash (RFC 3492 decode, no new crate). A domain gets no `ß` to `ss` mapping (IDNA2008).
- `src/text.rs` — `normalize` lowercases the full string (Greek final sigma). A token with U+200B that does not match a known name is also tried as its parts.
- `tests/free_text.rs`, `tests/csv.rs` — the tests of section 4. `tests/cli.rs` reads the version from `Cargo.toml`. `Cargo.lock` gets the new version.
- `Cargo.toml` — version `0.2.0`.
- `README.md` — release notes for 0.2.0: which pseudonyms change (ADR-0013).

### Removed

- None

### Out of scope (explicitly)

- Diacritic folding (`ö` to `o`) and transliteration (`ue` to `ü`).
- Genitive forms (`Zorbinas`).

## 3. Architectural Rules & Invariants

- [x] `tests/pseudonym_stability.rs` passes unchanged.
- [x] Column pseudonyms and free-text pseudonyms use the same normalization.
- [x] No new crate. No `unwrap()` or `expect()` outside tests.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [x] `O’Brien` and `O'Brien`, `Strauß` and `STRAUSS`, `İlker` and `Ilker` each get one pseudonym, in columns and in free text.
- [x] `Mül\u{ad}ler` and `Kes\u{200b}ta` in free text get the pseudonym of `Müller` and `Kesta`.
- [x] `Schreib an anna.zorbel@firma.test!` keeps the `!`. The address gets the same pseudonym as without the `!`.
- [x] `https://firma.test/x?u=anna.zorbel@firma.test&y=1` stays a valid URL. Only the address changes.
- [x] `Preis 3@4.50 EUR` and `Preis 3@Stück` stay unchanged.
- [x] Each address is replaced in `https://x.test/a.b@acme.test`, `https://a@acme.test/`, `https://x.test/share?to=a@acme.test,b@acme.test`, a JSON cell, and an HTML cell. The URL keeps its form.
- [x] `a.b=work@acme.test`, `z!q@acme.test`, and `a/b@acme.test` in free text leave no part of the real local part.
- [x] `Anna Herr`, `Kim Mba`, and the single word `Ing` are replaced as names.
- [x] `ΟΔΥΣΣΕΥΣ` and `Οδυσσευς` get one pseudonym.
- [x] A cell of 1.4 MB `a@b.cd,` and an `xn--` label of 400 KB take less than 1 second.
- [x] `anna.zorbel@localhost` and `"anna zorbel"@firma.test` are replaced.
- [x] `@bäckerei.test` and `@xn--bckerei-5wa.test` get one subdomain label.
- [x] `Dr. Hans Zorbel` keeps `Dr.`. `Ludwig van Beethoven` gives `van` to the last name. `Zorbel, Hans` keeps the comma order.
- [x] `mailto:anna.zorbel@firma.test` in an email column keeps `mailto:`.
- [x] 200 different first names with different last names give 0 collisions.

**Tests to add/update:**

- [x] `tests/free_text.rs`, `tests/csv.rs`, and unit tests in `src/text.rs`, `src/pseudonym.rs`, `src/known_names.rs` — the behaviors above.

**Commands that must pass:**

```sh
./scripts/docs-check
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

This spec changes pseudonyms of the affected values (ADR-0013). Release it as 0.2.0 with release notes.

Known limit: the collision count pairs first and last names in the order of the calls, not by row. A row with a first name and no last name can pair with the last name of the next row. This changes only the count in the summary line, never a pseudonym.
