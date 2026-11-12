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

/// Copy an image file agents can't read (see `needs_conversion`) into `dir` as PNG. The system's
/// own codecs are asked first, as they read HEIC, which the `image` crate doesn't: `sips` on a
/// Mac, WIC on Windows (where the HEIF Image Extensions are installed). Then the `image` crate
/// reads TIFF and BMP.
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
    let converted = sips_png(path, out) || wic_png(path, out) || image_crate_png(path, out);
    if !converted || !out.is_file() {
        let _ = std::fs::remove_file(out);
        // A photo Windows could read with an extension it doesn't have: say which.
        #[cfg(windows)]
        if is_heif(path) && !wic::reads_heif() {
            bail!(
                "Windows reads HEIC photos only with the HEIF Image Extensions from the Microsoft Store"
            );
        }
        bail!("it couldn't be converted to PNG");
    }
    Ok(())
}

/// A HEIC or HEIF photo, by its name.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_heif(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("heic") || e.eq_ignore_ascii_case("heif"))
}

/// `path` as PNG at `out`, by Windows' own image codecs (WIC). There are none off Windows.
fn wic_png(path: &Path, out: &Path) -> bool {
    #[cfg(windows)]
    {
        match wic::to_png(path, out) {
            Ok(()) => true,
            Err(e) => {
                tracing::info!("WIC couldn't convert {}: {e}", path.display());
                false
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (path, out);
        false
    }
}

/// Windows Imaging Component: a photo's first frame decoded by whatever codec Windows has for
/// it (HEIF's comes with the HEIF Image Extensions, with HEVC's for HEIC), written out as PNG.
#[cfg(windows)]
mod wic {
    use std::path::Path;
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_ContainerFormatHeif, GUID_ContainerFormatPng,
        GUID_WICPixelFormat32bppBGRA, IWICImagingFactory, WICBitmapDitherTypeNone,
        WICBitmapEncoderNoCache, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::core::{HSTRING, Result};

    /// `f` with COM ready on this thread, and the imaging factory.
    fn with_factory<T>(f: impl FnOnce(&IWICImagingFactory) -> Result<T>) -> Result<T> {
        // SAFETY: plain calls. A thread already in an apartment of another kind answers with an
        // error and stays as it is; it's left uninitialized only when this call initialized it.
        let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        // SAFETY: the factory's class id, in-process.
        let result = unsafe {
            CoCreateInstance::<_, IWICImagingFactory>(
                &CLSID_WICImagingFactory,
                None,
                CLSCTX_INPROC_SERVER,
            )
        }
        .and_then(|factory| f(&factory));
        if joined {
            // SAFETY: balances the successful CoInitializeEx above, on the same thread.
            unsafe { CoUninitialize() };
        }
        result
    }

    /// Windows has a codec that reads HEIF (and so HEIC) photos.
    pub fn reads_heif() -> bool {
        // SAFETY: asks for a decoder by container format; none installed is an error.
        with_factory(|factory| unsafe {
            factory.CreateDecoder(&GUID_ContainerFormatHeif, std::ptr::null())
        })
        .is_ok()
    }

    pub fn to_png(path: &Path, out: &Path) -> Result<()> {
        with_factory(|factory| {
            // SAFETY: WIC's documented sequence: decode the first frame, convert it to 32-bit
            // BGRA, and encode that as a PNG frame into a stream on `out`. Every interface is
            // released when it drops.
            unsafe {
                let decoder = factory.CreateDecoderFromFilename(
                    &HSTRING::from(path.as_os_str()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )?;
                let frame = decoder.GetFrame(0)?;
                let pixels = factory.CreateFormatConverter()?;
                pixels.Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )?;
                let (mut width, mut height) = (0, 0);
                pixels.GetSize(&mut width, &mut height)?;
                let stream = factory.CreateStream()?;
                stream.InitializeFromFilename(&HSTRING::from(out.as_os_str()), GENERIC_WRITE.0)?;
                let encoder = factory.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
                encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
                let (mut png, mut options) = (None, None);
                encoder.CreateNewFrame(&mut png, &mut options)?;
                let png =
                    png.ok_or_else(|| windows::core::Error::from(windows::core::HRESULT(-1)))?;
                png.Initialize(options.as_ref())?;
                png.SetSize(width, height)?;
                let mut format = GUID_WICPixelFormat32bppBGRA;
                png.SetPixelFormat(&mut format)?;
                png.WriteSource(&pixels, std::ptr::null())?;
                png.Commit()?;
                encoder.Commit()?;
            }
            Ok(())
        })
    }

    /// A photo encoded as HEIF by Windows' own codec, for tests and the capture build's
    /// `heic-demo`: `None` where it has none.
    #[cfg(any(test, feature = "shots"))]
    pub fn heif_of(width: u32, height: u32, out: &Path) -> Option<()> {
        use windows::Win32::Graphics::Imaging::GUID_WICPixelFormat32bppBGRA as BGRA;
        with_factory(|factory| unsafe {
            // SAFETY: the same sequence as `to_png`, from a bitmap made of `pixels`.
            let pixels: Vec<u8> = (0..width * height)
                .flat_map(|i| [(i * 40 % 256) as u8, 120, 200, 255])
                .collect();
            let bitmap =
                factory.CreateBitmapFromMemory(width, height, &BGRA, width * 4, &pixels)?;
            let stream = factory.CreateStream()?;
            stream.InitializeFromFilename(&HSTRING::from(out.as_os_str()), GENERIC_WRITE.0)?;
            let encoder = factory.CreateEncoder(&GUID_ContainerFormatHeif, std::ptr::null())?;
            encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
            let (mut frame, mut options) = (None, None);
            encoder.CreateNewFrame(&mut frame, &mut options)?;
            let frame =
                frame.ok_or_else(|| windows::core::Error::from(windows::core::HRESULT(-1)))?;
            frame.Initialize(options.as_ref())?;
            frame.SetSize(width, height)?;
            let mut format = BGRA;
            frame.SetPixelFormat(&mut format)?;
            frame.WriteSource(&bitmap, std::ptr::null())?;
            frame.Commit()?;
            encoder.Commit()?;
            Ok(())
        })
        .ok()
    }
}

/// A photo as an iPhone writes one (HEIC) at `out`, made by the system's own codec: Windows'
/// HEIF encoder (with the HEIF and HEVC extensions), or a Mac's `sips`. For the capture build's
/// `heic-demo`; `Err` says why there's none.
#[cfg(feature = "shots")]
pub fn heic_photo(out: &Path) -> anyhow::Result<()> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(windows)]
    {
        wic::heif_of(96, 64, out).ok_or_else(|| {
            anyhow::anyhow!("Windows has no HEIF encoder here (the HEIF and HEVC extensions)")
        })
    }
    #[cfg(target_os = "macos")]
    {
        let png = out.with_extension("png");
        image::RgbaImage::from_fn(96, 64, |x, y| {
            image::Rgba([(x * 2) as u8, (y * 4) as u8, 200, 255])
        })
        .save(&png)?;
        let made = std::process::Command::new("/usr/bin/sips")
            .args(["-s", "format", "heic"])
            .arg(&png)
            .arg("--out")
            .arg(out)
            .output()?;
        let _ = std::fs::remove_file(&png);
        anyhow::ensure!(
            made.status.success(),
            "sips: {}",
            String::from_utf8_lossy(&made.stderr)
        );
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        anyhow::bail!("no HEIF encoder here")
    }
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
                .child(
                    crate::pictures::picture(p, crate::pictures::Fit::Tile)
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
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
    // Once its frame is made (kept in Trek's cache), the tile says so to tests and live checks.
    let preview = preview.when(video && poster_file(path).is_file(), |el| {
        el.child(
            div()
                .id(("attachment-poster", i))
                .test_support()
                .absolute()
                .size(px(1.)),
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

/// A video's poster frame, as the system's own thumbnail of it, made once into Trek's cache and
/// read as any image: Quick Look's on the Mac (`qlmanage`), Explorer's on Windows (the shell's
/// thumbnail of the file). Where there is none (no thumbnailer for the format, and in tests), the
/// tile keeps the file's icon.
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
            if let Err(e) = make_poster(&video, &png) {
                // The tile keeps the file's icon; the log says why there's no frame.
                tracing::info!("no poster frame for {}: {e:#}", video.display());
                return Err(e.into());
            }
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

/// Make `video`'s poster frame at `png`, unless it's there already: the thumbnail Explorer shows
/// for it, from whichever thumbnailer Windows has for the format (its own reads MP4, MOV, AVI,
/// WMV and the rest Media Foundation plays), as big as the Mac's.
#[cfg(all(windows, not(any(test, feature = "test-support"))))]
fn make_poster(video: &Path, png: &Path) -> Result<()> {
    if png.is_file() {
        return Ok(());
    }
    if let Some(dir) = png.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Written beside it, then moved: a tile drawn meanwhile never reads half a file.
    let making = png.with_extension("making.png");
    let made = shell_thumbnail::to_png(video, shell_thumbnail::POSTER_EDGE, &making)
        .and_then(|()| std::fs::rename(&making, png).map_err(anyhow::Error::from));
    if made.is_err() {
        let _ = std::fs::remove_file(&making);
    }
    made
}

#[cfg(any(test, feature = "test-support", not(any(target_os = "macos", windows))))]
fn make_poster(video: &Path, _png: &Path) -> Result<()> {
    bail!("no poster frame for {} here", video.display())
}

/// Explorer's thumbnail of a file, through its shell item's image factory: what Explorer draws for
/// a video (a frame from it, by Windows' own thumbnailer or one a codec pack adds), never the
/// file's icon in its place. Written out as PNG.
#[cfg(windows)]
#[cfg_attr(any(test, feature = "test-support"), allow(dead_code))]
mod shell_thumbnail {
    use anyhow::{Context as _, Result, bail};
    use std::path::Path;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC,
        GetDIBits, GetObjectW, HBITMAP, ReleaseDC,
    };
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_THUMBNAILONLY,
    };
    use windows::core::HSTRING;

    /// How big a poster frame is made, at most, either way (`qlmanage -s 256` on the Mac).
    pub const POSTER_EDGE: i32 = 256;

    /// `path`'s thumbnail, at most `edge` pixels on its longer side, as a PNG at `out`.
    pub fn to_png(path: &Path, edge: i32, out: &Path) -> Result<()> {
        let (width, height, mut pixels) = thumbnail(path, edge)?;
        unpremultiply(&mut pixels);
        let image = image::RgbaImage::from_raw(width, height, pixels)
            .context("the thumbnail's pixels don't fill it")?;
        image
            .save_with_format(out, image::ImageFormat::Png)
            .with_context(|| format!("{} couldn't be written", out.display()))
    }

    /// The thumbnail's size and pixels, top row first, as RGBA (still premultiplied).
    fn thumbnail(path: &Path, edge: i32) -> Result<(u32, u32, Vec<u8>)> {
        // SAFETY: plain calls. The shell wants a single-threaded apartment; a thread already in
        // one of another kind answers with an error and stays as it is (thumbnails are made there
        // too). It's left uninitialized only when this call initialized it.
        let joined = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        // As Windows writes it: the shell takes `D:/media/clip.mp4` (an agent's, Git Bash's, a
        // pasted one) for "The parameter is incorrect", and the tile went without its frame.
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        // SAFETY: a NUL-ended path; the bitmap the factory hands over is ours, read, then freed.
        let result = unsafe {
            SHCreateItemFromParsingName::<_, _, IShellItemImageFactory>(
                &HSTRING::from(path.as_os_str()),
                None,
            )
            .and_then(|item| item.GetImage(SIZE { cx: edge, cy: edge }, SIIGBF_THUMBNAILONLY))
            .context("Windows has no thumbnail of it")
            .and_then(|bitmap| {
                let pixels = bitmap_pixels(bitmap);
                let _ = DeleteObject(bitmap.into());
                pixels
            })
        };
        if joined {
            // SAFETY: balances the successful CoInitializeEx above, on the same thread.
            unsafe { CoUninitialize() };
        }
        result
    }

    /// A bitmap's pixels as 32-bit RGBA, top row first.
    ///
    /// # Safety
    /// `bitmap` is a live bitmap, selected into no device context.
    unsafe fn bitmap_pixels(bitmap: HBITMAP) -> Result<(u32, u32, Vec<u8>)> {
        let mut info = BITMAP::default();
        // SAFETY: `info` is a BITMAP, as big as the call is told.
        let got = unsafe {
            GetObjectW(
                bitmap.into(),
                std::mem::size_of::<BITMAP>() as i32,
                Some((&mut info as *mut BITMAP).cast()),
            )
        };
        let (width, height) = (info.bmWidth, info.bmHeight.abs());
        if got == 0 || width <= 0 || height <= 0 {
            bail!("the thumbnail is no bitmap");
        }
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: rows top first.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        // SAFETY: the screen's device context, released below; `pixels` holds every row of 32-bit
        // pixels the header asks for.
        let rows = unsafe {
            let screen = GetDC(None);
            let rows = GetDIBits(
                screen,
                bitmap,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut header,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, screen);
            rows
        };
        if rows != height {
            bail!("the thumbnail's pixels couldn't be read");
        }
        // BGRA to RGBA.
        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        Ok((width as u32, height as u32, pixels))
    }

    /// Straight alpha from the shell's premultiplied RGBA; a bitmap whose alpha is all zero had
    /// none, and is opaque.
    pub fn unpremultiply(pixels: &mut [u8]) {
        if pixels.chunks_exact(4).all(|px| px[3] == 0) {
            pixels.chunks_exact_mut(4).for_each(|px| px[3] = 255);
            return;
        }
        for px in pixels.chunks_exact_mut(4) {
            let a = u32::from(px[3]);
            if a > 0 && a < 255 {
                for c in &mut px[..3] {
                    *c = (u32::from(*c) * 255 / a).min(255) as u8;
                }
            }
        }
    }
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
        let said = convert_file(&bad, &dir.join("snapshots"))
            .unwrap_err()
            .to_string();
        // A Windows without HEIF's codec says where to get it instead (CI's has none).
        assert!(
            said == "it couldn't be converted to PNG"
                || (cfg!(windows) && said.starts_with("Windows reads HEIC photos only with")),
            "{said}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A real HEIC photo (an iPhone's, or a sample) converted where this OS can:
    /// `TREK_HEIC=C:\path\IMG_0001.HEIC cargo test -p trek-app --lib a_real_heic -- --ignored`.
    #[test]
    #[ignore = "needs a HEIC photo, named by TREK_HEIC"]
    fn a_real_heic_photo_converts() {
        let photo = PathBuf::from(std::env::var("TREK_HEIC").expect("TREK_HEIC names a photo"));
        let dir = temp_dir("real-heic");
        let out = convert_file(&photo, &dir).unwrap();
        let back = image::open(&out).unwrap();
        println!(
            "{} → {}x{} PNG",
            photo.display(),
            back.width(),
            back.height()
        );
        assert!(back.width() > 0 && back.height() > 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// An iPhone's HEIC photo, made by Windows' own HEIF codec, comes out as a PNG agents read.
    #[cfg(windows)]
    #[test]
    fn a_heic_photo_is_read_by_windows_own_codec() {
        let dir = temp_dir("heic");
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("IMG_0001.HEIC");
        if super::wic::heif_of(64, 48, &photo).is_none() {
            // No HEIF encoder here (a PC without the HEIF and HEVC extensions, as CI's): nothing
            // to read back.
            let _ = std::fs::remove_dir_all(dir);
            return;
        }
        assert!(super::wic::reads_heif());
        let out = convert_file(&photo, &dir.join("snapshots")).unwrap();
        let back = image::open(&out).unwrap().to_rgba8();
        assert_eq!((back.width(), back.height()), (64, 48));
        assert!(crate::mentions::is_image(&out));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A video's poster frame on Windows: Explorer's thumbnail of it (`shell_thumbnail`).
    #[cfg(windows)]
    mod shell_thumbnails {
        use super::temp_dir;
        use crate::attachments::shell_thumbnail::{POSTER_EDGE, to_png, unpremultiply};

        /// Windows has a thumbnailer for `.ext` files: a thumbnail provider among its shell
        /// extensions, for the extension or for videos as a whole.
        fn windows_thumbnails(ext: &str) -> bool {
            use windows_sys::Win32::System::Registry::{
                HKEY_CLASSES_ROOT, RRF_RT_REG_SZ, RegGetValueW,
            };
            const PROVIDER: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
            [
                format!(r".{ext}\ShellEx\{PROVIDER}"),
                format!(r"SystemFileAssociations\.{ext}\ShellEx\{PROVIDER}"),
                format!(r"SystemFileAssociations\video\ShellEx\{PROVIDER}"),
            ]
            .iter()
            .any(|key| {
                let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
                let mut bytes = 0u32;
                // SAFETY: a NUL-ended name; only the default value's size is asked for.
                unsafe {
                    RegGetValueW(
                        HKEY_CLASSES_ROOT,
                        key.as_ptr(),
                        std::ptr::null(),
                        RRF_RT_REG_SZ,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        &mut bytes,
                    ) == 0
                }
            })
        }

        /// An attached video's tile shows Explorer's thumbnail of it: a frame, the video's shape,
        /// as a PNG. Needs a video, made here by ffmpeg where it's installed, and a Windows that
        /// thumbnails videos (a desktop's; Server without Media Foundation has no thumbnailer):
        /// without either, there's nothing to look at, and the test says so and passes.
        #[test]
        fn a_video_s_poster_is_the_frame_explorer_shows() {
            use std::process::Stdio;
            let dir = temp_dir("poster");
            std::fs::create_dir_all(&dir).unwrap();
            let video = dir.join("clip.mp4");
            // A second of ffmpeg's test pattern, 4:3.
            let made = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(["-f", "lavfi", "-i"])
                .arg("testsrc=size=160x120:rate=4:duration=1")
                .args(["-pix_fmt", "yuv420p"])
                .arg(&video)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !made || !windows_thumbnails("mp4") {
                eprintln!(
                    "skipped: {}",
                    if made {
                        "Windows has no thumbnailer for MP4 here"
                    } else {
                        "no ffmpeg to make a video with"
                    }
                );
                let _ = std::fs::remove_dir_all(dir);
                return;
            }
            let png = dir.join("poster.png");
            to_png(&video, POSTER_EDGE, &png).unwrap();
            let back = image::open(&png).unwrap().to_rgba8();
            let (w, h) = (back.width(), back.height());
            let edge = POSTER_EDGE as u32;
            assert!(w > 0 && h > 0 && w <= edge && h <= edge, "{w}x{h}");
            assert!(
                (w as f32 / h as f32 - 4.0 / 3.0).abs() < 0.1,
                "the video's 4:3, not a square icon: {w}x{h}"
            );
            assert!(back.pixels().all(|p| p.0[3] == 255), "opaque");
            // The pattern's colours, not a blank or a glyph.
            let colours: std::collections::HashSet<[u8; 3]> = back
                .pixels()
                .map(|p| [p.0[0] >> 4, p.0[1] >> 4, p.0[2] >> 4])
                .collect();
            assert!(colours.len() > 8, "{} colours", colours.len());

            // Written with forward slashes, as an agent or Git Bash writes a path: the same frame.
            let slashed = std::path::PathBuf::from(video.to_string_lossy().replace('\\', "/"));
            let again = dir.join("poster-slashed.png");
            to_png(&slashed, POSTER_EDGE, &again).unwrap();
            assert_eq!(image::open(&again).unwrap().width(), w);

            // A file Windows has no thumbnail of says so, rather than giving its icon.
            let text = dir.join("notes.txt");
            std::fs::write(&text, "not a video").unwrap();
            assert!(to_png(&text, POSTER_EDGE, &dir.join("x.png")).is_err());
            let _ = std::fs::remove_dir_all(dir);
        }

        /// A poster asked for from a thread already in the multithreaded apartment, as the app's
        /// image loader can ask from its pool: still made, the shell's thumbnailers being free
        /// of the asking thread's apartment.
        #[test]
        fn a_poster_is_made_from_a_thread_in_the_multithreaded_apartment() {
            use std::process::Stdio;
            use windows::Win32::System::Com::{
                COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
            };
            let dir = temp_dir("poster-mta");
            std::fs::create_dir_all(&dir).unwrap();
            let video = dir.join("clip.mp4");
            let made = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(["-f", "lavfi", "-i"])
                .arg("testsrc=size=160x120:rate=4:duration=1")
                .args(["-pix_fmt", "yuv420p"])
                .arg(&video)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !made || !windows_thumbnails("mp4") {
                eprintln!("skipped: no ffmpeg, or no thumbnailer for MP4 here");
                let _ = std::fs::remove_dir_all(dir);
                return;
            }
            let png = dir.join("poster.png");
            let (from, to) = (video.clone(), png.clone());
            let made = std::thread::spawn(move || {
                // SAFETY: plain calls, balanced on this thread.
                unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
                    .ok()
                    .unwrap();
                let made = to_png(&from, POSTER_EDGE, &to);
                unsafe { CoUninitialize() };
                made
            })
            .join()
            .unwrap();
            made.unwrap();
            let back = image::open(&png).unwrap();
            assert!(back.width() > 0 && back.height() > 0);
            let _ = std::fs::remove_dir_all(dir);
        }

        /// The shell's pixels are premultiplied; a bitmap with no alpha at all is opaque.
        #[test]
        fn a_thumbnail_s_alpha_is_made_straight() {
            let mut none = vec![10, 20, 30, 0, 40, 50, 60, 0];
            unpremultiply(&mut none);
            assert_eq!(none, [10, 20, 30, 255, 40, 50, 60, 255]);
            let mut half = vec![50, 100, 0, 128, 7, 8, 9, 255, 0, 0, 0, 0];
            unpremultiply(&mut half);
            assert_eq!(half, [99, 199, 0, 128, 7, 8, 9, 255, 0, 0, 0, 0]);
        }
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
