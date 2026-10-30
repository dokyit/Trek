#!/bin/zsh
# Tests for ios/scripts/phone-update.sh: what it says (`--status`, and the steps Trek reads when it
# runs it for the iPhone), against stand-ins for the iPhone (trek-mcp's `phone-device`), Xcode's
# devicectl, the installed Trek and the build tools, in a throwaway home folder. Nothing is built
# or installed. macOS only:
#
#   zsh ios/scripts/tests/phone-update.sh
set -u
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
SCRIPT="$ROOT/ios/scripts/phone-update.sh"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
fails=0

# The stand-ins: an installed Trek whose trek-mcp answers as $STUB_* say, xcrun and pgrep.
APP="$WORK/Applications/Trek.app"
mkdir -p "$APP/Contents/MacOS" "$WORK/bin"
cat > "$APP/Contents/MacOS/trek-mcp" <<'EOF'
#!/bin/sh
case "$1 $2" in
  "phone-device list") printf '%s\n' "${STUB_LIST:-No iPhone connected.}" ;;
  "phone-device apps") [ -n "${STUB_APPS:-}" ] && printf '%s\n' "$STUB_APPS" ;;
  mac-update*) printf '%s\n' "${STUB_MAC:-Trek 0.4.27 is up to date}"; exit "${STUB_MAC_CODE:-0}" ;;
esac
exit 0
EOF
cat > "$WORK/bin/xcrun" <<'EOF'
#!/bin/sh
# devicectl: no devices, and no answer about apps.
[ "$1 $2 $3" = "devicectl list devices" ] && exit 0
exit 1
EOF
printf '#!/bin/sh\nexit 1\n' > "$WORK/bin/pgrep"   # no build running, Trek not open
chmod +x "$APP/Contents/MacOS/trek-mcp" "$WORK/bin/xcrun" "$WORK/bin/pgrep"
defaults write "$APP/Contents/Info" CFBundleShortVersionString 0.4.27

# Run it in the throwaway home; prints what it said, without the time stamps.
run() {  # args...
  HOME="$WORK/home" TREK_REPO="$ROOT" TREK_MAC_APP="$APP" TREK_MCP="$APP/Contents/MacOS/trek-mcp" \
    PHONE_UPDATE_PATH="$WORK/bin:/usr/bin:/bin:/usr/sbin:/sbin" zsh "$SCRIPT" "$@" | cut -c21-
}

check() {  # what, actual, expected
  if [[ "$2" == "$3" ]]; then
    print "ok    $1"
  else
    print "FAIL  $1"
    print "  expected: $3"
    print "  got:      $2"
    fails=$((fails + 1))
  fi
}

PHONE="00008140-00040C912E52801C"
UNTIL=$(date -r 4070908800 '+%F %R')
LIST="$PHONE  Test iPhone  iOS 26.1  iPhone17,1  USB"
mkdir -p "$WORK/home/Library/Application Support/dev.trek.phone-update"
# Signed until 2099, so only versions count.
print "expires=4070908800" > "$WORK/home/Library/Application Support/dev.trek.phone-update/installed"

out=$(STUB_LIST="No iPhone connected." run --status)
check "no iPhone: says so, and names no device" "$out" \
  "Mac: Trek 0.4.27 is up to date
no iPhone in reach (not plugged in, and not on this Wi-Fi or asleep too long)"

out=$(STUB_LIST="$LIST" run --status)
check "an iPhone that doesn't answer is named, and said not to answer" "$out" \
  "Mac: Trek 0.4.27 is up to date
the iPhone (Test iPhone) didn't answer (or Trek isn't installed on it): nothing to update"

out=$(STUB_LIST="$LIST" STUB_APPS="dev.trek.TrekMobile  Trek  0.4.26 (855)" run --status)
check "an older app on the iPhone is due" "$out" \
  "Mac: Trek 0.4.27 is up to date
would update: Trek 0.4.27 on this Mac, 0.4.26 on the iPhone (the iPhone (Test iPhone) has 0.4.26 (855), this Mac 0.4.27, signing good until $UNTIL)"

out=$(STUB_LIST="$LIST" STUB_APPS="dev.trek.TrekMobile  Trek  0.4.27 (860)" run --status | tail -1)
check "the same version is up to date" "$out" \
  "up to date: the iPhone (Test iPhone) has 0.4.27 (860), this Mac 0.4.27, signing good until $UNTIL"

out=$(STUB_LIST="$LIST" STUB_APPS="dev.trek.TrekMobile  Trek  0.4.27 (860)" TREK_PHONE_TARGET=0.4.28 run --status | tail -1)
check "a release the phone was offered, newer than the Mac's, is due" "${out%%\ \(*}" \
  "would update: Trek 0.4.28 is out"

out=$(STUB_MAC="would install Trek 0.4.27 (Trek 0.4.26 is closed)" run --status | head -1)
check "this Mac's Trek is checked first" "$out" "Mac: would install Trek 0.4.27 (Trek 0.4.26 is closed)"

out=$(STUB_MAC="usage" STUB_MAC_CODE=2 run --status | head -1)
check "a Trek without mac-update is left to update while open" "$out" \
  "Mac: this Trek can't update itself while it's closed; it does when it's open"

out=$(TREK_MAC_UPDATE=0 run --status)
check "TREK_MAC_UPDATE=0 leaves this Mac's Trek be" "$out" \
  "no iPhone in reach (not plugged in, and not on this Wi-Fi or asleep too long)"

# Asked from Trek: the steps go to stdout, the log to its file, and the outcome is kept.
out=$(HOME="$WORK/home" TREK_REPO="$ROOT" TREK_MAC_APP="$APP" TREK_MCP="$APP/Contents/MacOS/trek-mcp" \
  PHONE_UPDATE_PATH="$WORK/bin:/usr/bin:/bin:/usr/sbin:/sbin" TREK_PHONE_UPDATE_EVENTS=1 \
  zsh "$SCRIPT" --asked)
check "asked with no iPhone in reach: fails in plain words" "$out" \
  "@failed Your Mac couldn't reach the iPhone: plug it in or join the same Wi-Fi"
last="$WORK/home/Library/Application Support/dev.trek.phone-update/last"
check "and keeps it for Settings" "$(grep -v '^at=' "$last")" \
  "result=failed
line=Your Mac couldn't reach the iPhone: plug it in or join the same Wi-Fi"
check "the log has it its own way" "$(tail -1 "$WORK/home/Library/Logs/trek-phone-update.log" | cut -c21-)" \
  "asked, but no iPhone in reach (not plugged in, and not on this Wi-Fi or asleep too long)"

if (( fails )); then
  print "$fails failed"
  exit 1
fi
print "all passed"
