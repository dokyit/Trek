//! Basecamp: the day's (or week's, or all time's) trek at a glance. On the left, what's ready
//! for review; on the right, the recap in a few sentences, the elevation profile (activity drawn
//! as a mountain, the summit flagged, the hiker at "now") and quiet stat tiles.
//!
//! The recap is computed off the main thread (`trek_core::basecamp`) when Basecamp opens and
//! whenever threads change while it's open; frames only draw it.

use crate::palette;
use crate::ui;
use chrono::Datelike as _;
use crate::workspace::{Route, Workspace, fmt_tokens};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use trek_core::basecamp::{self, Range, Recap, Span};
use trek_core::store::{Thread, now_ms};
use trek_core::{AgentId, RunState};

actions!(basecamp, [Leave]);

/// How long the numbers take to count up when Basecamp opens.
const COUNT_UP: Duration = Duration::from_millis(400);
/// The elevation profile: room above the silhouette (the summit flag), the silhouette, and the
/// axis labels under it.
const PROFILE_TOP: f32 = 24.;
const PROFILE_HEIGHT: f32 = 96.;
const PROFILE_AXIS: f32 = 20.;
/// The hiker on the profile, in points per sprite pixel.
const HIKER_CELL: f32 = 1.;

/// What a recap was computed from: another key means it's out of date.
#[derive(Debug, Clone, PartialEq)]
struct Key {
    range: Range,
    start: i64,
    turns: u64,
    threads: usize,
    latest: i64,
}

pub struct Basecamp {
    workspace: Entity<Workspace>,
    focus: FocusHandle,
    range: Range,
    recap: Option<Arc<Recap>>,
    key: Option<Key>,
    computing: bool,
    /// On screen (the main window's route).
    open: bool,
    /// When the numbers started counting up on this visit.
    shown_at: Option<Instant>,
    /// The stretch of the profile under the pointer.
    hovered: Option<usize>,
    /// Where the profile was last laid out, to tell which stretch the pointer is over.
    profile: Rc<Cell<Option<Bounds<Pixels>>>>,
    _compute: Option<Task<()>>,
    /// While open: a tick a minute (the hiker walks with the clock, the day may turn over).
    _clock: Option<Task<()>>,
    _subscription: Subscription,
}

impl Basecamp {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let _subscription = cx.observe(&workspace, |this, _, cx| this.sync(cx));
        let mut this = Self {
            workspace,
            focus: cx.focus_handle(),
            range: Range::Today,
            recap: None,
            key: None,
            computing: false,
            open: false,
            shown_at: None,
            hovered: None,
            profile: Rc::default(),
            _compute: None,
            _clock: None,
            _subscription,
        };
        this.sync(cx);
        this
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// The recap on screen, once it's up to date (none is being computed).
    #[cfg(test)]
    pub fn recap(&self) -> Option<&Recap> {
        self.recap.as_deref().filter(|_| !self.computing)
    }

    /// Where the profile was drawn, and the stretch under the pointer.
    #[cfg(test)]
    pub fn profile_hover(&self) -> (Option<Bounds<Pixels>>, Option<usize>) {
        (self.profile.get(), self.hovered)
    }

    /// The line over the profile, as drawn now.
    #[cfg(test)]
    pub fn profile_line(&self) -> Option<String> {
        self.recap.as_deref().map(|r| profile_line(r, self.hovered))
    }

    /// Count the numbers up again, as on opening.
    #[cfg(test)]
    pub fn replay_count_up(&mut self) {
        self.shown_at = None;
    }

    pub fn range(&self) -> Range {
        self.range
    }

