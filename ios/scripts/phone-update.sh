#!/bin/zsh
# Keeps Trek on the iPhone up to date without a cable, and Trek on this Mac while it's closed. Run
# every 30 minutes by the LaunchAgent ~/Library/LaunchAgents/dev.trek.phone-update.plist
# (ios/scripts/install-phone-update.sh puts it there), and by Trek when you ask it to update the
# iPhone (from the iPhone, or Settings › Phone & Machines); safe to run by hand. Logs to
# ~/Library/Logs/trek-phone-update.log.
#
# This Mac first: Trek updates itself only while it runs (it installs on quit), so while it's
# closed the app's own trek-mcp does it (`trek-mcp mac-update`: Trek's updater, the release checked
# against the key Trek ships, then swapped into /Applications). While Trek is open it's left to
# Trek. The iPhone then follows in the same run.
#
# The iPhone app is signed with a free Apple ID: no App Store, no TestFlight, so it can't update
# itself, and its signing lasts 7 days from when Xcode made the provisioning profile (installing
# again with the same profile doesn't extend it). So, when the iPhone is reachable (USB, or Wi-Fi
# once paired with Xcode), this Mac builds the app and installs it:
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
#   phone-update.sh --asked    someone asked, from Trek: build and install now, past a cargo build
#                              and Trek open on the iPhone (the install closes it; it's opened
#                              again after). TREK_PHONE_TARGET=<version> builds that release when
#                              it's newer than this Mac's Trek (the Mac installs it when Trek quits).
# With TREK_PHONE_UPDATE_EVENTS=1 (Trek runs it so) the log goes to the log file only, and stdout
# carries the steps, one per line: "@building 0.4.28", "@installing 0.4.28", "@installed 0.4.28",
# "@failed <why, in plain words>".
# Off: touch "~/Library/Application Support/dev.trek.phone-update/off" (or uninstall the agent:
# ios/scripts/install-phone-update.sh --uninstall); TREK_MAC_UPDATE=0 leaves this Mac's Trek be.
set -u
# (PHONE_UPDATE_PATH: the tests' stand-ins for xcrun and the rest.)
export PATH="${PHONE_UPDATE_PATH:-/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin}"
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
ASKED=0
[[ "$MODE" == --asked ]] && ASKED=1 MODE=--force
EVENTS="${TREK_PHONE_UPDATE_EVENTS:-}"
log() {
  if [[ -n "$EVENTS" ]]; then
    print -r -- "$(date '+%F %T') $*" >> "$LOG"
  else
    print -r -- "$(date '+%F %T') $*"
  fi
}
# A step, for Trek to show on the iPhone and in Settings.
event() { [[ -n "$EVENTS" ]] && print -r -- "@$*"; return 0 }
# How the last attempt went, for Settings (key=value lines, as `installed`).
record() {  # ok|failed, the line
  { print "at=$(date +%s)"; print "result=$1"; print "line=$2"; } > "$STATE/last"
}
# It didn't happen: the log says it its way, the iPhone and Settings in plain words.
fail() {  # plain words, [log line]
  log "${2:-$1}"
  event "failed $1"
  record failed "$1"
  exit 1
}

mkdir -p "$STATE" "${LOG:h}"
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
    (( ASKED )) && event "failed Your Mac is already updating the iPhone; it's done in a few minutes"
    exit 0
  fi
  rm -rf "$STATE/lock"
  mkdir "$STATE/lock" || exit 0
fi
print $$ > "$STATE/lock/pid"
TMP=$(mktemp -d)
trap 'rm -rf "$STATE/lock" "$TMP"' EXIT

# 1. This Mac's Trek, while it's closed (see the top). Not when someone asked from Trek, which is
# open then. A Trek before `mac-update` (0.4.28) exits 2 with its usage: it updates while open.
if (( ! ASKED )) && [[ "${TREK_MAC_UPDATE:-1}" != 0 && -x "$MAC_APP/Contents/MacOS/trek-mcp" ]]; then
  mac_args=(--app "$MAC_APP")
  [[ "$MODE" == --status ]] && mac_args+=(--status)
  said=$("$MAC_APP/Contents/MacOS/trek-mcp" mac-update "${mac_args[@]}" 2>/dev/null)
  case $? in
    2) log "Mac: this Trek can't update itself while it's closed; it does when it's open" ;;
    *) log "Mac: ${said:-no answer from trek-mcp mac-update}" ;;
  esac
