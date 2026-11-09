//! Images and files going out with the next message: what a paste attaches instead of inserting
//! as text, saving clipboard image data (and converting image files agents can't read) to disk,
//! and the strip above the prompt: thumbnails of the images (a click previews them; see
//! `image_preview`), and a tile for each other file (a video's poster frame, or the file's icon,
//! with its name and size). Files are mentioned after the message as `@path` as it goes
//! (`trek_core::media::split_attached` reads them back off it).

use crate::fluent::{self, PillExt as _};
use crate::mentions;
use crate::scale::{radius, text};
use crate::workspace::WorkspaceEvent;
use anyhow::{Context as _, Result, bail};
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};

/// Images and files going out with a view's next message.
#[derive(Debug, Default)]
pub struct Outbox {
    /// Images, sent as images.
    pub paths: Vec<PathBuf>,
    /// Other files and folders (videos, PDFs…), as tiles beside the images until the message
    /// goes, when they're mentioned after it as `@path`.
    pub files: Vec<PathBuf>,
    /// Images still being saved or converted.
    pub saving: usize,
    /// The user sent while some were still being saved: the send goes once they land, if the
    /// view still shows where it was sent (`Attaching::target`).
    held: Option<String>,
}

impl Outbox {
    /// Attach `paths`, skipping ones already attached.
    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if !self.paths.contains(&path) {
                self.paths.push(path);
            }
        }
    }

    /// Attach other files as tiles, skipping ones already attached.
    pub fn add_files(&mut self, files: impl IntoIterator<Item = PathBuf>) {
        for file in files {
            if !self.files.contains(&file) {
                self.files.push(file);
            }
        }
    }

    /// Take `path` back out (its thumbnail's or tile's ×, or Remove in the preview).
    pub fn remove(&mut self, path: &Path) {
        self.paths.retain(|p| p != path);
        self.files.retain(|p| p != path);
    }

    /// Nothing attached.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.files.is_empty()
    }

    /// Hold a send to `target` while images are still being saved (⌘V then Return straight
    /// away), so it doesn't go out without them. Returns whether it was held.
    pub fn hold_send(&mut self, target: String) -> bool {
        if self.saving > 0 {
            self.held = Some(target);
        }
        self.held.is_some()
    }

    /// `n` images finished saving: attach the ones that made it. Returns a message for the first
    /// failure, and where a held send that can go now was meant for. A failure cancels the held
    /// send, so the user can decide whether to send without that image.
    fn landed(
        &mut self,
        n: usize,
        results: Vec<(String, Result<PathBuf>)>,
    ) -> (Option<String>, Option<String>) {
        self.saving = self.saving.saturating_sub(n);
        let mut failed = None;
        for (label, result) in results {
            match result {
                Ok(path) => self.add([path]),
                Err(e) => {
                    failed.get_or_insert(format!("Couldn't attach {label}: {e}."));
                }
            }
        }
        if failed.is_some() {
            self.held = None;
        }
        let send = if self.saving == 0 {
            self.held.take()
        } else {
            None
        };
        (failed, send)
    }
}

/// A view that sends images with its next message (the composer, the side chat).
pub trait Attaching: Sized + 'static {
    fn outbox(&mut self) -> &mut Outbox;
    /// Where a message sent now would go (a thread, or a new one in a project), to tell whether a
    /// held send still goes where it was sent.
    fn target(&self, cx: &App) -> String;
    /// Send the message that waited for its images to be saved.
    fn send_held(&mut self, window: &mut Window, cx: &mut Context<Self>);
    /// Images that finished saving after the view moved on from `target` (the one they were
    /// pasted into). A view that keeps a draft per target puts them back in that draft; the
    /// default attaches them to what it shows now.
    fn land_elsewhere(&mut self, _target: &str, paths: Vec<PathBuf>) {
        self.outbox().add(paths);
    }
    /// Pasted files that aren't images go on the strip as tiles (`Outbox::files`); a view that
    /// doesn't draw them has them mentioned in its text where they're pasted.
    fn tiles_files(&self) -> bool {
        false
    }
}

/// A paste a composer takes over from its textarea.
#[derive(Debug, Clone, PartialEq)]
pub enum Pasted {
    /// Files copied in Finder: images to attach, and the rest to mention with `@`.
    Files {
        images: Vec<PathBuf>,
        others: Vec<PathBuf>,
    },
    /// Image data (a screenshot copied to the clipboard), saved to disk before attaching.
    Images(Vec<Image>),
}

