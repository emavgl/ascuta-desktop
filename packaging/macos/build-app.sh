#!/usr/bin/env bash
# Build Ascuta.app (+ a .dmg) on macOS.
#   packaging/macos/build-app.sh [version]
# Needs: Rust, Xcode command line tools. Output: dist/Ascuta-<version>-macos-<arch>.dmg
set -euo pipefail
cd "$(dirname "$0")/../.."
VERSION="${1:-v$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')}"
ARCH="$(uname -m)"
APP="dist/Ascuta.app"

cargo build --release --locked --features "capture sherpa"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/ascuta-desktop "$APP/Contents/MacOS/ascuta-desktop"
# sherpa-onnx / onnxruntime shared libraries next to the executable (rpath @executable_path)
for lib in target/release/*.dylib; do
  [ -e "$lib" ] && cp "$lib" "$APP/Contents/MacOS/"
done

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Ascuta</string>
  <key>CFBundleDisplayName</key><string>Ascuta Desktop</string>
  <key>CFBundleIdentifier</key><string>eu.ascuta.desktop</string>
  <key>CFBundleExecutable</key><string>ascuta-desktop</string>
  <key>CFBundleVersion</key><string>${VERSION#v}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION#v}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <!-- menu-bar app: no Dock icon -->
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>Ascuta listens to your voice while you hold the dictation key, and transcribes it on this Mac.</string>
</dict></plist>
PLIST

# Ad-hoc signature so Gatekeeper lets the user open it via right-click > Open.
# For distribution, replace "-" with a Developer ID identity and notarize.
codesign --force --deep -s - "$APP"

DMG="dist/Ascuta-${VERSION}-macos-${ARCH}.dmg"
rm -f "$DMG"
hdiutil create -volname "Ascuta" -srcfolder "$APP" -ov -format UDZO "$DMG"
( cd dist && shasum -a 256 "$(basename "$DMG")" > "$(basename "$DMG").sha256" )
ls -lh "$DMG"
