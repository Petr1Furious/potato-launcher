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
  pip3 install --quiet tomlkit httpx
elif command -v choco >/dev/null 2>&1; then
  # ImageMagick is preinstalled on GitHub Windows runners
  choco install jq -y
  pip install tomlkit httpx
else
  echo "No supported package manager found (apt-get/dnf/brew/choco)" >&2
  exit 1
fi
