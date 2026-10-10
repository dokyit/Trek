//! What Windows says about how Trek should look and move, read the way macOS's Reduce motion and
//! Reduce transparency are: the "Animation effects" and "Transparency effects" switches in
//! Settings, and whether this Windows has the Mica material at all. Reading only; Trek never
//! writes any of it.
//!
//! The decisions are plain functions of what was read (`glass_available`, `reduces_motion`), so
//! they run, and are tested, on every platform; the reads are Windows-only.

// The decisions are plain functions that other platforms' tests run too; only Windows calls them.
#![cfg_attr(not(windows), allow(dead_code))]

/// The first Windows 11 build with the system backdrop material (Mica, Mica Alt) a window can ask
/// for (22H2). Windows 10, and 11 before this, get an opaque window where macOS gets glass.
pub const MICA_BUILD: u32 = 22621;

/// Whether a Windows at `build`, with the Transparency effects switch at `transparency`, can show
/// glass: a window's Mica. Off either way, Trek's window stays opaque.
pub fn glass_available(build: u32, transparency: bool) -> bool {
    build >= MICA_BUILD && transparency
}

/// Whether Windows asks for reduced motion, given its "Animation effects" switch
/// (`SPI_GETCLIENTAREAANIMATION`): off means reduce.
pub fn reduces_motion(client_area_animation: bool) -> bool {
    !client_area_animation
}

/// Whether this Windows can show glass now: Mica, and the Transparency effects switch on.
/// Asked at most once a second, as it's read on every frame; a window's appearance-change
/// callback (Windows announces a switch in Settings that way) calls `forget` so the next frame
/// asks again.
#[cfg(windows)]
pub fn glass_now() -> bool {
    use std::time::{Duration, Instant};
    ASKED.with(|a| match a.get() {
        Some((at, on)) if at.elapsed() < Duration::from_secs(1) => on,
        _ => {
            let on = glass_available(build_number(), transparency_enabled());
            a.set(Some((Instant::now(), on)));
            on
        }
    })
}

#[cfg(windows)]
thread_local!(static ASKED: std::cell::Cell<Option<(std::time::Instant, bool)>> = const { std::cell::Cell::new(None) });

/// Ask the switches again at the next `glass_now`.
pub fn forget() {
    #[cfg(windows)]
    ASKED.with(|a| a.set(None));
}

/// The Windows build number (`CurrentBuild`), 0 if unreadable.
#[cfg(windows)]
pub fn build_number() -> u32 {
    use std::sync::OnceLock;
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(|| read_string(windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuild").and_then(|b| b.parse().ok()).unwrap_or(0))
}

/// The Transparency effects switch (Settings › Personalization › Colors). Windows keeps no value
/// until it's been touched, and transparency is on out of the box.
#[cfg(windows)]
fn transparency_enabled() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize".encode_utf16().chain([0]).collect();
    let name: Vec<u16> = "EnableTransparency".encode_utf16().chain([0]).collect();
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: both names end in a NUL; `value` is the 4 bytes `size` says, written on success.
    let status = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut value as *mut u32).cast(), &mut size) };
    status != 0 || value != 0
}

#[cfg(windows)]
fn read_string(root: windows_sys::Win32::System::Registry::HKEY, key: &str, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};
    let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let mut buf = [0u16; 128];
    let mut size = std::mem::size_of_val(&buf) as u32;
    // SAFETY: both names end in a NUL; `buf` is `size` bytes long, and `size` is how many are
    // written (the terminator included) on success.
    let status = unsafe { RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size) };
    if status != 0 {
        return None;
    }
    let chars = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..chars])).filter(|s| !s.is_empty())
}

/// Whether Windows's "Animation effects" is off (Settings › Accessibility › Visual effects),
/// which is the Windows ask for reduced motion. Not asking is "don't reduce".
#[cfg(windows)]
pub fn reduce_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};
    let mut on = 1i32;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL through `pvParam` and ignores `uiParam`;
    // `on` is a BOOL's size and outlives the call.
    let read = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut on as *mut i32).cast(), 0) };
    read != 0 && reduces_motion(on != 0)
}

/// Tell DWM how the window's backdrop material should look. `dark` picks its dark or light tone,
/// so Mica follows Trek's theme (Night, Paper) rather than the system's: Paper over a dark Mica
/// would be muddy, and Night over a light one glaring (GPUI sets this from the system's
/// appearance only). Without `glass` the material is taken off the window (GPUI's opaque
/// background leaves a Mica it set before in place), so none shows at the frame's edge.
#[cfg(windows)]
pub fn set_backdrop(window: &gpui_kit::Window, dark: bool, glass: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(hwnd: *mut core::ffi::c_void, attribute: u32, value: *const core::ffi::c_void, size: u32) -> i32;
    }
    /// `DWMWA_USE_IMMERSIVE_DARK_MODE`.
    const USE_IMMERSIVE_DARK_MODE: u32 = 20;
    /// `DWMWA_SYSTEMBACKDROP_TYPE`, and its `DWMSBT_NONE`.
    const SYSTEMBACKDROP_TYPE: u32 = 38;
    const BACKDROP_NONE: i32 = 1;
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else { return };
    let hwnd = win32.hwnd.get() as *mut core::ffi::c_void;
    let set = |attribute: u32, value: i32| {
        // SAFETY: `hwnd` is the live window GPUI created for this `window`, on its own thread;
        // both attributes take a 4-byte value, which `value` is and outlives the call. Windows
        // before 11 22H2 refuse the backdrop one, which is the answer wanted there.
        unsafe { DwmSetWindowAttribute(hwnd, attribute, (&value as *const i32).cast(), std::mem::size_of::<i32>() as u32) };
    };
    set(USE_IMMERSIVE_DARK_MODE, dark as i32);
    if !glass {
        set(SYSTEMBACKDROP_TYPE, BACKDROP_NONE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glass_needs_mica_and_the_transparency_switch() {
        // Windows 10 (19045) and 11 before 22H2: no Mica, whatever the switch says.
        assert!(!glass_available(19045, true));
        assert!(!glass_available(22000, true));
        assert!(!glass_available(MICA_BUILD - 1, true));
        // 22H2 and later: Mica, unless transparency effects are off.
        assert!(glass_available(MICA_BUILD, true));
        assert!(glass_available(26100, true));
        assert!(!glass_available(26100, false));
        // An unreadable build (0) is not Mica.
        assert!(!glass_available(0, true));
    }

    /// The reads themselves run here and answer something sensible; what they answer is this
    /// machine's own setting, so only the build number is held to a value.
    #[cfg(windows)]
    #[test]
    fn the_reads_work_on_this_machine() {
        assert!(build_number() >= 19041, "a Windows 10 or 11 build number, got {}", build_number());
        let _ = (reduce_motion(), glass_now());
        forget();
    }

    #[test]
    fn animation_effects_off_is_reduce_motion() {
        assert!(reduces_motion(false));
        assert!(!reduces_motion(true));
    }
}
