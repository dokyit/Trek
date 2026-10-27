#!/bin/sh
# Build Trek for a real iPhone and install it over USB (or the network, once paired).
#   ios/scripts/install-device.sh            # the first connected iPhone, your Xcode team
# Env: TEAM (a team id; default: the first team Xcode knows, e.g. your Personal Team),
#      DEVICE (a device UDID; default: the first available real iPhone),
#      BUNDLE (bundle id; default dev.trek.TrekMobile — a free team may need a unique one, e.g.
#      dev.trek.TrekMobile.<you>), LAUNCH=0 (install without opening it).
# With a free Apple ID the app works for 7 days from when Xcode made its provisioning profile;
# installing again with the same profile doesn't extend that, so a profile with 2 days or less
# left is set aside first and Xcode makes a fresh one. The first time, the iPhone asks you to
# trust the developer: Settings › General › VPN & Device Management. To keep it updated (and
# signed) over Wi-Fi without running this: ios/scripts/install-phone-update.sh.
#
# Building needs Xcode (the iOS SDK and its free signing). Installing doesn't: with a trek-mcp
# that has `phone-device` (Trek 0.4.22 and later, or this repo's build; TREK_MCP=... to pick one)
# the app goes over usbmuxd as Finder's installs do, and devicectl is only the fallback.
set -eu
cd "$(dirname "$0")/.."
TREK_MCP="$(sh scripts/trek-mcp-path.sh 2>/dev/null || true)"
TEAM="${TEAM:-$(defaults read com.apple.dt.Xcode IDEProvisioningTeamByIdentifier 2>/dev/null | grep -o 'teamID = [A-Z0-9]*' | head -1 | cut -d' ' -f3)}"
if [ -z "$TEAM" ]; then
  echo "No team: add your Apple ID in Xcode › Settings › Accounts (or set TEAM=...)." >&2
  exit 1
fi
# The phone on the cable, as usbmuxd sees it; else what devicectl reaches (Wi-Fi too).
[ -z "${DEVICE:-}" ] && [ -n "$TREK_MCP" ] && \
  DEVICE="$("$TREK_MCP" phone-device list 2>/dev/null | awk '/ USB$/ {print $1; exit}')"
DEVICE="${DEVICE:-$(xcrun devicectl list devices 2>/dev/null | awk '/iPhone/ && !/simulated/ && /(^|[^a-z])(available|connected)([^a-z]|$)/ {for (i=1;i<=NF;i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$/) print $i}' | head -1)}"
if [ -z "$DEVICE" ]; then
  echo "No iPhone: plug it in, unlock it and trust this Mac." >&2
  exit 1
fi
BUNDLE="${BUNDLE:-dev.trek.TrekMobile}"
DD="${DD:-/tmp/trek-ios-device-dd}"
STATE="$HOME/Library/Application Support/dev.trek.phone-update"
# The app's version is Trek's (the workspace's, as the Mac's About says), and its build the number
# of commits, so Settings › Version tells one install from the next.
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' ../Cargo.toml | head -1)"
BUILD="$(git rev-list --count HEAD 2>/dev/null || echo 1)"
echo "Team $TEAM, iPhone $DEVICE, bundle $BUNDLE, Trek ${VERSION:-?} ($BUILD)"

# A fresh profile when Xcode's is about to lapse (it would sign this install with it).
expiry() {  # profile -> "<epoch seconds it lapses> <application identifier>"
  security cms -D -i "$1" 2>/dev/null | plutil -convert xml1 -o - - 2>/dev/null > "$DD.profile.plist" || return 0
  exp=$(plutil -extract ExpirationDate raw "$DD.profile.plist" 2>/dev/null || true)
  app=$(plutil -extract Entitlements.application-identifier raw "$DD.profile.plist" 2>/dev/null || true)
  echo "$(date -j -u -f '%Y-%m-%dT%H:%M:%SZ' "$exp" +%s 2>/dev/null || echo 0) $app"
}
for f in "$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles"/*.mobileprovision; do
  [ -f "$f" ] || continue
  set -- $(expiry "$f")
  case "${2:-}" in *".$BUNDLE") ;; *) continue ;; esac
  if [ $(( ${1:-0} - $(date +%s) )) -le $((2 * 86400)) ]; then
    mkdir -p "$STATE/old-profiles"
    mv "$f" "$STATE/old-profiles/" && echo "Signing lapses within 2 days: Xcode makes a fresh profile."
  fi
done

xcodebuild -project Trek.xcodeproj -scheme Trek -configuration Debug \
  -destination "id=$DEVICE" -derivedDataPath "$DD" -allowProvisioningUpdates \
  DEVELOPMENT_TEAM="$TEAM" PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE" CODE_SIGN_STYLE=Automatic \
  ${VERSION:+MARKETING_VERSION="$VERSION"} CURRENT_PROJECT_VERSION="$BUILD" \
  build -quiet
APP="$DD/Build/Products/Debug-iphoneos/Trek.app"
if [ -n "$TREK_MCP" ] && "$TREK_MCP" phone-device install "$APP" --udid "$DEVICE"; then
  :
else
  xcrun devicectl device install app --device "$DEVICE" "$APP"
fi
# What's on the iPhone now, and when its signing lapses: the phone updater reads it.
set -- $(expiry "$APP/embedded.mobileprovision")
mkdir -p "$STATE"
printf 'version=%s\nbuild=%s\nat=%s\nexpires=%s\ndevice=%s\n' "$VERSION" "$BUILD" "$(date +%s)" "${1:-}" "$DEVICE" > "$STATE/installed"
rm -f "$DD.profile.plist"
[ -n "${1:-}" ] && [ "$1" != 0 ] && echo "Signed until $(date -r "$1" '+%F %R')."
if [ "${LAUNCH:-1}" != 0 ]; then
  xcrun devicectl device process launch --device "$DEVICE" "$BUNDLE" || \
    echo "Installed. If it won't open: Settings › General › VPN & Device Management › trust your Apple ID, then open Trek."
fi
