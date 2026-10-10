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

/// The id of the icon resource in `trek.exe` for `icon`. `build.rs` embeds the three `.ico`s
/// under these numbers (ember stays 1: GPUI's window class and Explorer read the exe's icon
/// from there), and a test keeps the two lists the same.
#[cfg(any(windows, test))]
pub fn icon_resource(icon: trek_core::settings::AppIcon) -> u16 {
    use trek_core::settings::AppIcon;
    match icon {
        AppIcon::Ember => 1,
        AppIcon::Night => 2,
        AppIcon::Glass => 3,
    }
}

/// Put icon resource `resource` on window `hwnd` (small, for the title bar, and big, for the
/// taskbar and Alt+Tab) and on the window class GPUI makes every window from, so windows opened
/// later start with it. Whether the icons could be loaded.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
pub fn set_window_icon(hwnd: isize, resource: u16) -> bool {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GCLP_HICON, GCLP_HICONSM, GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_SHARED, LoadImageW, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW,
        SetClassLongPtrW, WM_SETICON,
    };
    // SAFETY: the resource id goes where a name does (MAKEINTRESOURCE); shared icons belong to
    // the system, which keeps them for the life of the process, so there is nothing to free.
    // `hwnd` is a window of this process, asked from its own thread.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let load = |w: i32, h: i32| LoadImageW(module, resource as usize as *const u16, IMAGE_ICON, GetSystemMetrics(w), GetSystemMetrics(h), LR_SHARED);
        let (small, big) = (load(SM_CXSMICON, SM_CYSMICON), load(SM_CXICON, SM_CYICON));
        if small.is_null() || big.is_null() {
            return false;
        }
        let window = hwnd as HWND;
        SendMessageW(window, WM_SETICON, ICON_SMALL as WPARAM, small as LPARAM);
        SendMessageW(window, WM_SETICON, ICON_BIG as WPARAM, big as LPARAM);
        SetClassLongPtrW(window, GCLP_HICONSM, small as isize);
        SetClassLongPtrW(window, GCLP_HICON, big as isize);
        true
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
    use super::{badge_description, badge_rgba, icon_resource};
    use trek_core::settings::AppIcon;

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

    /// What `build.rs` embeds, as it spells it: (resource id, file in assets/brand).
    const EMBEDDED: [(AppIcon, &str); 3] = [(AppIcon::Ember, "trek.ico"), (AppIcon::Night, "trek-night.ico"), (AppIcon::Glass, "trek-glass.ico")];

    #[test]
    fn each_app_icon_has_its_own_resource_and_ember_stays_the_exes_icon() {
        // Resource 1 is what GPUI's window class loads at start and Explorer shows for the exe.
        assert_eq!(icon_resource(AppIcon::Ember), 1);
        let ids: Vec<u16> = EMBEDDED.iter().map(|(icon, _)| icon_resource(*icon)).collect();
        assert_eq!(ids, [1, 2, 3]);
    }

    #[test]
    fn build_rs_embeds_the_icons_under_the_ids_the_app_asks_for() {
        let build = include_str!("../build.rs");
        let brand = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brand");
        let mut seen = vec![];
        for (icon, file) in EMBEDDED {
            assert!(build.contains(&format!("({}, \"{file}\")", icon_resource(icon))), "build.rs doesn't embed {file} as resource {}", icon_resource(icon));
            let bytes = std::fs::read(brand.join(file)).unwrap_or_else(|e| panic!("{file}: {e} (script/windows-icons.py makes it)"));
            // An .ico: reserved 0, type 1, then at least the 256 px frame.
            assert_eq!(&bytes[..4], &[0, 0, 1, 0], "{file} isn't an icon file");
            assert!(bytes[4] >= 8, "{file} has too few sizes");
            seen.push(bytes);
        }
        // Three different pictures, not one drawn three times.
        assert!(seen[0] != seen[1] && seen[1] != seen[2] && seen[0] != seen[2]);
    }

    #[test]
    fn the_badge_reads_aloud_in_the_singular_and_plural() {
        assert_eq!(badge_description(1), "1 thread needs you");
        assert_eq!(badge_description(3), "3 threads need you");
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
