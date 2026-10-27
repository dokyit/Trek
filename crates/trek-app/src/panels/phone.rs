//! iPhone: the user's real iPhone beside the chat, live, and agents driving it.
//!
//! The screen comes over the USB cable as QuickTime gets it and taps and keys go as a Bluetooth
//! keyboard and mouse the phone pairs with once (`trek_phone::helper`, one process for both);
//! apps and files go over usbmuxd as Finder's do (`trek_phone::device`). No Xcode, no Developer
//! Mode, nothing installed on the phone.
//!
//! Agents' `trek-mcp phone` calls come here one at a time (`RightPanel::phone_call`). One thread
//! holds the phone at a time (`trek_phone::lease`); a strip says who, with Stop to take it back,
//! as the Browser's does. Installing and changing files ask first by the thread's access level,
//! on its approval card (`Workspace::browser_ask`). Trek turns AssistiveTouch on when a tap needs
//! a pointer and off again when the agent lets go or the user takes the phone back.

use crate::assets::Lucide;
use crate::workspace::Workspace;
use crate::workspace::phone_use::PhoneCall;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use trek_agents::Decision;
use trek_core::HandHolding;
use trek_ipc::phone::{self as tools, Sensitive};
use trek_phone::helper::{self, Event, Link, Status};
use trek_phone::hid::{self, Step};
use trek_phone::lease::{Lease, TOOK_OVER};
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

const NOT_PAIRED: &str = "The iPhone isn't paired with Trek's Bluetooth keyboard and mouse yet, so taps and keys can't reach it. The user does this once: on the iPhone, Settings › Bluetooth, tap \"Trek\" under Other Devices (Trek's iPhone tool must be open). Screenshots, apps and files work without it.";

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
    decode_wake: async_channel::Sender<(u32, u32)>,
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
    shots: u64,
    notice: Option<SharedString>,
    _tasks: Vec<Task<()>>,
}

