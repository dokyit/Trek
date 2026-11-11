# Performance on a Mac

What Trek costs on a Mac, measured, and the command that measures it again. Only what was run here
is reported: nothing below compares Trek with another app. Windows has its own numbers, and a longer
story of what was slow there: [PERF.md](PERF.md).

## Run it again

```sh
./bin/agent-cli perf                                 # everything (builds Trek first): about 25 minutes
./bin/agent-cli perf launch idle                     # some of it: launch idle stream long checkpoint memory
./bin/agent-cli perf memory --mem-idle-secs 540      # a session's memory, phase by phase, then left alone
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

## Memory over a session

Idle memory says what Trek costs before it's used. This says what a long session leaves behind:
`./bin/agent-cli perf memory` seeds a profile (`perf_seed`'s `SEED_HEAVY`, `SEED_IMAGE_THREADS`
and `SEED_ARTIFACTS`), opens Trek on a display of its own (`TREK_TEST_DISPLAY`, so macOS draws it
and pictures reach the GPU), and after each step notes `footprint` with its largest parts, and
what Trek itself holds (`perf mem`: transcripts, parsed answers, shaped text, pictures):

- eight threads of 200 items each, their answers 24 KB of prose, lists, tables and code
  blocks, every tenth 120 KB (11 MB of transcript in all): opened, then each read from the end
  up 270,000 px and back, a screenful a frame;
- three threads of screenshots, 45 pictures of the sizes screens make them (3000×2000,
  2880×1800, 3024×1964, 1920×1200, 1280×800: 702 MB decoded), two attached to each message and
  one in each answer: opened, then read the same way;
- a thread of four live pages;
- round all eleven three times, as someone going between them does;
- an answer streamed for a minute; then a new thread on screen and nothing else, for 25 seconds
  and for nine minutes more.

Taken on 2026-11-11 on the same MacBook Air, at a load of 21 to 93 (other builds). "Before" is
0.4.48 with only the measuring hooks added. "After" was run three times (the first two before
the hold on a left thread's shaped text and its five-minute end were added, which change nothing
kept while threads are read), and where the runs differ by more than a few MB the range is
given. They differ because `footprint` counts memory the allocator has been handed back and
hasn't yet returned to macOS: what Trek held (the counts) was the same each time.

| What | Before | After |
|---|---|---|
| Launch, 23 threads in the sidebar, the window drawn | 95 MB | 94 MB |
| Eight long threads opened | 145 MB | 145 MB |
| …and read through | 2,073 MB | 184 to 263 MB |
| Three threads of screenshots opened | 2,354 MB | 245 to 326 MB |
| …and read through | 3,630 MB | 317 to 422 MB |
| Round the threads three times | 3,633 MB | 267 to 417 MB |
| A one-minute answer streamed | 3,665 MB | 296 to 446 MB |
| Nothing on screen but a new thread, 25 s on | 3,656 MB | 286 to 436 MB |
| …nine minutes on | 3,625 MB | 186 to 242 MB |
| A thread shown again, of those gone round: until its frame is painted (median / p95) | 12.3 / 35.3 ms | 12.5 / 33.3 ms |
| Scrolling back up a thread of screenshots: a frame's work, p50 / p95 | 1.5 / 2.3 ms | 1.2 to 2.2 / 1.8 to 5.6 ms |

(The thread-shown-again row is from the two builds taking turns on a quiet Mac, 84 switches each,
at a load of 4 to 9: the stress's own figure moves with whatever else the Mac is doing, 5.6 / 14.5
ms before in one run and 35 / 50 ms in another, 7.7 to 17.9 / 17 to 43 ms after. "Before" was
left alone for the nine minutes in that second run, which had reached 3,641 MB where the first
reached 3,656.)
macOS had most of the 3.6 GB compressed or swapped by the end (161 MB of it resident), which is
the cost a user meets: a Mac short of memory because of a chat window.

**Where it was**, largest first:

- **Parsed answers: 1,470 MB.** An answer is drawn from a parsed document (`TextViewState`), and
  a document weighs about 150 bytes for each byte of its source: the text again as it's drawn,
  the blocks, and for code a style for every highlighted token (a 5 KB code block's styles are
  165 KB). The transcript kept the last 400 documents of the thread on screen and of each of
  twelve threads left, whatever they held: 2,137 documents of 9.6 MB of answers here.
- **Pictures: 1,550 MB.** `img(path)` decodes a file at its full size, 24 MB for a 3000×2000
  screenshot however small it's drawn, GPUI keeps the result for the run, and each one drawn has
  a texture as big. 45 pictures: 812 MB of heap and 738 MB of textures.
- **Shaped text: 450 MB.** The component kit keeps each paragraph's text as shaped (its lines,
  glyphs and a run per highlighted token) for the next frame's element of the same text. It kept
  it for as long as the paragraph's document lived, drawn or not: everything ever scrolled past.
- **Transcripts: 27 MB** for the 11 MB of text in the eight long threads, kept for every thread
  opened since launch.
- **Snapshots of live pages** weren't made in this run (a page's view is only made in a window
  that's in front, which a measurement never is). By their size: 5 MB for a card of the default
  height at 2×, 10 MB for the tallest, and the last 24 were kept whatever their size, up to 243 MB.

**What changed.** Each of these is now kept by what it weighs, the least recently used going
first, with a most for any one entry (`crates/trek-app/src/budget.rs`):

- Shaped text is let go of when no frame takes it back within two to three seconds of drawing
  (`RETAINED_IDLE` in `vendor/gpui-base/src/text/inline.rs`, a patch to the kit), and what a
  window drew last, once a minute. A thread left keeps what it was showing for five minutes
  (`hold_layouts`), so going back and forth doesn't shape it again.
- Parsed answers: 48 MiB for the thread on screen, reckoned at 150 bytes a byte of source
  (`MD_BUDGET` in `thread_view.rs`): some seventy parts of long answers with their code, several
  hundred short ones. A thread left keeps the documents it was showing, and all the threads left
  keep 24 MiB between them (`PARKED_BUDGET`), the ones left longest ago giving theirs up first.
- Pictures are decoded into copies the size they're drawn (`crates/trek-app/src/pictures.rs`):
  at most 2.4 MB for one in an answer (960×640), a third of a megabyte for a tile. 64 MiB of
  copies are kept, and those of a thread no longer shown go five to eight minutes after it left
  the screen. The viewer reads the whole picture when it opens, over the small copy, and lets go
  of it as it closes.
- Snapshots of live pages: 64 MiB, the last six of the tallest or thirteen of the usual.
- On the iPhone, the caches of parsed Markdown were counts too (3,000 to 6,000 entries, all
  dropped at once past that), keyed by the whole text, so an answer streaming in left an entry for
  every length it had on the way; and the image cache counted a picture's file size, a tenth to
  a thirtieth of what it takes decoded. They're kept by cost now, a streaming text replaces its
  shorter self, and a picture costs its pixels (`ios/Trek/Model/ByteLRU.swift`). Checked by the
  app's tests in a simulator; no memory was measured on a phone.

**What it costs.** Where something was always there before, it is now sometimes made again:

- Scrolled more than about thirty screens back in a thread of long answers, a row's answer is
  parsed again as it comes into view, as it was the first time (before: past 400 answers).
- A picture last drawn more than 64 MiB of pictures ago, or in a thread left more than five
  minutes ago, is read again when it's next on screen: its box is empty for the tenth of a
  second that takes. Nothing was read again in this run.
- A picture opened in the viewer is its small copy for the moment the whole one takes to read
  (it was already in memory before).
- Text shaped again when it comes back into view costs a few milliseconds of a frame. A thread
  shown again was 3 to 5 ms slower at the median (5.0 ms against 8.4 to 9.8, the builds taking
  turns) until it kept what it was showing for five minutes; with that it's as it was, and a
  thread left for longer pays those milliseconds once.

**Left alone: transcripts.** Unloading a thread's transcript means every reader of
`Workspace.live` has to tell "not in memory" from "empty": the phone's watchers, search results
being revealed, sub-agents reading their parent, queued follow-ups, background work, the
sidebar's previews (467 uses in 56 files). At 2.4 bytes kept for each byte of transcript it's
27 MB after eight very long threads and single megabytes after a usual day, which doesn't pay for
that yet. If it's done: unload only a thread that is off screen in every window, has no session,
turn, permission, queue, sub-agent, git job or phone watching it, and whose rows are all saved;
keep its `LiveThread` with `loaded` false so `ensure_loaded` brings it back as it does a thread
never opened.

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
- **Hours of use.** The longest stream here is two minutes, and the memory run above is twenty
  minutes of a session's worth of threads, not a day.
- **Memory on Windows, or on an iPhone.** The same code bounds the same caches there; the Windows
  growth in [PERF.md](PERF.md) has to be measured again on that laptop.
- **Notes.** A note's pictures are still decoded whole and kept for the run (`note_images.rs`).

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
