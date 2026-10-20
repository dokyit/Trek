//! Background computer use, watched. An agent working the computer in the background
//! (`trek-mcp computer` with `TREK_COMPUTER_MODE=background`) clicks and types in a window of
//! its own behind the user's, and tells Trek each step (`trek_ipc::COMPUTER_ACTIVITY`). Trek
//! shows it in a small floating window, picture in picture: the agent's window, live, with
//! where it last pointed, what it's doing ("Typing “hello”") and for which thread.
//!
//! The window floats over every app without taking the keyboard (a non-activating panel), and
//! can be dragged anywhere. The user can fold it to a slim pill, or put it away altogether;
//! it comes back from the menu bar icon, the command palette or the thread's working bar
//! ("Show"). It closes by itself a few seconds after the last turn using the computer ends.
//!
//! Frames are the agent's window as the window server has it (`CGWindowListCreateImage`, about
//! 12 ms at its size in points), four times a second while the picture is unfolded: the window
//! is drawn from its own buffer, so what covers it doesn't show.

use crate::workspace::{Route, Workspace};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::assets::Lucide;

/// The picture, unfolded: header, frame, caption.
const WIDTH: f32 = 340.;
const HEADER: f32 = 34.;
const FRAME: f32 = 196.;
const CAPTION: f32 = 46.;
/// Folded to a pill.
const PILL_W: f32 = 260.;
const PILL_H: f32 = 40.;
/// From the screen's edges, at first.
const MARGIN: f32 = 18.;
/// How often a new frame is captured while unfolded.
const FRAME_EVERY: Duration = Duration::from_millis(250);
/// How long the picture stays once a thread's turn is over, so its last step can be seen.
const LINGER: Duration = Duration::from_millis(if cfg!(test) { 200 } else { 5000 });
/// How often sessions are checked for turns that are over.
const SWEEP: Duration = Duration::from_millis(if cfg!(test) { 50 } else { 1000 });
/// How long a click's ring shows.
const RING: Duration = Duration::from_millis(900);

/// The window an agent works in, as it last said.
#[derive(Debug, Clone, PartialEq)]
pub struct Watched {
    pub id: i64,
    pub app: String,
    pub title: String,
    /// Its size in points, which `Step` points are in.
    pub width: f64,
    pub height: f64,
}

/// The agent's latest step there.
#[derive(Debug, Clone)]
pub struct Step {
    pub action: String,
    /// "Clicking", "Typing “hello”".
    pub detail: String,
    /// Where, in the window's own points (origin top-left).
    pub point: Option<(f64, f64)>,
    pub at: Instant,
}

pub struct Session {
    pub thread: String,
    pub window: Watched,
    pub step: Step,
    /// Where the agent last pointed, kept through steps that point nowhere (typing, keys), as
    /// a mouse pointer stays where it was left.
    pub pointer: Option<(f64, f64)>,
    /// When its thread's turn was seen to be over.
    ended: Option<Instant>,
}

/// A report from `trek-mcp computer`, as it sends it (see `trek_ipc::COMPUTER_ACTIVITY`).
pub fn parse(params: &Value) -> Option<(Watched, Step)> {
    let w = params.get("window")?;
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let window = Watched { id: w.get("id")?.as_i64()?, app: text(w, "app"), title: text(w, "title"), width: num(w, "width").unwrap_or(0.), height: num(w, "height").unwrap_or(0.) };
    let point = params.get("point").and_then(|p| Some((num(p, "x")?, num(p, "y")?)));
    Some((window, Step { action: text(params, "action"), detail: text(params, "detail"), point, at: Instant::now() }))
}

/// Who's using the computer in the background, and the picture-in-picture window showing it.
pub struct BackgroundUse {
    workspace: WeakEntity<Workspace>,
    /// Newest step first.
    pub sessions: Vec<Session>,
    /// The thread the picture follows, when the user picked one; else the newest step's.
    pinned: Option<String>,
    /// Put away by the user: it stays away until they bring it back.
    pub tucked: bool,
    /// Folded to a pill.
    pub folded: bool,
    /// The user brought the picture up by hand; with `tools.background_pip` off, the agent's next
    /// step leaves it be. Until they put it away, or the sessions end.
    asked: bool,
    window: Option<AnyWindowHandle>,
    /// It's about to open (`open`).
    opening: bool,
    /// Where the user left it.
    origin: Option<Point<Pixels>>,
    /// Ends sessions whose turns are over.
    _watch: Option<Task<()>>,
}

