//! Files the editor doesn't edit as text, shown for what they are in a tab of their own: an image
//! drawn (fit to the tab; a click shows it at actual pixels, drag or scroll to pan, a click goes
//! back), a PDF, a video or a song in the system's web view (WebView2 on Windows, WKWebView on a
//! Mac: the controls are the engine's own), and for anything else a note saying what it is, with
//! the file's own app a click away. A file Trek can't read says why, in words a person can act on,
//! with a Retry; one OneDrive keeps online is fetched first ("Fetching from OneDrive…").
//!
//! The web view gets the one file it was made for and nothing else: it's served under a scheme of
//! its own (`trek-file`) whose handler answers every request with that file's bytes, navigation
//! anywhere else is refused, and the page has no IPC, no script of Trek's, no devtools and no
//! downloads.

use crate::assets::Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How much of a file's start decides whether it's text.
pub const SNIFF: usize = 8 * 1024;

/// How long "Fetching from OneDrive…" waits before it says it's taking a while (and offers Retry;
/// the fetch goes on, and the file still opens once it lands).
const FETCH_LIMIT: Duration = Duration::from_secs(45);

/// What a file is, as far as the editor group shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Image,
    /// A PDF.
    Document,
    /// Video or audio.
    Media,
    /// Anything else that isn't text.
    Binary,
}

/// The kind a file's name alone says it is (images, PDFs, video and audio); `None` for the rest,
/// which its content decides.
pub fn kind_by_name(path: &Path) -> Option<Kind> {
    Some(match extension(path)?.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" => Kind::Image,
        "pdf" => Kind::Document,
        "mp4" | "webm" | "mov" | "m4v" | "mp3" | "m4a" | "wav" | "ogg" | "flac" => Kind::Media,
        _ => return None,
    })
}

/// What `path` is, from its name and `first`, the first `SNIFF` bytes of it: a NUL, or bytes that
/// can't be UTF-8, make it binary whatever it's called.
pub fn kind(path: &Path, first: &[u8]) -> Kind {
    kind_by_name(path).unwrap_or(if is_binary(first) { Kind::Binary } else { Kind::Text })
}

/// `first` (a file's start) holds a NUL or isn't UTF-8. A character cut off at the end is the
/// sample's edge, not the file's fault.
pub fn is_binary(first: &[u8]) -> bool {
    first.contains(&0) || std::str::from_utf8(first).is_err_and(|e| e.error_len().is_some())
}

fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_ascii_lowercase())
}

fn is_audio(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("mp3" | "m4a" | "wav" | "ogg" | "flac"))
}

/// The type the web view is told a file has.
fn mime(path: &Path) -> &'static str {
    match extension(path).as_deref() {
        Some("pdf") => "application/pdf",
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("m4a") => "audio/mp4",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("flac") => "audio/flac",
        _ => "application/octet-stream",
    }
}

/// What the status bar calls a file of `kind` (where a text file's language goes).
pub fn kind_label(path: &Path, kind: Kind) -> String {
    let ext = extension(path).map(|e| e.to_uppercase());
    match (kind, ext) {
        (Kind::Text, _) => super::status_bar::language(path).to_string(),
        (Kind::Image, Some(e)) => format!("{e} Image"),
        (Kind::Document, _) => "PDF".to_string(),
        (Kind::Media, Some(e)) if is_audio(path) => format!("{e} Audio"),
        (Kind::Media, Some(e)) => format!("{e} Video"),
        (Kind::Binary, Some(e)) => format!("{e} File"),
        _ => "Binary".to_string(),
    }
}

/// What the editor found instead of text, for the workbench to show a preview in its place.
pub enum NotText {
    Binary,
    Unreadable(std::io::Error),
}

/// Windows' `ERROR_CLOUD_FILE_PROVIDER_NOT_RUNNING` and `…_PROVIDER_TERMINATED`: the app that keeps
/// the file online (OneDrive) isn't there to fetch it.
const CLOUD_PROVIDER_NOT_RUNNING: i32 = 362;
const CLOUD_PROVIDER_TERMINATED: i32 = 404;

/// The cloud app that keeps `path` (OneDrive, OneDrive - Contoso…), by the folder it's under.
/// Split on either separator: a Windows path's `\` is no separator to a Mac's `Path`, and the
/// same words serve a path that came from a Windows machine's text.
pub fn cloud_app(path: &Path) -> Option<&'static str> {
    path.to_string_lossy().split(['\\', '/']).any(|s| s.len() >= 8 && s.is_char_boundary(8) && s[..8].eq_ignore_ascii_case("onedrive")).then_some("OneDrive")
}

/// Why `name` can't be shown, in words a person can act on. `windows`: the error is a Windows
/// one (its raw codes mean something else elsewhere).
pub fn unreadable_words(name: &str, err: &std::io::Error, app: Option<&str>, windows: bool) -> String {
    if windows && matches!(err.raw_os_error(), Some(CLOUD_PROVIDER_NOT_RUNNING | CLOUD_PROVIDER_TERMINATED)) {
        return match app {
            Some(app) => format!("This file is kept online by {app} and {app} isn't running. Start {app}, or right-click the file in Explorer and choose 'Always keep on this device'."),
            None => "This file is kept online by a cloud storage app that isn't running. Start it, or right-click the file in Explorer and choose 'Always keep on this device'.".to_string(),
        };
    }
    format!("Can't read {name}: {err}")
}

