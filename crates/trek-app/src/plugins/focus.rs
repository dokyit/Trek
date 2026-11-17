//! `trek:focus-timer`: a focus session as a walk round the composer. Before a walk, a chip on
//! the composer's top edge offers one (its time steps through 15, 25, 45 and 60 minutes); once
//! it starts, the hiker sets off from the chip and walks the card's border clockwise, feet on the
//! edge, turning at each corner, the trail it has walked drawn in the theme's accent and the
//! trail ahead in dots. Back at the chip, time's up: a toast and the alert sound say so, and the
//! chip offers a break (a shorter walk, in green) or another walk. A session is kept in the
//! placing plugin's data folder by the wall clock, so it outlasts a restart.
//!
//! Only time places it: where the hiker is, and which of its four steps it's on (a new one every
//! four art pixels, as the working bar's walk), come from the session's time on the hiker's 16 fps
//! clock. The ring redraws only when that changes (about once a second for a 25-minute session),
//! and only after it was drawn: a window that isn't drawn (covered, minimised, another screen in
//! front) stops asking, and the next frame picks up where time has got to. With Reduce motion the
//! hiker stands as it goes.

use crate::assets::Lucide;
use crate::composer::look;
use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::{Duration, Instant};

/// A session's length when the plugin names none.
pub const DEFAULT_MINUTES: f32 = 25.;
/// A break's length when the plugin names none.
pub const DEFAULT_BREAK: f32 = 5.;
/// The lengths the chip's time steps through before a session starts (the plugin's own is
/// added among them).
const LENGTHS: [f32; 4] = [15., 25., 45., 60.];
/// The hiker's clock: what a moment of the session is rounded to.
const FRAME: Duration = Duration::from_millis(1000 / trek_mascot::FPS);
/// Room the composer leaves above and below its card while the ring is on: the hiker stands
/// outside the edge it walks.
pub const CLEARANCE: f32 = 20.;
/// The chip's height; it sits on the top edge, half above it.
const CHIP_H: f32 = 24.;
/// Where the chip starts along the top edge, from the corner's end.
const CHIP_IN: f32 = 10.;
/// Dots of the trail ahead, apart; and of the trail walked (closer: it reads as a line).
const AHEAD_EVERY: f32 = 7.;
const WALKED_EVERY: f32 = 2.5;
/// The file in the placing plugin's data folder a session is kept in.
const KEPT: &str = "focus.json";

/// Where a session is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Session {
    /// Not started: the chip offers one.
    Ready,
    /// Under way since `since`, with `before` walked already (before a pause).
    Running {
        since: Instant,
        before: Duration,
    },
    Paused {
        walked: Duration,
    },
    /// A focus walk back at the start: the chip offers a break or another walk.
    Done,
}

/// What the walk is for: focus, or the break after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Focus,
    Break,
}

/// The focus timer: one for the app, made when a plugin first places it.
pub struct Focus {
    /// A focus walk's length: the plugin's, or what the chip was stepped to.
    pub minutes: f32,
    /// A break's length.
    pub break_minutes: f32,
    pub mode: Mode,
    pub session: Session,
    ring: Option<Entity<Ring>>,
    /// Says time's up even when nothing draws the ring.
    _done: Option<Task<()>>,
}

impl Global for Focus {}

impl Focus {
    /// A timer with no walk under way.
    pub fn new(minutes: f32, break_minutes: f32) -> Focus {
        Focus {
            minutes,
            break_minutes,
            mode: Mode::Focus,
            session: Session::Ready,
            ring: None,
            _done: None,
        }
    }

    fn length(&self) -> f32 {
        match self.mode {
            Mode::Focus => self.minutes,
            Mode::Break => self.break_minutes,
        }
    }

    fn total(&self) -> Duration {
        Duration::from_secs_f32(self.length().max(1. / 60.) * 60.)
    }

    /// How much of the session has gone, at `now`.
    pub fn walked(&self, now: Instant) -> Duration {
        match self.session {
            Session::Ready => Duration::ZERO,
            Session::Running { since, before } => before + now.saturating_duration_since(since),
            Session::Paused { walked } => walked,
            Session::Done => self.total(),
        }
    }

    /// The share of the session gone, 0 to 1, on the hiker's clock.
    pub fn share(&self, now: Instant) -> f32 {
        let walked = self.walked(now);
        let frames = (walked.as_secs_f32() / FRAME.as_secs_f32()).floor();
        (frames * FRAME.as_secs_f32() / self.total().as_secs_f32()).clamp(0., 1.)
    }