impl PhonePanel {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Frames decode on a thread of their own; only the newest waits.
        let decode_slot: Arc<Mutex<Option<Vec<u8>>>> = Arc::default();
        let (wake_tx, wake_rx) = async_channel::bounded::<(u32, u32)>(1);
        let (frames_tx, frames_rx) = async_channel::bounded::<Frame>(2);
        let (slot, renderer) = (decode_slot.clone(), cx.svg_renderer());
        let _ = std::thread::Builder::new()
            .name("trek-phone-decode".into())
            .spawn(move || {
                while let Ok((w, h)) = wake_rx.recv_blocking() {
                    let Some(jpeg) = slot.lock().unwrap().take() else {
                        continue;
                    };
                    let image = Image::from_bytes(ImageFormat::Jpeg, jpeg);
                    if let Ok(image) = image.to_image_data(renderer.clone()) {
                        let frame = Frame {
                            image,
                            width: w as f32,
                            height: h as f32,
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
            shots: 0,
            notice: None,
            _tasks: vec![frames, tick],
        }
    }

    /// Shown or hidden by the right panel: frames stream only while it's on screen.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if let Some(link) = &self.link {
            link.frames(visible, LIVE_FPS);
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
        let wanted = self.visible || self.busy() || self.lease.holder(Instant::now()).is_some();
        if !wanted {
            return;
        }
        self.manage_link(window, cx);
        if self.refreshed.is_none_or(|t| t.elapsed() >= DEVICE_REFRESH) {
            self.refresh_device(window, cx);
        }
        if let Some(link) = &self.link {
            link.ask_status();
        }
        // The strip goes a while after the last call.
        if self.last.as_ref().is_some_and(|a| {
            !self.busy() && a.at.elapsed() >= STRIP_LINGER && a.at.elapsed() < STRIP_LINGER + TICK
        }) {
            cx.notify();
        }
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
                let assistive = phones
                    .as_ref()
                    .ok()
                    .and_then(|p| p.first())
                    .map(|p| device::assistive_touch(Some(&p.udid)).ok());
                (phones, assistive.flatten())
            },
            |this, (phones, assistive), _, cx| {
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
                cx.notify();
            },
        );
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
                *self.decode_slot.lock().unwrap() = Some(jpeg);
                let _ = self.decode_wake.try_send((width, height));
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
                cx.notify();
            }
        }
    }

    fn show_frame(&mut self, frame: Frame, window: &mut Window, cx: &mut Context<Self>) {
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
        if !self.status.input_ready() {
            self.notice = Some(NOT_PAIRED.into());
            cx.notify();
            return;
        }
        let (Some(a), Some(b), Some(link)) = (
            self.to_fraction(start),
            self.to_fraction(p),
            self.link.clone(),
        ) else {
            return;
        };
        self.ensure_assistive(window, cx);
        let moved = ((p.x - start.x).as_f32().powi(2) + (p.y - start.y).as_f32().powi(2)).sqrt();
        let steps = if moved < TAP_SLOP {
            hid::tap(a.0, a.1, 80)
        } else {
            hid::swipe(a, b, 300)
        };
        let _ = link.hid(&steps);
    }

    fn home(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        match self.link.clone().filter(|_| self.status.input_ready()) {
            Some(link) => {
                let _ = link.hid(&hid::home());
            }
            None => {
                self.notice = Some(NOT_PAIRED.into());
                cx.notify();
            }
        }
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

    // MARK: drawing

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (name, detail) = match &self.phone {
            Some(p) => (
                if p.name.is_empty() {
                    "iPhone".to_string()
                } else {
                    p.name.clone()
                },
                format!("iOS {} · {}", p.ios, if p.usb { "USB" } else { "Wi-Fi" }),
            ),
            None => (
                "No iPhone".to_string(),
                "Plug it in with a data cable, unlock it and trust this Mac".to_string(), // words: ok, the iPhone tool is macOS only
            ),
        };
        h_flex()
            .h(px(40.))
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
                div()
                    .flex_shrink_0()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(name),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(detail),
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

    /// Screen, keyboard and mouse, AssistiveTouch: each a dot and a word.
    fn chips(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (green, amber, red) = (
            crate::palette::emerald(cx),
            crate::palette::amber(cx),
            crate::palette::red(cx),
        );
        let s = &self.status;
        let screen = if self.link.is_none() || s.camera.is_empty() {
            (theme.muted_foreground, "Starting…")
        } else if s.camera == "undetermined" {
            (amber, "Allow the camera when macOS asks")
        } else if s.camera == "denied" {
            (red, "Allow Camera for Trek")
        } else if s.screen.is_empty() {
            (amber, "No screen over USB")
        } else if s.frames == 0 || s.frame_age > 30. {
            (amber, "Locked or asleep")
        } else {
            (green, "Live")
        };
        let input = match s.bluetooth.as_str() {
            _ if s.input_ready() => (green, "Paired"),
            "advertising" => (amber, "Pair in iPhone Bluetooth"),
            "off" => (red, "Bluetooth is off"),
            "not allowed" => (red, "Allow Bluetooth for Trek"),
            "" | "starting" => (
                theme.muted_foreground,
                "Starting… (allow Bluetooth if asked)",
            ),
            _ => (red, "Bluetooth unavailable"),
        };
        let at = match self.assistive {
            Some(true) => (green, "On"),
            Some(false) => (theme.muted_foreground, "Off (on when needed)"),
            None => (theme.muted_foreground, "—"),
        };
        let chip = |label: &'static str, (color, word): (Hsla, &'static str)| {
            h_flex()
                .gap(px(5.))
                .child(div().size(px(6.)).rounded_full().bg(color))
                .child(div().text_color(theme.muted_foreground).child(label))
                .child(div().child(word))
        };
        h_flex()
            .flex_wrap()
            .px_3()
            .py(px(6.))
            .gap_x_4()
            .gap_y_1()
            .text_xs()
            .border_b_1()
            .border_color(theme.border)
            .child(chip("Screen", screen))
            .child(chip("Keyboard & mouse", input))
            .child(chip("AssistiveTouch", at))
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
                .text_xs()
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

    /// Steps for the user while the phone isn't paired, or Trek can't see it.
    fn setup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let lines: Vec<&str> = if self.phone.is_none() {
            vec![
                "Plug the iPhone into this Mac with a data cable (a charge-only cable gives no screen).", // words: ok
                "Unlock it and tap Trust.",
            ]
        } else if self.status.bluetooth == "advertising" {
            vec![
                "To tap and type from Trek, pair once: on the iPhone, Settings › Bluetooth, tap \"Trek\" under Other Devices.",
                "Trek turns AssistiveTouch on when it needs a pointer, and off again after. Turn Always Show Menu off in Settings › Accessibility › Touch › AssistiveTouch to keep its button out of the way.",
            ]
        } else {
            return None;
        };
        Some(
            v_flex()
                .mx_3()
                .mt_3()
                .p_3()
                .gap_1()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.foreground.opacity(0.03))
                .text_xs()
                .children(lines.into_iter().map(|l| div().child(l)))
                .into_any_element(),
        )
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
        let waiting = match &self.helper {
            Some(Err(e)) => Some(e.clone()),
            _ if self.frame.is_none() => Some(
                if self.phone.is_none() {
                    "Waiting for an iPhone on the cable…"
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
            .when(self.status.input_ready(), |el| el.cursor_pointer())
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
                let radius = px(0.11 * s.size.width.as_f32());
                el.child(
                    div()
                        .absolute()
                        .left(s.origin.x - px(6.))
                        .top(s.origin.y - px(6.))
                        .w(s.size.width + px(12.))
                        .h(s.size.height + px(12.))
                        .rounded(radius + px(6.))
                        .bg(rgb(0x0B0B0D))
                        .border_1()
                        .border_color(theme.border)
                        .shadow_lg(),
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
                        .text_sm()
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
        v_flex()
            .size_full()
            .child(self.header(cx))
            .child(self.chips(cx))
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
                        .child(div().flex_1().min_w_0().text_xs().child(n))
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
            .children(self.setup(cx))
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
                let (x, y) = self.point(&p, "x", "y")?;
                let hold = p["hold_ms"].as_u64().unwrap_or(80).min(10_000) as u32;
                self.input(hid::tap(x, y, hold), true).await?;
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
                let from = self.point(&p, "x1", "y1")?;
                let to = self.point(&p, "x2", "y2")?;
                let ms = p["duration_ms"].as_u64().unwrap_or(300).clamp(60, 5000) as u32;
                self.input(hid::swipe(from, to, ms), true).await?;
                text("Swiped. Take a screenshot to see where it went.")
            }
            "phone_type" => {
                let mut steps = hid::type_text(&s("text"))?;
                if p["submit"].as_bool() == Some(true) {
                    steps.extend(hid::key("enter")?);
                }
                self.input(steps, false).await?;
                text(format!("Typed {} characters.", s("text").chars().count()))
            }
            "phone_key" => {
                self.input(hid::key(&s("key"))?, false).await?;
                text(format!("Pressed {}.", s("key")))
            }
            "phone_home" => {
                self.input(hid::home(), false).await?;
                text("Went to the Home Screen.")
            }
            "phone_open_app" => {
                let name = s("name");
                if name.trim().is_empty() {
                    return Err("Which app? Give its name.".into());
                }
                let mut steps = hid::home();
                steps.push(Step::Wait(800));
                steps.extend(hid::key("cmd+space")?);
                steps.push(Step::Wait(700));
                steps.extend(hid::type_text(name.trim())?);
                steps.push(Step::Wait(1000));
                steps.extend(hid::key("enter")?);
                self.input(steps, false).await?;
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

    /// A point of the latest screenshot as fractions of the screen.
    fn point(&mut self, p: &Value, kx: &str, ky: &str) -> Result<(f64, f64), String> {
        let (x, y) = (
            p[kx].as_f64().ok_or(format!("{kx} is missing"))?,
            p[ky].as_f64().ok_or(format!("{ky} is missing"))?,
        );
        let size = self
            .this
            .update(self.cx, |p, _| p.shot_size)
            .map_err(gone)?;
        let (w, h) = size.unwrap_or((0., 0.));
        hid::fraction(x, y, w, h)
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
        self.sleep(Duration::from_millis(200)).await;
        let (status, holder) = self
            .this
            .update(self.cx, |p, _| {
                (
                    p.status.clone(),
                    p.lease.holder(Instant::now()).map(|h| h.title.clone()),
                )
            })
            .map_err(gone)?;
        let phone = phones?.into_iter().next().ok_or(
            "No iPhone is connected: ask the user to plug it in, unlock it and trust this Mac.", // words: ok
        )?;
        let screen = if status.camera == "denied" {
            "not allowed (System Settings › Privacy & Security › Camera › Trek)".to_string()
        } else if status.screen.is_empty() {
            "no screen over USB (it needs the cable, not Wi-Fi)".to_string()
        } else if status.frames == 0 || status.frame_age > 30. {
            "no picture: the iPhone is locked or its screen is off; ask the user to unlock it"
                .to_string()
        } else {
            format!("live, {}×{} pixels", status.width, status.height)
        };
        let input = if status.input_ready() {
            "paired".to_string()
        } else {
            match status.bluetooth.as_str() {
                "advertising" => format!("not paired. {NOT_PAIRED}"),
                "" => "the helper isn't running".to_string(),
                other => format!("Bluetooth {other}"),
            }
        };
        text(format!(
            "{} (iOS {}, {}, over {}).\nScreen: {screen}.\nKeyboard and mouse: {input}.\nAssistiveTouch: {} (Trek turns it on for taps).\nHeld by: {}.",
            phone.name,
            phone.ios,
            phone.model,
            if phone.usb { "USB" } else { "Wi-Fi" },
            match assistive {
                Some(true) => "on",
                Some(false) => "off",
                None => "unknown",
            },
            holder.unwrap_or_else(|| "nobody".into())
        ))
    }

    async fn screenshot(&mut self, edge: u32) -> Result<Value, String> {
        let link = self.link().await?;
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
        let n = self
            .this
            .update(self.cx, |p, _| {
                p.shots += 1;
                p.shots
            })
            .map_err(gone)?;
        let dir = trek_core::paths::data_dir().join("phone-shots");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{}-{n}.png", std::process::id()));
        let rx = link.shot(&path, edge);
        let shot = self
            .within(rx, Duration::from_secs(10))
            .await
            .ok_or("The iPhone's screen didn't come in time.")???;
        let size = (shot.width as f64, shot.height as f64);
        self.this
            .update(self.cx, |p, _| p.shot_size = Some(size))
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
        let ready = self
            .this
            .update(self.cx, |p, _| p.status.input_ready())
            .map_err(gone)?;
        if !ready {
            return Err(NOT_PAIRED.into());
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
            return Err(NOT_PAIRED.into());
        }
        Ok(())
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
    use super::{HandHolding, Sensitive, ask_text, fit, needs_asking, starting, tools};
    use gpui_kit::{px, size};
    use serde_json::json;

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

    #[test]
    fn every_tool_has_a_strip_word() {
        for t in tools::TOOL_NAMES {
            assert!(!starting(t).is_empty());
        }
        assert_eq!(starting("phone_tap"), "Tapping…");
    }
}
