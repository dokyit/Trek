//! Trek's semantic colors beyond the GPUI Kit theme tokens: run-state beacons and hand-holding tints.
//! "Color only for act now / in motion / broken" — everything else uses muted theme tokens.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Hsla, rgb};

fn pick(cx: &App, (night, paper): (u32, u32)) -> Hsla {
    if cx.theme().mode.is_dark() { rgb(night).into() } else { rgb(paper).into() }
}

// (Night, Paper). The status colours double as small text (card states, warnings, diff counts),
// so their Paper values are dark enough to read on Paper's tinted surfaces (`tests`).
const EMBER: (u32, u32) = (0xFF7A3D, 0xE85D1F);
const AMBER: (u32, u32) = (0xFFB020, 0x976300);
const INDIGO: (u32, u32) = (0x8B8CFF, 0x5B5BD6);
const EMERALD: (u32, u32) = (0x3FCF8E, 0x177D4E);
const RED: (u32, u32) = (0xFF5A5F, 0xCC343A);
const SKY: (u32, u32) = (0x5AA9FF, 0x2B6CC4);

pub fn ember(cx: &App) -> Hsla {
    pick(cx, EMBER)
}
pub fn amber(cx: &App) -> Hsla {
    pick(cx, AMBER)
}
pub fn indigo(cx: &App) -> Hsla {
    pick(cx, INDIGO)
}
pub fn emerald(cx: &App) -> Hsla {
    pick(cx, EMERALD)
}
pub fn red(cx: &App) -> Hsla {
    pick(cx, RED)
}
pub fn sky(cx: &App) -> Hsla {
    pick(cx, SKY)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance of `0xRRGGBB`.
    fn luminance(c: u32) -> f64 {
        let channel = |shift: u32| {
            let v = ((c >> shift) & 0xFF) as f64 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }

    fn contrast(a: u32, b: u32) -> f64 {
        let (hi, lo) = (luminance(a).max(luminance(b)), luminance(a).min(luminance(b)));
        (hi + 0.05) / (lo + 0.05)
    }

    /// Paper's surfaces small text sits on, from the theme file.
    fn paper_surfaces() -> Vec<u32> {
        let themes: serde_json::Value = serde_json::from_str(include_str!("../assets/themes/trek.json")).unwrap();
        let paper = themes["themes"].as_array().unwrap().iter().find(|t| t["name"] == "Trek Paper").expect("Trek Paper");
        ["background", "sidebar.background", "muted.background", "popover.background"]
            .iter()
            .map(|k| u32::from_str_radix(paper["colors"][*k].as_str().unwrap().trim_start_matches('#'), 16).unwrap())
            .collect()
    }

    #[test]
    fn status_colours_are_readable_as_small_text_on_paper() {
        let surfaces = paper_surfaces();
        assert_eq!(surfaces.len(), 4);
        for (name, (_, paper)) in [("amber", AMBER), ("indigo", INDIGO), ("emerald", EMERALD), ("red", RED), ("sky", SKY)] {
            for bg in &surfaces {
                let ratio = contrast(paper, *bg);
                assert!(ratio >= 4.5, "{name} #{paper:06X} on #{bg:06X}: {ratio:.2}:1");
            }
        }
    }
}
