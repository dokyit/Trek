//! What a thread's agent left running in the background, above the composer: a dev server, a
//! browser session, a test watcher, a `Monitor`. One quiet row (each task's own row when opened):
//! what it is, the last line it printed, how long it has run, and Stop where the agent can stop
//! it. Clicking a row shows the end of its output in a popover. The agent's own sub-agents aren't
//! here: they have rows in the transcript, and the thread waits on them.
//!
//! It can be put away to one slim "Background" line, per thread, for when the user knows.
//!
//! A view of its own, cached at its height: its once-every-two-seconds tick (the clocks, and
//! reading the tasks' output from the agent) redraws only the strip.

use crate::workspace::{Scope, Workspace, WorkspaceEvent};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Selectable, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::{Duration, Instant};
use trek_agents::BackgroundKind;

const COLUMN: f32 = 760.;
const ROW: f32 = 28.;
/// The slim line the strip folds to when put away.
const SLIM: f32 = 24.;
/// Space under the rows, above the composer.
const PAD: f32 = 4.;
/// How often the strip reads its tasks' output and moves its clocks on.
const TICK: Duration = Duration::from_secs(2);
/// The most of a task's output the popover shows: its last lines.
const OUTPUT_LINES: usize = 400;

pub struct BackgroundStrip {
    workspace: Entity<Workspace>,
    scope: Scope,
    shown: Option<Shown>,
    /// A row per task, rather than the first with a count.
    expanded: bool,
    /// Threads whose strip is put away (one slim line), as the user left it.
    hidden: std::collections::HashSet<String>,
    /// The task whose output is open in a popover.
    output: Option<String>,
    /// The window is in front: the strip reads output and ticks only then.
    active: bool,
    /// `TREK_OPEN_BACKGROUND=1` (design review): the rows open, and the first task's output.
    review: bool,
    _ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, PartialEq)]
struct Shown {
    thread: String,
    agent: trek_core::AgentId,
    rows: Vec<TaskRow>,
}

#[derive(Clone, PartialEq)]
struct TaskRow {
    id: String,
    kind: BackgroundKind,
    title: String,
    /// The last line it printed, as last read.
    line: Option<String>,
    /// Its output has been read (the popover reads it from the workspace, `Background::output`).
    read: bool,
    output_rev: u64,
    started: Instant,
    readable: bool,
    stoppable: bool,
    stopping: bool,
}

impl BackgroundStrip {
    pub fn new(workspace: Entity<Workspace>, scope: Scope, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| this.sync(cx)),
            cx.subscribe(&workspace, |this, ws, event: &WorkspaceEvent, cx| {
                if let WorkspaceEvent::Background { id } = event
                    && ws.read(cx).thread_id_in(&this.scope) == Some(id.as_str())
                {
                    this.sync(cx);
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                this.active = window.is_window_active() || crate::mascot::force_active();
                this._ticker = None;
                this.sync(cx);
            }),
        ];
        let mut this = Self {
            workspace,
            scope,
            shown: None,
            expanded: false,
            hidden: std::collections::HashSet::new(),
            output: None,
            active: window.is_window_active() || crate::mascot::force_active(),
            review: std::env::var("TREK_OPEN_BACKGROUND").is_ok_and(|v| v == "1"),
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
        let rows: Vec<TaskRow> = live
            .background_work()
            .map(|b| TaskRow {
                id: b.task.id.clone(),
                kind: b.task.kind,
                title: b.task.title.clone(),
                line: b.last_line().map(str::to_string),
                read: b.output.is_some(),
                output_rev: b.output_rev,
                started: b.started,
                readable: b.task.readable,
                stoppable: b.task.stoppable,
                stopping: b.stopping,
            })
            .collect();
        (!rows.is_empty()).then(|| Shown { thread: id.to_string(), agent: ws.thread(id).map(|t| t.agent.clone()).unwrap_or(trek_core::AgentId::ClaudeCode), rows })
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let shown = self.read(cx);
        // Tasks just started have their output read at once, not at the next tick.
        let new: Vec<(String, String)> = match (&shown, &self.shown) {
            (Some(s), old) => s
                .rows
                .iter()
                .filter(|r| r.readable && old.as_ref().is_none_or(|o| o.thread != s.thread || !o.rows.iter().any(|x| x.id == r.id)))
                .map(|r| (s.thread.clone(), r.id.clone()))
                .collect(),
            (None, _) => vec![],
        };
        if shown.as_ref().map(|s| &s.thread) != self.shown.as_ref().map(|s| &s.thread) {
            self.expanded = false;
            self.output = None;
        }
        if let Some(open) = &self.output
            && shown.as_ref().is_none_or(|s| !s.rows.iter().any(|r| r.id == *open))
        {
            self.output = None;
            self.overlay(false, cx);
        }
        if self.review && self.output.is_none() {
            self.expanded = true;
            self.output = shown.as_ref().and_then(|s| s.rows.first()).map(|r| r.id.clone());
        }
        if shown != self.shown {
            self.shown = shown;
            cx.notify();
        }
        if !new.is_empty() {
            self.workspace.update(cx, |ws, _| {
                for (thread, task) in new {
                    ws.read_background(&thread, &task);
                }
            });
        }
        self.tick(cx);
    }

