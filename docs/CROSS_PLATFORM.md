# Cross-platform Trek: keeping macOS and Windows the same app

Trek is one tree that builds for macOS and Windows. This document is about why features and fixes
keep landing on one of them only, and about the system that stops that: what's in place, and
what comes next.

## The architecture, in one picture

A feature is built in four layers, and only the lowest knows which OS it's on:

```
  UI (trek-app)            reads runtime values: "can this machine dictate?", words from
                           trek-words, fluent::WINDOWS for looks only. Never #[cfg] for behaviour.
        │
  Shared core              every decision and reading as plain code that takes the platform
  (trek-core, agents…)     as an argument (`windows: bool`): paths, command lines, permission
                           rules, verification, key tables, names. Built and tested on both CIs.
        │
  Cross-cutting crates     one crate per difference that recurs in many places: trek-private
                           (files only the user can read), trek-job (process groups and job
                           objects), trek-words (each platform's names). The #[cfg] pair lives
                           there once instead of in every caller.
        │
  OS backends              the only #[cfg]-gated code: one trait (Desktop, dictation's engine,
                           the web view, the text system), an implementation per OS, and a fake
                           that drives the layers above in tests anywhere.
```

Four things keep it that way, the first three run on both platforms' CI for every change:

1. **The parity ledger and the gate census** (`docs/parity`, `crates/trek-parity`). Every
   user-facing feature has a status per platform; every `#[cfg]` that names a platform is
   counted, and every file with one is claimed by the feature it serves. A new split can't land
   without a feature saying how each platform does it, or why one doesn't. Being there isn't
   being as good, so a feature also keeps **promises** (`[[feature.promise]]`): what a fix on
   one platform established ("a take comes back with its capitals and full stops"), with the
   test that holds each platform to it, or a written reason a platform has none. The check
   fails a promise whose test is missing, or whose `#[cfg]` keeps it off the platform it's
   named for; `docs/PARITY.md` lists them under "Promises". A quality fix adds a promise in the
   same change, and the other platform's column is then a test or a decision, never a blank.
2. **Every test runs on both CIs.** The core's readings are tested with each platform's rules on
   each runner; a program a test needs is the fixture binary, never a shell script; the other
   platform's keys and words can be switched on (`keys::with_mac`, `trek_words::pretend`). A test
   gated to one platform says why the other can't run it.
3. **The build is one pipeline on both**: one workspace, the same test targets sharded on a Mac
   and a Windows runner, Clippy and the capture build on both, native code built with the runtime
   Trek ships (`/MT`), and a hardening test that refuses a `trek.exe` naming a redistributable.
