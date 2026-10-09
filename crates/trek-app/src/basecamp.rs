//! Basecamp: what needs the user, then the range's work in numbers: totals, turns over time, and
//! tables by project and by model.
//!
//! The numbers are worked out off the main thread, every range in one pass (`summaries`, from
//! `trek_core::basecamp`'s cache, which reads only the threads that moved on since the last
//! pass), when Basecamp opens, soon after launch, and whenever threads change while it's open;
//! switching range only draws another range's. Frames only draw them. The phone's
//! Basecamp reads the same recap through the helpers at the end of this file.

use crate::palette;
use crate::ui;
use crate::workspace::{Route, Workspace, fmt_tokens};
use chrono::{DateTime, Datelike as _, Duration as Days, NaiveDate, TimeZone, Timelike as _};
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use trek_core::basecamp::{self, Range, Recap, ThreadActivity};
use trek_core::pricing::Spend;
use trek_core::store::{Activity, Thread, UsageRow, now_ms};
use trek_core::{AgentId, RunState, TokenUsage, UsageCost};

actions!(basecamp, [Leave]);

/// Rows of "Needs review" shown before "Show more".
const REVIEW_ROWS: usize = 8;
/// The activity chart's bars, at their tallest.
const CHART_HEIGHT: f32 = 96.;
/// A bar's widest: a week's seven stay bars, not blocks.
const MAX_BAR: f32 = 28.;
/// The tables' number columns.
const NUMBER_WIDTH: f32 = 84.;

/// What the summaries were worked out from: another key means they're out of date.
#[derive(Debug, Clone, PartialEq)]
struct Key {
    /// Today's start: a new day moves every range (and a new week the week).
    day: i64,
    turns: u64,
    threads_gen: u64,
    threads: usize,
    latest: i64,
}

/// How long after the window opens Basecamp reads its numbers on its own, so they're there
/// when it's first opened.
const WARM_UP: Duration = Duration::from_secs(10);

pub struct Basecamp {
    workspace: Entity<Workspace>,
    focus: FocusHandle,
    range: Range,
    /// Each range's numbers as last worked out, by `Range as usize`: switching range shows
    /// them at once, and reopening shows them while they're brought up to date.
    summaries: [Option<Arc<Summary>>; 3],
    key: Option<Key>,
    computing: bool,
    /// Passes over the store run so far (switching range runs none).
    pub(crate) passes: usize,
    /// On screen (the main window's route).
    open: bool,
    /// The chart's bar under the pointer.
    hovered: Option<usize>,
    /// "Needs review" lists every thread, not just the first `REVIEW_ROWS`.
    review_expanded: bool,
    _compute: Option<Task<()>>,
    /// While open: a tick a minute (relative times move on, the day may turn over).
    _clock: Option<Task<()>>,
    _warm_up: Option<Task<()>>,
    _subscription: Subscription,
}

