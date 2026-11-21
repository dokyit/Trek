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

/// How long every spring takes against its designed time (Settings' motion scale, which a Trek
/// theme sets): 1 as designed, 1.35 calmer, 0.75 brisker, 0 not at all. The app's, set in
/// `set_pace`; f32 bits.
static PACE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3F80_0000);

/// Set the motion scale every spring runs at from now on (see [`PACE`]): kept from 0 to 3, and
/// what isn't a number is 1.
pub fn set_pace(scale: f32) {
    let scale = if scale.is_finite() {
        scale.clamp(0., 3.)
    } else {
        1.
    };
    PACE.store(scale.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// The motion scale springs run at.
pub fn pace() -> f32 {
    f32::from_bits(PACE.load(std::sync::atomic::Ordering::Relaxed))
}

/// The spring's own time for `secs` of real time at motion scale `pace`; `None` at 0, where
/// everything is where it's going at once.
fn paced(pace: f32, secs: f32) -> Option<f32> {
    (pace > 0.).then(|| secs / pace)
}

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
        Spring {
            config,
            epsilon,
            from: SpringState {
                position: value,
                velocity: 0.,
            },
            at: now,
            target: value,
        }
    }

    /// Where it is and how fast it's going at `now`. Close enough to its target, it's there.
    pub fn state(&self, now: Instant) -> SpringState {
        let rest = SpringState {
            position: self.target,
            velocity: 0.,
        };
        if self.from == rest {
            return rest;
        }
        let Some(elapsed) = paced(pace(), now.saturating_duration_since(self.at).as_secs_f32())
        else {
            return rest;
        };
        let s = self.config.step(self.from, self.target, elapsed);
        if self.config.is_settled(s, self.target, self.epsilon) {
            rest
        } else {
            s
        }
    }

    pub fn value(&self, now: Instant) -> f32 {
        self.state(now).position
    }

    /// Where it's going.
    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn moving(&self, now: Instant) -> bool {
        self.state(now)
            != SpringState {
                position: self.target,
                velocity: 0.,
            }
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
        self.from = SpringState {
            position: value,
            velocity: 0.,
        };
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
        static SLOW: std::sync::LazyLock<Option<(f32, Instant)>> = std::sync::LazyLock::new(|| {
            std::env::var("TREK_SLOW_MOTION")
                .ok()?
                .parse::<f32>()
                .ok()
                .filter(|k| *k > 1.)
                .map(|k| (k, Instant::now()))
        });
        if let Some((k, from)) = *SLOW {
            return from + now.saturating_duration_since(from).div_f32(k);
        }
    }
    now
}

/// Ask for the next frame and have it drawn whole, cached views included. A cached view that
/// hasn't changed replays its last painting as it was, opacity and all, while its place and size
/// hold, so a fade over one needs this every frame it runs (or the view holds still until the
/// fade has gone, then vanishes).
pub fn redraw_whole(window: &Window) {
    window.on_next_frame(|window, _| window.refresh());
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
        Presence {
            item: None,
            present: false,
            spring: Spring::new(config, UNIT, 0., Instant::now()),
        }
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
    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
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
    Bounds::new(
        point(l(a.origin.x, b.origin.x), l(a.origin.y, b.origin.y)),
        size(
            l(a.size.width, b.size.width),
            l(a.size.height, b.size.height),
        ),
    )
}

// ---------- layout animation ----------

