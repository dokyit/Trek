# iPhone tool

## What it does

Shows the user's real iPhone, plugged into the Mac by USB, beside the chat: its screen live over the cable, taps and keys from the user or an agent (`trek-mcp phone`), and apps and files over usbmuxd. The screen, apps and files need no Xcode, Developer Mode or app on the phone; the USB tap route below needs all three (it's for a Mac that builds Trek's iPhone app).

Taps take one of three routes, decided from what Trek knows:

- **USB (Trek Driver)**: while the iPhone is unlocked on the cable and Trek's iPhone app was built and installed from this Mac with `ios/scripts/install-device.sh` (which now builds Trek Driver into it). The driver (`ios/TrekDriver`) is a small XCUITest that serves HTTP on the phone's loopback; Trek starts it on the first tap with `xcodebuild test-without-building` on the `.xctestrun` the script left in `~/Library/Application Support/dev.trek.phone-update/driver/` (about 7 s warm; longer the first time, when xcodebuild installs the app), reaches its port 8117 through its own usbmuxd client, and stops it at `phone_release`, when the tool closes, or after 10 idle minutes (the driver also stops itself after 15). Each start gets a fresh token the driver checks. Taps are ~0.3 s, a swipe its length plus ~0.4 s. The driver lives inside Trek's iPhone app rather than its own runner app because a free Apple ID installs at most three apps on a phone (the test bundle goes into `Trek.app/PlugIns`, the app gets the `continuous` background mode, both re-signed; `ios/scripts/build-driver.sh`). A locked phone goes to iPhone Mirroring.

- **Bluetooth**: the phone paired once with Trek as a Bluetooth keyboard and mouse (an iPhone on another Apple Account). AssistiveTouch is turned on while Trek taps and off after.
- **iPhone Mirroring**: an iPhone on the Mac's own Apple Account can't take Trek as a keyboard, so taps go through macOS's iPhone Mirroring window in the background. It connects only while the iPhone is locked; Trek opens it behind the user's windows and presses Connect. Trek takes this route once iPhone Mirroring has shown an iPhone (remembered in `<data>/phone-mirroring`, with the name it showed), or when Bluetooth can't work.

## Reach it as a user

Press ⌘J and choose **iPhone**. The header is one line (the device, its name, iOS and the link). Under it one strip: **Screen** (Live, Live in iPhone Mirroring, Locked, …) and **Taps** (Ready · USB, Starting the driver…, Install the driver (ios/scripts/install-device.sh), The driver didn't start, Ready · Bluetooth, Ready · iPhone Mirroring, Lock the iPhone to tap, Pair once in Bluetooth, iPhone Mirroring can't reach it, …), with **AssistiveTouch** only on the Bluetooth route. A line under the strip appears only when the user must act now; the ⓘ button opens "How this works".

