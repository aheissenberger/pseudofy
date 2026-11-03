# ADR-0012: Copy other project files unchanged

**Date:** 2026-11-02
**Status:** superseded by ADR-0014
**Deciders:** Andreas Heissenberger
**Related:** `docs/plan/pseudonymization.md` §2 · `docs/specs/0014-copy-other-files.md`

## Context

A project directory often has other files next to the CSV and XLSX files: a README, a PDF, images.
Today `pseudofy run` writes only the CSV and XLSX copies. The user must copy each other file by hand to get a complete output directory.
pseudofy cannot read the other files. A copied file can contain real names. The rule "pseudofy never writes a real value" covers only the files that pseudofy pseudonymizes.

## Decision

We will copy each other project file unchanged to the output directory if the config setting `copy_other_files` is `true`. The default is `false`.

- An other file is a regular file that is not a `.csv` file and not an `.xlsx` file.
- pseudofy never copies a file in `secrets/` or `config/`, a file in the output directory, a symlink, an Excel lock file (`~$*`), or a file with a path component that starts with `.`.
- pseudofy prints one line for each copied file: `<path>: copied unchanged`.
- The summary line gives the number of copied files.

## Consequences

### Positive

- The output directory can have the full project structure.
- The user does not copy files by hand.
- The default does not change: a user must turn on the copy of files that pseudofy does not check.

### Negative / Accepted costs

- A copied file can contain real names. pseudofy does not check it. The README tells the user to check each copied file before sharing.
- A file in a data format that pseudofy does not read (for example `.xls`, `.xlsm`, `.ods`, `.tsv`) is copied with its real values.
- The rule "pseudofy never writes a real value" now has an exception: files that pseudofy copies unchanged and names in its output.
- Hidden files and directories (for example `.git`, `.private_*`, `.DS_Store`) are not copied, also when the user wants them.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Default `true` | A copied file can contain real names. The user must turn on the copy of unchecked files. |
| Refuse known data formats (`.xls`, `.xlsm`, `.ods`, `.tsv`) | A fixed list is never complete. The per-file line names each copied file. |
| Copy hidden files too | `.private_*` holds secrets. `.git` can hold the history of real data. |

## Compliance & verification (optional)

`tests/cli.rs` checks the default, the setting `true`, each excluded file kind, and the printed line for each copied file.