/// One list's FLIP bookkeeping: where each row was laid out last, and the spring carrying it
/// from there. Held by the view that draws the list; rows are keyed by what they show (a
/// thread's id), not by their place, so a row that moves is the same row.
///
/// Rows may be gathered into groups (`flip_stack`): a group moves as one, its rows placed
/// against it, so groups trading places slide past each other whole.
#[derive(Default)]
pub struct Flip {
    /// The scope's top left as last laid out. Rows are placed against it, so scrolling the
    /// list (which moves the scope with them) isn't movement.
    origin: Point<Pixels>,
    /// The group whose rows are being placed, and its top left as drawn: they're placed
    /// against that instead.
    group: Option<(SharedString, Point<Pixels>)>,
    rows: HashMap<SharedString, Placed>,
    /// Rows may move this frame. Off without motion, and for changes that shouldn't animate (a
    /// search narrowing the list): rows take their new places as they are.
    animate: bool,
    /// A frame has been drawn: a row with no entry is new rather than first.
    primed: bool,
    /// Rows drawn this frame (from `begin`; kept till the next, as placing them comes after
    /// `end`).
    drawn: Vec<SharedString>,
    /// Every row as drawn this frame, in the order they were placed (`Band`). Of two rows drawn
    /// over the same pixels, only the one on top is painted there: the other is cut back to the
    /// side of it its middle is on. So no row's text is ever painted over another's, whatever
    /// moves where (a row rising past others, the rows closing up after it, a group sliding down
    /// as the Inbox grows, a row going back down to its project, several at once).
    bands: Vec<Band>,
    /// How many of `bands` are moving (or fading): with none, nothing is drawn over anything and
    /// no row is cut, however long the list.
    movers: usize,
    /// How far the group whose rows are being placed is drawn from its place (0 for none).
    group_offset: f32,
    /// How far the group under the one being placed is drawn from its place, against that one's
    /// own offset (`None`: it's the last): the group's frame (`flip_frame`, the Inbox's card)
    /// ends that much lower, with what's under it.
    below: Option<f32>,
    /// While a group that isn't going up is placed: the bands (top, bottom) of the groups going
    /// up past it. Its rows take no clicks under them (their own rows are placed later, so the
    /// rows they'll be painted over don't know them yet).
    over: Vec<(f32, f32)>,
}

/// A band down a list, top to bottom (window px).
pub type Span = (f32, f32);

/// A row as drawn this frame: the band it covers and its rank.
#[derive(Clone, Debug)]
struct Band {
    key: SharedString,
    top: f32,
    bottom: f32,
    /// How far from its place in its group it's drawn.
    offset: f32,
    rank: Rank,
}

/// Which of two rows drawn over each other is on top: the one with further to go up (`reach`,
/// how far below its place it's drawn, its group's way included), so a row rising past others is
/// over them and a row going down slides under what it passes; of two going the same way, the
/// one placed later. A row fading in where it goes, new, is under every other (it comes in where
/// the others already are, see-through), and one fading away (`FlipRow::fading`) under that.
#[derive(Clone, Copy, Debug)]
struct Rank {
    reach: f32,
    seq: usize,
}

impl Rank {
    fn over(&self, other: &Rank) -> bool {
        if (self.reach - other.reach).abs() > PIXEL {
            self.reach > other.reach
        } else {
            self.seq > other.seq
        }
    }
}

/// A row fading in ranks under every row that isn't (`Rank`).
const ENTERING: f32 = -1e9;

/// How far under another a row has gone (px of it cut off) when it has faded out altogether: its
/// edge rows of padding go first, so letters are cut only where they can't be seen. With the
/// rows moving, it's a frame or two either way.
const CUT_FADE: f32 = 3.;

/// A row's opacity with `cut` px of it cut off by the rows over it.
fn cut_opacity(cut: f32) -> f32 {
    if cut <= OVERLAP {
        1.
    } else {
        (1. - cut / CUT_FADE).clamp(0., 1.)
    }
}

/// How far two bands must overlap to count: less is rounding.
const OVERLAP: f32 = 0.25;

/// Where a row drawn over `top..bottom` may be painted, kept off `bands` (the rows over it):
/// above one, if its middle is above that one's, else below it. `None`: anywhere.
fn clip_off(top: f32, bottom: f32, bands: impl Iterator<Item = (f32, f32)>) -> Option<(f32, f32)> {
    if bottom <= top {
        return None;
    }
    let mid = (top + bottom) / 2.;
    let (mut from, mut to) = (top, bottom);
    let mut clipped = false;
    for (a, b) in bands {
        // Over it by more than a hair: rows that only touch don't cut each other.
        if a < bottom - OVERLAP && b > top + OVERLAP {
            clipped = true;
            if mid < (a + b) / 2. {
                to = to.min(a);
            } else {
                from = from.max(b);
            }
        }
    }
    clipped.then_some((from, to.max(from)))
}

