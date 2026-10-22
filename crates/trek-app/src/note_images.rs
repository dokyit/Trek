//! Pictures in notes: how a note's `![alt](link)` draws in its preview, and the strip of them
//! under the text in Write. Images kept with the notes (`attachments/…`, see
//! `trek_core::notes::add_image`), files elsewhere on disk and `https:` ones all show, fit to the
//! preview's width in their own proportions; a retina screenshot (a PNG that says it's 144 dpi)
//! at the size it was on screen, every pixel used. SVGs are drawn at the window's scale, so they
//! stay sharp. A click opens the image viewer (`image_preview`), stepping through the note's
//! other pictures; one that can't be found or read is a placeholder saying so.

use gpui_kit::component::text::{
    InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin,
    markdown_ast,
};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

/// The tallest an image in a note is drawn: a long phone screenshot narrows instead.
const TALLEST: f32 = 720.;

/// Where a note's image comes from, as far as drawing it goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A picture on disk GPUI decodes: its size in pixels, and how many pixels a point it was
    /// made at (2 for a retina screenshot).
    File {
        path: PathBuf,
        pixels: (u32, u32),
        density: f32,
    },
    /// An SVG on disk, drawn at the window's scale.
    Svg(PathBuf),
    /// On the web.
    Remote(SharedString),
    /// On disk, but not something Trek can draw (a HEIC photo, a damaged file).
    Unreadable(PathBuf),
    /// Nowhere to be found: the link as written.
    Missing(String),
}

impl Source {
    /// Where `link` in a note kept in `dir` points, and what it is.
    pub fn of(link: &str, dir: &Path) -> Option<Source> {
        let link = link.trim();
        if link.is_empty() || link.starts_with("data:") {
            return None;
        }
        let lower = link.to_ascii_lowercase();
        if lower.starts_with("https://") || lower.starts_with("http://") {
            return Some(Source::Remote(link.to_string().into()));
        }
        let Some(path) = path_of(link, dir) else {
            return Some(Source::Missing(link.to_string()));
        };
        if !path.is_file() {
            return Some(Source::Missing(link.to_string()));
        }
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
        {
            return Some(Source::Svg(path));
        }
        match crate::mentions::is_image(&path)
            .then(|| crate::image_preview::pixels(&path))
            .flatten()
        {
            Some(pixels) => Some(Source::File {
                density: density(&path),
                path,
                pixels,
            }),
            None => Some(Source::Unreadable(path)),
        }
    }

    /// The file it is on disk, if it's one the viewer can be asked to show.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Source::File { path, .. } | Source::Svg(path) | Source::Unreadable(path) => Some(path),
            _ => None,
        }
    }
}

/// The file `link` names, relative to the notes folder `dir`: `attachments/…`, `~/…`, absolute
/// paths and `file://` ones. `None` for another scheme (`ftp:`, `mailto:`).
fn path_of(link: &str, dir: &Path) -> Option<PathBuf> {
    let raw = link.strip_prefix("file://").unwrap_or(link);
    let path = if let Some(rest) = raw.strip_prefix("~/") {
        trek_core::paths::home().join(rest)
    } else if Path::new(raw).is_absolute() || raw.starts_with('/') {
        PathBuf::from(raw)
    } else if raw.contains(':') {
        return None;
    } else {
        dir.join(raw)
    };
    // Markdown writes a space in a link as `%20`.
    if !path.exists() && raw.contains('%') {
        let decoded = percent_decode(&path.to_string_lossy());
        return Some(PathBuf::from(decoded));
    }
    Some(path)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// How many pixels a point `path` was made at: a PNG's `pHYs` says its dots per inch (macOS
/// writes 144 into a retina screenshot), 72 a point. 1 when it doesn't say.
pub fn density(path: &Path) -> f32 {
    static SEEN: LazyLock<Mutex<HashMap<PathBuf, f32>>> = LazyLock::new(Default::default);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    *seen
        .entry(path.to_path_buf())
        .or_insert_with(|| png_dpi(path).map(density_at).unwrap_or(1.))
}

/// Pixels a point at `dpi`: a whole 1–3, only for what's clearly denser than a screen's 72.
fn density_at(dpi: f32) -> f32 {
    if dpi < 108. {
        1.
    } else {
        (dpi / 72.).round().clamp(1., 3.)
    }
}

/// A PNG's horizontal dots per inch, from its `pHYs` chunk (which comes before the pixels).
fn png_dpi(path: &Path) -> Option<f32> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut sig = [0u8; 8];
    f.read_exact(&mut sig).ok()?;
    if sig != *b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    for _ in 0..64 {
        let mut head = [0u8; 8];
        f.read_exact(&mut head).ok()?;
        let len = u32::from_be_bytes(head[..4].try_into().ok()?);
        match &head[4..] {
            b"pHYs" if len == 9 => {
                let mut data = [0u8; 9];
                f.read_exact(&mut data).ok()?;
                let per_unit = u32::from_be_bytes(data[..4].try_into().ok()?);
                // Unit 1: per metre. 0 is an aspect ratio only.
                return (data[8] == 1 && per_unit > 0).then_some(per_unit as f32 * 0.0254);
            }
            b"IDAT" | b"IEND" => return None,
            _ => {
                f.seek(SeekFrom::Current(len as i64 + 4)).ok()?;
            }
        }
    }
    None
}

