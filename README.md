# pseudofy

pseudofy replaces person names and email addresses in CSV and XLSX files with stable fake values.
One real value gets the same fake value in all files of one project and in each later run.

pseudofy pseudonymizes direct identifiers. It does not anonymize the data.
The output can still be personal data (GDPR Art. 4(5)).
Other columns can identify a person, and the holder of the project key can test real names.
pseudofy does not change file and directory names. Rename a file before you share it if its name contains a real name.
In XLSX files, pseudofy does not change comments, charts, data validations, headers and footers, conditional formatting, sheet names, defined names, and document properties.
pseudofy prints a warning for each workbook that has one of these parts, except sheet names, defined names, and document properties. Check these parts before you share the file.
pseudofy refuses workbooks with pivot tables.

## Development

Development runs in a devcontainer. The host needs Docker and an editor.

1. Open the folder in VS Code.
2. Select "Reopen in Container".

For the command line, install the devcontainer CLI on the host:

```sh
npm install -g @devcontainers/cli
devcontainer up --workspace-folder .
```

## Checks

Run all checks in the devcontainer:

```sh
devcontainer exec --workspace-folder . ./scripts/check
```

The script runs `cargo fmt --check`, `cargo clippy`, `cargo test`, and `./scripts/docs-check`.

## License

pseudofy is licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).
