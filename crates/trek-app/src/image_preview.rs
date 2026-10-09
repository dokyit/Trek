//! Images up close without leaving Trek: a click on an attachment (the composer's outbox, a sent
//! message, an image or image link in an answer) lays it over the window, fit to it, with its
//! name, size and a few actions. ←/→ step through the others it came with, a click or Space shows
//! it at actual pixels (drag or scroll to pan), Esc or a click beside it puts it away. Files Trek
//! can't draw go to Quick Look instead.
//!
//! Each window (the main one, thread windows) has one, drawn over everything else in it; whatever
//! is clicked finds its window's through `open`.

use crate::ui;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The slim bar along the top: name, size, actions.
const HEADER: f32 = 52.;
/// Room left and right of the image, for the chevrons.
const SIDE: f32 = 72.;
/// Below the image: the filmstrip when there are several, else a margin.
const FILMSTRIP: f32 = 76.;
const BOTTOM: f32 = 36.;
const THUMB: f32 = 40.;
/// How long it takes to come in and to go.
const OPENING: Duration = Duration::from_millis(170);
const CLOSING: Duration = Duration::from_millis(120);
/// A press on the actual-size image that moves less than this is a click (back to fit), not a pan.
const CLICK_SLOP: f32 = 4.;

/// Takes an image out of the outbox it's previewed from (the composer's ×, by path rather than
/// position: the outbox may have changed since the preview opened).
pub type Remove = Rc<dyn Fn(&Path, &mut Window, &mut App)>;

/// Preview `paths` at `index` in this window. One Trek can't draw goes to Quick Look.
pub fn open(paths: Vec<PathBuf>, index: usize, window: &mut Window, cx: &mut App) {
    show(paths, index, None, window, cx);
}

/// Preview a composer's outbox at `index`: as `open`, with Remove taking an image out of it.
pub fn open_outbox(paths: Vec<PathBuf>, index: usize, remove: impl Fn(&Path, &mut Window, &mut App) + 'static, window: &mut Window, cx: &mut App) {
    show(paths, index, Some(Rc::new(remove)), window, cx);
}

/// Whether the preview can draw `path`: a raster image GPUI decodes, on disk.
pub fn showable(path: &Path) -> bool {
    crate::mentions::is_image(path) && path.is_file()
}

fn show(paths: Vec<PathBuf>, index: usize, remove: Option<Remove>, window: &mut Window, cx: &mut App) {
    let Some(clicked) = paths.get(index).cloned() else { return };
    if !showable(&clicked) {
        quick_look(&clicked);
        return;
    }
    // Only what it can draw comes along to step through.
    let items: Vec<PathBuf> = paths.into_iter().filter(|p| showable(p)).collect();
    let index = items.iter().position(|p| *p == clicked).unwrap_or(0);
    match in_window(window, cx) {
        Some(preview) => preview.update(cx, |p, cx| p.present(items, index, remove, window, cx)),
        None => cx.open_with_system(&clicked),
    }
}