/// An image `natural` points big, drawn no wider than `widest` nor taller than `TALLEST`, in its
/// own proportions.
pub fn fit(natural: Size<Pixels>, widest: Pixels) -> Size<Pixels> {
    let (w, h) = (
        natural.width.as_f32().max(1.),
        natural.height.as_f32().max(1.),
    );
    let k = (widest.as_f32().max(48.) / w).min(TALLEST / h).min(1.);
    size(px((w * k).round()), px((h * k).round()))
}

/// What to ask GPUI's SVG renderer for (×100) on a window at `scale`: it already draws at
/// `SMOOTH_SVG_SCALE_FACTOR` pixels a point (enough for a 2× screen), so only more than that is
/// asked for.
fn svg_scale(scale: f32) -> u32 {
    ((scale / SMOOTH_SVG_SCALE_FACTOR).max(1.) * 100.).round() as u32
}

/// The SVG at a path drawn at a scale (×100, see `svg_scale`): sharp at the window's pixels.
enum SvgAt {}

impl Asset for SvgAt {
    type Source = (PathBuf, u32);
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        async move {
            let bytes = std::fs::read(&source.0)?;
            Ok(renderer.render_single_frame(&bytes, source.1 as f32 / 100.)?)
        }
    }
}

/// The most a picture from the web may weigh.
const WEB_MAX: usize = 25 << 20;

/// A picture from the web a note shows, fetched by Trek itself: GPUI's desktop apps have no
/// HTTP client of their own, and this keeps fetching to notes (an agent's answer doesn't load
/// pictures from the web by itself). Ready to draw, with how many pixels a point it has.
enum WebImage {}

impl Asset for WebImage {
    type Source = SharedString;
    type Output = Result<(Arc<RenderImage>, f32), SharedString>;

    fn load(
        url: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        async move {
            let fetched = trek_core::runtime().spawn(async move {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(20))
                    .user_agent("Trek")
                    .build()
                    .map_err(|e| e.to_string())?;
                let mut response = client
                    .get(url.as_ref())
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
                    bytes.extend_from_slice(&chunk);
                    if bytes.len() > WEB_MAX {
                        return Err("it's too big".to_string());
                    }
                }
                Ok(bytes)
            });
            let bytes = fetched
                .await
                .map_err(|e| SharedString::from(e.to_string()))??;
            let format =
                web_format(&bytes).ok_or_else(|| SharedString::from("it isn't an image"))?;
            let image = Image::from_bytes(format, bytes)
                .to_image_data(renderer)
                .map_err(|e| SharedString::from(e.to_string()))?;
            Ok((
                image,
                if format == ImageFormat::Svg {
                    SMOOTH_SVG_SCALE_FACTOR
                } else {
                    1.
                },
            ))
        }
    }
}

/// What kind of picture downloaded `bytes` are, by their content.
fn web_format(bytes: &[u8]) -> Option<ImageFormat> {
    match image::guess_format(bytes).ok() {
        Some(image::ImageFormat::Png) => Some(ImageFormat::Png),
        Some(image::ImageFormat::Jpeg) => Some(ImageFormat::Jpeg),
        Some(image::ImageFormat::Gif) => Some(ImageFormat::Gif),
        Some(image::ImageFormat::WebP) => Some(ImageFormat::Webp),
        Some(image::ImageFormat::Bmp) => Some(ImageFormat::Bmp),
        Some(image::ImageFormat::Tiff) => Some(ImageFormat::Tiff),
        Some(image::ImageFormat::Ico) => Some(ImageFormat::Ico),
        _ => {
            let head =
                String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_ascii_lowercase();
            head.contains("<svg").then_some(ImageFormat::Svg)
        }
    }
}