4. **Every feature is shown working on each platform, by the same steps.** A feature in the ledger
   says how to see it in the capture build (`live`: an isolated Trek driven by commands, ending
   in a check), or why an isolated run can't show it (`live_skip`). CI holds every feature to one
   or the other, and `.github/workflows/live.yml` runs every feature's steps on a Mac and on a PC
   for each pull request, the report in the run's summary and each feature's screenshot kept.
   `cargo run -p trek-parity --bin parity-live` runs them on whichever computer it's on and writes
   `docs/parity/live-<platform>.md`: the same steps on both, so a feature built on one is seen
   working, or not, on the other, before a user finds it.

   Where it can, a check asks the system rather than the window: the tray icon's rectangle as
   the system has it, the programs under Trek's own process and their end at Stop, the console
   windows on screen, a second launch handing its link to the running Trek through its pipe, a
   page's own process ended and the reload that follows, the banners in Windows' notification
   center, Trek's switch in Windows' notification settings, a default app of the check's own
   that Trek hands a file to, how often each view draws while an agent works, the processor Trek
   runs as, the libraries loaded in it, a real rust-analyzer's problems in the editor, `gh`
   having no login inside the run, a file that lives only in the cloud (a real Cloud Files
   placeholder whose data a sync provider of the check's own hands over when Windows asks), the
   Photos picker's sheet as AppKit has it, the phone server's Bonjour record resolved through the
   system's DNS-SD as a phone resolves it, and the close button leaving Trek in the tray with no
   window, its icon still there.

   On Windows (`c48a1d3d`, a fresh profile, rust-analyzer on PATH): 84 of the
   99 features shown working, 0 failing, 8 that aren't on Windows, and 7 that an isolated run
   can't show, each saying what checks it instead: other apps' accessibility trees, the real Codex
   and OpenCode CLIs (an isolated run starts no real agent, and never acts as the user), the
   system's share window (Windows shows it only over the window in front, and a capture run stays
   behind the user's), voice typing (a person checked it), where the installed Trek lives, and
   the phone server reached from another device on the network.

   On CI's Mac (`7d49b3a6`): 84 shown working, 0 failing. Getting there took four runs, each
   failure in steps written from the Mac's code alone and never run on one: the wrong id for a
   button, a chip an unbundled build hides, a list and a window never marked for the checks, a
   script given no window, a simulator list that comes a minute later the first time on a fresh
   runner, and port 9, on browsers' restricted list, which left the tab blank on a Mac: the
   runner's WebKit (macOS 15) stops such a load before it starts and then shows an empty page,
   telling no one (its own source shows the path). Trek's navigation delegate now says it before
   WebKit is asked, and both platforms say a restricted port is why (it had read as the ad
   blocker's on a Mac, and as a server that isn't running on Windows). Steps have to run on each
   platform, not only be written for it. CI's Windows jobs first never reached
   their checks: the verify driver started the app with the step's output pipe among the handles
   it inherited, so the step waited on the app until the job ran out of time. Then the runner's
   1024×768 screen folded away the column five checks look in (the job now sets 1920×1080), and
   a turn the mock agent ran at full speed ended before the slow runner's first git checkpoint
   (the check now paces it as an agent would take it).

   Building the checks found bugs. On Windows: video posters missing for a path written with
   `/`; an isolated run's Browser saving downloads into the user's Downloads folder; the capture
   build's import demo needing a `sqlite3` program Windows lacks; the capture build never taking
   the second-launch path at all; a trial build or capture run replacing the installed Trek's
   banner icon with one in its own throwaway folder; and the status bar saying LF for files Git
   for Windows checked out with CRLF; and the verify driver handing the app its caller's output
   pipe, so anything reading the driver's output (a CI step, an agent's tool) waited on the app
   until it quit. On both: a language server that had stopped (rustup's `rust-analyzer` without
   its component exits at once) still showed as running; and a capture run could act as the user
   on GitHub through their own `gh` login. A live check made a real (secret) gist that way; it was
   deleted at once, and a capture run now signs itself out of `gh` before anything starts.

So a fix made on one platform lands in a shared layer (and both CIs test it), or in a backend
whose counterpart the ledger names. A behaviour fix hidden behind `fluent::WINDOWS` fails review
(see "For someone changing Trek").

## What goes wrong

A feature or a fix is made on the platform its author is using. The other platform's side is a
`cfg` branch: it compiles, CI may pass (many tests are gated to one OS), and the gap shows up
when a user on the other platform notices. Nothing records whether the other side was *decided*
or *forgotten*.

A full audit of the Windows build (November 2026, branch `windows/parity`) found each of these
that way:

