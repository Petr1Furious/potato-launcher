#!/usr/bin/env bash
# generates packaging/flatpak/cargo-sources.json from Cargo.lock using
# flatpak-builder-tools' cargo generator (pinned to a known-good revision)
set -euo pipefail
cd "$(dirname "$0")/../.."

generator_rev="a1eb29c5f3038413ffafd4fea34e62c361c109ad"
generator="packaging/flatpak/flatpak-cargo-generator.py"

wget -O "$generator" \
  "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/${generator_rev}/cargo/flatpak-cargo-generator.py"
python3 "$generator" Cargo.lock -o packaging/flatpak/cargo-sources.json