struct Placed {
    /// Where it was laid out last frame, against its group (or the scope); `None` before it
    /// has been.
    y: Option<f32>,
    /// The group it was laid out in.
    group: Option<SharedString>,
    height: f32,
    /// How far it's drawn from where it's laid out.
    offset: Spring,
    /// Fading in, while it's new.
    enter: Option<Spring>,
}

impl Placed {
    fn new(now: Instant) -> Self {
        Placed {
            y: None,
            group: None,
            height: 0.,
            offset: Spring::new(ROW, PIXEL, 0., now),
            enter: None,
        }
    }
}

pub type FlipStore = Rc<RefCell<Flip>>;

/// How far above its place a new row starts, sliding down into it as it fades in.
const ENTER_RISE: f32 = -6.;

impl Flip {
    /// A frame starts drawing the list: forget the rows the last one didn't draw. (Not at its
    /// `end`: its rows are placed after that, and one leaving a group goes from where the group
    /// was drawn, even a group that's gone.)
    pub fn begin(&mut self, animate: bool) {
        self.animate = animate;
        if self.primed {
            let drawn: std::collections::HashSet<SharedString> = self.drawn.drain(..).collect();
            self.rows.retain(|k, _| drawn.contains(k));
        }
        self.drawn.clear();
        self.bands.clear();
        self.movers = 0;
        if !animate {
            for p in self.rows.values_mut() {
                p.enter = None;
            }
        }
    }

    /// The frame has drawn its rows.
    pub fn end(&mut self) {
        self.primed = true;
    }

    /// The row's height as last laid out.
    pub fn height(&self, key: &str) -> Option<f32> {
        self.rows
            .get(key)
            .filter(|p| p.y.is_some())
            .map(|p| p.height)
    }

    /// How far `key` is drawn from its place at `now`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn offset(&self, key: &str, now: Instant) -> Option<f32> {
        self.rows.get(key).map(|p| p.offset.value(now))
    }

    /// Note `key` as drawn this frame; its opacity while it fades in (1 once in).
    fn touch(&mut self, key: &SharedString, now: Instant) -> f32 {
        self.drawn.push(key.clone());
        let fresh = !self.rows.contains_key(key);
        let (animate, primed) = (self.animate, self.primed);
        let p = self
            .rows
            .entry(key.clone())
            .or_insert_with(|| Placed::new(now));
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

    /// Where `group`'s top is drawn against the scope at `now` (the scope's own top for none).
    fn group_top(&self, group: &Option<SharedString>, now: Instant) -> f32 {
        group
            .as_ref()
            .and_then(|g| self.rows.get(g))
            .map_or(0., |p| p.y.unwrap_or(0.) + p.offset.value(now))
    }

    /// `key` is laid out at `bounds` this frame: carry it from where it was drawn to there, and
    /// say how far from there it's drawn now. A row that changed groups goes from where it was
    /// drawn in the old one.
    fn place(
        &mut self,
        key: &SharedString,
        bounds: Bounds<Pixels>,
        now: Instant,
        window: &Window,
    ) -> f32 {
        let (group, origin) = match &self.group {
            Some((g, o)) => (Some(g.clone()), *o),
            None => (None, self.origin),
        };
        let y = (bounds.origin.y - origin.y).as_f32();
        let animate = self.animate;
        let moved = match self.rows.get(key) {
            Some(Placed {
                y: Some(was),
                group: from,
                ..
            }) if animate => {
                Some(was + self.group_top(from, now) - y - self.group_top(&group, now))
            }
            _ => None,
        };
        let p = self
            .rows
            .entry(key.clone())
            .or_insert_with(|| Placed::new(now));
        match moved {
            Some(d) if d.abs() > 0.5 => p.offset.shift(d, now),
            _ if !animate => p.offset.snap(0., now),
            _ => {}
        }
        p.y = Some(y);
        p.group = group;
        p.height = bounds.size.height.as_f32();
        p.offset.frame(now, window)
    }

    /// Where row `key`, drawn over `top..bottom` at `rank`, may be painted: off every row drawn
    /// over it this frame (`bands`, as far as they've been placed). `None`: anywhere.
    fn clip(&self, key: &SharedString, top: f32, bottom: f32, rank: Rank) -> Option<(f32, f32)> {
        if self.movers == 0 {
            return None;
        }
        clip_off(
            top,
            bottom,
            self.bands
                .iter()
                .filter(|b| &b.key != key && b.rank.over(&rank))
                .map(|b| (b.top, b.bottom)),
        )
    }

    /// Each row's band in the last frame, top to bottom: where it's drawn, and where it's painted
    /// (cut back off the rows over it; empty where they cover it). For tests, which check that no
    /// two are painted over each other.
    #[cfg(any(test, feature = "test-support"))]
    pub fn painted_bands(&self) -> Vec<(SharedString, Span, Span)> {
        let mut out: Vec<(SharedString, Span, Span)> = self
            .bands
            .iter()
            .filter(|b| b.bottom > b.top)
            .map(|b| {
                let painted = self
                    .clip(&b.key, b.top, b.bottom, b.rank)
                    .unwrap_or((b.top, b.bottom));
                (b.key.clone(), (b.top, b.bottom), painted)
            })
            .collect();
        out.sort_by(|a, b| a.1.0.total_cmp(&b.1.0));
        out
    }
}

