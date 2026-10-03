//! The bar above the composer while an agent works. Its header names who's at it and for how
//! long ("Opus 5.5 working for 17s") beside the hiker on its trail; above it, the turn's live group
//! of tool calls: a summary line ("Ran 2 commands · Exploring the project") and a row per call,
//! the newest sliding in. When the group ends it folds into its summary, which the transcript
//! then shows as a row of its own.
//!
//! It's a view of its own with its own ticker, so its animation frames re-render only this bar
//! (and the window around it), never the transcript, sidebar or composer. Only a fold moves the
//! transcript (the bar shrinks under it), for its quarter second.

use crate::activity::{self, ToolKind};
use crate::workspace::{Scope, Workspace, WorkspaceEvent};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use trek_core::store::{Item, ToolStatus};
use trek_core::{AgentId, RunState};

const COLUMN: f32 = 760.;

/// The header's height: the trail plus the gap above the composer.
pub const HEIGHT: f32 = crate::mascot::HEIGHT + 6.;
/// The live group: its summary line, a row per call, "+N earlier" over the rows when some
/// scrolled away, and the space around them.
const SUMMARY: f32 = 28.;
const ROW: f32 = 22.;
const EARLIER: f32 = 20.;
const GROUP_PAD: f32 = 4.;
/// Calls shown at once; older ones fold into "+N earlier".
pub const ROWS: usize = 6;
/// How long a new row takes to slide in.
const SLIDE: Duration = Duration::from_millis(180);
/// One sweep of the shimmer across a line.
const SWEEP: f32 = 1.8;

pub struct WorkingBar {
    workspace: Entity<Workspace>,
    /// The thread the bar is for: the main window's (whatever it shows) or a thread window's.
    scope: Scope,
    /// What's on screen, to skip re-rendering on workspace changes that don't touch the bar.
    shown: Option<Shown>,
    /// The window is frontmost (or `TREK_FORCE_ACTIVE`): the animation runs at full rate.
    active: bool,
    /// When each call on show arrived, while it slides in.
    arrived: HashMap<String, Instant>,
    /// The live group's rows that have settled (done, and in), drawn by a cached view of their
    /// own; `settled_len` of them, from the top. The rest redraw with the bar.
    settled: Entity<SettledRows>,
    settled_len: usize,
    /// The part above the settled rows: a group folding away, and the live group's summary line.
    top: Entity<BarTop>,
    /// The ticker and the frame interval it runs at.
    _ticker: Option<(Duration, Task<()>)>,
    _subscriptions: Vec<Subscription>,
}

/// The bar's inputs besides the clock.
#[derive(Clone, PartialEq)]
struct Shown {
    thread: String,
    /// The header, while the turn runs (and nothing waits on the user).
    header: Option<Header>,
    /// The turn's live group of tool calls.
    group: Option<Group>,
    /// A group that just ended, folding away until the instant given.
    folding: Option<(Group, Instant)>,
    /// No motion: reduced motion, or the window isn't in front.
    still: bool,
}

#[derive(Clone, PartialEq)]
struct Header {
    agent: AgentId,
    /// "Opus 5.5", or the agent's name when it doesn't say.
    model: String,
    started: Option<Instant>,
    agents: usize,
}

#[derive(Clone, PartialEq)]
struct Group {
    summary: String,
    kind: ToolKind,
    phrase: String,
    /// The last `ROWS` calls, oldest first.
    rows: Vec<LiveRow>,
    /// Calls before those.
    earlier: usize,
}

#[derive(Clone, PartialEq)]
struct LiveRow {
    /// The call's item id.
    id: String,
    op: activity::Op,
    lines: Option<(u32, u32)>,
    running: bool,
    failed: bool,
    /// A sub-agent's progress ("Reading src/routes.rs · 2 steps").
    activity: Option<String>,
}

impl Group {
    fn rows_height(&self) -> f32 {
        self.rows.len() as f32 * ROW + if self.earlier > 0 { EARLIER } else { 0. }
    }
}

