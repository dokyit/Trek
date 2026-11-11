#!/bin/zsh
# The site's master pictures of the Mac app: one script from a clean test profile to finished frames.
#
#   masters.sh stage                    relaunch the isolated test app (clean profile, 1320x840) and set the scene
#                                       (OPEN=one masters.sh stage: the open thread has only its plan turn)
#   masters.sh shoot W H [paper|night]  resize the window and take its masters (both themes if none is named)
#   masters.sh all                      stage, then 1320x840, 1180x760 and 1040x700 in both themes
#   masters.sh stop                     quit the test app and delete its throwaway profile
#
# Masters land in scratchpad/shots/v3/<W>x<H>/ (2x pixels):
#   plain-<theme>.png   the whole window as macOS draws it: chrome, corner shape and shadow, on transparency
#   glass-<theme>.png   the app's glass look with its real alpha (see-through sidebar and title bar), no shadow
#   raw/                the captures they were made from
# The scene: the scripted agent in its demo dress (Claude Code, Sonnet 5.5), two clean demo
# repositories, an Inbox with a plan, a question, an approval and a finished thread, one thread at
# work, and an open thread that runs from an answer with its changes to a plan waiting for approval.
# Everything is on the invisible Trek Test Display; nothing takes the keyboard or comes to the front.
HERE=${0:A:h}
S=${TREK_SITE_SCRATCH:?set TREK_SITE_SCRATCH to a scratch folder for masters, frames and stills}
W=${TREK_REPO:-${HERE:h:h}}
PY=${TREK_SITE_PYTHON:-python3}   # needs Pillow and numpy
T=$W/.agents/skills/verify-trek/scripts/trek-dev
V=$S/shots/v3
D=$V/demo
UNDER=$V/under.png
DRESS="Claude Code|Sonnet 5.5|Opus 5.5"
export CARGO_TARGET_DIR=$HOME/Library/Caches/Trek/target.nosync/wt-site CARGO_INCREMENTAL=0
cd $W
mkdir -p $V
q() { "$@" >/dev/null 2>&1 || echo "FAILED: $*"; }
shotdir() { $T status | $PY -c "import json,sys; print(json.load(sys.stdin)['runtime_dir'] + '/shot')"; }
winid() { $T windows | $PY -c "import json,sys; print(next(w['id'] for w in json.load(sys.stdin)['windows'] if w['main']))"; }

stage() {
  q $T stop
  q $T reset --yes
  $PY -c "from PIL import Image; Image.new('RGB',(64,64),(0,0,0)).save('$UNDER')"
  TREK_SHOT_UNDER=$UNDER $T start --size 1320x840 >/dev/null 2>&1 || { echo "FAILED: start"; return 1; }
  scene
}

scene() {
  rm -rf $D; mkdir -p $D/atlas-web $D/mobile
  python3 $HERE/make_repo.py $D/atlas-web atlas-web
  python3 $HERE/make_repo.py $D/mobile mobile
  q $T control dress "$DRESS"
  q $T control pace 0
  q $T control glass off
  q $T control theme paper

  # Finished and read, in the open thread's project.
  q $T project $D/atlas-web
  q $T send "Walk me through how the app starts" --timeout 120
  q $T control wait idle 40000
  q $T control title "Tour the startup code"
  q $T control click hub
  # The open thread, first turn: an answer with tool calls and the files it changed.
  # OPEN=one leaves this turn out, so the thread starts at its prompt (for the smallest window,
  # where the first turn would be scrolled out but for the foot of its last row).
  if [[ ${OPEN:-two} == two ]]; then
    q $T project $D/atlas-web
    q $T send "Add a --verbose flag to the CLI tools" --timeout 120
    q $T control wait idle 40000
  fi

  # The other project: one at work, one asking for an approval, one with a question.
  q $T project $D/mobile
  q $T send "Run the whole test suite on every platform mock:long 7200s" --no-wait
  q $T control wait 900
  q $T control title "Run the full test suite"
  q $T project $D/mobile
  q $T send "Apply the schema migration once I give permission mock:run ./migrate" --no-wait
  q $T control wait permission 20000
  q $T control title "Migrate the schema"
  q $T project $D/mobile
  q $T send "One question first mock:ask Which database?" --no-wait
  q $T control wait question 20000
  q $T control title "Choose a database"
  # One whose sub-agents report back while another thread is on screen, so it is unread in the Inbox.
  q $T project $D/mobile
  q $T send "Review the sync code with two subagents 8s" --no-wait
  q $T control wait 1500
  q $T control title "Review the sync code"
  if [[ ${OPEN:-two} == two ]]; then
    q $T open "title:Add a verbose flag"
    q $T control wait 10000
    # The open thread, second turn: a plan that waits for approval.
    q $T send "Now plan session auth for every route" --no-wait
  else
    q $T open "title:Tour the startup code"
    q $T control wait 10000
    q $T project $D/atlas-web
    q $T send "Plan session auth for every route" --no-wait
  fi
  q $T control wait plan 20000
  q $T control title "Require session auth"
  q $T control wait 9000
  q $T control hover title-text
  echo staged
}