/// Image files agents can't read as they are (HEIC photos, TIFF, BMP): a PNG copy is attached.
pub fn needs_conversion(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("heic" | "heif" | "tif" | "tiff" | "bmp")
    )
}

/// Split dropped, picked or pasted files into images to attach and the rest, to mention with `@`.
pub fn split_files(paths: impl IntoIterator<Item = PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    paths
        .into_iter()
        .partition(|p| mentions::is_image(p) || needs_conversion(p))
}

/// What to do with a clipboard item. `None` when it carries no image: the textarea pastes its
/// text as usual (copied non-image files paste as their names, as before).
pub fn pasted(item: &ClipboardItem) -> Option<Pasted> {
    let paths: Vec<PathBuf> = item
        .entries()
        .iter()
        .filter_map(|e| {
            if let ClipboardEntry::ExternalPaths(p) = e {
                Some(p.paths().to_vec())
            } else {
                None
            }
        })
        .flatten()
        .collect();
    if !paths.is_empty() {
        let (images, others) = split_files(paths);
        return (!images.is_empty()).then_some(Pasted::Files { images, others });
    }
    let images: Vec<Image> = item
        .entries()
        .iter()
        .filter_map(|e| {
            if let ClipboardEntry::Image(i) = e {
                Some(i.clone())
            } else {
                None
            }
        })
        .collect();
    (!images.is_empty()).then_some(Pasted::Images(images))
}

pub fn timestamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S-%3f").to_string()
}

/// Write clipboard image data into `dir` as a file the agents accept. PNG, JPEG, GIF and WebP
/// are kept as they are; TIFF, BMP and the like (what some apps put on the clipboard) become PNG.
pub fn save_image(image: &Image, dir: &Path) -> Result<PathBuf> {
    if image.format() == ImageFormat::Svg {
        bail!("SVG isn't supported, paste a PNG or JPEG instead");
    }
    std::fs::create_dir_all(dir)
        .with_context(|| format!("{} couldn't be created", dir.display()))?;
    let stem = format!("pasted-{}-{:08x}", timestamp(), image.id() as u32);
    let (ext, bytes) = image_bytes(image)?;
    let path = dir.join(format!("{stem}.{ext}"));
    std::fs::write(&path, bytes)
        .with_context(|| format!("{} couldn't be written", path.display()))?;
    Ok(path)
}

/// Clipboard image data as a file's kind and bytes: PNG, JPEG, GIF, WebP and SVG as they are,
/// TIFF, BMP and the like converted to PNG.
pub fn image_bytes(image: &Image) -> Result<(&'static str, Vec<u8>)> {
    Ok(match image.format() {
        ImageFormat::Png
        | ImageFormat::Jpeg
        | ImageFormat::Gif
        | ImageFormat::Webp
        | ImageFormat::Svg => (image.format().extension(), image.bytes().to_vec()),
        other => {
            let format = match other {
                ImageFormat::Tiff => image::ImageFormat::Tiff,
                ImageFormat::Bmp => image::ImageFormat::Bmp,
                ImageFormat::Ico => image::ImageFormat::Ico,
                _ => image::ImageFormat::Pnm,
            };
            let decoded = image::load_from_memory_with_format(image.bytes(), format)
                .context("its data couldn't be read")?;
            let mut png = Vec::new();
            decoded
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .context("it couldn't be converted to PNG")?;
            ("png", png)
        }
    })
}

/// Copy an image file agents can't read (see `needs_conversion`) into `dir` as PNG. `sips` is
/// asked first (it reads HEIC, which the `image` crate doesn't); where there is no `sips`
/// (Windows), or it turned the file down, the `image` crate reads TIFF and BMP.
pub fn convert_file(path: &Path, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("{} couldn't be created", dir.display()))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".into());
    let out = dir.join(format!("{stem}-{}.png", timestamp()));
    convert_to(path, &out)?;
    Ok(out)
}

/// `path` as a PNG at `out` (see `convert_file`).
pub fn convert_to(path: &Path, out: &Path) -> Result<()> {
    let converted = sips_png(path, out) || image_crate_png(path, out);
    if !converted || !out.is_file() {
        let _ = std::fs::remove_file(out);
        bail!("it couldn't be converted to PNG");
    }
    Ok(())
}

/// `path` as PNG at `out`, by macOS's own `sips` (HEIC included). There is none off macOS.
fn sips_png(path: &Path, out: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::process::Stdio;
        std::process::Command::new("/usr/bin/sips")
            .args(["-s", "format", "png"])
            .arg(path)
            .arg("--out")
            .arg(out)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, out);
        false
    }
}

