//! App Snapshots on a Linux desktop. There is no one capture tool to run there, so a snapshot
//! goes one of two ways, by what the session has:
//!
//! - **The desktop's screenshot portal** (`org.freedesktop.portal.Screenshot`, which GNOME, KDE
//!   and others answer on the session bus). Asked with `interactive`, the desktop shows its own
//!   picker for a window, an area or the screen; either way it saves a file and names it by a
//!   `file://` URI, which Trek reads into its snapshots folder.
//! - **The X server**, in an X11 session: the root window's pixels are the whole screen
//!   (`GetImage`). No picker comes with that, so it takes the screen and nothing else. Under
//!   Xwayland the root window shows X11 windows only, never the screen, so it isn't used there.
//!
//! What is decided here is plain code that builds and is tested on every platform: which way a
//! snapshot goes (`Session::route`), what the + menu and Settings offer for it (`crate::Tool`),
//! whether an X server is the session's own (`Signs`), what the portal's answer means
//! (`path_of_uri`, `take`), and the pixels' way into a PNG or JPEG (`frame_from_x`). Only the calls
//! to the session bus and the X server are Linux's (`real`), behind `Capture`, which a test stands
//! in for (`pretend`).

use gpui_kit::{AnyWindowHandle, App};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How long Trek's windows are given to get out of the way before the screen is read: a desktop
/// animates a window as it minimizes.
pub const SETTLE: Duration = Duration::from_millis(350);

/// What this session can take a snapshot through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Session {
    /// The desktop's portal answers for screenshots.
    pub portal: bool,
    /// An X server whose root window is the screen (an X11 session, not Xwayland).
    pub x11: bool,
}

/// What a snapshot is asked to take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Whatever is picked in the desktop's own tool: a window, an area or the screen.
    Pick,
    /// The whole screen, at once.
    Screen,
}

/// The way a snapshot is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The portal; `interactive` has the desktop show its picker first.
    Portal { interactive: bool },
    /// The root window's pixels, from the X server.
    XServer,
}

impl Session {
    /// How `mode` is taken here, if it can be. Picking is the portal's alone. The screen comes
    /// from the X server where that has it (at once, nothing asked, no copy kept elsewhere), else
    /// from the portal without its picker.
    pub fn route(self, mode: Mode) -> Option<Route> {
        match mode {
            Mode::Pick => self.portal.then_some(Route::Portal { interactive: true }),
            Mode::Screen if self.x11 => Some(Route::XServer),
            Mode::Screen => self.portal.then_some(Route::Portal { interactive: false }),
        }
    }

    /// What the shortcut takes when Settings ask for `wanted`: that, or the screen where there is
    /// nothing to pick with. `None`: this session can take no snapshot.
    pub fn nearest(self, wanted: Mode) -> Option<Mode> {
        [wanted, Mode::Screen]
            .into_iter()
            .find(|mode| self.route(*mode).is_some())
    }

    /// The sentence Settings › App Snapshots says about how snapshots are taken here.
    pub fn note(self) -> &'static str {
        if self.portal {
            "Your desktop's own screenshot tool does the picking: a window, an area or the screen. It keeps a copy of its own where it saves screenshots (GNOME's in ~/Pictures, KDE's a temporary file in /tmp), which the portal has no way to ask it not to keep; Trek leaves it."
        } else if self.x11 {
            "No screenshot portal answers in this session, so a snapshot takes the whole screen, as the X server has it. Picking a window or an area needs the desktop's portal (xdg-desktop-portal with a backend that takes screenshots, as GNOME and KDE have)."
        } else {
            NOTHING_TO_CAPTURE_WITH
        }
    }
}

/// Said when a snapshot is asked for where nothing can take one.
pub const NOTHING_TO_CAPTURE_WITH: &str = "Trek can't take a snapshot in this session: no screenshot portal answers on the session bus (xdg-desktop-portal, as GNOME and KDE have), and there is no X server whose screen it could read.";

/// What the environment says about the session's display.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Signs {
    /// `DISPLAY`.
    pub display: Option<String>,
    /// `WAYLAND_DISPLAY`. Trek takes it out of its own environment at launch to get X11 windows
    /// in a Wayland session, so its absence alone says little.
    pub wayland_display: Option<String>,
    /// `XDG_SESSION_TYPE`, as the login manager set it: `x11`, `wayland`, `tty`.
    pub session_type: Option<String>,
    /// A compositor's socket is in `XDG_RUNTIME_DIR` (`wayland_socket_in`).
    pub wayland_socket: bool,
}

impl Signs {
    /// Whether the X server at `DISPLAY` is the session's own, so its root window is the screen.
    /// Anything of Wayland's says no: there the X server is Xwayland.
    pub fn x11_session(&self) -> bool {
        let set = |value: &Option<String>| value.as_deref().is_some_and(|v| !v.trim().is_empty());
        set(&self.display)
            && !set(&self.wayland_display)
            && !self.wayland_socket
            && !self
                .session_type
                .as_deref()
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("wayland"))
    }
}

