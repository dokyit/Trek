//! Windows: input via `SendInput`, windows via `EnumWindows` and DWM, screenshots via GDI, apps via
//! `ShellExecuteW`. trek-mcp is per-monitor DPI aware, so every coordinate here is a physical pixel
//! with the primary display's top-left at (0, 0).
//!
//! The pure parts (coordinate normalisation, key strokes, the window filter, app lookup) are at
//! the top and tested; the Win32 calls below them only translate.

use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GdiFlush, GetDC, HALFTONE, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
    SetBrushOrgEx, SetStretchBltMode, StretchBlt,
};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MAPVK_VK_TO_VSC, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, MapVirtualKeyW,
    SendInput,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GA_ROOT, GWL_EXSTYLE, GetAncestor, GetClassNameW, GetForegroundWindow, GetSystemMetrics, GetWindowLongW,
    GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SM_CXSCREEN, SM_CXVIRTUALSCREEN,
    SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SPI_GETWHEELSCROLLLINES, SW_SHOWNORMAL,
    SystemParametersInfoW, WS_EX_TRANSPARENT, WindowFromPoint,
};

use super::{Button, Capture, Desktop, Display, Rect, WindowInfo};
use crate::keys::{self, VkCombo};

// ---- pure: tested below ----

/// A screen coordinate as `SendInput` wants it: 0..=65535 across the virtual screen (every
/// display) that starts at `origin` and is `size` pixels long. Aims at the pixel's centre, so
/// Windows' own rounding lands on it.
fn normalise(p: f64, origin: i32, size: i32) -> i32 {
    let size = size.max(1) as i64;
    let px = (p.round() as i64 - origin as i64).clamp(0, size - 1);
    ((2 * px + 1) * 32768 / size).clamp(0, 65535) as i32
}

/// Width and height fitted inside `max_side` on the long side, aspect kept, never upscaled.
fn fit(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    let long = w.max(h).max(1);
    if long <= max_side {
        return (w.max(1), h.max(1));
    }
    let s = max_side as f64 / long as f64;
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}

/// Wheel units for `lines` lines: a notch (120) scrolls the user's lines-per-notch setting, so a
/// line is a share of it. "One screen at a time" (or a nonsense setting) counts a notch a line.
fn wheel_delta(lines: i32, lines_per_notch: u32) -> i32 {
    const WHEEL_DELTA: i32 = 120;
    let per_line = match lines_per_notch {
        1..=100 => WHEEL_DELTA / lines_per_notch as i32,
        _ => WHEEL_DELTA,
    };
    lines.saturating_mul(per_line.max(1))
}

/// One key going down or up: a virtual key, or a UTF-16 unit typed as itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stroke {
    Key { vk: u16, up: bool },
    Unit { unit: u16, up: bool },
}

/// A combo's strokes: the modifiers down in order, the key down and up, the modifiers up in
/// reverse.
fn chord_strokes(combo: &VkCombo) -> Vec<Stroke> {
    let mut out: Vec<Stroke> = combo.held.iter().map(|&vk| Stroke::Key { vk, up: false }).collect();
    out.push(Stroke::Key { vk: combo.vk, up: false });
    out.push(Stroke::Key { vk: combo.vk, up: true });
    out.extend(combo.held.iter().rev().map(|&vk| Stroke::Key { vk, up: true }));
    out
}

/// Text as strokes: newlines press Return and tabs Tab (as on the Mac); everything else is typed
/// as Unicode, a surrogate pair as its two units.
fn text_strokes(s: &str) -> Vec<Stroke> {
    let s = s.replace("\r\n", "\n");
    let mut out = Vec::new();
    for ch in s.chars() {
        let vk = match ch {
            '\n' | '\r' => Some(keys::VK_RETURN),
            '\t' => Some(keys::VK_TAB),
            _ => None,
        };
        if let Some(vk) = vk {
            out.extend([Stroke::Key { vk, up: false }, Stroke::Key { vk, up: true }]);
            continue;
        }
        let mut buf = [0u16; 2];
        for &unit in ch.encode_utf16(&mut buf).iter() {
            out.extend([Stroke::Unit { unit, up: false }, Stroke::Unit { unit, up: true }]);
        }
    }
    out
}

/// Keys whose scan code has the 0xE0 prefix: without the extended flag, an arrow is read as the
/// number pad's.
fn is_extended(vk: u16) -> bool {
    matches!(
        vk,
        0x21..=0x28 // page up/down, end, home, arrows
            | 0x2D | 0x2E // insert, delete
            | 0x5B | 0x5C // left and right Windows keys
            | 0x6F // numpad divide
            | 0x90 // num lock
            | 0xA3 | 0xA5 // right ctrl, right alt
    )
}

