//! Interactive artifacts: a `trek-html` block in an answer runs, live, in a card in the answer.
//!
//! The block is the agent's own HTML document (`trek_core::artifact` has the format, the sandbox's
//! rules and what agents are told). Closed, it becomes a card: a header with its title and Reload,
//! Copy source and Expand, over a web view as wide as the answer and as tall as the document says
//! it is (160–720 px; it scrolls inside past that). While the agent is still writing it, the card
//! is a placeholder ("Building…"), and nothing runs.
//!
//! The web view is a native view over the window (`native.rs`), so the card does by hand what GPUI
//! does for everything else (`surface.rs`): each frame it is placed where the card is, cut to what
//! the transcript shows of it, and hidden when anything of Trek's lies over it — a menu, a dialog, a
//! toast — while a snapshot of the page stands in, drawn by GPUI under the overlay. Views are made
//! only for cards near the screen while the window is in front (making one brings the app
//! forward), at most `MAX_LIVE` a window; one scrolled far away is let go, its snapshot kept.
//!
//! Each window's root draws `layer_start` first and `layer_end` last: the start numbers the frame,
//! the end places every card's view from what its card recorded as it was laid out (cached views
//! replay their last frame without laying out again, so the end runs each frame regardless).

// Tests use stand-in views (`fake_web`): the native one is never made there.
#[cfg_attr(test, allow(dead_code))]
mod native;
mod surface;

use crate::assets::Lucide;
use crate::visualization::{Look, header, shell, summary_text, title_text};
use gpui_kit::component::text::{MarkdownNode, MarkdownParseContext, MarkdownPlugin, markdown_ast};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use surface::{Coverage, Placement, Surface};
use trek_core::artifact as core;

const PLUGIN: &str = "trek-artifact";
/// Live web views a window keeps at once; past this, the one farthest off screen is let go.
const MAX_LIVE: usize = 4;
/// Snapshots kept for cards whose views have gone (by document).
const SHOTS_KEPT: usize = 24;
/// The card's corner radius, less its 1 px edge: the bottom corners of what's drawn in it.
const RADIUS: f32 = 11.;
/// How long after a load (or a theme switch) the page is pictured, for its snapshot.
const SETTLE: Duration = Duration::from_millis(700);
/// A page that keeps changing its height is let be at this many changes.
const MAX_HEIGHT_CHANGES: u32 = 60;
/// Links the page sends to the browser: at most one this often.
const OPEN_EVERY: Duration = Duration::from_secs(1);

// ---------- the block ----------

/// A closed `trek-html` block: the document and what the card shows of it.
pub(crate) struct Doc {
    pub html: Arc<str>,
    pub hash: u64,
    pub title: Option<String>,
    /// Where it loads from, for the header ("Loads from cdn.jsdelivr.net").
    pub hosts: Vec<String>,
    /// The block as written, fences and all (shown as code when artifacts are off).
    pub source: Arc<str>,
    /// A video file from a note this plays (`html` is the player's page, `core::VIDEO_HTML`).
    pub media: Option<std::path::PathBuf>,
}

impl Doc {
    /// The policy it's served under.
    fn csp(&self) -> &'static str {
        if self.media.is_some() { core::VIDEO_CSP } else { core::CSP }
    }
}

#[derive(Clone)]
enum Block {
    Ready(Arc<Doc>),
    /// Its fence hasn't closed: the title so far, how much is written, the block as written.
    Building { title: Option<String>, bytes: usize, source: Arc<str> },
    /// Closed, but too big to run.
    TooBig { bytes: usize, source: Arc<str> },
}

/// Claims every `trek-html` fence in an answer. `live`: the answer is still being written, so a
/// fence still open is on its way. `size`: the answer's text size, for a block shown as code.
#[derive(Clone, Copy)]
pub struct ArtifactPlugin {
    pub live: bool,
    pub size: Pixels,
}

impl MarkdownPlugin for ArtifactPlugin {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        PLUGIN
    }

    // Runs on each delta while the block is the answer's last: a title scan and a hash, off the
    // main thread.
    fn parse(&self, node: &markdown_ast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let markdown_ast::Node::Code(code) = node else { return None };
        if code.lang.as_deref() != Some(core::FENCE) {
            return None;
        }
        let source = cx.node_source(node)?;
        let html = code.value.as_str();
        let title = core::title(html);
        let block = if !core::fence_closed(source) {
            Block::Building { title: title.clone(), bytes: html.len(), source: source.into() }
        } else if html.len() > core::MAX_BYTES {
            Block::TooBig { bytes: html.len(), source: source.into() }
        } else {
            Block::Ready(Arc::new(Doc { html: html.into(), hash: core::content_hash(html), title: title.clone(), hosts: core::hosts(html, 3), source: source.into(), media: None }))
        };
        let text = format!("Interactive artifact{}", title.map(|t| format!(": {t}")).unwrap_or_default());
        Some(MarkdownNode::new(PLUGIN, block).text(text.clone()).markdown(source.to_string()).accessibility_label(text))
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let key = node.source_range().map(|range| range.start).unwrap_or(0);
        let on = enabled(cx);
        let shown = match node.data::<Block>() {
            Some(Block::Ready(doc)) if on => card(doc, key, window, cx),
            Some(Block::Ready(doc)) => as_code(&doc.source, key, self.size, cx),
            Some(Block::Building { source, .. }) if !on => as_code(source, key, self.size, cx),
            Some(Block::Building { title, bytes, .. }) if self.live => building(title.as_deref(), *bytes, key, window, cx),
            Some(Block::Building { source, .. }) => note("the answer ended before the block was finished", source, key, self.size, cx),
            Some(Block::TooBig { bytes, source }) => note(&format!("it's {} KB, past the {} KB Trek runs", bytes / 1024, core::MAX_BYTES / 1024), source, key, self.size, cx),
            None => return div().into_any_element(),
        };
        // Padding, not margin: a margin here collapses through the Markdown block it sits in.
        div().py_2().child(shown).into_any_element()
    }
}

