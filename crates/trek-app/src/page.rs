//! Live pages in answers: an agent's `trek-html` block shown as the page it is, in a web view
//! inside the answer (WebView2 on Windows, WKWebView on macOS).
//!
//! The core crate decides what's a page (`trek_core::page`); this module draws its card and keeps
//! its web view. A web view is a native window above GPUI's drawing, so nothing GPUI draws clips
//! it. On Windows it sits in a host window of Trek's own (`winsys::PageHost`), kept each frame to
//! the part of the page inside the clip the transcript draws with, and Windows clips the page to
//! that. On macOS it shows only while the whole page is inside the clip; scrolled partly out, it
//! hides and the card says to scroll back.
//!
//! A view is made when its page comes within a screen of the transcript's view, at most three to
//! a window, and goes when its card stops being drawn (the thread left, the row scrolled out of
//! the list) or is more than three screens away. One that comes back is built again from its
//! source, so a page's own scroll position and state are lost then. While the palette, a menu or
//! a sheet is over the transcript (`Workspace::overlay_open`), views hide.
//!
//! The page is sandboxed as far as a web view allows: no devtools, no IPC, no custom protocol. The
//! view loads the document and nothing else (`trek_core::page::navigation`); a web link opens in
//! the user's browser, and new windows are never opened.

use gpui_kit::component::text::{MarkdownNode, MarkdownParseContext, MarkdownPlugin, markdown_ast};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::RefCell;
use std::hash::{Hash as _, Hasher as _};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use trek_core::page::{Navigation, Page};

use crate::assets::Lucide;

const PLUGIN: &str = "trek-page";
/// The page's height in the card until it's expanded; it scrolls inside that.
const COLLAPSED: f32 = 420.;
/// Expanded, the page is its own height, up to this share of the window.
const MAX_SHARE: f32 = 0.8;
const MIN_EXPANDED: f32 = 160.;
/// Web views a window keeps at once.
const MAX_LIVE: usize = 3;
/// A view is made within this many screens of the transcript's view, and dropped past `FAR`.
const NEAR: f32 = 1.;
const FAR: f32 = 3.;
/// The corners of the page inside its frame.
const RADIUS: f32 = 7.;
/// One sweep of the shimmer over a page still being written (the visualization's pace).
const SHIMMER: Duration = Duration::from_millis(1600);
/// A link the page follows by itself while it loads isn't the user's: none opens the browser
/// until the page has been up this long, and then no more than one per `BETWEEN_OPENS`.
const SETTLE: Duration = Duration::from_millis(1000);
const BETWEEN_OPENS: Duration = Duration::from_millis(2000);
/// How far into a page still being written its `<title>` is looked for.
const TITLE_SCAN: usize = 16 * 1024;

/// A `trek-html` block as parsed.
#[derive(Clone)]
enum Block {
    /// Closed, and a page Trek shows.
    Ready { page: Arc<Page>, ident: u64 },
    /// Its fence hasn't closed: how much is written, and its title once that's in.
    Arriving { bytes: usize, title: Option<String>, source: String },
    /// Closed, but not a page Trek shows: why, and the block as written.
    Invalid { reason: String, source: String },
}

/// Identifies one page across re-renders and re-parses: its document.
fn ident(html: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    html.hash(&mut hasher);
    hasher.finish()
}

/// The title of a page still being written, once its `<title>` (or first heading) has closed.
fn arriving_title(html: &str) -> Option<String> {
    let mut end = html.len().min(TITLE_SCAN);
    while !html.is_char_boundary(end) {
        end -= 1;
    }
    let head = &html[..end];
    let lower = head.to_ascii_lowercase();
    (lower.contains("</title") || lower.contains("</h1")).then(|| trek_core::page::title(head)).filter(|t| t != trek_core::page::UNTITLED)
}

/// The assistant-answer Markdown extension that claims every `trek-html` fence. `live`: the
/// answer is still being written, so a fence still open is a page on its way rather than one
/// never finished. `size`: the answer's text size, for a block shown as code.
#[derive(Clone, Copy)]
pub struct PagePlugin {
    pub live: bool,
    pub size: Pixels,
}

impl MarkdownPlugin for PagePlugin {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        PLUGIN
    }

    // Runs again on each delta while the block is the answer's last: while it's open, a length
    // and a look at its first 16 KiB for a title, so the cost stays flat however long it grows.
    fn parse(&self, node: &markdown_ast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let markdown_ast::Node::Code(code) = node else { return None };
        if code.lang.as_deref() != Some(trek_core::page::LANG) {
            return None;
        }
        let source = cx.node_source(node)?;
        let (block, text) = if trek_core::page::fence_closed(source) {
            match trek_core::page::parse(&code.value) {
                Ok(page) => {
                    let text = format!("Page: {}", page.title);
                    (Block::Ready { ident: ident(&page.html), page: Arc::new(page) }, text)
                }
                Err(error) => (Block::Invalid { reason: error.reason(), source: source.to_string() }, source.to_string()),
            }
        } else {
            let title = arriving_title(&code.value);
            let text = format!("Page{}, being written", title.as_deref().map(|t| format!(": {t}")).unwrap_or_default());
            (Block::Arriving { bytes: code.value.len(), title, source: source.to_string() }, text)
        };
        Some(MarkdownNode::new(PLUGIN, block).text(text.clone()).markdown(source.to_string()).accessibility_label(text))
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let key = node.source_range().map(|range| range.start).unwrap_or(0);
        // Padding, not margin: a margin here collapses through the Markdown block it sits in, and
        // the answer would be measured shorter than it draws.
        let shown = match node.data::<Block>() {
            Some(Block::Ready { page, ident }) => card(page.clone(), *ident, key, window, cx),
            Some(Block::Arriving { bytes, title, .. }) if self.live => building(*bytes, title.as_deref(), key, window, cx),
            Some(Block::Arriving { source, .. }) => invalid("the answer ended before the page was finished", source, key, self.size, cx),
            Some(Block::Invalid { reason, source }) => invalid(reason, source, key, self.size, cx),
            None => return div().into_any_element(),
        };
        div().py_2().child(shown).into_any_element()
    }
}

// ---------- look ----------

