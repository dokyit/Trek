//! App-wide behaviour driven by settings and workspace state: keeping the Mac awake while agents
//! run, the Dock badge and icon, the menu bar item, the UI font size, and the notification sound.

use crate::workspace::Workspace;
use gpui_kit::component::Theme;
use gpui_kit::*;
use trek_core::settings::{AppIcon, NotifyMode};

/// What has been applied to the system so far; the observer only acts on differences.
struct Applied {
    /// Holds off idle sleep while held: macOS's power assertion for the process, released when
    /// dropped (and by the system when Trek quits or dies).
    awake: Option<Task<Result<ActivityGuard>>>,
    badge: usize,
    menu_bar_icon: bool,
    ui_font_size: f32,
    /// `None` until the first sync, so the chosen icon is put up at launch.
    app_icon: Option<AppIcon>,
}

/// Watch the workspace and keep the system in step. The check is a few comparisons per notify,
/// so it runs on every workspace change rather than on a timer.
pub fn init(workspace: Entity<Workspace>, cx: &mut App) {
    let ws = workspace.read(cx);
    let mut applied = Applied {
        awake: None,
        badge: 0,
        menu_bar_icon: ws.settings.notifications.menu_bar_icon,
        ui_font_size: 0.,
        app_icon: None,
    };
    follow_reduce_motion(&workspace, cx);
    sync(&workspace, &mut applied, cx);
    cx.observe(&workspace, move |workspace, cx| sync(&workspace, &mut applied, cx)).detach();
}

/// macOS's Reduce motion is on, as last asked.
static SYSTEM_REDUCES_MOTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// macOS's Reduce motion (Accessibility › Display) or Trek's own setting stills every animation:
/// `Workspace::motion` reads both, and gpui's and gpui-component's own (toasts coming and going,
/// dialogs sliding in) read the app's flag, which follows either. Nothing tells gpui when the
/// system's changes, so it's asked every couple of seconds; Trek's setting is followed in `sync`.
fn follow_reduce_motion(workspace: &Entity<Workspace>, cx: &mut App) {
    if cfg!(test) {
        return;
    }
    SYSTEM_REDUCES_MOTION.store(reduce_motion(), std::sync::atomic::Ordering::Relaxed);
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
            SYSTEM_REDUCES_MOTION.store(reduce_motion(), std::sync::atomic::Ordering::Relaxed);
            let gone = cx.update(|cx| match workspace.upgrade() {
                Some(ws) => {
                    apply_reduce_motion(&ws, cx);
                    false
                }
                None => true,
            });
            if gone {
                break;
            }
        }
    })
    .detach();
}

/// The app's reduce-motion flag: the system's preference or Trek's.
fn apply_reduce_motion(workspace: &Entity<Workspace>, cx: &mut App) {
    if cfg!(test) {
        return;
    }
    let on = SYSTEM_REDUCES_MOTION.load(std::sync::atomic::Ordering::Relaxed) || workspace.read(cx).settings.appearance.reduce_motion;
    if cx.reduce_motion() != on {
        cx.set_reduce_motion(on)
    }
}

fn sync(workspace: &Entity<Workspace>, applied: &mut Applied, cx: &mut App) {
    apply_reduce_motion(workspace, cx);
    let ws = workspace.read(cx);
    let awake = ws.settings.general.prevent_sleep_while_running && ws.any_turn_running();
    let badge = if ws.settings.notifications.dock_badge { ws.needs_you_count() } else { 0 };
    let menu_bar_icon = ws.settings.notifications.menu_bar_icon;
    let ui_font_size = ws.settings.appearance.ui_font_size();
    let app_icon = ws.settings.appearance.app_icon;

    if awake != applied.awake.is_some() {
        applied.awake = awake.then(|| cx.prevent_idle_sleep("Agents are working in Trek"));
        tracing::debug!("keep awake: {awake}");
    }
    if badge != applied.badge {
        applied.badge = badge;
        set_dock_badge(badge);
    }
    if ui_font_size != applied.ui_font_size {
        applied.ui_font_size = ui_font_size;
        apply_ui_font_size(ui_font_size, cx);
    }
    if applied.app_icon != Some(app_icon) {
        applied.app_icon = Some(app_icon);
        set_app_icon(app_icon);
    }
    if menu_bar_icon != applied.menu_bar_icon {
        applied.menu_bar_icon = menu_bar_icon;
        crate::tray::set_enabled(workspace.clone(), menu_bar_icon, cx);
    }
}