/// The app name from an executable's full path: `C:\Program Files\Trek\trek.exe` → `trek`.
fn exe_name(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    match file.len().checked_sub(4) {
        Some(cut) if file.is_char_boundary(cut) && file[cut..].eq_ignore_ascii_case(".exe") => file[..cut].to_string(),
        _ => file.to_string(),
    }
}

/// What the window filter looks at.
struct Raw {
    visible: bool,
    /// Hidden by DWM: a suspended Store app, a window on another virtual desktop.
    cloaked: bool,
    iconic: bool,
    ex_style: u32,
    class: String,
    w: f64,
    h: f64,
}

/// Whether a top-level window is listed. Like the Mac's on-screen list: shown, not minimised and
/// not the desktop itself. Click-through windows (overlays) are left out too: a click goes
/// through them to the window below, which is the one the Trek check must see.
fn listed(r: &Raw) -> bool {
    r.visible
        && !r.cloaked
        && !r.iconic
        && r.ex_style & WS_EX_TRANSPARENT == 0
        && !matches!(r.class.as_str(), "Progman" | "WorkerW")
        && r.w >= 2.0
        && r.h >= 2.0
}

/// What to hand `ShellExecuteW` for an app `name`: a path or URI as given; else a Start menu
/// shortcut named like it ("Visual Studio Code"); else the program App Paths registers for it
/// ("chrome" → chrome.exe's path); else the name itself, for Windows to find on PATH.
fn resolve_app(name: &str, shortcut: impl Fn(&str) -> Option<String>, app_path: impl Fn(&str) -> Option<String>) -> String {
    let is_path = name.contains(['\\', '/']);
    let is_uri = name.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() >= 2 && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    if is_path || is_uri {
        return name.to_string();
    }
    if let Some(lnk) = shortcut(name) {
        return lnk;
    }
    let exe = if name.to_ascii_lowercase().ends_with(".exe") { name.to_string() } else { format!("{name}.exe") };
    app_path(&exe).unwrap_or_else(|| name.to_string())
}

/// Why `ShellExecuteW` didn't open something, from what it returned (≤ 32).
fn shell_error(code: isize) -> String {
    match code {
        0 | 8 => "Windows is out of memory".into(),
        2 | 3 => "there's no app, program or file by that name".into(),
        5 => "access is denied".into(),
        26 | 32 => "a file it needs is in use".into(),
        27 | 31 => "nothing is set to open it".into(),
        _ => format!("Windows couldn't open it (ShellExecute error {code})"),
    }
}

// ---- Win32 ----

pub struct Win;

impl Default for Win {
    fn default() -> Self {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            // SAFETY: both calls take a constant and a null reserved pointer, and run once,
            // before this process has asked Windows for any coordinate.
            unsafe {
                // Coordinates, window bounds and captures in physical pixels, matching each other
                // on every display whatever its scaling. Fails only if already set, which is fine.
                SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
                // ShellExecute may hand off to shell extensions that need COM on this thread.
                CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32);
            }
        });
        Win
    }
}

impl Desktop for Win {
    /// Windows asks no permission to send input (UIPI quietly drops it into elevated windows).
    fn may_post_input(&self) -> bool {
        true
    }