/// The colours a card draws with, from the theme: the visualization card's, so the two sit
/// together in an answer.
struct Look {
    dark: bool,
    fg: Hsla,
    muted: Hsla,
    hairline: Hsla,
    inset: Hsla,
    page: Hsla,
}

impl Look {
    fn of(cx: &App) -> Look {
        let theme = cx.theme();
        let dark = theme.mode.is_dark();
        Look {
            dark,
            fg: theme.foreground,
            muted: theme.muted_foreground,
            hairline: theme.foreground.opacity(if dark { 0.10 } else { 0.11 }),
            inset: if dark { theme.background.opacity(0.6) } else { theme.muted.opacity(0.55) },
            page: theme.background,
        }
    }
}

/// The card: a hairline edge, raised a step off the column, and the light a glass pane catches
/// along its top.
fn shell(id: SharedString, look: &Look, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let sheen_top = gpui_kit::white().opacity(if look.dark { 0.07 } else { 0.85 });
    let sheen = gpui_kit::white().opacity(if look.dark { 0.022 } else { 0.0 });
    v_flex()
        .id(id)
        .relative()
        .w_full()
        .rounded(px(12.))
        .border_1()
        .border_color(look.hairline)
        .bg(if look.dark { theme.foreground.opacity(0.022) } else { theme.background })
        .when(!look.dark, |el| {
            el.shadow(vec![BoxShadow { color: gpui_kit::black().opacity(0.04), offset: point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }])
        })
        .overflow_hidden()
        .child(div().absolute().top_0().left_0().right_0().h(px(1.)).bg(sheen_top))
        .child(div().absolute().top_0().left_0().right_0().h(px(64.)).bg(linear_gradient(180., linear_color_stop(sheen, 0.), linear_color_stop(sheen.opacity(0.), 1.))))
}

/// The title row: what it is, its name, and what can be done with it.
fn header(title: AnyElement, actions: AnyElement, look: &Look) -> Div {
    h_flex()
        .w_full()
        .gap(px(10.))
        .pl_4()
        .pr_3()
        .py(px(10.))
        .child(
            div()
                .flex_none()
                .size(px(22.))
                .rounded(px(6.))
                .border_1()
                .border_color(look.hairline)
                .bg(look.inset)
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(Lucide::AppWindow).size(px(12.)).text_color(look.muted)),
        )
        .child(div().flex_1().min_w_0().child(title))
        .child(actions)
}

fn title_text(title: impl Into<SharedString>, look: &Look) -> AnyElement {
    div().line_clamp(1).text_size(px(13.5)).line_height(relative(1.3)).font_semibold().text_color(look.fg).child(title.into()).into_any_element()
}

/// The frame the page sits in, `height` tall.
fn frame(height: f32, look: &Look) -> Div {
    div().relative().w_full().h(px(height)).rounded(px(RADIUS + 1.)).border_1().border_color(look.hairline).bg(look.page).overflow_hidden()
}

/// 0 to 1 and round again, once per `SHIMMER`, while anything asks for it; still without motion.
fn shimmer(motion: bool, window: &mut Window) -> f32 {
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    if !motion {
        return 0.5;
    }
    window.request_animation_frame();
    (START.elapsed().as_secs_f32() % SHIMMER.as_secs_f32()) / SHIMMER.as_secs_f32()
}

/// Motion is allowed: neither Trek's Reduce motion setting nor the system asks for less.
fn motion(cx: &App) -> bool {
    match cx.try_global::<crate::workspace::GlobalWorkspace>() {
        Some(ws) => ws.0.read(cx).motion(cx),
        None => !cx.reduce_motion(),
    }
}

/// A transcript that caches its rows' heights measures them again.
fn relayout(cx: &mut App) {
    let n = cx.try_global::<crate::visualization::Relayout>().map_or(0, |r| r.0);
    cx.set_global(crate::visualization::Relayout(n + 1));
}

/// "12 KB": how much of a page is written.
fn kilobytes(bytes: usize) -> String {
    format!("{} KB", bytes.div_ceil(1024).max(1))
}

// ---------- a page being written ----------

/// A page still being written: its card, its title once that's in, "Building a page… N KB",
/// and a band of light crossing where it will be. No web view yet.
fn building(bytes: usize, title: Option<&str>, key: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let look = Look::of(cx);
    let motion = motion(cx);
    let phase = shimmer(motion, window);
    let label = format!("Building a page… {}", kilobytes(bytes));
    let breath = 0.55 + 0.45 * (phase * std::f32::consts::TAU).cos().abs();
    let title = match title {
        Some(t) => title_text(t.to_string(), &look),
        None => div().h(px(17.)).flex().items_center().child(div().w(relative(0.4)).h(px(10.)).rounded(px(5.)).bg(look.fg.opacity(if look.dark { 0.07 } else { 0.06 }))).into_any_element(),
    };
    let status = h_flex()
        .id(SharedString::from(format!("page-progress-{key}")))
        .test_support()
        .flex_none()
        .h(px(24.))
        .pr_1()
        .text_size(px(11.))
        .text_color(look.muted)
        .opacity(breath)
        .child(label.clone())
        .into_any_element();
    // A soft band of light crossing the frame, in thin slices brightening to its middle (a
    // gradient to transparent draws with hard edges here).
    const SLICES: usize = 16;
    let band = look.fg.opacity(if look.dark { 0.05 } else { 0.055 });
    let sweep = h_flex().absolute().top_0().bottom_0().left(relative(phase * 1.6 - 0.4)).w(relative(0.4)).children((0..SLICES).map(|i| {
        let k = ((i as f32 + 0.5) / SLICES as f32 * std::f32::consts::PI).sin();
        div().flex_1().h_full().bg(band.opacity(k * k))
    }));
    shell(SharedString::from(format!("page-building-{key}")), &look, cx)
        .aria_label(label)
        .test_support()
        .child(header(title, status, &look))
        .child(div().w_full().px(px(12.)).pb(px(12.)).child(frame(COLLAPSED, &look).bg(look.inset).when(motion, |el| el.child(sweep))))
        .into_any_element()
}