    /// Follow the workspace: open or close with the route, and recompute when threads moved.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let open = self.workspace.read(cx).route == Route::Basecamp;
        if open != self.open {
            self.open = open;
            self.hovered = None;
            self.shown_at = None;
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
            // Plan limits for the "Left on" tiles, once the agents are known (asked at most every
            // 30 seconds; nothing is asked when no agent reports limits).
            let ws = self.workspace.read(cx);
            if ws.agent_status.is_empty() && !ws.usage_loading && !ws.detecting {
                self.workspace.update(cx, |ws, cx| {
                    ws.refresh_usage(cx);
                    ws.refresh_devin_usage(cx);
                });
            }
            self.refresh(cx);
            // The review list and the tiles read the workspace as it is now.
            cx.notify();
        }
    }

    pub fn set_range(&mut self, range: Range, cx: &mut Context<Self>) {
        if range != self.range {
            self.range = range;
            self.shown_at = None;
            self.hovered = None;
            self.refresh(cx);
            cx.notify();
        }
    }

    /// The clock moved on to `now`: a recap still current only moves its "now" (the hiker, the
    /// trail walked, "Updated"), one that isn't is computed again. Either way it's drawn again,
    /// relative times and the greeting with it.
    pub fn tick(&mut self, now: i64, cx: &mut Context<Self>) {
        self.refresh(cx);
        if !self.computing
            && let Some(recap) = self.recap.as_mut()
        {
            Arc::make_mut(recap).now = now;
        }
        cx.notify();
    }

    /// Compute the recap again if what it was computed from changed. One at a time: one that
    /// finishes looks again.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        // A new day (or week) makes the recap out of date. All time's own start is found with the
        // history, off the main thread; this one is today's, so it's computed again each day too.
        let window = self.range.window(&chrono::Local::now());
        let key = Key {
            range: self.range,
            start: window.start,
            turns: ws.turns_finished,
            threads: ws.threads.len(),
            latest: ws.threads.iter().map(|t| t.updated_at).max().unwrap_or(0),
        };
        if self.computing || self.key.as_ref() == Some(&key) {
            return;
        }
        self.computing = true;
        let store = ws.store.clone();
        let range = self.range;
        let work = cx.background_executor().spawn(async move {
            let now = chrono::Local::now();
            basecamp::recap(&store, range, &now).unwrap_or_else(|e| {
                tracing::warn!("basecamp: {e:#}");
                Recap::compute(range.window(&now), now.timestamp_millis(), &[])
            })
        });
        self._compute = Some(cx.spawn(async move |this, cx| {
            let recap = work.await;
            let _ = this.update(cx, |this, cx| {
                this.computing = false;
                // Switching range while it ran makes this one moot.
                if key.range == this.range {
                    this.recap = Some(Arc::new(recap));
                    this.key = Some(key);
                }
                this.refresh(cx);
                cx.notify();
            });
        }));
    }

    /// How far the numbers have counted up: 0 to 1 over `COUNT_UP`, eased; 1 with reduced motion.
    pub(crate) fn count_up(&mut self, cx: &App) -> f32 {
        if self.workspace.read(cx).settings.appearance.reduce_motion || cx.reduce_motion() {
            return 1.;
        }
        let started = *self.shown_at.get_or_insert_with(Instant::now);
        let t = (started.elapsed().as_secs_f32() / COUNT_UP.as_secs_f32()).min(1.);
        1. - (1. - t).powi(3)
    }

    /// `count_up`, drawing again until it's done.
    fn progress(&mut self, window: &mut Window, cx: &App) -> f32 {
        let p = self.count_up(cx);
        if p < 1. {
            window.request_animation_frame();
        }
        p
    }

    fn header(&self, review_unread: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let now = chrono::Local::now();
        let hello = basecamp::greeting(chrono::Timelike::hour(&now));
        let range = self.range;
        // All time says since when, once the recap knows.
        let since = self.recap.as_deref().filter(|r| r.window.range == Range::All && range == Range::All).and_then(|r| r.first).map(local);
        let greeting = match since {
            Some(d) if d.year() == now.year() => format!("{hello} — on the trail since {}", d.format("%-d %B")),
            Some(d) => format!("{hello} — on the trail since {}", d.format("%-d %B %Y")),
            None => format!("{hello}, {}", now.format("%A %-d %B")),
        };
        let this = cx.entity().downgrade();
        h_flex()
            .w_full()
            .gap(px(12.))
            .items_end()
            .child(div().text_size(px(24.)).font_semibold().line_height(px(30.)).child("Basecamp"))
            .child(div().pb(px(4.)).text_size(px(13.5)).text_color(theme.muted_foreground).child(greeting))
            .child(div().flex_1())
            .child(ui::segmented(
                "basecamp-range",
                [Range::Today, Range::Week, Range::All].map(|r| (r, r.label())).to_vec(),
                range,
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
            )
            .into_any_element()
    }

    /// "Ready for review": what needs the user first, then finished threads not looked at yet.
    fn review(&self, threads: &[Thread], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let projects: Vec<Option<String>> = {
            let ws = self.workspace.read(cx);
            threads.iter().map(|t| t.project_id.as_ref().and_then(|p| ws.project(p)).map(|p| p.name.clone())).collect()
        };
        let waiting: Vec<Option<Waiting>> = {
            let ws = self.workspace.read(cx);
            let sub_agents = ws.waiting_on_sub_agents();
            threads
                .iter()
                .map(|t| match t.run_state {
                    RunState::NeedsYou => Some(Waiting::of(ws.pending_request(&t.id))),
                    _ if sub_agents.contains(&t.id) => Some(Waiting::SubAgent),
                    _ => None,
                })
                .collect()
        };
        let now = self.workspace.read(cx).now();
        let rows: Vec<AnyElement> = threads.iter().zip(projects).zip(waiting).map(|((t, project), waiting)| review_row(t, project, waiting, now, cx)).collect();
        v_flex()
            .gap(px(4.))
            .child(
                h_flex()
                    .px(px(10.))
                    .pb(px(6.))
                    .gap(px(6.))
                    .text_size(px(13.))
                    .child(div().font_medium().child("Ready for review"))
                    .when(!threads.is_empty(), |el| el.child(div().text_color(theme.muted_foreground).child(threads.len().to_string()))),
            )
            .when(threads.is_empty(), |el| {
                el.child(div().px(px(10.)).text_size(px(13.)).text_color(theme.muted_foreground).child("Nothing waiting on you. Every thread is read."))
            })
            .children(rows)
            .into_any_element()
    }

    /// The recap: what the trek came to in sentences, the profile, the tiles.
    fn trek(&self, recap: &Recap, p: f32, tiles_per_row: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let title = match recap.window.range {
            Range::Today => "Today's trek",
            Range::Week => "This week's trek",
            Range::All => "Your trek so far",
        };
        let updated = if self.computing { "Updating…".to_string() } else { format!("Updated {}", crate::time::clock(recap.now)) };
        v_flex()
            .child(
                h_flex()
                    .pb(px(12.))
                    .text_size(px(13.))
                    .child(div().flex_1().font_medium().child(title))
                    .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child(updated)),
            )
            .child(narrative(&recap.narrative(basecamp::model_label), p, &self.workspace, cx))
            .child(div().mt(px(24.)).h(px(1.)).bg(theme.foreground.opacity(0.07)))
            .child(self.profile(recap, p, cx))
            .child(self.tiles(recap, p, tiles_per_row, cx))
            .into_any_element()
    }

    fn profile(&self, recap: &Recap, p: f32, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let total = basecamp::count(recap.prompts, "prompt", "prompts");
        let line = profile_line(recap, self.hovered);
        let data = Profile::of(recap, p, self.hovered, cx);
        let bounds = self.profile.clone();
        v_flex()
            .pt(px(16.))
            .child(
                h_flex()
                    .text_size(px(12.5))
                    .child(div().flex_1().min_w_0().truncate().text_color(theme.muted_foreground).child(line))
                    .child(div().text_color(theme.muted_foreground).child(tween(&total, p))),
            )
            .child(
                div()
                    .id("basecamp-profile")
                    .test_support()
                    .w_full()
                    .h(px(PROFILE_TOP + PROFILE_HEIGHT + PROFILE_AXIS))
                    .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _, cx| {
                        let Some(b) = this.profile.get() else { return };
                        let n = this.recap.as_ref().map_or(0, |r| r.buckets.len());
                        let w = b.size.width.as_f32();
                        if n == 0 || w <= 0. {
                            return;
                        }
                        let i = (((e.position.x - b.origin.x).as_f32() / w) * n as f32).floor().clamp(0., n as f32 - 1.) as usize;
                        if this.hovered != Some(i) {
                            this.hovered = Some(i);
                            cx.notify();
                        }
                    }))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if !*hovered && this.hovered.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .child(
                        canvas(
                            move |b, _, _| bounds.set(Some(b)),
                            move |b, _, window, cx| data.paint(b, window, cx),
                        )
                        .size_full(),
                    ),
            )
            .into_any_element()
    }

    fn tiles(&self, recap: &Recap, p: f32, per_row: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let mut tiles: Vec<AnyElement> = vec![];
        let figure = |text: String| div().text_size(px(17.)).font_medium().min_w_0().truncate().child(tween(&text, p));
        if let Some(best) = recap.best_model() {
            let reported = if recap.tokens_complete() { "" } else { "reported " };
            let note = match recap.token_share(best) {
                Some(100) => format!("All the {reported}tokens · {}", basecamp::count(best.turns, "turn", "turns")),
                Some(share) => format!("{share}% of {reported}tokens · {}", basecamp::count(best.turns, "turn", "turns")),
                None if best.turns == recap.turns => format!("Every turn ({})", recap.turns),
                None => format!("{} of {} turns", best.turns, recap.turns),
            };
            tiles.push(tile(
                "Your best model",
                h_flex().gap(px(8.)).min_w_0().child(ui::agent_logo(&best.agent, px(16.), cx)).child(figure(basecamp::model_label(&best.agent, best.model.as_deref()))),
                note,
                cx,
            ));
        }
        if let Some(top) = recap.projects.first() {
            let look = ws.project(&top.id).map(|p| ws.project_look(&p.path)).unwrap_or_default();
            let mut note = basecamp::count(top.prompts, "prompt", "prompts");
            if top.tokens > 0 {
                note.push_str(&format!(" · {} tokens", fmt_tokens(top.tokens)));
            }
            tiles.push(tile(
                "You worked most on",
                h_flex().gap(px(8.)).min_w_0().child(ui::project_badge(&top.name, &look, cx)).child(figure(top.name.clone())),
                note,
                cx,
            ));
        }
        let total = recap.tokens.total();
        if total > 0 {
            let note = tokens_note(recap, during(recap.window.range));
            let spark = Sparkline::of(recap, p, cx);
            tiles.push(tile(
                "You used",
                h_flex()
                    .gap(px(12.))
                    .child(figure(format!("{} tokens", fmt_tokens(total))))
                    .child(div().flex_1().min_w(px(24.)).h(px(18.)).child(canvas(|_, _, _| {}, move |b, _, window, _| spark.paint(b, window)).size_full())),
                note,
                cx,
            ));
        }
        if recap.agent_secs > 0 || recap.failed > 0 {
            // Said of the range, so a failure from before it still waiting for review (on the
            // left) doesn't contradict it.
            let when = during(recap.window.range);
            let note = match recap.failed {
                0 => format!("Nothing failed {when}"),
                n => format!("{} failed {when}", basecamp::count(n, "turn", "turns")),
            };
            tiles.push(tile("Your agents worked for", figure(basecamp::duration(recap.agent_secs)), note, cx));
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
            let color = if left <= 10. { palette::red(cx) } else if left <= 30. { palette::amber(cx) } else { theme.foreground.opacity(0.85) };
            tiles.push(tile(
                label,
                v_flex()
                    .gap(px(8.))
                    .child(h_flex().gap(px(8.)).child(ui::agent_logo(&agent, px(16.), cx)).child(figure(format!("{left:.0}%"))))
                    .child(div().h(px(4.)).w_full().rounded_full().bg(theme.foreground.opacity(0.08)).child(div().h_full().rounded_full().bg(color).w(relative(left / 100. * p)))),
                format!("{}{resets}", limit.label),
                cx,
            ));
        }
        let line = theme.foreground.opacity(0.07);
        let rows: Vec<AnyElement> = tiles
            .chunks_mut(per_row.max(1))
            .map(|row| {
                let n = row.len();
                // Stretched, so the hairline between tiles runs the row's full height.
                h_flex()
                    .items_stretch()
                    .border_t_1()
                    .border_color(line)
                    .children(row.iter_mut().enumerate().map(|(i, t)| {
                        let t = std::mem::replace(t, div().into_any_element());
                        div().flex_1().min_w_0().when(i > 0, |el| el.border_l_1().border_color(line)).child(t)
                    }))
                    // Short rows keep the grid: empty cells fill them out.
                    .children((n..per_row).map(|_| div().flex_1()))
                    .into_any_element()
            })
            .collect();
        v_flex().mt(px(20.)).border_b_1().border_color(line).children(rows).into_any_element()
    }

    /// Nothing on the trail in this range yet.
    fn empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let invitation = match self.range {
            Range::All => "Nothing on the trail yet — start a thread.".to_string(),
            range => format!("Nothing on the trail yet {} — start a thread.", during(range)),
        };
        let data = Profile::flat(self.recap.as_deref(), cx);
        v_flex()
            .id("basecamp-empty")
            .test_support()
            .gap(px(12.))
            .child(div().w_full().h(px(PROFILE_TOP + 40.)).child(canvas(|_, _, _| {}, move |b, _, window, cx| data.paint(b, window, cx)).size_full()))
            .child(div().text_size(px(17.)).line_height(px(26.)).text_color(theme.foreground.opacity(0.85)).child(invitation))
            .child(
                h_flex().child(
                    Button::new("basecamp-new-thread")
                        .primary()
                        .small()
                        .icon(crate::assets::Lucide::SquarePen)
                        .label("New thread")
                        .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.new_thread(cx)))),
                ),
            )
            .into_any_element()
    }
}

