# Plan: pseudofy — stable pseudonymization of names and emails

**Date:** 2026-11-02
**Author:** Claude (agent), for Andreas Heissenberger

## 1. Purpose

Projects contain CSV and XLSX files with real person names and email addresses.
The user must share these files without the real data.
pseudofy replaces each name and each email address with a valid fake value.

pseudofy pseudonymizes direct identifiers. It does not anonymize the data.
The output can still be personal data (GDPR Art. 4(5)): other columns can identify a person, and the project key holder can test real names.
The README must state this limit.

## 2. Requirements

1. pseudofy is a command-line app written in Rust.
2. pseudofy reads `.csv` and `.xlsx` files.
3. pseudofy replaces person names with fake person names.
4. pseudofy replaces email addresses with fake email addresses.
5. A fake email address has a valid syntax and uses the reserved domain `example.com` (RFC 2606).
   By default, each real domain gets one fake subdomain of `example.com`. A config setting turns this off.
16. The license is Apache-2.0.
6. One real value gets the same fake value in all files of one project.
7. One real value gets the same fake value in each later run with the same project key.
8. pseudofy never changes an input file. It writes the result to an output directory.
9. The output keeps the file structure: rows, columns, sheets, headers.
10. Development runs in a devcontainer. The host needs only Docker and an editor.
11. pseudofy also replaces a known real name in text cells outside the name columns.
12. Fake names come from a German (`de`) or an English (`en`) name list.
13. `pseudofy init` writes the system language into the config. If no list exists for that language, `init` uses `en` and warns.
14. GitHub Actions builds the macOS binary for Apple Silicon (`aarch64-apple-darwin`).
15. The source is in a public GitHub repository.
17. Homebrew installs pseudofy on an Apple Silicon Mac from the tap `aheissenberger/tools`.
18. pseudofy reads CSV files in UTF-8 and in Windows-1252. The copy keeps the encoding of the input file.

## 3. Terms

| Term | Meaning |
| --- | --- |
| Project | One directory tree with the input files. |
| Project key | A random secret. pseudofy derives all fake values from it. |
| Pseudonym | The fake value for one real value. |
| Rule | A config entry. It tells pseudofy which columns contain names. |
| Name column | A column that a rule lists in `first_name`, `last_name`, or `full_name`. |
| Known name | A real name that pseudofy found in a name column of the project. |

## 4. Work breakdown

| Spec | Content |
| --- | --- |
| `docs/specs/0001-rust-cli-scaffold-and-devcontainer.md` | Cargo project, devcontainer, check script |
| `docs/specs/0002-pseudonym-engine-and-csv.md` | Project key, pseudonym engine, config, CSV files |
| `docs/specs/0003-xlsx-support.md` | XLSX files |
| `docs/specs/0004-known-names-in-free-text.md` | Known names in text cells outside the name columns |
| `docs/specs/0005-ci-and-macos-release.md` | GitHub Actions checks and macOS release binary |
| `docs/specs/0008-homebrew-formula.md` | Homebrew formula in the tap `aheissenberger/homebrew-tools` |
| `docs/specs/0009-csv-windows-1252-input.md` | CSV files in Windows-1252 |
| `docs/specs/0010-xlsx-round-trip-fidelity.md` | Parts that the XLSX copy keeps and loses |

## 5. Later work (no spec yet)

- Detect phone numbers and IBANs by pattern, like email addresses.
- Treat quasi-identifiers (birth date, postcode). Tools like ARX (https://arx.deidentifier.org/) do this with generalization and k-anonymity.
- Use larger name lists to reduce pseudonym collisions in large projects.

## 6. Decisions

- `docs/adr/0001-use-rust-for-the-cli.md`
- `docs/adr/0002-develop-in-a-devcontainer.md`
- `docs/adr/0003-derive-pseudonyms-with-a-keyed-hash.md`
- `docs/adr/0004-initial-crate-set.md`
- `docs/adr/0005-build-releases-in-github-actions.md`
- `docs/adr/0008-distribute-on-macos-with-the-homebrew-tools-tap.md`
- `docs/adr/0009-decode-csv-with-encoding-rs.md`
- `docs/adr/0010-scrub-the-written-xlsx-package.md`
