#!/bin/sh
# The transcript while an answer is being written, measured: a long thread, long answers streaming
# into it (-TrekDemoStream), the scroll probe on, and Trek Driver's swipes. AXe, which scroll-run.sh
# uses, doesn't work on the iOS 27 simulator. Start Trek Driver first (see ios/README.md).
#   ios/scripts/stream-run.sh <udid> <out-prefix> [items in the thread, default 3000]
#   python3 -I ios/scripts/scroll-probe.py <out-prefix>.tsv      # jumps, blank frames, the place kept
# Four stretches, in the order they run:
#   at the latest, nothing touching it: the bottom follows what's written
#   swiping up while it's written: slow drags, flicks and flings
#   still, part-way up, while it's written: nothing on screen should move at all
#   the jump-down button, then at the latest again
set -u
UDID="$1"; OUT="$2"; N="${3:-3000}"
H="X-Trek-Token: ${TREK_DRIVER_TOKEN:-simtest}"; U="http://127.0.0.1:${TREK_DRIVER_PORT:-8117}"
swipe() { curl -s -m 30 -H "$H" -d "{\"x1\":201,\"y1\":$1,\"x2\":201,\"y2\":$2,\"duration_ms\":$3}" "$U/swipe" >/dev/null; }
xcrun simctl terminate "$UDID" dev.trek.TrekMobile 2>/dev/null
xcrun simctl launch "$UDID" dev.trek.TrekMobile -TrekDemo YES -TrekScrollProbe YES -TrekAppearance dark \
  -TrekBigThread "$N" -TrekBigRich YES -TrekOpen t-big -TrekDemoStream YES >/dev/null
sleep 30
i=0
while [ $i -lt 18 ]; do
  case $((i % 3)) in
    0) swipe 300 700 600; sleep 1.5 ;;
    1) swipe 280 760 250; sleep 1.5 ;;
    2) swipe 250 800 120; sleep 2 ;;
  esac
  i=$((i + 1))
done
sleep 25
# The jump-down button, over the composer's right: it's 14 pt higher while a turn runs, as the composer
# then has the follow-up line. This is on it either way.
curl -s -m 30 -H "$H" -d '{"x":369,"y":704,"hold_ms":0}' "$U/tap" >/dev/null
sleep 15
C="$(xcrun simctl get_app_container "$UDID" dev.trek.TrekMobile data)"
cp "$C/Library/Caches/scroll-probe.tsv" "$OUT.tsv"
wc -l "$OUT.tsv"
