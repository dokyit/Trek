#!/bin/zsh
# Build and sign Trek.app (release) into dist/.
#   script/bundle.sh [--install]   (--install also copies it to /Applications)
#
# TREK_TARGET=x86_64-apple-darwin builds the app for that architecture instead of this Mac's: the
# Intel build a release ships beside the Apple-silicon one (docs/RELEASING.md, "Intel Macs"). On
# Apple silicon it needs Rosetta for one step, the iPhone helper below.
#
# Signing (script/lib/sign.sh): $TREK_SIGN_IDENTITY, else "Trek Local Signing" (create it once with
# script/signing-identity.sh), else "Shelf Dev", else ad-hoc. A stable identity keeps macOS's
# Accessibility and Screen Recording grants across rebuilds; ad-hoc signatures change every build.
# Releases (archive, signature, manifest, GitHub) are script/release.sh. A bundle built here doesn't
# update itself (TREK_UPDATE_LOCAL_BUNDLE=1 at run time lets it, for testing the updater).
set -euo pipefail
cd "${0:A:h}/.."
source script/lib/sign.sh
source "$HOME/.cargo/env" 2>/dev/null || true

VERSION=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
# CFBundleShortVersionString takes numbers only; CFBundleVersion carries the full semver, which
# the updater checks before installing.
SHORT=${VERSION%%-*}
OUT=dist; APP="$OUT/Trek.app"
# Only release.sh marks a bundle as a release. One built here for yourself isn't: the updater
# treats it as a development build, so it never swaps itself for a published build of older code.
RELEASE_KEY=""
if [[ ${TREK_RELEASE_BUILD:-} == 1 ]]; then RELEASE_KEY="<key>TrekRelease</key><true/>"; fi

# Local bundles skip thin LTO ([profile.bundle] in Cargo.toml): the link was about a minute
# of this script's time. Only release.sh's builds (TREK_RELEASE_BUILD=1) keep --release.
TARGET_DIR=${CARGO_TARGET_DIR:-target}
PROFILE=bundle
if [[ ${TREK_RELEASE_BUILD:-} == 1 ]]; then PROFILE=release; fi
# Cargo keeps a --target build's output a folder deeper.
BIN_DIR="$TARGET_DIR/$PROFILE"
CARGO_TARGET=()
if [[ -n ${TREK_TARGET:-} ]]; then
  BIN_DIR="$TARGET_DIR/$TREK_TARGET/$PROFILE"
  CARGO_TARGET=(--target "$TREK_TARGET")
  # The oldest macOS Trek runs on, as Info.plist says below. Left alone, rustc aims an Intel build
  # at 10.12, for which the linker looks for a Swift library in the app (@rpath) and Trek wouldn't
  # start: "Library not loaded: @rpath/libswiftCoreMedia.dylib".
  export MACOSX_DEPLOYMENT_TARGET=13.0
fi
echo "• building $PROFILE $VERSION${TREK_TARGET:+ for $TREK_TARGET}"
cargo build --profile "$PROFILE" -p trek-app -p trek-mcp "${CARGO_TARGET[@]}"

