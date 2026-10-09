#!/bin/zsh
# Capture marketing media without touching real data, real agents or a mouse: run a manifest
# under media/ against a headless, isolated Trek (mock agent only) and collect the PNGs and
# videos it declares. An agent inside Trek can run this same script — no MCP tools needed.
#
#   script/capture.sh list                 # the suites under media/
#   script/capture.sh check [suite|all]    # lint manifests without launching anything
#   script/capture.sh all                  # every suite
#   script/capture.sh mac/readme           # one macOS suite (media/mac/readme.cmds)
#   script/capture.sh ios/store            # an iOS suite (media/ios/store.list)
#
# Knobs (env): BUILD=0 skips the cargo build · SIZE=1280x820 window in points · FPS=15 for
# video assembly · OUT=dist/media · KEEP=1 keeps the scratch work dir · SHOT_TIMEOUT=900s per
# batch · SIM=<simulator name> for iOS (a 6.9" Pro Max for App Store shots) · TREK_SHOT_UNDER=<png>
# for the backdrop under glass.
#
# Each .cmds line is a shots.rs command (see the doc comment in crates/trek-app/src/shots.rs);
# each .list line is `NAME [record=SECS] [wait=N] [demo=0|live] -- launch args`.
set -euo pipefail
cd "$(dirname "$0")/.."
source script/lib/shot.sh

BUILD=${BUILD:-1}
export SIZE="${SIZE:-1280x820}" FPS="${FPS:-15}"
OUT=${OUT:-dist/media}
KEEP=${KEEP:-0}
SHOT_TIMEOUT=${SHOT_TIMEOUT:-900}

say() { print -r -- "$(date +%H:%M:%S)  $*"; }

VERBS="route send attach project diff pair push new settled glass tint theme tools range usage-demo pace wait record editor agent-install toast click rclick hover elements type key scroll resize approve deny answer rewind shot quit"

check_cmds() {
  local file=$1 bad=0
  while IFS= read -r line; do
    line=${line%%#*}
    line=${(MS)line##[[:graph:]]*[[:graph:]]}
    [[ -z $line ]] && continue
    local verb=${line%% *}
    if (( ${VERBS[(I)$verb]} == 0 )); then
      print -r -- "  $file: unknown command: $line" >&2; bad=1
    elif [[ $verb == route ]]; then
      case ${line#route } in
        draft|no-project|basecamp|notes|appearance|first|settings:*|title:*|project:*|thread:*) ;;
        *) print -r -- "  $file: odd route: $line" >&2; bad=1;;
      esac
    fi
  done < "$file"
  return $bad
}

check_list() {
  local file=$1 bad=0
  while IFS= read -r line; do
    line=${line%%#*}
    line=${(MS)line##[[:graph:]]*[[:graph:]]}
    [[ -z $line ]] && continue
    if [[ $line != *[[:alnum:]]* || $line == " -- "* || $line == "-- "* ]]; then
      print -r -- "  $file: bad line (want 'name [opts] -- launch args'): $line" >&2; bad=1
    fi
  done < "$file"
  return $bad
}

lint() {
  local ok=1
  for f in media/mac/*.cmds(N); do check_cmds "$f" || ok=0; done
  for f in media/ios/*.list(N); do check_list "$f" || ok=0; done
  (( ok )) && { say "all manifests lint clean"; return 0; } || return 1
}

# Assemble <name>.frames/ + <name>.ffconcat into <name>.mp4 (and .gif for short clips).
encode() {
  local concat=$1 outdir=$2
  local base=${concat:t:r}
  command -v ffmpeg >/dev/null || die "ffmpeg needed for video (brew install ffmpeg)"
  ffmpeg -hide_banner -loglevel error -y -f concat -safe 0 -i "$concat" \
    -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" -c:v libx264 -pix_fmt yuv420p -movflags +faststart \
    "$outdir/$base.mp4"
  ffmpeg -hide_banner -loglevel error -y -f concat -safe 0 -i "$concat" \
    -vf "fps=12,scale=960:-1:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse" \
    "$outdir/$base.gif" || { rm -f "$outdir/$base.gif"; say "  no gif for $base (too few frames); the mp4 has them"; }
  cp "$SHOT_DIR/$base.frames/f00000.png" "$outdir/$base-poster.png" 2>/dev/null || true
}

verify() {
  # Every `shot <name>`/`record <name>` the manifest declared should have produced a file.
  local file=$1 missing=0 name
  while IFS= read -r line; do
    line=${line%%#*}; [[ -z ${line// } ]] && continue
    set -- ${=line}
    if [[ $1 == shot && -n ${2:-} && ! -f $SHOT_DIR/$2.png ]]; then
      print -r -- "  missing: $2.png" >&2; missing=1
    fi
    if [[ $1 == record && -n ${2:-} && $2 != (wait|stop) && ! -f $SHOT_DIR/$2.ffconcat ]]; then
      print -r -- "  missing: $2.ffconcat" >&2; missing=1
    fi
  done < "$file"
  return $missing
}

run_mac() {
  local suite=$1
  local file=media/mac/$suite.cmds
  [[ -f $file ]] || die "no manifest media/mac/$suite.cmds"
  check_cmds "$file" || die "fix the manifest first (or run 'capture.sh check')"
  if grep -q '^record ' "$file" && ! command -v ffmpeg >/dev/null; then
    die "manifest records video but ffmpeg isn't installed (brew install ffmpeg)"
  fi
  local work; work=$(mktemp -d /tmp/trek-capture-mac-XXXXXX)
  trap 'shot_stop; (( KEEP )) || rm -rf "$work"' EXIT
  local bin; bin=$(shot_build)
  say "launching (isolated, mock agent, ${SIZE}pt)"
  shot_launch "$work" "$bin"
  say "running $file"
  shot_run "$file" || die "batch failed (KEEP=1 keeps $work)"
  verify "$file" || die "declared artifacts missing (KEEP=1 keeps $work)"
  shot_stop
  local out="$OUT/$suite"
  mkdir -p "$out"
  for f in "$SHOT_DIR"/**/*.png(N); do
    # Frame sequences are intermediates for the video, not artifacts.
    [[ $f == *.frames/* ]] && continue
    local rel=${f#$SHOT_DIR/}
    mkdir -p "$out/${rel:h}"
    cp "$f" "$out/$rel"
    say "  $rel"
  done
  for f in "$SHOT_DIR"/*.ffconcat(N); do
    say "  encoding ${f:t:r}"
    encode "$f" "$out"
  done
  (( KEEP )) || rm -rf "$work"; trap - EXIT
  say "published → $out"
}

run_ios() {
  local suite=$1
  local file=media/ios/$suite.list
  [[ -f $file ]] || die "no manifest media/ios/$suite.list"
  check_list "$file" || die "fix the manifest first (or run 'capture.sh check')"
  ios/scripts/capture-list.sh "$file" "$OUT/$suite"
  say "published → $OUT/$suite"
}

case ${1:-} in
  list) print -l media/mac/*.cmds(N:t:r) media/ios/*.list(N:t:r) | sed -e 's/^/  /';;
  check) lint;;
  all) for f in media/mac/*.cmds(N:t:r); do run_mac "$f"; done
       for f in media/ios/*.list(N:t:r); do run_ios "$f"; done;;
  mac/*) run_mac "${1#mac/}";;
  ios/*) run_ios "${1#ios/}";;
  ""|help|--help|-h)
    sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//';;
  *) [[ -f media/mac/$1.cmds ]] && run_mac "$1"; [[ -f media/ios/$1.list ]] && run_ios "$1";;
esac