/// The element rows of a FLIP list are placed against: wrap the list in it (inside its scroll
/// area, so it scrolls with them).
pub fn flip_scope(store: &FlipStore, child: impl IntoElement) -> FlipScope {
    FlipScope {
        store: store.clone(),
        child: child.into_any_element(),
    }
}

/// A row of a FLIP list under `key`: drawn where it was last frame and sprung to its new place.
/// Rows new to a list already on screen fade and settle in.
pub fn flip_row(
    store: &FlipStore,
    key: impl Into<SharedString>,
    child: impl IntoElement,
    now: Instant,
    window: &Window,
) -> FlipRow {
    let key = key.into();
    let opacity = store.borrow_mut().touch(&key, now);
    if opacity < 1. {
        window.request_animation_frame();
    }
    // Always the same wrapper, so a row lays out the same in or out of an animation.
    let child = div()
        .w_full()
        .flex()
        .flex_col()
        .when(opacity < 1., |el| el.opacity(opacity))
        .child(child)
        .into_any_element();
    FlipRow {
        store: store.clone(),
        key,
        child,
        surface: None,
        fading: None,
        entering: opacity < 1.,
    }
}

impl FlipRow {
    /// What the row carries under it while it goes up past others: the surface it sits on, with
    /// `radius` corners, so nothing of what it passes shows through (they aren't painted under it
    /// either way). `None` where the list is see-through (glass, a picture), as a fill there would
    /// read as a patch.
    pub fn surface(mut self, fill: Option<Hsla>, radius: Pixels) -> Self {
        self.surface = fill.map(|f| (f, radius));
        self
    }

    /// A row fading away where it was, out of the layout (no height of its own; it draws
    /// `height`): under every other row, which aren't cut for it, and not painted under them.
    pub fn fading(mut self, height: Pixels) -> Self {
        self.fading = Some(height);
        self
    }
}

/// Groups of FLIP rows (a project's header and threads), stacked: each group moves as one, its
/// rows placed against it, and groups trading places pass whole. The ones going up are drawn
/// (and take clicks) over the ones going down, whose rows aren't painted under theirs.
pub fn flip_stack(
    store: &FlipStore,
    groups: Vec<(SharedString, AnyElement)>,
    now: Instant,
) -> FlipStack {
    let mut flip = store.borrow_mut();
    for (key, _) in &groups {
        flip.drawn.push(key.clone());
        flip.rows
            .entry(key.clone())
            .or_insert_with(|| Placed::new(now));
    }
    FlipStack {
        store: store.clone(),
        groups,
    }
}