/// A block that closed but isn't a page Trek shows (or never closed): a short note saying why,
/// over the block as written, as ordinary code.
fn invalid(reason: &str, source: &str, key: usize, size: Pixels, cx: &App) -> AnyElement {
    let look = Look::of(cx);
    let note = format!("Couldn't show this page: {reason}");
    v_flex()
        .id(SharedString::from(format!("page-invalid-{key}")))
        .aria_label(note.clone())
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
                .child(div().flex_1().min_w_0().text_color(look.muted).child(note)),
        )
        .child(crate::md::keyed(SharedString::from(format!("page-raw-{key}")), source.to_string(), None, None, size, false, false, cx))
        .into_any_element()
}

// ---------- the card ----------

fn card(page: Arc<Page>, ident: u64, key: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let state = window.use_keyed_state(SharedString::from(format!("trek-page-{key}")), cx, |_, cx| PageState::new(cx));
    // The same place in an answer can come to hold another page (an edit, a retry): its view
    // is the old page's, so it goes.
    state.update(cx, |s, _| s.show(page.clone(), ident));
    let look = Look::of(cx);
    let (expanded, natural, unavailable, live, shown, covered) = {
        let s = state.read(cx);
        (s.expanded, s.natural, s.unavailable, s.live.is_some(), s.shown, s.covered)
    };
    let tall = f32::from(window.viewport_size().height) * MAX_SHARE;
    let height = match (expanded, natural) {
        (false, _) => COLLAPSED,
        (true, Some(h)) => h.clamp(MIN_EXPANDED, tall.max(MIN_EXPANDED)),
        // No view to ask (or no answer yet): as tall as it may be.
        (true, None) if unavailable => tall.max(COLLAPSED),
        (true, None) => COLLAPSED,
    };
    let group = SharedString::from(format!("trek-page-group-{key}"));
    let actions = h_flex()
        .flex_none()
        .gap(px(2.))
        .opacity(0.72)
        .group_hover(group.clone(), |s| s.opacity(1.))
        .child(button(format!("page-copy-{key}"), Icon::new(Lucide::Copy), "Copy the page's source", &look, {
            let html = page.html.clone();
            move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(html.clone()));
                crate::toast::push(window, "Page source copied", cx);
            }
        }))
        .child(button(format!("page-open-{key}"), Icon::new(IconName::Globe), "Open in the Browser", &look, {
            let page = page.clone();
            move |_, window, cx| open_in_browser(&page, ident, window, cx)
        }))
        .child(button(format!("page-expand-{key}"), Icon::new(if expanded { Lucide::ChevronsDownUp } else { Lucide::ChevronsUpDown }), if expanded { "Collapse" } else { "Expand" }, &look, {
            let state = state.clone();
            move |_, _, cx| {
                state.update(cx, |s, cx| s.toggle_expanded(cx));
                relayout(cx);
            }
        }))
        .into_any_element();
    // What the frame shows when the page isn't over it: why there's none, or (on macOS, where a
    // page partly scrolled out hides) where it went. Blank while its view is made.
    let hint = if unavailable {
        Some("Couldn't open a web view for this page")
    } else if live && !shown && !covered {
        Some("Scroll to show the page")
    } else {
        None
    };
    let surface = Surface { state: state.clone(), theme: theme_script(cx), dark: look.dark, background: look.page };
    shell(SharedString::from(format!("page-card-{key}")), &look, cx)
        .aria_label(format!("Page: {}", page.title))
        .test_support()
        .group(group)
        .child(header(title_text(page.title.clone(), &look), actions, &look))
        .child(
            div().w_full().px(px(12.)).pb(px(12.)).child(
                frame(height, &look)
                    .child(div().size_full().child(surface))
                    .when_some(hint, |el, hint| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("page-hint-{key}")))
                                .test_support()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(12.))
                                .text_color(look.muted)
                                .child(hint),
                        )
                    }),
            ),
        )
        .into_any_element()
}

fn button(id: String, icon: Icon, tip: &'static str, look: &Look, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> AnyElement {
    let hover = look.fg.opacity(0.06);
    h_flex()
        .id(SharedString::from(id))
        .test_support()
        .h(px(24.))
        .min_w(px(24.))
        .px(px(5.))
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|s| s.bg(hover))
        .child(icon.size(px(13.)).text_color(look.muted))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx))
        .on_click(on_click)
        .into_any_element()
}

/// The Browser panel can't be handed a document, only an address: the page goes to a file in
/// Trek's data folder (`pages/<its hash>.html`, the same file for the same page) and the panel
/// opens that, in a tab of its own. A window with no Browser panel (a thread's own window) hands
/// the file to the system's browser instead.
fn open_in_browser(page: &Page, ident: u64, window: &mut Window, cx: &mut App) {
    let dir = trek_core::paths::data_dir().join("pages");
    let path = dir.join(format!("{ident:016x}.html"));
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &page.html)) {
        crate::toast::push(window, format!("Couldn't save the page to open it: {e}"), cx);
        return;
    }
    let url = file_url(&path);
    let root = window.root::<gpui_kit::component::Root>().flatten();
    let trek = root.and_then(|r| r.read(cx).view().clone().downcast::<crate::root::TrekWindow>().ok());
    match trek.map(|t| t.read(cx).right_panel.clone()) {
        Some(panel) => panel.update(cx, |p, cx| {
            p.open_tool(crate::workspace::PanelTool::Browser, window, cx);
            if let Some(browser) = p.browser() {
                browser.update(cx, |b, cx| b.open(&url, window, cx));
            }
        }),
        None => cx.open_url(&url),
    }
}

