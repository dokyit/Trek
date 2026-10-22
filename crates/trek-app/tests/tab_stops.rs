//! Tabs in the editor: a tab shows as wide as its tab stop. DirectWrite leaves a tab out of the
//! glyphs it shapes, which showed tab-indented code (Go, Makefiles) with no indentation at all;
//! `with_tab_stops` (vendor/gpui-base) gives each one a glyph of its own. Windows only: CoreText
//! lays tabs out itself, and the Mac's lines are left as it shapes them.
#![cfg(windows)]

mod common;

use common::{open, run};

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
                let run = gpui_kit::TextRun {
                    len: text.len(),
                    font: font.clone(),
                    color: gpui_kit::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                window.text_system().shape_line(
                    text.to_string().into(),
                    gpui_kit::px(14.),
                    &[run],
                    None,
                )
            };
            let x = |line: &gpui_kit::ShapedLine, index: usize| f32::from(line.x_for_index(index));
            let tabbed = |window: &mut gpui_kit::Window, text: &str| {
                let line = shape(window, text);
                with_tab_stops(line, 4, &font, window)
            };

            // `a⇥b`: the tab runs to the next stop, column 4, so `b` sits where it does after
            // three spaces; `⇥⇥x` is eight columns in. (Windows' own shaping gives a tab no width.)
            let a = tabbed(window, "a\tb");
            let spaces = shape(window, "a   b");
            assert!(
                near(x(&a, 2), x(&spaces, 4)),
                "b at {} where three spaces put it at {}",
                x(&a, 2),
                x(&spaces, 4)
            );
            assert!(
                near(f32::from(a.width), f32::from(spaces.width)),
                "the line's width follows"
            );
            let two = tabbed(window, "\t\tx");
            let eight = shape(window, "        x");
            assert!(
                near(x(&two, 2), x(&eight, 8)),
                "x at {} where eight spaces put it at {}",
                x(&two, 2),
                x(&eight, 8)
            );
            // The tab has a place of its own: it starts after `a` and ends where `b` starts, so a
            // caret before it and a click on it land there, not after it.
            assert!(
                near(x(&a, 1), x(&spaces, 1)),
                "the tab starts at {} after the `a`",
                x(&a, 1)
            );
            assert!(near(x(&a, 2) - x(&a, 1), x(&spaces, 4) - x(&spaces, 1)));
            assert_eq!(
                a.index_for_x(gpui_kit::px(x(&a, 1) + 2.)),
                Some(1),
                "a click inside the tab is on it"
            );
            assert_eq!(a.index_for_x(gpui_kit::px(x(&a, 2) + 2.)), Some(2));
            // A tab at the end of a line still takes its width.
            let trailing = tabbed(window, "ab\t");
            assert!(
                near(f32::from(trailing.width), x(&shape(window, "ab  "), 4)),
                "ab⇥ ends at column 4"
            );

            // Taken once: the line is as it was for the glyphs after the tab.
            let again = with_tab_stops(a.clone(), 4, &font, window);
            assert!(
                near(x(&again, 2), x(&a, 2)) && near(f32::from(again.width), f32::from(a.width))
            );
            // No tab, nothing touched.
            let plain = shape(window, "abc");
            let same = with_tab_stops(plain.clone(), 4, &font, window);
            assert!(near(f32::from(same.width), f32::from(plain.width)));
            // The tab size is the editor's: two columns for a tab size of two.
            let narrow = with_tab_stops(shape(window, "a\tb"), 2, &font, window);
            assert!(
                near(x(&narrow, 2), x(&shape(window, "a b"), 2)),
                "column 2 is one space past `a`"
            );
        });
    });
}

/// The blank that stands for a tab is a glyph of the line's font. After a character the font
/// hasn't got (CJK, an emoji: drawn from a fallback font, in a run of its own) it goes in a run of
/// the line's font, not the fallback's, where the same glyph id is some other glyph that would
/// draw; and the columns are the line font's, not the fallback's.
#[test]
fn a_tab_after_a_character_from_a_fallback_font_is_blank_in_the_lines_own_font() {
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
            let space = shape(window, " ");
            let own = space.runs[0].font_id;
            let column = f32::from(space.width);
            for text in ["中\tx", "x中\ty", "😀\tx"] {
                let line = shape(window, text);
                let tab = text.find('\t').unwrap();
                let before = text[..tab].char_indices().last().unwrap().0;
                assert_ne!(line.font_id_for_index(before), Some(own), "{text:?}: the character before the tab comes from another font here");
                let tabbed = with_tab_stops(line, 4, &font, window);
                assert_eq!(tabbed.font_id_for_index(tab), Some(own), "{text:?}: the tab's blank is the line font's");
                let after = f32::from(tabbed.x_for_index(tab + 1));
                let columns = after / column;
                assert!((columns - columns.round()).abs() < 0.05 && columns.round() >= 4., "{text:?}: what follows the tab is at column {columns}, a stop of the line's font");
                assert!(f32::from(tabbed.x_for_index(tab + 1)) > f32::from(tabbed.x_for_index(tab)), "{text:?}: the tab has width");
            }
        });
    });
}

/// In the editor itself: a selection over a tab is as wide as the tab is drawn (its rectangle
/// and the caret positions come from the same glyphs), and so is a selection across it.
#[test]
fn a_selection_over_a_tab_in_the_editor_is_as_wide_as_the_tab() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("main.go");
        std::fs::write(&file, "a\tb\n").unwrap();
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
        let editor = common::loaded(&trek, cx).await;
        trek.render(cx);
        let state = editor.read_with(cx, |e, _| e.text_state());
        let width = |range: std::ops::Range<usize>| {
            state
                .read_with(cx, |s, _| s.range_to_bounds(&range))
                .map(|b| f32::from(b.size.width))
                .expect("laid out")
        };
        let column = width(0..1);
        assert!(column > 0.);
        // `a⇥b` with tab size 4: the tab is columns 1 to 4.
        assert!(
            near(width(1..2), 3. * column),
            "the tab's selection is {} wide, three columns are {}",
            width(1..2),
            3. * column
        );
        assert!(near(width(2..3), column), "`b` is one column");
        assert!(
            near(width(0..3), 5. * column),
            "the whole line selected is {} wide, five columns are {}",
            width(0..3),
            5. * column
        );
        // Selected for real, the selection's ends are the same places.
        trek.window(cx, |_, cx| {
            state.update(cx, |s, cx| s.set_selected_range(1..2, cx))
        });
        trek.render(cx);
        let (start, end) = state.read_with(cx, |s, _| {
            let r = s.selected_range();
            (
                s.range_to_bounds(&(r.start..r.start)).unwrap().origin.x,
                s.range_to_bounds(&(r.end..r.end)).unwrap().origin.x,
            )
        });
        assert!(
            near(f32::from(end - start), 3. * column),
            "selected, the tab spans {}",
            f32::from(end - start)
        );
    });
}
