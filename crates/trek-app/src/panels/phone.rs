//! iPhone: the user's real iPhone beside the chat, live, and agents driving it.
//!
//! The screen comes over the USB cable as QuickTime gets it and taps and keys go as a Bluetooth
//! keyboard and mouse the phone pairs with once (`trek_phone::helper`, one process for both);
//! apps and files go over usbmuxd as Finder's do (`trek_phone::device`). No Xcode, no Developer
//! Mode, nothing installed on the phone.
//!
//! While the phone is unlocked on the cable and Trek's iPhone app was built here with Trek Driver
//! inside (`ios/scripts/install-device.sh`), taps, swipes and keys go over USB through the driver
//! instead (`trek_phone::driver`), started on the first tap and stopped when the agent lets go.
//! With no cable (usbmuxd sees the phone on Wi-Fi only) the driver does the same over Wi-Fi and
//! gives the picture too, while the phone is unlocked; whether it is comes from CoreDevice
//! (`driver::passcode_required`), and a locked one goes to iPhone Mirroring.
//! While the cable's picture is the screen, iOS draws 9:41 in its status bar (as for QuickTime);
//! the panel draws the phone's real time over it.
//!
//! A phone on the Mac's own Apple Account never takes the Mac as a Bluetooth keyboard (it's
//! already bonded for Continuity), so its taps and keys go through macOS's iPhone Mirroring
//! window instead, in the background (`trek_phone::mirror`), and the picture comes from that
//! window while it shows the phone. Mirroring connects only while the phone is locked; while
//! it's unlocked the picture comes over USB and the panel says to lock it to tap.
//!
//! Agents' `trek-mcp phone` calls come here one at a time (`RightPanel::phone_call`). One thread
//! holds the phone at a time (`trek_phone::lease`); a strip says who, with Stop to take it back,
//! as the Browser's does. Installing and changing files ask first by the thread's access level,
//! on its approval card (`Workspace::browser_ask`). Trek turns AssistiveTouch on when a tap needs
//! a pointer and off again when the agent lets go or the user takes the phone back.

use crate::assets::Lucide;
use crate::fluent;
use crate::scale::{text, to_rems};
use crate::workspace::Workspace;
use crate::workspace::phone_use::PhoneCall;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use trek_agents::Decision;
use trek_core::HandHolding;
use trek_ipc::phone::{self as tools, Sensitive};
use trek_phone::diagnose::{self, Blocker};
use trek_phone::driver::{self, Driver};
use trek_phone::helper::{self, Event, Link, MirrorCmd, Mirrored, Status};
use trek_phone::hid::{self, Step};
use trek_phone::lease::{Lease, TOOK_OVER};
use trek_phone::mirror::{self, Settle, State as MirrorState};
use trek_phone::route::{self, Facts, Route, Tone};
use trek_phone::{device, files};

/// Frames for the panel are at most this wide, and come this often at most.
const FRAME_WIDTH: u32 = 900;
const LIVE_FPS: u32 = 15;
const TICK: Duration = Duration::from_secs(1);
const DEVICE_REFRESH: Duration = Duration::from_secs(5);
const STRIP_LINGER: Duration = Duration::from_secs(12);
const ASK_WAIT: Duration = Duration::from_secs(15 * 60);
/// A screenshot's longest side by default, and at most.
const SHOT_EDGE: u32 = 1200;
const SHOT_MAX: u32 = 2000;
/// Pointer travel (window px) below which a press is a tap.
const TAP_SLOP: f32 = 6.;
/// How often iPhone Mirroring's window is read while Bluetooth can't tap.
const MIRROR_LOOK: Duration = Duration::from_secs(3);
/// How long iPhone Mirroring gets to show the phone once opened or told to connect.
/// (It gives up on its own after ~12 s; Trek presses Connect up to `mirror::PRESSES` times.)
const MIRROR_CONNECT: Duration = Duration::from_secs(75);
/// How often a lasting iPhone Mirroring time-out is diagnosed again (the user may have fixed it).
const DIAGNOSE_AGAIN: Duration = Duration::from_secs(60);

/// How long after the user tapped the picture (and the tap couldn't reach the phone) the strip
/// keeps saying what to do about it.
const WANT_TAPS: Duration = Duration::from_secs(60);

/// How often a frame from the cable is read for iOS's status bar.
const BAR_EVERY: Duration = Duration::from_millis(500);

/// Trek Driver stops after this long without a tap while no agent holds the phone.
const DRIVER_IDLE: Duration = Duration::from_secs(10 * 60);
/// How often CoreDevice is asked whether the phone is locked while it's on Wi-Fi only (each ask
/// takes ~1.5 s, in the background).
const PASSCODE_ASK: Duration = Duration::from_secs(8);
/// How long the picture waits after the driver failed to start before starting it again (each
/// try runs xcodebuild for seconds; the strip says why meanwhile).
const DRIVER_RETRY: Duration = Duration::from_secs(20);
/// How often the picture is asked of Trek Driver while it runs (a screenshot takes it ~0.3 s),
/// and the JPEG quality it comes at.
const DRIVER_FRAME: Duration = Duration::from_millis(350);
const DRIVER_JPEG: u8 = 70;

/// Where Trek remembers that iPhone Mirroring has shown an iPhone here, and its name: the route
/// then stays iPhone Mirroring while its window is closed.
fn mirroring_marker() -> PathBuf {
    trek_core::paths::data_dir().join("phone-mirroring")
}

/// What needs the user's yes, by the thread's access level: installing an app unless the thread
/// has full access; writing or removing files unless it's on Auto or full access.
pub fn needs_asking(kind: Sensitive, access: HandHolding) -> bool {
    match access {
        HandHolding::FullAccess => false,
        HandHolding::Auto => kind == Sensitive::Install,
        HandHolding::Supervised | HandHolding::AutoAcceptEdits => true,
    }
}

/// What the strip shows: who's at it and what they did last.
struct Activity {
    title: String,
    action: String,
    at: Instant,
}

struct Frame {
    image: Arc<RenderImage>,
    width: f32,
    height: f32,
    /// The status bar iOS drew in it (with its 9:41), when one shows.
    bar: Option<StatusBar>,
    /// From Trek Driver: the clock in it is the real one.
    real_clock: bool,
}

pub struct PhonePanel {
    workspace: Entity<Workspace>,
    visible: bool,
    helper: Option<Result<PathBuf, String>>,
    helper_building: bool,
    link: Option<Arc<Link>>,
    _link_events: Option<Task<()>>,
    link_retry: Option<Instant>,
    status: Status,
    phone: Option<device::Phone>,
    phone_error: Option<String>,
    refreshing: bool,
    refreshed: Option<Instant>,
    /// AssistiveTouch as last read, and whether Trek turned it on (and so turns it off again).
    assistive: Option<bool>,
    at_by_trek: bool,
    frame: Option<Frame>,
    stale: Vec<Arc<RenderImage>>,
    /// The newest undecoded frame, for the decoder thread.
    decode_slot: Arc<Mutex<Option<Vec<u8>>>>,
    decode_wake: async_channel::Sender<(u32, u32, bool)>,
    /// The picture comes from Trek Driver (`start_driver_frames`), the cable's capture off: the
    /// iPhone shows its real clock then (captured, it shows 9:41, on the phone too).
    driver_frames: bool,
    /// True while the loop asking the driver for frames runs; it clears it when it ends.
    driver_frames_live: Arc<AtomicBool>,
    driver_frames_task: Option<Task<()>>,
    /// When the picture's last start of the driver failed.
    driver_frames_retry: Option<Instant>,
    area: Rc<Cell<Bounds<Pixels>>>,
    press: Option<Point<Pixels>>,
    lease: Lease,
    queue: VecDeque<PhoneCall>,
    running: bool,
    /// Bumped by Stop: a call under way checks it between steps.
    stops: u64,
    last: Option<Activity>,
    allowed: HashSet<(String, Sensitive)>,
    /// The latest screenshot's size: agents' x/y are pixels in it.
    shot_size: Option<(f64, f64)>,
    /// Whether that screenshot was iPhone Mirroring's window rather than the cable's picture.
    shot_mirrored: bool,
    shots: u64,
    /// iPhone Mirroring as last read and settled, while Bluetooth can't tap (`None` before the
    /// first look).
    mirror: Option<MirrorState>,
    settle: Settle,
    mirror_looked: Option<Instant>,
    mirror_looking: bool,
    /// Whether the frames shown come from iPhone Mirroring's window (told to the helper).
    mirror_frames: bool,
    /// The iPhone iPhone Mirroring has shown on this Mac ("" when it didn't say which), once seen.
    mirroring_seen: Option<String>,
    /// When the user last tapped the picture without the tap reaching the phone.
    wanted_taps: Option<Instant>,
    explain_open: bool,
    /// Trek Driver, for taps over the cable or Wi-Fi while the phone is unlocked.
    driver: Arc<Driver>,
    driver_install: Option<driver::Install>,
    /// Trek's iPhone app was installed on this phone from this Mac (the driver is then offered).
    driver_built_here: bool,
    /// The driver's state as the strip last showed it.
    driver_shown: driver::State,
    /// A made-up driver for design review.
    demo_driver: Option<(driver::Avail, driver::State)>,
    /// Over Wi-Fi (no cable), whether the phone needs its passcode now, as CoreDevice last said
    /// (`driver::passcode_required`): over the cable its black picture says it.
    passcode: Option<bool>,
    passcode_asked: Option<Instant>,
    passcode_asking: bool,
    /// The phone's clock settings, for the status bar drawn over iOS's 9:41.
    clock: Option<device::Clock>,
    /// The time last drawn there (redrawn when the minute turns).
    clock_shown: Option<String>,
    /// What's in the way of iPhone Mirroring (`diagnose`), and when that was found; `None` once
    /// it should be looked at again (it just timed out, or the user asked to check again).
    blocker: Option<Blocker>,
    diagnosed: Option<Instant>,
    diagnosing: bool,
    /// A made-up phone for design review (`control phone <state>`): nothing real is touched.
    demo: bool,
    notice: Option<SharedString>,
    _tasks: Vec<Task<()>>,
}

