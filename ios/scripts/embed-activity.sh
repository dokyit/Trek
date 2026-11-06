#!/bin/sh
# Put Trek's Live Activity (ios/TrekActivity, a WidgetKit extension) inside a built Trek.app.
#   ios/scripts/embed-activity.sh <Trek.app> <derived data> [xcodebuild arguments…]
#   ios/scripts/embed-activity.sh --remove <Trek.app>
# Called by install-device.sh and phone-update.sh, which pass the signing (for a simulator:
# -destination 'generic/platform=iOS Simulator' CODE_SIGNING_ALLOWED=NO).
#
# Why it isn't a dependency of the Trek scheme: an extension needs an app id and a profile of its
# own, and a free (Personal Team) signing counts it as one of the three apps it allows on a phone.
# On a phone with no room left, the extension failing must not stop Trek installing: the callers
# build Trek as ever, add this, and install without it (--remove) if the phone refuses.
set -eu
resign() {
  # Sign again as Xcode signed Trek.app: the same identity, its entitlements. A simulator build
  # isn't signed for a person, and needs nothing.
  IDENTITY="$(codesign -dvv "$1" 2>&1 | sed -n 's/^Authority=\(Apple Development: .*\)$/\1/p' | head -1)"
  [ -n "$IDENTITY" ] || return 0
  ENTITLEMENTS="$(mktemp)"
  codesign -d --entitlements - --xml "$1" > "$ENTITLEMENTS" 2>/dev/null
  codesign -f -s "$IDENTITY" --entitlements "$ENTITLEMENTS" --generate-entitlement-der "$1"
  rm -f "$ENTITLEMENTS"
}
if [ "$1" = --remove ]; then
  rm -rf "$2/PlugIns/TrekActivity.appex"
  resign "$2"
  exit 0
fi
APP="$1"
DD="$2"
shift 2
cd "$(dirname "$0")/.."
xcodebuild -project Trek.xcodeproj -scheme TrekActivity -configuration Debug -derivedDataPath "$DD" "$@" build -quiet
case "$APP" in
  *iphonesimulator*) PRODUCTS="$DD/Build/Products/Debug-iphonesimulator" ;;
  *) PRODUCTS="$DD/Build/Products/Debug-iphoneos" ;;
esac
mkdir -p "$APP/PlugIns"
rm -rf "$APP/PlugIns/TrekActivity.appex"
cp -R "$PRODUCTS/TrekActivity.appex" "$APP/PlugIns/"
resign "$APP"
echo "Live Activity: $APP/PlugIns/TrekActivity.appex"
