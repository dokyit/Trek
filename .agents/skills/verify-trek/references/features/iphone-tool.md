# iPhone tool

## What it does

Shows the user's real iPhone, plugged into the Mac by USB, beside the chat: its screen live over the cable, taps and keys from the user or an agent (`trek-mcp phone`), and apps and files over usbmuxd. No Xcode, Developer Mode or app on the phone.

Taps take one of two routes, decided once from what Trek knows:

- **Bluetooth**: the phone paired once with Trek as a Bluetooth keyboard and mouse (an iPhone on another Apple Account). AssistiveTouch is turned on while Trek taps and off after.
- **iPhone Mirroring**: an iPhone on the Mac's own Apple Account can't take Trek as a keyboard, so taps go through macOS's iPhone Mirroring window in the background. It connects only while the iPhone is locked; Trek opens it behind the user's windows and presses Connect. Trek takes this route once iPhone Mirroring has shown an iPhone (remembered in `<data>/phone-mirroring`, with the name it showed), or when Bluetooth can't work.

## Reach it as a user

Press ⌘J and choose **iPhone**. The header is one line (the device, its name, iOS and the link). Under it one strip: **Screen** (Live, Live in iPhone Mirroring, Locked, …) and **Taps** (Ready · Bluetooth, Ready · iPhone Mirroring, Lock the iPhone to tap, Pair once in Bluetooth, iPhone Mirroring can't reach it, …), with **AssistiveTouch** only on the Bluetooth route. A line under the strip appears only when the user must act now; the ⓘ button opens "How this works".

## Reach it with the CLI

An isolated run never touches the real phone: the tool starts with a made-up one, and `control phone <state>` puts it in each situation (this also opens the tool).

```sh
./.agents/skills/verify-trek/scripts/trek-dev control phone usb        # unlocked, Mirroring route: Lock the iPhone to tap
./.agents/skills/verify-trek/scripts/trek-dev control phone lock       # the same while someone wants taps: the action line
./.agents/skills/verify-trek/scripts/trek-dev control phone locked     # locked, iPhone Mirroring closed: Ready
./.agents/skills/verify-trek/scripts/trek-dev control phone mirroring  # iPhone Mirroring showing the phone
./.agents/skills/verify-trek/scripts/trek-dev control phone pair       # Bluetooth route, not paired yet
./.agents/skills/verify-trek/scripts/trek-dev control phone bluetooth  # paired: Ready · Bluetooth, AssistiveTouch chip
./.agents/skills/verify-trek/scripts/trek-dev control phone none       # no iPhone on the cable
./.agents/skills/verify-trek/scripts/trek-dev control phone how        # the "How this works" popover
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

## Code

`crates/trek-app/src/panels/phone.rs` (the tool, its agent calls), `crates/trek-phone/src/route.rs` (the route and the strip's words, shared with `phone_status`), `crates/trek-phone/src/mirror.rs` (iPhone Mirroring's states, read through Accessibility), `crates/trek-phone/helper/main.swift` (USB capture, Bluetooth, iPhone Mirroring input), `crates/trek-phone/src/device.rs` (lockdown, AssistiveTouch, apps, files), `crates/trek-ipc/src/phone.rs` (the tools).

## Verify

The strip names one route and never shows the Bluetooth pairing step when iPhone Mirroring is the route; `phone_status` says the same words; errors from `phone_tap`, `phone_swipe` and `phone_type` start with the one thing to do. A blank iPhone Mirroring window (it is blank for about a second between its own screens) isn't taken for the phone's screen. Unlocking the phone brings the picture back over USB by itself. AssistiveTouch is never switched on the Mirroring route.