/// macOS's Quick Look on `path`, for files Trek doesn't draw itself (PDFs, SVGs, HEIC photos).
fn quick_look(path: &Path) {
    #[cfg(test)]
    QUICK_LOOKED.with(|q| q.borrow_mut().push(path.to_path_buf()));
    #[cfg(not(test))]
    {
        let child = std::process::Command::new("/usr/bin/qlmanage")
            .arg("-p")
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            // Reaped when it closes, so it doesn't linger as a zombie.
            Ok(mut child) => _ = std::thread::spawn(move || child.wait()),
            Err(e) => tracing::warn!("quick look {}: {e}", path.display()),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// What went to Quick Look in this test (nothing is launched).
    pub static QUICK_LOOKED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Each window's preview, so a click anywhere in it finds the one to open.
#[derive(Default)]
struct Previews(HashMap<WindowId, WeakEntity<ImagePreview>>);

impl Global for Previews {}

fn in_window(window: &Window, cx: &App) -> Option<Entity<ImagePreview>> {
    cx.try_global::<Previews>()?.0.get(&window.window_handle().window_id())?.upgrade()
}

/// `path`'s width and height in pixels, from its file's header (read once per path).
pub fn pixels(path: &Path) -> Option<(u32, u32)> {
    static SEEN: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, Option<(u32, u32)>>>> = std::sync::LazyLock::new(Default::default);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    *seen.entry(path.to_path_buf()).or_insert_with(|| image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.into_dimensions().ok())
}

/// How wide a tile `height` tall shows `path` in its own proportions, between `min` and `max`
/// (a square when its size can't be read). Sent messages draw their images this way.
pub fn tile_width(path: &Path, height: Pixels, min: Pixels, max: Pixels) -> Pixels {
    let aspect = pixels(path).filter(|(w, h)| *w > 0 && *h > 0).map(|(w, h)| w as f32 / h as f32).unwrap_or(1.);
    (height * aspect).clamp(min, max)
}

/// An image being shown, with what the header says about it.
struct Shown {
    path: PathBuf,
    /// Pixel width and height, read from the file's header.
    pixels: Option<(u32, u32)>,
    bytes: Option<u64>,
}

impl Shown {
    fn new(path: PathBuf) -> Self {
        let pixels = pixels(&path);
        let bytes = std::fs::metadata(&path).ok().map(|m| m.len());
        Shown { path, pixels, bytes }
    }

    fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| self.path.display().to_string())
    }
}

struct Showing {
    items: Vec<Shown>,
    index: usize,
    /// At actual pixels rather than fit to the window.
    actual: bool,
    remove: Option<Remove>,
}

/// A press on the actual-size image: where it went down, the scroll offset then, and whether it
/// has moved enough to be a pan.
struct Drag {
    from: Point<Pixels>,
    offset: Point<Pixels>,
    panned: bool,
}

pub struct ImagePreview {
    showing: Option<Showing>,
    focus: FocusHandle,
    /// Where focus was before it opened; it goes back there on close.
    restore: Option<FocusHandle>,
    /// The actual-size image's pan.
    scroll: ScrollHandle,
    drag: Option<Drag>,
    /// When it last started coming in (`false`) or going (`true`), while that runs.
    motion: Option<(Instant, bool)>,
}

