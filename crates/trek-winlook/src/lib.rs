//! What Windows says about how Trek should look and move, read the way macOS's Reduce motion and
//! Reduce transparency are: the "Animation effects" and "Transparency effects" switches in
//! Settings, contrast themes, and HDR. Reading only; Trek never writes any of it. And how a
//! window's glass is asked of DWM (`set_backdrop`).
//!
//! The decisions are plain functions of what was read (`glass_available`, `reduces_motion`), so
//! they run, and are tested, on every platform; the reads are Windows-only.

// The decisions are plain functions that other platforms' tests run too; only Windows calls them.
#![cfg_attr(not(windows), allow(dead_code))]

/// The first Windows build Trek shows glass on: Windows 11 22H2, where it's been tried. Windows 10,
/// and 11 before it, get an opaque window where macOS gets glass.
pub const GLASS_BUILD: u32 = 22621;

/// Whether a Windows at `build`, with the Transparency effects switch at `transparency` and High
/// Contrast at `high_contrast`, can show glass: what's behind a window showing through it
/// (`set_backdrop`). Off any way, Trek's window stays opaque: High Contrast asks for the plainest
/// surfaces there are.
pub fn glass_available(build: u32, transparency: bool, high_contrast: bool) -> bool {
    build >= GLASS_BUILD && transparency && !high_contrast
}

/// Whether Windows asks for reduced motion, given its "Animation effects" switch
/// (`SPI_GETCLIENTAREAANIMATION`): off means reduce.
pub fn reduces_motion(client_area_animation: bool) -> bool {
    !client_area_animation
}

/// The flag in DisplayConfig's answer about a display's colour that's set while HDR or Auto Color
/// Management is on: `advancedColorEnabled` in `DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO`, and the
/// same bit, `advancedColorActive`, in Windows 11 24H2's `..._INFO_2`.
pub const ADVANCED_COLOR_ON: u32 = 0x2;

/// Whether DWM blends a display's windows in linear light, from DisplayConfig's answers about it.
/// With HDR or Auto Color Management on (Windows' "Advanced Color") it converts every window to
/// linear scRGB and blends there, and the same see-through lets more of what's behind through
/// than on an SDR display, whose windows are blended as their sRGB values. `current` is Windows 11
/// 24H2's answer (its flags, and the colour mode it's in: 0 SDR, 1 wide colour, 2 HDR), `older`
/// the one before it (its flags); `None` for one Windows didn't give.
pub fn blends_linearly(current: Option<(u32, i32)>, older: Option<u32>) -> bool {
    match (current, older) {
        (Some((flags, mode)), _) => flags & ADVANCED_COLOR_ON != 0 || mode != 0,
        (None, Some(flags)) => flags & ADVANCED_COLOR_ON != 0,
        (None, None) => false,
    }
}

/// Whether this Windows can show glass now: the system backdrop, the Transparency effects switch
/// on and High Contrast off. Asked at most once a second, as it's read on every frame. GPUI tells
/// of a switch between light and dark only, so a window's appearance change and its coming back
/// to the front (from Windows' Settings, say) call `forget`, and the next frame asks again.
#[cfg(windows)]
pub fn glass_now() -> bool {
    use std::time::{Duration, Instant};
    ASKED.with(|a| match a.get() {
        Some((at, on)) if at.elapsed() < Duration::from_secs(1) => on,
        _ => {
            let on = glass_available(build_number(), transparency_enabled(), high_contrast());
            a.set(Some((Instant::now(), on)));
            on
        }
    })
}

#[cfg(windows)]
thread_local!(static ASKED: std::cell::Cell<Option<(std::time::Instant, bool)>> = const { std::cell::Cell::new(None) });

/// Whether DWM blends glass in linear light now, on any of the displays (`blends_linearly`); never
/// off Windows. Any display rather than the window's own: a window goes from one to another without
/// a word to Trek, and this errs to the side that keeps text readable. Asked as `glass_now`'s
/// switches are: at most once a second, and again after `forget` (Windows' Settings turn HDR and
/// Auto Color Management on and off).
pub fn linear_now() -> bool {
    #[cfg(windows)]
    {
        use std::time::{Duration, Instant};
        LINEAR_ASKED.with(|a| match a.get() {
            Some((at, on)) if at.elapsed() < Duration::from_secs(1) => on,
            _ => {
                let on = any_display_blends_linearly();
                a.set(Some((Instant::now(), on)));
                on
            }
        })
    }
    #[cfg(not(windows))]
    false
}

