#!/usr/bin/env bash
set -euo pipefail

if command -v apt-get >/dev/null 2>&1; then
  sudo apt-get update
  sudo apt-get install -y --no-install-recommends \
    python3 python3-tomlkit python3-httpx imagemagick jq
elif command -v dnf >/dev/null 2>&1; then
  dnf install -y python3 python3-tomlkit python3-httpx ImageMagick jq
elif command -v brew >/dev/null 2>&1; then
  brew install imagemagick jq
  venv_dir="${RUNNER_TEMP:-/tmp}/potato-packaging-venv"
  python3 -m venv "$venv_dir"
  "$venv_dir/bin/pip" install --quiet tomlkit httpx
  if [ -n "${GITHUB_PATH:-}" ]; then
    echo "$venv_dir/bin" >> "$GITHUB_PATH"
  fi
  export PATH="$venv_dir/bin:$PATH"
elif command -v choco >/dev/null 2>&1; then
  # ImageMagick is preinstalled on GitHub Windows runners
  choco install jq -y
  pip install tomlkit httpx
else
  echo "No supported package manager found (apt-get/dnf/brew/choco)" >&2
  exit 1
fi
