# iPhone Mirroring

How macOS's iPhone Mirroring works end to end, what it needs on both devices, how Trek's iPhone
tool drives it, and how to read why a Connect fails. The code is `crates/trek-phone/src/mirror.rs`
(its states, read through Accessibility), `crates/trek-phone/src/diagnose.rs` (why it can't reach
the phone), `crates/trek-phone/src/route.rs` (the words) and the `mirror` commands of
`crates/trek-phone/helper/main.swift` (input). This is the third tap route, after USB through
Trek Driver and Bluetooth; it's the one an iPhone on the Mac's own Apple Account falls back to
while it's locked.

Facts marked *unconfirmed* come from one unofficial source or from reading logs, not from Apple.

## What it needs

- A Mac with Apple silicon or the T2 chip on macOS Sequoia 15 or later, and an iPhone on iOS 18
  or later with a passcode. [1]
- Both signed in to the same Apple Account with two-factor authentication. [1]
- Wi-Fi and Bluetooth on, on both. Apple doesn't say they must share a network; a working session
  in one forum log ran over AWDL, Apple's peer-to-peer Wi-Fi (*unconfirmed*). [1] [3]
- Handoff on, on both. Apple's Mac User Guide lists it when iPhone Mirroring won't connect; the
  support article doesn't. On the iPhone it's in Settings › General › AirPlay & Continuity
  (AirPlay & Handoff before iOS 26); on the Mac in System Settings › General › AirDrop &
  Continuity (Desktop & Dock before macOS 26.4). [2] [1]
- The iPhone locked and near the Mac (within about 10 m); it may be charging or in StandBy.
  Using the iPhone ends the session. [1] [2] [4]
- The Mac not sharing its internet connection and not using AirPlay or Sidecar. [1]
- One Mac and one iPhone at a time. [1]
- No VPN or security software blocking local connections, and the Mac's firewall not blocking all
  incoming connections. [1] [2]
- Not in the European Union: Apple's support page still says it's unavailable there (citing the
  Digital Markets Act), as of macOS 27. [1] [5] The Mac decides with `eligibilityd`: the
  domain `OS_ELIGIBILITY_DOMAIN_IRON` in `/private/var/db/os_eligibility/eligibility.plist`, from
  the Apple Account's billing country and the Mac's location; answer 4 is eligible, 2 is not.
  Changing only the Mac's region setting doesn't change it. Since iOS 26 the iPhone runs its own
  check during setup and refuses with `SFAuthenticationErrorCodeRegionLocked` (Code=35) in the
  Mac's log (*unconfirmed*). [6] [7]
- No management profile that sets `allowiPhoneMirroring` to false, on either device (on the iPhone
  it needs a supervised device). [8] [9] No source says Screen Time blocks it.
- On the iPhone, Settings › General › AirPlay & Continuity › iPhone Mirroring lists each Mac it
  mirrors to, with its sessions; Edit removes a Mac. There is no on/off switch: removing the Mac
  (or a profile) is the only way to turn it off. [4] On the Mac, iPhone Mirroring › Settings has
  "Ask every time", "Authenticate automatically" and **Revoke iPhone Access**, which resets the
  setup for that iPhone. [1]

