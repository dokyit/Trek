#!/bin/zsh
# The site's pictures of the app, from their masters, and (with "ridges") its backdrop art.
#   site_assets.sh           convert the masters in $TREK_SITE_SCRATCH/shots/v3 into site/assets/img
#   site_assets.sh ridges    also redraw the ridges with gen_ridges.py
# A redrawn or reconverted picture loses the origin note the design tool embedded in it; embed it
# again (site/README.md says how).
HERE=${0:A:h}
S=${TREK_SITE_SCRATCH:?set TREK_SITE_SCRATCH to a scratch folder for masters, frames and stills}
W=${TREK_REPO:-${HERE:h:h}}
PY=${TREK_SITE_PYTHON:-python3}   # needs Pillow and numpy
M=$S/shots/v3; I=$W/site/assets/img
web() { cwebp -quiet -q 92 -alpha_q 100 -m 6 -sharp_yuv $1 -o $2; }
for t in paper night; do
  web $M/1040x700/plain-$t.png $I/window-$t.webp
  web $M/ios-loop/1180x760/plain-$t.png $I/simulator-$t.webp
  web $M/iphone/approval-$t.png $I/iphone-$t.webp
done
[[ ${1:-} == ridges ]] && $PY $HERE/gen_ridges.py $I 2880
ls -la $I | awk '{print $5, $9}' | grep -E 'window|simulator|iphone-|ridges'
