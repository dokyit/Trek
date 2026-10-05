#!/bin/sh
# Archive Trek for iPhone and upload it to TestFlight.
#   ios/scripts/testflight.sh
# Needs the paid Apple Developer Program on the Apple ID in Xcode › Settings › Accounts, and an
# app in App Store Connect with this bundle id (My Apps › + › New App; any free name, e.g.
# "Trek Agents"). Then each run uploads a new build; TestFlight keeps it 90 days and offers it
# to your iPhone in the TestFlight app.
# Env: TEAM (team id; default: the first paid team Xcode knows), BUNDLE (default dev.trek.TrekMobile),
#      BUILD (build number; default the date and time, so each upload is newer).
set -eu
cd "$(dirname "$0")/.."
TEAM="${TEAM:-$(defaults read com.apple.dt.Xcode IDEProvisioningTeamByIdentifier 2>/dev/null \
  | awk '/isFreeProvisioningTeam = 0/ {paid=1} /teamID =/ && paid {gsub(/[;"]/,"",$3); print $3; exit}')}"
if [ -z "$TEAM" ]; then
  echo "No paid team in Xcode. Join the Apple Developer Program, then Xcode › Settings › Accounts › Download Manual Profiles (or set TEAM=...)." >&2
  exit 1
fi
BUNDLE="${BUNDLE:-dev.trek.TrekMobile}"
BUILD="${BUILD:-$(date +%Y%m%d%H%M)}"
OUT="${OUT:-/tmp/trek-testflight}"
rm -rf "$OUT"; mkdir -p "$OUT"
echo "Team $TEAM, bundle $BUNDLE, build $BUILD"
xcodebuild archive -project Trek.xcodeproj -scheme Trek -configuration Release \
  -destination 'generic/platform=iOS' -archivePath "$OUT/Trek.xcarchive" -allowProvisioningUpdates \
  DEVELOPMENT_TEAM="$TEAM" PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE" CODE_SIGN_STYLE=Automatic \
  CURRENT_PROJECT_VERSION="$BUILD" INFOPLIST_KEY_ITSAppUsesNonExemptEncryption=NO -quiet
cat > "$OUT/ExportOptions.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>method</key><string>app-store-connect</string>
  <key>destination</key><string>upload</string>
  <key>teamID</key><string>$TEAM</string>
  <key>signingStyle</key><string>automatic</string>
  <key>uploadSymbols</key><true/>
</dict></plist>
PLIST
xcodebuild -exportArchive -archivePath "$OUT/Trek.xcarchive" -exportOptionsPlist "$OUT/ExportOptions.plist" \
  -exportPath "$OUT/export" -allowProvisioningUpdates
echo "Uploaded build $BUILD. It shows in TestFlight once Apple has processed it (usually 5–15 minutes)."
