#!/bin/zsh
# Keeps Trek on the iPhone up to date without a cable. Run every 30 minutes by the LaunchAgent
# ~/Library/LaunchAgents/dev.trek.phone-update.plist (ios/scripts/install-phone-update.sh puts it
# there); safe to run by hand. Logs to ~/Library/Logs/trek-phone-update.log.
#
# The app is signed with a free Apple ID: no App Store, no TestFlight, so it can't update itself,
# and its signing lasts 7 days from when Xcode made the provisioning profile (installing again
# with the same profile doesn't extend it). So, when the iPhone is reachable (USB, or Wi-Fi once
# paired with Xcode), this Mac builds the app and installs it:
#   - when Trek on this Mac is newer than the app on the iPhone (built at the Mac's version tag);
#   - when the signing has 2 days or less left (about 5 days in): the old profile is set aside so
#     Xcode makes a fresh one, good for 7 more days.
# It waits while a build runs (xcodebuild; cargo too, unless the signing is about to lapse) and
# while Trek is open on the iPhone (the Mac sees its connection), so it doesn't pull the app out
# from under you.
#
#   phone-update.sh            check, and update if due
#   phone-update.sh --status   say what it would do, change nothing
#   phone-update.sh --force    build and install now (still not over a build or an open app)
# Off: touch "~/Library/Application Support/dev.trek.phone-update/off" (or uninstall the agent:
# ios/scripts/install-phone-update.sh --uninstall).
set -u
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
STATE="$HOME/Library/Application Support/dev.trek.phone-update"
REPO="${TREK_REPO:-$HOME/Developer/Trek}"
MAC_APP="${TREK_MAC_APP:-/Applications/Trek.app}"
DEVICES="$HOME/Library/Application Support/dev.trek.Trek/mobile/devices.json"
BUNDLE="${BUNDLE:-dev.trek.TrekMobile}"
PROFILES="$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles"
LOG="$HOME/Library/Logs/trek-phone-update.log"
RENEW=$((2 * 86400))    # renew the signing with this much (or less) left
URGENT=$((12 * 3600))   # this close to lapsing, don't wait for builds or an open app
MODE="${1:-}"
log() { print -r -- "$(date '+%F %T') $*" }

mkdir -p "$STATE"
# Keep the log short.
if [[ -f "$LOG" ]] && (( $(stat -f %z "$LOG") > 1048576 )); then
  tail -n 2000 "$LOG" > "$LOG.tmp" && mv "$LOG.tmp" "$LOG"
fi
if [[ -e "$STATE/off" && "$MODE" != --force ]]; then
  log "off ($STATE/off)"
  exit 0
fi

# One at a time.
if ! mkdir "$STATE/lock" 2>/dev/null; then
  other=$(cat "$STATE/lock/pid" 2>/dev/null)
  if [[ -n "$other" ]] && kill -0 "$other" 2>/dev/null; then
    log "already running ($other)"
    exit 0
  fi
  rm -rf "$STATE/lock"
  mkdir "$STATE/lock" || exit 0
fi
print $$ > "$STATE/lock/pid"
TMP=$(mktemp -d)
trap 'rm -rf "$STATE/lock" "$TMP"' EXIT

# 1. The iPhone, reachable now: over USB, or the network (Xcode pairs it once over USB).
xcrun devicectl list devices > "$TMP/devices.txt" 2>/dev/null
UDID="${DEVICE:-$(awk '/iPhone/ && /physical/ && /(^|[^a-z])(available|connected)([^a-z]|$)/ {for (i=1;i<=NF;i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$/) print $i}' "$TMP/devices.txt" | head -1)}"
if [[ -z "$UDID" ]]; then
  log "no iPhone in reach (not plugged in, and not on this Wi-Fi or asleep too long)"
  exit 0
fi

