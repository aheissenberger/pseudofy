# ADR-0013: Normalize text before hashing (version 2)

**Date:** 2026-11-02
**Status:** accepted
**Deciders:** Andreas Heissenberger
**Related:** `docs/adr/0003-derive-pseudonyms-with-a-keyed-hash.md` · `docs/specs/0004-known-names-in-free-text.md` · `docs/specs/0019-text-normalization.md` · `docs/plan/pseudonymization.md` §2 (requirements 6, 7)

## Context

A review found that one real value can get two pseudonyms, and that invisible characters let a real name through:
- Name columns use NFC, trim, and lowercase. Free text also maps `’` to `'`, `ß` to `ss`, and removes U+0307. `O’Brien` and `O'Brien` get two pseudonyms. `Strauß` and `STRAUSS` get two pseudonyms.
- A soft hyphen or a zero-width character splits a name in two tokens. The output still shows the real name.
- An IDN domain and its punycode form get two subdomain labels.

Requirement 7 says that one real value gets the same pseudonym in each later run. A change of the normalization changes the pseudonyms of the affected values.

## Decision

We will use one normalization for name columns, free text, and email domains, and accept the change of the affected pseudonyms in release 0.2.0.

- Remove each Unicode format character (category Cf), for example U+00AD and U+200B.
- Apply NFC. Map `’` and `ʼ` to `'`. Map `ß` to `ss`. Remove U+0307 after `i`. Lowercase.
- Convert each punycode label of a domain (`xn--`) to Unicode.
- `tests/pseudonym_stability.rs` stays unchanged. Its values contain none of the affected characters.

## Consequences

### Positive

- One person gets one pseudonym in CSV and XLSX, in name columns and in free text.
- Invisible characters do not hide a name.

### Negative / Accepted costs

- A project from release 0.1.0 gets new pseudonyms for values with `’`, `ß`, `İ`, a format character, or a punycode domain. The README tells the user in the release notes.
- `Strauß` and `Strauss` get the same pseudonym. Two persons with these names become one person in the output.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Keep the old normalization for name columns | One person keeps two pseudonyms. |
| A config setting for the normalization version | More code and tests for one release with few users. |
| Remove diacritics (`ö` to `o`) | Too many different names become equal. |

## Compliance & verification (optional)

`tests/pseudonym_stability.rs` checks the values that do not change. Unit tests in `src/pseudonym.rs` check that each pair above gets one pseudonym.
