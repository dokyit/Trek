#!/bin/sh
# Build Trek for the simulator, then launch it in demo mode and screenshot one screen.
#   ios/scripts/shoot.sh <out.png> [launch args…]     e.g. shoot.sh /tmp/a.png -TrekOpen t-flaky
# Env: SIM (default "iPhone 18 Pro"), BUILD=0 to skip the build, WAIT seconds before the shot (default 3),
#      DEMO=0 to start unpaired (the pairing screen) instead of in demo mode.
set -eu
cd "$(dirname "$0")/.."
SIM="${SIM:-iPhone 18 Pro}"
OUT="$1"; shift
DD=/tmp/trek-ios-dd
if [ "${BUILD:-1}" = 1 ]; then
  xcodebuild -project Trek.xcodeproj -scheme Trek -destination "platform=iOS Simulator,name=$SIM" \
    -derivedDataPath "$DD" build -quiet
fi
xcrun simctl boot "$SIM" 2>/dev/null || true
xcrun simctl bootstatus "$SIM" -b >/dev/null
xcrun simctl status_bar "$SIM" override --time 9:41 --batteryState charged --batteryLevel 100 --cellularBars 4 --wifiBars 3 2>/dev/null || true
xcrun simctl install "$SIM" "$DD/Build/Products/Debug-iphonesimulator/Trek.app"
xcrun simctl terminate "$SIM" dev.trek.TrekMobile 2>/dev/null || true
if [ "${DEMO:-1}" = 1 ]; then
  xcrun simctl launch "$SIM" dev.trek.TrekMobile -TrekDemo YES "$@" >/dev/null
else
  xcrun simctl launch "$SIM" dev.trek.TrekMobile -TrekReset YES "$@" >/dev/null
fi
sleep "${WAIT:-3}"
xcrun simctl io "$SIM" screenshot "$OUT" >/dev/null
echo "$OUT"