/// While a page has the keys, GPUI's focus is on its card (the `Artifact` context): these are the
/// page's there, not Trek's. A Mac sends ⌘-keys (and some others) to the window's views before the
/// web view that has the keys, so without this ⌘C would copy Trek's text selection and Esc would
/// stop the agent rather than close the page's own popup. Trek's other shortcuts (⌘W, ⌘K, ⌘N…)
/// still reach Trek; Esc that the page doesn't use gives the keys back (`Message::Escape`).
pub fn key_bindings() -> Vec<KeyBinding> {
    let editing: &[&str] = if cfg!(target_os = "macos") {
        &["cmd-c", "cmd-v", "cmd-x", "cmd-a", "cmd-z", "cmd-shift-z", "cmd-f", "cmd-g", "cmd-shift-g"]
    } else {
        &["ctrl-c", "ctrl-v", "ctrl-x", "ctrl-a", "ctrl-z", "ctrl-y", "ctrl-shift-z", "ctrl-f"]
    };
    let plain = ["escape", "enter", "space", "tab", "shift-tab", "backspace", "delete", "up", "down", "left", "right", "home", "end", "pageup", "pagedown"];
    editing.iter().copied().chain(plain).map(|k| KeyBinding::new(k, NoAction {}, Some("Artifact"))).collect()
}

/// Interactive artifacts are on (Settings › General).
fn enabled(cx: &App) -> bool {
    cx.try_global::<crate::workspace::GlobalWorkspace>().is_none_or(|ws| ws.0.read(cx).settings.general.interactive_artifacts)
}

fn as_code(source: &str, key: usize, size: Pixels, cx: &App) -> AnyElement {
    crate::md::keyed(SharedString::from(format!("artifact-code-{key}")), source.to_string(), None, None, size, false, false, None, cx).into_any_element()
}

/// A block that can't run: a short note saying why, over it as written.
fn note(reason: &str, source: &str, key: usize, size: Pixels, cx: &App) -> AnyElement {
    let look = Look::of(cx);
    let text = format!("Couldn't run this interactive artifact: {reason}");
    v_flex()
        .id(SharedString::from(format!("artifact-note-{key}")))
        .aria_label(text.clone())
        .test_support()
        .w_full()
        .gap(px(6.))
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap(px(8.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(8.))
                .border_1()
                .border_color(look.hairline)
                .bg(crate::palette::amber(cx).opacity(if look.dark { 0.06 } else { 0.07 }))
                .text_size(px(12.))
                .line_height(relative(1.45))
                .child(div().flex_none().h(px(12. * 1.45)).flex().items_center().child(Icon::new(IconName::TriangleAlert).size(px(13.)).text_color(crate::palette::amber(cx))))
                .child(div().flex_1().min_w_0().text_color(look.muted).child(text)),
        )
        .child(as_code(source, key, size, cx))
        .into_any_element()
}

// ---------- the cards ----------

/// "Building…", breathing: the corner of a card still being written.
fn building_label(phase: f32, look: &Look) -> AnyElement {
    let breath = 0.55 + 0.45 * (phase * std::f32::consts::TAU).cos().abs();
    h_flex().flex_none().h(px(24.)).pr_1().text_size(px(11.)).text_color(look.muted).opacity(breath).child("Building…").into_any_element()
}

/// The card while the agent is still writing the document: nothing runs until it's whole.
fn building(title: Option<&str>, bytes: usize, key: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let look = Look::of(cx);
    let motion = crate::visualization::motion(cx);
    let phase = crate::visualization::shimmer(motion, window);
    let stub = look.fg.opacity(if look.dark { 0.07 } else { 0.06 });
    let bar = |w: f32, h: f32| div().w(relative(w)).h(px(h)).rounded(px(h / 2.)).bg(stub);
    let title_el = match title {
        Some(t) => title_text(t.to_string(), &look),
        None => div().h(px(17.)).flex().items_center().child(bar(0.42, 10.)).into_any_element(),
    };
    let summary = format!("Interactive · {} KB written; it runs once it's finished", bytes.div_ceil(1024));
    let band = look.fg.opacity(if look.dark { 0.05 } else { 0.055 });
    const SLICES: usize = 16;
    let sweep = h_flex().absolute().top_0().bottom_0().left(relative(phase * 1.6 - 0.4)).w(relative(0.4)).children((0..SLICES).map(|i| {
        let k = ((i as f32 + 0.5) / SLICES as f32 * std::f32::consts::PI).sin();
        div().flex_1().h_full().bg(band.opacity(k * k))
    }));
    shell(SharedString::from(format!("artifact-building-{key}")), &look, cx)
        .aria_label(format!("Interactive artifact being written{}", title.map(|t| format!(": {t}")).unwrap_or_default()))
        .test_support()
        .child(header(Lucide::Box, title_el, building_label(phase, &look), summary_text(summary, &look), &look))
        .child(div().w_full().px_4().pb_4().child(crate::visualization::inset(&look).relative().overflow_hidden().w_full().h(px(200.)).when(motion, |el| el.child(sweep))))
        .into_any_element()
}

