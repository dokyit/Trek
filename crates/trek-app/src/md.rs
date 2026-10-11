//! How agent answers render: Trek's markdown style and file-path chips — `src/main.rs` becomes a
//! small bordered chip with a file icon that opens the file, like T3 Code and Codex do.
//!
//! The style reads like T3 Code's: soft grey body text with bold in the theme's full foreground,
//! running text across the transcript's whole column (`column`), and sizes, gaps and list
//! spacing that all follow the transcript's text size (`Metrics`). Inline code stays quiet, and
//! folders in path chips take the project's colour.

use gpui_kit::base::Easing;
use gpui_kit::base::text::{TextView, TextViewMotion, TextViewStyle};
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::text::{InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin, TextViewState, markdown_ast};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The transcript column's widest, in multiples of the text size: 870 pt at the default 14.5 pt,
/// so answers use a big window instead of leaving half of it empty, and a larger text size
/// widens it in step. Prose, code, tables, cards, the working bar and the composer all share it;
/// a narrower window takes the column down with it.
pub const COLUMN: f32 = 60.;

/// Line height of running text.
pub const LINE_HEIGHT: f32 = 1.65;

/// The transcript column's widest at text size `size`.
pub fn column(size: Pixels) -> Pixels {
    size * COLUMN
}

/// Sizes and spacing of markdown set at `size`, the transcript's text size: all of it scales with
/// the text-size setting, so a larger size keeps the same rhythm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub size: Pixels,
    /// Between paragraphs, code blocks and tables.
    pub paragraph_gap: Pixels,
    /// Between list items.
    pub list_gap: Pixels,
    /// How far a nested list sits in from its parent's text.
    pub list_indent: Pixels,
    /// Above a heading (it starts a section) and below it (it belongs to what follows).
    pub heading_above: Pixels,
    pub heading_below: Pixels,
    /// Code blocks.
    pub code: Pixels,
    /// Table cells' vertical padding.
    pub cell_y: Pixels,
}

impl Metrics {
    pub fn at(size: Pixels) -> Self {
        Self {
            size,
            paragraph_gap: size * 0.8,
            list_gap: size * 0.3,
            list_indent: size * 1.25,
            heading_above: size * 1.1,
            heading_below: size * 0.4,
            code: size * 0.86,
            cell_y: size * 0.5,
        }
    }

    /// A calm scale: answers use headings as section labels, not posters.
    pub fn heading(&self, level: u8) -> Pixels {
        self.size
            * match level {
                1 => 1.3,
                2 => 1.17,
                3 => 1.05,
                _ => 1.,
            }
    }
}

/// The voice a markdown view speaks in: an answer, or the quieter one of the agent's reasoning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tone {
    Prose,
    Muted,
}

/// The colours of markdown text. Body text is a soft grey and bold the theme's full foreground,
/// so emphasis stands out by colour as well as weight (dimming the body alone flattens it,
/// brightening bold alone has nowhere to go).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ink {
    pub body: Hsla,
    pub strong: Hsla,
    /// List bullets and numbers.
    pub marker: Hsla,
}

/// How much of the foreground body text keeps. Paper's foreground is near-black on white, so it
/// takes a little more off for the same softness.
fn body_share(dark: bool) -> f32 {
    if dark { 0.86 } else { 0.82 }
}

impl Ink {
    pub fn new(foreground: Hsla, muted: Hsla, dark: bool, tone: Tone) -> Self {
        let body = foreground.opacity(body_share(dark));
        // Night's foreground is a soft white: bold goes the rest of the way to white.
        let strong = if dark { Hsla { l: foreground.l + (1. - foreground.l) * 0.5, ..foreground } } else { foreground };
        match tone {
            Tone::Prose => Self { body, strong, marker: muted },
            // Reasoning reads a step quieter throughout: its bold is the answer's body.
            Tone::Muted => Self { body: muted, strong: body, marker: muted },
        }
    }
}