/// The base UI size is the theme's rem: every `text_sm()`-style size scales with it.
pub fn apply_ui_font_size(size: f32, cx: &mut App) {
    if Theme::global(cx).font_size != px(size) {
        Theme::update(cx, |theme| theme.font_size = px(size));
    }
}

/// Show `count` on the Dock tile (cleared at 0). Must run on the main thread; a no-op elsewhere.
#[cfg(all(target_os = "macos", not(test)))]
fn set_dock_badge(count: usize) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let tile = NSApplication::sharedApplication(mtm).dockTile();
    let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
    tile.setBadgeLabel(label.as_deref());
}

#[cfg(all(not(target_os = "macos"), not(test)))]
fn set_dock_badge(_: usize) {}

/// The rendered icon for `icon` (`assets/brand`).
pub fn app_icon_image(icon: AppIcon) -> &'static str {
    match icon {
        AppIcon::Ember => "brand/icon.png",
        AppIcon::Night => "brand/icon-night.png",
        AppIcon::Glass => "brand/icon-glass.png",
    }
}

/// Show `icon` in the Dock while Trek runs. The bundle's own icon (Ember) is what Finder and a
/// quit app's Dock tile show: macOS only lets a running app change its tile.
#[cfg(all(target_os = "macos", not(test)))]
fn set_app_icon(icon: AppIcon) {
    use objc2::{AllocAnyThread as _, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(bytes) = crate::assets::brand_bytes(app_icon_image(icon)) else { return };
    let data = NSData::with_bytes(&bytes);
    let image = NSImage::initWithData(NSImage::alloc(), &data);
    // SAFETY: on the main thread (`mtm`), with an image AppKit retains; `None` puts the bundle's
    // icon back.
    unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(image.as_deref()) };
}

#[cfg(all(not(target_os = "macos"), not(test)))]
fn set_app_icon(_: AppIcon) {}

#[cfg(test)]
fn set_app_icon(icon: AppIcon) {
    APP_ICON.with(|i| i.set(Some(icon)));
}

#[cfg(test)]
thread_local! {
    /// What tests would have shown on the Dock tile (each GPUI test runs on its own thread).
    pub static DOCK_BADGE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Alert sounds tests would have played.
    pub static SOUNDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The Dock icon tests would have shown.
    pub static APP_ICON: std::cell::Cell<Option<AppIcon>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn set_dock_badge(count: usize) {
    DOCK_BADGE.with(|b| b.set(count));
}

/// macOS's Reduce Transparency is on (Accessibility › Display): liquid glass stays off. Asked at
/// most once a second, as it's read on every frame.
#[cfg(target_os = "macos")]
pub fn reduce_transparency() -> bool {
    use std::cell::Cell;
    use std::time::{Duration, Instant};
    thread_local!(static ASKED: Cell<Option<(Instant, bool)>> = const { Cell::new(None) });
    ASKED.with(|a| match a.get() {
        Some((at, on)) if at.elapsed() < Duration::from_secs(1) => on,
        _ => {
            let on = objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency();
            a.set(Some((Instant::now(), on)));
            on
        }
    })
}

#[cfg(not(target_os = "macos"))]
pub fn reduce_transparency() -> bool {
    false
}

/// macOS's Reduce motion is on (Accessibility › Display).
#[cfg(target_os = "macos")]
fn reduce_motion() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

#[cfg(not(target_os = "macos"))]
fn reduce_motion() -> bool {
    false
}

/// Move `path` to the Trash, where the user can still get it back.
#[cfg(all(target_os = "macos", not(test)))]
pub fn trash(path: &std::path::Path) -> anyhow::Result<()> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, None).map_err(|e| anyhow::anyhow!("{}", e.localizedDescription()))
}

