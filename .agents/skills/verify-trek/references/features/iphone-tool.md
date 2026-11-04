# iPhone tool

## What it does

Shows the user's real iPhone, plugged into the Mac by USB (or over Wi-Fi with no cable, below), beside the chat: its screen live over the cable, taps and keys from the user or an agent (`trek-mcp phone`), and apps and files over usbmuxd. The screen, apps and files need no Xcode, Developer Mode or app on the phone; the Trek Driver route below needs all three (it's for a Mac that builds Trek's iPhone app).

Taps take one of three routes, decided from what Trek knows (`Facts::route`, `crates/trek-phone/src/route.rs`):

- **USB (Trek Driver)**: while the iPhone is unlocked on the cable and Trek's iPhone app was built and installed from this Mac with `ios/scripts/install-device.sh` (which now builds Trek Driver into it). The driver (`ios/TrekDriver`) is a small XCUITest that serves HTTP on the phone's loopback; Trek starts it on the first tap with `xcodebuild test-without-building` on the `.xctestrun` the script left in `~/Library/Application Support/dev.trek.phone-update/driver/` (about 7 s warm; longer the first time, when xcodebuild installs the app), reaches its port 8117 through its own usbmuxd client over the cable (over Wi-Fi straight over TCP at the phone's address, which usbmuxd doesn't carry; the driver then takes only this Mac's connections, and an answer counts as whole by its Content-Length so a late close over Wi-Fi doesn't cut it short), and stops it at `phone_release`, when the tool closes, or after 10 idle minutes (the driver also stops itself after 15). Each start gets a fresh token the driver checks. Taps are ~0.3 s, a swipe its length plus ~0.4 s. The driver lives inside Trek's iPhone app rather than its own runner app because a free Apple ID installs at most three apps on a phone (the test bundle goes into `Trek.app/PlugIns`, the app gets the `continuous` background mode, both re-signed; `ios/scripts/build-driver.sh`). A locked phone goes to iPhone Mirroring.

