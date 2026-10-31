//! The card behind the composer's context ring: what fills the context window, how full the
//! plan's usage windows are, and Compact now.
//!
//! What the agent reports is shown as it reports it: the window's size and what's used. It
//! reports no breakdown, so the card estimates one from what Trek knows (the transcript's text,
//! the skill prompts and MCP servers a session is given) and says which rows are estimates. A
//! row Trek has nothing to go on for is left out rather than made up.

use crate::composer::Composer;
use crate::palette;
use crate::workspace::{Scope, Workspace};
use gpui_kit::base::{
    Align, MotionStatus, POPUP_PRIORITY, Placement, Positioner, Transition, transition,
    transition_with_status,
};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Duration;
use trek_agents::{CommandKind, UsageLimit};
use trek_core::AgentId;
use trek_core::store::{Item, Thread};

/// Roughly four characters to a token: how the transcript's share is estimated.
const CHARS_PER_TOKEN: usize = 4;
/// What one configured skill prompt (its name and description) costs the agent, roughly.
const SKILL_TOKENS: u64 = 80;
/// What one MCP server's tool definitions cost the agent, roughly.
const MCP_SERVER_TOKENS: u64 = 900;
/// The most an agent's own prompt and built-in tool definitions take, roughly (Claude Code's
/// are about 15–25k): past that, what nothing else accounts for is conversation Trek's
/// transcript doesn't hold in full (file reads, images, tool output it trimmed).
const SYSTEM_TOKENS: u64 = 24_000;
/// The segmented bar's height.
const BAR: f32 = 8.;
/// Below this share of the context window the ring stays out of the way.
pub const QUIET: f32 = 0.2;
/// From this share the ring says how much is used.
pub const NEAR: f32 = 0.6;
/// What the agent keeps of the window so compacting has room: a thirtieth, as an estimate — no
/// agent reports its reserve, so the card says it's one.
const RESERVE: f64 = 1. / 30.;
/// How long the messages column takes to fold up once a compaction lands.
const COLLAPSE: Duration = Duration::from_millis(1_100);
/// A hover's cross-fade, and the card's and the flyout's open.
const FADE: Duration = Duration::from_millis(140);
/// The most chunks the messages segment splits into while compacting.
const BLOCKS: u64 = 15;

/// How full the window is, 0–1.
pub fn fraction(used: u64, window: u64) -> f32 {
    (used as f32 / window.max(1) as f32).clamp(0., 1.)
}

/// "604.1k", "33k", "1M": a decimal only where it tells.
pub fn fmt_tokens(n: u64) -> String {
    let (v, suffix) = match n {
        0..=999 => return n.to_string(),
        // Up to what still rounds below 1000 of the unit; from there it's the next one.
        ..=999_949 => (n as f64 / 1_000., "k"),
        ..=999_949_999 => (n as f64 / 1_000_000., "M"),
        _ => (n as f64 / 1_000_000_000., "B"),
    };
    let v = (v * 10.).round() / 10.;
    if v.fract() == 0. {
        format!("{v:.0}{suffix}")
    } else {
        format!("{v:.1}{suffix}")
    }
}

/// "363k": whole thousands from ten thousand on, for a figure said in passing.
fn fmt_round(n: u64) -> String {
    if (10_000..999_500).contains(&n) {
        format!("{}k", (n + 500) / 1_000)
    } else {
        fmt_tokens(n)
    }
}

/// The card's colours. On Paper they are the design's own; on Night the same roles. The greys are
/// the theme's foreground at the design's strength over white, so both themes get them.
#[derive(Clone, Copy)]
struct Ink {
    /// Values and percents (#1A1A1A).
    text: Hsla,
    /// Labels (#606060).
    muted: Hsla,
    /// A disabled button's label (#969696).
    faint: Hsla,
    /// The tooltip's middle dot (#999999).
    dot: Hsla,
    surface: Hsla,
    /// The card's edge (#DEDEDE).
    edge: Hsla,
    /// The tooltip's edge (#ECECEC).
    tip_edge: Hsla,
    /// The flyout's edge (#E1E1E1).
    fly_edge: Hsla,
    /// Between the card's sections (#EFEFEF).
    rule: Hsla,
    /// An empty bar (#ECECEC).
    track: Hsla,
    /// The ring's hover circle, a disabled button (#F4F4F4).
    well: Hsla,
    /// The ring's empty arc (#D6D6D6).
    ring_track: Hsla,
    /// The Reserved block (#D2D2D2).
    reserved: Hsla,
    /// The Free swatch's edge (#D4D4D4).
    free_edge: Hsla,
    /// The ring's arc (#4A7FE5).
    arc: Hsla,
    /// A limit's fill (#4281ED).
    fill: Hsla,
    /// A split's second share (#9EC6F6).
    fill_soft: Hsla,
    /// The message chunk being summarized (#3D6BDF).
    processing: Hsla,
    /// What summarizing has folded (#2F4FB0).
    stripe: Hsla,
    /// Compact now: its fill (#E8F2FA), its hover (#E0E9FB), its label (#2D468F).
    action: Hsla,
    action_hover: Hsla,
    action_text: Hsla,
    /// The check by "Freed …" (#2C893C).
    done: Hsla,
}

/// `(night, paper)`, as the theme in use wants it.
fn pick(cx: &App, (night, paper): (u32, u32)) -> Hsla {
    if cx.theme().mode.is_dark() {
        rgb(night).into()
    } else {
        rgb(paper).into()
    }
}

fn ink(cx: &App) -> Ink {
    let theme = cx.theme();
    let fg = |a: f32| theme.foreground.opacity(a);
    let dark = theme.mode.is_dark();
    let blue = pick(cx, (0x5B92F2, 0x4281ED));
    Ink {
        text: theme.foreground,
        muted: theme.muted_foreground,
        faint: fg(0.46),
        dot: fg(0.42),
        surface: theme.popover,
        edge: fg(if dark { 0.12 } else { 0.144 }),
        tip_edge: fg(if dark { 0.1 } else { 0.083 }),
        fly_edge: fg(if dark { 0.12 } else { 0.13 }),
        rule: fg(if dark { 0.08 } else { 0.07 }),
        track: fg(if dark { 0.1 } else { 0.083 }),
        well: fg(if dark { 0.08 } else { 0.048 }),
        ring_track: fg(if dark { 0.22 } else { 0.18 }),
        reserved: fg(if dark { 0.3 } else { 0.197 }),
        free_edge: fg(if dark { 0.24 } else { 0.19 }),
        arc: pick(cx, (0x5B92F2, 0x4A7FE5)),
        fill: blue,
        fill_soft: pick(cx, (0x3D5F9E, 0x9EC6F6)),
        processing: pick(cx, (0x8DB0F8, 0x3D6BDF)),
        stripe: pick(cx, (0xA9C2F5, 0x2F4FB0)),
        action: if dark {
            blue.opacity(0.16)
        } else {
            rgb(0xE8F2FA).into()
        },
        action_hover: if dark {
            blue.opacity(0.26)
        } else {
            rgb(0xE0E9FB).into()
        },
        action_text: pick(cx, (0xA8C4FA, 0x2D468F)),
        done: pick(cx, (0x4CC86A, 0x2C893C)),
    }
}

/// The card's type: 12 px on a 16 px line.
fn small<E: Styled>(el: E) -> E {
    el.text_size(px(12.)).line_height(px(16.))
}