# 2. What's on it, and what's on this Mac.
xcrun devicectl device info apps --device "$UDID" --bundle-id "$BUNDLE" --json-output "$TMP/apps.json" >/dev/null 2>&1
PHONE=$(/usr/bin/python3 -I -c 'import json,sys
try:
    a = json.load(open(sys.argv[1]))["result"]["apps"]
    print(a[0]["version"], a[0]["bundleVersion"]) if a else print("")
except Exception:
    print("")' "$TMP/apps.json")
if [[ -z "$PHONE" ]]; then
  log "Trek isn't installed on $UDID (or the iPhone didn't answer): nothing to update"
  exit 0
fi
PHONE_VERSION=${PHONE%% *}
PHONE_BUILD=${PHONE##* }
MAC_VERSION=$(defaults read "$MAC_APP/Contents/Info.plist" CFBundleShortVersionString 2>/dev/null)
[[ -z "$MAC_VERSION" ]] && MAC_VERSION=$PHONE_VERSION

# "Is a newer than b?" Trek's versions: 0.4.21 > 0.4.20; a pre-release is older than its release.
newer() {
  /usr/bin/python3 -I -c 'import sys
def key(v):
    v = v.lstrip("v").split("+")[0]
    core, _, pre = v.partition("-")
    nums = [int(x) if x.isdigit() else 0 for x in core.split(".")]
    return (nums + [0] * (4 - len(nums)), pre == "", pre)
sys.exit(0 if key(sys.argv[1]) > key(sys.argv[2]) else 1)' "$1" "$2"
}

# When the signing lapses: as recorded at the last install, else Xcode's profile for the app.
EXPIRES=""
[[ -f "$STATE/installed" ]] && EXPIRES=$(awk -F= '$1 == "expires" {print $2}' "$STATE/installed")
profile_expiry() {  # profile file -> epoch seconds it expires, and its app id
  security cms -D -i "$1" 2>/dev/null > "$TMP/p.plist" || return 1
  local exp app
  exp=$(plutil -extract ExpirationDate raw "$TMP/p.plist" 2>/dev/null) || return 1
  app=$(plutil -extract Entitlements.application-identifier raw "$TMP/p.plist" 2>/dev/null)
  print -r -- "$(date -j -u -f '%Y-%m-%dT%H:%M:%SZ' "$exp" +%s 2>/dev/null) $app"
}
ours=()   # Xcode's cached profiles for this app
cached=""
for f in "$PROFILES"/*.mobileprovision(N); do
  read -r exp app <<< "$(profile_expiry "$f")"
  [[ "$app" == *".$BUNDLE" ]] || continue
  ours+=("$f")
  [[ -z "$cached" ]] || (( exp > cached )) && cached=$exp
done
# No install recorded (one from before this script): the newest profile Xcode has for the app.
[[ -z "$EXPIRES" ]] && EXPIRES=$cached
NOW=$(date +%s)
LEFT=$(( ${EXPIRES:-0} - NOW ))

why=()
newer "$MAC_VERSION" "$PHONE_VERSION" && why+=("Trek $MAC_VERSION on this Mac, $PHONE_VERSION on the iPhone")
if [[ -z "$EXPIRES" ]]; then
  why+=("the signing's expiry isn't known")
elif (( LEFT <= RENEW )); then
  why+=("the signing lapses in $(( LEFT / 3600 )) h")
fi
[[ "$MODE" == --force ]] && why+=("asked to")
summary="iPhone $UDID has $PHONE_VERSION ($PHONE_BUILD), this Mac $MAC_VERSION, signing "
if [[ -n "$EXPIRES" ]]; then summary+="good until $(date -r "$EXPIRES" '+%F %R')"; else summary+="of unknown age"; fi
if (( ${#why} == 0 )); then
  log "up to date: $summary"
  exit 0
fi
URGENT_NOW=$(( ${#EXPIRES} > 0 && LEFT <= URGENT ))
# (PHONE_UPDATE_NOW=1: as if it were, to try the whole run by hand.)
[[ -n "${PHONE_UPDATE_NOW:-}" ]] && URGENT_NOW=1

# 3. Not over a build, and not while Trek is open on the iPhone.
if pgrep -x xcodebuild >/dev/null; then
  log "due (${(j:; :)why}), but an Xcode build is running; next time"
  exit 0
fi
if (( ! URGENT_NOW )) && { pgrep -x cargo >/dev/null || pgrep -x rustc >/dev/null; }; then
  log "due (${(j:; :)why}), but a cargo build is running; next time"
  exit 0
fi
if (( ! URGENT_NOW )) && pgrep -f "$MAC_APP/Contents/MacOS/" >/dev/null && [[ -f "$DEVICES" ]]; then
  open_now=$(/usr/bin/python3 -I -c 'import json,sys
d = json.load(open(sys.argv[1])).get("devices", [])
print(sum(1 for x in d if x.get("platform") == "ios" and x.get("connected")))' "$DEVICES" 2>/dev/null)
  if [[ "${open_now:-0}" != 0 ]]; then
    log "due (${(j:; :)why}), but Trek is open on the iPhone; next time"
    exit 0
  fi
fi
if [[ "$MODE" == --status ]]; then
  log "would update: ${(j:; :)why} ($summary)"
  exit 0
fi

# 4. The source: the Mac's version tag (or the iPhone's, when it's ahead), in a worktree of the
# repo kept for this, so the checkout you work in is never touched.
TARGET=$MAC_VERSION
newer "$PHONE_VERSION" "$MAC_VERSION" && TARGET=$PHONE_VERSION
git -C "$REPO" fetch --quiet --tags origin 2>/dev/null
if ! git -C "$REPO" rev-parse -q --verify "refs/tags/v$TARGET^{commit}" >/dev/null; then
  log "due (${(j:; :)why}), but $REPO has no tag v$TARGET to build"
  exit 1
fi
SRC="$STATE/src"
if [[ -d "$SRC/.git" || -f "$SRC/.git" ]]; then
  git -C "$SRC" checkout -q --force --detach "v$TARGET" || { log "couldn't check out v$TARGET in $SRC"; exit 1; }
else
  rm -rf "$SRC"
  git -C "$REPO" worktree add -q --force --detach "$SRC" "v$TARGET" || { log "couldn't make a worktree at $SRC"; exit 1; }
fi

# 5. A fresh signing when this one is running out: Xcode reuses its cached profile until it
# lapses, so it goes aside (kept in $STATE/old-profiles) and Xcode makes a new one.
if [[ -z "$EXPIRES" ]] || (( LEFT <= RENEW )); then
  mkdir -p "$STATE/old-profiles"
  for f in "${ours[@]}"; do mv "$f" "$STATE/old-profiles/" && log "set aside $(basename "$f") for a fresh one"; done
  ls -t "$STATE/old-profiles"/*.mobileprovision(N) 2>/dev/null | tail -n +11 | while read -r f; do rm -f "$f"; done
fi

# 6. Build and install (as install-device.sh does, without opening the app on the iPhone).
TEAM="${TEAM:-$(defaults read com.apple.dt.Xcode IDEProvisioningTeamByIdentifier 2>/dev/null | grep -o 'teamID = [A-Z0-9]*' | head -1 | cut -d' ' -f3)}"
if [[ -z "$TEAM" ]]; then
  log "no team: add your Apple ID in Xcode › Settings › Accounts"
  exit 1
fi
VERSION=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$SRC/Cargo.toml" | head -1)
BUILD=$(git -C "$SRC" rev-list --count HEAD)
DD="$STATE/DerivedData"
log "updating: ${(j:; :)why}; building Trek $VERSION ($BUILD) from v$TARGET for $UDID"
start=$(date +%s)
if ! (cd "$SRC/ios" && nice -n 10 xcodebuild -project Trek.xcodeproj -scheme Trek -configuration Debug \
      -destination "id=$UDID" -derivedDataPath "$DD" -allowProvisioningUpdates \
      DEVELOPMENT_TEAM="$TEAM" PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE" CODE_SIGN_STYLE=Automatic \
      MARKETING_VERSION="$VERSION" CURRENT_PROJECT_VERSION="$BUILD" build -quiet > "$TMP/build.log" 2>&1); then
  log "build failed:"
  grep -E "error:" "$TMP/build.log" | head -5 | while read -r l; do log "  $l"; done
  exit 1
fi
APP="$DD/Build/Products/Debug-iphoneos/Trek.app"
if ! xcrun devicectl device install app --device "$UDID" "$APP" > "$TMP/install.log" 2>&1; then
  log "install failed: $(grep -iE "error" "$TMP/install.log" | head -2 | tr '\n' ' ')"
  exit 1
fi
read -r exp _ <<< "$(profile_expiry "$APP/embedded.mobileprovision")"
{
  print "version=$VERSION"
  print "build=$BUILD"
  print "at=$(date +%s)"
  print "expires=$exp"
  print "device=$UDID"
} > "$STATE/installed"
log "installed Trek $VERSION ($BUILD) in $(( $(date +%s) - start )) s; signing good until $(date -r "${exp:-0}" '+%F %R')"
