#!/bin/zsh
# The home page's feature pictures and loops, cut from real captures of the app.
#
#   feature_media.sh            make every picture and loop under site/assets/img/features/
#
# The page names each one by file name only: features/<name>-paper.webp and -night.webp for a
# picture in each theme, <name>.webp for one that serves both, <name>.mp4 (and <name>-night.mp4) for
# a loop with <name>-poster.webp (and <name>-night-poster.webp) as its still. A better capture
# replaces one by being written under the same name: drop it in, run
# `python3 script/site/stamp.py site`, done. Sizes on the page come from the CSS, so a new capture
# may be cropped differently.
#
# Sources, all real captures of the app with staged threads and no private data:
#   R3      the 17 November 2026 launch captures: the signpost in the title bar, real agents (Claude
#           Code with Haiku 4.5, a consult with GPT-5.6 Luna), Night theme; raw window frames
#           (capture/raw, 2880x1800 at 2x) and screen recordings (capture/video, 60 fps)
#   ATLAS   the 0.4.54 screenshot atlas, 2560x1644 (2x), mock agent; Paper versions where R3 has
#           none, cropped so no "Mock" name shows
#   FOOT    the 14 November launch footage, Paper, demo dress (Claude Code, Sonnet 5.5)
# Only raw app frames are used: none of the finished stills, with their captions and backdrops.
HERE=${0:A:h}
W=${TREK_REPO:-${HERE:h:h}}
B=${TREK_BUSINESS:-$HOME/Developer/Trek-business-2026-11}
R3=${R3:-$B/2026-11-17-launch-r3/capture}
ATLAS=${ATLAS:-$B/2026-11-15-audit/atlas}
FOOT=${FOOT:-$B/2026-11-14-launch/footage/app}
O=$W/site/assets/img/features
T=$(mktemp -d "${TMPDIR:-/tmp}/trek-features.XXXXXX")
trap "rm -rf $T" EXIT
mkdir -p $O
set -e

# crop SRC.png OUT.webp W H X Y [SCALE_W]: a crop of a still, as 8-bit RGB, to WebP.
crop() {
  local src=$1 out=$2 w=$3 h=$4 x=$5 y=$6 sw=$7
  ffmpeg -v error -y -i $src -vf "crop=$w:$h:$x:$y${sw:+,scale=$sw:-2:flags=lanczos},format=rgb24" $T/c.png
  cwebp -quiet -q 90 -m 6 $T/c.png -o $O/$out
}

# loop SRC.mp4 NAME START LENGTH CROP SCALE_W [POSTER_AT]: a silent H.264 loop, and a frame of it
# (the first, or the one POSTER_AT seconds in) as its still.
loop() {
  local src=$1 name=$2 ss=$3 len=$4 cr=$5 width=$6 at=${7:-0}
  ffmpeg -v error -y -ss $ss -t $len -i $src -an \
    -vf "${cr:+crop=$cr,}scale=$width:-2:flags=lanczos,fps=30,format=yuv420p" \
    -c:v libx264 -profile:v high -preset slow -crf ${CRF:-24} -movflags +faststart $O/$name.mp4
  ffmpeg -v error -y -ss $at -i $O/$name.mp4 -frames:v 1 $T/p.png
  cwebp -quiet -q 86 -m 6 $T/p.png -o $O/$name-poster.webp
}

# One inbox. Paper: the sidebar through the 14 November story (one stops to ask, goes to the top,
# is allowed, finishes). Night: the sidebar as three threads arrive in the Inbox, each waiting.
loop $FOOT/story.mp4     inbox        0  34 548:660:0:330    548 9
loop $R3/video/inbox.mp4 inbox-night 36  16 540:600:0:300    540 13.5

# Agents consult agents: Claude Code hands a review to GPT-5.6 Luna and waits on it.
loop $R3/video/consult.mp4 consult   16  20 1640:820:570:165 1280 9

# You stay in charge: the access menu. Night from R3, Paper from the atlas.
crop $R3/raw/access_menu.png access-night.webp 760 680 1120 1040
crop $ATLAS/composer--access-menu--paper.png access-paper.webp 691 655 1282 262

# Worktrees: where a new thread runs. The atlas has both themes.
crop $ATLAS/composer--env-place--paper.png worktree-paper.webp 500 259 1136 600
crop $ATLAS/composer--env-place--night.png worktree-night.webp 500 259 1136 600

# Undo any turn: the end of a turn with its buttons (copy, undo, retry, retry with, fork).
ffmpeg -v error -y -ss 30 -i $FOOT/story.mp4 -frames:v 1 -vf "crop=820:220:690:910" $T/u.png
cwebp -quiet -q 90 -m 6 $T/u.png -o $O/undo-paper.webp
crop $R3/raw/T_night.png undo-night.webp 820 170 590 1145

# Study: a question it set, with its answers and how sure you are.
crop $R3/raw/S_study_question.png study.webp 1580 1100 600 240

# Plugins: one at work, the focus timer round the composer.
crop $R3/raw/P_focus.png plugins.webp 1680 460 560 620

# Themes and backgrounds: the window through its looks, glass and painted backgrounds. Reshot
# 21 November 2026 by themes_loop.py from the isolated test app (the R3 take had the Focus walk
# plugin on the composer and the tools panel open), so it is not cut from R3 any more.

# iPhone: the phone's own screens, framed alike by the page. Paper: Allow pressed (a loop) beside
# Threads. Night: the same two screens as stills.
loop $FOOT/phone-allow.mp4 iphone 0 8.4 "" 590
for t in light dark; do
  ffmpeg -v error -y -i $R3/raw/phone/threads-$t.png -vf "scale=590:-2:flags=lanczos,format=rgb24" $T/t.png
  cwebp -quiet -q 88 -m 6 $T/t.png -o $O/iphone-threads-${${t/light/paper}/dark/night}.webp
done
ffmpeg -v error -y -i $R3/raw/phone/approval-dark.png -vf "scale=590:-2:flags=lanczos,format=rgb24" $T/a.png
cwebp -quiet -q 88 -m 6 $T/a.png -o $O/iphone-night.webp

ls $O
