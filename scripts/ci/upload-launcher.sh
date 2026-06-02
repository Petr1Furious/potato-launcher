#!/usr/bin/env bash
# uploads the built launcher artifacts for the current runner OS to the backend
set -euo pipefail
cd "$(dirname "$0")/../.."

source scripts/ci/lib.sh

token="$(backend_login)"

case "$RUNNER_OS" in
  Windows)
    upload_file "$token" "launchers/windows/exe" "build/${LAUNCHER_NAME}.exe"
    ;;
  Linux)
    upload_file "$token" "launchers/linux/bin" "build/${LOWER_LAUNCHER_NAME}"
    ;;
  macOS)
    upload_file "$token" "launchers/macos/dmg" "build/${LAUNCHER_NAME}.dmg"
    upload_file "$token" "launchers/macos/archive" "build/${LOWER_LAUNCHER_NAME}_macos.tar.gz"
    ;;
  *)
    echo "Unsupported runner OS: $RUNNER_OS" >&2
    exit 1
    ;;
esac
