# Performance

How fast Trek is on Windows, measured, and how to measure it again. Numbers are from
2026-10-10 on the Windows port's laptop: x64, 20 logical cores, 32 GB, Intel Iris Xe driving a
60 Hz 1920×1080 panel at 125 %, Windows 11 Pro, release build (`--features shots`), window
1280×820. Other builds were running on the machine at the same time, so treat single runs as
noisy and look at the medians.

## Reproducing

```powershell
cargo build --release -p trek-app --features shots --locked
cargo build --release -p trek-core --example perf_seed --locked
# Copy both out of a shared target dir first: another build may overwrite them mid-run.
$env:TREK_BIN = '<copy of trek.exe>'; $env:SEED_BIN = '<copy of perf_seed.exe>'
pwsh script/perf.ps1                    # everything, three runs each, medians in dist/perf
pwsh script/perf.ps1 scroll type        # some of it
$env:STRESS_MIN = 30; pwsh script/perf.ps1 stress
```

Every run is an isolated Trek (a throwaway `TREK_DATA_DIR`, the mock agent only, input
dispatched as events through the shots harness, never real keyboard or mouse input). The
200-thread profile is made by `crates/trek-core/examples/perf_seed.rs` through the store's own
API: 200 mock threads in four projects spread over four months, four turns each with token
reports, a 2,000-item thread titled "Long transcript", and a 3,000-line Rust file per project.

Trek measures itself with `TREK_PERF=1` (`crates/trek-app/src/perf.rs`): one `perf:` line per
measurement in the log (`<data dir>\logs` when `TREK_DATA_DIR` is set, else
`%LOCALAPPDATA%\Trek\logs`). A frame is timed from the main window's root render to the end of
its paint (the main thread's work; the GPU's present and the compositor come after).
`perf frames` also samples how late the main thread answers a 5 ms timer (`lag`), which
catches work done outside frames.

## Numbers

Medians of three runs. "Before" is `windows-port` at 7993b61 with only the measuring hooks
added; "after" adds the changes below (the streaming rows with all of them, the others with the
long-answer parts alone: the later two only touch answers over 6 KB and rows over 64 KB). The
two were measured an hour apart under different load from other builds; where the fix can't
matter (startup, Basecamp, the 2,000-item
scroll, typing) an interleaved A/B run of the same two builds came out the same within noise
(scroll p50 4.0–5.9 ms against 4.9–6.0 ms, typing p50 latency 11.7–15.4 ms against 12.0–13.9).

| What | Before | After |
|---|---|---|
| Startup to first frame, empty profile (from `main` / from process start) | 219 / 261 ms | 282 / 336 ms ¹ |
| Startup to first frame, 200 threads | 218 / 246 ms | 269 / 310 ms ¹ |
| …of which GPUI's Windows platform setup, before any Trek code | 113–128 ms | 128–152 ms |
| First launch of a freshly built `trek.exe` (from process start) | 5,278 ms | 5,248 ms ² |
| Basecamp All, 200 threads, first pass over the store (off the main thread) | 57 ms | 83 ms ¹ |
| Basecamp All, the frame that draws it (caught in 1 of 3 runs before, 2 of 3 after) | 7 ms | 12–15 ms |
| Scroll up the 2,000-item transcript, 100 px a frame: frame p50 / p95 / max | 4.2 / 7.7 / 11 ms | 6.1 / 10.6 / 18 ms ¹ |
| …frames over 16.7 ms (of ~630) | 0 | 1 |
| Typing 200 keys into a 3,000-line Rust file: key to paint p50 / p95 / max | 12.3 / 20.7 / 50 ms | 13.4 / 21.9 / 47 ms |
| Inbox with 200 threads and 8 sub-agents at work: frame p50 / p95 | 1.9 / 7.5 ms | 2.1 / 10.1 ms |
| …working set; quitting with the 8 still running | 121 MB | 116 MB; 462 ms, exit 0 |
| **A thread streaming its answer, 2 min in: frame p50 / p95 (frames in 3 s)** | **66 / 83 ms (43)** | **5.1 / 7.9 ms (151)** |
| …the same with Mica on | 59 / 87 ms (47) | 4.1 / 6.6 ms (146) |
| **Scroll up through that 2-minute answer once finished: frame p50 / p95** | **78 / 126 ms** ³ | **6.0 / 10.7 ms** |
| …wheel step to paint p50 / p95 | 83 / 339 ms | 14.1 / 22.3 ms |

¹ Load, not the change: see the A/B note above. ² Windows checks a binary it hasn't seen
before on its first run; every later launch of the same file is ~250–350 ms. Users meet this
once per update. ³ Only 8–10 of 300 wheel steps painted at all (the harness then aimed the
wheel off-screen, since fixed); a separate run aimed by hand measured 108 ms a frame.

Reading the numbers:

- Key-to-paint and wheel-to-paint include waiting for the next vsync (60 Hz: up to 16.7 ms).
  The measuring window sits behind other windows (`TREK_BACKGROUND`), and GPUI caps an inactive
  window's animation frames at 30 fps, which is why the transcript scroll's p95 latency sits at
  three vsyncs (~48 ms) while its frames take 6–11 ms. A window in front doesn't have that cap.
- `lag` (how late the main thread answers a 5 ms timer) bottoms out around 10 ms on Windows: the
  timer itself is that coarse. Only the excess over that floor is work.