impl BackgroundUse {
    pub fn new(workspace: WeakEntity<Workspace>) -> Self {
        Self { workspace, sessions: vec![], pinned: None, tucked: false, folded: false, asked: false, window: None, opening: false, origin: None, _watch: None }
    }

    /// The session on show.
    pub fn current(&self) -> Option<&Session> {
        self.pinned.as_ref().and_then(|t| self.sessions.iter().find(|s| &s.thread == t)).or(self.sessions.first())
    }

    pub fn session(&self, thread: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.thread == thread)
    }

    /// The picture is on screen.
    pub fn showing(&self) -> bool {
        self.window.is_some() || self.opening
    }

    /// A step reported by `thread`'s agent; `picture`: the user wants it shown
    /// (`tools.background_pip`). Called while the workspace is being updated, so it doesn't
    /// read the workspace.
    pub fn record(&mut self, thread: &str, params: &Value, picture: bool, cx: &mut Context<Self>) {
        let Some((window, step)) = parse(params) else { return };
        // A new window starts with no pointer on it.
        let kept = self.sessions.iter().find(|s| s.thread == thread && s.window.id == window.id).and_then(|s| s.pointer);
        self.sessions.retain(|s| s.thread != thread);
        let pointer = step.point.or(kept);
        self.sessions.insert(0, Session { thread: thread.to_string(), window, step, pointer, ended: None });
        self.watch(cx);
        if !picture {
            if !self.asked {
                self.close(cx);
            }
        } else if !self.tucked {
            self.open(cx);
        }
        cx.notify();
    }

    /// Bring the picture back (or show it the first time).
    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.tucked = false;
        self.set_folded(false, cx);
        if !self.sessions.is_empty() {
            self.asked = true;
            self.open(cx);
        }
        cx.notify();
    }

    /// Put it away until the user brings it back.
    pub fn tuck(&mut self, cx: &mut Context<Self>) {
        self.tucked = true;
        self.asked = false;
        self.close(cx);
        cx.notify();
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.showing() { self.tuck(cx) } else { self.show(cx) }
    }

    pub fn set_folded(&mut self, folded: bool, cx: &mut Context<Self>) {
        if self.folded == folded {
            return;
        }
        self.folded = folded;
        let shape = Self::size(folded);
        if let Some(w) = self.window {
            cx.defer(move |cx| {
                let _ = w.update(cx, |_, window, _| window.resize(shape));
            });
        }
        cx.notify();
    }

    /// Follow another thread's session, when several use the computer at once.
    pub fn next(&mut self, cx: &mut Context<Self>) {
        let Some(at) = self.current().and_then(|c| self.sessions.iter().position(|s| s.thread == c.thread)) else { return };
        self.pinned = self.sessions.get((at + 1) % self.sessions.len()).map(|s| s.thread.clone());
        cx.notify();
    }

    fn size(folded: bool) -> Size<Pixels> {
        if folded { size(px(PILL_W), px(PILL_H)) } else { size(px(WIDTH), px(HEADER + FRAME + CAPTION)) }
    }

    /// Open the picture, once the update asking for it is over: a window draws its first frame
    /// as it opens, and the frame reads this model (and the workspace, which may be mid-update).
    fn open(&mut self, cx: &mut Context<Self>) {
        if self.window.is_some() || self.opening {
            return;
        }
        self.opening = true;
        let this = cx.entity();
        cx.defer(move |cx| {
            let options = this.update(cx, |b, cx| {
                b.opening = false;
                (b.window.is_none() && !b.tucked && !b.sessions.is_empty()).then(|| b.options(cx))
            });
            let Some(options) = options else { return };
            let model = this.clone();
            match gpui_kit::open_window(options, cx, move |window, cx| cx.new(|cx| Pip::new(model, window, cx))) {
                Ok((handle, _)) => this.update(cx, |b, _| b.window = Some(handle)),
                Err(e) => tracing::warn!("background use window: {e:#}"),
            }
        });
    }

    fn options(&self, cx: &App) -> WindowOptions {
        let shape = Self::size(self.folded);
        let origin = self.origin.unwrap_or_else(|| {
            let screen = cx.primary_display().map(|d| d.visible_bounds()).unwrap_or(Bounds { origin: point(px(0.), px(0.)), size: size(px(1440.), px(900.)) });
            point(screen.origin.x + screen.size.width - shape.width - px(MARGIN), screen.origin.y + screen.size.height - shape.height - px(MARGIN))
        });
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size: shape })),
            titlebar: None,
            // Floats over every app and Space, and never takes the keyboard from the user.
            kind: WindowKind::PopUp,
            focus: false,
            show: true,
            is_movable: true,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some("dev.trek.Trek".into()),
            ..Default::default()
        }
    }

    /// Close the picture, remembering where it was; after the update, like `open`.
    fn close(&mut self, cx: &mut Context<Self>) {
        let Some(w) = self.window.take() else { return };
        let this = cx.entity();
        cx.defer(move |cx| {
            let origin = w.update(cx, |_, window, _| {
                let origin = window.bounds().origin;
                window.remove_window();
                origin
            });
            if let Ok(origin) = origin {
                this.update(cx, |b, _| b.origin = Some(origin));
            }
        });
    }

    /// Once a second while anyone uses the computer: sessions whose turns have been over for
    /// `LINGER` (or whose threads are gone) end, and with the last of them the picture closes.
    fn watch(&mut self, cx: &mut Context<Self>) {
        if self._watch.is_some() {
            return;
        }
        self._watch = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(SWEEP).await;
            let Ok(done) = this.update(cx, |this, cx| this.sweep(cx)) else { break };
            if done {
                break;
            }
        }));
    }

    /// Returns whether no session is left.
    fn sweep(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(ws) = self.workspace.upgrade() else { return true };
        let now = Instant::now();
        let before = self.sessions.len();
        for s in &mut self.sessions {
            let ws = ws.read(cx);
            let working = ws.thread(&s.thread).is_some_and(|t| t.run_state == trek_core::RunState::Working || ws.waiting(&s.thread));
            s.ended = if working { None } else { s.ended.or(Some(now)) };
        }
        let ws = ws.read(cx);
        self.sessions.retain(|s| ws.thread(&s.thread).is_some() && s.ended.is_none_or(|e| now - e < LINGER));
        if self.pinned.as_ref().is_some_and(|p| !self.sessions.iter().any(|s| &s.thread == p)) {
            self.pinned = None;
        }
        let empty = self.sessions.is_empty();
        if empty {
            self.close(cx);
            // The next time the computer is used in the background, the picture shows again.
            self.tucked = false;
            self.asked = false;
            self._watch = None;
        }
        if self.sessions.len() != before {
            cx.notify();
        }
        empty
    }

    /// Open the session's thread in Trek.
    fn open_thread(model: &Entity<Self>, cx: &mut App) {
        let b = model.read(cx);
        let (Some(ws), Some(thread)) = (b.workspace.upgrade(), b.current().map(|s| s.thread.clone())) else { return };
        cx.activate(true);
        ws.update(cx, |ws, cx| ws.show_in_main(Route::Thread(thread), cx));
    }
}