The first setup: lock the iPhone, open iPhone Mirroring, click Continue and enter the iPhone's
passcode on the iPhone; the iPhone may ask to trust the Mac. [2] Apple's own fixes, in order:
updates and a restart of both, stop heavy Bluetooth or Wi-Fi use, check VPN and security
software, Revoke iPhone Access and set it up again, and last, sign out of the Apple Account on
both and back in. [1] A community fix for a time-out deletes `onenessPairedDeviceID` from
`com.apple.ScreenContinuity` and sets it up again (macOS 15; the key isn't there on macOS 27.2). [16]

## How it connects

The app is `/System/Applications/iPhone Mirroring.app` (bundle `com.apple.ScreenContinuity`, built
on ScreenSharingKit and ScreenContinuityUI). [10] On a Connect it logs numbered checks under
`com.apple.screensharing:ScreenContinuityApp`, seen on macOS 27.2:

1. Mac supported, eligible, and not restricted by a profile (1.1 to 1.3).
2. iCloud signed in and healthy; Wi-Fi on; the continuity feature on (2 to 4).
3. Replicator has a paired device (5). `replicatord` keeps the pairing that iPhone widgets,
   notifications and iPhone Mirroring share; its IDS view says `isCloudPaired: true` for the
   phone when it's right. A Mac with none ends here with `noCompatiblePhone`. [3]
4. The phone's software version, screen size, remote-unlock pairing, the Mac's passcode or Touch
   ID, and whether the phone was onboarded (6 to 11.1), then Bluetooth on (13) and local
   authentication (14, 15).
5. "18.1 MirroringSession activate is in flight": MediaContinuityKit creates an endpoint and asks
   `rapportd` to browse for the application service `com.apple.MediaContinuityKit.iPhoneMirroring`
   on device type iPhone. The browse is Bluetooth LE only (`controlFlags < BLE >`) with an RSSI
   threshold of -70. If no endpoint turns up in 10 seconds, the app logs "Failed to find endpoint
   for serviceName", `MediaContinuityKit.TaskTimeoutError`, and tears the session down with
   `bringupTimedOut(during: sessionInitializing)`; the window says it timed out.

`rapportd` is the Rapport (Companion Link) daemon: it finds the account's devices over Bluetooth
LE ("BLE device found … IDS …, DF … , AcLv …"), over Bonjour as `_companion-link._tcp` (TXT keys
`rpBA`, `rpVr`, `rpAD`, …) and through IDS, and resolves the application services each device
offers. [11] `sharingd` handles the same-account authentication (`com.apple.sharing:Authentications`,
"Unlock enabled for device …"). A phone's BLE advertisement carries its activity level (`AcLv`:
Idle 3 is screen off, Screen 7 and User 11 are in use) and flags (`DF`); a working session in one
forum log showed `DF 0xA9 < MyMe MyiCloud Ranging AirDrop >`, a failing one `DF 0x30 < PeerMe
Ranging >` (*unconfirmed*: Apple doesn't document them). [3] Which process carries the video once
connected isn't documented.

## How input reaches the phone

The Mac only gets video: a click is a tap, a trackpad drag or scroll swipes, the keyboard types,
and the title bar has Home Screen and App Switcher. [2] There is no accessibility tree of the
phone's screen; the Mirroring window shows as `AXMainWindow` but not in `AXWindows`, and its own
overlays (Connect, Try Again, the time-out text) are the only AX content. [12] Trek reads those
overlays through Accessibility to know the state and press Connect, and keeps the window behind
the user's. Input goes to that window without bringing it forward: the helper makes it key inside
its app with `SLPSPostEventRecordTo` records, then posts mouse and key events to its process with
`SLEventPostToPid`, stamping the window with `CGEventSetWindowLocation`, the same way
`crates/trek-mcp/src/desktop/skylight.rs` drives background windows. [13] [14] Keys reach the
phone only while the window is key; a key event with Unicode text and keycode 0 is dropped, so
text goes as real keycodes. [13] [15] One project found events posted to the process ignored and
used the HID event tap with the window in front instead, so this depends on the macOS version.
[12]

## Reading why it fails

`cargo run -p trek-phone --example mirror -- diagnose` (and the iPhone tool, when a Connect times
out with the phone locked) checks, in order: whether Apple offers it where the Mac is
(`plutil -extract OS_ELIGIBILITY_DOMAIN_IRON.os_eligibility_answer_t raw` on the eligibility plist),
a management profile (`allowiPhoneMirroring` in `/Library/Managed Preferences`), the Mac's Wi-Fi,
Bluetooth and Handoff (`defaults -currentHost read com.apple.coreservices.useractivityd`
`ActivityAdvertisingAllowed` and `ActivityReceivingAllowed`; 0 is off), Internet Sharing, the Mac
on a network, the phone locked and on the Mac's network, the firewall, and then how the last
Connect ended in the app's log: `noCompatiblePhone` (not set up), `RegionLocked` (the iPhone's
region), "Failed to find endpoint" (the phone isn't offering it) or a drop (a network one, a VPN
named if one is on). What it can't see from the Mac: the iPhone's Handoff, its list of Macs, its
region, a profile on it, and Bluetooth range; those come out as "isn't offering".

