#!/usr/bin/env bash
# uploads the built Flatpak bundle (and .flatpakref, if present) to the backend
set -euo pipefail
cd "$(dirname "$0")/../.."

source scripts/ci/lib.sh

token="$(backend_login)"

flatpak_file="$(ls -1 ./*.flatpak 2>/dev/null | head -n1 || true)"
if [ -z "$flatpak_file" ]; then
  echo "No .flatpak found to upload" >&2
  exit 1
fi
upload_file "$token" "launchers/linux/flatpak" "$flatpak_file"

ref_file="$(ls -1 packaging/flatpak/*.flatpakref 2>/dev/null | head -n1 || true)"
if [ -n "$ref_file" ]; then
  upload_file "$token" "launchers/linux/flatpakref" "$ref_file"
fi
