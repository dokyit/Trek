#!/bin/sh
# Put Trek Driver (ios/TrekDriver: the small XCUITest runner the Mac's iPhone tool taps through
# while the iPhone is unlocked, on the cable or over Wi-Fi) inside a built, signed Trek.app, and write the
# .xctestrun Trek starts it with.
#   ios/scripts/build-driver.sh <signed Trek.app> <out dir>
# Called by install-device.sh, which then installs <out dir>/Trek.app instead.
#
# Why inside Trek.app rather than its own runner app: a free (Personal Team) profile installs at
# most three apps on a phone, and the driver must not take one of them. The test bundle goes into
# Trek.app/PlugIns as a hosted test's does, Trek.app gets the "continuous" background mode an
# XCTest runner has (so it keeps serving while other apps are in front), and both are signed again
# with the identity and entitlements Xcode signed Trek.app with. Opened from the Home Screen it's
# the same Trek; started by `xcodebuild test-without-building` it also serves the driver.
set -eu
APP="$1"
OUT="$2"
cd "$(dirname "$0")/.."
DD="${DRIVER_DD:-/tmp/trek-ios-driver-dd}"
BUNDLE="$(plutil -extract CFBundleIdentifier raw "$APP/Info.plist")"

# The test bundle, unsigned (it's signed below with Trek.app's identity, so it needs no profile
# or app id of its own).
xcodebuild -project TrekDriver/TrekDriver.xcodeproj -scheme TrekDriver -configuration Debug \
  -destination "generic/platform=iOS" -derivedDataPath "$DD" \
  CODE_SIGNING_ALLOWED=NO PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE.driver" build-for-testing -quiet
XCTEST="$DD/Build/Products/Debug-iphoneos/TrekDriver-Runner.app/PlugIns/TrekDriver.xctest"
XCTESTRUN="$(ls "$DD"/Build/Products/TrekDriver_*.xctestrun | head -1)"

rm -rf "$OUT"
mkdir -p "$OUT"
cp -R "$APP" "$OUT/Trek.app"
HOST="$OUT/Trek.app"
mkdir -p "$HOST/PlugIns" "$HOST/Frameworks"
cp -R "$XCTEST" "$HOST/PlugIns/"
# What loads a hosted test bundle into its app (Xcode's own hosted tests use it the same way).
cp "$(xcrun --sdk iphoneos --show-sdk-platform-path)/Developer/usr/lib/libXCTestBundleInject.dylib" "$HOST/Frameworks/"
if ! plutil -extract UIBackgroundModes raw "$HOST/Info.plist" >/dev/null 2>&1; then
  plutil -insert UIBackgroundModes -array "$HOST/Info.plist"
fi
plutil -extract UIBackgroundModes xml1 -o - "$HOST/Info.plist" | grep -q '<string>continuous</string>' || \
  plutil -insert UIBackgroundModes.0 -string continuous "$HOST/Info.plist"

# Sign again as Xcode signed Trek.app: the same identity, its entitlements.
IDENTITY="$(codesign -dvv "$APP" 2>&1 | sed -n 's/^Authority=\(Apple Development: .*\)$/\1/p' | head -1)"
if [ -z "$IDENTITY" ]; then
  echo "Couldn't tell which identity signed $APP." >&2
  exit 1
fi
codesign -d --entitlements - --xml "$APP" > "$OUT/entitlements.plist" 2>/dev/null
codesign -f -s "$IDENTITY" "$HOST/Frameworks/libXCTestBundleInject.dylib"
codesign -f -s "$IDENTITY" "$HOST/PlugIns/TrekDriver.xctest"
codesign -f -s "$IDENTITY" --entitlements "$OUT/entitlements.plist" --generate-entitlement-der "$HOST"
rm -f "$OUT/entitlements.plist"

# The run: Trek.app is the test host; Trek adds the port, a fresh token and TREK_DRIVER=1 to a
# copy of it each time it starts the driver.
RUN="$OUT/TrekDriver.xctestrun"
cp "$XCTESTRUN" "$RUN"
plutil -replace TrekDriver.TestHostPath -string "__TESTROOT__/Trek.app" "$RUN"
plutil -replace TrekDriver.TestHostBundleIdentifier -string "$BUNDLE" "$RUN"
plutil -replace TrekDriver.TestBundlePath -string "__TESTHOST__/PlugIns/TrekDriver.xctest" "$RUN"
plutil -replace TrekDriver.DependentProductPaths -json '["__TESTROOT__/Trek.app","__TESTROOT__/Trek.app/PlugIns/TrekDriver.xctest"]' "$RUN"
plutil -replace TrekDriver.IsAppHostedTestBundle -bool YES "$RUN"
plutil -replace TrekDriver.IsXCTRunnerHostedTestBundle -bool NO "$RUN"
plutil -replace TrekDriver.TestingEnvironmentVariables.DYLD_INSERT_LIBRARIES -string "__TESTHOST__/Frameworks/libXCTestBundleInject.dylib" "$RUN"
plutil -replace TrekDriver.TestingEnvironmentVariables.XCInjectBundleInto -string unused "$RUN"
plutil -replace TrekDriver.ProcessNamesForCrashReportCollection -json '["Trek"]' "$RUN"
plutil -replace TrekDriver.PreferredScreenCaptureFormat -string screenshots "$RUN"
plutil -replace TrekDriver.SystemAttachmentLifetime -string keepNever "$RUN"
plutil -replace TrekDriver.UserAttachmentLifetime -string keepNever "$RUN"
plutil -extract CFBundleVersion raw "$HOST/Info.plist" > "$OUT/build"
# What this driver can do, for the Mac's Trek (a driver built before a feature lacks its line):
# wifi, it serves over the local network when Trek names the Mac's addresses (TREK_DRIVER_PEERS).
echo wifi > "$OUT/features"
echo "Trek Driver: $HOST, $RUN"