#[cfg(windows)]
thread_local!(static LINEAR_ASKED: std::cell::Cell<Option<(std::time::Instant, bool)>> = const { std::cell::Cell::new(None) });

/// Ask the switches again at the next `glass_now`, and the displays at the next `linear_now`.
pub fn forget() {
    #[cfg(windows)]
    {
        ASKED.with(|a| a.set(None));
        LINEAR_ASKED.with(|a| a.set(None));
    }
}

/// Whether any display Windows is showing on blends in linear light (`blends_linearly`). Not
/// answered, it's taken as no: an SDR display's.
#[cfg(windows)]
fn any_display_blends_linearly() -> bool {
    use windows_sys::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_HEADER,
        DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DisplayConfigGetDeviceInfo,
        GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
    };
    use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    /// `DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO_2` (Windows 11 24H2), which windows-sys hasn't got,
    /// and what it's asked as.
    #[repr(C)]
    struct ColourNow {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
        flags: u32,
        encoding: i32,
        bits: u32,
        mode: i32,
    }
    const GET_ADVANCED_COLOR_INFO_2: i32 = 15;
    /// `DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO`, its flags read as plainly.
    #[repr(C)]
    struct ColourBefore {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
        flags: u32,
        encoding: i32,
        bits: u32,
    }
    // The displays can change between asking how many there are and asking for them: then again.
    for _ in 0..3 {
        let (mut paths, mut modes) = (0u32, 0u32);
        // SAFETY: both counts are live `u32`s the call writes.
        if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes) }
            != 0
        {
            return false;
        }
        let mut path = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
        let mut mode = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
        // SAFETY: each array is as long as its count, which the call writes no more than and
        // lowers to what it filled; the topology is only for asking the database, so it's null.
        let status = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut paths,
                path.as_mut_ptr(),
                &mut modes,
                mode.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if status == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        if status != 0 {
            return false;
        }
        path.truncate(paths as usize);
        return path.iter().any(|p| {
            let header = |kind: i32, size: usize| DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: kind,
                size: size as u32,
                adapterId: p.targetInfo.adapterId,
                id: p.targetInfo.id,
            };
            let mut now = ColourNow {
                header: header(GET_ADVANCED_COLOR_INFO_2, std::mem::size_of::<ColourNow>()),
                flags: 0,
                encoding: 0,
                bits: 0,
                mode: 0,
            };
            // SAFETY: a request starts with its header, which gives the whole request's size, and
            // Windows writes within that; the pointer is the whole request's. A Windows before 24H2
            // refuses the newer request, and the older one is asked instead.
            let current =
                (unsafe { DisplayConfigGetDeviceInfo((&mut now as *mut ColourNow).cast()) } == 0)
                    .then_some((now.flags, now.mode));
            let older = current.is_none().then(|| {
                let mut before = ColourBefore {
                    header: header(
                        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                        std::mem::size_of::<ColourBefore>(),
                    ),
                    flags: 0,
                    encoding: 0,
                    bits: 0,
                };
                // SAFETY: as above.
                (unsafe { DisplayConfigGetDeviceInfo((&mut before as *mut ColourBefore).cast()) }
                    == 0)
                    .then_some(before.flags)
            });
            blends_linearly(current, older.flatten())
        });
    }
    false
}

/// The Windows build number (`CurrentBuild`), 0 if unreadable.
#[cfg(windows)]
pub fn build_number() -> u32 {
    use std::sync::OnceLock;
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(|| {
        read_string(
            windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuild",
        )
        .and_then(|b| b.parse().ok())
        .unwrap_or(0)
    })
}

/// The Transparency effects switch (Settings › Personalization › Colors). Windows keeps no value
/// until it's been touched, and transparency is on out of the box.
#[cfg(windows)]
fn transparency_enabled() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
        .encode_utf16()
        .chain([0])
        .collect();
    let name: Vec<u16> = "EnableTransparency".encode_utf16().chain([0]).collect();
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: both names end in a NUL; `value` is the 4 bytes `size` says, written on success.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    status != 0 || value != 0
}

#[cfg(windows)]
fn read_string(
    root: windows_sys::Win32::System::Registry::HKEY,
    key: &str,
    name: &str,
) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};
    let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let mut buf = [0u16; 128];
    let mut size = std::mem::size_of_val(&buf) as u32;
    // SAFETY: both names end in a NUL; `buf` is `size` bytes long, and `size` is how many are
    // written (the terminator included) on success.
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    let chars = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..chars])).filter(|s| !s.is_empty())
}

