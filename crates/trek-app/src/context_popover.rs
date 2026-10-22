//! The card behind the composer's context ring: what fills the context window, how full the
//! plan's usage windows are, and Compact now.
//!
//! What the agent reports is shown as it reports it: the window's size and what's used. It
//! reports no breakdown, so the card estimates one from what Trek knows (the transcript's text,
//! the skill prompts and MCP servers a session is given) and says which rows are estimates. A
//! row Trek has nothing to go on for is left out rather than made up.

use crate::composer::Composer;
use crate::palette;
use crate::ui::{self, Pill};
use crate::workspace::{Scope, Workspace};
use gpui_kit::base::{
    Align, MotionStatus, POPUP_PRIORITY, Placement, Positioner, Transition, transition,
    transition_with_status,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    StyledExt as _, h_flex, v_flex,
};
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
/// How far a row's hover fill reaches past its text, which lines up with the header's.
const ROW_INSET: f32 = 6.;
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
/// The most small blocks the messages segment folds into while compacting.
const BLOCKS: u64 = 18;

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

/// The ring that fills as the window does, with the share beside it from `NEAR`. The composer
/// makes it a button; the editor's AI side bar draws it as it is.
pub fn dial(used: u64, window: u64, cx: &App) -> impl IntoElement {
    let theme = cx.theme().clone();
    let frac = fraction(used, window);
    let color = if frac >= 0.9 {
        palette::red(cx)
    } else if frac >= 0.75 {
        palette::amber(cx)
    } else {
        theme.foreground.opacity(0.75)
    };
    let track = theme.foreground.opacity(0.14);
    h_flex()
        .gap(px(5.))
        .items_center()
        // Near the limit the share is spelled out; before that it's on hover.
        .when(frac >= NEAR, |el| {
            el.child(
                div()
                    .text_xs()
                    .text_color(if frac >= 0.75 {
                        color
                    } else {
                        theme.muted_foreground
                    })
                    .child(format!("{:.0}%", frac * 100.)),
            )
        })
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let c = bounds.center();
                    let r = bounds.size.width.min(bounds.size.height) / 2. - px(1.5);
                    let ring = |from: f32, to: f32, color: Hsla, window: &mut Window| {
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
                    ring(0.0, 1.0, track, window);
                    if frac > 0.004 {
                        ring(0.0, frac, color, window);
                    }
                },
            )
            .size(px(16.)),
        )
}

/// The ring as the card's trigger. The tooltip says what the card says, so it shows only while
/// the card is closed; the ring lights up while it's open.
#[derive(IntoElement)]
pub struct RingTrigger {
    tip: SharedString,
    open: bool,
    dial: AnyElement,
}

