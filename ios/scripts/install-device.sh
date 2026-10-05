#!/bin/sh
# Build Trek for a real iPhone and install it over USB (or the network, once paired).
#   ios/scripts/install-device.sh            # the first connected iPhone, your Xcode team
# Env: TEAM (a team id; default: the first team Xcode knows, e.g. your Personal Team),
#      DEVICE (a device UDID; default: the first available real iPhone),
#      BUNDLE (bundle id; default dev.trek.TrekMobile — a free team may need a unique one, e.g.
#      dev.trek.TrekMobile.<you>).
# With a free Apple ID the app works for 7 days; run this again to renew it. The first time, the
# iPhone asks you to trust the developer: Settings › General › VPN & Device Management.
set -eu
cd "$(dirname "$0")/.."
TEAM="${TEAM:-$(defaults read com.apple.dt.Xcode IDEProvisioningTeamByIdentifier 2>/dev/null | grep -o 'teamID = [A-Z0-9]*' | head -1 | cut -d' ' -f3)}"
if [ -z "$TEAM" ]; then
  echo "No team: add your Apple ID in Xcode › Settings › Accounts (or set TEAM=...)." >&2
  exit 1
fi
DEVICE="${DEVICE:-$(xcrun devicectl list devices 2>/dev/null | awk '/iPhone/ && !/simulated/ && (/available/ || /connected/) {for (i=1;i<=NF;i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$/) print $i}' | head -1)}"
if [ -z "$DEVICE" ]; then
  echo "No iPhone: plug it in, unlock it and trust this Mac." >&2
  exit 1
fi
BUNDLE="${BUNDLE:-dev.trek.TrekMobile}"
DD="${DD:-/tmp/trek-ios-device-dd}"
echo "Team $TEAM, iPhone $DEVICE, bundle $BUNDLE"
xcodebuild -project Trek.xcodeproj -scheme Trek -configuration Debug \
  -destination "id=$DEVICE" -derivedDataPath "$DD" -allowProvisioningUpdates \
  DEVELOPMENT_TEAM="$TEAM" PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE" CODE_SIGN_STYLE=Automatic \
  build -quiet
xcrun devicectl device install app --device "$DEVICE" "$DD/Build/Products/Debug-iphoneos/Trek.app"
xcrun devicectl device process launch --device "$DEVICE" "$BUNDLE" || \
  echo "Installed. If it won't open: Settings › General › VPN & Device Management › trust your Apple ID, then open Trek."