/// A picture made ready to draw: its pixels, and their size in points.
fn drawn(image: Arc<RenderImage>, scale: f32) -> (Arc<RenderImage>, Size<Pixels>) {
    let s = image.size(0);
    (
        image.clone(),
        size(px(s.width.0 as f32 / scale), px(s.height.0 as f32 / scale)),
    )
}

/// The pictures in a note's preview.
pub struct NoteImages {
    /// The notes folder: `attachments/…` is relative to it.
    pub dir: PathBuf,
    /// The widest an image is drawn: the preview's text column.
    pub widest: Pixels,
    /// How wide the preview's lines are: each picture takes one of its own, so the words around
    /// it don't run alongside.
    pub line: Pixels,
    /// The note's pictures on disk, in order, so the viewer steps through them all.
    pub gallery: Arc<Vec<PathBuf>>,
}

struct Parsed {
    source: Source,
    alt: String,
}

/// A file attached to the note (a video, a PDF) that its preview shows as a chip.
struct ParsedFile {
    path: PathBuf,
    name: String,
}

/// The attached file (not an image) a note's `link` names: in the notes' own `attachments/`
/// and there on disk.
fn attached_file(link: &str, dir: &Path) -> Option<PathBuf> {
    let shown = link.trim().strip_prefix("./").unwrap_or(link.trim());
    let kind = shown
        .rsplit_once('.')
        .map(|(_, k)| k.to_ascii_lowercase())
        .unwrap_or_default();
    if !shown.starts_with(&format!("{}/", trek_core::notes::ATTACHMENTS))
        || trek_core::notes::IMAGE_KINDS.contains(&kind.as_str())
    {
        return None;
    }
    path_of(shown, dir).filter(|p| p.is_file())
}

/// A video attached to a note, on a line of its own: it plays in place, in a card under the
/// paragraph's place (`artifact::video_card`), where any other file is a chip.
pub struct NoteVideos {
    /// The notes folder: `attachments/…` is relative to it.
    pub dir: PathBuf,
}

/// The video a paragraph is, if that's all it holds: one link to one.
struct ParsedVideo {
    doc: Arc<crate::artifact::Doc>,
}

impl MarkdownPlugin for NoteVideos {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "trek-note-video"
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        _: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::Paragraph(paragraph) = node else {
            return None;
        };
        let [markdown_ast::Node::Link(link)] = paragraph.children.as_slice() else {
            return None;
        };
        let path = attached_file(&link.url, &self.dir)
            .filter(|p| trek_core::notes::is_video(&p.to_string_lossy()))?;
        let name = markdown_ast::Node::Link(link.clone()).to_string();
        let name = if name.trim().is_empty() {
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            name
        };
        let doc = crate::artifact::Doc::video(path, name.clone(), &link.url);
        Some(MarkdownNode::new("trek-note-video", ParsedVideo { doc: Arc::new(doc) }).text(name))
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(video) = node.data::<ParsedVideo>() else {
            return div().into_any_element();
        };
        let key = (video.doc.hash as usize)
            .wrapping_add(node.source_range().map(|r| r.start).unwrap_or(0));
        // Artifacts off: the file's chip, which opens it in Quick Look.
        let shown = crate::artifact::video_card(&video.doc, key, window, cx).unwrap_or_else(|| {
            let file = ParsedFile {
                path: video.doc.media.clone().unwrap_or_default(),
                name: video.doc.title.clone().unwrap_or_default(),
            };
            chip(&file, px(14.), px(22.), cx)
        });
        div().py_2().child(shown).into_any_element()
    }
}

