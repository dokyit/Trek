//! Trek's semantic colors beyond the GPUI Kit theme tokens: run-state beacons and hand-holding tints.
//! "Color only for act now / in motion / broken" — everything else uses muted theme tokens.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Global, Hsla, rgb};

pub mod scale;

fn pick(cx: &App, (night, paper): (u32, u32)) -> Hsla {
    if cx.theme().mode.is_dark() {
        rgb(night).into()
    } else {
        rgb(paper).into()
    }
}

// (Night, Paper). The status colours double as small text (card states, warnings, diff counts),
// so their Paper values are dark enough to read on Paper's tinted surfaces (`tests`).
const EMBER: (u32, u32) = (0xFF7A3D, 0xE85D1F);
const AMBER: (u32, u32) = (0xFFB020, 0x976300);
const INDIGO: (u32, u32) = (0x8B8CFF, 0x5B5BD6);
const EMERALD: (u32, u32) = (0x3FCF8E, 0x177D4E);
const RED: (u32, u32) = (0xFF5A5F, 0xCC343A);
const SKY: (u32, u32) = (0x5AA9FF, 0x2B6CC4);
/// The composer's send and voice ✓ buttons: a blue fill under a white glyph.
const SEND: (u32, u32) = (0x3D6EEB, 0x2A5FE4);
/// A `/` command or skill accepted into the composer: plain blue text (its ghost completion is
/// the same blue, faded).
const TOKEN: (u32, u32) = (0x86ABF5, 0x3E6DC2);
/// Links in agent answers: ember, a shade darker on Paper so it reads as body-size text.
pub const LINK: (u32, u32) = (0xFF7A3D, 0xC2410C);

/// A Trek theme's accent and the ink drawn on it (`trek_core::themes`), which Trek's own accents
/// take while the theme is in use: the working beacon, links in answers, and the send button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Accent {
    pub accent: Hsla,
    pub on_accent: Hsla,
}

/// The theme's accents for dark and light; `None` is Trek's own.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ThemeAccents {
    pub dark: Option<Accent>,
    pub light: Option<Accent>,
}

impl Global for ThemeAccents {}

/// Use `accents` from the next frame on (a theme applied, or a step of the switch to one).
pub fn set_theme_accents(accents: ThemeAccents, cx: &mut App) {
    if cx.try_global::<ThemeAccents>() != Some(&accents) {
        cx.set_global(accents);
    }
}

/// The theme's accent for the mode on screen, if it has one.
pub fn theme_accent(cx: &App) -> Option<Accent> {
    let accents = cx.try_global::<ThemeAccents>()?;
    if cx.theme().mode.is_dark() {
        accents.dark
    } else {
        accents.light
    }
}