    fn display(&self) -> Display {
        // SAFETY: `GetSystemMetrics` takes an index and no pointers.
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        Display { width: w.max(1) as f64, height: h.max(1) as f64, physical: None }
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, String> {
        unsafe extern "system" fn collect(hwnd: HWND, out: LPARAM) -> i32 {
            // SAFETY: `out` is the `&mut Vec<HWND>` passed to `EnumWindows` below, which calls this
            // synchronously on this thread while that vector is alive and not otherwise borrowed.
            unsafe { (*(out as *mut Vec<HWND>)).push(hwnd) };
            1
        }
        let mut hwnds: Vec<HWND> = Vec::new();
        // Top-level windows in z-order, front first.
        // SAFETY: `collect` has the callback's signature and `hwnds` outlives the call (see there).
        if unsafe { EnumWindows(Some(collect), &mut hwnds as *mut Vec<HWND> as LPARAM) } == 0 {
            return Err("EnumWindows failed".into());
        }
        let mut names = std::collections::HashMap::new();
        Ok(hwnds.into_iter().filter(|&h| listed(&raw(h))).map(|h| describe(h, &mut names)).collect())
    }

    /// The foreground window takes the keys, whichever window is topmost.
    fn key_window(&self) -> Result<Option<WindowInfo>, String> {
        // SAFETY: takes no arguments; the handle it returns is only ever passed back to Windows,
        // which checks it (a window closed since just fails the calls).
        let hwnd = unsafe { GetForegroundWindow() };
        Ok((!hwnd.is_null()).then(|| describe(hwnd, &mut Default::default())))
    }

    /// Windows' own hit test, which knows about click-through and child windows.
    fn owner_at(&self, x: f64, y: f64) -> Option<String> {
        // SAFETY: a point by value in, a handle out; Windows validates the handle (null is
        // allowed and answers null) in `GetAncestor` as everywhere below.
        let child = unsafe { WindowFromPoint(POINT { x: x.round() as i32, y: y.round() as i32 }) };
        let root = unsafe { GetAncestor(child, GA_ROOT) };
        let hwnd = if root.is_null() { child } else { root };
        (!hwnd.is_null()).then(|| process_name(window_pid(hwnd)))
    }

    fn capture(&self, region: Option<Rect>, max_side: u32) -> Result<Capture, String> {
        let (sx, sy, sw, sh) = match region {
            Some((x, y, w, h)) => (x.round() as i32, y.round() as i32, w.round().max(1.0) as i32, h.round().max(1.0) as i32),
            None => {
                let d = self.display();
                (0, 0, d.width as i32, d.height as i32)
            }
        };
        let (tw, th) = fit(sw as u32, sh as u32, max_side);
        let rgb = grab(sx, sy, sw, sh, tw as i32, th as i32)?;
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, tw, th);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder
            .write_header()
            .and_then(|mut w| w.write_image_data(&rgb))
            .map_err(|e| format!("Couldn't encode the screenshot: {e}"))?;
        use base64::Engine as _;
        Ok(Capture { png_base64: base64::engine::general_purpose::STANDARD.encode(&png), width: tw, height: th, note: "" })
    }

    fn move_to(&self, x: f64, y: f64) -> Result<(), String> {
        send(&[mouse(x, y, MOUSEEVENTF_MOVE, 0)])
    }

    fn click(&self, x: f64, y: f64, button: Button, count: u32) -> Result<(), String> {
        let (down, up) = match button {
            Button::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        };
        send(&[mouse(x, y, MOUSEEVENTF_MOVE, 0)])?;
        sleep_ms(30);
        for i in 1..=count {
            // Down and up together, so nothing slips between them.
            send(&[mouse(x, y, MOUSEEVENTF_MOVE | down, 0), mouse(x, y, MOUSEEVENTF_MOVE | up, 0)])?;
            if i < count {
                sleep_ms(40);
            }
        }
        Ok(())
    }

    fn drag(&self, (ax, ay): (f64, f64), (bx, by): (f64, f64)) -> Result<(), String> {
        send(&[mouse(ax, ay, MOUSEEVENTF_MOVE, 0)])?;
        sleep_ms(30);
        send(&[mouse(ax, ay, MOUSEEVENTF_MOVE | MOUSEEVENTF_LEFTDOWN, 0)])?;
        sleep_ms(60);
        const STEPS: i32 = 24;
        let moved = (1..=STEPS).try_for_each(|i| {
            let t = i as f64 / STEPS as f64;
            sleep_ms(12);
            send(&[mouse(ax + (bx - ax) * t, ay + (by - ay) * t, MOUSEEVENTF_MOVE, 0)])
        });
        sleep_ms(40);
        // Let go even if a move failed, so the button isn't left held.
        let released = send(&[mouse(bx, by, MOUSEEVENTF_MOVE | MOUSEEVENTF_LEFTUP, 0)]);
        moved.and(released)
    }

