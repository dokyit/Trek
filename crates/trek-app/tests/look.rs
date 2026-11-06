//! How Trek looks the same on both platforms: the fonts it ships, and that fixed-height controls
//! hold their text in them.

mod common;

use common::{open, run};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{Font, FontWeight, font, px};
use trek_app::fonts::{MONO, UI};
use trek_app::workspace::Mode;

#[test]
fn the_shipped_families_are_registered_and_the_theme_names_them() {
    run(async |cx| {
        let trek = open(cx);
        let names = cx.update(|cx| cx.text_system().all_font_names());
        assert!(names.iter().any(|n| n == UI), "{UI} is registered");
        assert!(names.iter().any(|n| n == MONO), "{MONO} is registered");
        // Both themes, so Paper isn't left on the system's fonts.
        for choice in [
            trek_core::settings::ThemeChoice::Night,
            trek_core::settings::ThemeChoice::Paper,
        ] {
            cx.update(|cx| trek_app::apply_theme(choice, None, cx));
            trek.render(cx);
            let (ui, mono) = cx.update(|cx| {
                (
                    cx.theme().font_family.to_string(),
                    cx.theme().mono_font_family.to_string(),
                )
            });
            assert_eq!((ui.as_str(), mono.as_str()), (UI, MONO), "{choice:?}");
        }
    });
}

#[test]
fn every_weight_the_interface_uses_is_its_own_face() {
    run(async |cx| {
        let _trek = open(cx);
        cx.update(|cx| {
            let text = cx.text_system();
            let face = |family: &'static str, weight: FontWeight| text.resolve_font(&Font { weight, ..font(family) });
            for family in [UI, MONO] {
                let ids = [FontWeight::NORMAL, FontWeight::MEDIUM, FontWeight::SEMIBOLD, FontWeight::BOLD].map(|w| face(family, w));
                for (i, a) in ids.iter().enumerate() {
                    for b in &ids[i + 1..] {
                        assert_ne!(a, b, "{family}: regular, medium, semibold and bold are four faces, not one stood in for all");
                    }
                }
            }
            // The mono face is on a 0.6 em grid (as Menlo and Consolas are), so columns line up
            // the way the layout code that counts characters expects.
            let mono = face(MONO, FontWeight::NORMAL);
            let advance = text.em_advance(mono, px(100.)).expect("advance").as_f32();
            assert!((advance - 60.).abs() < 0.5, "{MONO}'s advance is {advance} per 100 px");
        });
    });
}

#[test]
fn the_command_pill_holds_its_text_on_both_platforms() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        trek.render(cx);
        let pill = trek
            .bounds(cx, "command-center")
            .expect("the command pill is on screen");
        assert_eq!(
            pill.size.height,
            px(26.),
            "the pill is 26 px tall at every font"
        );
        // Inside its border, the tallest face the pill sets (the label is Inter at 12.5 px, the
        // folder's name medium weight) must fit from ascender to descender, on this platform's
        // text system.
        let inner = pill.size.height.as_f32() - 2.;
        cx.update(|cx| {
            let text = cx.text_system();
            for weight in [FontWeight::NORMAL, FontWeight::MEDIUM] {
                let id = text.resolve_font(&Font { weight, ..font(UI) });
                let natural =
                    text.ascent(id, px(12.5)).as_f32() + text.descent(id, px(12.5)).abs().as_f32();
                assert!(
                    natural <= inner,
                    "{weight:?}: {natural} px of type in a {inner} px pill"
                );
            }
        });
    });
}

/// An sRGB channel (0–1) as linear light.
fn lin(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light as an sRGB channel.
fn encode(l: f32) -> f32 {
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1. / 2.4) - 0.055
    }
}

/// WCAG relative luminance of an sRGB colour (0–1 channels).
fn luminance([r, g, b]: [f32; 3]) -> f32 {
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

fn contrast(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `top` laid at `alpha` over `under`, as an SDR display's windows are blended: their sRGB values.
fn over(top: [f32; 3], alpha: f32, under: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| top[i] * alpha + under[i] * (1. - alpha))
}

/// `top` laid at `alpha` over `under` in linear light, as Windows blends with HDR or Auto Color
/// Management on.
fn over_linear(top: [f32; 3], alpha: f32, under: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| encode(lin(top[i]) * alpha + lin(under[i]) * (1. - alpha)))
}