impl MarkdownPlugin for NoteImages {
    fn name(&self) -> &str {
        "trek-note-image"
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        _: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        match node {
            markdown_ast::Node::Image(image) => {
                let source = Source::of(&image.url, &self.dir)?;
                Some(
                    MarkdownNode::new(
                        "trek-note-image",
                        Parsed {
                            source,
                            alt: image.alt.clone(),
                        },
                    )
                    .text(image.alt.clone()),
                )
            }
            markdown_ast::Node::Link(link) => {
                let path = attached_file(&link.url, &self.dir)?;
                let written = node.to_string();
                let name = if written.trim().is_empty() {
                    path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                } else {
                    written
                };
                Some(
                    MarkdownNode::new(
                        "trek-note-image",
                        ParsedFile {
                            path,
                            name: name.clone(),
                        },
                    )
                    .text(name),
                )
            }
            _ => None,
        }
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        if let Some(file) = node.data::<ParsedFile>() {
            return Some(file_chip(file, context, cx));
        }
        let data = node.data::<Parsed>()?;
        let picture = picture(
            &data.source,
            &data.alt,
            self.widest.min(self.line),
            &self.gallery,
            window,
            cx,
        );
        Some(InlineElement::new(div().w(self.line).child(picture)))
    }
}

/// An attached file in a note's preview: its type's icon and its name; a click shows it (Quick
/// Look plays a video).
fn file_chip(file: &ParsedFile, context: &InlineRenderContext, cx: &App) -> InlineElement {
    let size = context.font_size() * 0.92;
    let height = context.line_height().min(context.font_size() * 1.6);
    let baseline = (height + context.font_size() * 0.7) / 2.;
    InlineElement::new(chip(file, size, height, cx)).with_baseline(baseline)
}

/// The chip itself: `size` the text's, `height` the chip's.
fn chip(file: &ParsedFile, size: Pixels, height: Pixels, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let name = file.path.to_string_lossy().to_string();
    let tint = crate::file_icon::tint(&name, cx);
    let hover = Hsla {
        a: (tint.fill.a * 1.8).min(1.),
        ..tint.fill
    };
    let path = file.path.clone();
    h_flex()
        .id(SharedString::from(format!(
            "note-file-{}",
            file.path
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        )))
        .test_support()
        .h(height)
        .px(px(7.))
        .gap(px(5.))
        .rounded(px(6.))
        .border_1()
        .border_color(tint.edge)
        .bg(tint.fill)
        .text_size(size)
        .text_color(theme.foreground)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(crate::file_icon::badge(&name, size * 1.1, cx))
        .child(file.name.clone())
        .on_click(move |_, window, cx| {
            crate::image_preview::open(vec![path.clone()], 0, window, cx)
        })
        .into_any_element()
}

/// `source` drawn in a note at most `widest` across; a click shows it in the viewer.
pub fn picture(
    source: &Source,
    alt: &str,
    widest: Pixels,
    gallery: &Arc<Vec<PathBuf>>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let scale = window.scale_factor();
    let ready = match source {
        Source::File {
            path,
            pixels,
            density,
        } => {
            let natural = size(px(pixels.0 as f32 / density), px(pixels.1 as f32 / density));
            Ok((ImageSource::from(path.clone()), natural))
        }
        Source::Svg(path) => match window.use_asset::<SvgAt>(&(path.clone(), svg_scale(scale)), cx)
        {
            Some(Ok(image)) => {
                let (image, natural) = drawn(
                    image,
                    svg_scale(scale) as f32 / 100. * SMOOTH_SVG_SCALE_FACTOR,
                );
                Ok((ImageSource::Render(image), natural))
            }
            Some(Err(_)) => Err(Problem::Unreadable),
            None => Err(Problem::Loading),
        },
        Source::Remote(url) => match window.use_asset::<WebImage>(url, cx) {
            Some(Ok((image, density))) => {
                let (image, natural) = drawn(image, density);
                Ok((ImageSource::Render(image), natural))
            }
            Some(Err(_)) => Err(Problem::Unreachable),
            None => Err(Problem::Loading),
        },
        Source::Unreadable(_) => Err(Problem::Unreadable),
        Source::Missing(_) => Err(Problem::Missing),
    };
    let id = SharedString::from(format!("note-image-{}", label(source)));
    match ready {
        Ok((image, natural)) => {
            let shown = fit(natural, widest);
            let theme = cx.theme();
            div()
                .id(id)
                .test_support()
                .relative()
                .w(shown.width)
                .h(shown.height)
                .my(px(6.))
                .rounded(px(8.))
                .overflow_hidden()
                .border_1()
                .border_color(theme.border)
                .cursor_pointer()
                .hover(|s| s.border_color(theme.foreground.opacity(0.35)))
                .child(img(image).size_full())
                .when_some(source.path().map(Path::to_path_buf), |el, path| {
                    el.child(crate::image_preview::thumb(path))
                })
                .on_click({
                    let (source, gallery) = (source.clone(), gallery.clone());
                    move |_, window, cx| open(&source, &gallery, window, cx)
                })
                .into_any_element()
        }
        Err(problem) => placeholder(id, source, alt, problem, widest, gallery, cx),
    }
}

