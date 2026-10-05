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
the paired Mac), `-TrekLink '<trek://pair… link>'` (open the pairing confirmation for it).
`ios/scripts/screenshots.sh` rebuilds and captures the demo set in `ios/screenshots/`;
`ios/scripts/live-screenshots.sh '<pairing URL>'` pairs with a running demo host (debug-only
`-TrekPair`) and captures the live threads list and the "Encrypted · pinned" settings.

## Against a real server

Until Trek itself serves the protocol, run the fake host from `crates/trek-remote`. It serves
`wss://` with a self-signed certificate (kept beside its devices file, so the fingerprint survives
restarts) and prints the fingerprint and a pairing URL for the simulator:

```sh
cargo run -p trek-remote --example demo_host        # listens on 0.0.0.0:7420, code K7Q2-9XMV
#   Simulator     trek://pair?host=127.0.0.1:7420&code=K7Q2-9XMV&name=…&hid=…&fp=<64 hex>
#   TLS           wss://, fingerprint B138-0087 (…)
xcrun simctl openurl booted '<the Simulator URL it printed>'
```

The app asks **"Pair with Trek Demo Mac?"** with the short fingerprint first; links never pair on
their own. Or tap **Enter address and code** and type `127.0.0.1:7420`, `K7Q2-9XMV` and the short
fingerprint (`B138-0087`). On a phone, use the Mac's LAN IP, tailnet IP or MagicDNS (`*.ts.net`)
name. The device token and the full certificate fingerprint are kept in the Keychain, so the app
reconnects on its own, pinned, after that (Settings › Unpair to forget it).

How the pinning works, and what happens when the certificate changes or a Mac has none:
[`docs/MOBILE.md` › TLS and pinning](../docs/MOBILE.md#tls-and-pinning-phone-side). To try those
paths:

- **Wrong fingerprint:** change a character of `fp` in the URL. Pairing fails with "That Mac's
  fingerprint is …, not …" and the host logs only a failed TLS handshake (the code isn't sent).
- **Identity changed:** pair, stop the host, move `identity.der`/`identity.key` out of the devices
  file's folder, start it again and relaunch the app: "This Mac's identity changed" and no
  reconnects until you pair again.
- **Unencrypted Mac:** `TREK_REMOTE_PLAIN=1 cargo run -p trek-remote --example demo_host` prints a
  URL without `fp`; the app shows the "Unencrypted connection" warning and pairs over `ws://` only
  if you choose to. Settings then says **Unencrypted** in amber instead of **Encrypted · pinned**.
- **Allow for session:** needs Face ID or the passcode. In the simulator, enrol Face ID with
  Features › Face ID › Enrolled (or `xcrun simctl spawn booted notifyutil -s
  com.apple.BiometricKit.enrollmentChanged 1 && xcrun simctl spawn booted notifyutil -p
  com.apple.BiometricKit.enrollmentChanged`), then answer the prompt with `notifyutil -p
  com.apple.BiometricKit_Sim.pearl.match` (or `.nomatch`).

## Layout

```
Trek/App        TrekApp, RootView, tabs, the "New thread" accessory, the pairing-link sheet, launch arguments
Trek/Model      Protocol.swift (wire types, mirrors trek-remote/src/protocol.rs), AppModel (state + actions,
                paired Mac in the Keychain), CommandRisk (destructive-command warnings), DeviceOwner (Face ID)
Trek/Net        TrekClient: URLSessionWebSocketTask over pinned TLS (PinningDelegate), pair/hello,
                reconnect with backoff, pings; PairingLink
Trek/Demo       MockHost: the in-process demo Mac
Trek/Design     Trek palette (DESIGN.md), status colours, project hues, file-type chips, agent logos
Trek/Views      Threads, Thread (transcript grouping, tool rows, request cards, composer),
                New thread sheet, Pairing (QR scanner, typed pairing, confirmation sheet), Settings, Search
```

## Not done yet

QR scanning is wired (VisionKit) but only testable on a device. Push notifications, Live
Activities, diff view and attachments are phase 2, as is enforcing the "Allow for session" step-up
on the Mac (today the phone requires Face ID or the passcode before it sends it; the Mac takes the
phone's word). Typed pairing pins only an 8-hex-character prefix of the fingerprint (see the
limitation in `docs/MOBILE.md`); scanning pins the whole thing. See `docs/MOBILE.md`.

## On your iPhone

- **Over USB, free Apple ID:** `ios/scripts/install-device.sh` builds with your Personal Team and
  installs on the plugged-in iPhone. It runs for 7 days; run it again to renew.
- **TestFlight (keeps a build 90 days, updates over the air):** needs the paid Apple Developer
  Program on the Apple ID in Xcode. Once enrolled:
  1. Xcode › Settings › Accounts: select your Apple ID so Xcode picks up the paid team.
  2. App Store Connect › My Apps › + › New App: platform iOS, any free name (e.g. "Trek Agents"),
     bundle id `dev.trek.TrekMobile` (register it when asked), SKU anything.
  3. `ios/scripts/testflight.sh` archives, signs and uploads a build (numbered by date and time).
  4. In App Store Connect › TestFlight, add yourself as an internal tester; install the TestFlight
     app on the iPhone and accept the invite. Later builds arrive there.
