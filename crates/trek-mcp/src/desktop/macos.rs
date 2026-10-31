//! macOS: screenshots via `screencapture`, mouse/keyboard via CoreGraphics CGEvents, windows via
//! CGWindowList, apps via `lsappinfo` and `open`.

use std::time::Duration;

use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
    ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::{CGPoint, CGRect};
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowListExcludeDesktopElements,
    kCGWindowListOptionIncludingWindow, kCGWindowListOptionOnScreenOnly,
};

use super::{AppBundle, Button, Capture, Desktop, Display, Input, Rect, RunningApp, WindowInfo};
use crate::keys::{self, KeyCombo};
use crate::util::{self, TempFile};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

const SCREEN_RECORDING_ERROR: &str = "Screen Recording permission is missing, so trek-mcp cannot capture the screen. \
Grant it to the app that launched the agent (Trek, or your terminal) in System Settings ▸ Privacy & Security ▸ Screen & System Audio Recording, \
then restart the agent session.";

/// Said when the app macOS reads this helper's permissions from runs from a file that is gone.
const HOST_REPLACED: &str = "\nThe app this agent runs under (Trek, or the terminal) was replaced on disk since it started, \
so macOS has set its permissions aside although System Settings still shows them allowed: quit that app, open it again, \
then start the agent again.";

/// Whether the app macOS reads this helper's permissions from (the process responsible for it:
/// Trek, or the terminal that started the agent) runs from a file that is gone or was replaced.
/// macOS then refuses every permission check for it, until it restarts.
fn host_replaced_on_disk() -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    type Responsible = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;
    // SAFETY: a private but long-standing libSystem symbol, looked up rather than linked so a
    // macOS without it costs nothing; the path buffer is writable for the length passed.
    unsafe {
        let sym = libc::dlsym(
            libc::RTLD_DEFAULT,
            c"responsibility_get_pid_responsible_for_pid".as_ptr(),
        );
        if sym.is_null() {
            return false;
        }
        let responsible: Responsible = std::mem::transmute(sym);
        let host = responsible(std::process::id() as libc::pid_t);
        if host <= 0 {
            return false;
        }
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let n = libc::proc_pidpath(host, buf.as_mut_ptr().cast(), buf.len() as u32);
        let Ok(n) = usize::try_from(n) else {
            return false;
        };
        n > 0 && !std::path::Path::new(std::ffi::OsStr::from_bytes(&buf[..n])).exists()
    }
}

#[derive(Default)]
pub struct Mac;

impl Desktop for Mac {
    fn may_post_input(&self) -> bool {
        unsafe { AXIsProcessTrusted() != 0 }
    }

    fn permissions_note(&self) -> String {
        if host_replaced_on_disk() {
            HOST_REPLACED.to_string()
        } else {
            String::new()
        }
    }