    /// What's left, in whole seconds (rounded up, so it reads 25:00 at the start and 0:00 at the
    /// end).
    pub fn left(&self, now: Instant) -> u64 {
        let left = self.total().saturating_sub(self.walked(now));
        left.as_secs() + u64::from(left.subsec_nanos() > 0)
    }

    /// The session as it's kept on disk: by the wall clock (`at_ms`), so a restart picks it up
    /// where time has got to.
    pub fn kept(&self, now: Instant, at_ms: i64) -> serde_json::Value {
        let state = match self.session {
            Session::Ready => "ready",
            Session::Running { .. } => "running",
            Session::Paused { .. } => "paused",
            Session::Done => "done",
        };
        serde_json::json!({
            "mode": if self.mode == Mode::Break { "break" } else { "focus" },
            "minutes": self.minutes,
            "state": state,
            "walked_ms": self.walked(now).as_millis() as u64,
            "at_ms": at_ms,
        })
    }

    /// Take up `kept` (`Focus::kept`) at `now`, the wall clock reading `at_ms`: a walk that was
    /// under way carries on as if Trek had stayed open, or is over if its time ran out meanwhile.
    pub fn take_up(&mut self, kept: &serde_json::Value, now: Instant, at_ms: i64) {
        if let Some(m) = kept["minutes"].as_f64() {
            self.minutes = (m as f32).clamp(1., 180.);
        }
        self.mode = if kept["mode"] == "break" {
            Mode::Break
        } else {
            Mode::Focus
        };
        let walked = Duration::from_millis(kept["walked_ms"].as_u64().unwrap_or(0));
        let away =
            Duration::from_millis((at_ms - kept["at_ms"].as_i64().unwrap_or(at_ms)).max(0) as u64);
        let total = self.total();
        self.session = match kept["state"].as_str() {
            Some("running") if walked + away < total => Session::Running {
                since: now,
                before: walked + away,
            },
            // A focus walk that ended while Trek was closed is done; a break is simply over.
            Some("running" | "done") if self.mode == Mode::Focus => Session::Done,
            Some("paused") => Session::Paused { walked },
            _ => {
                self.mode = Mode::Focus;
                Session::Ready
            }
        };
    }
}

/// The placing plugin's timer: (its id, a walk's minutes, a break's minutes).
fn placed(cx: &App) -> Option<(String, f32, f32)> {
    super::slot(trek_core::plugins::Slot::ComposerBorder, cx)
        .iter()
        .find_map(|p| match &p.tree.kind {
            trek_core::plugins::ui::Kind::Trek(
                trek_core::plugins::ui::Component::FocusTimer,
                props,
            ) => {
                let n = |k: &str, or: f32| {
                    props
                        .get(k)
                        .and_then(|m| m.as_f64())
                        .map_or(or, |m| m as f32)
                };
                Some((
                    p.plugin.to_string(),
                    n("minutes", DEFAULT_MINUTES),
                    n("break", DEFAULT_BREAK),
                ))
            }
            _ => None,
        })
}

/// The minutes the running plugins' focus timers ask for (the first one's).
fn minutes(cx: &App) -> f32 {
    placed(cx).map_or(DEFAULT_MINUTES, |p| p.1)
}

fn ensure(cx: &mut App) {
    if !cx.has_global::<Focus>() {
        let (minutes, break_minutes) =
            placed(cx).map_or((DEFAULT_MINUTES, DEFAULT_BREAK), |p| (p.1, p.2));
        cx.set_global(Focus::new(minutes, break_minutes));
    }
}

/// A plugin places the timer: the session kept from before (Trek quit mid-walk) is read off the
/// UI thread and taken up where time has got to. Once per launch.
pub fn restore(ws: &Entity<Workspace>, cx: &mut App) {
    let Some((plugin, _, _)) = placed(cx) else {
        return;
    };
    if cx.has_global::<Focus>() {
        return;
    }
    ensure(cx);
    let ws = ws.clone();
    cx.spawn(async move |cx| {
        let file = trek_core::plugins::data_dir(&plugin).join(KEPT);
        let kept: Option<serde_json::Value> = cx
            .background_spawn(async move {
                std::fs::read_to_string(file)
                    .ok()
                    .and_then(|t| serde_json::from_str(&t).ok())
            })
            .await;
        let Some(kept) = kept else { return };
        cx.update(|cx| {
            let now = crate::motion::now(cx);
            let f = cx.global_mut::<Focus>();
            // Started meanwhile: what the user did now wins.
            if f.session != Session::Ready {
                return;
            }
            f.take_up(&kept, now, trek_core::store::now_ms());
            arm(&ws, cx);
        });
    })
    .detach();
}