/// A soft shadow: `y` down, `blur` wide, black at `alpha`.
fn shadow(y: f32, blur: f32, alpha: f32) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0., 0., 0., alpha),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }]
}

/// The ring's arc colour: blue, amber from 75%, red from 90%.
fn arc_color(frac: f32, cx: &App) -> Hsla {
    level_color(frac, ink(cx).arc, cx)
}

/// The ring that fills as the window does: 13 px across, a 2 px stroke, its arc running
/// clockwise from 12 o'clock.
pub fn ring(used: u64, window: u64, cx: &App) -> impl IntoElement {
    let frac = fraction(used, window);
    let color = arc_color(frac, cx);
    let track = ink(cx).ring_track;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let c = bounds.center();
            let r = bounds.size.width.min(bounds.size.height) / 2. - px(1.);
            let arc = |from: f32, to: f32, color: Hsla, window: &mut Window| {
                let steps = ((to - from) * 96.).ceil().max(2.) as usize;
                let mut path = PathBuilder::stroke(px(2.));
                for i in 0..=steps {
                    let t = from + (to - from) * i as f32 / steps as f32;
                    let a = t * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                    let p = point(c.x + r * a.cos(), c.y + r * a.sin());
                    if i == 0 {
                        path.move_to(p)
                    } else {
                        path.line_to(p)
                    }
                }
                if let Ok(p) = path.build() {
                    window.paint_path(p, color);
                }
            };
            arc(0.0, 1.0, track, window);
            if frac > 0.004 {
                arc(0.0, frac, color, window);
            }
        },
    )
    .size(px(13.))
    .flex_none()
}

/// The ring with the share beside it from `NEAR`, for the editor's AI side bar, which has no
/// card to say it.
pub fn dial(used: u64, window: u64, cx: &App) -> impl IntoElement {
    let frac = fraction(used, window);
    let muted = cx.theme().muted_foreground;
    h_flex()
        .gap(px(5.))
        .items_center()
        // Near the limit the share is spelled out; before that it's on hover.
        .when(frac >= NEAR, |el| {
            el.child(
                div()
                    .text_xs()
                    .text_color(if frac >= 0.75 {
                        arc_color(frac, cx)
                    } else {
                        muted
                    })
                    .child(format!("{:.0}%", frac * 100.)),
            )
        })
        .child(ring(used, window, cx))
}

/// The ring as the card's trigger: a 30 px circle that greys on hover and while the card is
/// open. Its tooltip says what the card's header says, so it shows only while the card is
/// closed, centred above the ring.
#[derive(IntoElement)]
pub struct RingTrigger {
    used: u64,
    window: u64,
    /// Compact now is running: with the card closed, the ring is a folding arrow.
    busy: bool,
    open: bool,
}

impl RingTrigger {
    pub fn new(used: u64, window: u64, busy: bool) -> Self {
        Self {
            used,
            window,
            busy,
            open: false,
        }
    }
}

impl gpui_kit::component::Selectable for RingTrigger {
    fn selected(self, _: bool) -> Self {
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }

    fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    fn is_open(&self) -> bool {
        self.open
    }
}

impl RenderOnce for RingTrigger {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ink = ink(cx);
        let bounds = window.use_keyed_state("context-ring-bounds", cx, |_, _| {
            Cell::new(Bounds::<Pixels>::default())
        });
        let hovered = window.use_keyed_state("context-ring-hover", cx, |_, _| false);
        let anchor = bounds.read(cx).get();
        let tip = (*hovered.read(cx) && !self.open && anchor.size.width > px(0.)).then(|| {
            let frac = fraction(self.used, self.window);
            deferred(
                Positioner::side(anchor)
                    .placement(Placement::Top)
                    .align(Align::Center)
                    .offset(px(8.5))
                    .margin(px(4.))
                    .child(
                        small(h_flex())
                            .id("context-ring-tip")
                            .h(px(28.))
                            .px(px(10.5))
                            .gap(px(7.5))
                            .items_center()
                            .rounded(px(8.))
                            .bg(ink.surface)
                            .border_1()
                            .border_color(ink.tip_edge)
                            .shadow(shadow(4., 10., 0.05))
                            .whitespace_nowrap()
                            .child(div().text_color(ink.muted).child("Context"))
                            .child(
                                div()
                                    .font_medium()
                                    .font_features(figures())
                                    .text_color(ink.text)
                                    .child(format!("{:.0}%", frac * 100.)),
                            )
                            .child(div().text_color(ink.dot).child("·"))
                            .child(div().font_features(figures()).text_color(ink.muted).child(
                                format!("{} of {}", fmt_tokens(self.used), fmt_tokens(self.window)),
                            ))
                            // A short wait, then a quick fade, with no movement.
                            .with_animation(
                                "context-ring-tip-in",
                                Animation::new(Duration::from_millis(290)),
                                |el, t| el.opacity(((t - 0.52) / 0.48).clamp(0., 1.)),
                            ),
                    ),
            )
            .with_priority(POPUP_PRIORITY + 1)
        });
        let glyph = if self.busy && !self.open {
            Icon::new(crate::assets::Lucide::RefreshCw)
                .small()
                .text_color(ink.muted)
                .into_any_element()
        } else {
            ring(self.used, self.window, cx).into_any_element()
        };
        div()
            .id("context-ring")
            .test_support()
            .relative()
            .size(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .when(self.open, |el| el.bg(ink.well))
            .hover(move |s| s.bg(ink.well))
            .on_hover(move |on: &bool, window: &mut Window, cx: &mut App| {
                hovered.update(cx, |h, _| *h = *on);
                window.refresh();
            })
            .child(
                canvas(
                    move |b, _w, cx| bounds.update(cx, |c, _| c.set(b)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
                .top_0()
                .left_0(),
            )
            .child(glyph)
            .children(tip)
    }
}

/// What Compact now is doing, kept on the composer so it survives the card closing.
#[derive(Clone, PartialEq)]
pub enum Compact {
    Idle,
    /// `/compact` sent: `before` is what the window held, `generation` keys the bar's animation.
    Running {
        generation: u64,
        before: u64,
    },
    /// The agent's next context report held this many tokens less than `before`.
    Freed {
        tokens: u64,
        before: u64,
    },
    /// The turn ended without the context moving.
    Unchanged {
        before: u64,
    },
}

/// One segment of the bar: a column of the legend too.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    System,
    Mcp,
    Skills,
    Messages,
    Free,
    Reserved,
}

impl Kind {
    /// Its name on the wire (`context_card`).
    fn key(self) -> &'static str {
        match self {
            Kind::System => "system",
            Kind::Mcp => "mcp",
            Kind::Skills => "skills",
            Kind::Messages => "messages",
            Kind::Free => "free",
            Kind::Reserved => "reserved",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::System => "System",
            Kind::Mcp => "MCP",
            Kind::Skills => "Skills",
            Kind::Messages => "Messages",
            Kind::Free => "Free",
            Kind::Reserved => "Reserved",
        }
    }

