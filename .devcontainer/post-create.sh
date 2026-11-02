#!/usr/bin/env bash
# Run once after the devcontainer is created.
set -euo pipefail

# The volumes for ~/.claude and ~/.config/gh are created as root.
sudo chown vscode:vscode /home/vscode/.claude /home/vscode/.config/gh

# Tools for Claude Code: code search, file search, shell script lint.
sudo apt-get update
sudo apt-get install -y --no-install-recommends ripgrep fd-find shellcheck
sudo ln -sf /usr/bin/fdfind /usr/local/bin/fd

# actionlint and cargo-deny: pinned versions, prebuilt binaries, SHA-256 check.
# The checksums come from the release pages (actionlint_1.7.12_checksums.txt and
# the .sha256 asset of each cargo-deny archive). To update, change the version
# and the checksums of both architectures together (specs 0008 and 0020).
actionlint_version="1.7.12"
cargo_deny_version="0.20.2"

case "$(uname -m)" in
aarch64 | arm64)
	actionlint_arch="arm64"
	actionlint_sha256="325e971b6ba9bfa504672e29be93c24981eeb1c07576d730e9f7c8805afff0c6"
	deny_target="aarch64-unknown-linux-musl"
	deny_sha256="995c82be0defc7a025cae49a2aa2644ce8245c9a3318fc4103907c6a285e8c7d"
	;;
x86_64 | amd64)
	actionlint_arch="amd64"
	actionlint_sha256="8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8"
	deny_target="x86_64-unknown-linux-musl"
	deny_sha256="9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f"
	;;
*)
	echo "Unsupported architecture: $(uname -m)" >&2
	exit 1
	;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

actionlint_file="actionlint_${actionlint_version}_linux_${actionlint_arch}.tar.gz"
curl -fsSL -o "${tmp}/${actionlint_file}" \
	"https://github.com/rhysd/actionlint/releases/download/v${actionlint_version}/${actionlint_file}"
echo "${actionlint_sha256}  ${tmp}/${actionlint_file}" | sha256sum --check --quiet
mkdir "${tmp}/actionlint"
tar -xzf "${tmp}/${actionlint_file}" -C "${tmp}/actionlint" actionlint
sudo install -m 0755 "${tmp}/actionlint/actionlint" /usr/local/bin/actionlint

deny_name="cargo-deny-${cargo_deny_version}-${deny_target}"
curl -fsSL -o "${tmp}/${deny_name}.tar.gz" \
	"https://github.com/EmbarkStudios/cargo-deny/releases/download/${cargo_deny_version}/${deny_name}.tar.gz"
echo "${deny_sha256}  ${tmp}/${deny_name}.tar.gz" | sha256sum --check --quiet
mkdir "${tmp}/deny"
tar -xzf "${tmp}/${deny_name}.tar.gz" -C "${tmp}/deny" --strip-components=1
sudo install -m 0755 "${tmp}/deny/cargo-deny" /usr/local/bin/cargo-deny
