#!/usr/bin/env bash
# shared helpers for uploading built launcher artifacts to the backend
# expects BACKEND_API_BASE, ADMIN_SECRET_TOKEN and VERSION env vars
set -euo pipefail

# logs in with the admin token and prints the resulting access token
backend_login() {
  local resp token
  resp="$(curl -sS --fail-with-body -X POST "$BACKEND_API_BASE/auth/login" \
    -H 'Content-Type: application/json' \
    --data "{\"token\":\"$ADMIN_SECRET_TOKEN\"}")"
  token="$(echo "$resp" | jq -r '.access_token // empty')"
  if [ -z "$token" ]; then
    echo "Failed to get access token from login response:" >&2
    echo "$resp" >&2
    return 1
  fi
  printf '%s' "$token"
}

# upload_file <token> <endpoint-path> <file>
upload_file() {
  local token="$1" endpoint="$2" file="$3"
  echo "Uploading $file -> $endpoint (version=$VERSION)"
  curl -sS --fail-with-body -X POST "$BACKEND_API_BASE/$endpoint?version=$VERSION" \
    -H "Authorization: Bearer $token" \
    -H "Content-Type: application/octet-stream" \
    --data-binary @"$file"
}
