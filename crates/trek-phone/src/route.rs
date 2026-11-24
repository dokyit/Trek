//! Which way taps and keys reach the phone, decided once from what Trek knows, and what the
//! iPhone tool's strip and `phone_status` say about it: the same words in both places.
//!
//! - **Trek Driver** while the phone is unlocked and Trek Driver is installed (or should be:
//!   Trek's iPhone app was installed from this Mac with `install-device.sh`): taps go through the
//!   driver, an XCUITest inside that app (`crate::driver`). Over the cable when it's plugged in;
//!   else over Wi-Fi, when usbmuxd sees the phone on the network (paired by cable once, "Show
//!   this iPhone when on Wi-Fi" on) and the driver can serve there. With no cable the picture
//!   comes from the driver too, and whether the phone is locked from CoreDevice (`passcode`).
//! - Else **Bluetooth** when the phone took Trek's keyboard and mouse.
//! - Else **iPhone Mirroring** when that route is known to work here: iPhone Mirroring has shown
//!   an iPhone on this Mac (it pairs only with one on the Mac's own Apple Account, which is just
//!   the phone that can't take the Bluetooth keyboard), or the phone is bonded to the Mac for
//!   Continuity (its restored subscriptions), or Bluetooth can't work at all.
//! - Else Bluetooth, which then needs the one-time pairing on the phone.

use crate::diagnose::Blocker;
use crate::driver::{Avail, REINSTALL, State as Driver};
use crate::helper::Status;
use crate::mirror::State as Mirror;

/// How long since the last frame over the cable the screen still counts as live: the cable sends
/// frames only when the picture changes, so a still screen goes quiet without locking.
pub const USB_QUIET: f64 = 30.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Trek Driver, over the cable or Wi-Fi, while the phone is unlocked.
    Driver,
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
    /// The picture comes from CoreDevice's live screen share (`coredevice::live`): the cable's
    /// capture is off for it too.
    pub live_frames: bool,
    pub assistive: Option<bool>,
    /// Someone wants taps now (an agent holds the phone, or the user just tapped it): the lock
    /// step is then a thing to do, not just a state.
    pub wants_taps: bool,
    /// Whether Trek Driver can be used with this phone, and where it is.
    pub driver: Avail,
    pub driver_state: Driver,
    /// What `diagnose` found in the way, after iPhone Mirroring timed out with the phone locked.
    pub blocker: Option<Blocker>,
    /// usbmuxd sees the phone over Wi-Fi only (no cable): no picture comes over the cable, and
    /// the driver's is the one there is.
    pub wifi: bool,
    /// Over Wi-Fi, whether the phone needs its passcode now (CoreDevice's lock state), which the
    /// cable's black picture tells otherwise; `None` before it's known.
    pub passcode: Option<bool>,
    /// Installing Trek Driver from the tool, where that stands.
    pub setup: Setup,
    /// Seconds until the installed driver's free signing runs out, when that's known.
    pub driver_left: Option<i64>,
    /// The phone on the cable hasn't trusted this Mac (no pairing record, or one it refuses).
    pub untrusted: bool,
    /// Why usbmuxd couldn't be asked for phones, when it couldn't.
    pub mux_error: Option<String>,
    /// The driver's picture has brought nothing for a while (not even its keep-alive): what's
    /// shown is the last frame, not the screen now.
    pub stalled: bool,
}

pub use crate::setup::{Missing, Setup};

impl Missing {
    /// The button for what's missing (a Mac's: a PC's tool never shows this strip).
    pub fn fix(self) -> Fix {
        match self {
            Missing::Mac => Fix::CheckAgain,
            Missing::Xcode => Fix::GetXcode,
            Missing::AppleId => Fix::OpenXcode,
            Missing::Source => Fix::GetSource,
        }
    }
}

/// A pane of System Settings › Privacy & Security.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Camera,
    Accessibility,
    Bluetooth,
}

impl Pane {
    pub fn url(self) -> &'static str {
        match self {
            Pane::Camera => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera"
            }
            Pane::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            Pane::Bluetooth => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Bluetooth"
            }
        }
    }
}

/// The button on the strip's action line: the next step, done for the user where Trek can.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    /// Connect iPhone Mirroring once more, and look at what's in the way again.
    CheckAgain,
    InstallDriver,
    RenewDriver,
    UpdateDriver,
    ReinstallDriver,
    /// The driver's install failed: run it again.
    TryAgain,
    GetXcode,
    OpenXcode,
    GetSource,
    Settings(Pane),
    OpenMirroring,
    /// Ask usbmuxd for the phone again.
    LookAgain,
}

impl Fix {
    pub fn label(self) -> &'static str {
        match self {
            Fix::CheckAgain => "Check again",
            Fix::InstallDriver => "Install driver",
            Fix::RenewDriver => "Renew",
            Fix::UpdateDriver => "Update driver",
            Fix::ReinstallDriver => "Reinstall driver",
            Fix::TryAgain => "Try again",
            Fix::GetXcode => "Get Xcode",
            Fix::OpenXcode => "Open Xcode",
            Fix::GetSource => "Get the source",
            Fix::Settings(_) => "Open Settings",
            Fix::OpenMirroring => "Open iPhone Mirroring",
            Fix::LookAgain => "Look again",
        }
    }

    /// Whether it builds and installs the driver (and so waits while that's under way).
    pub fn installs(self) -> bool {
        matches!(
            self,
            Fix::InstallDriver
                | Fix::RenewDriver
                | Fix::UpdateDriver
                | Fix::ReinstallDriver
                | Fix::TryAgain
        )
    }
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
    /// The action line's button, when Trek can take the next step (or check again after the
    /// user took it).
    pub fix: Option<Fix>,
    /// A quiet line under the strip when nothing must be done now: what the picture is, or how
    /// an install goes.
    pub note: Option<String>,
}

const LOCK: &str = "Lock the iPhone to let Trek tap it.";
const VIEW_ONLY: &str =
    "View only while the iPhone is unlocked. Lock it to tap it here through iPhone Mirroring.";
const CHOOSE: &str = "Same Apple Account as this Mac? Lock the iPhone and open iPhone Mirroring. Another account? On the iPhone, open Settings › Bluetooth and tap \"Trek\".";
const CHOOSE_ADVICE: &str = "Taps aren't set up for this iPhone yet. Ask the user: if it's on this Mac's Apple Account, lock it and open iPhone Mirroring (Trek's iPhone tool has a button for it); if it's on another account, pair it once in Settings › Bluetooth (tap \"Trek\" under Other Devices). Screenshots, apps and files work without either.";
const UNTRUSTED: &str = "Unlock the iPhone and tap Trust when it asks about this Mac.";
const UNREACHABLE: &str =
    "Turn Wi-Fi and Bluetooth on for both, on the same network, with the iPhone nearby.";