/// The group of calls from `items[start..]`, as the bar shows it.
fn group(ws: &Workspace, id: &str, tools: &[usize], headline: Option<&str>) -> Option<Group> {
    let live = ws.live.get(id)?;
    let cwd = ws.thread(id).and_then(|t| t.cwd.clone());
    let mut kinds = Vec::with_capacity(tools.len());
    let mut rows = Vec::with_capacity(tools.len().min(ROWS));
    let mut last = None;
    for (n, &ix) in tools.iter().enumerate() {
        let Some(Item::Tool { id: call, title, detail, status, .. }) = live.items.get(ix) else { continue };
        kinds.push(activity::tool_kind(title));
        last = Some((title, detail));
        if n + ROWS < tools.len() {
            continue;
        }
        let task = live.tasks.iter().find(|t| &t.id == call && t.done.is_none());
        rows.push(LiveRow {
            id: live.items.id_at(ix).unwrap_or_default().to_string(),
            op: activity::op(title, detail, cwd.as_deref()),
            lines: live.lines.get(call).copied(),
            running: *status == ToolStatus::Running,
            failed: matches!(status, ToolStatus::Failed | ToolStatus::Denied),
            activity: task.map(|t| {
                let steps = if t.tool_uses == 1 { "1 step".to_string() } else { format!("{} steps", t.tool_uses) };
                if t.activity.is_empty() { steps } else { format!("{} · {steps}", t.activity) }
            }),
        });
    }
    let (title, detail) = last?;
    Some(Group {
        summary: activity::summarize(&kinds),
        kind: kinds.last().copied().unwrap_or(ToolKind::Other),
        phrase: activity::phrase(title, detail, headline),
        earlier: tools.len().saturating_sub(ROWS),
        rows,
    })
}

/// The model `thread` runs, as people call it ("Opus 5.5"), or its agent's name.
fn model_name(ws: &Workspace, thread: &trek_core::store::Thread) -> String {
    let models = ws.models_for(&thread.agent);
    let named = match thread.model.as_deref() {
        Some(m) => Some(models.iter().find(|i| crate::composer::same_model(m, &i.id)).map(|i| i.name.clone()).unwrap_or_else(|| short_model(m))),
        None => crate::composer::default_model(&models).map(|m| m.name.clone()),
    };
    named.filter(|n| !n.is_empty()).unwrap_or_else(|| thread.agent.display_name())
}

/// A model id without its provider path: `openrouter/qwen/qwen3-coder` → "qwen3-coder".
fn short_model(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).to_string()
}

