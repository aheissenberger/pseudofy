# ADR-0003: Derive pseudonyms with a keyed hash

**Date:** 2026-11-02
**Status:** proposed
**Deciders:** Andreas Heissenberger
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/specs/0002-pseudonym-engine-and-csv.md`

## Context

One real value must get the same pseudonym in all files of a project.
One real value must also get the same pseudonym in later runs.
A mapping table from real values to pseudonyms contains the real data. That table needs the same protection as the input.
A plain hash without a key permits a dictionary attack on known names.
ENISA recommends a keyed hash for pseudonymisation, with the key stored apart from the data.
The result is pseudonymised data in the sense of GDPR Art. 4(5). It is not anonymous data.

## Decision

We will derive each pseudonym from HMAC-SHA256 with a project key.
The HMAC input is the kind of value (`first`, `last`, `email`, `domain`) and the normalized real value.
The HMAC output selects entries from bundled name lists.
pseudofy stores no mapping table.

Each language (`de`, `en`) has its own name lists. `config/pseudofy.toml` sets the language for the project.
`pseudofy init` writes the language once. Thus all machines give the same pseudonyms.

The project key is 32 random bytes in `secrets/pseudofy.key` in the project.
The command `pseudofy init` creates the key. pseudofy never prints the key.

## Consequences

### Positive

- Pseudonyms are stable across files and runs. No state file is necessary.
- Without the project key, nobody can test whether a real name is in the output.

### Negative / Accepted costs

- If the project key is lost, new runs give different pseudonyms.
- Two real values can get the same pseudonym. pseudofy reports each collision that it detects in one run.
- A change to the bundled name lists changes all pseudonyms. The lists are therefore versioned and frozen.
- A change of the project language changes all name pseudonyms.
- With `email_subdomains = true`, the output shows the number of real domains. It also shows which persons share one real domain.
- The project key holder can test whether a real name is in the output.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Mapping table file | The file contains the real data. It needs protection like the input. |
| Unkeyed hash (SHA-256) | A dictionary attack recovers known names. |
| Seeded random generator from the `fake` crate | The output can change with each `fake` release. |
| Format-preserving encryption | Output is not a realistic name. |

## Compliance & verification (optional)

A test runs pseudofy twice with the same key and checks that the outputs are equal.
A test runs pseudofy with two keys and checks that the outputs differ.
