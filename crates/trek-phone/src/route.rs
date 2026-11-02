//! Which way taps and keys reach the phone, decided once from what Trek knows, and what the
//! iPhone tool's strip and `phone_status` say about it: the same words in both places.
//!
//! - **USB** while the phone is unlocked on the cable and Trek Driver is installed (or should be:
//!   Trek's iPhone app was installed from this Mac with `install-device.sh`): taps go through the
//!   driver, an XCUITest inside that app (`crate::driver`).
//! - Else **Bluetooth** when the phone took Trek's keyboard and mouse.
//! - Else **iPhone Mirroring** when that route is known to work here: iPhone Mirroring has shown
//!   an iPhone on this Mac (it pairs only with one on the Mac's own Apple Account, which is just
//!   the phone that can't take the Bluetooth keyboard), or the phone is bonded to the Mac for
//!   Continuity (its restored subscriptions), or Bluetooth can't work at all.
//! - Else Bluetooth, which then needs the one-time pairing on the phone.

use crate::diagnose::Blocker;
use crate::driver::{Avail, INSTALL, State as Driver};
use crate::helper::Status;
use crate::mirror::State as Mirror;

/// How long since the last frame over the cable the screen still counts as live: the cable sends
/// frames only when the picture changes, so a still screen goes quiet without locking.
pub const USB_QUIET: f64 = 30.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Trek Driver over the cable, while the phone is unlocked.
    Usb,
    Bluetooth,
    Mirroring,
}

/// What the strip is decided from.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// Whether usbmuxd sees a phone.
    pub phone: bool,
    /// Whether the helper runs (its status is real).
    pub helper: bool,
    pub status: Status,
    /// iPhone Mirroring as last read (`None` before the first look).
    pub mirror: Option<Mirror>,
    /// iPhone Mirroring has shown an iPhone on this Mac before (remembered).
    pub mirroring_known: bool,
    /// The picture comes from iPhone Mirroring's window.
    pub mirror_frames: bool,
    /// The picture comes from Trek Driver: the cable's capture is off, so the iPhone shows its
    /// real clock (captured, it shows 9:41, on the phone too).
    pub driver_frames: bool,
    pub assistive: Option<bool>,
    /// Someone wants taps now (an agent holds the phone, or the user just tapped it): the lock
    /// step is then a thing to do, not just a state.
    pub wants_taps: bool,
    /// Whether Trek Driver can be used with this phone, and where it is.
    pub driver: Avail,
    pub driver_state: Driver,
    /// What `diagnose` found in the way, after iPhone Mirroring timed out with the phone locked.
    pub blocker: Option<Blocker>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Wait,
    Bad,
    Quiet,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    pub label: &'static str,
    pub word: String,
    pub tone: Tone,
}

fn chip(label: &'static str, tone: Tone, word: impl Into<String>) -> Chip {
    Chip {
        label,
        word: word.into(),
        tone,
    }
}

/// The strip: a chip each for the screen and taps (and AssistiveTouch on the Bluetooth route),
/// and the one thing the user must do now, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Strip {
    pub route: Route,
    pub screen: Chip,
    pub taps: Chip,
    pub assistive: Option<Chip>,
    /// One short line for the user, only when they must act now.
    pub action: Option<String>,
    /// What an agent should do or tell the user, when taps can't reach the phone.
    pub advice: Option<String>,
    /// The action line is the diagnosis of a timed-out iPhone Mirroring: the tool offers to
    /// check again (after the user fixed it, or to find out).
    pub recheck: bool,
}

const LOCK: &str = "Lock the iPhone to let Trek tap it.";
const UNREACHABLE: &str =
    "Turn Wi-Fi and Bluetooth on for both, on the same network, with the iPhone nearby.";
const PAIR: &str = "On the iPhone, open Settings › Bluetooth and tap \"Trek\" under Other Devices.";