const PAIR: &str = "On the iPhone, open Settings › Bluetooth and tap \"Trek\" under Other Devices.";
const UNLOCK_WIFI: &str = "Unlock the iPhone to let Trek tap it over Wi-Fi.";
const UNLOCK_WIFI_ADVICE: &str = "The iPhone is locked and has no cable: ask the user to unlock it, so Trek taps over Wi-Fi through Trek Driver.";

impl Facts {
    /// Whether the picture is live: Trek Driver's, or the cable's lately (the iPhone is unlocked
    /// then, and Mirroring can't connect).
    pub fn usb_live(&self) -> bool {
        let s = &self.status;
        self.driver_frames
            || self.live_frames
            || (!s.screen.is_empty()
                && s.frames > 0
                && !s.dark
                && (0. ..USB_QUIET).contains(&s.frame_age))
    }

    /// Whether the iPhone is known to be locked: the cable gives no picture at all (an unlocked
    /// screen sends one as soon as capture starts), or its last one is black. Over Wi-Fi: it needs
    /// its passcode, as CoreDevice says.
    pub fn locked(&self) -> bool {
        if self.wifi {
            return self.passcode == Some(true);
        }
        let s = &self.status;
        !self.driver_frames
            && !self.live_frames
            && !s.screen.is_empty()
            && (s.frames == 0 || s.dark)
    }

    /// Whether Bluetooth can't carry taps at all here.
    fn bluetooth_cannot(&self) -> bool {
        let s = &self.status;
        s.linked_only() || matches!(s.bluetooth.as_str(), "off" | "not allowed" | "unsupported")
    }

    /// Whether taps go through Trek Driver: it's (or should be) installed and the phone is
    /// unlocked, on the cable (its picture isn't black; or the cable's screen isn't read yet and
    /// the driver already runs) or over Wi-Fi (not known to be locked: the driver's start says if
    /// it is). A locked phone goes to iPhone Mirroring.
    pub fn driver_route(&self) -> bool {
        if self.driver == Avail::None || !self.phone || self.locked() || self.mirror_frames {
            return false;
        }
        self.wifi || !self.status.screen.is_empty() || self.driver_state == Driver::Ready
    }

    /// Over Wi-Fi with the phone locked and the driver ready: unlocking it brings taps back
    /// through the driver, which is less to ask than what the other routes still need.
    fn unlock_for_driver(&self) -> bool {
        self.wifi && self.phone && self.driver == Avail::Ready && self.locked()
    }

