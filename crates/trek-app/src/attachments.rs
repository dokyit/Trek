//! Images going out with the next message: what a paste attaches instead of inserting as text,
//! saving clipboard image data (and converting image files agents can't read) to disk, and the
//! thumbnail strip above the prompt.

use crate::mentions;
use crate::workspace::WorkspaceEvent;
use anyhow::{Context as _, Result, bail};
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Images going out with a view's next message.
#[derive(Debug, Default)]
pub struct Outbox {
    pub paths: Vec<PathBuf>,
    /// Images still being saved or converted.
    pub saving: usize,
    /// The user sent while some were still being saved: the send goes once they land.
    held: bool,
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

    /// Hold a send while images are still being saved (⌘V then Return straight away), so it
    /// doesn't go out without them. Returns whether it was held.
    pub fn hold_send(&mut self) -> bool {
        self.held |= self.saving > 0;
        self.held
    }

    /// `n` images finished saving: attach the ones that made it. Returns a message for the first
    /// failure, and whether a held send can go now. A failure cancels the held send, so the user
    /// can decide whether to send without that image.
    fn landed(&mut self, n: usize, results: Vec<(String, Result<PathBuf>)>) -> (Option<String>, bool) {
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
            self.held = false;
        }
        let send = self.held && self.saving == 0;
        if send {
            self.held = false;
        }
        (failed, send)
    }
}

/// A view that sends images with its next message (the composer, the side chat).
pub trait Attaching: Sized + 'static {
    fn outbox(&mut self) -> &mut Outbox;
    /// Send the message that waited for its images to be saved.
    fn send_held(&mut self, window: &mut Window, cx: &mut Context<Self>);
}

/// A paste a composer takes over from its textarea.
#[derive(Debug, Clone, PartialEq)]
pub enum Pasted {
    /// Files copied in Finder: images to attach, and the rest to mention with `@`.
    Files { images: Vec<PathBuf>, others: Vec<PathBuf> },
    /// Image data (a screenshot copied to the clipboard), saved to disk before attaching.
    Images(Vec<Image>),
}

/// Image files agents can't read as they are (HEIC photos, TIFF, BMP): a PNG copy is attached.
pub fn needs_conversion(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(), Some("heic" | "heif" | "tif" | "tiff" | "bmp"))
}

/// Split dropped, picked or pasted files into images to attach and the rest, to mention with `@`.
pub fn split_files(paths: impl IntoIterator<Item = PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    paths.into_iter().partition(|p| mentions::is_image(p) || needs_conversion(p))
}

/// What to do with a clipboard item. `None` when it carries no image: the textarea pastes its
/// text as usual (copied non-image files paste as their names, as before).
pub fn pasted(item: &ClipboardItem) -> Option<Pasted> {
    let paths: Vec<PathBuf> = item
        .entries()
        .iter()
        .filter_map(|e| if let ClipboardEntry::ExternalPaths(p) = e { Some(p.paths().to_vec()) } else { None })
        .flatten()
        .collect();
    if !paths.is_empty() {
        let (images, others) = split_files(paths);
        return (!images.is_empty()).then_some(Pasted::Files { images, others });
    }
    let images: Vec<Image> = item.entries().iter().filter_map(|e| if let ClipboardEntry::Image(i) = e { Some(i.clone()) } else { None }).collect();
    (!images.is_empty()).then_some(Pasted::Images(images))
}

fn timestamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S-%3f").to_string()
}