impl ImagePreview {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let me = cx.entity().downgrade();
        let id = window.window_handle().window_id();
        let previews = cx.default_global::<Previews>();
        previews.0.retain(|_, p| p.upgrade().is_some());
        previews.0.insert(id, me);
        ImagePreview { showing: None, focus: cx.focus_handle(), restore: None, scroll: ScrollHandle::new(), drag: None, motion: None }
    }

    pub fn is_open(&self) -> bool {
        self.showing.is_some() && !self.closing()
    }

    /// The image on screen and whether it's at actual pixels.
    #[cfg(test)]
    pub fn current(&self) -> Option<(PathBuf, bool)> {
        self.showing.as_ref().filter(|_| !self.closing()).map(|s| (s.items[s.index].path.clone(), s.actual))
    }

    fn closing(&self) -> bool {
        matches!(self.motion, Some((_, true)))
    }

    fn animate(&self, cx: &App) -> bool {
        crate::workspace::workspace_global(cx).read(cx).motion(cx)
    }

    fn present(&mut self, items: Vec<PathBuf>, index: usize, remove: Option<Remove>, window: &mut Window, cx: &mut Context<Self>) {
        if items.is_empty() {
            return;
        }
        // Opened again while open: focus already went where it goes back to.
        if !self.is_open() {
            self.restore = window.focused(cx).filter(|f| *f != self.focus);
        }
        let index = index.min(items.len() - 1);
        self.showing = Some(Showing { items: items.into_iter().map(Shown::new).collect(), index, actual: false, remove });
        self.drag = None;
        self.motion = self.animate(cx).then(|| (Instant::now(), false));
        self.focus.focus(window, cx);
        set_overlay(true, cx);
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_open() {
            return;
        }
        if let Some(h) = self.restore.take() {
            h.focus(window, cx);
        }
        self.drag = None;
        if self.animate(cx) {
            self.motion = Some((Instant::now(), true));
        } else {
            self.finish_close(cx);
        }
        set_overlay(false, cx);
        cx.notify();
    }

    fn finish_close(&mut self, cx: &mut Context<Self>) {
        self.showing = None;
        self.motion = None;
        cx.notify();
    }

    /// Step `by` images (wrapping round), fit to the window again.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(s) = self.showing.as_mut() else { return };
        let n = s.items.len() as isize;
        if n < 2 {
            return;
        }
        s.index = (s.index as isize + by).rem_euclid(n) as usize;
        s.actual = false;
        self.drag = None;
        cx.notify();
    }

    fn go_to(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(s) = self.showing.as_mut().filter(|s| ix < s.items.len()) {
            s.index = ix;
            s.actual = false;
            self.drag = None;
            cx.notify();
        }
    }

    /// Fit ⇄ actual pixels. `at`: the point to keep under the pointer (a click), else the centre.
    fn toggle_actual(&mut self, at: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        let (viewport, scale) = (window.viewport_size(), window.scale_factor());
        let Some(s) = self.showing.as_mut() else { return };
        let Some(layout) = Layout::of(&s.items[s.index], s.items.len(), viewport, scale) else { return };
        if !s.actual && !layout.zoomable() {
            return;
        }
        s.actual = !s.actual;
        if s.actual {
            // Keep the spot that was clicked (or the middle) where it was on screen.
            let fit = layout.fit_bounds();
            let at = at.filter(|p| fit.contains(p)).unwrap_or(fit.center());
            let (fx, fy) = ((at.x - fit.origin.x) / fit.size.width, (at.y - fit.origin.y) / fit.size.height);
            let image = layout.actual_bounds_in_content();
            let target = point(image.origin.x + image.size.width * fx - (at.x - layout.pan.origin.x), image.origin.y + image.size.height * fy - (at.y - layout.pan.origin.y));
            self.scroll.set_offset(clamp_offset(point(-target.x, -target.y), layout.content(), layout.pan.size));
        }
        cx.notify();
    }

    fn set_actual(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.showing.as_ref().is_some_and(|s| s.actual != on) {
            self.toggle_actual(None, window, cx);
        }
    }

    fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.showing.as_ref().map(|s| s.items[s.index].path.clone()) else { return };
        match std::fs::read(&path) {
            Ok(bytes) => {
                cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(format_of(&path), bytes)));
                gpui_kit::component::WindowExt::push_notification(window, "Image copied", cx);
            }
            Err(e) => gpui_kit::component::WindowExt::push_notification(window, format!("Couldn't copy the image: {e}"), cx),
        }
    }

    /// Take the image on screen out of the outbox; the next one shows, or the preview closes
    /// with the last.
    fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.showing.as_mut() else { return };
        let Some(remove) = s.remove.clone() else { return };
        let gone = s.items.remove(s.index);
        s.actual = false;
        let empty = s.items.is_empty();
        if !empty {
            s.index = s.index.min(s.items.len() - 1);
        }
        remove(&gone.path, window, cx);
        if empty {
            self.close(window, cx);
        }
        cx.notify();
    }

    fn key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = ev.keystroke.modifiers;
        let plain = !m.platform && !m.control && !m.alt && !m.function;
        let removable = self.showing.as_ref().is_some_and(|s| s.remove.is_some());
        match ev.keystroke.key.as_str() {
            "escape" => self.close(window, cx),
            "left" if plain => self.step(-1, cx),
            "right" if plain => self.step(1, cx),
            "space" if plain && !m.shift => self.toggle_actual(None, window, cx),
            "=" | "+" if m.platform => self.set_actual(true, window, cx),
            "0" | "-" if m.platform => self.set_actual(false, window, cx),
            "c" if m.platform && !m.shift => self.copy(window, cx),
            "backspace" | "delete" if m.platform && removable => self.remove(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// A press on the image: fit zooms to actual pixels there; at actual pixels it may become a
    /// pan, or a click back to fit (`mouse_up`).
    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.showing.as_ref().is_some_and(|s| s.actual) {
            self.drag = Some(Drag { from: e.position, offset: self.scroll.offset(), panned: false });
            cx.notify();
        } else {
            self.toggle_actual(Some(e.position), window, cx);
        }
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            cx.notify();
            return;
        }
        let d = e.position - drag.from;
        if !drag.panned && d.x.as_f32().hypot(d.y.as_f32()) < CLICK_SLOP {
            return;
        }
        drag.panned = true;
        let offset = drag.offset + d;
        let Some(s) = self.showing.as_ref() else { return };
        if let Some(layout) = Layout::of(&s.items[s.index], s.items.len(), window.viewport_size(), window.scale_factor()) {
            self.scroll.set_offset(clamp_offset(offset, layout.content(), layout.pan.size));
        }
        cx.notify();
    }

    fn mouse_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.take() {
            if !drag.panned {
                self.toggle_actual(None, window, cx);
            }
            cx.notify();
        }
    }
}

