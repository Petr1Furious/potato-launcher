#!/usr/bin/env bash
# Publishes the launcher files in ./release to the storage target.
# Expects: LAUNCHER_PUBLISH_TARGET, LAUNCHER_NAME, VERSION, POTATO (path to the
# potato binary), plus credentials for the target: LAUNCHER_PUBLISH_SSH_KEY and
# LAUNCHER_PUBLISH_KNOWN_HOSTS for rsync://, AWS_* for s3://.
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

"$POTATO" publish-launcher \
  --target "$LAUNCHER_PUBLISH_TARGET" \
  --name "$LAUNCHER_NAME" \
  --version "$VERSION" \
  "${files[@]}"