/// `path` as PNG at `out`, by the `image` crate (TIFF, BMP and the like, not HEIC).
fn image_crate_png(path: &Path, out: &Path) -> bool {
    image::open(path)
        .and_then(|decoded| decoded.save_with_format(out, image::ImageFormat::Png))
        .is_ok()
}

/// An image to write into Trek's snapshots folder before it can be attached.
enum Job {
    Data(Image),
    File(PathBuf),
}

impl Job {
    fn run(&self, dir: &Path) -> Result<PathBuf> {
        match self {
            Job::Data(image) => save_image(image, dir),
            Job::File(path) => convert_file(path, dir),
        }
    }

    /// How a failure names it.
    fn label(&self) -> String {
        match self {
            Job::Data(_) => "the pasted image".into(),
            Job::File(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string()),
        }
    }
}

/// Write `jobs` into Trek's snapshots folder off the main thread, then attach them (and send a
/// message that was waiting for them).
fn save<T: Attaching>(this: &mut T, jobs: Vec<Job>, window: &mut Window, cx: &mut Context<T>) {
    if jobs.is_empty() {
        return;
    }
    let n = jobs.len();
    this.outbox().saving += n;
    let meant_for = this.target(cx);
    cx.spawn_in(window, async move |view, cx| {
        let dir = trek_core::paths::data_dir().join("snapshots");
        let results = cx
            .background_executor()
            .spawn(async move {
                jobs.iter()
                    .map(|j| (j.label(), j.run(&dir)))
                    .collect::<Vec<_>>()
            })
            .await;
        let _ = view.update_in(cx, |this, window, cx| {
            // The user moved to another thread while these were being saved: they belong to
            // the one they were pasted into.
            let mut away = vec![];
            let results = if meant_for == this.target(cx) {
                results
            } else {
                results
                    .into_iter()
                    .filter_map(|(label, result)| match result {
                        Ok(path) => {
                            away.push(path);
                            None
                        }
                        Err(e) => Some((label, Err(e))),
                    })
                    .collect()
            };
            if !away.is_empty() {
                this.land_elsewhere(&meant_for, away);
            }
            settle(this, n, results, window, cx);
        });
    })
    .detach();
}

/// `n` of the images being saved are done: attach the ones in `results`, say what failed, and
/// send a message that waited for them once nothing is left.
fn settle<T: Attaching>(
    this: &mut T,
    n: usize,
    results: Vec<(String, Result<PathBuf>)>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let (failed, send) = this.outbox().landed(n, results);
    if let Some(message) = failed {
        crate::workspace::workspace_global(cx).update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::Toast {
                message,
                undo: None,
            })
        });
    }
    // Sent elsewhere than the view now shows (the user moved on meanwhile): it stays in the
    // composer rather than go somewhere it wasn't meant for.
    if send.is_some_and(|target| target == this.target(cx)) {
        this.send_held(window, cx);
    }
    cx.notify();
}

/// `n` files are on their way from somewhere slow (the Photos picker copies each out of the
/// library first): the strip shows them as saving, and a send waits for them (`arrived`).
pub fn expect<T: Attaching>(this: &mut T, n: usize, cx: &mut Context<T>) {
    this.outbox().saving += n;
    cx.notify();
}

/// The `n` files `expect` announced are in, already put where they go by the caller: say which
/// couldn't be had (`failed`: each one's name and why), and send a message that waited for them.
pub fn arrived<T: Attaching>(
    this: &mut T,
    n: usize,
    failed: Vec<(String, anyhow::Error)>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let results = failed
        .into_iter()
        .map(|(label, e)| (label, Err(e)))
        .collect();
    settle(this, n, results, window, cx);
}

/// Attach image files (dropped, picked or pasted): ones agents read go on at once, the rest
/// (HEIC, TIFF, BMP) once a PNG copy is made.
pub fn attach_files<T: Attaching>(
    this: &mut T,
    images: Vec<PathBuf>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let (convert, ready): (Vec<PathBuf>, Vec<PathBuf>) =
        images.into_iter().partition(|p| needs_conversion(p));
    this.outbox().add(ready);
    save(
        this,
        convert.into_iter().map(Job::File).collect(),
        window,
        cx,
    );
    cx.notify();
}