While the picture comes over the cable, iOS draws 9:41 and full bars in its status bar (as for QuickTime). The panel draws the phone's real time (its time zone and 12/24-hour setting, read from lockdown) over the clock, in the status bar's own colors, only when a frame shows a status bar (not over a full-screen app or iPhone Mirroring's picture) and never below it.

## Reach it with the CLI

An isolated run never touches the real phone: the tool starts with a made-up one, and `control phone <state>` puts it in each situation (this also opens the tool).

```sh
./.agents/skills/verify-trek/scripts/trek-dev control phone usb        # unlocked, Mirroring route: Lock the iPhone to tap
./.agents/skills/verify-trek/scripts/trek-dev control phone lock       # the same while someone wants taps: the action line
./.agents/skills/verify-trek/scripts/trek-dev control phone locked     # locked, iPhone Mirroring closed: Ready
./.agents/skills/verify-trek/scripts/trek-dev control phone unreachable  # locked, timed out, diagnosed: the blocker and Check again
./.agents/skills/verify-trek/scripts/trek-dev control phone mirroring  # iPhone Mirroring showing the phone
./.agents/skills/verify-trek/scripts/trek-dev control phone pair       # Bluetooth route, not paired yet
./.agents/skills/verify-trek/scripts/trek-dev control phone bluetooth  # paired: Ready · Bluetooth, AssistiveTouch chip
./.agents/skills/verify-trek/scripts/trek-dev control phone none       # no iPhone on the cable
./.agents/skills/verify-trek/scripts/trek-dev control phone how        # the "How this works" popover
./.agents/skills/verify-trek/scripts/trek-dev control phone driver           # unlocked, driver installed: Ready · USB
./.agents/skills/verify-trek/scripts/trek-dev control phone driver-starting  # Starting the driver…
./.agents/skills/verify-trek/scripts/trek-dev control phone driver-install   # Install the driver (ios/scripts/install-device.sh), with its action line
./.agents/skills/verify-trek/scripts/trek-dev control phone driver-how       # the popover on the USB route
./.agents/skills/verify-trek/scripts/trek-dev screenshot iphone-tool
```

A live check on the real phone (it starts a second helper beside the user's Trek's; switch back to a made-up state when done):

```sh
./.agents/skills/verify-trek/scripts/trek-dev control phone real
./.agents/skills/verify-trek/scripts/trek-dev control phone 'call phone_status {}'
cat "$(./.agents/skills/verify-trek/scripts/trek-dev status | jq -r .runtime_dir)/shot/phone-reply.json"
./.agents/skills/verify-trek/scripts/trek-dev control phone 'call phone_release {}'
./.agents/skills/verify-trek/scripts/trek-dev control phone usb
```

`call <method> <json>` runs any `phone_*` tool as an agent on full access would; the answer lands in `<runtime>/shot/phone-reply.json`.

The driver by hand (what Trek does): `ios/scripts/install-device.sh` (or `ios/scripts/build-driver.sh <signed Trek.app> <dir>`), then `xcodebuild test-without-building -xctestrun <copy with TREK_DRIVER=1, TREK_DRIVER_PORT, TREK_DRIVER_TOKEN> -destination id=<udid> -only-testing:TrekDriver/Drive` and HTTP to port 8117 over usbmuxd: `GET /status` (screen in points), `POST /tap {x,y,hold_ms}`, `/swipe {x1,y1,x2,y2,duration_ms}`, `/type {text}`, `/key {key: home|lock|volume_up|volume_down|spotlight|enter|backspace|…}`, `/open {bundle}`, `GET /screenshot`, `POST /stop`, each with `X-Trek-Token`. Its HTTP parsing has its own tests: `xcodebuild test -project ios/TrekDriver/TrekDriver.xcodeproj -scheme TrekDriver -destination 'platform=iOS Simulator,name=iPhone 17'`.

Why iPhone Mirroring can't reach the phone, from the Mac's side (Wi-Fi, Bluetooth, Internet Sharing, firewall, VPN, the phone on the Mac's network, and how its last Connect ended in its log), with the one blocker last:

```sh
cargo run -p trek-phone --example mirror -- diagnose
```

The tool runs the same checks when iPhone Mirroring times out with the phone locked, and its action line names that blocker with **Check again**.

## Code

`crates/trek-app/src/panels/phone.rs` (the tool, its agent calls, the status bar clock), `crates/trek-phone/src/route.rs` (the route and the strip's words, shared with `phone_status`), `crates/trek-phone/src/driver.rs` (starting and talking to Trek Driver, tools onto its endpoints, pixels to points), `ios/TrekDriver/` (the driver; `project.yml` for xcodegen), `ios/scripts/build-driver.sh` (puts it in Trek.app), `crates/trek-phone/src/mirror.rs` (iPhone Mirroring's states, read through Accessibility; Trek's Connect retries), `crates/trek-phone/src/diagnose.rs` (why it can't reach the phone), `crates/trek-phone/helper/main.swift` (USB capture, Bluetooth, iPhone Mirroring input), `crates/trek-phone/src/device.rs` (lockdown, AssistiveTouch, apps, files, the phone's clock, a TCP port on the phone), `crates/trek-ipc/src/phone.rs` (the tools).

## Verify

The strip names one route and never shows the Bluetooth pairing step when iPhone Mirroring is the route; an unlocked phone with the driver taps over USB and a locked one goes to iPhone Mirroring; Trek's iPhone app on the phone is never replaced by an older driver build (a build mismatch says to run install-device.sh again); `phone_status` says the same words; errors from `phone_tap`, `phone_swipe` and `phone_type` start with the one thing to do. A blank iPhone Mirroring window (it is blank for about a second between its own screens) isn't taken for the phone's screen. Unlocking the phone brings the picture back over USB by itself. AssistiveTouch is never switched on the Mirroring route.