| What | How it slipped through |
|---|---|
| Dictation existed only on macOS. On Windows the mic button was hidden, and its Win+H hint was the tooltip of that hidden button. | The Windows backend was a stub, and no test looked for the mic on Windows except one that asserted it was absent. |
| The composer's "Add photos & files" dialog showed **folders only** on Windows: no file could be attached from the + menu. | GPUI's Windows dialog can't pick files and folders together and picks folders when asked for both. The test platform says mixed picking works, so no test could see it. |
| Closing Trek's window on Windows quit Trek and stopped every running agent. On the Mac, closing the window lets agents carry on. | "Quit with the main window" was a Windows-only decision. Nothing compared it with the Mac's behaviour. |
| The iPhone tool didn't exist on Windows: its crate, tab, agent tools and Settings section were all gated to macOS. Yet its apps and files work through Apple Mobile Device Service, which Windows has. | One `cfg(target_os = "macos")` at each level, with no record of which parts truly needed a Mac. |
| Copilot updates failed for good under WinGet (0x8A150057) once Copilot had updated itself. Version probes left processes holding files open. VS Code's `copilot` launcher was taken for Copilot. | The updater's Windows paths had unit tests, but no Windows-specific failure had ever been reproduced. The failures weren't logged. |
| The model picker showed bare names ("opus") when Claude Code hadn't downloaded its model list. | Platform-neutral, but only seen on the machine where it happened. |
| Settings told Windows users about "the Mac's speech recognition" and about MLX (Apple silicon only). | The wording was written once, on a Mac. |
| The other way round: three fixes made on Windows never reached the Mac. An opened timeline row scrolled out of view, the context bar's slivers ran over "Free", and "Compacted" showed while the summary was still being written. | They sat behind `fluent::WINDOWS`, the Windows *styling* switch, so behaviour rode along with corner radii. |
| Windows code is never linted: CI runs Clippy on macOS only. | Removed to speed CI up (d610eb41). |
| Releases went out without their Windows downloads (v0.4.6, v0.4.9, v0.4.10), and after a red CI run (v0.4.49). | The release workflow doesn't look at CI, nor check what it published. |
| Once dictation was on Windows, some takes came back all lowercase and without stops ("use the chain rule to find the derivative of sine of x squared"), where the Mac's recognizer punctuates every take. | The ledger said "yes" for both: it tracked being there, not how well. Whisper writes that way now and then when given nothing to go on. Promises now name the test that holds each platform to it. |

When the audit started, about 343 of the workspace's 3,525 tests ran on one OS only, and the code
had 1,308 platform gates in 187 files, 595 of them in trek-app.

## Principles

1. **Every user-facing feature has a recorded status on each platform**, in one checked-in place.
   "Missing on Windows" is fine, if it's written down with why and what would bring it.