impl PhonePanel {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Frames decode on a thread of their own; only the newest waits.
        let decode_slot: Arc<Mutex<Option<Vec<u8>>>> = Arc::default();
        let (wake_tx, wake_rx) = async_channel::bounded::<(u32, u32, bool)>(1);
        let (frames_tx, frames_rx) = async_channel::bounded::<Frame>(2);
        let (slot, renderer) = (decode_slot.clone(), cx.svg_renderer());
        let _ = std::thread::Builder::new()
            .name("trek-phone-decode".into())
            .spawn(move || {
                // Where iOS's status bar is, looked for twice a second (it moves only when the
                // app in front changes).
                let mut bar = None;
                let mut bar_at: Option<Instant> = None;
                while let Ok((w, h, real_clock)) = wake_rx.recv_blocking() {
                    let Some(jpeg) = slot.lock().unwrap().take() else {
                        continue;
                    };
                    if real_clock {
                        // The driver's picture: nothing to cover.
                        bar = None;
                        bar_at = None;
                    } else if bar_at.is_none_or(|t| t.elapsed() >= BAR_EVERY) {
                        bar = status_bar(&jpeg);
                        bar_at = Some(Instant::now());
                    }
                    // The cable says the screen's size with each frame; the driver's picture
                    // is its own size, read from its header (it's decoded once, just below).
                    let (w, h) = if w == 0 || h == 0 {
                        match picture_size(&jpeg) {
                            Some(size) => size,
                            None => continue,
                        }
                    } else {
                        (w, h)
                    };
                    let image = Image::from_bytes(ImageFormat::Jpeg, jpeg);
                    if let Ok(image) = image.to_image_data(renderer.clone()) {
                        let frame = Frame {
                            image,
                            width: w as f32,
                            height: h as f32,
                            bar,
                            real_clock,
                        };
                        if frames_tx.send_blocking(frame).is_err() {
                            break;
                        }
                    }
                }
            });
        let frames = cx.spawn_in(window, async move |this, cx| {
            while let Ok(frame) = frames_rx.recv().await {
                if this
                    .update_in(cx, |this, window, cx| this.show_frame(frame, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let tick = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this
                    .update_in(cx, |this, window, cx| this.tick(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            workspace,
            visible: false,
            helper: None,
            helper_building: false,
            link: None,
            _link_events: None,
            link_retry: None,
            status: Status::default(),
            phone: None,
            phone_error: None,
            refreshing: false,
            refreshed: None,
            assistive: None,
            at_by_trek: false,
            frame: None,
            stale: vec![],
            decode_slot,
            decode_wake: wake_tx,
            area: Rc::new(Cell::new(Bounds::default())),
            press: None,
            lease: Lease::default(),
            queue: VecDeque::new(),
            running: false,
            stops: 0,
            last: None,
            allowed: HashSet::new(),
            shot_size: None,
            shot_mirrored: false,
            shots: 0,
            mirror: None,
            settle: Settle::default(),
            mirror_looked: None,
            mirror_looking: false,
            mirror_frames: false,
            driver_frames: false,
            driver_frames_live: Arc::new(AtomicBool::new(false)),
            driver_frames_task: None,
            driver_frames_retry: None,
            mirroring_seen: std::fs::read_to_string(mirroring_marker())
                .ok()
                .map(|s| s.trim().to_string()),
            wanted_taps: None,
            explain_open: false,
            driver: Arc::new(Driver::new()),
            driver_install: None,
            driver_built_here: false,
            driver_shown: driver::State::Off,
            demo_driver: None,
            passcode: None,
            passcode_asked: None,
            passcode_asking: false,
            clock: None,
            clock_shown: None,
            blocker: None,
            diagnosed: None,
            diagnosing: false,
            // An isolated capture run never reaches for the user's real phone.
            demo: cfg!(feature = "shots") && std::env::var_os("TREK_SHOT_DIR").is_some(),
            notice: None,
            _tasks: vec![frames, tick],
        }
    }

    /// What the strip and `phone_status` are decided from.
    fn facts(&self) -> Facts {
        let other = self.mirror_other();
        Facts {
            phone: self.phone.is_some(),
            helper: self.link.is_some() || self.demo,
            status: self.status.clone(),
            mirror: self.mirror.clone().filter(|_| !other),
            mirroring_known: self.mirroring_seen.is_some() && !other,
            mirror_frames: self.mirror_frames,
            driver_frames: self.driver_frames,
            assistive: self.assistive,
            wants_taps: self.busy()
                || self.lease.holder(Instant::now()).is_some()
                || self.wanted_taps.is_some_and(|t| t.elapsed() < WANT_TAPS),
            driver: self.driver_avail(),
            driver_state: match &self.demo_driver {
                Some((_, state)) => state.clone(),
                None => self.driver.state(),
            },
            blocker: self.blocker.clone(),
            wifi: self.on_wifi(),
            passcode: self.passcode.filter(|_| self.on_wifi()),
        }
    }

    /// Whether usbmuxd sees the phone over Wi-Fi only (no cable).
    fn on_wifi(&self) -> bool {
        self.phone.as_ref().is_some_and(|p| !p.usb)
    }

    /// Whether Trek Driver can be used with the phone, on the cable or over Wi-Fi.
    fn driver_avail(&self) -> driver::Avail {
        if let Some((avail, _)) = &self.demo_driver {
            return avail.clone();
        }
        let Some(phone) = self.phone.as_ref() else {
            return driver::Avail::None;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        driver::availability(
            self.driver_install.as_ref(),
            &phone.udid,
            self.driver_built_here,
            now,
            phone.usb,
        )
    }

    /// Over Wi-Fi with the driver on offer, ask CoreDevice now and then whether the phone is
    /// locked (the route goes to iPhone Mirroring while it is); on the cable, forget it.
    fn ask_passcode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let udid = match &self.phone {
            Some(p) if !p.usb => p.udid.clone(),
            _ => {
                self.passcode = None;
                return;
            }
        };
        if self.passcode_asking
            || self.driver_avail() == driver::Avail::None
            || self
                .passcode_asked
                .is_some_and(|t| t.elapsed() < PASSCODE_ASK)
        {
            return;
        }
        self.passcode_asking = true;
        self.passcode_asked = Some(Instant::now());
        self.background(
            window,
            cx,
            move || driver::passcode_required(&udid),
            |this, passcode, _, cx| {
                this.passcode_asking = false;
                if this.passcode != passcode {
                    this.passcode = passcode;
                    cx.notify();
                }
            },
        );
    }

    /// Whether iPhone Mirroring is set up with another iPhone of the account than this one: it
    /// isn't this one's route then, and must not get its taps.
    fn mirror_other(&self) -> bool {
        self.mirroring_seen.as_deref().is_some_and(|seen| {
            self.phone
                .as_ref()
                .is_some_and(|p| !route::mirrors(seen, &p.name, &p.model))
        })
    }

    /// Remember that iPhone Mirroring has an iPhone here (`name`, when it said which).
    fn remember_mirroring(&mut self, name: Option<String>) {
        let name = name.unwrap_or_default();
        let known = self.mirroring_seen.as_deref();
        if known == Some(name.as_str()) || (name.is_empty() && known.is_some()) || self.demo {
            return;
        }
        let _ = std::fs::write(mirroring_marker(), &name);
        self.mirroring_seen = Some(name);
    }

    /// Shown or hidden by the right panel: frames stream only while it's on screen.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible {
            self.stop_driver_frames(cx);
        }
        if let Some(link) = &self.link {
            link.frames(visible && !self.driver_frames, LIVE_FPS);
        }
        if visible {
            self.refreshed = None;
        }
        cx.notify();
    }

    /// The tab is closing: the helper stops (the Bluetooth keyboard goes away), AssistiveTouch
    /// goes back off if Trek turned it on, and calls waiting fail.
    pub fn release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = false;
        for call in self.queue.drain(..) {
            call.answer(Err("The user closed Trek's iPhone tool.".into()));
        }
        self.restore_assistive(cx);
        self.stop_driver_frames(cx);
        self.stop_driver(cx);
        self.link = None;
        self._link_events = None;
        for image in self
            .stale
            .drain(..)
            .chain(self.frame.take().map(|f| f.image))
        {
            cx.drop_image(image, Some(&mut *window));
        }
    }

    fn busy(&self) -> bool {
        self.running || !self.queue.is_empty()
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.demo {
            let state = self.driver.state();
            if state != self.driver_shown {
                self.driver_shown = state.clone();
                cx.notify();
            }
            let held = self.busy() || self.lease.holder(Instant::now()).is_some();
            if state == driver::State::Ready
                && !held
                && self.driver.idle().is_some_and(|i| i >= DRIVER_IDLE)
            {
                self.stop_driver(cx);
            }
        }
        if self.visible
            && self.clock_shown.is_some()
            && self.clock_shown != Some(phone_time(self.clock, chrono::Utc::now()))
        {
            cx.notify();
        }
        if !self.demo {
            self.manage_driver_frames(cx);
        }
        let wanted = self.visible || self.busy() || self.lease.holder(Instant::now()).is_some();
        if !wanted || self.demo {
            return;
        }
        self.manage_link(window, cx);
        if self.refreshed.is_none_or(|t| t.elapsed() >= DEVICE_REFRESH) {
            self.refresh_device(window, cx);
        }
        self.ask_passcode(window, cx);
        if let Some(link) = &self.link {
            link.ask_status();
        }
        if !self.status.input_ready()
            && self
                .mirror_looked
                .is_none_or(|t| t.elapsed() >= MIRROR_LOOK)
        {
            self.look_mirror(window, cx);
        }
        self.diagnose_mirror(window, cx);
        // The "do this now" line goes when the user's tap is a minute old.
        if self
            .wanted_taps
            .is_some_and(|t| t.elapsed() >= WANT_TAPS && t.elapsed() < WANT_TAPS + TICK)
        {
            cx.notify();
        }
        // The strip goes a while after the last call.
        if self.last.as_ref().is_some_and(|a| {
            !self.busy() && a.at.elapsed() >= STRIP_LINGER && a.at.elapsed() < STRIP_LINGER + TICK
        }) {
            cx.notify();
        }
    }

    /// Read iPhone Mirroring's window (a few Accessibility calls in the helper).
    fn look_mirror(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(link) = self.link.clone().filter(|_| !self.mirror_looking) else {
            return;
        };
        self.mirror_looking = true;
        self.mirror_looked = Some(Instant::now());
        let rx = link.mirror(&MirrorCmd::Look);
        cx.spawn_in(window, async move |this, cx| {
            let answer = rx.recv().await.ok();
            let _ = this.update(cx, |this, cx| {
                this.mirror_looking = false;
                if let Some(m) = answer {
                    this.see_mirror(&m.look, cx);
                }
            });
        })
        .detach();
    }

    /// What iPhone Mirroring showed just now: settled (a blank moment between its screens isn't
    /// the phone's), and remembered once it shows an iPhone is set up here.
    fn see_mirror(&mut self, look: &mirror::Look, cx: &mut Context<Self>) {
        match self.settle.see(MirrorState::of(look), Instant::now()) {
            Some(state) => {
                if state.set_up() {
                    self.remember_mirroring(look.device());
                }
                self.set_mirror(state, cx)
            }
            // Look again soon: it's blank, and may be the phone's screen.
            None => self.mirror_looked = None,
        }
    }

    /// iPhone Mirroring's state: while it shows the phone, the picture comes from its window.
    fn set_mirror(&mut self, state: MirrorState, cx: &mut Context<Self>) {
        let live = state == MirrorState::Live && !self.status.input_ready();
        if live != self.mirror_frames
            && let Some(link) = &self.link
        {
            link.mirror_frames(live);
            self.mirror_frames = live;
        }
        if self.mirror.as_ref() != Some(&state) {
            if state == MirrorState::TimedOut {
                self.diagnosed = None;
            }
            self.mirror = Some(state);
            cx.notify();
        }
    }

    /// iPhone Mirroring timed out with the phone locked: find out why (on the Mac's side, a few
    /// seconds off the UI thread), once per time-out and again each minute while it lasts.
    fn diagnose_mirror(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let facts = self.facts();
        if self.diagnosing
            || self.demo
            || facts.route() != Route::Mirroring
            || facts.mirror != Some(MirrorState::TimedOut)
            || !facts.locked()
            || self.diagnosed.is_some_and(|t| t.elapsed() < DIAGNOSE_AGAIN)
        {
            return;
        }
        self.diagnosing = true;
        let udid = self.phone.as_ref().map(|p| p.udid.clone());
        let bluetooth = match self.status.bluetooth.as_str() {
            "off" => Some(false),
            "" | "starting" | "not allowed" | "unsupported" | "unavailable" => None,
            _ => Some(true),
        };
        self.background(
            window,
            cx,
            move || diagnose::run(udid.as_deref(), Some(true), bluetooth),
            |this, d, _, cx| {
                this.diagnosing = false;
                this.diagnosed = Some(Instant::now());
                tracing::info!("iPhone Mirroring can't reach the phone: {:?}", d.checks);
                this.blocker = Some(d.blocker);
                cx.notify();
            },
        );
    }

    /// Run blocking work off the UI thread, then apply its result.
    fn background<R: Send + 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(&mut Self, R, &mut Window, &mut Context<Self>) + 'static,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            let _ = this.update_in(cx, |this, window, cx| done(this, result, window, cx));
        })
        .detach();
    }

    fn refresh_device(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        self.background(
            window,
            cx,
            || {
                let phones = device::list();
                let first = phones.as_ref().ok().and_then(|p| p.first());
                let assistive = first.map(|p| device::assistive_touch(Some(&p.udid)).ok());
                let clock = first.and_then(|p| device::clock(Some(&p.udid)).ok());
                let built_here = first.is_some_and(|p| driver::built_here(&p.udid));
                let install = driver::Install::read(&driver::dir());
                (phones, assistive.flatten(), (clock, built_here, install))
            },
            |this, (phones, assistive, (clock, built_here, install)), _, cx| {
                this.clock = clock.or(this.clock);
                this.driver_built_here = built_here;
                this.driver_install = install;
                // Gone, or unplugged while it ran for the cable (it serves the phone's loopback
                // only, which Wi-Fi can't reach): the driver (and its xcodebuild) goes rather than
                // waiting out its idle; over Wi-Fi it starts again for the network.
                let usb = phones.as_ref().ok().and_then(|p| p.first().map(|p| p.usb));
                if usb.is_none() || (usb == Some(false) && this.driver.wifi() == Some(false)) {
                    this.stop_driver(cx);
                }
                this.refreshing = false;
                this.refreshed = Some(Instant::now());
                match phones {
                    Ok(p) => {
                        this.phone = p.into_iter().next();
                        this.phone_error = None;
                    }
                    Err(e) => {
                        this.phone = None;
                        this.phone_error = Some(e);
                    }
                }
                this.assistive = assistive;
                this.aim();
                cx.notify();
            },
        );
    }

    /// Tell the helper which phone to drive: the user's other devices (an Apple Watch) take
    /// Trek's keyboard too, and only the phone may count as paired or get its reports.
    fn aim(&self) {
        if let (Some(link), Some(phone)) = (&self.link, &self.phone) {
            link.target(&phone.name);
        }
    }

    /// Build the helper once, then keep it running while the tool is wanted.
    fn manage_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.link.is_some()
            || self
                .link_retry
                .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        match &self.helper {
            None if !self.helper_building => {
                self.helper_building = true;
                self.background(
                    window,
                    cx,
                    || {
                        let exe = std::env::current_exe().ok();
                        let exe_dir = exe.as_deref().and_then(Path::parent);
                        helper::ensure(exe_dir, &trek_core::paths::data_dir().join("bin"))
                    },
                    |this, result, window, cx| {
                        this.helper_building = false;
                        if let Err(e) = &result {
                            tracing::warn!("iPhone helper unavailable: {e}");
                        }
                        this.helper = Some(result);
                        this.manage_link(window, cx);
                        cx.notify();
                    },
                );
            }
            Some(Ok(path)) => match Link::start(path, FRAME_WIDTH) {
                Ok((link, events)) => {
                    link.frames(self.visible, LIVE_FPS);
                    self.link = Some(link);
                    self.aim();
                    self._link_events = Some(cx.spawn_in(window, async move |this, cx| {
                        while let Ok(event) = events.recv().await {
                            let done = matches!(event, Event::Exited);
                            if this
                                .update_in(cx, |this, window, cx| {
                                    this.link_event(event, window, cx)
                                })
                                .is_err()
                                || done
                            {
                                break;
                            }
                        }
                    }));
                }
                Err(e) => {
                    self.link_retry = Some(Instant::now());
                    self.notice = Some(e.into());
                }
            },
            _ => {}
        }
    }

    fn link_event(&mut self, event: Event, _window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Frame {
                jpeg,
                width,
                height,
            } => {
                if self.driver_frames {
                    return;
                }
                *self.decode_slot.lock().unwrap() = Some(jpeg);
                let _ = self.decode_wake.try_send((width, height, false));
            }
            Event::Status(s) => {
                if s != self.status {
                    self.status = s;
                    cx.notify();
                }
            }
            Event::Error(e) => {
                self.notice = Some(e.into());
                cx.notify();
            }
            Event::Exited => {
                self.link = None;
                self._link_events = None;
                self.link_retry = Some(Instant::now());
                self.status = Status::default();
                self.mirror_frames = false;
                self.mirror_looking = false;
                cx.notify();
            }
        }
    }

    fn show_frame(&mut self, frame: Frame, window: &mut Window, cx: &mut Context<Self>) {
        if frame.real_clock && self.driver_frames_task.is_none() {
            // A frame of a loop already stopped.
            return;
        }
        if frame.real_clock && !self.driver_frames {
            self.driver_frames = true;
            if let Some(link) = &self.link {
                link.frames(false, LIVE_FPS);
            }
        }
        for old in self.stale.drain(..) {
            cx.drop_image(old, Some(&mut *window));
        }
        if let Some(old) = self.frame.replace(frame) {
            self.stale.push(old.image);
        }
        if self.visible {
            cx.notify();
        }
    }

    /// AssistiveTouch on (a mouse shows a pointer only then); remembers that Trek did it.
    fn ensure_assistive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistive == Some(true) {
            return;
        }
        let udid = self.phone.as_ref().map(|p| p.udid.clone());
        self.background(
            window,
            cx,
            move || device::set_assistive_touch(udid.as_deref(), true),
            |this, result, _, cx| {
                match result {
                    Ok(()) => {
                        this.assistive = Some(true);
                        this.at_by_trek = true;
                    }
                    Err(e) => this.notice = Some(format!("AssistiveTouch: {e}").into()),
                }
                cx.notify();
            },
        );
    }

    /// AssistiveTouch back off, if Trek turned it on.
    fn restore_assistive(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.at_by_trek) {
            return;
        }
        self.assistive = Some(false);
        let udid = self.phone.as_ref().map(|p| p.udid.clone());
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = device::set_assistive_touch(udid.as_deref(), false) {
                    tracing::warn!("couldn't turn AssistiveTouch back off: {e}");
                }
            })
            .detach();
    }

    /// The picture from Trek Driver while it can run (the phone unlocked, on the cable or over
    /// Wi-Fi, the driver installed, the panel on screen): started and stopped as that changes,
    /// and stopped when its loop ended (the driver went away). After a start that failed it
    /// waits `DRIVER_RETRY` before starting again by itself (a tap starts it at once).
    fn manage_driver_frames(&mut self, cx: &mut Context<Self>) {
        let wanted = self.visible
            && !self.mirror_frames
            && self.driver_avail() == driver::Avail::Ready
            && self.facts().route() == Route::Driver;
        let waiting = self
            .driver_frames_retry
            .is_some_and(|t| t.elapsed() < DRIVER_RETRY)
            && matches!(self.driver.state(), driver::State::Failed(_));
        if wanted && !waiting && self.driver_frames_task.is_none() {
            self.start_driver_frames(cx);
        } else if self.driver_frames_task.is_some()
            && (!wanted || !self.driver_frames_live.load(Ordering::Relaxed))
        {
            if matches!(self.driver.state(), driver::State::Failed(_)) {
                self.driver_frames_retry = Some(Instant::now());
            }
            self.stop_driver_frames(cx);
        }
    }

    /// A loop asking the driver (started if need be) for the screen a few times a second; its
    /// first frame switches the picture over (`show_frame`) and the cable's capture off, so the
    /// iPhone shows its real clock.
    fn start_driver_frames(&mut self, cx: &mut Context<Self>) {
        let (Some(install), Some(udid)) = (
            self.driver_install.clone(),
            self.phone.as_ref().map(|p| p.udid.clone()),
        ) else {
            return;
        };
        let live = Arc::new(AtomicBool::new(true));
        self.driver_frames_live = live.clone();
        let d = self.driver.clone();
        let slot = self.decode_slot.clone();
        let wake = self.decode_wake.clone();
        let executor = cx.background_executor().clone();
        self.driver_frames_task = Some(cx.background_executor().spawn(async move {
            if d.ensure(&install, &udid, driver::START_LIMIT).is_err() {
                live.store(false, Ordering::Relaxed);
                return;
            }
            while live.load(Ordering::Relaxed) {
                let started = Instant::now();
                match d.screenshot(FRAME_WIDTH, DRIVER_JPEG) {
                    Ok(jpeg) => {
                        *slot.lock().unwrap() = Some(jpeg);
                        let _ = wake.try_send((0, 0, true));
                    }
                    Err(_) => break,
                }
                if let Some(rest) = DRIVER_FRAME.checked_sub(started.elapsed()) {
                    executor.timer(rest).await;
                }
            }
            live.store(false, Ordering::Relaxed);
        }));
    }

    /// The cable's capture takes the picture over again.
    fn stop_driver_frames(&mut self, cx: &mut Context<Self>) {
        self.driver_frames_live.store(false, Ordering::Relaxed);
        self.driver_frames_task = None;
        if self.driver_frames {
            self.driver_frames = false;
            if let Some(link) = &self.link {
                link.frames(self.visible, LIVE_FPS);
            }
            // Over Wi-Fi no picture from the cable follows: the last one mustn't pass for live.
            if self.on_wifi()
                && let Some(old) = self.frame.take()
            {
                self.stale.push(old.image);
            }
            cx.notify();
        }
    }

    /// Trek Driver stops (in the background: it asks the phone, then ends xcodebuild).
    fn stop_driver(&mut self, cx: &mut Context<Self>) {
        if self.demo
            || matches!(
                self.driver.state(),
                driver::State::Off | driver::State::Failed(_)
            )
        {
            return;
        }
        let d = self.driver.clone();
        cx.background_executor()
            .spawn(async move { d.stop() })
            .detach();
    }

    /// The user's tap or key over the cable: the driver started if need be, then `steps` (made
    /// from the screen's size in points) played.
    fn usb_input(
        &mut self,
        steps: impl FnOnce((f64, f64)) -> Vec<driver::Step> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(install), Some(udid), driver::Avail::Ready) = (
            self.driver_install.clone(),
            self.phone.as_ref().map(|p| p.udid.clone()),
            self.driver_avail(),
        ) else {
            self.cant_tap(cx);
            return;
        };
        let d = self.driver.clone();
        cx.notify();
        self.background(
            window,
            cx,
            move || {
                let screen = d.ensure(&install, &udid, driver::START_LIMIT)?;
                driver::run(&*d, &steps(screen))
            },
            |this, result, _, cx| {
                if let Err(e) = result {
                    this.notice = Some(e.into());
                }
                cx.notify();
            },
        );
    }

    // MARK: the user's own input on the mirror

    /// Where the screen is drawn in the mirror area, if there's a frame.
    fn screen_rect(&self) -> Option<Bounds<Pixels>> {
        let f = self.frame.as_ref()?;
        fit(self.area.get().size, f.width, f.height)
    }

    /// A window position as fractions of the screen.
    fn to_fraction(&self, p: Point<Pixels>) -> Option<(f64, f64)> {
        let s = self.screen_rect()?;
        let origin = self.area.get().origin + s.origin;
        let x = ((p.x - origin.x).as_f32() / s.size.width.as_f32()).clamp(0., 1.);
        let y = ((p.y - origin.y).as_f32() / s.size.height.as_f32()).clamp(0., 1.);
        Some((x as f64, y as f64))
    }

    fn pointer_up(&mut self, p: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(start) = self.press.take() else {
            return;
        };
        if self.facts().route() == Route::Driver {
            let (Some(a), Some(b)) = (self.to_fraction(start), self.to_fraction(p)) else {
                return;
            };
            let moved =
                ((p.x - start.x).as_f32().powi(2) + (p.y - start.y).as_f32().powi(2)).sqrt();
            self.usb_input(
                move |screen| {
                    let (a, b) = (driver::to_points(a, screen), driver::to_points(b, screen));
                    vec![if moved < TAP_SLOP {
                        driver::Step::Post("/tap", json!({ "x": a.0, "y": a.1, "hold_ms": 80 }))
                    } else {
                        driver::Step::Post(
                            "/swipe",
                            json!({ "x1": a.0, "y1": a.1, "x2": b.0, "y2": b.1, "duration_ms": 300 }),
                        )
                    }]
                },
                window,
                cx,
            );
            return;
        }
        let mirrored = !self.status.input_ready() && self.mirror_frames;
        if !self.status.input_ready() && !mirrored {
            self.cant_tap(cx);
            return;
        }
        let (Some(a), Some(b), Some(link)) = (
            self.to_fraction(start),
            self.to_fraction(p),
            self.link.clone(),
        ) else {
            return;
        };
        let moved = ((p.x - start.x).as_f32().powi(2) + (p.y - start.y).as_f32().powi(2)).sqrt();
        if mirrored {
            // The picture is iPhone Mirroring's window: its fractions are the window's.
            let _ = link.mirror(&if moved < TAP_SLOP {
                MirrorCmd::Tap {
                    x: a.0,
                    y: a.1,
                    hold_ms: 80,
                }
            } else {
                MirrorCmd::Drag {
                    from: a,
                    to: b,
                    ms: 300,
                }
            });
            return;
        }
        self.ensure_assistive(window, cx);
        let steps = if moved < TAP_SLOP {
            hid::tap(a.0, a.1, 80)
        } else {
            hid::swipe(a, b, 300)
        };
        let _ = link.hid(&steps);
    }

    fn home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.facts().route() == Route::Driver {
            let home = |_| vec![driver::Step::Post("/key", json!({ "key": "home" }))];
            self.usb_input(home, window, cx);
            return;
        }
        match self.link.clone() {
            Some(link) if self.status.input_ready() => {
                let _ = link.hid(&hid::home());
            }
            Some(link) if self.mirror_frames => {
                let _ = link.mirror(&MirrorCmd::Menu(mirror::HOME));
            }
            _ => self.cant_tap(cx),
        }
    }

    /// The user tapped but taps can't reach the phone now: the strip's line says what to do
    /// for a minute, and with the phone locked iPhone Mirroring is brought up for the next tap.
    fn cant_tap(&mut self, cx: &mut Context<Self>) {
        self.wanted_taps = Some(Instant::now());
        let facts = self.facts();
        if facts.route() == Route::Mirroring
            && !facts.usb_live()
            && let Some(link) = &self.link
        {
            match self.mirror {
                Some(MirrorState::Closed) => drop(link.mirror(&MirrorCmd::Open)),
                Some(MirrorState::TimedOut) => {
                    drop(link.mirror(&MirrorCmd::Press("Connect".into())))
                }
                _ => {}
            }
            self.mirror_looked = None;
        }
        cx.notify();
    }

    /// "Check again" on a timed-out iPhone Mirroring: connect once more, and look at what's in
    /// the way again when that times out too.
    fn check_again(&mut self, cx: &mut Context<Self>) {
        self.diagnosed = None;
        if let Some(link) = &self.link {
            drop(link.mirror(&MirrorCmd::Press("Connect".into())));
        }
        self.mirror_looked = None;
        cx.notify();
    }

    // MARK: agents

    /// An agent's tool call: run after those before it.
    pub fn agent_call(&mut self, call: PhoneCall, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(why) = self
            .lease
            .claim(&call.thread, &call.title, call.turn, Instant::now())
        {
            call.answer(Err(why));
            return;
        }
        self.last = Some(Activity {
            title: call.title.clone(),
            action: starting(&call.method).into(),
            at: Instant::now(),
        });
        self.queue.push_back(call);
        self.manage_link(window, cx);
        cx.notify();
        if self.running {
            return;
        }
        self.running = true;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let next = this.update(cx, |p, _| {
                    let next = p.queue.pop_front();
                    if next.is_none() {
                        p.running = false;
                    }
                    next.map(|c| (c, p.stops))
                });
                let Ok(Some((call, stops))) = next else { break };
                let result = Run {
                    this: this.clone(),
                    cx,
                    call: &call,
                    stops,
                }
                .go()
                .await;
                let done = match &result {
                    Ok(v) => v["text"]
                        .as_str()
                        .and_then(|t| t.lines().next())
                        .unwrap_or("Done")
                        .to_string(),
                    Err(e) => e.lines().next().unwrap_or_default().to_string(),
                };
                call.answer(result);
                let ended = this.update(cx, |p, cx| {
                    if let Some(a) = p.last.as_mut() {
                        a.action = done;
                        a.at = Instant::now();
                    }
                    cx.notify();
                });
                if ended.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// The user takes the phone back: the call under way stops at its next step, those waiting
    /// fail, and the turn that held it gets no more.
    fn agent_stop(&mut self, cx: &mut Context<Self>) {
        self.stops += 1;
        self.lease.take_back();
        for call in self.queue.drain(..) {
            call.answer(Err(TOOK_OVER.into()));
        }
        self.last = None;
        self.restore_assistive(cx);
        cx.notify();
    }

    fn agent_hand_back(&mut self, cx: &mut Context<Self>) {
        self.lease.hand_back();
        cx.notify();
    }

    /// A made-up phone in one of the strip's situations, for design review (`control phone
    /// <state>` in an isolated capture run): usb, lock, locked, mirroring, pair, bluetooth,
    /// none, how, Trek Driver's: driver, driver-starting, driver-install, driver-how, and with no
    /// cable: wifi, wifi-starting, wifi-locked, wifi-how.
    #[cfg(feature = "shots")]
    pub fn demo(&mut self, state: &str, cx: &mut Context<Self>) -> anyhow::Result<()> {
        if state == "real" {
            // The user's real phone after all, for a live check (`phone call …` drives it).
            self.demo = false;
            self.demo_driver = None;
            self.passcode = None;
            self.phone = None;
            self.status = Status::default();
            self.mirror = None;
            self.mirroring_seen = std::fs::read_to_string(mirroring_marker())
                .ok()
                .map(|s| s.trim().to_string());
            self.refreshed = None;
            cx.notify();
            return Ok(());
        }
        let usb = Status {
            camera: "granted".into(),
            screen: "Toby's iPhone".into(),
            width: 1320,
            height: 2868,
            frames: 40,
            frame_age: 0.2,
            bluetooth: "advertising".into(),
            ..Status::default()
        };
        self.demo = true;
        self.link = None;
        self._link_events = None;
        self.phone = Some(device::Phone {
            udid: "demo".into(),
            name: "Toby's iPhone".into(),
            ios: "27.2".into(),
            model: "iPhone17,2".into(),
            usb: true,
        });
        self.status = usb.clone();
        self.mirror = Some(MirrorState::TimedOut);
        self.mirroring_seen = Some("iPhone 16 Pro Max".into());
        self.mirror_frames = false;
        self.driver_frames = false;
        self.assistive = None;
        self.wanted_taps = None;
        self.explain_open = false;
        self.notice = None;
        self.demo_driver = None;
        self.blocker = None;
        self.passcode = None;
        match state {
            "usb" => {}
            "driver" => self.demo_driver = Some((driver::Avail::Ready, driver::State::Off)),
            "driver-starting" => {
                self.demo_driver = Some((driver::Avail::Ready, driver::State::Starting))
            }
            "driver-install" => {
                self.demo_driver = Some((
                    driver::Avail::Install("it isn't installed yet".into()),
                    driver::State::Off,
                ));
                self.wanted_taps = Some(Instant::now());
            }
            "driver-live" => {
                self.demo_driver = Some((driver::Avail::Ready, driver::State::Ready));
                self.driver_frames = true;
            }
            "driver-how" => {
                self.demo_driver = Some((driver::Avail::Ready, driver::State::Ready));
                self.explain_open = true;
            }
            // Locked and timing out, diagnosed: the phone isn't offering iPhone Mirroring.
            "unreachable" => {
                self.status.dark = true;
                self.blocker = Some(Blocker::PhoneNotOffering);
            }
            "lock" => self.wanted_taps = Some(Instant::now()),
            "how" => self.explain_open = true,
            "locked" => {
                self.status.dark = true;
                self.mirror = Some(MirrorState::Closed);
            }
            "mirroring" => {
                self.status.dark = true;
                self.mirror = Some(MirrorState::Live);
                self.mirror_frames = true;
            }
            "pair" => {
                self.mirroring_seen = None;
                self.mirror = Some(MirrorState::Closed);
            }
            "bluetooth" => {
                self.mirroring_seen = None;
                self.mirror = None;
                self.status.hosts = 1;
                self.status.bluetooth = "connected".into();
                self.assistive = Some(false);
            }
            "none" => {
                self.phone = None;
                self.status = Status::default();
            }
            // No cable: usbmuxd sees the phone over Wi-Fi, the driver gives the picture and takes
            // taps; locked (as CoreDevice says), iPhone Mirroring, here timing out, so the line
            // says to unlock it.
            "wifi" | "wifi-starting" | "wifi-locked" | "wifi-how" => {
                if let Some(p) = &mut self.phone {
                    p.usb = false;
                }
                self.status = Status {
                    camera: "granted".into(),
                    bluetooth: "advertising".into(),
                    ..Status::default()
                };
                self.mirror = Some(MirrorState::Closed);
                self.passcode = Some(state == "wifi-locked");
                let driver = match state {
                    "wifi-starting" => driver::State::Starting,
                    "wifi-locked" => driver::State::Off,
                    _ => driver::State::Ready,
                };
                self.demo_driver = Some((driver::Avail::Ready, driver));
                self.driver_frames = matches!(state, "wifi" | "wifi-how");
                self.explain_open = state == "wifi-how";
                if state == "wifi-locked" {
                    self.mirror = Some(MirrorState::TimedOut);
                    self.wanted_taps = Some(Instant::now());
                }
            }
            other => anyhow::bail!(
                "phone: unknown state {other:?} (usb|lock|locked|unreachable|mirroring|pair|bluetooth|none|how|driver|driver-starting|driver-install|driver-how|wifi|wifi-starting|wifi-locked|wifi-how)"
            ),
        }
        cx.notify();
        Ok(())
    }

    // MARK: drawing

    /// One line: the device glyph, its name, then iOS and the link, muted.
    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (name, detail) = match &self.phone {
            Some(p) => (
                if p.name.is_empty() {
                    "iPhone".to_string()
                } else {
                    p.name.clone()
                },
                Some(format!(
                    "iOS {} · {}",
                    p.ios,
                    if p.usb { "USB" } else { "Wi-Fi" }
                )),
            ),
            None => ("No iPhone".to_string(), None),
        };
        h_flex()
            .h(px(36.))
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Icon::new(Lucide::Smartphone)
                    .small()
                    .text_color(theme.muted_foreground),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(to_rems(text::LABEL))
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    )
                    .when_some(detail, |el, d| {
                        el.child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_size(to_rems(text::META))
                                .text_color(theme.muted_foreground)
                                .child(d),
                        )
                    }),
            )
            .child(
                crate::ui::icon_button("phone-home", Lucide::House, "Home Screen")
                    .xsmall()
                    .on_click(cx.listener(|this, _, window, cx| this.home(window, cx))),
            )
            .child(
                crate::ui::icon_button(
                    "phone-refresh",
                    Lucide::RefreshCw,
                    "Look for the iPhone again",
                )
                .xsmall()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.refreshed = None;
                    this.link_retry = None;
                    this.notice = None;
                    this.refresh_device(window, cx);
                    this.manage_link(window, cx);
                })),
            )
            .into_any_element()
    }

    /// The status strip: Screen and Taps (and AssistiveTouch on the Bluetooth route), each a dot
    /// and a few words, with "How this works" at its end; under it, the one thing to do now.
    fn chips(&self, strip: &route::Strip, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let color = |tone: Tone| match tone {
            Tone::Good => crate::palette::emerald(cx),
            Tone::Wait => crate::palette::amber(cx),
            Tone::Bad => crate::palette::red(cx),
            Tone::Quiet => theme.muted_foreground,
        };
        let chip = |c: &route::Chip| {
            h_flex()
                .flex_shrink_0()
                .gap(px(5.))
                .child(div().size(px(6.)).rounded_full().bg(color(c.tone)))
                .child(div().text_color(theme.muted_foreground).child(c.label))
                .child(div().child(c.word.clone()))
        };
        let route = strip.route;
        let wifi = self.on_wifi();
        let explain = Popover::new("phone-how")
            .anchor(Anchor::TopRight)
            .appearance(false)
            .open(self.explain_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.explain_open = *open;
                cx.notify();
            }))
            .trigger(
                crate::ui::icon_button("phone-how-button", IconName::Info, "How this works")
                    .xsmall(),
            )
            .content(move |_, _, cx| {
                let theme = cx.theme().clone();
                crate::ui::menu_surface(cx)
                    .w(px(340.))
                    .p_3()
                    .gap_3()
                    .text_size(to_rems(text::META))
                    .child(
                        div()
                            .text_size(to_rems(text::LABEL))
                            .font_weight(FontWeight::MEDIUM)
                            .child("How this works"),
                    )
                    .children(
                        route::explainer(route, wifi)
                            .into_iter()
                            .map(|(title, body)| {
                                v_flex()
                                    .gap(px(2.))
                                    .child(div().font_weight(FontWeight::MEDIUM).child(title))
                                    .child(div().text_color(theme.muted_foreground).child(body))
                            }),
                    )
            });
        let recheck = strip.recheck;
        let checking = self.diagnosing;
        let action = strip.action.clone().map(|line| {
            h_flex()
                .gap(px(6.))
                .text_color(theme.foreground)
                .child(
                    Icon::new(IconName::CircleAlert)
                        .xsmall()
                        .text_color(crate::palette::amber(cx)),
                )
                .child(div().flex_1().min_w_0().child(line))
                .when(recheck, |row| {
                    row.child(
                        Button::new("phone-check-again")
                            .xsmall()
                            .ghost()
                            .label(if checking {
                                "Checking…"
                            } else {
                                "Check again"
                            })
                            .disabled(checking)
                            .on_click(cx.listener(|this, _, _, cx| this.check_again(cx))),
                    )
                })
        });
        v_flex()
            .id("phone-strip")
            .px_3()
            .py(px(6.))
            .gap_1()
            .text_size(to_rems(text::META))
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .flex_wrap()
                            .gap_x_4()
                            .gap_y_1()
                            .child(chip(&strip.screen))
                            .child(chip(&strip.taps))
                            .children(strip.assistive.as_ref().map(chip)),
                    )
                    .child(explain),
            )
            .children(action)
            .into_any_element()
    }

    fn strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let ember = crate::palette::ember(cx);
        let row = |id: &'static str| {
            h_flex()
                .id(id)
                .h(px(32.))
                .px(px(12.))
                .gap(px(8.))
                .text_size(to_rems(text::META))
                .border_b_1()
                .border_color(theme.border)
        };
        let busy = self.busy();
        let holder = self.lease.holder(Instant::now()).is_some();
        if let Some(a) = self
            .last
            .as_ref()
            .filter(|a| busy || holder || a.at.elapsed() < STRIP_LINGER)
        {
            return Some(
                row("phone-agent-strip")
                    .bg(ember.opacity(0.1))
                    .child(Icon::new(Lucide::Sparkles).xsmall().text_color(ember))
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_weight(FontWeight::MEDIUM)
                            .child(if busy {
                                "Agent is using your iPhone"
                            } else {
                                "Agent has your iPhone"
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(theme.muted_foreground)
                            .child(format!("{} · {}", a.title, a.action)),
                    )
                    .child(
                        Button::new("phone-agent-stop")
                            .xsmall()
                            .outline()
                            .icon(Lucide::Hand)
                            .label("Stop")
                            .tooltip(
                                "Take the iPhone back: the agent's iPhone tools stop for this turn",
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.agent_stop(cx))),
                    )
                    .into_any_element(),
            );
        }
        self.lease.taken_over().then(|| {
            row("phone-agent-taken")
                .bg(theme.foreground.opacity(0.04))
                .child(
                    Icon::new(Lucide::Hand)
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(theme.muted_foreground)
                        .child("You have the iPhone. The agent can use it again in its next turn."),
                )
                .child(
                    Button::new("phone-agent-hand-back")
                        .xsmall()
                        .ghost()
                        .label("Hand back")
                        .on_click(cx.listener(|this, _, _, cx| this.agent_hand_back(cx))),
                )
                .into_any_element()
        })
    }

    fn mirror(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let area = self.area.clone();
        let entity = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let before = area.get();
                area.set(bounds);
                if before.size != bounds.size {
                    let entity = entity.clone();
                    cx.defer(move |cx| {
                        let _ = entity.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let layout = self
            .screen_rect()
            .zip(self.frame.as_ref().map(|f| f.image.clone()));
        // iOS's 9:41 over the cable: the real time drawn over it (never on iPhone Mirroring's
        // picture, which has the real one).
        let bar = self
            .frame
            .as_ref()
            .and_then(|f| f.bar)
            .filter(|_| !self.mirror_frames);
        self.clock_shown = bar.map(|_| phone_time(self.clock, chrono::Utc::now()));
        let clock = bar.zip(self.clock_shown.clone());
        let waiting = match &self.helper {
            _ if self.demo => None,
            Some(Err(e)) => Some(e.clone()),
            _ if self.frame.is_none() => Some(
                if self.phone.is_none() {
                    "Waiting for an iPhone on the cable…"
                } else if self.on_wifi() {
                    let facts = self.facts();
                    if facts.locked() && facts.route() == Route::Mirroring {
                        "Locked. Its screen shows here once it's unlocked, or while Trek taps it."
                    } else if facts.route() == Route::Driver
                        && facts.driver == driver::Avail::Ready
                    {
                        "Over Wi-Fi the screen comes through Trek Driver: starting it…"
                    } else {
                        "Plug the iPhone in to see its screen: over Wi-Fi it comes through Trek Driver."
                    }
                } else if self.status.frames == 0
                    && !self.status.screen.is_empty()
                    && self.facts().route() == Route::Mirroring
                {
                    "Locked. Its screen shows here once it's unlocked, or while Trek taps it."
                } else if self.status.frames == 0 && !self.status.screen.is_empty() {
                    "Unlock the iPhone to see its screen."
                } else {
                    "Connecting to the screen…"
                }
                .to_string(),
            ),
            _ => None,
        };
        div()
            .id("phone-mirror")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .when(self.status.input_ready() || self.mirror_frames, |el| {
                el.cursor_pointer()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, _| this.press = Some(e.position)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, e: &MouseUpEvent, window, cx| {
                    this.pointer_up(e.position, window, cx)
                }),
            )
            .child(measure)
            .when_some(layout, |el, (s, image)| {
                // scale: ok, the iPhone's own corners
                let radius = px(0.11 * s.size.width.as_f32());
                el.child(
                    div()
                        .absolute()
                        .left(s.origin.x - px(6.))
                        .top(s.origin.y - px(6.))
                        .w(s.size.width + px(12.))
                        .h(s.size.height + px(12.))
                        // scale: ok, its bezel
                        .rounded(radius + px(6.))
                        .bg(rgb(0x0B0B0D))
                        .border_1()
                        .border_color(theme.border)
                        .when(!fluent::WINDOWS, |el| el.shadow_lg())
                        .when(fluent::WINDOWS, |el| {
                            el.shadow(fluent::flyout_shadow(Vec::new(), cx))
                        }),
                )
                .child(
                    img(image)
                        .absolute()
                        .left(s.origin.x)
                        .top(s.origin.y)
                        .w(s.size.width)
                        .h(s.size.height)
                        .rounded(radius),
                )
                .when_some(clock, |el, (bar, time)| {
                    let (w, h) = (s.size.width.as_f32(), s.size.height.as_f32());
                    let (x0, x1) = bar.cover(&time);
                    let (r, g, b) = bar.background;
                    let ink = if bar.light_ink {
                        rgb(0xFFFFFF)
                    } else {
                        rgb(0x000000)
                    };
                    el.child(
                        div()
                            .id("phone-clock")
                            .absolute()
                            .left(s.origin.x + px(x0 * w))
                            .top(s.origin.y + px(bar.top * h))
                            .w(px((x1 - x0) * w))
                            .h(px((bar.bottom - bar.top) * h))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgb(u32::from_be_bytes([0, r, g, b])))
                            .text_color(ink)
                            .font_family(".SystemUIFont")
                            .font_weight(FontWeight::SEMIBOLD)
                            // scale: ok, iOS's clock, to scale
                            .text_size(px(STATUS_FONT * w))
                            .line_height(px((bar.bottom - bar.top) * h))
                            .whitespace_nowrap()
                            .child(time),
                    )
                })
            })
            .when_some(waiting, |el, text| {
                el.child(
                    v_flex()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .px_6()
                        .text_size(to_rems(text::LABEL))
                        .text_center()
                        .text_color(theme.muted_foreground)
                        .when(self.helper_building, |el| el.child(Spinner::new().small()))
                        .child(text),
                )
            })
            .into_any_element()
    }
}

impl Render for PhonePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let notice = self.notice.clone();
        let strip = self.facts().strip();
        v_flex()
            .size_full()
            .child(self.header(cx))
            .child(self.chips(&strip, cx))
            .children(self.strip(cx))
            .when_some(notice, |el, n| {
                el.child(
                    h_flex()
                        .px_3()
                        .py(px(6.))
                        .gap_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .bg(crate::palette::amber(cx).opacity(0.08))
                        .child(Icon::new(IconName::CircleAlert).xsmall())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(to_rems(text::META))
                                .child(n),
                        )
                        .child(
                            crate::ui::icon_button(
                                "phone-notice-close",
                                IconName::Close,
                                "Dismiss",
                            )
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.notice = None;
                                cx.notify();
                            })),
                        ),
                )
            })
            .child(self.mirror(cx))
    }
}