/// `path` as a `file:` URL: forward slashes, a drive letter after `file:///`, and anything but
/// the plain characters of a path percent-encoded.
fn file_url(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from(if path.starts_with('/') { "file://" } else { "file:///" });
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Trek's theme for the page: `data-theme` and the `--trek-*` colours on its root, and its
/// `color-scheme`. Run as the document starts (its root may not be there yet: then as soon as it
/// is, before the page's own scripts), and again when the theme changes.
fn theme_script(cx: &App) -> String {
    let theme = cx.theme();
    let dark = theme.mode.is_dark();
    let colours = [
        ("--trek-bg", theme.background),
        ("--trek-fg", theme.foreground),
        ("--trek-muted", theme.muted_foreground),
        ("--trek-accent", crate::palette::ember(cx)),
        ("--trek-border", theme.border),
    ];
    let set: String = colours.iter().map(|(name, colour)| format!("s.setProperty('{name}','{}');", css_color(*colour))).collect();
    let (name, scheme) = if dark { ("night", "dark") } else { ("paper", "light") };
    format!(
        "(()=>{{const apply=()=>{{const r=document.documentElement;if(!r)return false;r.dataset.theme='{name}';const s=r.style;{set}s.colorScheme='{scheme}';return true;}};if(!apply())new MutationObserver((_,o)=>{{if(apply())o.disconnect();}}).observe(document,{{childList:true}});}})();"
    )
}

fn css_color(c: Hsla) -> String {
    let c = c.to_rgb();
    format!("rgba({:.0},{:.0},{:.0},{:.3})", c.r * 255., c.g * 255., c.b * 255., c.a)
}

// ---------- the web view ----------

/// One page's card as the window keeps it while it's drawn: its view, and what's been done to it.
pub(crate) struct PageState {
    page: Option<(u64, Arc<Page>)>,
    live: Option<Live>,
    /// No view could be made for it (no native window, as in tests; no WebView2): the card says so.
    unavailable: bool,
    /// A view is being made for it (`Surface::build`).
    building: bool,
    expanded: bool,
    /// The page's own height, asked of it on expanding.
    natural: Option<f32>,
    /// The page is on screen (as of the last frame).
    shown: bool,
    /// Something is over the transcript.
    covered: bool,
    /// How far the page was from the transcript's view, in screens, as last laid out.
    distance: f32,
    opens: async_channel::Sender<String>,
    heights: async_channel::Sender<f32>,
    _tasks: Vec<Task<()>>,
    _overlay: Option<Subscription>,
}

impl PageState {
    fn new(cx: &mut Context<Self>) -> Self {
        // Web links the page follows open in the browser, from the GPUI thread.
        let (opens, opened) = async_channel::unbounded::<String>();
        let open = cx.spawn(async move |_, cx| {
            while let Ok(url) = opened.recv().await {
                cx.update(|cx| cx.open_url(&url));
            }
        });
        let (heights, measured) = async_channel::unbounded::<f32>();
        let measure = cx.spawn(async move |this, cx| {
            while let Ok(height) = measured.recv().await {
                if this.update(cx, |s, cx| {
                    s.natural = Some(height);
                    cx.notify();
                }).is_err() {
                    break;
                }
                cx.update(relayout);
            }
        });
        let overlay = cx.try_global::<crate::workspace::GlobalWorkspace>().map(|g| g.0.clone()).map(|ws| {
            cx.observe(&ws, |this: &mut Self, ws, cx| {
                let covered = ws.read(cx).overlay_open;
                if covered != this.covered {
                    this.covered = covered;
                    if covered && let Some(live) = &mut this.live {
                        live.hide(cx);
                    }
                    cx.notify();
                }
            })
        });
        let covered = cx.try_global::<crate::workspace::GlobalWorkspace>().is_some_and(|g| g.0.read(cx).overlay_open);
        PageState {
            page: None,
            live: None,
            unavailable: false,
            building: false,
            expanded: false,
            natural: None,
            shown: false,
            covered,
            distance: f32::INFINITY,
            opens,
            heights,
            _tasks: vec![open, measure],
            _overlay: overlay,
        }
    }

    /// This card shows `page`: a view of another one goes.
    fn show(&mut self, page: Arc<Page>, ident: u64) {
        if self.page.as_ref().is_none_or(|(i, _)| *i != ident) {
            self.page = Some((ident, page));
            self.live = None;
            self.natural = None;
        }
    }

    fn toggle_expanded(&mut self, cx: &mut Context<Self>) {
        self.expanded = !self.expanded;
        if self.expanded
            && let Some(view) = self.live.as_ref().and_then(|l| l.view.as_ref())
        {
            let heights = self.heights.clone();
            let asked = view.evaluate_script_with_callback("Math.ceil(document.documentElement.scrollHeight)", move |answer| {
                if let Ok(height) = answer.trim().trim_matches('"').parse::<f32>() {
                    let _ = heights.try_send(height);
                }
            });
            if asked.is_err() {
                self.natural = None;
            }
        }
        cx.notify();
    }

    fn drop_view(&mut self) {
        self.live = None;
        self.shown = false;
    }

    /// The page has the keys: give them back to Trek.
    fn return_focus(&self, cx: &App) {
        if let Some(live) = &self.live {
            live.return_focus(cx);
        }
    }
}

// Every window's live views, to keep each window to `MAX_LIVE`.
thread_local! {
    static LIVE: RefCell<Vec<(WindowId, WeakEntity<PageState>)>> = const { RefCell::new(Vec::new()) };
}

/// Make room for one more view in `window` for `me`, `distance` screens away: past `MAX_LIVE`,
/// the one farthest from the screen goes, if it's farther than `me`. Whether there's room.
fn make_room(window: WindowId, me: &Entity<PageState>, distance: f32, cx: &mut App) -> bool {
    let others: Vec<Entity<PageState>> = LIVE.with(|live| {
        let mut live = live.borrow_mut();
        live.retain(|(_, state)| state.upgrade().is_some());
        live.iter().filter(|(w, _)| *w == window).filter_map(|(_, s)| s.upgrade()).filter(|s| s != me).collect()
    });
    let others: Vec<(Entity<PageState>, f32)> = others.into_iter().filter(|s| s.read(cx).live.is_some()).map(|s| {
        let d = s.read(cx).distance;
        (s, d)
    }).collect();
    if others.len() < MAX_LIVE {
        return true;
    }
    match others.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
        Some((farthest, d)) if d > distance => {
            farthest.update(cx, |s, cx| {
                s.drop_view();
                cx.notify();
            });
            true
        }
        _ => false,
    }
}

/// How far `body` is from `clip`, in screens (the clip's heights): 0 when they overlap.
fn distance(body: Bounds<Pixels>, clip: Bounds<Pixels>) -> f32 {
    let screen = f32::from(clip.size.height).max(1.);
    let above = f32::from(clip.top() - body.bottom());
    let below = f32::from(body.top() - clip.bottom());
    above.max(below).max(0.) / screen
}

