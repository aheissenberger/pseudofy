#!/usr/bin/env bash
# Run once after the devcontainer is created.
set -euo pipefail

# The volumes for ~/.claude and ~/.config/gh are created as root.
sudo chown vscode:vscode /home/vscode/.claude /home/vscode/.config/gh

# Tools for Claude Code: code search, file search, shell script lint.
sudo apt-get update
sudo apt-get install -y --no-install-recommends ripgrep fd-find shellcheck
sudo ln -sf /usr/bin/fdfind /usr/local/bin/fd