fn ease_out(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

impl WorkingBar {
    pub fn new(workspace: Entity<Workspace>, scope: Scope, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| this.sync(cx)),
            // Tool calls change only the transcript: no notify, an event.
            cx.subscribe(&workspace, |this, ws, event: &WorkspaceEvent, cx| {
                if let WorkspaceEvent::Transcript { id, .. } = event
                    && ws.read(cx).thread_id_in(&this.scope) == Some(id.as_str())
                {
                    this.sync(cx);
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                this.active = window.is_window_active() || crate::mascot::force_active();
                // Restart the ticker at the new rate rather than after its current wait.
                this._ticker = None;
                this.sync(cx);
            }),
        ];
        let bar = cx.weak_entity();
        let mut this = Self {
            top: cx.new(|_| BarTop { bar }),
            workspace,
            scope,
            shown: None,
            active: window.is_window_active() || crate::mascot::force_active(),
            arrived: HashMap::new(),
            settled: cx.new(|_| SettledRows { rows: vec![], earlier: 0 }),
            settled_len: 0,
            _ticker: None,
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    fn read(&self, cx: &App) -> Option<Shown> {
        let ws = self.workspace.read(cx);
        let id = ws.thread_id_in(&self.scope)?;
        let live = ws.live.get(id)?;
        let thread = ws.thread(id)?;
        // Trek's own setting: GPUI's reduce-motion flag holds toasts and spinners still in tests,
        // where the bar's frames are what's measured.
        let still = ws.settings.appearance.reduce_motion || !self.active;
        // The header shows while the thread works and isn't waiting on the user (its cards take
        // this spot then).
        let header = (thread.run_state == RunState::Working && live.permissions.is_empty()).then(|| Header {
            agent: thread.agent.clone(),
            model: model_name(ws, thread),
            started: live.turn_started,
            agents: live.active_tasks().max(live.background),
        });
        let current = activity::live(ws, id).and_then(|t| group(ws, id, &t.tools, t.headline.as_deref()));
        // Folding is motion: a still bar lets the group go at once.
        let folding = live.fold.as_ref().filter(|f| !still && f.until > Instant::now()).and_then(|f| {
            let start = live.items.position(&f.first)?;
            let tools: Vec<usize> = live.items[start..]
                .iter()
                .enumerate()
                .take_while(|(_, i)| matches!(i, Item::Tool { .. } | Item::Reasoning { .. }))
                .filter(|(_, i)| matches!(i, Item::Tool { .. }))
                .map(|(n, _)| start + n)
                .collect();
            Some((group(ws, id, &tools, None)?, f.until))
        });
        (header.is_some() || folding.is_some()).then(|| Shown { thread: id.to_string(), header, group: current, folding, still })
    }

    /// The height of the part above the settled rows: a folding group (shrinking frame by frame)
    /// and the live group's summary line.
    fn top_height(&self) -> f32 {
        let Some(s) = &self.shown else { return 0. };
        let folding = s.folding.as_ref().map_or(0., |(g, until)| {
            let left = 1. - ease_out(fold_progress(*until));
            GROUP_PAD + SUMMARY + (g.rows_height() + GROUP_PAD) * left
        });
        (folding + if s.group.is_some() { GROUP_PAD + SUMMARY } else { 0. }).round()
    }

    fn settled_height(&self) -> f32 {
        match self.shown.as_ref().and_then(|s| s.group.as_ref()) {
            Some(g) => self.settled_len as f32 * ROW + if g.earlier > 0 { EARLIER } else { 0. },
            None => 0.,
        }
    }

    /// The bar's own part: the live rows below the settled ones, and the header.
    fn bottom_height(&self) -> f32 {
        let Some(s) = &self.shown else { return 0. };
        let rows = s.group.as_ref().map_or(0., |g| g.rows.len().saturating_sub(self.settled_len) as f32 * ROW + GROUP_PAD);
        rows + if s.header.is_some() { HEIGHT } else { 0. }
    }

    /// Redraw the bar's animated parts.
    fn redraw(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if self.shown.as_ref().is_some_and(|s| s.group.is_some() || s.folding.is_some()) {
            self.top.update(cx, |_, cx| cx.notify());
        }
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let shown = self.read(cx);
        // Calls that weren't on show in this thread a moment ago slide in.
        if let Some(new) = shown.as_ref().filter(|s| !s.still) {
            let before: HashSet<&str> = match &self.shown {
                Some(old) if old.thread == new.thread => old.group.iter().flat_map(|g| g.rows.iter().map(|r| r.id.as_str())).collect(),
                _ => new.group.iter().flat_map(|g| g.rows.iter().map(|r| r.id.as_str())).collect(),
            };
            let now = Instant::now();
            for row in new.group.iter().flat_map(|g| &g.rows) {
                if !before.contains(row.id.as_str()) {
                    self.arrived.entry(row.id.clone()).or_insert(now);
                }
            }
        }
        self.arrived.retain(|_, at| at.elapsed() < SLIDE);
        if shown != self.shown {
            // The top's last frame: it may have had a group that's gone now.
            self.top.update(cx, |_, cx| cx.notify());
            self.shown = shown;
            self.redraw(cx);
        }
        self.settle(cx);
        self.tick(cx);
    }

    /// Hand the live group's leading rows that are done and in to the cached view: up to the
    /// first that's still running or sliding in.
    fn settle(&mut self, cx: &mut Context<Self>) {
        let (rows, earlier) = match self.shown.as_ref().and_then(|s| s.group.as_ref()) {
            Some(g) => {
                let n = g.rows.iter().position(|r| r.running || self.arrived.contains_key(&r.id)).unwrap_or(g.rows.len());
                (g.rows[..n].to_vec(), g.earlier)
            }
            None => (vec![], 0),
        };
        self.settled_len = rows.len();
        self.settled.update(cx, |s, cx| {
            if s.rows != rows || s.earlier != earlier {
                s.rows = rows;
                s.earlier = earlier;
                cx.notify();
            }
        });
    }

    /// How often the bar redraws now: briskly while a group folds, at
    /// the hiker's rate while the window is in front, once a second (the clock) otherwise.
    fn rate(&self) -> Option<Duration> {
        let s = self.shown.as_ref()?;
        if s.header.as_ref().is_none_or(|h| h.started.is_none()) && s.folding.is_none() {
            return None;
        }
        Some(if s.still {
            Duration::from_secs(1)
        } else if s.folding.is_some() {
            // 30 a second: smooth enough for a quarter second of motion, and each one redraws the
            // transcript too (it grows into the space). Rows slide in on the hiker's frames:
            // calls come often, and a fade that short reads the same at either rate.
            Duration::from_millis(33)
        } else {
            Duration::from_millis(1000 / crate::mascot::FPS)
        })
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let rate = self.rate();
        if self._ticker.as_ref().map(|(r, _)| *r) == rate {
            return;
        }
        self._ticker = rate.map(|every| {
            let task = cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(every).await;
                let Ok(()) = this.update(cx, |this, cx| {
                    this.redraw(cx);
                    // Slides end and folds finish between workspace changes.
                    let sliding = this.arrived.len();
                    this.arrived.retain(|_, at| at.elapsed() < SLIDE);
                    if this.arrived.len() != sliding {
                        this.settle(cx);
                    }
                    if let Some(s) = this.shown.as_mut()
                        && s.folding.as_ref().is_some_and(|(_, until)| *until <= Instant::now())
                    {
                        s.folding = None;
                    }
                    this.tick(cx);
                }) else {
                    break;
                };
            });
            (every, task)
        });
    }
}