/// Whether a Wayland compositor's socket (`wayland-0`, `wayland-1`…) is in `runtime_dir`.
pub fn wayland_socket_in(runtime_dir: &Path) -> bool {
    std::fs::read_dir(runtime_dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("wayland-") && !name.ends_with(".lock")
        })
}

/// What the portal answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// It saved a screenshot: the URI it named it by.
    Saved(String),
    /// The user closed its picker.
    Cancelled,
    /// It couldn't, or no portal answered; why, in its words.
    Failed(String),
}

/// The screen's pixels: `width * height` of them, three bytes each (red, green, blue), top row
/// first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// The session bus and the X server, as far as a snapshot needs them.
pub trait Capture: Send + Sync {
    /// What this session has. Asked while drawing, so it answers at once with what is known.
    fn session(&self) -> Session;
    /// Ask the portal for a screenshot. Returns once it has answered: with `interactive`, when the
    /// user has picked or closed its picker.
    fn ask_portal(&self, interactive: bool) -> Asked;
    /// The whole screen, from the X server; else why not, to be shown.
    fn screen(&self) -> Result<Frame, String>;
    /// Trek's windows out of the way of what is being captured (Hide Trek while capturing).
    fn step_aside(&self, cx: &mut App);
    /// And back, with `front` the one in front.
    fn come_back(&self, front: AnyWindowHandle, cx: &mut App);
    /// Whether the session's first look is still out: `session` answers at once with what it has
    /// (none until the look answers), so a surface that needs the whole answer waits on this.
    fn looking(&self) -> bool {
        false
    }
    /// How long the windows are given to go.
    fn settle(&self) -> Duration {
        SETTLE
    }
}

/// The kind of file a snapshot is kept as (Settings › App Snapshots › Format).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpg,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpg => "jpg",
        }
    }

    fn kind(self) -> image::ImageFormat {
        match self {
            Format::Png => image::ImageFormat::Png,
            Format::Jpg => image::ImageFormat::Jpeg,
        }
    }
}

/// How a snapshot went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Taken {
    /// It's in the file that was asked for.
    Saved,
    /// The user closed the desktop's picker: nothing to attach, nothing to say.
    Cancelled,
    /// Why not, as the end of "Couldn't attach the snapshot: …".
    Failed(String),
}

/// Take a snapshot by `route` into `out`, a `format` file. It waits for the user where the
/// desktop's picker is up, so not on the main thread.
pub fn take(capture: &dyn Capture, route: Route, format: Format, out: &Path) -> Taken {
    let bytes = match route {
        Route::XServer => capture
            .screen()
            .map_err(|why| format!("the X server didn't hand over the screen ({why})"))
            .and_then(|frame| encode_frame(frame, format)),
        Route::Portal { interactive } => match capture.ask_portal(interactive) {
            Asked::Cancelled => return Taken::Cancelled,
            Asked::Failed(why) => Err(format!("the desktop's screenshot portal said \"{why}\"")),
            Asked::Saved(uri) => read_saved(&uri).and_then(|bytes| recode(bytes, format)),
        },
    };
    let written = bytes.and_then(|bytes| {
        std::fs::write(out, bytes).map_err(|e| format!("it couldn't be saved ({e})"))
    });
    match written {
        Ok(()) => Taken::Saved,
        Err(why) => Taken::Failed(why),
    }
}

/// The file a `file://` URI names, as the portal writes them: no host (or `localhost`), the path
/// percent-encoded. `None` for anything else.
pub fn path_of_uri(uri: &str) -> Option<PathBuf> {
    path_of_uri_on(uri, cfg!(windows))
}