impl Facts {
    /// Whether the cable brought a picture lately: the iPhone is unlocked (Mirroring can't
    /// connect then).
    pub fn usb_live(&self) -> bool {
        let s = &self.status;
        self.driver_frames
            || (!s.screen.is_empty()
                && s.frames > 0
                && !s.dark
                && (0. ..USB_QUIET).contains(&s.frame_age))
    }

    /// Whether the iPhone is known to be locked: the cable gives no picture at all (an unlocked
    /// screen sends one as soon as capture starts), or its last one is black.
    pub fn locked(&self) -> bool {
        let s = &self.status;
        !self.driver_frames && !s.screen.is_empty() && (s.frames == 0 || s.dark)
    }

    /// Whether Bluetooth can't carry taps at all here.
    fn bluetooth_cannot(&self) -> bool {
        let s = &self.status;
        s.linked_only() || matches!(s.bluetooth.as_str(), "off" | "not allowed" | "unsupported")
    }

    /// Whether taps go over the cable through Trek Driver: it's (or should be) installed and the
    /// phone is unlocked on the cable (its picture isn't black; or the cable's screen isn't read
    /// yet and the driver already runs). A locked phone goes to iPhone Mirroring.
    pub fn usb_route(&self) -> bool {
        if self.driver == Avail::None || !self.phone || self.locked() || self.mirror_frames {
            return false;
        }
        !self.status.screen.is_empty() || self.driver_state == Driver::Ready
    }

    pub fn route(&self) -> Route {
        if self.usb_route() {
            return Route::Usb;
        }
        let mirroring_here =
            self.mirroring_known || self.mirror.as_ref().is_some_and(Mirror::set_up);
        if self.status.input_ready() {
            Route::Bluetooth
        } else if mirroring_here
            || self.status.linked_only()
            || (self.bluetooth_cannot() && self.mirror != Some(Mirror::Missing))
        {
            Route::Mirroring
        } else {
            Route::Bluetooth
        }
    }

    pub fn strip(&self) -> Strip {
        let route = self.route();
        let s = &self.status;
        let screen = if !self.phone {
            chip("Screen", Tone::Quiet, "No iPhone")
        } else if self.mirror_frames {
            chip("Screen", Tone::Good, "Live in iPhone Mirroring")
        } else if self.driver_frames {
            // The header says it's the cable; the picture is the driver's, with the real clock.
            chip("Screen", Tone::Good, "Live")
        } else if !self.helper || s.camera.is_empty() {
            chip("Screen", Tone::Quiet, "Starting…")
        } else if s.camera == "undetermined" {
            chip("Screen", Tone::Wait, "Allow the camera")
        } else if s.camera == "denied" {
            chip("Screen", Tone::Bad, "Camera not allowed")
        } else if s.screen.is_empty() {
            chip("Screen", Tone::Wait, "None over this cable")
        } else if self.usb_live() {
            // The header says it's the cable.
            chip("Screen", Tone::Good, "Live")
        } else if self.locked() {
            chip("Screen", Tone::Quiet, "Locked")
        } else {
            chip("Screen", Tone::Quiet, "Locked or idle")
        };
        let (taps, mut action, advice) = match route {
            Route::Usb => self.usb_taps(),
            Route::Bluetooth => self.bluetooth_taps(),
            Route::Mirroring => self.mirroring_taps(),
        };
        let assistive =
            (route == Route::Bluetooth && s.input_ready()).then(|| match self.assistive {
                Some(true) => chip("AssistiveTouch", Tone::Good, "On"),
                Some(false) => chip("AssistiveTouch", Tone::Quiet, "Off until a tap"),
                None => chip("AssistiveTouch", Tone::Quiet, "—"),
            });
        if !self.phone {
            action = Some("Plug the iPhone in with a data cable, unlock it and tap Trust.".into());
        } else if s.camera == "denied" {
            action = Some(
                "Allow Trek in System Settings › Privacy & Security › Camera to see the screen."
                    .into(),
            );
        }
        let recheck = route == Route::Mirroring
            && self.phone
            && self.mirror == Some(Mirror::TimedOut)
            && self.locked()
            && action.is_some();
        Strip {
            route,
            screen,
            taps,
            assistive,
            action,
            recheck,
            advice: advice.or_else(|| {
                (!self.phone).then(|| {
                    "No iPhone is connected: ask the user to plug it in, unlock it and trust this Mac." // words: ok, the iPhone tool is macOS only
                        .to_string()
                })
            }),
        }
    }

