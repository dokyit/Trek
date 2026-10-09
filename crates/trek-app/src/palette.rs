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
/// Links in agent answers: ember, a shade darker on Paper so it reads as body-size text.
pub(crate) const LINK: (u32, u32) = (0xFF7A3D, 0xC2410C);

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
pub fn link(cx: &App) -> Hsla {
    pick(cx, LINK)
}

/// Categorical hues for chart series, in a fixed order that keeps neighbours apart for
/// colour-blind readers (checked for adjacent-pair ΔE ≥ 8 under deutan/protan/tritan
/// simulation). Paper's emerald and amber are a step lighter than their text colours: these
/// are marks, never text.
const SERIES: [(u32, u32); 6] = [(0x5AA9FF, 0x2B6CC4), (0xFF7A3D, 0xE85D1F), (0x3FCF8E, 0x1F9D63), (0x8B8CFF, 0x5B5BD6), (0xFFB020, 0xB07800), (0xF472B6, 0xDB2777)];

/// Series `ix`'s mark colour (the order repeats past six; the schema allows no more).
pub fn series(ix: usize, cx: &App) -> Hsla {
    pick(cx, SERIES[ix % SERIES.len()])
}

/// A thread at work (its turn running, or waiting on its sub-agents): ember, in every mode.
pub fn working(cx: &App) -> Hsla {
    ember(cx)
}
/// A thread waiting on the user (an approval, a question, a plan): amber, in every mode.
pub fn needs_you(cx: &App) -> Hsla {
    amber(cx)
}
/// A thread whose turn failed: red, in every mode.
pub fn failed(cx: &App) -> Hsla {
    red(cx)
}
/// A thread's run-state colour, the same in the sidebar, the tabs and the IDE; `None` when idle.
pub fn run_state(state: trek_core::RunState, cx: &App) -> Option<Hsla> {
    match state {
        trek_core::RunState::Working => Some(working(cx)),
        trek_core::RunState::NeedsYou => Some(needs_you(cx)),
        trek_core::RunState::Failed => Some(failed(cx)),
        trek_core::RunState::Idle => None,
    }
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
        for (name, (_, paper)) in [("amber", AMBER), ("indigo", INDIGO), ("emerald", EMERALD), ("red", RED), ("sky", SKY), ("link", LINK)] {
            for bg in &surfaces {
                let ratio = contrast(paper, *bg);
                assert!(ratio >= 4.5, "{name} #{paper:06X} on #{bg:06X}: {ratio:.2}:1");
            }
        }
    }
}