impl Basecamp {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let _subscription = cx.observe(&workspace, |this, _, cx| this.sync(cx));
        // Read once in the background soon after launch, so the first open has its numbers.
        let _warm_up = (!cfg!(test)).then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(WARM_UP).await;
                let _ = this.update(cx, |this, cx| this.refresh(cx));
            })
        });
        let mut this = Self {
            workspace,
            focus: cx.focus_handle(),
            range: Range::Today,
            summaries: Default::default(),
            key: None,
            computing: false,
            passes: 0,
            open: false,
            hovered: None,
            review_expanded: false,
            _compute: None,
            _clock: None,
            _warm_up,
            _subscription,
        };
        this.sync(cx);
        this
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// The range's numbers on screen, as last worked out (perhaps while newer ones are).
    fn shown(&self) -> Option<&Arc<Summary>> {
        self.summaries[self.range as usize].as_ref()
    }

    /// Everything on screen, once it's up to date.
    #[cfg(test)]
    pub fn summary(&self) -> Option<&Summary> {
        self.shown().map(|s| &**s).filter(|_| !self.computing)
    }

    /// Everything on screen now, up to date or not.
    #[cfg(test)]
    pub fn shown_summary(&self) -> Option<&Summary> {
        self.shown().map(|s| &**s)
    }

    /// The chart's bar under the pointer.
    #[cfg(test)]
    pub fn hovered_bar(&self) -> Option<usize> {
        self.hovered
    }

    pub fn range(&self) -> Range {
        self.range
    }

    /// Follow the workspace: open or close with the route, and bring the numbers up to date
    /// when threads moved.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let open = self.workspace.read(cx).route == Route::Basecamp;
        if open != self.open {
            self.open = open;
            self.hovered = None;
            self._clock = open.then(|| {
                cx.spawn(async move |this, cx| loop {
                    cx.background_executor().timer(Duration::from_secs(60)).await;
                    if this.update(cx, |this, cx| this.tick(now_ms(), cx)).is_err() {
                        break;
                    }
                })
            });
            cx.notify();
        }
        if open {
            self.refresh(cx);
            // The needs lists read the workspace as it is now.
            cx.notify();
        }
    }

    /// Show `range`: what was worked out for it is drawn at once (every range is worked out in
    /// the same pass), nothing is read again.
    pub fn set_range(&mut self, range: Range, cx: &mut Context<Self>) {
        if range != self.range {
            self.range = range;
            self.hovered = None;
            self.refresh(cx);
            cx.notify();
        }
    }

    /// The clock moved on to `now`: summaries still current only move their "now", ones that
    /// aren't (a new day) are worked out again. Either way they're drawn again, relative times
    /// with them.
    pub fn tick(&mut self, now: i64, cx: &mut Context<Self>) {
        self.refresh(cx);
        if !self.computing {
            for summary in self.summaries.iter_mut().flatten() {
                Arc::make_mut(summary).recap.now = now;
            }
        }
        cx.notify();
    }

    /// Bring every range's numbers up to date if what they were worked out from changed. One
    /// pass at a time, off the main thread: it reads only the threads that moved on since the
    /// last (`basecamp::Cache`), then works out all three ranges. The very first, with nothing
    /// worked out yet, shows today or the week (read on their own, quickly) before all time.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let key = Key {
            day: Range::Today.window(&chrono::Local::now()).start,
            turns: ws.turns_finished,
            threads_gen: ws.threads_gen,
            threads: ws.threads.len(),
            latest: ws.threads.iter().map(|t| t.updated_at).max().unwrap_or(0),
        };
        if self.computing || self.key.as_ref() == Some(&key) {
            return;
        }
        self.computing = true;
        self.passes += 1;
        let (store, cache) = (ws.store.clone(), ws.basecamp_cache.clone());
        let first = (self.range != Range::All && self.summaries.iter().all(Option::is_none) && !cache.warm()).then_some(self.range);
        let (tx, rx) = async_channel::unbounded::<(bool, Vec<(Range, Summary)>)>();
        cx.background_executor()
            .spawn(async move {
                let now = chrono::Local::now();
                if let Some(range) = first {
                    let window = range.window(&now);
                    match basecamp::Gathered::gather_since(&store, window.start, None) {
                        Ok(g) => _ = tx.send_blocking((false, vec![(range, Summary::compute(range, &now, window, &g.threads))])),
                        Err(e) => tracing::warn!("basecamp: {e:#}"),
                    }
                }
                let all = cache.with(&store, |g| summaries(g, &now)).unwrap_or_else(|e| {
                    tracing::warn!("basecamp: {e:#}");
                    RANGES.map(|r| (r, Summary::compute(r, &now, r.window(&now), &[]))).to_vec()
                });
                let _ = tx.send_blocking((true, all));
            })
            .detach();
        self._compute = Some(cx.spawn(async move |this, cx| {
            while let Ok((done, found)) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    for (range, summary) in found {
                        this.summaries[range as usize] = Some(Arc::new(summary));
                    }
                    if done {
                        this.computing = false;
                        this.key = Some(key.clone());
                        // What moved while it ran is read now.
                        this.refresh(cx);
                    }
                    cx.notify();
                });
            }
        }));
    }

    fn header(&self, review_unread: bool, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        h_flex()
            .w_full()
            .gap(px(12.))
            .flex_wrap()
            .child(div().flex_none().text_size(px(22.)).font_semibold().line_height(px(30.)).child("Basecamp"))
            .child(div().flex_1())
            .child(
                h_flex()
                    .min_w_0()
                    .flex_wrap()
                    .justify_end()
                    .gap(px(12.))
                    .child(ui::segmented(
                        "basecamp-range",
                        [Range::Today, Range::Week, Range::All].map(|r| (r, r.label())).to_vec(),
                        self.range,
                        move |r, _, cx| {
                            let _ = this.update(cx, |this, cx| this.set_range(r, cx));
                        },
                        cx,
                    ))
                    .child(
                        Button::new("basecamp-mark-read")
                            .outline()
                            .small()
                            .label("Mark all read")
                            .disabled(!review_unread)
                            .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.mark_all_read(cx)))),
                    ),
            )
            .into_any_element()
    }

    /// "Needs you" (questions, approvals, plans, failures), then "Needs review" (finished
    /// threads not looked at yet): the workspace's `ready_for_review`, split where it ranks them.
    fn needs(&self, threads: &[Thread], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (needs, review): (Vec<(&Thread, Option<Waiting>)>, Vec<(&Thread, Option<Waiting>)>) = {
            let ws = self.workspace.read(cx);
            let sub_agents = ws.waiting_on_sub_agents();
            threads
                .iter()
                .map(|t| {
                    let waiting = match t.run_state {
                        RunState::NeedsYou => Some(Waiting::of(ws.pending_request(&t.id))),
                        _ if sub_agents.contains(&t.id) => Some(Waiting::SubAgent),
                        _ => None,
                    };
                    (t, waiting)
                })
                .partition(|(t, waiting)| t.needs_you() || waiting.is_some())
        };
        let projects: HashMap<String, String> = {
            let ws = self.workspace.read(cx);
            threads.iter().filter_map(|t| t.project_id.as_ref()).filter_map(|p| ws.project(p)).map(|p| (p.id.clone(), p.name.clone())).collect()
        };
        let now = self.workspace.read(cx).now();
        let project = |t: &Thread| t.project_id.as_ref().and_then(|p| projects.get(p)).cloned();
        let shown = if self.review_expanded { review.len() } else { review.len().min(REVIEW_ROWS) };
        let more = review.len() - shown;
        let needs_rows: Vec<AnyElement> = needs.iter().map(|(t, w)| needs_row(t, project(t), *w, now, cx)).collect();
        let review_rows: Vec<AnyElement> = review.iter().take(shown).map(|(t, w)| needs_row(t, project(t), *w, now, cx)).collect();
        v_flex()
            .id("basecamp-needs")
            .test_support()
            .gap(px(20.))
            .child(
                v_flex()
                    .child(section_title("Needs you", Some(needs.len()), cx))
                    .when(needs.is_empty(), |el| el.child(div().py(px(6.)).text_size(px(13.)).text_color(theme.muted_foreground).child("Nothing is waiting on you.")))
                    .when(!needs.is_empty(), |el| el.child(list(needs_rows, cx))),
            )
            .when(!review.is_empty(), |el| {
                el.child(
                    v_flex()
                        .child(section_title("Needs review", Some(review.len()), cx))
                        .child(list(review_rows, cx))
                        .when(more > 0 || (self.review_expanded && review.len() > REVIEW_ROWS), |el| {
                            let label = if more > 0 { format!("Show {more} more") } else { "Show less".to_string() };
                            el.child(
                                div()
                                    .id("basecamp-review-more")
                                    .test_support()
                                    .pt(px(6.))
                                    .px(px(8.))
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(theme.foreground))
                                    .child(label)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.review_expanded = !this.review_expanded;
                                        cx.notify();
                                    })),
                            )
                        }),
                )
            })
            .into_any_element()
    }

    /// The range's totals in a row of figures.
    fn totals(&self, recap: &Recap, spend: &Spend, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let line = theme.foreground.opacity(0.07);
        let figures = [
            ("Threads", recap.threads.to_string(), None),
            ("Turns", recap.turns.to_string(), None),
            ("Tokens", fmt_tokens(recap.tokens.total()), None),
            ("Cost", cost(spend), None),
            ("Failures", recap.failed.to_string(), (recap.failed > 0).then(|| palette::red(cx))),
            ("Agent time", agent_time(recap.agent_secs), None),
        ];
        h_flex()
            .id("basecamp-totals")
            .test_support()
            .w_full()
            .items_stretch()
            .border_t_1()
            .border_b_1()
            .border_color(line)
            .children(figures.into_iter().enumerate().map(|(i, (label, value, color))| {
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .py(px(12.))
                    .px(px(14.))
                    .gap(px(4.))
                    .when(i > 0, |el| el.border_l_1().border_color(line))
                    .child(div().text_size(px(12.)).text_color(theme.muted_foreground).truncate().child(label))
                    .child(div().text_size(px(17.)).font_medium().truncate().when_some(color, |el, c| el.text_color(c)).child(value))
            }))
            .into_any_element()
    }

    /// How far the chart's entrance has played (1 at once without motion). It plays when the
    /// chart is first drawn for a range, so again each time Basecamp opens: the moment it started
    /// is kept with the chart's drawing and goes when that does.
    fn entrance(&self, window: &mut Window, cx: &mut Context<Self>) -> f32 {
        if !self.workspace.read(cx).motion(cx) {
            return 1.;
        }
        let now = crate::motion::now(cx);
        let since = window.use_keyed_state(SharedString::from(format!("basecamp-chart-enter-{:?}", self.range)), cx, |_, _| now);
        let t = now.saturating_duration_since(*since.read(cx)).as_secs_f32() / crate::visualization::ENTER.as_secs_f32();
        if t < 1. {
            window.request_animation_frame();
        }
        t.min(1.)
    }

    /// Turns over the range, a bar a stretch, with the stretch under the pointer spelled out. The
    /// bars grow in one after another, as a visualization's do.
    fn chart(&self, chart: &Chart, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let enter = self.entrance(window, cx);
        let theme = cx.theme().clone();
        let max = chart.bars.iter().map(|b| b.turns).max().unwrap_or(0);
        let total: usize = chart.bars.iter().map(|b| b.turns).sum();
        let line = match self.hovered.and_then(|i| chart.bars.get(i)) {
            Some(b) => {
                let mut parts = vec![b.label.clone(), basecamp::count(b.turns, "turn", "turns")];
                if b.tokens > 0 {
                    parts.push(format!("{} tokens", fmt_tokens(b.tokens)));
                }
                parts.join(" · ")
            }
            None => format!("Turns per {}", chart.step.noun()),
        };
        let n = chart.bars.len();
        let gap = if n > 40 { 1. } else { 3. };
        let bar_color = theme.foreground.opacity(0.42);
        let bars = chart.bars.iter().enumerate().map(|(i, b)| {
            let hovered = self.hovered == Some(i);
            let grown = crate::visualization::arrive(enter, i, n);
            let frac = if max > 0 { b.turns as f32 / max as f32 * grown } else { 0. };
            div()
                .id(("basecamp-bar", i))
                .test_support()
                .flex_1()
                .min_w(px(1.))
                .h_full()
                .flex()
                .flex_col()
                .justify_end()
                .items_center()
                .when(hovered, |el| el.bg(theme.foreground.opacity(0.04)))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    let next = if *hovered { Some(i) } else { this.hovered.filter(|h| *h != i) };
                    if next != this.hovered {
                        this.hovered = next;
                        cx.notify();
                    }
                }))
                .when(b.turns > 0, |el| {
                    el.child(div().id(("basecamp-bar-fill", i)).test_support().w_full().max_w(px(MAX_BAR)).h(relative(frac)).min_h(px(2. * grown)).rounded(px(1.5)).bg(if hovered { theme.foreground.opacity(0.8) } else { bar_color }))
                })
        }).collect::<Vec<_>>();
        let mut ticks = chart.ticks.iter().peekable();
        let labels = (0..n).map(|i| {
            let label = ticks.next_if(|(at, _)| *at == i).map(|(_, l)| l.clone());
            // Wider than its bar, a label hangs over its neighbours, centred on its own.
            div().flex_1().min_w(px(1.)).flex().justify_center().when_some(label, |el, l| el.child(div().flex_none().whitespace_nowrap().child(l)))
        }).collect::<Vec<_>>();
        v_flex()
            .gap(px(8.))
            .child(
                h_flex()
                    .text_size(px(12.5))
                    .child(div().flex_1().min_w_0().truncate().text_color(theme.muted_foreground).child(line))
                    .child(div().text_color(theme.muted_foreground).child(basecamp::count(total, "turn", "turns"))),
            )
            .child(
                h_flex()
                    .id("basecamp-chart")
                    .test_support()
                    .w_full()
                    .h(px(CHART_HEIGHT))
                    .items_end()
                    .gap(px(gap))
                    .border_b_1()
                    .border_color(theme.foreground.opacity(0.12))
                    .children(bars),
            )
            .child(h_flex().w_full().gap(px(gap)).text_size(px(11.)).text_color(theme.muted_foreground).children(labels))
            .into_any_element()
    }

    /// A table of `rows` under `title`: the name, then threads, turns, tokens, cost and failures.
    fn table(&self, id: &'static str, title: &'static str, name: &'static str, rows: &[Row], cx: &App) -> AnyElement {
        let theme = cx.theme();
        let line = theme.foreground.opacity(0.07);
        let ws = self.workspace.read(cx);
        let number = |text: String| div().w(px(NUMBER_WIDTH)).flex_none().text_right().truncate().child(text);
        let header = h_flex()
            .h(px(28.))
            .px(px(8.))
            .text_size(px(12.))
            .text_color(theme.muted_foreground)
            .border_b_1()
            .border_color(line)
            .child(div().flex_1().min_w_0().child(name))
            .children(["Threads", "Turns", "Tokens", "Cost", "Failures"].map(|h| number(h.to_string())));
        let body = rows.iter().enumerate().map(|(i, r)| {
            let badge = match (&r.agent, &r.project) {
                (Some(agent), _) => Some(ui::agent_logo(agent, px(14.), cx)),
                (None, Some(id)) => {
                    let look = ws.project(id).map(|p| ws.project_look(&p.path)).unwrap_or_default();
                    Some(ui::project_badge(&r.label, &look, cx))
                }
                (None, None) => None,
            };
            h_flex()
                .id((id, i))
                .test_support()
                .h(px(32.))
                .px(px(8.))
                .text_size(px(13.))
                .border_b_1()
                .border_color(line)
                .child(h_flex().flex_1().min_w_0().gap(px(8.)).children(badge).child(div().min_w_0().truncate().child(r.label.clone())))
                .child(number(r.threads.to_string()))
                .child(number(r.turns.to_string()))
                .child(number(fmt_tokens(r.tokens)))
                .child(number(cost(&r.spend)))
                .child(number(r.failed.to_string()).when(r.failed > 0, |el| el.text_color(palette::red(cx))))
        });
        v_flex().child(section_title(title, None, cx)).child(v_flex().child(header).children(body)).into_any_element()
    }

    /// Nothing done in this range yet.
    fn empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let text = match self.range {
            Range::Today => "No activity today.",
            Range::Week => "No activity this week.",
            Range::All => "No activity yet.",
        };
        h_flex()
            .id("basecamp-empty")
            .test_support()
            .gap(px(16.))
            .py(px(14.))
            .border_t_1()
            .border_b_1()
            .border_color(theme.foreground.opacity(0.07))
            .child(div().flex_1().text_size(px(13.)).text_color(theme.muted_foreground).child(text))
            .child(
                Button::new("basecamp-new-thread")
                    .outline()
                    .small()
                    .icon(crate::assets::Lucide::SquarePen)
                    .label("New thread")
                    .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.new_thread(cx)))),
            )
            .into_any_element()
    }
}