/// Whether a High Contrast theme is on (Settings › Accessibility › Contrast themes).
#[cfg(windows)]
fn high_contrast() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
    /// `HIGHCONTRASTW`, and what's asked of it.
    #[repr(C)]
    struct HighContrast {
        size: u32,
        flags: u32,
        scheme: *mut u16,
    }
    const SPI_GETHIGHCONTRAST: u32 = 0x0042;
    const HCF_HIGHCONTRASTON: u32 = 0x1;
    let mut hc = HighContrast {
        size: std::mem::size_of::<HighContrast>() as u32,
        flags: 0,
        scheme: std::ptr::null_mut(),
    };
    // SAFETY: SPI_GETHIGHCONTRAST fills the `HIGHCONTRASTW` that `pvParam` points to, whose size
    // is in its first field and `uiParam`; `hc` is one, and outlives the call. Its scheme's name is
    // the system's, read only through `flags`.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.size,
            (&mut hc as *mut HighContrast).cast(),
            0,
        )
    };
    read != 0 && hc.flags & HCF_HIGHCONTRASTON != 0
}

/// Whether Windows's "Animation effects" is off (Settings › Accessibility › Visual effects),
/// which is the Windows ask for reduced motion. Not asking is "don't reduce".
#[cfg(windows)]
pub fn reduce_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };
    let mut on = 1i32;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL through `pvParam` and ignores `uiParam`;
    // `on` is a BOOL's size and outlives the call.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut on as *mut i32).cast(),
            0,
        )
    };
    read != 0 && reduces_motion(on != 0)
}

/// Tell DWM how to show what's behind the window, after GPUI's see-through window (or its opaque
/// one, without `glass`): blurred where glass is `frosted`, and as it is at the Frost slider's
/// Clear end, where it isn't. The blur is a plain one (`ACCENT_ENABLE_BLURBEHIND`), under none of
/// Windows' own colour: its materials frost too much for a clear end (Mica lets nothing through,
/// and Acrylic hides two thirds of what's behind under its grey before Trek's colour goes on), and
/// GPUI's blurred window is Acrylic's (`ACCENT_ENABLE_ACRYLICBLURBEHIND`), which lags behind the
/// pointer as a window is dragged. Nothing of the frame reaches into the window, as DWM's would
/// draw minimize, maximize and close there, under Trek's own. `dark` gives the window Windows'
/// dark frame in a dark theme, Trek's own choice rather than the system's (GPUI sets it from the
/// system's appearance only); and Windows 11's rounded corners are asked for.
#[cfg(windows)]
pub fn set_backdrop(window: &gpui_kit::Window, dark: bool, glass: bool, frosted: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut core::ffi::c_void,
            attribute: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    /// `DWMWA_USE_IMMERSIVE_DARK_MODE`.
    const USE_IMMERSIVE_DARK_MODE: u32 = 20;
    /// `DWMWA_WINDOW_CORNER_PREFERENCE`, and its `DWMWCP_ROUND`.
    const WINDOW_CORNER_PREFERENCE: u32 = 33;
    const CORNER_ROUND: i32 = 2;
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = win32.hwnd.get() as *mut core::ffi::c_void;
    let set = |attribute: u32, value: i32| {
        // SAFETY: `hwnd` is the live window GPUI created for this `window`, on its own thread;
        // each attribute takes a 4-byte value, which `value` is and outlives the call. Windows
        // before 11 refuses the corner one, which is the answer wanted there.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attribute,
                (&value as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            )
        };
    };
    set(USE_IMMERSIVE_DARK_MODE, dark as i32);
    // Windows 11 rounds a window by itself only if it has a resizing frame and a caption it draws,
    // and GPUI's draw none, as Trek draws its own title bar: they have to ask.
    set(WINDOW_CORNER_PREFERENCE, CORNER_ROUND);
    if glass && frosted {
        blur_behind(hwnd);
    }
}