2. **Differences live at a backend boundary, not in the UI.** UI code asks a runtime value ("can
   this machine dictate?", "can its open dialog pick files and folders together?"). Only the code
   that calls an OS API is `#[cfg]`-gated, behind a trait with a fake.
3. **Both platforms' UI paths are tested on every CI runner.** The UI reads a runtime value, so a
   Mac runner can render the Windows UI with fakes for its backends, and the other way round.
   Real-OS tests then only have to cover the backends.
4. **A change to one platform names its counterpart.** The other side is either the same code
   (and a test covers both), done in the same change, or a recorded gap.
5. **Nothing ships that CI hasn't passed on both platforms**, and a release is checked for every
   platform's downloads.

## What's in place

### The parity ledger and the gate count (`crates/trek-parity`)

- `docs/parity/ledger.toml` lists every user-facing feature with its status on each platform:
  `yes`, `partial`, `no` or `n/a`. It also says how each platform does it, a note for anything
  `partial` or `no` (why, and what would bring it), and the code and tests that cover it.
  `docs/PARITY.md` is the same ledger as a table, generated from it.
- `docs/parity/gates.toml` records how many platform gates each source file has. That covers every
  `cfg(windows)`, `cfg!(target_os = "macos")` and `cfg_attr(windows, …)`, and each use of Trek's
  styling switch (`fluent::WINDOWS`, `fluent::pick`), which compiles one platform's way just the
  same.
- `cargo test -p trek-parity` checks three things, on both platforms' CI, as part of
  `cargo test --workspace`:
  - The ledger is well formed: unique ids, a note for every gap, and paths that exist.
  - Its table is current.
  - **Every file's gate count is the one recorded.** A change that adds or removes a gate fails
    until the counts are recorded again. The failure message says to update the feature in the
    ledger first.
  - **Every gated file belongs to a feature.** A file with gates must be named in the `code` or
    `tests` of the ledger feature whose platforms it makes differ. The 139 files that predated
    the rule were listed as `unclaimed` in `gates.toml` and have all been claimed since, so the
    list is empty, and it can't grow: a file new to it fails even when the counts are being
    recorded, so a new platform split can't land without a feature that owns it.

  ```
  TREK_PARITY_UPDATE=1 cargo test -p trek-parity   # record the counts and regenerate PARITY.md
  ```

  The count is deliberately simple: it doesn't know what a gate does, only that one appeared or
  went. Its job is to make that moment visible. The `gates.toml` diff in a pull request shows
  where a platform split was added, and the ledger diff beside it shows what was decided. When a
  refactor moves code behind a backend boundary, the counts go down, and the ratchet locks that
  progress in.

### Lints on both platforms

CI has a `lint-windows` job (`.github/workflows/ci.yml`). It runs the same Clippy correctness and
suspicious lints as the Mac's `lint` job, over trek-app's targets and every crate's lib and bins.
It has its own cache, parallel with the tests.

### Releases

`.github/workflows/release.yml`:

- **`ci-passed`**: the macOS, Intel and Windows builds wait for it. It looks up the CI workflow's
  completed runs of the commit being released and fails unless one passed. Dry runs aren't held
  up. `skip_ci_check` is the emergency override, and it's visible in the run's inputs.
- **`complete`**: after the publish jobs, whatever became of them. It checks that the release has
  every platform's archive, signature, disk image or installer, and that its `<channel>.json`
  lists `darwin-aarch64`, `darwin-x86_64` and `windows-x86_64` for this version. A release left
  without a platform says which, in red, instead of looking whole.

### The pull request template

The template has an "other platform" section. A change says what happens on the other side:
same code, done there too, or recorded in the ledger.

### The patterns this branch uses, to copy

- **Runtime values instead of `cfg` in the UI.** "The app ends with its main window" is a global
  set at launch on Windows (`root::quit_with_main_window`). Its tests (`tests/titlebar.rs`, module
  `ends_with_its_main_window`) set it themselves, so the Windows close-with-agents-at-work flow is
  tested on the Mac's CI too. The same goes for `photos::menu` (the + menu's items) and the
  attach dialog asking GPUI whether mixed picking works (`can_select_mixed_files_and_dirs`).
- **Backends behind a boundary, with a fake.** Dictation keeps the composer's API (`available`,
  `ensure_permission`, `start`, `Dictation`), whichever engine is behind it: Apple's Speech
  framework, Whisper on a Windows PC, or Windows' own recognizer for a packaged Trek. What the
  Windows engines decide (speech detection, how passes add up to the text, what a download's
  response means, the sentences) is in modules compiled on every platform, so the Mac's CI tests
  it too. A test fake drives the voice UI, the model's download included, with no microphone.
- **Native code takes the runtime Trek ships with.** C or C++ built from source into the Windows
  app (whisper.cpp) gets the static Visual C++ runtime (/MT, `vendor/whisper-rs-sys/build.rs`),
  as Trek's own programs link it (`script/static_vcruntime.rs`). Built the default way, it made
  `trek.exe` ask for `msvcp140.dll`, and Trek wouldn't have started on a PC without the
  redistributable. `tests/hardening.rs` (CI) and `release.ps1` now refuse a `trek.exe` that names
  any of the redistributable's DLLs.
- **A native build adds one folder to the link path, not its build tree.** On Windows, Cargo puts
  every search path a build script names on PATH for each program `cargo test` and `cargo run`
  start. whisper-rs-sys named all 111 folders of CMake's build tree, some 11,000 characters, and
  `cmd.exe` stopped finding commands in tests that start it: the Windows CI jobs failed while
  every test on its own passed. Trek's copy names only where the libraries are installed.