/// Trek's markdown style at `size`, in `tone`: body, bold and heading colours from the theme,
/// quiet inline code, bordered code blocks and hairline tables.
pub fn style(size: Pixels, tone: Tone, cx: &App) -> TextViewStyle {
    let theme = cx.theme();
    let m = Metrics::at(size);
    let ink = Ink::new(theme.foreground, theme.muted_foreground, theme.mode.is_dark(), tone);
    // The base style counts its gaps in rems; Trek's follow the transcript's text size instead.
    let rem = |p: Pixels| rems(p / theme.font_size);
    let line = theme.foreground.opacity(0.08);
    let code_block = StyleRefinement::default()
        .bg(theme.foreground.opacity(0.035))
        .border_1()
        .border_color(line)
        .rounded(px(10.))
        .px(px(14.))
        .py(px(12.))
        .text_size(m.code)
        .line_height(relative(1.55))
        .text_color(theme.foreground.opacity(0.92));
    let table = StyleRefinement::default().border_1().border_color(line).rounded(px(8.));
    let table_head = StyleRefinement::default().bg(theme.foreground.opacity(0.035)).text_color(ink.strong).font_weight(FontWeight::MEDIUM);
    let table_cell = StyleRefinement::default().px(px(12.)).py(m.cell_y);
    TextViewStyle::default()
        .with_foreground(ink.body)
        .with_strong(ink.strong)
        .with_muted_foreground(theme.muted_foreground)
        .with_link(crate::palette::link(cx))
        .with_selection(theme.selection)
        .with_code_background(theme.foreground.opacity(0.035))
        .with_border(line)
        .with_dark(theme.mode.is_dark())
        .with_paragraph_gap(rem(m.paragraph_gap))
        .with_list_gap(rem(m.list_gap))
        .with_list_indent(rem(m.list_indent))
        .with_list_marker(ink.marker)
        .with_heading(move |level| {
            StyleRefinement::default()
                .text_size(m.heading(level))
                .line_height(relative(1.3))
                .text_color(ink.strong)
                .pt(m.heading_above)
                .pb(m.heading_below)
        })
        .with_code_block(code_block)
        .with_table(table)
        .with_table_head(table_head)
        .with_table_cell(table_cell)
        .with_inline_code(HighlightStyle {
            background_color: Some(theme.foreground.opacity(0.075)),
            color: Some(theme.foreground.opacity(0.92)),
            ..Default::default()
        })
}

/// A markdown view of an agent's answer in `cwd`, set at `size`, with Trek's style and path
/// chips. `folder`: the tint of folder icons in the chips (the project's colour).
pub fn view(state: &Entity<TextViewState>, cwd: Option<PathBuf>, folder: Option<Hsla>, size: Pixels, cx: &App) -> TextView {
    dress(TextView::new(state), cwd, folder, size, Tone::Prose, None, cx)
}

/// `view`, with Trek's native visualization blocks enabled. Assistant answers use this; tool
/// output, reasoning and secondary previews keep treating the same fence as ordinary code.
/// `live`: the answer is still streaming in, so a visualization block it hasn't closed yet is
/// drawn as far as it's written (in a finished answer one left open can't be drawn).
pub fn answer(state: &Entity<TextViewState>, cwd: Option<PathBuf>, folder: Option<Hsla>, size: Pixels, live: bool, cx: &App) -> TextView {
    dress(TextView::new(state), cwd, folder, size, Tone::Prose, Some(live), cx)
}

/// `view`, for the agent's reasoning: the same markdown, a step quieter.
pub fn thought(state: &Entity<TextViewState>, cwd: Option<PathBuf>, folder: Option<Hsla>, size: Pixels, cx: &App) -> TextView {
    dress(TextView::new(state), cwd, folder, size, Tone::Muted, None, cx)
}

/// `view` of `text` with its state kept by the window under `id`, for answers shown outside the
/// transcript (the side chat, which gets visualization blocks; notes, which don't). `live` as
/// for `answer`.
pub fn keyed(id: impl Into<ElementId>, text: impl Into<SharedString>, cwd: Option<PathBuf>, folder: Option<Hsla>, size: Pixels, visualizations: bool, live: bool, cx: &App) -> TextView {
    dress(TextView::markdown(id, text), cwd, folder, size, Tone::Prose, visualizations.then_some(live), cx)
}

/// Text streaming in fades in, as it arrives.
pub fn streaming() -> TextViewMotion {
    TextViewMotion::default().with_stream_fade(Duration::from_millis(280)).with_stream_fade_stagger(Duration::from_millis(10)).with_stream_fade_easing(Easing::EaseOut)
}

