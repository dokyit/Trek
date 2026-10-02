#!/bin/zsh
# Build Trek.app (release), its icon, an update archive and the update manifest.
# Usage: script/bundle.sh [--install]   (--install copies to /Applications)
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
OUT=dist; APP="$OUT/Trek.app"
source "$HOME/.cargo/env" 2>/dev/null || true

echo "• building release $VERSION"
cargo build --release -p trek-app -p trek-mcp

echo "• icon"
ICONSET=$(mktemp -d)/AppIcon.iconset; mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  resvg -w $s -h $s assets/brand/trek-icon-dark.svg "$ICONSET/icon_${s}x${s}.png"
  resvg -w $((s*2)) -h $((s*2)) assets/brand/trek-icon-dark.svg "$ICONSET/icon_${s}x${s}@2x.png"
done
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

echo "• bundle"
cp target/release/trek "$APP/Contents/MacOS/trek"
cp target/release/trek-mcp "$APP/Contents/MacOS/trek-mcp"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Trek</string>
  <key>CFBundleDisplayName</key><string>Trek</string>
  <key>CFBundleIdentifier</key><string>dev.trek.Trek</string>
  <key>CFBundleExecutable</key><string>trek</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict></plist>
PLIST
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 && echo "• signed (ad-hoc; use a Developer ID + notarytool for distribution)"

echo "• update archive + manifest"
ARCH=$(uname -m); [ "$ARCH" = arm64 ] && ARCH=aarch64
TAR="$OUT/Trek-$VERSION-darwin-$ARCH.app.tar.gz"
tar -czf "$TAR" -C "$OUT" Trek.app
SHA=$(shasum -a 256 "$TAR" | cut -d' ' -f1)
SIG=""; if [ -f "$HOME/.trek-signing/minisign.key" ] && command -v minisign >/dev/null; then
  minisign -S -s "$HOME/.trek-signing/minisign.key" -m "$TAR" -x "$TAR.minisig" -W && SIG=$(cat "$TAR.minisig"); fi
python3 - "$VERSION" "$(basename "$TAR")" "$SHA" "$SIG" "darwin-$ARCH" > "$OUT/stable.json" <<'PY'
import json, sys
v, name, sha, sig, plat = sys.argv[1:6]
print(json.dumps({"version": v, "notes": "", "platforms": {plat: {
  "url": f"https://github.com/trek-app/trek/releases/download/v{v}/{name}", "sha256": sha, "signature": sig}}}, indent=2))
PY
du -sh "$APP" "$TAR"
if [[ "${1:-}" == "--install" ]]; then rm -rf /Applications/Trek.app && cp -R "$APP" /Applications/ && echo "• installed to /Applications/Trek.app"; fi
