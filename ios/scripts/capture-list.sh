#!/bin/sh
# Run an iOS capture manifest: one artifact per line, shot or clip.
#   ios/scripts/capture-list.sh <manifest.list> [outdir]
#
# Line shape (see media/ios/*.list):
#   NAME [record=SECS] [wait=N] [demo=0|1|live] [-- <launch args for the app>]
#
# NAME gets .png (via shoot.sh) or .mov (via record.sh) under outdir. The first line builds,
# the rest reuse it. SIM picks the simulator — pin a 6.9" Pro Max for App Store sizes, e.g.
#   SIM="iPhone 17 Pro Max" ios/scripts/capture-list.sh media/ios/store.list dist/media/store
set -eu
cd "$(dirname "$0")/.."
MANIFEST=${1:?manifest}; OUTDIR=${2:-screenshots}
mkdir -p "$OUTDIR"
BUILD=1
grep -vE '^[[:space:]]*(#|$)' "$MANIFEST" | while IFS= read -r line; do
  # Split `name opts… -- args…` on the first ` -- `.
  opts=$line; args=""
  case $line in *" -- "*) opts=${line%% -- *}; args=${line#* -- };; esac
  set -- $opts
  name=$1; shift
  RECORD="" DEMO="" W=""
  for kv in "$@"; do
    case $kv in
      record=*) RECORD=${kv#record=};; wait=*) W=${kv#wait=};; demo=*) DEMO=${kv#demo=};; *)
        echo "capture-list: bad option $kv (record=|wait=|demo=)" >&2; exit 2;;
    esac
  done
  # shellcheck disable=SC2086 # args are meant to word-split
  if [ -n "$RECORD" ]; then
    env BUILD=$BUILD ${W:+WAIT=$W} ${DEMO:+DEMO=$DEMO} DUR=$RECORD \
      ./scripts/record.sh "$OUTDIR/$name.mov" $args
  else
    env BUILD=$BUILD ${W:+WAIT=$W} ${DEMO:+DEMO=$DEMO} \
      ./scripts/shoot.sh "$OUTDIR/$name.png" $args
  fi
  BUILD=0
done
