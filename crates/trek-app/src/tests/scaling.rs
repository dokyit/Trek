//! Display scaling (100, 125, 150, 175, 200 %), Windows's colours (the app mode, Transparency
//! effects) and Reduce motion from the system: what Trek lays out and tells the system as the
//! window's scale or the system's switches change.
//!
//! The test window can be given any scale (`simulate_window_scale_factor_change`, which keeps its
//! logical size, as Windows's suggested rectangle does), so layout is held at all of them here;
//! the sizes Windows works out in device pixels (icons, the tray) are pure functions of the DPI
//! (`winsys`, `tray`), and window placement across displays of other scales is in `window_place`.

use super::harness::{Trek, open, open_with, run};
use crate::workspace::{Mode, Route, SettingsPage};
use gpui_kit::{TestAppContext, px};
use trek_core::settings::ThemeChoice;

/// 100 %, 125 %, 150 %, 175 % and 200 %.
const SCALES: [f32; 5] = [1., 1.25, 1.5, 1.75, 2.];

/// The test window's width (`harness::launch`).
const WIDTH: f32 = 1280.;

fn at_scale(trek: &Trek, cx: &mut TestAppContext, scale: f32) {
    cx.simulate_window_scale_factor_change(trek.window, scale);
    trek.render(cx);
    assert_eq!(trek.window(cx, |window, _| window.scale_factor()), scale);
}

#[test]
fn the_title_bar_controls_are_where_windows_hit_tests_them_at_every_scale() {
    run(async |cx| {
        let trek = open(cx);
        for route in [Route::Basecamp, Route::Settings(SettingsPage::Appearance), Route::Notes] {
            trek.update(cx, |ws, cx| ws.navigate(route.clone(), cx));
            for scale in SCALES {
                at_scale(&trek, cx, scale);
                // The window is the same size in logical pixels at every scale, so its controls
                // are too: GPUI turns the pointer into logical pixels (physical / scale) before
                // it asks which control it is over, and Windows's own hit test needs nothing else.
                let k = scale;
                let tools = trek.bounds(cx, "toggle-tools").unwrap_or_else(|| panic!("{route:?} at {scale}: the tools button is on screen"));
                if cfg!(windows) {
                    // `TitleBar`'s own caption buttons carry no test ids; Trek's copy of them,
                    // drawn over whatever covers the bar (the palette), is the same row.
                    trek.press(cx, "secondary-k");
                    let bar = crate::chrome::TITLE_BAR_HEIGHT_PX;
                    let mut left = WIDTH;
                    for id in ["caption-close", "caption-maximize", "caption-minimize"] {
                        let b = trek.bounds(cx, id).unwrap_or_else(|| panic!("{route:?} at {scale}: {id} is on screen"));
                        assert!((b.size.width.as_f32() - bar).abs() <= 1. / k + 0.01 && (b.size.height.as_f32() - bar).abs() <= 1. / k + 0.01, "{route:?} at {scale}: {id} is {:?}", b.size);
                        assert!((b.origin.y.as_f32()).abs() <= 1. / k + 0.01, "{route:?} at {scale}: {id} is at the window's top");
                        // Edge to edge, the right one at the window's edge: no gap a click would
                        // fall through into the drag area, no overlap.
                        assert!((b.right().as_f32() - left).abs() <= 1. / k + 0.01, "{route:?} at {scale}: {id} ends at {:?}, the next starts at {left}", b.right());
                        left = b.origin.x.as_f32();
                    }
                    assert!(tools.right().as_f32() <= left, "{route:?} at {scale}: Trek's own buttons end at {:?}, the window's controls start at {left}", tools.right());
                    // (Each button is cut to whole device pixels, up to half one short or long.)
                    assert!((left - (WIDTH - crate::chrome::RIGHT_RESERVE)).abs() <= 2. / k, "{route:?} at {scale}: three buttons {} wide start at {left}", crate::chrome::TITLE_BAR_HEIGHT_PX);
                    trek.press(cx, "escape");
                }
                // And what's on the bar isn't pushed past the window's edge at any scale.
                assert!(tools.right() <= px(WIDTH), "{route:?} at {scale}: {:?}", tools.right());
            }
        }
    });
}

#[test]
fn the_toast_sits_in_the_same_place_at_every_scale() {
    run(async |cx| {
        let trek = open(cx);
        trek.window(cx, |w, cx| crate::toast::push(w, "Saved", cx));
        let mut seen = vec![];
        for scale in SCALES {
            at_scale(&trek, cx, scale);
            let toast = trek.bounds(cx, "notification").unwrap_or_else(|| panic!("at {scale}: the toast is on screen"));
            let window = trek.window(cx, |w, _| w.viewport_size());
            assert!(toast.origin.x >= px(0.) && toast.right() <= window.width && toast.origin.y >= px(0.) && toast.bottom() <= window.height, "at {scale}: {toast:?} in {window:?}");
            // Centred, as at every scale.
            let centre = toast.center().x.as_f32();
            assert!((centre - window.width.as_f32() / 2.).abs() < 40., "at {scale}: centred at {centre}");
            seen.push((toast.origin.y.as_f32(), toast.size.height.as_f32()));
        }
        // Logical pixels are what it's placed in, so the answer doesn't depend on the scale.
        assert!(seen.windows(2).all(|w| (w[0].0 - w[1].0).abs() < 1. && (w[0].1 - w[1].1).abs() < 1.), "{seen:?}");
    });
}