    fn usb_taps(&self) -> (Chip, Option<String>, Option<String>) {
        let lock_instead = self.mirroring_known
            || self.mirror.as_ref().is_some_and(Mirror::set_up)
            || self.status.linked_only();
        match (&self.driver, &self.driver_state) {
            (Avail::Install(why), _) => (
                chip(
                    "Taps",
                    Tone::Wait,
                    format!("Install the driver ({INSTALL})"),
                ),
                self.wants_taps.then(|| {
                    format!(
                        "Run {INSTALL} to tap over USB{}.",
                        if lock_instead {
                            ", or lock the iPhone to tap through iPhone Mirroring"
                        } else {
                            ""
                        }
                    )
                }),
                Some(format!(
                    "Taps over USB need Trek Driver, and {why}: ask the user to run {INSTALL} (it builds Trek's iPhone app with the driver inside){}.",
                    if lock_instead {
                        ", or to lock the iPhone so taps go through iPhone Mirroring"
                    } else {
                        ""
                    }
                )),
            ),
            (_, Driver::Starting) => (chip("Taps", Tone::Wait, "Starting the driver…"), None, None),
            (_, Driver::Failed(why)) => (
                chip("Taps", Tone::Bad, "The driver didn't start"),
                self.wants_taps.then(|| why.clone()),
                Some(why.clone()),
            ),
            // Off: it starts with the first tap.
            _ => (chip("Taps", Tone::Good, "Ready · USB"), None, None),
        }
    }

    fn bluetooth_taps(&self) -> (Chip, Option<String>, Option<String>) {
        let s = &self.status;
        if s.input_ready() {
            return (chip("Taps", Tone::Good, "Ready · Bluetooth"), None, None);
        }
        match s.bluetooth.as_str() {
            _ if !self.helper => (chip("Taps", Tone::Quiet, "Starting…"), None, None),
            "advertising" | "connected" => (
                chip("Taps", Tone::Wait, "Pair once in Bluetooth"),
                Some(PAIR.into()),
                Some(NOT_PAIRED.into()),
            ),
            "" | "starting" => (chip("Taps", Tone::Quiet, "Starting…"), None, None),
            "off" => (
                chip("Taps", Tone::Bad, "Bluetooth is off"),
                Some("Turn Bluetooth on to let Trek tap the iPhone.".into()),
                Some("Bluetooth is off on the Mac: ask the user to turn it on.".into()),
            ),
            "not allowed" => (
                chip("Taps", Tone::Bad, "Bluetooth not allowed"),
                Some("Allow Trek in System Settings › Privacy & Security › Bluetooth.".into()),
                Some("Trek isn't allowed to use Bluetooth: ask the user to allow it in System Settings › Privacy & Security › Bluetooth.".into()),
            ),
            _ => (
                chip("Taps", Tone::Bad, "Bluetooth unavailable"),
                None,
                Some("Bluetooth isn't available, so taps can't reach the iPhone.".into()),
            ),
        }
    }