/// Start, pause (a running walk) or carry on (a paused one), or stop it. `toggle` starts,
/// pauses or carries on, whichever fits (the plugin's key); `break` walks the break; `length`
/// steps a walk's length before it starts.
pub fn control(verb: &str, ws: &Entity<Workspace>, cx: &mut App) {
    ensure(cx);
    let now = crate::motion::now(cx);
    let plugin_minutes = minutes(cx);
    let f = cx.global_mut::<Focus>();
    let verb = match (verb, f.session) {
        ("toggle", Session::Running { .. }) => "pause",
        ("toggle", _) => "start",
        (v, _) => v,
    };
    match (verb, f.session) {
        ("start", Session::Ready | Session::Done) => {
            f.mode = Mode::Focus;
            f.session = Session::Running {
                since: now,
                before: Duration::ZERO,
            };
        }
        ("break", _) => {
            f.mode = Mode::Break;
            f.session = Session::Running {
                since: now,
                before: Duration::ZERO,
            };
        }
        ("start" | "pause", Session::Paused { walked }) => {
            f.session = Session::Running {
                since: now,
                before: walked,
            }
        }
        ("pause", Session::Running { .. }) => {
            f.session = Session::Paused {
                walked: f.walked(now),
            }
        }
        ("length", Session::Ready | Session::Done) => {
            let mut lengths: Vec<f32> = LENGTHS.to_vec();
            if !lengths.contains(&plugin_minutes) {
                lengths.push(plugin_minutes);
                lengths.sort_by(f32::total_cmp);
            }
            let current = f.minutes;
            f.minutes = lengths
                .iter()
                .copied()
                .find(|m| *m > current + 0.01)
                .unwrap_or(lengths[0]);
            f.mode = Mode::Focus;
            f.session = Session::Ready;
        }
        ("stop", _) => {
            f.mode = Mode::Focus;
            f.session = Session::Ready;
        }
        _ => return,
    }
    keep(cx);
    arm(ws, cx);
}

/// Keep the session on disk (off the UI thread), for the next launch.
fn keep(cx: &mut App) {
    let Some((plugin, _, _)) = placed(cx) else {
        return;
    };
    let now = crate::motion::now(cx);
    let text = cx
        .global::<Focus>()
        .kept(now, trek_core::store::now_ms())
        .to_string();
    cx.background_spawn(async move {
        let dir = trek_core::plugins::data_dir(&plugin);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(KEPT), text);
    })
    .detach();
}

/// Arm the timer that says time's up (even if the ring isn't drawn then), and redraw.
fn arm(ws: &Entity<Workspace>, cx: &mut App) {
    let now = crate::motion::now(cx);
    let f = cx.global::<Focus>();
    let left = match f.session {
        Session::Running { .. } => Some(f.total().saturating_sub(f.walked(now))),
        _ => None,
    };
    let ws_weak = ws.downgrade();
    let done = left.map(|left| {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(left).await;
            cx.update(|cx| {
                if !cx.has_global::<Focus>() {
                    return;
                }
                let f = cx.global_mut::<Focus>();
                if !matches!(f.session, Session::Running { .. }) {
                    return;
                }
                let minutes = f.minutes;
                // A walk ends at the trailhead, offering a break; a break ends ready for the next.
                let message = match f.mode {
                    Mode::Focus => {
                        f.session = Session::Done;
                        format!(
                            "Back at the trailhead: {} focused. Take a break?",
                            minutes_words(minutes)
                        )
                    }
                    Mode::Break => {
                        f.mode = Mode::Focus;
                        f.session = Session::Ready;
                        "Break's over: ready for the next walk".to_string()
                    }
                };
                if let Some(ring) = f.ring.clone() {
                    ring.update(cx, |_, cx| cx.notify());
                }
                keep(cx);
                crate::system::play_alert_sound();
                if let Some(ws) = ws_weak.upgrade() {
                    ws.update(cx, |_, cx| {
                        cx.emit(WorkspaceEvent::Toast {
                            message,
                            undo: None,
                        });
                        cx.notify();
                    });
                }
            });
        })
    });
    cx.global_mut::<Focus>()._done = done;
    if let Some(ring) = cx.global::<Focus>().ring.clone() {
        ring.update(cx, |_, cx| cx.notify());
    }
    ws.update(cx, |_, cx| cx.notify());
}