    /// The legend row's id (`context-seg-*` are the shots driver's handles).
    fn row_id(self) -> &'static str {
        match self {
            Kind::System => "context-seg-system",
            Kind::Mcp => "context-seg-mcp",
            Kind::Skills => "context-seg-skills",
            Kind::Messages => "context-seg-messages",
            Kind::Free => "context-seg-free",
            Kind::Reserved => "context-seg-reserved",
        }
    }

    /// The bar segment's id.
    fn bar_id(self) -> &'static str {
        match self {
            Kind::System => "context-bar-system",
            Kind::Mcp => "context-bar-mcp",
            Kind::Skills => "context-bar-skills",
            Kind::Messages => "context-bar-messages",
            Kind::Free => "context-bar-free",
            Kind::Reserved => "context-bar-reserved",
        }
    }

    /// Its segment's colour (Free and Reserved are greys: see `Ink`).
    fn color(self, cx: &App) -> Hsla {
        match self {
            Kind::System => pick(cx, (0x9B6BFF, 0x7F3DEB)),
            Kind::Mcp => pick(cx, (0x3DDC8A, 0x2CE57F)),
            Kind::Skills => pick(cx, (0xF0793A, 0xDF621C)),
            Kind::Messages => pick(cx, (0x5B92F2, 0x4681EC)),
            Kind::Free => ink(cx).track,
            Kind::Reserved => ink(cx).reserved,
        }
    }
}

/// A segment: its size in the window, and what its flyout says about it.
struct Part {
    kind: Kind,
    tokens: u64,
    /// Its share of the window, 0–1.
    share: f32,
    /// The flyout's breakdown rows: a label, and its value where there is one.
    detail: Vec<(String, Option<String>)>,
    /// A muted line under the rows.
    note: Option<String>,
}

/// One of the plan's usage windows.
struct LimitRow {
    id: &'static str,
    label: String,
    percent: f32,
    resets: String,
    /// What fills the window when the agent breaks it down (the weekly's per-model shares, say).
    /// Empty where the agent doesn't say: the row then keeps its plain bar on hover.
    split: Vec<(String, f32)>,
}

/// The plan's usage limits, as the agent last reported them.
struct Limits {
    session: Option<LimitRow>,
    weekly: Option<LimitRow>,
    /// The thread is paused at a limit right now.
    paused: Option<String>,
}

/// The card's numbers and segments, as [`Card::shown`] hands them to a phone.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub used: u64,
    pub window: u64,
    pub reserved: u64,
    pub parts: Vec<ShownPart>,
}

/// A segment of the bar: `system` · `mcp` · `skills` · `messages` · `free` · `reserved`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShownPart {
    pub kind: &'static str,
    pub label: &'static str,
    pub tokens: u64,
    pub detail: Vec<(String, Option<String>)>,
    pub note: Option<String>,
}

/// Everything the card shows, gathered once per render.
pub struct Card {
    used: u64,
    window: u64,
    /// What the agent keeps free of the window so compacting has room (`RESERVE`): it isn't part
    /// of "used", and "until auto-compact" is the free space left after it.
    reserved: u64,
    parts: Vec<Part>,
    limits: Limits,
    compact: Compact,
    /// The thread's agent, for the compact button's tooltip.
    agent: Option<AgentId>,
    /// A turn is running: compaction has to wait for it.
    running: bool,
    /// Nothing to compact (a draft, or no session yet).
    no_thread: bool,
}

/// Keyed element state (it goes away with the card): the hovered part and its rows' bounds, for
/// dimming and the flyout's placement.
type HoveredPart = Entity<Option<Kind>>;
type RowBounds = Entity<RefCell<HashMap<Kind, Bounds<Pixels>>>>;
type HoveredLimit = Entity<Option<&'static str>>;

impl Card {
    /// The card for `scope`'s thread, `used` of `window` tokens in its context.
    pub fn of(
        ws: &Workspace,
        scope: &Scope,
        thread: Option<&Thread>,
        used: u64,
        window: u64,
        compact: Compact,
    ) -> Card {
        let window = window.max(1);
        let used = used.min(window);
        let reserved = ((window as f64 * RESERVE) as u64).min(window.saturating_sub(used));
        let free = window - used - reserved;
        let id = thread.map(|t| t.id.as_str());
        let live = id.and_then(|id| ws.live.get(id));
        let (messages, transcript_tokens) = live.map(|l| transcript(&l.items)).unwrap_or((0, 0));
        let skills: Vec<String> = thread
            .map(|t| {
                ws.slash_commands(scope, &t.agent)
                    .into_iter()
                    .filter(|c| c.kind == CommandKind::Skill)
                    .map(|c| c.name)
                    .collect()
            })
            .unwrap_or_default();
        let mcp: Vec<String> = ws.mcp_servers().into_iter().map(|s| s.name).collect();
        // What Trek tells the agent about the project (a session's `instructions`): the notes
        // waiting for its next message, else the project's own.
        let instructions = live
            .and_then(|l| l.notes_pending.clone())
            .or_else(|| thread.and_then(|t| ws.notes_for(t)))
            .map(|notes| tokens_of(&notes));
        let Split {
            system: system_tokens,
            skills: skill_tokens,
            mcp: mcp_tokens,
            instructions: instruction_tokens,
            messages: message_tokens,
        } = split(
            used,
            transcript_tokens,
            skills.len(),
            mcp.len(),
            instructions.unwrap_or(0),
        );
        let share = |n: u64| n as f32 / window as f32;
        let count =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = vec![];
        if system_tokens > 0 {
            parts.push(Part {
                kind: Kind::System,
                tokens: system_tokens,
                share: share(system_tokens),
                detail: vec![(
                    "The agent's own prompt and its tools' definitions".into(),
                    None,
                )],
                note: Some(
                    "Estimated: what the agent counts that isn't a message, skill or server."
                        .into(),
                ),
            });
        }
        if mcp_tokens > 0 {
            // The agent's count may be smaller than what is estimated here: each row shows what
            // `split` had room for, so the two never add up to more than the segment.
            let mut detail = vec![];
            if !mcp.is_empty() {
                detail.push((
                    mcp.join(", "),
                    Some(fmt_tokens(mcp_tokens - instruction_tokens)),
                ));
            }
            if instructions.is_some() && instruction_tokens > 0 {
                detail.push(("Instructions".into(), Some(fmt_tokens(instruction_tokens))));
            }
            parts.push(Part {
                kind: Kind::Mcp,
                tokens: mcp_tokens,
                share: share(mcp_tokens),
                detail,
                note: Some(format!(
                    "Estimated, about {MCP_SERVER_TOKENS} tokens a server."
                )),
            });
        }
        if skill_tokens > 0 {
            parts.push(Part {
                kind: Kind::Skills,
                tokens: skill_tokens,
                share: share(skill_tokens),
                detail: vec![(
                    format!(
                        "{} the agent can be asked for",
                        count(skills.len(), "skill prompt", "skill prompts")
                    ),
                    None,
                )],
                note: Some(format!(
                    "{} · estimated, about {SKILL_TOKENS} tokens each.",
                    skills.join(", ")
                )),
            });
        }
        if message_tokens > 0 {
            let mut detail = vec![(
                count(
                    messages,
                    "message and tool result",
                    "messages and tool results",
                ),
                None,
            )];
            if message_tokens < transcript_tokens {
                detail.push((
                    "The transcript holds more than the window: the agent has summarized it."
                        .into(),
                    None,
                ));
            }
            parts.push(Part {
                kind: Kind::Messages,
                tokens: message_tokens,
                share: share(message_tokens),
                detail,
                note: Some(format!(
                    "Estimated from their text ({CHARS_PER_TOKEN} characters a token)."
                )),
            });
        }
        parts.push(Part {
            kind: Kind::Free,
            tokens: free,
            share: share(free),
            detail: vec![(
                "Room for the conversation, before the agent has to compact.".into(),
                None,
            )],
            note: None,
        });
        parts.push(Part {
            kind: Kind::Reserved,
            tokens: reserved,
            share: share(reserved),
            detail: vec![("Kept free so compacting has room to run".into(), None)],
            note: Some("Estimated: no agent reports its reserve.".into()),
        });
        Card {
            used,
            window,
            reserved,
            parts,
            limits: Limits::of(ws, thread),
            compact,
            agent: thread.map(|t| t.agent.clone()),
            running: id.is_some_and(|id| ws.turn_running(id)),
            no_thread: thread.is_none(),
        }
    }