/// What the header says under the title: that it's live, and where it reaches.
fn summary_of(doc: &Doc) -> String {
    if let Some(file) = &doc.media {
        let kind = file.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
        let size = std::fs::metadata(file).ok().map(|m| crate::image_preview::file_size(m.len()));
        return ["Video".to_string(), kind].into_iter().filter(|p| !p.is_empty()).chain(size).collect::<Vec<_>>().join(" · ");
    }
    match doc.hosts.as_slice() {
        [] => "Interactive · self-contained, in a sandbox".to_string(),
        hosts => format!("Interactive · loads from {}", hosts.join(", ")),
    }
}

/// A video in a note, played in place in a card; `None` when interactive artifacts are off (the
/// note shows its file chip instead).
pub(crate) fn video_card(doc: &Arc<Doc>, key: usize, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    enabled(cx).then(|| card(doc, key, window, cx))
}

fn card(doc: &Arc<Doc>, key: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let state = window.use_keyed_state(SharedString::from(format!("trek-artifact-state-{key}")), cx, |window, cx| ArtifactState::new(doc.clone(), Mode::Inline, window, cx));
    let css = theme_css(cx);
    let background = card_rgb(cx);
    state.update(cx, |s, cx| s.sync_doc(doc, css, background, cx));
    let look = Look::of(cx);
    let s = state.read(cx);
    let height = s.height;
    let status = s.status();
    let focus = s.focus.clone();
    let video = doc.media.clone();
    let title = doc.title.clone().unwrap_or_else(|| if video.is_some() { "Video" } else { "Interactive artifact" }.to_string());
    let group = SharedString::from(format!("artifact-group-{key}"));
    let actions = {
        let (reload, expand, html) = (state.clone(), doc.clone(), doc.html.to_string());
        let bar = h_flex().flex_none().gap(px(2.)).opacity(0.72).group_hover(group.clone(), |s| s.opacity(1.));
        let expand_button = action(SharedString::from(format!("artifact-expand-{key}")), Lucide::Maximize2, "Expand", &look, move |window, cx| expand_artifact(&expand, window, cx));
        match video {
            // A video: open it in its own app, or expand it. No reload or source to copy.
            Some(file) => bar
                .child(action(SharedString::from(format!("artifact-open-{key}")), Lucide::SquareArrowOutUpRight, "Open in the default app", &look, move |_, cx| cx.open_with_system(&file)))
                .child(expand_button)
                .into_any_element(),
            None => bar
                .child(action(SharedString::from(format!("artifact-reload-{key}")), Lucide::RefreshCw, "Reload", &look, move |_, cx| reload.update(cx, |s, cx| s.reload(cx))))
                .child(action(SharedString::from(format!("artifact-copy-{key}")), Lucide::Copy, "Copy source", &look, move |window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(html.clone()));
                    crate::toast::push(window, "Artifact source copied", cx);
                }))
                .child(expand_button)
                .into_any_element(),
        }
    };
    let body = div()
        .id(("artifact-body", key))
        .test_support()
        .track_focus(&focus)
        .key_context("Artifact")
        .relative()
        .w_full()
        // The page's height, and the hairline over it.
        .h(px(height + 1.))
        .border_t_1()
        .border_color(look.hairline)
        .child(status_layer(status, &look))
        .child(SurfaceElement { state: state.clone() });
    shell(SharedString::from(format!("trek-artifact-{key}")), &look, cx)
        .aria_label(if doc.media.is_some() { format!("Video: {title}") } else { format!("Interactive artifact: {title}") })
        .test_support()
        .group(group)
        .child(header(if doc.media.is_some() { Lucide::Play } else { Lucide::Box }, title_text(title, &look), actions, summary_text(summary_of(doc), &look), &look))
        .child(body)
        .into_any_element()
}

/// What a card shows under its page: a word while it starts, or why it can't.
#[derive(Clone, PartialEq)]
enum Status {
    Starting,
    Running,
    Unavailable(String),
}

fn status_layer(status: Status, look: &Look) -> AnyElement {
    let text = match status {
        Status::Starting => "Starting…".to_string(),
        Status::Running => return div().into_any_element(),
        Status::Unavailable(why) => why,
    };
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .px_6()
        .text_size(px(12.))
        .text_color(look.muted)
        .text_center()
        .child(text)
        .into_any_element()
}

/// A header button. Its tooltip blocks the mouse, as every overlay does, so a card's web view
/// steps aside for it (`surface::analyze`) rather than covering it.
fn action(id: SharedString, icon: Lucide, tip: &'static str, look: &Look, on_click: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let (muted, hover) = (look.muted, look.fg.opacity(0.06));
    h_flex()
        .id(id)
        .test_support()
        .aria_label(tip)
        .h(px(24.))
        .min_w(px(24.))
        .px(px(5.))
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|s| s.bg(hover))
        .child(Icon::new(icon).size(px(13.)).text_color(muted))
        .tooltip(move |_, cx| cx.new(|_| Tip(tip)).into())
        .on_click(move |_, window, cx| on_click(window, cx))
        .into_any_element()
}

struct Tip(&'static str);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .occlude()
            .mt(px(4.))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .text_size(px(12.))
            .child(self.0)
    }
}