/// How far a fold ending at `until` has got, 0 to 1.
fn fold_progress(until: Instant) -> f32 {
    let left = until.saturating_duration_since(Instant::now()).as_secs_f32();
    1. - left / activity::FOLD.as_secs_f32()
}

/// `text` with a soft highlight sweeping across it, from `base` to `hi` at its peak: a band a
/// few characters wide that crosses the line every `SWEEP` seconds. Still, it's just `base`.
fn shimmer(text: SharedString, clock: f32, still: bool, base: Hsla, hi: Hsla) -> AnyElement {
    if still {
        return div().text_color(base).child(text).into_any_element();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len() as f32;
    let width = 5.;
    let head = (clock / SWEEP).fract() * (n + width * 2.) - width;
    let mut highlights = vec![];
    for (i, (start, ch)) in chars.iter().enumerate() {
        let d = (i as f32 - head).abs();
        if d < width {
            // Smooth falloff from the band's centre.
            let k = 0.5 + 0.5 * (std::f32::consts::PI * d / width).cos();
            let mix = |a: f32, b: f32| a + (b - a) * k;
            let color = Hsla { h: hi.h, s: mix(base.s, hi.s), l: mix(base.l, hi.l), a: mix(base.a, hi.a) };
            highlights.push((*start..*start + ch.len_utf8(), HighlightStyle { color: Some(color), ..Default::default() }));
        }
    }
    div().text_color(base).child(StyledText::new(text).with_highlights(highlights)).into_any_element()
}

/// "+12 −3" in the diff colours.
pub fn lines_chip(added: u32, removed: u32, cx: &App) -> AnyElement {
    let (a, r) = activity::lines_label(added, removed);
    h_flex()
        .flex_none()
        .gap(px(4.))
        .text_xs()
        .children(a.map(|a| div().text_color(crate::palette::emerald(cx)).child(a)))
        .children(r.map(|r| div().text_color(crate::palette::red(cx)).child(r)))
        .into_any_element()
}

impl WorkingBar {
    /// The group's summary line: kind icon, counts, and what it's about (shimmering while live).
    fn summary_line(g: &Group, clock: f32, live: bool, still: bool, phrase_opacity: f32, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        h_flex()
            .h(px(SUMMARY))
            .gap_2()
            .text_sm()
            .text_color(muted)
            .child(activity::group_icon(g.kind).small())
            .child(div().min_w_0().truncate().text_color(theme.foreground.opacity(0.85)).child(g.summary.clone()))
            .child(
                h_flex()
                    .flex_none()
                    .max_w(relative(0.6))
                    .gap_2()
                    .opacity(phrase_opacity)
                    .child(div().child("·"))
                    .child(div().min_w_0().truncate().child(shimmer(g.phrase.clone().into(), clock, still || !live, muted, theme.foreground))),
            )
            .into_any_element()
    }

    /// How far `r` has slid in, while it does.
    fn slide(&self, r: &LiveRow, still: bool) -> Option<f32> {
        self.arrived.get(&r.id).filter(|_| !still).map(|at| ease_out(at.elapsed().as_secs_f32() / SLIDE.as_secs_f32()))
    }
}

/// The hairline the rows hang from.
fn rows_rail(cx: &App) -> Div {
    v_flex().ml(px(7.)).pl_4().border_l_1().border_color(cx.theme().border)
}

fn earlier_line(n: usize, cx: &App) -> Div {
    div().h(px(EARLIER)).flex().items_center().text_xs().text_color(cx.theme().muted_foreground).child(format!("+{n} earlier"))
}

/// A whole group's rows, drawn as they are (a group folding away).
fn all_rows(g: &Group, cx: &App) -> Div {
    rows_rail(cx).when(g.earlier > 0, |el| el.child(earlier_line(g.earlier, cx))).children(g.rows.iter().map(|r| row(r, None, 0., true, cx)))
}

/// One call's row. `slide`: how far in it has slid (0 to 1), while it does.
fn row(r: &LiveRow, slide: Option<f32>, clock: f32, still: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let text = theme.foreground.opacity(0.85);
    h_flex()
        .id(SharedString::from(format!("live-row-{}", r.id)))
        .test_support()
        .h(px(ROW))
        .gap(px(6.))
        .text_size(px(12.5))
        .when_some(slide, |el, t| el.relative().top(px(5. * (1. - t))).opacity(t))
        .when(!r.op.verb.is_empty(), |el| el.child(div().flex_none().text_color(muted).child(r.op.verb.clone())))
        .when_some(r.op.file.as_deref(), |el, f| el.child(crate::file_icon::badge(f, px(13.), cx)))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family(theme.mono_font_family.clone())
                .when(r.failed, |el| el.line_through())
                .child(shimmer(r.op.text.clone().into(), clock, still || !r.running, if r.running { muted } else { text }, theme.foreground)),
        )
        .when_some(r.activity.clone(), |el, a| el.child(div().flex_none().max_w(relative(0.4)).truncate().text_xs().text_color(muted).child(a)))
        .when_some(r.lines, |el, (a, d)| el.child(lines_chip(a, d, cx)))
        .when(r.failed, |el| el.child(Icon::new(IconName::CircleX).xsmall().text_color(crate::palette::red(cx))))
        .into_any_element()
}