pub struct FlipScope {
    store: FlipStore,
    child: AnyElement,
}

pub struct FlipRow {
    store: FlipStore,
    key: SharedString,
    child: AnyElement,
    surface: Option<(Hsla, Pixels)>,
    fading: Option<Pixels>,
    /// New, fading in.
    entering: bool,
}

/// A FLIP row as drawn this frame.
pub struct RowFrame {
    /// Where it's drawn: its place, offset (for a row fading away, as tall as it draws).
    drawn: Bounds<Pixels>,
    rank: Rank,
    /// Drawn below its place in its group: on its way up past others there.
    lifted: bool,
}

pub struct FlipStack {
    store: FlipStore,
    groups: Vec<(SharedString, AnyElement)>,
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

impl IntoElement for FlipStack {
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

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        // Before the rows: they're placed against this.
        self.store.borrow_mut().origin = bounds.origin;
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

/// Where a row drawn at `drawn` may be painted (`Flip::clip`), if not anywhere.
fn row_mask(
    store: &FlipStore,
    key: &SharedString,
    drawn: Bounds<Pixels>,
    rank: Rank,
) -> Option<ContentMask<Pixels>> {
    store
        .borrow()
        .clip(key, drawn.top().as_f32(), drawn.bottom().as_f32(), rank)
        .map(|(top, bottom)| ContentMask {
            bounds: Bounds::from_corners(
                point(drawn.left(), px(top)),
                point(drawn.right(), px(bottom)),
            ),
        })
}

impl Element for FlipRow {
    type RequestLayoutState = ();
    type PrepaintState = RowFrame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    /// The layout is done: compare where the row is now with where it was, and draw it offset
    /// by what's left of the difference (its hitboxes go with it, so a click lands on what's
    /// under the pointer). Where a row over it covers it, it takes no clicks either (from the
    /// rows placed before it; it isn't painted under any of them).
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> RowFrame {
        let offset = self
            .store
            .borrow_mut()
            .place(&self.key, bounds, now(cx), window);
        // On whole device pixels, as the stack's groups are: a band off by a fraction would let
        // a passing row show a sliver under the one going up.
        let scale = window.scale_factor();
        let offset = (offset * scale).round() / scale;
        let height = self.fading.unwrap_or(bounds.size.height);
        let drawn = Bounds::new(
            point(bounds.left(), bounds.top() + px(offset)),
            size(bounds.size.width, height),
        );
        let rank = {
            let mut flip = self.store.borrow_mut();
            let rank = Rank {
                reach: match (self.fading, offset + flip.group_offset) {
                    (Some(_), _) => f32::NEG_INFINITY,
                    // Coming in where it goes; one already on its way up passes as any row does.
                    (None, reach) if self.entering && reach <= PIXEL => ENTERING,
                    (None, reach) => reach,
                },
                seq: flip.bands.len(),
            };
            flip.bands.push(Band {
                key: self.key.clone(),
                top: drawn.top().as_f32(),
                bottom: drawn.bottom().as_f32(),
                offset,
                rank,
            });
            if rank.reach.abs() > PIXEL {
                flip.movers += 1;
            }
            rank
        };
        // Its hitboxes are kept off the groups going up past its own, too: they take the clicks
        // there.
        let mask = {
            let flip = self.store.borrow();
            let (top, bottom) = (drawn.top().as_f32(), drawn.bottom().as_f32());
            // At rest nothing is drawn over anything (`movers`).
            let placed = if flip.movers == 0 {
                &[][..]
            } else {
                &flip.bands[..]
            };
            clip_off(
                top,
                bottom,
                placed
                    .iter()
                    .filter(|b| b.key != self.key && b.rank.over(&rank))
                    .map(|b| (b.top, b.bottom))
                    .chain(flip.over.iter().copied()),
            )
            .map(|(top, bottom)| ContentMask {
                bounds: Bounds::from_corners(
                    point(drawn.left(), px(top)),
                    point(drawn.right(), px(bottom)),
                ),
            })
        };
        window.with_content_mask(mask, |window| {
            if offset == 0. {
                self.child.prepaint(window, cx);
            } else {
                window.with_element_offset(point(px(0.), px(offset)), |window| {
                    self.child.prepaint(window, cx)
                });
            }
        });
        RowFrame {
            drawn,
            rank,
            lifted: offset > PIXEL && self.fading.is_none(),
        }
    }