/// `path_of_uri` by one platform's rules: Windows writes a drive after the slash
/// (`file:///C:/Users/…`), which is no part of the path there. (No portal runs on Windows; the
/// tests that read a real file through a URI do.)
pub fn path_of_uri_on(uri: &str, windows: bool) -> Option<PathBuf> {
    let rest = uri.trim().strip_prefix("file://")?;
    let path = match rest.find('/')? {
        0 => rest,
        slash if rest[..slash].eq_ignore_ascii_case("localhost") => &rest[slash..],
        _ => return None,
    };
    let raw = path.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut at = 0;
    while at < raw.len() {
        if raw[at] == b'%' {
            let hex = raw.get(at + 1..at + 3)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            bytes.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            at += 3;
        } else {
            bytes.push(raw[at]);
            at += 1;
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    if path.contains('\0') {
        return None;
    }
    let drive = matches!(path.as_bytes(), [b'/', letter, b':', ..] if letter.is_ascii_alphabetic());
    Some(PathBuf::from(if windows && drive {
        &path[1..]
    } else {
        &path[..]
    }))
}

/// The bytes of the screenshot the portal saved at `uri`.
fn read_saved(uri: &str) -> Result<Vec<u8>, String> {
    let path = path_of_uri(uri).ok_or_else(|| {
        format!("the desktop named its screenshot in a way Trek can't read: {uri}")
    })?;
    let bytes = std::fs::read(&path).map_err(|e| {
        format!(
            "the desktop's screenshot {} couldn't be read ({e})",
            path.display()
        )
    })?;
    if bytes.is_empty() {
        return Err(format!(
            "the desktop's screenshot {} is empty",
            path.display()
        ));
    }
    Ok(bytes)
}

/// A screenshot file's `bytes` as a `format` file's: kept as they are when they're one already
/// (a PNG's see-through corners and all), else drawn again.
fn recode(bytes: Vec<u8>, format: Format) -> Result<Vec<u8>, String> {
    if image::guess_format(&bytes).is_ok_and(|kind| kind == format.kind()) {
        return Ok(bytes);
    }
    let decoded = image::load_from_memory(&bytes)
        .map_err(|e| format!("the desktop's screenshot isn't a picture Trek reads ({e})"))?;
    encode(over_white(&decoded.to_rgba8()), format)
}

/// `image` without its alpha, over white: a window's see-through corners and shadow aren't left
/// black in a JPEG, which has no alpha.
fn over_white(image: &image::RgbaImage) -> image::RgbImage {
    let mut flat = image::RgbImage::new(image.width(), image.height());
    for (from, to) in image.pixels().zip(flat.pixels_mut()) {
        let alpha = u32::from(from[3]);
        for channel in 0..3 {
            to[channel] = ((u32::from(from[channel]) * alpha + 255 * (255 - alpha)) / 255) as u8;
        }
    }
    flat
}

fn encode_frame(frame: Frame, format: Format) -> Result<Vec<u8>, String> {
    let image = image::RgbImage::from_raw(frame.width, frame.height, frame.rgb)
        .ok_or("the screen's size doesn't match its pixels")?;
    encode(image, format)
}

fn encode(image: image::RgbImage, format: Format) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), format.kind())
        .map_err(|e| format!("it couldn't be written as a {} ({e})", format.extension()))?;
    Ok(bytes)
}

/// Larger than any screen; keeps a wrong answer from asking for gigabytes.
const MAX_SIDE: u32 = 65_535;

/// The X server's answer to `GetImage` (ZPixmap) as a `Frame`: `data` is `width * height` pixels
/// of `bits_per_pixel`, each 0x00RRGGBB in the server's byte order (`lsb_first`: blue comes
/// first), as every true-colour screen of depth 24 or 32 hands them over.
pub fn frame_from_x(
    width: u32,
    height: u32,
    bits_per_pixel: u8,
    lsb_first: bool,
    data: &[u8],
) -> Result<Frame, String> {
    if bits_per_pixel != 32 {
        return Err(format!(
            "its pixels are {bits_per_pixel} bits each, and Trek reads 32"
        ));
    }
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!("a screen of {width}×{height}"));
    }
    let pixels = width as usize * height as usize;
    let Some(data) = data.get(..pixels * 4) else {
        return Err("the image was cut short".into());
    };
    let mut rgb = Vec::with_capacity(pixels * 3);
    for pixel in data.chunks_exact(4) {
        if lsb_first {
            rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        } else {
            rgb.extend_from_slice(&[pixel[1], pixel[2], pixel[3]]);
        }
    }
    Ok(Frame { width, height, rgb })
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// The desktop a test put in place of the real one (each GPUI test runs on its own thread).
    static PRETEND: std::cell::RefCell<Option<Arc<dyn Capture>>> =
        const { std::cell::RefCell::new(None) };
}

/// What snapshots are taken through on a Linux desktop: the session's own bus and X server there,
/// nothing on macOS and Windows (which have `screencapture` and Snipping Tool). Under a test, only
/// what the test put there (`pretend`): never the computer's own desktop.
pub fn capture() -> Option<Arc<dyn Capture>> {
    #[cfg(any(test, feature = "test-support"))]
    {
        PRETEND.with(|pretend| pretend.borrow().clone())
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        system()
    }
}