/// The file's data isn't on this device (OneDrive's Files On-Demand): reading it fetches it. Asked
/// of its attributes, which doesn't open it.
pub fn online_only(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call.
        let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
        attributes != INVALID_FILE_ATTRIBUTES && crate::shortcuts::is_online_only(attributes)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// The name of the app the system opens `path`'s kind of file with ("Microsoft Edge", "Photos"),
/// when it says.
fn default_app(path: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::{ASSOCF_INIT_IGNOREUNKNOWN, ASSOCF_NOTRUNCATE, ASSOCSTR_FRIENDLYAPPNAME, AssocQueryStringW};
        use windows::core::{HSTRING, PCWSTR, PWSTR};
        let ext = HSTRING::from(format!(".{}", path.extension()?.to_str()?));
        let flags = ASSOCF_INIT_IGNOREUNKNOWN | ASSOCF_NOTRUNCATE;
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        // SAFETY: `buf` holds `len` UTF-16 units and outlives the call; `ext` is NUL-terminated.
        let found = unsafe { AssocQueryStringW(flags, ASSOCSTR_FRIENDLYAPPNAME, &ext, PCWSTR::null(), Some(PWSTR(buf.as_mut_ptr())), &mut len) };
        if found.is_err() {
            return None;
        }
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end])).filter(|n| !n.trim().is_empty())
    }
    #[cfg(not(windows))]
    {
        // Finder's choice isn't asked for: "Open in Default App" says it on a Mac.
        let _ = path;
        None
    }
}

/// What reading the file found: its size, and an image's pixels.
struct Probe {
    bytes: u64,
    pixels: Option<(u32, u32)>,
}

/// Read what the view says about `path`, off the main thread. Reading its start is what fetches
/// a file kept online, and what finds that it can't be read; a text file kept online is fetched
/// whole (up to what the editor would take), so it opens in the editor at once afterwards.
fn probe(path: &Path, kind: Kind) -> std::io::Result<Probe> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a file"));
    }
    match kind {
        Kind::Text => {
            std::io::copy(&mut (&mut file).take(2_000_001), &mut std::io::sink())?;
        }
        Kind::Binary => {}
        _ => {
            let _ = file.read(&mut [0u8; 1])?;
        }
    }
    // Not the remembered size: the file may have been rewritten since it was last open here.
    let pixels = if kind == Kind::Image { crate::image_preview::pixels_now(path) } else { None };
    Ok(Probe { bytes: meta.len(), pixels })
}

enum Status {
    /// Reading the file's size (and an image's pixels), off the main thread.
    Loading,
    /// The file is kept online and being fetched; `slow` once that's taken `FETCH_LIMIT`.
    Fetching { slow: bool },
    Ready,
    /// Why it can't be shown.
    Unreadable(String),
}

/// A preview asks the workbench for the file again: Retry, or a text file that was kept online
/// has arrived (`fetched`) and opens in the editor now.
pub enum PreviewEvent {
    Reopen { fetched: bool },
}

/// The page a PDF, a video or a song plays in: a native view laid over the window, or (in tests)
/// a stand-in.
pub trait WebSurface {
    fn element(&self) -> AnyElement;
    /// On screen or not: a native view floats over GPUI, so it's hidden whenever its tab isn't
    /// what the window shows (another tab, Agents mode, a menu over it).
    fn set_shown(&self, shown: bool, cx: &mut App);
    /// The native view, for `shots` to capture (WebView2's CapturePreview).
    #[cfg(all(feature = "shots", windows))]
    fn native(&self) -> Option<Entity<gpui_wry::WebView>> {
        None
    }
}

enum Web {
    /// Not asked for (an image, or the file not read yet).
    None,
    Made(Box<dyn WebSurface>),
    /// No web view could be made (no WebView2 runtime): the note says so.
    Unavailable,
    /// A PDF too big to hand over whole (`WHOLE_LIMIT`): its own app shows it.
    TooBig,
}

pub struct PreviewView {
    pub(crate) path: PathBuf,
    pub(crate) kind: Kind,
    status: Status,
    bytes: Option<u64>,
    pixels: Option<(u32, u32)>,
    /// The image at actual pixels, panned.
    actual: bool,
    scroll: ScrollHandle,
    /// A press on the actual-size image: where, the scroll offset then, and whether it's a pan yet.
    drag: Option<(Point<Pixels>, Point<Pixels>, bool)>,
    /// Where the image is drawn, as last laid out.
    area: Bounds<Pixels>,
    /// The image's own cache: GPUI's global one keeps a path's picture for the whole run, so a
    /// file rewritten since (an agent's chart) would open as it was.
    images: Entity<RetainAllImageCache>,
    web: Web,
    /// Whether the workbench has the tab on screen (the web view shows only then).
    shown: bool,
    /// The default app's name, for "Open in …".
    app: Option<String>,
    focus: FocusHandle,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<PreviewEvent> for PreviewView {}

impl Focusable for PreviewView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PreviewView {
    /// A preview of `path`, a `kind` the editor doesn't edit; with `error`, the file couldn't be
    /// read and the view says why.
    pub fn new(path: PathBuf, kind: Kind, error: Option<std::io::Error>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            kind,
            status: Status::Loading,
            bytes: None,
            pixels: None,
            actual: false,
            scroll: ScrollHandle::new(),
            drag: None,
            area: Bounds::default(),
            images: RetainAllImageCache::new(cx),
            web: Web::None,
            shown: false,
            app: None,
            focus: cx.focus_handle(),
            _tasks: vec![],
        };
        match error {
            Some(e) => this.status = Status::Unreadable(unreadable_words(&this.name(), &e, cloud_app(&this.path), cfg!(windows))),
            None => this.load(window, cx),
        }
        this
    }

    fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| self.path.display().to_string())
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (path, kind) = (self.path.clone(), self.kind);
        if online_only(&path) {
            self.status = Status::Fetching { slow: false };
            self._tasks.push(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FETCH_LIMIT).await;
                let _ = this.update(cx, |this, cx| {
                    if let Status::Fetching { slow } = &mut this.status {
                        *slow = true;
                        cx.notify();
                    }
                });
            }));
        }
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let found = cx.background_executor().spawn(async move { (probe(&path, kind), default_app(&path)) }).await;
            let _ = this.update_in(cx, |this, window, cx| this.probed(found, window, cx));
        }));
    }

    fn probed(&mut self, (probe, app): (std::io::Result<Probe>, Option<String>), window: &mut Window, cx: &mut Context<Self>) {
        self.app = app;
        match probe {
            Ok(p) => {
                self.bytes = Some(p.bytes);
                self.pixels = p.pixels;
                self.status = Status::Ready;
                match self.kind {
                    Kind::Text => cx.emit(PreviewEvent::Reopen { fetched: true }),
                    Kind::Document if p.bytes > WHOLE_LIMIT => self.web = Web::TooBig,
                    Kind::Document | Kind::Media => {
                        self.web = match make_web(&self.path, window, cx) {
                            Some(web) => {
                                web.set_shown(self.shown, cx);
                                Web::Made(web)
                            }
                            None => Web::Unavailable,
                        };
                    }
                    Kind::Image | Kind::Binary => {}
                }
            }
            Err(e) => self.status = Status::Unreadable(unreadable_words(&self.name(), &e, cloud_app(&self.path), cfg!(windows))),
        }
        cx.notify();
    }

    /// The workbench has this tab on screen, or doesn't.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown == shown {
            return;
        }
        self.shown = shown;
        if let Web::Made(web) = &self.web {
            web.set_shown(shown, cx);
        }
    }

    /// Whether its web view is on screen (`None`: it has none).
    #[cfg(test)]
    pub fn web_shown(&self) -> Option<bool> {
        matches!(self.web, Web::Made(_)).then_some(self.shown)
    }

    /// Its native web view, for `shots` to capture.
    #[cfg(all(feature = "shots", windows))]
    pub fn web_view(&self) -> Option<Entity<gpui_wry::WebView>> {
        match &self.web {
            Web::Made(web) => web.native(),
            _ => None,
        }
    }

    /// What its note says, when it shows one rather than the file.
    #[cfg(test)]
    pub fn note_text(&self) -> Option<String> {
        match (&self.status, self.kind, &self.web) {
            (Status::Unreadable(why), ..) => Some(why.clone()),
            (Status::Ready, Kind::Binary, _) | (Status::Ready, Kind::Document | Kind::Media, Web::Unavailable | Web::TooBig) => Some(self.cant_show()),
            (Status::Ready, Kind::Image, _) if self.undecodable() => Some(self.cant_show()),
            _ => None,
        }
    }

    /// What the status bar says: the kind, the image's pixels, the file's size.
    pub fn status_items(&self) -> (String, Option<String>, Option<String>) {
        let pixels = self.pixels.map(|(w, h)| format!("{w} × {h}"));
        (kind_label(&self.path, self.kind), pixels, self.bytes.map(crate::image_preview::file_size))
    }

    /// An image whose header gives no size isn't one Trek can draw (an SVG has none to give): a
    /// damaged file, or a format it doesn't decode. It's said, not left a blank tab.
    fn undecodable(&self) -> bool {
        self.kind == Kind::Image && self.pixels.is_none() && extension(&self.path).as_deref() != Some("svg")
    }

    fn open_label(&self) -> String {
        match &self.app {
            Some(app) => format!("Open in {app}"),
            None => "Open in Default App".to_string(),
        }
    }

    fn open_button(&self) -> Button {
        let path = self.path.clone();
        Button::new("ide-preview-open").small().outline().icon(Icon::new(Lucide::SquareArrowOutUpRight)).label(self.open_label()).on_click(move |_, _, cx| cx.open_with_system(&path))
    }

    fn reveal_button(&self) -> Button {
        let path = self.path.clone();
        Button::new("ide-preview-reveal").small().ghost().icon(IconName::FolderOpen).label(crate::words::words().reveal_in_file_manager).on_click(move |_, _, cx| cx.reveal_path(&path))
    }

    /// The line over a PDF or a video: the file's name and size, and its own app.
    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .id("ide-preview-header")
            .test_support()
            .h(px(34.))
            .flex_none()
            .px(px(12.))
            .gap(px(8.))
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(12.))
            .child(div().id("ide-preview-name").test_support().min_w_0().truncate().font_medium().child(self.name()))
            .when_some(self.bytes.map(crate::image_preview::file_size), |el, size| el.child(div().flex_none().text_color(theme.muted_foreground).child(size)))
            .child(div().flex_1())
            .child(self.open_button())
            .child(self.reveal_button())
    }

    /// A note in the middle of the tab, for what isn't drawn: what it is, and what can be done.
    fn note(&self, icon: Icon, text: String, hint: Option<String>, buttons: Vec<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .id("ide-preview-note")
            .test_support()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .px(px(24.))
            .child(icon.size(px(30.)).text_color(theme.muted_foreground.opacity(0.7)))
            .child(div().id("ide-preview-words").test_support().max_w(px(480.)).text_center().text_size(px(13.)).child(text))
            .when_some(hint, |el, hint| el.child(div().max_w(px(480.)).text_center().text_size(px(12.)).text_color(theme.muted_foreground).child(hint)))
            .child(h_flex().gap(px(8.)).flex_wrap().justify_center().children(buttons))
            .into_any_element()
    }

    /// "Trek can't show blob.bin (BIN File, 12 KB)."
    fn cant_show(&self) -> String {
        let what = match (self.kind, extension(&self.path)) {
            (Kind::Binary, Some(e)) => format!("{} file", e.to_uppercase()),
            (Kind::Binary, None) => "binary file".to_string(),
            (kind, _) => kind_label(&self.path, kind),
        };
        match self.bytes.map(crate::image_preview::file_size) {
            Some(size) => format!("Trek can't show {} ({what}, {size}).", self.name()),
            None => format!("Trek can't show {} ({what}).", self.name()),
        }
    }

    fn retry_button(cx: &mut Context<Self>) -> AnyElement {
        Button::new("ide-preview-retry")
            .small()
            .outline()
            .icon(Icon::new(Lucide::RefreshCw))
            .label("Retry")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PreviewEvent::Reopen { fetched: false })))
            .into_any_element()
    }

    /// Fit ⇄ actual pixels, keeping the spot pressed (or the middle) where it was.
    fn toggle_actual(&mut self, at: Option<Point<Pixels>>, scale: f32, cx: &mut Context<Self>) {
        let Some(layout) = ImageLayout::of(self.pixels, self.area, scale) else { return };
        if !self.actual && !layout.zoomable() {
            return;
        }
        self.actual = !self.actual;
        if self.actual {
            let fit = layout.fit;
            let at = at.filter(|p| fit.contains(p)).unwrap_or(fit.center());
            let (fx, fy) = ((at.x - fit.origin.x) / fit.size.width, (at.y - fit.origin.y) / fit.size.height);
            let image = layout.actual_in_content();
            let target = point(image.origin.x + image.size.width * fx - (at.x - self.area.origin.x), image.origin.y + image.size.height * fy - (at.y - self.area.origin.y));
            self.scroll.set_offset(crate::image_preview::clamp_offset(point(-target.x, -target.y), layout.content(), self.area.size));
        }
        cx.notify();
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.actual {
            self.drag = Some((e.position, self.scroll.offset(), false));
        } else {
            self.toggle_actual(Some(e.position), window.scale_factor(), cx);
        }
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some((from, offset, panned)) = self.drag.as_mut() else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let d = e.position - *from;
        if !*panned && d.x.as_f32().hypot(d.y.as_f32()) < 4. {
            return;
        }
        *panned = true;
        let to = *offset + d;
        if let Some(layout) = ImageLayout::of(self.pixels, self.area, window.scale_factor()) {
            self.scroll.set_offset(crate::image_preview::clamp_offset(to, layout.content(), self.area.size));
        }
        cx.notify();
    }

    fn mouse_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, _, panned)) = self.drag.take() {
            if !panned {
                self.toggle_actual(None, window.scale_factor(), cx);
            }
        }
    }

    /// The image: fit to the tab (never past actual pixels), or at actual pixels in a pan area.
    fn image(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let me = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let _ = me.update(cx, |p, _| p.area = bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let layout = ImageLayout::of(self.pixels, self.area, window.scale_factor());
        let path = self.path.clone();
        let press = cx.listener(|this, e: &MouseDownEvent, window, cx| this.mouse_down(e, window, cx));
        let stage = div()
            .id("ide-preview-stage")
            .relative()
            .size_full()
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| this.mouse_move(e, window, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| this.mouse_up(window, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, window, cx| this.mouse_up(window, cx)))
            .child(measure);
        match layout {
            Some(layout) if self.actual => {
                let (content, at) = (layout.content(), layout.actual_in_content());
                let grabbing = self.drag.as_ref().is_some_and(|d| d.2);
                stage
                    .child(
                        div().id("ide-preview-pan").absolute().top_0().left_0().size_full().overflow_scroll().track_scroll(&self.scroll).child(
                            div().relative().w(content.width).h(content.height).child(
                                div()
                                    .id("ide-preview-image")
                                    .test_support()
                                    .absolute()
                                    .left(at.origin.x)
                                    .top(at.origin.y)
                                    .w(at.size.width)
                                    .h(at.size.height)
                                    .cursor(if grabbing { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
                                    .child(img(path).image_cache(&self.images).size_full())
                                    .on_mouse_down(MouseButton::Left, press),
                            ),
                        ),
                    )
                    .into_any_element()
            }
            layout => {
                let zoomable = layout.as_ref().is_some_and(ImageLayout::zoomable);
                // Before the first layout (or with no size in the file's header) the image fits
                // itself; after it, it's placed where a click measures it to be.
                let image = div().id("ide-preview-image").test_support().when(zoomable, |el| el.cursor(CursorStyle::PointingHand)).on_mouse_down(MouseButton::Left, press);
                let image = match layout {
                    Some(l) => image.absolute().left(l.fit.origin.x - self.area.origin.x).top(l.fit.origin.y - self.area.origin.y).w(l.fit.size.width).h(l.fit.size.height).child(img(path).image_cache(&self.images).size_full()),
                    None => image.absolute().top(px(PAD)).left(px(PAD)).right(px(PAD)).bottom(px(PAD)).child(img(path).image_cache(&self.images).size_full().object_fit(ObjectFit::ScaleDown)),
                };
                stage.child(image).into_any_element()
            }
        }
    }
}

/// Room round a fit image.
const PAD: f32 = 24.;

/// Where an image goes in an `area`: fit inside it (never past actual pixels), and at actual
/// pixels in a pan area as big as it.
struct ImageLayout {
    area: Bounds<Pixels>,
    actual: Size<Pixels>,
    /// In window coordinates.
    fit: Bounds<Pixels>,
}

impl ImageLayout {
    fn of(pixels: Option<(u32, u32)>, area: Bounds<Pixels>, scale: f32) -> Option<Self> {
        let (w, h) = pixels.filter(|(w, h)| *w > 0 && *h > 0)?;
        if area.size.width <= px(2. * PAD) || area.size.height <= px(2. * PAD) {
            return None;
        }
        let actual = size(px(w as f32 / scale), px(h as f32 / scale));
        let room = size(area.size.width - px(2. * PAD), area.size.height - px(2. * PAD));
        let k = (room.width / actual.width).min(room.height / actual.height).min(1.);
        let shown = size(actual.width * k, actual.height * k);
        let fit = Bounds::new(area.origin + point((area.size.width - shown.width) / 2., (area.size.height - shown.height) / 2.), shown);
        Some(Self { area, actual, fit })
    }

    fn zoomable(&self) -> bool {
        self.fit.size.width < self.actual.width - px(0.5)
    }

    fn content(&self) -> Size<Pixels> {
        size(self.actual.width.max(self.area.size.width), self.actual.height.max(self.area.size.height))
    }

    fn actual_in_content(&self) -> Bounds<Pixels> {
        let c = self.content();
        Bounds::new(point((c.width - self.actual.width) / 2., (c.height - self.actual.height) / 2.), self.actual)
    }
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.status {
            Status::Loading => div().into_any_element(),
            Status::Fetching { slow } => {
                let app = cloud_app(&self.path).unwrap_or("the cloud");
                let (text, retry) = if *slow {
                    (format!("{app} is still fetching {}. It opens here once it's on this device.", self.name()), Some(Self::retry_button(cx)))
                } else {
                    (format!("Fetching {} from {app}…", self.name()), None)
                };
                self.note(Icon::new(Lucide::Cloud), text, None, retry.into_iter().collect(), cx)
            }
            Status::Unreadable(why) => {
                let buttons = vec![Self::retry_button(cx), self.reveal_button().into_any_element()];
                self.note(Icon::new(IconName::TriangleAlert), why.clone(), None, buttons, cx)
            }
            Status::Ready => match (self.kind, &self.web) {
                (Kind::Image, _) if !self.undecodable() => self.image(window, cx),
                (Kind::Document | Kind::Media, Web::Made(web)) => v_flex()
                    .size_full()
                    .child(self.header(cx))
                    .child(div().id("ide-preview-web").test_support().flex_1().min_h_0().child(web.element()))
                    .into_any_element(),
                (Kind::Document | Kind::Media, Web::Unavailable | Web::TooBig) => {
                    let too_big = matches!(self.web, Web::TooBig);
                    let hint = if too_big {
                        "A PDF this big is left to its own app."
                    } else if cfg!(windows) {
                        "PDFs, video and audio show in Microsoft Edge WebView2, which couldn't be started on this PC."
                    } else {
                        "PDFs, video and audio show in the system's web view, which isn't available."
                    };
                    let mut buttons = vec![self.open_button().into_any_element(), self.reveal_button().into_any_element()];
                    if cfg!(windows) && !too_big {
                        buttons.push(Button::new("ide-preview-get-webview2").small().ghost().label("Get WebView2").on_click(|_, _, cx| cx.open_url(crate::panels::browser::WEBVIEW2_DOWNLOAD)).into_any_element());
                    }
                    self.note(Icon::new(IconName::File), self.cant_show(), Some(hint.to_string()), buttons, cx)
                }
                _ => {
                    let buttons = vec![self.open_button().into_any_element(), self.reveal_button().into_any_element()];
                    self.note(Icon::new(IconName::File), self.cant_show(), None, buttons, cx)
                }
            },
        };
        div().id("ide-preview").test_support().track_focus(&self.focus).size_full().child(body)
    }
}

/// Make the web view a PDF or a video plays in, or `None` when there's none to be had.
fn make_web(path: &Path, window: &mut Window, cx: &mut App) -> Option<Box<dyn WebSurface>> {
    #[cfg(test)]
    {
        let _ = (path, window, cx);
        FAKE_WEB.with(std::cell::Cell::get).then(|| Box::new(FakeWeb) as Box<dyn WebSurface>)
    }
    #[cfg(not(test))]
    native_web(path, window, cx)
}

#[cfg(test)]
thread_local! {
    /// Whether tests get a stand-in web view (none, as with no WebView2, when not).
    static FAKE_WEB: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Tests on this thread get a stand-in web view (`true`) or none at all.
#[cfg(test)]
pub fn fake_web(on: bool) {
    FAKE_WEB.with(|f| f.set(on));
}

#[cfg(test)]
struct FakeWeb;

#[cfg(test)]
impl WebSurface for FakeWeb {
    fn element(&self) -> AnyElement {
        div().id("ide-preview-web-fake").test_support().size_full().into_any_element()
    }
    fn set_shown(&self, _: bool, _: &mut App) {}
}

/// The scheme the file is served under: on Windows wry serves `<scheme>://<host>/` as
/// `http://<scheme>.<host>/`, and answers every request there itself.
const FILE_SCHEME: &str = "trek-file";

/// The file's address under the scheme: its name (so the PDF viewer can name it), encoded.
fn file_url(path: &Path) -> String {
    format!("{}{}", file_origin(cfg!(windows)), file_path(path))
}

/// `/<name>`, percent-encoded.
fn file_path(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    std::iter::once("/".to_string()).chain(name.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })).collect()
}