/// Blur what's behind `hwnd`, through user32's `SetWindowCompositionAttribute`, which GPUI asks
/// for its see-through and blurred windows with too (it's not in Windows' headers).
#[cfg(windows)]
fn blur_behind(hwnd: *mut core::ffi::c_void) {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    /// `ACCENT_POLICY`, and the `WINDOWCOMPOSITIONATTRIBDATA` that carries it.
    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        gradient: u32,
        animation: u32,
    }
    #[repr(C)]
    struct CompositionData {
        attribute: u32,
        data: *mut core::ffi::c_void,
        size: usize,
    }
    /// `WCA_ACCENT_POLICY`, and `ACCENT_ENABLE_BLURBEHIND`.
    const ACCENT_POLICY: u32 = 19;
    const ENABLE_BLURBEHIND: u32 = 3;
    type SetWindowCompositionAttribute =
        unsafe extern "system" fn(*mut core::ffi::c_void, *mut CompositionData) -> i32;
    // SAFETY: user32 is loaded in any process with a window, and both names are NUL-terminated
    // statics. What's found is user32's `SetWindowCompositionAttribute`, whose signature this is;
    // `hwnd` is the live window GPUI made, on its own thread, and the policy and what carries it
    // outlive the call, which only reads them.
    unsafe {
        let user32 = GetModuleHandleW(windows_sys::w!("user32.dll"));
        let Some(found) = GetProcAddress(user32, c"SetWindowCompositionAttribute".as_ptr().cast())
        else {
            return;
        };
        let set_attribute: SetWindowCompositionAttribute = std::mem::transmute(found);
        let mut accent = AccentPolicy {
            state: ENABLE_BLURBEHIND,
            flags: 0,
            gradient: 0,
            animation: 0,
        };
        let mut data = CompositionData {
            attribute: ACCENT_POLICY,
            data: (&mut accent as *mut AccentPolicy).cast(),
            size: std::mem::size_of::<AccentPolicy>(),
        };
        set_attribute(hwnd, &mut data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glass_needs_windows_11_22h2_and_the_transparency_switch() {
        // Windows 10 (19045) and 11 before 22H2: no glass, whatever the switch says.
        assert!(!glass_available(19045, true, false));
        assert!(!glass_available(22000, true, false));
        assert!(!glass_available(GLASS_BUILD - 1, true, false));
        // 22H2 and later: glass, unless transparency effects are off.
        assert!(glass_available(GLASS_BUILD, true, false));
        assert!(glass_available(26100, true, false));
        assert!(!glass_available(26100, false, false));
        // An unreadable build (0) is too old.
        assert!(!glass_available(0, true, false));
    }

    #[test]
    fn high_contrast_keeps_the_window_opaque() {
        assert!(!glass_available(26100, true, true));
        assert!(!glass_available(26100, false, true));
    }

    /// The reads themselves run here and answer something sensible; what they answer is this
    /// machine's own setting, so only the build number is held to a value.
    #[cfg(windows)]
    #[test]
    fn the_reads_work_on_this_machine() {
        assert!(
            build_number() >= 19041,
            "a Windows 10 or 11 build number, got {}",
            build_number()
        );
        let _ = (reduce_motion(), glass_now(), linear_now());
        forget();
    }

    /// HDR, and Auto Color Management's wide colour, blend in linear light; SDR doesn't. Windows
    /// 11 24H2's answer is taken over the older one, which says only whether either is on.
    #[test]
    fn hdr_and_auto_color_management_blend_in_linear_light() {
        let (sdr, wide, hdr) = (0, 1, 2);
        // 24H2: active, or in a mode other than SDR.
        assert!(blends_linearly(Some((ADVANCED_COLOR_ON, hdr)), None));
        assert!(blends_linearly(Some((ADVANCED_COLOR_ON, wide)), None));
        assert!(blends_linearly(Some((0, wide)), None));
        // Supported and switched on in Settings, but not active (this laptop's panel: no
        // profile for it): SDR still. Whatever the older answer says.
        let supported_and_asked_for = 0x1 | 0x40 | 0x80;
        assert!(!blends_linearly(Some((supported_and_asked_for, sdr)), None));
        assert!(!blends_linearly(
            Some((supported_and_asked_for, sdr)),
            Some(ADVANCED_COLOR_ON)
        ));
        // Before 24H2: the older answer's flag.
        assert!(blends_linearly(None, Some(0x1 | ADVANCED_COLOR_ON)));
        assert!(!blends_linearly(None, Some(0x1 | 0x4)));
        // No answer at all: an SDR display.
        assert!(!blends_linearly(None, None));
    }

    #[test]
    fn animation_effects_off_is_reduce_motion() {
        assert!(reduces_motion(false));
        assert!(!reduces_motion(true));
    }
}
