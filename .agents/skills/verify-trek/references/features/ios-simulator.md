# iOS Simulator

## What it does

Discovers Xcode simulators, boots/shuts them down, mirrors frames, sends touch/keyboard input through AXe, installs/launches apps, opens URLs, and attaches device screenshots to chat.

## Reach it as a user

Press ⌘J and choose **Simulator**. Pick or boot a device, install AXe if prompted, then use the mirrored screen and header actions. Drop an `.app`/`.ipa` or use the install and URL fields.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open tool:simulator
./.agents/skills/verify-trek/scripts/trek-dev screenshot simulator-panel
```

The Trek verification CLI opens the panel only. Device automation belongs to `trek-mcp` (`sim_list`, `sim_boot`, `sim_screenshot`, `sim_tap`, `sim_type`, and related tools) and may change simulator state.

## Code

`crates/trek-app/src/panels/simulator.rs`, `crates/trek-mcp/src/simulator.rs`.

## Verify

Device status matches `xcrun simctl`, the mirror updates after input, screenshot coordinates map to device points, install/launch errors are actionable, and shutting down releases polling work.