/// A frame of the agent's window: BGRA pixels, as the window server draws it.
struct Frame {
    image: Arc<RenderImage>,
    width: f32,
    height: f32,
    window: i64,
}

/// The picture-in-picture window's view.
pub struct Pip {
    model: Entity<BackgroundUse>,
    frame: Option<Frame>,
    /// Frames replaced, released once the next one is drawn.
    stale: Vec<Arc<RenderImage>>,
    capturing: bool,
    _frames: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Pip {
    fn new(model: Entity<BackgroundUse>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let frames = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(FRAME_EVERY).await;
            if this.update_in(cx, |this, window, cx| this.capture(window, cx)).is_err() {
                break;
            }
        });
        // However the window goes, the model stops counting it as on screen.
        let handle = window.window_handle();
        let gone = cx.on_release(move |this: &mut Self, cx| {
            let model = this.model.clone();
            cx.defer(move |cx| model.update(cx, |b, cx| {
                if b.window == Some(handle) {
                    b.window = None;
                    cx.notify();
                }
            }));
        });
        Self { model: model.clone(), frame: None, stale: vec![], capturing: false, _frames: frames, _subscriptions: vec![cx.observe(&model, |_, _, cx| cx.notify()), gone] }
    }

    fn capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Clicks' rings fade on the frame clock.
        cx.notify();
        let m = self.model.read(cx);
        if self.capturing || m.folded {
            return;
        }
        let Some(id) = m.current().map(|s| s.window.id) else { return };
        self.capturing = true;
        let task = cx.background_executor().spawn(async move { grab(id) });
        cx.spawn_in(window, async move |this, cx| {
            let grabbed = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.capturing = false;
                let Some((bgra, w, h)) = grabbed else { return };
                let Some(pixels) = image::RgbaImage::from_raw(w, h, bgra) else { return };
                for old in this.stale.drain(..) {
                    cx.drop_image(old, Some(&mut *window));
                }
                let image = Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
                if let Some(old) = this.frame.replace(Frame { image, width: w as f32, height: h as f32, window: id }) {
                    this.stale.push(old.image);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// Window `id` as BGRA pixels at its size in points.
#[cfg(target_os = "macos")]
fn grab(id: i64) -> Option<(Vec<u8>, u32, u32)> {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use core_graphics::window::{create_image, kCGWindowImageBoundsIgnoreFraming, kCGWindowImageNominalResolution, kCGWindowListOptionIncludingWindow};
    let null = CGRect::new(&CGPoint::new(f64::INFINITY, f64::INFINITY), &CGSize::new(0., 0.));
    let image = create_image(null, kCGWindowListOptionIncludingWindow, u32::try_from(id).ok()?, kCGWindowImageBoundsIgnoreFraming | kCGWindowImageNominalResolution)?;
    let (w, h, row) = (image.width(), image.height(), image.bytes_per_row());
    if w == 0 || h == 0 || image.bits_per_pixel() != 32 {
        return None;
    }
    let data = image.data();
    let bytes = data.bytes();
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        out.extend_from_slice(bytes.get(y * row..y * row + w * 4)?);
    }
    Some((out, w as u32, h as u32))
}

#[cfg(not(target_os = "macos"))]
fn grab(_: i64) -> Option<(Vec<u8>, u32, u32)> {
    None
}

/// Where a frame of `fw`×`fh` sits in a box of `bw`×`bh`, kept whole: its offset and scale.
fn fit(fw: f32, fh: f32, bw: f32, bh: f32) -> (f32, f32, f32) {
    let s = (bw / fw.max(1.)).min(bh / fh.max(1.));
    ((bw - fw * s) / 2., (bh - fh * s) / 2., s)
}

impl Render for Pip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = self.model.read(cx);
        let theme = cx.theme();
        let (fg, muted, border) = (theme.foreground, theme.muted_foreground, theme.border);
        let card = theme.background.opacity(0.94);
        let live = crate::palette::emerald(cx);
        let accent = theme.primary;
        let Some(s) = m.current() else { return div().size_full().into_any_element() };
        let count = m.sessions.len();
        let app = s.window.app.clone();
        let detail = if s.step.detail.is_empty() { "Working".to_string() } else { s.step.detail.clone() };
        let thread = self.model.read(cx).workspace.upgrade().and_then(|ws| ws.read(cx).thread(&s.thread).map(|t| t.title.clone())).unwrap_or_default();
        let model = self.model.clone();
        let dot = div().flex_none().size(px(7.)).rounded_full().bg(live);
        let icon_button = |id: &'static str, icon: Lucide, tip: SharedString| {
            div()
                .id(id)
                .flex_none()
                .size(px(22.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(muted)
                .hover(|el| el.bg(fg.opacity(0.08)).text_color(fg))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                .child(Icon::new(icon).xsmall())
        };
        let shell = div().size_full().rounded(px(12.)).bg(card).border_1().border_color(border).overflow_hidden().text_color(fg);

        if m.folded {
            let model = model.clone();
            return shell
                .id("background-use-pill")
                .rounded_full()
                .child(
                    h_flex()
                        .size_full()
                        .px(px(14.))
                        .gap(px(8.))
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                        .child(dot)
                        .child(div().flex_none().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).child(app))
                        .child(div().min_w_0().flex_1().truncate().text_size(px(12.)).text_color(muted).child(detail))
                        .child(icon_button("background-use-unfold", Lucide::Maximize2, "Show the picture".into()).on_click(move |_, _, cx| model.update(cx, |m, cx| m.set_folded(false, cx)))),
                )
                .into_any_element();
        }

        // The frame, kept whole in its box, and where the agent last pointed on it.
        let box_w = WIDTH - 2.;
        let frame = self.frame.as_ref().filter(|f| f.window == s.window.id);
        let picture = match frame {
            Some(f) => {
                let (x0, y0, k) = fit(f.width, f.height, box_w, FRAME);
                // The frame is the window at its size in points, so a step's point scales with it.
                let per_point = k * f.width / (s.window.width as f32).max(1.);
                let pointer = s.pointer.map(|(x, y)| (x0 + x as f32 * per_point, y0 + y as f32 * per_point));
                let since = s.step.at.elapsed();
                let ring = (s.step.action == "click" && since < RING).then(|| since.as_secs_f32() / RING.as_secs_f32());
                div()
                    .relative()
                    .size_full()
                    .child(img(f.image.clone()).absolute().left(px(x0)).top(px(y0)).w(px(f.width * k)).h(px(f.height * k)))
                    .children(ring.zip(pointer).map(|(t, (x, y))| {
                        let r = 6. + 16. * t;
                        div().absolute().left(px(x - r)).top(px(y - r)).size(px(r * 2.)).rounded_full().border_2().border_color(accent.opacity(0.9 * (1. - t)))
                    }))
                    .children(pointer.map(|(x, y)| div().absolute().left(px(x - 5.)).top(px(y - 5.)).size(px(10.)).rounded_full().bg(accent).border_2().border_color(gpui_kit::white().opacity(0.9))))
                    .into_any_element()
            }
            None => h_flex().size_full().justify_center().text_size(px(12.)).text_color(muted).child(format!("Watching {app}…")).into_any_element(),
        };
        let title = if s.window.title.is_empty() { app.clone() } else { format!("{app} · {}", s.window.title) };
        let (m1, m2, m3) = (model.clone(), model.clone(), model.clone());
        let open = model.clone();
        let _ = window;
        shell
            .id("background-use")
            .child(
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .id("background-use-header")
                            .h(px(HEADER))
                            .flex_none()
                            .px(px(12.))
                            .gap(px(7.))
                            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                            .child(dot)
                            .child(div().flex_none().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child("Background use"))
                            .child(div().min_w_0().flex_1().truncate().text_size(px(12.)).text_color(muted).child(title))
                            .when(count > 1, |el| el.child(icon_button("background-use-next", Lucide::ArrowLeftRight, "Show the next thread using the computer".into()).on_click(move |_, _, cx| m1.update(cx, |m, cx| m.next(cx)))))
                            .child(icon_button("background-use-fold", Lucide::Minus, "Fold to a pill".into()).on_click(move |_, _, cx| m2.update(cx, |m, cx| m.set_folded(true, cx))))
                            .child(icon_button("background-use-hide", Lucide::X, format!("Put away (bring it back from the {})", crate::words::words().tray_label.to_lowercase()).into()).on_click(move |_, _, cx| m3.update(cx, |m, cx| m.tuck(cx)))),
                    )
                    .child(div().h(px(FRAME)).flex_none().mx(px(1.)).bg(gpui_kit::black().opacity(0.35)).overflow_hidden().child(picture))
                    .child(
                        h_flex()
                            .id("background-use-caption")
                            .h(px(CAPTION))
                            .flex_none()
                            .px(px(12.))
                            .gap(px(8.))
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .flex_1()
                                    .child(div().truncate().text_size(px(12.5)).child(detail))
                                    .child(div().truncate().text_size(px(11.)).text_color(muted).child(if count > 1 { format!("{thread} · {count} threads") } else { thread })),
                            )
                            .child(icon_button("background-use-open", Lucide::ArrowUpRight, "Open the thread in Trek".into()).on_click(move |_, _, cx| BackgroundUse::open_thread(&open, cx))),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Watched, fit, parse};
    use serde_json::json;

    #[test]
    fn reports_are_read_as_trek_mcp_sends_them() {
        let (w, step) = parse(&json!({
            "action": "click", "detail": "Clicking",
            "window": {"id": 7, "pid": 42, "app": "Safari", "title": "Docs", "x": 400.0, "y": 100.0, "width": 800.0, "height": 600.0},
            "point": {"x": 20.0, "y": 30.0}, "from": null,
        }))
        .unwrap();
        assert_eq!(w, Watched { id: 7, app: "Safari".into(), title: "Docs".into(), width: 800., height: 600. });
        assert_eq!((step.action.as_str(), step.detail.as_str(), step.point), ("click", "Clicking", Some((20., 30.))));
        let (_, keys) = parse(&json!({"action": "key", "detail": "Pressing return", "window": {"id": 7}, "point": null})).unwrap();
        assert_eq!(keys.point, None);
        assert!(parse(&json!({"action": "click"})).is_none(), "no window, nothing to show");
    }

    #[test]
    fn frames_are_kept_whole_in_their_box() {
        // A wide window letterboxed top and bottom, a tall one pillarboxed.
        assert_eq!(fit(800., 400., 340., 196.), (0., 13., 0.425));
        let (x, y, k) = fit(200., 400., 340., 196.);
        assert_eq!((y, k), (0., 0.49));
        assert!((x - 121.).abs() < 0.01);
    }
}