/// The artifact large, over the window, in a view of its own (it starts afresh). Esc, the page's
/// Esc or the close button puts it away.
fn expand_artifact(doc: &Arc<Doc>, window: &mut Window, cx: &mut App) {
    let state = cx.new(|cx| ArtifactState::new(doc.clone(), Mode::Expanded, window, cx));
    let css = theme_css(cx);
    let background = card_rgb(cx);
    state.update(cx, |s, cx| s.sync_doc(doc, css, background, cx));
    let title = doc.title.clone().unwrap_or_else(|| "Interactive artifact".to_string());
    window.open_dialog(cx, move |dialog, window, cx| {
        let size = window.viewport_size();
        let (w, h) = ((size.width - px(96.)).max(px(420.)), (size.height - px(220.)).max(px(280.)));
        let css = theme_css(cx);
        let background = card_rgb(cx);
        let doc = state.read(cx).doc.clone();
        state.update(cx, |s, cx| s.sync_doc(&doc, css, background, cx));
        let status = state.read(cx).status();
        let look = Look::of(cx);
        dialog
            .w(w)
            .margin_top(px(56.))
            .title(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(title.clone()))
            .child(
                div()
                    .id("artifact-expanded")
                    .test_support()
                    .track_focus(&state.read(cx).focus.clone())
                    .key_context("Artifact")
                    .relative()
                    .w_full()
                    .h(h)
                    .rounded(px(8.))
                    .overflow_hidden()
                    .child(status_layer(status, &look))
                    .child(SurfaceElement { state: state.clone() }),
            )
    });
}

// ---------- theme ----------

fn css_color(c: Hsla) -> String {
    let c = c.to_rgb();
    format!("rgba({:.0},{:.0},{:.0},{:.3})", c.r * 255., c.g * 255., c.b * 255., c.a)
}

/// A font as CSS: the system's UI font for GPUI's (`.SystemUIFont`), else the named one, then the
/// system's as fallbacks (a font Trek embeds isn't one the web view knows).
fn font_stack(name: &str, mono: bool) -> String {
    let fallback = if mono { "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" } else { "-apple-system, BlinkMacSystemFont, \"Segoe UI Variable Text\", \"Segoe UI\", system-ui, sans-serif" };
    let name = name.trim();
    if name.is_empty() || name.starts_with('.') || name.eq_ignore_ascii_case("system-ui") || name.contains(['"', ';', '{', '}', '<', '\\']) {
        fallback.to_string()
    } else {
        format!("\"{name}\", {fallback}")
    }
}

/// Trek's theme as the document sees it.
pub(crate) fn theme(cx: &App) -> core::Theme {
    let t = cx.theme();
    let dark = t.mode.is_dark();
    let size = cx.try_global::<crate::workspace::GlobalWorkspace>().map(|ws| ws.0.read(cx).settings.appearance.transcript_font_size()).unwrap_or(14.5);
    core::Theme {
        dark,
        background: css_color(t.background),
        foreground: css_color(t.foreground),
        muted: css_color(t.muted_foreground),
        accent: css_color(crate::palette::ember(cx)),
        border: css_color(t.foreground.opacity(if dark { 0.12 } else { 0.13 })),
        surface: css_color(t.foreground.opacity(if dark { 0.045 } else { 0.035 })),
        positive: css_color(crate::palette::emerald(cx)),
        warning: css_color(crate::palette::amber(cx)),
        negative: css_color(crate::palette::red(cx)),
        info: css_color(crate::palette::sky(cx)),
        font: font_stack(&t.font_family, false),
        mono: font_stack(&t.mono_font_family, true),
        font_size: (size * 0.93 * 2.).round() / 2.,
        radius: 8.,
    }
}

fn theme_css(cx: &App) -> String {
    core::theme_css(&theme(cx))
}

/// The card's colour, opaque: what a page shows through to where WebView2 can't be see-through.
fn card_rgb(cx: &App) -> (u8, u8, u8) {
    let t = cx.theme();
    let c = if t.mode.is_dark() { t.background.blend(t.foreground.opacity(0.022)) } else { t.background }.to_rgb();
    ((c.r * 255.).round() as u8, (c.g * 255.).round() as u8, (c.b * 255.).round() as u8)
}

// ---------- state ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// In an answer, in the transcript.
    Inline,
    /// Over the window (Expand).
    Expanded,
}

/// A snapshot standing in for a page: at `size` (window px), in the theme `css` was.
#[derive(Clone)]
struct Snap {
    image: Arc<RenderImage>,
    size: Size<Pixels>,
    css: String,
}

/// Where the card was laid out, and the hit boxes it left, to read what covers it.
#[derive(Clone)]
struct Geom {
    body: Bounds<Pixels>,
    /// What the transcript shows (the content mask it was drawn in).
    mask: Bounds<Pixels>,
    /// The hit box before `cur`, and `cur`, inserted in frame `cur_frame`.
    prev: Option<Hitbox>,
    cur: Option<Hitbox>,
    cur_frame: u64,
}

/// One artifact card's page: its view while it has one, its snapshot, its height. Kept by the
/// window for as long as the card is drawn (`use_keyed_state`), so a card that's no longer drawn
/// (scrolled out of the transcript's rows, another thread shown) takes its view with it.
pub(crate) struct ArtifactState {
    doc: Arc<Doc>,
    mode: Mode,
    /// The theme's stylesheet in the document now.
    css: String,
    background: (u8, u8, u8),
    /// The document as the view is served it.
    served: Rc<RefCell<String>>,
    /// The view's secret, which its bootstrap tags messages with.
    nonce: String,
    surface: Option<Box<dyn Surface>>,
    /// No view could be made: why.
    unavailable: Option<String>,
    loaded: bool,
    pub height: f32,
    shot: Option<Snap>,
    snapping: bool,
    geom: Option<Geom>,
    /// Its view is on screen.
    live: bool,
    focus: FocusHandle,
    /// The page has the keys; what GPUI had focused before it took them.
    has_keys: bool,
    had_focus: Option<FocusHandle>,
    opened_at: Option<Instant>,
    height_changes: u32,
    _events: Option<Task<()>>,
}

