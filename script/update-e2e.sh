#!/bin/zsh
# End-to-end check of the updater on this Mac, without publishing anything: run an old Trek.app
# from a scratch folder against a local feed serving a newer release and watch it download,
# verify, swap itself and relaunch, with no clicks. Or serve a tampered archive and check it's
# refused and the app is left alone.
#
#   script/update-e2e.sh <old Trek-*.app.tar.gz> <new release dir> [--tamper checksum|signature] [--keep]
#
# The release dir is what `script/release.sh <version> --dry-run` leaves in dist/release/<version>/.
# Everything happens under $TREK_E2E_DIR (default /tmp/trek-update-e2e) with its own TREK_DATA_DIR,
# so your real Trek data is never touched. The test app opens without taking focus
# (TREK_BACKGROUND) and restarts as soon as the update is ready (TREK_UPDATE_AUTO_RESTART).
# Processes it started are stopped and the folder removed at the end unless --keep.
set -euo pipefail
OLD=${1:?old archive}; NEW=${2:?new release dir}; shift 2
TAMPER="" KEEP=0
while (( $# )); do
  case $1 in
    --tamper) TAMPER=${2:-}; shift 2 ;;
    --keep) KEEP=1; shift ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done
[[ -z $TAMPER || $TAMPER == checksum || $TAMPER == signature ]] || { echo "--tamper checksum|signature" >&2; exit 2; }
OLD=${OLD:A}; NEW=${NEW:A}
WORK=${TREK_E2E_DIR:-/tmp/trek-update-e2e}
PORT=${TREK_E2E_PORT:-8765}
APP="$WORK/app/Trek.app"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

say() { print -r -- "$(date +%H:%M:%S)  $*"; }
version_of() { plutil -extract CFBundleVersion raw -o - "$1/Contents/Info.plist"; }
# Only processes running from the scratch copy: never any other Trek.
test_pids() { ps -axo pid=,command= | awk -v app="$APP/Contents/MacOS/trek" '{ cmd=$0; sub(/^ *[0-9]+ +/, "", cmd); sub(/^\/private/, "", cmd) } cmd == app { print $1 }'; }
SERVER=""
cleanup() {
  for p in $(test_pids); do kill "$p" 2>/dev/null || true; done
  [[ -n $SERVER ]] && kill "$SERVER" 2>/dev/null || true
  if (( ! KEEP )); then
    [[ -d $APP ]] && "$LSREGISTER" -u "$APP" 2>/dev/null || true
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

rm -rf "$WORK"; mkdir -p "$WORK/app" "$WORK/serve" "$WORK/data"
tar -xzf "$OLD" -C "$WORK/app"
FROM=$(version_of "$APP")
MANIFEST=$(print -l "$NEW"/{stable,beta,nightly}.json(N) | head -1)
[[ -n $MANIFEST ]] || { echo "no manifest in $NEW" >&2; exit 1; }
CHANNEL=${MANIFEST:t:r}
cp "$NEW"/Trek-*.app.tar.gz "$MANIFEST" "$WORK/serve/"
ARCHIVE=$(print -l "$WORK"/serve/Trek-*.app.tar.gz | head -1)
TO=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$MANIFEST")

if [[ -n $TAMPER ]]; then
  # Flip one byte in the middle of the archive.
  python3 - "$ARCHIVE" <<'PY'
import sys
p = sys.argv[1]; b = bytearray(open(p, "rb").read()); b[len(b) // 2] ^= 0xFF; open(p, "wb").write(b)
PY
fi
# Point the manifest at the local copy (the URL isn't signed; the archive is). For --tamper
# signature the checksum is updated too, so only the signature can catch it.
python3 - "$WORK/serve/${MANIFEST:t}" "$ARCHIVE" "$PORT" "$TAMPER" <<'PY'
import hashlib, json, os, sys
path, archive, port, tamper = sys.argv[1:5]
m = json.load(open(path))
for a in m["platforms"].values():
    a["url"] = f"http://127.0.0.1:{port}/{os.path.basename(archive)}"
    if tamper == "signature":
        a["sha256"] = hashlib.sha256(open(archive, "rb").read()).hexdigest()
json.dump(m, open(path, "w"), indent=2)
PY
cat > "$WORK/data/settings.toml" <<TOML
[updates]
channel = "$CHANNEL"
auto_check = true
auto_download = true
feed_url = "http://127.0.0.1:$PORT/{channel}.json"

[onboarding]
completed = true

[import]
claude_code = false
codex = false
opencode = false

[notifications]
mode = "off"
dock_badge = false
menu_bar_icon = false
TOML

python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$WORK/serve" > "$WORK/http.log" 2>&1 &
SERVER=$!
sleep 0.5
say "serving $CHANNEL.json ($TO) on 127.0.0.1:$PORT${TAMPER:+, archive tampered ($TAMPER)}"
say "starting Trek $FROM from $APP (pid below)"
TREK_DATA_DIR="$WORK/data" TREK_BACKGROUND=1 TREK_UPDATE_AUTO_RESTART=1 RUST_LOG=warn,trek=info \
  "$APP/Contents/MacOS/trek" > "$WORK/first.log" 2>&1 &
FIRST=$!
say "pid $FIRST"

OUTCOME=timeout
for i in {1..120}; do
  if grep -q "rejected" "$WORK/first.log"; then OUTCOME=rejected; break; fi
  if ! kill -0 $FIRST 2>/dev/null; then
    NEXT=$(test_pids | head -1)
    if [[ -n $NEXT && ! -f "$WORK/data/updates/updated-from" ]]; then OUTCOME=updated; break; fi
  fi
  sleep 0.5
done

say "outcome: $OUTCOME"
print -- "--- log of Trek $FROM (pid $FIRST)"; grep -v '^$' "$WORK/first.log" | sed 's/\x1b\[[0-9;]*m//g' || true
print -- "--- feed requests"; sed 's/^/    /' "$WORK/http.log"
print -- "--- result"
print "    bundle version:   $(version_of "$APP")"
print "    signature:        $(codesign --verify --strict --deep "$APP" 2>&1 && echo valid), $(codesign -dvv "$APP" 2>&1 | sed -n 's/^Authority=//p' | head -1)"
if [[ $OUTCOME == updated ]]; then
  print "    running now:      pid $NEXT, $(ps -o lstart= -p "$NEXT") (Trek $FROM exited)"
  print "    previous version: $(version_of "$WORK/data/updates/previous.noindex/Trek.app") kept in updates/previous.noindex"
fi
print "    leftovers:        $(ls -A "$WORK/app" | grep -v '^Trek.app$' || echo none) next to the app; staged copy $([[ -e $WORK/data/updates/staged.noindex ]] && echo present || echo gone)"

if [[ -n $TAMPER ]]; then
  [[ $OUTCOME == rejected && $(version_of "$APP") == "$FROM" ]] && { say "PASS: tampered update refused, Trek $FROM untouched"; exit 0; }
else
  [[ $OUTCOME == updated && $(version_of "$APP") == "$TO" ]] && { say "PASS: Trek $FROM updated itself to $TO and relaunched"; exit 0; }
fi
say "FAIL"; exit 1
