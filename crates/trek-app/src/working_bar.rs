//! The bar above the composer while an agent works. Its header says who's at it (the agent's
//! logo), a trail word that changes every few seconds and how long it's been ("Breaking trail…
//! 17s"), beside the hiker on its trail; above it, the turn's live group of tool calls: a summary
//! line ("Ran 2 commands · Exploring the project") and a row per call,
//! the newest sliding in. When the group ends it folds into its summary, which the transcript
//! then shows as a row of its own. Clicking the group opens it in the transcript instead, where
//! each call's output can be read while the turn goes on. Under a transcript too short to reach
//! it, the bar is drawn right under the transcript's end (`Lift`), as the next row would be.
//!
//! It's a view of its own with its own ticker, so its animation frames re-render only this bar
//! (and the window around it), never the transcript, sidebar or composer. Only a fold moves the
//! transcript (the bar shrinks under it), for its quarter second.

use crate::activity::{self, ToolKind};
use crate::workspace::{Scope, Workspace, WorkspaceEvent};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};
use trek_core::store::{Item, ToolStatus};
use trek_core::{AgentId, RunState};

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

pub struct WorkingBar {
    workspace: Entity<Workspace>,
    /// The thread the bar is for: the main window's (whatever it shows) or a thread window's.
    scope: Scope,
    /// What's on screen, to skip re-rendering on workspace changes that don't touch the bar.
    shown: Option<Shown>,
    /// The window is frontmost (or `TREK_FORCE_ACTIVE`): the animation runs at full rate; behind
    /// another app it runs at half (it's still on show).
    active: bool,
    /// The hiker's own clock: seconds walked, and when it last took a step. It moves only while
    /// the bar animates, so after a pause the hiker carries on from where it stood instead of
    /// jumping to where the turn's clock would put it.
    walk: std::cell::Cell<(f32, Option<Instant>)>,
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
    /// No motion: reduced motion.
    still: bool,
    /// The transcript column's widest (`Workspace::column`).
    width: Pixels,
}

#[derive(Clone, PartialEq)]
struct Header {
    agent: AgentId,
    /// Its clock: when the turn started, or the wait began.
    started: Option<Instant>,
    agents: usize,
    /// It waits on sub-agents rather than working itself (its turn is over, or blocked in a call
    /// for their answers): their logos, and "Waiting on Sol".
    waiting: Option<(Vec<AgentId>, String)>,
}

#[derive(Clone, PartialEq)]
pub(crate) struct Group {
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

/// What a sub-agent is doing, as the bar shows its own agent's live group: the calls of the
/// sub-agent `child`'s latest turn (one Trek runs), its newest ones as rows.
pub(crate) fn child_group(ws: &Workspace, child: &str) -> Option<Group> {
    let live = ws.live.get(child)?;
    let start = live.items.iter().rposition(|i| matches!(i, Item::User { aside: false, .. })).map_or(0, |u| u + 1);
    let tools: Vec<usize> = (start..live.items.len()).filter(|ix| matches!(live.items.get(*ix), Some(i @ Item::Tool { .. }) if activity::placement(i) == activity::Place::Group)).collect();
    group(ws, child, &tools, None)
}

/// The same for one of the agent's own sub-agents, from the calls it reported
/// (`SubTask::steps`, the newest last; `earlier` more before them). The last is under way while
/// it runs.
pub(crate) fn steps_group(steps: &[(String, String)], earlier: usize, running: bool, cwd: Option<&std::path::Path>) -> Option<Group> {
    let (title, detail) = steps.last()?;
    let kinds: Vec<ToolKind> = steps.iter().map(|(t, _)| activity::tool_kind(t)).collect();
    let rows = steps
        .iter()
        .enumerate()
        .skip(steps.len().saturating_sub(ROWS))
        .map(|(n, (t, d))| LiveRow { id: format!("step-{}", earlier + n), op: activity::op(t, d, cwd), lines: None, running: running && n + 1 == steps.len(), failed: false, activity: None })
        .collect();
    Some(Group {
        summary: activity::summarize(&kinds),
        kind: kinds.last().copied().unwrap_or(ToolKind::Other),
        phrase: activity::phrase(title, detail, None),
        earlier: earlier + steps.len().saturating_sub(ROWS),
        rows,
    })
}

impl Group {
    /// Rows shown (the "+N earlier" line counts as one).
    pub(crate) fn lines(&self) -> usize {
        self.rows.len() + usize::from(self.earlier > 0)
    }