impl ArtifactState {
    fn new(doc: Arc<Doc>, mode: Mode, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = window.window_handle().window_id();
        let me = cx.weak_entity();
        let registry = cx.default_global::<Registry>();
        registry.windows.entry(id).or_default().states.push(me);
        let height = registry.heights.get(&doc.hash).copied().unwrap_or(core::DEFAULT_HEIGHT);
        let shot = registry.shots.iter().rev().find(|(h, _)| *h == doc.hash).map(|(_, s)| s.clone());
        Self {
            served: Rc::new(RefCell::new(String::new())),
            doc,
            mode,
            css: String::new(),
            background: (0, 0, 0),
            nonce: nonce(),
            surface: None,
            unavailable: None,
            loaded: false,
            height,
            shot,
            snapping: false,
            geom: None,
            live: false,
            focus: cx.focus_handle(),
            has_keys: false,
            had_focus: None,
            opened_at: None,
            height_changes: 0,
            _events: None,
        }
    }

    fn status(&self) -> Status {
        match (&self.unavailable, self.loaded) {
            (Some(why), _) => Status::Unavailable(why.clone()),
            (None, false) if self.shot.is_none() => Status::Starting,
            _ => Status::Running,
        }
    }

    /// Keep the document and its theme as the card has them: a new document (the same place in
    /// an answer that changed) loads afresh; a theme switch reaches the page in place.
    fn sync_doc(&mut self, doc: &Arc<Doc>, css: String, background: (u8, u8, u8), cx: &mut Context<Self>) {
        let new_doc = doc.hash != self.doc.hash;
        if !new_doc && css == self.css {
            return;
        }
        if new_doc {
            self.doc = doc.clone();
            self.height = cx.try_global::<Registry>().and_then(|r| r.heights.get(&doc.hash).copied()).unwrap_or(core::DEFAULT_HEIGHT);
            self.shot = None;
            self.height_changes = 0;
        }
        let restyle = !new_doc && !self.css.is_empty();
        self.css = css;
        *self.served.borrow_mut() = core::document_with(&self.doc.html, &self.css, self.doc.csp());
        if let Some(surface) = &self.surface {
            if new_doc {
                self.loaded = false;
                surface.reload();
            } else if restyle {
                surface.eval(&core::theme_update_script(&self.css));
                if self.background != background {
                    surface.set_background(background);
                }
                self.settle_then_snap(cx);
            }
        }
        self.background = background;
    }