/// `visualizations`: draw `trek-viz` blocks, and whether the text is still streaming in.
fn dress(view: TextView, cwd: Option<PathBuf>, folder: Option<Hsla>, size: Pixels, tone: Tone, visualizations: Option<bool>, cx: &App) -> TextView {
    let link_cwd = cwd.clone();
    let view = view.selectable(true)
        .style(style(size, tone, cx))
        .text_size(size)
        .line_height(relative(LINE_HEIGHT))
        .plugin(LocalImages { cwd: cwd.clone() })
        .plugin(PathChips { cwd, folder })
        .on_link_click(move |url, _, window, cx| {
            // A link to an image on disk previews it here; to another file or folder, opens it
            // with the system; a scheme (https:, mailto:) stays a URL; anything else can't open —
            // open_url's -50 says so bluntly.
            if let Some(path) = link_path(url, link_cwd.as_deref()) {
                if crate::image_preview::showable(&path) {
                    crate::image_preview::open(vec![path], 0, window, cx);
                } else {
                    cx.open_with_system(&path);
                }
            } else if is_url(url) {
                cx.open_url(url);
            } else {
                crate::toast::push(window, format!("No file or URL at {url}"), cx);
            }
        });
    // Liveness is read only when rendering, so the answer finishing doesn't reparse it.
    let view = match visualizations {
        Some(live) => view.plugin(crate::visualization::VisualizationPlugin { live, size }),
        None => view,
    };
    view
        .code_block_actions(|block, _, cx| {
            // Language label and a copy button in the block's corner.
            let code = block.code().to_string();
            let lang = block.lang().map(|l| l.to_string()).unwrap_or_default();
            let muted = cx.theme().muted_foreground;
            h_flex()
                .absolute()
                .top(px(8.))
                .right(px(8.))
                .gap(px(6.))
                .text_size(px(11.5))
                .text_color(muted)
                .when(!lang.is_empty(), |el| el.child(lang))
                .child(
                    gpui_kit::component::button::Button::new("copy")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Copy).text_color(muted))
                        .tooltip("Copy")
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                            crate::toast::push(window, "Copied", cx);
                        }),
                )
        })
}

struct PathRef {
    /// What the agent wrote.
    raw: String,
    /// Resolved on disk, when it exists.
    resolved: Option<PathBuf>,
}

/// The file a link points at on disk, if it does: absolute and `~` paths as written, `file://`
/// unwrapped, and relative links resolved against the answer's folder. Only real paths count.
fn link_path(url: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let raw = url.strip_prefix("file://").unwrap_or(url);
    let path = if let Some(rest) = raw.strip_prefix("~/") {
        trek_core::paths::home().join(rest)
    } else if raw.starts_with('/') || Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else if raw.contains(':') {
        return None;
    } else {
        cwd?.join(raw)
    };
    path.exists().then_some(path)
}

/// `url` has a scheme — https:, mailto:, vscode: — so the system can open it.
fn is_url(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once(':') else { return false };
    scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Inline code that names a file or folder.
fn looks_like_path(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > 260 || s.contains(char::is_whitespace) && !s.starts_with('/') && !s.starts_with('~') {
        return false;
    }
    if s.contains("://") || s.starts_with('-') || s.contains(['(', ')', '{', '}', '=', ';', '<', '>', '|', '`', '"', '\'']) {
        return false;
    }
    let has_sep = s.contains('/');
    let file = s.trim_end_matches('/').rsplit('/').next().unwrap_or(s);
    let has_ext = file
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && (1..=6).contains(&ext.len()) && ext.starts_with(|c: char| c.is_ascii_alphabetic()) && ext.chars().all(|c| c.is_ascii_alphanumeric()));
    let dotfile = file.starts_with('.') && file.len() > 1 && file[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
    (has_sep && (has_ext || s.ends_with('/') || s.starts_with('/') || s.starts_with("~/") || s.starts_with("./"))) || (has_ext && !file.contains("..")) || dotfile
}

/// The path in a chip's text, without a `:line[:col]` after it.
fn path_part(raw: &str) -> &str {
    let raw = raw.trim();
    raw.split(':').next().unwrap_or(raw)
}

fn resolve(raw: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let raw = path_part(raw);
    let p = if let Some(rest) = raw.strip_prefix("~/") {
        trek_core::paths::home().join(rest)
    } else if raw.starts_with('/') {
        PathBuf::from(raw)
    } else {
        cwd?.join(raw)
    };
    crate::system::lately::exists(&p).then_some(p)
}

/// A path that stays in the thread's folder `cwd`: relative ones that don't climb out of it, and
/// absolute ones under it. Only these folders wear the project's colour; `~/` or `/tmp/` aren't
/// the project's.
fn in_folder(raw: &str, cwd: Option<&Path>) -> bool {
    let raw = path_part(raw);
    let path = match raw.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => trek_core::paths::home().join(rest.trim_start_matches('/')),
        _ => PathBuf::from(raw),
    };
    if path.components().any(|c| c == std::path::Component::ParentDir) {
        return false;
    }
    path.is_relative() || cwd.is_some_and(|cwd| path.starts_with(cwd))
}

