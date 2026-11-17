#!/bin/zsh
# The home page's feature pictures and loops, cut from real captures of the app.
#
#   feature_media.sh            make every picture and loop under site/assets/img/features/
#
# The page names each one by file name only (features/<name>-paper.webp, -night.webp for Night,
# <name>.mp4 and <name>-poster.webp for a loop), so a better capture replaces one by being written
# under the same name: drop it in, run `python3 script/site/stamp.py site`, done. Sizes on the page
# come from the CSS, not from the files, so a new capture may be cropped differently.
#
# Sources (all real captures of the app, staged threads, no private data):
#   ATLAS   the 0.4.54 screenshot atlas, 2560x1644 (2x), mock agent; crops leave out its "Mock" names
#   FOOT    the 2026-11-14 launch footage, 2640x1680 (2x) at 60 fps, demo dress (Claude Code, Sonnet 5.5)
#   R3      the 2026-11-17 launch captures; anything found there under a slot's name wins
HERE=${0:A:h}
W=${TREK_REPO:-${HERE:h:h}}
B=${TREK_BUSINESS:-$HOME/Developer/Trek-business-2026-11}
ATLAS=${ATLAS:-$B/2026-11-15-audit/atlas}
FOOT=${FOOT:-$B/2026-11-14-launch/footage/app}
R3=${R3:-$B/2026-11-17-launch-r3/out/site}
O=$W/site/assets/img/features
T=$(mktemp -d "${TMPDIR:-/tmp}/trek-features.XXXXXX")
trap "rm -rf $T" EXIT
mkdir -p $O
set -e

# still NAME SRC_BASE X Y W H: an atlas crop in Paper and Night (2x pixels kept).
still() {
  local name=$1 src=$2 x=$3 y=$4 w=$5 h=$6
  for theme in paper night; do
    if [[ -f $R3/$name-$theme.webp ]]; then cp $R3/$name-$theme.webp $O/; continue; fi
    ffmpeg -v error -y -i $ATLAS/$src--$theme.png -vf "crop=$w:$h:$x:$y" $T/$name-$theme.png
    cwebp -quiet -q 90 -m 6 $T/$name-$theme.png -o $O/$name-$theme.webp
  done
}

# loop NAME SRC START LENGTH CROP SCALE_W: a silent H.264 loop and its first frame as the poster.
loop() {
  local name=$1 src=$2 ss=$3 len=$4 crop=$5 width=$6
  if [[ -f $R3/$name.mp4 ]]; then cp $R3/$name.mp4 $O/; else
    ffmpeg -v error -y -ss $ss -t $len -i $FOOT/$src -an \
      -vf "${crop:+crop=$crop,}scale=$width:-2:flags=lanczos,fps=30,format=yuv420p" \
      -c:v libx264 -profile:v high -preset slow -crf ${CRF:-24} -movflags +faststart $O/$name.mp4
  fi
  ffmpeg -v error -y -i $O/$name.mp4 -frames:v 1 $T/$name-poster.png
  cwebp -quiet -q 86 -m 6 $T/$name-poster.png -o $O/$name-poster.webp
}

# Pictures: the access levels, the worktree choice, Study, Plugins.
still access   composer--access-menu    1282 262  691 655
still worktree composer--env-place      1117 628  524 238
still study    v2--study-start           768 372 1559 307
still plugins  v2--settings-plugins      884 144 1303 1484

# Loops. The Inbox: the sidebar through the whole story (one stops to ask, goes to the top, is
# allowed, finishes). Sub-agents: the first ten seconds of the same story, the thread's middle.
# Themes: the four looks and a painted background. iPhone: Allow pressed on the phone.
loop inbox    story.mp4        0 34   548:700:0:330      548
loop agents   story.mp4      0.4 9.6  1800:690:680:180   1280
loop themes   themes.mp4       0 15   ""                 1600
loop iphone   phone-allow.mp4  0 8.4  ""                 590

# The end of a turn with its actions (undo, retry, fork), from the same story.
if [[ ! -f $R3/undo-paper.webp ]]; then
  ffmpeg -v error -y -ss 30 -i $FOOT/story.mp4 -frames:v 1 -vf "crop=1240:300:690:830" $T/undo-paper.png
  cwebp -quiet -q 90 -m 6 $T/undo-paper.png -o $O/undo-paper.webp
else cp $R3/undo-paper.webp $O/; fi

ls -la $O