/// The live group's rows that are done and in: "+N earlier", then the rows. Its own view, cached,
/// so they're laid out once rather than on every frame of the bar.
pub struct SettledRows {
    rows: Vec<LiveRow>,
    earlier: usize,
}

impl Render for SettledRows {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("SettledRows");
        column(rows_rail(cx).when(self.earlier > 0, |el| el.child(earlier_line(self.earlier, cx))).children(self.rows.iter().map(|r| row(r, None, 0., true, cx))))
    }
}

/// The part of the bar above the settled rows: a group folding away, and the live group's
/// summary line. A view of its own, as the settled rows below it are (`WorkingBar::redraw`).
pub struct BarTop {
    bar: WeakEntity<WorkingBar>,
}

impl Render for BarTop {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("BarTop");
        let Some(shown) = self.bar.upgrade().and_then(|b| b.read(cx).shown.clone()) else { return div().into_any_element() };
        let clock = shown.header.as_ref().and_then(|h| h.started).map_or(0., |t| t.elapsed().as_secs_f32());
        let still = shown.still;
        let folding = shown.folding.as_ref().map(|(g, until)| {
            let t = ease_out(fold_progress(*until));
            column(
                v_flex()
                    .id("live-fold")
                    .test_support()
                    .pt(px(GROUP_PAD))
                    .child(WorkingBar::summary_line(g, clock, false, still, 1. - t, cx))
                    .child(div().h(px((g.rows_height() + GROUP_PAD) * (1. - t))).overflow_hidden().opacity(1. - t).child(all_rows(g, cx))),
            )
        });
        let group = shown.group.as_ref().map(|g| {
            column(div().id("live-group").test_support().pt(px(GROUP_PAD)).child(WorkingBar::summary_line(g, clock, true, still, 1., cx)))
        });
        v_flex().size_full().overflow_hidden().justify_end().children(folding).children(group).into_any_element()
    }
}

