//! Images going out with the next message: what a paste attaches instead of inserting as text,
//! saving clipboard image data to disk, and the thumbnail strip above the prompt.

use crate::mentions;
use crate::workspace::WorkspaceEvent;
use anyhow::{Context as _, Result, bail};
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};

/// A paste a composer takes over from its textarea.
#[derive(Debug, Clone, PartialEq)]
pub enum Pasted {
    /// Files copied in Finder: images to attach, and the rest to mention with `@`.
    Files { images: Vec<PathBuf>, others: Vec<PathBuf> },
    /// Image data (a screenshot copied to the clipboard), saved to disk before attaching.
    Images(Vec<Image>),
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
        let (images, others): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| mentions::is_image(p));
        return (!images.is_empty()).then_some(Pasted::Files { images, others });
    }
    let images: Vec<Image> = item.entries().iter().filter_map(|e| if let ClipboardEntry::Image(i) = e { Some(i.clone()) } else { None }).collect();
    (!images.is_empty()).then_some(Pasted::Images(images))
}

/// Write clipboard image data into `dir` as a file the agents accept. PNG, JPEG, GIF and WebP
/// are kept as they are; TIFF, BMP and the like (what some apps put on the clipboard) become PNG.
pub fn save_image(image: &Image, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("{} couldn't be created", dir.display()))?;
    let stem = format!("pasted-{}-{:08x}", chrono::Local::now().format("%Y%m%d-%H%M%S-%3f"), image.id() as u32);
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

/// Apply a paste the textarea handed over to the view that owns `input`: image files attach at
/// once, other copied files are mentioned with `@`, and image data is attached once it's saved to
/// Trek's snapshots folder (off the main thread). `list` picks the view's attachments and its
/// count of images still being saved.
pub fn paste<T: 'static>(
    this: &mut T,
    pasted: Pasted,
    input: &Entity<TextareaState>,
    list: fn(&mut T) -> (&mut Vec<PathBuf>, &mut usize),
    window: &mut Window,
    cx: &mut Context<T>,
) {
    match pasted {
        Pasted::Files { images, others } => {
            let (paths, _) = list(this);
            for path in images {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
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
        Pasted::Images(images) => {
            let n = images.len();
            *list(this).1 += n;
            cx.spawn(async move |view, cx| {
                let dir = trek_core::paths::data_dir().join("snapshots");
                let saved = cx.background_executor().spawn(async move { images.iter().map(|i| save_image(i, &dir)).collect::<Vec<_>>() }).await;
                let _ = view.update(cx, |this, cx| {
                    let (paths, pending) = list(this);
                    *pending = pending.saturating_sub(n);
                    let mut failed = None;
                    for r in saved {
                        match r {
                            Ok(path) => paths.push(path),
                            Err(e) => failed = Some(e),
                        }
                    }
                    if let Some(e) = failed {
                        let message = format!("Couldn't attach the pasted image: {e}.");
                        crate::workspace::workspace_global(cx).update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message, undo: None }));
                    }
                    cx.notify();
                });
            })
            .detach();
        }
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
    use super::{Pasted, pasted, save_image};
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