/// Apply a paste the textarea handed over to the view that owns `input`: image files attach,
/// other copied files are mentioned with `@`, and image data is attached once it's saved to
/// Trek's snapshots folder (off the main thread).
pub fn paste<T: Attaching>(
    this: &mut T,
    pasted: Pasted,
    input: &Entity<TextareaState>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    match pasted {
        Pasted::Files { images, others } => {
            attach_files(this, images, window, cx);
            if this.tiles_files() {
                this.outbox().add_files(others);
            } else if !others.is_empty() {
                let refs: String = others
                    .iter()
                    .map(|p| {
                        format!(
                            "{} ",
                            trek_core::media::mention_of(&p.display().to_string())
                        )
                    })
                    .collect();
                input.update(cx, |s, cx| {
                    let value = s.value().to_string();
                    let cursor = s.cursor().min(value.len());
                    let word_start = value.get(..cursor).is_none_or(|before| {
                        before.is_empty() || before.ends_with(char::is_whitespace)
                    });
                    s.insert(
                        if word_start { refs } else { format!(" {refs}") },
                        window,
                        cx,
                    )
                });
            }
        }
        Pasted::Images(images) => save(
            this,
            images.into_iter().map(Job::Data).collect(),
            window,
            cx,
        ),
    }
    cx.notify();
}

/// Thumbnails of `paths` (`size` square) with a remove button on hover, plus a spinner tile while
/// `busy` (a snapshot or paste is still being saved). A click previews the outbox from that image
/// (its name is in the preview's header, so no tooltip: one would stay up over the preview), where
/// Remove takes it out as the × does (`on_remove`).
pub fn thumbnails(
    paths: &[PathBuf],
    size: Pixels,
    busy: bool,
    on_remove: impl Fn(&Path, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    strip(paths, &[], size, busy, on_remove, cx)
}

/// `thumbnails` of the images, then a tile for each of `files` (`file_tile`), as tall as they are.
pub fn strip(
    paths: &[PathBuf],
    files: &[PathBuf],
    size: Pixels,
    busy: bool,
    on_remove: impl Fn(&Path, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let radius = px(fluent::control(radius::BUTTON));
    let all: std::rc::Rc<[PathBuf]> = paths.into();
    let tiles: Vec<AnyElement> = files
        .iter()
        .enumerate()
        .map(|(i, f)| file_tile(i, f, size, on_remove.clone(), cx))
        .collect();
    h_flex()
        .gap_2()
        .flex_wrap()
        .children(paths.iter().enumerate().map(|(i, p)| {
            let (remove, preview_remove) = (on_remove.clone(), on_remove.clone());
            let (path, all) = (p.clone(), all.clone());
            div()
                .id(("attachment", i))
                .test_support()
                .group("att")
                .relative()
                .flex_none()
                .size(size)
                .rounded(radius)
                .overflow_hidden()
                .border_1()
                .border_color(theme.border)
                .cursor_pointer()
                .hover(|s| s.border_color(theme.foreground.opacity(0.35)))
                .child(img(p.clone()).size_full().object_fit(ObjectFit::Cover))
                .child(crate::image_preview::thumb(p.clone()))
                // A faint lift under the pointer: it opens.
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(gpui_kit::white().opacity(0.08))
                        .invisible()
                        .group_hover("att", |s| s.visible()),
                )
                .on_click(move |_, window, cx| {
                    crate::image_preview::open_outbox(
                        all.to_vec(),
                        i,
                        preview_remove.clone(),
                        window,
                        cx,
                    )
                })
                .child(
                    div()
                        .id(("att-x", i))
                        .test_support()
                        .absolute()
                        .top(px(3.))
                        .right(px(3.))
                        .size(px(18.))
                        .rounded_pill()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui_kit::black().opacity(0.65))
                        .invisible()
                        .group_hover("att", |s| s.visible())
                        .cursor_pointer()
                        .child(
                            Icon::new(IconName::Close)
                                .xsmall()
                                .text_color(gpui_kit::white()),
                        )
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            remove(&path, window, cx);
                        }),
                )
        }))
        .children(tiles)
        .when(busy, |el| {
            el.child(
                div()
                    .flex_none()
                    .size(size)
                    .rounded(radius)
                    .border_1()
                    .border_dashed()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Spinner::new().small().color(theme.muted_foreground)),
            )
        })
        .into_any_element()
}