/// Why an image isn't drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Problem {
    Loading,
    Missing,
    Unreadable,
    Unreachable,
}

/// What a placeholder calls the image: its file's name, or the link.
fn label(source: &Source) -> String {
    match source {
        Source::Remote(url) => url.to_string(),
        Source::Missing(link) => link.clone(),
        other => other
            .path()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}

/// A box where an image would be, saying why it isn't.
fn placeholder(
    id: SharedString,
    source: &Source,
    alt: &str,
    problem: Problem,
    widest: Pixels,
    gallery: &Arc<Vec<PathBuf>>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (icon, headline) = match problem {
        Problem::Loading => (crate::assets::Lucide::Image, "Loading image…"),
        Problem::Missing => (crate::assets::Lucide::ImageOff, "Image not found"),
        Problem::Unreadable => (
            crate::assets::Lucide::ImageOff,
            "Can't show this image here",
        ),
        Problem::Unreachable => (crate::assets::Lucide::ImageOff, "Couldn't load this image"),
    };
    let name = if alt.trim().is_empty() {
        label(source)
    } else {
        format!("{} · {}", alt.trim(), label(source))
    };
    let clickable = matches!(source, Source::Unreadable(_) | Source::Remote(_));
    h_flex()
        .id(id)
        .test_support()
        .my(px(6.))
        .w(widest.min(px(440.)))
        .px(px(12.))
        .py(px(10.))
        .gap(px(10.))
        .rounded(px(8.))
        .border_1()
        .border_dashed()
        .border_color(theme.foreground.opacity(0.2))
        .bg(theme.foreground.opacity(0.03))
        .child(Icon::new(icon).small().text_color(theme.muted_foreground))
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(theme.foreground.opacity(0.8))
                        .child(headline),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(theme.muted_foreground)
                        .child(name),
                ),
        )
        .when(clickable, |el| {
            let (source, gallery) = (source.clone(), gallery.clone());
            el.cursor_pointer()
                .on_click(move |_, window, cx| open(&source, &gallery, window, cx))
        })
        .into_any_element()
}

/// Show `source` bigger: in the viewer, stepping through the note's `gallery` (Quick Look for
/// what Trek doesn't draw itself), or in the browser for one on the web.
fn open(source: &Source, gallery: &Arc<Vec<PathBuf>>, window: &mut Window, cx: &mut App) {
    match source {
        Source::Remote(url) => cx.open_url(url),
        other => {
            let Some(path) = other.path() else { return };
            match gallery.iter().position(|p| p == path) {
                Some(i) => crate::image_preview::open(gallery.to_vec(), i, window, cx),
                None => crate::image_preview::open(vec![path.to_path_buf()], 0, window, cx),
            }
        }
    }
}

/// A note as the preview reads it: a line that's only a picture (as Trek puts them in) is a
/// paragraph of its own, so the lines around it don't join it with a space. Fenced code is left
/// as it is; the note itself isn't changed.
pub fn for_reading(body: &str) -> std::borrow::Cow<'_, str> {
    let lines: Vec<&str> = body.split('\n').collect();
    let mut fenced = false;
    let alone: Vec<bool> = lines
        .iter()
        .map(|l| {
            let t = l.trim();
            if t.starts_with("```") || t.starts_with("~~~") {
                fenced = !fenced;
                return false;
            }
            !fenced
                && (t.starts_with("![") && trek_core::notes::images(t).first().is_some_and(|i| i.range == (0..t.len()))
                    // A video plays in a card of its own.
                    || t.starts_with('[') && trek_core::notes::files(t).first().is_some_and(|f| f.range == (0..t.len()) && trek_core::notes::is_video(&f.link)))
        })
        .collect();
    if !alone.contains(&true) {
        return body.into();
    }
    let mut out = String::with_capacity(body.len() + 16);
    for (i, line) in lines.iter().enumerate() {
        let blank = |j: usize| lines.get(j).is_none_or(|l| l.trim().is_empty());
        if i > 0 {
            out.push('\n');
            // A blank line between a picture and the text beside it.
            if (alone[i] && !blank(i - 1)) || (alone[i - 1] && !alone[i] && !blank(i)) {
                out.push('\n');
            }
        }
        out.push_str(line);
    }
    out.into()
}