impl Render for Basecamp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("Basecamp");
        let theme = cx.theme().clone();
        let review: Vec<Thread> = self.workspace.read(cx).ready_for_review().into_iter().cloned().collect();
        let unread = review.iter().any(Thread::is_unseen);
        let summary = self.shown().cloned();
        let activity: Vec<AnyElement> = match summary.as_deref() {
            Some(s) if !s.recap.is_empty() => vec![
                self.totals(&s.recap, &s.spend, cx),
                self.chart(&s.chart, window, cx),
                self.table("basecamp-project", "Projects", "Project", &s.projects, cx),
                self.table("basecamp-model", "Models", "Model", &s.models, cx),
            ],
            Some(_) => vec![self.empty(cx)],
            None => vec![div().text_size(px(13.)).text_color(theme.muted_foreground).child("Loading…").into_any_element()],
        };
        div()
            .id("basecamp")
            .test_support()
            .key_context("Basecamp")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Leave, _, cx| this.workspace.update(cx, |ws, cx| ws.leave_basecamp(cx))))
            .size_full()
            .overflow_y_scroll()
            .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(960.))
                    .mx_auto()
                    .px(px(40.))
                    .pt(px(28.))
                    .pb(px(48.))
                    .gap(px(28.))
                    .child(self.header(unread, cx))
                    .child(self.needs(&review, cx))
                    .children(activity),
            )
    }
}

/// A section's title, with a count beside it.
fn section_title(title: &'static str, count: Option<usize>, cx: &App) -> AnyElement {
    h_flex()
        .pb(px(6.))
        .gap(px(6.))
        .text_size(px(13.))
        .child(div().font_medium().child(title))
        .when_some(count.filter(|n| *n > 0), |el, n| el.child(div().text_color(cx.theme().muted_foreground).child(n.to_string())))
        .into_any_element()
}