    /// Tick while there's something on show (not put away) and the window is in front.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.shown.as_ref().is_none_or(|s| self.hidden.contains(&s.thread)) || !self.active {
            self._ticker = None;
            return;
        }
        if self._ticker.is_some() {
            return;
        }
        self._ticker = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            let Ok(()) = this.update(cx, |this, cx| {
                cx.notify();
                this.poll(cx);
            }) else {
                break;
            };
        }));
    }

    /// Ask the agent for the end of each task's output.
    fn poll(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.shown else { return };
        let (thread, tasks): (String, Vec<String>) = (s.thread.clone(), s.rows.iter().filter(|r| r.readable).map(|r| r.id.clone()).collect());
        self.workspace.update(cx, |ws, _| {
            for task in tasks {
                ws.read_background(&thread, &task);
            }
        });
    }

    /// Its height as laid out: a row (or a row per task, opened), and the space under it.
    pub fn height(&self) -> f32 {
        match &self.shown {
            Some(s) if self.hidden.contains(&s.thread) => SLIM + PAD,
            Some(s) => (if self.expanded { s.rows.len() } else { 1 }) as f32 * ROW + PAD,
            None => 0.,
        }
    }

    /// Put the strip away to its slim line, or bring it back.
    fn set_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        let Some(thread) = self.shown.as_ref().map(|s| s.thread.clone()) else { return };
        if hidden {
            self.hidden.insert(thread);
            if self.output.is_some() {
                self.set_output(None, cx);
            }
        } else {
            self.hidden.remove(&thread);
            self.poll(cx);
        }
        self.tick(cx);
        cx.notify();
    }

    /// The strip put away: "Background · 2 running", which brings it back.
    fn slim(&self, n: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        h_flex()
            .id("bg-show")
            .test_support()
            .w_full()
            .h(px(SLIM))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(7.))
            .text_size(px(12.5))
            .text_color(muted)
            .cursor_pointer()
            .hover(|s| s.bg(theme.foreground.opacity(0.05)).text_color(theme.foreground))
            .child(Icon::new(crate::assets::Lucide::Activity).size(px(14.)))
            .child("Background")
            .child(div().text_xs().child(format!("· {n} running")))
            .child(div().flex_1())
            .child(Icon::new(IconName::ChevronUp).xsmall())
            .on_click(cx.listener(|this, _, _, cx| this.set_hidden(false, cx)))
            .into_any_element()
    }

    fn set_output(&mut self, task: Option<String>, cx: &mut Context<Self>) {
        let open = task.is_some();
        self.output = task;
        self.overlay(open, cx);
        if open {
            self.poll(cx);
        }
        cx.notify();
    }

    /// A popover is open over the main window: native views (the browser) hide so they don't
    /// cover it.
    fn overlay(&self, open: bool, cx: &mut Context<Self>) {
        if self.scope != Scope::Main {
            return;
        }
        self.workspace.update(cx, |ws, cx| {
            if ws.overlay_open != open {
                ws.overlay_open = open;
                cx.notify();
            }
        });
    }

    fn stop(&mut self, task: &str, cx: &mut Context<Self>) {
        let Some(thread) = self.shown.as_ref().map(|s| s.thread.clone()) else { return };
        self.workspace.update(cx, |ws, cx| ws.stop_background(&thread, task, cx));
    }

    /// What a row shows of a task: its icon, what it is, its last line, how long it has run.
    fn row(&self, ix: usize, r: &TaskRow, more: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let open = self.output.as_deref() == Some(r.id.as_str());
        let id = r.id.clone();
        let stop = r.stoppable.then(|| {
            let id = id.clone();
            Button::new(("bg-stop", ix))
                .ghost()
                .xsmall()
                .icon(Icon::new(crate::assets::Lucide::Square).text_color(muted))
                .tooltip(if r.stopping { "Stopping…" } else { "Stop it" })
                .disabled(r.stopping)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.stop(&id, cx);
                }))
        });
        // The first row opens the rest (or folds them back).
        let toggle = (ix == 0 && more > 0).then(|| {
            h_flex()
                .id("bg-toggle")
                .test_support()
                .flex_none()
                .gap(px(4.))
                .px(px(6.))
                .h(px(20.))
                .rounded(px(5.))
                .text_xs()
                .text_color(muted)
                .cursor_pointer()
                .hover(|s| s.bg(theme.foreground.opacity(0.07)).text_color(theme.foreground))
                .when(!self.expanded, |el| el.child(format!("+{more}")))
                .child(Icon::new(if self.expanded { IconName::ChevronDown } else { IconName::ChevronUp }).xsmall())
                .on_click(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.expanded = !this.expanded;
                    cx.notify();
                }))
        });
        // The first row puts the strip away.
        let hide = (ix == 0).then(|| {
            Button::new("bg-hide")
                .ghost()
                .xsmall()
                .icon(Icon::new(crate::assets::Lucide::ChevronsDownUp).text_color(muted))
                .tooltip("Put away")
                .on_click(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.set_hidden(true, cx);
                }))
        });
        let line = r.line.clone().unwrap_or_else(|| if r.readable { String::new() } else { "running".into() });
        let trigger = Trigger {
            open,
            el: h_flex()
                .id(("bg-task", ix))
                .test_support()
                .w_full()
                .h(px(ROW))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(7.))
                .text_size(px(12.5))
                .cursor_pointer()
                .when(open, |el| el.bg(theme.foreground.opacity(0.07)))
                .hover(|s| s.bg(theme.foreground.opacity(0.05)))
                .child(kind_icon(r.kind).size(px(14.)).text_color(muted))
                .child(div().flex_none().max_w(relative(0.42)).truncate().font_family(theme.mono_font_family.clone()).text_color(theme.foreground.opacity(0.85)).child(r.title.clone()))
                .child(div().flex_1().min_w_0().truncate().font_family(theme.mono_font_family.clone()).text_xs().text_color(muted).child(line))
                .child(div().flex_none().text_xs().text_color(muted).child(crate::time::elapsed(r.started.elapsed())))
                .children(stop)
                .children(toggle)
                .children(hide)
                .into_any_element(),
        };
        let entity = cx.entity();
        let task = r.id.clone();
        Popover::new(("bg-output", ix))
            .anchor(Anchor::BottomLeft)
            .appearance(false)
            .open(open)
            .on_open_change(cx.listener(move |this, open: &bool, _, cx| this.set_output(open.then(|| task.clone()), cx)))
            .trigger(trigger)
            .content(move |_, _, cx| entity.update(cx, |this, cx| this.output_card(cx)))
            .into_any_element()
    }

    /// The popover over a task: what it is, the end of what it printed, and Stop.
    fn output_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some((r, agent, thread)) = self.shown.as_ref().and_then(|s| Some((s.rows.iter().find(|r| Some(&r.id) == self.output.as_ref())?.clone(), s.agent.clone(), s.thread.clone()))) else {
            return div().into_any_element();
        };
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let text = r.read.then(|| {
            let ws = self.workspace.read(cx);
            let output = ws.live.get(&thread).and_then(|l| l.background.iter().find(|b| b.task.id == r.id)).and_then(|b| b.output.as_deref()).unwrap_or_default();
            let lines: Vec<&str> = output.lines().collect();
            lines[lines.len().saturating_sub(OUTPUT_LINES)..].join("\n")
        });
        let id = r.id.clone();
        let stop = r.stoppable.then(|| {
            Button::new("bg-output-stop")
                .small()
                .outline()
                .icon(Icon::new(crate::assets::Lucide::Square))
                .label(if r.stopping { "Stopping…" } else { "Stop" })
                .disabled(r.stopping)
                .on_click(cx.listener(move |this, _, _, cx| this.stop(&id, cx)))
        });
        let note = if !r.readable {
            format!("{} doesn't share this task's output while it runs.", agent.display_name())
        } else if text.as_deref().is_some_and(|t| !t.trim().is_empty()) {
            "The end of its output, read every few seconds.".to_string()
        } else if text.is_some() {
            "Nothing printed yet.".to_string()
        } else {
            "Reading its output…".to_string()
        };
        crate::ui::menu_surface(cx)
            .id("bg-output-card")
            .test_support()
            .w(px(560.))
            .p(px(12.))
            .gap(px(10.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .child(kind_icon(r.kind).size(px(15.)).text_color(muted))
                    .child(div().min_w_0().flex_1().truncate().font_family(theme.mono_font_family.clone()).child(r.title.clone()))
                    .child(div().flex_none().text_xs().text_color(muted).child(format!("{} · {}", kind_label(r.kind), crate::time::elapsed(r.started.elapsed()))))
                    .children(stop),
            )
            .when_some(text.filter(|t| !t.trim().is_empty()), |el, t| {
                el.child(
                    div()
                        .id("bg-output-text")
                        .test_support()
                        .max_h(px(300.))
                        .overflow_y_scroll()
                        .p(px(10.))
                        .rounded(px(8.))
                        .bg(theme.foreground.opacity(0.045))
                        .font_family(theme.mono_font_family.clone())
                        .text_xs()
                        .line_height(relative(1.45))
                        .child(t),
                )
            })
            .child(div().text_xs().text_color(muted).child(note))
            .into_any_element()
    }
}