    fn scroll(&self, x: f64, y: f64, dx: i32, dy: i32) -> Result<(), String> {
        send(&[mouse(x, y, MOUSEEVENTF_MOVE, 0)])?;
        sleep_ms(30);
        let mut lines: u32 = 3;
        // SAFETY: for SPI_GETWHEELSCROLLLINES the pointer is a `UINT` to fill, which `lines` is;
        // on failure it keeps the default.
        unsafe { SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &mut lines as *mut u32 as *mut _, 0) };
        let mut inputs = Vec::new();
        // The wheel is positive away from the user (up); our dy is positive down. A horizontal
        // wheel is positive to the right, like dx.
        if dy != 0 {
            inputs.push(mouse(x, y, MOUSEEVENTF_WHEEL, -wheel_delta(dy, lines)));
        }
        if dx != 0 {
            inputs.push(mouse(x, y, MOUSEEVENTF_HWHEEL, wheel_delta(dx, lines)));
        }
        send(&inputs)
    }

    fn type_text(&self, s: &str) -> Result<(), String> {
        // A few characters at a time, with a breath between, like the Mac's 20-unit chunks.
        for chunk in text_strokes(s).chunks(40) {
            send(&chunk.iter().map(|&s| key_input(s)).collect::<Vec<_>>())?;
            sleep_ms(8);
        }
        Ok(())
    }

    fn key(&self, combo: &VkCombo) -> Result<(), String> {
        let strokes = chord_strokes(combo);
        let (down, up) = strokes.split_at(combo.held.len() + 1);
        let pressed = send(&down.iter().map(|&s| key_input(s)).collect::<Vec<_>>());
        sleep_ms(15);
        // Release whatever happened, so no modifier is left held down.
        let released = send(&up.iter().map(|&s| key_input(s)).collect::<Vec<_>>());
        pressed.and(released)
    }

    fn open_app(&self, name: &str) -> Result<(), String> {
        let target = resolve_app(name, start_menu_shortcut, app_path);
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (verb, file) = (wide("open"), wide(&target));
        // SAFETY: `verb` and `file` are NUL-terminated and outlive the call; the other pointers are
        // null (no window, parameters or directory), all of which `ShellExecuteW` allows.
        let code = unsafe {
            ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
        } as isize;
        if code <= 32 {
            return Err(shell_error(code));
        }
        Ok(())
    }
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// A mouse event at screen pixel (`x`, `y`), placed absolutely on the virtual screen.
fn mouse(x: f64, y: f64, flags: u32, data: i32) -> INPUT {
    // SAFETY: `GetSystemMetrics` takes an index and no pointers.
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: normalise(x, vx, vw),
                dy: normalise(y, vy, vh),
                mouseData: data as u32,
                dwFlags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// A key stroke as `SendInput` wants it: a virtual key with its scan code (apps that read scan
/// codes see the right key), or a UTF-16 unit.
fn key_input(stroke: Stroke) -> INPUT {
    let ki = match stroke {
        Stroke::Key { vk, up } => KEYBDINPUT {
            wVk: vk,
            // SAFETY: a plain lookup of a virtual-key code; no pointers.
            wScan: unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16,
            dwFlags: if is_extended(vk) { KEYEVENTF_EXTENDEDKEY } else { 0 } | if up { KEYEVENTF_KEYUP } else { 0 },
            time: 0,
            dwExtraInfo: 0,
        },
        Stroke::Unit { unit, up } => KEYBDINPUT {
            wVk: 0,
            wScan: unit,
            dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 },
            time: 0,
            dwExtraInfo: 0,
        },
    };
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } }
}

fn send(inputs: &[INPUT]) -> Result<(), String> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: `inputs` is a valid slice of `INPUT` for the count given, and `cbSize` is `INPUT`'s size.
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(format!(
            "Windows didn't take the input ({}). The screen may be locked, or a UAC prompt is up.",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn raw(hwnd: HWND) -> Raw {
    let (_, _, w, h) = bounds(hwnd);
    let mut cloaked: u32 = 0;
    // SAFETY: DWMWA_CLOAKED fills a `DWORD`: `cloaked`, four bytes. A window that has gone just
    // fails the call and leaves it 0.
    unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, &mut cloaked as *mut u32 as *mut _, 4);
    }
    let mut class = [0u16; 64];
    // SAFETY: the buffer and the length passed are `class`'s.
    let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) }.max(0) as usize;
    // SAFETY (the three calls below): they take the handle by value, which Windows validates.
    Raw {
        visible: unsafe { IsWindowVisible(hwnd) } != 0,
        cloaked: cloaked != 0,
        iconic: unsafe { IsIconic(hwnd) } != 0,
        ex_style: unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32,
        class: String::from_utf16_lossy(&class[..n]),
        w,
        h,
    }
}

/// A window's frame as drawn (without the invisible resize borders), else its rectangle.
fn bounds(hwnd: HWND) -> (f64, f64, f64, f64) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let size = std::mem::size_of::<RECT>() as u32;
    // SAFETY: DWMWA_EXTENDED_FRAME_BOUNDS fills a `RECT`: `r`, `size` bytes.
    let ok = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS as u32, &mut r as *mut RECT as *mut _, size) } == 0;
    if !ok {
        // SAFETY: `r` is a `RECT` for it to fill.
        unsafe { GetWindowRect(hwnd, &mut r) };
    }
    (r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64)
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: `pid` is a `DWORD` for it to fill (left 0 if the window has gone).
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

