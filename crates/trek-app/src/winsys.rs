//! What Trek asks of Windows beyond its windows: the taskbar button's overlay badge (the Dock
//! badge's twin), the identity toasts are shown under, and the alert sound. Each is a thin call;
//! `system` and `root` decide when to make it, and the tests watch that.

/// The AppUserModelID and name Windows knows Trek by (`set_app_identity`).
#[cfg(windows)]
pub const APP_ID: &str = "dev.trek.Trek";
#[cfg(windows)]
pub const APP_NAME: &str = "Trek";

/// The overlay badge's pixels, RGBA: a red disc with `count` on it in white (99 at most). Drawn
/// at `size` px square; the taskbar scales it to its 16 px slot.
#[cfg(any(windows, test))]
pub fn badge_rgba(count: usize, size: usize) -> Vec<u8> {
    // Digits as 3×5 bitmaps, a row per three bits.
    const DIGITS: [[u8; 5]; 10] = [
        [0b111, 0b101, 0b101, 0b101, 0b111],
        [0b010, 0b110, 0b010, 0b010, 0b111],
        [0b111, 0b001, 0b111, 0b100, 0b111],
        [0b111, 0b001, 0b111, 0b001, 0b111],
        [0b101, 0b101, 0b111, 0b001, 0b001],
        [0b111, 0b100, 0b111, 0b001, 0b111],
        [0b111, 0b100, 0b111, 0b101, 0b111],
        [0b111, 0b001, 0b001, 0b001, 0b001],
        [0b111, 0b101, 0b111, 0b101, 0b111],
        [0b111, 0b101, 0b111, 0b001, 0b111],
    ];
    let digits: Vec<usize> = count.min(99).to_string().bytes().map(|b| (b - b'0') as usize).collect();
    // One digit is drawn big, two smaller so they fit the disc.
    let scale = if digits.len() == 1 { size / 8 } else { size / 10 };
    let (text_w, text_h) = ((digits.len() * 4 - 1) * scale, 5 * scale);
    let (left, top) = ((size - text_w) / 2, (size - text_h) / 2);
    let mut out = vec![0u8; size * size * 4];
    let radius = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            // Anti-aliased edge: coverage falls off over the last pixel of the radius.
            let (dx, dy) = (x as f32 + 0.5 - radius, y as f32 + 0.5 - radius);
            let coverage = (radius - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            if coverage == 0.0 {
                continue;
            }
            let (mut r, mut g, mut b) = (0xd1, 0x34, 0x38);
            if x >= left && y >= top && x < left + text_w && y < top + text_h {
                let (cx, cy) = ((x - left) / scale, (y - top) / scale);
                let (digit, col) = (cx / 4, cx % 4);
                if col < 3 && DIGITS[digits[digit]][cy] >> (2 - col) & 1 == 1 {
                    (r, g, b) = (0xff, 0xff, 0xff);
                }
            }
            let at = (y * size + x) * 4;
            out[at..at + 4].copy_from_slice(&[r, g, b, (coverage * 255.0).round() as u8]);
        }
    }
    out
}

/// What the taskbar button's badge says to a screen reader.
#[cfg(any(windows, test))]
pub fn badge_description(count: usize) -> String {
    format!("{count} thread{} need{} you", if count == 1 { "" } else { "s" }, if count == 1 { "s" } else { "" })
}

