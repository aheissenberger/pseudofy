# ADR-0014: Copy excluded and other project files unchanged

**Date:** 2026-11-03
**Status:** accepted
**Deciders:** Andreas Heissenberger
**Related:** `docs/adr/0012-copy-other-project-files-unchanged.md` · `docs/specs/0014-copy-other-files.md` · `docs/specs/0021-rule-file-lists-and-exclusions.md`

## Context

ADR-0012 copies each other file unchanged if `copy_other_files` is `true`. An other file is a file that is not a `.csv` file and not an `.xlsx` file.
Spec 0021 adds `files_excluded` to the rules. The user wants pseudofy to handle an excluded CSV or XLSX file like an other file: no pseudonymization, and a copy only with `copy_other_files = true`.
ADR-0012 does not permit an unchanged copy of a CSV or XLSX file.

## Decision

We will handle an excluded CSV or XLSX file as an other file. This ADR replaces ADR-0012 and keeps each of its rules.

- An other file is a regular file that is not a `.csv` file and not an `.xlsx` file, or a CSV or XLSX file that the rules exclude (spec 0021).
- pseudofy copies each other file unchanged to the output directory if `copy_other_files` is `true`. The default is `false`.
- pseudofy never copies a file in `secrets/` or `config/`, a file in the output directory, a symlink, an Excel lock file (`~$*`), or a file with a path component that starts with `.`.
- pseudofy prints one line for each copied file: `<path>: copied unchanged`.
- The summary line gives the number of copied files and the number of excluded files.

## Consequences

### Positive

- The user can keep files out of the pseudonymization, for example templates without person data, and still get a complete output directory.
- One rule covers all files that pseudofy does not change.

### Negative / Accepted costs

- With `copy_other_files = true`, an excluded CSV or XLSX file reaches the output with all its real values. A wrong glob in `files_excluded` leaks a data file.
- The rule "pseudofy never writes a real value" has an exception: files that pseudofy copies unchanged and names in its output.
- The README tells the user to check each copied file before sharing, and names this risk for `files_excluded`.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| No copy of an excluded file | The user wants excluded files handled like other files. |
| Email replacement only, as for a file that no rule matches | The user wants no change of an excluded file. |
| A separate setting for the copy of excluded files | Two settings for one behavior. `copy_other_files` already controls each unchanged copy. |

## Compliance & verification (optional)

`tests/cli.rs` checks an excluded file with `copy_other_files` `false` and `true`, and the tests of ADR-0012.
