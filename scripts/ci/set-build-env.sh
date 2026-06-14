#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/../.."

name="${LAUNCHER_NAME:-}"
if [ -z "$name" ] && [ -f build.env ]; then
  name="$(grep -E '^LAUNCHER_NAME=' build.env | head -n1 | cut -d= -f2-)"
fi
if [ -z "$name" ]; then
  echo "LAUNCHER_NAME is not set (workflow vars or build.env)" >&2
  exit 1
fi

lower="$(echo "$name" | tr '[:upper:]' '[:lower:]' | tr -d "'" | tr ' ' '_')"

{
  echo "LAUNCHER_NAME=$name"
  echo "LOWER_LAUNCHER_NAME=$lower"
  echo "VERSION=$GITHUB_SHA"
} >> "$GITHUB_ENV"