/// What DWM's Acrylic is under Trek's chrome on Windows 11, in each theme's tone: over a black
/// window behind, a white one, and the worst of a busy picture of saturated colour and text.
/// Taken from the screen, with the chrome's own colour taken back out at the Frost slider's Clear
/// end and its middle (the two agree). White is Night's worst (its brightest Acrylic), black
/// Paper's.
const ACRYLIC: [(trek_core::settings::ThemeChoice, [[f32; 3]; 3]); 2] = [
    (
        trek_core::settings::ThemeChoice::Night,
        [
            [0.224, 0.224, 0.216],
            [0.592, 0.592, 0.583],
            [0.535, 0.592, 0.567],
        ],
    ),
    (
        trek_core::settings::ThemeChoice::Paper,
        [
            [0.519, 0.534, 0.511],
            [0.879, 0.895, 0.871],
            [0.747, 0.743, 0.616],
        ],
    ),
];

fn rgb(c: gpui_kit::Hsla) -> [f32; 3] {
    let c = c.to_rgb();
    [c.r, c.g, c.b]
}

#[test]
fn text_stays_readable_on_windows_glass_in_both_themes_at_every_tint() {
    use trek_app::ui::{Material, chrome_alpha, glass_quiet_text, panel_alpha};
    // On an SDR display; `..._blended_in_linear_light` below is HDR's and Auto Color Management's.
    let acrylic_glass = Material::Acrylic { linear: false };
    run(async |cx| {
        let _trek = open(cx);
        for (choice, acrylics) in ACRYLIC {
            cx.update(|cx| trek_app::apply_theme(choice, None, cx));
            let (bg, side, fg, muted, dark) = cx.update(|cx| {
                let t = cx.theme();
                (
                    t.background,
                    t.sidebar,
                    t.foreground,
                    t.muted_foreground,
                    t.mode.is_dark(),
                )
            });
            // On glass the quiet text is glass's own (`ui::glass_quiet_text`).
            let quiet = glass_quiet_text(dark);
            for acrylic in acrylics {
                for step in 0..=15 {
                    let tint = 0.2 + 0.05 * step as f32;
                    // An inset panel (the transcript) is solid on Windows: its text, and the quiet
                    // text glass swaps in for the whole theme, and the theme's own after.
                    let panel = over(rgb(bg), panel_alpha(tint, acrylic_glass), acrylic);
                    let (c_fg, c_muted) = (
                        contrast(rgb(fg), panel),
                        contrast(rgb(quiet), panel)
                            .min(contrast(over(rgb(muted), muted.a, panel), panel)),
                    );
                    assert!(
                        c_fg >= 7.,
                        "{choice:?} panel at tint {tint:.2} over {acrylic:?}: text {c_fg:.1}:1"
                    );
                    assert!(
                        c_muted >= 4.5,
                        "{choice:?} panel at tint {tint:.2} over {acrylic:?}: quiet text {c_muted:.1}:1"
                    );
                    // The chrome (sidebar, title bar): the sidebar's colour over the Acrylic.
                    let chrome = over(rgb(side), chrome_alpha(tint, acrylic_glass, dark), acrylic);
                    let (c_fg, c_quiet) = (contrast(rgb(fg), chrome), contrast(rgb(quiet), chrome));
                    assert!(
                        c_fg >= 7.,
                        "{choice:?} chrome at tint {tint:.2} over {acrylic:?}: text {c_fg:.1}:1"
                    );
                    assert!(
                        c_quiet >= 4.5,
                        "{choice:?} chrome at tint {tint:.2} over {acrylic:?}: quiet text {c_quiet:.1}:1"
                    );
                }
            }
            // Glass's quiet text keeps its step down from the text, in both themes.
            assert!(
                contrast(rgb(fg), rgb(quiet)) >= 1.8,
                "{choice:?}: quiet text as loud as text"
            );
        }
    });
}

