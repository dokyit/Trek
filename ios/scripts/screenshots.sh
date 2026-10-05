#!/bin/sh
# Rebuild and capture the demo-mode screenshots in ios/screenshots/.
set -eu
cd "$(dirname "$0")"
OUT="$(cd .. && pwd)/screenshots"
mkdir -p "$OUT"
./shoot.sh "$OUT/01-sessions-light.png" -TrekAppearance light
BUILD=0 ./shoot.sh "$OUT/02-sessions-dark.png" -TrekAppearance dark
BUILD=0 WAIT=4 ./shoot.sh "$OUT/03-approval-light.png" -TrekAppearance light -TrekOpen t-flaky
BUILD=0 WAIT=4 ./shoot.sh "$OUT/04-working-dark.png" -TrekAppearance dark -TrekOpen t-inbox -TrekExpand YES
BUILD=0 WAIT=4 ./shoot.sh "$OUT/05-question-light.png" -TrekAppearance light -TrekOpen t-ratelimit
BUILD=0 WAIT=4 ./shoot.sh "$OUT/06-plan-dark.png" -TrekAppearance dark -TrekOpen t-settings
BUILD=0 WAIT=4 ./shoot.sh "$OUT/07-new-session-light.png" -TrekAppearance light -TrekSheet new
BUILD=0 ./shoot.sh "$OUT/08-settings-dark.png" -TrekAppearance dark -TrekTab settings
BUILD=0 ./shoot.sh "$OUT/09-search-light.png" -TrekAppearance light -TrekTab search
BUILD=0 DEMO=0 ./shoot.sh "$OUT/00-pairing-light.png" -TrekAppearance light