/// Put `count` on the taskbar button of window `hwnd` (cleared at 0). Returns whether it was
/// applied: the button may not exist yet, in which case the caller asks again.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
pub fn set_overlay_badge(hwnd: isize, count: usize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO};
    use windows::core::{BOOL, HSTRING, PCWSTR};

    const SIZE: usize = 32;
    // SAFETY: COM calls on the main thread, which GPUI has initialised COM on; every handle made
    // here is freed here (the taskbar keeps its own copy of the icon).
    unsafe {
        let Ok(list) = CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None::<&windows::core::IUnknown>, CLSCTX_INPROC_SERVER) else { return false };
        if list.HrInit().is_err() {
            return false;
        }
        let window = HWND(hwnd as *mut _);
        if count == 0 {
            return list.SetOverlayIcon(window, HICON::default(), PCWSTR::null()).is_ok();
        }
        // Bitmaps want premultiplied BGRA.
        let mut bgra = badge_rgba(count, SIZE);
        for px in bgra.chunks_exact_mut(4) {
            let a = px[3] as u32;
            let (r, g, b) = (px[0] as u32 * a / 255, px[1] as u32 * a / 255, px[2] as u32 * a / 255);
            px.copy_from_slice(&[b as u8, g as u8, r as u8, a as u8]);
        }
        let color = CreateBitmap(SIZE as i32, SIZE as i32, 1, 32, Some(bgra.as_ptr().cast()));
        let mask_bits = vec![0u8; SIZE * SIZE / 8];
        let mask = CreateBitmap(SIZE as i32, SIZE as i32, 1, 1, Some(mask_bits.as_ptr().cast()));
        let info = ICONINFO { fIcon: BOOL(1), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        let Ok(icon) = icon else { return false };
        let description = HSTRING::from(badge_description(count));
        let applied = list.SetOverlayIcon(window, icon, &description).is_ok();
        let _ = DestroyIcon(icon);
        applied
    }
}

/// The window's handle, the way Win32 knows it.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
pub fn hwnd_of(window: &gpui_kit::Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    // Not `window.window_handle()`: GPUI's own method of that name is a different thing.
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// Register the identity and icon toasts show under, before the first is posted. GPUI registers
/// the AppUserModelID's name (`set_app_identity`); an unpackaged app also needs an icon there, or
/// its toasts show a blank one. Written once per run, to the current user's classes only.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
pub fn register_toast_identity(app_id: &str, name: &str, icon_png: &[u8]) {
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW};
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }
    // Next to the other state, where the next run finds it again.
    let icon = trek_core::paths::data_dir().join("toast-icon.png");
    if let Err(e) = std::fs::write(&icon, icon_png) {
        tracing::warn!("toast icon: {e}");
        return;
    }
    let key = wide(&format!(r"Software\Classes\AppUserModelId\{app_id}"));
    let mut hkey: HKEY = std::ptr::null_mut();
    // SAFETY: NUL-ended names, and a key handle that is closed below.
    unsafe {
        if RegCreateKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut hkey, std::ptr::null_mut()) != 0 {
            return;
        }
        for (value, text) in [("DisplayName", name.to_string()), ("IconUri", icon.to_string_lossy().into_owned())] {
            let (value, data) = (wide(value), wide(&text));
            RegSetValueExW(hkey, value.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32);
        }
        RegCloseKey(hkey);
    }
}

/// What Windows runs for a `trek://` link: Trek, with the link as its one argument. Quoted both
/// ways: a path with spaces, and a link with `&` in it.
#[cfg(any(windows, test))]
pub fn protocol_command(exe: &std::path::Path) -> String {
    format!("\"{}\" \"%1\"", exe.display())
}

/// Where the protocol's keys go for real: the current user's classes.
#[cfg(windows)]
pub const CLASSES: &str = r"Software\Classes";

/// What `register_protocol` did.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registered {
    /// The key was already as it should be: nothing written.
    Already,
    Written,
}

/// Make Windows hand `trek://` links to `exe`: `<root>\trek` marked as a URL protocol, with
/// `shell\open\command` running Trek on the link. `root` is a key under `HKEY_CURRENT_USER`
/// (`CLASSES`, or a key of a test's own); nothing is ever written to the machine's classes.
/// Read first and written only when the command differs, so a run that finds it right touches
/// nothing (and doesn't make the registry hive busy at every start).
#[cfg(windows)]
pub fn register_protocol(root: &str, exe: &std::path::Path) -> std::io::Result<Registered> {
    let command = protocol_command(exe);
    let key = format!(r"{root}\trek");
    let command_key = format!(r"{key}\shell\open\command");
    if reg_read(&command_key, "").as_deref() == Some(command.as_str()) && reg_read(&key, "URL Protocol").is_some() {
        return Ok(Registered::Already);
    }
    reg_write(&key, "", "URL:Trek")?;
    reg_write(&key, "URL Protocol", "")?;
    reg_write(&format!(r"{key}\DefaultIcon"), "", &format!("\"{}\",0", exe.display()))?;
    reg_write(&command_key, "", &command)?;
    Ok(Registered::Written)
}

