#!/bin/sh
# A scripted scroll up a transcript in the simulator (AXe swipes: slow drags, flicks and flings),
# recorded, with the app's scroll probe (ScrollProbe.swift) pulled out of it afterwards.
#   ios/scripts/scroll-run.sh <udid> <out-prefix> <swipes> [launch args...]
#   ios/scripts/scroll-run.sh booted /tmp/run 30 -TrekBigThread 5000 -TrekBigRich YES -TrekOpen t-big
#   python3 -I ios/scripts/scroll-probe.py /tmp/run.tsv      # jumps and blank frames
# Writes <out-prefix>.mov and <out-prefix>.tsv. Needs AXe (brew install cameroncooke/axe/axe).
set -u
UDID="$1"; OUT="$2"; N="$3"; shift 3
xcrun simctl terminate "$UDID" dev.trek.TrekMobile 2>/dev/null
xcrun simctl launch "$UDID" dev.trek.TrekMobile -TrekDemo YES -TrekScrollProbe YES -TrekAppearance dark "$@" >/dev/null
sleep 10
xcrun simctl io "$UDID" recordVideo --codec h264 --force "$OUT.mov" 2>/dev/null &
REC=$!
sleep 1.5
i=0
while [ $i -lt "$N" ]; do
  # Alternate a slow drag (no fling), a medium flick and a fast fling, like a reader does.
  case $((i % 3)) in
    0) axe swipe --start-x 220 --start-y 300 --end-x 220 --end-y 700 --duration 0.6 --delta 15 --udid "$UDID" >/dev/null 2>&1; sleep 0.4 ;;
    1) axe swipe --start-x 220 --start-y 280 --end-x 220 --end-y 760 --duration 0.25 --delta 15 --udid "$UDID" >/dev/null 2>&1; sleep 1.0 ;;
    2) axe swipe --start-x 220 --start-y 250 --end-x 220 --end-y 800 --duration 0.12 --delta 15 --udid "$UDID" >/dev/null 2>&1; sleep 1.6 ;;
  esac
  i=$((i + 1))
done
sleep 1
kill -INT $REC
wait $REC 2>/dev/null
C="$(xcrun simctl get_app_container "$UDID" dev.trek.TrekMobile data)"
cp "$C/Library/Caches/scroll-probe.tsv" "$OUT.tsv"
wc -l "$OUT.tsv"