/// Where a page's view goes in a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placement {
    /// The part of the page inside the clip, in device pixels in the window: the host's place.
    visible: Option<[i32; 4]>,
    /// The whole page in device pixels, from `visible`'s top left (the view's place in the host).
    page: [i32; 4],
    /// The whole page is inside the clip.
    whole: bool,
}

/// Where the page laid out at `body` goes, drawn clipped to `clip`, at `scale` device pixels to
/// the point. Both rectangles are rounded edge by edge, so an edge the clip doesn't cut is the
/// same pixel in both.
fn placement(body: Bounds<Pixels>, clip: Bounds<Pixels>, scale: f32) -> Placement {
    let device = |b: Bounds<Pixels>| {
        let (x0, y0) = ((f32::from(b.left()) * scale).round() as i32, (f32::from(b.top()) * scale).round() as i32);
        let (x1, y1) = ((f32::from(b.right()) * scale).round() as i32, (f32::from(b.bottom()) * scale).round() as i32);
        [x0, y0, x1 - x0, y1 - y0]
    };
    let page = device(body);
    let cut = body.intersect(&clip);
    let visible = Some(device(cut)).filter(|[_, _, w, h]| *w > 0 && *h > 0);
    let whole = visible == Some(page);
    let from = visible.map_or([page[0], page[1]], |v| [v[0], v[1]]);
    Placement { visible, page: [page[0] - from[0], page[1] - from[1], page[2], page[3]], whole }
}

/// The links a page follows by itself (`SETTLE`, `BETWEEN_OPENS`): whether one at `now` may open
/// the browser, for a page up since `since` that last opened one at `last`.
fn may_open(since: Instant, last: Option<Instant>, now: Instant) -> bool {
    now.duration_since(since) >= SETTLE && last.is_none_or(|l| now.duration_since(l) >= BETWEEN_OPENS)
}

/// A page's web view, and on Windows the host window it sits in. Dropped, it hides and gives
/// the keys back, then the view goes, then its host.
struct Live {
    view: Option<wry::WebView>,
    #[cfg(windows)]
    host: crate::winsys::PageHost,
    /// What was last done with it, so an unchanged frame does nothing.
    placed: Option<(Placement, bool)>,
    /// The page's size as the view was last given it.
    sized: Option<[i32; 2]>,
    theme: String,
    radius: i32,
}

/// A web view of `html` in `parent`, hidden until placed: Trek's theme set as its document starts,
/// kept on that document (`trek_core::page::navigation`), web links it follows sent to `opens`
/// for the browser, no new windows, no devtools.
fn make_view<W: raw_window_handle::HasWindowHandle>(parent: &W, html: String, theme: &str, dark: bool, background: Hsla, opens: async_channel::Sender<String>) -> wry::Result<wry::WebView> {
    let since = Instant::now();
    let last: Arc<Mutex<Option<Instant>>> = Arc::default();
    let open = Arc::new(move |url: String| {
        let now = Instant::now();
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        if may_open(since, *last, now) {
            *last = Some(now);
            let _ = opens.try_send(url);
        }
    });
    let (on_navigate, on_window) = (open.clone(), open);
    let document: RefCell<Option<String>> = RefCell::new(None);
    #[cfg(windows)]
    let mut context = wry::WebContext::new(Some(crate::panels::browser::webview_data_dir()));
    // Made without the keys (WebView2 focuses a new view by default): the page gets them when
    // it's clicked. Browser shortcuts (print, find, reload) stay Trek's keys.
    #[cfg(windows)]
    let builder = {
        use wry::WebViewBuilderExtWindows as _;
        let rgb = background.to_rgb();
        wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_focused(false)
            .with_browser_accelerator_keys(false)
            .with_theme(if dark { wry::Theme::Dark } else { wry::Theme::Light })
            .with_background_color(((rgb.r * 255.).round() as u8, (rgb.g * 255.).round() as u8, (rgb.b * 255.).round() as u8, 255))
    };
    #[cfg(not(windows))]
    let builder = {
        let _ = (dark, background);
        wry::WebViewBuilder::new()
    };
    builder
        .with_html(html)
        .with_visible(false)
        .with_devtools(false)
        .with_accept_first_mouse(true)
        .with_bounds(wry::Rect { position: wry::dpi::PhysicalPosition::new(0, 0).into(), size: wry::dpi::PhysicalSize::new(1u32, 1u32).into() })
        .with_initialization_script_for_main_only(theme, true)
        .with_navigation_handler(move |uri| {
            let mut document = document.borrow_mut();
            match trek_core::page::navigation(document.as_deref(), &uri) {
                Navigation::Allow => {
                    document.get_or_insert(uri);
                    true
                }
                Navigation::Deny => false,
                Navigation::Open(url) => {
                    on_navigate(url);
                    false
                }
            }
        })
        .with_new_window_req_handler(move |url, _| {
            if trek_core::page::is_web(&url) {
                on_window(url);
            }
            wry::NewWindowResponse::Deny
        })
        .build_as_child(parent)
}

/// A window by its raw handle, as a parent for wry (macOS: the window's content view).
#[cfg_attr(windows, allow(dead_code))]
struct Parent(raw_window_handle::RawWindowHandle);

impl raw_window_handle::HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: taken from a window still open (`Surface::build` checks before building), on
        // the thread that owns it, and only borrowed while the view is made.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.0) })
    }
}

impl Live {
    /// A view of `page` in the window `parent`, hidden until placed; `None` when no view can be
    /// made. WebView2 handles the window's messages while it makes one: not to be called in an
    /// update.
    fn build(page: &Page, parent: raw_window_handle::RawWindowHandle, theme: &str, dark: bool, background: Hsla, opens: async_channel::Sender<String>) -> Option<Live> {
        let html = page.html.clone();
        #[cfg(windows)]
        let host = match parent {
            raw_window_handle::RawWindowHandle::Win32(handle) => crate::winsys::PageHost::new(handle.hwnd.get())?,
            _ => return None,
        };
        #[cfg(windows)]
        let view = make_view(&host, html, theme, dark, background, opens);
        #[cfg(not(windows))]
        let view = make_view(&Parent(parent), html, theme, dark, background, opens);
        let view = view.inspect_err(|e| tracing::warn!("page: no web view: {e}")).ok()?;
        Some(Live {
            view: Some(view),
            #[cfg(windows)]
            host,
            placed: None,
            sized: None,
            theme: theme.to_string(),
            radius: 0,
        })
    }