/// Register the protocol for the running Trek, in the user's classes. Once a launch; a failure
/// is logged and nothing else: links then keep going where they went. Not for a process with a
/// data folder of its own (a test, a capture run, a trial build): it mustn't take over the links
/// of the Trek the user has.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
pub fn register_trek_protocol() {
    if std::env::var_os("TREK_SHOT_DIR").is_some() || std::env::var_os("TREK_DATA_DIR").is_some_and(|d| !d.is_empty()) {
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return tracing::warn!("trek:// links: where Trek is: {e}"),
    };
    match register_protocol(CLASSES, &exe) {
        Ok(Registered::Written) => tracing::info!("trek:// links now open {}", exe.display()),
        Ok(Registered::Already) => {}
        Err(e) => tracing::warn!("trek:// links: couldn't register the protocol: {e}"),
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A string value of a key under `HKEY_CURRENT_USER` (`""` is the key's default value).
#[cfg(windows)]
fn reg_read(key: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let (key, value) = (wide(key), wide(value));
    let mut bytes = 0u32;
    // SAFETY: NUL-ended names; the first call only asks how long the value is, the second fills
    // a buffer of that size.
    unsafe {
        if RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut bytes) != 0 {
            return None;
        }
        let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
        if RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut bytes) != 0 {
            return None;
        }
        // The size counts the terminating NUL.
        buf.truncate((bytes as usize / 2).saturating_sub(1));
        Some(String::from_utf16_lossy(&buf))
    }
}

/// Set a string value of a key under `HKEY_CURRENT_USER`, making the key (and those above it).
#[cfg(windows)]
fn reg_write(key: &str, value: &str, text: &str) -> std::io::Result<()> {
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW};
    let (key, value, data) = (wide(key), wide(value), wide(text));
    let mut hkey: HKEY = std::ptr::null_mut();
    // SAFETY: NUL-ended names, and a key handle that is closed below.
    unsafe {
        let made = RegCreateKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut hkey, std::ptr::null_mut());
        if made != 0 {
            return Err(std::io::Error::from_raw_os_error(made as i32));
        }
        let set = RegSetValueExW(hkey, value.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32);
        RegCloseKey(hkey);
        if set != 0 {
            return Err(std::io::Error::from_raw_os_error(set as i32));
        }
    }
    Ok(())
}

/// The system's notification sound, waiting until it has played.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
pub fn play_notification_sound() {
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_NODEFAULT};
    let alias: Vec<u16> = "SystemNotification".encode_utf16().chain([0]).collect();
    // SAFETY: a NUL-ended alias; no module handle, as it names a system sound.
    unsafe { PlaySoundW(alias.as_ptr(), std::ptr::null_mut(), SND_ALIAS | SND_NODEFAULT) };
}

#[cfg(test)]
mod tests {
    use super::{badge_description, badge_rgba};

    fn lit(pixels: &[u8], white: bool) -> usize {
        pixels.chunks_exact(4).filter(|p| p[3] > 0 && (p[0] == 0xff && p[1] == 0xff) == white).count()
    }

    #[test]
    fn the_badge_is_a_disc_with_the_count_on_it() {
        let one = badge_rgba(1, 32);
        assert_eq!(one.len(), 32 * 32 * 4);
        // Corners are clear, the middle of the edge is the disc's red, and there are digit pixels.
        assert_eq!(one[3], 0);
        let edge = (16 * 32 + 1) * 4;
        assert_eq!(&one[edge..edge + 3], &[0xd1, 0x34, 0x38]);
        assert!(lit(&one, true) > 0);
        // A different count draws differently; past 99 it stays 99.
        assert_ne!(one, badge_rgba(2, 32));
        assert_ne!(badge_rgba(9, 32), badge_rgba(10, 32));
        assert_eq!(badge_rgba(99, 32), badge_rgba(250, 32));
        // Two digits have more ink than one.
        assert!(lit(&badge_rgba(88, 32), true) > lit(&badge_rgba(1, 32), true));
    }