/// The screen fitted into `area` with a margin, keeping its aspect ratio.
fn fit(area: Size<Pixels>, w: f32, h: f32) -> Option<Bounds<Pixels>> {
    let margin = 18.;
    let (aw, ah) = (
        area.width.as_f32() - 2. * margin,
        area.height.as_f32() - 2. * margin,
    );
    if aw < 60. || ah < 60. || w < 1. || h < 1. {
        return None;
    }
    let s = (aw / w).min(ah / h);
    let (sw, sh) = (w * s, h * s);
    Some(Bounds {
        origin: point(
            px((area.width.as_f32() - sw) / 2.),
            px((area.height.as_f32() - sh) / 2.),
        ),
        size: size(px(sw), px(sh)),
    })
}

/// The status bar clock's type size, as a share of the screen's width (17 pt on a 440 pt screen).
// scale: ok, iOS's own
const STATUS_FONT: f32 = 17. / 440.;

/// The status bar iOS drew in a frame from the cable (with its 9:41): where its clock is, as
/// fractions of the frame, and its colors. Found only on a portrait frame whose left ear shows a
/// clock-sized run of text on a flat background: an app that hides the status bar (a video) gets
/// nothing drawn over it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct StatusBar {
    /// The clock's text, left and right.
    left: f32,
    right: f32,
    /// The band drawn over, top and bottom: inside the status bar, never below it.
    top: f32,
    bottom: f32,
    /// Where what follows the clock starts (a Focus glyph), if anything does.
    next: Option<f32>,
    background: (u8, u8, u8),
    light_ink: bool,
}

