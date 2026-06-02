#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

python3 packaging/configure.py
cd packaging/flatpak
python3 generate.py
