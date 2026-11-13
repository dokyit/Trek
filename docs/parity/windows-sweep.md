# The Windows sweep

In November 2026 the Windows build was gone through feature by feature against the Mac's
(pull requests #19 and #20). This page lists what that found, what was done about each finding,
and what now holds it: a feature in [`ledger.toml`](ledger.toml), its tests, and its live check.
The ledger is the record of where each feature stands ([PARITY.md](../PARITY.md) is its table);
this page is how it got there. What's still different is at the end.

## What was reported

### Updating agents

- Copilot installed with WinGet couldn't be updated: WinGet refuses a portable package that has
  updated itself (0x8A150057), for that version and every later one. Trek runs `copilot update`.
- VS Code's `copilot` launcher, first on the machine PATH, was taken for the CLI. Trek passes over
  it for the real CLI behind it.
- Version probes left programs running, and an update replaced the files of a running agent. A
  probe ends everything it started, and an update waits for the agent's running copies.
- WinGet's codes and npm's "file in use" came back as raw output. They're said in words, other
  failures show the updater's last lines, and every outcome is logged.
- `winget upgrade` is pinned to the `winget` source, and agents' WinGet package folders are on the
  PATH Trek searches.
- Agents install on a PC without Node.js (Trek sets it up), Qwen Code installs with npm, and
  Goose's Windows script comes from its new address. Devin's install, whose own `devin update`
  only says to run its install line again, is updated by Trek from Devin's release bundle,
  checked against the SHA-256 in Devin's manifest.
- **Held by:** `agent-updates` (live: Settings › Agents), its promise ("An update waits for its
  agent's running turn"), and three opt-in checks against what's installed:
  - `cargo test -p trek-core live_agent_versions -- --ignored --nocapture` (read-only);
  - `TREK_LIVE_UPDATE=<agent> cargo test -p trek-core live_update -- --ignored --nocapture` (one
    real update, as the Update button runs it);
  - `cargo test -p trek-core live_devin_bundle -- --ignored --nocapture` (Devin's real bundle,
    into a scratch install of its own).
- **Run on a PC, 2026-11-13:**
  - Five agents were found, each with the way it was installed: Claude Code and Copilot (WinGet),
    Codex (npm), OpenCode (Chocolatey) and Devin (its installer). Updates were offered for Codex,
    Copilot and Devin.
  - Codex went from 0.167.2 to 0.169.0 in 15 seconds through the Update button's path, and the
    check then offered nothing.
  - Devin's 3000.12.57 bundle (67 MB) matched the SHA-256 in its manifest, and installed into a
    scratch install.

  Earlier runs took Copilot from 1.0.108 to 1.0.109 (the update WinGet had refused) and Codex
  from 0.167.1 to 0.167.2.

### The model picker's names

- A Claude Code without its downloaded model list showed its models as raw ids or a bare family
  name. They're named from the ids ("Opus 5.5", "Fable 5.1 (1M context)") in the picker, the
  composer's button and Basecamp, and `<synthetic>` is skipped on import.
- **Held by:**
  - `model-names` (live: the picker's menu) and its promise ("A Claude model is named by its
    version before Claude Code's own catalog is in").
  - `cargo test -p trek-agents models_live -- --ignored --nocapture`, which asks the installed
    Claude Code and Codex for their models as the picker does, and fails on a raw id or a bare
    family name.
- **Run on a PC, 2026-11-13:** Claude Code's 13 models came back as "Opus 5.5", "Fable 5.2",
  "Sonnet 5.5", "Haiku 5.5"…, and Codex's 8 as "GPT-6.1 Sol", "GPT-6 Astra", "GPT-6.1 Terra"….

### Dictating instead of typing

- Windows' own speech recognizer never answers an app without package identity, and Trek's
  installer doesn't package it: the mic heard a person, and no words came back.
- The composer's mic now dictates with Whisper on the PC (whisper.cpp with OpenAI's multilingual
  base model), in the Mac's voice row:
  - words show in grey as you speak, and settle after ✓;
  - the model (141 MB) downloads the first time the mic is clicked, pinned and checked against its
    SHA-256, and ✕ then a click carries on from the same byte;
  - being offline, a full disk, a bad download, and a muted or switched-off microphone each say
    what to do;
  - nothing it hears leaves the PC.
- Found on the way, and fixed:
  - a take ended after Windows' 5-second wait for the first words;
  - the quick pass looped ("Use the chain rule to use the chain rule to …");
  - whisper.cpp's language guess picked Cantonese, which the base model lacks;
  - a take came back in lowercase without stops, where the Mac's is punctuated;
  - ✕ and ✓ weren't circles on Windows;
  - the waveform's silent dots were barely there, on both platforms, because their opacity was
    applied twice.
- Windows' voice typing (Win+H) types into Trek's fields too. It opens by itself only on a
  processor that can't run Whisper.
- **Held by:**
  - `dictation`: live, with its promise "A take comes back written, with capitals and full stops";
  - the engine's test on real speech;
  - `win-h-in-trek`, checked by a person.

### The iPhone in the sidebar

- The sidebar's Devices showed no iPhone on Windows. It now shows the paired iPhone, Connected,
  then when it was last seen. Clicking it opens Phone & Machines, which lists it with Unpair.
- **Held by:** `devices-sidebar`. Its live check pairs a stand-in iPhone over Trek's real phone
  protocol, on a Mac and a PC in CI.

### The iPhone tool on a PC

- **Through Apple Mobile Device Service:** the phone's details, its apps (install an `.ipa`),
  files and AssistiveTouch, with agents' tools for each.
- **For the iPhone app:**
  - Bonjour announces the PC (`phone-discovery`);
  - Settings › iPhone says when Windows Firewall keeps phones out, and names the network
    (`phone-reachable`).
- **Through CoreDevice** (the services Xcode's Device Hub uses), with no Mac once the phone's
  developer disk image is in a folder:
  - the Screen tab: a picture about every second, where a click taps and a drag swipes, a box to
    type into, and Home;
  - all of the agents' phone tools, reading the screen as elements included.

  It was run on an iPhone 16 Pro Max with iOS 27.2.
- **Found on the real phone, and fixed:**
  - Xcode's disk image isn't laid out as open-source tools expect, so Trek reads its manifest;
  - iOS 27 takes the image only as a cryptex;
  - a tap did nothing, because the screen stream stopped before the touch's lift (it stays 0.7 s
    now);
  - every picture stayed in memory, 15 MB a second;
  - iOS 27 lists apps only as a stream;
  - its screenshot service takes PNG only;
  - the Screen tab went from about 17% of a core to 6%.
- **Held by:**
  - `iphone-tool`: live, with a made-up phone;
  - `crates/trek-app/tests/iphone.rs`;
  - for a real phone, `cargo run -p trek-phone --features coredevice --example pc_screen`.

## What else the sweep found

### On the Mac, missing on Windows, there now

- HEIC photos, through Windows' own codecs (`heic-photos`).
- In the Browser (`block-trackers`, `browser-downloads`, `browser-asks`, `browser-popups`):
  - Block trackers;
  - downloads in Trek's toasts;
  - a page's dialogs and permission asks in Trek's bar;
  - popups that keep `window.opener` and close with `window.close()`.
- Computer use by element, through UI Automation (`computer-use-elements`).
- The share window, from More… (`share-sheet`).
- Banners that start Trek after it has quit (`banners`).
- Video posters from Explorer's thumbnails, and video transcripts with the mic's Whisper
  (`video-posters`, `video-transcripts`).
- Agents keep working with the window closed, and Trek can stay in the tray
  (`keep-working-window-closed`).
- Accounts keep the user's setup files in step, as hard links (`accounts`).
- Updating Trek while it's closed, through a task in Task Scheduler (`update-while-closed`).
- API-key and local-model agents run their commands in Git for Windows' bash, never WSL's
  (`direct-agents-shell`).
- File paths written the Windows way show as chips (`path-chips`).
- An npx MCP server that cmd.exe can't carry still starts (`mcp-servers`).
- Codex's sandbox notice offers Set up in Terminal (`codex-sandbox`).
- Computer use's `open_app` brings a running app forward and knows the PC's own keys
  (`computer-use`).
- A turn that ran the project's check through PowerShell or cmd.exe is marked Verified
  (`verification-skills`).

### Windows bugs, fixed

- The + menu's open dialog showed folders only.
- Closing the window killed running agents. Now it asks, and Keep Working lets them finish.
- Settings › Notifications didn't say when Windows had Trek's notifications off.
- Phone uploads didn't check for free space first.
- The PC went by two names, one for phones and one for other machines (`computer-name`).
- Mac-only words (speech recognition, MLX, Python whisper) showed on Windows (`platform-words`).
- A thread's link handed to a running Trek brought the main window forward first.
- The uninstallers left Trek's `trek://` and app id keys, and skills lost their links when copied
  (`skills-symlinks`).
- **whisper.cpp's build:**
  - it made `trek.exe` need the Visual C++ redistributable (`msvcp140.dll`), which no check
    looked for (`no-runtime-to-install`);
  - it put 111 folders on the link path, past cmd.exe's limit.
- The Explorer took `..\x`, `C:\x`, `todo:v2.md` (a hidden stream) and `nul` as names in a folder
  (`explorer-names`).
- **Pages that didn't load:**
  - a page that failed showed a generic error, and one that crashed showed a blank page
    (`browser-crash`);
  - a port browsers keep pages off blamed a server that isn't running.
- A video's poster was missing for any path written with `/`.
- The status bar said LF for every file.
- An old update download was kept forever once Windows had given its Trek's process id to
  another process. On the PC this was written on, one from October had sat for weeks under a
  system service's id: 128 MB, on a nearly full disk. Now a process that started after the folder
  was made isn't taken for its owner, on both platforms (`self-update`).
- **Smaller ones:**
  - a background task's output always opened in Notepad;
  - an ffmpeg installed while Trek ran wasn't found;
  - a folder opened in the editor said "Access is denied";
  - a read-only file couldn't be saved;
  - a script copied to another machine lost its executable bit.
- The browser's starter extensions didn't say what works in WebView2 (`browser-extensions`).
- A PC's iPhone tools still called themselves "apps and files" after the screen came, in
  trek-mcp's usage and, for a tool name that isn't one, in what agents were told. They list every
  tool now (`agent-tool-families`).

### Security

- A deny rule didn't hold against cmd.exe's own spellings: `cmd /c d^el x` ran `del`. Trek now
  reads cmd.exe's text as cmd.exe does (`permission-rules`).
- Secret files were the user's alone on the Mac (0600) but took their folder's access on Windows,
  which doesn't hold for a data folder on a second drive. Every secret goes through
  `trek-private` (`private-files`, `pairing-key`).

### In the checks themselves

- A capture run could act as the user on GitHub: it made a real secret gist, which was deleted
  within minutes. It signs itself out first thing now (`capture-build`).
- An isolated run's browser saved downloads into the real Downloads folder.
- A trial build or a capture run replaced the installed Trek's banner icon.
- The Windows verify driver handed the app its caller's output pipe, so CI's live jobs waited
  until they ran out of time.
- The capture build's `import-demo` needed `sqlite3`, which Windows doesn't have, and its
  stand-in `gh` was a shell script, which Windows can't start.

### Fixes kept behind Windows' styling switch, now on the Mac too

- An opened timeline row stays in view, the context bar's slivers stay inside it, and "Compact
  now" stays until the summary is done (`transcript-polish`).
- Night's chip hover and selected segment, the completion ghost's place, and the CI graph's
  labels at 4.5:1 (`contrast-fixes`).

### Mac bugs found from the PC

- CoreText laid tabs out every 28 pt, whatever the tab size (`editor-tabs`).
- Open in didn't find apps in ~/Applications (`open-in`).
- A port browsers keep pages off left a blank tab in WebKit.
- A language server that had stopped still showed as running, on both platforms
  (`editor-language-servers`).
- Bonjour's reader had `dns-sd`'s escaping wrong (`phone-discovery`).

## What's still different

At the end of the sweep the ledger has 99 features:

- **The same on both:** 80.
- **The Mac's by nature (4):**
  - the Photos library;
  - the iOS Simulator;
  - Hide and Minimize keys;
  - onboarding's permissions.
- **Windows' by nature (4):**
  - guarded folders;
  - Win+H;
  - previewing files that live only in the cloud;
  - no console windows.

The rest are gaps. Each has a note in the ledger on what would close it.

- **Partial on Windows:**
  - `native-builds`: there's no ARM64 build.
  - `snapshots`: these go through Snipping Tool.
  - `quick-look`: a file opens in its default app.
  - `ad-blocker`: there's no per-site allow, and it stops when Edge drops Manifest V2.
  - `browser-extensions`: WebView2 runs only their content scripts.
  - `machine-target`: a PC as the target is tested on both CIs, but no run between two real
    computers has had a PC as the target yet.
  - `capture-build`: it has no `browser press` and no whole-window capture.
- **Not on Windows:**
  - `signed-builds`: there's no Authenticode certificate yet.
  - `computer-use-background`: it needs per-window capture and input.
  - `phone-app-install`: the iPhone app is built with Xcode.
- **Partial on the Mac:**
  - `signed-builds`: builds are signed, not notarized.
  - `timeline-thoughts`: whether the Mac shows them is a design decision.

## Keeping it this way

- Every platform split in the code belongs to a ledger feature, and `cargo test -p trek-parity`
  fails one that doesn't, on both CIs.
- A promise names the test that holds each platform to a behaviour.
- Every feature's live steps run on a Mac and on a PC in CI (`.github/workflows/live.yml`). Where
  they can't, the ledger says why (`live_skip`), who checked it by hand (`live_person`), or that
  only CI's throwaway machines may run them (`live_ci`).
- The pull request template asks what happens on the other platform.
- [CROSS_PLATFORM.md](../CROSS_PLATFORM.md) has the architecture, and the rules for a change.