pub fn ember(cx: &App) -> Hsla {
    if let Some(a) = theme_accent(cx) {
        return a.accent;
    }
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
pub fn send(cx: &App) -> Hsla {
    if let Some(a) = theme_accent(cx) {
        return a.accent;
    }
    pick(cx, SEND)
}
/// The glyph on a [`send`] fill: white, or the theme's ink for its accent.
pub fn on_send(cx: &App) -> Hsla {
    theme_accent(cx).map_or(gpui_kit::white(), |a| a.on_accent)
}
pub fn token(cx: &App) -> Hsla {
    pick(cx, TOKEN)
}
pub fn link(cx: &App) -> Hsla {
    if let Some(a) = theme_accent(cx) {
        return a.accent;
    }
    pick(cx, LINK)
}

/// Categorical hues for chart series, in a fixed order that keeps neighbours apart for
/// colour-blind readers (checked for adjacent-pair ΔE ≥ 8 under deutan/protan/tritan
/// simulation). Paper's emerald and amber are a step lighter than their text colours: these
/// are marks, never text.
const SERIES: [(u32, u32); 6] = [
    (0x5AA9FF, 0x2B6CC4),
    (0xFF7A3D, 0xE85D1F),
    (0x3FCF8E, 0x1F9D63),
    (0x8B8CFF, 0x5B5BD6),
    (0xFFB020, 0xB07800),
    (0xF472B6, 0xDB2777),
];

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
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }

    fn contrast(a: u32, b: u32) -> f64 {
        let (hi, lo) = (
            luminance(a).max(luminance(b)),
            luminance(a).min(luminance(b)),
        );
        (hi + 0.05) / (lo + 0.05)
    }

    /// Paper's surfaces small text sits on, from the theme file.
    fn paper_surfaces() -> Vec<u32> {
        let themes: serde_json::Value =
            serde_json::from_str(include_str!("../../trek-assets/assets/themes/trek.json"))
                .unwrap();
        let paper = themes["themes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "Trek Paper")
            .expect("Trek Paper");
        [
            "background",
            "sidebar.background",
            "muted.background",
            "popover.background",
        ]
        .iter()
        .map(|k| {
            u32::from_str_radix(
                paper["colors"][*k]
                    .as_str()
                    .unwrap()
                    .trim_start_matches('#'),
                16,
            )
            .unwrap()
        })
        .collect()
    }

    /// Every colour in theme `name` that's a grey (its channels within 0x20 of each other), as
    /// (key, 0xRRGGBB), translucent ones included by their colour.
    fn greys(name: &str) -> Vec<(String, u32)> {
        let themes: serde_json::Value =
            serde_json::from_str(include_str!("../../trek-assets/assets/themes/trek.json"))
                .unwrap();
        let theme = themes["themes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .expect(name);
        theme["colors"]
            .as_object()
            .unwrap()
            .iter()
            .filter_map(|(k, v)| {
                let hex = v.as_str()?.trim_start_matches('#');
                let c = u32::from_str_radix(hex.get(..6)?, 16).ok()?;
                let (r, g, b) = ((c >> 16) & 255, (c >> 8) & 255, c & 255);
                // The ink on an ember fill is ember-dark on purpose.
                let on_ember = k.ends_with("primary.foreground");
                (r.max(g).max(b) - r.min(g).min(b) <= 0x20 && !on_ember).then(|| (k.clone(), c))
            })
            .collect()
    }

    #[test]
    fn each_theme_s_greys_are_one_family() {
        // Night's greys are zinc: red and green level, blue a touch above, never the blue-grey
        // of another theme. Paper's are warm: red over blue by a little, never a tan.
        let night = greys("Trek Night");
        assert!(night.len() > 30, "{}", night.len());
        for (k, c) in night {
            let (r, g, b) = (
                (c >> 16) as i32 & 255,
                (c >> 8) as i32 & 255,
                c as i32 & 255,
            );
            assert!(
                (g - r).abs() <= 1 && (0..=9).contains(&(b - r)),
                "Night {k} #{c:06X} isn't a zinc grey"
            );
        }
        let paper = greys("Trek Paper");
        assert!(paper.len() > 30, "{}", paper.len());
        for (k, c) in paper {
            let (r, g, b) = (
                (c >> 16) as i32 & 255,
                (c >> 8) as i32 & 255,
                c as i32 & 255,
            );
            assert!(
                (-2..=10).contains(&(r - b)) && g <= r.max(b) && g >= r.min(b),
                "Paper {k} #{c:06X} isn't a warm grey"
            );
        }
    }

    #[test]
    fn status_colours_are_readable_as_small_text_on_paper() {
        let surfaces = paper_surfaces();
        assert_eq!(surfaces.len(), 4);
        for (name, (_, paper)) in [
            ("amber", AMBER),
            ("indigo", INDIGO),
            ("emerald", EMERALD),
            ("red", RED),
            ("sky", SKY),
            ("link", LINK),
            ("token", TOKEN),
        ] {
            for bg in &surfaces {
                let ratio = contrast(paper, *bg);
                assert!(
                    ratio >= 4.5,
                    "{name} #{paper:06X} on #{bg:06X}: {ratio:.2}:1"
                );
            }
        }
    }

    /// A colour of theme `name` from the theme file, as 0xRRGGBB.
    fn colour(name: &str, key: &str) -> u32 {
        let themes: serde_json::Value =
            serde_json::from_str(include_str!("../../trek-assets/assets/themes/trek.json"))
                .unwrap();
        let theme = themes["themes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .expect(name);
        let hex = theme["colors"][key].as_str().expect(key);
        u32::from_str_radix(&hex.trim_start_matches('#')[..6], 16).unwrap()
    }

    #[test]
    fn red_buttons_and_red_text_both_read_in_each_theme() {
        // A red button's white text, at rest and under the pointer, and red text on the
        // window: no one red does both on Night, so buttons take `danger` and text takes `red`.
        for name in ["Trek Night", "Trek Paper"] {
            let ink = colour(name, "danger.foreground");
            for fill in ["danger.background", "danger.hover.background"] {
                let c = contrast(ink, colour(name, fill));
                assert!(c >= 4.5, "{name}: text on {fill} is {c:.2}:1");
            }
            let c = contrast(colour(name, "base.red"), colour(name, "background"));
            assert!(c >= 4.5, "{name}: red text on the window is {c:.2}:1");
        }
    }
}