    /// Once per run, ask macOS to prompt for Accessibility: the silent check alone never adds
    /// trek-mcp to the list, and a bundled helper can't be picked in the pane by hand.
    fn ask_to_post_input(&self) {
        use core_foundation::boolean::CFBoolean;
        static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if ASKED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let options = CFDictionary::from_CFType_pairs(&[(
            CFString::from_static_string("AXTrustedCheckOptionPrompt"),
            CFBoolean::true_value(),
        )]);
        unsafe {
            AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef());
        }
    }

    fn display(&self) -> Display {
        let display = CGDisplay::main();
        let bounds = display.bounds();
        let physical = display
            .display_mode()
            .map(|m| (m.pixel_width(), m.pixel_height()));
        Display {
            width: bounds.size.width,
            height: bounds.size.height,
            physical,
        }
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, String> {
        on_screen_windows()
    }

    fn capture(&self, region: Option<Rect>, max_side: u32) -> Result<Capture, String> {
        let tmp = TempFile::new("png");
        let mut capture_args: Vec<String> = vec!["-x".into(), "-t".into(), "png".into()];
        match region {
            Some((x0, y0, rw, rh)) => capture_args.push(format!(
                "-R{},{},{},{}",
                x0.round(),
                y0.round(),
                rw.round(),
                rh.round()
            )),
            None => capture_args.push("-m".into()), // main display only
        }
        capture_args.push(tmp.path_str().to_string());
        let arg_refs: Vec<&str> = capture_args.iter().map(String::as_str).collect();
        let out = util::run(
            "/usr/sbin/screencapture",
            &arg_refs,
            None,
            Duration::from_secs(20),
        )?;
        let size = std::fs::metadata(&tmp.0).map(|m| m.len()).unwrap_or(0);
        if !out.success() || size == 0 {
            let detail = if out.success() {
                "empty capture".to_string()
            } else {
                out.reason()
            };
            return Err(format!(
                "{SCREEN_RECORDING_ERROR}{}\n(screencapture: {detail})",
                self.permissions_note()
            ));
        }
        let note = if unsafe { CGPreflightScreenCaptureAccess() } {
            ""
        } else {
            "\nWarning: Screen Recording permission does not appear to be granted; other apps' windows may be missing from the image."
        };
        let (_, w0, h0) = util::load_png(&tmp.0)?;
        if w0.max(h0) > max_side {
            util::sips_fit(tmp.path_str(), max_side)?;
        }
        let (png_base64, width, height) = util::load_png(&tmp.0)?;
        Ok(Capture {
            png_base64,
            width,
            height,
            note,
        })
    }

    fn move_to(&self, x: f64, y: f64) -> Result<(), String> {
        post_mouse(
            CGEventType::MouseMoved,
            CGPoint::new(x, y),
            CGMouseButton::Left,
            None,
        )
    }

    fn click(&self, x: f64, y: f64, button: Button, count: u32) -> Result<(), String> {
        let (down, up, btn) = match button {
            Button::Left => (
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGMouseButton::Left,
            ),
            Button::Right => (
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGMouseButton::Right,
            ),
        };
        let pt = CGPoint::new(x, y);
        post_mouse(CGEventType::MouseMoved, pt, CGMouseButton::Left, None)?;
        sleep_ms(30);
        for i in 1..=count as i64 {
            post_mouse(down, pt, btn, Some(i))?;
            sleep_ms(15);
            post_mouse(up, pt, btn, Some(i))?;
            if i < count as i64 {
                sleep_ms(40);
            }
        }
        Ok(())
    }

    fn drag(&self, (ax, ay): (f64, f64), (bx, by): (f64, f64)) -> Result<(), String> {
        let start = CGPoint::new(ax, ay);
        post_mouse(CGEventType::MouseMoved, start, CGMouseButton::Left, None)?;
        sleep_ms(30);
        post_mouse(
            CGEventType::LeftMouseDown,
            start,
            CGMouseButton::Left,
            Some(1),
        )?;
        sleep_ms(60);
        const STEPS: i32 = 24;
        for i in 1..=STEPS {
            let t = i as f64 / STEPS as f64;
            let p = CGPoint::new(ax + (bx - ax) * t, ay + (by - ay) * t);
            post_mouse(
                CGEventType::LeftMouseDragged,
                p,
                CGMouseButton::Left,
                Some(1),
            )?;
            sleep_ms(12);
        }
        sleep_ms(40);
        post_mouse(
            CGEventType::LeftMouseUp,
            CGPoint::new(bx, by),
            CGMouseButton::Left,
            Some(1),
        )
    }

    fn scroll(&self, x: f64, y: f64, dx: i32, dy: i32) -> Result<(), String> {
        post_mouse(
            CGEventType::MouseMoved,
            CGPoint::new(x, y),
            CGMouseButton::Left,
            None,
        )?;
        sleep_ms(30);
        // CGEvent wheel deltas are positive for up/left; our API is positive for down/right.
        let ev = CGEvent::new_scroll_event(source()?, ScrollEventUnit::LINE, 2, -dy, -dx, 0)
            .map_err(|_| "Failed to create scroll event".to_string())?;
        ev.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn type_text(&self, s: &str) -> Result<(), String> {
        for segment in split_for_typing(s) {
            match segment {
                TypeSegment::Key(code) => press(KeyCombo {
                    keycode: code,
                    flags: 0,
                })?,
                TypeSegment::Text(chunk) => {
                    for down in [true, false] {
                        let ev = CGEvent::new_keyboard_event(source()?, 0, down)
                            .map_err(|_| "Failed to create keyboard event".to_string())?;
                        ev.set_flags(CGEventFlags::empty());
                        ev.set_string_from_utf16_unchecked(&chunk);
                        ev.post(CGEventTapLocation::HID);
                    }
                }
            }
            sleep_ms(8);
        }
        Ok(())
    }

    fn key(&self, combo: &KeyCombo) -> Result<(), String> {
        press(*combo)
    }

    fn open_app(&self, name: &str) -> Result<(), String> {
        util::run_ok(
            "/usr/bin/open",
            &["-a", name],
            None,
            Duration::from_secs(30),
        )
        .map(|_| ())
    }

    fn works_behind(&self) -> bool {
        true
    }

    fn window(&self, id: i64) -> Result<Option<WindowInfo>, String> {
        let Ok(id) = u32::try_from(id) else {
            return Ok(None);
        };
        let arr = copy_window_info(kCGWindowListOptionIncludingWindow, id)
            .ok_or("CGWindowListCopyWindowInfo returned nothing")?;
        Ok(arr
            .iter()
            .find_map(|item| window_info(*item as CFDictionaryRef, true))
            .filter(|w| w.id == id as i64))
    }

    /// `screencapture -l`, which draws the window from its own buffer: what covers it doesn't
    /// show. A zoom is cut from the full-resolution capture.
    fn capture_window(
        &self,
        w: &WindowInfo,
        region: Option<Rect>,
        max_side: u32,
    ) -> Result<Capture, String> {
        let tmp = TempFile::new("png");
        let out = util::run(
            "/usr/sbin/screencapture",
            &[
                "-x",
                "-o",
                "-t",
                "png",
                &format!("-l{}", w.id),
                tmp.path_str(),
            ],
            None,
            Duration::from_secs(20),
        )?;
        let size = std::fs::metadata(&tmp.0).map(|m| m.len()).unwrap_or(0);
        if !out.success() || size == 0 {
            if !unsafe { CGPreflightScreenCaptureAccess() } {
                return Err(format!(
                    "{SCREEN_RECORDING_ERROR}{}",
                    self.permissions_note()
                ));
            }
            let detail = if out.success() {
                "empty capture".to_string()
            } else {
                out.reason()
            };
            return Err(format!(
                "Couldn't capture {}'s window ({detail}). It may have closed or been minimised: list_windows shows what's open.",
                w.owner
            ));
        }
        let (_, w0, h0) = util::load_png(&tmp.0)?;
        if let Some((x, y, rw, rh)) = region {
            // Pixels per point in this capture (2 on a Retina display).
            let scale = w0 as f64 / w.w.max(1.0);
            // At least a pixel in: sips takes an offset of 0 as none, and cuts from the centre.
            let (cx, cy) = ((x * scale).round().max(1.0), (y * scale).round().max(1.0));
            let cw = (rw * scale).round().clamp(1.0, w0 as f64 - cx);
            let ch = (rh * scale).round().clamp(1.0, h0 as f64 - cy);
            let (cw, ch, cx, cy) = (
                cw.to_string(),
                ch.to_string(),
                cx.to_string(),
                cy.to_string(),
            );
            util::run_ok(
                "/usr/bin/sips",
                &[
                    "-c",
                    &ch,
                    &cw,
                    "--cropOffset",
                    &cy,
                    &cx,
                    tmp.path_str(),
                    "--out",
                    tmp.path_str(),
                ],
                None,
                Duration::from_secs(30),
            )
            .map_err(|e| format!("sips failed to cut the zoom out: {e}"))?;
        }
        let (_, w1, h1) = util::load_png(&tmp.0)?;
        if w1.max(h1) > max_side {
            util::sips_fit(tmp.path_str(), max_side)?;
        }
        let (png_base64, width, height) = util::load_png(&tmp.0)?;
        Ok(Capture {
            png_base64,
            width,
            height,
            note: "",
        })
    }

    fn behind(&self, w: &WindowInfo, input: &Input) -> Result<(), String> {
        super::skylight::deliver(w, input)
    }

    /// `lsappinfo list`: the apps with a Dock icon or a menu bar item, not background daemons.
    fn running_apps(&self) -> Vec<RunningApp> {
        util::run_ok(
            "/usr/bin/lsappinfo",
            &["list"],
            None,
            Duration::from_secs(5),
        )
        .map(|out| parse_lsappinfo_list(&out))
        .unwrap_or_default()
    }

    /// A path as given; else the usual folders, then Spotlight, by name or bundle id.
    fn installed_apps(&self, name: &str) -> Vec<AppBundle> {
        let name = name.trim().trim_end_matches('/');
        let mut paths: Vec<String> = vec![];
        if name.contains('/') {
            paths.push(name.to_string());
        } else if !name.contains('"') && !name.is_empty() {
            let file = format!("{}.app", name.strip_suffix(".app").unwrap_or(name));
            let home = std::env::var("HOME").unwrap_or_default();
            for dir in [
                "/Applications",
                "/Applications/Utilities",
                "/System/Applications",
                "/System/Applications/Utilities",
                "/System/Library/CoreServices",
                &format!("{home}/Applications"),
            ] {
                paths.push(format!("{dir}/{file}"));
            }
            let stem = file.trim_end_matches(".app");
            let query = if looks_like_bundle_id(name) {
                format!("kMDItemCFBundleIdentifier == \"{name}\"")
            } else {
                format!(
                    "kMDItemContentType == \"com.apple.application-bundle\" && (kMDItemFSName == \"{file}\"c || kMDItemDisplayName == \"{stem}\"c)"
                )
            };
            if let Ok(out) =
                util::run_ok("/usr/bin/mdfind", &[&query], None, Duration::from_secs(5))
            {
                paths.extend(out.lines().map(str::to_string));
            }
        }
        let mut seen = std::collections::HashSet::new();
        paths
            .into_iter()
            .filter(|p| p.ends_with(".app") && seen.insert(p.clone()))
            .filter(|p| std::path::Path::new(p).join("Contents").is_dir())
            .map(|path| AppBundle {
                bundle_id: bundle_id(&path),
                path,
            })
            .collect()
    }

    /// `open -g -a <path>`: that copy, left behind the user's front app.
    fn launch_behind(&self, app: &AppBundle) -> Result<(), String> {
        eprintln!(
            "trek-mcp computer: launching {} ({}) behind the user's windows",
            app.path,
            app.bundle_id.as_deref().unwrap_or("no bundle id")
        );
        util::run_ok(
            "/usr/bin/open",
            &["-g", "-a", &app.path],
            None,
            Duration::from_secs(30),
        )
        .map(|_| ())
    }

    /// `open -g` on the running copy's own bundle: Launch Services hands that process a reopen
    /// (it makes a window if it has none) instead of starting another.
    fn reopen_behind(&self, app: &RunningApp) -> Result<(), String> {
        let path = app
            .path
            .as_deref()
            .ok_or_else(|| format!("{} has no bundle to reopen it by.", app.name))?;
        eprintln!(
            "trek-mcp computer: asking the running {} (pid {}, {path}) for a window",
            app.name, app.pid
        );
        util::run_ok(
            "/usr/bin/open",
            &["-g", "-a", path],
            None,
            Duration::from_secs(30),
        )
        .map(|_| ())
    }

    /// Since the last key, click or pointer movement, by the HID system's count. Events posted
    /// to one app, as background use posts them, sometimes reset it too: `computer` tells its
    /// own apart by when it posted them.
    fn user_idle(&self) -> Option<Duration> {
        // kCGEventSourceStateHIDSystemState, kCGAnyInputEventType.
        let secs = unsafe { CGEventSourceSecondsSinceLastEventType(1, u32::MAX) };
        (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
    }

    fn screen_locked(&self) -> bool {
        // SAFETY: a copy of the session's dictionary, released by the wrapper; null outside a
        // login session.
        let dict = unsafe { CGSessionCopyCurrentDictionary() };
        if dict.is_null() {
            return true;
        }
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_create_rule(dict) };
        dict.find(CFString::from_static_string("CGSSessionScreenIsLocked"))
            .and_then(|v| v.downcast::<core_foundation::boolean::CFBoolean>())
            .is_some_and(bool::from)
    }

    fn bring_forward(&self, w: &WindowInfo) -> Result<(), String> {
        super::skylight::bring_forward(w)
    }
}

/// A bundle id rather than an app's name: `com.apple.TextEdit`.
fn looks_like_bundle_id(name: &str) -> bool {
    name.contains('.')
        && !name.ends_with(".app")
        && !name.contains(char::is_whitespace)
        && name.split('.').count() >= 2
}

/// The bundle id in an app's Info.plist.
fn bundle_id(path: &str) -> Option<String> {
    let url = core_foundation::url::CFURL::from_path(path, true)?;
    let bundle = core_foundation::bundle::CFBundle::new(url)?;
    bundle
        .info_dictionary()
        .find(CFString::from_static_string("CFBundleIdentifier"))
        .and_then(|v| v.downcast::<CFString>())
        .map(|s| s.to_string())
}

/// The apps in `lsappinfo list` output that the user can see (type Foreground or UIElement).
fn parse_lsappinfo_list(out: &str) -> Vec<RunningApp> {
    let quoted = |s: &str| -> Option<String> {
        let s = s.trim();
        let s = s.strip_prefix('"')?;
        Some(s[..s.find('"')?].to_string())
    };
    let mut apps = vec![];
    let mut cur: Option<(RunningApp, bool)> = None;
    let done = |cur: Option<(RunningApp, bool)>, apps: &mut Vec<RunningApp>| {
        if let Some((app, seen)) = cur
            && seen
            && app.pid > 0
        {
            apps.push(app);
        }
    };
    for line in out.lines() {
        let t = line.trim_start();
        // `123) "TextEdit" ASN:0x0-0x1a9aa99: (in front)`
        if let Some((n, rest)) = t.split_once(") \"")
            && n.chars().all(|c| c.is_ascii_digit())
            && !n.is_empty()
        {
            done(cur.take(), &mut apps);
            let name = rest.split_once("\" ASN:").map(|(n, _)| n).unwrap_or(rest);
            cur = Some((
                RunningApp {
                    name: name.to_string(),
                    bundle_id: None,
                    path: None,
                    pid: 0,
                },
                false,
            ));
            continue;
        }
        let Some((app, seen)) = cur.as_mut() else {
            continue;
        };
        if let Some(v) = t.strip_prefix("bundleID=") {
            app.bundle_id = quoted(v);
        } else if let Some(v) = t.strip_prefix("bundle path=") {
            app.path = quoted(v);
        } else if let Some(v) = t.strip_prefix("pid = ") {
            app.pid = v
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0);
            *seen = t.contains("type=\"Foreground\"") || t.contains("type=\"UIElement\"");
        }
    }
    done(cur.take(), &mut apps);
    apps
}

