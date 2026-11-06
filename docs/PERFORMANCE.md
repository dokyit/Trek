# Performance on a Mac

What Trek costs on a Mac, measured, and the command that measures it again. Only what was run here
is reported: nothing below compares Trek with another app. Windows has its own numbers, and a longer
story of what was slow there: [PERF.md](PERF.md).

## Run it again

```sh
./bin/agent-cli perf                                 # everything (builds Trek first): about 15 minutes
./bin/agent-cli perf launch idle                     # some of it: launch idle stream long checkpoint
./bin/agent-cli perf idle --front                    # idle on screen too: a small window above yours for 20 s
./bin/agent-cli perf --bin <trek> --tools <folder>   # a Trek you built, and the perf_seed and perf_checkpoint examples
./bin/agent-cli perf --out dist/perf/mac.json        # the results as JSON too
```

It builds Trek in release mode with the `shots` feature, which the harness that drives it needs
(`cargo build --release -p trek-app --features shots`), and two examples of `trek-core`. Every run is the
isolated profile: a throwaway data folder, the mock agent only, nothing imported, nothing of yours read or
written, the window behind your others and never given the keyboard (`--front` puts it above them, still
without the keyboard). Thread histories come from `crates/trek-core/examples/perf_seed.rs`, written through
the store's own API.

## The numbers

Taken on 2026-11-05 on a MacBook Air (Mac17,4): Apple M5 with 10 cores (4 performance, 6 efficiency), 24 GB,
macOS 27.2, the built-in 60 Hz display, on its charger. Release build, `arm64`.

**The Mac wasn't idle.** It's the one Trek is developed on, and other builds came and went. Each measurement
started after nothing had compiled for 45 seconds, at a load average of 6 to 38 on 10 cores (the 1-minute figure
`uptime` gives), and a few overlapped a build that started later: the idle runs ended at a load of 167, the last
checkpoint run at 203. Launch times move most with load; memory and the process's own CPU much less.

| What | Result |
|---|---|
| Launch to the first frame, warm: no threads / 10 / 100 | 201 / 191 / 186 ms |
| Launch, cold, 100 threads | 559 ms from a volume mounted for the first time; 235 and 243 ms after remounting it |
| Memory while idle (footprint): no threads / 10 / 100 | 55 / 56 / 57 MB |
| …the same as RSS | 87 / 88 / 91 MB |
| …100 threads, the 1280×820 window drawn as it is in front | 80 MB (60 MB in the same pair of runs undrawn) |
| CPU while idle over 60 s, window behind others: no threads / 10 / 100 | 0.03 / 0.02 / 0.03 % of one core |
| …context switches | 13 / 6 / 6 a second |
| CPU while idle over 15–30 s, window on screen, 10 or 100 threads | 0.13 to 0.20 % of one core, 22 to 24 context switches a second |
| Streaming a two-minute answer, 100 threads: CPU | 21 and 25 % of one core in two runs (28 and 28 % for the build before this work) |
| …RSS before, at its peak, a while after | 97, 160, 127 MB |
| …each frame's work on the main thread, p50 / p95 | 1.7 / 2.3 ms, 57 frames a second |
| Opening a thread of 10,000 items: history loaded / first frame of it | 136 / 157 ms (83 / 107 ms in an earlier run) |
| …RSS before opening it, open, after scrolling it | 139, 171, 183 MB |
| …scrolling it, a frame's work p50 / p95 | 3.3 / 5.1 ms, 300 frames, none missed |
| …a new answer streaming at its end, p50 / p95 | 5.4 / 8.2 ms |
| A turn's Git checkpoint, off the main thread: 500 files / this repo (4,727) / 100,000 files | 80 / 140–360 / 210 ms (520–740 ms the first time in a run) |
| …100,000 files whose times all changed | 2.7 s, 9.6 s the first time (the limit is 10 s) |

How each is taken:

- **Launch**: from spawning the executable to the main window's first frame painted, with the thread list in it
  (the store is read before the window opens). Warm: the median of five launches, after one that's thrown away.
  Launching an app through Launch Services, as a double click does, adds time this doesn't count. From `main`,
  about 40 ms goes to GPUI's platform setup, 70 ms to setting up the component kit and its highlighting, and 50 ms
  to the window and its first frame; Trek's store and workspace take a few milliseconds, 100 threads or none.
- **Cold launch**: Trek and its data on a volume of their own, unmounted and mounted again before each run, so none
  of their pages are in the file cache. It can't drop the system's libraries, nor the cache of the disk image under
  the volume (`purge` needs root), so a launch after a restart will take longer. The first run is also the first time
  macOS sees the file.
- **Memory**: `footprint` and `ps`, 25 seconds after launch. Footprint is what Activity Monitor calls Memory, the GPU's
  included; RSS also counts system libraries shared with every app. A window behind others isn't drawn after its first
  frame; "drawn as it is in front" keeps drawing it (`TREK_FORCE_ACTIVE`), which adds its surfaces and glyphs. An
  answer with code in it grows the glyph cache further.
- **Idle CPU**: the process's CPU time from `ps` over 60 seconds, as a share of one core, and its context switches from
  `top`. The window is behind others, where macOS stops its display link, and the harness looks for commands every
  5 seconds.
- **Idle on screen**: 15 to 30 seconds with a 640×420 window above the others, not active. The harness looks for commands every
  300 ms here, about 18 of those context switches a second.