fi

# Installing and reading the phone go through Trek's own trek-mcp (`phone-device`, over usbmuxd
# as Finder does) when there's one that has it; devicectl (Xcode) is the fallback, and still
# what reaches a phone paired over Wi-Fi only. Building needs Xcode either way.
TREK_MCP="$(TREK_REPO="$REPO" TREK_MAC_APP="$MAC_APP" sh "$REPO/ios/scripts/trek-mcp-path.sh" 2>/dev/null)"

# 2. The iPhone, reachable now: over USB, or the network (Xcode pairs it once over USB). Only a
# line that starts with a device id names one ("No iPhone connected." doesn't).
UDID="${DEVICE:-}"
NAME=""
is_udid='^([0-9A-Fa-f]{8}-[0-9A-Fa-f]{16}|[0-9a-f]{40})$'
if [[ -n "$TREK_MCP" ]]; then
  "$TREK_MCP" phone-device list > "$TMP/mux.txt" 2>/dev/null
  # "00008140-…  Toby's iPhone  iOS 26.1  iPhone17,1  USB"
  [[ -z "$UDID" ]] && UDID=$(awk -v re="$is_udid" '$1 ~ re {print $1; exit}' "$TMP/mux.txt")
  NAME=$(awk -F '  ' -v u="$UDID" '$1 == u {print $2; exit}' "$TMP/mux.txt")
fi
VIA_MUX=0
[[ -n "$UDID" ]] && grep -q "^$UDID " "$TMP/mux.txt" 2>/dev/null && VIA_MUX=1
if [[ -z "$UDID" ]]; then
  xcrun devicectl list devices > "$TMP/devices.txt" 2>/dev/null
  UDID=$(awk '/iPhone/ && /physical/ && /(^|[^a-z])(available|connected)([^a-z]|$)/ {for (i=1;i<=NF;i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$/) print $i}' "$TMP/devices.txt" | head -1)
fi
if [[ -z "$UDID" ]]; then
  (( ASKED )) && fail "Your Mac couldn't reach the iPhone: plug it in or join the same Wi-Fi" \
    "asked, but no iPhone in reach (not plugged in, and not on this Wi-Fi or asleep too long)"
  log "no iPhone in reach (not plugged in, and not on this Wi-Fi or asleep too long)"
  exit 0
fi
PHONE_NAME="the iPhone${NAME:+ ($NAME)}"

# 3. What's on it, and what's on this Mac.
PHONE=""
if (( VIA_MUX )); then
  # "dev.trek.TrekMobile  Trek  0.4.21 (826)" -> "0.4.21 826"
  PHONE=$("$TREK_MCP" phone-device apps --udid "$UDID" 2>/dev/null \
    | awk -v b="$BUNDLE" '$1 == b {v=$(NF-1); n=$NF; gsub(/[()]/, "", n); print v, n; exit}')