/// Native views (the browser) hide while a Trek surface is over them.
fn set_overlay(open: bool, cx: &mut App) {
    crate::workspace::workspace_global(cx).update(cx, |ws, cx| {
        if ws.overlay_open != open {
            ws.overlay_open = open;
            cx.notify();
        }
    });
}

fn format_of(path: &Path) -> ImageFormat {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("jpg" | "jpeg") => ImageFormat::Jpeg,
        Some("gif") => ImageFormat::Gif,
        Some("webp") => ImageFormat::Webp,
        _ => ImageFormat::Png,
    }
}

/// A file's size as Finder gives it (decimal units).
fn file_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        b if b >= 1_000 => format!("{} KB", (b as f64 / 1e3).round() as u64),
        b => format!("{b} bytes"),
    }
}

/// The header's second line: "2 of 5 · 1440 × 900 · 312 KB".
fn meta(s: &Showing) -> String {
    let item = &s.items[s.index];
    let mut parts = vec![];
    if s.items.len() > 1 {
        parts.push(format!("{} of {}", s.index + 1, s.items.len()));
    }
    if let Some((w, h)) = item.pixels {
        parts.push(format!("{w} × {h}"));
    }
    parts.extend(item.bytes.map(file_size));
    parts.join(" · ")
}

/// Keep a pan offset on the image: offsets run from 0 (top left) down to minus the overhang.
fn clamp_offset(offset: Point<Pixels>, content: Size<Pixels>, area: Size<Pixels>) -> Point<Pixels> {
    let max_x = (content.width - area.width).max(px(0.));
    let max_y = (content.height - area.height).max(px(0.));
    point(offset.x.clamp(-max_x, px(0.)), offset.y.clamp(-max_y, px(0.)))
}

/// Where the image goes in a window `viewport` big: the stage it has (between the header and the
/// filmstrip, clear of the chevrons) and its size fit to that and at actual pixels.
struct Layout {
    stage: Bounds<Pixels>,
    /// Where the actual-size image pans: the window's whole width, the chevrons over it.
    pan: Bounds<Pixels>,
    /// Actual pixels in points: one image pixel to one screen pixel.
    actual: Size<Pixels>,
    fit: Size<Pixels>,
}

impl Layout {
    fn of(item: &Shown, count: usize, viewport: Size<Pixels>, scale: f32) -> Option<Self> {
        let (w, h) = item.pixels.filter(|(w, h)| *w > 0 && *h > 0)?;
        let bottom = if count > 1 { FILMSTRIP } else { BOTTOM };
        let stage = Bounds::new(point(px(SIDE), px(HEADER)), size((viewport.width - px(2. * SIDE)).max(px(1.)), (viewport.height - px(HEADER + bottom)).max(px(1.))));
        let pan = Bounds::new(point(px(0.), stage.origin.y), size(viewport.width, stage.size.height));
        let actual = size(px(w as f32 / scale), px(h as f32 / scale));
        // Never past actual pixels: a small image stays small and sharp.
        let k = (stage.size.width / actual.width).min(stage.size.height / actual.height).min(1.);
        Some(Layout { stage, pan, actual, fit: size(actual.width * k, actual.height * k) })
    }

