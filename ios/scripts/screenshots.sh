#!/bin/sh
# Rebuild and capture the demo-mode screenshots in ios/screenshots/.
# The live ones (10–12, 15) need a running demo host: see live-screenshots.sh.
set -eu
cd "$(dirname "$0")"
OUT="$(cd .. && pwd)/screenshots"
mkdir -p "$OUT"
# Any well-formed link works for the confirmation sheet; nothing connects until "Pair" is tapped.
LINK='trek://pair?host=192.168.1.20:7420&code=K7Q2-9XMV&name=Tobias%E2%80%99s%20MacBook%20Pro&hid=7f3c&fp=b138008777ff7ea5d27b6178a442a960f0b7ede92c61bf7d4d0d59ea94050dcb'
./shoot.sh "$OUT/01-threads-light.png" -TrekAppearance light
BUILD=0 ./shoot.sh "$OUT/02-threads-dark.png" -TrekAppearance dark
BUILD=0 WAIT=4 ./shoot.sh "$OUT/03-approval-destructive-light.png" -TrekAppearance light -TrekOpen t-flaky
BUILD=0 WAIT=4 ./shoot.sh "$OUT/04-working-dark.png" -TrekAppearance dark -TrekOpen t-inbox -TrekExpand YES
BUILD=0 WAIT=4 ./shoot.sh "$OUT/05-question-light.png" -TrekAppearance light -TrekOpen t-ratelimit
BUILD=0 WAIT=4 ./shoot.sh "$OUT/06-plan-dark.png" -TrekAppearance dark -TrekOpen t-settings
BUILD=0 WAIT=4 ./shoot.sh "$OUT/07-new-thread-light.png" -TrekAppearance light -TrekSheet new
BUILD=0 ./shoot.sh "$OUT/08-settings-dark.png" -TrekAppearance dark -TrekTab settings
BUILD=0 ./shoot.sh "$OUT/09-search-light.png" -TrekAppearance light -TrekTab search
BUILD=0 DEMO=0 ./shoot.sh "$OUT/00-pairing-light.png" -TrekAppearance light
BUILD=0 DEMO=0 ./shoot.sh "$OUT/13-pairing-confirm-light.png" -TrekAppearance light -TrekLink "$LINK"
BUILD=0 DEMO=0 ./shoot.sh "$OUT/14-pairing-unencrypted-dark.png" -TrekAppearance dark -TrekLink "${LINK%&fp=*}"