echo "• icon"
# The artwork below all derives from assets/brand/*.svg; rerender only when those files, the
# resvg version, or the sizes change. The cache lives under the target dir, so deleting dist/
# doesn't cost a rerender either.
ICON_CACHE="$TARGET_DIR/icons"
ICON_KEY=$( { shasum -a 256 assets/brand/*.svg; resvg --version; print -r -- "iconset 16 32 128 256 512 @2x, 16 32 px drawn small; menubar 22 44"; } | shasum -a 256 | cut -d' ' -f1 )
if [[ $(cat "$ICON_CACHE/stamp" 2>/dev/null) != "$ICON_KEY" \
      || ! -f "$ICON_CACHE/AppIcon.icns" \
      || ! -f "$ICON_CACHE/MenuBarIconTemplate.png" \
      || ! -f "$ICON_CACHE/MenuBarIconTemplate@2x.png" ]]; then
  mkdir -p "$ICON_CACHE"
  ICONSET=$(mktemp -d)/AppIcon.iconset; mkdir -p "$ICONSET"
  pids=()
  # The art for a frame of $1 px: 16 and 32 px (16 pt at 1x and 2x, 32 pt at 1x) are drawn for
  # their size, the mountain on whole pixels (trek_icon.py), as the full art blurs there. From
  # 64 px up it's the full art. Night is the bundle's icon; the others go up at run time.
  art() { case $1 in 16|32) print -r -- assets/brand/trek-icon-night-$1.svg ;; *) print -r -- assets/brand/trek-icon-night.svg ;; esac }
  for s in 16 32 128 256 512; do
    resvg -w $s -h $s "$(art $s)" "$ICONSET/icon_${s}x${s}.png" & pids+=($!)
    resvg -w $((s*2)) -h $((s*2)) "$(art $((s*2)))" "$ICONSET/icon_${s}x${s}@2x.png" & pids+=($!)
  done
  for p in $pids; do wait $p; done
  iconutil -c icns "$ICONSET" -o "$ICON_CACHE/AppIcon.icns"
  pids=()
  resvg -w 22 -h 22 assets/brand/menubar-idle.svg "$ICON_CACHE/MenuBarIconTemplate.png" & pids+=($!)
  resvg -w 44 -h 44 assets/brand/menubar-idle.svg "$ICON_CACHE/MenuBarIconTemplate@2x.png" & pids+=($!)
  for p in $pids; do wait $p; done
  print -r -- "$ICON_KEY" > "$ICON_CACHE/stamp"
fi
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ICON_CACHE/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"

echo "• bundle"
cp "$BIN_DIR/trek" "$APP/Contents/MacOS/trek"
cp "$BIN_DIR/trek-mcp" "$APP/Contents/MacOS/trek-mcp"
# The iPhone tool's helper (screen over USB, Bluetooth keyboard and mouse), built here so a
# person installing Trek needs no Swift compiler; without one here, Trek builds it on first use.
# Its Info.plist is linked in as `trek_phone::helper::build` does, for the architecture trek-mcp was
# built for (an Intel bundle's trek-mcp runs under Rosetta on Apple silicon).
if ! "$BIN_DIR/trek-mcp" phone-device build-helper "$APP/Contents/MacOS/trek-phone-helper"; then
  echo "• (no iPhone helper bundled: Trek builds it on first use)"
fi
# A build for another architecture holds nothing of this Mac's, and needs no library the app doesn't
# hold: a mistake here would reach users as a Trek that won't open, with nothing here to show it.
if [[ -n ${TREK_TARGET:-} ]]; then
  for exe in "$APP"/Contents/MacOS/*; do
    [[ $(lipo -archs "$exe") == "${TREK_TARGET%%-*}" ]] || { echo "bundle: ${exe:t} isn't only ${TREK_TARGET%%-*}" >&2; exit 1; }
    if otool -L "$exe" | grep -q '@rpath/'; then
      echo "bundle: ${exe:t} looks for a library in the app, which holds none: $(otool -L "$exe" | grep '@rpath/')" >&2
      exit 1
    fi
  done
fi
# Menu bar glyph as a bundle resource, so menu bar managers (Shelf, …) can show Trek's real
# template icon instead of the colour app icon when they stand in for an evicted item.
cp "$ICON_CACHE/MenuBarIconTemplate.png" "$APP/Contents/Resources/MenuBarIconTemplate.png"
cp "$ICON_CACHE/MenuBarIconTemplate@2x.png" "$APP/Contents/Resources/MenuBarIconTemplate@2x.png"
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
  <key>NSMicrophoneUsageDescription</key><string>Trek records the mic so Speech can transcribe what you dictate into the composer, and for a web page in its Browser that you let use it.</string>
  <key>NSSpeechRecognitionUsageDescription</key><string>Trek transcribes what you dictate into the composer.</string>
  <key>NSLocationUsageDescription</key><string>A web page in Trek's Browser asked for your location; Trek asks you first, for each site.</string>
  <key>NSCameraUsageDescription</key><string>Trek shows your iPhone's screen in its iPhone tool (it comes over the USB cable as a camera's picture does), and the camera to a web page in its Browser that you let use it.</string>
  <key>NSBluetoothAlwaysUsageDescription</key><string>Trek is a Bluetooth keyboard and mouse for your iPhone, so you and your agents can tap and type on it from the iPhone tool.</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>dev.trek.Trek</string>
      <key>CFBundleURLSchemes</key><array><string>trek</string></array>
    </dict>
  </array>
  $RELEASE_KEY
</dict></plist>
PLIST

echo "• signing"
xattr -cr "$APP"
trek_codesign_bundle "$APP"
echo "• signed with \"$(trek_sign_identity)\" ($(trek_sign_identity_kind))"
du -sh "$APP"

if [[ "${1:-}" == "--install" ]]; then
  # A bundle replaced under a running Trek leaves it without its Accessibility and Screen
  # Recording permissions until it restarts (macOS reads them through the file it runs from).
  if ps -axo comm= | grep -qx "/Applications/Trek.app/Contents/MacOS/trek"; then
    echo "• Trek is running from /Applications: quit it first (a bundle replaced under it loses its permissions until it restarts)" >&2
    exit 1
  fi
  # ditto without extended attributes: Finder and file-provider metadata on a synced folder
  # would otherwise follow the app and trip strict signature checks.
  rm -rf /Applications/Trek.app && ditto --noextattr --norsrc "$APP" /Applications/Trek.app && echo "• installed to /Applications/Trek.app"
fi