/// Rows between hairlines.
fn list(rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let line = cx.theme().foreground.opacity(0.07);
    v_flex().border_t_1().border_color(line).children(rows.into_iter().map(move |r| div().border_b_1().border_color(line).child(r))).into_any_element()
}

/// A row of "Needs you" or "Needs review", on one line: status icon, title, project, diff stat,
/// then what it waits for (or how long ago it finished). Clicking it opens the thread.
fn needs_row(t: &Thread, project: Option<String>, waiting: Option<Waiting>, now: i64, cx: &mut Context<Basecamp>) -> AnyElement {
    let theme = cx.theme().clone();
    // `key` names the status for tests (`review-paused-<id>`).
    let (key, icon, color, status): (&str, Icon, Hsla, Option<SharedString>) = match (t.run_state, waiting) {
        (RunState::NeedsYou, w) | (_, w @ Some(Waiting::SubAgent)) => {
            let w = w.unwrap_or(Waiting::Unknown);
            ("needs", w.icon(), palette::amber(cx), Some(w.label().into()))
        }
        (RunState::Failed, _) => ("failed", Icon::new(IconName::CircleX), palette::red(cx), Some("Failed".into())),
        // Stopped at a usage limit, to go on at its reset: not done, and no failure either.
        (RunState::Idle, _) if t.paused.is_some() => {
            let until = match t.paused.as_ref().and_then(|p| p.resets_at) {
                Some(at) => format!("Paused until {}", crate::time::reset_clock(at, now)),
                None => "Paused at its limit".to_string(),
            };
            ("paused", Icon::new(crate::assets::Lucide::Clock), palette::amber(cx), Some(until.into()))
        }
        _ => ("done", Icon::new(IconName::CircleCheck), palette::emerald(cx), None),
    };
    let id = t.id.clone();
    h_flex()
        .id(SharedString::from(format!("review-{}", t.id)))
        .test_support()
        .h(px(34.))
        .gap(px(10.))
        .px(px(8.))
        .text_size(px(13.))
        .cursor_pointer()
        .hover(|s| s.bg(theme.list_hover))
        .child(icon.size(px(14.)).text_color(color))
        .child(div().flex_1().min_w_0().truncate().child(t.title.clone()))
        .when_some(project, |el, p| el.child(div().max_w(px(180.)).flex_none().truncate().text_size(px(12.)).text_color(theme.muted_foreground).child(p)))
        .when(t.additions > 0 || t.deletions > 0, |el| {
            el.child(
                h_flex()
                    .flex_none()
                    .gap(px(4.))
                    .text_size(px(12.))
                    .child(div().text_color(palette::emerald(cx)).child(format!("+{}", t.additions)))
                    .child(div().text_color(palette::red(cx)).child(format!("−{}", t.deletions))),
            )
        })
        .child(
            div().id(SharedString::from(format!("review-{key}-{}", t.id))).test_support().flex_none().min_w(px(36.)).text_right().text_size(px(12.)).map(|el| match status {
                Some(s) => el.font_medium().text_color(color).child(s),
                None => el.text_color(theme.muted_foreground).child(crate::time::relative(t.updated_at)),
            }),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            let id = id.clone();
            this.workspace.update(cx, |ws, cx| ws.navigate(Route::Thread(id), cx))
        }))
        .into_any_element()
}

/// "$1.28", or "—" when none of it has a price.
fn cost(spend: &Spend) -> String {
    if spend.priced() { crate::cost::usd(spend.usd()) } else { "—".to_string() }
}

/// "<1m", "42m", "1h 2m".
fn agent_time(secs: u64) -> String {
    if secs < 60 { "<1m".to_string() } else { basecamp::duration(secs) }
}

/// What Basecamp shows for a range, computed once per change (never per frame).
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// The range's totals.
    pub recap: Recap,
    /// What the range cost: the tables' cost, all of it (reports that name no model priced as
    /// their thread's, which the recap's own total leaves unpriced).
    pub spend: Spend,
    /// Most tokens first.
    pub projects: Vec<Row>,
    /// Most tokens first.
    pub models: Vec<Row>,
    pub chart: Chart,
}

/// A project's or a model's part in a range.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    /// A model's agent, for its logo.
    pub agent: Option<AgentId>,
    /// A project's id (neither: threads in no project).
    pub project: Option<String>,
    /// The user's threads that worked in the range (sub-agents' work counts, not as threads).
    pub threads: usize,
    pub turns: usize,
    pub tokens: u64,
    pub spend: Spend,
    pub failed: usize,
}

/// The stretch each bar of the chart covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Hour,
    Day,
    /// This many weeks from a Monday.
    Weeks(u32),
}