- **Words per platform in `trek-words`.** Every visible sentence that differs by platform is held
  whole there, and a test checks that the Windows table names nothing of the Mac's (Finder, Dock,
  Keychain, …). `trek_words::pretend` lets a test read either platform's wording on any OS.
- **A pure reading takes the platform as an argument.** Reading a command line or a path is a
  function that's told which platform's rules to use (`windows: bool`), and the public one passes
  `cfg!(windows)`: path chips (`md.rs`'s `looks_like_path_on`), the verification CLI's runs
  (`verification.rs`'s `invocations`), the verdict on a turn (`verdict_on`). Each CI then tests
  both readings, so a Windows rule can't break unnoticed on a Mac change, or the other way round.
- **One crate for one difference used in many places.** "Only the user can read this file" is
  0600 on the Mac and an owner-only access list on Windows; it was written out (or not) in every
  place a secret was saved, and Windows was missed in five. `trek-private` holds it once
  (`create_new`, `write`, `create_dir`, `restrict`, `is_private`), every secret file goes through
  it, and tests check `is_private` on both platforms instead of 0600 on one. Ten files' 35 gates
  became 21, six of them in that crate. When the same `#[cfg]` pair appears a third time, it's
  that kind of difference.
- **Tests that need a program use the fixture binary, never a shell script.** A `#!/bin/sh`
  stand-in runs on the Mac alone, and a `.cmd` one hands its arguments to cmd.exe. The
  test-fixtures program has verbs for what tests need (`record` notes its arguments and stdin and
  answers; `cat`, `echo`, `exit`, …), and copied under a program's name it stands in for that
  program (`gh`, `gh.exe`). `trek_test_fixtures::bin` rebuilds it when its sources are newer, so a
  CI cache never hands a test an old one.
- **The other platform's keys and links, on each runner.** `keys::with_mac(false, || open(cx))`
  binds Windows' keys on a Mac (Ctrl+K, Alt+Shift+E, Ctrl+F4 are tested on both CIs). A link test
  on Windows makes a junction (`mklink /J`), which needs no privilege, where the Mac makes a
  symlink; one that needs a file symlink skips that case where the process may not make one.

## What comes next

In order of how much each removes the problem, smallest first where that's close.

1. **A `Platform` value for UI code** (small to start, then incremental). Today it's `cfg!` in the
   UI. Instead, add a `trek-platform` crate (or a module of trek-app) with
   `Platform { os, capabilities }`, read at launch and overridable in tests, generalizing
   `trek_words::pretend` and `keys::with_mac`. Capabilities are named for what they allow:
   `Dictation`, `MixedOpenDialog`, `PhoneScreen`, `Simulator`, `NativeGlass`,
   `BackgroundComputerUse`, … Each one comes from its backend's real probe, and has a fake.
   New code uses it from day one. Old `cfg!`s convert when their code is touched, and the gate
   count shows the progress.
2. **The other platform's UI in each runner's suite** (medium). With (1), `open_with(Platform::
   windows())` renders the Windows UI on the Mac runner, and the other way round. Start with the
   screens that differ most: Settings › Tools, the composer's + menu and mic, onboarding, the
   tools panel and the title bar. GPUI's test platform needs one knob for it
   (`can_select_mixed_files_and_dirs` is fixed at `true` today).
3. **Claim the rest of the gated files** (done, 2026-11-12). Every gated file belongs to a
   feature, and the check holds new ones to it (see above). Claiming them was a sweep of every
   split: it found the gaps the ledger records since (a deny rule past cmd.exe's `^`, files that
   aren't the user's alone on Windows, tests that run on one platform only).
4. **Draft until complete** (small, needs one real release to try). `release.sh` creates the
   GitHub release as soon as the Mac half is done, so a Windows failure leaves a public release
   without Windows downloads. Create it as a draft, and let `complete` publish it once every
   platform is attached. `complete` today only says what's missing.
5. **The capture build in CI** (done). The `shots` feature (the verify driver and the
   screenshots) is checked in both platforms' lint jobs (`cargo check -p trek-app --features
   shots`), so a change can't break it unnoticed.
6. **Screens checked on both platforms** (medium to large). A handful of key screens rendered by
   the capture build on each runner, compared with checked-in references and with each other where
   they should match (spacing, wording, what's present). The verify driver
   (`.agents/skills/verify-trek`) already does it by hand.
7. **Real-OS backend tests where the OS is the point** (ongoing). Windows dictation, Apple Mobile
   Device Service, WebView2's events, DWM: each backend keeps its pure logic in a module tested
   everywhere, and a thin OS layer with a few tests on its own runner.
8. **Close the recorded gaps** (ongoing). The ledger is the backlog. Computer use by element,
   the browser's asks and popups, HEIC photos, the share window, banners that start Trek,
   dictation on the PC and video transcripts without whisper have closed. The biggest left for Windows users are computer use in the
   background, window snapshots through Windows.Graphics.Capture, signed builds, an ARM64 build,
   updating while Trek is closed, and the iPhone app's own build, which needs a Mac. The iPhone's
   screen and taps from a PC no longer need one: CoreDevice's own services (the ones Xcode's Device
   Hub uses) reach the phone over its tunnel with no driver to sign (`trek-phone`'s `pc_screen`
   example), and are left to run against a phone, then to put in the panel.

## For someone changing Trek

- Platform-neutral code (most of trek-core, the agents, the UI's logic) needs nothing extra. Its
  tests already run on both platforms.
- Adding a `cfg(windows)` or `cfg!(target_os = "macos")`:
  1. Ask whether it can be a runtime value or a backend instead.
  2. If it's a real difference, update the feature in `docs/parity/ledger.toml`: its `how`, or
     `partial`/`no` with a note.
  3. Run `TREK_PARITY_UPDATE=1 cargo test -p trek-parity`, and commit the ledger, `gates.toml`
     and `PARITY.md` with the change.
- Fixing a bug seen on one platform: check whether the other platform's path has it too, and say
  so in the pull request.
- Adding a feature, or changing what one shows: give it `live` steps in the ledger (the capture
  build's commands, `crates/trek-app/src/shots.rs`, ending in `count <element id> <n>`), or say in
  `live_skip` why an isolated run can't show it; then start the capture build
  (`.agents/skills/verify-trek`) and run
  `cargo run -p trek-parity --bin parity-live -- --shot-dir <its runtime folder>/shot --feature <id>`.
- Fixing how well something works (not whether it's there): add a `[[feature.promise]]` to the
  feature with the test that holds this platform to it, and the other platform's test, or under
  `untested` why there isn't one.
- Writing a test: make it run on both CIs. A reading takes the platform as an argument; a program
  it needs is the fixture binary (a verb, or copied under the program's name), not a script; a
  platform's keys are bound with `keys::with_mac`; a link on Windows is a junction. A test gated
  to one platform says why the other can't run it, and the feature's ledger note says so too.
- Saving a secret: through `trek-private`, never `fs::write` with a `#[cfg(unix)]` mode beside it.
- Adding C or C++ to the Windows build: give it the static runtime (/MT), as
  `vendor/whisper-rs-sys/build.rs` does for whisper.cpp (`tests/hardening.rs` fails otherwise),
  and keep its build script's link-search paths to the folders its libraries are in.
- `fluent::WINDOWS` is for how Windows looks: corners, strokes, colours. A behaviour change goes on
  both platforms, or into the ledger with why not, never behind the styling switch.
- Words that name a platform's things go in `trek-words`, held whole when more than a word
  differs.
