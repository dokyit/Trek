# Capture helpers for script/capture.sh — source this file (zsh), the way script/lib/sign.sh is.
#
# State set by shot_launch: SHOT_WORK (the scratch dir), SHOT_DIR (the app's TREK_SHOT_DIR),
# SHOT_PID (the app process). Built-in knobs come from the caller's env: BUILD, SIZE, FPS,
# TREK_SHOT_UNDER.

typeset -g SHOT_WORK="" SHOT_DIR="" SHOT_PID=""

die() { print -r -- "capture: $*" >&2; exit 1; }

# Build the app with the shots feature once; prints the binary path.
shot_build() {
  [[ ${BUILD:-1} == 1 ]] || { print -r -- target/debug/trek; return; }
  print -r -- "capture: building trek --features shots" >&2
  cargo build -p trek-app --features shots -q >&2 || die "cargo build failed"
  [[ -x target/debug/trek ]] || die "target/debug/trek missing after build"
  print -r -- target/debug/trek
}

# shot_launch <workdir> — seed a throwaway data folder and start Trek headless. Isolation comes
# from TREK_SHOT_DIR itself (the process never touches the Keychain or real data, and only the
# mock agent can run); the seeded settings make the mock the default agent so a plain `send`
# can't aim at a vendor CLI that isn't there.
shot_launch() {
  SHOT_WORK=$1
  SHOT_DIR=$SHOT_WORK/shot
  mkdir -p "$SHOT_DIR" "$SHOT_WORK/data"
  cat > "$SHOT_WORK/data/settings.toml" <<'TOML'
[general]
default_agent = "direct:mock"

[onboarding]
completed = true

[import]
claude_code = false
codex = false
opencode = false

[notifications]
mode = "off"
dock_badge = false
menu_bar_icon = false

[updates]
auto_check = false
auto_download = false
TOML

  local bin=$2
  TREK_DATA_DIR="$SHOT_WORK/data" TREK_SHOT_DIR="$SHOT_DIR" TREK_MOCK_AGENT=1 \
    TREK_BACKGROUND=1 TREK_FORCE_ACTIVE=1 TREK_WINDOW_SIZE="${SIZE:-1280x820}" \
    RUST_LOG=warn,trek=info \
    "$bin" > "$SHOT_WORK/trek.log" 2>&1 &
  SHOT_PID=$!

  # Readiness probe: a `wait 100` round trip proves the command loop is up.
  rm -f "$SHOT_DIR/done"
  print -r -- "wait 100" > "$SHOT_DIR/.cmd.tmp" && mv "$SHOT_DIR/.cmd.tmp" "$SHOT_DIR/cmd"
  local waited=0
  while [[ ! -f $SHOT_DIR/done ]]; do
    sleep 0.5; waited=$((waited + 1))
    (( waited < 120 )) || die "Trek didn't answer the shot dir within 60s — see $SHOT_WORK/trek.log"
    kill -0 $SHOT_PID 2>/dev/null || die "Trek exited during launch — see $SHOT_WORK/trek.log"
  done
}

# shot_run <batch file> — feed the file to the app as one cmd batch (comment and blank lines
# stripped, written atomically), wait for `done`, and fail loudly on any `err` line.
shot_run() {
  local batch=$1 timeout=${SHOT_TIMEOUT:-900}
  [[ -f $batch ]] || die "no such batch file: $batch"
  rm -f "$SHOT_DIR/done"
  # `@REPO@` in a manifest stands for the repository's root (a real project to open).
  grep -vE '^[[:space:]]*(#|$)' "$batch" | sed "s|@REPO@|${SHOT_REPO:-$PWD}|g" > "$SHOT_DIR/.cmd.tmp" || true
  mv "$SHOT_DIR/.cmd.tmp" "$SHOT_DIR/cmd"
  local waited=0 ticks=$((timeout * 4))
  while [[ ! -f $SHOT_DIR/done ]]; do
    sleep 0.25; waited=$((waited + 1))
    (( waited < ticks )) || { print -r -- "capture: batch timed out after ${timeout}s (manifest still on disk at $SHOT_DIR/cmd, app log at $SHOT_WORK/trek.log)" >&2; return 1; }
    if ! kill -0 $SHOT_PID 2>/dev/null; then
      # `quit` ends the app right after `done` is written — check once more before failing.
      sleep 0.5
      [[ -f $SHOT_DIR/done ]] && break
      print -r -- "capture: Trek exited mid-batch — see $SHOT_WORK/trek.log" >&2; return 1
    fi
  done
  local done_text
  done_text=$(cat "$SHOT_DIR/done")
  if [[ $done_text != ok ]]; then
    print -r -- "capture: batch failed:" >&2
    print -r -- "$done_text" | sed 's/^/  /' >&2
    tail -20 "$SHOT_WORK/trek.log" | sed 's/^/  log: /' >&2
    return 1
  fi
}

# shot_stop — ask for a clean exit first (manifests can end with `quit` themselves; harmless
# either way), then kill whatever's left.
shot_stop() {
  [[ -n $SHOT_PID ]] || return 0
  if kill -0 $SHOT_PID 2>/dev/null; then
    rm -f "$SHOT_DIR/done"
    print -r -- "quit" > "$SHOT_DIR/.cmd.tmp" && mv "$SHOT_DIR/.cmd.tmp" "$SHOT_DIR/cmd"
    for i in {1..40}; do kill -0 $SHOT_PID 2>/dev/null || break; sleep 0.25; done
    kill $SHOT_PID 2>/dev/null || true
  fi
  SHOT_PID=""
}