impl Step {
    /// "hour", "day", "week", "4 weeks".
    fn noun(self) -> String {
        match self {
            Step::Hour => "hour".into(),
            Step::Day => "day".into(),
            Step::Weeks(1) => "week".into(),
            Step::Weeks(n) => format!("{n} weeks"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    /// The stretch it covers: "2–3 PM", "Tue, Oct 7", "Week of Oct 6".
    pub label: String,
    pub turns: usize,
    pub tokens: u64,
}

/// Turns and tokens over the range: hours for today, days for the week, days (weeks past three
/// months) for all time, which spans at least a week.
#[derive(Debug, Clone, PartialEq)]
pub struct Chart {
    pub step: Step,
    pub bars: Vec<Bar>,
    /// Axis labels: the bar each sits under, and its text.
    pub ticks: Vec<(usize, String)>,
}

/// All time is drawn a day a bar up to this many days, then a week (or several) a bar.
const CHART_DAYS: i64 = 91;
/// All time in at most this many bars of weeks.
const CHART_WEEKS: i64 = 104;

impl Summary {
    pub fn compute<Tz: TimeZone>(range: Range, now: &DateTime<Tz>, window: basecamp::Window, threads: &[ThreadActivity]) -> Summary {
        let recap = Recap::compute(window, now.timestamp_millis(), threads);
        let (projects, models) = tables(&window, threads);
        let chart = chart(range, now, recap.first, threads);
        let mut spend = Spend::default();
        for p in &projects {
            spend.merge(&p.spend);
        }
        Summary { recap, spend, projects, models, chart }
    }
}

/// The ranges, in the order they're offered.
const RANGES: [Range; 3] = [Range::Today, Range::Week, Range::All];

/// Basecamp's numbers for every range at `now`, from what was gathered: nothing is read.
pub fn summaries<Tz: TimeZone>(g: &basecamp::Gathered, now: &DateTime<Tz>) -> Vec<(Range, Summary)> {
    RANGES
        .map(|range| {
            let window = g.window(range, now);
            (range, Summary::compute(range, now, window, g.threads_in(&window)))
        })
        .to_vec()
}

/// The tables by project and by model. A thread's turns and failures go to its model (the one it
/// was set to, else the one its reports name most); its tokens and their cost to the model each
/// report names (else the thread's). Every column of either table adds up to the recap's total
/// (threads aside, in the model table: a thread that switched models counts under each; cost
/// aside, as the recap leaves reports that name no model unpriced).
pub fn tables(window: &basecamp::Window, threads: &[ThreadActivity]) -> (Vec<Row>, Vec<Row>) {
    struct Acc<'a> {
        row: Row,
        /// Reports, with the model each is priced as.
        usage: Vec<(&'a UsageRow, Option<&'a str>)>,
    }
    fn at<'a, 'b>(rows: &'b mut Vec<(String, Acc<'a>)>, key: String, new: impl FnOnce() -> Row) -> &'b mut Acc<'a> {
        let i = match rows.iter().position(|(k, _)| *k == key) {
            Some(i) => i,
            None => {
                rows.push((key, Acc { row: new(), usage: vec![] }));
                rows.len() - 1
            }
        };
        &mut rows[i].1
    }
    let blank = |label: String, agent: Option<AgentId>, project: Option<String>| Row { label, agent, project, threads: 0, turns: 0, tokens: 0, spend: Spend::default(), failed: 0 };
    let model_key = |agent: &AgentId, model: Option<&str>| format!("{}\u{0}{}", agent.key(), model.map(basecamp::model_key).unwrap_or_default());
    let mut projects: Vec<(String, Acc)> = vec![];
    let mut models: Vec<(String, Acc)> = vec![];
    for t in threads {
        let usage: Vec<&UsageRow> = t.usage.iter().filter(|u| window.contains(u.at)).collect();
        let (mut prompts, mut turns, mut failed) = (0, 0, 0);
        for a in t.activity.iter().filter(|a| window.contains(a.at())) {
            match a {
                Activity::Prompt { .. } => prompts += 1,
                Activity::TurnEnd { .. } => turns += 1,
                Activity::TurnStopped { failed: f, .. } => {
                    turns += 1;
                    failed += *f as usize;
                }
            }
        }
        // As the recap counts it: a thread with no record of its failed turns failed once.
        if failed == 0 && t.thread.run_state == RunState::Failed && window.contains(t.thread.updated_at) {
            failed = 1;
        }
        let worked = prompts > 0 || turns > 0 || !usage.is_empty();
        if !worked && failed == 0 {
            continue;
        }
        let own = (worked && t.thread.parent_id.is_none()) as usize;
        let tokens: u64 = usage.iter().map(|u| u.tokens.total()).sum();
        let model = t.thread.model.as_deref().or_else(|| busiest_model(&usage));
        // A report that names no model was the thread's.
        let named: Vec<(&UsageRow, Option<&str>)> = usage.iter().map(|u| (*u, u.model.as_deref().or(model))).collect();
        // By project.
        let (key, label, id) = match (&t.thread.project_id, &t.project) {
            (Some(id), Some(name)) => (id.clone(), name.clone(), Some(id.clone())),
            _ => (String::new(), "No project".to_string(), None),
        };
        let p = at(&mut projects, key, || blank(label, None, id));
        p.row.threads += own;
        p.row.turns += turns;
        p.row.failed += failed;
        p.row.tokens += tokens;
        p.usage.extend(named.iter().copied());
        // By model.
        let agent = &t.thread.agent;
        let m = at(&mut models, model_key(agent, model), || blank(basecamp::model_label(agent, model), Some(agent.clone()), None));
        m.row.threads += own;
        m.row.turns += turns;
        m.row.failed += failed;
        let mut counted = vec![model_key(agent, model)];
        for &(u, named) in &named {
            let key = model_key(&u.agent, named);
            let m = at(&mut models, key.clone(), || blank(basecamp::model_label(&u.agent, named), Some(u.agent.clone()), None));
            m.row.tokens += u.tokens.total();
            m.usage.push((u, named));
            if !counted.contains(&key) {
                m.row.threads += own;
                counted.push(key);
            }
        }
    }
    let finish = |rows: Vec<(String, Acc)>| {
        let mut out: Vec<Row> = rows
            .into_iter()
            .map(|(_, a)| Row { spend: spend_of(&a.usage), ..a.row })
            .collect();
        out.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(b.turns.cmp(&a.turns)).then(a.label.cmp(&b.label)));
        out
    };
    (finish(projects), finish(models))
}

/// The model `usage` reports the most tokens for, if any report names one.
fn busiest_model<'a>(usage: &[&'a UsageRow]) -> Option<&'a str> {
    let mut by: Vec<(String, &'a str, u64)> = vec![];
    for u in usage {
        let Some(m) = u.model.as_deref() else { continue };
        let key = basecamp::model_key(m);
        match by.iter_mut().find(|(k, ..)| *k == key) {
            Some((_, _, n)) => *n += u.tokens.total(),
            None => by.push((key, m, u.tokens.total())),
        }
    }
    by.into_iter().max_by_key(|(.., n)| *n).map(|(_, m, _)| m)
}

/// What `rows` cost, each priced as the model beside it, once per agent, model, UTC day and kind
/// of cost (as the recap prices them: report by report, a long history is slow).
fn spend_of(rows: &[(&UsageRow, Option<&str>)]) -> Spend {
    type Sum<'a> = (&'a AgentId, Option<&'a str>, i64, Option<bool>, TokenUsage, f64);
    let mut index: HashMap<(&AgentId, Option<&str>, i64, Option<bool>), usize> = HashMap::new();
    let mut sums: Vec<Sum> = vec![];
    for (u, model) in rows {
        let key = (&u.agent, *model, u.at.div_euclid(86_400_000), u.cost.map(|c| c.reported));
        let i = *index.entry(key).or_insert_with(|| {
            sums.push((key.0, key.1, u.at, key.3, TokenUsage::default(), 0.));
            sums.len() - 1
        });
        sums[i].4.add(&u.tokens);
        sums[i].5 += u.cost.map_or(0., |c| c.usd);
    }
    let mut spend = Spend::default();
    for (agent, model, at, reported, tokens, usd) in &sums {
        spend.add(agent, *model, tokens, reported.map(|reported| UsageCost { usd: *usd, reported }), *at);
    }
    spend
}

