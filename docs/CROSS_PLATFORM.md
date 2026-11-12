# Cross-platform Trek: keeping macOS and Windows the same app

Trek is one tree that builds for macOS and Windows. This document is about why features and fixes
keep landing on one of them only, and about the system that stops that: what's in place, and
what comes next.

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
    `tests` of the ledger feature whose platforms it makes differ. The 139 files that predate
    the rule are listed as `unclaimed` in `gates.toml`. That list only shrinks: a file new to it,
    or one on it that gains a gate, fails even when the counts are being recorded, so a new
    platform split can't land without a feature that owns it.

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
3. **Claim the rest of the gated files** (ongoing). The check now requires a feature for any
   file that gains a gate (see above). What's left is the `unclaimed` list in `gates.toml`:
   each file comes off it when the feature it belongs to names it.
4. **Draft until complete** (small, needs one real release to try). `release.sh` creates the
   GitHub release as soon as the Mac half is done, so a Windows failure leaves a public release
   without Windows downloads. Create it as a draft, and let `complete` publish it once every
   platform is attached. `complete` today only says what's missing.
5. **The capture build in CI** (small). The `shots` feature (the verify driver and the
   screenshots) is never built in CI, so it breaks unnoticed. A `cargo check -p trek-app
   --features shots` on both platforms, in the lint jobs, would catch that.
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
   updating while Trek is closed, and the iPhone app's own build, which needs a Mac.

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
- Adding C or C++ to the Windows build: give it the static runtime (/MT), as
  `vendor/whisper-rs-sys/build.rs` does for whisper.cpp (`tests/hardening.rs` fails otherwise),
  and keep its build script's link-search paths to the folders its libraries are in.
- `fluent::WINDOWS` is for how Windows looks: corners, strokes, colours. A behaviour change goes on
  both platforms, or into the ledger with why not, never behind the styling switch.
- Words that name a platform's things go in `trek-words`, held whole when more than a word
  differs.