    /// Put the view where `p` says, shown or not; on Windows only the part inside the clip is
    /// ever shown, on macOS only a page wholly inside it.
    fn place(&mut self, p: Placement, show: bool, scale: f32, cx: &App) {
        if self.placed == Some((p, show)) {
            return;
        }
        self.placed = Some((p, show));
        self.radius = (RADIUS * scale).round() as i32;
        let Some(view) = &self.view else { return };
        let [dx, dy, w, h] = p.page;
        let size = [w.max(1), h.max(1)];
        if cfg!(windows) {
            let Some(visible) = p.visible.filter(|_| show) else {
                self.hide(cx);
                return;
            };
            if self.sized != Some(size) {
                self.sized = Some(size);
                let _ = view.set_bounds(wry::Rect { position: wry::dpi::PhysicalPosition::new(dx, dy).into(), size: wry::dpi::PhysicalSize::new(size[0] as u32, size[1] as u32).into() });
                let _ = view.set_visible(true);
            }
            self.show_in_host(visible, [dx, dy, size[0], size[1]]);
        } else if show && p.whole {
            // Wholly inside the clip, the page is the visible part.
            let at = p.visible.unwrap_or(p.page);
            let _ = view.set_bounds(wry::Rect { position: wry::dpi::PhysicalPosition::new(at[0], at[1]).into(), size: wry::dpi::PhysicalSize::new(size[0] as u32, size[1] as u32).into() });
            let _ = view.set_visible(true);
        } else {
            self.hide(cx);
        }
    }

    /// Show the host at `visible`, the view at `page` inside it.
    #[cfg(windows)]
    fn show_in_host(&self, visible: [i32; 4], page: [i32; 4]) {
        self.host.place_page(page);
        self.host.place(Some(visible), Some((page, self.radius)));
    }

    #[cfg(not(windows))]
    fn show_in_host(&self, _: [i32; 4], _: [i32; 4]) {}

    /// Off the screen, and the keys back to Trek if the page had them: no one can see it.
    fn hide(&mut self, cx: &App) {
        self.return_focus(cx);
        #[cfg(windows)]
        self.host.place(None, None);
        #[cfg(not(windows))]
        if let Some(view) = &self.view {
            let _ = view.set_visible(false);
        }
        if let Some((p, _)) = self.placed {
            self.placed = Some((p, false));
        }
    }

    /// The page has the keys: give them back to Trek's window. On Windows once the update under
    /// way is over, as the window hears of it at once.
    fn return_focus(&self, cx: &App) {
        #[cfg(windows)]
        if let Some(owner) = self.host.focus_owner() {
            crate::system::after_update(cx, move |_| crate::winsys::set_focus(owner));
        }
        #[cfg(not(windows))]
        {
            let _ = cx;
            if let Some(view) = &self.view {
                let _ = view.focus_parent();
            }
        }
    }

    /// Trek's theme changed: tell the page.
    fn retheme(&mut self, theme: &str, dark: bool) {
        if self.theme == theme {
            return;
        }
        self.theme = theme.to_string();
        let Some(view) = &self.view else { return };
        let _ = view.evaluate_script(theme);
        #[cfg(windows)]
        {
            use wry::WebViewExtWindows as _;
            let _ = view.set_theme(if dark { wry::Theme::Dark } else { wry::Theme::Light });
        }
        #[cfg(not(windows))]
        let _ = dark;
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // Off the screen first, and the keys back to Trek if the page had them: now, as a window
        // destroyed with them leaves them with no one (and dropping can't wait for an update to end).
        #[cfg(windows)]
        {
            self.host.return_focus();
            self.host.place(None, None);
        }
        #[cfg(not(windows))]
        if let Some(view) = &self.view {
            let _ = view.focus_parent();
            let _ = view.set_visible(false);
        }
        // The view before its host: it's a child of the host's window.
        drop(self.view.take());
    }
}

/// Where a page's view goes: laid out as the frame's inside, and each frame placing the view
/// there (and only where the transcript's clip lets it be seen).
struct Surface {
    state: Entity<PageState>,
    theme: String,
    dark: bool,
    background: Hsla,
}

impl Surface {
    /// Make the page's view in the window `id`, once the frame being drawn is done: WebView2
    /// handles the window's messages while it makes one, which GPUI can't take mid-frame. The card
    /// may be gone by then, or the window closed: then nothing is made.
    fn build(&self, id: WindowId, window: &Window, cx: &mut App) {
        let raw = raw_window_handle::HasWindowHandle::window_handle(window).ok().map(|h| h.as_raw());
        let Some((ident, page)) = self.state.read(cx).page.clone() else { return };
        let opens = self.state.read(cx).opens.clone();
        self.state.update(cx, |s, _| s.building = true);
        LIVE.with(|live| {
            let mut live = live.borrow_mut();
            if !live.iter().any(|(_, s)| s.entity_id() == self.state.entity_id()) {
                live.push((id, self.state.downgrade()));
            }
        });
        let state = self.state.downgrade();
        let (theme, dark, background) = (self.theme.clone(), self.dark, self.background);
        crate::system::after_update(cx, move |cx| {
            let wanted = cx.update(|cx| {
                let open = cx.windows().iter().any(|w| w.window_id() == id);
                open && state.upgrade().is_some_and(|s| {
                    let s = s.read(cx);
                    s.building && s.live.is_none() && s.page.as_ref().is_some_and(|(i, _)| *i == ident)
                })
            });
            let built = if wanted { raw.and_then(|raw| Live::build(&page, raw, &theme, dark, background, opens)) } else { None };
            let _ = state.update(cx, |s, cx| {
                s.building = false;
                if wanted {
                    s.unavailable = built.is_none();
                    s.live = built;
                }
                cx.notify();
            });
        });
    }
}

