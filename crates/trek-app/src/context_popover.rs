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
use crate::workspace::{Scope, Workspace, fmt_tokens};
use gpui_kit::component::button::Button;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
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
/// The segmented bar's height.
const BAR: f32 = 10.;
/// Below this share of the context window the ring stays out of the way.
pub const QUIET: f32 = 0.2;
/// From this share the ring says how much is used.
pub const NEAR: f32 = 0.6;

/// How full the window is, 0–1.
pub fn fraction(used: u64, window: u64) -> f32 {
    (used as f32 / window.max(1) as f32).clamp(0., 1.)
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

/// One segment of the bar.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    System,
    Skills,
    Mcp,
    Messages,
    Free,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::System => "System & tools",
            Kind::Skills => "Skills",
            Kind::Mcp => "MCP",
            Kind::Messages => "Messages",
            Kind::Free => "Free",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Kind::System => "context-seg-system",
            Kind::Skills => "context-seg-skills",
            Kind::Mcp => "context-seg-mcp",
            Kind::Messages => "context-seg-messages",
            Kind::Free => "context-seg-free",
        }
    }

    fn color(self, cx: &App) -> Hsla {
        let theme = cx.theme();
        match self {
            Kind::System => palette::indigo(cx),
            Kind::Skills => palette::ember(cx),
            Kind::Mcp => palette::emerald(cx),
            Kind::Messages => palette::sky(cx),
            Kind::Free => theme.foreground.opacity(0.1),
        }
    }
}

/// A segment: its size in the window, and what the legend row says about it.
struct Part {
    kind: Kind,
    tokens: u64,
    /// Its share of the window, 0–1.
    share: f32,
    /// Whether the size is Trek's estimate rather than the agent's own count.
    estimated: bool,
    tip: (String, Vec<String>),
}

/// One of the plan's usage windows.
struct LimitRow {
    id: &'static str,
    label: String,
    percent: f32,
    resets: String,
    tip: (String, Vec<String>),
}

/// The plan's usage limits, as the agent last reported them.
struct Limits {
    session: Option<LimitRow>,
    weekly: Option<LimitRow>,
    /// The thread is paused at a limit right now.
    paused: Option<String>,
    /// Why there are no rows (the agent reports none, or hasn't been read).
    note: Option<String>,
}

