#!/bin/zsh
# rec.sh NAME PAGE WIDTH HEIGHT SCALE SECONDS [light|dark] [selector|scrollY] [tour]
# Serve the site (or ROOT) for the length of one recording and film it with record.mjs.
HERE=${0:A:h}
S=${TREK_SITE_SCRATCH:?set TREK_SITE_SCRATCH to a scratch folder for masters, frames and stills}
W=${TREK_REPO:-${HERE:h:h}}
PY=${TREK_SITE_PYTHON:-python3}   # needs Pillow and numpy
V=${ROOT:-$W/site}
(cd $V && exec python3 -m http.server 4175 --bind 127.0.0.1 >/dev/null 2>&1) &
srv=$!
trap "kill $srv 2>/dev/null" EXIT
sleep 0.6
node $HERE/record.mjs $S/film/$1 http://127.0.0.1:4175/$2 $3 $4 $5 $6 ${7:-light} "${8:-}" "${9:-}"