struct PathChips {
    cwd: Option<PathBuf>,
    /// Folder icons' tint: the project's colour.
    folder: Option<Hsla>,
}

impl MarkdownPlugin for PathChips {
    fn name(&self) -> &str {
        "trek-path"
    }

    fn parse(&self, node: &markdown_ast::Node, _: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let markdown_ast::Node::InlineCode(code) = node else { return None };
        if !looks_like_path(&code.value) {
            return None;
        }
        let raw = code.value.trim().to_string();
        let resolved = resolve(&raw, self.cwd.as_deref());
        Some(MarkdownNode::new("trek-path", PathRef { raw: raw.clone(), resolved }).text(raw))
    }

    fn render_inline(&self, node: &MarkdownNode, context: &InlineRenderContext, _: &mut Window, cx: &mut App) -> Option<InlineElement> {
        let data = node.data::<PathRef>()?;
        let theme = cx.theme();
        let dir = data.raw.ends_with('/') || data.resolved.as_ref().is_some_and(|p| crate::system::lately::is_dir(p));
        let trimmed = data.raw.trim_end_matches('/');
        let label = trimmed.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(trimmed).to_string();
        let size = context.font_size() * 0.88;
        let height = context.line_height().min(context.font_size() * 1.5);
        let resolved = data.resolved.clone();
        let tooltip = data.resolved.as_ref().map(|p| trek_core::paths::tildify(p)).unwrap_or_else(|| data.raw.clone());
        let id = SharedString::from(format!("path-{}", data.raw));
        // Files wear their type's colour, folders the project's (others' folders stay neutral).
        let tint = if dir {
            match self.folder.filter(|_| in_folder(&data.raw, self.cwd.as_deref())) {
                Some(c) => crate::file_icon::tint_of(c, cx),
                None => crate::file_icon::tint("", cx),
            }
        } else {
            crate::file_icon::tint(path_part(trimmed), cx)
        };
        let hover = Hsla { a: (tint.fill.a * 1.8).min(1.), ..tint.fill };
        let chip = h_flex()
            .id(id)
            .test_support()
            .h(height)
            .px(px(5.))
            .gap(px(4.))
            .rounded(px(5.))
            .border_1()
            .border_color(tint.edge)
            .bg(tint.fill)
            .text_size(size)
            .font_family(theme.mono_font_family.clone())
            .text_color(tint.ink)
            .child(if dir { crate::file_icon::folder(path_part(trimmed), false, size, cx) } else { crate::file_icon::badge(path_part(trimmed), size, cx) })
            .child(label)
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx))
            .when_some(resolved, |el, path| {
                el.cursor_pointer().hover(move |s| s.bg(hover)).on_click(move |_, window, cx| {
                    if path.is_dir() {
                        cx.open_with_system(&path)
                    } else if crate::image_preview::showable(&path) {
                        crate::image_preview::open(vec![path.clone()], 0, window, cx)
                    } else {
                        cx.reveal_path(&path)
                    }
                })
            });
        // Sit on the text baseline: the chip is a little taller than the glyphs around it.
        let baseline = (height + context.font_size() * 0.7) / 2.;
        Some(InlineElement::new(chip).with_baseline(baseline))
    }
}

/// An image an answer shows from disk (`![shot](out/shot.png)`), with its size in pixels.
struct LocalImage {
    path: PathBuf,
    pixels: (u32, u32),
}

