# Feature Delta Spec: Pseudonym engine and CSV files

**Date:** 2026-11-02
**Status:** active
**Author:** Claude (agent)
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/specs/0001-rust-cli-scaffold-and-devcontainer.md` · `docs/adr/0003-derive-pseudonyms-with-a-keyed-hash.md` · `docs/adr/0004-initial-crate-set.md`

---

## 1. Objective & Context

**Problem:** CSV files in a project contain real names and email addresses.
**Today:** `pseudofy init` and `pseudofy run` print "not implemented".
**After this change:** `init` creates a project key and a config. `run` writes pseudonymized copies of all CSV files.

## 2. Scope

### Added

- `src/key.rs` — create and load `secrets/pseudofy.key` (32 random bytes, hex). `init` fails if the file exists.
- `src/config.rs` — load `config/pseudofy.toml`. The key `language` is `de` or `en`. The optional key `email_subdomains` is `true` or `false`. The default is `true`. Each `[[rule]]` has `files` (glob) and the column lists `first_name`, `last_name`, `full_name`, `email` (header text).
- `src/pseudonym.rs` — HMAC-SHA256 mapping for the kinds `first`, `last`, `email`, `domain` (ADR-0003). The input is NFC-normalized, trimmed, and lowercased.
- `src/names.rs`, `data/de/first-names.txt`, `data/de/last-names.txt`, `data/en/first-names.txt`, `data/en/last-names.txt` — frozen name lists, embedded with `include_str!`. Each list has at least 1000 entries.
- `src/locale.rs` — read the system language with `sys-locale`. Map it to `de` or `en`. Any other language gives `en` and a warning.
- `src/text.rs` — split a full name into tokens. "First Middle Last" and "Last, First" are supported. Each token keeps its case style (`Anna`, `ANNA`, `anna`).
- `src/csv_file.rs` — read a CSV file, replace values, write the copy. The delimiter (`,` `;` tab) is detected from the header line.
- `src/run.rs` — find files under `--project`, apply rules, write to `--out`, print a summary.
- `tests/csv.rs`, `tests/fixtures/` — CSV tests with invented data only.

### Modified

- `src/main.rs` — `init --project <DIR> [--language de|en]` and `run --project <DIR> --out <DIR>` call the new modules.
- `init` writes `config/pseudofy.toml` with the language, `email_subdomains = true`, and one example rule. `init` fails if the file exists.
- `Cargo.toml` — add the crates from ADR-0004 except `umya-spreadsheet`.
- `tests/cli.rs` — remove the test for the "not implemented" output.
- `README.md` — state that pseudofy does not pseudonymize file and directory names.

### Removed

- The "not implemented" output for `init` and `run`.

### Out of scope (explicitly)

- XLSX files (spec 0003).
- Name detection without a rule (free text, heuristics).
- Known names in text cells outside the name columns (spec 0004).

## 3. Architectural Rules & Invariants

- [ ] pseudofy never writes to an input file. If `--out` is inside `--project` and not excluded, `run` fails.
- [ ] `run` skips `secrets/`, `config/`, and `--out` during the file search.
- [ ] pseudofy never prints, logs, or writes the project key or a real value.
- [ ] File and directory names are not real values in this spec. `run` copies them to `--out` unchanged. Messages can show the relative path.
- [ ] Equal normalized real values give equal pseudonyms in all files.
- [ ] A pseudonym is never equal to its real value.
- [ ] If `email_subdomains` is `false`, each fake email is `<first>.<last>@example.com`.
- [ ] If `email_subdomains` is `true`, each fake email is `<first>.<last>@<label>.example.com`.
- [ ] Each fake email is ASCII and valid.
- [ ] `<first>.<last>` comes from the `email` pseudonym of the full address. `<label>` comes from the `domain` pseudonym of the real domain.
- [ ] `<label>` is a pronounceable DNS label of 6 to 10 lowercase letters. It is made from the HMAC output.
- [ ] Equal real domains (case-insensitive) give one label. Different real domains give different labels, unless `run` reports a collision.
- [ ] The email local part transliterates German letters: `ä`→`ae`, `ö`→`oe`, `ü`→`ue`, `ß`→`ss`.
- [ ] `run` reads the language only from the config. `run` never reads the system language.
- [ ] `run` replaces emails in each cell of each column, not only in `email` columns.
- [ ] The output keeps row count, column count, header, delimiter, and line ending.
- [ ] Tests use only invented data and temporary directories.

## 4. Acceptance Criteria & Verification

**Behavior:**

- [ ] A name in two files gets the same pseudonym. `Anna` in `first_name` and `Anna Muster` in `full_name` share the first-name pseudonym.
- [ ] Two runs with one key give byte-equal output. Two keys give different output.
- [ ] With `email_subdomains = false`, `a@acme.test` and `c@other.test` both get the domain `example.com`.
- [ ] Without `email_subdomains`, `a@acme.test` and `b@ACME.test` get one fake subdomain. `c@other.test` gets a different fake subdomain.
- [ ] `run` without `secrets/pseudofy.key` exits with a non-zero code and tells the user to run `pseudofy init`.
- [ ] `run` prints the number of files, replaced values, and detected collisions.
- [ ] `init` on a system with language `fr` writes `language = "en"` and prints a warning.
- [ ] `init --language de` writes `language = "de"` and ignores the system language.

**Tests to add/update:**

- [ ] `src/pseudonym.rs` unit tests — stability, key dependence, no identity mapping, domain label format.
- [ ] `src/text.rs` unit tests — token split, case style, transliteration.
- [ ] `src/locale.rs` unit tests — language mapping and fallback.
- [ ] `tests/csv.rs` — end-to-end runs on `tests/fixtures/`.

**Commands that must pass:**

```sh
devcontainer exec --workspace-folder . ./scripts/check
```

## 5. Rollback / Notes (optional)

The name lists need a license that permits redistribution. Use public-domain census lists or write new lists.
