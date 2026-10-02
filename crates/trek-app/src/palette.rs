//! Trek's semantic colors beyond the GPUI Kit theme tokens: run-state beacons and hand-holding tints.
//! "Color only for act now / in motion / broken" — everything else uses muted theme tokens.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Hsla, rgb};

fn pick(cx: &App, night: u32, paper: u32) -> Hsla {
    if cx.theme().mode.is_dark() { rgb(night).into() } else { rgb(paper).into() }
}

pub fn ember(cx: &App) -> Hsla {
    pick(cx, 0xFF7A3D, 0xE85D1F)
}
pub fn amber(cx: &App) -> Hsla {
    pick(cx, 0xFFB020, 0xC98500)
}
pub fn indigo(cx: &App) -> Hsla {
    pick(cx, 0x8B8CFF, 0x5B5BD6)
}
pub fn emerald(cx: &App) -> Hsla {
    pick(cx, 0x3FCF8E, 0x1E9E62)
}
pub fn red(cx: &App) -> Hsla {
    pick(cx, 0xFF5A5F, 0xD93A3F)
}
pub fn sky(cx: &App) -> Hsla {
    pick(cx, 0x5AA9FF, 0x2F7FE0)
}

/// Sunrise gradient stops used by the logo and the effort meter.
pub const SUNRISE: [u32; 3] = [0xFF4D2E, 0xFF8A3D, 0xFFC56B];

pub fn sunrise_at(t: f32) -> Hsla {
    let t = t.clamp(0.0, 1.0);
    let (a, b, u) = if t < 0.55 { (SUNRISE[0], SUNRISE[1], t / 0.55) } else { (SUNRISE[1], SUNRISE[2], (t - 0.55) / 0.45) };
    let ch = |c: u32, s: u32| ((c >> s) & 0xFF) as f32;
    let mix = |s: u32| ch(a, s) + (ch(b, s) - ch(a, s)) * u;
    let v = ((mix(16) as u32) << 16) | ((mix(8) as u32) << 8) | mix(0) as u32;
    rgb(v).into()
}
