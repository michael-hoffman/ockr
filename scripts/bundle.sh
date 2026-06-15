#!/usr/bin/env bash
#
# bundle.sh — build ockr.app and (optionally) a distributable ockr.dmg.
#
# Uses only built-in macOS tools (sips/iconutil/hdiutil/codesign) — no
# cargo-bundle or create-dmg dependency, so it runs on any Mac with Xcode
# command-line tools.
#
# Usage:
#   scripts/bundle.sh            # build .app + .dmg (ad-hoc signed)
#   scripts/bundle.sh --app-only # build .app only, skip the .dmg
#
# Code signing:
#   By default the .app is ad-hoc signed (identity "-"), which is enough to
#   run locally and to distribute to users who right-click → Open.  To sign
#   with a real Developer ID for notarization, set:
#       export CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
#
set -euo pipefail

# ── Locate repo root (script lives in scripts/) ─────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

APP_NAME="ockr"
BIN_NAME="ockr"
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)"/\1/')"
BUNDLE_ID="dev.ockr.editor"

DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
CONTENTS="$APP/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RES_DIR="$CONTENTS/Resources"

APP_ONLY=0
[[ "${1:-}" == "--app-only" ]] && APP_ONLY=1

echo "▸ ockr $VERSION — building release binary…"
cargo build --release

echo "▸ Assembling $APP_NAME.app…"
rm -rf "$APP"
mkdir -p "$MACOS_DIR" "$RES_DIR"

# Binary.
cp "target/release/$BIN_NAME" "$MACOS_DIR/$BIN_NAME"
chmod +x "$MACOS_DIR/$BIN_NAME"

# Icon — regenerate ockr.icns from the iconset if iconutil is available,
# otherwise fall back to the checked-in assets/ockr.icns.
if [[ -d "assets/$APP_NAME.iconset" ]] && command -v iconutil >/dev/null; then
    iconutil -c icns "assets/$APP_NAME.iconset" -o "$RES_DIR/$APP_NAME.icns"
elif [[ -f "assets/$APP_NAME.icns" ]]; then
    cp "assets/$APP_NAME.icns" "$RES_DIR/$APP_NAME.icns"
else
    echo "⚠ no icon found — bundle will use the default app icon"
fi

# Info.plist.
cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>            <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>     <string>$APP_NAME</string>
    <key>CFBundleExecutable</key>      <string>$BIN_NAME</string>
    <key>CFBundleIdentifier</key>      <string>$BUNDLE_ID</string>
    <key>CFBundleVersion</key>         <string>$VERSION</string>
    <key>CFBundleShortVersionString</key> <string>$VERSION</string>
    <key>CFBundlePackageType</key>     <string>APPL</string>
    <key>CFBundleIconFile</key>        <string>$APP_NAME.icns</string>
    <key>LSMinimumSystemVersion</key>  <string>11.0</string>
    <key>NSHighResolutionCapable</key> <true/>
    <key>NSHumanReadableCopyright</key> <string>© $(date +%Y) Michael Hoffman. MIT License.</string>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>        <string>Typst Document</string>
            <key>CFBundleTypeExtensions</key>  <array><string>typ</string></array>
            <key>CFBundleTypeRole</key>        <string>Editor</string>
        </dict>
    </array>
</dict>
</plist>
PLIST

# ── Code signing ────────────────────────────────────────────────────────────
IDENTITY="${CODESIGN_IDENTITY:--}"
echo "▸ Code signing (identity: $IDENTITY)…"
codesign --force --deep --sign "$IDENTITY" "$APP"

echo "✓ Built $APP"

if [[ "$APP_ONLY" -eq 1 ]]; then
    exit 0
fi

# ── DMG ───────────────────────────────────────────────────────────────────────
DMG="$DIST/$APP_NAME-$VERSION.dmg"
echo "▸ Creating $DMG…"
rm -f "$DMG"

# Staging dir with the .app + a symlink to /Applications for drag-install.
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"

hdiutil create \
    -volname "$APP_NAME $VERSION" \
    -srcfolder "$STAGE" \
    -ov -format UDZO \
    "$DMG" >/dev/null

rm -rf "$STAGE"
echo "✓ Built $DMG"
echo ""
echo "Done. Distribute dist/$APP_NAME-$VERSION.dmg"
echo "(ad-hoc signed — users may need right-click → Open on first launch)"