/// A file going with the message that isn't an image, `size` tall: a video's poster frame with a
/// play badge (the Mac's; its icon elsewhere, or until the frame is made), or the file's or
/// folder's icon; then its name, cut in the middle so its kind still shows, and its kind and size.
/// The full path is its tooltip. A click opens it (Quick Look on the Mac, its app on Windows, a
/// folder in the file manager); the × on hover takes it off (`on_remove`).
fn file_tile(
    i: usize,
    path: &Path,
    size: Pixels,
    on_remove: impl Fn(&Path, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let radius = px(fluent::control(radius::BUTTON));
    let name = file_name(path);
    let shown = path.to_string_lossy().to_string();
    let folder = path.is_dir();
    let video = !folder && trek_core::media::is_video_name(&name);
    let icon = if folder {
        crate::file_icon::folder(&shown, false, px(22.), cx)
    } else {
        crate::file_icon::badge(&shown, px(22.), cx)
    };
    let preview = div()
        .relative()
        .flex_none()
        .size(size)
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.muted)
        .child(icon)
        .when(video, |el| {
            let poster = path.to_path_buf();
            // Over the icon, which shows until the frame is made (or where it can't be).
            el.child(
                img(move |window: &mut Window, cx: &mut App| {
                    window.use_asset::<Poster>(&poster, cx)
                })
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .object_fit(ObjectFit::Cover)
                .with_fallback(|| div().into_any_element()),
            )
            .child(
                div()
                    .absolute()
                    .size(px(20.))
                    .rounded_pill()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui_kit::black().opacity(0.55))
                    .child(
                        Icon::new(crate::assets::Lucide::Play)
                            .size(px(10.))
                            .text_color(gpui_kit::white()),
                    ),
            )
        });
    let (open, remove_path) = (path.to_path_buf(), path.to_path_buf());
    div()
        .id(("attachment-file", i))
        .test_support()
        .group("att")
        .relative()
        .flex_none()
        .h(size)
        .max_w(px(240.))
        .rounded(radius)
        .overflow_hidden()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .cursor_pointer()
        .hover(|s| s.border_color(theme.foreground.opacity(0.35)))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(shown.clone()).build(window, cx)
        })
        .child(
            h_flex().h_full().min_w_0().child(preview).child(
                gpui_kit::component::v_flex()
                    .min_w_0()
                    .pl(px(10.))
                    .pr(px(26.))
                    .gap(px(2.))
                    .child(
                        div()
                            .whitespace_nowrap()
                            .text_size(px(text::LABEL))
                            .text_color(theme.foreground)
                            .child(middle_cut(&name, 26)),
                    )
                    .child(
                        div()
                            .whitespace_nowrap()
                            .text_size(px(text::META))
                            .text_color(theme.muted_foreground)
                            .child(kind_and_size(path, folder)),
                    ),
            ),
        )
        .on_click(move |_, window, cx| {
            if open.is_dir() {
                cx.open_with_system(&open);
            } else {
                crate::image_preview::open(vec![open.clone()], 0, window, cx);
            }
        })
        .child(
            div()
                .id(("att-file-x", i))
                .test_support()
                .absolute()
                .top(px(3.))
                .right(px(3.))
                .size(px(18.))
                .rounded_pill()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::black().opacity(0.65))
                .invisible()
                .group_hover("att", |s| s.visible())
                .cursor_pointer()
                .child(
                    Icon::new(IconName::Close)
                        .xsmall()
                        .text_color(gpui_kit::white()),
                )
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    on_remove(&remove_path, window, cx);
                }),
        )
        .into_any_element()
}

/// A file sent with a message, under it in the transcript: its icon and name in a chip of its
/// type's colour, the full path as its tooltip; a click opens it as a tile's does.
pub fn sent_file_chip(id: impl Into<ElementId>, path: &Path, cx: &App) -> AnyElement {
    let shown = path.to_string_lossy().to_string();
    let name = middle_cut(&file_name(path), 40);
    let open = path.to_path_buf();
    let tip = shown.clone();
    let size = px(text::LABEL);
    let chip = if path.is_dir() {
        let t = crate::file_icon::tint(&shown, cx);
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .h(size * 1.62)
            .px(size * 0.42)
            .gap(size * 0.34)
            // The shape of `file_icon::chip` beside it, a pill scaled to its text.
            .rounded(if cfg!(windows) { px(4.) } else { size * 0.42 }) // scale: ok
            .border_1()
            .border_color(t.edge)
            .bg(t.fill)
            .text_color(t.ink)
            .text_size(size)
            .font_family(cx.theme().mono_font_family.clone())
            .child(crate::file_icon::folder(&shown, false, size * 1.02, cx))
            .child(div().min_w_0().truncate().child(name))
    } else {
        crate::file_icon::chip(id, &shown, name, size, cx)
    };
    chip.test_support()
        .cursor_pointer()
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
        .on_click(move |_, window, cx| {
            if open.is_dir() {
                cx.open_with_system(&open);
            } else {
                crate::image_preview::open(vec![open.clone()], 0, window, cx);
            }
        })
        .into_any_element()
}

