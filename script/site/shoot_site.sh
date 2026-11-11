#!/bin/zsh
# Stills of the real site folder, served only for the length of this script.
# shoot_site.sh [home|all]   VT=<ms> sets the moment (default 4300: the Inbox example's waiting moment)
HERE=${0:A:h}
S=${TREK_SITE_SCRATCH:?set TREK_SITE_SCRATCH to a scratch folder for masters, frames and stills}
W=${TREK_REPO:-${HERE:h:h}}
PY=${TREK_SITE_PYTHON:-python3}   # needs Pillow and numpy
O=$S/site/final; mkdir -p $O
(cd $W/site && exec python3 -m http.server 4176 --bind 127.0.0.1 >/dev/null 2>&1) & a=$!
(cd $S/frames && exec python3 -m http.server 8732 --bind 127.0.0.1 >/dev/null 2>&1) & b=$!
trap "kill $a $b 2>/dev/null" EXIT
sleep 0.7
export VT=${VT:-4300}
U=http://127.0.0.1:4176
for m in light dark; do
  $HERE/shoot.sh $O/home-first-$m.png $U/ 1440 900 $m 1 >/dev/null &
  $HERE/shoot.sh $O/home-full-$m.png $U/ 1440 ${FULL:-4700} $m 1 >/dev/null &
done
wait %3 %4 %5 %6 2>/dev/null
for m in light dark; do $HERE/shoot_phone.sh $O/home-phone-$m.png $U/ ${PHONE:-6200} $m >/dev/null; done
if [[ ${1:-home} == all ]]; then
  $HERE/shoot.sh $O/privacy-light.png $U/privacy/ 1440 2600 light 1 >/dev/null
  $HERE/shoot.sh $O/notfound-light.png $U/404.html 1440 900 light 1 >/dev/null
  $HERE/shoot.sh $O/notfound-dark.png $U/404.html 1440 900 dark 1 >/dev/null
fi
ls $O | tr '\n' ' '
