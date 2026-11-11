#!/bin/zsh
# shoot_phone.sh OUT.png URL HEIGHT [dark|light] [WIDTH]
# Headless Chrome won't make a window narrower than about 500 px, so the page is drawn in a frame
# of the phone's width (its media queries answer to the frame) and the picture is cut to it.
# A page taller than Chrome can capture at 2x in one go is taken in slices and joined.
HERE=${0:A:h}
S=${TREK_SITE_SCRATCH:?set TREK_SITE_SCRATCH to a scratch folder for masters, frames and stills}
W=${TREK_REPO:-${HERE:h:h}}
PY=${TREK_SITE_PYTHON:-python3}   # needs Pillow and numpy
set -u
out=$1; url=$2; h=$3; scheme=${4:-light}; w=${5:-390}
slice=7500
y=0; n=0; parts=()
while (( y < h )); do
  wrap=$S/frames/phone-frame-$$-$n.html
  sh=$(( h - y < slice ? h - y : slice ))
  print -r -- "<!doctype html><meta charset=utf-8><title>frame</title><style>html,body{margin:0;background:#888;overflow:hidden}iframe{display:block;border:0;width:${w}px;height:${h}px;margin-top:-${y}px}</style><iframe src=\"$url\"></iframe>" > $wrap
  $HERE/shoot.sh $out.part$n.png "http://127.0.0.1:8732/phone-frame-$$-$n.html" 600 $sh $scheme 2 >/dev/null
  rm -f $wrap
  parts+=($out.part$n.png)
  y=$(( y + slice )); n=$(( n + 1 ))
done
$PY -c "
from PIL import Image
import sys
out, w = sys.argv[1], int(sys.argv[2]) * 2
ims = [Image.open(p).convert('RGB').crop((0, 0, w, Image.open(p).height)) for p in sys.argv[3:]]
sheet = Image.new('RGB', (w, sum(i.height for i in ims)))
y = 0
for i in ims:
    sheet.paste(i, (0, y)); y += i.height
sheet.save(out)
" $out $w $parts && rm -f $parts && echo "ok $out"