/// Tests' folders are their own: gone for good, not into the user's Trash.
#[cfg(any(not(target_os = "macos"), test))]
pub fn trash(path: &std::path::Path) -> anyhow::Result<()> {
    if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }.map_err(Into::into)
}

/// Whether Trek is the frontmost app (a relaunch after an update comes back to the front only then).
#[cfg(target_os = "macos")]
pub fn app_is_active() -> bool {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    MainThreadMarker::new().is_some_and(|mtm| NSApplication::sharedApplication(mtm).isActive())
}

#[cfg(not(target_os = "macos"))]
pub fn app_is_active() -> bool {
    true
}

/// Put `window` on screen behind every other app's windows, without making it key.
#[cfg(target_os = "macos")]
pub fn order_back(window: &Window) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return };
    // SAFETY: `ns_view` is the live NSView GPUI created for this window; both messages are
    // plain AppKit calls made on the main thread.
    unsafe {
        let view = appkit.ns_view.as_ptr().cast::<AnyObject>();
        let ns_window: *mut AnyObject = msg_send![view, window];
        if !ns_window.is_null() {
            let _: () = msg_send![ns_window, orderBack: std::ptr::null_mut::<AnyObject>()];
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn order_back(_: &Window) {}

/// Liquid glass from the system (macOS 26 and later): an `NSGlassEffectView` under the window's
/// content, which frosts whatever is behind the window. `on` adds it (once), off removes it.
/// Returns false where the system has no Liquid Glass; GPUI's blurred background stands in.
#[cfg(target_os = "macos")]
pub fn native_glass(window: &Window, on: bool) -> bool {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::NSRect;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Some(glass) = AnyClass::get(c"NSGlassEffectView") else { return false };
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return false };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return false };
    // SAFETY: plain AppKit messages on this window's live content view, on the main thread. The
    // glass view is retained by its superview once added; `alloc`/`init` hand over the one
    // reference that `addSubview:` then takes (released below).
    unsafe {
        let view = appkit.ns_view.as_ptr().cast::<AnyObject>();
        let ns_window: *mut AnyObject = msg_send![view, window];
        if ns_window.is_null() {
            return true;
        }
        let content: *mut AnyObject = msg_send![ns_window, contentView];
        if content.is_null() {
            return true;
        }
        let subviews: *mut AnyObject = msg_send![content, subviews];
        let count: usize = msg_send![subviews, count];
        let mut existing = vec![];
        for i in 0..count {
            let sub: *mut AnyObject = msg_send![subviews, objectAtIndex: i];
            let is_glass: bool = msg_send![sub, isKindOfClass: glass];
            if is_glass {
                existing.push(sub);
            }
        }
        match (on, existing.is_empty()) {
            (true, true) => {
                let frame: NSRect = msg_send![content, bounds];
                let g: *mut AnyObject = msg_send![glass, alloc];
                let g: *mut AnyObject = msg_send![g, initWithFrame: frame];
                // Width and height follow the window.
                let _: () = msg_send![g, setAutoresizingMask: 18u64];
                // Below everything GPUI draws (NSWindowBelow).
                let _: () = msg_send![content, addSubview: g, positioned: -1i64, relativeTo: std::ptr::null_mut::<AnyObject>()];
                let _: () = msg_send![g, release];
            }
            (false, false) => {
                for g in existing {
                    let _: () = msg_send![g, removeFromSuperview];
                }
            }
            _ => {}
        }
        true
    }
}

#[cfg(not(target_os = "macos"))]
pub fn native_glass(_: &Window, _: bool) -> bool {
    false
}

