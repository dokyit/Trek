//! The user's own mouse, trackpad and keys on the picture (`trek_phone::gesture`): a press goes
//! to the phone held as long as the button was, a drag as its whole path with its timing, a
//! scroll as swipes, and once the picture was clicked, typing, until Escape or a click outside
//! the tool. While the button is down a dot shows the touch and a trail the drag.
//!
//! Over Trek Driver the inputs go one at a time and in order (`pump`: a word typed mustn't arrive
//! shuffled), text typed meanwhile merged into one request; the helper keeps the Bluetooth and
//! iPhone Mirroring ones in order itself. The user's input always wins: an agent holding the
//! phone loses it, as with Stop.

use super::*;
use trek_phone::gesture::{self, Input, Key, Mods, Stroke};

/// How long scrolling may pause before what was gathered goes to the phone, and how long it
/// gathers at most (a long fling goes in pieces).
const SCROLL_PAUSE: Duration = Duration::from_millis(120);
const SCROLL_GATHER: Duration = Duration::from_millis(300);
/// A wheel's line, in pixels of the picture.
const WHEEL_LINE: f32 = 24.;
/// How long the pointer must have rested on the picture before a scroll with no phase of its own
/// (a fling's tail, a smooth-scrolling mouse) counts there: a fling begun over the chat that
/// drifts onto the picture isn't the phone's.
const HOVER_SETTLE: Duration = Duration::from_millis(300);
/// The most inputs waiting for the driver (the phone is far behind past that: more are dropped).
const QUEUE_MAX: usize = 64;

/// The user's input on the picture.
pub(super) struct Touch {
    /// The picture has the keys while this is focused.
    pub(super) focus: FocusHandle,
    /// Where the keys were before the picture took them: they go back there.
    before: Option<FocusHandle>,
    press: Option<Press>,
    scroll: Option<Scroll>,
    /// The latest scroll event taken over the picture (a trackpad's tail keeps coming).
    scrolled: Option<Instant>,
    scroll_task: Option<Task<()>>,
    /// Redraws the dot once a press has been held long enough to be a long press.
    hold_task: Option<Task<()>>,
    /// Since when the pointer has been over the picture.
    hovered: Option<Instant>,
    /// The user's inputs waiting for Trek Driver, and whether one is on its way.
    queue: VecDeque<Input>,
    sending: bool,
    _focus: [Subscription; 2],
}

/// A press under way: its stroke, when it began, and the pointer now (fractions of the picture,
/// held to its edges).
struct Press {
    stroke: Stroke,
    at: Instant,
    pointer: (f64, f64),
}

/// Scrolling gathered since it last went to the phone: where it began (fractions of the picture)
/// and how far (pixels of the picture).
struct Scroll {
    at: (f64, f64),
    delta: (f64, f64),
    first: Instant,
}

impl Touch {
    pub(super) fn new(window: &mut Window, cx: &mut Context<PhonePanel>) -> Self {
        let focus = cx.focus_handle();
        let subscriptions = [
            cx.on_focus(&focus, window, |_, _, cx| cx.notify()),
            cx.on_blur(&focus, window, |_, _, cx| cx.notify()),
        ];
        Self {
            focus,
            before: None,
            press: None,
            scroll: None,
            scrolled: None,
            scroll_task: None,
            hold_task: None,
            hovered: None,
            queue: VecDeque::new(),
            sending: false,
            _focus: subscriptions,
        }
    }
}

