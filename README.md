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
pseudofy keeps links to other workbooks, but removes their stored values, because these values can contain real names.
Excel shows an error in a cell that uses such a link until you open the linked workbook or update the link.
pseudofy does not change the path of a linked workbook. The path can contain a user name.

## Install on macOS

pseudofy has a binary for Macs with Apple Silicon.

### Homebrew

```sh
brew install aheissenberger/tools/pseudofy
```

Homebrew checks the SHA256 of the download and adds no quarantine attribute.

### Manual download

Download the binary from the GitHub release:

```sh
version=0.1.0
asset="pseudofy-${version}-aarch64-apple-darwin.tar.gz"
curl -LO "https://github.com/aheissenberger/pseudofy/releases/download/v${version}/${asset}"
curl -LO "https://github.com/aheissenberger/pseudofy/releases/download/v${version}/${asset}.sha256"
shasum -a 256 -c "${asset}.sha256"
tar -xzf "${asset}"
```

The binary has no Apple Developer ID signature. macOS blocks a downloaded binary until you remove the quarantine attribute:

```sh
xattr -d com.apple.quarantine pseudofy
./pseudofy --version
```

Move `pseudofy` to a directory in your `PATH`, for example `/usr/local/bin`.

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