shoot() { # W H [theme]
  local w=$1 h=$2 themes
  if [[ -n ${3:-} ]]; then themes=($3); else themes=(paper night); fi
  local out=$V/${w}x${h} shot=$(shotdir) id=$(winid)
  mkdir -p $out/raw
  $PY $HERE/shotcmd.py $shot "resize $w $h" "wait 600" || return 1
  for theme in $themes; do
    q $T control theme $theme
    q $T control glass off
    q $T control hover title-text
    q $T control wait 1200
    screencapture -x -l$id $out/raw/real-$theme.png
    q $T control glass on
    q $T control wait 1200
    $PY $HERE/glass_pair.py $shot $UNDER $out/raw/glass-$theme $w $h >/dev/null
    screencapture -x -l$id $out/raw/realglass-$theme.png
    $PY $HERE/unglass.py $out/raw/glass-$theme-black.png $out/raw/glass-$theme-white.png $out/raw/glass-$theme-rgba.png >/dev/null
    q $T control glass off
    $PY $HERE/compose3.py $out/raw $out $theme
  done
  q $T control theme paper
}

# The iOS loop: the same scene, plus a thread in a project "trek-ios" whose agent checks the Threads
# screen in the Simulator, with Trek's Simulator tool beside it mirroring the real iPhone 17e
# simulator, which runs Trek's own iPhone app in its demo mode. Plain masters only.
UDID=4B71E9BE-9CB8-4076-A0D8-4D5AB0741BA7
phone() { # light|dark: the iPhone app in the simulator, in that appearance (no Simulator.app)
  xcrun simctl boot $UDID 2>/dev/null
  xcrun simctl bootstatus $UDID -b >/dev/null
  xcrun simctl status_bar $UDID override --time 9:41 --batteryState charged --batteryLevel 100 --cellularBars 4 --wifiBars 3 2>/dev/null
  xcrun simctl terminate $UDID dev.trek.TrekMobile 2>/dev/null
  xcrun simctl launch $UDID dev.trek.TrekMobile -TrekDemo YES -TrekAppearance $1 >/dev/null
  sleep 6
}
loop_scene() { # after `stage`
  mkdir -p $D/trek-ios && python3 $HERE/make_repo.py $D/trek-ios trek-ios
  phone light
  q $T project $D/trek-ios
  q $T send "Check the Threads screen on the simulator" --no-wait
  q $T control wait 6000
  q $T control hover title-text
}
loop_shoot() { # W H
  local w=$1 h=$2 out=$V/ios-loop/${1}x${2} shot=$(shotdir) id=$(winid)
  mkdir -p $out/raw
  $PY $HERE/shotcmd.py $shot "resize $w $h" "wait 600" || return 1
  for pair in paper:light night:dark; do
    local theme=${pair%%:*}
    phone ${pair#*:}
    q $T control theme $theme
    q $T control glass off
    q $T control hover title-text
    q $T control wait 2500
    screencapture -x -l$id $out/raw/real-$theme.png
    $PY $HERE/compose3.py $out/raw $out $theme
  done
  q $T control theme paper
}
sim_off() { xcrun simctl status_bar $UDID clear 2>/dev/null; xcrun simctl shutdown $UDID 2>/dev/null; xcrun simctl list devices | grep $UDID; }

case $1 in
  loop-scene) loop_scene ;;
  loop-shoot) loop_shoot $2 $3 ;;
  loop) stage && loop_scene && loop_shoot 1320 840 && loop_shoot 1180 760; sim_off ;;
  sim-off) sim_off ;;
  stage) stage ;;
  scene) scene ;;
  shoot) shoot $2 $3 $4 ;;
  all) stage && shoot 1320 840 && shoot 1180 760 && shoot 1040 700 ;;
  stop) q $T stop; q $T reset --yes; echo stopped ;;
  *) sed -n 2,9p $0 ;;
esac
