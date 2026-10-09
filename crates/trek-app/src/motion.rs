//! Motion after motion.dev's (Framer Motion's) ideas, on Trek's terms:
//!
//! - `Spring`: a value that travels to its target on a spring. Retargeted mid-flight it keeps its
//!   position and its velocity, so a quick second ⌘B turns the sidebar round where it is rather
//!   than starting over. Between retargets it's a pure function of time, so any view can read it
//!   (the title bar and the window share the sidebar's).
//! - `Presence`: AnimatePresence. What's going away stays mounted, with what it showed, until it
//!   has gone all the way out.
//! - `flip_scope` / `flip_row`: layout animation (FLIP). A row laid out somewhere else than last
//!   frame is drawn where it was and springs to where it is now.
//!
//! Every caller asks `Workspace::motion` first and passes the answer in: without motion, every
//! value is at its target at once. Frames are asked for only while something moves; at rest none
//! of this costs anything.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

/// Large surfaces (the sidebar, the switch to the editor, the image preview, sheets): critically
/// damped, so nothing that big bounces; most of the way in a quarter second.
pub const SURFACE: SpringConfig = SpringConfig::new(400., 40., 1.);
/// Rows sliding to their new places in a list: a touch quicker, still without overshoot.
pub const ROW: SpringConfig = SpringConfig::new(500., 45., 1.);

/// Settled within this of the target on a 0-to-1 value: past what an eye can tell, and frames
/// stop well before the spring's long tail.
pub const UNIT: f32 = 0.004;
/// The same for values in pixels.
pub const PIXEL: f32 = 0.4;

/// A value on a spring. See the module's notes.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    config: SpringConfig,
    epsilon: f32,
    /// Where it was, and how fast it went, at `at` (the last retarget).
    from: SpringState,
    at: Instant,
    target: f32,
}

impl Spring {
    pub fn new(config: SpringConfig, epsilon: f32, value: f32, now: Instant) -> Self {
        Spring { config, epsilon, from: SpringState { position: value, velocity: 0. }, at: now, target: value }
    }

    /// Where it is and how fast it's going at `now`. Close enough to its target, it's there.
    pub fn state(&self, now: Instant) -> SpringState {
        let rest = SpringState { position: self.target, velocity: 0. };
        if self.from == rest {
            return rest;
        }
        let s = self.config.step(self.from, self.target, now.saturating_duration_since(self.at).as_secs_f32());
        if self.config.is_settled(s, self.target, self.epsilon) { rest } else { s }
    }

    pub fn value(&self, now: Instant) -> f32 {
        self.state(now).position
    }

    pub fn moving(&self, now: Instant) -> bool {
        self.state(now) != SpringState { position: self.target, velocity: 0. }
    }

    /// Head for `target` from wherever it is, as fast as it's already going. Without motion it's
    /// there at once.
    pub fn set(&mut self, target: f32, motion: bool, now: Instant) {
        if !motion {
            self.snap(target, now);
        } else if target != self.target {
            self.from = self.state(now);
            self.at = now;
            self.target = target;
        }
    }

    /// Be at `value`, still.
    pub fn snap(&mut self, value: f32, now: Instant) {
        self.from = SpringState { position: value, velocity: 0. };
        self.at = now;
        self.target = value;
    }

    /// Move by `delta` without losing speed or target: FLIP, when the layout moved under it.
    pub fn shift(&mut self, delta: f32, now: Instant) {
        let mut s = self.state(now);
        s.position += delta;
        self.from = s;
        self.at = now;
    }

    /// Its value at `now`, asking `window` for another frame while it moves.
    pub fn frame(&self, now: Instant, window: &Window) -> f32 {
        let s = self.state(now);
        if s.position != self.target || s.velocity != 0. {
            window.request_animation_frame();
        }
        s.position
    }
}

/// The app's clock: real time in the app, the test scheduler's in tests (`advance_clock`).
pub fn now(cx: &App) -> Instant {
    let now = cx.background_executor().now();
    // Design review (`shots`): `TREK_SLOW_MOTION=<k>` runs Trek's springs k times slower, so a
    // recording catches them mid-flight.
    #[cfg(feature = "shots")]
    {
        static SLOW: std::sync::LazyLock<Option<(f32, Instant)>> = std::sync::LazyLock::new(|| std::env::var("TREK_SLOW_MOTION").ok()?.parse::<f32>().ok().filter(|k| *k > 1.).map(|k| (k, Instant::now())));
        if let Some((k, from)) = *SLOW {
            return from + now.saturating_duration_since(from).div_f32(k);
        }
    }
    now
}