/// The page a video or a song plays in: the engine's own player, still until it's played (a
/// media file loaded on its own would start at once, sound and all), on black.
fn media_page(path: &Path) -> String {
    let tag = if is_audio(path) { "audio" } else { "video" };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>html,body{{margin:0;height:100%;background:#000}}body{{display:flex;align-items:center;justify-content:center}}video{{max-width:100%;max-height:100%}}audio{{width:min(520px,90%)}}</style></head><body><{tag} src=\"{}\" controls controlslist=\"nodownload\" disablepictureinpicture preload=\"metadata\"></{tag}></body></html>",
        file_path(path)
    )
}

/// Nothing on the media page runs, or loads from anywhere but the file.
const MEDIA_CSP: &str = "default-src 'none'; media-src 'self'; style-src 'unsafe-inline'";

/// What the view is answered for `asked` (a request's path): the media page at `/` for a video or
/// a song, else the file, whatever the path names.
fn answer(path: &Path, asked: &str, range: Option<&str>) -> wry::http::Response<Vec<u8>> {
    if asked == "/" && kind_by_name(path) == Some(Kind::Media) {
        return wry::http::Response::builder()
            .header("Content-Type", "text/html; charset=utf-8")
            .header("Content-Security-Policy", MEDIA_CSP)
            .header("X-Content-Type-Options", "nosniff")
            .header("Cache-Control", "no-store")
            .body(media_page(path).into_bytes())
            .unwrap_or_default();
    }
    serve(path, range)
}

fn file_origin(windows: bool) -> String {
    if windows { format!("http://{FILE_SCHEME}.preview") } else { format!("{FILE_SCHEME}://preview") }
}

/// The largest piece of a file one answer carries: players ask for the rest as they need it.
const CHUNK: u64 = 4 << 20;

/// The biggest file answered whole (a request with no `Range`, as a PDF's first is): it's held in
/// memory twice over while the web view takes it, and a file past this would bring Trek down
/// asking for the room. A bigger PDF is left to its own app; players ask for video in pieces.
const WHOLE_LIMIT: u64 = 256 << 20;

/// The first and last byte `range` (a `Range` header) asks for of a file `len` long: `None` for
/// the whole file, `Err` when it asks for none of it.
fn parse_range(range: Option<&str>, len: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = range.and_then(|r| r.trim().strip_prefix("bytes=")) else { return Ok(None) };
    // Only the first of several ranges: players ask for one at a time.
    let spec = spec.split(',').next().unwrap_or_default().trim();
    let (a, b) = spec.split_once('-').ok_or(())?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", "") => return Err(()),
        ("", suffix) => {
            let n: u64 = suffix.parse().map_err(|_| ())?;
            (len.saturating_sub(n), len.saturating_sub(1))
        }
        (s, "") => (s.parse().map_err(|_| ())?, len.saturating_sub(1)),
        (s, e) => (s.parse().map_err(|_| ())?, e.parse::<u64>().map_err(|_| ())?.min(len.saturating_sub(1))),
    };
    if len == 0 || start >= len || start > end {
        return Err(());
    }
    // An open-ended range gets a piece at a time.
    Ok(Some((start, end.min(start + CHUNK - 1))))
}