fn describe(hwnd: HWND, names: &mut std::collections::HashMap<u32, String>) -> WindowInfo {
    let pid = window_pid(hwnd);
    let owner = names.entry(pid).or_insert_with(|| process_name(pid)).clone();
    let mut title = [0u16; 512];
    // SAFETY: the buffer and the length passed are `title`'s.
    let n = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) }.max(0) as usize;
    let (x, y, w, h) = bounds(hwnd);
    WindowInfo { owner, title: String::from_utf16_lossy(&title[..n]), pid: pid as i64, id: hwnd as i64, x, y, w, h }
}

/// The executable's name for a process, or "" when Windows won't say (a protected process).
fn process_name(pid: u32) -> String {
    // SAFETY: plain values in; a null handle (refused) is checked below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return String::new();
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `process` is a live handle with query access; `buf` holds `len` units, and `len`
    // comes back as how many were written.
    let ok = unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0;
    // SAFETY: `process` was opened above and is closed exactly once, here.
    unsafe { CloseHandle(process) };
    if ok { exe_name(&String::from_utf16_lossy(&buf[..len as usize])) } else { String::new() }
}

/// Releases GDI objects whatever way `grab` leaves.
struct Gdi {
    screen: HDC,
    mem: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
}

impl Drop for Gdi {
    fn drop(&mut self) {
        // SAFETY: each handle is either null (not made) or one `grab` made and still owns; `old` is
        // put back before the bitmap it replaced is deleted, and nothing is released twice.
        unsafe {
            if !self.old.is_null() {
                SelectObject(self.mem, self.old);
            }
            if !self.dib.is_null() {
                DeleteObject(self.dib);
            }
            if !self.mem.is_null() {
                DeleteDC(self.mem);
            }
            ReleaseDC(std::ptr::null_mut(), self.screen);
        }
    }
}