/// Take the session `secs` seconds on (the capture build's `plugin focus at <secs>`, so a check
/// sees the hiker part way round without waiting): it's as if it had started that long ago.
pub fn jump(secs: f32, ws: &Entity<Workspace>, cx: &mut App) {
    control("start", ws, cx);
    let now = crate::motion::now(cx);
    let f = cx.global_mut::<Focus>();
    f.session = Session::Running {
        since: now,
        before: Duration::from_secs_f32(secs.max(0.)),
    };
    arm(ws, cx);
}

fn minutes_words(m: f32) -> String {
    let m = m.round() as u32;
    if m == 1 {
        "1 minute".into()
    } else {
        format!("{m} minutes")
    }
}

fn clock(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The ring and its chip, drawn over the composer's card (`Composer`'s card is the parent, so it
/// fills it). `None` unless a running plugin places the timer on the composer's edge.
pub fn ring(ws: &Entity<Workspace>, cx: &mut App) -> Option<AnyElement> {
    if super::slot(trek_core::plugins::Slot::ComposerBorder, cx).is_empty() {
        return None;
    }
    ensure(cx);
    let ring = match cx.global::<Focus>().ring.clone() {
        Some(r) => r,
        None => {
            let ws = ws.clone();
            let r = cx.new(|_| Ring {
                ws,
                tick: None,
                drawn: Default::default(),
                drawn_length: Default::default(),
            });
            cx.global_mut::<Focus>().ring = Some(r.clone());
            r
        }
    };
    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(ring)
            .into_any_element(),
    )
}

/// The chip alone, for a plugin that places the timer somewhere else.
pub fn chip(ws: &Entity<Workspace>, cx: &App) -> AnyElement {
    let (session, mode, left, minutes) = match cx.try_global::<Focus>() {
        Some(f) => {
            let now = crate::motion::now(cx);
            (f.session, f.mode, f.left(now), f.minutes)
        }
        None => (
            Session::Ready,
            Mode::Focus,
            (minutes(cx) * 60.) as u64,
            minutes(cx),
        ),
    };
    chip_element(session, mode, left, minutes, ws, cx)
}

fn chip_element(
    session: Session,
    mode: Mode,
    left: u64,
    minutes: f32,
    ws: &Entity<Workspace>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let tone = match mode {
        Mode::Focus => theme.primary,
        Mode::Break => crate::palette::emerald(cx),
    };
    let button = |id: &'static str, icon: Icon, tip: &'static str, verb: &'static str| {
        let ws = ws.clone();
        Button::new(id)
            .ghost()
            .xsmall()
            .icon(
                icon.size(px(13.))
                    .text_color(theme.foreground.opacity(0.75)),
            )
            .tooltip(tip)
            .on_click(move |_, _, cx| control(verb, &ws, cx))
    };
    let label = match (session, mode) {
        (Session::Done, _) => "Done",
        (Session::Paused { .. }, _) => "Paused",
        (_, Mode::Break) => "Break",
        _ => "Focus",
    };
    let time = match session {
        Session::Ready => clock((minutes * 60.) as u64),
        Session::Running { .. } | Session::Paused { .. } => clock(left),
        Session::Done => minutes_words(minutes),
    };
    // Before a walk the time is a control: each click steps the length (15, 25, 45, 60).
    let time_el = div()
        .id("focus-length")
        .test_support()
        .px(px(3.))
        .rounded(px(trek_palette::scale::radius::TAG))
        .font_family(theme.mono_font_family.clone())
        .text_color(theme.foreground)
        .font_weight(FontWeight::MEDIUM)
        .child(time)
        .when(session == Session::Ready, |el| {
            let ws = ws.clone();
            el.cursor_pointer()
                .hover(|s| s.bg(theme.foreground.opacity(0.08)))
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Change the length")
                        .build(window, cx)
                })
                .on_click(move |_, _, cx| control("length", &ws, cx))
        });
    h_flex()
        .id("focus-chip")
        .test_support()
        .h(px(CHIP_H))
        .pl(px(9.))
        .pr(px(2.))
        .gap(px(5.))
        .rounded_full()
        .bg(look::card_fill(cx))
        .border_1()
        .border_color(match session {
            Session::Ready => theme.border,
            _ => tone.opacity(0.55),
        })
        .text_size(px(crate::scale::text::META))
        .child(
            Icon::new(match mode {
                Mode::Focus => Lucide::Timer,
                Mode::Break => Lucide::Coffee,
            })
            .size(px(12.))
            .text_color(match session {
                Session::Running { .. } => tone,
                Session::Done => crate::palette::emerald(cx),
                _ => theme.muted_foreground,
            }),
        )
        .child(
            div()
                .text_color(theme.muted_foreground)
                .child(label.to_string()),
        )
        .child(time_el)
        .map(|el| match session {
            Session::Ready => el.child(button(
                "focus-start",
                Icon::new(Lucide::Play),
                "Start the walk",
                "start",
            )),
            Session::Running { .. } => el
                .child(button(
                    "focus-pause",
                    Icon::new(Lucide::Pause),
                    "Pause",
                    "pause",
                ))
                .child(button(
                    "focus-stop",
                    Icon::new(Lucide::X),
                    "Stop and reset",
                    "stop",
                )),
            Session::Paused { .. } => el
                .child(button(
                    "focus-resume",
                    Icon::new(Lucide::Play),
                    "Carry on",
                    "start",
                ))
                .child(button(
                    "focus-stop",
                    Icon::new(Lucide::X),
                    "Stop and reset",
                    "stop",
                )),
            Session::Done => el
                .child(button(
                    "focus-break",
                    Icon::new(Lucide::Coffee),
                    "Take a break",
                    "break",
                ))
                .child(button(
                    "focus-again",
                    Icon::new(Lucide::RefreshCw),
                    "Walk again",
                    "start",
                ))
                .child(button("focus-clear", Icon::new(Lucide::X), "Done", "stop")),
        })
        .into_any_element()
}