/// The answer to a request for the file: all of it, or the range asked for.
fn serve(path: &Path, range: Option<&str>) -> wry::http::Response<Vec<u8>> {
    use std::io::{Read as _, Seek as _};
    let mime = mime(path);
    let fail = |status: u16| wry::http::Response::builder().status(status).body(Vec::new()).unwrap_or_default();
    let Ok(mut file) = std::fs::File::open(path) else { return fail(404) };
    let Ok(len) = file.metadata().map(|m| m.len()) else { return fail(404) };
    // `nosniff`: a file called .pdf that holds a web page is still only offered as a PDF.
    let base = || wry::http::Response::builder().header("Content-Type", mime).header("X-Content-Type-Options", "nosniff").header("Accept-Ranges", "bytes").header("Cache-Control", "no-store");
    match parse_range(range, len) {
        Ok(Some((start, end))) => {
            let mut body = vec![0; (end - start + 1) as usize];
            if file.seek(std::io::SeekFrom::Start(start)).and_then(|_| file.read_exact(&mut body)).is_err() {
                return fail(500);
            }
            base().status(206).header("Content-Range", format!("bytes {start}-{end}/{len}")).body(body).unwrap_or_default()
        }
        Ok(None) if len > WHOLE_LIMIT => fail(413),
        Ok(None) => {
            let mut body = Vec::with_capacity(len as usize);
            if file.read_to_end(&mut body).is_err() {
                return fail(500);
            }
            base().status(200).body(body).unwrap_or_default()
        }
        Err(()) => wry::http::Response::builder().status(416).header("Content-Range", format!("bytes */{len}")).body(Vec::new()).unwrap_or_default(),
    }
}

