#!/bin/sh
# Build Trek for the simulator, launch it like shoot.sh, and record the screen to .mov —
# the recording twin of shoot.sh. Whatever the launch args put on screen is what gets filmed:
# use -TrekDemo/-TrekOpen/-TrekAppearance/-TrekTab/-TrekSheet/-TrekLink/-TrekPair to script it.
#   ios/scripts/record.sh <out.mov> [launch args…]    e.g. record.sh /tmp/a.mov -TrekOpen t-inbox -TrekExpand YES
# Env: SIM (default "iPhone 18 Pro"; App Store shots want a 6.9" Pro Max), DD (derived data,
# default /tmp/trek-ios-dd), BUILD=0 to skip the build, DUR seconds of recording after launch
# (default 8), WAIT seconds to settle before recording starts (default 1), DEMO=0 to start
# unpaired (the pairing screen), DEMO=live to relaunch a paired app.
set -eu
cd "$(dirname "$0")/.."
SIM="${SIM:-iPhone 18 Pro}"
OUT="$1"; shift
DD="${DD:-/tmp/trek-ios-dd}"
if [ "${BUILD:-1}" = 1 ]; then
  xcodebuild -project Trek.xcodeproj -scheme Trek -destination "platform=iOS Simulator,name=$SIM" \
    -derivedDataPath "$DD" build -quiet
fi
xcrun simctl boot "$SIM" 2>/dev/null || true
xcrun simctl bootstatus "$SIM" -b >/dev/null
xcrun simctl status_bar "$SIM" override --time 9:41 --batteryState charged --batteryLevel 100 --cellularBars 4 --wifiBars 3 2>/dev/null || true
xcrun simctl install "$SIM" "$DD/Build/Products/Debug-iphonesimulator/Trek.app"
xcrun simctl terminate "$SIM" dev.trek.TrekMobile 2>/dev/null || true

sleep "${WAIT:-1}"
xcrun simctl io "$SIM" recordVideo --codec h264 "$OUT" >/dev/null 2>&1 &
REC=$!
trap 'kill -INT "$REC" 2>/dev/null || true' EXIT

if [ "${DEMO:-1}" = 1 ]; then
  xcrun simctl launch "$SIM" dev.trek.TrekMobile -TrekDemo YES "$@" >/dev/null
elif [ "${DEMO}" = live ]; then
  xcrun simctl launch "$SIM" dev.trek.TrekMobile "$@" >/dev/null
else
  xcrun simctl launch "$SIM" dev.trek.TrekMobile -TrekReset YES "$@" >/dev/null
fi
sleep "${DUR:-8}"
kill -INT "$REC" 2>/dev/null || true
wait "$REC" 2>/dev/null || true
trap - EXIT
[ -s "$OUT" ] || { echo "record.sh: no video written to $OUT" >&2; exit 1; }
echo "$OUT"