/// With HDR or Auto Color Management on, Windows blends every window in linear light (scRGB)
/// rather than as its sRGB values, and dark colour laid over light lets far more of it through:
/// Night's chrome keeps more of its colour there (`ui::acrylic_chrome_floor`). No SDR screen
/// shows it, so it's two models, from the measured Acrylic: Trek's chrome laid over that Acrylic
/// in linear light; and the Acrylic laid in linear light too, as the layer of colour over what's
/// behind it is over the black and white windows. In both, the text reads at every tint, and the
/// glass still shows at Clear, more than it does on an SDR screen at Frosted (Night's, with the
/// Acrylic in linear light, about as much as at Clear).
#[test]
fn text_stays_readable_on_windows_glass_blended_in_linear_light() {
    use trek_app::ui::{Material, chrome_alpha, glass_quiet_text};
    let (sdr, linear) = (
        Material::Acrylic { linear: false },
        Material::Acrylic { linear: true },
    );
    run(async |cx| {
        let _trek = open(cx);
        for (choice, [black, white, busy]) in ACRYLIC {
            cx.update(|cx| trek_app::apply_theme(choice, None, cx));
            let (side, fg, dark) = cx.update(|cx| {
                let t = cx.theme();
                (rgb(t.sidebar), rgb(t.foreground), t.mode.is_dark())
            });
            let quiet = rgb(glass_quiet_text(dark));
            // The Acrylic as a layer, `k` of colour `t` over what's behind, as its sRGB values over
            // the black and white windows say (over grey that layer is right to a fifth of a
            // level); what was behind the busy picture's, by the same.
            let k = [0, 1, 2].map(|i| 1. - (white[i] - black[i]));
            let t = [0, 1, 2].map(|i| black[i] / k[i]);
            let behind =
                |a: [f32; 3]| [0, 1, 2].map(|i| ((a[i] - k[i] * t[i]) / (1. - k[i])).clamp(0., 1.));
            let laid =
                |b: [f32; 3]| [0, 1, 2].map(|i| encode(k[i] * lin(t[i]) + (1. - k[i]) * lin(b[i])));
            let models = |acrylic: [f32; 3]| {
                [
                    ("over the measured Acrylic", acrylic),
                    ("over the Acrylic in linear light", laid(behind(acrylic))),
                ]
            };
            for acrylic in [black, white, busy] {
                for (model, under) in models(acrylic) {
                    for step in 0..=15 {
                        let tint = 0.2 + 0.05 * step as f32;
                        let chrome = over_linear(side, chrome_alpha(tint, linear, dark), under);
                        let (c_fg, c_quiet) = (contrast(fg, chrome), contrast(quiet, chrome));
                        assert!(
                            c_fg >= 7.,
                            "{choice:?} chrome at tint {tint:.2} {model} {acrylic:?}: text {c_fg:.1}:1"
                        );
                        assert!(
                            c_quiet >= 4.5,
                            "{choice:?} chrome at tint {tint:.2} {model} {acrylic:?}: quiet text {c_quiet:.1}:1"
                        );
                    }
                }
            }
            // How far a white window behind moves the chrome from a black one: the glass showing.
            let sdr_shows = |tint: f32| {
                let a = chrome_alpha(tint, sdr, dark);
                contrast(over(side, a, white), over(side, a, black))
            };
            let [(_, measured_white), (_, laid_white)] = models(white);
            let [(_, measured_black), (_, laid_black)] = models(black);
            let a = chrome_alpha(0.2, linear, dark);
            for (model, shows) in [
                (
                    "over the measured Acrylic",
                    contrast(
                        over_linear(side, a, measured_white),
                        over_linear(side, a, measured_black),
                    ),
                ),
                (
                    "over the Acrylic in linear light",
                    contrast(
                        over_linear(side, a, laid_white),
                        over_linear(side, a, laid_black),
                    ),
                ),
            ] {
                assert!(
                    shows > sdr_shows(0.95),
                    "{choice:?} {model}: Clear shows {shows:.3}, under SDR's Frosted {:.3}",
                    sdr_shows(0.95)
                );
                if dark && model == "over the Acrylic in linear light" {
                    assert!(
                        shows >= 0.95 * sdr_shows(0.2),
                        "{choice:?} {model}: Clear shows {shows:.3}, against SDR's {:.3}",
                        sdr_shows(0.2)
                    );
                }
            }
        }
    });
}

#[test]
fn glass_picks_its_material_by_platform_and_switch() {
    use gpui_kit::WindowBackgroundAppearance as W;
    use trek_app::ui::backdrop;
    // Off: opaque, whatever the platform has.
    assert_eq!(backdrop(false, false, true), W::Opaque);
    assert_eq!(backdrop(false, true, false), W::Opaque);
    // macOS 26: the system's glass laid under a transparent window; macOS before: GPUI's blur.
    assert_eq!(backdrop(true, true, false), W::Transparent);
    assert_eq!(backdrop(true, false, false), W::Blurred);
    // Windows 11 22H2 and later: GPUI's system-backdrop path (winlook then asks DWM for Acrylic).
    assert_eq!(backdrop(true, false, true), W::MicaAltBackdrop);
    // Which Windows can: the system backdrop needs 22H2, and the Transparency effects switch on (Windows 10, or
    // the switch off, leaves the window opaque: glass is never asked for).
    assert!(!trek_app::winlook::glass_available(19045, true, false));
    assert!(trek_app::winlook::glass_available(22631, true, false));
    assert!(!trek_app::winlook::glass_available(22631, false, false));
}