impl StatusBar {
    /// The band to draw over for `time`: the clock's own box, widened (evenly) for a longer time
    /// than "9:41", but never over what follows it or off the ear.
    fn cover(&self, time: &str) -> (f32, f32) {
        let margin = 0.012;
        let center = (self.left + self.right) / 2.;
        let text = time.chars().count() as f32 * STATUS_FONT * 0.62;
        let half = ((self.right - self.left) / 2.).max(text / 2.) + margin;
        let limit = self.next.map_or(0.36, |n| n - 0.004);
        ((center - half).max(0.02), (center + half).min(limit))
    }
}

fn luma((r, g, b): (u8, u8, u8)) -> f32 {
    0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
}

/// A picture's width and height, from its header: without decoding the whole picture.
fn picture_size(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Look for the status bar's clock in a frame from the cable.
fn status_bar(jpeg: &[u8]) -> Option<StatusBar> {
    let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).ok()?;
    // A JPEG decodes to RGB already: `into_rgb8` hands that over where `to_rgb8` copied it.
    find_bar(&img.into_rgb8())
}

fn find_bar(img: &image::RgbImage) -> Option<StatusBar> {
    let (w, h) = img.dimensions();
    if w < 100 || h <= w {
        return None;
    }
    let (wf, hf) = (w as f32, h as f32);
    let at = |x: f32, y: f32| {
        let p = img.get_pixel(((x * wf) as u32).min(w - 1), ((y * hf) as u32).min(h - 1));
        (p[0], p[1], p[2])
    };
    // The band the clock sits in, and the background beside it (left of the text: flat).
    let (y0, y1) = ((0.015 * hf) as u32, (0.055 * hf) as u32);
    let background = at(0.03, 0.034);
    let flat = [0.012, 0.02, 0.045].iter().all(|&x| {
        [0.02, 0.034, 0.048]
            .iter()
            .all(|&y| (luma(at(x, y)) - luma(background)).abs() < 24.)
    });
    if !flat {
        return None;
    }
    let ink = |x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        (luma((p[0], p[1], p[2])) - luma(background)).abs() > 90.
    };
    // Columns of the left ear with ink, grouped; the clock is the first groups close together.
    let ear = (0.36 * wf) as u32;
    let mut groups: Vec<(u32, u32)> = vec![];
    for x in 0..ear {
        if (y0..y1).any(|y| ink(x, y)) {
            match groups.last_mut() {
                Some(g) if x <= g.1 + 1 => g.1 = x,
                _ => groups.push((x, x)),
            }
        }
    }
    let gap = (0.011 * wf) as u32;
    let first = *groups.first()?;
    let mut clock = first;
    let mut count = 1;
    let mut next = None;
    for g in &groups[1..] {
        if g.0 - clock.1 > gap {
            next = Some(g.0 as f32 / wf);
            break;
        }
        clock.1 = g.1;
        count += 1;
    }
    let (left, right) = (clock.0 as f32 / wf, clock.1 as f32 / wf);
    // "9:41" is four glyphs, about a tenth of the width, a little in from the edge.
    if count < 3 || !(0.04..0.25).contains(&left) || !(0.05..0.2).contains(&(right - left)) {
        return None;
    }
    let rows: Vec<u32> = (y0..y1)
        .filter(|&y| (clock.0..=clock.1).any(|x| ink(x, y)))
        .collect();
    let (top, bottom) = (*rows.first()? as f32 / hf, *rows.last()? as f32 / hf);
    if !(0.008..0.02).contains(&(bottom - top)) {
        return None;
    }
    Some(StatusBar {
        left,
        right,
        top: (top - 0.008).max(0.012),
        bottom: (bottom + 0.008).min(0.052),
        next,
        background,
        light_ink: luma(background) < 128.,
    })
}