/// The transcript's column, as the bar's parts line up with it.
fn column(el: impl IntoElement) -> Div {
    h_flex().w_full().flex_none().justify_center().px_6().child(div().w_full().max_w(px(COLUMN)).child(el))
}

impl Render for WorkingBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("WorkingBar");
        let Some(shown) = self.shown.clone() else { return div().into_any_element() };
        let theme = cx.theme();
        let muted = theme.muted_foreground.opacity(0.85);
        let elapsed = shown.header.as_ref().and_then(|h| h.started).map(|t| t.elapsed());
        let clock = elapsed.map(|d| d.as_secs_f32()).unwrap_or(0.);
        let still = shown.still;
        // The live rows past the settled ones (running, or sliding in).
        let rows = shown.group.as_ref().map(|g| {
            let settled = self.settled_len.min(g.rows.len());
            column(rows_rail(cx).children(g.rows[settled..].iter().map(|r| row(r, self.slide(r, still), clock, still, cx)))).pb(px(GROUP_PAD))
        });
        let header = shown.header.as_ref().map(|h| {
            h_flex()
                .id("working-bar")
                .test_support()
                .w_full()
                .h(px(HEIGHT))
                .flex_none()
                .justify_center()
                .px_6()
                .pb(px(6.))
                .child(
                    h_flex()
                        .w_full()
                        .max_w(px(COLUMN))
                        .px(px(4.))
                        .gap(px(14.))
                        .items_end()
                        .child(
                            h_flex()
                                .flex_none()
                                // Fixed width so the trail doesn't jump as the clock grows.
                                .w(px(300.))
                                .pb(px(4.))
                                .gap(px(6.))
                                .text_size(px(13.))
                                .child(crate::ui::agent_logo(&h.agent, px(14.), cx))
                                .child(div().flex_none().text_color(theme.foreground.opacity(0.92)).child(h.model.clone()))
                                .child(div().min_w_0().truncate().text_color(muted).child(working_for(elapsed)))
                                .when(h.agents > 0, |el| el.child(div().flex_none().text_color(muted).child(agents_out(h.agents)))),
                        )
                        .child(div().flex_1().min_w_0().child(crate::mascot::trail(clock, still, cx))),
                )
        });
        v_flex()
            .w_full()
            .h(px(self.bottom_height()))
            .overflow_hidden()
            .justify_end()
            .children(rows)
            .children(header)
            .into_any_element()
    }
}

/// "working for 17s", or "working" before the clock starts.
fn working_for(elapsed: Option<Duration>) -> String {
    match elapsed {
        Some(d) => format!("working for {}", crate::time::elapsed(d)),
        None => "working".into(),
    }
}