/// A note's pictures on disk, in order, for the viewer to step through.
pub fn gallery(body: &str, dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for image in trek_core::notes::images(body) {
        if let Some(path) = Source::of(&image.link, dir)
            .as_ref()
            .and_then(Source::path)
            .map(Path::to_path_buf)
            && !out.contains(&path)
        {
            out.push(path);
        }
    }
    out
}

/// Height of the tiles under the text in Write.
const TILE: f32 = 72.;

/// An attached file as a tile in Write: its type's icon, name and size; a click shows it (Quick
/// Look plays a video). One that's gone is a dashed tile saying so.
fn file_tile(i: usize, file: &trek_core::notes::FileRef, dir: &Path, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let path = path_of(&file.link, dir).filter(|p| p.is_file());
    let tile = div()
        .id(("note-file", i))
        .test_support()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(10.))
        .w(px(190.))
        .h(px(TILE))
        .px(px(12.))
        .rounded(px(8.))
        .border_1()
        .overflow_hidden();
    let Some(path) = path else {
        return tile
            .border_dashed()
            .border_color(theme.foreground.opacity(0.2))
            .bg(theme.foreground.opacity(0.03))
            .child(
                Icon::new(crate::assets::Lucide::Paperclip)
                    .small()
                    .text_color(theme.muted_foreground),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme.foreground.opacity(0.8))
                            .child("File not found"),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(theme.muted_foreground)
                            .child(file.name.clone()),
                    ),
            )
            .into_any_element();
    };
    let on_disk = path.to_string_lossy().to_string();
    let kind = path
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_default();
    let meta = [
        Some(kind).filter(|k| !k.is_empty()),
        std::fs::metadata(&path)
            .ok()
            .map(|m| crate::image_preview::file_size(m.len())),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let name = if file.name.trim().is_empty() {
        on_disk.rsplit('/').next().unwrap_or_default().to_string()
    } else {
        file.name.clone()
    };
    tile.border_color(theme.border)
        .bg(theme.foreground.opacity(0.03))
        .cursor_pointer()
        .hover(|s| s.border_color(theme.foreground.opacity(0.35)))
        .child(crate::file_icon::badge(&on_disk, px(28.), cx))
        .child(
            v_flex()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(theme.foreground)
                        .child(name),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(theme.muted_foreground)
                        .child(meta),
                ),
        )
        .on_click(move |_, window, cx| {
            crate::image_preview::open(vec![path.clone()], 0, window, cx)
        })
        .into_any_element()
}

