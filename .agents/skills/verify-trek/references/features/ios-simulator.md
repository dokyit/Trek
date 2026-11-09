# iOS Simulator

## What it does

Discovers Xcode simulators, boots/shuts them down, mirrors frames, sends touch/keyboard input through AXe, installs/launches apps, opens URLs, and attaches device screenshots to chat. The input and frame helper (`trek-simhid`, `crates/trek-app/simhid/main.swift`, built from AXe's FBSimulatorControl) exits quietly when its simulator shuts down: FBSimulatorControl crashes on that, and macOS used to show a crash report every time a simulator shut down with Trek open. It writes "the simulator went away under its connection" to stderr instead; Trek starts it again when needed. `cargo test -p trek-app --lib the_helper_builds -- --ignored` builds it (`TREK_SIMHID_KEEP=<path>` keeps the binary to try by hand).

Agents read the screen before they look at it. `sim_elements` gives AXe's `describe-ui` (the accessibility tree, as JSON with frames in device points) as the same outline computer use and the Browser give: one line per element with its role, name, value (never a secure field's), state and frame (`[frame=x,y,width,height]` in `sim_screenshot` pixels), and a ref (`[ref=e12]`) on each one a tap does something to; `filter`, `interactive_only` and `max` narrow it. `sim_tap` with a `ref` reads the screen again and taps the element's centre only once it's found again and proven the same: the same role and name at the same place in the tree, about where it was, among the same neighbours (the simulator's tree has no object identity, so the roles and names beside it stand in for one: a list that moved up a row under look-alike buttons is refused). A changed, disabled or off-screen element is refused with nothing tapped, and "read the elements again". `sim_wait` waits for `text` or an `element` to show (or, with `gone`, to go), at most `timeout_ms`, and answers with the fresh outline either way. A tap AXe didn't confirm in time is reported as "outcome unknown" and not sent again. The outline and its refs are `crates/trek-mcp/src/outline.rs`, shared with computer use.

## Reach it as a user

Press ⌘J and choose **Simulator**. Pick or boot a device, install AXe if prompted, then use the mirrored screen and header actions. Drop an `.app`/`.ipa` or use the install and URL fields.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open tool:simulator
./.agents/skills/verify-trek/scripts/trek-dev screenshot simulator-panel
```

The Trek verification CLI opens the panel only. `send "mock:simtest"` has the mock agent run `xcodebuild test` in a shell: the Simulator tool comes forward (Settings › Tools › Show the simulator live) and shows each test's mark when the call returns. A live run comes from `trek-mcp simulator`'s `sim_test`, which writes its log to `<data>/sim-runs` (`TREK_SIM_RUNS`) as it goes; the tool tails the newest one. Device automation belongs to `trek-mcp` (`sim_list`, `sim_boot`, `sim_elements`, `sim_screenshot`, `sim_tap`, `sim_wait`, `sim_type`, and related tools) and may change simulator state. To check the element tools, run `trek-mcp simulator` over stdio against one booted simulator: `sim_launch` `com.apple.Preferences`, `sim_elements`, `sim_tap` a row by its ref, `sim_wait` for the next screen's text; then shut the simulator down.

## Code

`crates/trek-app/src/panels/simulator.rs`, `crates/trek-mcp/src/simulator.rs`, `crates/trek-mcp/src/outline.rs` (the outline and its refs), `crates/trek-ipc/src/simtest.rs` (the `xcodebuild test` parser).

## Verify

Device status matches `xcrun simctl`, the mirror updates after input, screenshot coordinates map to device points, install/launch errors are actionable, and shutting down releases polling work. Elements: the outline names what's on the screen with frames that match the screenshot, a tap by ref lands on that element, a ref to something that has gone or changed is refused with nothing tapped, and a wait answers when its text shows.