const PAUSE_JS: &str = "document.querySelectorAll('video,audio').forEach(function(m){m.pause()})";

/// A native web view made for `path`.
struct NativeWeb(Entity<gpui_wry::WebView>);

impl WebSurface for NativeWeb {
    fn element(&self) -> AnyElement {
        self.0.clone().into_any_element()
    }

    /// Told whenever it changes (and once as it's made): `WebView::visible` starts out `true` for
    /// a view built hidden, so it can't say whether the native view shows.
    fn set_shown(&self, shown: bool, cx: &mut App) {
        self.0.update(cx, |v, _| {
            if shown {
                v.show();
            } else {
                // A hidden view plays on: a video in a tab behind stops when it's left.
                let _ = v.evaluate_script(PAUSE_JS);
                v.hide();
            }
        });
    }

    #[cfg(all(feature = "shots", windows))]
    fn native(&self) -> Option<Entity<gpui_wry::WebView>> {
        Some(self.0.clone())
    }
}

#[cfg_attr(test, allow(dead_code))]
fn native_web(path: &Path, window: &mut Window, cx: &mut App) -> Option<Box<dyn WebSurface>> {
    use raw_window_handle::HasWindowHandle as _;
    let handle = window.window_handle().ok()?;
    let file = path.to_path_buf();
    let origin = format!("{}/", file_origin(cfg!(windows)));
    #[cfg(windows)]
    let mut context = wry::WebContext::new(Some(crate::panels::browser::webview_data_dir()));
    // It mustn't take the keys from Trek (WebView2 focuses a new view): a click on it does.
    #[cfg(windows)]
    let builder = wry::WebViewBuilder::new_with_web_context(&mut context).with_focused(false);
    #[cfg(not(windows))]
    let builder = wry::WebViewBuilder::new();
    // A PDF opens as itself, in the engine's viewer; a video or a song in the page that holds it.
    let start = if kind_by_name(path) == Some(Kind::Media) { origin.clone() } else { file_url(path) };
    let built = builder
        .with_url(start)
        .with_visible(false)
        .with_devtools(false)
        .with_accept_first_mouse(true)
        // Every request under the scheme gets this file (or its page), whatever it names: nothing
        // else is served.
        .with_asynchronous_custom_protocol(FILE_SCHEME.into(), move |_, request, responder| {
            let file = file.clone();
            let asked = request.uri().path().to_string();
            let range = request.headers().get("range").and_then(|r| r.to_str().ok()).map(str::to_string);
            std::thread::spawn(move || responder.respond(answer(&file, &asked, range.as_deref())));
        })
        // The file's own page only: a link in a PDF goes nowhere from here.
        .with_navigation_handler(move |url| url.starts_with(&origin))
        .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
        .with_download_started_handler(|_, _| false)
        .build_as_child(&handle)
        .inspect_err(|e| tracing::warn!("preview: no web view: {e}"))
        .ok()?;
    Some(Box::new(NativeWeb(cx.new(|cx| gpui_wry::WebView::new(built, window, cx)))))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's prelude has a `test` macro of its own.
    use super::{CHUNK, Kind, MEDIA_CSP, WHOLE_LIMIT, answer, cloud_app, file_origin, file_url, kind, kind_label, parse_range, serve, unreadable_words};
    use std::path::Path;

    #[test]
    fn images_pdfs_and_media_go_by_name_and_the_rest_by_what_they_hold() {
        for (name, want) in [
            ("shot.PNG", Kind::Image),
            ("photo.jpeg", Kind::Image),
            ("logo.svg", Kind::Image),
            ("favicon.ico", Kind::Image),
            ("paper.pdf", Kind::Document),
            ("clip.mp4", Kind::Media),
            ("clip.MOV", Kind::Media),
            ("song.flac", Kind::Media),
        ] {
            assert_eq!(kind(Path::new(name), b"anything"), want, "{name}");
        }
        assert_eq!(kind(Path::new("notes.txt"), b"plain words\n"), Kind::Text);
        assert_eq!(kind(Path::new("notes.txt"), b"plain\0words"), Kind::Binary, "a NUL makes even a .txt binary");
        assert_eq!(kind(Path::new("data.bin"), "plain UTF-8, ünïcode\n".as_bytes()), Kind::Text, "a .bin of text is text");
        assert_eq!(kind(Path::new("Makefile"), b""), Kind::Text, "an empty file is text");
        assert_eq!(kind(Path::new("latin1.txt"), b"caf\xe9 au lait"), Kind::Binary, "not UTF-8");
        // A character cut off by the sample's edge is still text.
        let cut = &"é".as_bytes()[..1];
        assert_eq!(kind(Path::new("long.txt"), &[b"x".as_slice(), cut].concat()), Kind::Text);
        assert_eq!(kind(Path::new(r"C:\Users\me\Pictures\a.webp"), b""), Kind::Image, "a Windows path anywhere");
    }

    #[test]
    fn the_status_bar_names_the_kind() {
        assert_eq!(kind_label(Path::new("a.png"), Kind::Image), "PNG Image");
        assert_eq!(kind_label(Path::new("a.pdf"), Kind::Document), "PDF");
        assert_eq!(kind_label(Path::new("a.mp4"), Kind::Media), "MP4 Video");
        assert_eq!(kind_label(Path::new("a.mp3"), Kind::Media), "MP3 Audio");
        assert_eq!(kind_label(Path::new("a.bin"), Kind::Binary), "BIN File");
        assert_eq!(kind_label(Path::new("blob"), Kind::Binary), "Binary");
        assert_eq!(kind_label(Path::new("a.rs"), Kind::Text), "Rust");
    }

    #[test]
    fn a_file_onedrive_cant_fetch_says_what_to_do() {
        // Tobias's beta.3: OneDrive wasn't running, and Windows said only "(os error 362)".
        let not_running = std::io::Error::from_raw_os_error(362);
        let path = Path::new(r"C:\Users\me\OneDrive\Documents\game\launcher_imgui.ini");
        assert_eq!(cloud_app(path), Some("OneDrive"));
        assert_eq!(
            unreadable_words("launcher_imgui.ini", &not_running, cloud_app(path), true),
            "This file is kept online by OneDrive and OneDrive isn't running. Start OneDrive, or right-click the file in Explorer and choose 'Always keep on this device'."
        );
        assert_eq!(cloud_app(Path::new(r"C:\Users\me\OneDrive - Contoso\a.txt")), Some("OneDrive"), "a work OneDrive too");
        let words = unreadable_words("a.txt", &std::io::Error::from_raw_os_error(404), None, true);
        assert!(words.starts_with("This file is kept online by a cloud storage app that isn't running."), "{words}");
        // 362 means nothing of the kind outside Windows.
        assert!(unreadable_words("a.txt", &not_running, Some("OneDrive"), false).starts_with("Can't read a.txt: "));
        assert_eq!(cloud_app(Path::new("/Users/me/Documents/a.txt")), None);
    }

    #[test]
    fn other_errors_name_the_file_and_say_the_error() {
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Access is denied.");
        assert_eq!(unreadable_words("secret.key", &denied, None, true), "Can't read secret.key: Access is denied.");
        #[cfg(windows)]
        {
            let words = unreadable_words("secret.key", &std::io::Error::from_raw_os_error(5), None, true);
            assert!(words.starts_with("Can't read secret.key: ") && words.contains("os error 5"), "{words}");
        }
    }

    #[test]
    fn ranges_are_served_a_piece_at_a_time() {
        assert_eq!(parse_range(None, 100), Ok(None));
        assert_eq!(parse_range(Some("bytes=0-9"), 100), Ok(Some((0, 9))));
        assert_eq!(parse_range(Some("bytes=90-"), 100), Ok(Some((90, 99))));
        assert_eq!(parse_range(Some("bytes=-10"), 100), Ok(Some((90, 99))));
        assert_eq!(parse_range(Some("bytes=50-500"), 100), Ok(Some((50, 99))), "clamped to the file");
        assert_eq!(parse_range(Some("bytes=0-"), 100 << 20), Ok(Some((0, CHUNK - 1))), "an open end gets a piece");
        assert_eq!(parse_range(Some("bytes=100-"), 100), Err(()), "past the end");
        assert_eq!(parse_range(Some("bytes=9-3"), 100), Err(()));
        assert_eq!(parse_range(Some("bytes=x-1"), 100), Err(()));
        assert_eq!(parse_range(Some("items=0-1"), 100), Ok(None), "not bytes: the whole file");
    }

    #[test]
    fn the_view_is_served_the_file_it_was_made_for_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("trek-preview-serve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.mp4");
        std::fs::write(&file, b"0123456789").unwrap();
        let whole = serve(&file, None);
        assert_eq!(whole.status(), 200);
        assert_eq!(whole.headers()["content-type"], "video/mp4");
        assert_eq!(whole.body(), b"0123456789");
        assert_eq!(whole.headers()["x-content-type-options"], "nosniff");
        // Whole, a file is held in memory: past the limit only pieces are handed over.
        let huge = dir.join("huge.pdf");
        std::fs::File::create(&huge).unwrap().set_len(WHOLE_LIMIT + 1).unwrap();
        assert_eq!(serve(&huge, None).status(), 413, "no 256 MB allocation to answer a request");
        assert_eq!(serve(&huge, Some("bytes=0-9")).body(), &[0u8; 10]);
        let part = serve(&file, Some("bytes=2-4"));
        assert_eq!(part.status(), 206);
        assert_eq!(part.headers()["content-range"], "bytes 2-4/10");
        assert_eq!(part.body(), b"234");
        assert_eq!(serve(&file, Some("bytes=20-")).status(), 416);
        assert_eq!(serve(&dir.join("gone.mp4"), None).status(), 404);
        std::fs::remove_dir_all(&dir).unwrap();
        // The address names the file, encoded, under the scheme's own origin.
        assert_eq!(file_url(Path::new("/x/my clip#1.mp4")), format!("{}/my%20clip%231.mp4", file_origin(cfg!(windows))));
        assert_eq!(file_origin(true), "http://trek-file.preview");
        assert_eq!(file_origin(false), "trek-file://preview");
    }

    #[test]
    fn video_and_audio_play_in_a_page_that_waits_to_be_played_and_runs_nothing() {
        let dir = std::env::temp_dir().join(format!("trek-preview-page-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (clip, song, pdf) = (dir.join("my clip.mp4"), dir.join("song.mp3"), dir.join("paper.pdf"));
        for f in [&clip, &song, &pdf] {
            std::fs::write(f, b"bytes").unwrap();
        }
        let page = answer(&clip, "/", None);
        assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
        assert_eq!(page.headers()["content-security-policy"], MEDIA_CSP);
        let html = String::from_utf8(page.body().clone()).unwrap();
        assert!(html.contains("<video src=\"/my%20clip.mp4\" controls controlslist=\"nodownload\""), "{html}");
        assert_eq!(page.headers()["x-content-type-options"], "nosniff");
        assert!(!html.contains("autoplay") && !html.contains("<script"), "{html}");
        assert!(String::from_utf8(answer(&song, "/", None).body().clone()).unwrap().contains("<audio src=\"/song.mp3\" controls"));
        // The file itself under any other path; a PDF is the file even at `/`.
        assert_eq!(answer(&clip, "/my%20clip.mp4", None).body(), b"bytes");
        assert_eq!(answer(&clip, "/../../etc/passwd", None).body(), b"bytes", "only ever the one file");
        assert_eq!(answer(&pdf, "/", None).headers()["content-type"], "application/pdf");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
