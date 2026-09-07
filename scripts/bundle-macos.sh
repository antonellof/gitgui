#!/usr/bin/env bash
# Build gitgui.app for macOS: the release binary, an Info.plist and an icon
# set made from assets/logo.png. Launched from Finder there is no terminal,
# so gitgui opens its desktop window. Usage: scripts/bundle-macos.sh [out-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=${1:-dist}
APP="$OUT/gitgui.app"
VER=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

cargo build --release
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/gitgui "$APP/Contents/MacOS/gitgui"

ICONSET=$(mktemp -d)/gitgui.iconset
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" assets/logo.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z "$d" "$d" assets/logo.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/gitgui.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>gitgui</string>
  <key>CFBundleDisplayName</key><string>gitgui</string>
  <key>CFBundleIdentifier</key><string>dev.antonellof.gitgui</string>
  <key>CFBundleVersion</key><string>${VER}</string>
  <key>CFBundleShortVersionString</key><string>${VER}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>gitgui</string>
  <key>CFBundleIconFile</key><string>gitgui</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
echo "built $APP ($VER)"
