//! `trek:hiker`: the hiker on a dotted trail, walking back and forth while an agent works (any
//! thread's, or the one the place is drawn for) and standing where it stopped when none does, or
//! with Reduce motion. It walks on the app's clock (`motion::now`) at the working bar's pace, so
//! the two are the same walk; it draws only while its place is on screen, and asks for its next
//! frame only while it walks.

use crate::workspace::Workspace;
use gpui_kit::*;
use std::time::{Duration, Instant};

/// The hiker's frame: what a tick waits.
const FRAME: Duration = Duration::from_millis(1000 / trek_mascot::FPS);

/// One placed hiker: where it is on its trail and when it last took a step.
pub struct Hiker {
    ws: Entity<Workspace>,
    /// The thread its place is drawn for; `None`, any thread's work walks it.
    thread: Option<String>,
    walk: trek_mascot::Walk,
    since: Option<Instant>,
    travel: trek_mascot::Travel,
    tick: Option<Task<()>>,
    /// It was walking as last drawn, to draw it again when that changes: nothing asks a hiker
    /// standing still to draw (it has no tick), so the workspace's changes are watched for the
    /// work starting.
    walking: bool,
    _watch: Subscription,
}

impl Hiker {
    fn new(ws: Entity<Workspace>, thread: Option<String>, cx: &mut Context<Self>) -> Self {
        let _watch = cx.observe(&ws, |this, _, cx| {
            if this.walking(cx) != this.walking {
                cx.notify();
            }
        });
        Self {
            ws,
            thread,
            walk: trek_mascot::Walk::default(),
            since: None,
            travel: trek_mascot::Travel::default(),
            tick: None,
            walking: false,
            _watch,
        }
    }

    /// It should be walking: an agent is at work, and motion is allowed.
    fn walking(&self, cx: &App) -> bool {
        self.working(cx) && self.ws.read(cx).motion(cx)
    }

    /// An agent is at work, for this place: its thread's turn, or any turn in the window.
    fn working(&self, cx: &App) -> bool {
        let ws = self.ws.read(cx);
        match &self.thread {
            Some(thread) => ws.turn_running(thread),
            None => ws.any_turn_running(),
        }
    }

    /// Where the hiker is, 0 at the trail's start to 1 at its end (what the capture build and the
    /// tests read).
    pub fn at(&self) -> f32 {
        self.walk.at
    }
}

impl Render for Hiker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let walking = self.walking(cx);
        self.walking = walking;
        if walking {
            let now = crate::motion::now(cx);
            self.since = self.walk.advance(self.since, now, self.travel.get(), false);
            // The next frame, once drawn: a hiker that stops being drawn stops asking.
            self.tick = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(FRAME).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            }));
        } else {
            // Standing: the time it stood isn't walked when it sets off again.
            self.since = None;
            self.tick = None;
        }
        div().w_full().min_w(px(96.)).child(trek_mascot::trail(
            self.walk,
            &self.travel,
            !walking,
            cx,
        ))
    }
}

/// A placed hiker, the one that keeps its walk across frames in `id`'s keyed state.
#[derive(IntoElement)]
pub struct Placed {
    id: ElementId,
    ws: Entity<Workspace>,
    thread: Option<String>,
}

impl Placed {
    pub fn new(id: ElementId, ws: Entity<Workspace>, thread: Option<String>) -> Self {
        Self { id, ws, thread }
    }
}

impl RenderOnce for Placed {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Placed { id, ws, thread } = self;
        window.use_keyed_state(id, cx, move |_, cx| {
            #[cfg(any(test, feature = "test-support"))]
            SHOWN.with(|s| s.borrow_mut().push(cx.weak_entity()));
            Hiker::new(ws, thread, cx)
        })
    }
}

/// How far along their trails the hikers made so far are, in the order they were made (tests).
#[cfg(any(test, feature = "test-support"))]
pub fn walks(cx: &App) -> Vec<f32> {
    SHOWN.with(|s| {
        s.borrow()
            .iter()
            .filter_map(|w| w.upgrade())
            .map(|h| h.read(cx).at())
            .collect()
    })
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static SHOWN: std::cell::RefCell<Vec<WeakEntity<Hiker>>> = const { std::cell::RefCell::new(Vec::new()) };
}