- **Streaming**: `mock:stream` for two minutes in the thread on screen, a token every 14 ms (about 70 a second, a fast
  model's pace). Frames are drawn as if the window were in front (`TREK_FORCE_ACTIVE`). CPU is over the stream; RSS
  peaks are from a sample every 5 seconds. A frame is timed from render to the end of paint (`crates/trek-perf`). Nearly
  all of this CPU is drawing: with the window hidden, which draws nothing, the same stream costs 0.8 %.
- **Long thread**: 10,000 items, as turns of a prompt, a thought, a tool call with 30 lines of output, a markdown answer
  and an end. Scrolling is 100 px a frame for 300 frames, each timed to its paint.
- **Checkpoint**: `crates/trek-core/examples/perf_checkpoint.rs` takes the same snapshot a turn does, five times. "This
  repo" is a clone of Trek's own repository; 100,000 files is one made for it, 800 bytes a file.

## What changed

The same measurements before and after. "Before" is the code before this work with only the measuring hooks added.

| What | Before | After |
|---|---|---|
| First token after a message, this repo (median of 5) | 190 ms (179–231) | 122 ms (119–123) |
| …in a repo of 100,000 files | 602 ms (263–783) | 122 ms (111–127) |
| …the same repo after every file's time changed | 2.8 s (2.1–3.0) | 135 ms (122–141) |
| Idle on screen, 10 or 100 threads: CPU (runs of 15–30 s) | 0.40 to 0.73 % of one core | 0.13 to 0.20 % |
| …context switches | 143 to 157 a second | 22 to 24 a second |

The first token is the mock agent's, which has no model to wait for and answers at once: what's left is how long the
message waited. 100 ms of it is a wait that's meant (below), and the rest is the session and the harness.

**A message waited for its Git checkpoint.** A turn starts with a snapshot of the files so a rewind can put them back
(`crates/trek-core/src/checkpoint.rs`), and the message was held until the snapshot was done, up to the snapshot's limit of
10 seconds. That's the trap that made Zed remove its per-message checkpoints and Conductor make its own non-blocking. Now a
message waits for its checkpoint at most 100 ms (`CHECKPOINT_WAIT` in `crates/trek-app/src/workspace.rs`), and a
checkpoint that needs longer goes on being taken while the agent works. A small repository's is done within the wait (500
files: 80 ms), so the agent starts from exactly the files the checkpoint holds. Restoring files is different: a message
still waits for that in full, since a turn mustn't start before the files are back.

The price: in a repository whose snapshot outlasts the wait, the snapshot may read a file after the agent changed it. That
change then counts as the files before the turn, and a rewind of the turn leaves that file as it is. For that the agent has to
answer and edit a file inside the snapshot's time, which here is a few hundred milliseconds; it's more likely only where a
snapshot takes seconds (every file's time changed, as above, or far more files), up to the 10-second limit, past which a turn
goes without one.

**An idle window on screen woke the CPU 60 times a second.** While a window can be seen, GPUI's display link ticks at the
display's rate whether there's anything to draw or not. Now a window with nothing to draw for 250 ms stops listening to it,
and starts again when GPUI wants a frame (`FRAMES_IDLE_LIMIT` and `frame_waker` in `vendor/gpui-pre-macos/src/window.rs`). The
first frame after a pause is asked for at once, not at the next tick, so input isn't painted any later. After 2 seconds still, one
wheel step was painted in 1.8 to 16 ms (median 3 ms) against 2.5 to 45 ms (median 27) with the link always on, and one key in 1.5
to 3.3 ms against 2.3 to 9.9; none went unpainted. (Those runs were at a load of 200 to 300. With the link ticking, a frame waits
for the next tick.) The CPU numbers in the table are from `ps`, which counts in steps of 10 ms: over 20 seconds, 0.05 % is one
step. A run where the window turned out not to be seen (another Space, a locked screen) shows the "after" numbers whatever the
build, so each "before" run was paired with an "after" one, and only pairs whose "before" ticked count.

**Tried, and not kept**: applying a streamed answer's text 30 times a second instead of 60. Frames went from 57 to 45 a second
(the working bar's own frames fill in), and CPU didn't move outside the spread between runs. Streaming's CPU in the table came out
lower after this work than before (21 and 25 % against 28 and 28 %); nothing here was aimed at it, so it isn't claimed.

## What didn't need changing

- **Idle timers.** A hidden Trek has nothing that ticks for nothing: in 20 seconds of `sample` its threads spent nearly all their
  time waiting, and its timers are a minute apart (the sidebar's clock, housekeeping) but for one that asks the system every
  2 seconds about reduced motion.
- **Long threads.** Older messages aren't loaded lazily: the whole history is read when a thread opens, off the main thread, and kept
  in memory, while rows are laid out only as they come on screen. At 10,000 items that's the time and memory above, so paging the
  history in wouldn't pay for itself yet. Items with much larger tool output than these cost more memory.

## Not measured

- **A real Intel Mac.** Below, the Intel build runs under Rosetta here, which says little about speed or an Intel GPU.
- **The window active.** A measurement must never take the keyboard, so the window is never frontmost. Active, its animations
  aren't capped at the 30 frames a second of an inactive window, and a focused field's caret blinks.
- **Real agents.** Only the mock, which costs nothing to run. A real agent's own process comes on top.
- **Hours of use.** The longest stream here is two minutes.

## Intel, under Rosetta

The Intel build (`docs/RELEASING.md`, "Intel Macs") on this Mac under Rosetta, at a load of 18 to 54. It runs, draws the same
(transcript, code highlighting, composer), and asks the updater for the Apple-silicon build. These say what Rosetta costs here,
not how an Intel Mac would do:

| What | Result |
|---|---|
| Launch to the first frame, warm, 100 threads | 318 ms |
| Launch, cold | 7.2 s the first time (Rosetta translates the file), then 356 and 362 ms |
| Memory while idle, 100 threads (footprint) | 60 MB |
| CPU while idle over 60 s | 0.03 % of one core |