// ---- CoreGraphics helpers ----

fn source() -> Result<CGEventSource, String> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "Failed to create CGEventSource".to_string())
}

fn post_mouse(
    ty: CGEventType,
    pt: CGPoint,
    button: CGMouseButton,
    click_state: Option<i64>,
) -> Result<(), String> {
    let ev = CGEvent::new_mouse_event(source()?, ty, pt, button)
        .map_err(|_| "Failed to create mouse event".to_string())?;
    // Don't inherit modifier keys the user may be holding.
    ev.set_flags(CGEventFlags::empty());
    if let Some(n) = click_state {
        ev.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, n);
    }
    ev.post(CGEventTapLocation::HID);
    Ok(())
}

fn press(combo: KeyCombo) -> Result<(), String> {
    let flags = CGEventFlags::from_bits_truncate(combo.flags);
    for down in [true, false] {
        let ev = CGEvent::new_keyboard_event(source()?, combo.keycode, down)
            .map_err(|_| "Failed to create keyboard event".to_string())?;
        ev.set_flags(flags);
        ev.post(CGEventTapLocation::HID);
        if down {
            sleep_ms(15);
        }
    }
    Ok(())
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

#[derive(Debug, PartialEq)]
pub(super) enum TypeSegment {
    Key(u16),
    /// UTF-16 chunk (≤ 20 units, the CGEventKeyboardSetUnicodeString limit).
    Text(Vec<u16>),
}

pub(super) fn split_for_typing(s: &str) -> Vec<TypeSegment> {
    const MAX_UNITS: usize = 20;
    let mut out = Vec::new();
    let mut cur: Vec<u16> = Vec::new();
    let s = s.replace("\r\n", "\n");
    for ch in s.chars() {
        let key = match ch {
            '\n' | '\r' => Some(keys::KEY_RETURN),
            '\t' => Some(keys::KEY_TAB),
            _ => None,
        };
        if let Some(code) = key {
            if !cur.is_empty() {
                out.push(TypeSegment::Text(std::mem::take(&mut cur)));
            }
            out.push(TypeSegment::Key(code));
            continue;
        }
        let mut buf = [0u16; 2];
        let units = ch.encode_utf16(&mut buf);
        if cur.len() + units.len() > MAX_UNITS {
            out.push(TypeSegment::Text(std::mem::take(&mut cur)));
        }
        cur.extend_from_slice(units);
    }
    if !cur.is_empty() {
        out.push(TypeSegment::Text(cur));
    }
    out
}

fn on_screen_windows() -> Result<Vec<WindowInfo>, String> {
    let arr = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )
    .ok_or("CGWindowListCopyWindowInfo returned nothing")?;
    Ok(arr
        .iter()
        .filter_map(|item| window_info(*item as CFDictionaryRef, false))
        .collect())
}

