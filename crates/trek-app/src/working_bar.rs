//! The bar above the composer while an agent works: the trail word, elapsed time, sub-agents out,
//! and the hiker. It's a view of its own with its own ticker, so its animation frames re-render
//! only this bar (and the window around it), never the transcript, sidebar or composer.

use crate::workspace::{Route, Workspace};
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::{Duration, Instant};
use trek_core::RunState;

const COLUMN: f32 = 760.;

/// The bar's height while it shows: the trail plus the gap above the composer.
pub const HEIGHT: f32 = crate::mascot::HEIGHT + 6.;

pub struct WorkingBar {
    workspace: Entity<Workspace>,
    /// What's on screen, to skip re-rendering on workspace changes that don't touch the bar.
    shown: Option<Shown>,
    /// The window is frontmost (or `TREK_FORCE_ACTIVE`): the animation runs at full rate.
    active: bool,
    _ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// The bar's inputs besides the clock.
#[derive(Clone, PartialEq)]
struct Shown {
    thread: String,
    started: Option<Instant>,
    agents: usize,
    still: bool,
}

impl WorkingBar {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| this.sync(cx)),
            cx.observe_window_activation(window, |this, window, cx| {
                this.active = window.is_window_active() || crate::mascot::force_active();
                // Restart the ticker at the new rate rather than after its current wait.
                this._ticker = None;
                this.sync(cx);
            }),
        ];
        let mut this = Self {
            workspace,
            shown: None,
            active: window.is_window_active() || crate::mascot::force_active(),
            _ticker: None,
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    /// The bar shows while the thread on screen works and isn't waiting on the user (its cards
    /// take this spot then).
    fn read(&self, cx: &App) -> Option<Shown> {
        let ws = self.workspace.read(cx);
        let Route::Thread(id) = &ws.route else { return None };
        let live = ws.live.get(id)?;
        (ws.thread(id)?.run_state == RunState::Working && live.permissions.is_empty()).then(|| Shown {
            thread: id.clone(),
            started: live.turn_started,
            agents: live.active_tasks().max(live.background),
            still: ws.settings.appearance.reduce_motion || !self.active,
        })
    }

    pub fn visible(&self) -> bool {
        self.shown.is_some()
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let shown = self.read(cx);
        let ticking = shown.as_ref().is_some_and(|s| s.started.is_some());
        if ticking && self._ticker.is_none() {
            // The hiker and the word sweep at mascot::FPS while the window is in front; otherwise
            // only the elapsed time moves, once a second.
            self._ticker = Some(cx.spawn(async move |this, cx| loop {
                let Ok(fast) = this.update(cx, |this, cx| {
                    cx.notify();
                    this.shown.as_ref().is_some_and(|s| !s.still)
                }) else {
                    break;
                };
                let wait = if fast { Duration::from_millis(1000 / crate::mascot::FPS) } else { Duration::from_secs(1) };
                cx.background_executor().timer(wait).await;
            }));
        } else if !ticking {
            self._ticker = None;
        }
        if shown != self.shown {
            self.shown = shown;
            cx.notify();
        }
    }
}

impl Render for WorkingBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("WorkingBar");
        let Some(shown) = self.shown.clone() else { return div().into_any_element() };
        let theme = cx.theme();
        let elapsed = shown.started.map(|t| t.elapsed());
        let word = crate::mascot::word(&shown.thread, elapsed.map(|d| d.as_secs()).unwrap_or(0));
        let clock = elapsed.map(|d| d.as_secs_f32()).unwrap_or(0.);
        let muted = theme.muted_foreground.opacity(0.8);
        h_flex()
            .id("working-bar")
            .test_support()
            .w_full()
            .h(px(HEIGHT))
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
                            // Fixed width so the trail doesn't jump when the word changes.
                            .w(px(330.))
                            .pb(px(4.))
                            .gap(px(8.))
                            .text_size(px(13.))
                            .child(crate::mascot::word_label(word, clock, shown.still, cx))
                            .when_some(elapsed.map(crate::time::elapsed), |el, t| el.child(div().text_color(muted).child(t)))
                            .when(shown.agents > 0, |el| el.child(div().text_color(muted).child(agents_out(shown.agents)))),
                    )
                    .child(div().flex_1().min_w_0().child(crate::mascot::trail(clock, shown.still, cx))),
            )
            .into_any_element()
    }
}

/// "· 1 agent out", "· 3 agents out".
pub fn agents_out(n: usize) -> String {
    if n == 1 { "· 1 agent out".into() } else { format!("· {n} agents out") }
}

#[cfg(test)]
impl WorkingBar {
    /// The label as shown: "Trailblazing… 4s · 2 agents out", or `None` when hidden.
    pub(crate) fn label(&self) -> Option<String> {
        let s = self.shown.as_ref()?;
        let elapsed = s.started.map(|t| t.elapsed());
        let mut out = format!("{}…", crate::mascot::word(&s.thread, elapsed.map(|d| d.as_secs()).unwrap_or(0)));
        if let Some(e) = elapsed {
            out.push(' ');
            out.push_str(&crate::time::elapsed(e));
        }
        if s.agents > 0 {
            out.push(' ');
            out.push_str(&agents_out(s.agents));
        }
        Some(out)
    }
}