#[test]
fn a_window_told_of_a_new_scale_cuts_its_icons_again_once() {
    run(async |cx| {
        let trek = open(cx);
        let changes = || crate::system::SCALE_CHANGES.with(|n| n.get());
        // The test window starts at 2.
        trek.render(cx);
        let start = changes();
        for (scale, expected) in [(2., 0), (1.25, 1), (1.25, 0), (1.5, 1), (1., 1), (2., 1)] {
            let before = changes();
            at_scale(&trek, cx, scale);
            assert_eq!(changes() - before, expected, "moved to {scale}");
        }
        assert_eq!(changes() - start, 4);
        // Moving or resizing at the same scale leaves them be.
        let before = changes();
        cx.simulate_window_resize(trek.window, gpui_kit::size(px(1100.), px(700.)));
        trek.render(cx);
        assert_eq!(changes(), before);
    });
}

/// Windows tells a window its dark title and Mica tone from the system's colours when they change,
/// which Trek's own theme (Paper chosen on a dark system) may not be: the window is told again.
#[test]
fn a_change_of_the_system_s_colours_tells_the_window_its_glass_again() {
    run(async |cx| {
        let trek = open(cx);
        trek.render(cx);
        let asks = || crate::ui::BACKDROP_ASKS.with(|n| n.get());
        let before = asks();
        trek.render(cx);
        assert_eq!(asks(), before, "told once, and left alone while nothing changes");
        cx.update(|cx| trek.root.update(cx, |root, cx| {
            crate::ui::colours_changed(&mut root.glass_applied);
            cx.notify();
        }));
        trek.render(cx);
        assert_eq!(asks(), before + 1, "told again, once");
        trek.render(cx);
        assert_eq!(asks(), before + 1);
    });
}

/// Trek follows the system's app mode (Windows: Settings › Personalization › Colors › Choose your
/// mode) while its theme is System: Night when dark, Paper when light, whichever it was.
#[test]
fn the_system_theme_is_night_in_the_dark_and_paper_in_the_light() {
    use gpui_kit::component::{ActiveTheme as _, Theme};
    use gpui_kit::WindowAppearance;
    run(async |cx| {
        let _trek = open(cx);
        cx.update(|cx| crate::apply_theme(ThemeChoice::System, None, cx));
        for (appearance, name, dark) in [(WindowAppearance::Dark, "Trek Night", true), (WindowAppearance::Light, "Trek Paper", false), (WindowAppearance::VibrantDark, "Trek Night", true), (WindowAppearance::VibrantLight, "Trek Paper", false)] {
            // What `Theme::sync_system_appearance` does with the window's appearance.
            cx.update(|cx| Theme::change(appearance, None, cx));
            let (shown, is_dark) = cx.update(|cx| (cx.theme().theme_name().to_string(), cx.theme().mode.is_dark()));
            assert_eq!((shown.as_str(), is_dark), (name, dark), "{appearance:?}");
        }
        // Chosen outright, it is what it was chosen as, whatever the system says.
        for (choice, name) in [(ThemeChoice::Night, "Trek Night"), (ThemeChoice::Paper, "Trek Paper")] {
            cx.update(|cx| crate::apply_theme(choice, None, cx));
            let shown = cx.update(|cx| cx.theme().theme_name().to_string());
            assert_eq!(shown, name);
        }
    });
}

#[test]
fn without_glass_to_show_the_window_stays_opaque() {
    run(async |cx| {
        let trek = open_with(cx, |s| {
            s.appearance.glass = true;
            s.appearance.glass_tint = 0.6;
        });
        // Windows 11 with Transparency effects on: Mica, at the tint.
        assert_eq!(trek.read(cx, |ws, _| ws.glass_unless(false)), Some(0.6));
        // Windows 10, or 11 with the switch off (`winlook::glass_available` says no): opaque.
        assert!(!crate::winlook::glass_available(19045, true) && !crate::winlook::glass_available(26100, false));
        assert_eq!(trek.read(cx, |ws, _| ws.glass_unless(true)), None);
        // And an opaque chrome is the theme's own sidebar colour, nothing let through.
        let (chrome, side) = cx.update(|cx| {
            use gpui_kit::component::ActiveTheme as _;
            (crate::ui::chrome_bg(None, cx), cx.theme().sidebar)
        });
        assert_eq!(chrome, side);
    });
}

#[test]
fn the_editor_and_the_other_screens_hold_together_at_every_scale() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        for scale in SCALES {
            at_scale(&trek, cx, scale);
            // The command pill is the fixed-height control in the editor's title bar: it keeps its
            // height at every scale, and sits whole on device pixels (`placement`).
            let pill = trek.bounds(cx, "command-center").unwrap_or_else(|| panic!("at {scale}: the command pill"));
            assert!((pill.size.height.as_f32() - 26.).abs() <= 1. / scale + 0.01, "at {scale}: {:?}", pill.size.height);
            assert!(pill.right() <= px(WIDTH) && pill.origin.x >= px(0.), "at {scale}: {pill:?}");
        }
    });
}