    pub fn route(&self) -> Route {
        if self.driver_route() {
            return Route::Driver;
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
        } else if self.untrusted {
            chip("Screen", Tone::Wait, "Not trusted")
        } else if self.mirror_frames {
            chip("Screen", Tone::Good, "Live in iPhone Mirroring")
        } else if self.driver_frames && self.stalled {
            // The last frame stays up, dimmed: it isn't the screen now.
            chip("Screen", Tone::Wait, "Reconnecting…")
        } else if self.driver_frames && self.wifi {
            chip("Screen", Tone::Good, "Live · Wi-Fi")
        } else if self.driver_frames {
            // The header says it's the cable; the picture is the driver's, with the real clock.
            chip("Screen", Tone::Good, "Live")
        } else if self.live_frames {
            chip("Screen", Tone::Good, "Live")
        } else if self.wifi {
            // No cable: the driver's picture is the one there is (and iPhone Mirroring's, above).
            if self.locked() {
                chip("Screen", Tone::Quiet, "Locked")
            } else if route == Route::Driver
                && self.driver == Avail::Ready
                && !matches!(self.driver_state, Driver::Failed(_))
            {
                chip("Screen", Tone::Quiet, "Starting…")
            } else {
                chip("Screen", Tone::Quiet, "None over Wi-Fi")
            }
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
        let Taps {
            mut taps,
            mut action,
            mut advice,
            mut fix,
        } = match route {
            Route::Driver => self.driver_taps(),
            Route::Bluetooth => self.bluetooth_taps(),
            Route::Mirroring => self.mirroring_taps(),
        };
        let mut note = None;
        // Locked over Wi-Fi with the driver ready, and the other route can't tap now (Bluetooth
        // isn't paired, iPhone Mirroring can't reach the phone or isn't here): unlocking is the
        // way. iPhone Mirroring that's ready, or only connecting, needs nothing.
        let unlock = self.unlock_for_driver()
            && match route {
                Route::Bluetooth => !s.input_ready(),
                Route::Mirroring => matches!(
                    self.mirror,
                    Some(Mirror::TimedOut | Mirror::Missing | Mirror::NotAllowed)
                ),
                Route::Driver => false,
            };
        if unlock {
            if route == Route::Bluetooth {
                taps = chip("Taps", Tone::Wait, "Unlock the iPhone to tap");
            }
            action = self.wants_taps.then(|| UNLOCK_WIFI.to_string());
            advice = Some(UNLOCK_WIFI_ADVICE.to_string());
            fix = None;
        }
        // The picture is the phone's, but nothing taps it while it's unlocked: say so before
        // the user finds out by clicking.
        if route == Route::Mirroring
            && self.driver == Avail::None
            && !self.mirror_frames
            && self.usb_live()
            && action.is_none()
        {
            note = Some(VIEW_ONLY.to_string());
        }
        // Installing the driver from the tool: how it goes, or what it needs.
        match &self.setup {
            Setup::Idle => {}
            Setup::Checking => {
                note = Some("Checking what this Mac has to build the driver with…".into());
            }
            Setup::Building | Setup::Installing => {
                if route == Route::Driver {
                    taps = chip("Taps", Tone::Wait, "Installing the driver…");
                }
                action = None;
                fix = None;
                note = Some(
                    if self.setup == Setup::Building {
                        "Building Trek's iPhone app with its driver. This takes a few minutes."
                    } else {
                        "Installing on the iPhone. Keep it unlocked and nearby."
                    }
                    .into(),
                );
            }
            Setup::Missing(m) => {
                action = Some(m.line().into());
                fix = Some(m.fix());
            }
            Setup::Failed(why) => {
                if route == Route::Driver {
                    taps = chip("Taps", Tone::Bad, "The driver didn't install");
                }
                action = Some(why.clone());
                fix = Some(Fix::TryAgain);
            }
            // Installed, yet still no driver: the app went on without it (it didn't build).
            Setup::Installed
                if route == Route::Driver && matches!(self.driver, Avail::Install(_)) =>
            {
                action = Some(
                    "Trek's iPhone app is installed, but its driver didn't build. Try again, or lock the iPhone to tap through iPhone Mirroring."
                        .into(),
                );
                fix = Some(Fix::TryAgain);
            }
            Setup::Installed => {}
        }
        let assistive =
            (route == Route::Bluetooth && s.input_ready()).then(|| match self.assistive {
                Some(true) => chip("AssistiveTouch", Tone::Good, "On"),
                Some(false) => chip("AssistiveTouch", Tone::Quiet, "Off until a tap"),
                None => chip("AssistiveTouch", Tone::Quiet, "—"),
            });
        if !self.phone {
            action = Some(match &self.mux_error {
                Some(e) => format!("Couldn't ask macOS which iPhones are connected: {e}"),
                None => "Plug the iPhone in with a data cable, unlock it and tap Trust.".into(),
            });
            fix = self.mux_error.is_some().then_some(Fix::LookAgain);
            note = None;
        } else if self.untrusted {
            action = Some(UNTRUSTED.into());
            fix = Some(Fix::LookAgain);
            note = None;
        } else if s.camera == "denied" && !self.wifi {
            action = Some(
                "Allow Trek in System Settings › Privacy & Security › Camera to see the screen."
                    .into(),
            );
            fix = Some(Fix::Settings(Pane::Camera));
        }
        let advice = if self.phone && self.untrusted {
            Some(
                "The iPhone hasn't trusted this Mac: ask the user to unlock it and tap Trust."
                    .into(),
            )
        } else {
            advice
        };
        Strip {
            route,
            screen,
            taps,
            assistive,
            action,
            fix,
            note,
            advice: advice.or_else(|| {
                (!self.phone).then(|| {
                    "No iPhone is connected: ask the user to plug it in, unlock it and trust this Mac." // words: ok, the iPhone tool is macOS only
                        .to_string()
                })
            }),
        }
    }

    fn driver_taps(&self) -> Taps {
        let lock_instead = self.mirroring_known
            || self.mirror.as_ref().is_some_and(Mirror::set_up)
            || self.status.linked_only();
        let link = if self.wifi { "Wi-Fi" } else { "USB" };
        match (&self.driver, &self.driver_state) {
            (Avail::Install(why), _) => {
                let (word, line, fix) = install_step(why, link);
                Taps {
                    taps: chip("Taps", Tone::Wait, word),
                    action: Some(format!(
                        "{line}{}",
                        if lock_instead {
                            " Or lock the iPhone to tap through iPhone Mirroring."
                        } else {
                            ""
                        }
                    )),
                    advice: Some(format!(
                        "Taps over {link} need Trek Driver, and {why}: ask the user to press {} in Trek's iPhone tool (it builds Trek's iPhone app with the driver inside and installs it){}.",
                        fix.label(),
                        if lock_instead {
                            ", or to lock the iPhone so taps go through iPhone Mirroring"
                        } else {
                            ""
                        }
                    )),
                    fix: Some(fix),
                }
            }
            (_, Driver::Starting) => Taps::only(chip("Taps", Tone::Wait, "Starting the driver…")),
            (_, Driver::Failed(why)) => {
                // Not a lock problem (Xcode can't reach the phone, say): iPhone Mirroring, where
                // it works here, takes the phone once it's locked.
                let or_lock = if lock_instead && !why.starts_with("Unlock") {
                    " Or lock the iPhone to tap through iPhone Mirroring."
                } else {
                    ""
                };
                Taps {
                    taps: chip("Taps", Tone::Bad, "The driver didn't start"),
                    action: self.wants_taps.then(|| format!("{why}{or_lock}")),
                    advice: Some(format!("{why}{or_lock}")),
                    fix: (self.wants_taps && why.contains(REINSTALL))
                        .then_some(Fix::ReinstallDriver),
                }
            }
            // Off: it starts with the first tap. A day before its free signing runs out, the
            // line says so, with the way to renew it.
            _ => match self.driver_left.filter(|&left| left < RENEW_WARNING) {
                Some(left) => Taps {
                    taps: chip("Taps", Tone::Good, format!("Ready · {link}")),
                    action: Some(format!(
                        "Trek Driver's free signing runs out {}. Renew it to keep tapping.",
                        runs_out(left)
                    )),
                    advice: None,
                    fix: Some(Fix::RenewDriver),
                },
                None => Taps::only(chip("Taps", Tone::Good, format!("Ready · {link}"))),
            },
        }
    }

    fn bluetooth_taps(&self) -> Taps {
        let s = &self.status;
        if s.input_ready() {
            return Taps::only(chip("Taps", Tone::Good, "Ready · Bluetooth"));
        }
        match s.bluetooth.as_str() {
            _ if !self.helper => Taps::only(chip("Taps", Tone::Quiet, "Starting…")),
            // iPhone Mirroring is on this Mac but hasn't shown this phone: it may be on the
            // Mac's own Apple Account, which never lists Trek in Bluetooth. Both ways are offered.
            "advertising" | "connected" if self.mirror != Some(Mirror::Missing) => {
                let allow = self.mirror == Some(Mirror::NotAllowed);
                Taps {
                    taps: chip("Taps", Tone::Wait, "Not set up"),
                    action: Some(if allow {
                        "Same Apple Account as this Mac? Allow Trek in Privacy & Security › Accessibility so it can tap through iPhone Mirroring. Another account? On the iPhone, open Settings › Bluetooth and tap \"Trek\".".into()
                    } else {
                        CHOOSE.into()
                    }),
                    advice: Some(CHOOSE_ADVICE.into()),
                    fix: Some(if allow {
                        Fix::Settings(Pane::Accessibility)
                    } else {
                        Fix::OpenMirroring
                    }),
                }
            }
            "advertising" | "connected" => Taps {
                taps: chip("Taps", Tone::Wait, "Pair once in Bluetooth"),
                action: Some(PAIR.into()),
                advice: Some(NOT_PAIRED.into()),
                fix: None,
            },
            "" | "starting" => Taps::only(chip("Taps", Tone::Quiet, "Starting…")),
            "off" => Taps {
                taps: chip("Taps", Tone::Bad, "Bluetooth is off"),
                action: Some("Turn Bluetooth on to let Trek tap the iPhone.".into()),
                advice: Some("Bluetooth is off on the Mac: ask the user to turn it on.".into()),
                fix: None,
            },
            "not allowed" => Taps {
                taps: chip("Taps", Tone::Bad, "Bluetooth not allowed"),
                action: Some(
                    "Allow Trek in System Settings › Privacy & Security › Bluetooth.".into(),
                ),
                advice: Some("Trek isn't allowed to use Bluetooth: ask the user to allow it in System Settings › Privacy & Security › Bluetooth.".into()),
                fix: Some(Fix::Settings(Pane::Bluetooth)),
            },
            _ => Taps {
                taps: chip("Taps", Tone::Bad, "Bluetooth unavailable"),
                action: None,
                advice: Some("Bluetooth isn't available, so taps can't reach the iPhone.".into()),
                fix: None,
            },
        }
    }

    fn mirroring_taps(&self) -> Taps {
        let lock = |tone| Taps {
            taps: chip("Taps", tone, "Lock the iPhone to tap"),
            action: self.wants_taps.then(|| LOCK.to_string()),
            advice: Some(Mirror::PhoneInUse.advice()),
            fix: None,
        };
        match self.mirror.as_ref() {
            Some(Mirror::Live) => Taps::only(chip("Taps", Tone::Good, "Ready · iPhone Mirroring")),
            Some(Mirror::Missing) => Taps {
                taps: chip("Taps", Tone::Bad, "Needs iPhone Mirroring"),
                action: None,
                advice: Some(Mirror::Missing.advice()),
                fix: None,
            },
            Some(Mirror::NotAllowed) => Taps {
                taps: chip("Taps", Tone::Bad, "Allow Accessibility"),
                action: Some(
                    "Allow Trek in System Settings › Privacy & Security › Accessibility.".into(),
                ),
                advice: Some(Mirror::NotAllowed.advice()),
                fix: Some(Fix::Settings(Pane::Accessibility)),
            },
            Some(Mirror::MacLocked) => Taps {
                taps: chip("Taps", Tone::Wait, "Unlock iPhone Mirroring"),
                action: Some("iPhone Mirroring wants Touch ID or your password.".into()),
                advice: Some(Mirror::MacLocked.advice()),
                fix: None,
            },
            // Locked and still timing out: locking again won't change it. The diagnosis names
            // what's in the way (shown whether or not someone wants taps: it needs the user);
            // before it's in, the general advice. Either way the line offers to check again.
            Some(Mirror::TimedOut) if self.locked() => match &self.blocker {
                Some(b) => Taps {
                    taps: chip("Taps", Tone::Bad, "iPhone Mirroring can't reach it"),
                    action: Some(b.action()),
                    advice: Some(b.advice()),
                    fix: Some(Fix::CheckAgain),
                },
                None => Taps {
                    taps: chip("Taps", Tone::Bad, "iPhone Mirroring can't reach it"),
                    action: self.wants_taps.then(|| UNREACHABLE.to_string()),
                    advice: Some(Mirror::unreachable_advice().into()),
                    fix: self.wants_taps.then_some(Fix::CheckAgain),
                },
            },
            Some(Mirror::PhoneInUse | Mirror::TimedOut) => lock(Tone::Wait),
            _ if self.usb_live() => lock(Tone::Wait),
            Some(Mirror::Connecting) => Taps::only(chip("Taps", Tone::Wait, "Connecting…")),
            // Its first-run setup, or something else it says: not ready until it's dealt with.
            Some(Mirror::Says(said)) => Taps {
                taps: chip("Taps", Tone::Wait, "Check iPhone Mirroring"),
                action: Some(format!(
                    "iPhone Mirroring says \"{said}\". Finish there, then try again."
                )),
                advice: Some(Mirror::Says(said.clone()).advice()),
                fix: Some(Fix::OpenMirroring),
            },
            // Its window couldn't be read this time: looked at again in a moment.
            Some(Mirror::Unread) => Taps::only(chip("Taps", Tone::Quiet, "Checking…")),
            // Closed (Trek opens it when needed), or not looked at yet.
            Some(Mirror::Closed) | None => {
                Taps::only(chip("Taps", Tone::Good, "Ready · iPhone Mirroring"))
            }
        }
    }
}

/// A route's chip for taps, its action line and advice, and the line's button.
struct Taps {
    taps: Chip,
    action: Option<String>,
    advice: Option<String>,
    fix: Option<Fix>,
}

impl Taps {
    /// Just the chip: nothing to do.
    fn only(taps: Chip) -> Self {
        Taps {
            taps,
            action: None,
            advice: None,
            fix: None,
        }
    }
}

/// How long before the driver's free signing runs out the strip says so (the phone updater
/// renews it two days ahead when it can reach the phone; this is when it hasn't).
pub const RENEW_WARNING: i64 = 24 * 3600;

/// "in 5 hours", for when the driver's signing runs out `left` seconds from now.
fn runs_out(left: i64) -> String {
    match left / 3600 {
        _ if left <= 0 => "now".into(),
        0 => "within the hour".into(),
        1 => "in an hour".into(),
        h => format!("in {h} hours"),
    }
}

/// The chip's word, the line and the button for a driver that isn't there (`why`, from
/// [`crate::driver::availability`]).
fn install_step(why: &str, link: &str) -> (&'static str, String, Fix) {
    if why.contains("lapsed") {
        (
            "Driver expired",
            "Trek Driver's free signing ran out after 7 days. Renew it to tap the iPhone while it's unlocked.".into(),
            Fix::RenewDriver,
        )
    } else if why.contains("cable only") {
        (
            "Driver needs an update",
            "The driver on the iPhone works over the cable only. Update it to tap over Wi-Fi."
                .into(),
            Fix::UpdateDriver,
        )
    } else {
        (
            "Driver not installed",
            format!("Install Trek Driver to tap the iPhone over {link} while it's unlocked."),
            Fix::InstallDriver,
        )
    }
}

// The marketing names iPhone Mirroring calls a phone by: the device services' table.
pub use crate::device::model_name;

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

/// For the iPhone tool's "How this works": the route in use first, then the user's own mouse
/// and keys, the other ways to tap in a line, the screen, apps and files. How it works with no
/// cable comes right after the route while the phone is on Wi-Fi only (`wifi`), else last, and
/// only where Trek Driver is offered (`driver`) or the phone is on Wi-Fi.
pub fn explainer(route: Route, wifi: bool, driver: bool) -> Vec<(&'static str, &'static str)> {
    let screen = (
        "Screen",
        "While the iPhone is unlocked, its screen comes over the USB cable; iOS then shows 9:41 in its status bar, on the iPhone too, and Trek draws the real time over it. While it's locked and Trek taps through iPhone Mirroring, the picture comes from iPhone Mirroring.",
    );
    let mirroring = (
        "Taps through iPhone Mirroring",
        "An iPhone on this Mac's own Apple Account can't take Trek as a Bluetooth keyboard, so Trek taps through iPhone Mirroring, in the background. It connects only while the iPhone is locked and nearby. Choose Authenticate Automatically in iPhone Mirroring's Settings so it doesn't ask for Touch ID each time.", // words: ok, the iPhone tool is macOS only
    );
    let bluetooth = (
        "Taps over Bluetooth",
        "An iPhone on another Apple Account pairs once with Trek as a Bluetooth keyboard and mouse: Settings › Bluetooth, \"Trek\" under Other Devices. Trek turns AssistiveTouch on while it taps and off again after.",
    );
    let usb = (
        "Taps over USB",
        "While the iPhone is unlocked, Trek taps, swipes and types through Trek Driver, a small test runner inside Trek's iPhone app. It starts with the first tap (about 10 seconds) and stops when nobody needs it. Its free signing lasts 7 days; the tool says when to renew it.",
    );
    let others = (
        "Other ways",
        match (route, driver) {
            (Route::Driver, _) => {
                "While the iPhone is locked, taps go through iPhone Mirroring (an iPhone on this Mac's Apple Account) or Bluetooth (another account)."
            }
            (Route::Mirroring, true) => {
                "While the iPhone is unlocked, Trek Driver taps it. An iPhone on another Apple Account can pair with Trek over Bluetooth instead."
            }
            (Route::Mirroring, false) => {
                "An iPhone on another Apple Account can pair with Trek over Bluetooth instead, and then taps while it's unlocked too."
            }
            (Route::Bluetooth, true) => {
                "An iPhone on this Mac's Apple Account taps through iPhone Mirroring instead, and through Trek Driver while it's unlocked."
            }
            (Route::Bluetooth, false) => {
                "An iPhone on this Mac's Apple Account taps through iPhone Mirroring instead, while it's locked."
            }
        },
    );
    let files = (
        "Apps and files",
        "Installing apps and copying files go over the cable (or Wi-Fi), with nothing to set up.",
    );
    let over_wifi = (
        "Over Wi-Fi",
        "With no cable, Trek reaches an iPhone that was plugged in and trusted once, when Show this iPhone when on Wi-Fi is on for it in Finder and Xcode connects to it over the network. While it's unlocked, the picture and taps come through Trek Driver, a little slower than over the cable; while it's locked, taps go through iPhone Mirroring.", // words: ok, the iPhone tool is macOS only
    );
    let hands = (
        "Your mouse and keys",
        "Click the picture to tap, or hold to long-press. A drag goes to the iPhone as you made it, pauses included, when you let go (through iPhone Mirroring, as a straight swipe). Scroll with the trackpad or mouse wheel. After a click, typing goes to the iPhone; Esc or a click outside this tool gives the keys back.",
    );
    let first = match route {
        Route::Driver => usb,
        Route::Mirroring => mirroring,
        Route::Bluetooth => bluetooth,
    };
    let mut out = vec![first, hands, others, screen, files];
    // Right after the route in use while there's no cable; else last.
    if wifi {
        out.insert(1, over_wifi);
    } else if driver {
        out.push(over_wifi);
    }
    out
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
        assert_eq!(strip.route, Route::Driver);
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
        assert_eq!(
            strip.fix,
            Some(Fix::CheckAgain),
            "the line offers to check again"
        );
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
        assert!(strip.advice.unwrap().contains("Revoke iPhone Access"));
        assert_eq!(strip.fix, Some(Fix::CheckAgain));
        // Live again: nothing to do, nothing to check.
        f.mirror = Some(Mirror::Live);
        let strip = f.strip();
        assert_eq!(strip.action, None);
        assert_ne!(strip.fix, Some(Fix::CheckAgain));
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
        // No iPhone Mirroring on this Mac: the Bluetooth route, and its one step.
        let strip = facts(usb_live(), Some(Mirror::Missing)).strip();
        assert_eq!(strip.route, Route::Bluetooth);
        assert_eq!(strip.taps.word, "Pair once in Bluetooth");
        assert_eq!(strip.action.as_deref(), Some(PAIR));
        assert!(strip.advice.unwrap().starts_with("Ask the user to pair"));
        // iPhone Mirroring is here but hasn't shown this phone: it may be on the Mac's own Apple
        // Account, which never sees Trek in Bluetooth, so both ways are offered, Mirroring first.
        let strip = facts(usb_live(), Some(Mirror::Closed)).strip();
        assert_eq!(strip.route, Route::Bluetooth);
        assert_eq!(strip.taps.word, "Not set up");
        assert_eq!(strip.action.as_deref(), Some(CHOOSE));
        assert_eq!(strip.fix, Some(Fix::OpenMirroring));
        assert!(strip.advice.unwrap().contains("this Mac's Apple Account"));
        // Before it's been looked at, the same.
        assert_eq!(facts(usb_live(), None).strip().taps.word, "Not set up");
        // Trek can't read iPhone Mirroring: allowing that comes first.
        let strip = facts(usb_live(), Some(Mirror::NotAllowed)).strip();
        assert_eq!(strip.fix, Some(Fix::Settings(Pane::Accessibility)));
        assert!(strip.action.unwrap().contains("Accessibility"));
        // Paired over Bluetooth: nothing to choose.
        let mut s = usb_live();
        s.hosts = 1;
        assert_eq!(
            facts(s, Some(Mirror::Closed)).strip().taps.word,
            "Ready · Bluetooth"
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
        assert_eq!(strip.route, Route::Driver);
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
        assert_eq!(with_driver(s, Avail::Ready).route(), Route::Driver);
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
        assert_eq!(f.route(), Route::Driver);
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
        assert_ne!(f.route(), Route::Driver);
    }

    #[test]
    fn a_missing_driver_offers_to_install_it() {
        let mut f = with_driver(usb_live(), Avail::Install("it isn't installed yet".into()));
        let strip = f.strip();
        assert_eq!(strip.route, Route::Driver);
        assert_eq!(strip.taps.word, "Driver not installed");
        assert_eq!(
            strip.action.as_deref(),
            Some(
                "Install Trek Driver to tap the iPhone over USB while it's unlocked. Or lock the iPhone to tap through iPhone Mirroring."
            )
        );
        assert_eq!(strip.fix, Some(Fix::InstallDriver));
        let advice = strip.advice.unwrap();
        assert!(advice.contains("Install driver"), "{advice}");
        assert!(advice.contains("lock the iPhone"), "{advice}");
        // No script, anywhere a user or agent reads.
        for state in [
            "it isn't installed yet",
            "its 7-day signing lapsed",
            "the one on the iPhone works over the cable only",
            "it was built for another iPhone",
        ] {
            f.driver = Avail::Install(state.into());
            let strip = f.strip();
            for line in [
                strip.taps.word,
                strip.action.unwrap(),
                strip.advice.unwrap(),
            ] {
                assert!(!line.contains("scripts/"), "{line}");
            }
        }
        // Lapsed: renew; cable-only on Wi-Fi: update.
        f.driver = Avail::Install("its 7-day signing lapsed".into());
        let strip = f.strip();
        assert_eq!(strip.taps.word, "Driver expired");
        assert_eq!(strip.fix, Some(Fix::RenewDriver));
        f.driver = Avail::Install("the one on the iPhone works over the cable only".into());
        assert_eq!(f.strip().fix, Some(Fix::UpdateDriver));
        // The driver's start says to reinstall: the line offers it.
        f.driver = Avail::Ready;
        f.wants_taps = true;
        f.driver_state = Driver::Failed(format!(
            "The iPhone has another Trek Driver (version 0): {REINSTALL}."
        ));
        assert_eq!(f.strip().fix, Some(Fix::ReinstallDriver));
    }

    #[test]
    fn installing_the_driver_says_how_it_goes() {
        let mut f = with_driver(usb_live(), Avail::Install("it isn't installed yet".into()));
        f.setup = Setup::Checking;
        let strip = f.strip();
        assert!(strip.note.unwrap().starts_with("Checking"));
        assert!(Setup::Checking.busy());
        // What this Mac lacks, each with its own next step.
        for (missing, fix) in [
            (Missing::Xcode, Fix::GetXcode),
            (Missing::AppleId, Fix::OpenXcode),
            (Missing::Source, Fix::GetSource),
        ] {
            f.setup = Setup::Missing(missing);
            let strip = f.strip();
            assert_eq!(strip.action.as_deref(), Some(missing.line()));
            assert_eq!(strip.fix, Some(fix));
            assert!(!fix.installs());
        }
        // Building, then installing: the chip and a quiet line, no button.
        f.setup = Setup::Building;
        let strip = f.strip();
        assert_eq!(strip.taps.word, "Installing the driver…");
        assert_eq!((strip.action, strip.fix), (None, None));
        assert!(strip.note.unwrap().contains("few minutes"));
        f.setup = Setup::Installing;
        assert!(f.strip().note.unwrap().contains("unlocked"));
        // It failed: the updater's words, and Try again.
        f.setup = Setup::Failed("Your Mac is busy with another Xcode build".into());
        let strip = f.strip();
        assert_eq!(strip.taps.word, "The driver didn't install");
        assert_eq!(
            strip.action.as_deref(),
            Some("Your Mac is busy with another Xcode build")
        );
        assert_eq!(strip.fix, Some(Fix::TryAgain));
        assert!(Fix::TryAgain.installs() && !Setup::Failed(String::new()).busy());
        // Installed but still no driver (it didn't build): said, with Try again.
        f.setup = Setup::Installed;
        let strip = f.strip();
        assert!(strip.action.unwrap().contains("didn't build"));
        assert_eq!(strip.fix, Some(Fix::TryAgain));
        // Installed with the driver: ready, nothing to do.
        f.driver = Avail::Ready;
        let strip = f.strip();
        assert_eq!(strip.taps.word, "Ready · USB");
        assert_eq!((strip.action, strip.fix), (None, None));
    }

    #[test]
    fn a_driver_about_to_lapse_says_so_with_renew() {
        let mut f = with_driver(usb_live(), Avail::Ready);
        f.driver_left = Some(3 * 86400);
        assert_eq!(f.strip().action, None, "days left: the updater renews it");
        f.driver_left = Some(5 * 3600 + 60);
        let strip = f.strip();
        assert_eq!(strip.taps.word, "Ready · USB");
        assert_eq!(
            strip.action.as_deref(),
            Some("Trek Driver's free signing runs out in 5 hours. Renew it to keep tapping.")
        );
        assert_eq!(strip.fix, Some(Fix::RenewDriver));
        assert_eq!(runs_out(1800), "within the hour");
        assert_eq!(runs_out(3700), "in an hour");
        assert_eq!(runs_out(-5), "now");
    }

    #[test]
    fn no_phone_says_how_to_connect_it() {
        let f = Facts::default();
        let strip = f.strip();
        assert_eq!(strip.screen.word, "No iPhone");
        assert!(strip.action.unwrap().starts_with("Plug the iPhone in"));
        assert_eq!(strip.fix, None);
        // usbmuxd itself didn't answer: that, not "plug it in", with Look again.
        let f = Facts {
            mux_error: Some("Couldn't reach usbmuxd: refused".into()),
            ..Facts::default()
        };
        let strip = f.strip();
        assert!(strip.action.unwrap().contains("Couldn't reach usbmuxd"));
        assert_eq!(strip.fix, Some(Fix::LookAgain));
    }

    #[test]
    fn an_untrusted_phone_says_to_tap_trust() {
        let mut s = usb_live();
        s.screen.clear();
        s.frames = 0;
        let mut f = facts(s, Some(Mirror::Closed));
        f.untrusted = true;
        let strip = f.strip();
        assert_eq!(
            (strip.screen.word.as_str(), strip.screen.tone),
            ("Not trusted", Tone::Wait)
        );
        assert_eq!(strip.action.as_deref(), Some(UNTRUSTED));
        assert_eq!(strip.fix, Some(Fix::LookAgain));
        assert!(strip.advice.unwrap().contains("tap Trust"));
        // Trusted, no screen yet: no Trust line.
        f.untrusted = false;
        assert_ne!(f.strip().action.as_deref(), Some(UNTRUSTED));
    }

    #[test]
    fn a_live_picture_nothing_can_tap_says_its_view_only() {
        // Same Apple Account, no driver on this Mac, unlocked on the cable.
        let mut f = facts(usb_live(), Some(Mirror::Closed));
        f.mirroring_known = true;
        let strip = f.strip();
        assert_eq!(strip.route, Route::Mirroring);
        assert_eq!(strip.taps.word, "Lock the iPhone to tap");
        assert_eq!(strip.note.as_deref(), Some(VIEW_ONLY));
        // After a click: the action line instead.
        f.wants_taps = true;
        let strip = f.strip();
        assert_eq!(strip.action.as_deref(), Some(LOCK));
        assert_eq!(strip.note, None);
        // With the driver offered the route is the driver's: not view-only.
        f.wants_taps = false;
        f.driver = Avail::Ready;
        assert_eq!(f.strip().note, None);
    }

    #[test]
    fn iphone_mirroring_is_ready_only_when_it_is() {
        let mut s = usb_live();
        s.dark = true;
        let mut f = facts(s, Some(Mirror::Closed));
        f.mirroring_known = true;
        assert_eq!(f.strip().taps.word, "Ready · iPhone Mirroring");
        f.mirror = Some(Mirror::Says("Welcome to iPhone Mirroring".into()));
        let strip = f.strip();
        assert_eq!(
            (strip.taps.word.as_str(), strip.taps.tone),
            ("Check iPhone Mirroring", Tone::Wait)
        );
        assert!(
            strip
                .action
                .unwrap()
                .contains("Welcome to iPhone Mirroring")
        );
        assert_eq!(strip.fix, Some(Fix::OpenMirroring));
        f.mirror = Some(Mirror::Unread);
        let strip = f.strip();
        assert_eq!(
            (strip.taps.word.as_str(), strip.taps.tone),
            ("Checking…", Tone::Quiet)
        );
        // Accessibility: the button opens its pane.
        f.mirror = Some(Mirror::NotAllowed);
        assert_eq!(f.strip().fix, Some(Fix::Settings(Pane::Accessibility)));
    }

    #[test]
    fn a_stalled_stream_isnt_shown_as_live() {
        let mut f = facts(usb_live(), Some(Mirror::Closed));
        f.driver = Avail::Ready;
        f.driver_state = Driver::Ready;
        f.driver_frames = true;
        assert_eq!(f.strip().screen.word, "Live");
        f.stalled = true;
        let strip = f.strip();
        assert_eq!(
            (strip.screen.word.as_str(), strip.screen.tone),
            ("Reconnecting…", Tone::Wait)
        );
        f.wifi = true;
        assert_eq!(f.strip().screen.word, "Reconnecting…");
    }

    #[test]
    fn permission_lines_open_their_settings() {
        let mut s = usb_live();
        s.camera = "denied".into();
        assert_eq!(
            facts(s, Some(Mirror::Closed)).strip().fix,
            Some(Fix::Settings(Pane::Camera))
        );
        let mut s = usb_live();
        s.bluetooth = "not allowed".into();
        let strip = facts(s, Some(Mirror::Missing)).strip();
        assert_eq!(strip.fix, Some(Fix::Settings(Pane::Bluetooth)));
        assert!(Pane::Bluetooth.url().ends_with("Privacy_Bluetooth"));
    }

    #[test]
    fn mirroring_counts_only_for_the_phone_it_named() {
        assert!(mirrors("iPhone 16 Pro Max", "tobyj’s iPhone", "iPhone17,2"));
        assert!(mirrors("tobyj's iPhone", "tobyj’s iPhone", "iPhone17,2"));
        assert!(mirrors("", "tobyj’s iPhone", "iPhone17,2"));
        assert!(!mirrors("iPhone 15", "Work phone", "iPhone17,2"));
        assert!(mirrors("iPhone 15", "Work phone", "iPhone99,1"));
        // The iPhone 17 family and Air by name.
        assert!(mirrors("iPhone 17 Pro", "Work phone", "iPhone18,1"));
        assert!(mirrors("iPhone Air", "Work phone", "iPhone18,4"));
        assert!(!mirrors("iPhone 16", "Work phone", "iPhone18,3"));
    }

    #[test]
    fn the_explainer_leads_with_the_route_in_use() {
        assert_eq!(
            explainer(Route::Mirroring, false, false)[0].0,
            "Taps through iPhone Mirroring"
        );
        assert_eq!(
            explainer(Route::Bluetooth, false, false)[0].0,
            "Taps over Bluetooth"
        );
        let usb = explainer(Route::Driver, false, true);
        assert_eq!(usb[0].0, "Taps over USB");
        // How the user's own mouse and keys work comes right after the route, then the others
        // in a line.
        assert_eq!(usb[1].0, "Your mouse and keys");
        assert!(usb[1].1.contains("hold to long-press") && usb[1].1.contains("Esc"));
        assert_eq!(usb[2].0, "Other ways");
        assert!(
            usb.iter()
                .any(|(t, body)| *t == "Screen" && body.contains("9:41"))
        );
        // Wi-Fi last where the driver is offered; with no cable, right after the route in use.
        assert_eq!(usb.last().unwrap().0, "Over Wi-Fi");
        let wifi = explainer(Route::Driver, true, true);
        assert_eq!((wifi[0].0, wifi[1].0), ("Taps over USB", "Over Wi-Fi"));
        assert_eq!(wifi.len(), usb.len());
        assert!(wifi[1].1.contains("Show this iPhone when on Wi-Fi"));
        assert_eq!(explainer(Route::Mirroring, true, false)[1].0, "Over Wi-Fi");
        // Without the driver: no Trek Driver story, and short.
        let plain = explainer(Route::Mirroring, false, false);
        assert!(plain.len() <= 5, "{plain:?}");
        assert!(plain.iter().all(|(_, body)| !body.contains("Trek Driver")));
        // No scripts for anyone.
        for route in [Route::Driver, Route::Mirroring, Route::Bluetooth] {
            for (wifi, driver) in [(false, false), (true, true), (false, true)] {
                for (_, body) in explainer(route, wifi, driver) {
                    assert!(!body.contains("scripts/"), "{body}");
                }
            }
        }
    }

    /// The phone over Wi-Fi only: the helper runs (Bluetooth, iPhone Mirroring) but the cable
    /// has no screen; usbmuxd sees the phone on the network.
    fn on_wifi(avail: Avail, mirror: Option<Mirror>) -> Facts {
        let status = Status {
            camera: "granted".into(),
            bluetooth: "advertising".into(),
            ..Status::default()
        };
        let mut f = facts(status, mirror);
        f.wifi = true;
        f.mirroring_known = true;
        f.driver = avail;
        f
    }

    #[test]
    fn an_unlocked_phone_on_wifi_taps_through_the_driver() {
        // The user's case: no cable, the driver installed, the lock not known yet.
        let mut f = on_wifi(Avail::Ready, Some(Mirror::Closed));
        assert!(!f.locked());
        let strip = f.strip();
        assert_eq!(strip.route, Route::Driver);
        assert_eq!(strip.taps.word, "Ready · Wi-Fi");
        assert_eq!(
            strip.screen.word, "Starting…",
            "the driver's picture is on its way"
        );
        assert_eq!((strip.action, strip.advice), (None, None));
        // Unlocked as CoreDevice says, the driver's picture in.
        f.passcode = Some(false);
        f.driver_state = Driver::Ready;
        f.driver_frames = true;
        let strip = f.strip();
        assert_eq!(strip.route, Route::Driver);
        assert_eq!(
            (strip.screen.word.as_str(), strip.screen.tone),
            ("Live · Wi-Fi", Tone::Good)
        );
        assert_eq!(strip.taps.word, "Ready · Wi-Fi");
        assert!(strip.assistive.is_none());
        // Starting.
        f.driver_frames = false;
        f.driver_state = Driver::Starting;
        assert_eq!(f.strip().taps.word, "Starting the driver…");
        // The camera isn't asked for: nothing comes over a cable.
        f.status.camera = "denied".into();
        let strip = f.strip();
        assert_eq!(strip.screen.word, "Starting…");
        assert_eq!(strip.action, None);
    }

    #[test]
    fn the_cable_wins_when_both_are_there() {
        // Plugged in again: usbmuxd lists the cable first, so the facts aren't Wi-Fi's.
        let mut f = with_driver(usb_live(), Avail::Ready);
        f.passcode = Some(true); // a stale Wi-Fi reading doesn't count on the cable
        assert!(!f.locked());
        let strip = f.strip();
        assert_eq!(strip.route, Route::Driver);
        assert_eq!(strip.taps.word, "Ready · USB");
        f.driver_frames = true;
        assert_eq!(f.strip().screen.word, "Live");
    }

    #[test]
    fn a_locked_phone_on_wifi_goes_to_mirroring_or_says_to_unlock() {
        // Locked, iPhone Mirroring ready: it takes the taps, nothing to do.
        let mut f = on_wifi(Avail::Ready, Some(Mirror::Closed));
        f.passcode = Some(true);
        f.wants_taps = true;
        assert!(f.locked());
        let strip = f.strip();
        assert_eq!(strip.route, Route::Mirroring);
        assert_eq!(strip.screen.word, "Locked");
        assert_eq!(strip.taps.word, "Ready · iPhone Mirroring");
        assert_eq!(strip.action, None);
        // Mirroring showing it: its picture.
        f.mirror = Some(Mirror::Live);
        f.mirror_frames = true;
        assert_eq!(f.strip().screen.word, "Live in iPhone Mirroring");
        // iPhone Mirroring can't reach it: unlocking for the driver is the way, not a recheck.
        f.mirror = Some(Mirror::TimedOut);
        f.mirror_frames = false;
        f.blocker = Some(Blocker::PhoneNotOffering);
        let strip = f.strip();
        assert_eq!(strip.route, Route::Mirroring);
        assert_eq!(strip.taps.word, "iPhone Mirroring can't reach it");
        assert_eq!(strip.action.as_deref(), Some(UNLOCK_WIFI));
        assert_ne!(strip.fix, Some(Fix::CheckAgain));
        assert!(strip.advice.unwrap().contains("unlock it"));
        // Nobody wants taps: no line, the advice stays for agents.
        f.wants_taps = false;
        assert_eq!(f.strip().action, None);
        // No iPhone Mirroring route here and no Bluetooth pairing: unlock, not pair.
        let mut f = on_wifi(Avail::Ready, Some(Mirror::Closed));
        f.mirroring_known = false;
        f.passcode = Some(true);
        f.wants_taps = true;
        let strip = f.strip();
        assert_eq!(strip.route, Route::Bluetooth);
        assert_eq!(strip.taps.word, "Unlock the iPhone to tap");
        assert_eq!(strip.action.as_deref(), Some(UNLOCK_WIFI));
        // Paired over Bluetooth: it taps while locked, nothing to do.
        f.status.hosts = 1;
        f.status.bluetooth = "connected".into();
        let strip = f.strip();
        assert_eq!(strip.taps.word, "Ready · Bluetooth");
        assert_eq!(strip.action, None);
        // Unlocked again: the driver.
        let mut f = on_wifi(Avail::Ready, Some(Mirror::TimedOut));
        f.passcode = Some(false);
        assert_eq!(f.strip().route, Route::Driver);
    }

    #[test]
    fn the_driver_on_wifi_says_what_it_needs() {
        // An older driver (cable only), or none: install, or lock for iPhone Mirroring.
        let mut f = on_wifi(
            Avail::Install("the one on the iPhone works over the cable only".into()),
            Some(Mirror::Closed),
        );
        f.wants_taps = true;
        let strip = f.strip();
        assert_eq!(strip.route, Route::Driver);
        assert_eq!(strip.screen.word, "None over Wi-Fi");
        assert_eq!(strip.taps.word, "Driver needs an update");
        assert_eq!(
            strip.action.as_deref(),
            Some(
                "The driver on the iPhone works over the cable only. Update it to tap over Wi-Fi. Or lock the iPhone to tap through iPhone Mirroring."
            )
        );
        assert_eq!(strip.fix, Some(Fix::UpdateDriver));
        assert!(
            strip
                .advice
                .unwrap()
                .starts_with("Taps over Wi-Fi need Trek Driver")
        );
        // Xcode can't reach it over the network: the reason, and iPhone Mirroring once locked.
        let mut f = on_wifi(Avail::Ready, Some(Mirror::Closed));
        f.wants_taps = true;
        f.driver_state = Driver::Failed(
            "Xcode can't reach the iPhone: plug it in once (Xcode then reaches it over Wi-Fi too), or put both on the same Wi-Fi.".into(),
        );
        let strip = f.strip();
        assert_eq!(strip.taps.word, "The driver didn't start");
        assert_eq!(strip.screen.word, "None over Wi-Fi");
        let action = strip.action.unwrap();
        assert!(
            action.starts_with("Xcode can't reach the iPhone"),
            "{action}"
        );
        assert!(action.ends_with("Or lock the iPhone to tap through iPhone Mirroring."));
        // Locked as the start found: unlocking is the one thing.
        f.driver_state = Driver::Failed(
            "Unlock the iPhone: Trek Driver starts only while it's unlocked.".into(),
        );
        assert!(!f.strip().action.unwrap().contains("Or lock"));
        // A Mac that never built Trek's iPhone app: the routes without the driver, no picture.
        let f = on_wifi(Avail::None, Some(Mirror::Closed));
        let strip = f.strip();
        assert_eq!(strip.route, Route::Mirroring);
        assert_eq!(strip.screen.word, "None over Wi-Fi");
        assert_eq!(strip.taps.word, "Ready · iPhone Mirroring");
    }
}