/// The screen's pixels in (`sx`, `sy`, `sw`, `sh`), scaled to `tw`×`th`, as RGB rows top down.
fn grab(sx: i32, sy: i32, sw: i32, sh: i32, tw: i32, th: i32) -> Result<Vec<u8>, String> {
    let fail = |what: &str| format!("Couldn't capture the screen ({what} failed). The screen may be locked.");
    // SAFETY: a null window asks for the whole screen's DC; null (failure) is checked below. `Gdi`
    // releases it, and what's made from it, whichever way this function leaves.
    let screen = unsafe { GetDC(std::ptr::null_mut()) };
    if screen.is_null() {
        return Err(fail("GetDC"));
    }
    let mut gdi = Gdi { screen, mem: std::ptr::null_mut(), dib: std::ptr::null_mut(), old: std::ptr::null_mut() };
    // SAFETY: `screen` is a live DC.
    gdi.mem = unsafe { CreateCompatibleDC(screen) };
    if gdi.mem.is_null() {
        return Err(fail("CreateCompatibleDC"));
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: tw,
            biHeight: -th, // top-down rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `info` describes a 32-bit top-down bitmap and outlives the call; `bits` is where it
    // writes the pixels' address; no file mapping is used (null, 0).
    gdi.dib = unsafe { CreateDIBSection(gdi.mem, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0) };
    if gdi.dib.is_null() || bits.is_null() {
        return Err(fail("CreateDIBSection"));
    }
    // SAFETY: both handles were made above and are live.
    gdi.old = unsafe { SelectObject(gdi.mem, gdi.dib) };
    // CAPTUREBLT takes layered windows (menus, tooltips, translucent windows) too.
    // SAFETY: the memory DC holds the bitmap `tw`×`th` the copy fills; the screen DC is live.
    let copied = unsafe {
        if (tw, th) == (sw, sh) {
            BitBlt(gdi.mem, 0, 0, tw, th, screen, sx, sy, SRCCOPY | CAPTUREBLT)
        } else {
            SetStretchBltMode(gdi.mem, HALFTONE);
            SetBrushOrgEx(gdi.mem, 0, 0, std::ptr::null_mut());
            StretchBlt(gdi.mem, 0, 0, tw, th, screen, sx, sy, sw, sh, SRCCOPY | CAPTUREBLT)
        }
    };
    if copied == 0 {
        return Err(fail("BitBlt"));
    }
    // SAFETY: no arguments; it only waits for pending drawing to reach the bitmap.
    unsafe { GdiFlush() };
    let pixels = (tw as usize) * (th as usize);
    // BGRX in memory; the PNG wants RGB.
    // SAFETY: `bits` points into the DIB section, which is `tw`×`th` pixels of 4 bytes (rows are
    // already 4-byte aligned, so no padding) and stays alive until `gdi` drops at the end of this
    // function, after `rgb` has copied everything out.
    let bgrx = unsafe { std::slice::from_raw_parts(bits as *const u8, pixels * 4) };
    let mut rgb = Vec::with_capacity(pixels * 3);
    for px in bgrx.chunks_exact(4) {
        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    Ok(rgb)
}

/// A Start menu shortcut called `name` (the user's, then everyone's), as a path.
fn start_menu_shortcut(name: &str) -> Option<String> {
    let want = format!("{name}.lnk").to_lowercase();
    let roots = [("APPDATA", r"Microsoft\Windows\Start Menu\Programs"), ("ProgramData", r"Microsoft\Windows\Start Menu\Programs")];
    roots.iter().find_map(|(var, sub)| {
        let root = std::path::PathBuf::from(std::env::var_os(var)?).join(sub);
        find_file(&root, &want, 4)
    })
}

fn find_file(dir: &std::path::Path, lowercase_name: &str, depth: u32) -> Option<String> {
    let entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    let here = entries.iter().find(|e| e.file_name().to_string_lossy().to_lowercase() == lowercase_name);
    if let Some(e) = here {
        return Some(e.path().display().to_string());
    }
    if depth == 0 {
        return None;
    }
    entries
        .iter()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .find_map(|e| find_file(&e.path(), lowercase_name, depth - 1))
}

/// The program App Paths registers for `exe` (the user's, then the machine's). Read only.
fn app_path(exe: &str) -> Option<String> {
    let key: Vec<u16> = format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{exe}").encode_utf16().chain([0]).collect();
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().find_map(|hive: HKEY| {
        let mut buf = [0u16; 1024];
        let mut size = std::mem::size_of_val(&buf) as u32;
        // SAFETY: `key` is NUL-terminated; `buf` has `size` bytes, and `size` comes back as the
        // bytes written (a string value is always NUL-terminated with RRF_RT_REG_SZ).
        let err = unsafe {
            RegGetValueW(hive, key.as_ptr(), std::ptr::null(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut size)
        };
        if err != 0 {
            return None;
        }
        let n = (size as usize / 2).saturating_sub(1).min(buf.len());
        let path = String::from_utf16_lossy(&buf[..n]).trim().trim_matches('"').to_string();
        (!path.is_empty()).then_some(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{VK_CONTROL, VK_LWIN, VK_SHIFT, parse_vk};

    /// Where Windows puts a normalised coordinate: (n × size) / 65536 pixels in.
    fn landing(n: i32, origin: i32, size: i32) -> i32 {
        origin + ((n as i64 * size as i64) >> 16) as i32
    }

    #[test]
    fn coordinates_normalise_onto_the_virtual_screen() {
        // One 1920x1080 display: every pixel lands on itself, corners included.
        for p in [0, 1, 959, 960, 1918, 1919] {
            let n = normalise(p as f64, 0, 1920);
            assert!((0..=65535).contains(&n));
            assert_eq!(landing(n, 0, 1920), p, "pixel {p} → {n}");
        }
        // A second display to the left: the virtual screen starts at -2560 and is 4480 wide; the
        // primary's (0, 0) is 2560 pixels in.
        assert_eq!(landing(normalise(0.0, -2560, 4480), -2560, 4480), 0);
        assert_eq!(landing(normalise(-2560.0, -2560, 4480), -2560, 4480), -2560);
        assert_eq!(landing(normalise(1919.0, -2560, 4480), -2560, 4480), 1919);
        // Off the virtual screen clamps to its edge; fractions round to the nearest pixel.
        assert_eq!(normalise(-10.0, 0, 1920), normalise(0.0, 0, 1920));
        assert_eq!(normalise(99999.0, 0, 1920), normalise(1919.0, 0, 1920));
        assert_eq!(normalise(959.6, 0, 1920), normalise(960.0, 0, 1920));
        // A 4K display at 150%: still physical pixels.
        assert_eq!(landing(normalise(3839.0, 0, 3840), 0, 3840), 3839);
    }

    #[test]
    fn captures_fit_the_long_side() {
        assert_eq!(fit(1920, 1080, 1568), (1568, 882));
        assert_eq!(fit(1080, 1920, 1568), (882, 1568));
        assert_eq!(fit(3840, 2160, 1568), (1568, 882));
        assert_eq!(fit(1024, 768, 1568), (1024, 768), "never upscaled");
        assert_eq!(fit(300, 150, 1568), (300, 150));
        assert_eq!(fit(5000, 1, 1568), (1568, 1), "never zero");
    }

    #[test]
    fn a_line_is_a_share_of_a_wheel_notch() {
        assert_eq!(wheel_delta(1, 3), 40);
        assert_eq!(wheel_delta(3, 3), 120, "three lines are one notch at the default setting");
        assert_eq!(wheel_delta(-5, 3), -200);
        assert_eq!(wheel_delta(2, 1), 240);
        assert_eq!(wheel_delta(1, u32::MAX), 120, "one screen at a time: a notch a line");
        assert_eq!(wheel_delta(1, 0), 120);
    }

    #[test]
    fn chords_press_modifiers_around_the_key_and_release_them_in_reverse() {
        let k = |vk, up| Stroke::Key { vk, up };
        assert_eq!(
            chord_strokes(&parse_vk("ctrl+shift+t").unwrap()),
            vec![k(VK_CONTROL, false), k(VK_SHIFT, false), k(0x54, false), k(0x54, true), k(VK_SHIFT, true), k(VK_CONTROL, true)]
        );
        assert_eq!(chord_strokes(&parse_vk("escape").unwrap()), vec![k(0x1B, false), k(0x1B, true)]);
        assert_eq!(chord_strokes(&parse_vk("win+r").unwrap())[0], k(VK_LWIN, false));
        // Every down has its up.
        for combo in ["cmd+alt+shift+win+f5", "a", "ctrl++"] {
            let s = chord_strokes(&parse_vk(combo).unwrap());
            let downs = s.iter().filter(|s| matches!(s, Stroke::Key { up: false, .. })).count();
            assert_eq!(downs * 2, s.len(), "{combo}");
        }
    }

    #[test]
    fn text_types_as_unicode_with_return_and_tab_as_keys() {
        let k = |vk, up| Stroke::Key { vk, up };
        let u = |unit, up| Stroke::Unit { unit, up };
        assert_eq!(
            text_strokes("é\r\n\tb"),
            vec![u(0xE9, false), u(0xE9, true), k(0x0D, false), k(0x0D, true), k(0x09, false), k(0x09, true), u(0x62, false), u(0x62, true)]
        );
        // A character beyond the BMP goes as its surrogate pair, each unit down and up.
        assert_eq!(text_strokes("😀"), vec![u(0xD83D, false), u(0xD83D, true), u(0xDE00, false), u(0xDE00, true)]);
        assert_eq!(text_strokes("a\rb").len(), 6, "a lone carriage return presses Return too");
        assert!(text_strokes("").is_empty());
    }

    #[test]
    fn navigation_keys_are_extended_and_letters_are_not() {
        for name in ["left", "right", "up", "down", "home", "end", "pageup", "pagedown", "forwarddelete"] {
            assert!(is_extended(keys::vk(name).unwrap()), "{name}");
        }
        for name in ["a", "return", "delete", "escape", "f5", "space", "tab"] {
            assert!(!is_extended(keys::vk(name).unwrap()), "{name}");
        }
        assert!(is_extended(VK_LWIN));
    }

    #[test]
    fn executables_are_named_without_folder_or_extension() {
        assert_eq!(exe_name(r"C:\Program Files\Trek\trek.exe"), "trek");
        assert_eq!(exe_name(r"C:\Program Files\Trek\Trek.EXE"), "Trek");
        assert_eq!(exe_name(r"D:\trek-target\debug\trek-mcp.exe"), "trek-mcp");
        assert_eq!(exe_name("C:/tools/node"), "node");
        assert_eq!(exe_name(r"C:\Windows\explorer.exe"), "explorer");
        assert_eq!(exe_name(""), "");
        assert_eq!(exe_name(r"C:\x\é.exe"), "é");
    }

    #[test]
    fn the_window_list_keeps_what_a_click_could_land_on() {
        let app = || Raw { visible: true, cloaked: false, iconic: false, ex_style: 0, class: "Chrome_WidgetWin_1".into(), w: 1200.0, h: 800.0 };
        assert!(listed(&app()));
        assert!(!listed(&Raw { visible: false, ..app() }), "hidden");
        assert!(!listed(&Raw { cloaked: true, ..app() }), "on another virtual desktop");
        assert!(!listed(&Raw { iconic: true, ..app() }), "minimised");
        assert!(!listed(&Raw { ex_style: WS_EX_TRANSPARENT | 0x80000, ..app() }), "click-through overlay");
        assert!(!listed(&Raw { class: "Progman".into(), ..app() }), "the desktop");
        assert!(!listed(&Raw { class: "WorkerW".into(), ..app() }), "the desktop's wallpaper layer");
        assert!(!listed(&Raw { w: 1.0, ..app() }), "a sliver");
        // Tool windows (menus, popovers, the taskbar) stay: a click on them doesn't reach what's
        // behind, and Trek's own popovers must count as Trek's.
        assert!(listed(&Raw { ex_style: 0x80 /* WS_EX_TOOLWINDOW */, ..app() }));
        assert!(listed(&Raw { ex_style: 0x80000 /* WS_EX_LAYERED */, ..app() }));
    }

    #[test]
    fn apps_resolve_by_start_menu_name_then_app_paths_then_as_given() {
        let shortcut = |name: &str| (name.eq_ignore_ascii_case("visual studio code")).then(|| r"C:\Start\Visual Studio Code.lnk".to_string());
        let app_path = |exe: &str| (exe.eq_ignore_ascii_case("chrome.exe")).then(|| r"C:\Chrome\chrome.exe".to_string());
        assert_eq!(resolve_app("Visual Studio Code", shortcut, app_path), r"C:\Start\Visual Studio Code.lnk");
        assert_eq!(resolve_app("chrome", shortcut, app_path), r"C:\Chrome\chrome.exe");
        assert_eq!(resolve_app("chrome.exe", shortcut, app_path), r"C:\Chrome\chrome.exe");
        assert_eq!(resolve_app("notepad", shortcut, app_path), "notepad", "left to PATH");
        // Paths and URIs go as given, never looked up.
        assert_eq!(resolve_app(r"C:\Tools\app.exe", shortcut, app_path), r"C:\Tools\app.exe");
        assert_eq!(resolve_app("ms-settings:display", shortcut, app_path), "ms-settings:display");
        assert_eq!(resolve_app("https://example.com", shortcut, app_path), "https://example.com");
        assert!(shell_error(2).contains("no app"));
        assert!(shell_error(31).contains("nothing is set to open it"));    }

    /// Lists the windows on screen and captures the primary display: nothing else, and no input.
    /// Run by hand: `cargo test -p trek-mcp -- --ignored --exact desktop::windows::tests::smoke_lists_windows_and_captures_the_screen --nocapture`.
    #[test]
    #[ignore = "reads the real screen"]
    fn smoke_lists_windows_and_captures_the_screen() {
        let desktop = Win::default();
        let display = desktop.display();
        assert!(display.width >= 640.0 && display.height >= 480.0, "{display:?}");
        let windows = desktop.windows().unwrap();
        assert!(!windows.is_empty(), "something is on screen");
        for w in &windows {
            eprintln!("{:>16} {:>6} {:>6}x{:<6} at {:>6},{:<6} {}", w.owner, w.pid, w.w, w.h, w.x, w.y, w.title);
        }
        assert!(windows.iter().all(|w| w.w >= 2.0 && w.h >= 2.0));
        let front = desktop.key_window().unwrap();
        eprintln!("in front: {:?}", front.map(|w| w.owner));
        let shot = desktop.capture(None, crate::util::MAX_IMAGE_SIDE).unwrap();
        let (fw, fh) = fit(display.width as u32, display.height as u32, crate::util::MAX_IMAGE_SIDE);
        assert_eq!((shot.width, shot.height), (fw, fh));
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD.decode(&shot.png_base64).unwrap();
        assert_eq!(crate::util::png_size(&png), Some((fw, fh)));
        // Not a blank frame: a real screen has more than one colour.
        let mut reader = png::Decoder::new(std::io::Cursor::new(&png)).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut buf).unwrap();
        assert!(buf.chunks_exact(3).any(|p| p != &buf[..3]), "the capture is one flat colour");
        // A zoom is captured at the screen's own resolution.
        let zoom = desktop.capture(Some((0.0, 0.0, 300.0, 200.0)), crate::util::MAX_IMAGE_SIDE).unwrap();
        assert_eq!((zoom.width, zoom.height), (300, 200));
        eprintln!("display {}x{}, screenshot {}x{} ({} KB)", display.width, display.height, shot.width, shot.height, png.len() / 1024);
    }
}