/// The ring over the composer's card.
pub struct Ring {
    ws: Entity<Workspace>,
    /// The next redraw, armed after each paint.
    tick: Option<Task<()>>,
    /// The chip's width as last laid out: the trail stops short of it.
    drawn: std::rc::Rc<std::cell::Cell<f32>>,
    /// The trail's length as last painted: how far a pixel of the hiker's is in time.
    drawn_length: std::rc::Rc<std::cell::Cell<f32>>,
}

/// The card's edge as a path: where a point `s` points along it is, clockwise from the top edge's
/// start (just past the top-left corner), and which way up the hiker stands there (quarter turns).
struct Edge {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
}

impl Edge {
    fn len(&self) -> f32 {
        2. * (self.w - 2. * self.r) + 2. * (self.h - 2. * self.r) + std::f32::consts::TAU * self.r
    }

    /// The point `s` along, and the hiker's quarter turns there.
    fn at(&self, s: f32) -> (f32, f32, u8) {
        let Edge { x, y, w, h, r } = *self;
        let arc = std::f32::consts::FRAC_PI_2 * r;
        let (top, side) = ((w - 2. * r).max(0.), (h - 2. * r).max(0.));
        let mut s = s.rem_euclid(self.len().max(1.));
        // Each piece: (its length, where along it is → point and turns).
        let corner = |cx: f32, cy: f32, from: f32, t: f32| {
            let a = from + t * std::f32::consts::FRAC_PI_2;
            (cx + r * a.cos(), cy + r * a.sin())
        };
        let pieces: [(f32, &dyn Fn(f32) -> (f32, f32, u8)); 8] = [
            (top, &|d| (x + r + d, y, 0)),
            (arc, &|d| {
                let t = if arc > 0. { d / arc } else { 0. };
                let (px, py) = corner(x + w - r, y + r, -std::f32::consts::FRAC_PI_2, t);
                (px, py, if t < 0.5 { 0 } else { 1 })
            }),
            (side, &|d| (x + w, y + r + d, 1)),
            (arc, &|d| {
                let t = if arc > 0. { d / arc } else { 0. };
                let (px, py) = corner(x + w - r, y + h - r, 0., t);
                (px, py, if t < 0.5 { 1 } else { 2 })
            }),
            (top, &|d| (x + w - r - d, y + h, 2)),
            (arc, &|d| {
                let t = if arc > 0. { d / arc } else { 0. };
                let (px, py) = corner(x + r, y + h - r, std::f32::consts::FRAC_PI_2, t);
                (px, py, if t < 0.5 { 2 } else { 3 })
            }),
            (side, &|d| (x, y + h - r - d, 3)),
            (arc, &|d| {
                let t = if arc > 0. { d / arc } else { 0. };
                let (px, py) = corner(x + r, y + r, std::f32::consts::PI, t);
                (px, py, if t < 0.5 { 3 } else { 0 })
            }),
        ];
        for (len, f) in pieces {
            if s <= len {
                return f(s);
            }
            s -= len;
        }
        (x + r, y, 0)
    }
}