/// With `TREK_FORCE_ACTIVE`, a display link's worth of frame requests (60 Hz) for `window` while
/// macOS hides it, so measurements and screenshots of a window kept behind others
/// (`TREK_BACKGROUND`) cover drawing too. Every Trek window keeps one for as long as it's open.
pub fn hidden_frames<V: 'static>(window: &mut Window, cx: &mut Context<V>) -> Option<Task<()>> {
    crate::mascot::force_active().then(|| {
        cx.spawn_in(window, async move |_, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_micros(16_667)).await;
            if cx.update(|window, _| display_if_hidden(window)).is_err() {
                break;
            }
        })
    })
}

/// Have Core Animation draw `window` now if macOS counts it as hidden (covered by other windows,
/// or on another Space), where GPUI's display link is stopped. GPUI draws and presents the frame
/// if anything changed. Only for `TREK_FORCE_ACTIVE` measurements.
#[cfg(target_os = "macos")]
pub fn display_if_hidden(window: &Window) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    /// `NSWindowOcclusionStateVisible`.
    const VISIBLE: usize = 1 << 1;
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return };
    // SAFETY: `ns_view` is the live NSView GPUI created for this window; these are plain AppKit
    // and Core Animation calls made on the main thread.
    unsafe {
        let view = appkit.ns_view.as_ptr().cast::<AnyObject>();
        let ns_window: *mut AnyObject = msg_send![view, window];
        if ns_window.is_null() {
            return;
        }
        let occlusion: usize = msg_send![ns_window, occlusionState];
        let layer: *mut AnyObject = msg_send![view, layer];
        if occlusion & VISIBLE == 0 && !layer.is_null() {
            let _: () = msg_send![layer, setNeedsDisplay];
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn display_if_hidden(_: &Window) {}

/// What the disk said of a path a moment ago, for the views that ask each time they draw: a
/// worktree's folder, a project's `.git`, a path named in an answer. A stat a frame adds up
/// while text streams, and stalls the window on a slow or networked disk. Answers are kept for
/// two seconds (not at all in tests, which change the disk and look again at once).
pub mod lately {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    const KEPT: Duration = Duration::from_secs(if cfg!(test) { 0 } else { 2 });

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    enum Asked {
        Exists,
        IsDir,
        InRepo,
    }

    thread_local! {
        static ANSWERS: RefCell<HashMap<(Asked, PathBuf), (Instant, bool)>> = RefCell::default();
    }

    fn answer(asked: Asked, path: &Path, ask: impl FnOnce() -> bool) -> bool {
        ANSWERS.with(|a| {
            let mut a = a.borrow_mut();
            let now = Instant::now();
            if let Some((at, yes)) = a.get(&(asked, path.to_path_buf())) {
                if now.duration_since(*at) < KEPT {
                    return *yes;
                }
            }
            // Paths come and go with the threads on screen: start over rather than grow.
            if a.len() > 2000 {
                a.clear();
            }
            let yes = ask();
            a.insert((asked, path.to_path_buf()), (now, yes));
            yes
        })
    }

    pub fn exists(path: &Path) -> bool {
        answer(Asked::Exists, path, || path.exists())
    }

    pub fn is_dir(path: &Path) -> bool {
        answer(Asked::IsDir, path, || path.is_dir())
    }

    /// `workspace::in_repo`, for a view.
    pub fn in_repo(cwd: Option<&Path>) -> bool {
        cwd.is_some_and(|c| answer(Asked::InRepo, c, || crate::workspace::in_repo(Some(c))))
    }

    /// `Worktree::is_missing`, for a view.
    pub fn worktree_missing(w: &trek_core::worktree::Worktree) -> bool {
        !exists(&w.path.join(".git"))
    }
}

/// Play the alert sound off the main thread. One at a time: threads that finish together chime
/// once, rather than each start a player of its own (two hundred of them at once kept chiming
/// for minutes, and Trek crawled meanwhile).
#[cfg(not(test))]
pub fn play_alert_sound() {
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    static PLAYING: AtomicBool = AtomicBool::new(false);
    if PLAYING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let _ = Command::new("/usr/bin/afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        PLAYING.store(false, Ordering::SeqCst);
    });
}

#[cfg(test)]
pub fn play_alert_sound() {
    SOUNDS.with(|n| n.set(n.get() + 1));
}

/// The Windows release as Settings › About shows it: "Windows 11 24H2 (26100)". Read from the
/// registry: `ProductName` there still says "Windows 10" on 11, so the build number (22000 and
/// up is 11) names the major version.
#[cfg(windows)]
pub fn windows_version() -> String {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    fn value(name: &str) -> Option<String> {
        let key: Vec<u16> = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion".encode_utf16().chain([0]).collect();
        let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let mut buf = [0u16; 128];
        let mut size = std::mem::size_of_val(&buf) as u32;
        // SAFETY: both names end in a NUL; `buf` is `size` bytes long, and `size` is how many are
        // written (the terminator included) on success.
        let status = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size) };
        if status != 0 {
            return None;
        }
        let chars = (size as usize / 2).saturating_sub(1).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..chars])).filter(|s| !s.is_empty())
    }
    let build = value("CurrentBuild");
    let major = if build.as_deref().and_then(|b| b.parse::<u32>().ok()).is_some_and(|b| b >= 22000) { "Windows 11" } else { "Windows 10" };
    let mut out = major.to_string();
    out.extend(value("DisplayVersion").map(|v| format!(" {v}")));
    out.extend(build.map(|b| format!(" ({b})")));
    out
}

