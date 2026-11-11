#!/bin/zsh
# Frames of Trek's iPhone app in its demo mode, from the simulator, at the device's own 3x.
# Uses simctl only: Simulator.app is never opened, and the device is shut down afterwards.
#   iphone_masters.sh           (expects the app built into shots/v3/ios-dd; see the xcodebuild line below)
#   xcodebuild -project ios/Trek.xcodeproj -scheme Trek -destination "platform=iOS Simulator,id=$UDID" -derivedDataPath $DD build -quiet
HERE=${0:A:h}
S=${TREK_SITE_SCRATCH:?set TREK_SITE_SCRATCH to a scratch folder for masters, frames and stills}
W=${TREK_REPO:-${HERE:h:h}}
PY=${TREK_SITE_PYTHON:-python3}   # needs Pillow and numpy
UDID=4B71E9BE-9CB8-4076-A0D8-4D5AB0741BA7   # iPhone 17e
DD=$S/shots/v3/ios-dd
OUT=$S/shots/v3/iphone
APP=$DD/Build/Products/Debug-iphonesimulator/Trek.app
BUNDLE=dev.trek.TrekMobile
mkdir -p $OUT
xcrun simctl boot $UDID 2>/dev/null || true
xcrun simctl bootstatus $UDID -b >/dev/null
xcrun simctl status_bar $UDID override --time 9:41 --batteryState charged --batteryLevel 100 --cellularBars 4 --wifiBars 3 2>/dev/null || true
xcrun simctl install $UDID $APP
shot() { # name, launch args…
  local name=$1; shift
  xcrun simctl terminate $UDID $BUNDLE 2>/dev/null || true
  xcrun simctl launch $UDID $BUNDLE -TrekDemo YES "$@" >/dev/null
  sleep 5
  xcrun simctl io $UDID screenshot $OUT/$name.png >/dev/null 2>&1 && echo "$name $(sips -g pixelWidth -g pixelHeight $OUT/$name.png | awk '/pixel/{printf "%s ", $2}')"
}
xcrun simctl launch $UDID $BUNDLE -TrekDemo YES -TrekAppearance light >/dev/null; sleep 6   # the first launch after an install is blank for a moment
shot threads-paper -TrekAppearance light
shot threads-night -TrekAppearance dark
shot basecamp-paper -TrekAppearance light -TrekTab basecamp
shot basecamp-night -TrekAppearance dark -TrekTab basecamp
shot approval-paper -TrekAppearance light -TrekOpen t-flaky
shot approval-night -TrekAppearance dark -TrekOpen t-flaky
xcrun simctl terminate $UDID $BUNDLE 2>/dev/null || true
xcrun simctl status_bar $UDID clear 2>/dev/null || true
xcrun simctl shutdown $UDID
xcrun simctl list devices | grep $UDID