/// The icon a task's kind wears.
pub fn kind_icon(kind: BackgroundKind) -> Icon {
    match kind {
        BackgroundKind::Shell => Icon::new(crate::assets::Lucide::Terminal),
        BackgroundKind::Monitor => Icon::new(crate::assets::Lucide::Radar),
        BackgroundKind::Agent => Icon::new(crate::assets::Lucide::Users),
        BackgroundKind::Other => Icon::new(crate::assets::Lucide::Activity),
    }
}

fn kind_label(kind: BackgroundKind) -> &'static str {
    match kind {
        BackgroundKind::Shell => "Command",
        BackgroundKind::Monitor => "Watching",
        BackgroundKind::Agent => "Sub-agent",
        BackgroundKind::Other => "Task",
    }
}

/// A row as a popover's trigger (which must say whether it's open).
#[derive(IntoElement)]
struct Trigger {
    open: bool,
    el: AnyElement,
}

impl Selectable for Trigger {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }
    fn is_selected(&self) -> bool {
        self.open
    }
}

impl RenderOnce for Trigger {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.el
    }
}

impl Render for BackgroundStrip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("BackgroundStrip");
        let Some(shown) = self.shown.clone() else { return div().into_any_element() };
        let more = shown.rows.len() - 1;
        let rows: Vec<AnyElement> = if self.hidden.contains(&shown.thread) {
            vec![self.slim(shown.rows.len(), cx)]
        } else {
            shown.rows.iter().enumerate().take(if self.expanded { shown.rows.len() } else { 1 }).map(|(ix, r)| self.row(ix, r, more, cx)).collect()
        };
        h_flex()
            .id("background-strip")
            .test_support()
            .w_full()
            .justify_center()
            .px_6()
            .pb(px(PAD))
            .child(v_flex().w_full().max_w(px(COLUMN)).children(rows))
            .into_any_element()
    }
}

/// The strip as a window lays it out: cached at its height (zero while there's nothing to show).
pub fn cached(strip: &Entity<BackgroundStrip>, cx: &App) -> AnyElement {
    let h = strip.read(cx).height();
    strip.clone().cached(StyleRefinement::default().w_full().flex_none().h(px(h))).into_any_element()
}

#[cfg(test)]
impl BackgroundStrip {
    /// The task whose output is open.
    pub(crate) fn output_open(&self) -> Option<String> {
        self.output.clone()
    }

    /// The rows as shown: "npm run dev — ➜ Local: http://localhost:5173/", with "(stop)" where
    /// it can be stopped.
    pub(crate) fn rows(&self) -> Vec<String> {
        let Some(s) = &self.shown else { return vec![] };
        if self.hidden.contains(&s.thread) {
            return vec![format!("Background · {} running", s.rows.len())];
        }
        s.rows
            .iter()
            .take(if self.expanded { s.rows.len() } else { 1 })
            .map(|r| format!("{} — {}{}", r.title, r.line.clone().unwrap_or_default(), if r.stoppable { " (stop)" } else { "" }))
            .collect()
    }
}
