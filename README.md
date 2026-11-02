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

The checksum shows that the download is complete. For each release after 0.1.0, a provenance attestation also shows that GitHub Actions built the file from this repository:

```sh
gh attestation verify "${asset}" -R aheissenberger/pseudofy \
  --signer-workflow aheissenberger/pseudofy/.github/workflows/release.yml
```

The binary has no Apple Developer ID signature. macOS blocks a downloaded binary until you remove the quarantine attribute:

```sh
xattr -d com.apple.quarantine pseudofy
./pseudofy --version
```

Move `pseudofy` to a directory in your `PATH`, for example `/usr/local/bin`.

## Usage

1. Create the project key and the config:

   ```sh
   pseudofy init --project ./project
   ```

2. Edit `project/config/pseudofy.toml`. A comment explains each setting. A setting with `#` in front shows its default value.
3. Write the pseudonymized copies:

   ```sh
   pseudofy run --project ./project --out ./out
   ```

Keep `project/secrets/pseudofy.key` secret. The same key gives the same fake values in each later run.

pseudofy finds the header row of a CSV file or a sheet in the first 20 non-empty rows. It is the first row with a header text of a rule.
In the rows above the header (for example a title) and in the header itself, pseudofy replaces emails and known names.
If a rule matches a file and pseudofy finds none of the header texts of that rule, the file fails and pseudofy writes no copy.
In a workbook, this check runs for each sheet. Use `sheets` in the rule to skip a sheet without person data.
A CSV file with an unclosed quote also fails.

By default, `run` writes only the copies of the CSV and XLSX files.
With `copy_other_files = true` in the config, `run` also copies each other file unchanged, for example a PDF or a text file.
pseudofy does not check these files. They can contain real names. Check each copied file before you share it.
pseudofy skips hidden files and directories (names that start with `.`). It does not process or copy them.

Rules for `--out`:

- `--out` must not be inside `secrets/` or `config/` of the project.
- If `--out` is inside the project, it must be new or empty. pseudofy then writes the marker file `.pseudofy-out` into it, so later runs can use it again.
- pseudofy never overwrites an input file.
- Each copy keeps the permission bits of its input file. pseudofy warns if other users can write to `--out`. Use an output directory that only you can write to.

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