/// Write clipboard image data into `dir` as a file the agents accept. PNG, JPEG, GIF and WebP
/// are kept as they are; TIFF, BMP and the like (what some apps put on the clipboard) become PNG.
pub fn save_image(image: &Image, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("{} couldn't be created", dir.display()))?;
    let stem = format!("pasted-{}-{:08x}", timestamp(), image.id() as u32);
    let (ext, bytes) = match image.format() {
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::Webp => (image.format().extension(), image.bytes().to_vec()),
        ImageFormat::Svg => bail!("SVG isn't supported, paste a PNG or JPEG instead"),
        other => {
            let format = match other {
                ImageFormat::Tiff => image::ImageFormat::Tiff,
                ImageFormat::Bmp => image::ImageFormat::Bmp,
                ImageFormat::Ico => image::ImageFormat::Ico,
                _ => image::ImageFormat::Pnm,
            };
            let decoded = image::load_from_memory_with_format(image.bytes(), format).context("its data couldn't be read")?;
            let mut png = Vec::new();
            decoded.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).context("it couldn't be converted to PNG")?;
            ("png", png)
        }
    };
    let path = dir.join(format!("{stem}.{ext}"));
    std::fs::write(&path, bytes).with_context(|| format!("{} couldn't be written", path.display()))?;
    Ok(path)
}

/// Copy an image file agents can't read (see `needs_conversion`) into `dir` as PNG, with macOS's
/// own `sips` (it reads HEIC, which the `image` crate doesn't).
pub fn convert_file(path: &Path, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("{} couldn't be created", dir.display()))?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
    let out = dir.join(format!("{stem}-{}.png", timestamp()));
    let converted = std::process::Command::new("/usr/bin/sips")
        .args(["-s", "format", "png"])
        .arg(path)
        .arg("--out")
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !converted || !out.is_file() {
        let _ = std::fs::remove_file(&out);
        bail!("it couldn't be converted to PNG");
    }
    Ok(out)
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
            Job::File(path) => path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string()),
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
    cx.spawn_in(window, async move |view, cx| {
        let dir = trek_core::paths::data_dir().join("snapshots");
        let results = cx.background_executor().spawn(async move { jobs.iter().map(|j| (j.label(), j.run(&dir))).collect::<Vec<_>>() }).await;
        let _ = view.update_in(cx, |this, window, cx| {
            let (failed, send) = this.outbox().landed(n, results);
            if let Some(message) = failed {
                crate::workspace::workspace_global(cx).update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message, undo: None }));
            }
            if send {
                this.send_held(window, cx);
            }
            cx.notify();
        });
    })
    .detach();
}

/// Attach image files (dropped, picked or pasted): ones agents read go on at once, the rest
/// (HEIC, TIFF, BMP) once a PNG copy is made.
pub fn attach_files<T: Attaching>(this: &mut T, images: Vec<PathBuf>, window: &mut Window, cx: &mut Context<T>) {
    let (convert, ready): (Vec<PathBuf>, Vec<PathBuf>) = images.into_iter().partition(|p| needs_conversion(p));
    this.outbox().add(ready);
    save(this, convert.into_iter().map(Job::File).collect(), window, cx);
    cx.notify();
}

/// Apply a paste the textarea handed over to the view that owns `input`: image files attach,
/// other copied files are mentioned with `@`, and image data is attached once it's saved to
/// Trek's snapshots folder (off the main thread).
pub fn paste<T: Attaching>(this: &mut T, pasted: Pasted, input: &Entity<TextareaState>, window: &mut Window, cx: &mut Context<T>) {
    match pasted {
        Pasted::Files { images, others } => {
            attach_files(this, images, window, cx);
            if !others.is_empty() {
                let refs: String = others.iter().map(|p| format!("@{} ", p.display())).collect();
                input.update(cx, |s, cx| {
                    let value = s.value().to_string();
                    let cursor = s.cursor().min(value.len());
                    let word_start = value.get(..cursor).is_none_or(|before| before.is_empty() || before.ends_with(char::is_whitespace));
                    s.insert(if word_start { refs } else { format!(" {refs}") }, window, cx)
                });
            }
        }
        Pasted::Images(images) => save(this, images.into_iter().map(Job::Data).collect(), window, cx),
    }
    cx.notify();
}