impl Render for Basecamp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("Basecamp");
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let review: Vec<Thread> = ws.ready_for_review().into_iter().cloned().collect();
        let unread = review.iter().any(Thread::is_unseen);
        // Room for the columns side by side, and for three tiles in a row.
        let side = if ws.sidebar_collapsed { 16. } else { crate::root::SIDEBAR_WIDTH + 8. };
        let room = window.viewport_size().width.as_f32() - side;
        let tiles_per_row = if room >= 1080. { 3 } else { 2 };
        let recap = self.recap.clone();
        let p = if recap.is_some() { self.progress(window, cx) } else { 0. };
        let right = match recap.as_deref() {
            Some(r) if !r.is_empty() => self.trek(r, p, tiles_per_row, cx),
            Some(_) => self.empty(cx),
            None => div().text_size(px(13.)).text_color(theme.muted_foreground).child("Reading the trail…").into_any_element(),
        };
        let columns = if review.is_empty() && recap.as_deref().is_some_and(Recap::is_empty) {
            // A fresh start: just the invitation, with no empty list beside it.
            div().max_w(px(560.)).child(right).into_any_element()
        } else {
            div()
                .flex()
                .flex_wrap()
                .items_start()
                .gap_x(px(40.))
                .gap_y(px(32.))
                .child(div().w(px(300.)).flex_none().child(self.review(&review, cx)))
                .child(div().flex_1().min_w(px(420.)).child(right))
                .into_any_element()
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
                    .max_w(px(1120.))
                    .mx_auto()
                    .px(px(40.))
                    .pt(px(28.))
                    .pb(px(48.))
                    .gap(px(28.))
                    .child(self.header(unread, cx))
                    .child(columns),
            )
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

