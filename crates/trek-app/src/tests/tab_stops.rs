//! Tabs in the editor: a tab shows as wide as its tab stop. DirectWrite leaves a tab out of the
//! glyphs it shapes, which showed tab-indented code (Go, Makefiles) with no indentation at all;
//! `with_tab_stops` (vendor/gpui-base) gives each one a glyph of its own.

use super::harness::{open, run};

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.6
}

#[test]
fn a_tab_is_as_wide_as_its_tab_stop_not_nothing() {
    use gpui_kit::base::input::with_tab_stops;
    use gpui_kit::component::ActiveTheme as _;
    run(async |cx| {
        let trek = open(cx);
        trek.window(cx, |window, cx| {
            let font = gpui_kit::font(cx.theme().mono_font_family.clone());
            let shape = |window: &mut gpui_kit::Window, text: &str| {
                let run = gpui_kit::TextRun { len: text.len(), font: font.clone(), color: gpui_kit::black(), background_color: None, underline: None, strikethrough: None };
                window.text_system().shape_line(text.to_string().into(), gpui_kit::px(14.), &[run], None)
            };
            let x = |line: &gpui_kit::ShapedLine, index: usize| f32::from(line.x_for_index(index));
            let tabbed = |window: &mut gpui_kit::Window, text: &str| {
                let line = shape(window, text);
                with_tab_stops(line, 4, window)
            };

            // `a⇥b`: the tab runs to the next stop, column 4, so `b` sits where it does after
            // three spaces; `⇥⇥x` is eight columns in. (Windows' own shaping gives a tab no width.)
            let a = tabbed(window, "a\tb");
            let spaces = shape(window, "a   b");
            assert!(near(x(&a, 2), x(&spaces, 4)), "b at {} where three spaces put it at {}", x(&a, 2), x(&spaces, 4));
            assert!(near(f32::from(a.width), f32::from(spaces.width)), "the line's width follows");
            let two = tabbed(window, "\t\tx");
            let eight = shape(window, "        x");
            assert!(near(x(&two, 2), x(&eight, 8)), "x at {} where eight spaces put it at {}", x(&two, 2), x(&eight, 8));
            // The tab has a place of its own: it starts after `a` and ends where `b` starts, so a
            // caret before it and a click on it land there, not after it.
            assert!(near(x(&a, 1), x(&spaces, 1)), "the tab starts at {} after the `a`", x(&a, 1));
            assert!(near(x(&a, 2) - x(&a, 1), x(&spaces, 4) - x(&spaces, 1)));
            assert_eq!(a.index_for_x(gpui_kit::px(x(&a, 1) + 2.)), Some(1), "a click inside the tab is on it");
            assert_eq!(a.index_for_x(gpui_kit::px(x(&a, 2) + 2.)), Some(2));
            // A tab at the end of a line still takes its width.
            let trailing = tabbed(window, "ab\t");
            assert!(near(f32::from(trailing.width), x(&shape(window, "ab  "), 4)), "ab⇥ ends at column 4");

            // Taken once: the line is as it was for the glyphs after the tab.
            let again = with_tab_stops(a.clone(), 4, window);
            assert!(near(x(&again, 2), x(&a, 2)) && near(f32::from(again.width), f32::from(a.width)));
            // No tab, nothing touched.
            let plain = shape(window, "abc");
            let same = with_tab_stops(plain.clone(), 4, window);
            assert!(near(f32::from(same.width), f32::from(plain.width)));
            // The tab size is the editor's: two columns for a tab size of two.
            let narrow = with_tab_stops(shape(window, "a\tb"), 2, window);
            assert!(near(x(&narrow, 2), x(&shape(window, "a b"), 2)), "column 2 is one space past `a`");
        });
    });
}

