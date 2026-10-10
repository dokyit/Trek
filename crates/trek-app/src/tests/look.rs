//! How Trek looks the same on both platforms: the fonts it ships, and that fixed-height controls
//! hold their text in them.

use super::harness::{open, run};
use crate::fonts::{MONO, UI};
use crate::workspace::Mode;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{Font, FontWeight, font, px};

#[test]
fn the_shipped_families_are_registered_and_the_theme_names_them() {
    run(async |cx| {
        let trek = open(cx);
        let names = cx.update(|cx| cx.text_system().all_font_names());
        assert!(names.iter().any(|n| n == UI), "{UI} is registered");
        assert!(names.iter().any(|n| n == MONO), "{MONO} is registered");
        // Both themes, so Paper isn't left on the system's fonts.
        for choice in [trek_core::settings::ThemeChoice::Night, trek_core::settings::ThemeChoice::Paper] {
            cx.update(|cx| crate::apply_theme(choice, None, cx));
            trek.render(cx);
            let (ui, mono) = cx.update(|cx| (cx.theme().font_family.to_string(), cx.theme().mono_font_family.to_string()));
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
        let pill = trek.bounds(cx, "command-center").expect("the command pill is on screen");
        assert_eq!(pill.size.height, px(26.), "the pill is 26 px tall at every font");
        // Inside its border, the tallest face the pill sets (the label is Inter at 12.5 px, the
        // folder's name medium weight) must fit from ascender to descender, on this platform's
        // text system.
        let inner = pill.size.height.as_f32() - 2.;
        cx.update(|cx| {
            let text = cx.text_system();
            for weight in [FontWeight::NORMAL, FontWeight::MEDIUM] {
                let id = text.resolve_font(&Font { weight, ..font(UI) });
                let natural = text.ascent(id, px(12.5)).as_f32() + text.descent(id, px(12.5)).abs().as_f32();
                assert!(natural <= inner, "{weight:?}: {natural} px of type in a {inner} px pill");
            }
        });
    });
}

/// WCAG relative luminance of an sRGB colour (0–1 channels).
fn luminance([r, g, b]: [f32; 3]) -> f32 {
    let lin = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

fn contrast(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `top` laid at `alpha` over `under`.
fn over(top: [f32; 3], alpha: f32, under: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| top[i] * alpha + under[i] * (1. - alpha))
}

fn rgb(c: gpui_kit::Hsla) -> [f32; 3] {
    let c = c.to_rgb();
    [c.r, c.g, c.b]
}

#[test]
fn text_stays_readable_on_mica_in_both_themes_at_every_tint() {
    use crate::ui::{Material, panel_alpha};
    use trek_core::settings::ThemeChoice;
    run(async |cx| {
        let _trek = open(cx);
        // Mica Alt's range under each theme's tone: the plain material and what a bold wallpaper
        // makes of it (the brightest dark Mica, the dimmest light one).
        for (choice, micas) in [(ThemeChoice::Night, [[0.06, 0.06, 0.06], [0.30, 0.30, 0.36]]), (ThemeChoice::Paper, [[0.95, 0.95, 0.95], [0.76, 0.78, 0.84]])] {
            cx.update(|cx| crate::apply_theme(choice, None, cx));
            let (bg, side, fg, muted) = cx.update(|cx| {
                let t = cx.theme();
                (t.background, t.sidebar, t.foreground, t.muted_foreground)
            });
            for mica in micas {
                for step in 0..=15 {
                    let tint = 0.2 + 0.05 * step as f32;
                    // An inset panel (the transcript): the text and the quieter text on it.
                    let panel = over(rgb(bg), panel_alpha(tint, Material::Mica), mica);
                    let (c_fg, c_muted) = (contrast(rgb(fg), panel), contrast(over(rgb(muted), muted.a, panel), panel));
                    assert!(c_fg >= 7., "{choice:?} panel at tint {tint:.2} over {mica:?}: text {c_fg:.1}:1");
                    assert!(c_muted >= 4.5, "{choice:?} panel at tint {tint:.2} over {mica:?}: quiet text {c_muted:.1}:1");
                    // The chrome (sidebar, title bar) is the sidebar's colour at the tint itself.
                    let chrome = over(rgb(side), crate::ui::chrome_alpha(tint, Material::Mica), mica);
                    let (c_fg, c_muted) = (contrast(rgb(fg), chrome), contrast(over(rgb(muted), muted.a, chrome), chrome));
                    assert!(c_fg >= 4.5, "{choice:?} chrome at tint {tint:.2} over {mica:?}: text {c_fg:.1}:1");
                    assert!(c_muted >= 3., "{choice:?} chrome at tint {tint:.2} over {mica:?}: quiet text {c_muted:.1}:1");
                }
            }
        }
    });
}

#[test]
fn glass_picks_its_material_by_platform_and_switch() {
    use crate::ui::backdrop;
    use gpui_kit::WindowBackgroundAppearance as W;
    // Off: opaque, whatever the platform has.
    assert_eq!(backdrop(false, false, true), W::Opaque);
    assert_eq!(backdrop(false, true, false), W::Opaque);
    // macOS 26: the system's glass laid under a transparent window; macOS before: GPUI's blur.
    assert_eq!(backdrop(true, true, false), W::Transparent);
    assert_eq!(backdrop(true, false, false), W::Blurred);
    // Windows 11 22H2 and later: Mica Alt.
    assert_eq!(backdrop(true, false, true), W::MicaAltBackdrop);
    // Which Windows can: Mica needs 22H2, and the Transparency effects switch on (Windows 10, or
    // the switch off, leaves the window opaque: glass is never asked for).
    assert!(!crate::winlook::glass_available(19045, true));
    assert!(crate::winlook::glass_available(22631, true));
    assert!(!crate::winlook::glass_available(22631, false));
}