/// This computer's own desktop, where it's a Linux one.
pub fn system() -> Option<Arc<dyn Capture>> {
    #[cfg(target_os = "linux")]
    {
        Some(Arc::new(real::Desktop))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Start finding out what the session has, so it is known by the time a menu asks.
pub fn warm() {
    if let Some(capture) = capture() {
        let _ = capture.session();
    }
}

/// Whether a desktop's first look at the session is still out (the real one on Linux; a pretend
/// desktop answers at once, and there is none on a Mac or Windows).
pub fn looking() -> bool {
    capture().is_some_and(|desktop| desktop.looking())
}

/// `entity` is notified once the session's first look has answered, for a surface that drew
/// without it (the settings page, the palette). Does nothing where no look is out.
pub fn refresh_when_known<T: 'static>(cx: &mut gpui_kit::Context<T>) {
    if !looking() {
        return;
    }
    cx.spawn(async move |this, cx| {
        // The portal's first answer can take a moment (it may have to be started); give it
        // a reasonable while, then let the surface be until the next render.
        for _ in 0..150 {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            if !looking() {
                break;
            }
        }
        let _ = this.update(cx, |_, cx| cx.notify());
    })
    .detach();
}

/// Until the guard drops, snapshots on this thread go through `capture`, on any platform.
#[cfg(any(test, feature = "test-support"))]
pub fn pretend(capture: Arc<dyn Capture>) -> impl Drop {
    struct Restore(Option<Arc<dyn Capture>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PRETEND.with(|pretend| *pretend.borrow_mut() = self.0.take());
        }
    }
    Restore(PRETEND.with(|pretend| pretend.borrow_mut().replace(capture)))
}

#[cfg(target_os = "linux")]
mod real {
    //! The session bus (through ashpd, as GPUI reaches the portal's settings) and the X server
    //! (x11rb, as GPUI's X11 windows do).