/// The chart of `range` at `now` (in `now`'s time zone). `first`: the earliest activity, where
/// all time starts (at least a week back, so it's never a single day).
pub fn chart<Tz: TimeZone>(range: Range, now: &DateTime<Tz>, first: Option<i64>, threads: &[ThreadActivity]) -> Chart {
    let tz = now.timezone();
    let today = now.date_naive();
    let monday = |d: NaiveDate| d - Days::days(d.weekday().num_days_from_monday() as i64);
    let (start, step, n) = match range {
        Range::Today => (today, Step::Hour, 24),
        Range::Week => (monday(today), Step::Day, 7),
        Range::All => {
            let first = first.and_then(|f| tz.timestamp_millis_opt(f).single()).map(|d| d.date_naive()).filter(|d| *d < today).unwrap_or(today);
            let first = first.min(today - Days::days(6));
            match (today - first).num_days() + 1 {
                days @ ..=CHART_DAYS => (first, Step::Day, days),
                _ => {
                    let from = monday(first);
                    let weeks = (today - from).num_days() / 7 + 1;
                    let per = (weeks + CHART_WEEKS - 1) / CHART_WEEKS;
                    (from, Step::Weeks(per as u32), (weeks + per - 1) / per)
                }
            }
        }
    };
    let n = n as usize;
    let bar_of = |ms: i64| -> Option<usize> {
        let local = tz.timestamp_millis_opt(ms).single()?.naive_local();
        let i = match step {
            Step::Hour => (local.date() == today).then_some(local.hour() as i64)?,
            Step::Day => (local.date() - start).num_days(),
            Step::Weeks(per) => (local.date() - start).num_days().div_euclid(7 * per as i64),
        };
        usize::try_from(i).ok().filter(|i| *i < n)
    };
    let day_of = |i: usize| match step {
        Step::Hour => start,
        Step::Day => start + Days::days(i as i64),
        Step::Weeks(per) => start + Days::days(i as i64 * 7 * per as i64),
    };
    let this_year = |d: NaiveDate| d.year() == today.year();
    let date = |d: NaiveDate| if this_year(d) { d.format("%b %-d").to_string() } else { d.format("%b %-d, %Y").to_string() };
    let mut bars: Vec<Bar> = (0..n)
        .map(|i| {
            let label = match step {
                Step::Hour => format!("{}–{}", hour_name(i as u32), hour_name(i as u32 + 1)),
                Step::Day => format!("{}, {}", day_of(i).format("%a"), date(day_of(i))),
                Step::Weeks(1) => format!("Week of {}", date(day_of(i))),
                Step::Weeks(per) => format!("{} – {}", date(day_of(i)), date(day_of(i) + Days::days(7 * per as i64 - 1))),
            };
            Bar { label, turns: 0, tokens: 0 }
        })
        .collect();
    for t in threads {
        for a in &t.activity {
            if let Activity::TurnEnd { at, .. } | Activity::TurnStopped { at, .. } = *a
                && let Some(i) = bar_of(at)
            {
                bars[i].turns += 1;
            }
        }
        for u in &t.usage {
            if let Some(i) = bar_of(u.at) {
                bars[i].tokens += u.tokens.total();
            }
        }
    }
    let ticks: Vec<(usize, String)> = match step {
        Step::Hour => [(0, "12 AM"), (6, "6 AM"), (12, "Noon"), (18, "6 PM")].iter().map(|(i, l)| (*i, l.to_string())).collect(),
        Step::Day if range == Range::Week => (0..n).map(|i| (i, day_of(i).format("%a").to_string())).collect(),
        _ => {
            // A date under every bar while there's room, else six spread across.
            let label = |i: usize| {
                let d = day_of(i);
                if this_year(d) || step == Step::Day { d.format("%b %-d").to_string() } else { d.format("%b %Y").to_string() }
            };
            let at: Vec<usize> = if n <= 8 { (0..n).collect() } else { (0..6).map(|k| (k * (n - 1) + 2) / 5).collect() };
            let mut out: Vec<(usize, String)> = vec![];
            for i in at {
                if out.last().is_none_or(|(j, l)| *j != i && *l != label(i)) {
                    out.push((i, label(i)));
                }
            }
            out
        }
    };
    Chart { step, bars, ticks }
}

/// "12 AM", "2 PM" for an hour of the day (24 is midnight again).
fn hour_name(h: u32) -> String {
    let h = h % 24;
    let twelve = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{twelve} {}", if h < 12 { "AM" } else { "PM" })
}

// The phone's Basecamp (`remote/basecamp.rs`) draws the recap as tiles and a profile, worded
// here so both read alike.

/// What a stat tile shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TileKind {
    BestModel,
    WorkedMostOn,
    Tokens,
    /// Agent time, with failed turns in its note.
    AgentTime,
    PlanLeft,
}

/// A tile's words, and what's drawn beside its figure: Basecamp's and the phone's.
#[derive(Debug, Clone)]
pub(crate) struct TileData {
    pub kind: TileKind,
    pub label: String,
    pub figure: String,
    pub note: String,
    /// The logo beside the figure.
    pub agent: Option<AgentId>,
    /// The project badge beside it (id, name).
    pub project: Option<(String, String)>,
    /// How much of a plan is left, 0–100.
    pub left: Option<f32>,
    pub resets_at: Option<i64>,
}

/// The tiles under the profile, in order: the best model, the project worked on most, the
/// tokens, the agents' time, then what's left of each plan that reports limits.
pub(crate) fn tile_data(recap: &Recap, ws: &Workspace) -> Vec<TileData> {
    let mut tiles = vec![];
    let tile = |kind, label: &str, figure: String, note: String| TileData { kind, label: label.to_string(), figure, note, agent: None, project: None, left: None, resets_at: None };
    if let Some(best) = recap.best_model() {
        let reported = if recap.tokens_complete() { "" } else { "reported " };
        let note = match recap.token_share(best) {
            Some(100) => format!("All the {reported}tokens · {}", basecamp::count(best.turns, "turn", "turns")),
            Some(share) => format!("{share}% of {reported}tokens · {}", basecamp::count(best.turns, "turn", "turns")),
            None if best.turns == recap.turns => format!("Every turn ({})", recap.turns),
            None => format!("{} of {} turns", best.turns, recap.turns),
        };
        tiles.push(TileData { agent: Some(best.agent.clone()), ..tile(TileKind::BestModel, "Your best model", basecamp::model_label(&best.agent, best.model.as_deref()), note) });
    }
    if let Some(top) = recap.projects.first() {
        let mut note = basecamp::count(top.prompts, "prompt", "prompts");
        if top.tokens > 0 {
            note.push_str(&format!(" · {} tokens", fmt_tokens(top.tokens)));
        }
        tiles.push(TileData { project: Some((top.id.clone(), top.name.clone())), ..tile(TileKind::WorkedMostOn, "You worked most on", top.name.clone(), note) });
    }
    let total = recap.tokens.total();
    if total > 0 {
        tiles.push(tile(TileKind::Tokens, "You used", format!("{} tokens", fmt_tokens(total)), tokens_note(recap, during(recap.window.range))));
    }
    if recap.agent_secs > 0 || recap.failed > 0 {
        // Said of the range, so a failure from before it still waiting for review (on the
        // left) doesn't contradict it.
        let when = during(recap.window.range);
        let note = match recap.failed {
            0 => format!("Nothing failed {when}"),
            n => format!("{} failed {when}", basecamp::count(n, "turn", "turns")),
        };
        tiles.push(tile(TileKind::AgentTime, "Your agents worked for", basecamp::duration(recap.agent_secs), note));
    }
    // Plan limits, for each agent that reports them: the one closest to running out.
    let mut agents: Vec<(&String, &trek_agents::AgentStatus)> = ws.agent_status.iter().filter(|(_, s)| !s.limits.is_empty()).collect();
    agents.sort_by_key(|(k, _)| k.as_str());
    for (key, status) in agents {
        let Some(limit) = status.limits.iter().max_by(|a, b| a.percent.total_cmp(&b.percent)) else { continue };
        let agent = AgentId::from_key(key);
        let left = (100. - limit.percent).clamp(0., 100.);
        let label = format!("Left on {}", status.plan.clone().unwrap_or_else(|| agent.display_name()));
        let resets = limit.resets_at.map(crate::time::until).map(|u| format!(" · resets {u}")).unwrap_or_default();
        tiles.push(TileData {
            agent: Some(agent),
            left: Some(left),
            resets_at: limit.resets_at,
            ..tile(TileKind::PlanLeft, &label, format!("{left:.0}%"), format!("{}{resets}", limit.label))
        });
    }
    tiles
}