/// AnimatePresence: `T` (what a surface shows) stays mounted while it animates out, and comes
/// back from wherever it is if it's wanted again before it has gone.
pub struct Presence<T> {
    item: Option<T>,
    present: bool,
    spring: Spring,
}

impl<T> Presence<T> {
    pub fn new(config: SpringConfig) -> Self {
        Presence { item: None, present: false, spring: Spring::new(config, UNIT, 0., Instant::now()) }
    }

    /// Mount `item` and bring it in (one on its way out turns round where it is).
    pub fn enter(&mut self, item: T, motion: bool, now: Instant) {
        self.item = Some(item);
        self.present = true;
        self.spring.set(1., motion, now);
    }

    /// Take it out. It stays mounted until it's all the way out (at once without motion).
    pub fn exit(&mut self, motion: bool, now: Instant) {
        if !self.present {
            return;
        }
        self.present = false;
        self.spring.set(0., motion, now);
        if !motion {
            self.item = None;
        }
    }

    /// In, or on its way in: not leaving.
    pub fn is_present(&self) -> bool {
        self.present && self.item.is_some()
    }

    /// Mounted: in, or still on its way out.
    #[cfg(test)]
    pub fn is_mounted(&self) -> bool {
        self.item.is_some()
    }

    pub fn item(&self) -> Option<&T> {
        self.item.as_ref()
    }

    pub fn item_mut(&mut self) -> Option<&mut T> {
        self.item.as_mut()
    }

    /// How far in it is at `now` (0 out, 1 in), read without side effects.
    #[cfg(test)]
    pub fn progress(&self, now: Instant) -> f32 {
        self.spring.value(now).clamp(0., 1.)
    }

    /// For drawing: how far in it is at `now`, or `None` once it has gone all the way out (it
    /// unmounts then). Asks for frames while it moves.
    pub fn sample(&mut self, now: Instant, window: &Window) -> Option<f32> {
        self.item.as_ref()?;
        let moving = self.spring.moving(now);
        if !self.present && !moving {
            self.item = None;
            return None;
        }
        Some(self.spring.frame(now, window).clamp(0., 1.))
    }
}

/// `a` to `b`, `t` of the way.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn lerp_bounds(a: Bounds<Pixels>, b: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let l = |a: Pixels, b: Pixels| px(lerp(a.as_f32(), b.as_f32(), t));
    Bounds::new(point(l(a.origin.x, b.origin.x), l(a.origin.y, b.origin.y)), size(l(a.size.width, b.size.width), l(a.size.height, b.size.height)))
}

// ---------- layout animation ----------

/// One list's FLIP bookkeeping: where each row was laid out last, and the spring carrying it
/// from there. Held by the view that draws the list; rows are keyed by what they show (a
/// thread's id), not by their place, so a row that moves is the same row.
#[derive(Default)]
pub struct Flip {
    /// The scope's top left as last laid out. Rows are placed against it, so scrolling the
    /// list (which moves the scope with them) isn't movement.
    origin: Point<Pixels>,
    rows: HashMap<SharedString, Placed>,
    /// Rows may move this frame. Off without motion, and for changes that shouldn't animate (a
    /// search narrowing the list): rows take their new places as they are.
    animate: bool,
    /// A frame has been drawn: a row with no entry is new rather than first.
    primed: bool,
    /// Rows drawn this frame (`begin` to `end`).
    drawn: Vec<SharedString>,
}

struct Placed {
    /// Where it was laid out last frame, against the scope; `None` before it has been.
    y: Option<f32>,
    height: f32,
    /// How far it's drawn from where it's laid out.
    offset: Spring,
    /// Fading in, while it's new.
    enter: Option<Spring>,
}

pub type FlipStore = Rc<RefCell<Flip>>;

/// How far above its place a new row starts, sliding down into it as it fades in.
const ENTER_RISE: f32 = -6.;

impl Flip {
    /// A frame starts drawing the list.
    pub fn begin(&mut self, animate: bool) {
        self.animate = animate;
        self.drawn.clear();
        if !animate {
            for p in self.rows.values_mut() {
                p.enter = None;
            }
        }
    }

    /// The frame has drawn its rows: forget the ones it didn't.
    pub fn end(&mut self) {
        let drawn: std::collections::HashSet<SharedString> = self.drawn.drain(..).collect();
        self.rows.retain(|k, _| drawn.contains(k));
        self.primed = true;
    }