/// Thumbnails of `paths` (`size` square) with a remove button on hover, plus a spinner tile while
/// `busy` (a snapshot or paste is still being saved).
pub fn thumbnails(
    paths: &[PathBuf],
    size: Pixels,
    busy: bool,
    on_remove: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let radius = size * (10. / 56.);
    h_flex()
        .gap_2()
        .flex_wrap()
        .children(paths.iter().enumerate().map(|(i, p)| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let on_remove = on_remove.clone();
            div()
                .id(("attachment", i))
                .group("att")
                .relative()
                .flex_none()
                .size(size)
                .rounded(radius)
                .overflow_hidden()
                .border_1()
                .border_color(theme.border)
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(name.clone()).build(window, cx))
                .child(img(p.clone()).size_full().object_fit(ObjectFit::Cover))
                .child(
                    div()
                        .id(("att-x", i))
                        .absolute()
                        .top(px(3.))
                        .right(px(3.))
                        .size(px(18.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui_kit::black().opacity(0.65))
                        .invisible()
                        .group_hover("att", |s| s.visible())
                        .cursor_pointer()
                        .child(Icon::new(IconName::Close).xsmall().text_color(gpui_kit::white()))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_remove(i, window, cx);
                        }),
                )
        }))
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

#[cfg(test)]
mod tests {
    // Not `super::*`: the gpui glob import brings its own `test` attribute.
    use super::{Outbox, Pasted, convert_file, needs_conversion, pasted, save_image, split_files};
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
        image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut out), format).unwrap();
        out
    }

    #[test]
    fn text_pastes_stay_text() {
        assert_eq!(pasted(&ClipboardItem::new_string("hello".into())), None);
        // Finder copy of non-image files: their names paste as text, as before.
        let files = ClipboardItem {
            entries: vec![
                ClipboardEntry::ExternalPaths(ExternalPaths(vec![PathBuf::from("/p/src/main.rs")].into())),
                ClipboardEntry::String(gpui_kit::ClipboardString::new("main.rs".into())),
            ],
        };
        assert_eq!(pasted(&files), None);
    }

    #[test]
    fn copied_image_files_attach_and_other_files_are_mentioned() {
        let item = ClipboardItem {
            entries: vec![
                ClipboardEntry::ExternalPaths(ExternalPaths(vec![PathBuf::from("/d/shot.PNG"), PathBuf::from("/d/notes.md"), PathBuf::from("/d/b.jpeg")].into())),
                ClipboardEntry::String(gpui_kit::ClipboardString::new("shot.PNG notes.md b.jpeg".into())),
            ],
        };
        assert_eq!(
            pasted(&item),
            Some(Pasted::Files { images: vec![PathBuf::from("/d/shot.PNG"), PathBuf::from("/d/b.jpeg")], others: vec![PathBuf::from("/d/notes.md")] })
        );
    }

    #[test]
    fn photos_and_tiffs_from_finder_attach_too() {
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(vec![PathBuf::from("/d/IMG_1234.HEIC"), PathBuf::from("/d/scan.tiff"), PathBuf::from("/d/a.pdf")].into()))],
        };
        assert_eq!(
            pasted(&item),
            Some(Pasted::Files { images: vec![PathBuf::from("/d/IMG_1234.HEIC"), PathBuf::from("/d/scan.tiff")], others: vec![PathBuf::from("/d/a.pdf")] })
        );
        assert!(needs_conversion(&PathBuf::from("/d/x.bmp")) && !needs_conversion(&PathBuf::from("/d/x.png")));
        let (images, others) = split_files([PathBuf::from("/d/x.webp"), PathBuf::from("/d/x.svg")]);
        assert_eq!((images, others), (vec![PathBuf::from("/d/x.webp")], vec![PathBuf::from("/d/x.svg")]));
    }

    #[test]
    fn a_send_waits_for_images_still_being_saved() {
        let mut outbox = Outbox::default();
        // Nothing being saved: the send goes now.
        assert!(!outbox.hold_send());
        outbox.saving = 2;
        assert!(outbox.hold_send());
        // The first image lands; the second is still out, so the send keeps waiting.
        assert_eq!(outbox.landed(1, vec![("the pasted image".into(), Ok(PathBuf::from("/s/a.png")))]), (None, false));
        assert_eq!(outbox.landed(1, vec![("the pasted image".into(), Ok(PathBuf::from("/s/b.png")))]), (None, true));
        assert_eq!(outbox.paths, vec![PathBuf::from("/s/a.png"), PathBuf::from("/s/b.png")]);
        // Once sent, a later paste doesn't send by itself.
        outbox.saving = 1;
        assert_eq!(outbox.landed(1, vec![("the pasted image".into(), Ok(PathBuf::from("/s/c.png")))]), (None, false));
    }

    #[test]
    fn a_failed_image_cancels_the_waiting_send() {
        let mut outbox = Outbox { saving: 1, ..Default::default() };
        assert!(outbox.hold_send());
        let (failed, send) = outbox.landed(1, vec![("IMG_1.HEIC".into(), Err(anyhow::anyhow!("it couldn't be converted to PNG")))]);
        assert_eq!(failed.as_deref(), Some("Couldn't attach IMG_1.HEIC: it couldn't be converted to PNG."));
        assert!(!send && outbox.paths.is_empty() && outbox.saving == 0);
        assert!(!outbox.hold_send());
    }

    #[test]
    fn converts_image_files_agents_cant_read() {
        let dir = temp_dir("convert");
        std::fs::create_dir_all(&dir).unwrap();
        let tiff = dir.join("scan.tiff");
        std::fs::write(&tiff, encoded(image::ImageFormat::Tiff)).unwrap();
        let out = convert_file(&tiff, &dir.join("snapshots")).unwrap();
        assert!(out.file_name().unwrap().to_string_lossy().starts_with("scan-"));
        let back = image::open(&out).unwrap().to_rgba8();
        assert_eq!((back.width(), back.height()), (2, 1));
        assert!(crate::mentions::is_image(&out));

        let bad = dir.join("broken.heic");
        std::fs::write(&bad, b"not an image").unwrap();
        assert_eq!(convert_file(&bad, &dir.join("snapshots")).unwrap_err().to_string(), "it couldn't be converted to PNG");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn image_data_is_taken() {
        let image = Image::from_bytes(ImageFormat::Png, encoded(image::ImageFormat::Png));
        assert_eq!(pasted(&ClipboardItem::new_image(&image)), Some(Pasted::Images(vec![image])));
    }

    #[test]
    fn saves_png_as_is_and_converts_tiff() {
        let dir = temp_dir("save");
        let png = encoded(image::ImageFormat::Png);
        let saved = save_image(&Image::from_bytes(ImageFormat::Png, png.clone()), &dir).unwrap();
        assert_eq!(saved.extension().and_then(|e| e.to_str()), Some("png"));
        assert_eq!(std::fs::read(&saved).unwrap(), png);

        let tiff = save_image(&Image::from_bytes(ImageFormat::Tiff, encoded(image::ImageFormat::Tiff)), &dir).unwrap();
        assert_eq!(tiff.extension().and_then(|e| e.to_str()), Some("png"));
        let back = image::open(&tiff).unwrap().to_rgba8();
        assert_eq!((back.width(), back.height(), back.get_pixel(1, 0).0), (2, 1, [0, 0, 255, 255]));
        assert!(crate::mentions::is_image(&tiff));

        assert!(save_image(&Image::from_bytes(ImageFormat::Svg, b"<svg/>".to_vec()), &dir).is_err());
        let bad = save_image(&Image::from_bytes(ImageFormat::Tiff, b"not a tiff".to_vec()), &dir).unwrap_err();
        assert_eq!(bad.to_string(), "its data couldn't be read");
        let _ = std::fs::remove_dir_all(dir);
    }
}