    /// Load the document again (Reload).
    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(surface) = &self.surface {
            self.loaded = false;
            self.height_changes = 0;
            surface.reload();
            cx.notify();
        }
    }

    /// Make the view (the card is near the screen and the window is in front).
    fn make(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = async_channel::unbounded();
        let spec = native::Spec {
            origin: core::origin(self.doc.hash, cfg!(windows)),
            document: self.served.clone(),
            csp: self.doc.csp(),
            media: self.doc.media.clone(),
            bootstrap: core::bootstrap(&self.nonce),
            background: self.background,
            events: tx,
        };
        match make_surface(spec, window) {
            Ok(surface) => {
                self.surface = Some(surface);
                self.loaded = false;
                self._events = Some(cx.spawn_in(window, async move |this, cx| {
                    while let Ok(event) = rx.recv().await {
                        if this.update_in(cx, |this, window, cx| this.on_event(event, window, cx)).is_err() {
                            break;
                        }
                    }
                }));
            }
            Err(why) => {
                tracing::warn!("artifact: no web view: {why}");
                self.unavailable = Some(if cfg!(windows) { "Interactive artifacts need Microsoft's WebView2 runtime.".to_string() } else { format!("Couldn't start a web view: {why}") });
                cx.notify();
            }
        }
    }

    /// Let go of the view (far off screen, or past `MAX_LIVE`); its snapshot stays.
    fn drop_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut surface) = self.surface.take() {
            surface.place(None, window);
            self.release_keys(window, cx);
            self._events = None;
            self.loaded = false;
            if self.live {
                self.live = false;
                cx.notify();
            }
        }
    }

    fn on_event(&mut self, event: native::Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            native::Event::Loaded => {
                self.loaded = true;
                cx.notify();
                self.settle_then_snap(cx);
            }
            native::Event::Popup(url) => self.open_link(&url, cx),
            native::Event::Message(body) => match core::parse_message(&body, &self.nonce) {
                Some(core::Message::Height(h)) => self.set_height(h, cx),
                Some(core::Message::Focus) => {
                    if !self.has_keys {
                        self.has_keys = true;
                        self.had_focus = window.focused(cx).filter(|f| *f != self.focus);
                        // GPUI's focus moves to the card, where nothing is bound: the editing
                        // keys (⌘C, ⌘V, ⌘A…) reach the page, and Trek's own shortcuts still work.
                        window.focus(&self.focus, cx);
                    }
                }
                Some(core::Message::Escape) => {
                    if self.mode == Mode::Expanded {
                        window.close_dialog(cx);
                    } else {
                        self.release_keys(window, cx);
                    }
                }
                Some(core::Message::Open(url)) => self.open_link(&url, cx),
                Some(core::Message::Wheel(dx, dy)) => self.scroll_around(dx, dy, window, cx),
                None => tracing::debug!("artifact: message ignored"),
            },
        }
    }

    /// A wheel the page didn't use: the transcript under the card scrolls, as GPUI would scroll it
    /// for a wheel over its own content (the web view takes the wheel from GPUI).
    fn scroll_around(&mut self, dx: f32, dy: f32, window: &mut Window, cx: &mut App) {
        if self.mode != Mode::Inline {
            return;
        }
        let Some(g) = &self.geom else { return };
        let visible = g.body.intersect(&g.mask);
        if visible.size.height <= px(0.) {
            return;
        }
        let event = ScrollWheelEvent { position: visible.center(), delta: ScrollDelta::Pixels(point(px(-dx), px(-dy))), ..Default::default() };
        window.dispatch_event(event.to_platform_input(), cx);
    }

    /// A link from the page, to the user's browser: web or mail only, one a second at most.
    fn open_link(&mut self, url: &str, cx: &mut Context<Self>) {
        let Some(url) = core::external(url) else { return };
        if self.opened_at.is_some_and(|at| at.elapsed() < OPEN_EVERY) {
            return;
        }
        self.opened_at = Some(Instant::now());
        cx.open_url(&url);
    }

    fn set_height(&mut self, h: f32, cx: &mut Context<Self>) {
        if self.mode == Mode::Expanded {
            return;
        }
        let h = core::clamp_height(h);
        if (h - self.height).abs() < 1. || self.height_changes >= MAX_HEIGHT_CHANGES {
            return;
        }
        self.height_changes += 1;
        self.height = h;
        cx.default_global::<Registry>().heights.insert(self.doc.hash, h);
        cx.notify();
        crate::visualization::relayout(cx);
        self.settle_then_snap(cx);
    }

    /// The keys back to Trek, and its focus where it was before the page took them.
    fn release_keys(&mut self, window: &mut Window, cx: &mut App) {
        if !self.has_keys {
            return;
        }
        self.has_keys = false;
        if let Some(surface) = &self.surface {
            surface.release_keys(window);
        }
        if let Some(had) = self.had_focus.take() {
            window.focus(&had, cx);
        }
    }

    /// Picture the page once it has settled (a load, a new height, a theme), so a snapshot is
    /// ready before it's needed.
    fn settle_then_snap(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let _ = this.update(cx, |this, cx| {
                if this.live {
                    this.snap(cx);
                }
            });
        })
        .detach();
    }

    /// Picture the page now.
    fn snap(&mut self, cx: &mut Context<Self>) {
        if self.snapping || !self.loaded {
            return;
        }
        let (Some(surface), Some(geom)) = (&self.surface, &self.geom) else { return };
        let Some(rx) = surface.snapshot() else { return };
        self.snapping = true;
        let (size, css, hash) = (geom.body.size, self.css.clone(), self.doc.hash);
        cx.spawn(async move |this, cx| {
            let shot = match rx.recv().await {
                Ok(Ok(bytes)) => cx.background_executor().spawn(async move { native::decode(&bytes) }).await,
                Ok(Err(e)) => Err(e),
                Err(_) => Err("the view went".to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                this.snapping = false;
                match shot.and_then(|s| image::RgbaImage::from_raw(s.width, s.height, s.bgra).ok_or_else(|| "a short picture".to_string())) {
                    Ok(pixels) => {
                        let snap = Snap { image: Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])), size, css };
                        cx.default_global::<Registry>().keep_shot(hash, snap.clone());
                        this.shot = Some(snap);
                        cx.notify();
                    }
                    Err(e) => tracing::debug!("artifact: no snapshot: {e}"),
                }
            });
        })
        .detach();
    }

    /// Place the view for this frame, `frame`, from where the card was laid out and what covers
    /// it. `overlay`: a Trek surface over the window that hides native views (the palette).
    fn sync(&mut self, frame: u64, overlay: bool, on: bool, window: &mut Window, cx: &mut Context<Self>) -> (bool, f32) {
        // Trek moved its focus elsewhere while the page had the keys (⌘K's palette, ⌘L): the
        // keys follow it, or what's typed into the palette would go to the page.
        if self.has_keys && !self.focus.is_focused(window) {
            self.had_focus = None;
            self.release_keys(window, cx);
        }
        let Some(g) = self.geom.clone() else {
            self.show(None, window, cx);
            return (self.surface.is_some(), f32::INFINITY);
        };
        let visible = g.body.intersect(&g.mask);
        let gap = surface::gap(g.body, g.mask);
        let viewport = f32::from(g.mask.size.height).max(200.);
        // The hit box that's in the frame on screen: this frame's isn't, yet.
        let probe = if g.cur_frame == frame { g.prev.clone() } else { g.cur.clone() };
        let coverage = match &probe {
            Some(hb) if visible.size.width > px(0.) && visible.size.height > px(0.) => {
                let known = hb.bounds.intersect(&hb.content_mask.bounds);
                // Where it wasn't last frame (scrolled in since), nothing is known to cover it.
                surface::analyze(visible, |p| !known.contains(&p) || hb.is_hovered_at(p, window))
            }
            _ => Coverage { clip: None, covered: false },
        };
        if on && self.surface.is_none() && self.unavailable.is_none() && gap <= viewport && window.is_window_active() {
            self.make(window, cx);
        }
        if self.surface.is_some() && (gap > viewport * 2. || !on) {
            self.drop_surface(window, cx);
        }
        let placement = match coverage {
            Coverage { clip: Some(clip), covered: false } if on && !overlay => Some(Placement { body: g.body, clip, round_bottom: self.mode == Mode::Inline && clip.bottom() >= g.body.bottom() - px(0.5) }),
            _ => None,
        };
        self.show(placement, window, cx);
        (self.surface.is_some(), if visible.size.height > px(0.) { 0. } else { gap })
    }

    fn show(&mut self, placement: Option<Placement>, window: &mut Window, cx: &mut Context<Self>) {
        let live = placement.is_some() && self.surface.is_some();
        if self.live && !live {
            // Pictured as it goes: the snapshot is what stands in for it.
            self.snap(cx);
        }
        if let Some(surface) = self.surface.as_mut() {
            surface.place(placement, window);
        }
        if live != self.live {
            self.live = live;
            cx.notify();
            // The expanded card's dialog redraws with the window.
            window.request_animation_frame();
        }
    }
}

