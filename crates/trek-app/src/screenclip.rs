//! App Snapshots on Windows. macOS hands the picking to `screencapture`, which writes a file;
//! Windows' own picker, Snipping Tool's overlay (`ms-screenclip:`), puts what was picked on the
//! clipboard instead. So a snapshot here is: note the clipboard's sequence number, open the overlay,
//! and look at the clipboard every quarter of a second for up to a minute until a new image shows
//! up. A snip that is cancelled changes nothing, so the wait ends by the user pressing the shortcut
//! again, or by the timeout.
//!
//! The clipboard and the overlay sit behind `Backend`, so the wait (`Pickup`) and the conversion to
//! PNG (`to_png`) are tested without either; the tests of the composer install a fake one.

// The overlay and clipboard are Windows'; the wait and the conversion are plain code that the tests
// run everywhere, so on a Mac the parts only the real backend builds are unused.
#![cfg_attr(not(any(windows, test)), allow(dead_code))]

use anyhow::{Result, bail, ensure};
use gpui_kit::{App, Global};
use std::sync::Arc;
use std::time::Duration;

/// How often the clipboard is looked at.
pub const INTERVAL: Duration = Duration::from_millis(250);
/// How long a snapshot waits for a snip before giving up.
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// Image data as the clipboard held it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clip {
    /// A PNG file's bytes (the clipboard's registered "PNG" format).
    Png(Vec<u8>),
    /// A device-independent bitmap (`CF_DIBV5` or `CF_DIB`): its header, then the pixels.
    Dib(Vec<u8>),
}

/// What one look at the clipboard found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    Image(Clip),
    /// It changed to something that isn't an image (text copied meanwhile).
    NotAnImage,
    /// Another program had it open; ask again next time.
    Busy,
}

/// The clipboard, as far as a snapshot needs it.
pub trait Source: Send {
    /// A number that changes whenever the clipboard's contents do.
    fn sequence(&mut self) -> u32;
    /// The image on the clipboard, if there is one.
    fn read(&mut self) -> Read;
}

/// The overlay that picks, and the clipboard its result lands on.
pub trait Backend: Send + Sync {
    /// Open the overlay. The error is shown to the user.
    fn launch(&self) -> Result<(), String>;
    fn source(&self) -> Box<dyn Source>;
    fn interval(&self) -> Duration {
        INTERVAL
    }
    fn timeout(&self) -> Duration {
        TIMEOUT
    }
}

/// The backend a test (or a platform) put in place of the real one.
struct Installed(Arc<dyn Backend>);

impl Global for Installed {}

/// Use `backend` from now on (tests).
#[cfg(test)]
pub fn install(backend: Arc<dyn Backend>, cx: &mut App) {
    cx.set_global(Installed(backend));
}

/// What snapshots use: the real overlay and clipboard on Windows, nothing elsewhere.
pub fn backend(cx: &App) -> Arc<dyn Backend> {
    if let Some(Installed(backend)) = cx.try_global::<Installed>() {
        return backend.clone();
    }
    #[cfg(windows)]
    return Arc::new(real::Snipping);
    #[cfg(not(windows))]
    return Arc::new(Unavailable);
}

/// The stand-in where there is no such overlay (macOS has `screencapture` instead).
#[cfg(not(windows))]
struct Unavailable;

#[cfg(not(windows))]
impl Backend for Unavailable {
    fn launch(&self) -> Result<(), String> {
        Err("the snipping tool is part of Windows".into())
    }
    fn source(&self) -> Box<dyn Source> {
        struct Empty;
        impl Source for Empty {
            fn sequence(&mut self) -> u32 {
                0
            }
            fn read(&mut self) -> Read {
                Read::NotAnImage
            }
        }
        Box::new(Empty)
    }
}

/// Where a wait stands after a look at the clipboard.
#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    Waiting,
    Arrived(Clip),
    TimedOut,
}

/// The wait for a snip: remembers the clipboard as it was, and what has been seen of it since.
#[derive(Debug)]
pub struct Pickup {
    seen: u32,
    waited: Duration,
    interval: Duration,
    timeout: Duration,
}

impl Pickup {
    /// `seen` is the clipboard's sequence number from before the overlay opened.
    pub fn new(seen: u32, interval: Duration, timeout: Duration) -> Self {
        Self { seen, waited: Duration::ZERO, interval, timeout }
    }