/// How to tell the user a thread needs them or finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alert {
    /// In-app toast (only for threads that aren't on screen).
    pub toast: bool,
    /// System notification banner.
    pub banner: bool,
    pub sound: bool,
}

/// Decide from the notification settings whether the window is focused and the thread on screen.
/// A thread you're looking at in the focused window never alerts.
pub fn alert_for(n: &trek_core::settings::Notifications, focused: bool, viewing: bool) -> Alert {
    let wanted = n.mode != NotifyMode::Off && !(focused && viewing) && (!focused || !n.only_when_unfocused);
    Alert {
        toast: !viewing,
        banner: wanted && matches!(n.mode, NotifyMode::Banner | NotifyMode::BannerAndSound),
        sound: wanted && matches!(n.mode, NotifyMode::Sound | NotifyMode::BannerAndSound),
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the gpui glob import brings its own `test` attribute.
    use super::{Alert, alert_for};
    use trek_core::settings::{Notifications, NotifyMode};

    #[test]
    fn alerts_follow_notification_settings() {
        let mut n = Notifications::default();
        // Default: banner + sound, only when unfocused.
        assert_eq!(alert_for(&n, false, false), Alert { toast: true, banner: true, sound: true });
        assert_eq!(alert_for(&n, true, false), Alert { toast: true, banner: false, sound: false });
        assert_eq!(alert_for(&n, false, true), Alert { toast: false, banner: true, sound: true });
        assert_eq!(alert_for(&n, true, true), Alert { toast: false, banner: false, sound: false });
        n.only_when_unfocused = false;
        assert_eq!(alert_for(&n, true, false), Alert { toast: true, banner: true, sound: true });
        assert_eq!(alert_for(&n, true, true), Alert { toast: false, banner: false, sound: false });
        n.mode = NotifyMode::Sound;
        assert_eq!(alert_for(&n, false, false), Alert { toast: true, banner: false, sound: true });
        n.mode = NotifyMode::Banner;
        assert_eq!(alert_for(&n, false, false), Alert { toast: true, banner: true, sound: false });
        n.mode = NotifyMode::Off;
        assert_eq!(alert_for(&n, false, false), Alert { toast: true, banner: false, sound: false });
    }
}