- **Wi-Fi (Trek Driver, no cable)**: the same driver route when usbmuxd sees the phone over the network only. The picture comes from the driver too (nothing comes over a cable), apps and files go over Wi-Fi through usbmuxd's network entry, and the header says Wi-Fi. What it needs, once:
  1. The iPhone plugged in and trusted once (Trust This Computer), so the Mac holds its pairing record.
  2. **Show this iPhone when on Wi-Fi**: in Finder, with the iPhone plugged in, select it in the sidebar, General › Options › Show this iPhone when on Wi-Fi, Apply (stored on the phone as lockdown's `EnableWifiConnections`). usbmuxd then lists it on the network whenever both are on the same Wi-Fi.
  3. **Xcode reaches it over the network**: paired with Xcode by cable once (Developer Mode on, as for the USB route). Xcode 15 and later then reach it over Wi-Fi by themselves; Window › Devices and Simulators shows it with a network icon when unplugged (tick **Connect via network** there if your Xcode shows that box). Check: `xcrun xcdevice list` says `"interface" : "network"` and `xcrun devicectl list devices` says `available (paired)`.
  4. Trek Driver built from this source or later (`ios/scripts/install-device.sh`, or the phone updater's next install): `build-driver.sh` writes `wifi` in the driver's `features`; an older driver says "Install the driver" over Wi-Fi and keeps working on the cable.
  5. The iPhone unlocked. Trek asks CoreDevice every 8 s while the tool is open (`xcrun devicectl device info lockState`, ~1.5 s): `passcodeRequired` true (locked) sends the route to iPhone Mirroring (or Bluetooth), as on the cable. A screen that just went dark within the passcode's grace period counts as unlocked.

  Over Wi-Fi usbmuxd connects to the phone's network address, not its loopback, so Trek starts the driver with `TREK_DRIVER_PEERS` = the Mac's own address toward the phone (`device::local_address_toward`): the driver then listens on the local network too (never cellular, local addresses only) and answers only that address, with the token; anything else gets a 403 naming the address it came from. A driver started over the cable serves the loopback only, so unplugging stops it and the next frame or tap starts it again for Wi-Fi; one started over Wi-Fi keeps answering when the cable comes back. The cable is preferred whenever it's plugged in. Trek's iPhone app already asks for the local network (`NSLocalNetworkUsageDescription`); if the iPhone asks, allow it. Starting over Wi-Fi is slower than over the cable, and so are the frames.

- **Bluetooth**: the phone paired once with Trek as a Bluetooth keyboard and mouse (an iPhone on another Apple Account). AssistiveTouch is turned on while Trek taps and off after.
- **iPhone Mirroring**: an iPhone on the Mac's own Apple Account can't take Trek as a keyboard, so taps go through macOS's iPhone Mirroring window in the background. It connects only while the iPhone is locked; Trek opens it behind the user's windows and presses Connect. Trek takes this route once iPhone Mirroring has shown an iPhone (remembered in `<data>/phone-mirroring`, with the name it showed), or when Bluetooth can't work.

## Reach it as a user

Press ⌘J and choose **iPhone**. The header is one line (the device, its name, iOS and the link). Under it one strip: **Screen** (Live, Live · Wi-Fi, Live in iPhone Mirroring, Locked, None over Wi-Fi, …) and **Taps** (Ready · USB, Ready · Wi-Fi, Starting the driver…, Install the driver (ios/scripts/install-device.sh), The driver didn't start, Ready · Bluetooth, Ready · iPhone Mirroring, Lock the iPhone to tap, Unlock the iPhone to tap, Pair once in Bluetooth, iPhone Mirroring can't reach it, …), with **AssistiveTouch** only on the Bluetooth route. A line under the strip appears only when the user must act now; the ⓘ button opens "How this works" (its "Over Wi-Fi" paragraph comes second while there's no cable, last otherwise).

Over Wi-Fi, locked: when iPhone Mirroring is ready it takes the taps with nothing to do; when it can't (it timed out, isn't on this Mac, or Bluetooth isn't paired) the line says "Unlock the iPhone to let Trek tap it over Wi-Fi." Unlocked with a driver that can't run (not installed, or Xcode can't reach the phone) and iPhone Mirroring known here, the line adds "or lock the iPhone to tap through iPhone Mirroring".

While the picture comes over the cable, iOS draws 9:41 and full bars in its status bar, and the Calendar icon's demo date, on the phone itself too (as for QuickTime). So on the USB route the panel takes its picture from Trek Driver instead whenever the driver can run (the phone unlocked on the cable, the driver installed, the panel on screen): `manage_driver_frames` starts the driver if need be and asks it for a JPEG about three times a second (`GET /screenshot?w=900&q=70`); the first frame switches the picture over and turns the cable's capture off, and the phone shows its real clock again within a few seconds of the tool opening. An agent's `phone_screenshot` comes from the driver too then (a PNG scaled to `max_edge`). When the driver stops (the phone locked or unplugged, the panel hidden), the cable's capture takes over again. Over the cable the panel draws the phone's real time (its time zone and 12/24-hour setting, read from lockdown) over the clock, in the status bar's own colors, only when a frame shows a status bar (not over a full-screen app or iPhone Mirroring's picture) and never below it.

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
./.agents/skills/verify-trek/scripts/trek-dev control phone driver-live      # the driver's picture: Screen Live, the clock real
./.agents/skills/verify-trek/scripts/trek-dev control phone wifi             # no cable: header Wi-Fi, Screen Live · Wi-Fi, Taps Ready · Wi-Fi
./.agents/skills/verify-trek/scripts/trek-dev control phone wifi-starting    # no cable, the driver starting: Screen Starting…, Taps Starting the driver…
./.agents/skills/verify-trek/scripts/trek-dev control phone wifi-locked      # no cable, locked, iPhone Mirroring can't reach it: Unlock the iPhone to let Trek tap it over Wi-Fi.
./.agents/skills/verify-trek/scripts/trek-dev control phone wifi-how         # the popover with no cable: "Over Wi-Fi" second
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

The driver by hand (what Trek does): `ios/scripts/install-device.sh` (or `ios/scripts/build-driver.sh <signed Trek.app> <dir>`), then `xcodebuild test-without-building -xctestrun <copy with TREK_DRIVER=1, TREK_DRIVER_PORT, TREK_DRIVER_TOKEN> -destination id=<udid> -only-testing:TrekDriver/Drive` and HTTP to port 8117 over usbmuxd: `GET /status` (screen in points), `POST /tap {x,y,hold_ms}`, `/swipe {x1,y1,x2,y2,duration_ms}`, `/type {text}`, `/key {key: home|lock|volume_up|volume_down|spotlight|enter|backspace|…}`, `/open {bundle}`, `GET /screenshot` (`?w=<max width in pixels>&q=<JPEG quality>`; PNG without `q`), `POST /stop`, each with `X-Trek-Token`. Its HTTP parsing has its own tests: `xcodebuild test -project ios/TrekDriver/TrekDriver.xcodeproj -scheme TrekDriver -destination 'platform=iOS Simulator,name=iPhone 17'`.

Which iPhones usbmuxd sees and how (USB, or Wi-Fi with its address and the Mac's there), read only:

```sh
cargo run -p trek-phone --example devices
```

Why iPhone Mirroring can't reach the phone, from the Mac's side (whether Apple offers it where the Mac is, a management profile, Wi-Fi, Bluetooth, Handoff, Internet Sharing, firewall, VPN, the phone on the Mac's network, and how its last Connect ended in its log: not found, dropped, not set up, or turned down for the iPhone's region), with the one blocker last and the advice for an agent under it (how it all works and what the logs say: `docs/IPHONE-MIRRORING.md`):

```sh
cargo run -p trek-phone --example mirror -- diagnose
```

The tool runs the same checks when iPhone Mirroring times out with the phone locked, and its action line names that blocker with **Check again**.

## Code

`crates/trek-app/src/panels/phone.rs` (the tool, its agent calls, the status bar clock), `crates/trek-phone/src/route.rs` (the route and the strip's words, shared with `phone_status`), `crates/trek-phone/src/driver.rs` (starting and talking to Trek Driver, tools onto its endpoints, pixels to points), `ios/TrekDriver/` (the driver; `project.yml` for xcodegen), `ios/scripts/build-driver.sh` (puts it in Trek.app), `crates/trek-phone/src/mirror.rs` (iPhone Mirroring's states, read through Accessibility; Trek's Connect retries), `crates/trek-phone/src/diagnose.rs` (why it can't reach the phone), `crates/trek-phone/helper/main.swift` (USB capture, Bluetooth, iPhone Mirroring input), `crates/trek-phone/src/device.rs` (lockdown, AssistiveTouch, apps, files, the phone's clock, a TCP port on the phone, the link: USB or Wi-Fi), `crates/trek-ipc/src/phone.rs` (the tools).

## Verify

The strip names one route and never shows the Bluetooth pairing step when iPhone Mirroring is the route; an unlocked phone with the driver taps over USB and a locked one goes to iPhone Mirroring; Trek's iPhone app on the phone is never replaced by an older driver build (a build mismatch says to run install-device.sh again); `phone_status` says the same words; errors from `phone_tap`, `phone_swipe` and `phone_type` start with the one thing to do. A blank iPhone Mirroring window (it is blank for about a second between its own screens) isn't taken for the phone's screen. Unlocking the phone brings the picture back over USB by itself. AssistiveTouch is never switched on the Mirroring route. With no cable and the driver installed, an unlocked phone taps and shows its picture over Wi-Fi through the driver (the header says Wi-Fi), a locked one goes to iPhone Mirroring, plugging the cable in moves the route back to USB, and the screen never shows a stale picture as live over Wi-Fi.
