#!/bin/bash
# Build the release binary; on macOS also assemble a .app bundle (so the app
# is menu-bar only, with no Dock icon) under build/.
set -euo pipefail
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release
BIN="$PROJECT_DIR/target/release/agents-pixel-dungeon"
mkdir -p "$PROJECT_DIR/build"
if [ "$(uname)" = "Darwin" ]; then
  APP_DIR="$PROJECT_DIR/build/Agents Pixel Dungeon.app"
  rm -rf "$APP_DIR"
  mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources/Licenses"
  cp "$BIN" "$APP_DIR/Contents/MacOS/agents-pixel-dungeon"
  # The guild service and its command line, next to the app so it can start it.
  cp "$PROJECT_DIR/target/release/agents-pixel-dungeon-guild" "$APP_DIR/Contents/MacOS/agents-pixel-dungeon-guild"
  cp "$PROJECT_DIR/Resources/Info.plist" "$APP_DIR/Contents/Info.plist"
  # Cargo.toml holds the version (release-please bumps it); the bundle copies it.
  VERSION="$(grep -m1 '^version' "$PROJECT_DIR/Cargo.toml" | cut -d'"' -f2)"
  /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" "$APP_DIR/Contents/Info.plist"
  cp "$PROJECT_DIR/Resources/Icon/icon.icns" "$APP_DIR/Contents/Resources/icon.icns"
  cp "$PROJECT_DIR/Resources/Licenses/"* "$APP_DIR/Contents/Resources/Licenses/"
  cp "$PROJECT_DIR/LICENSE" "$APP_DIR/Contents/Resources/Licenses/APP-MIT.txt"
  cp "$PROJECT_DIR/NOTICE.md" "$APP_DIR/Contents/Resources/Licenses/NOTICE.md"
  # Sign with a "Developer ID Application" certificate when one is in the
  # keychain (or CODESIGN_IDENTITY names one), with the hardened runtime
  # notarization requires; ad-hoc otherwise, for local builds. Nested
  # binaries first, then the bundle. scripts/notarize.sh staples afterwards.
  IDENTITY="${CODESIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null | grep -o '"Developer ID Application: [^"]*"' | head -1 | tr -d '"' || true)}"
  if [ -n "$IDENTITY" ]; then
    codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP_DIR/Contents/MacOS/agents-pixel-dungeon-guild"
    codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP_DIR"
    echo "Signed with: $IDENTITY"
  else
    codesign --force --deep --sign - "$APP_DIR"
    echo "Signed ad-hoc (no Developer ID Application certificate in the keychain)"
  fi
  echo "Built: $APP_DIR"
else
  cp "$BIN" "$PROJECT_DIR/build/agents-pixel-dungeon"
  cp "$PROJECT_DIR/target/release/agents-pixel-dungeon-guild" "$PROJECT_DIR/build/agents-pixel-dungeon-guild"
  cp "$PROJECT_DIR/Resources/Icon/icon_256.png" "$PROJECT_DIR/build/agents-pixel-dungeon.png"
  cp "$PROJECT_DIR/Resources/agents-pixel-dungeon.desktop" "$PROJECT_DIR/build/"
  echo "Built: $PROJECT_DIR/build/agents-pixel-dungeon (with .desktop and icon)"
fi