    #[test]
    fn the_badge_reads_aloud_in_the_singular_and_plural() {
        assert_eq!(badge_description(1), "1 thread needs you");
        assert_eq!(badge_description(3), "3 threads need you");
    }

    #[test]
    fn a_link_runs_trek_with_the_link_as_one_argument() {
        let exe = std::path::Path::new(r"C:\Program Files\Trek\trek.exe");
        assert_eq!(super::protocol_command(exe), r#""C:\Program Files\Trek\trek.exe" "%1""#);
    }

    /// Against a key of the test's own (`HKCU\Software\Trek-tests\<pid>`), deleted afterwards:
    /// never the real `trek` key.
    #[cfg(windows)]
    mod registry {
        use super::super::{Registered, protocol_command, reg_read, register_protocol};
        use std::path::Path;
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};

        struct TestKey(String);

        impl TestKey {
            fn new(tag: &str) -> TestKey {
                TestKey(format!(r"Software\Trek-tests\{}-{tag}", std::process::id()))
            }
        }

        impl Drop for TestKey {
            fn drop(&mut self) {
                let key: Vec<u16> = self.0.encode_utf16().chain([0]).collect();
                // SAFETY: a NUL-ended name of a key this test made.
                unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) };
            }
        }

        #[test]
        fn the_protocol_is_registered_once_and_then_left_alone() {
            let root = TestKey::new("once");
            let exe = Path::new(r"C:\Apps\Trek\trek.exe");
            assert_eq!(reg_read(&format!(r"{}\trek", root.0), "URL Protocol"), None, "nothing there yet");

            assert_eq!(register_protocol(&root.0, exe).unwrap(), Registered::Written);
            let key = format!(r"{}\trek", root.0);
            assert_eq!(reg_read(&key, "URL Protocol").as_deref(), Some(""), "marked as a URL protocol");
            assert_eq!(reg_read(&format!(r"{key}\shell\open\command"), "").as_deref(), Some(protocol_command(exe).as_str()));
            assert_eq!(reg_read(&key, "").as_deref(), Some("URL:Trek"));

            assert_eq!(register_protocol(&root.0, exe).unwrap(), Registered::Already, "the second run writes nothing");
        }

        /// Trek moved (an update, another folder): the command follows it.
        #[test]
        fn a_command_that_differs_is_replaced() {
            let root = TestKey::new("moved");
            register_protocol(&root.0, Path::new(r"C:\Old\trek.exe")).unwrap();
            let new = Path::new(r"D:\New place\trek.exe");
            assert_eq!(register_protocol(&root.0, new).unwrap(), Registered::Written);
            assert_eq!(reg_read(&format!(r"{}\trek\shell\open\command", root.0), "").as_deref(), Some(protocol_command(new).as_str()));
            assert_eq!(register_protocol(&root.0, new).unwrap(), Registered::Already);
        }
    }
}

/// By hand (`cargo test -p trek-app overlay -- --ignored`): puts the badge on the taskbar button of
/// a window made for the purpose, which shows for a second or two, and clears it. The tests
/// above only draw the pixels; this is the one that asks Windows.
#[cfg(all(windows, test))]
mod by_hand {
    use windows_sys::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage, WS_OVERLAPPEDWINDOW};

    fn pump(ms: u64) {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < until {
            // SAFETY: a message loop over this thread's own windows.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    #[ignore = "opens a window on the taskbar"]
    fn overlay_badge_is_taken_by_the_taskbar() {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let title: Vec<u16> = "Trek overlay badge test\0".encode_utf16().collect();
        // SAFETY: a plain window of a system class, destroyed below; COM for this thread.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hwnd: HWND = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW, 200, 200, 320, 120, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            assert!(!hwnd.is_null());
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            pump(1500);
            assert!(super::set_overlay_badge(hwnd as isize, 3), "the badge went up");
            pump(1500);
            assert!(super::set_overlay_badge(hwnd as isize, 12), "and changed");
            pump(1000);
            assert!(super::set_overlay_badge(hwnd as isize, 0), "and came down");
            DestroyWindow(hwnd);
        }
    }
}