- Mica on or off makes no measurable difference to frame time.

### Stress: 200 threads, three 30-minute streams with sub-agents

`pwsh script/perf.ps1 stress`: the 200-thread profile, three threads streaming `mock:stream 30m`
at once (each answer passes 1 MB) and a fourth with two sub-agents reporting back after 28
minutes. Once a minute it looks round as someone would (Basecamp, the long thread, one of the
streaming threads) and times 3 s of frames on the streaming thread.

| | Before | After |
|---|---|---|
| Streaming thread, frame p50 at minute 1 / 3 / 5 | 48 / 124 / 387 ms | 7 / 7 / 5 ms |
| …at minute 15 / 30 | (stopped at 7: 2–3 fps) | 13 / 18 ms p95 (p50 6–8 ms throughout) |
| Main-thread lag, worst in any 3 s window | 467 ms by minute 5 | 36 ms (one window 53 ms) |
| Working set, start → end | 117 → 280 MB at minute 7 | 118 → 296 MB at minute 31 |
| Handles, start → end | 510 → 497 | 507 → 567 (the jump when the sub-agents reported back, then flat) |
| Threads | ~20 | ~20 |
| Quit | exit 0, no leaked handles, no panic | exit 0, no leaked handles, no panic |

Memory grows about 6 MB a minute with three streams going and isn't given back when they end,
before and after these changes alike. That is far more than the text (about 110 KB a minute
across the three), so something keeps more per update than it should; which part of the process
it is needs a heap profile (not possible here without admin). It's the open item from this pass.

## What was slow, and what changed

**A long answer made every frame slow, and a streaming one slower each second.** The
transcript is a virtualized list, but each answer was one row, and a row is laid out whole
whenever it's drawn. A frame cost in proportion to the whole answer on screen: a 40 KB answer
took over 100 ms a frame, still or streaming. While streaming it got steadily worse, since each
batch of tokens re-measures the last row, which was the whole answer: the before stress run
went 48 → 95 → 124 → 274 → 387 ms a frame over its first five minutes (2–3 fps, with the main
thread busy throughout), and was stopped there.

Now an answer of 6 KB or more is drawn in parts of at least 2 KB, each a row with its own
document (`answer_parts` in `crates/trek-app/src/thread_view.rs`). The list lays out only the
parts on screen, and streaming re-measures only the last part. Cuts fall only where a new
block starts at the left margin after a blank line, never inside fenced code, so each part is
whole markdown; a part after the first gets the paragraph gap the block would have had, so the
answer reads as one. A cut depends only on the text before it, so cuts made mid-stream stay put.
Answers under 6 KB, nearly all of them, are drawn exactly as before.

**Then, in half-hour streams, two smaller costs that grew with the answer.** With parts in place
the 30-minute run still drifted (frame p95 9 → 23 ms, main-thread stalls up to 65 ms by the end):

- Every answer row copied its item on each draw, which for a part of a long answer meant the
  whole answer, once per part on screen, every frame. Answer rows don't read the item's text
  (they draw their document), so they no longer copy it.
- Mid-turn, a thread's changes are saved a second after they happen. A save rewrites each
  changed row whole and indexes all its text for search again, on the main thread: 6–21 ms for a
  300 KB answer, more beyond, once a second per streaming thread. The save now waits a second
  more for each 64 KB of its largest changed row, up to ten (`save_delay` in `workspace.rs`), so
  the main thread spends about the same per second on saves however long the answer gets. The
  price: a crash mid-stream can lose up to ten seconds of a very long answer instead of one
  (quitting saves everything, as before).

Nothing else measured was worth changing: startup's largest share is GPUI's own platform setup
(DirectX, DirectComposition and DirectWrite, ~110–150 ms) before Trek's code runs; Trek's part
(fonts, theme, the workspace and its store, the window) is ~20–40 ms. Basecamp's first pass reads
200 threads in 60–80 ms off the main thread and later passes only what changed. The sidebar,
transcript and editor stay well inside a 60 Hz frame.

## Not measured

- **A window in front.** Every run keeps Trek behind other windows so nothing on the desktop is
  disturbed; GPUI then caps animation frames at 30 fps (see above). Frame times are unaffected,
  latencies are pessimistic by up to a vsync or two.
- **The GPU's side.** Frame times end at paint; present, the compositor and Mica's blur run after.
- **Cold disk cache.** Every startup after the first ran with Trek's files in the OS cache; a
  first launch after a reboot wasn't measured. The first launch of a new binary was (5.2 s).
- **Defender on the data folder.** All runs kept their data under `%TEMP%`; adding a Defender
  exclusion to compare needs admin. The saves' own cost (6–21 ms at 300 KB, above) includes
  whatever Defender adds; between saves, three streams leave the main thread's lag near its floor.
- **ConPTY start-up for the terminal tool, named-pipe IPC (`trek-ipc`) round trips**, and
  DirectWrite shaping on its own: no hooks for them yet.
- **Real agents.** Only the mock agent, which costs nothing to run; a real CLI agent's process
  and its output parsing come on top.
- **Where inside a frame the time goes.** No sampling profiler is installed (WPR/WPA, Superluminal
  and ETW-based tools need admin); the long-answer cause was found by experiment (the same answer
  static and streaming, then split) and from the code.