fn millis(since: Instant) -> u32 {
    since.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

impl PhonePanel {
    /// Whether the user's taps reach the phone now (the pointer is a hand over the picture).
    pub(super) fn touchable(&self) -> bool {
        (self.facts().route() == Route::Driver && self.driver_avail() == driver::Avail::Ready)
            || self.status.input_ready()
            || self.mirror_frames
    }

    /// Where the picture is in the window.
    fn picture(&self) -> Option<Bounds<Pixels>> {
        let s = self.screen_rect()?;
        Some(Bounds {
            origin: self.area.get().origin + s.origin,
            size: s.size,
        })
    }

    fn picture_size(&self) -> Option<(f64, f64)> {
        self.picture().map(|b| {
            (
                f64::from(b.size.width.as_f32()),
                f64::from(b.size.height.as_f32()),
            )
        })
    }

    /// The keys are the phone's.
    pub(super) fn typing(&self, window: &Window) -> bool {
        self.touch.focus.is_focused(window)
    }

    fn take_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.typing(window) {
            return;
        }
        self.touch.before = window.focused(cx);
        window.focus(&self.touch.focus, cx);
        cx.notify();
    }

    /// The keys go back to where they were before the picture took them.
    pub(super) fn give_keys_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.typing(window) {
            return;
        }
        match self.touch.before.take() {
            Some(before) => window.focus(&before, cx),
            None => window.blur(cx),
        }
        cx.notify();
    }

    /// The user's input wins over an agent's: one holding the phone (or with calls waiting)
    /// loses it, as with Stop.
    fn user_wins(&mut self, cx: &mut Context<Self>) {
        if self.busy() || self.lease.holder(Instant::now()).is_some() {
            self.agent_stop(cx);
        }
    }

    pub(super) fn press_down(
        &mut self,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Off the picture (its margin) isn't the phone.
        let (Some(b), Some(size)) = (self.picture(), self.picture_size()) else {
            return;
        };
        if !b.contains(&e.position) {
            return;
        }
        let Some(at) = self.to_fraction(e.position) else {
            return;
        };
        self.take_keys(window, cx);
        self.user_wins(cx);
        // The picture comes faster from the press on, so the screen is fresh as it moves.
        self.note_input();
        self.touch.press = Some(Press {
            stroke: Stroke::new(at, size),
            at: Instant::now(),
            pointer: at,
        });
        let long = Duration::from_millis(u64::from(gesture::LONG_PRESS_MS));
        self.touch.hold_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(long).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        }));
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn press_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.touch.press.is_none() {
            // Hovering: when the pointer came onto the picture (for scrolls with no phase).
            let over = self.picture().is_some_and(|b| b.contains(&e.position));
            match (over, self.touch.hovered) {
                (true, None) => self.touch.hovered = Some(Instant::now()),
                (false, Some(_)) => self.touch.hovered = None,
                _ => {}
            }
            return;
        }
        if e.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(at) = self.to_fraction(e.position) else {
            return;
        };
        let Some(press) = self.touch.press.as_mut() else {
            return;
        };
        press.pointer = at;
        press.stroke.push(at, millis(press.at));
        cx.notify();
    }

    /// The pointer left the picture's area (moves are only seen over it).
    pub(super) fn hover(&mut self, over: bool) {
        if !over {
            self.touch.hovered = None;
        }
    }

    pub(super) fn press_up(
        &mut self,
        p: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(press) = self.touch.press.take() else {
            return;
        };
        self.touch.hold_task = None;
        let ms = millis(press.at);
        let at = self.to_fraction(p).unwrap_or(press.pointer);
        let input = press.stroke.finish(at, ms);
        // A click on the home bar goes Home (a drag from it is the phone's own swipe up).
        if let gesture::Input::Tap { at, .. } = &input
            && gesture::on_home_bar(*at)
        {
            self.home(window, cx);
            cx.notify();
            return;
        }
        self.send_input(input, window, cx);
        cx.notify();
    }

    /// Scrolling over the picture: gathered, then sent as swipes once it pauses (or has gathered
    /// a while), in the direction the content went on the Mac.
    pub(super) fn scroll_wheel(
        &mut self,
        e: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(size) = self.picture_size() else {
            return;
        };
        if self.touch.press.is_some() || !self.picture().is_some_and(|b| b.contains(&e.position)) {
            return;
        }
        self.note_input();
        let now = Instant::now();
        let going = self
            .touch
            .scrolled
            .is_some_and(|t| now.duration_since(t) < SCROLL_PAUSE);
        let wheel = matches!(e.delta, ScrollDelta::Lines(_));
        let settled = self
            .touch
            .hovered
            .is_some_and(|t| now.duration_since(t) >= HOVER_SETTLE);
        if !(going || wheel || settled || e.touch_phase == TouchPhase::Started) {
            return;
        }
        cx.stop_propagation();
        self.touch.scrolled = Some(now);
        let delta = e.delta.pixel_delta(px(WHEEL_LINE));
        let delta = (f64::from(delta.x.as_f32()), f64::from(delta.y.as_f32()));
        if delta == (0., 0.) {
            return;
        }
        if self.touch.scroll.is_none() {
            let Some(at) = self.to_fraction(e.position) else {
                return;
            };
            if !going {
                self.user_wins(cx);
            }
            self.touch.scroll = Some(Scroll {
                at,
                delta: (0., 0.),
                first: now,
            });
        }
        let Some(s) = self.touch.scroll.as_mut() else {
            return;
        };
        s.delta = (s.delta.0 + delta.0, s.delta.1 + delta.1);
        let far = s.delta.0.abs() >= size.0 * 0.5 || s.delta.1.abs() >= size.1 * 0.5;
        if far || now.duration_since(s.first) >= SCROLL_GATHER {
            self.flush_scroll(window, cx);
            return;
        }
        self.touch.scroll_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SCROLL_PAUSE).await;
            let _ = this.update_in(cx, |this, window, cx| this.flush_scroll(window, cx));
        }));
    }

    fn flush_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.touch.scroll_task = None;
        let (Some(s), Some(size)) = (self.touch.scroll.take(), self.picture_size()) else {
            return;
        };
        for swipe in gesture::scroll(s.at, s.delta, size) {
            self.send_input(swipe, window, cx);
        }
    }

    /// A key while the picture has the keys: to the phone, except Trek's own shortcuts. Escape
    /// goes to the phone too and gives the keys back.
    pub(super) fn key_down(
        &mut self,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &e.keystroke;
        let m = &k.modifiers;
        let mods = Mods {
            platform: m.platform,
            control: m.control,
            alt: m.alt,
            shift: m.shift,
        };
        let input = match gesture::key(&k.key, k.key_char.as_deref(), mods) {
            Key::Trek => return,
            Key::Paste => match cx.read_from_clipboard().and_then(|c| c.text()) {
                Some(text) if !text.is_empty() => Input::Text(text),
                _ => {
                    cx.stop_propagation();
                    return;
                }
            },
            Key::Escape => {
                cx.stop_propagation();
                self.give_keys_back(window, cx);
                if self.touchable() {
                    let escape = Input::Key {
                        name: "escape".into(),
                        times: 1,
                    };
                    self.send_input(escape, window, cx);
                }
                return;
            }
            Key::Phone(input) => input,
        };
        cx.stop_propagation();
        self.user_wins(cx);
        self.send_input(input, window, cx);
    }

    /// One input from the user to the phone, by the route taps take now.
    fn send_input(&mut self, input: Input, window: &mut Window, cx: &mut Context<Self>) {
        if self.facts().route() == Route::Driver {
            let merged = self
                .touch
                .queue
                .back_mut()
                .is_some_and(|last| last.merge(&input));
            if !merged && self.touch.queue.len() < QUEUE_MAX {
                self.touch.queue.push_back(input);
            }
            self.pump(window, cx);
            return;
        }
        let Some(link) = self.link.clone() else {
            self.cant_tap(cx);
            return;
        };
        if self.status.input_ready() {
            match input.hid() {
                Ok(steps) => {
                    if input.touches() {
                        self.ensure_assistive(window, cx);
                    }
                    let _ = link.hid(&steps);
                }
                Err(e) => self.notice = Some(e.into()),
            }
        } else if self.mirror_frames {
            // The picture is iPhone Mirroring's window: its fractions are the window's.
            match input.mirror() {
                Ok(cmds) => {
                    for cmd in cmds {
                        let _ = link.mirror(&cmd);
                    }
                }
                Err(e) => self.notice = Some(e.into()),
            }
        } else {
            self.cant_tap(cx);
        }
        cx.notify();
    }

    /// The next of the user's inputs to Trek Driver (started if need be), once the one before
    /// it is done.
    fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.touch.sending || self.touch.queue.is_empty() {
            return;
        }
        let (Some(install), Some(udid), driver::Avail::Ready) = (
            self.driver_install.clone(),
            self.phone.as_ref().map(|p| p.udid.clone()),
            self.driver_avail(),
        ) else {
            self.touch.queue.clear();
            self.cant_tap(cx);
            return;
        };
        let Some(input) = self.touch.queue.pop_front() else {
            return;
        };
        // Escape with nothing to escape from (no keyboard on the phone) says nothing.
        let quiet = matches!(&input, Input::Key { name, .. } if name == "escape");
        self.touch.sending = true;
        let d = self.driver.clone();
        cx.notify();
        self.background(
            window,
            cx,
            move || {
                let screen = d.ensure(&install, &udid, driver::START_LIMIT)?;
                driver::run(&*d, &input.driver(screen, &|f| d.takes(f)))
            },
            move |this, result, window, cx| {
                this.touch.sending = false;
                if let Err(e) = result
                    && !quiet
                {
                    // The rest would only fail the same way (the driver didn't start, say).
                    this.touch.queue.clear();
                    this.notice = Some(e.into());
                }
                this.pump(window, cx);
                cx.notify();
            },
        );
    }

    /// The touch drawn over the picture `s` (in the mirror area) while the button is down: a dot
    /// under the pointer, ringed once it's a long press, and the trail of a drag.
    pub(super) fn touch_marks(&self, s: Bounds<Pixels>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(press) = &self.touch.press else {
            return vec![];
        };
        let ring = crate::palette::sky(cx);
        let (w, h) = (s.size.width.as_f32(), s.size.height.as_f32());
        let local =
            |x: f64, y: f64| point(s.origin.x + px(x as f32 * w), s.origin.y + px(y as f32 * h));
        let mut trail: Vec<Point<Pixels>> = press
            .stroke
            .spots()
            .iter()
            .map(|p| local(p.x, p.y))
            .collect();
        let pointer = local(press.pointer.0, press.pointer.1);
        trail.push(pointer);
        let long = press.stroke.long_press(millis(press.at));
        // scale: ok, a fingertip on the phone's own scale, a little larger once it's a long press
        let size = px((w * if long { 0.12 } else { 0.1 }).clamp(20., 52.));
        let mut out = vec![];
        if press.stroke.moved() {
            out.push(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let at = |p: Point<Pixels>| bounds.origin + p;
                        // Dark under light: it shows on any screen.
                        let strokes = [
                            (px(6.), gpui_kit::black().opacity(0.25)),
                            (px(3.), gpui_kit::white().opacity(0.85)),
                        ];
                        for (width, color) in strokes {
                            let mut path = PathBuilder::stroke(width);
                            path.move_to(at(trail[0]));
                            for p in &trail[1..] {
                                path.line_to(at(*p));
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .into_any_element(),
            );
        }
        out.push(
            div()
                .id("phone-touch")
                .test_support()
                .absolute()
                .left(pointer.x - size / 2.)
                .top(pointer.y - size / 2.)
                .size(size)
                .rounded_full()
                .bg(gpui_kit::white().opacity(0.45))
                .border_2()
                .border_color(if long {
                    ring
                } else {
                    gpui_kit::black().opacity(0.3)
                })
                .into_any_element(),
        );
        out
    }
}

/// A made-up Home Screen for design review (`control phone touch`): a wallpaper, rows of icons
/// and a dock, as a JPEG the size of the driver's picture.
#[cfg(feature = "shots")]
pub(super) fn demo_home_screen() -> Vec<u8> {
    let (w, h) = (440u32, 956u32);
    let mut img = image::RgbImage::from_fn(w, h, |_, y| {
        let t = y as f32 / h as f32;
        image::Rgb([
            (38. + 70. * t) as u8,
            (52. + 24. * t) as u8,
            (110. - 30. * t) as u8,
        ])
    });
    let colors = [
        [255, 94, 58],
        [52, 199, 89],
        [0, 122, 255],
        [255, 204, 0],
        [175, 82, 222],
        [255, 45, 85],
        [90, 200, 250],
        [142, 142, 147],
    ];
    let mut icon = |x0: u32, y0: u32, side: u32, c: [u8; 3]| {
        let r = side as f32 * 0.23;
        for y in y0..y0 + side {
            for x in x0..x0 + side {
                let (dx, dy) = ((x - x0) as f32, (y - y0) as f32);
                let cx = dx.clamp(r, side as f32 - r);
                let cy = dy.clamp(r, side as f32 - r);
                if (dx - cx).hypot(dy - cy) <= r {
                    img.put_pixel(x, y, image::Rgb(c));
                }
            }
        }
    };
    for row in 0..5u32 {
        for col in 0..4u32 {
            icon(
                34 + col * 104,
                96 + row * 112,
                64,
                colors[((row * 4 + col) % 8) as usize],
            );
        }
    }
    for col in 0..4u32 {
        icon(34 + col * 104, 862, 64, colors[((col + 3) % 8) as usize]);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    let _ = img.write_to(&mut out, image::ImageFormat::Jpeg);
    out.into_inner()
}

impl PhonePanel {
    /// The user's touch in one of its looks, for design review: `touch` a drag under way (an
    /// icon held, then moved: its trail and dot), `hold` a press held into a long press,
    /// `typing` the keys on the phone.
    #[cfg(feature = "shots")]
    pub(super) fn demo_touch(&mut self, state: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.touch.press = None;
        let started = Instant::now() - Duration::from_millis(1400);
        match state {
            "touch" => {
                let mut stroke = Stroke::new((0.17, 0.14), (300., 652.));
                let mut at = (0.17, 0.14);
                for i in 1..=24 {
                    let t = f64::from(i) / 24.;
                    at = (0.17 + 0.5 * t, 0.14 + 0.36 * t * t);
                    stroke.push(at, 700 + i * 20);
                }
                self.touch.press = Some(Press {
                    stroke,
                    at: started,
                    pointer: at,
                });
            }
            "hold" => {
                self.touch.press = Some(Press {
                    stroke: Stroke::new((0.62, 0.38), (300., 652.)),
                    at: started,
                    pointer: (0.62, 0.38),
                });
            }
            "typing" => self.take_keys(window, cx),
            _ => {}
        }
    }
}