/// The last part of `path`, as a tile names it.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// `name` in at most `max` characters: cut in the middle with an ellipsis, keeping its extension
/// and a few characters before it (`Quarterly rep…2026-Q3.pdf`).
pub fn middle_cut(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max || max < 5 {
        return name.to_string();
    }
    let ext = name
        .rfind('.')
        .filter(|&dot| dot > 0)
        .map_or(0, |dot| name[dot..].chars().count());
    let tail = (ext + 4).min(max / 2);
    let head = max - 1 - tail;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// A size as a tile says it: `512 bytes`, `13 KB`, `5.3 MB`, `1.2 GB`.
pub fn size_label(bytes: u64) -> String {
    const K: u64 = 1024;
    match bytes {
        b if b >= K * K * K => format!("{:.1} GB", b as f64 / (K * K * K) as f64),
        b if b >= K * K => format!("{:.1} MB", b as f64 / (K * K) as f64),
        b if b >= K => format!("{} KB", b.div_ceil(K)),
        b => format!("{b} bytes"),
    }
}

/// What a tile says under a file's name: `PDF · 13 KB`, `Folder`, or that it's gone.
fn kind_and_size(path: &Path, folder: bool) -> String {
    if folder {
        return "Folder".into();
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return "Not found".into();
    };
    let size = size_label(meta.len());
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if !ext.is_empty() => format!("{} · {size}", ext.to_uppercase()),
        _ => size,
    }
}

/// A video's poster frame, as Quick Look draws its thumbnail, made once into Trek's cache and read
/// as any image. The Mac's (`qlmanage`); elsewhere, and in tests, there is none, and the tile
/// keeps the file's icon.
pub enum Poster {}

impl Asset for Poster {
    type Source = PathBuf;
    type Output = std::result::Result<std::sync::Arc<RenderImage>, ImageCacheError>;

    fn load(
        video: Self::Source,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let png = poster_file(&video);
        let read = ImageAssetLoader::load(Resource::Path(png.as_path().into()), cx);
        async move {
            make_poster(&video, &png)?;
            read.await
        }
    }
}

/// Where `video`'s poster frame is kept: by its path, size and time changed, so an edited file
/// gets a new one.
fn poster_file(video: &Path) -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    video.hash(&mut h);
    if let Ok(meta) = std::fs::metadata(video) {
        meta.len().hash(&mut h);
        meta.modified().ok().hash(&mut h);
    }
    trek_core::paths::data_dir()
        .join("cache")
        .join("posters")
        .join(format!("{:016x}.png", h.finish()))
}

/// Make `video`'s poster frame at `png`, unless it's there already.
#[cfg(all(target_os = "macos", not(any(test, feature = "test-support"))))]
fn make_poster(video: &Path, png: &Path) -> Result<()> {
    if png.is_file() {
        return Ok(());
    }
    // `qlmanage -t` writes `<name>.png` into the folder it's given: one of its own, then moved.
    let work = png.with_extension("making");
    std::fs::create_dir_all(&work)?;
    let made = std::process::Command::new("/usr/bin/qlmanage")
        .args(["-t", "-s", "256", "-o"])
        .arg(&work)
        .arg(video)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let out = std::fs::read_dir(&work)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "png"));
    let result = match (made, out) {
        (Ok(status), Some(out)) if status.success() => {
            std::fs::rename(&out, png).map_err(anyhow::Error::from)
        }
        _ => Err(anyhow::anyhow!(
            "Quick Look made no frame of {}",
            video.display()
        )),
    };
    let _ = std::fs::remove_dir_all(&work);
    result
}