/// The line beside "Basecamp": a greeting with the date, or for all time, since when.
pub(crate) fn greeting_line(range: Range, recap: Option<&Recap>) -> String {
    let now = chrono::Local::now();
    let hello = basecamp::greeting(chrono::Timelike::hour(&now));
    let since = recap.filter(|r| r.window.range == Range::All && range == Range::All).and_then(|r| r.first).map(local);
    match since {
        Some(d) if d.year() == now.year() => format!("{hello} — on the trail since {}", d.format("%-d %B")),
        Some(d) => format!("{hello} — on the trail since {}", d.format("%-d %B %Y")),
        None => format!("{hello}, {}", now.format("%A %-d %B")),
    }
}

/// "Today's trek", "This week's trek", "Your trek so far".
pub(crate) fn trek_title(range: Range) -> &'static str {
    match range {
        Range::Today => "Today's trek",
        Range::Week => "This week's trek",
        Range::All => "Your trek so far",
    }
}

/// What the empty recap invites to.
pub(crate) fn invitation(range: Range) -> String {
    match range {
        Range::All => "Nothing on the trail yet — start a thread.".to_string(),
        range => format!("Nothing on the trail yet {} — start a thread.", during(range)),
    }
}

/// What a thread that needs the user is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiting {
    Approval,
    Question,
    Plan,
    /// The request went with its session (Trek was relaunched): it's asked again on reopening.
    Unknown,
    /// A sub-agent of the thread's waits on an approval: its row in the thread opens it.
    SubAgent,
}

impl Waiting {
    pub fn of(request: Option<&crate::workspace::PendingPermission>) -> Waiting {
        match request.map(|r| &r.prompt) {
            Some(Some(trek_agents::Prompt::Questions(_))) => Waiting::Question,
            Some(Some(trek_agents::Prompt::Plan(_))) => Waiting::Plan,
            Some(None) => Waiting::Approval,
            None => Waiting::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Waiting::Approval => "Approval",
            Waiting::Question => "Question",
            Waiting::Plan => "Plan to review",
            Waiting::Unknown => "Needs you",
            Waiting::SubAgent => "Sub-agent needs you",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Waiting::Approval => Icon::new(crate::assets::Lucide::ShieldCheck),
            Waiting::Question => Icon::new(crate::assets::Lucide::MessageSquare),
            Waiting::Plan => Icon::new(crate::assets::Lucide::ListChecks),
            Waiting::Unknown => Icon::new(IconName::CircleAlert),
            Waiting::SubAgent => Icon::new(crate::assets::Lucide::ShieldCheck),
        }
    }
}


/// The line over the profile: the hovered stretch's numbers, else where the summit was.
pub(crate) fn profile_line(recap: &Recap, hovered: Option<usize>) -> String {
    match hovered.and_then(|i| recap.buckets.get(i).map(|b| (i, b))) {
        Some((i, b)) => {
            let mut parts = vec![stretch_label(recap, i)];
            if b.prompts > 0 {
                parts.push(basecamp::count(b.prompts, "prompt", "prompts"));
            }
            if b.agent_secs > 0 {
                parts.push(format!("{} of agent time", basecamp::duration(b.agent_secs)));
            }
            if b.tokens > 0 {
                parts.push(format!("{} tokens", fmt_tokens(b.tokens)));
            }
            if parts.len() == 1 {
                parts.push("quiet".into());
            }
            parts.join(" · ")
        }
        None => match recap.peak() {
            Some(i) => format!("Summit {}", summit_label(recap, i)),
            None => "A flat trail so far".into(),
        },
    }
}


const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

/// The range as a sentence ends with it: "Nothing failed today", "… so far".
pub(crate) fn during(range: Range) -> &'static str {
    match range {
        Range::Today => "today",
        Range::Week => "this week",
        Range::All => "so far",
    }
}

fn local(ms: i64) -> chrono::DateTime<chrono::Local> {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().with_timezone(&chrono::Local)
}

/// The local hour of `ms`: "2 PM".
fn hour(ms: i64) -> String {
    local(ms).format("%-I %p").to_string()
}

/// The day a stretch of days or weeks starts on: "Sep 14", with the year when it isn't this
/// one. Read at its noon, as across a clock change it may start an hour either side of midnight.
fn day(start: i64) -> String {
    let d = local(start + DAY / 2);
    if d.year() == chrono::Local::now().year() { d.format("%b %-d").to_string() } else { d.format("%b %-d, %Y").to_string() }
}

/// A stretch of the profile, for the line above it: "2–3 PM", "Tue 3–6 PM", "Sep 14", "week of
/// Sep 8", "Sep 8 – Oct 5".
pub(crate) fn stretch_label(recap: &Recap, i: usize) -> String {
    let w = &recap.window;
    let start = w.start + i as i64 * w.bucket_ms;
    if w.bucket_ms >= 2 * 7 * DAY {
        return format!("{} – {}", day(start), day(start + w.bucket_ms - DAY));
    }
    if w.bucket_ms >= 7 * DAY {
        return format!("week of {}", day(start));
    }
    if w.bucket_ms >= DAY {
        return day(start);
    }
    let (a, b) = (hour(start), hour(start + w.bucket_ms));
    let span = match (a.split_once(' '), b.split_once(' ')) {
        (Some((h0, m0)), Some((h1, m1))) if m0 == m1 => format!("{h0}–{h1} {m1}"),
        _ => format!("{a}–{b}"),
    };
    // Over more than a day, which day.
    if w.end - w.start > DAY + HOUR { format!("{} {span}", local(start).format("%a")) } else { span }
}

/// Where the summit was: "at 2 PM", "on Tuesday", "on Sep 14", "the week of Sep 8".
pub(crate) fn summit_label(recap: &Recap, i: usize) -> String {
    let w = &recap.window;
    let start = w.start + i as i64 * w.bucket_ms;
    if w.bucket_ms >= 2 * 7 * DAY {
        format!("in the weeks from {}", day(start))
    } else if w.bucket_ms >= 7 * DAY {
        format!("the week of {}", day(start))
    } else if w.bucket_ms >= DAY {
        format!("on {}", day(start))
    } else if w.end - w.start > DAY + HOUR {
        format!("on {}", local(start).format("%A"))
    } else {
        format!("at {}", hour(start))
    }
}

/// The profile's axis labels, as fractions of its width: the hours of a day, the days of a few,
/// else five dates spread across it ("Sep 14", or "Sep 2025" when it reaches back past this year).
pub(crate) fn ticks(w: &basecamp::Window) -> Vec<(f32, String)> {
    let span = (w.end - w.start).max(1) as f32;
    if w.bucket_ms < 3 * HOUR {
        return [(6, "6 AM"), (12, "Noon"), (18, "6 PM")].iter().map(|(h, l)| ((*h as f32 * HOUR as f32) / span, l.to_string())).collect();
    }
    if w.bucket_ms < DAY {
        let days = ((w.end - w.start + DAY / 2) / DAY).max(1);
        return (0..days).map(|d| ((d as f32 + 0.5) / days as f32, local(w.start + d * DAY + DAY / 2).format("%a").to_string())).collect();
    }
    let n = w.buckets();
    let years = local(w.start + DAY / 2).year() != chrono::Local::now().year();
    let mut out: Vec<(f32, String)> = vec![];
    for f in [0.1, 0.3, 0.5, 0.7, 0.9] {
        let i = ((f * n as f32) as usize).min(n - 1);
        let d = local(w.start + i as i64 * w.bucket_ms + DAY / 2);
        let label = if years { d.format("%b %Y") } else { d.format("%b %-d") }.to_string();
        if out.last().is_none_or(|(_, l)| *l != label) {
            out.push(((i as f32 + 0.5) / n as f32, label));
        }
    }
    out
}