    /// One look, once an interval: a new image ends the wait with it; other changes to the
    /// clipboard (some text copied meanwhile) are noted and waited past; a clipboard another
    /// program holds is looked at again next time, as it still looks new.
    pub fn poll(&mut self, source: &mut dyn Source) -> Poll {
        let now = source.sequence();
        if now != self.seen {
            match source.read() {
                Read::Image(clip) => return Poll::Arrived(clip),
                Read::NotAnImage => self.seen = now,
                Read::Busy => {}
            }
        }
        self.waited += self.interval;
        if self.waited >= self.timeout { Poll::TimedOut } else { Poll::Waiting }
    }
}

/// `clip` as PNG bytes, for the attachment.
pub fn to_png(clip: Clip) -> Result<Vec<u8>> {
    let dib = match clip {
        Clip::Png(bytes) => return Ok(bytes),
        Clip::Dib(bytes) => bytes,
    };
    let (width, height, rgb) = match dib_pixels(&dib) {
        Ok(decoded) => decoded,
        // 1, 4, 8 and 16 bits, or compressed: the `image` crate reads those, as a BMP file.
        Err(_) => return other_dib_to_png(&dib),
    };
    let mut png = Vec::new();
    let image = image::RgbImage::from_raw(width, height, rgb).ok_or_else(|| anyhow::anyhow!("the image's size doesn't match its pixels"))?;
    image::DynamicImage::ImageRgb8(image).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    Ok(png)
}

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
/// Larger than any screen; keeps a corrupt header from asking for gigabytes.
const MAX_SIDE: u64 = 32_768;

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Where a DIB's pixels start, after its header, bit masks and colour table.
fn pixel_offset(dib: &[u8], header: u32, bits: u16, compression: u32) -> Option<usize> {
    // A 40-byte header keeps its three masks after it; the longer ones have them inside.
    let masks = if compression == BI_BITFIELDS && header == 40 { 12 } else { 0 };
    let used = le_u32(dib, 32)?;
    let colours = if used != 0 { used as usize } else if bits <= 8 { 1usize << bits } else { 0 };
    Some(header as usize + masks + colours * 4)
}

/// An uncompressed 24 or 32 bit DIB as RGB: `(width, height, pixels)`. Screenshots are opaque,
/// so a 32 bit DIB's fourth byte (alpha, which `CF_DIB` leaves unset) is not read.
fn dib_pixels(dib: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let header = le_u32(dib, 0).unwrap_or(0);
    ensure!(header >= 40, "an old-style bitmap header");
    let (width, height) = (le_u32(dib, 4).unwrap_or(0) as i32, le_u32(dib, 8).unwrap_or(0) as i32);
    let bits = le_u16(dib, 14).unwrap_or(0);
    let compression = le_u32(dib, 16).unwrap_or(u32::MAX);
    ensure!(matches!(bits, 24 | 32) && matches!(compression, BI_RGB | BI_BITFIELDS), "{bits} bit pixels, compression {compression}");
    ensure!(width > 0 && height != 0, "an empty image");
    let (w, h) = (width as u64, height.unsigned_abs() as u64);
    ensure!(w <= MAX_SIDE && h <= MAX_SIDE, "an image {w}×{h} is too large");
    // Masks pick each colour out of a 32 bit pixel; 24 bit pixels are plain BGR.
    let masks = if bits == 32 && compression == BI_BITFIELDS {
        [le_u32(dib, 40).unwrap_or(0), le_u32(dib, 44).unwrap_or(0), le_u32(dib, 48).unwrap_or(0)]
    } else {
        [0x00ff_0000, 0x0000_ff00, 0x0000_00ff]
    };
    ensure!(masks.iter().all(|m| *m != 0), "a colour mask is empty");
    let start = pixel_offset(dib, header, bits, compression).ok_or_else(|| anyhow::anyhow!("a bitmap cut short"))?;
    let stride = (w * bits as u64).div_ceil(32) * 4;
    ensure!(dib.len() as u64 >= start as u64 + stride * h, "a bitmap cut short");
    let channel = |pixel: u32, mask: u32| {
        let shift = mask.trailing_zeros();
        (((pixel & mask) >> shift) as u64 * 255 / (mask >> shift) as u64) as u8
    };
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for row in 0..h {
        // A positive height is a bottom-up bitmap: the first row stored is the lowest.
        let from = if height > 0 { h - 1 - row } else { row };
        let line = &dib[start + (from * stride) as usize..];
        for x in 0..w as usize {
            if bits == 24 {
                rgb.extend_from_slice(&[line[x * 3 + 2], line[x * 3 + 1], line[x * 3]]);
            } else {
                let pixel = u32::from_le_bytes([line[x * 4], line[x * 4 + 1], line[x * 4 + 2], line[x * 4 + 3]]);
                rgb.extend_from_slice(&[channel(pixel, masks[0]), channel(pixel, masks[1]), channel(pixel, masks[2])]);
            }
        }
    }
    Ok((width as u32, h as u32, rgb))
}