    fn mirroring_taps(&self) -> (Chip, Option<String>, Option<String>) {
        let lock = |tone| {
            (
                chip("Taps", tone, "Lock the iPhone to tap"),
                self.wants_taps.then(|| LOCK.to_string()),
                Some(Mirror::PhoneInUse.advice()),
            )
        };
        match self.mirror.as_ref() {
            Some(Mirror::Live) => (
                chip("Taps", Tone::Good, "Ready · iPhone Mirroring"),
                None,
                None,
            ),
            Some(Mirror::Missing) => (
                chip("Taps", Tone::Bad, "Needs iPhone Mirroring"),
                None,
                Some(Mirror::Missing.advice()),
            ),
            Some(Mirror::NotAllowed) => (
                chip("Taps", Tone::Bad, "Allow Accessibility"),
                Some("Allow Trek in System Settings › Privacy & Security › Accessibility.".into()),
                Some(Mirror::NotAllowed.advice()),
            ),
            Some(Mirror::MacLocked) => (
                chip("Taps", Tone::Wait, "Unlock iPhone Mirroring"),
                Some("iPhone Mirroring wants Touch ID or your password.".into()),
                Some(Mirror::MacLocked.advice()),
            ),
            // Locked and still timing out: locking again won't change it. The diagnosis names
            // what's in the way (shown whether or not someone wants taps: it needs the user);
            // before it's in, the general advice.
            Some(Mirror::TimedOut) if self.locked() => match &self.blocker {
                Some(b) => (
                    chip("Taps", Tone::Bad, "iPhone Mirroring can't reach it"),
                    Some(b.action()),
                    Some(b.advice()),
                ),
                None => (
                    chip("Taps", Tone::Bad, "iPhone Mirroring can't reach it"),
                    self.wants_taps.then(|| UNREACHABLE.to_string()),
                    Some(Mirror::unreachable_advice().into()),
                ),
            },
            Some(Mirror::PhoneInUse | Mirror::TimedOut) => lock(Tone::Wait),
            _ if self.usb_live() => lock(Tone::Wait),
            Some(Mirror::Connecting) => (chip("Taps", Tone::Wait, "Connecting…"), None, None),
            // Closed (Trek opens it when needed) or anything it says that isn't known.
            _ => (
                chip("Taps", Tone::Good, "Ready · iPhone Mirroring"),
                None,
                None,
            ),
        }
    }
}

/// An iPhone's marketing name from its model identifier, for the recent ones iPhone Mirroring
/// works with (it names the phone by model when connecting).
pub fn model_name(product_type: &str) -> Option<&'static str> {
    Some(match product_type {
        "iPhone11,2" => "iPhone XS",
        "iPhone11,4" | "iPhone11,6" => "iPhone XS Max",
        "iPhone11,8" => "iPhone XR",
        "iPhone12,1" => "iPhone 11",
        "iPhone12,3" => "iPhone 11 Pro",
        "iPhone12,5" => "iPhone 11 Pro Max",
        "iPhone12,8" => "iPhone SE",
        "iPhone13,1" => "iPhone 12 mini",
        "iPhone13,2" => "iPhone 12",
        "iPhone13,3" => "iPhone 12 Pro",
        "iPhone13,4" => "iPhone 12 Pro Max",
        "iPhone14,4" => "iPhone 13 mini",
        "iPhone14,5" => "iPhone 13",
        "iPhone14,2" => "iPhone 13 Pro",
        "iPhone14,3" => "iPhone 13 Pro Max",
        "iPhone14,6" => "iPhone SE",
        "iPhone14,7" => "iPhone 14",
        "iPhone14,8" => "iPhone 14 Plus",
        "iPhone15,2" => "iPhone 14 Pro",
        "iPhone15,3" => "iPhone 14 Pro Max",
        "iPhone15,4" => "iPhone 15",
        "iPhone15,5" => "iPhone 15 Plus",
        "iPhone16,1" => "iPhone 15 Pro",
        "iPhone16,2" => "iPhone 15 Pro Max",
        "iPhone17,1" => "iPhone 16 Pro",
        "iPhone17,2" => "iPhone 16 Pro Max",
        "iPhone17,3" => "iPhone 16",
        "iPhone17,4" => "iPhone 16 Plus",
        "iPhone17,5" => "iPhone 16e",
        _ => return None,
    })
}

