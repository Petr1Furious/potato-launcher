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
    cargo build --bin launcher --profile release-lto
    mv "target/release-lto/launcher.exe" "build/${LAUNCHER_NAME}.exe"
    ;;

  Linux)
    cargo build --bin launcher --profile release-lto
    mv "target/release-lto/launcher" "build/${LOWER_LAUNCHER_NAME}"
    ;;

  macOS)
    command -v cargo-bundle >/dev/null 2>&1 || cargo install cargo-bundle
    rustup target add x86_64-apple-darwin aarch64-apple-darwin

    MACOSX_DEPLOYMENT_TARGET=10.12 \
      cargo build --bin launcher --profile release-lto --target x86_64-apple-darwin

    MACOSX_DEPLOYMENT_TARGET=11.0 \
      cargo bundle --package launcher --bin launcher --profile release-lto --target aarch64-apple-darwin

    mkdir -p app
    app="app/${LAUNCHER_NAME}.app"
    mv "target/aarch64-apple-darwin/release-lto/bundle/osx/${LAUNCHER_NAME}.app" "$app"

    # permissions required by some mods (e.g. simple voice chat)
    plist="$app/Contents/Info.plist"
    plutil -replace NSCameraUsageDescription \
      -string "A Minecraft mod wants to access your camera." "$plist"
    plutil -replace NSMicrophoneUsageDescription \
      -string "A Minecraft mod wants to access your microphone." "$plist"
    plutil -insert NSEnableAutomaticCustomizeTouchBarMenuItem -bool false "$plist"
    plutil -insert NSFunctionBarAPIEnabled -bool false "$plist"

    # merge the Intel and Apple Silicon binaries into a universal one
    lipo -create -output "$app/Contents/MacOS/launcher" \
      "target/x86_64-apple-darwin/release-lto/launcher" \
      "$app/Contents/MacOS/launcher"

    codesign --force --deep --sign - "$app"
    ln -sf /Applications app/Applications

    # CI runners sometimes fail hdiutil with "Resource busy", so retry a few times
    dmg="${LAUNCHER_NAME}.dmg"
    for attempt in 1 2 3 4 5; do
      if hdiutil create "$dmg" -ov -volname "$LAUNCHER_NAME" -fs HFS+ -srcfolder app/; then
        break
      fi
      echo "Retrying hdiutil create... ($attempt/5)"
      sleep 5
    done
    mv "$dmg" build/

    # archive used by the automatic self-updater on macos
    mv "$app" app/update.app
    tar -czvf "build/${LOWER_LAUNCHER_NAME}_macos.tar.gz" -C app update.app
    ;;

  *)
    echo "Unsupported runner OS: $RUNNER_OS" >&2
    exit 1
    ;;
esac

os_lower="$(echo "$RUNNER_OS" | tr '[:upper:]' '[:lower:]')"
echo "$VERSION" > "build/version_${os_lower}.txt"
