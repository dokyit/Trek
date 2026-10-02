//! App-wide behaviour driven by settings and workspace state: keeping the Mac awake while agents
//! run, the Dock badge, the menu bar item, the UI font size, and the notification sound.

use crate::workspace::Workspace;
use gpui_kit::component::Theme;
use gpui_kit::*;
use std::process::{Child, Command, Stdio};
use trek_core::settings::NotifyMode;

/// What has been applied to the system so far; the observer only acts on differences.
struct Applied {
    caffeinate: Option<Child>,
    badge: usize,
    menu_bar_icon: bool,
    ui_font_size: f32,
}

/// Watch the workspace and keep the system in step. The check is a few comparisons per notify,
/// so it runs on every workspace change rather than on a timer.
pub fn init(workspace: Entity<Workspace>, cx: &mut App) {
    let ws = workspace.read(cx);
    let mut applied = Applied {
        caffeinate: None,
        badge: 0,
        menu_bar_icon: ws.settings.notifications.menu_bar_icon,
        ui_font_size: 0.,
    };
    sync(&workspace, &mut applied, cx);
    cx.observe(&workspace, move |workspace, cx| sync(&workspace, &mut applied, cx)).detach();
}

fn sync(workspace: &Entity<Workspace>, applied: &mut Applied, cx: &mut App) {
    let ws = workspace.read(cx);
    let awake = ws.settings.general.prevent_sleep_while_running && ws.any_turn_running();
    let badge = if ws.settings.notifications.dock_badge { ws.needs_you_count() } else { 0 };
    let menu_bar_icon = ws.settings.notifications.menu_bar_icon;
    let ui_font_size = ws.settings.appearance.ui_font_size();

    keep_awake(&mut applied.caffeinate, awake);
    if badge != applied.badge {
        applied.badge = badge;
        set_dock_badge(badge);
    }
    if ui_font_size != applied.ui_font_size {
        applied.ui_font_size = ui_font_size;
        apply_ui_font_size(ui_font_size, cx);
    }
    if menu_bar_icon != applied.menu_bar_icon {
        applied.menu_bar_icon = menu_bar_icon;
        crate::tray::set_enabled(workspace.clone(), menu_bar_icon, cx);
    }
}

/// One `caffeinate -i -w <pid>` while `on`: it holds off idle sleep and exits on its own if Trek dies.
fn keep_awake(child: &mut Option<Child>, on: bool) {
    if let Some(c) = child {
        // Respawn if it died underneath us (killed by hand, etc.).
        let alive = matches!(c.try_wait(), Ok(None));
        if !on || !alive {
            let _ = c.kill();
            let _ = c.wait();
            *child = None;
            if !on {
                tracing::debug!("caffeinate stopped");
            }
        }
    }
    if on && child.is_none() {
        match Command::new("/usr/bin/caffeinate")
            .args(["-i", "-w", &std::process::id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => {
                tracing::debug!("caffeinate started (pid {})", c.id());
                *child = Some(c);
            }
            Err(e) => tracing::warn!("caffeinate: {e}"),
        }
    }
}

/// The base UI size is the theme's rem: every `text_sm()`-style size scales with it.
pub fn apply_ui_font_size(size: f32, cx: &mut App) {
    if Theme::global(cx).font_size != px(size) {
        Theme::update(cx, |theme| theme.font_size = px(size));
    }
}

/// Show `count` on the Dock tile (cleared at 0). Must run on the main thread; a no-op elsewhere.
#[cfg(target_os = "macos")]
fn set_dock_badge(count: usize) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let tile = NSApplication::sharedApplication(mtm).dockTile();
    let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
    tile.setBadgeLabel(label.as_deref());
}

#[cfg(not(target_os = "macos"))]
fn set_dock_badge(_: usize) {}

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

/// Play the alert sound off the main thread.
pub fn play_alert_sound() {
    std::thread::spawn(|| {
        let _ = Command::new("/usr/bin/afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
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
