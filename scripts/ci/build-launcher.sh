#!/usr/bin/env bash
# Builds the launcher for the current runner OS into ./build
# Expects: LAUNCHER_NAME, LOWER_LAUNCHER_NAME, VERSION, RUNNER_OS
# and env vars read by crates/build-config
# (LAUNCHER_APP_ID, LAUNCHER_ICON, INSTANCE_MANIFEST_URLS, BACKEND_API_BASE)
set -euo pipefail
cd "$(dirname "$0")/../.."

mkdir -p build

case "$RUNNER_OS" in
  Windows)
    # Statically link the MSVC C runtime so the exe doesn't need
    # VCRUNTIME140.dll / the Visual C++ Redistributable.
    RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" \
      cargo build --bin launcher --profile release-lto
    mv "target/release-lto/launcher.exe" "build/${LAUNCHER_NAME}.exe"
    ;;

  Linux)
    cargo build --bin launcher --profile release-lto
    mv "target/release-lto/launcher" "build/${LOWER_LAUNCHER_NAME}"
    ;;

  macOS)
    command -v cargo-bundle >/dev/null 2>&1 || cargo install cargo-bundle --version 0.12.0 --locked
    rustup target add x86_64-apple-darwin aarch64-apple-darwin

    cargo bundle --package launcher --bin launcher --profile release-lto --format osx \
      --target aarch64-apple-darwin --target x86_64-apple-darwin
    app_name="${LAUNCHER_NAME}.app"
    app="target/universal/release-lto/bundle/osx/${app_name}"

    codesign --force --deep --sign - "$app"

    staging="target/dmg-staging"
    rm -rf "$staging" && mkdir -p "$staging"
    cp -R "$app" "$staging/"
    ln -s /Applications "$staging/Applications"
    # CI runners sometimes fail disk image creation with "Resource busy", so retry a few times
    dmg="build/${LAUNCHER_NAME}.dmg"
    for attempt in 1 2 3 4 5; do
      rm -f "$dmg"
      diskutil image create from --volumeName "$LAUNCHER_NAME" "$staging" "$dmg" && break
      [ "$attempt" = 5 ] && exit 1
      echo "Retrying diskutil image create... ($attempt/5)"
      sleep 5
    done

    # archive used by the automatic self-updater on macos, rooted at update.app/
    tar -czf "build/${LOWER_LAUNCHER_NAME}_macos.tar.gz" -C "$(dirname "$app")" \
      -s ",^${app_name},update.app," "$app_name"
    ;;

  *)
    echo "Unsupported runner OS: $RUNNER_OS" >&2
    exit 1
    ;;
esac

os_lower="$(echo "$RUNNER_OS" | tr '[:upper:]' '[:lower:]')"
echo "$VERSION" > "build/version_${os_lower}.txt"