/// An app's window (layer 0, at least 2x2) from its window-list entry. `on_screen`: only one
/// that's on screen now (not minimised, not on another Space).
fn window_info(item: CFDictionaryRef, on_screen: bool) -> Option<WindowInfo> {
    let key = CFString::from_static_string;
    let dict: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(item) };
    let get_str = |k: &'static str| {
        dict.find(key(k))
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
            .unwrap_or_default()
    };
    let get_num = |k: &'static str| {
        dict.find(key(k))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .unwrap_or(-1)
    };
    if get_num("kCGWindowLayer") != 0 {
        return None;
    }
    if on_screen
        && !dict
            .find(key("kCGWindowIsOnscreen"))
            .and_then(|v| v.downcast::<core_foundation::boolean::CFBoolean>())
            .is_some_and(bool::from)
    {
        return None;
    }
    let bounds = dict
        .find(key("kCGWindowBounds"))
        .and_then(|v| v.downcast::<CFDictionary>())
        .and_then(|d| CGRect::from_dict_representation(&d))?;
    if bounds.size.width < 2.0 || bounds.size.height < 2.0 {
        return None;
    }
    Some(WindowInfo {
        owner: get_str("kCGWindowOwnerName"),
        title: get_str("kCGWindowName"),
        pid: get_num("kCGWindowOwnerPID"),
        id: get_num("kCGWindowNumber"),
        x: bounds.origin.x,
        y: bounds.origin.y,
        w: bounds.size.width,
        h: bounds.size.height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_running_apps_are_read_from_lsappinfo() {
        // As macOS 27 prints it: a regular app, a background helper, one with no bundle id.
        let out = r#" 3) "universalaccessd" ASN:0x0-0xc00c:
    bundleID=[ NULL ]
    bundle path="/usr/sbin/universalaccessd"
    pid = 683 token=[sess=100021 pid=683 uid:501,501,501 g:20,20 pV:1664] !signalled type="BackgroundOnly" flavor=3 Version=[ NULL ]  fileType="????" creator="????" Arch=ARM64
123) "Touchline 27" ASN:0x0-0x1a9aa99: (in front)
    bundleID="local.tobias.Touchline27"
    bundle path="/Users/tobias/Applications/Touchline 27.app"
    executable path="/Users/tobias/Applications/Touchline 27.app/Contents/MacOS/FM26MacEditor"
    pid = 42521 token=[sess=100021 pid=42521 uid:501,501,501 g:20,20 pV:2790489] type="Foreground" flavor=3 Version="4" fileType="APPL" creator="????" Arch=ARM64
    coalition: 57984  { 42521 42523 42580 42582 }
124) "Raycast" ASN:0x0-0x1a9ea9d:
    bundleID=[ NULL ]
    bundle path="/Applications/Raycast.app"
    pid = 900 token=[sess=100021 pid=900 uid:501] type="UIElement" flavor=3
"#;
        assert_eq!(
            parse_lsappinfo_list(out),
            vec![
                RunningApp {
                    name: "Touchline 27".into(),
                    bundle_id: Some("local.tobias.Touchline27".into()),
                    path: Some("/Users/tobias/Applications/Touchline 27.app".into()),
                    pid: 42521,
                },
                RunningApp {
                    name: "Raycast".into(),
                    bundle_id: None,
                    path: Some("/Applications/Raycast.app".into()),
                    pid: 900,
                },
            ]
        );
        assert!(parse_lsappinfo_list("").is_empty());
        assert!(
            looks_like_bundle_id("com.apple.TextEdit") && !looks_like_bundle_id("TextEdit.app")
        );
        assert!(!looks_like_bundle_id("Touchline 27") && !looks_like_bundle_id("Notes"));
    }

    #[test]
    fn typing_segments() {
        let segs = split_for_typing("hi\r\nyou\tthere");
        assert_eq!(
            segs,
            vec![
                TypeSegment::Text("hi".encode_utf16().collect()),
                TypeSegment::Key(36),
                TypeSegment::Text("you".encode_utf16().collect()),
                TypeSegment::Key(48),
                TypeSegment::Text("there".encode_utf16().collect()),
            ]
        );
        // long text is chunked at 20 UTF-16 units without splitting surrogate pairs
        let s = "a".repeat(19) + "😀" + "b";
        let segs = split_for_typing(&s);
        assert_eq!(segs.len(), 2);
        assert!(matches!(&segs[0], TypeSegment::Text(u) if u.len() == 19));
        assert!(matches!(&segs[1], TypeSegment::Text(u) if u.len() == 3));
    }
}
