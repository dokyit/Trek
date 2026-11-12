---
name: verify-trek
description: Drive, debug, and verify Trek in a disposable mock-only profile. Use it to check any Trek change works before calling it done.
metadata:
  trek: verification
  cli: ./.agents/skills/verify-trek/scripts/trek-dev
---

# Verify Trek

Run the CLI from the checkout or worktree being verified. Its state is keyed to `git rev-parse --show-toplevel`, so it builds that checkout. JSON is the default output; add `--human` for readable text. On Windows the driver is `scripts/trek-dev.ps1` — run it with `pwsh` in place of `./scripts/trek-dev` (same subcommands and output).

The driver always launches with `TREK_SHOT_DIR`, a throwaway `TREK_DATA_DIR`, imports and notifications off, and `direct:mock` as the only agent. Never replace those safeguards or point the driver at a normal Trek process: imported threads can resume real sessions.

On a Mac the isolated app opens on an invisible display of its own, the [Trek Test Display](#test-display-macos): none of its windows is on the owner's screens, and nothing of theirs moves.

## Workflow

1. Read the relevant entry in the [Feature Map](references/features/README.md).
2. Check the machine, start the isolated app, and reach the feature.
3. Capture the result and inspect logs when behavior is wrong.
4. Run `check` before calling the change verified; stop or reset the profile afterward.

```sh
./.agents/skills/verify-trek/scripts/trek-dev preflight
./.agents/skills/verify-trek/scripts/trek-dev start
./.agents/skills/verify-trek/scripts/trek-dev status
./.agents/skills/verify-trek/scripts/trek-dev open settings:appearance
./.agents/skills/verify-trek/scripts/trek-dev project .
./.agents/skills/verify-trek/scripts/trek-dev send "Show a short streamed answer"
./.agents/skills/verify-trek/scripts/trek-dev control theme paper
./.agents/skills/verify-trek/scripts/trek-dev screenshot appearance --output /tmp/trek-appearance.png
./.agents/skills/verify-trek/scripts/trek-dev logs --lines 120 --grep error
./.agents/skills/verify-trek/scripts/trek-dev check
./.agents/skills/verify-trek/scripts/trek-dev stop
./.agents/skills/verify-trek/scripts/trek-dev reset --dry-run
./.agents/skills/verify-trek/scripts/trek-dev reset --yes
```

## Every feature at once

Each feature in `docs/parity/ledger.toml` says how to see it working here (`live`: these commands, ending in what must be on screen), or why an isolated run can't show it (`live_skip`). With the app started, `parity-live` runs them all, or the ones named, and writes a page of what passed:

```sh
cargo run -p trek-parity --bin parity-live -- --shot-dir <runtime_dir from status>/shot \
    [--feature dictation]... [--report docs/parity/live-<platform>.md]
```

It makes a small git project beside the command folder for steps to work in (`{project}` in a step; `{/}` is the path separator), and saves `live-<id>.png` for each feature. Commands written for it: `count <id>[*] <n>+` (n or more), `reveal <id>` (Settings scrolled until it's on screen; `reveal-in <scroller> <id>` for another), `pair-phone [name]|leave` (a stand-in iPhone paired over the real protocol, with a phone server on the loopback alone), and on Windows a real shell in the terminal when the app is started with `TREK_VERIFY_SHELL=1` (no profile, never a command). Checks that ask the system rather than the window: `tray off|on|<state>` (the tray or menu bar icon, as the system has it on screen), `programs <name> <n>` then `programs-ended` (a program under Trek, as `mock:hold` leaves one running through Trek's own executor, gone after Stop), `console-windows mark|new <n>` (Windows), `second-launch <link>` (Windows: Trek started again hands it over and exits), `native` and `self-contained`, and `browser crash` (the page's own process ended) with `browser expect crashed|failed[:<address>]|loaded`. A check that counts an element below the fold fails: reveal it first; a row's id often carries an index (`tool-group#2`), so count it as `tool-group*`.

Use `--help` after any command for its routes, arguments, dry-run behavior, and examples. `check --quick` is for iteration only: the full check parses shell scripts and runs `bin/agent-cli --selftest`, runs the same Rust package tests as CI, builds the `shots` feature, then launches isolated Trek and captures Basecamp and General Settings.

## Test display (macOS)

`start` sets `TREK_TEST_DISPLAY=1`: before it opens a window, the app makes a virtual display named "Trek Test Display" (`crates/trek-app/src/test_display.rs`, `shots` builds only) and every window of the run opens there: the main window, thread windows and the background-use picture, with sheets and popovers going where their window is. Nobody sees that display. macOS still counts a window on it as visible, so it draws it at the display's rate (a streaming thread: 55 frames a second with no help, none at all behind the owner's windows), and its real pixels can be captured without showing it to anyone.

```sh
./.agents/skills/verify-trek/scripts/trek-dev preflight                          # test_display.api: can this macOS make one
./.agents/skills/verify-trek/scripts/trek-dev status                             # test_display: id, size, bounds, where it sits
./.agents/skills/verify-trek/scripts/trek-dev windows                            # each window's id, frame and display; all_on_test_display
./.agents/skills/verify-trek/scripts/trek-dev screenshot glass --real            # the main window as macOS composites it
./.agents/skills/verify-trek/scripts/trek-dev screenshot glass --real --display  # the whole test display, every window on it
./.agents/skills/verify-trek/scripts/trek-dev start --no-test-display            # as before: on the owner's screen, behind their windows
```

- **Size.** 2x, like a Retina screen, so screenshots are as sharp as before: 1920x1200 points for the default window, larger for a larger `--size` (up to 3840x2160). A window larger than the owner's own screen now fits (`--size 2000x1300`).
- **Which screenshot.** `screenshot` is still Trek's own offscreen render: no permission, no native views, and glass over a stand-in. `screenshot --real` is `screencapture` of the window's rectangle on the test display: real Liquid Glass and vibrancy over that display's wallpaper, web views and other native views included, at the same pixel size. It uses the Screen Recording permission of the app the terminal runs in (the test binary asks for none), and macOS shows its capture indicator in the menu bar for a moment, as for any `screencapture`. `--display` takes the whole test display, for a sheet, a second window or the background-use picture.
- **Where it sits.** Diagonally off one corner of the desktop (bottom-left unless that's a hot corner), touching it at that one point, so a pointer can't wander onto it along an edge. macOS puts every new display beside the main one first: for the first third of a second of a `start` it is there, along the right edge, before it's at its corner. It's placed for the app only: the owner's displays keep their arrangement, mode and main display, nothing is mirrored, and Trek saves nothing to their display settings. It goes when the app exits, crashes or is killed; on `stop` the windows close first (`stop` reports `test_display.gone`).
- **Several at once.** Each checkout's app has its own display, in the first free slot: "Trek Test Display", "Trek Test Display 2", up to eight. A later one hangs off the far corner of an earlier one, off the real desktop altogether. When the earlier app quits, macOS puts the later display back beside the main one and the later app moves it to a corner within a quarter of a second.
- **Opting out.** `start --no-test-display`, or `TREK_TEST_DISPLAY=0` in the environment. If the display can't be made (`status` says why under `test_display.why`), the run carries on the old way by itself. `TREK_FORCE_ACTIVE` stays set either way: it's what keeps a window drawn when there is no display, and when one of the app's own windows covers another (a thread window opened behind the main one).
- **With "Displays have separate Spaces" off** (`preflight` says so), the test window should be in whatever Space was showing when it opened, and stop being drawn when the owner switches away. That hasn't been tried: the Mac this was built on has it on.
- Never place a display "for the session" or permanently (`kCGConfigureForSession`, `kCGConfigurePermanently`): macOS 27 writes both to `/Library/Preferences/com.apple.windowserver.displays.plist`.
- Windows has no test display and needs none; `trek-dev.ps1` refuses `--real`.

To check the display itself: `start`, then `windows` must say `all_on_test_display: true` and the main window `drawn: true`; `status` must show `test_display.live: true` and a `sits` that names one corner; `stop` must say `test_display.gone: true`. To see that nothing of the owner's moved, compare `CGGetActiveDisplayList` bounds and every window's bounds from `CGWindowListCopyWindowInfo` (all of them, other Spaces included) before `start`, while it runs, and after `stop`; the pointer's own window, at the topmost layer, follows the mouse.

## Development environment

Trek development requires Apple-silicon macOS 13+, stable Rust, full Xcode, and Xcode's Metal Toolchain. On Xcode 26+, install a missing Metal compiler once with:

```sh
xcodebuild -downloadComponent MetalToolchain
```

A fresh checkout needs no secrets, database seed, test account, or vendor-agent login for verification. Cargo fetches Rust dependencies. The CLI writes its disposable settings, SQLite data, logs, command files, and screenshots beneath the macOS temp directory shown by `status`; it never uses `~/Library/Application Support/dev.trek.Trek` or the Keychain.

For ordinary manual development, `cargo run -p trek-app` uses the normal profile and may discover real agents and imports. Do not use that mode for automated verification.

