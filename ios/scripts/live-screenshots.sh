#!/bin/sh
# Pair the simulator with a running demo host over pinned TLS and capture the live screenshots.
#   cargo run -p trek-remote --example demo_host      # in another terminal
#   ios/scripts/live-screenshots.sh '<the "Simulator" pairing URL it printed>'
# Uses the debug-only -TrekPair launch argument (pairs without the confirmation sheet).
# Env: as shoot.sh (SIM, DD, BUILD).
set -eu
cd "$(dirname "$0")"
URL="$1"
OUT="$(cd .. && pwd)/screenshots"
mkdir -p "$OUT"
DEMO=0 WAIT=8 ./shoot.sh "$OUT/10-live-threads-dark.png" -TrekAppearance dark -TrekPair "$URL"
# Paired now: this launch reconnects with hello, pinned to the fingerprint kept in the Keychain.
BUILD=0 DEMO=live WAIT=7 ./shoot.sh "$OUT/15-settings-encrypted-light.png" -TrekAppearance light -TrekTab settings