    /// The row's height as last laid out.
    pub fn height(&self, key: &str) -> Option<f32> {
        self.rows.get(key).filter(|p| p.y.is_some()).map(|p| p.height)
    }

    /// How far `key` is drawn from its place at `now`.
    #[cfg(test)]
    pub fn offset(&self, key: &str, now: Instant) -> Option<f32> {
        self.rows.get(key).map(|p| p.offset.value(now))
    }

    /// Note `key` as drawn this frame; its opacity while it fades in (1 once in).
    fn touch(&mut self, key: &SharedString, now: Instant) -> f32 {
        self.drawn.push(key.clone());
        let fresh = !self.rows.contains_key(key);
        let (animate, primed) = (self.animate, self.primed);
        let p = self.rows.entry(key.clone()).or_insert_with(|| Placed { y: None, height: 0., offset: Spring::new(ROW, PIXEL, 0., now), enter: None });
        // New to a list already on screen: it fades in and settles into its place.
        if fresh && animate && primed {
            let mut e = Spring::new(SURFACE, UNIT, 0., now);
            e.set(1., true, now);
            p.enter = Some(e);
            p.offset.snap(ENTER_RISE, now);
            p.offset.set(0., true, now);
        }
        match p.enter {
            Some(e) if e.moving(now) => e.value(now).clamp(0., 1.),
            _ => {
                p.enter = None;
                1.
            }
        }
    }
}

/// The element rows of a FLIP list are placed against: wrap the list in it (inside its scroll
/// area, so it scrolls with them).
pub fn flip_scope(store: &FlipStore, child: impl IntoElement) -> FlipScope {
    FlipScope { store: store.clone(), child: child.into_any_element() }
}

/// A row of a FLIP list under `key`: drawn where it was last frame and sprung to its new place.
/// Rows new to a list already on screen fade and settle in.
pub fn flip_row(store: &FlipStore, key: impl Into<SharedString>, child: impl IntoElement, now: Instant, window: &Window) -> FlipRow {
    let key = key.into();
    let opacity = store.borrow_mut().touch(&key, now);
    if opacity < 1. {
        window.request_animation_frame();
    }
    // Always the same wrapper, so a row lays out the same in or out of an animation.
    let child = div().w_full().flex().flex_col().when(opacity < 1., |el| el.opacity(opacity)).child(child).into_any_element();
    FlipRow { store: store.clone(), key, child }
}

pub struct FlipScope {
    store: FlipStore,
    child: AnyElement,
}

pub struct FlipRow {
    store: FlipStore,
    key: SharedString,
    child: AnyElement,
}

impl IntoElement for FlipScope {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl IntoElement for FlipRow {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for FlipScope {
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
        // Before the rows: they're placed against this.
        self.store.borrow_mut().origin = bounds.origin;
        self.child.prepaint(window, cx);
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App) {
        self.child.paint(window, cx);
    }
}

impl Element for FlipRow {
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

    /// The layout is done: compare where the row is now with where it was, and draw it offset
    /// by what's left of the difference (its hitboxes go with it, so a click lands on what's
    /// under the pointer).
    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) {
        let now = now(cx);
        let offset = {
            let mut flip = self.store.borrow_mut();
            let (origin, animate) = (flip.origin, flip.animate);
            let y = (bounds.origin.y - origin.y).as_f32();
            let p = flip.rows.entry(self.key.clone()).or_insert_with(|| Placed { y: None, height: 0., offset: Spring::new(ROW, PIXEL, 0., now), enter: None });
            match p.y {
                Some(was) if animate && (was - y).abs() > 0.5 => p.offset.shift(was - y, now),
                _ if !animate => p.offset.snap(0., now),
                _ => {}
            }
            p.y = Some(y);
            p.height = bounds.size.height.as_f32();
            p.offset.frame(now, window)
        };
        if offset == 0. {
            self.child.prepaint(window, cx);
        } else {
            window.with_element_offset(point(px(0.), px(offset)), |window| self.child.prepaint(window, cx));
        }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App) {
        self.child.paint(window, cx);
    }
}

// ---------- sheets leaving ----------

/// Where a sheet's view was last drawn, kept by the view (`sheet_marker` notes it).
pub type SheetBounds = Rc<std::cell::Cell<Option<Bounds<Pixels>>>>;

/// A sheet on its way out: its view drawn once more where it was.
struct Leaving {
    view: AnyView,
    bounds: Bounds<Pixels>,
    left: Spring,
}

/// Each window's sheet on its way out (a window has one dialog up at a time).
#[derive(Default)]
struct SheetsLeaving(HashMap<WindowId, Leaving>);