fi
if [[ -z "$PHONE" ]]; then
  xcrun devicectl device info apps --device "$UDID" --bundle-id "$BUNDLE" --json-output "$TMP/apps.json" >/dev/null 2>&1
  PHONE=$(/usr/bin/python3 -I -c 'import json,sys
try:
    a = json.load(open(sys.argv[1]))["result"]["apps"]
    print(a[0]["version"], a[0]["bundleVersion"]) if a else print("")
except Exception:
    print("")' "$TMP/apps.json")
fi
if [[ -z "$PHONE" ]]; then
  (( ASKED )) && fail "The iPhone didn't answer: unlock it and keep it near your Mac" \
    "asked, but $PHONE_NAME didn't answer (or Trek isn't installed on it)"
  log "$PHONE_NAME didn't answer (or Trek isn't installed on it): nothing to update"
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
# A release the phone was offered that this Mac hasn't installed yet (Trek installs it on quit).
WANTED="${TREK_PHONE_TARGET:-}"
[[ -n "$WANTED" ]] && ! newer "$WANTED" "$MAC_VERSION" && WANTED=""
[[ -n "$WANTED" ]] && newer "$WANTED" "$PHONE_VERSION" && why+=("Trek $WANTED is out")
if [[ -z "$EXPIRES" ]]; then
  why+=("the signing's expiry isn't known")
elif (( LEFT <= RENEW )); then
  why+=("the signing lapses in $(( LEFT / 3600 )) h")
fi
[[ "$MODE" == --force ]] && why+=("asked to")
summary="$PHONE_NAME has $PHONE_VERSION ($PHONE_BUILD), this Mac $MAC_VERSION, signing "
if [[ -n "$EXPIRES" ]]; then summary+="good until $(date -r "$EXPIRES" '+%F %R')"; else summary+="of unknown age"; fi
if (( ${#why} == 0 )); then
  log "up to date: $summary"
  exit 0
fi
URGENT_NOW=$(( ${#EXPIRES} > 0 && LEFT <= URGENT ))
# (PHONE_UPDATE_NOW=1: as if it were, to try the whole run by hand.)
[[ -n "${PHONE_UPDATE_NOW:-}" ]] && URGENT_NOW=1

# 4. Not over a build, and not while Trek is open on the iPhone (unless someone asked, from it or
# from Trek on this Mac).
if pgrep -x xcodebuild >/dev/null; then
  (( ASKED )) && fail "Your Mac is busy with another Xcode build; try again in a few minutes" \
    "asked (${(j:; :)why}), but an Xcode build is running"
  log "due (${(j:; :)why}), but an Xcode build is running; next time"
  exit 0
fi
# TREK_PHONE_UPDATE_IGNORE_BUILDS=1 (set for one run with `launchctl setenv`) goes ahead over a
# cargo build, as --asked does: for a Mac that builds all day, when the user wants the phone updated now.
if (( ! URGENT_NOW && ! ASKED )) && [[ -z "${TREK_PHONE_UPDATE_IGNORE_BUILDS:-}" ]] && { pgrep -x cargo >/dev/null || pgrep -x rustc >/dev/null; }; then
  log "due (${(j:; :)why}), but a cargo build is running; next time"
  exit 0
fi
open_now=0
if pgrep -f "$MAC_APP/Contents/MacOS/" >/dev/null && [[ -f "$DEVICES" ]]; then
  open_now=$(/usr/bin/python3 -I -c 'import json,sys
d = json.load(open(sys.argv[1])).get("devices", [])
print(sum(1 for x in d if x.get("platform") == "ios" and x.get("connected")))' "$DEVICES" 2>/dev/null)
fi
if (( ! URGENT_NOW && ! ASKED )) && [[ "${open_now:-0}" != 0 ]]; then
  log "due (${(j:; :)why}), but Trek is open on the iPhone; next time"
  exit 0
fi
if [[ "$MODE" == --status ]]; then
  log "would update: ${(j:; :)why} ($summary)"
  exit 0
fi

# 5. The source: the Mac's version tag (the iPhone's, when it's ahead; a release it was offered,
# when that's newer still and tagged), in a worktree of the repo kept for this, so the checkout
# you work in is never touched.
[[ -d "$REPO/.git" || -f "$REPO/.git" ]] || fail "Your Mac has no copy of Trek's source to build the iPhone app from" \
  "due (${(j:; :)why}), but there's no repository at $REPO (TREK_REPO)"
TARGET=$MAC_VERSION
newer "$PHONE_VERSION" "$MAC_VERSION" && TARGET=$PHONE_VERSION
git -C "$REPO" fetch --quiet --tags origin 2>/dev/null
if [[ -n "$WANTED" ]] && newer "$WANTED" "$TARGET"; then
  if git -C "$REPO" rev-parse -q --verify "refs/tags/v$WANTED^{commit}" >/dev/null; then
    TARGET=$WANTED
  else
    log "Trek $WANTED was asked for, but $REPO has no tag v$WANTED yet; building v$TARGET"
  fi
fi
if ! git -C "$REPO" rev-parse -q --verify "refs/tags/v$TARGET^{commit}" >/dev/null; then
  fail "Your Mac's copy of Trek's source has no Trek $TARGET to build" \
    "due (${(j:; :)why}), but $REPO has no tag v$TARGET to build"
fi
SRC="$STATE/src"
if [[ -d "$SRC/.git" || -f "$SRC/.git" ]]; then
  git -C "$SRC" checkout -q --force --detach "v$TARGET" || fail "Your Mac couldn't get Trek $TARGET's source ready" "couldn't check out v$TARGET in $SRC"
else
  rm -rf "$SRC"
  git -C "$REPO" worktree add -q --force --detach "$SRC" "v$TARGET" || fail "Your Mac couldn't get Trek $TARGET's source ready" "couldn't make a worktree at $SRC"
fi

# 6. A fresh signing when this one is running out: Xcode reuses its cached profile until it
# lapses, so it goes aside (kept in $STATE/old-profiles) and Xcode makes a new one.
if [[ -z "$EXPIRES" ]] || (( LEFT <= RENEW )); then
  mkdir -p "$STATE/old-profiles"
  for f in "${ours[@]}"; do mv "$f" "$STATE/old-profiles/" && log "set aside $(basename "$f") for a fresh one"; done
  ls -t "$STATE/old-profiles"/*.mobileprovision(N) 2>/dev/null | tail -n +11 | while read -r f; do rm -f "$f"; done
fi

# 7. Build and install (as install-device.sh does, opening the app again only if it was open).
TEAM="${TEAM:-$(defaults read com.apple.dt.Xcode IDEProvisioningTeamByIdentifier 2>/dev/null | grep -o 'teamID = [A-Z0-9]*' | head -1 | cut -d' ' -f3)}"
[[ -n "$TEAM" ]] || fail "Xcode on your Mac has no Apple ID to sign with: add yours in Xcode › Settings › Accounts" \
  "no team: add your Apple ID in Xcode › Settings › Accounts"
VERSION=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$SRC/Cargo.toml" | head -1)
BUILD=$(git -C "$SRC" rev-list --count HEAD)
DD="$STATE/DerivedData"
log "updating: ${(j:; :)why}; building Trek $VERSION ($BUILD) from v$TARGET for $UDID"
event "building $VERSION"
start=$(date +%s)
if ! (cd "$SRC/ios" && nice -n 10 xcodebuild -project Trek.xcodeproj -scheme Trek -configuration Debug \
      -destination "id=$UDID" -derivedDataPath "$DD" -allowProvisioningUpdates \
      DEVELOPMENT_TEAM="$TEAM" PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE" CODE_SIGN_STYLE=Automatic \
      MARKETING_VERSION="$VERSION" CURRENT_PROJECT_VERSION="$BUILD" build -quiet > "$TMP/build.log" 2>&1); then
  log "build failed:"
  grep -E "error:" "$TMP/build.log" | head -5 | while read -r l; do log "  $l"; done
  first=$(grep -m1 -E "error:" "$TMP/build.log" | sed 's/.*error: *//' | cut -c1-140)
  fail "Building Trek $VERSION on your Mac failed${first:+: $first}" "build of Trek $VERSION failed"
fi
APP="$DD/Build/Products/Debug-iphoneos/Trek.app"
# Over a running app: installd ends the app before it replaces it, as an App Store update does,
# and nothing starts it again. Seen with `simctl install` over Trek running in the Simulator (the
# install succeeds, the app is gone); on an iPhone, usbmuxd's install and devicectl's go through
# the same installd, but that wasn't tried on one. So the app is opened again below if it was open,
# and the iPhone that asked was told ("Installing… Trek will close and open again").
event "installing $VERSION"
if (( VIA_MUX )) && "$TREK_MCP" phone-device install "$APP" --udid "$UDID" > "$TMP/install.log" 2>&1; then
  log "installed over usbmuxd (no Xcode)"
elif ! xcrun devicectl device install app --device "$UDID" "$APP" > "$TMP/install.log" 2>&1; then
  said=$(grep -iE "error" "$TMP/install.log" | head -2 | tr '\n' ' ')
  fail "Your Mac couldn't install Trek on the iPhone: unlock it, and keep it plugged in or on the same Wi-Fi" \
    "install failed: $said"
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
record ok "Installed Trek $VERSION on $PHONE_NAME"
event "installed $VERSION"
# Open again where the install closed it (Developer Mode is on: a development-signed app needs it).
if [[ "${open_now:-0}" != 0 ]] && ! xcrun devicectl device process launch --device "$UDID" "$BUNDLE" >/dev/null 2>&1; then
  log "couldn't open Trek on the iPhone again; it opens when tapped"
fi
exit 0