/// The note's pictures as small tiles, for Write (where the text shows only their links). A
/// click opens one in the viewer (no tooltip: it would stay up over the viewer); one that can't
/// be shown is a tile saying so.
pub fn strip(body: &str, dir: &Path, cx: &App) -> Option<AnyElement> {
    let mut pieces: Vec<(
        usize,
        Result<trek_core::notes::ImageRef, trek_core::notes::FileRef>,
    )> = trek_core::notes::images(body)
        .into_iter()
        .map(|i| (i.range.start, Ok(i)))
        .collect();
    pieces.extend(
        trek_core::notes::files(body)
            .into_iter()
            .map(|f| (f.range.start, Err(f))),
    );
    if pieces.is_empty() {
        return None;
    }
    pieces.sort_by_key(|(at, _)| *at);
    let theme = cx.theme().clone();
    let gallery = Arc::new(gallery(body, dir));
    let tiles = pieces
        .into_iter()
        .enumerate()
        .filter_map(|(i, (_, piece))| {
            let image = match piece {
                Ok(image) => image,
                Err(file) => return Some(file_tile(i, &file, dir, cx)),
            };
            let source = Source::of(&image.link, dir)?;
            let (width, picture): (Pixels, Option<ImageSource>) = match &source {
                Source::File { path, pixels, .. } => {
                    let aspect = pixels.0.max(1) as f32 / pixels.1.max(1) as f32;
                    (
                        px((TILE * aspect).clamp(TILE * 0.6, TILE * 2.2)),
                        Some(path.clone().into()),
                    )
                }
                Source::Svg(path) => (px(TILE * 1.6), Some(path.clone().into())),
                Source::Remote(url) => {
                    let url = url.clone();
                    let web =
                        move |window: &mut Window, cx: &mut App| {
                            let loaded = window.use_asset::<WebImage>(&url, cx)?;
                            Some(loaded.map(|(image, _)| image).map_err(|e| {
                                ImageCacheError::Other(Arc::new(anyhow::anyhow!("{e}")))
                            }))
                        };
                    (px(TILE * 1.4), Some(ImageSource::Custom(Arc::new(web))))
                }
                Source::Unreadable(_) | Source::Missing(_) => (px(TILE), None),
            };
            let broken = picture.is_none();
            // A drawing shows whole (its edges are the point); a photo fills its tile.
            let fit = if matches!(source, Source::Svg(_)) {
                ObjectFit::Contain
            } else {
                ObjectFit::Cover
            };
            let missing_icon = || {
                Icon::new(crate::assets::Lucide::ImageOff)
                    .small()
                    .text_color(theme.muted_foreground)
            };
            let (source2, gallery) = (source.clone(), gallery.clone());
            Some(
                div()
                    .id(("note-thumb", i))
                    .test_support()
                    .relative()
                    .flex_none()
                    .w(width)
                    .h(px(TILE))
                    .rounded(px(8.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.foreground.opacity(0.03))
                    .cursor_pointer()
                    .hover(|s| s.border_color(theme.foreground.opacity(0.35)))
                    .when_some(picture, |el, src| {
                        let fallback_color = theme.muted_foreground;
                        el.child(img(src).size_full().object_fit(fit).with_fallback(move || {
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(crate::assets::Lucide::ImageOff)
                                        .small()
                                        .text_color(fallback_color),
                                )
                                .into_any_element()
                        }))
                    })
                    .when(broken, |el| {
                        el.flex()
                            .items_center()
                            .justify_center()
                            .border_dashed()
                            .child(missing_icon())
                    })
                    .when_some(
                        source.path().filter(|_| !broken).map(Path::to_path_buf),
                        |el, path| el.child(crate::image_preview::thumb(path)),
                    )
                    .on_click(move |_, window, cx| open(&source2, &gallery, window, cx))
                    .into_any_element(),
            )
        });
    Some(
        h_flex()
            .id("note-images")
            .test_support()
            .flex_none()
            .w_full()
            .px(px(28.))
            .py(px(10.))
            .gap(px(8.))
            .overflow_x_scroll()
            .border_t_1()
            .border_color(theme.foreground.opacity(0.06))
            .children(tiles)
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::{Source, density_at, fit, gallery, png_dpi};
    use gpui_kit::{px, size};

    fn png_with_dpi(dpi: Option<u32>) -> Vec<u8> {
        let mut img = image::RgbaImage::new(4, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let mut bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let Some(dpi) = dpi else { return bytes };
        // A pHYs chunk right after IHDR (8 + 25 bytes in), as macOS writes one.
        let per_metre = (dpi as f32 / 0.0254).round() as u32;
        let mut data = Vec::new();
        data.extend(per_metre.to_be_bytes());
        data.extend(per_metre.to_be_bytes());
        data.push(1);
        let mut chunk = Vec::new();
        chunk.extend(9u32.to_be_bytes());
        chunk.extend(b"pHYs");
        chunk.extend(&data);
        chunk.extend([0, 0, 0, 0]);
        bytes.splice(33..33, chunk);
        bytes
    }

    #[test]
    fn a_retina_screenshot_is_drawn_at_its_size_on_screen() {
        let dir = std::env::temp_dir().join(format!("trek-note-images-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let retina = dir.join("retina.png");
        std::fs::write(&retina, png_with_dpi(Some(144))).unwrap();
        let plain = dir.join("plain.png");
        std::fs::write(&plain, png_with_dpi(None)).unwrap();
        assert!((png_dpi(&retina).unwrap() - 144.).abs() < 0.5);
        assert_eq!(png_dpi(&plain), None);
        assert_eq!(
            (
                density_at(72.),
                density_at(96.),
                density_at(144.),
                density_at(216.),
                density_at(600.)
            ),
            (1., 1., 2., 3., 3.)
        );
        let Some(Source::File {
            density, pixels, ..
        }) = Source::of("retina.png", &dir)
        else {
            panic!("a file")
        };
        assert_eq!((density, pixels), (2., (4, 2)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_picture_on_its_own_line_reads_as_its_own_paragraph() {
        use super::for_reading;
        assert_eq!(
            for_reading("Trip\n![a](x.png)\nafter"),
            "Trip\n\n![a](x.png)\n\nafter"
        );
        assert_eq!(
            for_reading("![a](x.png)\n![b](y.png)"),
            "![a](x.png)\n\n![b](y.png)"
        );
        // Already apart, inline, in a list or in code: as written.
        assert_eq!(
            for_reading("Trip\n\n![a](x.png)\n\nafter"),
            "Trip\n\n![a](x.png)\n\nafter"
        );
        assert_eq!(
            for_reading("see ![a](x.png) here\nnext"),
            "see ![a](x.png) here\nnext"
        );
        assert_eq!(for_reading("- ![a](x.png)\n- b"), "- ![a](x.png)\n- b");
        assert_eq!(
            for_reading("```\n![a](x.png)\ncode\n```"),
            "```\n![a](x.png)\ncode\n```"
        );
    }

    #[test]
    fn a_download_is_known_by_its_content() {
        use gpui_kit::ImageFormat;
        assert_eq!(
            super::web_format(&png_with_dpi(None)),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            super::web_format(
                b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>"
            ),
            Some(ImageFormat::Svg)
        );
        assert_eq!(
            super::web_format(b"<!doctype html><title>Not found</title>"),
            None,
            "an error page isn't a picture"
        );
    }

    #[test]
    fn svgs_are_asked_for_only_past_the_renderers_own_scale() {
        // GPUI's renderer already draws two pixels a point.
        assert_eq!(
            (
                super::svg_scale(1.),
                super::svg_scale(2.),
                super::svg_scale(3.)
            ),
            (100, 100, 150)
        );
    }

    #[test]
    fn images_fit_the_column_in_their_proportions() {
        // Small ones as they are; wide ones down to the column; tall ones down to the cap.
        assert_eq!(
            fit(size(px(200.), px(100.)), px(600.)),
            size(px(200.), px(100.))
        );
        assert_eq!(
            fit(size(px(1440.), px(900.)), px(600.)),
            size(px(600.), px(375.))
        );
        let tall = fit(size(px(400.), px(4000.)), px(600.));
        assert_eq!(tall.height, px(super::TALLEST));
        assert_eq!(tall.width, px(72.));
    }

    #[test]
    fn links_in_a_note_are_sorted_by_where_they_lead() {
        let dir = std::env::temp_dir().join(format!("trek-note-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("attachments")).unwrap();
        std::fs::write(
            dir.join("attachments/a.svg"),
            "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'/>",
        )
        .unwrap();
        std::fs::write(dir.join("attachments/bad.png"), b"not a png").unwrap();
        std::fs::write(dir.join("attachments/photo.heic"), b"heic").unwrap();
        std::fs::write(dir.join("My Pic.png"), png_with_dpi(None)).unwrap();
        assert!(matches!(
            Source::of("attachments/a.svg", &dir),
            Some(Source::Svg(_))
        ));
        assert!(matches!(
            Source::of("attachments/bad.png", &dir),
            Some(Source::Unreadable(_))
        ));
        assert!(matches!(
            Source::of("attachments/photo.heic", &dir),
            Some(Source::Unreadable(_))
        ));
        assert!(
            matches!(Source::of("attachments/gone.png", &dir), Some(Source::Missing(l)) if l == "attachments/gone.png")
        );
        assert!(matches!(
            Source::of("https://example.com/a.png", &dir),
            Some(Source::Remote(_))
        ));
        assert!(matches!(
            Source::of("My%20Pic.png", &dir),
            Some(Source::File { .. })
        ));
        assert!(matches!(
            Source::of("My Pic.png", &dir),
            Some(Source::File { .. })
        ));
        assert_eq!(
            Source::of("data:image/png;base64,xyz", &dir),
            None,
            "the markdown view draws those itself"
        );
        let abs = dir.join("My Pic.png");
        assert!(matches!(
            Source::of(&abs.to_string_lossy(), &dir),
            Some(Source::File { .. })
        ));
        // The viewer steps through what's on disk, each once.
        let body = "![](attachments/a.svg)\n![](https://example.com/a.png)\n![](attachments/gone.png)\n![](My%20Pic.png)\n![](attachments/a.svg)";
        assert_eq!(
            gallery(body, &dir),
            [dir.join("attachments/a.svg"), dir.join("My Pic.png")]
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