impl Global for SheetsLeaving {}

/// Goes inside a sheet's view (which must be `relative`), over it: notes where it's drawn, so it
/// can leave from there.
pub fn sheet_marker(bounds: &SheetBounds) -> impl IntoElement {
    let bounds = bounds.clone();
    canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

/// The sheet showing `view` has just been taken down. gpui-component's dialogs go at once; with
/// motion Trek draws the view once more where it was, fading and lifting away over a clearing
/// scrim (`leaving_sheet`), AnimatePresence-style.
pub fn sheet_left(view: AnyView, bounds: &SheetBounds, motion: bool, window: &mut Window, cx: &mut App) {
    let Some(bounds) = bounds.get().filter(|_| motion) else { return };
    let now = now(cx);
    let mut left = Spring::new(SURFACE, UNIT, 1., now);
    left.set(0., true, now);
    cx.default_global::<SheetsLeaving>().0.insert(window.window_handle().window_id(), Leaving { view, bounds, left });
    window.refresh();
}

/// This window's sheet on its way out, if one is, to draw over everything else; frames come
/// while it goes.
pub fn leaving_sheet(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let id = window.window_handle().window_id();
    let now = now(cx);
    let leaving = cx.try_global::<SheetsLeaving>()?.0.get(&id)?;
    if !leaving.left.moving(now) {
        cx.global_mut::<SheetsLeaving>().0.remove(&id);
        return None;
    }
    let t = leaving.left.frame(now, window).clamp(0., 1.);
    let (view, b) = (leaving.view.clone(), leaving.bounds);
    let theme = gpui_kit::component::ActiveTheme::theme(cx);
    Some(
        div()
            .id("sheet-leaving")
            .test_support()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(theme.overlay.opacity(t))
            .child(
                // The dialog's own face, as gpui-component draws it round the view.
                div()
                    .absolute()
                    .left(b.origin.x - px(1.))
                    .top(b.origin.y - px(1.) - px(10. * (1. - t)))
                    .w(b.size.width + px(2.))
                    .h(b.size.height + px(2.))
                    .opacity(t)
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius_lg)
                    .overflow_hidden()
                    .shadow_xl()
                    .child(view),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::{Presence, SURFACE, Spring, UNIT};
    use std::time::{Duration, Instant};

    #[test]
    fn a_spring_turned_round_mid_flight_keeps_going_from_where_it_is() {
        let t0 = Instant::now();
        let mut s = Spring::new(SURFACE, UNIT, 0., t0);
        s.set(1., true, t0);
        let mid = t0 + Duration::from_millis(80);
        let (at, going) = (s.value(mid), s.state(mid).velocity);
        assert!(at > 0.1 && at < 0.9 && going > 0., "partway out and still going: {at} {going}");
        s.set(0., true, mid);
        // No jump at the turn: it's where it was, as fast as it was.
        assert_eq!(s.value(mid), at);
        assert_eq!(s.state(mid).velocity, going);
        // It carries on a little, then comes back without passing zero (no bounce).
        let later: Vec<f32> = (1..60).map(|i| s.value(mid + Duration::from_millis(i * 10))).collect();
        assert!(later[0] >= at, "momentum carries it on for a moment");
        assert!(later.iter().all(|v| *v >= 0.), "critically damped: never past where it's going");
        assert_eq!(s.value(mid + Duration::from_secs(2)), 0.);
        assert!(!s.moving(mid + Duration::from_secs(2)));
    }

    #[test]
    fn without_motion_a_spring_is_at_its_target_at_once() {
        let t0 = Instant::now();
        let mut s = Spring::new(SURFACE, UNIT, 0., t0);
        s.set(1., false, t0);
        assert_eq!(s.value(t0), 1.);
        assert!(!s.moving(t0));
    }

    #[test]
    fn presence_keeps_its_item_until_it_has_gone() {
        let t0 = Instant::now();
        let mut p = Presence::new(SURFACE);
        p.enter("sheet", true, t0);
        p.exit(true, t0 + Duration::from_millis(400));
        assert!(!p.is_present() && p.is_mounted());
        assert!(p.progress(t0 + Duration::from_millis(450)) > 0.);
        // Wanted again before it went: the same item turns round.
        p.enter("sheet", true, t0 + Duration::from_millis(450));
        assert!(p.is_present());
        p.exit(false, t0 + Duration::from_millis(500));
        assert!(!p.is_mounted(), "without motion it goes at once");
    }
}