/// Images in answers that are files on disk: drawn at a sensible size and, clicked, previewed
/// here. Remote ones, and files Trek can't draw, render as the markdown view does by default.
struct LocalImages {
    cwd: Option<PathBuf>,
}

/// The widest and tallest an answer's image is drawn; the preview shows it whole.
const IMAGE_MAX: (f32, f32) = (480., 320.);

/// An image `pixels` big drawn in an answer: a point a pixel, down to fit `IMAGE_MAX`.
fn image_size((w, h): (u32, u32)) -> Size<Pixels> {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let k = (IMAGE_MAX.0 / w).min(IMAGE_MAX.1 / h).min(1.);
    size(px(w * k), px(h * k))
}

impl MarkdownPlugin for LocalImages {
    fn name(&self) -> &str {
        "trek-image"
    }

    fn parse(&self, node: &markdown_ast::Node, _: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let markdown_ast::Node::Image(image) = node else { return None };
        let path = link_path(&image.url, self.cwd.as_deref()).filter(|p| crate::image_preview::showable(p))?;
        let pixels = crate::image_preview::pixels(&path)?;
        Some(MarkdownNode::new("trek-image", LocalImage { path, pixels }).text(image.alt.clone()))
    }

    fn render_inline(&self, node: &MarkdownNode, _: &InlineRenderContext, _: &mut Window, cx: &mut App) -> Option<InlineElement> {
        let data = node.data::<LocalImage>()?;
        let theme = cx.theme();
        let path = data.path.clone();
        let shown = image_size(data.pixels);
        let el = div()
            .id(SharedString::from(format!("md-image-{}", path.display())))
            .test_support()
            .relative()
            .w(shown.width)
            .h(shown.height)
            .my(px(4.))
            .rounded(px(8.))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .cursor_pointer()
            .hover(|s| s.border_color(theme.foreground.opacity(0.35)))
            .child(img(path.clone()).size_full())
            .child(crate::image_preview::thumb(path.clone()))
            .on_click(move |_, window, cx| crate::image_preview::open(vec![path.clone()], 0, window, cx));
        Some(InlineElement::new(el))
    }
}

use gpui_kit::prelude::FluentBuilder as _;

#[cfg(test)]
mod tests {
    use super::{COLUMN, IMAGE_MAX, Ink, LINE_HEIGHT, Metrics, Tone, column, image_size, in_folder, is_url, link_path, looks_like_path, path_part};
    use std::path::Path;
    use gpui_kit::{Hsla, px, rgb};

    #[test]
    fn spacing_and_sizes_scale_with_the_text() {
        let base = Metrics::at(px(14.5));
        let big = Metrics::at(px(18.));
        let k = 18. / 14.5;
        let close = |a: gpui_kit::Pixels, b: gpui_kit::Pixels| (a - b).abs() < px(0.01);
        for (a, b) in [
            (column(base.size), column(big.size)),
            (base.paragraph_gap, big.paragraph_gap),
            (base.list_gap, big.list_gap),
            (base.list_indent, big.list_indent),
            (base.heading_above, big.heading_above),
            (base.heading_below, big.heading_below),
            (base.code, big.code),
            (base.cell_y, big.cell_y),
        ] {
            assert!(close(a * k, b), "{a:?} × {k} vs {b:?}");
        }
        for level in 1..=6 {
            assert!(close(base.heading(level) * k, big.heading(level)));
        }
        // Headings: modest, from the largest down to body size, with more room above than below.
        assert!(base.heading(1) > base.heading(2) && base.heading(2) > base.heading(3) && base.heading(3) >= base.heading(4));
        assert!(base.heading(1) <= base.size * 1.35, "a section label, not a poster");
        assert_eq!(base.heading(6), base.size);
        assert!(base.heading_above > base.heading_below * 2.);
        // Lists breathe a little, less than paragraphs; code sits a size below the prose.
        assert!(base.list_gap > px(0.) && base.list_gap < base.paragraph_gap);
        assert!(base.code < base.size && (base.code - px(12.5)).abs() < px(0.1), "{:?}", base.code);
        assert!((1.6..=1.7).contains(&LINE_HEIGHT));
    }