#[cfg(not(all(target_os = "macos", not(any(test, feature = "test-support")))))]
fn make_poster(video: &Path, _png: &Path) -> Result<()> {
    bail!("no poster frame for {} here", video.display())
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the gpui glob import brings its own `test` attribute.
    use super::{
        Outbox, Pasted, convert_file, middle_cut, needs_conversion, pasted, save_image, size_label,
        split_files,
    };
    use gpui_kit::{ClipboardEntry, ClipboardItem, ExternalPaths, Image, ImageFormat};
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek-paste-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A 2×1 RGBA image encoded as `format`.
    fn encoded(format: image::ImageFormat) -> Vec<u8> {
        let img = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), format)
            .unwrap();
        out
    }

    #[test]
    fn text_pastes_stay_text() {
        assert_eq!(pasted(&ClipboardItem::new_string("hello".into())), None);
        // A design's link (Figma's Copy link) stays a link the agent can hand to Figma's tools.
        assert_eq!(
            pasted(&ClipboardItem::new_string(
                "https://www.figma.com/design/AbC123/App?node-id=1-2".into()
            )),
            None
        );
        // Finder copy of non-image files: their names paste as text, as before.
        let files = ClipboardItem {
            entries: vec![
                ClipboardEntry::ExternalPaths(ExternalPaths(
                    vec![PathBuf::from("/p/src/main.rs")].into(),
                )),
                ClipboardEntry::String(gpui_kit::ClipboardString::new("main.rs".into())),
            ],
        };
        assert_eq!(pasted(&files), None);
    }

    #[test]
    fn copied_image_files_attach_and_other_files_are_mentioned() {
        let item = ClipboardItem {
            entries: vec![
                ClipboardEntry::ExternalPaths(ExternalPaths(
                    vec![
                        PathBuf::from("/d/shot.PNG"),
                        PathBuf::from("/d/notes.md"),
                        PathBuf::from("/d/b.jpeg"),
                    ]
                    .into(),
                )),
                ClipboardEntry::String(gpui_kit::ClipboardString::new(
                    "shot.PNG notes.md b.jpeg".into(),
                )),
            ],
        };
        assert_eq!(
            pasted(&item),
            Some(Pasted::Files {
                images: vec![PathBuf::from("/d/shot.PNG"), PathBuf::from("/d/b.jpeg")],
                others: vec![PathBuf::from("/d/notes.md")]
            })
        );
    }

    #[test]
    fn photos_and_tiffs_from_finder_attach_too() {
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
                vec![
                    PathBuf::from("/d/IMG_1234.HEIC"),
                    PathBuf::from("/d/scan.tiff"),
                    PathBuf::from("/d/a.pdf"),
                ]
                .into(),
            ))],
        };
        assert_eq!(
            pasted(&item),
            Some(Pasted::Files {
                images: vec![
                    PathBuf::from("/d/IMG_1234.HEIC"),
                    PathBuf::from("/d/scan.tiff")
                ],
                others: vec![PathBuf::from("/d/a.pdf")]
            })
        );
        assert!(
            needs_conversion(&PathBuf::from("/d/x.bmp"))
                && !needs_conversion(&PathBuf::from("/d/x.png"))
        );
        let (images, others) = split_files([PathBuf::from("/d/x.webp"), PathBuf::from("/d/x.svg")]);
        assert_eq!(
            (images, others),
            (
                vec![PathBuf::from("/d/x.webp")],
                vec![PathBuf::from("/d/x.svg")]
            )
        );
    }

    #[test]
    fn tiles_cut_long_names_in_the_middle_and_say_sizes_shortly() {
        assert_eq!(middle_cut("notes.md", 26), "notes.md");
        // The kind still shows at the end.
        assert_eq!(
            middle_cut("ScreenRecording_11-06-2026-17-49-53_1.mp4", 26),
            "ScreenRecording_1…53_1.mp4"
        );
        assert_eq!(
            middle_cut("ScreenRecording_11-06-2026-17-49-53_1.mp4", 26)
                .chars()
                .count(),
            26
        );
        assert_eq!(
            middle_cut("a-very-long-name-without-an-extension", 20),
            "a-very-long-nam…sion"
        );
        assert_eq!(
            middle_cut("Été à Paris, le grand voyage.mov", 20),
            "Été à Paris…yage.mov"
        );
        assert_eq!(size_label(512), "512 bytes");
        assert_eq!(size_label(13_779), "14 KB");
        assert_eq!(size_label(5_577_009), "5.3 MB");
        assert_eq!(size_label(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    #[test]
    fn files_go_beside_the_images_and_come_off_with_their_x() {
        let mut outbox = Outbox::default();
        assert!(outbox.is_empty());
        outbox.add_files([PathBuf::from("/d/clip.mov"), PathBuf::from("/d/a.pdf")]);
        outbox.add_files([PathBuf::from("/d/clip.mov")]);
        outbox.add([PathBuf::from("/d/shot.png")]);
        assert_eq!(
            outbox.files,
            vec![PathBuf::from("/d/clip.mov"), PathBuf::from("/d/a.pdf")]
        );
        outbox.remove(&PathBuf::from("/d/clip.mov"));
        outbox.remove(&PathBuf::from("/d/shot.png"));
        assert_eq!(outbox.files, vec![PathBuf::from("/d/a.pdf")]);
        assert!(outbox.paths.is_empty() && !outbox.is_empty());
    }

    #[test]
    fn a_send_waits_for_images_still_being_saved() {
        let mut outbox = Outbox::default();
        // Nothing being saved: the send goes now.
        assert!(!outbox.hold_send("thread:a".into()));
        outbox.saving = 2;
        assert!(outbox.hold_send("thread:a".into()));
        // The first image lands; the second is still out, so the send keeps waiting.
        assert_eq!(
            outbox.landed(
                1,
                vec![("the pasted image".into(), Ok(PathBuf::from("/s/a.png")))]
            ),
            (None, None)
        );
        // Once all are in, it goes where it was sent.
        assert_eq!(
            outbox.landed(
                1,
                vec![("the pasted image".into(), Ok(PathBuf::from("/s/b.png")))]
            ),
            (None, Some("thread:a".into()))
        );
        assert_eq!(
            outbox.paths,
            vec![PathBuf::from("/s/a.png"), PathBuf::from("/s/b.png")]
        );
        // Once sent, a later paste doesn't send by itself.
        outbox.saving = 1;
        assert_eq!(
            outbox.landed(
                1,
                vec![("the pasted image".into(), Ok(PathBuf::from("/s/c.png")))]
            ),
            (None, None)
        );
    }

    #[test]
    fn a_failed_image_cancels_the_waiting_send() {
        let mut outbox = Outbox {
            saving: 1,
            ..Default::default()
        };
        assert!(outbox.hold_send("thread:a".into()));
        let (failed, send) = outbox.landed(
            1,
            vec![(
                "IMG_1.HEIC".into(),
                Err(anyhow::anyhow!("it couldn't be converted to PNG")),
            )],
        );
        assert_eq!(
            failed.as_deref(),
            Some("Couldn't attach IMG_1.HEIC: it couldn't be converted to PNG.")
        );
        assert!(send.is_none() && outbox.paths.is_empty() && outbox.saving == 0);
        assert!(!outbox.hold_send("thread:a".into()));
    }

    #[test]
    fn converts_image_files_agents_cant_read() {
        let dir = temp_dir("convert");
        std::fs::create_dir_all(&dir).unwrap();
        let tiff = dir.join("scan.tiff");
        std::fs::write(&tiff, encoded(image::ImageFormat::Tiff)).unwrap();
        let out = convert_file(&tiff, &dir.join("snapshots")).unwrap();
        assert!(
            out.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("scan-")
        );
        let back = image::open(&out).unwrap().to_rgba8();
        assert_eq!((back.width(), back.height()), (2, 1));
        assert!(crate::mentions::is_image(&out));

        let bad = dir.join("broken.heic");
        std::fs::write(&bad, b"not an image").unwrap();
        assert_eq!(
            convert_file(&bad, &dir.join("snapshots"))
                .unwrap_err()
                .to_string(),
            "it couldn't be converted to PNG"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn image_data_is_taken() {
        let image = Image::from_bytes(ImageFormat::Png, encoded(image::ImageFormat::Png));
        assert_eq!(
            pasted(&ClipboardItem::new_image(&image)),
            Some(Pasted::Images(vec![image]))
        );
    }

    #[test]
    fn saves_png_as_is_and_converts_tiff() {
        let dir = temp_dir("save");
        let png = encoded(image::ImageFormat::Png);
        let saved = save_image(&Image::from_bytes(ImageFormat::Png, png.clone()), &dir).unwrap();
        assert_eq!(saved.extension().and_then(|e| e.to_str()), Some("png"));
        assert_eq!(std::fs::read(&saved).unwrap(), png);

        let tiff = save_image(
            &Image::from_bytes(ImageFormat::Tiff, encoded(image::ImageFormat::Tiff)),
            &dir,
        )
        .unwrap();
        assert_eq!(tiff.extension().and_then(|e| e.to_str()), Some("png"));
        let back = image::open(&tiff).unwrap().to_rgba8();
        assert_eq!(
            (back.width(), back.height(), back.get_pixel(1, 0).0),
            (2, 1, [0, 0, 255, 255])
        );
        assert!(crate::mentions::is_image(&tiff));

        assert!(
            save_image(
                &Image::from_bytes(ImageFormat::Svg, b"<svg/>".to_vec()),
                &dir
            )
            .is_err()
        );
        let bad = save_image(
            &Image::from_bytes(ImageFormat::Tiff, b"not a tiff".to_vec()),
            &dir,
        )
        .unwrap_err();
        assert_eq!(bad.to_string(), "its data couldn't be read");
        let _ = std::fs::remove_dir_all(dir);
    }
}
