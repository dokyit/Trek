#!/bin/zsh
# Build and sign Trek.app (release) into dist/.
#   script/bundle.sh [--install]   (--install also copies it to /Applications)
#
# Signing (script/lib/sign.sh): $TREK_SIGN_IDENTITY, else "Trek Local Signing" (create it once with
# script/signing-identity.sh), else ad-hoc. A stable identity keeps macOS's Accessibility and
# Screen Recording grants across rebuilds; ad-hoc signatures change every build.
# Releases (archive, signature, manifest, GitHub) are script/release.sh.
set -euo pipefail
cd "${0:A:h}/.."
source script/lib/sign.sh
source "$HOME/.cargo/env" 2>/dev/null || true

VERSION=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
# CFBundleShortVersionString takes numbers only; CFBundleVersion carries the full semver, which
# the updater checks before installing.
SHORT=${VERSION%%-*}
OUT=dist; APP="$OUT/Trek.app"

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
# Menu bar glyph as a bundle resource, so menu bar managers (Shelf, …) can show Trek's real
# template icon instead of the colour app icon when they stand in for an evicted item.
resvg -w 22 -h 22 assets/brand/menubar-idle.svg "$APP/Contents/Resources/MenuBarIconTemplate.png"
resvg -w 44 -h 44 assets/brand/menubar-idle.svg "$APP/Contents/Resources/MenuBarIconTemplate@2x.png"
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
  <key>CFBundleShortVersionString</key><string>$SHORT</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict></plist>
PLIST

echo "• signing"
xattr -cr "$APP"
trek_codesign_bundle "$APP"
echo "• signed with \"$(trek_sign_identity)\" ($(trek_sign_identity_kind))"
du -sh "$APP"

if [[ "${1:-}" == "--install" ]]; then
  rm -rf /Applications/Trek.app && cp -R "$APP" /Applications/ && echo "• installed to /Applications/Trek.app"
fi