    #[test]
    fn the_column_is_wide_and_follows_the_text_size() {
        assert_eq!(column(px(14.5)), px(14.5 * COLUMN));
        // Wide on a big window at the default size, the old 760 pt cap left well behind.
        assert!(column(px(14.5)) >= px(860.) && column(px(14.5)) <= px(900.), "{:?}", column(px(14.5)));
        // Every size the setting offers widens it in step.
        let sizes = [13.5, 14.5, 16., 17.5].map(px);
        assert!(sizes.windows(2).all(|w| column(w[0]) < column(w[1])));
        assert!(column(px(13.5)) > px(760.));
    }

    // WCAG 2 contrast, with translucent colours composited over the surface they sit on.

    fn srgb(c: Hsla) -> [f32; 4] {
        let r = gpui_kit::Rgba::from(c);
        [r.r, r.g, r.b, r.a]
    }

    fn over(top: Hsla, under: [f32; 3]) -> [f32; 3] {
        let [r, g, b, a] = srgb(top);
        [r * a + under[0] * (1. - a), g * a + under[1] * (1. - a), b * a + under[2] * (1. - a)]
    }

    fn luminance(c: [f32; 3]) -> f32 {
        let lin = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
    }

    fn contrast(a: [f32; 3], b: [f32; 3]) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    struct Colors {
        dark: bool,
        background: Hsla,
        foreground: Hsla,
        muted: Hsla,
    }

    fn theme(name: &str) -> Colors {
        let themes: serde_json::Value = serde_json::from_str(include_str!("../assets/themes/trek.json")).unwrap();
        let t = themes["themes"].as_array().unwrap().iter().find(|t| t["name"] == name).expect(name);
        let color = |k: &str| -> Hsla { rgb(u32::from_str_radix(t["colors"][k].as_str().unwrap().trim_start_matches('#'), 16).unwrap()).into() };
        Colors { dark: t["mode"] == "dark", background: color("background"), foreground: color("foreground"), muted: color("muted.foreground") }
    }

    #[test]
    fn file_chips_read_in_their_colours_in_both_themes() {
        const AA: f32 = 4.5;
        for name in ["Trek Night", "Trek Paper"] {
            let t = theme(name);
            let bg = over(t.background, [0.; 3]);
            for path in ["a.rs", "a.ts", "a.js", "a.py", "a.go", "a.swift", "a.kt", "a.java", "a.rb", "a.md", "a.json", "a.toml", "a.yml", "a.html", "a.css", "a.sh", "a.sql", "a.png"] {
                let Some(fill) = crate::file_icon::type_colour(path) else { continue };
                let tint = crate::file_icon::tint_in(rgb(fill).into(), t.dark);
                let chip = over(tint.fill, bg);
                let ratio = contrast(over(tint.ink, chip), chip);
                assert!(ratio >= AA, "{path} in {name}: {ratio:.2}");
            }
        }
    }

    #[test]
    fn answers_are_readable_in_both_themes() {
        // Body-size text (14.5 pt, not "large") needs 4.5:1.
        const AA: f32 = 4.5;
        for (name, link) in [("Trek Night", crate::palette::LINK.0), ("Trek Paper", crate::palette::LINK.1)] {
            let t = theme(name);
            let bg = over(t.background, [0.; 3]);
            let on = |c: Hsla, surface: [f32; 3]| contrast(over(c, surface), surface);
            let ink = Ink::new(t.foreground, t.muted, t.dark, Tone::Prose);
            let quiet = Ink::new(t.foreground, t.muted, t.dark, Tone::Muted);
            // What the inline code and path chips sit on, as `style` and `PathChips` draw them.
            let code_bg = over(t.foreground.opacity(0.075), bg);
            let chip_bg = over(t.foreground.opacity(0.045), bg);
            for (what, ratio) in [
                ("body", on(ink.body, bg)),
                ("bold", on(ink.strong, bg)),
                ("muted", on(t.muted, bg)),
                ("list marker", on(ink.marker, bg)),
                ("reasoning", on(quiet.body, bg)),
                ("reasoning bold", on(quiet.strong, bg)),
                ("inline code", on(t.foreground.opacity(0.92), code_bg)),
                ("path chip", on(t.foreground.opacity(0.9), chip_bg)),
                ("link", on(rgb(link).into(), bg)),
            ] {
                assert!(ratio >= AA, "{name}: {what} {ratio:.2}:1");
            }
            // Soft body, bright bold: emphasis stands out by colour as well as weight.
            let (body, strong) = (luminance(over(ink.body, bg)), luminance(over(ink.strong, bg)));
            if t.dark {
                assert!(strong > body * 1.25, "{name}: bold {strong} vs body {body}");
            } else {
                assert!(strong < body * 0.6, "{name}: bold {strong} vs body {body}");
            }
            // Body keeps 80–88% of the foreground; reasoning is quieter than the answer.
            assert!((0.8..=0.88).contains(&ink.body.a), "{name}: {}", ink.body.a);
            assert!(on(quiet.body, bg) < on(ink.body, bg));
            assert!(on(quiet.strong, bg) > on(quiet.body, bg), "{name}: reasoning's bold still stands out");
        }
    }