impl Render for Ring {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(f) = cx.try_global::<Focus>() else {
            return div().into_any_element();
        };
        let now = crate::motion::now(cx);
        let session = f.session;
        let share = f.share(now);
        let left = f.left(now);
        let minutes = f.minutes;
        let total = f.total().as_secs_f32();
        let walking = matches!(session, Session::Running { .. });
        let still = !self.ws.read(cx).motion(cx);
        let dark = cx.theme().mode.is_dark();
        let accent = match f.mode {
            Mode::Focus => cx.theme().primary,
            Mode::Break => crate::palette::emerald(cx),
        };
        let mode = f.mode;
        let ahead = cx.theme().foreground.opacity(0.22);
        let radius = look::RADIUS * corner_scale();
        let chip_w = self.drawn.clone();
        let started = !matches!(session, Session::Ready);
        let scale = window.scale_factor();
        let cell = trek_mascot::cell_size(scale);
        // The next moment anything changes: the clock's next second, or the hiker's next pixel.
        if walking {
            let walked = share * total;
            let to_second = 1. - (total - walked).fract();
            // Points a second along a trail of about a typical card's length; the painted length
            // decides exactly, and an early tick only redraws the same.
            let length = self.drawn_length.get().max(400.);
            let speed = length / total;
            let to_pixel = if still || speed <= 0. {
                f32::MAX
            } else {
                cell / speed
            };
            let wait = to_second.min(to_pixel).max(FRAME.as_secs_f32());
            let wait = Duration::from_secs_f32(wait);
            self.tick = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            }));
        } else {
            self.tick = None;
        }
        let length_cell = self.drawn_length.clone();
        div()
            .size_full()
            .relative()
            .child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, _| {
                        let edge = Edge {
                            x: b.origin.x.as_f32(),
                            y: b.origin.y.as_f32(),
                            w: b.size.width.as_f32(),
                            h: b.size.height.as_f32(),
                            r: radius.min(b.size.height.as_f32() / 2.),
                        };
                        // Before a walk only the chip shows: the composer keeps its own edge.
                        if !started {
                            return;
                        }
                        // The journey: from the chip's right end round to its left end.
                        let chip_start = CHIP_IN;
                        let chip_end = chip_start + chip_w.get() + 6.;
                        let journey = (edge.len() - (chip_end - chip_start + 6.)).max(1.);
                        length_cell.set(journey);
                        let from = chip_end;
                        let at = share * journey;
                        let scale = window.scale_factor();
                        // scale: ok, a trail's dot, a whole number of device pixels across
                        let dot = ((2. * scale).round() / scale).max(1. / scale);
                        let mut paint_dot = |s: f32, colour: Hsla| {
                            let (x, y, _) = edge.at(from + s);
                            window.paint_quad(
                                fill(
                                    Bounds::new(
                                        point(
                                            px(trek_mascot::snap(x - dot / 2., scale)),
                                            px(trek_mascot::snap(y - dot / 2., scale)),
                                        ),
                                        size(px(dot), px(dot)),
                                    ),
                                    colour,
                                )
                                // scale: ok, a trail's dot is round
                                .corner_radii(px(dot / 2.)),
                            );
                        };
                        if started {
                            let mut s = 0.;
                            while s < at {
                                paint_dot(s, accent);
                                s += WALKED_EVERY;
                            }
                        }
                        let mut s = if started {
                            at + AHEAD_EVERY
                        } else {
                            AHEAD_EVERY
                        };
                        while s < journey {
                            paint_dot(s, ahead);
                            s += AHEAD_EVERY;
                        }
                        // The hiker, where time has it, on the trail's line.
                        let (x, y, turns) = edge.at(from + at);
                        let pose = if walking && !still {
                            trek_mascot::pose_after(at / cell)
                        } else {
                            trek_mascot::STAND
                        };
                        trek_mascot::paint_turned(
                            point(px(x), px(y)),
                            cell,
                            pose,
                            turns,
                            dark,
                            window,
                        );
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top(px(-CHIP_H / 2.))
                    .left(px(radius.max(0.) + CHIP_IN))
                    .child({
                        let chip_w = self.drawn.clone();
                        div()
                            .child(chip_element(session, mode, left, minutes, &self.ws, cx))
                            .child(
                                canvas(
                                    move |b, _, _| chip_w.set(b.size.width.as_f32()),
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full(),
                            )
                    }),
            )
            .into_any_element()
    }
}