/// What the tokens tile says under its figure: what they'd cost at API prices (as far as they
/// have a price, saying how many haven't), then where they came from. Kept short, as the tile is
/// narrow and a caveat saying the total is incomplete has to stay in view: with one, the span
/// goes (the header's range already names it).
pub(crate) fn tokens_note(recap: &basecamp::Recap, when: &str) -> String {
    let partial = recap.tokens_partial().then(|| format!("{} of {} threads", recap.threads_with_tokens, recap.threads));
    if !recap.spend.priced() {
        return match partial {
            Some(p) => format!("From {p}"),
            None if recap.tokens.cache_read > 0 => format!("{} read from cache", fmt_tokens(recap.tokens.cache_read)),
            None => "As your agents reported them".to_string(),
        };
    }
    let cost = format!("≈ {} at API prices", crate::cost::usd(recap.spend.usd()));
    let unpriced = (recap.spend.unpriced() > 0).then(|| format!("{} tokens unpriced", fmt_tokens(recap.spend.unpriced())));
    let caveats: Vec<String> = [unpriced, partial].into_iter().flatten().collect();
    if caveats.is_empty() { format!("{cost} {when}") } else { format!("{cost} · {}", caveats.join(" · ")) }
}


#[cfg(test)]
mod tests {
    use super::{DAY, stretch_label, summit_label, ticks, tokens_note};
    use trek_core::basecamp::{Range, Recap, ThreadActivity};
    use trek_core::store::{Activity, Store, UsageRow};
    use trek_core::{AgentId, Effort, HandHolding, TokenUsage, UsageCost};

    #[test]
    fn the_tokens_tile_says_what_they_cost_at_api_prices() {
        let window = Range::Today.window(&chrono::Local::now());
        let at = window.start + 60_000;
        let s = Store::in_memory().unwrap();
        let thread = |agent: AgentId| {
            let t = s.create_thread(None, agent, None, Effort::High, HandHolding::Auto).unwrap();
            ThreadActivity { thread: t, project: None, activity: vec![Activity::Prompt { at }, Activity::TurnEnd { at: at + 1_000, took_secs: 1 }], usage: vec![] }
        };
        let row = |t: &ThreadActivity, model: &str, tokens: TokenUsage, cost: Option<UsageCost>| UsageRow { thread_id: t.thread.id.clone(), at, agent: t.thread.agent.clone(), model: Some(model.into()), tokens, cost };
        let mut claude = thread(AgentId::ClaudeCode);
        claude.usage = vec![row(&claude, "claude-opus-5-5", TokenUsage { input: 10, output: 400, cache_read: 30_000, cache_write: 2_000 }, Some(UsageCost::reported(1.25)))];
        // Recorded before Trek kept costs: priced now, 100,000 × $0.20 + 10,000 × $1.20 per million.
        let mut codex = thread(AgentId::Codex);
        codex.usage = vec![row(&codex, "gpt-5.6-luna", TokenUsage { input: 100_000, output: 10_000, ..Default::default() }, None)];
        let r = Recap::compute(window, at + 5_000, &[claude.clone(), codex]);
        assert_eq!(tokens_note(&r, "today"), "≈ $1.28 at API prices today");
        assert_eq!(tokens_note(&r, super::during(Range::All)), "≈ $1.28 at API prices so far");
        // Threads that didn't report their tokens are said to be missing from it.
        let quiet = thread(AgentId::Acp("gemini".into()));
        let r = Recap::compute(window, at + 5_000, &[claude.clone(), quiet]);
        assert_eq!(tokens_note(&r, "this week"), "≈ $1.25 at API prices · 1 of 2 threads");
        // Tokens on a model without a known price are said to be left out.
        let mut fusion = thread(AgentId::Acp("devin".into()));
        fusion.usage = vec![row(&fusion, "fusion-x", TokenUsage { input: 2_000_000, output: 5_000, ..Default::default() }, None)];
        let r = Recap::compute(window, at + 5_000, &[claude.clone(), fusion.clone()]);
        assert_eq!(tokens_note(&r, "today"), "≈ $1.25 at API prices · 2M tokens unpriced");
        let r = Recap::compute(window, at + 5_000, &[claude.clone(), fusion.clone(), thread(AgentId::Acp("gemini".into()))]);
        assert_eq!(tokens_note(&r, "today"), "≈ $1.25 at API prices · 2M tokens unpriced · 2 of 3 threads");
        // Nothing priced: just where the tokens came from.
        let r = Recap::compute(window, at + 5_000, &[fusion, thread(AgentId::Acp("gemini".into()))]);
        assert_eq!(tokens_note(&r, "today"), "From 1 of 2 threads");
    }

    #[test]
    fn all_time_is_labelled_by_the_hour_day_or_week_it_is_drawn_in() {
        let now = chrono::Local::now();
        let ago = |days: i64| now - chrono::Duration::days(days);
        let recap = |days: i64| Recap::compute(Range::All.window_from(&now, Some(ago(days).timestamp_millis())), now.timestamp_millis(), &[]);
        // Just today: as today.
        let r = recap(0);
        assert_eq!(ticks(&r.window).iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(), ["6 AM", "Noon", "6 PM"]);
        let afternoon = stretch_label(&r, 14);
        assert!(afternoon.ends_with(" PM") && !afternoon.contains(&now.format("%a").to_string()), "{afternoon}");
        // A few days: the day's name with the hours.
        let r = recap(3);
        assert_eq!(ticks(&r.window).len(), 4);
        assert!(stretch_label(&r, 0).starts_with(&ago(3).format("%a ").to_string()), "{}", stretch_label(&r, 0));
        assert_eq!(summit_label(&r, 0), format!("on {}", ago(3).format("%A")));
        // Six weeks: dates.
        let r = recap(41);
        assert!(stretch_label(&r, 0).starts_with(&ago(41).format("%b %-d").to_string()), "{}", stretch_label(&r, 0));
        assert!(summit_label(&r, 0).starts_with("on "));
        assert_eq!(ticks(&r.window).len(), 5);
        // A year: weeks, with the year on the axis.
        let r = recap(400);
        assert_eq!(r.window.bucket_ms, 7 * DAY);
        let week = stretch_label(&r, 0);
        assert!(week.starts_with("week of ") && week.ends_with(&super::local(r.window.start + DAY / 2).format(", %Y").to_string()), "{week}");
        let axis = ticks(&r.window);
        assert!(axis.len() == 5 && axis[0].1.chars().rev().take(4).all(|c| c.is_ascii_digit()), "{axis:?}");
        assert_eq!(super::profile_line(&r, None), "A flat trail so far");
        // Years: several weeks a stretch.
        let r = recap(3 * 365);
        assert!(stretch_label(&r, 0).contains(" – "), "{}", stretch_label(&r, 0));
        assert!(summit_label(&r, 0).starts_with("in the weeks from "));
    }
}