    #[test]
    #[cfg_attr(windows, ignore = "the cases are Unix absolute paths (/Users/me, /tmp); Windows ones (drive-letter paths) arrive with the paths phase")]
    fn only_the_threads_own_folders_are_the_projects() {
        let cwd = Some(Path::new("/Users/me/code/app"));
        for inside in ["src/", "./src/", "src/ui/", "/Users/me/code/app/src/", "src/main.rs:12"] {
            assert!(in_folder(inside, cwd), "{inside}");
        }
        for outside in ["~/", "~/Downloads/", "/tmp/", "/usr/local/", "../other-repo/", "src/../../x/", "/Users/me/code/application/"] {
            assert!(!in_folder(outside, cwd), "{outside}");
        }
        // Without a folder only relative paths can be its.
        assert!(in_folder("src/", None) && !in_folder("/Users/me/code/app/src/", None));
    }

    #[test]
    fn spots_paths() {
        for p in ["src/main.rs", "crates/trek-agents/src/codex.rs", "settings.local.json", "~/.codex/config.toml", "/Users/x/Trek/", "research_notes/", ".gitignore", "README.md"] {
            assert!(looks_like_path(p), "{p}");
        }
        for p in ["turn/start", "cargo build", "foo()", "x = 1", "https://a.com/b.js", "v1.2", "--flag", "a..b"] {
            assert!(!looks_like_path(p), "{p}");
        }
    }

    #[test]
    fn line_numbers_are_not_part_of_the_path() {
        assert_eq!(path_part("/Users/me/app/src/main.rs:42"), "/Users/me/app/src/main.rs");
        assert_eq!(path_part("src/lib.rs:7:3"), "src/lib.rs");
        assert_eq!(path_part(" README.md "), "README.md");
        assert_eq!(crate::file_icon::icon_name(path_part("src/main.rs:42")), crate::file_icon::icon_name("main.rs"));
    }

    #[test]
    fn a_link_to_a_file_on_disk_beats_a_url() {
        let dir = std::env::temp_dir().join(format!("trek-md-links-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("shot.png");
        std::fs::write(&file, b"x").unwrap();
        let abs = file.to_string_lossy().to_string();
        assert_eq!(link_path(&abs, Some(&dir)).as_deref(), Some(file.as_path()));
        assert_eq!(link_path("shot.png", Some(&dir)).as_deref(), Some(file.as_path()));
        #[cfg(unix)]
        assert_eq!(link_path("file:///tmp", None).as_deref(), Some(Path::new("/tmp")));
        assert!(link_path("https://example.com", Some(&dir)).is_none());
        assert!(link_path("mailto:a@b.co", Some(&dir)).is_none());
        assert!(link_path("nope.txt", Some(&dir)).is_none());
        assert!(is_url("https://x.y") && is_url("mailto:a@b") && is_url("file:///x"));
        assert!(!is_url("/tmp/x") && !is_url("a b") && !is_url("x"));
    }

    #[test]
    fn answer_images_are_a_point_a_pixel_up_to_a_bound() {
        assert_eq!(image_size((64, 32)), gpui_kit::size(px(64.), px(32.)));
        let shot = image_size((2880, 1800));
        assert!((shot.width.as_f32() - IMAGE_MAX.0).abs() < 0.01 && shot.height.as_f32() <= IMAGE_MAX.1);
        let tall = image_size((400, 4000));
        assert!((tall.height.as_f32() - IMAGE_MAX.1).abs() < 0.01 && (tall.width.as_f32() - 32.).abs() < 0.01);
    }
}