    fn part(&self, kind: Kind) -> Option<&Part> {
        self.parts.iter().find(|p| p.kind == kind)
    }

    /// What the card shows, for a phone (`remote::context_card`): the window, and the bar's
    /// segments in order, each with its legend label and what its flyout says.
    pub fn shown(&self) -> Shown {
        Shown {
            used: self.used,
            window: self.window,
            reserved: self.reserved,
            parts: self
                .parts
                .iter()
                .map(|p| ShownPart {
                    kind: p.kind.key(),
                    label: p.kind.label(),
                    tokens: p.tokens,
                    detail: p.detail.clone(),
                    note: p.note.clone(),
                })
                .collect(),
        }
    }

    pub fn render(
        &self,
        me: WeakEntity<Composer>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let ink = ink(cx);
        let hover = window.use_keyed_state("context-hover", cx, |_, _| None::<Kind>);
        let rows = window.use_keyed_state("context-rows", cx, |_, _| {
            RefCell::new(HashMap::<Kind, Bounds<Pixels>>::new())
        });
        let bounds = window.use_keyed_state("context-bounds", cx, |_, _| {
            Cell::new(Bounds::<Pixels>::default())
        });
        let limit_hover = window.use_keyed_state("context-limit", cx, |_, _| None::<&'static str>);
        let hovered = *hover.read(cx);
        // Shown values: the messages column folds up as the compaction's reports land, and
        // everything it feeds (used, free, the header's share) follows it.
        let messages = self.part(Kind::Messages).map(|p| p.tokens).unwrap_or(0);
        let folded = transition_with_status(
            "context-messages",
            messages as f32,
            Transition::new(COLLAPSE).ease(|t| 1. - (1. - t).powi(3)),
            window,
            cx,
        );
        let shown_messages = folded.value.max(0.) as u64;
        let shown_used = self
            .used
            .saturating_sub(messages.saturating_sub(shown_messages));
        let shown_free = self
            .window
            .saturating_sub(self.reserved)
            .saturating_sub(shown_used);
        // While Compact now runs (or the fold is still playing) the bar stands taller and the
        // messages segment is message chunks being summarized right to left; the rest of the
        // time it's one piece like the others.
        let running = matches!(self.compact, Compact::Running { .. });
        let folding = running || folded.status == MotionStatus::Running;
        if running {
            // The shimmer and the sweep need frames; idle, the card asks for none.
            window.request_animation_frame();
        }
        let still = cx.reduce_motion();
        let frac = fraction(shown_used, self.window);
        let header = small(h_flex())
            .w_full()
            .items_center()
            .gap(px(7.5))
            .child(div().text_color(ink.muted).child("Context"))
            .child(
                div()
                    .font_medium()
                    .font_features(figures())
                    .text_color(level_color(frac, ink.text, cx))
                    .child(format!("{:.0}%", frac * 100.)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_color(ink.muted)
                    .font_features(figures())
                    .child(format!(
                        "{} of {}",
                        fmt_tokens(shown_used),
                        fmt_tokens(self.window)
                    )),
            );
        let dim = |kind: Kind, window: &mut Window, cx: &mut App| {
            transition(
                SharedString::from(format!("context-dim-{}", kind.bar_id())),
                if hovered.is_some() && hovered != Some(kind) {
                    0.35
                } else {
                    1.
                },
                Transition::new(FADE),
                window,
                cx,
            )
        };
        let hover_seg = |kind: Kind, hover: HoveredPart, me: WeakEntity<Composer>| {
            move |on: &bool, _window: &mut Window, cx: &mut App| {
                hover.update(cx, |h, _| *h = on.then_some(kind));
                let _ = me.update(cx, |_, cx| cx.notify());
            }
        };
        // 8 px tall at rest, 16 while summarizing: it grows into the room around it, so nothing
        // below it moves.
        let tall = transition(
            "context-bar-tall",
            if folding { 1. } else { 0. },
            Transition::new(Duration::from_millis(300)).ease(|t| 1. - (1. - t).powi(3)),
            window,
            cx,
        );
        let height = BAR * (1. + tall);
        let radius = 2. + 2. * tall;
        let shown_tokens = |p: &Part| match p.kind {
            Kind::Messages => shown_messages,
            Kind::Free => shown_free,
            _ => p.tokens,
        };
        // Widths are each part's share of the bar less its gaps; a part that holds anything
        // keeps a sliver, so a few hundred tokens of skills still show.
        let segments: Vec<AnyElement> = self
            .parts
            .iter()
            .filter(|p| shown_tokens(p) > 0)
            .map(|p| {
                let opacity = dim(p.kind, window, cx);
                let tokens = shown_tokens(p);
                let seg = |el: Div| {
                    grow(el, tokens as f32)
                        .id(p.kind.bar_id())
                        .test_support()
                        .h_full()
                        .min_w(px(2.))
                        .opacity(opacity)
                        .on_hover(hover_seg(p.kind, hover.clone(), me.clone()))
                };
                match p.kind {
                    Kind::Messages if folding => {
                        let chunk = (messages.max(shown_messages).div_ceil(BLOCKS)).max(1);
                        let n = shown_messages.div_ceil(chunk).max(1) as usize;
                        // The chunk being summarized: it runs right to left while the agent
                        // works, and rests on the last one as the fold plays.
                        let at = if running && !still {
                            n - 1 - (clock() * 6.) as usize % n
                        } else {
                            n - 1
                        };
                        let color = Kind::Messages.color(cx);
                        seg(h_flex())
                            .relative()
                            .gap(px(2.))
                            .children((0..n).map(|i| {
                                let el = grow(div(), CHUNKS[i % CHUNKS.len()])
                                    .h_full()
                                    .min_w(px(1.))
                                    .rounded(px(radius))
                                    .overflow_hidden();
                                if i > at {
                                    el.child(stripes(ink.stripe))
                                } else {
                                    el.bg(if i == at { ink.processing } else { color })
                                }
                            }))
                            // The playhead at the used region's end, standing out of the bar.
                            .child(
                                div()
                                    .absolute()
                                    .right(px(-2.))
                                    .top(px(-4. * tall))
                                    .w(px(2.))
                                    .h(px(height + 8. * tall))
                                    .opacity(tall)
                                    .bg(ink.text),
                            )
                            .into_any_element()
                    }
                    kind => seg(div())
                        .rounded(px(radius))
                        .bg(kind.color(cx))
                        .into_any_element(),
                }
            })
            .collect();
        let bar = div().relative().w_full().h(px(BAR)).child(
            h_flex()
                .absolute()
                .left_0()
                .right_0()
                .top(px((BAR - height) / 2.))
                .h(px(height))
                .gap(px(2.))
                .children(segments),
        );
        let legend = div()
            .grid()
            .grid_cols(2)
            .gap_x(px(12.5))
            .children(self.parts.iter().map(|p| {
                legend_row(
                    p,
                    shown_tokens(p),
                    hovered,
                    hover.clone(),
                    rows.clone(),
                    me.clone(),
                    window,
                    cx,
                )
            }));
        // The flyout 9 px to the card's right (flipped left when there's no room), its first
        // line on the hovered row's.
        let card = bounds.read(cx).get();
        let flyout = hovered
            .and_then(|kind| self.part(kind))
            .and_then(|p| {
                let row = rows.read(cx).borrow().get(&p.kind).copied()?;
                (row.size.width > px(0.) && card.size.width > px(0.)).then(|| {
                    let anchor = Bounds::new(
                        point(card.left(), row.top() + px(3. - FLY_TOP)),
                        size(card.size.width, row.size.height),
                    );
                    (p, anchor)
                })
            })
            .map(|(p, anchor)| {
                deferred(
                    Positioner::side(anchor)
                        .placement(Placement::Right)
                        .align(Align::Start)
                        .offset(px(9.))
                        .margin(px(8.))
                        .occlude()
                        .child(flyout(p, cx)),
                )
                .with_priority(POPUP_PRIORITY + 1)
                .into_any_element()
            });
        let rule = || div().h(px(1.)).w_full().flex_none().bg(ink.rule);
        let context = v_flex()
            .px(px(PAD))
            .pt(px(11.5))
            .pb(px(13.5))
            .child(header)
            .child(div().mt(px(10.)).child(bar))
            .child(div().mt(px(7.)).child(legend));
        div()
            .relative()
            .child(
                canvas(
                    move |b, _w, cx| {
                        bounds.update(cx, |c, _| c.set(b));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
                .top_0()
                .left_0(),
            )
            .child(
                v_flex()
                    .w(px(280.))
                    .rounded(px(14.))
                    .bg(ink.surface)
                    .border_1()
                    .border_color(ink.edge)
                    .shadow(shadow(4., 14., 0.05))
                    .child(reveal(
                        context.into_any_element(),
                        "context-reveal-context",
                        0,
                    ))
                    .when_some(
                        self.limits
                            .render(limit_hover.clone(), me.clone(), window, cx),
                        |el, limits| {
                            el.child(rule())
                                .child(reveal(limits, "context-reveal-limits", 1))
                        },
                    )
                    .child(rule())
                    .child(reveal(
                        self.footer(me, window, shown_free, folding, cx),
                        "context-reveal-footer",
                        2,
                    )),
            )
            .when_some(flyout, |el, f| el.child(f))
            .with_animation(
                "context-card",
                Animation::new(Duration::from_millis(150)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t),
            )
            .into_any_element()
    }

    fn footer(
        &self,
        me: WeakEntity<Composer>,
        window: &mut Window,
        until: u64,
        folding: bool,
        cx: &mut App,
    ) -> AnyElement {
        let ink = ink(cx);
        // The footer says how much room is left before the agent compacts on its own, or what
        // Compact now is doing.
        let summarizing = |window: &mut Window| -> AnyElement {
            window.request_animation_frame();
            small(div())
                .child(crate::working_bar::shimmer(
                    "Summarizing messages".into(),
                    clock(),
                    cx.reduce_motion(),
                    ink.muted,
                    ink.surface.blend(ink.muted.opacity(0.55)),
                ))
                .into_any_element()
        };
        let line = |text: String, color: Hsla| {
            small(div())
                .truncate()
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        let status = match self.compact {
            Compact::Running { .. } => summarizing(window),
            Compact::Freed { .. } if folding => summarizing(window),
            Compact::Freed { tokens, .. } => h_flex()
                .gap(px(6.))
                .items_center()
                .child(check(ink.done))
                .child(line(
                    format!("Freed {} tokens", fmt_tokens(tokens)),
                    ink.muted,
                ))
                .with_animation(
                    "context-freed",
                    Animation::new(Duration::from_millis(200)),
                    |el, t| el.opacity(t),
                )
                .into_any_element(),
            Compact::Unchanged { .. } => {
                line("The agent kept the context as it was".into(), ink.muted)
            }
            Compact::Idle if self.used >= self.window => {
                line("The window is full".into(), palette::red(cx))
            }
            Compact::Idle => line(
                format!("{} until auto-compact", fmt_round(until)),
                ink.muted,
            ),
        };
        let (label, enabled, why) = match self.compact {
            Compact::Running { .. } => ("Compact now", false, None),
            Compact::Freed { .. } => ("Compacted", false, None),
            Compact::Idle | Compact::Unchanged { .. } => {
                let why = if self.no_thread {
                    "Nothing to compact yet: send a message first."
                } else if self.running {
                    "Stop the running turn to compact."
                } else if self.limits.paused.is_some() {
                    "The thread is paused at a usage limit."
                } else {
                    "Sends /compact to {agent}: it summarizes the conversation so far."
                };
                let why = why.replace(
                    "{agent}",
                    &self
                        .agent
                        .as_ref()
                        .map_or_else(|| "the agent".to_string(), |a| a.display_name()),
                );
                (
                    "Compact now",
                    !(self.no_thread || self.running || self.limits.paused.is_some()),
                    Some(SharedString::from(why)),
                )
            }
        };
        let button = small(h_flex())
            .id("context-compact")
            .test_support()
            .flex_none()
            .h(px(28.))
            .pl(px(12.5))
            .pr(px(11.))
            .items_center()
            .rounded_full()
            .font_medium()
            .whitespace_nowrap()
            .map(|el| {
                if enabled {
                    el.bg(ink.action)
                        .text_color(ink.action_text)
                        .cursor_pointer()
                        .hover(move |s| s.bg(ink.action_hover))
                        .on_click(move |_, _, cx| {
                            let _ = me.update(cx, |this, cx| this.compact_now(cx));
                        })
                } else {
                    el.bg(ink.well).text_color(ink.faint)
                }
            })
            .when_some(why, |el, why| {
                el.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(why.clone()).build(window, cx)
                })
            })
            .child(label);
        h_flex()
            .w_full()
            .items_center()
            .gap(px(8.))
            .pl(px(PAD))
            .pr(px(10.5))
            .pt(px(10.))
            .pb(px(10.5))
            .child(div().flex_1().min_w_0().child(status))
            .child(button)
            .into_any_element()
    }
}

/// The card's left and right padding.
const PAD: f32 = 14.5;
/// The flyout's first line, from its top: its 12 px cap top less the line's leading.
const FLY_TOP: f32 = 8.3;
/// The message chunks' relative widths while summarizing: uneven, as messages are.
const CHUNKS: [f32; 15] = [
    2.6, 4.7, 14.4, 3.2, 4.7, 12.3, 3.2, 5.9, 15.2, 3.6, 6.1, 2.6, 4.3, 6.3, 9.7,
];

/// `el` takes `weight` of its row's room past its siblings' fixed sizes.
fn grow<E: Styled>(mut el: E, weight: f32) -> E {
    el.style().flex_grow = Some(weight);
    el.style().flex_shrink = Some(1.);
    el.flex_basis(px(0.))
}

/// What summarizing has folded: thin stripes, 1 px every 2 px, the height of the bar.
fn stripes(color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let mut x = b.left();
            while x < b.right() {
                window.paint_quad(fill(
                    Bounds::new(point(x, b.top()), size(px(1.), b.size.height)),
                    color,
                ));
                x += px(2.);
            }
        },
    )
    .size_full()
}

/// The check by "Freed …": a stroke that draws itself in.
fn check(color: Hsla) -> AnyElement {
    div()
        .size(px(10.))
        .flex_none()
        .with_animation(
            "context-check",
            Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()),
            move |el, t| {
                el.child(
                    canvas(
                        |_, _, _| {},
                        move |b, _, window, _| {
                            let o = b.origin;
                            let pts = [(0.5, 5.2), (3.6, 8.2), (9.5, 2.0)];
                            let p = |(x, y): (f32, f32)| point(o.x + px(x), o.y + px(y));
                            let seg = |a: (f32, f32), b: (f32, f32)| {
                                ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
                            };
                            let (l1, l2) = (seg(pts[0], pts[1]), seg(pts[1], pts[2]));
                            let mut left = t * (l1 + l2);
                            let mut path = PathBuilder::stroke(px(1.5));
                            path.move_to(p(pts[0]));
                            for (a, b, l) in [(pts[0], pts[1], l1), (pts[1], pts[2], l2)] {
                                if left <= 0. {
                                    break;
                                }
                                let k = (left / l).min(1.);
                                path.line_to(p((a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k)));
                                left -= l;
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                        },
                    )
                    .size_full(),
                )
            },
        )
        .into_any_element()
}

/// A section of the card fading in, `step` tenths of a second after the first.
fn reveal(el: AnyElement, id: &'static str, step: u32) -> AnyElement {
    let delay = step as f32 * 90.;
    let total = delay + 150.;
    div()
        .child(el)
        .with_animation(
            id,
            Animation::new(Duration::from_millis(total as u64)),
            move |el, t| {
                let start = delay / total;
                el.opacity(((t - start) / (1. - start)).clamp(0., 1.))
            },
        )
        .into_any_element()
}

/// Seconds since the process start, for the shimmer's sweep: any monotonic phase does.
fn clock() -> f32 {
    static T0: std::sync::LazyLock<std::time::Instant> =
        std::sync::LazyLock::new(std::time::Instant::now);
    T0.elapsed().as_secs_f32()
}

/// Tabular figures, so numbers that change in place don't jitter. Only on numbers: on the
/// labels they'd widen the "5" of "5-hour".
fn figures() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

/// Red at 90%, amber at 75%, `calm` below: the ring's thresholds.
fn level_color(frac: f32, calm: Hsla, cx: &App) -> Hsla {
    if frac >= 0.9 {
        palette::red(cx)
    } else if frac >= 0.75 {
        palette::amber(cx)
    } else {
        calm
    }
}

/// A legend cell: swatch, name, size. Hovering dims the other cells and the bar's other parts,
/// and opens the part's flyout to the card's side.
#[allow(clippy::too_many_arguments)]
fn legend_row(
    part: &Part,
    tokens: u64,
    hovered: Option<Kind>,
    hover: HoveredPart,
    rows: RowBounds,
    me: WeakEntity<Composer>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let ink = ink(cx);
    let kind = part.kind;
    let dimmed = hovered.is_some() && hovered != Some(kind);
    let opacity = transition(
        SharedString::from(format!("context-dim-{}", kind.row_id())),
        if dimmed { 0.4 } else { 1. },
        Transition::new(FADE),
        window,
        cx,
    );
    small(h_flex())
        .id(kind.row_id())
        .test_support()
        .relative()
        .min_w_0()
        .h(px(22.))
        .gap(px(6.))
        .items_center()
        .opacity(opacity)
        .on_hover(move |on: &bool, _window: &mut Window, cx: &mut App| {
            hover.update(cx, |h, _| *h = on.then_some(kind));
            let _ = me.update(cx, |_, cx| cx.notify());
        })
        .child(
            canvas(
                move |b, _w, cx| {
                    rows.update(cx, |rows, _| {
                        rows.borrow_mut().insert(kind, b);
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full()
            .top_0()
            .left_0(),
        )
        .child(
            div()
                .size(px(8.))
                .rounded(px(2.))
                .flex_none()
                // A dimmed swatch goes a little further than its text.
                .when(dimmed, |el| el.opacity(0.85))
                .bg(kind.color(cx))
                .when(kind == Kind::Free, |el| {
                    el.border_1().border_color(ink.free_edge)
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(ink.muted)
                .child(kind.label()),
        )
        .child(
            div()
                .flex_none()
                .text_color(ink.text)
                .font_features(figures())
                .child(fmt_tokens(tokens)),
        )
        .into_any_element()
}

/// A short colour dash: the flyout's and the limit split's key.
fn dash(color: Hsla) -> Div {
    div()
        .w(px(10.))
        .h(px(3.))
        .rounded_full()
        .flex_none()
        .bg(color)
}

/// A part's flyout: a colour dash, its name, its size and share, then its breakdown.
fn flyout(part: &Part, cx: &App) -> AnyElement {
    let ink = ink(cx);
    let color = part.kind.color(cx);
    let share = if part.tokens > 0 && part.share < 0.005 {
        "<1%".to_string()
    } else {
        format!("{:.0}%", part.share * 100.)
    };
    let row = |(label, value): &(String, Option<String>)| {
        h_flex()
            .gap(px(12.))
            .items_start()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(ink.muted)
                    .child(label.clone()),
            )
            .when_some(value.clone(), |el, value| {
                el.child(
                    div()
                        .flex_none()
                        .font_features(figures())
                        .text_color(ink.text)
                        .child(value),
                )
            })
    };
    small(v_flex())
        .min_w(px(192.))
        .max_w(px(264.))
        .pt(px(FLY_TOP))
        .pb(px(8.4))
        .pl(px(10.))
        .pr(px(11.))
        .gap(px(4.))
        .rounded(px(10.))
        .bg(ink.surface)
        .border_1()
        .border_color(ink.fly_edge)
        .shadow(shadow(6., 14., 0.08))
        .child(
            h_flex()
                .items_center()
                .gap(px(7.))
                .child(dash(color))
                .child(div().text_color(ink.text).child(part.kind.label()))
                .child(div().flex_1().min_w(px(12.)))
                .child(
                    div()
                        .font_features(figures())
                        .text_color(ink.text)
                        .child(fmt_tokens(part.tokens)),
                )
                .child(
                    div()
                        .ml(px(0.5))
                        .font_features(figures())
                        .text_color(ink.muted)
                        .child(share),
                ),
        )
        .children(part.detail.iter().map(row))
        .when_some(part.note.clone(), |el, note| {
            el.child(div().text_color(ink.faint).child(note))
        })
        .with_animation(
            SharedString::from(format!("context-flyout-{}", part.kind.row_id())),
            Animation::new(Duration::from_millis(100)),
            |el, t| el.opacity(t),
        )
        .into_any_element()
}

impl Limits {
    fn of(ws: &Workspace, thread: Option<&Thread>) -> Limits {
        let Some(thread) = thread else {
            return Limits {
                session: None,
                weekly: None,
                paused: None,
            };
        };
        let now = ws.now();
        let status = ws.usage_status(&thread.agent.key());
        let limits: &[UsageLimit] = status.map_or(&[], |s| s.limits.as_slice());
        let paused = ws.pause(&thread.id).map(|p| match p.resets_at {
            Some(at) => format!(
                "Paused at a limit · resets {}",
                crate::time::reset_clock(at, now)
            ),
            None => "Paused at a limit".to_string(),
        });
        let session = limits
            .iter()
            .find(|l| {
                l.window == "5h" || {
                    let label = l.label.to_lowercase();
                    label.contains("5-hour") || label.contains("session")
                }
            })
            .map(|l| row(l, now, false));
        let weekly = limits
            .iter()
            .find(|l| {
                let label = l.label.to_lowercase();
                label == "weekly limit" || (label.starts_with("weekly") && !l.label.contains('·'))
            })
            .map(|l| row(l, now, true));
        // The per-model windows the plan reports ("Weekly · Opus"): the weekly row's split.
        let models: Vec<(String, f32)> = limits
            .iter()
            .filter(|l| l.label.contains('·'))
            .map(|l| {
                (
                    l.label
                        .rsplit('·')
                        .next()
                        .unwrap_or(&l.label)
                        .trim()
                        .to_string(),
                    l.percent,
                )
            })
            .collect();
        let mut weekly = weekly;
        if let Some(w) = weekly.as_mut() {
            w.split = models;
        }
        Limits {
            session,
            weekly,
            paused,
        }
    }

    /// The section, or nothing when the agent reports no plan limits: the card doesn't spend a
    /// row saying it has nothing to say.
    fn render(
        &self,
        hover: HoveredLimit,
        me: WeakEntity<Composer>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        if self.session.is_none() && self.weekly.is_none() && self.paused.is_none() {
            return None;
        }
        let hovered = *hover.read(cx);
        Some(
            v_flex()
                .w_full()
                .px(px(PAD))
                .pt(px(13.5))
                .pb(px(15.))
                .gap(px(14.5))
                .children(
                    self.session
                        .iter()
                        .map(|row| limit_row(row, hovered, hover.clone(), me.clone(), window, cx)),
                )
                .children(
                    self.weekly
                        .iter()
                        .map(|row| limit_row(row, hovered, hover.clone(), me.clone(), window, cx)),
                )
                .when_some(self.paused.clone(), |el, paused| {
                    el.child(small(div()).text_color(palette::amber(cx)).child(paused))
                })
                .into_any_element(),
        )
    }
}

fn row(limit: &UsageLimit, now: i64, weekly: bool) -> LimitRow {
    let resets = match limit.resets_at {
        Some(at) if weekly => format!("Resets {}", day_clock(at)),
        Some(at) => format!("Resets in {}", resets_in(at - now)),
        None => "Reset time unknown".to_string(),
    };
    LimitRow {
        id: if weekly {
            "context-limit-weekly"
        } else {
            "context-limit-session"
        },
        // Whatever the agent calls them ("5-hour limit", "Current session"), the rows are the
        // plan's session and weekly windows.
        label: if weekly {
            "Weekly limit"
        } else {
            "Session limit"
        }
        .into(),
        percent: limit.percent,
        resets,
        split: vec![],
    }
}

/// "2 hr 49 min", for a reset inside a day.
fn resets_in(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    match s {
        0..=59 => "under a minute".into(),
        60..=3_599 => format!("{} min", (s + 30) / 60),
        3_600..=86_399 => format!("{} hr {} min", s / 3_600, (s % 3_600) / 60),
        _ => format!("{} days", (s + 86_399) / 86_400),
    }
}

/// "Mon 8:00 AM", for a reset within the week.
fn day_clock(at: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%a %-I:%M %p")
                .to_string()
        })
        .unwrap_or_default()
}

/// A limit row: its window's name, share and reset, then its bar. Where the agent says what the
/// window holds, hovering splits the header and the bar by it, and dims the other row.
fn limit_row(
    row: &LimitRow,
    hovered: Option<&'static str>,
    hover: HoveredLimit,
    me: WeakEntity<Composer>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let ink = ink(cx);
    let on = hovered == Some(row.id);
    let opacity = transition(
        SharedString::from(format!("context-dim-{}", row.id)),
        if hovered.is_some() && !on { 0.4 } else { 1. },
        Transition::new(FADE),
        window,
        cx,
    );
    let color = if row.percent >= 95. {
        palette::red(cx)
    } else if row.percent >= 85. {
        palette::amber(cx)
    } else {
        ink.fill
    };
    let percent = |value: f32| {
        div()
            .font_medium()
            .font_features(figures())
            .text_color(ink.text)
            .child(format!("{value:.0}%"))
    };
    let expanded = on && !row.split.is_empty();
    let header = if expanded {
        small(h_flex())
            .w_full()
            .items_center()
            .justify_between()
            .children(row.split.iter().enumerate().map(|(i, (name, share))| {
                h_flex()
                    .items_center()
                    .child(dash(split_tone(i, color, &ink)))
                    .child(div().ml(px(6.5)).text_color(ink.muted).child(name.clone()))
                    .child(percent(*share).ml(px(7.5)))
            }))
            .into_any_element()
    } else {
        small(h_flex())
            .w_full()
            .items_center()
            .gap(px(7.5))
            .child(div().text_color(ink.muted).child(row.label.clone()))
            .child(percent(row.percent))
            .child(div().flex_1())
            .child(
                div()
                    .text_color(ink.muted)
                    .font_features(figures())
                    .child(row.resets.clone()),
            )
            .into_any_element()
    };
    let target = (row.percent / 100.).clamp(0., 1.);
    let bar = div()
        .w_full()
        .h(px(6.))
        .rounded_full()
        .bg(ink.track)
        .child(if expanded {
            h_flex()
                .size_full()
                .gap(px(2.))
                .children(row.split.iter().enumerate().map(|(i, (_, share))| {
                    div()
                        .h_full()
                        .rounded_full()
                        .w(relative((share / 100.).clamp(0., 1.)))
                        .bg(split_tone(i, color, &ink))
                }))
                .into_any_element()
        } else {
            // On open the fill grows from nothing, once its row is in.
            div()
                .h_full()
                .rounded_full()
                .bg(color)
                .with_animation(
                    SharedString::from(format!("{}-fill", row.id)),
                    Animation::new(Duration::from_millis(560)),
                    move |el, t| {
                        let k = ((t - 0.3) / 0.7).clamp(0., 1.);
                        el.w(relative(target * (1. - (1. - k).powi(3))))
                    },
                )
                .into_any_element()
        });
    let id = row.id;
    v_flex()
        .id(row.id)
        .test_support()
        .w_full()
        .gap(px(8.5))
        .opacity(opacity)
        .when(!row.split.is_empty(), |el| {
            el.on_hover(move |on: &bool, _window: &mut Window, cx: &mut App| {
                hover.update(cx, |h, _| *h = on.then_some(id));
                let _ = me.update(cx, |_, cx| cx.notify());
            })
        })
        .child(header)
        .child(bar)
        .into_any_element()
}

/// The split's tones: the first share the row's colour, the second a light blue, the rest
/// fainter still.
fn split_tone(i: usize, color: Hsla, ink: &Ink) -> Hsla {
    match i {
        0 => color,
        1 if color == ink.fill => ink.fill_soft,
        1 => color.opacity(0.45),
        _ => color.opacity(0.25),
    }
}

/// The agent's `used` tokens, split by what fills them.
#[derive(Debug, PartialEq, Eq)]
struct Split {
    system: u64,
    skills: u64,
    mcp: u64,
    /// The part of `mcp` that is the project's instructions, as far as the agent's count has room.
    instructions: u64,
    messages: u64,
}

/// What Trek knows is configured is in every request, so it's counted first, at rough per-item
/// sizes. The transcript's share comes out of what's left: the agent's count is the truth, and a
/// summarized transcript holds more text than the window does. The rest is the agent's own
/// prompt and its tools' definitions, up to what such a prompt plausibly takes; anything past
/// that is conversation too.
fn split(used: u64, transcript: u64, skills: usize, mcp: usize, instructions: u64) -> Split {
    let instructions = instructions.min(used);
    let servers = (mcp as u64 * MCP_SERVER_TOKENS).min(used - instructions);
    let mcp = servers + instructions;
    let skills = (skills as u64 * SKILL_TOKENS).min(used - mcp);
    let rest = used - mcp - skills;
    let messages = transcript.min(rest).max(rest.saturating_sub(SYSTEM_TOKENS));
    Split {
        system: rest - messages,
        skills,
        mcp,
        instructions,
        messages,
    }
}

/// Tokens the transcript's text stands for: messages and tool results, about four characters
/// to a token. Returns how many there are, and the tokens.
fn transcript(items: &[Item]) -> (usize, u64) {
    let mut messages = 0;
    let mut chars = 0;
    for item in items {
        let (counted, len) = match item {
            Item::User { text, .. } => (true, text.chars().count()),
            Item::Assistant { text } => (true, text.chars().count()),
            Item::Tool {
                title,
                detail,
                output,
                ..
            } => (
                true,
                title.chars().count() + detail.chars().count() + output.chars().count(),
            ),
            Item::Reasoning { text } => (false, text.chars().count()),
            Item::Notice { text } | Item::Error { text } => (false, text.chars().count()),
            _ => (false, 0),
        };
        if counted {
            messages += 1;
        }
        chars += len;
    }
    (messages, tokens_of_len(chars))
}

fn tokens_of(text: &str) -> u64 {
    tokens_of_len(text.chars().count())
}

fn tokens_of_len(chars: usize) -> u64 {
    (chars as u64).div_ceil(CHARS_PER_TOKEN as u64)
}

#[cfg(test)]
mod tests {
    use super::{
        MCP_SERVER_TOKENS, SKILL_TOKENS, SYSTEM_TOKENS, Split, day_clock, fmt_round, fmt_tokens,
        split, transcript,
    };
    use trek_core::store::Item;

    #[test]
    fn a_transcript_longer_than_the_window_leaves_the_configured_parts_their_share() {
        // A summarized thread: its transcript estimates more than the agent counts.
        let s = split(200_000, 500_000, 4, 2, 3_200);
        assert_eq!(
            s.mcp,
            2 * MCP_SERVER_TOKENS + 3_200,
            "the servers and the instructions are in every request"
        );
        assert_eq!(s.skills, 4 * SKILL_TOKENS);
        assert_eq!(s.messages, 200_000 - s.mcp - s.skills);
        assert_eq!(s.system, 0, "nothing left to guess at");
    }

    #[test]
    fn what_no_part_accounts_for_is_the_agents_own_prompt() {
        let s = split(11_000, 168, 4, 2, 0);
        assert_eq!(
            s,
            Split {
                system: 11_000 - 168 - 4 * SKILL_TOKENS - 2 * MCP_SERVER_TOKENS,
                skills: 4 * SKILL_TOKENS,
                mcp: 2 * MCP_SERVER_TOKENS,
                instructions: 0,
                messages: 168,
            }
        );
        // And a window smaller than the estimates never goes negative.
        let tiny = split(500, 10, 4, 2, 0);
        assert_eq!(tiny.mcp + tiny.skills + tiny.messages + tiny.system, 500);
    }

    #[test]
    fn instructions_larger_than_the_agents_count_never_underflow_the_servers_row() {
        // A fresh session counts 1,200 tokens while the project's notes estimate 3,000.
        let s = split(1_200, 100, 0, 2, 3_000);
        assert_eq!(
            s.mcp, 1_200,
            "the segment is capped at what the agent counts"
        );
        assert_eq!(
            s.instructions, 1_200,
            "the instructions take what room there is"
        );
        assert_eq!(
            s.mcp - s.instructions,
            0,
            "the servers row is what's left, not a wrap"
        );
        // With room for both, each keeps its own size.
        let roomy = split(50_000, 100, 0, 2, 3_000);
        assert_eq!(roomy.instructions, 3_000);
        assert_eq!(roomy.mcp - roomy.instructions, 2 * MCP_SERVER_TOKENS);
        // The servers give way first when only some of them fit.
        let some = split(2_000, 100, 0, 2, 1_500);
        assert_eq!((some.mcp, some.instructions), (2_000, 1_500));
    }

    #[test]
    fn what_the_transcript_doesnt_hold_is_still_conversation_past_a_prompts_size() {
        // A long thread whose file reads and images Trek's transcript only summarizes.
        let s = split(120_400, 315, 4, 3, 0);
        assert_eq!(s.system, SYSTEM_TOKENS, "a prompt is only so big");
        assert_eq!(s.messages, 120_400 - SYSTEM_TOKENS - s.mcp - s.skills);
    }

    #[test]
    fn the_transcript_is_estimated_at_four_characters_a_token() {
        let items = vec![
            Item::Assistant {
                text: "efgh".repeat(10),
            },
            Item::Tool {
                id: "t".into(),
                title: "Read".into(),
                detail: "abcd".into(),
                output: "efgh".into(),
                status: trek_core::store::ToolStatus::Done,
            },
            Item::TurnEnd {
                at: 0,
                took_secs: 1,
            },
        ];
        let (messages, tokens) = transcript(&items);
        assert_eq!(messages, 2, "a turn's end is no message");
        assert_eq!(tokens, 13, "52 characters, rounded up");
    }

    #[test]
    fn a_reset_within_the_week_reads_as_its_day() {
        // 2026-10-26T15:00:00Z, a Monday.
        let at = 1_793_026_800_000;
        let day = day_clock(at);
        assert!(day.starts_with("Mon "), "{day}");
    }

    #[test]
    fn token_counts_read_as_the_card_writes_them() {
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(8_300), "8.3k");
        assert_eq!(fmt_tokens(32_900), "32.9k");
        assert_eq!(fmt_tokens(33_000), "33k");
        assert_eq!(fmt_tokens(604_100), "604.1k");
        assert_eq!(fmt_tokens(541_600), "541.6k");
        assert_eq!(fmt_tokens(1_000_000), "1M");
        assert_eq!(fmt_tokens(1_500_000), "1.5M");
    }

    #[test]
    fn the_footer_rounds_what_is_left_to_whole_thousands() {
        assert_eq!(fmt_round(362_900), "363k", "as the design says it");
        assert_eq!(fmt_round(72_900), "73k");
        assert_eq!(fmt_round(8_300), "8.3k", "small figures keep their decimal");
        assert_eq!(fmt_round(999_400), "999k");
    }
}