/// Whether the iPhone iPhone Mirroring named (`seen`: its name or model; "" when it didn't say)
/// can be this phone: then the phone is on this Mac's Apple Account and Mirroring is its route.
/// A different iPhone of the account must not get this one's taps.
pub fn mirrors(seen: &str, name: &str, product_type: &str) -> bool {
    let plain = |s: &str| s.replace('\u{2019}', "'").trim().to_lowercase();
    let seen = plain(seen);
    if seen.is_empty() || seen == plain(name) {
        return true;
    }
    match model_name(product_type) {
        Some(model) => seen == plain(model),
        // A model this table doesn't know: nothing says it's another phone.
        None => true,
    }
}

/// Why taps can't reach a phone that hasn't paired, for an agent: the action first.
pub const NOT_PAIRED: &str = "Ask the user to pair the iPhone once: on the iPhone, Settings › Bluetooth, tap \"Trek\" under Other Devices (Trek's iPhone tool must be open). Screenshots, apps and files work without it.";

/// The full story, for the iPhone tool's "How this works".
pub fn explainer(route: Route) -> Vec<(&'static str, &'static str)> {
    let screen = (
        "Screen",
        "While the iPhone is unlocked, its screen comes over the USB cable. While it's locked and Trek taps through iPhone Mirroring, the picture comes from iPhone Mirroring's window.",
    );
    let mirroring = (
        "Taps through iPhone Mirroring",
        "An iPhone on this Mac's own Apple Account can't take Trek as a Bluetooth keyboard and mouse, so Trek taps and types through iPhone Mirroring, in the background. It connects only while the iPhone is locked and nearby: Trek opens it behind your windows and presses Connect when it needs to. Choose Authenticate Automatically in iPhone Mirroring's Settings so it doesn't ask for Touch ID each time.", // words: ok, the iPhone tool is macOS only
    );
    let bluetooth = (
        "Taps over Bluetooth",
        "An iPhone on another Apple Account pairs once with Trek as a Bluetooth keyboard and mouse: Settings › Bluetooth, \"Trek\" under Other Devices. Trek turns AssistiveTouch on while it taps (that's what shows a pointer) and off again after; turn Always Show Menu off in Settings › Accessibility › Touch › AssistiveTouch to keep its button out of the way.",
    );
    let files = (
        "Apps and files",
        "Installing apps and copying files go over the cable, with nothing to set up.",
    );
    let usb = (
        "Taps over USB",
        "While the iPhone is unlocked on the cable, Trek taps, swipes and types through Trek Driver, a small test runner inside Trek's iPhone app that ios/scripts/install-device.sh builds with it; it starts with the first tap (about 10 seconds) and stops when the agent lets go.",
    );
    let clock = (
        "The clock",
        "While its screen is captured over USB, iOS shows 9:41 and full bars in the status bar, on the iPhone too, as it does for QuickTime. While Trek Driver runs, the picture comes from the driver instead and the clock is the real one; otherwise Trek draws the iPhone's real time over it.",
    );
    match route {
        Route::Usb => vec![usb, screen, clock, files, mirroring, bluetooth],
        Route::Mirroring => vec![mirroring, screen, clock, files, usb, bluetooth],
        Route::Bluetooth => vec![bluetooth, screen, clock, files, usb, mirroring],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb_live() -> Status {
        Status {
            camera: "granted".into(),
            screen: "tobyj's iPhone".into(),
            width: 1320,
            height: 2868,
            frames: 40,
            frame_age: 0.2,
            bluetooth: "advertising".into(),
            ..Status::default()
        }
    }

    fn facts(status: Status, mirror: Option<Mirror>) -> Facts {
        Facts {
            phone: true,
            helper: true,
            status,
            mirror,
            ..Facts::default()
        }
    }

    #[test]
    fn the_drivers_picture_counts_as_live_over_usb_with_the_cable_quiet() {
        // The cable's capture is off (its last frame long ago), the driver gives the picture.
        let mut s = usb_live();
        s.frame_age = 600.;
        let mut f = facts(s, Some(Mirror::Closed));
        f.driver = Avail::Ready;
        f.driver_state = Driver::Ready;
        f.driver_frames = true;
        assert!(f.usb_live() && !f.locked());
        let strip = f.strip();
        assert_eq!(strip.route, Route::Usb);
        assert_eq!(
            (strip.screen.word.as_str(), strip.screen.tone),
            ("Live", Tone::Good)
        );
        assert_eq!(strip.taps.word, "Ready · USB");
        // Without the driver's picture the same cable state reads as idle.
        f.driver_frames = false;
        assert!(!f.usb_live());
        assert_eq!(f.strip().screen.word, "Locked or idle");
    }

    #[test]
    fn a_paired_phone_taps_over_bluetooth_with_assistive_touch_shown() {
        let mut s = usb_live();
        s.hosts = 1;
        s.bluetooth = "connected".into();
        let mut f = facts(s, Some(Mirror::Closed));
        f.assistive = Some(false);
        let strip = f.strip();
        assert_eq!(strip.route, Route::Bluetooth);
        assert_eq!(strip.screen.word, "Live");
        assert_eq!(strip.taps.word, "Ready · Bluetooth");
        assert_eq!(strip.assistive.unwrap().word, "Off until a tap");
        assert_eq!(strip.action, None);
        assert_eq!(strip.advice, None);
    }

    #[test]
    fn mirroring_set_up_on_this_mac_means_no_pairing_step() {
        // The user's case: same Apple Account, Bluetooth advertising, Mirroring timed out
        // while the phone was unlocked.
        let f = facts(usb_live(), Some(Mirror::TimedOut));
        let strip = f.strip();
        assert_eq!(strip.route, Route::Mirroring);
        assert_eq!(strip.taps.word, "Lock the iPhone to tap");
        assert!(strip.assistive.is_none());
        assert_eq!(strip.action, None, "nobody wants taps: nothing to do now");
        let advice = strip.advice.unwrap();
        assert!(
            advice.starts_with("Ask the user to lock the iPhone"),
            "{advice}"
        );
        assert!(!advice.contains("Settings › Bluetooth"));
        let mut f = f;
        f.wants_taps = true;
        assert_eq!(f.strip().action.as_deref(), Some(LOCK));
        // Remembered from an earlier look, with iPhone Mirroring closed now.
        let mut f = facts(usb_live(), Some(Mirror::Closed));
        f.mirroring_known = true;
        assert_eq!(f.strip().route, Route::Mirroring);
        assert_eq!(f.strip().taps.word, "Lock the iPhone to tap");
    }

    #[test]
    fn timing_out_while_locked_is_a_network_problem_not_a_lock_one() {
        // Seen live: the phone locked (no picture over the cable at all), iPhone Mirroring timed
        // out "connecting" and its log said it found no endpoint for the phone.
        let mut s = usb_live();
        s.frames = 0;
        s.frame_age = -1.;
        let mut f = facts(s, Some(Mirror::TimedOut));
        f.wants_taps = true;
        assert!(f.locked());
        let strip = f.strip();
        assert_eq!(strip.taps.word, "iPhone Mirroring can't reach it");
        assert_eq!(strip.action.as_deref(), Some(UNREACHABLE));
        assert!(strip.recheck, "the line offers to check again");
        assert!(
            !strip
                .advice
                .unwrap()
                .contains("lock the iPhone (side button)")
        );
        // Once diagnosed: the one blocker, in place of the general line, for the user and agents.
        f.blocker = Some(Blocker::PhoneNotOffering);
        f.wants_taps = false;
        let strip = f.strip();
        assert_eq!(strip.action, Some(Blocker::PhoneNotOffering.action()));
        assert!(strip.action.unwrap().contains("restart the iPhone"));
        assert!(strip.advice.unwrap().contains("Reset iPhone Access"));
        assert!(strip.recheck);
        // Live again: nothing to do, nothing to check.
        f.mirror = Some(Mirror::Live);
        let strip = f.strip();
        assert_eq!(strip.action, None);
        assert!(!strip.recheck);
    }

    #[test]
    fn a_locked_phone_is_ready_through_mirroring() {
        let mut s = usb_live();
        s.frame_age = 90.;
        let mut f = facts(s, Some(Mirror::Closed));
        f.mirroring_known = true;
        let strip = f.strip();
        assert_eq!(strip.screen.word, "Locked or idle");
        assert_eq!(strip.taps.word, "Ready · iPhone Mirroring");
        // Just locked: the last frame is black.
        f.status.frame_age = 2.;
        f.status.dark = true;
        assert!(!f.usb_live());
        assert_eq!(f.strip().screen.word, "Locked");
        assert_eq!(f.strip().taps.word, "Ready · iPhone Mirroring");
        f.mirror = Some(Mirror::Live);
        f.mirror_frames = true;
        let strip = f.strip();
        assert_eq!(strip.screen.word, "Live in iPhone Mirroring");
        assert_eq!(strip.taps.word, "Ready · iPhone Mirroring");
        assert_eq!(strip.action, None);
        f.mirror = Some(Mirror::MacLocked);
        f.mirror_frames = false;
        assert_eq!(f.strip().taps.word, "Unlock iPhone Mirroring");
        assert!(f.strip().action.is_some());
    }

    #[test]
    fn pairing_is_asked_only_where_bluetooth_can_work() {
        // Nothing says Mirroring works here: the Bluetooth route, and its one step.
        let strip = facts(usb_live(), Some(Mirror::Closed)).strip();
        assert_eq!(strip.route, Route::Bluetooth);
        assert_eq!(strip.taps.word, "Pair once in Bluetooth");
        assert_eq!(strip.action.as_deref(), Some(PAIR));
        assert!(strip.advice.unwrap().starts_with("Ask the user to pair"));
        // No iPhone Mirroring on this Mac: still Bluetooth.
        assert_eq!(
            facts(usb_live(), Some(Mirror::Missing)).route(),
            Route::Bluetooth
        );
        // A Continuity bond: Bluetooth can't work, whatever Mirroring says.
        let mut s = usb_live();
        s.linked = 1;
        let strip = facts(s, None).strip();
        assert_eq!(strip.route, Route::Mirroring);
        assert_ne!(strip.taps.word, "Pair once in Bluetooth");
        // Bluetooth off, iPhone Mirroring there: Mirroring.
        let mut s = usb_live();
        s.bluetooth = "off".into();
        assert_eq!(facts(s, Some(Mirror::Closed)).route(), Route::Mirroring);
    }

    fn with_driver(status: Status, avail: Avail) -> Facts {
        let mut f = facts(status, Some(Mirror::TimedOut));
        f.mirroring_known = true;
        f.driver = avail;
        f
    }

    #[test]
    fn an_unlocked_phone_taps_over_usb_when_the_driver_is_there() {
        // The user's case: same Apple Account (Mirroring known), unlocked on the cable.
        let f = with_driver(usb_live(), Avail::Ready);
        let strip = f.strip();
        assert_eq!(strip.route, Route::Usb);
        assert_eq!(strip.taps.word, "Ready · USB");
        assert_eq!((strip.action, strip.advice), (None, None));
        assert!(strip.assistive.is_none());
        // Starting, then running.
        let mut f = f;
        f.driver_state = Driver::Starting;
        assert_eq!(f.strip().taps.word, "Starting the driver…");
        f.driver_state = Driver::Failed(
            "Unlock the iPhone: Trek Driver starts only while it's unlocked.".into(),
        );
        f.wants_taps = true;
        let strip = f.strip();
        assert_eq!(strip.taps.word, "The driver didn't start");
        assert!(strip.action.unwrap().starts_with("Unlock the iPhone"));
        // Paired over Bluetooth too: the cable still wins while unlocked.
        let mut s = usb_live();
        s.hosts = 1;
        s.bluetooth = "connected".into();
        assert_eq!(with_driver(s, Avail::Ready).route(), Route::Usb);
    }

    #[test]
    fn a_locked_phone_goes_back_to_mirroring_and_no_driver_means_no_usb_route() {
        let mut s = usb_live();
        s.dark = true;
        let f = with_driver(s, Avail::Ready);
        assert!(f.locked());
        assert_eq!(f.route(), Route::Mirroring);
        // Locked while the driver still runs: iPhone Mirroring all the same.
        let mut f = f;
        f.driver_state = Driver::Ready;
        assert_eq!(f.route(), Route::Mirroring);
        // The cable's screen not read yet, the driver running: USB.
        let mut f = with_driver(Status::default(), Avail::Ready);
        f.driver_state = Driver::Ready;
        assert_eq!(f.route(), Route::Usb);
        // iPhone Mirroring showing the phone: Mirroring.
        let mut s = usb_live();
        s.dark = true;
        let mut f = with_driver(s, Avail::Ready);
        f.mirror_frames = true;
        assert_eq!(f.route(), Route::Mirroring);
        // A Mac that never built Trek's iPhone app: as before.
        assert_eq!(
            with_driver(usb_live(), Avail::None).route(),
            Route::Mirroring
        );
        assert_eq!(
            with_driver(usb_live(), Avail::None).strip().taps.word,
            "Lock the iPhone to tap"
        );
        // No phone at all: never USB.
        let mut f = with_driver(usb_live(), Avail::Ready);
        f.phone = false;
        assert_ne!(f.route(), Route::Usb);
    }

    #[test]
    fn a_missing_driver_says_how_to_install_it() {
        let mut f = with_driver(usb_live(), Avail::Install("it isn't installed yet".into()));
        let strip = f.strip();
        assert_eq!(strip.route, Route::Usb);
        assert_eq!(
            strip.taps.word,
            "Install the driver (ios/scripts/install-device.sh)"
        );
        assert_eq!(strip.action, None, "nobody wants taps: nothing to do now");
        let advice = strip.advice.unwrap();
        assert!(advice.contains("ios/scripts/install-device.sh"), "{advice}");
        assert!(advice.contains("lock the iPhone"), "{advice}");
        f.wants_taps = true;
        assert!(
            f.strip()
                .action
                .unwrap()
                .starts_with("Run ios/scripts/install-device.sh")
        );
    }

    #[test]
    fn no_phone_says_how_to_connect_it() {
        let f = Facts::default();
        let strip = f.strip();
        assert_eq!(strip.screen.word, "No iPhone");
        assert!(strip.action.unwrap().starts_with("Plug the iPhone in"));
    }

    #[test]
    fn mirroring_counts_only_for_the_phone_it_named() {
        assert!(mirrors("iPhone 16 Pro Max", "tobyj’s iPhone", "iPhone17,2"));
        assert!(mirrors("tobyj's iPhone", "tobyj’s iPhone", "iPhone17,2"));
        assert!(mirrors("", "tobyj’s iPhone", "iPhone17,2"));
        assert!(!mirrors("iPhone 15", "Work phone", "iPhone17,2"));
        assert!(mirrors("iPhone 15", "Work phone", "iPhone99,1"));
    }

    #[test]
    fn the_explainer_leads_with_the_route_in_use() {
        assert_eq!(
            explainer(Route::Mirroring)[0].0,
            "Taps through iPhone Mirroring"
        );
        assert_eq!(explainer(Route::Bluetooth)[0].0, "Taps over Bluetooth");
        let usb = explainer(Route::Usb);
        assert_eq!(usb[0].0, "Taps over USB");
        assert!(
            usb.iter()
                .any(|(t, body)| *t == "The clock" && body.contains("9:41"))
        );
    }
}