    use super::{Asked, Capture, Frame, Session, Signs};
    use gpui_kit::{AnyWindowHandle, App, Window};
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, Ordering};

    pub struct Desktop;

    static PORTAL: AtomicBool = AtomicBool::new(false);
    static X11: AtomicBool = AtomicBool::new(false);
    /// The look was asked for, and has answered (`looking`).
    static STARTED: AtomicBool = AtomicBool::new(false);
    static ANSWERED: AtomicBool = AtomicBool::new(false);
    static ASKED: Once = Once::new();
    /// The X windows `step_aside` minimized that were on screen before: `come_back` maps back
    /// only these, never one the user had already minimized.
    static ASIDE: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

    /// Ask the X server and the session bus what they have, once, on a thread of its own: the
    /// portal is started by the bus the first time it's asked for, which can take seconds. Until
    /// they've answered, the session has neither.
    fn find_out() {
        ASKED.call_once(|| {
            STARTED.store(true, Ordering::Relaxed);
            let _ = std::thread::Builder::new()
                .name("trek-snapshots".into())
                .spawn(|| {
                    X11.store(x_server_is_the_screens(), Ordering::Relaxed);
                    PORTAL.store(portal_answers(), Ordering::Relaxed);
                    ANSWERED.store(true, Ordering::Relaxed);
                });
        });
    }

    /// Whether the X server Trek is on is the session's own (`Signs`), and says so itself:
    /// Xwayland names an extension after itself.
    fn x_server_is_the_screens() -> bool {
        use x11rb::protocol::xproto::ConnectionExt as _;
        let var = |name: &str| std::env::var(name).ok();
        let signs = Signs {
            display: var("DISPLAY"),
            wayland_display: var("WAYLAND_DISPLAY"),
            session_type: var("XDG_SESSION_TYPE"),
            wayland_socket: std::env::var_os("XDG_RUNTIME_DIR")
                .is_some_and(|dir| super::wayland_socket_in(std::path::Path::new(&dir))),
        };
        if !signs.x11_session() {
            return false;
        }
        let Ok((conn, _)) = x11rb::connect(None) else {
            return false;
        };
        let xwayland = match conn.query_extension(b"XWAYLAND") {
            Ok(cookie) => cookie.reply().map(|reply| reply.present).unwrap_or(true),
            Err(_) => true,
        };
        !xwayland
    }

    /// Whether the portal has its Screenshot interface: xdg-desktop-portal exports it only when a
    /// backend of the desktop's takes screenshots, so its `version` is there to read or it isn't.
    fn portal_answers() -> bool {
        use ashpd::zbus;
        zbus::block_on(async {
            let Ok(bus) = zbus::Connection::session().await else {
                return false;
            };
            bus.call_method(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.freedesktop.portal.Screenshot", "version"),
            )
            .await
            .is_ok()
        })
    }

    /// The X11 window behind `window`, if it is one.
    fn x_window(window: &Window) -> Option<u32> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
            Ok(RawWindowHandle::Xcb(handle)) => Some(handle.window.get()),
            Ok(RawWindowHandle::Xlib(handle)) => Some(handle.window as u32),
            _ => None,
        }
    }

    impl Capture for Desktop {
        fn session(&self) -> Session {
            find_out();
            Session {
                portal: PORTAL.load(Ordering::Relaxed),
                x11: X11.load(Ordering::Relaxed),
            }
        }

        fn looking(&self) -> bool {
            STARTED.load(Ordering::Relaxed) && !ANSWERED.load(Ordering::Relaxed)
        }

        fn ask_portal(&self, interactive: bool) -> Asked {
            use ashpd::desktop::ResponseError;
            use ashpd::desktop::screenshot::Screenshot;
            let answer = ashpd::zbus::block_on(async {
                Screenshot::request()
                    .interactive(interactive)
                    .modal(true)
                    .send()
                    .await?
                    .response()
            });
            match answer {
                Ok(saved) => Asked::Saved(saved.uri().as_str().to_string()),
                Err(ashpd::Error::Response(ResponseError::Cancelled)) => Asked::Cancelled,
                Err(e) => Asked::Failed(e.to_string()),
            }
        }

        fn screen(&self) -> Result<Frame, String> {
            use x11rb::connection::Connection as _;
            use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder};
            let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
            let setup = conn.setup();
            let root = setup
                .roots
                .get(screen)
                .ok_or("the X server has no such screen")?;
            let (width, height) = (root.width_in_pixels, root.height_in_pixels);
            let reply = conn
                .get_image(ImageFormat::Z_PIXMAP, root.root, 0, 0, width, height, !0)
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| e.to_string())?;
            let bits = setup
                .pixmap_formats
                .iter()
                .find(|format| format.depth == reply.depth)
                .map(|format| format.bits_per_pixel)
                .unwrap_or(32);
            super::frame_from_x(
                u32::from(width),
                u32::from(height),
                bits,
                setup.image_byte_order == ImageOrder::LSB_FIRST,
                &reply.data,
            )
        }

        /// Minimized: GPUI's `hide` does nothing on Linux. Only a window on screen is remembered
        /// (`ASIDE`): one the user already minimized stays minimized when Trek comes back.
        fn step_aside(&self, cx: &mut App) {
            use x11rb::connection::Connection as _;
            use x11rb::protocol::xproto::{ConnectionExt as _, MapState};
            let conn = x11rb::connect(None).ok().map(|(conn, _)| conn);
            let mut aside = Vec::new();
            for handle in cx.windows() {
                let _ = handle.update(cx, |_, window, _| {
                    // On screen now, or can't be asked (kept, so it comes back either way).
                    let mapped = match (&conn, x_window(window)) {
                        (Some(conn), Some(id)) => conn
                            .get_window_attributes(id)
                            .ok()
                            .and_then(|cookie| cookie.reply().ok())
                            .is_none_or(|attrs| attrs.map_state == MapState::VIEWABLE),
                        _ => true,
                    };
                    if mapped && let Some(id) = x_window(window) {
                        aside.push(id);
                    }
                    window.minimize_window();
                });
            }
            *ASIDE.lock().unwrap() = aside;
        }

        /// Mapping a minimized window is how X11 asks for it back (ICCCM 4.1.4), from any
        /// connection; a window manager may then leave it behind others, so `front` asks for the
        /// focus too — only the windows `step_aside` found on screen are mapped back.
        fn come_back(&self, front: AnyWindowHandle, cx: &mut App) {
            use x11rb::connection::Connection as _;
            use x11rb::protocol::xproto::ConnectionExt as _;
            let aside: Vec<u32> = std::mem::take(&mut *ASIDE.lock().unwrap());
            let conn = x11rb::connect(None).ok().map(|(conn, _)| conn);
            for handle in cx.windows() {
                let _ = handle.update(cx, |_, window, _| {
                    let id = x_window(window);
                    if let (Some(conn), Some(id)) = (&conn, id)
                        && aside.contains(&id)
                    {
                        let _ = conn.map_window(id);
                        let _ = conn.flush();
                        if handle == front {
                            window.activate_window();
                        }
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn session(portal: bool, x11: bool) -> Session {
        Session { portal, x11 }
    }

    #[test]
    fn a_snapshot_goes_the_way_the_session_has() {
        // GNOME or KDE on X11: the desktop picks, the X server hands over the screen.
        let both = session(true, true);
        assert_eq!(
            both.route(Mode::Pick),
            Some(Route::Portal { interactive: true })
        );
        assert_eq!(both.route(Mode::Screen), Some(Route::XServer));
        // A Wayland session: the portal for both, the screen without its picker.
        let wayland = session(true, false);
        assert_eq!(
            wayland.route(Mode::Pick),
            Some(Route::Portal { interactive: true })
        );
        assert_eq!(
            wayland.route(Mode::Screen),
            Some(Route::Portal { interactive: false })
        );
        // A bare X11 session (a window manager and no desktop, CI's Xvfb): the screen alone.
        let bare = session(false, true);
        assert_eq!(bare.route(Mode::Pick), None);
        assert_eq!(bare.route(Mode::Screen), Some(Route::XServer));
        let neither = session(false, false);
        assert_eq!(neither.route(Mode::Pick), None);
        assert_eq!(neither.route(Mode::Screen), None);
    }

    #[test]
    fn the_shortcut_takes_the_screen_where_nothing_picks() {
        assert_eq!(session(true, true).nearest(Mode::Pick), Some(Mode::Pick));
        assert_eq!(
            session(true, true).nearest(Mode::Screen),
            Some(Mode::Screen)
        );
        assert_eq!(session(false, true).nearest(Mode::Pick), Some(Mode::Screen));
        assert_eq!(session(false, false).nearest(Mode::Pick), None);
        assert_eq!(session(false, false).nearest(Mode::Screen), None);
    }

    #[test]
    fn settings_say_what_the_session_can_do() {
        assert!(session(true, true).note().contains("does the picking"));
        assert!(session(true, false).note().contains("does the picking"));
        let bare = session(false, true).note();
        assert!(
            bare.contains("the whole screen") && bare.contains("needs the desktop's portal"),
            "{bare}"
        );
        assert_eq!(session(false, false).note(), NOTHING_TO_CAPTURE_WITH);
    }

    fn signs(display: Option<&str>, wayland: Option<&str>, kind: Option<&str>) -> Signs {
        Signs {
            display: display.map(String::from),
            wayland_display: wayland.map(String::from),
            session_type: kind.map(String::from),
            wayland_socket: false,
        }
    }

    #[test]
    fn an_x_server_is_the_screen_only_in_an_x11_session() {
        assert!(signs(Some(":0"), None, Some("x11")).x11_session());
        // CI's Xvfb, a container: nobody logged in, so no session type.
        assert!(signs(Some(":99"), None, None).x11_session());
        assert!(signs(Some(":0"), Some(""), Some("tty")).x11_session());
        // Xwayland: a Wayland session with an X server for its X11 apps.
        assert!(!signs(Some(":0"), Some("wayland-0"), Some("wayland")).x11_session());
        // Trek's own launch took WAYLAND_DISPLAY out: the session type still says it.
        assert!(!signs(Some(":0"), None, Some("wayland")).x11_session());
        assert!(!signs(Some(":0"), None, Some(" Wayland ")).x11_session());
        // A compositor started from a console says `tty`; its socket gives it away.
        let from_a_console = Signs {
            wayland_socket: true,
            ..signs(Some(":0"), None, Some("tty"))
        };
        assert!(!from_a_console.x11_session());
        // No X server at all.
        assert!(!signs(None, None, Some("x11")).x11_session());
        assert!(!signs(Some(" "), None, None).x11_session());
    }

    #[test]
    fn a_compositors_socket_is_found_in_the_runtime_folder() {
        let dir = std::env::temp_dir().join(format!("trek-screenclip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!wayland_socket_in(&dir), "an empty folder");
        assert!(!wayland_socket_in(&dir.join("missing")), "no folder");
        std::fs::write(dir.join("bus"), "").unwrap();
        // The lock a compositor leaves beside its socket isn't the socket.
        std::fs::write(dir.join("wayland-0.lock"), "").unwrap();
        assert!(!wayland_socket_in(&dir));
        std::fs::write(dir.join("wayland-1"), "").unwrap();
        assert!(wayland_socket_in(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_portals_uri_is_read_as_a_file() {
        // As a Linux desktop writes them, read by Linux's rules on every runner.
        let path_of_uri = |uri: &str| path_of_uri_on(uri, false);
        // The form GNOME's portal answers in: a file among the user's pictures, the spaces of its
        // name encoded.
        assert_eq!(
            path_of_uri(
                "file:///home/ada/Pictures/Screenshots/Screenshot%20From%202026-11-26%2010-04-31.png"
            ),
            Some(PathBuf::from(
                "/home/ada/Pictures/Screenshots/Screenshot From 2026-11-26 10-04-31.png"
            ))
        );
        // The form KDE's answers in: a temporary file.
        assert_eq!(
            path_of_uri("file:///tmp/xdg-desktop-portal-kde.XsLQpf.png"),
            Some(PathBuf::from("/tmp/xdg-desktop-portal-kde.XsLQpf.png"))
        );
        // Letters outside ASCII, encoded byte by byte.
        assert_eq!(
            path_of_uri("file:///home/zo%C3%AB/Bilder/Bildschirmfoto%2001.png"),
            Some(PathBuf::from("/home/zoë/Bilder/Bildschirmfoto 01.png"))
        );
        assert_eq!(
            path_of_uri("file://localhost/home/ada/shot.png\n"),
            Some(PathBuf::from("/home/ada/shot.png"))
        );
        // A drive is Windows' to read as one; on Linux it's a folder's odd name.
        assert_eq!(
            path_of_uri_on("file:///C:/Users/ada/shot%201.png", true),
            Some(PathBuf::from("C:/Users/ada/shot 1.png"))
        );
        assert_eq!(
            path_of_uri("file:///C:/Users/ada/shot%201.png"),
            Some(PathBuf::from("/C:/Users/ada/shot 1.png"))
        );
        for not_a_file in [
            "",
            "https://example.com/shot.png",
            "file://",
            "file://elsewhere/home/ada/shot.png",
            "file:///home/ada/shot%2.png",
            "file:///home/ada/shot%zz.png",
            "file:///home/ada/shot%00.png",
            "file:///home/ada/%ff%fe.png",
            "/home/ada/shot.png",
        ] {
            assert_eq!(path_of_uri(not_a_file), None, "{not_a_file:?}");
        }
    }

    #[test]
    fn the_x_servers_pixels_become_red_green_and_blue() {
        // Two pixels, 0x00102030 and 0x00a0b0c0, as a little-endian server sends them.
        let lsb = [0x30, 0x20, 0x10, 0, 0xc0, 0xb0, 0xa0, 0xff];
        assert_eq!(
            frame_from_x(2, 1, 32, true, &lsb).unwrap().rgb,
            [0x10, 0x20, 0x30, 0xa0, 0xb0, 0xc0]
        );
        // And as a big-endian one does.
        let msb = [0, 0x10, 0x20, 0x30, 0xff, 0xa0, 0xb0, 0xc0];
        assert_eq!(
            frame_from_x(2, 1, 32, false, &msb).unwrap().rgb,
            [0x10, 0x20, 0x30, 0xa0, 0xb0, 0xc0]
        );
        // More bytes than the picture needs (a server pads) are left.
        assert_eq!(
            frame_from_x(1, 1, 32, true, &lsb).unwrap(),
            Frame {
                width: 1,
                height: 1,
                rgb: vec![0x10, 0x20, 0x30]
            }
        );
        assert!(frame_from_x(2, 2, 32, true, &lsb).is_err(), "cut short");
        assert!(frame_from_x(2, 1, 16, true, &lsb).is_err(), "16 bit pixels");
        assert!(frame_from_x(0, 1, 32, true, &lsb).is_err(), "no screen");
        assert!(
            frame_from_x(u32::MAX, u32::MAX, 32, true, &lsb).is_err(),
            "a size nothing has"
        );
    }

    /// A desktop that answers as scripted, and notes what it was asked.
    #[derive(Default)]
    struct Fake {
        session: Session,
        looking: bool,
        portal: Mutex<Option<Asked>>,
        screen: Option<Frame>,
        asked: Mutex<Vec<&'static str>>,
    }

    impl Capture for Fake {
        fn session(&self) -> Session {
            self.session
        }
        fn looking(&self) -> bool {
            self.looking
        }
        fn ask_portal(&self, interactive: bool) -> Asked {
            self.asked
                .lock()
                .unwrap()
                .push(if interactive { "pick" } else { "portal screen" });
            self.portal
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(Asked::Failed("no portal".into()))
        }
        fn screen(&self) -> Result<Frame, String> {
            self.asked.lock().unwrap().push("x screen");
            self.screen.clone().ok_or_else(|| "BadMatch".to_string())
        }
        fn step_aside(&self, _: &mut App) {}
        fn come_back(&self, _: AnyWindowHandle, _: &mut App) {}
    }

    /// A folder of this test's own.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("trek-screenclip-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `path` as a desktop would name it (on Windows, with its drive after the slash).
    fn uri_of(path: &Path) -> String {
        let path = path
            .to_string_lossy()
            .replace('\\', "/")
            .replace(' ', "%20");
        if path.starts_with('/') {
            format!("file://{path}")
        } else {
            format!("file:///{path}")
        }
    }

    /// A 16×8 PNG: its left half opaque red, its right half wholly see-through.
    fn png() -> Vec<u8> {
        let picture = image::RgbaImage::from_fn(16, 8, |x, _| {
            if x < 8 {
                image::Rgba([200, 10, 20, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            }
        });
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(picture)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn the_screen_from_the_x_server_is_saved_in_the_format_asked_for() {
        let dir = scratch("x");
        let fake = Fake {
            session: session(false, true),
            screen: Some(Frame {
                width: 2,
                height: 1,
                rgb: vec![10, 120, 250, 0, 0, 0],
            }),
            ..Fake::default()
        };
        let out = dir.join("snapshot.png");
        assert_eq!(take(&fake, Route::XServer, Format::Png, &out), Taken::Saved);
        let saved = image::open(&out).unwrap().to_rgb8();
        assert_eq!((saved.width(), saved.height()), (2, 1));
        assert_eq!(saved.get_pixel(0, 0).0, [10, 120, 250]);
        let out = dir.join("snapshot.jpg");
        assert_eq!(take(&fake, Route::XServer, Format::Jpg, &out), Taken::Saved);
        assert_eq!(
            image::guess_format(&std::fs::read(&out).unwrap()).unwrap(),
            image::ImageFormat::Jpeg
        );
        assert_eq!(*fake.asked.lock().unwrap(), ["x screen", "x screen"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_portals_file_becomes_the_snapshot() {
        // Where the desktop saves it has a space in its name, as GNOME's does.
        let dir = scratch("portal dir");
        let theirs = dir.join("Screenshot From 2026-11-26.png");
        std::fs::write(&theirs, png()).unwrap();
        let fake = Fake {
            session: session(true, false),
            portal: Mutex::new(Some(Asked::Saved(uri_of(&theirs)))),
            ..Fake::default()
        };
        // A PNG asked for and a PNG given: kept byte for byte, see-through pixel and all.
        let out = dir.join("snapshot.png");
        assert_eq!(
            take(
                &fake,
                Route::Portal { interactive: true },
                Format::Png,
                &out
            ),
            Taken::Saved
        );
        assert_eq!(std::fs::read(&out).unwrap(), png());
        assert!(theirs.exists(), "the desktop's own copy is left alone");
        // A JPEG asked for: drawn again, what was see-through now white.
        let out = dir.join("snapshot.jpg");
        assert_eq!(
            take(
                &fake,
                Route::Portal { interactive: false },
                Format::Jpg,
                &out
            ),
            Taken::Saved
        );
        let saved = image::open(&out).unwrap().to_rgb8();
        assert_eq!(
            image::guess_format(&std::fs::read(&out).unwrap()).unwrap(),
            image::ImageFormat::Jpeg
        );
        assert_eq!((saved.width(), saved.height()), (16, 8));
        let (red, white) = (saved.get_pixel(2, 3).0, saved.get_pixel(13, 3).0);
        assert!(red[0] > 150 && red[1] < 80, "{red:?}");
        assert!(white.iter().all(|c| *c > 200), "{white:?}");
        assert_eq!(*fake.asked.lock().unwrap(), ["pick", "portal screen"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_closed_picker_is_nothing_and_a_failure_is_said() {
        let dir = scratch("answers");
        let out = dir.join("snapshot.png");
        let pick = Route::Portal { interactive: true };
        let fake = Fake {
            session: session(true, true),
            portal: Mutex::new(Some(Asked::Cancelled)),
            ..Fake::default()
        };
        assert_eq!(take(&fake, pick, Format::Png, &out), Taken::Cancelled);
        assert!(!out.exists());

        let failed = |asked: Asked| {
            *fake.portal.lock().unwrap() = Some(asked);
            match take(&fake, pick, Format::Png, &out) {
                Taken::Failed(why) => why,
                other => panic!("{other:?}"),
            }
        };
        let why = failed(Asked::Failed(
            "Portal request didn't succeed with no information".into(),
        ));
        assert!(why.contains("didn't succeed"), "{why}");
        // A file that isn't there, a URI that names no file, and a file that is no picture.
        let why = failed(Asked::Saved(uri_of(&dir.join("gone.png"))));
        assert!(why.contains("couldn't be read"), "{why}");
        let why = failed(Asked::Saved("https://example.com/shot.png".into()));
        assert!(why.contains("https://example.com/shot.png"), "{why}");
        std::fs::write(dir.join("empty.png"), "").unwrap();
        let why = failed(Asked::Saved(uri_of(&dir.join("empty.png"))));
        assert!(why.contains("is empty"), "{why}");
        std::fs::write(dir.join("words.png"), "not a picture").unwrap();
        *fake.portal.lock().unwrap() = Some(Asked::Saved(uri_of(&dir.join("words.png"))));
        match take(&fake, pick, Format::Jpg, &out) {
            Taken::Failed(why) => assert!(why.contains("isn't a picture"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(!out.exists(), "nothing was saved");

        // An X server that refuses.
        match take(&fake, Route::XServer, Format::Png, &out) {
            Taken::Failed(why) => assert!(why.contains("BadMatch"), "{why}"),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_look_still_out_is_seen_until_the_desktop_answers() {
        assert!(!looking(), "no desktop under a test");
        {
            let _desktop = pretend(Arc::new(Fake::default()));
            assert!(!looking(), "a pretend desktop answers at once");
        }
        {
            let _desktop = pretend(Arc::new(Fake {
                looking: true,
                ..Fake::default()
            }));
            assert!(looking());
        }
        assert!(!looking());
    }

    #[test]
    fn a_pretended_desktop_is_the_one_asked_until_it_goes() {
        assert!(capture().is_none(), "never the computer's own under a test");
        {
            let _desktop = pretend(Arc::new(Fake {
                session: session(true, false),
                ..Fake::default()
            }));
            assert_eq!(capture().unwrap().session(), session(true, false));
            warm();
        }
        assert!(capture().is_none());
    }
}