impl IntoElement for Surface {
    type Element = Surface;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Surface {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let style = Style { size: Size::full(), ..Default::default() };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) {
        let clip = window.content_mask().bounds;
        let screens = distance(bounds, clip);
        let id = Window::window_handle(window).window_id();
        let scale = window.scale_factor();
        let (live, wanted) = {
            let s = self.state.read(cx);
            (s.live.is_some(), s.live.is_none() && !s.building && !s.unavailable && !s.covered && screens <= NEAR)
        };
        self.state.update(cx, |s, _| s.distance = screens);
        if live && screens > FAR {
            self.state.update(cx, |s, cx| {
                s.drop_view();
                cx.notify();
            });
        }
        if wanted && make_room(id, &self.state, screens, cx) {
            self.build(id, window, cx);
        }
        let p = placement(bounds, clip, scale);
        let (theme, dark) = (self.theme.clone(), self.dark);
        self.state.update(cx, |s, cx| {
            let covered = s.covered;
            let Some(live) = &mut s.live else { return };
            live.retheme(&theme, dark);
            live.place(p, !covered, scale, cx);
            let shown = !covered && if cfg!(windows) { p.visible.is_some() } else { p.whole };
            if shown != s.shown {
                s.shown = shown;
                cx.notify();
            }
        });
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, _: &mut App) {
        // A press GPUI sees is outside the page (the page's own go to its window): the keys come
        // back to Trek. Windows moves them only between top-level windows by itself.
        let state = self.state.clone();
        window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture {
                state.read(cx).return_focus(cx);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{BETWEEN_OPENS, SETTLE, arriving_title, distance, file_url, kilobytes, may_open, placement};
    use gpui_kit::{Bounds, point, px, size};
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<gpui_kit::Pixels> {
        Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
    }

    #[test]
    fn a_page_cut_by_the_clip_shows_only_the_part_inside_it_with_the_page_kept_in_place() {
        let clip = rect(0., 100., 800., 600.);
        // Wholly inside: the host is the page, the view at its top left.
        let p = placement(rect(40., 200., 600., 420.), clip, 1.);
        assert_eq!((p.visible, p.page, p.whole), (Some([40, 200, 600, 420]), [0, 0, 600, 420], true));
        // Half above the clip at 150 %: the host starts at the clip, the page above it by as much
        // as is cut off, every edge the clip doesn't cut where it was.
        let p = placement(rect(40., -110., 600., 420.), clip, 1.5);
        assert_eq!(p.visible, Some([60, 150, 900, 315]));
        assert_eq!(p.page, [0, -315, 900, 630]);
        assert!(!p.whole);
        // Below the bottom edge: nothing to show.
        let p = placement(rect(40., 700., 600., 420.), clip, 1.);
        assert_eq!(p.visible, None);
        // A fractional position: the two rectangles' shared edges round to the same pixel.
        let p = placement(rect(10.3, 650.6, 300., 100.), clip, 1.25);
        let v = p.visible.unwrap();
        assert_eq!(v[0] + p.page[0], (10.3f32 * 1.25).round() as i32);
        assert_eq!(v[1] + v[3], (700f32 * 1.25).round() as i32, "cut at the clip's bottom");
    }

    #[test]
    fn distance_is_counted_in_screens() {
        let clip = rect(0., 0., 800., 500.);
        assert_eq!(distance(rect(0., 400., 800., 420.), clip), 0.);
        assert_eq!(distance(rect(0., 750., 800., 420.), clip), 0.5);
        assert_eq!(distance(rect(0., -1920., 800., 420.), clip), 3.);
    }

    #[test]
    fn a_page_opens_links_only_once_it_has_settled_and_not_in_a_burst() {
        let up = Instant::now();
        assert!(!may_open(up, None, up + Duration::from_millis(200)), "while it loads: not the user's");
        assert!(may_open(up, None, up + SETTLE));
        let opened = up + SETTLE;
        assert!(!may_open(up, Some(opened), opened + Duration::from_millis(500)));
        assert!(may_open(up, Some(opened), opened + BETWEEN_OPENS));
    }

    #[test]
    fn the_browser_gets_the_page_as_a_file_url() {
        assert_eq!(file_url(Path::new(r"C:\Users\A B\Trek\pages\00ff.html")), "file:///C:/Users/A%20B/Trek/pages/00ff.html");
        assert_eq!(file_url(Path::new("/Users/me/Library/Application Support/Trek/pages/x#1.html")), "file:///Users/me/Library/Application%20Support/Trek/pages/x%231.html");
    }

    /// A real WebView2 in a real window (off screen, never activated) showing `mock:page` the way
    /// a card does: in a host cut to part of it. What the headless harness can't reach: the page
    /// runs (tabs, the cube, the sort), Trek's theme is on it before its scripts, the view sits at
    /// the page's place inside the cut host, it never takes the keys, and it stays on its document
    /// while web links go to the browser. Run by hand: `cargo test -p trek-app a_real_page -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "creates a real WebView2 window, which needs a desktop session and the WebView2 runtime (CI runs without one)"]
    fn a_real_page_runs_in_a_cut_host_and_stays_on_its_document() {
        use crate::winsys::PageHost;
        use windows_sys::Win32::Foundation::{HWND, RECT};
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        // The WebView2 profile goes in the tests' throwaway data folder (the harness's), never the user's.
        trek_core::paths::isolate(std::env::temp_dir().join(format!("trek-ui-tests-{}", std::process::id())));
        assert!(crate::panels::browser::webview_data_dir().starts_with(std::env::temp_dir()));
        let pump = || unsafe {
            let mut msg = std::mem::zeroed::<MSG>();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        };
        let settle = |ms: u64| {
            let until = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < until {
                pump();
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let parent = unsafe { CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), std::ptr::null(), WS_POPUP | WS_CLIPCHILDREN, -32000, -32000, 900, 700, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) };
        assert!(!parent.is_null());
        unsafe { ShowWindow(parent, SW_SHOWNOACTIVATE) };
        let host = PageHost::new(parent as isize).expect("a host");
        let page = trek_agents::mock::PAGE;
        let html = page.split_once("```trek-html\n").unwrap().1.split_once("\n```").unwrap().0.to_string();
        let theme = "(()=>{const apply=()=>{const r=document.documentElement;if(!r)return false;r.dataset.theme='night';r.style.setProperty('--trek-accent','rgba(232,96,60,1.000)');r.style.colorScheme='dark';return true;};if(!apply())new MutationObserver((_,o)=>{if(apply())o.disconnect();}).observe(document,{childList:true});})();";
        let (opens, opened) = async_channel::unbounded();
        let view = super::make_view(&host, html, theme, true, gpui_kit::black(), opens).expect("a WebView2 (is the runtime installed?)");
        // Half the page cut off at the top: the host is the 220 px left, the page 200 px above it.
        let (page_at, host_at) = ([0, -200, 600, 420], [10, 50, 600, 220]);
        let _ = view.set_bounds(wry::Rect { position: wry::dpi::PhysicalPosition::new(0, -200).into(), size: wry::dpi::PhysicalSize::new(600u32, 420u32).into() });
        let _ = view.set_visible(true);
        host.place_page(page_at);
        host.place(Some(host_at), Some((page_at, 7)));
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let eval = |js: &str| -> String {
            let tx = tx.clone();
            view.evaluate_script_with_callback(js, move |r| {
                let _ = tx.send(r);
            })
            .expect("evaluate");
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                pump();
                if let Ok(r) = rx.try_recv() {
                    return r.trim_matches('"').to_string();
                }
                assert!(Instant::now() < deadline, "no answer to {js}");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        while eval("document.readyState") != "complete" {
            assert!(Instant::now() < deadline, "the page never loaded");
            settle(50);
        }
        // Where Windows has them: the view at the page's place inside the cut host.
        let rect = |hwnd: HWND| {
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe { GetWindowRect(hwnd, &mut r) };
            [r.left, r.top, r.right - r.left, r.bottom - r.top]
        };
        let (h, v) = (rect(host.hwnd() as HWND), rect(unsafe { GetWindow(host.hwnd() as HWND, GW_CHILD) }));
        assert_eq!([h[2], h[3]], [600, 220]);
        assert_eq!([v[0] - h[0], v[1] - h[1], v[2], v[3]], page_at);

        // The page's `--accent` is `var(--trek-accent, …)`: Trek's, set before the page's styles ran.
        assert_eq!(eval("document.documentElement.dataset.theme + '|' + getComputedStyle(document.documentElement).getPropertyValue('--accent').trim()"), "night|rgba(232,96,60,1.000)");
        assert_eq!(eval("document.querySelectorAll('[role=tab]')[1].click(); document.getElementById('clamps').hidden + '|' + document.getElementById('model').hidden"), "false|true");
        assert_eq!(eval("document.querySelectorAll('#table th')[1].click(); [...document.querySelectorAll('#table tbody tr')].map(r => r.cells[1].textContent).join(',')"), "45,50,60,80");
        assert_eq!(eval("document.querySelectorAll('#table th')[1].click(); [...document.querySelectorAll('#table tbody tr')].map(r => r.cells[1].textContent).join(',')"), "80,60,50,45");
        eval("document.querySelectorAll('[role=tab]')[0].click(); 0");
        let turned = eval(
            "(() => { const c = document.getElementById('cube'), r = c.getBoundingClientRect(), x = r.left + 50, y = r.top + 50, before = yaw; \
             c.dispatchEvent(new PointerEvent('pointerdown', { clientX: x, clientY: y, pointerId: 1, bubbles: true })); \
             c.dispatchEvent(new PointerEvent('pointermove', { clientX: x + 80, clientY: y, pointerId: 1, bubbles: true })); \
             c.dispatchEvent(new PointerEvent('pointerup', { clientX: x + 80, clientY: y, pointerId: 1, bubbles: true })); \
             return (yaw - before).toFixed(2); })()",
        );
        assert_eq!(turned, "0.80", "dragging turns the cube");
        assert!(!host.has_focus(), "the page never took the keys");

        // Away from its document: a file goes nowhere; a web link, once the page has settled, goes
        // to the browser; the page stays where it was either way.
        eval("location.href = 'file:///C:/Windows/win.ini'; 0");
        settle(600);
        assert_eq!(eval("String(!!document.getElementById('cube'))"), "true");
        assert!(opened.try_recv().is_err());
        settle(SETTLE.as_millis() as u64);
        eval("location.href = 'https://example.com/'; 0");
        settle(600);
        assert_eq!(opened.try_recv().ok().as_deref(), Some("https://example.com/"));
        assert_eq!(eval("String(!!document.getElementById('cube'))"), "true");
        // A new window is never opened; its web address goes to the browser too, but not twice in
        // a burst.
        eval("window.open('https://example.org/'); 0");
        settle(300);
        assert!(opened.try_recv().is_err(), "a second link in a burst");
        settle(BETWEEN_OPENS.as_millis() as u64);
        eval("window.open('https://example.org/'); 0");
        settle(600);
        assert_eq!(opened.try_recv().ok().as_deref(), Some("https://example.org/"));

        // Scrolled out: the host hides; dropped: the view, then the host, go.
        host.place(None, None);
        assert_eq!(unsafe { IsWindowVisible(host.hwnd() as HWND) }, 0);
        let hwnd = host.hwnd() as HWND;
        drop(view);
        pump();
        drop(host);
        assert_eq!(unsafe { IsWindow(hwnd) }, 0);
        unsafe { DestroyWindow(parent) };
        pump();
    }

    #[test]
    fn a_page_being_written_is_counted_and_named_once_its_title_is_in() {
        assert_eq!(kilobytes(0), "1 KB");
        assert_eq!(kilobytes(1025), "2 KB");
        assert_eq!(arriving_title("<!doctype html><title>Desk fi"), None);
        assert_eq!(arriving_title("<!doctype html><title>Desk fit</title><p>"), Some("Desk fit".into()));
        assert_eq!(arriving_title("<!doctype html><body><h1>Plan</h1>"), Some("Plan".into()));
        // Only its start is looked at.
        assert_eq!(arriving_title(&format!("<!doctype html>{}<title>Late</title>", " ".repeat(20_000))), None);
    }
}