    /// Painted after every row has been placed: kept off every row over it this frame; going
    /// up past others, on the surface it carries. A row a few pixels under another fades out
    /// (`CUT_FADE`): nothing of it is cut mid-letter where it can be seen.
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        frame: &mut RowFrame,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mask = row_mask(&self.store, &self.key, frame.drawn, frame.rank);
        let opacity = mask.as_ref().map_or(1., |m| {
            let cut = (m.bounds.top() - frame.drawn.top())
                .max(frame.drawn.bottom() - m.bounds.bottom())
                .as_f32();
            cut_opacity(cut)
        });
        window.with_content_mask(mask, |window| {
            window.with_paint_opacity(opacity, |window| {
                if let Some((surface, radius)) = self.surface.filter(|_| frame.lifted) {
                    window.paint_quad(fill(frame.drawn, surface).corner_radii(radius));
                }
                self.child.paint(window, cx)
            })
        });
    }
}

/// How a group's frame (`flip_frame`) is painted: a filled box with a border, as a `div` would
/// paint it.
#[derive(Clone, Copy, Debug)]
pub struct FrameLook {
    pub fill: Hsla,
    pub border: Hsla,
    pub border_width: Pixels,
    pub radius: Pixels,
}

/// A box drawn round a group's rows (the Inbox's card) in a stack (`flip_stack`), which moves
/// with them: while they slide, its top and bottom are drawn where its first row and what's
/// under it are, not where the layout has them, so its edges never cross a row that's still on
/// its way. Where a row is drawn across its top or bottom edge (one rising into it from below,
/// one going down out of it) the edge isn't painted: no line through a row's text, whether or
/// not the row has a fill of its own. `child` is the group's content, laid out as the box (its
/// insets the border's room).
pub fn flip_frame(store: &FlipStore, look: FrameLook, child: impl IntoElement) -> FlipFrame {
    FlipFrame {
        store: store.clone(),
        look,
        child: child.into_any_element(),
    }
}

pub struct FlipFrame {
    store: FlipStore,
    look: FrameLook,
    child: AnyElement,
}

impl IntoElement for FlipFrame {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

/// Where a group's frame is drawn this frame (`flip_frame`).
pub struct FrameDrawn {
    bounds: Bounds<Pixels>,
}

impl Element for FlipFrame {
    type RequestLayoutState = ();
    type PrepaintState = FrameDrawn;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    /// Its rows placed, it ends where they're drawn: its top with its first row's, its bottom
    /// with the group under it (or, last in the stack, with its own last row).
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> FrameDrawn {
        let first = self.store.borrow().bands.len();
        self.child.prepaint(window, cx);
        let flip = self.store.borrow();
        let own = &flip.bands[first.min(flip.bands.len())..];
        let top = own.first().map_or(0., |b| b.offset);
        let bottom = flip
            .below
            .or_else(|| {
                own.iter()
                    .rev()
                    .find(|b| b.bottom > b.top)
                    .map(|b| b.offset)
            })
            .unwrap_or(0.);
        let top = bounds.top() + px(top);
        let bottom = (bounds.bottom() + px(bottom)).max(top);
        FrameDrawn {
            bounds: Bounds::from_corners(point(bounds.left(), top), point(bounds.right(), bottom)),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        drawn: &mut FrameDrawn,
        window: &mut Window,
        cx: &mut App,
    ) {
        let b = drawn.bounds;
        let (top, bottom) = (b.top().as_f32(), b.bottom().as_f32());
        // The rows drawn over its top or bottom edge (the line its border paints there), as bands
        // within it.
        let w = self.look.border_width.as_f32();
        let edges = [(top, top + w), (bottom - w, bottom)];
        let across: Vec<(f32, f32)> = {
            let flip = self.store.borrow();
            let mut across: Vec<(f32, f32)> = flip
                .bands
                .iter()
                .filter(|r| {
                    edges
                        .iter()
                        .any(|(a, z)| r.top < z - OVERLAP && r.bottom > a + OVERLAP)
                })
                .map(|r| (r.top.max(top), r.bottom.min(bottom)))
                .collect();
            across.sort_by(|a, b| a.0.total_cmp(&b.0));
            across
        };
        let look = self.look;
        let quad = |edges: bool| {
            let w = look.border_width;
            fill(b, look.fill)
                .corner_radii(look.radius)
                .border_widths(Edges {
                    top: if edges { w } else { px(0.) },
                    bottom: if edges { w } else { px(0.) },
                    left: w,
                    right: w,
                })
                .border_color(look.border)
        };
        if across.is_empty() {
            window.paint_quad(quad(true));
        } else {
            // Its height in pieces: where a row is across an edge, no edge; elsewhere, all of it.
            let mut at = top;
            let mut pieces: Vec<(f32, f32, bool)> = vec![];
            for (a, z) in across {
                if a > at {
                    pieces.push((at, a, true));
                }
                if z > at.max(a) {
                    pieces.push((at.max(a), z, false));
                }
                at = at.max(z);
            }
            if bottom > at {
                pieces.push((at, bottom, true));
            }
            for (a, z, edges) in pieces {
                let mask = ContentMask {
                    bounds: Bounds::from_corners(point(b.left(), px(a)), point(b.right(), px(z))),
                };
                window.with_content_mask(Some(mask), |window| window.paint_quad(quad(edges)));
            }
        }
        self.child.paint(window, cx);
    }
}

/// A group in a stack as laid out this frame.
pub struct Stacked {
    bounds: Bounds<Pixels>,
    offset: f32,
}

impl Stacked {
    /// Drawn below its place: on its way up.
    fn rising(&self) -> bool {
        self.offset > PIXEL
    }
}

/// The order a stack's groups are drawn in this frame.
pub struct StackFrame {
    order: Vec<usize>,
}

impl Element for FlipStack {
    type RequestLayoutState = Vec<LayoutId>;
    type PrepaintState = StackFrame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Vec<LayoutId>) {
        let ids: Vec<LayoutId> = self
            .groups
            .iter_mut()
            .map(|(_, g)| g.request_layout(window, cx))
            .collect();
        let mut style = Style::default();
        style.display = Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.size.width = relative(1.).into();
        (window.request_layout(style, ids.iter().copied(), cx), ids)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        ids: &mut Vec<LayoutId>,
        window: &mut Window,
        cx: &mut App,
    ) -> StackFrame {
        let now = now(cx);
        // Offsets on whole device pixels, as the rows are drawn: a band that's off by a fraction
        // would let a passing row show a sliver under the group going up.
        let scale = window.scale_factor();
        let groups: Vec<Stacked> = self
            .groups
            .iter()
            .zip(ids.iter())
            .map(|((key, _), id)| {
                let bounds = window.layout_bounds(*id);
                let offset = self.store.borrow_mut().place(key, bounds, now, window);
                Stacked {
                    bounds,
                    offset: (offset * scale).round() / scale,
                }
            })
            .collect();
        // Groups going up last: drawn, and taking clicks, over the ones they pass (their rows
        // rank over the rows they pass, `Rank`, so those are cut back off them too).
        let mut order: Vec<usize> = (0..groups.len()).collect();
        order.sort_by_key(|i| groups[*i].rising());
        let rising: Vec<(f32, f32)> = groups
            .iter()
            .filter(|g| g.rising())
            .map(|g| {
                (
                    g.bounds.top().as_f32() + g.offset,
                    g.bounds.bottom().as_f32() + g.offset,
                )
            })
            .collect();
        for &i in &order {
            let (key, group) = &mut self.groups[i];
            let g = &groups[i];
            let top = point(g.bounds.origin.x, g.bounds.origin.y + px(g.offset));
            let outer = {
                let mut flip = self.store.borrow_mut();
                flip.group_offset = g.offset;
                flip.below = groups.get(i + 1).map(|next| next.offset - g.offset);
                flip.over = if g.rising() { vec![] } else { rising.clone() };
                flip.group.replace((key.clone(), top))
            };
            window.with_element_offset(point(px(0.), px(g.offset)), |window| {
                group.prepaint(window, cx)
            });
            let mut flip = self.store.borrow_mut();
            flip.group = outer;
            flip.group_offset = 0.;
            flip.below = None;
            flip.over.clear();
        }
        StackFrame { order }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Vec<LayoutId>,
        frame: &mut StackFrame,
        window: &mut Window,
        cx: &mut App,
    ) {
        for &i in &frame.order {
            self.groups[i].1.paint(window, cx);
        }
    }
}

// ---------- leaving ----------

/// Something on its way out: drawn as it is, but out of reach of the pointer. Its hitboxes are
/// laid under an empty mask, so nothing in it is hovered, pressed or scrolled, and it blocks
/// nothing: a click goes to whatever is under it.
pub fn inert(child: impl IntoElement) -> Inert {
    Inert(child.into_any_element())
}

pub struct Inert(AnyElement);

impl IntoElement for Inert {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Inert {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.0.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(
            Some(ContentMask {
                bounds: Bounds::default(),
            }),
            |window| self.0.prepaint(window, cx),
        );
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.paint(window, cx);
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
    canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}

/// The sheet showing `view` has just been taken down. gpui-component's dialogs go at once; with
/// motion Trek draws the view once more where it was, fading and lifting away over a clearing
/// scrim (`leaving_sheet`), AnimatePresence-style.
pub fn sheet_left(
    view: AnyView,
    bounds: &SheetBounds,
    motion: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(bounds) = bounds.get().filter(|_| motion) else {
        return;
    };
    let now = now(cx);
    let mut left = Spring::new(SURFACE, UNIT, 1., now);
    left.set(0., true, now);
    cx.default_global::<SheetsLeaving>().0.insert(
        window.window_handle().window_id(),
        Leaving { view, bounds, left },
    );
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
    // Inert: a click as it goes reaches what's under it, not the sheet.
    Some(
        inert(
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
                ),
        )
        .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_motion_scale_stretches_a_springs_time_and_zero_stills_it() {
        assert_eq!(super::paced(1., 0.3), Some(0.3));
        assert_eq!(super::paced(2., 0.3), Some(0.15));
        assert_eq!(super::paced(0.5, 0.3), Some(0.6));
        assert_eq!(super::paced(0., 0.3), None);
    }

    use super::{Presence, SURFACE, Spring, UNIT};
    use std::time::{Duration, Instant};

    #[test]
    fn a_spring_turned_round_mid_flight_keeps_going_from_where_it_is() {
        let t0 = Instant::now();
        let mut s = Spring::new(SURFACE, UNIT, 0., t0);
        s.set(1., true, t0);
        let mid = t0 + Duration::from_millis(80);
        let (at, going) = (s.value(mid), s.state(mid).velocity);
        assert!(
            at > 0.1 && at < 0.9 && going > 0.,
            "partway out and still going: {at} {going}"
        );
        s.set(0., true, mid);
        // No jump at the turn: it's where it was, as fast as it was.
        assert_eq!(s.value(mid), at);
        assert_eq!(s.state(mid).velocity, going);
        // It carries on a little, then comes back without passing zero (no bounce).
        let later: Vec<f32> = (1..60)
            .map(|i| s.value(mid + Duration::from_millis(i * 10)))
            .collect();
        assert!(later[0] >= at, "momentum carries it on for a moment");
        assert!(
            later.iter().all(|v| *v >= 0.),
            "critically damped: never past where it's going"
        );
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