/// The bar as a window lays it out: its three parts each cached at their current height (zero
/// while hidden), so frames that redraw other views (the sidebar's clock) reuse them, the bar's
/// own frames redraw only the parts that move, and rows that settled aren't laid out again.
pub fn cached(bar: &Entity<WorkingBar>, cx: &App) -> AnyElement {
    let b = bar.read(cx);
    let style = |h: f32| StyleRefinement::default().w_full().flex_none().h(px(h));
    v_flex()
        .w_full()
        .flex_none()
        .child(b.top.clone().cached(style(b.top_height())))
        .child(b.settled.clone().cached(style(b.settled_height())))
        .child(bar.clone().cached(style(b.bottom_height())))
        .into_any_element()
}

/// "· 1 agent out", "· 3 agents out".
pub fn agents_out(n: usize) -> String {
    if n == 1 { "· 1 agent out".into() } else { format!("· {n} agents out") }
}

#[cfg(test)]
impl WorkingBar {
    /// The header as shown: "Mock Swift working for 4s · 2 agents out", or `None` when hidden.
    pub(crate) fn label(&self) -> Option<String> {
        let h = self.shown.as_ref()?.header.as_ref()?;
        let mut out = format!("{} {}", h.model, working_for(h.started.map(|t| t.elapsed())));
        if h.agents > 0 {
            out.push(' ');
            out.push_str(&agents_out(h.agents));
        }
        Some(out)
    }

    /// The live group as shown: its summary line, then a line per row ("Read src/main.rs",
    /// "+12 −3" after an edit's), "+N earlier" first when some scrolled away.
    pub(crate) fn live_group(&self) -> Option<Vec<String>> {
        let g = self.shown.as_ref()?.group.as_ref()?;
        let mut out = vec![format!("{} · {}", g.summary, g.phrase)];
        if g.earlier > 0 {
            out.push(format!("+{} earlier", g.earlier));
        }
        for r in &g.rows {
            let mut line = [r.op.verb.as_str(), r.op.text.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
            if let Some((a, d)) = r.lines {
                line.push_str(&format!(" +{a} −{d}"));
            }
            if r.running {
                line.push_str(" (running)");
            }
            out.push(line);
        }
        Some(out)
    }

    /// The summary line of the group folding away, if one is.
    pub(crate) fn folding(&self) -> Option<String> {
        self.shown.as_ref()?.folding.as_ref().map(|(g, _)| g.summary.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{EARLIER, Group, LiveRow, ROW, ROWS, ToolKind, activity, ease_out, short_model, working_for};
    use std::time::Duration;

    #[test]
    fn heights_add_up() {
        let row = |id: &str| LiveRow { id: id.into(), op: activity::op("Read", "a.rs", None), lines: None, running: false, failed: false, activity: None };
        let mut g = Group { summary: "Read 1 file".into(), kind: ToolKind::Read, phrase: "Exploring the project".into(), rows: vec![row("a")], earlier: 0 };
        assert_eq!(g.rows_height(), ROW);
        g.rows = (0..ROWS).map(|i| row(&i.to_string())).collect();
        g.earlier = 3;
        assert_eq!(g.rows_height(), ROWS as f32 * ROW + EARLIER);
    }

    #[test]
    fn model_ids_lose_their_provider_path() {
        assert_eq!(short_model("openrouter/qwen/qwen3-coder"), "qwen3-coder");
        assert_eq!(short_model("gpt-5.6-luna"), "gpt-5.6-luna");
        assert_eq!(working_for(Some(Duration::from_secs(17))), "working for 17s");
        assert_eq!(working_for(None), "working");
    }

    #[test]
    fn eases_out_and_clamps() {
        assert_eq!(ease_out(0.), 0.);
        assert_eq!(ease_out(1.), 1.);
        assert_eq!(ease_out(2.), 1.);
        assert!(ease_out(0.5) > 0.5, "fast, then slow");
    }
}