/// A row of "Ready for review": status, title, then agent, diff stat and project.
fn review_row(t: &Thread, project: Option<String>, waiting: Option<Waiting>, now: i64, cx: &mut Context<Basecamp>) -> AnyElement {
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
        .items_start()
        .gap(px(10.))
        .px(px(10.))
        .py(px(8.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|s| s.bg(theme.list_hover))
        .child(div().pt(px(2.)).child(icon.size(px(15.)).text_color(color)))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(4.))
                .child(div().min_w_0().truncate().text_size(px(13.5)).child(t.title.clone()))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(ui::agent_logo(&t.agent, px(12.), cx))
                        .when(t.additions > 0 || t.deletions > 0, |el| {
                            el.child(div().text_color(palette::emerald(cx)).child(format!("+{}", t.additions)))
                                .child(div().text_color(palette::red(cx)).child(format!("−{}", t.deletions)))
                        })
                        .when_some(project, |el, p| el.child(div().min_w_0().truncate().child(p))),
                ),
        )
        .child(
            div().id(SharedString::from(format!("review-{key}-{}", t.id))).test_support().text_size(px(12.)).flex_none().map(|el| match status {
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

/// A stat tile: a quiet label, the figure, a note under it. No card: tiles sit between hairlines.
fn tile(label: impl Into<SharedString>, figure: impl IntoElement, note: impl Into<SharedString>, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .min_w_0()
        .px(px(16.))
        .py(px(16.))
        .gap(px(8.))
        .child(div().text_size(px(12.)).text_color(muted).truncate().child(label.into()))
        .child(figure)
        // Up to two lines: a narrow tile would otherwise cut a note's tail, often its caveat.
        .child(div().text_size(px(12.)).text_color(muted).line_clamp(2).text_ellipsis().child(note.into()))
        .into_any_element()
}

/// The narrative as wrapping text, its projects and models as inline badges. Punctuation stays
/// with the word (or badge) before it.
fn narrative(spans: &[Span], p: f32, workspace: &Entity<Workspace>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let quiet = theme.foreground.opacity(0.62);
    let strong = theme.foreground;
    let ws = workspace.read(cx);
    // Each group wraps as one: a word, or a badge and the punctuation after it.
    let mut groups: Vec<Vec<AnyElement>> = vec![];
    let mut space_before = true;
    let push = |groups: &mut Vec<Vec<AnyElement>>, el: AnyElement, new_word: bool| match groups.last_mut() {
        Some(g) if !new_word => g.push(el),
        _ => groups.push(vec![el]),
    };
    for span in spans {
        match span {
            Span::Text(text) => {
                for (i, word) in text.split_whitespace().enumerate() {
                    let new_word = i > 0 || space_before || text.starts_with(char::is_whitespace);
                    push(&mut groups, div().text_color(quiet).child(word.to_string()).into_any_element(), new_word);
                }
                space_before = text.ends_with(char::is_whitespace);
            }
            Span::Strong(text) => {
                push(&mut groups, div().text_color(strong).font_medium().child(tween(text, p)).into_any_element(), space_before);
                space_before = false;
            }
            Span::Project(name) => {
                let look = ws.projects.iter().find(|pr| &pr.name == name).map(|pr| ws.project_look(&pr.path)).unwrap_or_default();
                let chip = h_flex().gap(px(6.)).child(ui::project_badge(name, &look, cx)).child(div().text_color(strong).font_medium().child(name.clone()));
                push(&mut groups, chip.into_any_element(), space_before);
                space_before = false;
            }
            Span::Model { agent, label } => {
                let chip = h_flex().gap(px(6.)).child(ui::agent_logo(agent, px(16.), cx)).child(div().text_color(strong).font_medium().child(label.clone()));
                push(&mut groups, chip.into_any_element(), space_before);
                space_before = false;
            }
        }
    }
    div()
        .id("basecamp-narrative")
        .test_support()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(5.))
        .gap_y(px(4.))
        .max_w(px(680.))
        .text_size(px(17.))
        .line_height(px(28.))
        .children(groups.into_iter().map(|g| h_flex().children(g)))
        .into_any_element()
}


/// The line over the profile: the hovered stretch's numbers, else where the summit was.
fn profile_line(recap: &Recap, hovered: Option<usize>) -> String {
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

/// `text` with every number in it scaled by `p` (0..1), decimals kept: the count-up.
fn tween(text: &str, p: f32) -> String {
    if p >= 1. {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if !c.is_ascii_digit() {
            out.push(c);
            continue;
        }
        let mut number = c.to_string();
        while let Some(&d) = chars.peek() {
            let decimal_point = d == '.' && !number.contains('.');
            if d.is_ascii_digit() || decimal_point {
                number.push(d);
                chars.next();
            } else {
                break;
            }
        }
        // A trailing dot ends a sentence rather than starting decimals.
        let trailing_dot = number.ends_with('.');
        let digits = number.trim_end_matches('.');
        let decimals = digits.split_once('.').map_or(0, |(_, d)| d.len());
        let value = digits.parse::<f64>().unwrap_or(0.) * p as f64;
        if decimals == 0 {
            out.push_str(&(value.round() as i64).to_string());
        } else {
            out.push_str(&format!("{value:.decimals$}"));
        }
        if trailing_dot {
            out.push('.');
        }
    }
    out
}

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

/// The range as a sentence ends with it: "Nothing failed today", "… so far".
fn during(range: Range) -> &'static str {
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
fn stretch_label(recap: &Recap, i: usize) -> String {
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
fn summit_label(recap: &Recap, i: usize) -> String {
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
fn ticks(w: &basecamp::Window) -> Vec<(f32, String)> {
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

/// What the elevation profile draws, worked out once per render.
struct Profile {
    /// Height of each stretch, 0..1 of the highest (already scaled by the count-up).
    heights: Vec<f32>,
    /// Where "now" is across the window, 0..1.
    now: f32,
    peak: Option<usize>,
    hovered: Option<usize>,
    ticks: Vec<(f32, String)>,
    land: Hsla,
    ridge: Hsla,
    trail: Hsla,
    label: Hsla,
    flag: Hsla,
}

impl Profile {
    fn of(recap: &Recap, p: f32, hovered: Option<usize>, cx: &App) -> Profile {
        let n = recap.buckets.len();
        let max = (0..n).map(|i| recap.elevation(i)).fold(0., f32::max);
        let heights = (0..n).map(|i| if max > 0. { recap.elevation(i) / max * p } else { 0. }).collect();
        Profile { heights, peak: recap.peak(), hovered, ..Profile::flat(Some(recap), cx) }
    }

    /// A trail with no climb yet: the empty state's.
    fn flat(recap: Option<&Recap>, cx: &App) -> Profile {
        let theme = cx.theme();
        let (now, ticks) = match recap {
            Some(r) => {
                let span = (r.window.end - r.window.start).max(1) as f32;
                (((r.now - r.window.start) as f32 / span).clamp(0., 1.), ticks(&r.window))
            }
            None => (0.5, vec![]),
        };
        Profile {
            heights: vec![],
            now,
            peak: None,
            hovered: None,
            ticks,
            land: theme.foreground,
            ridge: theme.foreground.opacity(0.42),
            trail: theme.foreground.opacity(0.16),
            label: theme.muted_foreground,
            flag: palette::ember(cx),
        }
    }

    fn paint(&self, b: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let (left, width) = (b.origin.x.as_f32(), b.size.width.as_f32());
        let top = b.origin.y.as_f32() + PROFILE_TOP;
        let height = (b.size.height.as_f32() - PROFILE_TOP - PROFILE_AXIS).max(8.);
        let base = top + height;
        let n = self.heights.len();
        let now_x = left + self.now * width;
        let at = |x: f32, y: f32| point(px(x), px(y));
        let x_of = |i: usize| left + (i as f32 + 0.5) / n.max(1) as f32 * width;
        let y_of = |h: f32| base - h * (height - 2.);
        // The climb so far: every stretch whose middle is behind us, then where we stand now.
        let mut pts: Vec<(f32, f32)> = vec![(left, base)];
        if n > 0 {
            let current = ((self.now * n as f32) as usize).min(n - 1);
            for i in 0..current {
                pts.push((x_of(i), y_of(self.heights[i])));
            }
            pts.push((now_x, y_of(self.heights[current])));
        } else {
            pts.push((now_x, base));
        }
        let ground = pts.last().map_or(base, |p| p.1);
        // Catmull-Rom through the points, as cubic Béziers; kept from dipping below the ground.
        let curve = |path: &mut PathBuilder| {
            for k in 0..pts.len() - 1 {
                let p0 = pts[k.saturating_sub(1)];
                let (p1, p2) = (pts[k], pts[k + 1]);
                let p3 = pts[(k + 2).min(pts.len() - 1)];
                let c1 = (p1.0 + (p2.0 - p0.0) / 6., (p1.1 + (p2.1 - p0.1) / 6.).min(base));
                let c2 = (p2.0 - (p3.0 - p1.0) / 6., (p2.1 - (p3.1 - p1.1) / 6.).min(base));
                path.cubic_bezier_to(at(p2.0, p2.1), at(c1.0, c1.1), at(c2.0, c2.1));
            }
        };
        if pts.len() > 1 && pts.iter().any(|p| p.1 < base - 0.5) {
            let mut land = PathBuilder::fill();
            land.move_to(at(left, base));
            curve(&mut land);
            land.line_to(at(now_x, base));
            land.close();
            if let Ok(path) = land.build() {
                window.paint_path(path, linear_gradient(180., linear_color_stop(self.land.opacity(0.18), 0.), linear_color_stop(self.land.opacity(0.02), 1.)));
            }
            let mut ridge = PathBuilder::stroke(px(1.5));
            ridge.move_to(at(left, base));
            curve(&mut ridge);
            if let Ok(path) = ridge.build() {
                window.paint_path(path, self.ridge);
            }
        }
        // The ground walked, and the trail ahead in dots.
        window.paint_quad(fill(Bounds::new(at(left, base), size(px((now_x - left).max(0.)), px(1.))), self.trail));
        let mut x = now_x + 10.;
        while x < left + width - 2. {
            window.paint_quad(fill(Bounds::new(at(x, base - 0.5), size(px(2.), px(2.))), self.trail).corner_radii(px(1.)));
            x += 7.;
        }
        if let Some(h) = self.hovered.filter(|h| *h < n) {
            let x = x_of(h);
            window.paint_quad(fill(Bounds::new(at(x - 0.5, top - 4.), size(px(1.), px(base - top + 4.))), self.trail));
            if x <= now_x + 1. {
                let y = y_of(self.heights[h]);
                window.paint_quad(fill(Bounds::new(at(x - 3., y - 3.), size(px(6.), px(6.))), self.ridge.opacity(0.9)).corner_radii(px(3.)));
            }
        }
        // The summit flag: a pole and an ember pennant.
        if let Some(i) = self.peak.filter(|i| *i < n && self.heights[*i] > 0.) {
            let (x, y) = (x_of(i).min(now_x), y_of(self.heights[i]));
            window.paint_quad(fill(Bounds::new(at(x - 0.5, y - 18.), size(px(1.), px(18.))), self.ridge));
            let mut pennant = PathBuilder::fill();
            pennant.move_to(at(x + 0.5, y - 18.));
            pennant.line_to(at(x + 10.5, y - 14.5));
            pennant.line_to(at(x + 0.5, y - 11.));
            pennant.close();
            if let Ok(path) = pennant.build() {
                window.paint_path(path, self.flag);
            }
        }
        // The hiker, where the day stands.
        let (w, _) = crate::mascot::size_at(HIKER_CELL);
        crate::mascot::stand(now_x - w / 2., ground + 1., HIKER_CELL, window);
        // Axis labels.
        let font = window.text_style().font();
        for (frac, label) in &self.ticks {
            let run = TextRun { len: label.len(), font: font.clone(), color: self.label, background_color: None, underline: None, strikethrough: None };
            let line = window.text_system().shape_line(label.clone().into(), px(11.), &[run], None);
            let x = (left + frac * width - line.width.as_f32() / 2.).clamp(left, left + width - line.width.as_f32());
            let _ = line.paint(at(x, base + 6.), px(14.), TextAlign::Left, None, window, cx);
        }
    }
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

/// The tokens tile's sparkline: tokens used so far, climbing through the window.
struct Sparkline {
    points: Vec<f32>,
    now: f32,
    color: Hsla,
}

impl Sparkline {
    fn of(recap: &Recap, p: f32, cx: &App) -> Sparkline {
        let total: u64 = recap.buckets.iter().map(|b| b.tokens).sum();
        let mut sum = 0;
        let points = recap
            .buckets
            .iter()
            .map(|b| {
                sum += b.tokens;
                if total > 0 { sum as f32 / total as f32 * p } else { 0. }
            })
            .collect();
        let span = (recap.window.end - recap.window.start).max(1) as f32;
        Sparkline { points, now: ((recap.now - recap.window.start) as f32 / span).clamp(0., 1.), color: cx.theme().foreground.opacity(0.55) }
    }

    fn paint(&self, b: Bounds<Pixels>, window: &mut Window) {
        let n = self.points.len();
        if n == 0 {
            return;
        }
        let (left, top, w, h) = (b.origin.x.as_f32(), b.origin.y.as_f32() + 1., b.size.width.as_f32(), b.size.height.as_f32() - 2.);
        let shown = ((self.now * n as f32).ceil() as usize).clamp(1, n);
        let mut line = PathBuilder::stroke(px(1.5));
        line.move_to(point(px(left), px(top + h)));
        for (i, v) in self.points.iter().take(shown).enumerate() {
            line.line_to(point(px(left + (i as f32 + 1.) / shown as f32 * w), px(top + h - v * h)));
        }
        if let Ok(path) = line.build() {
            window.paint_path(path, self.color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DAY, stretch_label, summit_label, ticks, tokens_note, tween};
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

    #[test]
    fn numbers_count_up_in_place() {
        assert_eq!(tween("18 prompts", 0.5), "9 prompts");
        assert_eq!(tween("29.6M tokens", 0.5), "14.8M tokens");
        assert_eq!(tween("1h 2m", 0.), "0h 0m");
        assert_eq!(tween("77%", 1.), "77%");
        assert_eq!(tween("9 of 14 turns.", 0.5), "5 of 7 turns.");
        assert_eq!(tween("under a minute", 0.3), "under a minute");
    }
}