impl Drop for ArtifactState {
    fn drop(&mut self) {
        // The native view hides as it goes (`NativeSurface`'s drop): not one frame left over the
        // route that replaced the card.
        self.surface.take();
    }
}

/// A view's secret: 128 random bits, hex.
fn nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

// ---------- the element ----------

/// Records where the card's page is each time it's laid out, leaves a hit box there (what covers
/// it is read from the next frame's hit test), and draws the snapshot while the view isn't live.
struct SurfaceElement {
    state: Entity<ArtifactState>,
}

impl IntoElement for SurfaceElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for SurfaceElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let inset = Edges::<Length> { top: px(0.).into(), left: px(0.).into(), ..Default::default() };
        let style = Style { position: Position::Absolute, inset, size: Size::full(), ..Default::default() };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) {
        let mask = window.content_mask().bounds;
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let frame = cx.default_global::<Registry>().windows.get(&window.window_handle().window_id()).map_or(0, |w| w.frame);
        self.state.update(cx, |s, _| {
            let (prev, cur) = match s.geom.take() {
                // Laid out twice in a frame: the earlier one never reached the screen.
                Some(g) if g.cur_frame == frame => (g.prev, Some(hitbox)),
                Some(g) => (g.cur, Some(hitbox)),
                None => (None, Some(hitbox)),
            };
            s.geom = Some(Geom { body: bounds, mask, prev, cur, cur_frame: frame });
        });
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App) {
        let s = self.state.read(cx);
        if !s.live
            && let Some(shot) = s.shot.as_ref().filter(|shot| shot.css == s.css && (shot.size.width - bounds.size.width).abs() < px(1.5))
        {
            let image = Bounds::new(bounds.origin, shot.size);
            let r = if s.mode == Mode::Inline { px(RADIUS) } else { px(0.) };
            let corners = Corners { top_left: px(0.), top_right: px(0.), bottom_left: r, bottom_right: r };
            let _ = window.paint_image(bounds, image, corners, shot.image.clone(), 0, false);
        }
        // A click anywhere else in Trek takes the keys back from the page (GPUI's view doesn't
        // take them by itself when it's clicked).
        let state = self.state.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture && !bounds.contains(&e.position) && state.read(cx).has_keys {
                state.update(cx, |s, cx| {
                    s.had_focus = None;
                    s.release_keys(window, cx);
                });
            }
        });
    }
}

// ---------- the window layer ----------

/// Each window's cards, and the frame it's on.
#[derive(Default)]
struct WindowArtifacts {
    states: Vec<WeakEntity<ArtifactState>>,
    frame: u64,
    /// This frame was asked for by the last one, to see what it drew.
    followup: bool,
}

#[derive(Default)]
struct Registry {
    windows: HashMap<WindowId, WindowArtifacts>,
    /// Documents' heights as they last said, so a card made again (scrolled back, the thread
    /// shown again) takes its height at once and the transcript doesn't jump.
    heights: HashMap<u64, f32>,
    /// Snapshots, most recent last, by document.
    shots: Vec<(u64, Snap)>,
    /// Snapshots no longer kept, to take out of the sprite atlas at the next frame.
    retired: Vec<Arc<RenderImage>>,
}

impl Global for Registry {}

impl Registry {
    fn keep_shot(&mut self, hash: u64, snap: Snap) {
        if let Some(ix) = self.shots.iter().position(|(h, _)| *h == hash) {
            let (_, old) = self.shots.remove(ix);
            self.retired.push(old.image);
        }
        self.shots.push((hash, snap));
        if self.shots.len() > SHOTS_KEPT {
            let (_, old) = self.shots.remove(0);
            self.retired.push(old.image);
        }
    }
}

/// Drawn first in a window's root: numbers the frame.
pub fn layer_start() -> impl IntoElement {
    Layer { end: false }
}

/// Drawn last in a window's root: places every card's view in the window.
pub fn layer_end() -> impl IntoElement {
    Layer { end: true }
}

struct Layer {
    end: bool,
}

impl IntoElement for Layer {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Layer {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let style = Style { position: Position::Absolute, size: size(px(0.).into(), px(0.).into()), ..Default::default() };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) {
        let id = window.window_handle().window_id();
        if !self.end {
            cx.default_global::<Registry>().windows.entry(id).or_default().frame += 1;
            return;
        }
        sync_window(window, cx);
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (), _: &mut Window, _: &mut App) {}
}

fn sync_window(window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let registry = cx.default_global::<Registry>();
    for image in std::mem::take(&mut registry.retired) {
        // Still on a card (or kept): it stays in the atlas.
        if Arc::strong_count(&image) == 1 {
            let _ = window.drop_image(image);
        }
    }
    let Some(entry) = registry.windows.get_mut(&id) else { return };
    entry.states.retain(|s| s.upgrade().is_some());
    if entry.states.is_empty() {
        registry.windows.remove(&id);
        return;
    }
    let frame = entry.frame;
    // An overlay drawn this frame shows in the hit test only next frame: ask for one, once.
    entry.followup = !entry.followup;
    if entry.followup {
        window.request_animation_frame();
    }
    let states: Vec<Entity<ArtifactState>> = entry.states.iter().filter_map(|s| s.upgrade()).collect();
    let overlay = cx.try_global::<crate::workspace::GlobalWorkspace>().is_some_and(|ws| {
        let ws = ws.0.read(cx);
        ws.overlay_open && ws.main_window == Some(window.window_handle())
    });
    let on = enabled(cx);
    let mut views: Vec<(Entity<ArtifactState>, f32)> = Vec::new();
    for state in states {
        let (has_view, distance) = state.update(cx, |s, cx| s.sync(frame, overlay, on, window, cx));
        if has_view {
            views.push((state, distance));
        }
    }
    // Too many views: the ones farthest off screen go.
    if views.len() > MAX_LIVE {
        views.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (state, distance) in views.into_iter().skip(MAX_LIVE) {
            if distance > 0. {
                state.update(cx, |s, cx| s.drop_surface(window, cx));
            }
        }
    }
}