    fn zoomable(&self) -> bool {
        self.fit.width < self.actual.width - px(0.5)
    }

    fn fit_bounds(&self) -> Bounds<Pixels> {
        Bounds::new(self.stage.origin + point((self.stage.size.width - self.fit.width) / 2., (self.stage.size.height - self.fit.height) / 2.), self.fit)
    }

    /// The actual-size image's scrolling area: the image, or the pan area where it's narrower.
    fn content(&self) -> Size<Pixels> {
        size(self.actual.width.max(self.pan.size.width), self.actual.height.max(self.pan.size.height))
    }

    fn actual_bounds_in_content(&self) -> Bounds<Pixels> {
        let c = self.content();
        Bounds::new(point((c.width - self.actual.width) / 2., (c.height - self.actual.height) / 2.), self.actual)
    }
}

fn ease_out(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

/// A press that lands on something over the backdrop stops there: it isn't a click beside the
/// image. (Not `occlude`: the pointer over these still counts as over the preview, so the
/// chevrons stay up.)
fn keep_press<E: InteractiveElement>(el: E) -> E {
    el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

impl ImagePreview {
    /// A round button on the backdrop, `y` from the top: the chevrons (left for `by` < 0). They
    /// show while the pointer is over the preview.
    fn chevron(&self, id: &'static str, by: isize, y: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (icon, tip) = if by < 0 { (IconName::ChevronLeft, "Previous (←)") } else { (IconName::ChevronRight, "Next (→)") };
        let inset = px((SIDE - 36.) / 2.);
        keep_press(div().id(id)).test_support()
            .absolute()
            .top(y)
            .when(by < 0, |el| el.left(inset))
            .when(by > 0, |el| el.right(inset))
            .invisible()
            .group_hover("preview", |s| s.visible())
            .size(px(36.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.popover.opacity(0.92))
            .border_1()
            .border_color(theme.foreground.opacity(0.1))
            .shadow_md()
            .text_color(theme.foreground.opacity(0.8))
            .cursor_pointer()
            .hover(|s| s.text_color(theme.foreground).bg(theme.popover))
            .child(Icon::new(icon).size(px(16.)))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| this.step(by, cx)))
            .into_any_element()
    }

    fn header(&self, s: &Showing, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let item = &s.items[s.index];
        let (open_path, reveal_path) = (item.path.clone(), item.path.clone());
        keep_press(h_flex().id("preview-header"))
            .h(px(HEADER))
            .w_full()
            .flex_none()
            // Clear of the traffic lights.
            .pl(px(86.))
            .pr(px(10.))
            .gap(px(12.))
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(div().id("preview-name").test_support().truncate().text_size(px(13.)).font_medium().text_color(theme.foreground).child(item.name()))
                    .child(div().id("preview-meta").test_support().truncate().text_size(px(11.5)).text_color(theme.muted_foreground).child(meta(s))),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(2.))
                    .child(ui::icon_button("preview-open", Icon::new(crate::assets::Lucide::SquareArrowOutUpRight), "Open in Preview").on_click(move |_, _, cx| cx.open_with_system(&open_path)))
                    .child(ui::icon_button("preview-reveal", IconName::FolderOpen, "Reveal in Finder").on_click(move |_, _, cx| cx.reveal_path(&reveal_path)))
                    .child(ui::icon_button("preview-copy", IconName::Copy, "Copy image (⌘C)").on_click(cx.listener(|this, _, window, cx| this.copy(window, cx))))
                    .when(s.remove.is_some(), |el| {
                        el.child(ui::icon_button("preview-remove", Icon::new(crate::assets::Lucide::Trash), "Remove from message (⌘⌫)").on_click(cx.listener(|this, _, window, cx| this.remove(window, cx))))
                    })
                    .child(div().w(px(1.)).h(px(16.)).mx(px(6.)).bg(theme.foreground.opacity(0.12)))
                    .child(ui::icon_button("preview-close", IconName::Close, "Close (Esc)").on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))),
            )
            .into_any_element()
    }

    fn filmstrip(&self, s: &Showing, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        h_flex()
            .id("preview-filmstrip")
            .test_support()
            .absolute()
            .bottom(px((FILMSTRIP - THUMB) / 2. - 4.))
            .left_0()
            .w_full()
            .justify_center()
            .child(keep_press(h_flex().id("preview-thumbs")).gap(px(6.)).p(px(4.)).children(s.items.iter().enumerate().map(|(i, item)| {
                let on = i == s.index;
                div()
                    .id(("preview-thumb", i))
                    .test_support()
                    .size(px(THUMB))
                    .flex_none()
                    .rounded(px(7.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(theme.foreground.opacity(if on { 0.85 } else { 0.1 }))
                    .when(!on, |el| el.opacity(0.55).hover(|s| s.opacity(0.9)).cursor_pointer())
                    .child(img(item.path.clone()).size_full().object_fit(ObjectFit::Cover))
                    .on_click(cx.listener(move |this, _, _, cx| this.go_to(i, cx)))
            })))
            .into_any_element()
    }

    /// The image itself: fit to the stage (`t` of the way in), or at actual pixels in a pan area.
    fn image(&self, s: &Showing, t: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let item = &s.items[s.index];
        let edge = theme.foreground.opacity(if theme.mode.is_dark() { 0.08 } else { 0.06 });
        let under = theme.foreground.opacity(0.03);
        fn frame<E: Styled>(el: E, edge: Hsla, under: Hsla) -> E {
            el.rounded(px(6.)).overflow_hidden().border_1().border_color(edge).shadow_lg().bg(under)
        }
        let Some(layout) = Layout::of(item, s.items.len(), window.viewport_size(), window.scale_factor()) else {
            // No size in the file's header: the image fits itself.
            return div()
                .absolute()
                .left(px(SIDE))
                .top(px(HEADER))
                .right(px(SIDE))
                .bottom(px(if s.items.len() > 1 { FILMSTRIP } else { BOTTOM }))
                .child(keep_press(div().id("preview-image").test_support()).size_full().child(img(item.path.clone()).size_full().object_fit(ObjectFit::Contain)))
                .into_any_element();
        };
        let press = cx.listener(|this, e: &MouseDownEvent, window, cx| this.mouse_down(e, window, cx));
        if s.actual {
            let (area, content, at) = (layout.pan, layout.content(), layout.actual_bounds_in_content());
            let grabbing = self.drag.as_ref().is_some_and(|d| d.panned);
            return div()
                .id("preview-pan")
                .absolute()
                .left(area.origin.x)
                .top(area.origin.y)
                .w(area.size.width)
                .h(area.size.height)
                .overflow_scroll()
                .track_scroll(&self.scroll)
                .child(
                    div().relative().w(content.width).h(content.height).child(
                        frame(div().id("preview-image").test_support(), edge, under)
                            .absolute()
                            .left(at.origin.x)
                            .top(at.origin.y)
                            .w(at.size.width)
                            .h(at.size.height)
                            .rounded(px(2.))
                            .cursor(if grabbing { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
                            .child(img(item.path.clone()).size_full())
                            .on_mouse_down(MouseButton::Left, press),
                    ),
                )
                .into_any_element();
        }
        // Coming in, it grows the last few percent into place.
        let k = 0.965 + 0.035 * t;
        let shown = size(layout.fit.width * k, layout.fit.height * k);
        let stage = layout.stage;
        let origin = stage.origin + point((stage.size.width - shown.width) / 2., (stage.size.height - shown.height) / 2.);
        frame(div().id("preview-image").test_support(), edge, under)
            .absolute()
            .left(origin.x)
            .top(origin.y)
            .w(shown.width)
            .h(shown.height)
            .when(layout.zoomable(), |el| el.cursor_pointer())
            .child(img(item.path.clone()).size_full())
            .on_mouse_down(MouseButton::Left, press)
            .into_any_element()
    }
}

impl Render for ImagePreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // How far it has come in (1: all the way), and whether it's going.
        let (t, closing) = match self.motion {
            Some((at, closing)) => {
                let p = at.elapsed().as_secs_f32() / if closing { CLOSING } else { OPENING }.as_secs_f32();
                if p >= 1. {
                    if closing {
                        self.finish_close(cx);
                    }
                    self.motion = None;
                    (1., closing)
                } else {
                    window.request_animation_frame();
                    (if closing { 1. - ease_out(p) } else { ease_out(p) }, closing)
                }
            }
            None => (1., false),
        };
        let Some(s) = self.showing.as_ref() else { return div().into_any_element() };
        let theme = cx.theme().clone();
        // The window's own surface, coming in over what was there (`t`) until the image is the only
        // thing with colour. Opaque once in: any of the transcript showing through (its images
        // above all) reads as clutter around the picture.
        let backdrop = if theme.mode.is_dark() { theme.background } else { theme.sidebar };
        let several = s.items.len() > 1;
        let image = self.image(s, t, window, cx);
        let header = self.header(s, cx);
        let film = several.then(|| self.filmstrip(s, cx));
        let chevrons = several.then(|| {
            let y = px(HEADER) + (window.viewport_size().height - px(HEADER + FILMSTRIP)) / 2. - px(18.);
            [self.chevron("preview-prev", -1, y, cx), self.chevron("preview-next", 1, y, cx)]
        });
        div()
            .id("attachment-preview")
            .test_support()
            .group("preview")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .opacity(t)
            .when(!closing, |el| {
                el.track_focus(&self.focus)
                    .key_context("AttachmentPreview")
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.key(ev, window, cx)))
                    // ⌘W puts the preview away rather than the tab or window under it.
                    .on_action(cx.listener(|this, _: &crate::CloseTab, window, cx| this.close(window, cx)))
                    .on_action(cx.listener(|this, _: &crate::CloseWindow, window, cx| this.close(window, cx)))
                    .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| this.mouse_move(e, window, cx)))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| this.mouse_up(window, cx)))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, window, cx| this.mouse_up(window, cx)))
            })
            // Clicking beside the image puts it away (what's over the backdrop keeps its presses).
            .child(div().id("preview-backdrop").absolute().top_0().left_0().size_full().bg(backdrop).on_click(cx.listener(|this, _, window, cx| this.close(window, cx))))
            .child(image)
            .child(header)
            .children(chevrons.into_iter().flatten())
            .children(film)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Layout, Shown, clamp_offset, file_size};
    use gpui_kit::{point, px, size};
    use std::path::PathBuf;

    fn shown(pixels: Option<(u32, u32)>) -> Shown {
        Shown { path: PathBuf::from("/x.png"), pixels, bytes: None }
    }

    #[test]
    fn sizes_read_as_finder_gives_them() {
        assert_eq!(file_size(812), "812 bytes");
        assert_eq!(file_size(312_400), "312 KB");
        assert_eq!(file_size(4_260_000), "4.3 MB");
    }

    #[test]
    fn big_images_fit_and_small_ones_stay_actual_size() {
        let window = size(px(1280.), px(820.));
        // A Retina screenshot: 2880 pixels are 1440 points, more than the stage has.
        let big = Layout::of(&shown(Some((2880, 1800))), 1, window, 2.).unwrap();
        assert!(big.zoomable());
        assert!(big.fit.width <= big.stage.size.width && big.fit.height <= big.stage.size.height);
        assert!((big.fit.width / big.fit.height - 1.6).abs() < 0.01, "keeps its shape");
        // An icon isn't blown up.
        let small = Layout::of(&shown(Some((64, 64))), 3, window, 2.).unwrap();
        assert_eq!(small.fit, size(px(32.), px(32.)));
        assert!(!small.zoomable());
        assert!(Layout::of(&shown(None), 1, window, 2.).is_none());
    }

    #[test]
    fn panning_stays_on_the_image() {
        let content = size(px(2000.), px(1000.));
        let area = size(px(1000.), px(800.));
        assert_eq!(clamp_offset(point(px(50.), px(-500.)), content, area), point(px(0.), px(-200.)));
        assert_eq!(clamp_offset(point(px(-1500.), px(0.)), content, area), point(px(-1000.), px(0.)));
    }
}