To read the logs by hand (they're large; narrow by process and time):

```sh
log show --last 1d --style compact --predicate 'process == "iPhone Mirroring" AND subsystem BEGINSWITH "com.apple.screensharing"'
log show --start "<connect time>" --end "<10 s later>" --style compact --predicate 'process == "rapportd"' | grep -E "MediaContinuityKit|BLE device found"
log show --last 1d --style compact --predicate 'process == "rapportd" AND eventMessage CONTAINS "0 -> app_svc: com.apple.MediaContinuityKit"'
```

The last one prints a line for each time `rapportd` found the phone's iPhone Mirroring endpoint;
none means the phone never offered it.

### The case on the developer's Mac (2026-10-31 to 11-02)

An iPhone 16 Pro Max on iOS 27.2 and a MacBook Air on macOS 27.2, same Apple Account, in the US.
The Mac's eligibility answer was 4 (billing country US), no profile, Handoff at its default (on),
Wi-Fi and Bluetooth on, no Internet Sharing. iPhone Mirroring had last connected on 2026-10-27
(`previousSessionDate` in `com.apple.ScreenContinuity`). The log, which reaches back to 10-31,
holds 24 Connects (from 10-31 22:13 to 11-02 20:52), and every one ended the same way:

- At 11-02 20:52:15 the app passed every check through "11.1 Device has previously onboarded",
  with Replicator listing "tobyj's iPhone, paired, iPhone 16 Pro Max" and sharingd "Unlock enabled
  for device … iPhone17,2". `rapportd` started the BLE browse for
  `com.apple.MediaContinuityKit.iPhoneMirroring` with RSSI threshold -70; the only same-account
  BLE devices it saw in those 10 seconds were unresolved, at RSSI -88 (`DF 0x30 < PeerMe Ranging >`,
  `DF 0x90 < PeerMe AirDrop >`). At 20:52:25 it stopped: "Failed to find endpoint".
- At 11-01 07:42:30 `rapportd` did resolve the phone over BLE (its identity, with a Hotspot field
  only an iPhone has) at RSSI -58, idle with the screen off, `DF 0x30 < PeerMe Ranging >`; the
  Connect at 07:42:50 still found no endpoint.
- Across the three days `rapportd` found the same phone's `com.apple.siri.companionlink` service
  1,039 times and its widgets service (`com.apple.ApplicationService.chrono`) 75 times, over
  Wi-Fi at 192.168.1.159 with iOS 27.2.0; its iPhone Mirroring service, not once.
- None of the known failure lines were there: no `noCompatiblePhone`, no `RegionLocked`, no
  "does not have expected status flags", and `isCloudPaired: true` for the phone.

So the Mac and the pairing were right and the phone was in range, but the phone wasn't offering
iPhone Mirroring. That is the phone's side: its Handoff, the Mac's entry in its iPhone Mirroring
list, or its state after the iOS 27.2 update. The phone never advertised the `MyiCloud` flag over
BLE in those days (the iPad on the same account did), which fits but isn't proof. Trek's action
line for this case says to check Handoff and the Mac's entry under iPhone Mirroring on the
iPhone, restart it and lock it; then Revoke iPhone Access on the Mac and set it up again.

## Sources

1. Apple, "Use iPhone Mirroring to interact with your iPhone from your Mac": https://support.apple.com/en-us/120421
2. Apple, Mac User Guide, "Control your iPhone from your Mac": https://support.apple.com/guide/mac-help/control-your-iphone-from-your-mac-mchl444d53a6/mac
3. Apple Developer Forums, iPhone Mirroring `noCompatiblePhone` with rapportd and replicatord logs: https://developer.apple.com/forums/thread/844438 and https://developer.apple.com/forums/thread/782917
4. Apple, Personal Safety User Guide, iPhone Mirroring: https://support.apple.com/guide/personal-safety/ips70daa1bcf/web
5. MacRumors, "Apple on iPhone Mirroring still not in the EU": https://www.macrumors.com/2025/06/18/apple-on-iphone-mirroring-still-not-in-eu/
6. iphone-mirroring-bypass (eligibility domain and the iPhone's region check): https://github.com/nxm/iphone-mirroring-bypass
7. Apple Developer Forums, iPhone Mirroring eligibility: https://developer.apple.com/forums/thread/758238
8. Jamf, "Managing iPhone Mirroring with Jamf Pro": https://support.jamf.com/en/articles/11024638-managing-iphone-mirroring-with-jamf-pro
9. Apple Platform Deployment, restrictions: https://support.apple.com/guide/deployment/dep0f7dd3d8
10. iPhone Mirroring app notes (bundle, frameworks): https://gist.github.com/powext/404171ac080a4d6ee2b98253f4ca863c
11. pyatv, Companion Link protocol (`_companion-link._tcp`, its TXT keys, pairing): https://pyatv.dev/documentation/protocols/
12. iphone-mirror-mcp, architecture notes (AX tree, event posting): https://github.com/nickatnight96/iphone-mirror-mcp/blob/main/docs/architecture.md
13. cua, background keys to iPhone Mirroring: https://github.com/trycua/cua/issues/4919
14. chords for iPhone Mirroring (background clicks with `CGEventSetWindowLocation`): https://github.com/KeyChord/chords-com.apple.ScreenContinuity
15. oh-my-openagent, typing into iPhone Mirroring: https://github.com/code-yeongyu/oh-my-openagent/pull/11749
16. The Bit Guru, "iPhone Mirroring not working" (`onenessPairedDeviceID`): https://thebitguru.com/blog/1758-iphone-mirroring-not-working