/// Everything the card shows, gathered once per render.
pub struct Card {
    used: u64,
    window: u64,
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
        let id = thread.map(|t| t.id.as_str());
        let live = id.and_then(|id| ws.live.get(id));
        let (messages, transcript_tokens) = live.map(|l| transcript(&l.items)).unwrap_or((0, 0));
        // The agent's count is the truth; the transcript can't hold more of it than that.
        let message_tokens = transcript_tokens.min(used);
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
        // What the agent counts that isn't a message: its prompt and its tools' definitions.
        // Split by what Trek knows is configured, at rough per-item sizes; the rest is the
        // agent's own prompt.
        let rest = used - message_tokens;
        let mcp_tokens = (mcp.len() as u64 * MCP_SERVER_TOKENS).min(rest);
        let skill_tokens = (skills.len() as u64 * SKILL_TOKENS).min(rest - mcp_tokens);
        let system_tokens = rest - mcp_tokens - skill_tokens;
        let free = window - used;
        let share = |n: u64| n as f32 / window as f32;
        let count =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = vec![];
        if system_tokens > 0 {
            parts.push(Part {
                kind: Kind::System,
                tokens: system_tokens,
                share: share(system_tokens),
                estimated: true,
                tip: (
                    "System & tools".into(),
                    vec![
                        "The agent's own prompt and its tools' definitions.".into(),
                        "Estimated: what the agent counts that isn't a message, skill or server."
                            .into(),
                    ],
                ),
            });
        }
        if skill_tokens > 0 {
            parts.push(Part {
                kind: Kind::Skills,
                tokens: skill_tokens,
                share: share(skill_tokens),
                estimated: true,
                tip: (
                    "Skills".into(),
                    vec![
                        format!(
                            "{} the agent can be asked for, about {SKILL_TOKENS} tokens each.",
                            count(skills.len(), "skill prompt", "skill prompts")
                        ),
                        skills.join(", "),
                    ],
                ),
            });
        }
        if mcp_tokens > 0 {
            let per = mcp_tokens / mcp.len() as u64;
            let mut lines: Vec<String> = mcp
                .iter()
                .map(|name| format!("{name} · about {} tokens", fmt_tokens(per)))
                .collect();
            if let Some(tokens) = instructions {
                lines.push(format!(
                    "Instructions · about {} tokens (what Trek tells it about the project)",
                    fmt_tokens(tokens)
                ));
            }
            lines.push(format!(
                "{} about {MCP_SERVER_TOKENS} tokens each. Estimated.",
                count(mcp.len(), "server", "servers")
            ));
            parts.push(Part {
                kind: Kind::Mcp,
                tokens: mcp_tokens,
                share: share(mcp_tokens),
                estimated: true,
                tip: ("MCP".into(), lines),
            });
        }
        if message_tokens > 0 {
            let mut lines = vec![
                format!(
                    "{} in the transcript.",
                    count(
                        messages,
                        "message and tool result",
                        "messages and tool results"
                    )
                ),
                format!(
                    "About {} tokens from their text ({CHARS_PER_TOKEN} characters a token).",
                    fmt_tokens(message_tokens)
                ),
            ];
            if message_tokens < transcript_tokens {
                lines.push(
                    "The transcript holds more text than the window: the agent has summarized it."
                        .into(),
                );
            }
            parts.push(Part {
                kind: Kind::Messages,
                tokens: message_tokens,
                share: share(message_tokens),
                estimated: true,
                tip: ("Messages".into(), lines),
            });
        }
        parts.push(Part {
            kind: Kind::Free,
            tokens: free,
            share: share(free),
            estimated: false,
            tip: (
                "Free".into(),
                vec![format!(
                    "{} left in the {} window.",
                    fmt_tokens(free),
                    fmt_tokens(window)
                )],
            ),
        });
        Card {
            used,
            window,
            parts,
            limits: Limits::of(ws, thread),
            compact,
            agent: thread.map(|t| t.agent.clone()),
            running: id.is_some_and(|id| ws.turn_running(id)),
            no_thread: thread.is_none(),
        }
    }

    pub fn render(&self, me: WeakEntity<Composer>, cx: &App) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let percent = (self.used as f32 / self.window as f32 * 100.).round() as u32;
        let header = h_flex()
            .w_full()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .text_color(theme.foreground)
                    .child(format!("Context {percent}%")),
            )
            .child(div().flex_1())
            .child(div().text_xs().text_color(muted).child(format!(
                "{} of {}",
                fmt_tokens(self.used),
                fmt_tokens(self.window)
            )));
        let bar = h_flex()
            .w_full()
            .h(px(BAR))
            .rounded(px(BAR / 2.))
            .overflow_hidden()
            .bg(theme.foreground.opacity(0.06))
            .children(
                self.parts
                    .iter()
                    .map(|p| div().h_full().w(relative(p.share)).bg(p.kind.color(cx))),
            );
        // While compacting, the bar gives up its right end as if the messages were being folded
        // away; it's the same segments, shrinking.
        let bar = match self.compact {
            Compact::Running { generation, .. } => div()
                .w_full()
                .child(bar)
                .with_animation(
                    ("context-compress", generation),
                    Animation::new(Duration::from_millis(1_400)),
                    |el, t| el.pr(px(170. * t)),
                )
                .into_any_element(),
            _ => bar.into_any_element(),
        };
        let legend = v_flex().w_full().gap(px(1.)).children(
            self.parts
                .iter()
                .filter(|p| p.kind != Kind::Free)
                .map(|p| legend_row(p, cx)),
        );
        let footer = self.footer(me, cx);
        let mut card = ui::menu_surface(cx).child(
            v_flex()
                .w(px(360.))
                .p(px(11.))
                .gap(px(10.))
                .child(header)
                .child(bar)
                .child(legend)
                .child(div().h(px(1.)).w_full().bg(theme.border))
                .child(self.limits.render(cx))
                .child(div().h(px(1.)).w_full().bg(theme.border))
                .child(footer),
        );
        if self.parts.iter().all(|p| p.kind == Kind::Free) {
            // Nothing counted yet: say so rather than show an empty bar.
            card = card.child(div());
        }
        card.into_any_element()
    }

    fn footer(&self, me: WeakEntity<Composer>, cx: &App) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let emerald = palette::emerald(cx);
        let status = match self.compact {
            Compact::Running { .. } => h_flex()
                .gap(px(5.))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child("Summarizing messages"),
                )
                .into_any_element(),
            Compact::Freed { tokens, .. } => h_flex()
                .gap(px(5.))
                .child(Icon::new(IconName::Check).xsmall().text_color(emerald))
                .child(
                    div()
                        .text_xs()
                        .text_color(emerald)
                        .child(format!("Freed {} tokens", fmt_tokens(tokens))),
                )
                .into_any_element(),
            Compact::Unchanged { .. } => div()
                .text_xs()
                .text_color(muted)
                .child("The agent kept the context as it was")
                .into_any_element(),
            Compact::Idle => {
                let left = self.window - self.used;
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(if left == 0 {
                        "The window is full".to_string()
                    } else {
                        format!("{} until the window is full", fmt_tokens(left))
                    })
                    .into_any_element()
            }
        };
        let button = match self.compact {
            Compact::Running { .. } => Button::new("context-compact")
                .small()
                .label("Compacting…")
                .loading(true)
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
            .child(div().flex_1().min_w_0().child(status))
            .child(button)
            .into_any_element()
    }
}

