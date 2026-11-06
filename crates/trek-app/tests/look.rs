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

/// What can be behind Windows' glass, as sRGB values: a black window, a white one, and grey and
/// saturated colours between them (the cyan is a bright wallpaper's). The blur changes nothing over
/// a flat window, and over a busier one only averages what's there, so black and white bound it.
const BEHIND: [[f32; 3]; 6] = [
    [0., 0., 0.],
    [1., 1., 1.],
    [0.5, 0.5, 0.5],
    [0., 0., 1.],
    [1., 1., 0.],
    [0.18, 0.77, 0.88],
];

fn rgb(c: gpui_kit::Hsla) -> [f32; 3] {
    let c = c.to_rgb();
    [c.r, c.g, c.b]
}

/// On Windows the Frost slider runs from Clear, where what's behind shows as it is, to Frosted.
/// From its middle up the text reads (7:1, the quiet text 4.5:1) whatever's behind: a white window
/// at a dark theme's worst, a black one at a light theme's. What's read sits on solid panes at
/// every tint. And Clear is clear: a white window behind and a black one look worlds apart.
#[test]
fn windows_glass_reads_from_the_middle_of_frost_up() {
    use trek_app::ui::{Material, chrome_alpha, glass_quiet_text, panel_alpha};
    use trek_core::settings::ThemeChoice;
    // On an SDR display; `windows_glass_reads_in_linear_light_too` is HDR's and Auto Color
    // Management's.
    let glass = Material::SeeThrough { linear: false };
    run(async |cx| {
        let _trek = open(cx);
        for choice in [ThemeChoice::Night, ThemeChoice::Paper] {
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
            for step in 0..=100 {
                let tint = 0.2 + 0.0075 * step as f32;
                for behind in BEHIND {
                    // An inset panel (the transcript) is solid on Windows: its text, and the quiet
                    // text glass swaps in for the whole theme, and the theme's own after.
                    let panel = over(rgb(bg), panel_alpha(tint, glass), behind);
                    let (c_fg, c_muted) = (
                        contrast(rgb(fg), panel),
                        contrast(rgb(quiet), panel)
                            .min(contrast(over(rgb(muted), muted.a, panel), panel)),
                    );
                    assert!(
                        c_fg >= 7.,
                        "{choice:?} panel at tint {tint:.2} over {behind:?}: text {c_fg:.1}:1"
                    );
                    assert!(
                        c_muted >= 4.5,
                        "{choice:?} panel at tint {tint:.2} over {behind:?}: quiet text {c_muted:.1}:1"
                    );
                    // The chrome (sidebar, title bar): the sidebar's colour over what's behind.
                    if step < 50 {
                        continue;
                    }
                    let chrome = over(rgb(side), chrome_alpha(tint, glass, dark), behind);
                    let (c_fg, c_quiet) = (contrast(rgb(fg), chrome), contrast(rgb(quiet), chrome));
                    assert!(
                        c_fg >= 7.,
                        "{choice:?} chrome at tint {tint:.2} over {behind:?}: text {c_fg:.1}:1"
                    );
                    assert!(
                        c_quiet >= 4.5,
                        "{choice:?} chrome at tint {tint:.2} over {behind:?}: quiet text {c_quiet:.1}:1"
                    );
                }
            }
            let clear = chrome_alpha(0.2, glass, dark);
            let shows = contrast(
                over(rgb(side), clear, [1.; 3]),
                over(rgb(side), clear, [0.; 3]),
            );
            assert!(shows >= 10., "{choice:?}: Clear shows {shows:.1}:1");
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
/// there the chrome keeps what makes it look as it does on an SDR display (`ui::in_linear_light`),
/// so from the middle of Frost up the text reads there too, and Clear stays clear. Laid in linear
/// light as it is, the middle of Night's slider wouldn't read over a white window.
#[test]
fn windows_glass_reads_in_linear_light_too() {
    use trek_app::ui::{Material, chrome_alpha, glass_quiet_text, in_linear_light};
    use trek_core::settings::ThemeChoice;
    let glass = Material::SeeThrough { linear: true };
    run(async |cx| {
        let _trek = open(cx);
        for choice in [ThemeChoice::Night, ThemeChoice::Paper] {
            cx.update(|cx| trek_app::apply_theme(choice, None, cx));
            let (side, fg, dark) = cx.update(|cx| {
                let t = cx.theme();
                (t.sidebar, rgb(t.foreground), t.mode.is_dark())
            });
            let quiet = rgb(glass_quiet_text(dark));
            let alpha = |tint: f32| in_linear_light(chrome_alpha(tint, glass, dark), side, dark);
            for step in 50..=100 {
                let tint = 0.2 + 0.0075 * step as f32;
                for behind in BEHIND {
                    let chrome = over_linear(rgb(side), alpha(tint), behind);
                    let (c_fg, c_quiet) = (contrast(fg, chrome), contrast(quiet, chrome));
                    assert!(
                        c_fg >= 7.,
                        "{choice:?} chrome at tint {tint:.2} over {behind:?}: text {c_fg:.1}:1"
                    );
                    assert!(
                        c_quiet >= 4.5,
                        "{choice:?} chrome at tint {tint:.2} over {behind:?}: quiet text {c_quiet:.1}:1"
                    );
                }
            }
            let shows = contrast(
                over_linear(rgb(side), alpha(0.2), [1.; 3]),
                over_linear(rgb(side), alpha(0.2), [0.; 3]),
            );
            assert!(shows >= 10., "{choice:?}: Clear shows {shows:.1}:1");
            if dark {
                let as_it_is = over_linear(rgb(side), chrome_alpha(0.575, glass, dark), [1.; 3]);
                assert!(contrast(quiet, as_it_is) < 4.5);
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
    // Windows 11 22H2 and later: a see-through window, which winlook blurs behind (but at Clear).
    assert_eq!(backdrop(true, false, true), W::Transparent);
    // Which Windows can: 22H2, and the Transparency effects switch on (Windows 10, or the switch
    // off, leaves the window opaque: glass is never asked for).
    assert!(!trek_app::winlook::glass_available(19045, true, false));
    assert!(trek_app::winlook::glass_available(22631, true, false));
    assert!(!trek_app::winlook::glass_available(22631, false, false));
}
