#!/bin/zsh
# Render the disk image window's background, assets/dmg/background.svg, to the two PNGs
# script/dmg.sh uses (1x and 2x). They're committed, so a release doesn't depend on the fonts of
# the machine that builds it; run this after changing the SVG.
#
#   script/dmg-background.sh
set -euo pipefail
cd "${0:A:h}/.."
command -v resvg >/dev/null || { echo "dmg-background: resvg isn't installed (brew install resvg)" >&2; exit 1; }
resvg -w 640 -h 420 assets/dmg/background.svg assets/dmg/background.png
resvg -w 1280 -h 840 assets/dmg/background.svg assets/dmg/background@2x.png
echo "• assets/dmg/background.png, assets/dmg/background@2x.png"