/// The time as the phone's status bar shows it: its time zone, its 12- or 24-hour clock (the
/// Mac's time zone until the phone's is read).
fn phone_time(clock: Option<device::Clock>, now: chrono::DateTime<chrono::Utc>) -> String {
    use chrono::Timelike as _;
    let (offset, hours24) = match clock {
        Some(c) => (c.utc_offset, c.hours24),
        None => (chrono::Local::now().offset().local_minus_utc(), false),
    };
    let t = now + chrono::Duration::seconds(i64::from(offset));
    let (h, m) = (t.hour(), t.minute());
    if hours24 {
        format!("{h:02}:{m:02}")
    } else {
        format!("{}:{m:02}", if h % 12 == 0 { 12 } else { h % 12 })
    }
}

/// "Tapping…" for the strip while a call of `method` runs.
fn starting(method: &str) -> &'static str {
    match method {
        "phone_screenshot" => "Looking at the screen…",
        "phone_tap" => "Tapping…",
        "phone_swipe" => "Swiping…",
        "phone_type" => "Typing…",
        "phone_key" => "Pressing a key…",
        "phone_home" => "Going home…",
        "phone_open_app" => "Opening an app…",
        "phone_install" => "Installing an app…",
        "phone_push_file" | "phone_pull_file" => "Copying a file…",
        "phone_remove_file" => "Removing a file…",
        "phone_release" => "Letting go…",
        _ => "Checking the iPhone…",
    }
}

/// The file name of a path, for approvals.
fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// What an approval card says for a sensitive call.
fn ask_text(method: &str, p: &Value) -> (String, String) {
    let s = |k: &str| p[k].as_str().unwrap_or_default().to_string();
    let place = |remote: String| match p["app"].as_str() {
        Some(app) => format!("{remote} in {app}"),
        None => remote,
    };
    match method {
        "phone_install" => (
            format!("Install {} on your iPhone", file_name(&s("path"))),
            s("path"),
        ),
        "phone_remove_file" => (
            format!("Remove {} from your iPhone", file_name(&s("path"))),
            place(s("path")),
        ),
        _ => (
            format!("Copy {} to your iPhone", file_name(&s("local"))),
            format!("{} → {}", s("local"), place(s("remote"))),
        ),
    }
}

/// A call being run.
struct Run<'a> {
    this: WeakEntity<PhonePanel>,
    cx: &'a mut AsyncWindowContext,
    call: &'a PhoneCall,
    /// `PhonePanel::stops` when it started: Stop since means the user took over.
    stops: u64,
}

/// How a call's taps and keys reach the phone.
enum Via {
    /// Trek's Bluetooth keyboard and mouse, which the phone took (one on another Apple Account).
    Bluetooth,
    /// macOS's iPhone Mirroring window, showing the phone.
    Mirroring(Arc<Link>),
    /// Trek Driver over the cable or Wi-Fi, running; the screen in points.
    Usb(Arc<Driver>, (f64, f64)),
}

fn gone<E>(_: E) -> String {
    "Trek's iPhone tool closed.".to_string()
}

fn text(t: impl Into<String>) -> Result<Value, String> {
    Ok(json!({ "text": t.into() }))
}