// ---------- making views ----------

fn make_surface(spec: native::Spec, window: &mut Window) -> Result<Box<dyn Surface>, String> {
    #[cfg(test)]
    {
        let _ = window;
        match FAKE.with(|f| f.borrow().clone()) {
            Some(log) => {
                {
                    let mut log = log.borrow_mut();
                    log.made += 1;
                    log.bootstraps.push(spec.bootstrap.clone());
                }
                Ok(Box::new(FakeSurface { log, spec, placed: None }))
            }
            None => Err("no web view in tests".to_string()),
        }
    }
    #[cfg(not(test))]
    {
        native::NativeSurface::new(spec, window).map(|s| Box::new(s) as Box<dyn Surface>)
    }
}

#[cfg(test)]
thread_local! {
    static FAKE: RefCell<Option<Rc<RefCell<FakeLog>>>> = const { RefCell::new(None) };
}

/// What stand-in views were asked to do, in a test.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct FakeLog {
    pub made: usize,
    pub scripts: Vec<String>,
    pub reloads: usize,
    /// The bootstrap each view was made with.
    pub bootstraps: Vec<String>,
}

/// Tests on this thread get stand-in views, which log to what this returns (`None`: no views at
/// all, as without WebView2).
#[cfg(test)]
pub(crate) fn fake_web(on: bool) -> Option<Rc<RefCell<FakeLog>>> {
    let log = on.then(|| Rc::new(RefCell::new(FakeLog::default())));
    FAKE.with(|f| *f.borrow_mut() = log.clone());
    log
}

#[cfg(test)]
struct FakeSurface {
    log: Rc<RefCell<FakeLog>>,
    #[allow(dead_code)]
    spec: native::Spec,
    placed: Option<Placement>,
}

#[cfg(test)]
impl Surface for FakeSurface {
    fn place(&mut self, at: Option<Placement>, _: &Window) {
        self.placed = at;
    }
    fn eval(&self, js: &str) {
        self.log.borrow_mut().scripts.push(js.to_string());
    }
    fn reload(&self) {
        self.log.borrow_mut().reloads += 1;
    }
    fn snapshot(&self) -> Option<async_channel::Receiver<Result<Vec<u8>, String>>> {
        None
    }
    fn release_keys(&self, _: &Window) {}
    fn set_background(&self, _: (u8, u8, u8)) {}
    fn placed(&self) -> Option<Placement> {
        self.placed
    }
}

/// The cards in `window` as the layer last placed them: whether each is live, and where.
#[cfg(any(test, feature = "shots"))]
pub(crate) fn placements(window: &Window, cx: &App) -> Vec<ArtifactReport> {
    let Some(entry) = cx.try_global::<Registry>().and_then(|r| r.windows.get(&window.window_handle().window_id())) else { return Vec::new() };
    entry
        .states
        .iter()
        .filter_map(|s| s.upgrade())
        .map(|s| {
            let s = s.read(cx);
            ArtifactReport {
                title: s.doc.title.clone(),
                expanded: s.mode == Mode::Expanded,
                has_view: s.surface.is_some(),
                live: s.live,
                loaded: s.loaded,
                height: s.height,
                body: s.geom.as_ref().map(|g| g.body),
                mask: s.geom.as_ref().map(|g| g.mask),
                #[cfg(test)]
                placed: s.surface.as_ref().and_then(|v| v.placed()),
                has_snapshot: s.shot.is_some(),
                has_keys: s.has_keys,
            }
        })
        .collect()
}

#[cfg(any(test, feature = "shots"))]
#[derive(Debug, Clone)]
#[cfg_attr(test, allow(dead_code))]
pub(crate) struct ArtifactReport {
    pub title: Option<String>,
    pub expanded: bool,
    pub has_view: bool,
    pub live: bool,
    pub loaded: bool,
    pub height: f32,
    pub body: Option<Bounds<Pixels>>,
    pub mask: Option<Bounds<Pixels>>,
    #[cfg(test)]
    pub placed: Option<Placement>,
    pub has_snapshot: bool,
    /// The page has the keyboard.
    pub has_keys: bool,
}

/// Feed a test's stand-in view the page's message `body` (as its bootstrap would send it), for
/// every card in the window. `{nonce}` in `body` is replaced by each view's secret.
#[cfg(test)]
pub(crate) fn post(window: &mut Window, body: &str, cx: &mut App) {
    let Some(entry) = cx.try_global::<Registry>().and_then(|r| r.windows.get(&window.window_handle().window_id())) else { return };
    let states: Vec<_> = entry.states.iter().filter_map(|s| s.upgrade()).collect();
    for state in states {
        state.update(cx, |s, cx| {
            if s.surface.is_some() {
                let message = body.replace("{nonce}", &s.nonce);
                s.on_event(native::Event::Message(message), window, cx);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::font_stack;

    #[test]
    fn fonts_fall_back_to_the_systems() {
        assert!(font_stack(".SystemUIFont", false).starts_with("-apple-system"));
        assert!(font_stack("Inter", false).starts_with("\"Inter\", -apple-system"));
        assert!(font_stack("Lilex", true).starts_with("\"Lilex\", ui-monospace"));
        assert!(font_stack("x\"; } body { color: red", false).starts_with("-apple-system"), "nothing that breaks out of the stylesheet");
    }
}
