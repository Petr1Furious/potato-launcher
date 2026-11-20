#!/usr/bin/env bash
# Needs LAUNCHER_PUBLISH_TARGET, LAUNCHER_NAME, VERSION and POTATO (the potato binary),
# plus LAUNCHER_PUBLISH_SSH_KEY and LAUNCHER_PUBLISH_KNOWN_HOSTS for rsync:// or AWS_* for s3://.
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ -n "${LAUNCHER_PUBLISH_SSH_KEY:-}" ]; then
  mkdir -p ~/.ssh
  chmod 700 ~/.ssh
  printf '%s\n' "$LAUNCHER_PUBLISH_SSH_KEY" > ~/.ssh/id_publish
  chmod 600 ~/.ssh/id_publish
  printf '%s\n' "${LAUNCHER_PUBLISH_KNOWN_HOSTS:-}" >> ~/.ssh/known_hosts
  printf 'Host *\n  IdentityFile ~/.ssh/id_publish\n' >> ~/.ssh/config
fi

shopt -s nullglob
files=(release/launcher/*)
if [ ${#files[@]} -eq 0 ]; then
  echo "No launcher files in release/launcher" >&2
  exit 1
fi

extra=()
icon="${LAUNCHER_ICON:-$(grep -E '^LAUNCHER_ICON=' build.env | head -n1 | cut -d= -f2-)}"
if [[ "$icon" == http*://* ]]; then
  curl -fsSL -o "release/icon.${icon##*.}" "$icon"
  icon="release/icon.${icon##*.}"
fi
if [ -n "$icon" ] && [ -f "$icon" ]; then
  extra+=(--icon "$icon")
fi
if [ -n "${GITHUB_REPOSITORY:-}" ]; then
  extra+=(--source "${GITHUB_SERVER_URL:-https://github.com}/$GITHUB_REPOSITORY")
fi

"$POTATO" publish-launcher \
  --target "$LAUNCHER_PUBLISH_TARGET" \
  --name "$LAUNCHER_NAME" \
  --version "$VERSION" \
  "${extra[@]}" \
  "${files[@]}"