    /// The group drawn as a sub-agent's row shows it: its summary line and its rows, the call
    /// under way shimmering (`clock` in seconds; `still`, it doesn't).
    pub(crate) fn element(&self, clock: f32, still: bool, cx: &App) -> AnyElement {
        let live = self.rows.iter().any(|r| r.running);
        v_flex()
            .child(WorkingBar::summary_line(self, clock, None, still || !live, 1., false, cx))
            .child(rows_rail(cx).when(self.earlier > 0, |el| el.child(earlier_line(self.earlier, None, cx))).children(self.rows.iter().map(|r| row(r, None, clock, still, None, cx))))
            .into_any_element()
    }

    #[cfg(test)]
    pub(crate) fn describe(&self) -> Vec<String> {
        let mut out = vec![format!("{} · {}", self.summary, self.phrase)];
        out.extend(self.rows.iter().map(|r| [r.op.verb.as_str(), r.op.text.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ")));
        out
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
            // A failed or denied change changed nothing.
            lines: live.lines.get(call).copied().filter(|_| !matches!(status, ToolStatus::Failed | ToolStatus::Denied)),
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
            walk: std::cell::Cell::new((0., None)),
            arrived: HashMap::new(),
            settled: cx.new(|_| SettledRows { rows: vec![], earlier: 0, width: px(0.), open: None }),
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
        let still = !ws.motion(cx);
        // The header shows while the thread works and isn't waiting on the user (its cards take
        // this spot then), and while it waits on its sub-agents.
        let waited = ws.waiting_on(id);
        let waiting = (!waited.is_empty()).then(|| {
            let mut logos: Vec<AgentId> = vec![];
            for w in &waited {
                if !logos.contains(&w.agent) && logos.len() < 3 {
                    logos.push(w.agent.clone());
                }
            }
            (logos, crate::workspace::waiting_label(&waited))
        });
        // The wait's clock is the longest-waited one's, to the second. Worked out afresh, it'd
        // land a hair off each time and redraw the bar on every workspace change: the one shown
        // stands while it's within a second.
        let shown = self.shown.as_ref().filter(|s| s.thread == id).and_then(|s| s.header.as_ref()).filter(|h| h.waiting.is_some()).and_then(|h| h.started);
        let since = waited.iter().map(|w| w.elapsed).max().map(|d| steady(Instant::now() - Duration::from_secs(d.as_secs()), shown));
        let working = thread.run_state == RunState::Working || ws.waiting(id);
        let header = (working && live.permissions.is_empty()).then(|| Header {
            agent: thread.agent.clone(),
            started: if waiting.is_some() { since } else { live.turn_started },
            agents: live.active_tasks().max(live.background_agents().count()),
            waiting,
        });
        let current = activity::live(ws, id).and_then(|t| group(ws, id, &t.tools, t.headline.as_deref()));
        // The transcript holds a folding group back wherever it's shown, so every bar on it shows
        // the group until then: folding, or as it was when the bar is still.
        let folding = live.fold.as_ref().filter(|f| f.until > Instant::now()).and_then(|f| {
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
        (header.is_some() || folding.is_some()).then(|| Shown { thread: id.to_string(), header, group: current, folding, still, width: ws.column() })
    }

    /// The height of the part above the settled rows: a folding group (shrinking frame by frame)
    /// and the live group's summary line.
    fn top_height(&self) -> f32 {
        let Some(s) = &self.shown else { return 0. };
        let folding = s.folding.as_ref().map_or(0., |(g, until)| {
            let left = 1. - fold_eased(*until, s.still);
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
        let width = self.shown.as_ref().map_or(px(0.), |s| s.width);
        let (rows, earlier) = match self.shown.as_ref().and_then(|s| s.group.as_ref()) {
            Some(g) => {
                let n = g.rows.iter().position(|r| r.running || self.arrived.contains_key(&r.id)).unwrap_or(g.rows.len());
                (g.rows[..n].to_vec(), g.earlier)
            }
            None => (vec![], 0),
        };
        self.settled_len = rows.len();
        let open = self.opener();
        self.settled.update(cx, |s, cx| {
            if s.rows != rows || s.earlier != earlier || s.width != width || s.open.as_ref().map(|o| &o.thread) != open.as_ref().map(|o| &o.thread) {
                s.rows = rows;
                s.earlier = earlier;
                s.width = width;
                s.open = open;
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
        // Waiting is calm: the clock ticks and the hiker looks about, once a second.
        let waiting = s.header.as_ref().is_some_and(|h| h.waiting.is_some());
        Some(if s.still || (waiting && s.folding.is_none()) {
            Duration::from_secs(1)
        } else if s.folding.is_some() {
            // 30 a second: smooth enough for a quarter second of motion, and each one redraws the
            // transcript too (it grows into the space). Rows slide in on the hiker's frames:
            // calls come often, and a fade that short reads the same at either rate.
            Duration::from_millis(33)
        } else if self.active {
            Duration::from_millis(1000 / crate::mascot::FPS)
        } else {
            Duration::from_millis(2000 / crate::mascot::FPS)
        })
    }

    /// Advance the hiker's clock by the time since its last step, at most a few frames' worth: a
    /// bar that stopped drawing (hidden, another thread on screen) picks up where it left off.
    fn step_walk(&self, still: bool) -> f32 {
        let (walked, last) = self.walk.get();
        let now = Instant::now();
        if still {
            self.walk.set((walked, None));
            return walked;
        }
        let dt = last.map_or(0., |l| now.duration_since(l).as_secs_f32().min(0.2));
        self.walk.set((walked + dt, Some(now)));
        walked + dt
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

/// `at`, or `shown` when that's within a second of it.
fn steady(at: Instant, shown: Option<Instant>) -> Instant {
    match shown {
        Some(s) if (if at > s { at - s } else { s - at }) < Duration::from_secs(1) => s,
        _ => at,
    }
}

/// How far a fold ending at `until` has got, 0 to 1.
fn fold_progress(until: Instant) -> f32 {
    let left = until.saturating_duration_since(Instant::now()).as_secs_f32();
    1. - left / activity::FOLD.as_secs_f32()
}

/// The fold's progress, eased; a still bar holds the group as it is until it goes.
fn fold_eased(until: Instant, still: bool) -> f32 {
    if still { 0. } else { ease_out(fold_progress(until)) }
}

/// `text` with a soft highlight sweeping across it, from `base` to `hi` at its peak. Still, it's
/// just `base`.
///
/// The highlight is the same text again in brighter colours, painted only inside a band that
/// moves along the line, rather than a colour per character: GPUI shapes a line again whenever
/// its colour runs change, which on every frame costs more than the rest of the bar together.
fn shimmer(text: SharedString, clock: f32, still: bool, base: Hsla, hi: Hsla) -> AnyElement {
    let plain = div().text_color(base).child(text.clone());
    if still {
        return plain.into_any_element();
    }
    let copies = BAND.iter().map(|(half, k)| (*half, div().text_color(mix(base, hi, *k)).child(text.clone()).into_any_element())).collect();
    Sweep { clock, base: plain.into_any_element(), copies }.into_any_element()
}

/// The shimmer's band: half-widths (px) and how far towards the highlight each step goes, widest
/// and faintest first, so the brighter ones paint over it into a soft peak.
const BAND: [(f32, f32); 3] = [(30., 0.35), (18., 0.7), (8., 1.)];
/// How fast the band crosses a line, in px a second.
const SPEED: f32 = 240.;

/// `a` blended `k` of the way to `b` (in `b`'s hue: the highlight is a brighter `a`).
fn mix(a: Hsla, b: Hsla, k: f32) -> Hsla {
    let m = |x: f32, y: f32| x + (y - x) * k;
    Hsla { h: b.h, s: m(a.s, b.s), l: m(a.l, b.l), a: m(a.a, b.a) }
}

/// Where the band's centre is after `clock` seconds over a line `width` px wide: it comes in
/// past the left edge, crosses, and leaves past the right before coming round again.
fn band_centre(clock: f32, width: f32) -> f32 {
    let edge = BAND[0].0;
    let travel = width.max(0.) + 2. * edge;
    (clock * SPEED) % travel - edge
}

/// The text, then its brighter copies, each painted only within its band (`shimmer`).
struct Sweep {
    clock: f32,
    base: AnyElement,
    copies: Vec<(f32, AnyElement)>,
}

impl IntoElement for Sweep {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Sweep {
    fn band(&self, bounds: Bounds<Pixels>, half: f32) -> Bounds<Pixels> {
        let centre = band_centre(self.clock, bounds.size.width.as_f32());
        Bounds::new(point(bounds.origin.x + px(centre - half), bounds.origin.y), size(px(half * 2.), bounds.size.height))
    }
}

impl Element for Sweep {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        (self.base.request_layout(window, cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) {
        self.base.prepaint(window, cx);
        // Laid out at the line's own size, the copies truncate where it does, and their lines
        // come from the text system's cache: the same text in a single colour.
        let space = size(AvailableSpace::Definite(bounds.size.width), AvailableSpace::Definite(bounds.size.height));
        for (_, copy) in &mut self.copies {
            copy.layout_as_root(space, window, cx);
            copy.prepaint_at(bounds.origin, window, cx);
        }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App) {
        self.base.paint(window, cx);
        let bands: Vec<Bounds<Pixels>> = self.copies.iter().map(|(half, _)| self.band(bounds, *half)).collect();
        for ((_, copy), band) in self.copies.iter_mut().zip(bands) {
            window.with_content_mask(Some(ContentMask { bounds: band }), |window| copy.paint(window, cx));
        }
    }
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

/// Opens the live group in the transcript (`Workspace::open_live_group`): from the bar, the
/// summary line, "+N earlier" and each row do (a row opens its own call there too).
#[derive(Clone)]
struct Opener {
    workspace: Entity<Workspace>,
    thread: String,
}

impl Opener {
    fn on_click(&self, call: Option<&str>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let (workspace, thread, call) = (self.workspace.clone(), self.thread.clone(), call.map(str::to_string));
        move |_, _, cx| workspace.update(cx, |ws, cx| ws.open_live_group(&thread, call.clone(), cx))
    }
}

impl WorkingBar {
    /// The group's summary line: kind icon, counts, and what it's about (shimmering while live).
    /// `open`: it opens the group in the transcript (the live one does; a folding one is going).
    /// `chevron`: it reads as one that opens (not in a sub-agent's row, which opens itself).
    fn summary_line(g: &Group, clock: f32, open: Option<&Opener>, still: bool, phrase_opacity: f32, chevron: bool, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let live = open.is_some();
        h_flex()
            .id(if live { "live-summary" } else { "fold-summary" })
            .test_support()
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
            // As the transcript's summary rows do: it opens (a sub-agent's, shown in its row, doesn't).
            .when(chevron, |el| el.child(Icon::new(IconName::ChevronRight).xsmall().opacity(0.6 * phrase_opacity)))
            .when_some(open, |el, o| el.cursor_pointer().hover(|s| s.text_color(theme.foreground)).on_click(o.on_click(None)))
            .into_any_element()
    }

    /// How far `r` has slid in, while it does.
    fn slide(&self, r: &LiveRow, still: bool) -> Option<f32> {
        self.arrived.get(&r.id).filter(|_| !still).map(|at| ease_out(at.elapsed().as_secs_f32() / SLIDE.as_secs_f32()))
    }

    fn opener(&self) -> Option<Opener> {
        Some(Opener { workspace: self.workspace.clone(), thread: self.shown.as_ref()?.thread.clone() })
    }
}

/// The hairline the rows hang from.
fn rows_rail(cx: &App) -> Div {
    v_flex().ml(px(7.)).pl_3().border_l_1().border_color(cx.theme().border)
}

fn earlier_line(n: usize, open: Option<&Opener>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .id("live-earlier")
        .test_support()
        .h(px(EARLIER))
        .px(px(4.))
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(format!("+{n} earlier"))
        .when_some(open, |el, o| el.cursor_pointer().hover(|s| s.text_color(theme.foreground)).on_click(o.on_click(None)))
        .into_any_element()
}

/// A whole group's rows, drawn as they are (a group folding away).
fn all_rows(g: &Group, cx: &App) -> Div {
    rows_rail(cx).when(g.earlier > 0, |el| el.child(earlier_line(g.earlier, None, cx))).children(g.rows.iter().map(|r| row(r, None, 0., true, None, cx)))
}

/// One call's row. `slide`: how far in it has slid (0 to 1), while it does.
fn row(r: &LiveRow, slide: Option<f32>, clock: f32, still: bool, open: Option<&Opener>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let text = theme.foreground.opacity(0.85);
    h_flex()
        .id(SharedString::from(format!("live-row-{}", r.id)))
        .test_support()
        .h(px(ROW))
        .px(px(4.))
        .gap(px(6.))
        .rounded(px(4.))
        .text_size(px(12.5))
        .when_some(slide, |el, t| el.relative().top(px(5. * (1. - t))).opacity(t))
        .when(!r.op.verb.is_empty(), |el| el.child(div().flex_none().text_color(muted).child(r.op.verb.clone())))
        .map(|el| {
            let label = div()
                .min_w_0()
                .truncate()
                .font_family(theme.mono_font_family.clone())
                .when(r.failed, |el| el.line_through());
            match r.op.file.as_deref() {
                // A file sits in a chip of its type's colour, its name in that colour's ink.
                Some(f) => {
                    let t = crate::file_icon::tint(f, cx);
                    el.child(
                        h_flex()
                            .min_w_0()
                            .h(px(ROW - 4.))
                            .px(px(5.))
                            .gap(px(5.))
                            .rounded(px(5.))
                            .border_1()
                            .border_color(t.edge)
                            .bg(t.fill)
                            .child(crate::file_icon::badge(f, px(13.), cx))
                            .child(label.child(shimmer(r.op.text.clone().into(), clock, still || !r.running, if r.running { t.ink.opacity(0.7) } else { t.ink }, theme.foreground))),
                    )
                }
                None => el.child(label.child(shimmer(r.op.text.clone().into(), clock, still || !r.running, if r.running { muted } else { text }, theme.foreground))),
            }
        })
        .when_some(r.activity.clone(), |el, a| el.child(div().flex_none().max_w(relative(0.4)).truncate().text_xs().text_color(muted).child(a)))
        .when_some(r.lines, |el, (a, d)| el.child(lines_chip(a, d, cx)))
        .when(r.failed, |el| el.child(Icon::new(IconName::CircleX).xsmall().text_color(crate::palette::red(cx))))
        .when_some(open, |el, o| el.cursor_pointer().hover(|s| s.bg(theme.list_hover)).on_click(o.on_click(Some(&r.id))))
        .into_any_element()
}

/// The live group's rows that are done and in: "+N earlier", then the rows. Its own view, cached,
/// so they're laid out once rather than on every frame of the bar.
pub struct SettledRows {
    rows: Vec<LiveRow>,
    earlier: usize,
    width: Pixels,
    open: Option<Opener>,
}

impl Render for SettledRows {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("SettledRows");
        let open = self.open.as_ref();
        column(self.width, rows_rail(cx).when(self.earlier > 0, |el| el.child(earlier_line(self.earlier, open, cx))).children(self.rows.iter().map(|r| row(r, None, 0., true, open, cx))))
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
        let Some((shown, open)) = self.bar.upgrade().and_then(|b| Some((b.read(cx).shown.clone()?, b.read(cx).opener()))) else { return div().into_any_element() };
        let clock = shown.header.as_ref().and_then(|h| h.started).map_or(0., |t| t.elapsed().as_secs_f32());
        let still = shown.still;
        let folding = shown.folding.as_ref().map(|(g, until)| {
            let t = fold_eased(*until, still);
            column(
                shown.width,
                v_flex()
                    .id("live-fold")
                    .test_support()
                    .pt(px(GROUP_PAD))
                    .child(WorkingBar::summary_line(g, clock, None, still, 1. - t, true, cx))
                    .child(div().h(px((g.rows_height() + GROUP_PAD) * (1. - t))).overflow_hidden().opacity(1. - t).child(all_rows(g, cx))),
            )
        });
        let group = shown.group.as_ref().map(|g| {
            column(shown.width, div().id("live-group").test_support().pt(px(GROUP_PAD)).child(WorkingBar::summary_line(g, clock, open.as_ref(), still, 1., true, cx)))
        });
        v_flex().size_full().overflow_hidden().justify_end().children(folding).children(group).into_any_element()
    }
}

/// The transcript's column, `width` at its widest, as the bar's parts line up with it.
fn column(width: Pixels, el: impl IntoElement) -> Div {
    h_flex().w_full().flex_none().justify_center().px_6().child(div().w_full().max_w(width).child(el))
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
            let open = self.opener();
            column(shown.width, rows_rail(cx).children(g.rows[settled..].iter().map(|r| row(r, self.slide(r, still), clock, still, open.as_ref(), cx)))).pb(px(GROUP_PAD))
        });
        let header = shown.header.as_ref().map(|h| {
            let who = match &h.waiting {
                // Their logos, stacked, and who they are; no shimmer: nothing to see here but time.
                Some((logos, label)) => h_flex()
                    .min_w_0()
                    .gap(px(6.))
                    .child(h_flex().flex_none().children(logos.iter().enumerate().map(|(i, a)| {
                        div().when(i > 0, |el| el.ml(px(-4.))).p(px(1.)).rounded(px(4.)).bg(theme.background).child(crate::ui::agent_logo(a, px(13.), cx))
                    })))
                    .child(div().id("waiting-on").test_support().min_w_0().truncate().text_color(theme.foreground.opacity(0.88)).child(label.clone()))
                    .children(elapsed.map(|d| div().flex_none().text_color(muted).child(format!("· {}", crate::time::elapsed(d)))))
                    .into_any_element(),
                None => h_flex()
                    .min_w_0()
                    .gap(px(6.))
                    // The logo says which agent; the composer's pill names the model.
                    .child(crate::ui::agent_logo(&h.agent, px(14.), cx))
                    .child(div().min_w_0().truncate().child(shimmer(trail_word(&shown.thread, elapsed).into(), clock, still, theme.foreground.opacity(0.88), theme.foreground)))
                    .children(elapsed.map(|d| div().flex_none().text_color(muted).child(crate::time::elapsed(d))))
                    .when(h.agents > 0, |el| el.child(div().flex_none().text_color(muted).child(agents_out(h.agents))))
                    .into_any_element(),
            };
            let trail = if h.waiting.is_some() { crate::mascot::waiting(clock, still, cx) } else { crate::mascot::trail(self.step_walk(still), still, cx) };
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
                        .max_w(shown.width)
                        .px(px(4.))
                        .gap(px(14.))
                        .items_end()
                        .child(
                            h_flex()
                                .flex_none()
                                // Fixed width so the trail doesn't jump as the clock grows.
                                .w(px(300.))
                                .pb(px(4.))
                                .text_size(px(13.))
                                .child(who),
                        )
                        .child(div().flex_1().min_w_0().child(trail)),
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

/// "Breaking trail…": what the turn in `thread` is doing after `elapsed` (the first word before
/// its clock starts).
pub fn trail_word(thread: &str, elapsed: Option<Duration>) -> String {
    format!("{}…", crate::mascot::word(thread, elapsed.map_or(0, |d| d.as_secs())))
}

/// The bar as a window lays it out: its three parts each cached at their current height (zero
/// while hidden), so frames that redraw other views (the sidebar's clock) reuse them, the bar's
/// own frames redraw only the parts that move, and rows that settled aren't laid out again.
/// `tail`: where the transcript above ends (`ThreadView::tail`), which the bar follows up.
pub fn cached(bar: &Entity<WorkingBar>, tail: Rc<Cell<Option<Pixels>>>, cx: &App) -> AnyElement {
    let b = bar.read(cx);
    let style = |h: f32| StyleRefinement::default().w_full().flex_none().h(px(h));
    let parts = v_flex()
        .w_full()
        .flex_none()
        .child(b.top.clone().cached(style(b.top_height())))
        .child(b.settled.clone().cached(style(b.settled_height())))
        .child(bar.clone().cached(style(b.bottom_height())));
    Lift { tail, child: parts.into_any_element() }.into_any_element()
}

/// How far the bar is lifted to sit under a transcript ending at `tail` (the bar's own place
/// starts at `top`): up to the transcript's end, never down.
fn lift(top: Pixels, tail: Option<Pixels>) -> Pixels {
    tail.map_or(px(0.), |t| (top - t).max(px(0.)))
}

/// Lifts the bar up to just under the transcript when the transcript doesn't reach down to it,
/// so the live group and the header follow the conversation rather than wait at the bottom of
/// an empty window (and a folded group lands right where its summary row then appears). Only its
/// drawing moves: the space it leaves stays put, so nothing lays out again. The transcript is
/// laid out earlier in the same frame and notes where it ends.
struct Lift {
    tail: Rc<Cell<Option<Pixels>>>,
    child: AnyElement,
}

impl IntoElement for Lift {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Lift {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) {
        let up = lift(bounds.top(), self.tail.get());
        window.with_element_offset(point(px(0.), -up), |window| self.child.prepaint(window, cx));
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App) {
        self.child.paint(window, cx);
    }
}

/// "· 1 agent out", "· 3 agents out".
pub fn agents_out(n: usize) -> String {
    if n == 1 { "· 1 agent out".into() } else { format!("· {n} agents out") }
}

#[cfg(test)]
impl WorkingBar {
    /// The header as shown: "Breaking trail… 4s · 2 agents out", "Waiting on Sol · 2s", or
    /// `None` when hidden.
    pub(crate) fn label(&self) -> Option<String> {
        let s = self.shown.as_ref()?;
        let h = s.header.as_ref()?;
        let elapsed = h.started.map(|t| t.elapsed());
        if let Some((_, label)) = &h.waiting {
            return Some(match elapsed {
                Some(d) => format!("{label} · {}", crate::time::elapsed(d)),
                None => label.clone(),
            });
        }
        let mut out = trail_word(&s.thread, elapsed);
        if let Some(d) = elapsed {
            out.push(' ');
            out.push_str(&crate::time::elapsed(d));
        }
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

    /// How often the bar redraws now (`None`: it doesn't).
    pub(crate) fn frame_interval(&self) -> Option<Duration> {
        self.rate()
    }

    /// Rows sliding in now.
    pub(crate) fn sliding(&self) -> usize {
        self.arrived.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{BAND, EARLIER, Group, LiveRow, ROW, ROWS, SPEED, ToolKind, activity, band_centre, ease_out, lift, mix, steady, trail_word};
    use gpui_kit::{Hsla, px};
    use std::time::{Duration, Instant};

    #[test]
    fn the_wait_s_clock_holds_still_between_reads() {
        let t = Instant::now();
        assert_eq!(steady(t + Duration::from_millis(400), Some(t)), t, "a hair off: the one shown stands");
        assert_eq!(steady(t, Some(t + Duration::from_millis(900))), t + Duration::from_millis(900));
        assert_eq!(steady(t + Duration::from_secs(3), Some(t)), t + Duration::from_secs(3), "another wait");
        assert_eq!(steady(t, None), t);
    }

    #[test]
    fn the_shimmer_band_crosses_the_line_and_comes_round() {
        let edge = BAND[0].0;
        // It starts just off the left edge, wholly outside the line…
        assert_eq!(band_centre(0., 300.), -edge);
        // …crosses at its speed…
        assert_eq!(band_centre(1., 300.), SPEED - edge);
        // …leaves past the right edge, then starts over.
        let lap = (300. + 2. * edge) / SPEED;
        assert!((band_centre(lap - 0.001, 300.) - (300. + edge)).abs() < 1.);
        assert!((band_centre(lap + 0.5, 300.) - band_centre(0.5, 300.)).abs() < 0.01);
        // Narrow and empty lines too.
        assert!(band_centre(0.1, 0.).is_finite());
        // Its steps grow brighter towards the middle and narrower.
        assert!(BAND.windows(2).all(|w| w[0].0 > w[1].0 && w[0].1 < w[1].1));
        assert_eq!(BAND[BAND.len() - 1].1, 1., "the peak is the highlight itself");
    }

    #[test]
    fn mixing_runs_from_one_colour_to_the_other() {
        let a = Hsla { h: 0.1, s: 0.2, l: 0.4, a: 0.5 };
        let b = Hsla { h: 0.6, s: 0.4, l: 0.9, a: 1. };
        assert_eq!(mix(a, b, 1.), b);
        assert_eq!(mix(a, b, 0.).l, a.l);
        assert!((mix(a, b, 0.5).l - 0.65).abs() < 1e-6);
    }

    #[test]
    fn the_bar_lifts_to_a_short_transcript_only() {
        assert_eq!(lift(px(700.), Some(px(300.))), px(400.), "up to where the transcript ends");
        assert_eq!(lift(px(700.), Some(px(700.))), px(0.));
        assert_eq!(lift(px(700.), Some(px(760.))), px(0.), "a long one: never down over the composer");
        assert_eq!(lift(px(700.), None), px(0.), "its end out of view");
    }

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
    fn the_header_says_a_trail_word_that_moves_on() {
        let word = trail_word("t1", Some(Duration::from_secs(17)));
        assert!(word.ends_with('…') && crate::mascot::WORDS.contains(&word.trim_end_matches('…')), "{word}");
        // It holds within a step and the clock not having started reads as its first second.
        assert_eq!(trail_word("t1", Some(Duration::from_millis(16_100))), word);
        assert_eq!(trail_word("t1", None), trail_word("t1", Some(Duration::ZERO)));
        assert!(!word.contains("working"), "no generic \"working\"");
    }

    #[test]
    fn eases_out_and_clamps() {
        assert_eq!(ease_out(0.), 0.);
        assert_eq!(ease_out(1.), 1.);
        assert_eq!(ease_out(2.), 1.);
        assert!(ease_out(0.5) > 0.5, "fast, then slow");
    }
}