/// A DIB of a kind `dib_pixels` doesn't read, through the `image` crate: a BMP file is the DIB
/// with a 14 byte file header in front.
fn other_dib_to_png(dib: &[u8]) -> Result<Vec<u8>> {
    let header = le_u32(dib, 0).unwrap_or(0);
    if header < 12 {
        bail!("its data couldn't be read");
    }
    let bits = le_u16(dib, 14).unwrap_or(0);
    let compression = le_u32(dib, 16).unwrap_or(0);
    let offset = pixel_offset(dib, header, bits, compression).unwrap_or(header as usize) + 14;
    let mut bmp = Vec::with_capacity(dib.len() + 14);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&(offset as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    let decoded = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp).map_err(|_| anyhow::anyhow!("its data couldn't be read"))?;
    let mut png = Vec::new();
    decoded.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|_| anyhow::anyhow!("it couldn't be converted to PNG"))?;
    Ok(png)
}

#[cfg(windows)]
mod real {
    //! Snipping Tool and the Win32 clipboard.

    use super::{Backend, Clip, Read, Source};
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};

    const CF_DIB: u32 = 8;
    const CF_DIBV5: u32 = 17;

    pub struct Snipping;

    impl Backend for Snipping {
        fn launch(&self) -> Result<(), String> {
            // SAFETY: NUL-ended literals; the call hands the URI to the shell and returns.
            let result = unsafe { ShellExecuteW(None, w!("open"), w!("ms-screenclip:"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
            // Success is any value above 32; the rest are the shell's error codes.
            if result.0 as usize > 32 { Ok(()) } else { Err(format!("Windows answered {}", result.0 as usize)) }
        }

        fn source(&self) -> Box<dyn Source> {
            Box::new(Clipboard)
        }
    }

    pub struct Clipboard;

    /// The clipboard, open; closed again when this goes. Every `OpenClipboard` here is a `Guard`.
    pub struct Guard;

    impl Guard {
        /// `None` when another program has the clipboard open through a window (as one writing to it
        /// does). Without a window of its own this gets through while others read it the same way.
        pub fn open() -> Option<Self> {
            // SAFETY: no owner window; paired with the `CloseClipboard` in `drop`.
            unsafe { OpenClipboard(None).ok().map(|()| Guard) }
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: only made by `open`, which opened it.
            let _ = unsafe { CloseClipboard() };
        }
    }

    /// The bytes of the clipboard's `format`, copied out. The clipboard must be open.
    fn bytes_of(_open: &Guard, format: u32) -> Option<Vec<u8>> {
        // SAFETY: the clipboard is open (`_open`), so the handle it gives stays valid until it
        // closes; the memory is locked while it is copied and unlocked after.
        unsafe {
            let handle: HANDLE = GetClipboardData(format).ok()?;
            let memory = HGLOBAL(handle.0);
            let size = GlobalSize(memory);
            let at = GlobalLock(memory);
            if at.is_null() {
                return None;
            }
            let copy = std::slice::from_raw_parts(at as *const u8, size).to_vec();
            let _ = GlobalUnlock(memory);
            Some(copy)
        }
    }

    impl Source for Clipboard {
        fn sequence(&mut self) -> u32 {
            // SAFETY: a plain query.
            unsafe { GetClipboardSequenceNumber() }
        }

        fn read(&mut self) -> Read {
            let Some(open) = Guard::open() else { return Read::Busy };
            // SAFETY: plain queries while the clipboard is open.
            let png = unsafe { RegisterClipboardFormatW(w!("PNG")) };
            let mut present = false;
            // PNG is lossless and keeps what was picked as it is; the DIBs are what everything has.
            for (format, wrap) in [(png, Clip::Png as fn(Vec<u8>) -> Clip), (CF_DIBV5, Clip::Dib), (CF_DIB, Clip::Dib)] {
                // SAFETY: as above.
                if format == 0 || unsafe { IsClipboardFormatAvailable(format) }.is_err() {
                    continue;
                }
                present = true;
                if let Some(bytes) = bytes_of(&open, format) {
                    return Read::Image(wrap(bytes));
                }
            }
            // An image that is there but wouldn't come out (the owner is slow to render it) is
            // asked for again; no image at all is a clipboard of something else.
            if present { Read::Busy } else { Read::NotAnImage }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Clipboard, Guard};
        use crate::screenclip::Source as _;
        use std::time::{Duration, Instant};
        use windows::Win32::System::DataExchange::{CloseClipboard, OpenClipboard};
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE};
        use windows::core::w;

        /// Opened for a moment within `within`, whoever else might be looking (a clipboard manager
        /// does). Through a window of its own: Windows lets an `OpenClipboard(NULL)` through while the
        /// clipboard is open without a window (as `Guard` leaves it), from any thread, so only an
        /// open with a window can tell that a `Guard` was never closed.
        fn opens_with_a_window(within: Duration) -> bool {
            // SAFETY: a message-only window of a system class, made and destroyed on this thread;
            // the clipboard is only opened and closed, never emptied or written.
            unsafe {
                let window = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), None, WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None).expect("a message-only window");
                let until = Instant::now() + within;
                let opened = loop {
                    if OpenClipboard(Some(window)).is_ok() {
                        let _ = CloseClipboard();
                        break true;
                    }
                    if Instant::now() >= until {
                        break false;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                };
                let _ = DestroyWindow(window);
                opened
            }
        }

        #[test]
        fn a_look_at_the_real_clipboard_leaves_it_closed() {
            // Reads only, never writes: whatever the user has copied stays as it was.
            let mut clipboard = Clipboard;
            for _ in 0..3 {
                let _ = clipboard.sequence();
                let _ = clipboard.read();
            }
            assert!(opens_with_a_window(Duration::from_secs(5)), "the clipboard stayed open after a read");
        }

        #[test]
        fn a_clipboard_left_open_would_be_noticed() {
            // The check above, against a clipboard held open: it must say so. (Only opens it, for a
            // moment; when another program has it for seconds there is nothing to show.)
            let until = Instant::now() + Duration::from_secs(5);
            let open = loop {
                if let Some(open) = Guard::open() {
                    break open;
                }
                if Instant::now() >= until {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            assert!(!opens_with_a_window(Duration::from_millis(200)), "a window opened the clipboard while a Guard held it");
            drop(open);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A clipboard that changes as scripted: each look at `sequence` takes the next step.
    struct Fake {
        steps: VecDeque<(u32, Read)>,
        last: (u32, Read),
    }

    impl Fake {
        fn new(start: u32, steps: impl IntoIterator<Item = (u32, Read)>) -> Self {
            Self { steps: steps.into_iter().collect(), last: (start, Read::NotAnImage) }
        }
    }

    impl Source for Fake {
        fn sequence(&mut self) -> u32 {
            if let Some(next) = self.steps.pop_front() {
                self.last = next;
            }
            self.last.0
        }
        fn read(&mut self) -> Read {
            self.last.1.clone()
        }
    }

    fn pickup(seen: u32) -> Pickup {
        Pickup::new(seen, Duration::from_millis(250), Duration::from_secs(60))
    }

    fn png_clip() -> Clip {
        Clip::Png(vec![0x89, b'P', b'N', b'G'])
    }

    #[test]
    fn a_new_image_ends_the_wait() {
        let mut clipboard = Fake::new(7, [(7, Read::NotAnImage), (7, Read::NotAnImage), (8, Read::Image(png_clip()))]);
        let mut wait = pickup(7);
        assert_eq!(wait.poll(&mut clipboard), Poll::Waiting);
        assert_eq!(wait.poll(&mut clipboard), Poll::Waiting);
        assert_eq!(wait.poll(&mut clipboard), Poll::Arrived(png_clip()));
    }

    #[test]
    fn an_image_already_there_is_not_a_snapshot() {
        // The clipboard held a picture before the overlay opened (same sequence number): waiting.
        let mut clipboard = Fake::new(7, [(7, Read::Image(png_clip()))]);
        assert_eq!(pickup(7).poll(&mut clipboard), Poll::Waiting);
    }

    #[test]
    fn text_copied_meanwhile_is_waited_past() {
        let mut clipboard = Fake::new(1, [(2, Read::NotAnImage), (2, Read::NotAnImage), (3, Read::Image(png_clip()))]);
        let mut wait = pickup(1);
        assert_eq!(wait.poll(&mut clipboard), Poll::Waiting);
        assert_eq!(wait.poll(&mut clipboard), Poll::Waiting);
        assert_eq!(wait.poll(&mut clipboard), Poll::Arrived(png_clip()));
    }

    #[test]
    fn a_clipboard_someone_else_holds_is_asked_again() {
        let mut clipboard = Fake::new(1, [(2, Read::Busy), (2, Read::Busy), (2, Read::Image(png_clip()))]);
        let mut wait = pickup(1);
        assert_eq!(wait.poll(&mut clipboard), Poll::Waiting);
        assert_eq!(wait.poll(&mut clipboard), Poll::Waiting);
        assert_eq!(wait.poll(&mut clipboard), Poll::Arrived(png_clip()));
    }

    #[test]
    fn nothing_after_a_minute_times_out() {
        // A cancelled snip changes nothing: 240 looks at a quarter second each.
        let mut clipboard = Fake::new(5, []);
        let mut wait = pickup(5);
        for look in 1..240 {
            assert_eq!(wait.poll(&mut clipboard), Poll::Waiting, "look {look}");
        }
        assert_eq!(wait.poll(&mut clipboard), Poll::TimedOut);
    }

    /// A 2×2 DIB: BGR rows, bottom row first (red, green / blue, white), each row padded to 4 bytes.
    fn dib24() -> Vec<u8> {
        let mut dib = Vec::new();
        for field in [40u32, 2, 2] {
            dib.extend_from_slice(&field.to_le_bytes());
        }
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        dib.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]); // bottom: blue, white
        dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]); // top: red, green
        dib
    }

    /// The same picture as 32 bit pixels with their alpha byte unset, as `CF_DIB` has them.
    fn dib32(height: i32) -> Vec<u8> {
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&height.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        let (red, green, blue, white) = ([0, 0, 255, 0], [0, 255, 0, 0], [255, 0, 0, 0], [255, 255, 255, 0]);
        let rows = if height > 0 { [[blue, white], [red, green]] } else { [[red, green], [blue, white]] };
        for row in rows {
            row.iter().for_each(|p| dib.extend_from_slice(p));
        }
        dib
    }

    fn pixels(png: &[u8]) -> (u32, u32, Vec<[u8; 3]>) {
        let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png).unwrap().to_rgb8();
        (decoded.width(), decoded.height(), decoded.pixels().map(|p| p.0).collect())
    }

    #[test]
    fn a_dib_becomes_a_png_the_right_way_up() {
        let picture = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
        assert_eq!(pixels(&to_png(Clip::Dib(dib24())).unwrap()), (2, 2, picture.clone()), "24 bit, bottom-up");
        assert_eq!(pixels(&to_png(Clip::Dib(dib32(2))).unwrap()), (2, 2, picture.clone()), "32 bit, bottom-up");
        assert_eq!(pixels(&to_png(Clip::Dib(dib32(-2))).unwrap()), (2, 2, picture), "32 bit, top-down");
    }

    #[test]
    fn a_dib_with_bit_masks_reads_them() {
        // 32 bit BI_BITFIELDS with the masks after the header, RGB laid out as 0xRRGGBB00.
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&BI_BITFIELDS.to_le_bytes());
        dib.extend_from_slice(&[0; 20]);
        for mask in [0xff00_0000u32, 0x00ff_0000, 0x0000_ff00] {
            dib.extend_from_slice(&mask.to_le_bytes());
        }
        dib.extend_from_slice(&0x1020_3000u32.to_le_bytes());
        assert_eq!(pixels(&to_png(Clip::Dib(dib)).unwrap()), (1, 1, vec![[0x10, 0x20, 0x30]]));
    }

    #[test]
    fn a_png_is_kept_and_a_broken_dib_is_refused() {
        assert_eq!(to_png(png_clip()).unwrap(), vec![0x89, b'P', b'N', b'G']);
        let mut cut = dib24();
        cut.truncate(cut.len() - 4);
        assert!(to_png(Clip::Dib(cut)).is_err(), "pixels missing");
        assert!(to_png(Clip::Dib(vec![1, 2, 3])).is_err(), "no header");
        // A header claiming a huge picture is refused before anything is allocated for it.
        let mut huge = dib24();
        huge[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(to_png(Clip::Dib(huge)).is_err());
    }

    #[test]
    fn an_eight_bit_dib_goes_through_the_image_crate() {
        // 1×1, 8 bit, a two-colour palette (blue then red), the pixel is entry 1.
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&8u16.to_le_bytes());
        dib.extend_from_slice(&[0; 16]); // compression, image size, pixels per metre ×2
        dib.extend_from_slice(&2u32.to_le_bytes()); // colours used
        dib.extend_from_slice(&[0; 4]);
        dib.extend_from_slice(&[255, 0, 0, 0, 0, 0, 255, 0]);
        dib.extend_from_slice(&[1, 0, 0, 0]);
        assert_eq!(pixels(&to_png(Clip::Dib(dib)).unwrap()), (1, 1, vec![[255, 0, 0]]));
    }
}