impl RingTrigger {
    pub fn new(tip: String, dial: AnyElement) -> Self {
        Self {
            tip: tip.into(),
            open: false,
            dial,
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
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let ring = Pill::new("context-ring").ghost(true).selected(self.open);
        if self.open {
            ring.child(self.dial)
        } else {
            ring.tooltip(self.tip).child(self.dial)
        }
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

    fn color(self, cx: &App) -> Hsla {
        let theme = cx.theme();
        match self {
            Kind::System => palette::indigo(cx),
            Kind::Mcp => palette::emerald(cx),
            Kind::Skills => palette::ember(cx),
            Kind::Messages => palette::sky(cx),
            Kind::Free => theme.foreground.opacity(0.12),
            Kind::Reserved => theme.foreground.opacity(0.32),
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
            let mut detail = vec![(
                mcp.join(", "),
                Some(fmt_tokens(mcp_tokens - instructions.unwrap_or(0))),
            )];
            if let Some(tokens) = instructions {
                detail.push(("Instructions".into(), Some(fmt_tokens(tokens))));
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

    pub fn render(
        &self,
        me: WeakEntity<Composer>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
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
        // While Compact now runs (or the fold is still playing) the messages segment is small
        // blocks collapsing left; the rest of the time it's one piece like the others.
        let folding = matches!(self.compact, Compact::Running { .. })
            || folded.status == MotionStatus::Running;
        if matches!(self.compact, Compact::Running { .. }) {
            // The shimmer and the fold need frames; idle, the card asks for none.
            window.request_animation_frame();
        }
        let frac = fraction(shown_used, self.window);
        let header = h_flex()
            .w_full()
            .items_baseline()
            .gap(px(5.))
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .text_color(theme.foreground)
                    .child("Context"),
            )
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .font_features(figures())
                    .text_color(level_color(frac, theme.foreground, cx))
                    .child(format!("{:.0}%", frac * 100.)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .font_features(figures())
                    .child(format!(
                        "{} of {}",
                        fmt_tokens(shown_used),
                        fmt_tokens(self.window)
                    )),
            );
        // Every segment that holds something keeps a sliver, so a few hundred tokens of skills
        // still show beside a hundred thousand of messages; the hairline gaps let it read as
        // parts. The free middle shows the track through its spacer; Reserved is the grey block
        // at the far right.
        let dim = |kind: Kind, window: &mut Window, cx: &mut App| {
            transition(
                SharedString::from(format!("context-dim-{}", kind.row_id())),
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
        let track = theme.foreground.opacity(0.09);
        let last = self.parts.len() - 1;
        let bar = h_flex()
            .w_full()
            .h(px(BAR))
            .gap(px(2.))
            .rounded(px(BAR / 2.))
            .bg(track)
            .children(self.parts.iter().enumerate().map(|(i, p)| {
                let opacity = dim(p.kind, window, cx);
                let share = match p.kind {
                    Kind::Messages => shown_messages as f32 / self.window as f32,
                    Kind::Free => shown_free as f32 / self.window as f32,
                    _ => p.share,
                };
                let seg = |el: Div| {
                    el.id(p.kind.bar_id())
                        .test_support()
                        .h_full()
                        .w(relative(share.max(0.)))
                        .opacity(opacity)
                        .on_hover(hover_seg(p.kind, hover.clone(), me.clone()))
                };
                match p.kind {
                    Kind::Free => {
                        // The spacer's hitbox hovers the flyout; the track shows through it.
                        seg(div()).into_any_element()
                    }
                    Kind::Messages if folding => {
                        let chunk = ((messages.max(shown_messages) + BLOCKS - 1) / BLOCKS).max(1);
                        let n = shown_messages.div_ceil(chunk).max(1);
                        seg(h_flex())
                            .gap(px(1.))
                            .children((0..n).map(|_| {
                                div()
                                    .h_full()
                                    .flex_1()
                                    .rounded(px(1.))
                                    .bg(Kind::Messages.color(cx))
                            }))
                            // The fill's edge while it folds: a caret the free space follows.
                            .child(
                                div()
                                    .w(px(2.))
                                    .h_full()
                                    .flex_none()
                                    .bg(theme.foreground.opacity(0.8)),
                            )
                            .into_any_element()
                    }
                    _ => seg(div())
                        .min_w(px(3.))
                        .rounded(px(1.5))
                        .when(i == 0, |el| el.rounded_l(px(BAR / 2.)))
                        .when(i == last, |el| el.rounded_r(px(BAR / 2.)))
                        .bg(p.kind.color(cx))
                        .into_any_element(),
                }
            }));
        // Two columns, the rows' hover fill reaching past the text so the swatches line up with
        // the header above.
        let legend = div()
            .mx(px(-ROW_INSET))
            .grid()
            .grid_cols(2)
            .gap_x(px(4.))
            .children(self.parts.iter().map(|p| {
                legend_row(
                    p,
                    hovered,
                    hover.clone(),
                    rows.clone(),
                    me.clone(),
                    window,
                    cx,
                )
            }));
        // The flyout to the card's right (flipped left when there's no room), on the row the
        // pointer hovers.
        let flyout = hovered
            .and_then(|kind| self.part(kind))
            .and_then(|p| {
                let anchor = rows
                    .read(cx)
                    .borrow()
                    .get(&p.kind)
                    .copied()
                    .unwrap_or_else(|| bounds.read(cx).get());
                (anchor.size.width > px(0.)).then(|| (p, anchor))
            })
            .map(|(p, anchor)| {
                deferred(
                    Positioner::side(anchor)
                        .placement(Placement::Right)
                        .align(Align::Start)
                        .offset(px(10.))
                        .margin(px(8.))
                        .occlude()
                        .child(flyout(p, cx)),
                )
                .with_priority(POPUP_PRIORITY + 1)
                .into_any_element()
            });
        let rule = || div().h(px(1.)).w_full().bg(theme.border);
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
                ui::menu_surface(cx).child(
                    v_flex()
                        .w(px(340.))
                        .p(px(11.))
                        .gap(px(10.))
                        .child(v_flex().gap(px(8.)).child(header).child(bar))
                        .child(legend)
                        .when_some(
                            self.limits
                                .render(limit_hover.clone(), me.clone(), window, cx),
                            |el, limits| el.child(rule()).child(limits),
                        )
                        .child(rule())
                        .child(self.footer(me, window, shown_free, folding, cx)),
                ),
            )
            .when_some(flyout, |el, f| el.child(f))
            .with_animation(
                "context-card",
                Animation::new(Duration::from_millis(200)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t).mt(px(4. * (1. - t))),
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
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let emerald = palette::emerald(cx);
        // What's free is the legend's last row but one; the footer speaks only when it has news,
        // else it says how much room is left before the agent compacts on its own.
        let summarizing = |window: &mut Window| -> AnyElement {
            window.request_animation_frame();
            div()
                .text_xs()
                .child(crate::working_bar::shimmer(
                    "Summarizing messages".into(),
                    clock(),
                    cx.reduce_motion(),
                    muted,
                    theme.foreground,
                ))
                .into_any_element()
        };
        let status = match self.compact {
            Compact::Running { .. } => Some(summarizing(window)),
            Compact::Freed { .. } if folding => Some(summarizing(window)),
            Compact::Freed { tokens, .. } => Some(
                h_flex()
                    .gap(px(5.))
                    .child(Icon::new(IconName::Check).xsmall().text_color(emerald))
                    .child(
                        div()
                            .text_xs()
                            .text_color(emerald)
                            .child(format!("Freed {} tokens", fmt_tokens(tokens))),
                    )
                    .into_any_element(),
            ),
            Compact::Unchanged { .. } => Some(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("The agent kept the context as it was")
                    .into_any_element(),
            ),
            Compact::Idle if self.used >= self.window => Some(
                div()
                    .text_xs()
                    .text_color(palette::red(cx))
                    .child("The window is full")
                    .into_any_element(),
            ),
            Compact::Idle => Some(
                div()
                    .text_xs()
                    .text_color(muted)
                    .font_features(figures())
                    .child(format!("{} until auto-compact", fmt_tokens(until)))
                    .into_any_element(),
            ),
        };
        let button = match self.compact {
            Compact::Running { .. } => Button::new("context-compact")
                .small()
                .label("Compact now")
                .disabled(true),
            Compact::Freed { .. } => Button::new("context-compact")
                .small()
                .label("Compacted")
                .disabled(true),
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
                Button::new("context-compact")
                    .small()
                    .info()
                    .label("Compact now")
                    .disabled(self.no_thread || self.running || self.limits.paused.is_some())
                    .tooltip(why)
                    .on_click(move |_, _, cx| {
                        let _ = me.update(cx, |this, cx| this.compact_now(cx));
                    })
            }
        };
        h_flex()
            .w_full()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().min_w_0().children(status))
            .child(button)
            .into_any_element()
    }
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

/// Red at 90%, amber at 75%, `calm` below: the dial's thresholds.
fn level_color(frac: f32, calm: Hsla, cx: &App) -> Hsla {
    if frac >= 0.9 {
        palette::red(cx)
    } else if frac >= 0.75 {
        palette::amber(cx)
    } else {
        calm
    }
}

/// A legend cell: swatch, name, size. Hovering lifts the row, dims the rest, and opens the
/// part's flyout to the card's side.
fn legend_row(
    part: &Part,
    hovered: Option<Kind>,
    hover: HoveredPart,
    rows: RowBounds,
    me: WeakEntity<Composer>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let on = hovered == Some(part.kind);
    let opacity = transition(
        SharedString::from(format!("context-dim-{}", part.kind.row_id())),
        if hovered.is_some() && !on { 0.35 } else { 1. },
        Transition::new(FADE),
        window,
        cx,
    );
    let kind = part.kind;
    // The name takes the row's color: GPUI fixes a text's color when it lays it out, before a
    // group hover is known, but the row's own hover is known by then.
    h_flex()
        .id(kind.row_id())
        .test_support()
        .relative()
        .min_w_0()
        .h(px(24.))
        .px(px(ROW_INSET))
        .gap(px(7.))
        .rounded(px(6.))
        .items_center()
        .text_xs()
        .opacity(opacity)
        .text_color(theme.muted_foreground)
        .hover(|s| {
            s.bg(theme.foreground.opacity(0.07))
                .text_color(theme.foreground)
        })
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
                .rounded(px(2.5))
                .flex_none()
                .bg(part.kind.color(cx)),
        )
        .child(div().flex_1().min_w_0().truncate().child(part.kind.label()))
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(theme.foreground)
                .font_features(figures())
                .child(fmt_tokens(part.tokens)),
        )
        .into_any_element()
}

/// A part's flyout: a colour dash, its name, its size and share, then its breakdown.
fn flyout(part: &Part, cx: &App) -> AnyElement {
    let theme = cx.theme().clone();
    let muted = theme.muted_foreground;
    let row = |(label, value): &(String, Option<String>)| {
        h_flex()
            .gap(px(8.))
            .items_baseline()
            .child(
                div()
                    .min_w_0()
                    .text_xs()
                    .text_color(muted)
                    .child(label.clone()),
            )
            .child(div().flex_1())
            .when_some(value.clone(), |el, value| {
                el.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .font_features(figures())
                        .text_color(theme.foreground)
                        .child(value),
                )
            })
    };
    ui::menu_surface(cx)
        .child(
            v_flex()
                .w(px(240.))
                .p(px(10.))
                .gap(px(6.))
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(7.))
                        .child(
                            div()
                                .w(px(10.))
                                .h(px(3.))
                                .rounded(px(1.5))
                                .flex_none()
                                .bg(part.kind.color(cx)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_medium()
                                .text_color(theme.foreground)
                                .child(part.kind.label()),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_xs()
                                .font_features(figures())
                                .text_color(theme.foreground)
                                .child(fmt_tokens(part.tokens)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_features(figures())
                                .text_color(muted)
                                .child(format!("{:.0}%", part.share * 100.)),
                        ),
                )
                .children(part.detail.iter().map(row))
                .when_some(part.note.clone(), |el, note| {
                    el.child(div().text_xs().text_color(muted.opacity(0.8)).child(note))
                }),
        )
        .with_animation(
            SharedString::from(format!("context-flyout-{}", part.kind.row_id())),
            Animation::new(FADE).with_easing(ease_out_quint()),
            |el, t| el.opacity(t).mt(px(4. * (1. - t))),
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
                .gap(px(2.))
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
                    el.child(div().text_xs().text_color(palette::amber(cx)).child(paused))
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
        label: limit.label.clone(),
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
/// window holds, hovering splits the bar by it (and dims the other row).
fn limit_row(
    row: &LimitRow,
    hovered: Option<&'static str>,
    hover: HoveredLimit,
    me: WeakEntity<Composer>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let on = hovered == Some(row.id);
    let opacity = transition(
        SharedString::from(format!("context-dim-{}", row.id)),
        if hovered.is_some() && !on { 0.35 } else { 1. },
        Transition::new(FADE),
        window,
        cx,
    );
    let color = if row.percent >= 95. {
        palette::red(cx)
    } else if row.percent >= 85. {
        palette::amber(cx)
    } else {
        palette::sky(cx)
    };
    let expanded = on && !row.split.is_empty();
    let header = if expanded {
        h_flex()
            .w_full()
            .gap(px(14.))
            .items_baseline()
            .children(row.split.iter().enumerate().map(|(i, (name, percent))| {
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .child(
                        div()
                            .w(px(10.))
                            .h(px(3.))
                            .rounded(px(1.5))
                            .flex_none()
                            .bg(split_tone(i, color)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.foreground)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_features(figures())
                            .text_color(theme.muted_foreground)
                            .child(format!("{percent:.0}%")),
                    )
            }))
            .into_any_element()
    } else {
        h_flex()
            .w_full()
            .gap(px(6.))
            .items_baseline()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.foreground)
                    .child(row.label.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .font_features(figures())
                    .text_color(theme.foreground)
                    .child(format!("{:.0}%", row.percent)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .font_features(figures())
                    .child(row.resets.clone()),
            )
            .into_any_element()
    };
    let bar = div()
        .w_full()
        .h(px(4.))
        .rounded_full()
        .bg(theme.foreground.opacity(0.08))
        .child(if expanded {
            h_flex()
                .size_full()
                .gap(px(2.))
                .children(row.split.iter().enumerate().map(|(i, (_, percent))| {
                    div()
                        .h_full()
                        .rounded_full()
                        .w(relative((percent / 100.).clamp(0., 1.)))
                        .bg(split_tone(i, color))
                }))
                .into_any_element()
        } else {
            div()
                .h_full()
                .rounded_full()
                .w(relative((row.percent / 100.).clamp(0., 1.)))
                .bg(color)
                .into_any_element()
        });
    let id = row.id;
    v_flex()
        .id(row.id)
        .test_support()
        .w_full()
        .gap(px(5.))
        .px(px(ROW_INSET))
        .py(px(5.))
        .rounded(px(6.))
        .opacity(opacity)
        .when(!row.split.is_empty(), |el| {
            el.hover(|s| s.bg(theme.foreground.opacity(0.07))).on_hover(
                move |on: &bool, _window: &mut Window, cx: &mut App| {
                    hover.update(cx, |h, _| *h = on.then_some(id));
                    let _ = me.update(cx, |_, cx| cx.notify());
                },
            )
        })
        .child(header)
        .child(bar)
        .into_any_element()
}

/// The split's tones: the first share full colour, the rest ever fainter.
fn split_tone(i: usize, color: Hsla) -> Hsla {
    match i {
        0 => color,
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
    messages: u64,
}

/// What Trek knows is configured is in every request, so it's counted first, at rough per-item
/// sizes. The transcript's share comes out of what's left: the agent's count is the truth, and a
/// summarized transcript holds more text than the window does. The rest is the agent's own
/// prompt and its tools' definitions, up to what such a prompt plausibly takes; anything past
/// that is conversation too.
fn split(used: u64, transcript: u64, skills: usize, mcp: usize, instructions: u64) -> Split {
    let mcp = (mcp as u64 * MCP_SERVER_TOKENS + instructions).min(used);
    let skills = (skills as u64 * SKILL_TOKENS).min(used - mcp);
    let rest = used - mcp - skills;
    let messages = transcript.min(rest).max(rest.saturating_sub(SYSTEM_TOKENS));
    Split {
        system: rest - messages,
        skills,
        mcp,
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
        MCP_SERVER_TOKENS, SKILL_TOKENS, SYSTEM_TOKENS, Split, day_clock, fmt_tokens, split,
        transcript,
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
                messages: 168,
            }
        );
        // And a window smaller than the estimates never goes negative.
        let tiny = split(500, 10, 4, 2, 0);
        assert_eq!(tiny.mcp + tiny.skills + tiny.messages + tiny.system, 500);
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
}