fn legend_row(part: &Part, cx: &App) -> AnyElement {
    let theme = cx.theme().clone();
    let value = if part.estimated {
        format!("{} est.", fmt_tokens(part.tokens))
    } else {
        fmt_tokens(part.tokens)
    };
    let (title, lines) = (part.tip.0.clone(), part.tip.1.clone());
    h_flex()
        .id(part.kind.id())
        .test_support()
        .w_full()
        .h(px(24.))
        .px(px(5.))
        .gap(px(8.))
        .rounded(px(6.))
        .items_center()
        .hover(|s| s.bg(theme.foreground.opacity(0.06)))
        .child(
            div()
                .size(px(8.))
                .rounded_full()
                .flex_none()
                .bg(part.kind.color(cx)),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.foreground)
                .child(part.kind.label()),
        )
        .child(div().flex_1())
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(value),
        )
        .tooltip(tooltip(title, lines))
        .into_any_element()
}

impl Limits {
    fn of(ws: &Workspace, thread: Option<&Thread>) -> Limits {
        let Some(thread) = thread else {
            return Limits {
                session: None,
                weekly: None,
                paused: None,
                note: None,
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
        // The per-model windows the plan reports ("Weekly · Opus"), for the weekly row's detail.
        let models: Vec<(String, f32)> = limits
            .iter()
            .filter(|l| l.label.contains('·'))
            .map(|l| (l.label.clone(), l.percent))
            .collect();
        let mut weekly = weekly;
        if let Some(w) = weekly.as_mut() {
            if models.is_empty() {
                w.tip.1.push("No per-model windows reported.".into());
            } else {
                for (label, percent) in &models {
                    w.tip.1.push(format!("{label} · {percent:.0}%"));
                }
            }
        }
        let note = if limits.is_empty() {
            Some(match status {
                Some(_) => format!("{} reports no plan limits", thread.agent.display_name()),
                None => format!("No plan limits read for {}", thread.agent.display_name()),
            })
        } else {
            None
        };
        Limits {
            session,
            weekly,
            paused,
            note,
        }
    }

    fn render(&self, cx: &App) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        v_flex()
            .w_full()
            .gap(px(6.))
            .child(div().text_xs().text_color(muted).child("Limits"))
            .when_some(self.session.as_ref(), |el, row| {
                el.child(limit_row(row, cx))
            })
            .when_some(self.weekly.as_ref(), |el, row| el.child(limit_row(row, cx)))
            .when_some(self.note.clone(), |el, note| {
                el.child(div().text_xs().text_color(muted).child(note))
            })
            .when_some(self.paused.clone(), |el, paused| {
                el.child(div().text_xs().text_color(palette::amber(cx)).child(paused))
            })
            .into_any_element()
    }
}

fn row(limit: &UsageLimit, now: i64, weekly: bool) -> LimitRow {
    let resets = match limit.resets_at {
        Some(at) if weekly => format!("Resets {}", day_clock(at)),
        Some(at) => format!("Resets in {}", crate::time::countdown(at, now)),
        None => "Reset time unknown".to_string(),
    };
    let mut lines = vec![format!("{:.0}% of the window used", limit.percent)];
    if let Some(at) = limit.resets_at {
        lines.push(format!(
            "Resets {} (in {})",
            crate::time::reset_clock(at, now),
            crate::time::countdown(at, now)
        ));
    }
    if !weekly {
        lines.push("One window shared by every model.".into());
    }
    LimitRow {
        id: if weekly {
            "context-limit-weekly"
        } else {
            "context-limit-session"
        },
        label: limit.label.clone(),
        percent: limit.percent,
        resets,
        tip: (limit.label.clone(), lines),
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

fn limit_row(row: &LimitRow, cx: &App) -> AnyElement {
    let theme = cx.theme().clone();
    let color = if row.percent >= 95. {
        palette::red(cx)
    } else if row.percent >= 85. {
        palette::amber(cx)
    } else {
        theme.foreground.opacity(0.55)
    };
    let (title, lines) = (row.tip.0.clone(), row.tip.1.clone());
    v_flex()
        .id(row.id)
        .test_support()
        .w_full()
        .gap(px(4.))
        .px(px(5.))
        .py(px(4.))
        .rounded(px(6.))
        .hover(|s| s.bg(theme.foreground.opacity(0.06)))
        .child(
            h_flex()
                .w_full()
                .gap(px(6.))
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(format!("{} {:.0}%", row.label, row.percent)),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(row.resets.clone()),
                ),
        )
        .child(
            div()
                .w_full()
                .h(px(4.))
                .rounded(px(2.))
                .overflow_hidden()
                .bg(theme.foreground.opacity(0.08))
                .child(
                    div()
                        .h_full()
                        .w(relative((row.percent / 100.).clamp(0., 1.)))
                        .bg(color),
                ),
        )
        .tooltip(tooltip(title, lines))
        .into_any_element()
}

/// A tooltip with a title and a few lines under it.
fn tooltip(
    title: String,
    lines: Vec<String>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| {
        let (title, lines) = (title.clone(), lines.clone());
        Tooltip::element(move |_, cx| {
            v_flex()
                .gap(px(3.))
                .max_w(px(300.))
                .child(div().text_sm().font_medium().child(title.clone()))
                .children(lines.iter().map(|line| {
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(line.clone())
                }))
        })
        .build(window, cx)
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
    use super::{day_clock, transcript};
    use trek_core::store::Item;

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
}