impl Run<'_> {
    async fn sleep(&mut self, d: Duration) {
        self.cx.background_executor().timer(d).await;
    }

    /// The helper's answer on `rx`, unless `limit` passes first (`None`) or it stopped.
    async fn within<T>(
        &mut self,
        rx: async_channel::Receiver<T>,
        limit: Duration,
    ) -> Option<Result<T, String>> {
        let started = Instant::now();
        loop {
            match rx.try_recv() {
                Ok(v) => return Some(Ok(v)),
                Err(async_channel::TryRecvError::Closed) => {
                    return Some(Err("Trek's iPhone helper stopped.".into()));
                }
                Err(async_channel::TryRecvError::Empty) if started.elapsed() > limit => {
                    return None;
                }
                Err(async_channel::TryRecvError::Empty) => {
                    self.sleep(Duration::from_millis(20)).await
                }
            }
        }
    }

    fn check(&mut self) -> Result<(), String> {
        let stops = self.this.update(self.cx, |p, _| p.stops).map_err(gone)?;
        if stops != self.stops {
            return Err(TOOK_OVER.into());
        }
        Ok(())
    }

    /// Blocking work (the phone's services) off the UI thread.
    async fn blocking<R: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> R + Send + 'static,
    ) -> R {
        self.cx
            .background_executor()
            .spawn(async move { work() })
            .await
    }

    fn udid(&mut self) -> Result<Option<String>, String> {
        self.this
            .update(self.cx, |p, _| p.phone.as_ref().map(|p| p.udid.clone()))
            .map_err(gone)
    }

    async fn go(mut self) -> Result<Value, String> {
        let method = self.call.method.clone();
        let p = self.call.params.clone();
        if let Some(kind) = tools::sensitive(&method) {
            self.ask(kind, &method, &p).await?;
        }
        self.check()?;
        let s = |k: &str| p[k].as_str().unwrap_or_default().to_string();
        let app = p["app"].as_str().map(str::to_string);
        match method.as_str() {
            "phone_status" => self.status().await,
            "phone_screenshot" => {
                let edge = p["max_edge"]
                    .as_u64()
                    .map(|e| (e as u32).clamp(200, SHOT_MAX))
                    .unwrap_or(SHOT_EDGE);
                self.screenshot(edge).await
            }
            "phone_tap" => {
                let hold = p["hold_ms"].as_u64().unwrap_or(80).min(10_000) as u32;
                match self.via().await? {
                    Via::Usb(d, screen) => self.usb(d, screen, &method, &p).await?,
                    Via::Bluetooth => {
                        let (x, y) = self.point(&p, "x", "y", false)?;
                        self.input(hid::tap(x, y, hold), true).await?;
                    }
                    Via::Mirroring(link) => {
                        let (x, y) = self.point(&p, "x", "y", true)?;
                        let cmd = MirrorCmd::Tap {
                            x,
                            y,
                            hold_ms: hold,
                        };
                        let limit = Duration::from_millis(u64::from(hold) + 5000);
                        self.mirror_act(&link, cmd, limit).await?;
                    }
                }
                text(format!(
                    "{} ({}, {}). Take a screenshot to see what it did.",
                    if hold >= 500 {
                        "Long-pressed"
                    } else {
                        "Tapped"
                    },
                    p["x"],
                    p["y"]
                ))
            }
            "phone_swipe" => {
                let ms = p["duration_ms"].as_u64().unwrap_or(300).clamp(60, 5000) as u32;
                match self.via().await? {
                    Via::Usb(d, screen) => self.usb(d, screen, &method, &p).await?,
                    Via::Bluetooth => {
                        let from = self.point(&p, "x1", "y1", false)?;
                        let to = self.point(&p, "x2", "y2", false)?;
                        self.input(hid::swipe(from, to, ms), true).await?;
                    }
                    Via::Mirroring(link) => {
                        let from = self.point(&p, "x1", "y1", true)?;
                        let to = self.point(&p, "x2", "y2", true)?;
                        let limit = Duration::from_millis(u64::from(ms) + 5000);
                        self.mirror_act(&link, MirrorCmd::Drag { from, to, ms }, limit)
                            .await?;
                    }
                }
                text("Swiped. Take a screenshot to see where it went.")
            }
            "phone_type" => {
                let submit = p["submit"].as_bool() == Some(true);
                match self.via().await? {
                    Via::Usb(d, screen) => self.usb(d, screen, &method, &p).await?,
                    Via::Bluetooth => {
                        let mut steps = hid::type_text(&s("text"))?;
                        if submit {
                            steps.extend(hid::key("enter")?);
                        }
                        self.input(steps, false).await?;
                    }
                    Via::Mirroring(link) => {
                        let typed = s("text");
                        let limit = Duration::from_millis(typed.chars().count() as u64 * 60 + 5000);
                        self.mirror_act(&link, MirrorCmd::Text(typed), limit)
                            .await?;
                        if submit {
                            self.mirror_key(&link, "enter").await?;
                        }
                    }
                }
                {
                    let n = s("text").chars().count();
                    text(format!(
                        "Typed {n} character{}.",
                        if n == 1 { "" } else { "s" }
                    ))
                }
            }
            "phone_key" => {
                match self.via().await? {
                    Via::Usb(d, screen) => self.usb(d, screen, &method, &p).await?,
                    Via::Bluetooth => self.input(hid::key(&s("key"))?, false).await?,
                    Via::Mirroring(link) => self.mirror_key(&link, &s("key")).await?,
                }
                text(format!("Pressed {}.", s("key")))
            }
            "phone_home" => {
                match self.via().await? {
                    Via::Usb(d, screen) => self.usb(d, screen, &method, &p).await?,
                    Via::Bluetooth => self.input(hid::home(), false).await?,
                    Via::Mirroring(link) => {
                        let cmd = MirrorCmd::Menu(mirror::HOME);
                        self.mirror_act(&link, cmd, Duration::from_secs(5)).await?
                    }
                }
                text("Went to the Home Screen.")
            }
            "phone_open_app" => {
                let name = s("name");
                if name.trim().is_empty() {
                    return Err("Which app? Give its name.".into());
                }
                match self.via().await? {
                    Via::Usb(d, screen) => {
                        self.usb(d, screen, &method, &p).await?;
                        self.sleep(Duration::from_millis(600)).await;
                        return text(format!(
                            "Opened {}. Take a screenshot to check it's the app.",
                            name.trim()
                        ));
                    }
                    Via::Bluetooth => {
                        let mut steps = hid::home();
                        steps.push(Step::Wait(800));
                        steps.extend(hid::key("cmd+space")?);
                        steps.push(Step::Wait(700));
                        steps.extend(hid::type_text(name.trim())?);
                        steps.push(Step::Wait(1000));
                        steps.extend(hid::key("enter")?);
                        self.input(steps, false).await?;
                    }
                    Via::Mirroring(link) => {
                        let menu = |key| (MirrorCmd::Menu(key), Duration::from_secs(5));
                        let (home, spotlight) = (menu(mirror::HOME), menu(mirror::SPOTLIGHT));
                        self.mirror_act(&link, home.0, home.1).await?;
                        self.sleep(Duration::from_millis(800)).await;
                        self.mirror_act(&link, spotlight.0, spotlight.1).await?;
                        self.sleep(Duration::from_millis(700)).await;
                        let typed = MirrorCmd::Text(name.trim().to_string());
                        self.mirror_act(&link, typed, Duration::from_secs(10))
                            .await?;
                        self.sleep(Duration::from_millis(1000)).await;
                        self.mirror_key(&link, "enter").await?;
                    }
                }
                self.sleep(Duration::from_millis(800)).await;
                text(format!(
                    "Opened Spotlight, searched for \"{}\" and pressed Return. Take a screenshot to check it's the app.",
                    name.trim()
                ))
            }
            "phone_apps" => {
                let udid = self.udid()?;
                let apps = self.blocking(move || device::apps(udid.as_deref())).await?;
                let lines: Vec<String> = apps
                    .iter()
                    .map(|a| {
                        format!(
                            "{} — {} {}{}",
                            a.name,
                            a.bundle,
                            a.version,
                            if a.file_sharing {
                                " [file sharing]"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect();
                text(format!("{} apps:\n{}", lines.len(), lines.join("\n")))
            }
            "phone_install" => {
                let path = PathBuf::from(s("path"));
                let udid = self.udid()?;
                let shown = path.display().to_string();
                self.blocking(move || device::install(udid.as_deref(), &path))
                    .await?;
                text(format!(
                    "Installed {shown}. A development-signed app opens only once its developer is trusted on the iPhone (Settings › General › VPN & Device Management)."
                ))
            }
            "phone_files" => {
                let path = p["path"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| if app.is_some() { "/Documents" } else { "/" }.to_string());
                let udid = self.udid()?;
                let shown = path.clone();
                let entries = self
                    .blocking(move || device::list_dir(udid.as_deref(), &path, app.as_deref()))
                    .await?;
                let lines: Vec<String> = entries
                    .iter()
                    .map(|e| {
                        if e.dir {
                            format!("{}/", e.name)
                        } else {
                            format!("{}  {}", e.name, files::human_size(e.size))
                        }
                    })
                    .collect();
                text(if lines.is_empty() {
                    format!("{shown} is empty.")
                } else {
                    format!("{shown}:\n{}", lines.join("\n"))
                })
            }
            "phone_push_file" => {
                let (local, remote) = (PathBuf::from(s("local")), s("remote"));
                let udid = self.udid()?;
                let shown = remote.clone();
                let n = self
                    .blocking(move || {
                        device::push(udid.as_deref(), &local, &remote, app.as_deref())
                    })
                    .await?;
                text(format!("Copied {} to {shown}.", files::human_size(n)))
            }
            "phone_pull_file" => {
                let (remote, local) = (s("remote"), PathBuf::from(s("local")));
                let udid = self.udid()?;
                let shown = local.display().to_string();
                let n = self
                    .blocking(move || {
                        device::pull(udid.as_deref(), &remote, &local, app.as_deref())
                    })
                    .await?;
                text(format!("Copied {} to {shown}.", files::human_size(n)))
            }
            "phone_remove_file" => {
                let path = s("path");
                let udid = self.udid()?;
                let shown = path.clone();
                self.blocking(move || device::remove(udid.as_deref(), &path, app.as_deref()))
                    .await?;
                text(format!("Removed {shown}."))
            }
            "phone_release" => {
                let thread = self.call.thread.clone();
                self.this
                    .update(self.cx, |p, cx| {
                        let held = p.lease.release(&thread);
                        p.restore_assistive(cx);
                        p.stop_driver(cx);
                        p.last = None;
                        cx.notify();
                        held
                    })
                    .map_err(gone)?;
                text("Let go of the iPhone.")
            }
            other => Err(format!("Unknown iPhone tool {other}")),
        }
    }

    /// A point of the latest screenshot as fractions of the screen (`mirrored`: of iPhone
    /// Mirroring's window, which that screenshot must then have been).
    fn point(
        &mut self,
        p: &Value,
        kx: &str,
        ky: &str,
        mirrored: bool,
    ) -> Result<(f64, f64), String> {
        let (x, y) = (
            p[kx].as_f64().ok_or(format!("{kx} is missing"))?,
            p[ky].as_f64().ok_or(format!("{ky} is missing"))?,
        );
        let (size, shot_mirrored) = self
            .this
            .update(self.cx, |p, _| (p.shot_size, p.shot_mirrored))
            .map_err(gone)?;
        if size.is_some() && shot_mirrored != mirrored {
            return Err(if mirrored {
                "Take a phone_screenshot first: the picture comes from iPhone Mirroring now, and x/y are pixels in it."
            } else {
                "Take a phone_screenshot first: the picture no longer comes from iPhone Mirroring, and x/y are pixels in it."
            }
            .into());
        }
        let (w, h) = size.unwrap_or((0., 0.));
        hid::fraction(x, y, w, h)
    }

    fn bluetooth(&mut self) -> Result<bool, String> {
        self.this
            .update(self.cx, |p, _| p.status.input_ready())
            .map_err(gone)
    }

    /// What the panel's strip is decided from, now.
    fn facts(&mut self) -> Result<Facts, String> {
        self.this.update(self.cx, |p, _| p.facts()).map_err(gone)
    }

    /// Whether the cable brought a lit picture in the last few seconds: the iPhone is unlocked
    /// and in use, so iPhone Mirroring can't connect.
    fn usb_busy(&mut self) -> Result<bool, String> {
        self.this
            .update(self.cx, |p, _| {
                let s = &p.status;
                s.frames > 0 && !s.dark && (0. ..3.).contains(&s.frame_age)
            })
            .map_err(gone)
    }

    /// How taps and keys reach the phone now: its Bluetooth keyboard and mouse when it took
    /// them, else iPhone Mirroring, brought to show the phone; or the one thing the user must do.
    async fn via(&mut self) -> Result<Via, String> {
        let link = self.link().await?;
        if self.facts()?.route() == Route::Driver {
            return self.usb_via().await;
        }
        if self.bluetooth()? {
            return Ok(Via::Bluetooth);
        }
        let before = self.facts()?;
        let other = self
            .this
            .update(self.cx, |p, _| p.mirror_other())
            .map_err(gone)?;
        // No iPhone Mirroring here, or it's set up with another iPhone: pairing is the way.
        if before.route() == Route::Bluetooth
            && (other || before.mirror == Some(MirrorState::Missing))
        {
            return Err(self.why_no_taps()?);
        }
        match self.mirror_live(&link).await {
            Ok(()) => Ok(Via::Mirroring(link)),
            Err(e) => {
                // What it showed on the way may have settled the route.
                let after = self.facts()?;
                Err(match after.route() {
                    Route::Mirroring => e,
                    Route::Bluetooth | Route::Driver => after.strip().advice.unwrap_or(e),
                })
            }
        }
    }

    /// Trek Driver running on the phone (started now if need be: the strip says "Starting the
    /// driver…" meanwhile), or why it can't.
    async fn usb_via(&mut self) -> Result<Via, String> {
        let (install, udid, d, avail) = self
            .this
            .update(self.cx, |p, cx| {
                cx.notify();
                (
                    p.driver_install.clone(),
                    p.phone.as_ref().map(|p| p.udid.clone()),
                    p.driver.clone(),
                    p.driver_avail(),
                )
            })
            .map_err(gone)?;
        let (Some(install), Some(udid), driver::Avail::Ready) = (install, udid, avail) else {
            return Err(self.why_no_taps()?);
        };
        self.check()?;
        let running = d.clone();
        let screen = self
            .blocking(move || running.ensure(&install, &udid, driver::START_LIMIT))
            .await;
        let _ = self.this.update(self.cx, |_, cx| cx.notify());
        match screen {
            Ok(screen) => Ok(Via::Usb(d, screen)),
            Err(e) => Err(self.why_no_taps().unwrap_or(e)),
        }
    }

    /// A tool's taps, swipes or keys through Trek Driver: x/y are pixels of the latest
    /// screenshot (the cable's), turned into the driver's points.
    async fn usb(
        &mut self,
        d: Arc<Driver>,
        screen: (f64, f64),
        method: &str,
        p: &Value,
    ) -> Result<(), String> {
        self.check()?;
        let (size, mirrored) = self
            .this
            .update(self.cx, |p, _| (p.shot_size, p.shot_mirrored))
            .map_err(gone)?;
        let pointer = matches!(method, "phone_tap" | "phone_swipe");
        if pointer && size.is_some() && mirrored {
            return Err("Take a phone_screenshot first: the picture no longer comes from iPhone Mirroring, and x/y are pixels in it.".into());
        }
        let (w, h) = size.unwrap_or((0., 0.));
        let (p, method) = (p.clone(), method.to_string());
        let udid = self.udid()?;
        self.blocking(move || {
            let point = |kx: &str, ky: &str| {
                let x = p[kx].as_f64().ok_or(format!("{kx} is missing"))?;
                let y = p[ky].as_f64().ok_or(format!("{ky} is missing"))?;
                Ok(driver::to_points(hid::fraction(x, y, w, h)?, screen))
            };
            let apps = if method == "phone_open_app" {
                device::all_apps(udid.as_deref()).unwrap_or_default()
            } else {
                vec![]
            };
            let steps = driver::plan(&method, &p, &point, &|n| driver::find_app(&apps, n))?;
            driver::run(&*d, &steps)
        })
        .await
    }

    /// One command for iPhone Mirroring; what it shows afterwards is the panel's too.
    async fn mirror_cmd(
        &mut self,
        link: &Arc<Link>,
        cmd: MirrorCmd,
        limit: Duration,
    ) -> Result<Mirrored, String> {
        let rx = link.mirror(&cmd);
        let m = self
            .within(rx, limit)
            .await
            .ok_or("iPhone Mirroring didn't answer in time.")??;
        self.this
            .update(self.cx, |p, cx| {
                p.mirror_looked = Some(Instant::now());
                p.see_mirror(&m.look, cx);
            })
            .map_err(gone)?;
        Ok(m)
    }

    /// Tap, type or choose a menu item in iPhone Mirroring; its error, with what to do about it.
    async fn mirror_act(
        &mut self,
        link: &Arc<Link>,
        cmd: MirrorCmd,
        limit: Duration,
    ) -> Result<(), String> {
        self.check()?;
        let m = self.mirror_cmd(link, cmd, limit).await?;
        match (m.error, MirrorState::of(&m.look)) {
            (None, _) => Ok(()),
            (Some(e), MirrorState::Live) => Err(e),
            (Some(e), state) => Err(format!("{} ({e})", state.advice())),
        }
    }

    /// A key through iPhone Mirroring: ⌘Space is its Spotlight menu item, the rest go to its
    /// window as keystrokes.
    async fn mirror_key(&mut self, link: &Arc<Link>, combo: &str) -> Result<(), String> {
        let plain = combo.to_ascii_lowercase().replace(' ', "");
        let cmd = if ["cmd+space", "command+space", "meta+space"].contains(&plain.as_str()) {
            MirrorCmd::Menu(mirror::SPOTLIGHT)
        } else {
            let (code, flags) = mirror::key(combo)?;
            MirrorCmd::Key(code, flags)
        };
        self.mirror_act(link, cmd, Duration::from_secs(5)).await
    }

    /// iPhone Mirroring showing the phone: opened behind the user's windows, or told to connect,
    /// and taken for the phone's screen only once a blank window has lasted; never tried while
    /// the iPhone is unlocked and in use, which only times it out. After a time-out Connect is
    /// pressed again only once the phone shows on the Mac's network, a bounded number of times
    /// (`mirror::Retry`), and a stuck iPhone Mirroring is relaunched once; giving up says what
    /// `diagnose` found in the way.
    async fn mirror_live(&mut self, link: &Arc<Link>) -> Result<(), String> {
        let first = self
            .mirror_cmd(link, MirrorCmd::Look, Duration::from_secs(5))
            .await?;
        let state = MirrorState::of(&first.look);
        let settled = self
            .this
            .update(self.cx, |p, _| p.mirror.clone())
            .map_err(gone)?;
        if state == MirrorState::Live && settled == Some(MirrorState::Live) {
            return Ok(());
        }
        match state {
            MirrorState::Missing | MirrorState::NotAllowed | MirrorState::MacLocked => {
                return Err(state.advice());
            }
            _ => {}
        }
        if self.usb_busy()? {
            return Err(MirrorState::PhoneInUse.advice());
        }
        let has_connect = |m: &Mirrored| m.look.buttons.iter().any(|b| b == "Connect");
        // Closed, or a window Accessibility can't give (seen with it running windowless):
        // opening it again, in the background, brings its window back.
        let kick = if matches!(state, MirrorState::Closed | MirrorState::Unread) {
            Some(MirrorCmd::Open)
        } else if has_connect(&first) {
            Some(MirrorCmd::Press("Connect".into()))
        } else {
            None
        };
        let mut retry = mirror::Retry {
            presses: u32::from(matches!(kick, Some(MirrorCmd::Press(_)))),
            relaunched: false,
        };
        if let Some(kick) = kick
            && let Some(e) = self
                .mirror_cmd(link, kick, Duration::from_secs(15))
                .await?
                .error
        {
            return Err(e);
        }
        let mut settle = Settle::default();
        let mut kicked = Instant::now();
        let started = Instant::now();
        let mut connecting = false;
        // Since when it has been connecting, or shown no window to read, this attempt.
        let mut connecting_since: Option<Instant> = None;
        let mut unread_since: Option<Instant> = None;
        loop {
            self.sleep(Duration::from_millis(500)).await;
            self.check()?;
            let m = self
                .mirror_cmd(link, MirrorCmd::Look, Duration::from_secs(5))
                .await?;
            let raw = MirrorState::of(&m.look);
            if matches!(raw, MirrorState::Unread | MirrorState::Closed) {
                unread_since.get_or_insert_with(Instant::now);
            } else {
                unread_since = None;
            }
            let state = settle.see(raw, Instant::now());
            match &state {
                Some(MirrorState::Live) => {
                    // The phone's picture settles a moment after it connects.
                    self.sleep(Duration::from_millis(300)).await;
                    return Ok(());
                }
                Some(
                    s @ (MirrorState::Missing
                    | MirrorState::NotAllowed
                    | MirrorState::MacLocked
                    | MirrorState::PhoneInUse),
                ) => return Err(s.advice()),
                Some(MirrorState::Connecting) => {
                    connecting = true;
                    connecting_since.get_or_insert_with(Instant::now);
                }
                // The window still shows the last attempt's for a moment after Connect.
                Some(MirrorState::TimedOut)
                    if connecting || kicked.elapsed() > Duration::from_secs(3) =>
                {
                    connecting = false;
                    connecting_since = None;
                    if !has_connect(&m) || self.usb_busy()? {
                        return Err(self.cant_reach().await?);
                    }
                    if !self.press_again(&mut retry).await? {
                        return Err(self.cant_reach().await?);
                    }
                    kicked = Instant::now();
                    let _ = self
                        .mirror_cmd(
                            link,
                            MirrorCmd::Press("Connect".into()),
                            Duration::from_secs(15),
                        )
                        .await?;
                }
                _ => {}
            }
            let stuck = [connecting_since, unread_since]
                .iter()
                .any(|t| t.is_some_and(|t| t.elapsed() > mirror::STUCK));
            if stuck {
                if retry.stuck() != mirror::Next::Relaunch {
                    return Err(self.cant_reach().await?);
                }
                // Stuck connecting, or no window to read: quit it and open it again (it
                // connects by itself on launch).
                self.blocking(mirror::quit).await;
                self.sleep(Duration::from_millis(1500)).await;
                if let Some(e) = self
                    .mirror_cmd(link, MirrorCmd::Open, Duration::from_secs(15))
                    .await?
                    .error
                {
                    return Err(e);
                }
                settle = Settle::default();
                connecting = false;
                connecting_since = None;
                unread_since = None;
                kicked = Instant::now();
            }
            if started.elapsed() > MIRROR_CONNECT {
                return Err(self.cant_reach().await?);
            }
        }
    }

    /// Whether to press Connect again after a time-out: only once the phone shows on the Mac's
    /// network (waiting a little for it) or when that can't be told, a bounded number of times.
    async fn press_again(&mut self, retry: &mut mirror::Retry) -> Result<bool, String> {
        let mut waited = false;
        loop {
            let udid = self.udid()?;
            let seen = self
                .blocking(move || diagnose::phone_seen(udid.as_deref()))
                .await;
            match retry.timed_out(seen, waited) {
                mirror::Next::Press => {
                    // A moment for the last attempt to let go of the phone.
                    self.sleep(Duration::from_secs(2)).await;
                    return Ok(true);
                }
                mirror::Next::Wait => {
                    waited = true;
                    let since = Instant::now();
                    while since.elapsed() < mirror::PHONE_WAIT {
                        self.sleep(Duration::from_secs(2)).await;
                        self.check()?;
                        let udid = self.udid()?;
                        let seen = self
                            .blocking(move || diagnose::phone_seen(udid.as_deref()))
                            .await;
                        if seen != Some(false) {
                            break;
                        }
                    }
                }
                _ => return Ok(false),
            }
        }
    }

    /// Done trying: find out what's in the way (the panel's line shows it too), and say it.
    async fn cant_reach(&mut self) -> Result<String, String> {
        let (udid, locked, bluetooth) = self
            .this
            .update(self.cx, |p, _| {
                let facts = p.facts();
                // Locked, or lit and in use; else (an old picture) it can't be told.
                let locked = if facts.locked() {
                    Some(true)
                } else {
                    facts.usb_live().then_some(false)
                };
                (
                    p.phone.as_ref().map(|p| p.udid.clone()),
                    locked,
                    (p.status.bluetooth == "off").then_some(false),
                )
            })
            .map_err(gone)?;
        let d = self
            .blocking(move || diagnose::run(udid.as_deref(), locked, bluetooth))
            .await;
        tracing::info!("iPhone Mirroring can't reach the phone: {:?}", d.checks);
        self.this
            .update(self.cx, |p, cx| {
                p.blocker = Some(d.blocker);
                p.diagnosed = Some(Instant::now());
                cx.notify();
            })
            .map_err(gone)?;
        self.why_no_taps()
    }

    /// Wait (a little) for the helper: built, running, and reporting.
    async fn link(&mut self) -> Result<Arc<Link>, String> {
        let started = Instant::now();
        loop {
            let state = self
                .this
                .update_in(self.cx, |p, window, cx| {
                    p.manage_link(window, cx);
                    match &p.helper {
                        Some(Err(e)) => Err(e.clone()),
                        _ => Ok(p.link.clone().filter(|_| !p.status.bluetooth.is_empty())),
                    }
                })
                .map_err(gone)??;
            if let Some(link) = state {
                return Ok(link);
            }
            // Building the helper the first time takes a while.
            if started.elapsed() > Duration::from_secs(90) {
                return Err("Trek's iPhone helper didn't start.".into());
            }
            self.sleep(Duration::from_millis(250)).await;
        }
    }

    /// The panel's strip in words, with what an agent should do when taps can't reach the phone.
    async fn status(&mut self) -> Result<Value, String> {
        let link = self.link().await.ok();
        if let Some(link) = &link {
            link.ask_status();
        }
        let udid = self.udid()?;
        let (phones, assistive) = self
            .blocking(move || {
                let phones = device::list();
                (phones, device::assistive_touch(udid.as_deref()).ok())
            })
            .await;
        if let Some(link) = &link
            && !self.bluetooth()?
        {
            self.mirror_cmd(link, MirrorCmd::Look, Duration::from_secs(5))
                .await?;
        }
        self.sleep(Duration::from_millis(200)).await;
        let (mut facts, status, holder) = self
            .this
            .update(self.cx, |p, _| {
                (
                    p.facts(),
                    p.status.clone(),
                    p.lease.holder(Instant::now()).map(|h| h.title.clone()),
                )
            })
            .map_err(gone)?;
        let phone = phones?.into_iter().next().ok_or(
            "No iPhone is connected: ask the user to plug it in, unlock it and trust this Mac.", // words: ok
        )?;
        facts.phone = true;
        facts.wifi = !phone.usb;
        facts.assistive = assistive;
        // An agent asking is one that wants to tap.
        facts.wants_taps = true;
        let strip = facts.strip();
        let mut lines = vec![format!(
            "{} · iOS {} · {} ({}).",
            phone.name,
            phone.ios,
            if phone.usb { "USB" } else { "Wi-Fi" },
            phone.model
        )];
        let mut screen = format!("Screen: {}", strip.screen.word);
        if strip.screen.word == "Live" {
            screen += &format!(" over USB, {}×{} pixels", status.width, status.height);
        } else if strip.screen.word == "Live · Wi-Fi" {
            screen += " (from Trek Driver)";
        }
        lines.push(format!("{screen}."));
        lines.push(format!("Taps: {}.", strip.taps.word));
        if let Some(advice) = &strip.advice {
            lines.push(format!("To do: {advice}"));
        } else if strip.route == Route::Driver && facts.wifi {
            lines.push("No cable: taps, swipes and keys go over Wi-Fi through Trek Driver while the iPhone stays unlocked (it starts with the first tap or screenshot, slower than over the cable, and stops at phone_release), and screenshots come from it too; x/y are pixels in the latest phone_screenshot. Apps and files go over Wi-Fi.".into());
        } else if strip.route == Route::Driver {
            lines.push("Taps, swipes and keys go over the USB cable through Trek Driver while the iPhone stays unlocked (it starts with the first tap, about 10 s, and stops at phone_release); x/y are pixels in the latest phone_screenshot.".into());
        } else if strip.route == Route::Mirroring && !facts.mirror_frames {
            lines.push("Taps go through iPhone Mirroring, which Trek opens behind the user's windows when needed; screenshots then come from its window.".into());
        }
        if let Some(at) = &strip.assistive {
            lines.push(format!(
                "AssistiveTouch: {} (Trek turns it on for Bluetooth taps).",
                at.word.to_lowercase()
            ));
        }
        lines.push(format!(
            "Held by: {}.",
            holder.unwrap_or_else(|| "nobody".into())
        ));
        text(lines.join("\n"))
    }

    /// The screen as a picture: iPhone Mirroring's window while it shows the phone (brought up
    /// when Mirroring is the route and the phone is locked), else Trek Driver's while it runs,
    /// else the cable's. With no cable, Trek Driver's (started for it) or iPhone Mirroring's.
    async fn screenshot(&mut self, edge: u32) -> Result<Value, String> {
        let link = self.link().await?;
        // Trek Driver running: its picture has the real clock, and the cable's capture (which
        // makes iOS show 9:41) stays off.
        let driver = self
            .this
            .update(self.cx, |p, _| {
                (p.driver_frames || p.driver.state() == driver::State::Ready)
                    .then(|| p.driver.clone())
            })
            .map_err(gone)?;
        let facts = self.facts()?;
        let wifi = facts.wifi;
        // Over Wi-Fi the driver's picture is the one there is: it starts for it.
        let driver = match driver {
            None if wifi && facts.route() == Route::Driver => match self.usb_via().await? {
                Via::Usb(d, _) => Some(d),
                _ => None,
            },
            d => d,
        };
        if let Some(d) = driver
            && self.facts()?.route() == Route::Driver
        {
            match self.driver_screenshot(d, edge).await {
                Ok(v) => return Ok(v),
                Err(e) if wifi => return Err(e),
                Err(e) => tracing::debug!("the driver's screenshot failed, the cable's next: {e}"),
            }
        }
        let mut why_not = None;
        if !self.bluetooth()? && self.facts()?.route() != Route::Driver {
            let raw = self
                .mirror_cmd(&link, MirrorCmd::Look, Duration::from_secs(5))
                .await
                .map(|m| MirrorState::of(&m.look))
                .ok();
            let (settled, facts) = self
                .this
                .update(self.cx, |p, _| (p.mirror.clone(), p.facts()))
                .map_err(gone)?;
            if raw == Some(MirrorState::Live) && settled == Some(MirrorState::Live) {
                return self.mirror_screenshot(&link, edge).await;
            }
            if facts.route() == Route::Mirroring && !facts.usb_live() {
                match self.mirror_live(&link).await {
                    Ok(()) => return self.mirror_screenshot(&link, edge).await,
                    Err(e) => why_not = Some(e),
                }
            }
        }
        if wifi {
            return Err(match why_not {
                Some(m) => format!(
                    "No picture: the iPhone has no cable and is locked, and iPhone Mirroring isn't showing it. {m}"
                ),
                None => "No picture: with no cable the screen comes through Trek Driver while the iPhone is unlocked, or iPhone Mirroring while it's locked (see phone_status). Ask the user to plug it in to see it now.".into(),
            });
        }
        self.usb_screenshot(&link, edge)
            .await
            .map(|mut v| {
                if let Some(why) = &why_not
                    && let Some(t) = v["text"].as_str()
                {
                    v["text"] = format!("{t} Taps can't reach it now. {why}").into();
                }
                v
            })
            .map_err(|e| match why_not {
                // Locked: the cable has nothing to show, and that's expected; what matters is
                // why iPhone Mirroring didn't show it.
                Some(m) if e.starts_with("No picture from the iPhone") => format!(
                    "No picture: the iPhone is locked, so nothing comes over USB, and iPhone Mirroring isn't showing it. {m}"
                ),
                Some(m) => format!("{e} {m}"),
                None => e,
            })
    }

    fn shot_path(&mut self) -> Result<PathBuf, String> {
        let n = self
            .this
            .update(self.cx, |p, _| {
                p.shots += 1;
                p.shots
            })
            .map_err(gone)?;
        let dir = trek_core::paths::data_dir().join("phone-shots");
        let _ = std::fs::create_dir_all(&dir);
        Ok(dir.join(format!("{}-{n}.png", std::process::id())))
    }

    async fn mirror_screenshot(&mut self, link: &Arc<Link>, edge: u32) -> Result<Value, String> {
        let path = self.shot_path()?;
        let rx = link.mirror_shot(&path, edge);
        let shot = self
            .within(rx, Duration::from_secs(10))
            .await
            .ok_or("iPhone Mirroring's window didn't come in time.")???;
        let size = (shot.width as f64, shot.height as f64);
        self.this
            .update(self.cx, |p, _| {
                p.shot_size = Some(size);
                p.shot_mirrored = true;
            })
            .map_err(gone)?;
        Ok(json!({
            "text": format!(
                "The iPhone's screen in iPhone Mirroring, {}×{}. x/y for phone_tap and phone_swipe are pixels in this picture. Taps and keys go through iPhone Mirroring, which needs the iPhone to stay locked: if it ends, the user unlocked it.",
                shot.width, shot.height
            ),
            "image": shot.path,
        }))
    }

    /// The screen from Trek Driver as PNG, its long side at most `edge` pixels.
    async fn driver_screenshot(&mut self, d: Arc<Driver>, edge: u32) -> Result<Value, String> {
        let path = self.shot_path()?;
        let bytes = self
            .blocking(move || {
                let width = match d.screen() {
                    Some((w, h)) if h > w => (edge as f64 * w / h).round() as u32,
                    _ => edge,
                };
                d.screenshot(width.max(1), 0)
            })
            .await?;
        // Its size from its header: this runs on the main thread, and decoding the whole
        // picture for two numbers kept it busy for nothing.
        let (width, height) =
            picture_size(&bytes).ok_or("Trek Driver's picture couldn't be read.")?;
        std::fs::write(&path, &bytes).map_err(|e| format!("Couldn't save the screenshot: {e}"))?;
        self.this
            .update(self.cx, |p, _| {
                p.shot_size = Some((width as f64, height as f64));
                p.shot_mirrored = false;
            })
            .map_err(gone)?;
        Ok(json!({
            "text": format!(
                "The iPhone's screen, {width}×{height}, from Trek Driver. x/y for phone_tap and phone_swipe are pixels in this picture."
            ),
            "image": path.display().to_string(),
        }))
    }

    async fn usb_screenshot(&mut self, link: &Arc<Link>, edge: u32) -> Result<Value, String> {
        // The first frame comes a second or two after the helper finds the screen.
        let started = Instant::now();
        loop {
            let s = self
                .this
                .update(self.cx, |p, _| p.status.clone())
                .map_err(gone)?;
            if s.frames > 0 || started.elapsed() > Duration::from_secs(6) {
                break;
            }
            link.ask_status();
            self.sleep(Duration::from_millis(250)).await;
        }
        let path = self.shot_path()?;
        let rx = link.shot(&path, edge);
        let shot = self
            .within(rx, Duration::from_secs(10))
            .await
            .ok_or("The iPhone's screen didn't come in time.")???;
        let size = (shot.width as f64, shot.height as f64);
        self.this
            .update(self.cx, |p, _| {
                p.shot_size = Some(size);
                p.shot_mirrored = false;
            })
            .map_err(gone)?;
        let stale = if shot.age > 5. {
            format!(
                " (the newest frame is {:.0} s old: the screen hasn't changed, or the iPhone locked)",
                shot.age
            )
        } else {
            String::new()
        };
        Ok(json!({
            "text": format!(
                "The iPhone's screen, {}×{} (it's {}×{} pixels){stale}. x/y for phone_tap and phone_swipe are pixels in this picture.",
                shot.width, shot.height, shot.screen_width, shot.screen_height
            ),
            "image": shot.path,
        }))
    }

    /// Send reports to the phone; `pointer` input needs AssistiveTouch.
    async fn input(&mut self, steps: Vec<Step>, pointer: bool) -> Result<(), String> {
        let link = self.link().await?;
        if !self.bluetooth()? {
            return Err(self.why_no_taps()?);
        }
        if pointer {
            let (on, udid) = self
                .this
                .update(self.cx, |p, _| {
                    (p.assistive, p.phone.as_ref().map(|p| p.udid.clone()))
                })
                .map_err(gone)?;
            if on != Some(true) {
                let now = {
                    let udid = udid.clone();
                    self.blocking(move || device::assistive_touch(udid.as_deref()))
                        .await
                        .unwrap_or(false)
                };
                if !now {
                    self.blocking(move || device::set_assistive_touch(udid.as_deref(), true))
                        .await?;
                    self.this
                        .update(self.cx, |p, _| p.at_by_trek = true)
                        .map_err(gone)?;
                    // The pointer appears a moment after.
                    self.sleep(Duration::from_millis(600)).await;
                }
                self.this
                    .update(self.cx, |p, _| p.assistive = Some(true))
                    .map_err(gone)?;
            }
        }
        self.check()?;
        let budget = Duration::from_millis(u64::from(hid::duration_ms(&steps)) + 5000);
        let rx = link.hid(&steps);
        let (sent, missed) = self
            .within(rx, budget)
            .await
            .ok_or("The keyboard and mouse didn't answer in time.")??;
        if missed > 0 && sent == 0 {
            return Err(self.why_no_taps()?);
        }
        Ok(())
    }

    /// Why taps can't reach the phone, the action first, as the strip has it.
    fn why_no_taps(&mut self) -> Result<String, String> {
        let mut facts = self.facts()?;
        facts.wants_taps = true;
        let strip = facts.strip();
        Ok(strip.advice.unwrap_or_else(|| match strip.route {
            Route::Driver => "Trek Driver isn't answering: try once more.".into(),
            Route::Mirroring => "iPhone Mirroring didn't show the iPhone in time: try once more, and if it's unlocked, ask the user to lock it.".into(),
            Route::Bluetooth => route::NOT_PAIRED.into(),
        }))
    }

    /// Ask the user, on the thread's card, when the access level says to.
    async fn ask(&mut self, kind: Sensitive, method: &str, p: &Value) -> Result<(), String> {
        let thread = self.call.thread.clone();
        if !needs_asking(kind, self.call.access) {
            return Ok(());
        }
        let (allowed, workspace) = self
            .this
            .update(self.cx, |p, _| {
                (
                    p.allowed.contains(&(thread.clone(), kind)),
                    p.workspace.clone(),
                )
            })
            .map_err(gone)?;
        if allowed {
            return Ok(());
        }
        let (title, detail) = ask_text(method, p);
        let (request, rx) = workspace.update(self.cx, |ws, cx| {
            ws.browser_ask(&thread, title.clone(), detail, cx)
        });
        let started = Instant::now();
        let decision = loop {
            match rx.try_recv() {
                Ok(d) => break d,
                Err(async_channel::TryRecvError::Closed) => break Decision::Deny,
                Err(async_channel::TryRecvError::Empty) => {}
            }
            let pending =
                workspace.update(self.cx, |ws, _| ws.browser_ask_pending(&thread, &request));
            let took_over = self.check().is_err();
            if !pending || took_over || started.elapsed() > ASK_WAIT {
                workspace.update(self.cx, |ws, cx| ws.browser_withdraw(&thread, &request, cx));
                if took_over {
                    return Err(TOOK_OVER.into());
                }
                break Decision::Deny;
            }
            self.sleep(Duration::from_millis(200)).await;
        };
        match decision {
            Decision::Allow => Ok(()),
            Decision::AllowForSession => {
                let _ = self
                    .this
                    .update(self.cx, |p, _| p.allowed.insert((thread, kind)));
                Ok(())
            }
            Decision::Deny => Err(format!(
                "The user didn't allow it ({title}). Don't try another way round; ask them if it's needed."
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's own `test` attribute would come with it.
    use super::{
        HandHolding, Sensitive, StatusBar, ask_text, find_bar, fit, needs_asking, phone_time,
        picture_size, starting, tools,
    };
    use gpui_kit::{px, size};
    use serde_json::json;
    use trek_phone::device::Clock;

    #[test]
    fn a_pictures_size_comes_from_its_header() {
        let img = image::RgbImage::from_pixel(37, 81, image::Rgb([200, 40, 90]));
        for format in [image::ImageFormat::Jpeg, image::ImageFormat::Png] {
            let mut bytes = std::io::Cursor::new(Vec::new());
            img.write_to(&mut bytes, format).unwrap();
            let bytes = bytes.into_inner();
            let decoded = image::load_from_memory(&bytes).unwrap();
            assert_eq!(
                picture_size(&bytes),
                Some((decoded.width(), decoded.height()))
            );
            assert_eq!(picture_size(&bytes), Some((37, 81)));
        }
        assert_eq!(picture_size(b"not a picture"), None);
        assert_eq!(picture_size(&[]), None);
    }

    #[test]
    fn approvals_follow_the_access_level() {
        use HandHolding::*;
        assert!(!needs_asking(Sensitive::Install, FullAccess));
        assert!(!needs_asking(Sensitive::Files, FullAccess));
        assert!(needs_asking(Sensitive::Install, Auto));
        assert!(!needs_asking(Sensitive::Files, Auto));
        for level in [Supervised, AutoAcceptEdits] {
            assert!(needs_asking(Sensitive::Install, level));
            assert!(needs_asking(Sensitive::Files, level));
        }
    }

    #[test]
    fn approval_cards_name_the_file_and_where_it_goes() {
        let (title, detail) = ask_text(
            "phone_push_file",
            &json!({"local": "/tmp/notes.txt", "remote": "/Documents/notes.txt", "app": "com.x"}),
        );
        assert_eq!(title, "Copy notes.txt to your iPhone");
        assert_eq!(detail, "/tmp/notes.txt → /Documents/notes.txt in com.x");
        let (title, _) = ask_text("phone_install", &json!({"path": "/b/Trek.app"}));
        assert_eq!(title, "Install Trek.app on your iPhone");
        let (title, _) = ask_text("phone_remove_file", &json!({"path": "/Downloads/a.txt"}));
        assert_eq!(title, "Remove a.txt from your iPhone");
    }

    #[test]
    fn the_screen_fits_the_area_keeping_its_shape() {
        let area = size(px(400.), px(900.));
        let s = fit(area, 1320., 2868.).unwrap();
        let ratio = s.size.height.as_f32() / s.size.width.as_f32();
        assert!((ratio - 2868. / 1320.).abs() < 0.01);
        assert!(s.size.width.as_f32() <= 400. - 36. + 0.5);
        assert!(fit(size(px(50.), px(50.)), 1320., 2868.).is_none());
    }

    /// A 440×956 frame (a third of the cable's) with a flat status bar and "9:41" drawn as four
    /// blocks in the left ear, then a Focus glyph.
    fn frame(background: [u8; 3], ink: [u8; 3]) -> image::RgbImage {
        let mut img = image::RgbImage::from_pixel(440, 956, image::Rgb(background));
        let glyphs = [(54, 64), (66, 68), (71, 80), (82, 87), (94, 112)];
        for (x0, x1) in glyphs {
            for x in x0..=x1 {
                for y in 26..39 {
                    img.put_pixel(x, y, image::Rgb(ink));
                }
            }
        }
        img
    }

    #[test]
    fn the_status_bar_clock_is_found_and_covered_within_the_bar() {
        let bar = find_bar(&frame([10, 10, 11], [255, 255, 255])).expect("a status bar");
        assert!((bar.left - 54. / 440.).abs() < 0.003, "{bar:?}");
        assert!((bar.right - 87. / 440.).abs() < 0.003, "{bar:?}");
        assert!(bar.next.is_some_and(|n| (n - 94. / 440.).abs() < 0.003));
        assert!(bar.light_ink);
        // Never below the status bar (about 6% of the height on these phones).
        assert!(bar.top >= 0.012 && bar.bottom <= 0.052, "{bar:?}");
        // A longer time widens the cover, but not over the Focus glyph.
        let (x0, x1) = bar.cover("12:58");
        assert!(x0 < bar.left && x1 < bar.next.unwrap(), "{x0} {x1} {bar:?}");
        // Dark text on a light app.
        let bar = find_bar(&frame([250, 250, 250], [0, 0, 0])).unwrap();
        assert!(!bar.light_ink);
    }

    #[test]
    fn no_status_bar_means_nothing_drawn() {
        // A full-screen video: content right up to the top.
        let mut img = image::RgbImage::new(440, 956);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x * 7 % 255) as u8, (y * 13 % 255) as u8, 90]);
        }
        assert_eq!(find_bar(&img), None);
        // Landscape.
        assert_eq!(find_bar(&image::RgbImage::new(956, 440)), None);
        // A flat top with nothing in it (the status bar hidden).
        assert_eq!(
            find_bar(&image::RgbImage::from_pixel(
                440,
                956,
                image::Rgb([20, 20, 20])
            )),
            None
        );
    }

    #[test]
    fn the_time_is_the_phones() {
        use chrono::TimeZone as _;
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 11, 1, 12, 52, 0)
            .unwrap();
        let ny = Clock {
            utc_offset: -5 * 3600,
            hours24: false,
        };
        assert_eq!(phone_time(Some(ny), now), "7:52");
        let berlin = Clock {
            utc_offset: 3600,
            hours24: true,
        };
        assert_eq!(phone_time(Some(berlin), now), "13:52");
        let midnight = chrono::Utc.with_ymd_and_hms(2026, 11, 1, 5, 3, 0).unwrap();
        assert_eq!(phone_time(Some(ny), midnight), "12:03");
        let _ = StatusBar::cover;
    }

    #[test]
    fn every_tool_has_a_strip_word() {
        for t in tools::TOOL_NAMES {
            assert!(!starting(t).is_empty());
        }
        assert_eq!(starting("phone_tap"), "Tapping…");
    }
}
