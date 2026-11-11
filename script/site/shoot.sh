#!/bin/zsh
# shoot.sh OUT.png URL WIDTH HEIGHT [dark|light] [scale]
# Headless Chrome only: nothing appears on screen. One throwaway profile per call.
HERE=${0:A:h}
S=${TREK_SITE_SCRATCH:?set TREK_SITE_SCRATCH to a scratch folder for masters, frames and stills}
W=${TREK_REPO:-${HERE:h:h}}
PY=${TREK_SITE_PYTHON:-python3}   # needs Pillow and numpy
set -u
out=$1; url=$2; w=$3; h=$4; scheme=${5:-light}; scale=${6:-1}
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
prof=$(mktemp -d "${TMPDIR:-/tmp}/trek-site-chrome.XXXXXX")
args=(--headless=new --user-data-dir=$prof --no-first-run --no-default-browser-check --disable-extensions
  --hide-scrollbars --force-device-scale-factor=$scale --window-size=$w,$h
  --virtual-time-budget=${VT:-9000} --run-all-compositor-stages-before-draw --screenshot=$out)
[[ $scheme == dark ]] && args+=(--force-dark-mode --blink-settings=preferredColorScheme=0)
[[ $scheme == light ]] && args+=(--blink-settings=preferredColorScheme=1)
"$chrome" $args "$url" >/dev/null 2>&1 &
pid=$!
( sleep 75; kill -9 $pid 2>/dev/null ) &
watch=$!
wait $pid 2>/dev/null
kill $watch 2>/dev/null
rm -rf $prof
[[ -s $out ]] && echo "ok $out" || echo "FAILED $url"
