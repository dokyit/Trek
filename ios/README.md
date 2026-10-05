# Trek for iPhone

The companion app for Trek on the Mac: see every thread and its status, read transcripts, answer
approvals, questions and plans, steer or queue follow-ups, stop a turn and start new threads while
the agents keep running on your Mac. Design and wire protocol: [`docs/MOBILE.md`](../docs/MOBILE.md).

SwiftUI, iOS 26 (Liquid Glass), bundle id `dev.trek.TrekMobile`, no third-party dependencies.

## Build and run

Needs Xcode 26 or later (built with Xcode 27).

```sh
open ios/Trek.xcodeproj            # then Run on an iPhone simulator
```

or from the command line:

```sh
cd ios
xcodebuild -project Trek.xcodeproj -scheme Trek \
  -destination 'platform=iOS Simulator,name=iPhone 18 Pro' -derivedDataPath /tmp/trek-ios-dd build
xcrun simctl boot "iPhone 18 Pro"; open -a Simulator
xcrun simctl install booted /tmp/trek-ios-dd/Build/Products/Debug-iphonesimulator/Trek.app
xcrun simctl launch booted dev.trek.TrekMobile -TrekDemo YES
```

The project uses a file-system-synchronized group: new files under `ios/Trek/` are part of the
target without touching `project.pbxproj`. For a device build, set your team in Signing &
Capabilities.

## Demo mode

Tap **Explore the demo** on the pairing screen (or launch with `-TrekDemo YES`). `MockHost` plays a
Mac with 13 realistic threads across four projects and five agents: pending approval, question and
plan, a failed build, threads that keep working, and replies to whatever you send. It answers the
same protocol messages a Mac does, so everything in the app works without one.

Launch arguments for screenshots: `-TrekOpen <thread id>` (e.g. `t-flaky`, `t-inbox`,
`t-ratelimit`, `t-settings`), `-TrekTab settings|search`, `-TrekSheet new`,
`-TrekAppearance light|dark`, `-TrekExpand YES` (open all tool groups), `-TrekReset YES` (forget
the paired Mac). `ios/scripts/screenshots.sh` rebuilds and captures the set in `ios/screenshots/`.

## Against a real server

Until Trek itself serves the protocol, run the fake host from `crates/trek-remote`:

```sh
cargo run -p trek-remote --example demo_host        # listens on 0.0.0.0:7420, code K7Q2-9XMV
xcrun simctl openurl booted "trek://pair?host=127.0.0.1:7420&code=K7Q2-9XMV"
```

or tap **Enter address and code** and type `127.0.0.1:7420` and `K7Q2-9XMV`. On a phone, use the
Mac's LAN or Tailscale address. The pairing token is kept in the Keychain, so the app reconnects on
its own after that (Settings › Unpair to forget it).

## Layout

```
Trek/App        TrekApp, RootView, tabs, the "New session" accessory, launch arguments
Trek/Model      Protocol.swift (wire types, mirrors trek-remote/src/protocol.rs), AppModel (state + actions)
Trek/Net        TrekClient: URLSessionWebSocketTask, pair/hello, reconnect with backoff, pings
Trek/Demo       MockHost: the in-process demo Mac
Trek/Design     Trek palette (DESIGN.md), status colours, project hues, file-type chips, agent logos
Trek/Views      Sessions, Thread (transcript grouping, tool rows, request cards, composer),
                New session sheet, Pairing (QR scanner), Settings, Search
```

## Not done yet

QR scanning is wired (VisionKit) but only testable on a device; push notifications, Live
Activities, diff view, attachments, Face ID step-up enforcement and `wss://` pinning are phase 1b/2
(see `docs/MOBILE.md`).
