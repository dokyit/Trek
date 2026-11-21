//! Motion (`trek_app::motion`): springs that turn round where they are, rows that slide to new
//! places, and what goes away animating out before it unmounts. Time is the test scheduler's
//! clock, moved on by hand; without motion everything is where it's going at once.

mod common;

use common::{Trek, mock, open, open_with, run};
use gpui_kit::component::WindowExt as _;
use gpui_kit::{TestAppContext, px};
use std::path::PathBuf;
use std::time::Duration;
use trek_app::workspace::{Mode, Route, WorkspaceEvent};
use trek_core::store::now_ms;
use trek_core::{Effort, HandHolding, RunState};

/// Motion on (the harness starts with the system's Reduce motion set).
fn moving(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(false));
}

/// Move the clock on `ms` and draw a frame.
fn frame(trek: &Trek, cx: &mut TestAppContext, ms: u64) {
    cx.executor().advance_clock(Duration::from_millis(ms));
    trek.render(cx);
}

/// How far the sidebar is out: 1 out, 0 folded.
fn sidebar(trek: &Trek, cx: &mut TestAppContext) -> f32 {
    let now = cx.executor().now();
    cx.read(|cx| trek.root.read(cx).sidebar_motion.borrow().value(now))
}

/// How far the window has crossed to the editor: 0 Agents, 1 the editor.
fn editor_in(trek: &Trek, cx: &mut TestAppContext) -> f32 {
    let now = cx.executor().now();
    cx.read(|cx| trek.root.read(cx).mode_motion.value(now))
}

#[test]
fn a_second_toggle_turns_the_sidebar_round_where_it_is() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        trek.render(cx);
        assert_eq!(sidebar(&trek, cx), 1.);
        trek.press(cx, "secondary-b");
        frame(&trek, cx, 60);
        let folding = sidebar(&trek, cx);
        assert!(folding > 0.1 && folding < 0.9, "part-way: {folding}");
        // Drawn part-way out of the window's left edge, not gone.
        let x = trek
            .bounds(cx, "sidebar")
            .expect("still drawn while it folds")
            .origin
            .x;
        assert!(
            x < px(0.) && x > px(-trek_app::root::SIDEBAR_WIDTH),
            "{x:?}"
        );

        // ⌘B again, mid-fold: it turns round from where it is, at the speed it had.
        trek.press(cx, "secondary-b");
        trek.render(cx);
        let turned = sidebar(&trek, cx);
        assert!(
            (turned - folding).abs() < 0.01,
            "no jump at the turn: {folding} → {turned}"
        );
        frame(&trek, cx, 16);
        assert!(
            sidebar(&trek, cx) < turned,
            "still folding for a moment: it was moving that way"
        );
        let samples: Vec<f32> = (0..40)
            .map(|_| {
                frame(&trek, cx, 16);
                sidebar(&trek, cx)
            })
            .collect();
        assert!(
            samples.windows(2).all(|w| w[1] >= w[0] - 1e-4),
            "then it comes back out steadily: {samples:?}"
        );
        assert!(samples.iter().all(|v| *v <= 1.), "without overshooting");
        frame(&trek, cx, 1_000);
        assert_eq!(sidebar(&trek, cx), 1.);
        assert_eq!(trek.bounds(cx, "sidebar").map(|b| b.origin.x), Some(px(0.)));
    });
}

#[test]
fn switching_to_the_editor_crosses_over_and_turns_round() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        frame(&trek, cx, 50);
        let crossing = editor_in(&trek, cx);
        assert!(crossing > 0.05 && crossing < 0.95, "{crossing}");
        assert!(
            trek.visible(cx, "sidebar"),
            "Agents is still drawn, fading out under the editor"
        );

        trek.update(cx, |ws, cx| ws.set_mode(Mode::Agents, cx));
        trek.render(cx);
        assert!(
            (editor_in(&trek, cx) - crossing).abs() < 0.01,
            "back from where it was"
        );
        frame(&trek, cx, 1_000);
        assert_eq!(editor_in(&trek, cx), 0.);
        assert!(trek.visible(cx, "sidebar"));

        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        frame(&trek, cx, 1_000);
        assert_eq!(editor_in(&trek, cx), 1.);
        assert!(
            !trek.visible(cx, "sidebar"),
            "once across, only the editor is drawn"
        );
    });
}

/// What a page's loading line is in a window of its own, with the app's motion as it is.
struct Line(bool);

impl gpui_kit::Render for Line {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{ParentElement as _, Styled as _};
        gpui_kit::div()
            .relative()
            .size_full()
            .child(trek_app::panels::browser::loading_line(
                self.0,
                gpui_kit::hsla(0.05, 0.9, 0.5, 1.),
            ))
    }
}

#[test]
fn a_loading_line_with_reduced_motion_is_still_there_and_not_sliding() {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, point, size};
    run(async |cx| {
        let _trek = open(cx);
        moving(cx);
        let mut shown = vec![];
        for still in [false, true] {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(600.), px(100.)),
                })),
                ..Default::default()
            };
            let (window, _) = cx
                .update(|cx| gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Line(still))))
                .expect("window");
            frame_in(cx, window, 0);
            shown.push(in_window(cx, window, |w, _| {
                let (line, bar) = (
                    w.find("browser-loading").bounds(),
                    w.find("browser-loading-bar").bounds(),
                );
                (
                    line.size.width.as_f32(),
                    bar.size.width.as_f32(),
                    bar.origin.x.as_f32() - line.origin.x.as_f32(),
                )
            }));
        }
        // Sliding: a third of the line wide, starting off its left end. Still: all of it, in place.
        let ((line, moving, at), (_, still, at_still)) = (shown[0], shown[1]);
        assert!((moving - 0.32 * line).abs() < 2. && at < 0., "{shown:?}");
        assert!(
            (still - line).abs() < 1. && at_still.abs() < 1.,
            "{shown:?}"
        );
    });
}

/// Windows's Animation effects switch off (`winlook::reduces_motion(false)`) is Reduce motion:
/// with the read injected, as the clock is, it stills what Trek's own setting does, and puts
/// motion back when the switch does.
#[test]
fn the_systems_animation_effects_off_stills_everything_the_setting_does() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        trek.render(cx);
        assert!(trek.read(cx, |ws, cx| ws.motion(cx)));
        let system_off = |cx: &mut TestAppContext, animation_effects: bool| {
            cx.update(|cx| {
                trek_app::system::reduce_motion_as(
                    trek_app::winlook::reduces_motion(animation_effects),
                    &trek.ws,
                    cx,
                )
            })
        };

        system_off(cx, false);
        assert!(
            !trek.read(cx, |ws, cx| ws.motion(cx)),
            "the system's switch alone is enough"
        );
        assert!(
            cx.update(|cx| cx.reduce_motion()),
            "and GPUI's and the components' own animations follow it"
        );
        trek.press(cx, "secondary-b");
        assert_eq!(sidebar(&trek, cx), 0., "folded at once");
        trek.press(cx, "secondary-b");
        assert_eq!(sidebar(&trek, cx), 1., "and out at once");
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        trek.render(cx);
        assert_eq!(editor_in(&trek, cx), 1.);
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Agents, cx));
        trek.render(cx);
        trek.press(cx, "secondary-k");
        assert!(trek.visible(cx, "palette"));
        trek.press(cx, "escape");
        assert!(!trek.visible(cx, "palette"), "gone at once");

        // Animation effects back on: motion is back.
        system_off(cx, true);
        assert!(trek.read(cx, |ws, cx| ws.motion(cx)));
        assert!(!cx.update(|cx| cx.reduce_motion()));
        trek.press(cx, "secondary-b");
        frame(&trek, cx, 60);
        let folding = sidebar(&trek, cx);
        assert!(folding > 0.1 && folding < 0.9, "part-way again: {folding}");
        frame(&trek, cx, 1_000);

        // Trek's own setting holds it still with the system's switch on; the two don't undo each other.
        trek.update(cx, |ws, _| ws.settings.appearance.reduce_motion = true);
        system_off(cx, true);
        assert!(!trek.read(cx, |ws, cx| ws.motion(cx)) && cx.update(|cx| cx.reduce_motion()));
        system_off(cx, false);
        assert!(!trek.read(cx, |ws, cx| ws.motion(cx)));
    });
}

#[test]
fn reduce_motion_puts_everything_where_it_goes_at_once() {
    run(async |cx| {
        // Trek's own setting, with the system's off.
        let trek = open_with(cx, |s| s.appearance.reduce_motion = true);
        moving(cx);
        trek.press(cx, "secondary-b");
        assert_eq!(sidebar(&trek, cx), 0.);
        assert!(!trek.visible(cx, "sidebar"));
        trek.press(cx, "secondary-b");
        assert!(trek.visible(cx, "sidebar"));

        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        trek.render(cx);
        assert_eq!(editor_in(&trek, cx), 1.);
        assert!(!trek.visible(cx, "sidebar"));
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Agents, cx));
        trek.render(cx);

        trek.press(cx, "secondary-k");
        assert!(trek.visible(cx, "palette"));
        trek.press(cx, "escape");
        assert!(
            !trek.visible(cx, "palette"),
            "gone at once, nothing left to fade"
        );

        // A row that moves takes its new place at once.
        let ids: Vec<String> = (0..3)
            .map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i + 1) * 60_000))
            .collect();
        trek.render(cx);
        bump(&trek, cx, &ids[2]);
        trek.render(cx);
        assert_eq!(offset(&trek, cx, &ids[2]), Some(0.));
        trek.update(cx, |ws, cx| ws.archive(&ids[1], cx));
        trek.render(cx);
        assert!(ghosts(&trek, cx).is_empty() && !trek.visible(cx, format!("live-line-{}", ids[1])));
    });
}

/// A seen, unsettled thread in the harness project, `older_by` ms old.
fn quiet(trek: &Trek, cx: &mut TestAppContext, title: &str, older_by: i64) -> String {
    trek.update(cx, |ws, cx| {
        let mut t = ws
            .store
            .create_thread(
                Some(&trek.project),
                mock(),
                None,
                Effort::Medium,
                HandHolding::Auto,
            )
            .expect("thread");
        t.title = title.into();
        t.updated_at = now_ms() - older_by;
        t.last_seen_at = t.updated_at;
        ws.store.save_thread(&t).expect("save");
        ws.reload(cx);
        t.id
    })
}

/// News for `id` (read already): it's the newest, so it goes to the top of its group.
fn bump(trek: &Trek, cx: &mut TestAppContext, id: &str) {
    trek.update(cx, |ws, cx| {
        ws.store
            .update_thread(id, |t| {
                t.updated_at = now_ms();
                t.last_seen_at = t.updated_at;
            })
            .unwrap();
        ws.reload(cx);
    });
}

fn row_y(trek: &Trek, cx: &mut TestAppContext, id: &str) -> Option<f32> {
    trek.bounds(cx, format!("inbox-{id}"))
        .or_else(|| trek.bounds(cx, format!("live-line-{id}")))
        .map(|b| b.origin.y.as_f32())
}

fn offset(trek: &Trek, cx: &mut TestAppContext, id: &str) -> Option<f32> {
    let sidebar = cx.read(|cx| trek.root.read(cx).sidebar.clone());
    sidebar.read_with(cx, |s, cx| s.row_offset(id, cx))
}

fn ghosts(trek: &Trek, cx: &mut TestAppContext) -> Vec<String> {
    let sidebar = cx.read(|cx| trek.root.read(cx).sidebar.clone());
    sidebar.read_with(cx, |s, _| s.ghosts())
}

#[test]
fn rows_slide_from_where_they_were_to_where_they_go() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        let ids: Vec<String> = (0..3)
            .map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i + 1) * 60_000))
            .collect();
        trek.render(cx);
        frame(&trek, cx, 1_000);
        let before: Vec<f32> = ids
            .iter()
            .map(|id| row_y(&trek, cx, id).expect("a row"))
            .collect();
        assert!(
            before.windows(2).all(|w| w[0] < w[1]),
            "newest first: {before:?}"
        );

        // The oldest gets news and goes to the top. The first frame draws every row where it
        // was (the same row, under its own id), each sprung toward its new place.
        bump(&trek, cx, &ids[2]);
        trek.render(cx);
        assert!(
            trek.visible(cx, format!("live-line-{}", ids[2])),
            "the row that moved is the same row"
        );
        let first: Vec<f32> = ids.iter().map(|id| row_y(&trek, cx, id).unwrap()).collect();
        for (b, f) in before.iter().zip(&first) {
            assert!(
                (b - f).abs() < 1.,
                "drawn where it was: {before:?} → {first:?}"
            );
        }
        assert!(
            offset(&trek, cx, &ids[2]).unwrap() > 20.,
            "the risen row is a long way below its place"
        );
        assert!(
            offset(&trek, cx, &ids[0]).unwrap() < 0.,
            "the others are above theirs"
        );

        frame(&trek, cx, 80);
        let mid = row_y(&trek, cx, &ids[2]).unwrap();
        assert!(mid < before[2] && mid > before[0], "on its way up: {mid}");

        frame(&trek, cx, 1_000);
        let after: Vec<f32> = ids.iter().map(|id| row_y(&trek, cx, id).unwrap()).collect();
        assert_eq!(after[2], before[0], "at the top");
        assert_eq!(
            (after[0], after[1]),
            (before[1], before[2]),
            "the rest one down"
        );
        assert!(ids.iter().all(|id| offset(&trek, cx, id) == Some(0.)));
    });
}

#[test]
fn a_new_row_settles_in_and_an_archived_one_folds_away() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        let ids: Vec<String> = (0..3)
            .map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i + 1) * 60_000))
            .collect();
        trek.render(cx);
        frame(&trek, cx, 1_000);
        let before: Vec<f32> = ids.iter().map(|id| row_y(&trek, cx, id).unwrap()).collect();

        // Archived: its row stays where it was while it folds away; the ones under it close up.
        trek.update(cx, |ws, cx| ws.archive(&ids[1], cx));
        trek.render(cx);
        assert_eq!(ghosts(&trek, cx), [ids[1].clone()]);
        assert_eq!(row_y(&trek, cx, &ids[1]), Some(before[1]));
        frame(&trek, cx, 60);
        let closing = row_y(&trek, cx, &ids[2]).unwrap();
        assert!(
            closing < before[2] && closing > before[1],
            "the row under it closes the gap: {closing}"
        );
        assert_eq!(
            row_y(&trek, cx, &ids[1]),
            Some(before[1]),
            "fading where it was"
        );
        frame(&trek, cx, 1_000);
        assert!(ghosts(&trek, cx).is_empty());
        assert!(
            !trek.visible(cx, format!("live-line-{}", ids[1])),
            "and it's gone"
        );
        assert_eq!(row_y(&trek, cx, &ids[2]), Some(before[1]));

        // A new thread comes in from a little above its place.
        let new = quiet(&trek, cx, "New", 1_000);
        trek.render(cx);
        assert!(offset(&trek, cx, &new).unwrap() < 0.);
        frame(&trek, cx, 1_000);
        assert_eq!(offset(&trek, cx, &new), Some(0.));
        assert_eq!(row_y(&trek, cx, &new), Some(before[0]));
    });
}

/// A glyph's (or an icon's) box: left, top, width, height.
type Glyph = (f32, f32, f32, f32);

/// The text painted over `b` in the last frame: each glyph's (or icon's) box, as painted (inside
/// its mask), against `b`'s top left.
fn text_over(
    trek: &Trek,
    cx: &mut TestAppContext,
    b: gpui_kit::Bounds<gpui_kit::Pixels>,
) -> Vec<Glyph> {
    trek.window(cx, |window, _| {
        let k = window.scale_factor();
        let (x0, y0, x1, y1) = (
            b.left().as_f32(),
            b.top().as_f32(),
            b.right().as_f32(),
            b.bottom().as_f32(),
        );
        window
            .painted_monochrome_sprites()
            .iter()
            .filter_map(|s| {
                let (m, g) = (s.content_mask.bounds, s.bounds);
                let left = g.origin.x.0.max(m.origin.x.0) / k;
                let top = g.origin.y.0.max(m.origin.y.0) / k;
                let right = g.right().0.min(m.right().0) / k;
                let bottom = g.bottom().0.min(m.bottom().0) / k;
                // Painted, and over `b` by more than a hair.
                let over = right.min(x1) - left.max(x0) > 0.5 && bottom.min(y1) - top.max(y0) > 0.5;
                (right > left && bottom > top && over).then(|| {
                    (
                        g.origin.x.0 / k - x0,
                        g.origin.y.0 / k - y0,
                        g.size.width.0 / k,
                        g.size.height.0 / k,
                    )
                })
            })
            .collect()
    })
}

/// `id` asks for an approval, or finished unread.
fn set_state(trek: &Trek, cx: &mut TestAppContext, id: &str, state: RunState, unread: bool) {
    trek.update(cx, |ws, cx| {
        ws.store
            .update_thread(id, |t| {
                t.run_state = state;
                if unread {
                    t.last_seen_at = t.updated_at - 1;
                }
            })
            .unwrap();
        ws.reload(cx);
    });
}

#[test]
fn a_thread_rising_to_the_inbox_is_drawn_over_the_rows_it_passes_not_through_them() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        trek.update(cx, |ws, _| ws.settings.appearance.glass = false);
        // Three finished in the Inbox; the one at work and two quiet ones in the project.
        let done: Vec<String> = [
            "Explain the cart code",
            "Check the page for typos",
            "Write a tagline",
        ]
        .iter()
        .enumerate()
        .map(|(i, title)| quiet(&trek, cx, title, (i as i64 + 1) * 60_000))
        .collect();
        for id in &done {
            set_state(&trek, cx, id, RunState::Idle, true);
        }
        let quiet_ones: Vec<String> = (0..2)
            .map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i + 5) * 60_000))
            .collect();
        let hero = quiet(&trek, cx, "Fix the checkout total", 4 * 60_000);
        set_state(&trek, cx, &hero, RunState::Working, false);
        trek.render(cx);
        frame(&trek, cx, 1_000);
        assert!(
            trek.read(cx, |ws, _| !ws.see_through()),
            "an opaque sidebar"
        );
        assert!(
            trek.visible(cx, format!("live-line-{hero}")),
            "at work in its project"
        );
        let pid = trek.read(cx, |ws, _| {
            ws.thread(&hero).unwrap().project_id.clone().unwrap()
        });
        let others: Vec<String> = done
            .iter()
            .map(|id| format!("inbox-{id}"))
            .chain(quiet_ones.iter().map(|id| format!("live-line-{id}")))
            .chain([
                format!("live-proj-head-{pid}"),
                "section-projects".to_string(),
            ])
            .collect();

        // It asks for an approval: up to the top of the Inbox, past the rows in between.
        set_state(&trek, cx, &hero, RunState::NeedsYou, false);
        trek.render(cx);
        let mut passing: Vec<(gpui_kit::Bounds<gpui_kit::Pixels>, Vec<Glyph>)> = vec![];
        for _ in 0..40 {
            frame(&trek, cx, 8);
            let Some(row) = trek.bounds(cx, format!("inbox-{hero}")) else {
                continue;
            };
            if offset(&trek, cx, &hero) == Some(0.) {
                break;
            }
            // Over another row's place this frame: they'd overprint if both were painted.
            let over_another = others.iter().any(|o| {
                trek.bounds(cx, o.clone()).is_some_and(|b| {
                    b.top() < row.bottom() - gpui_kit::px(2.)
                        && b.bottom() > row.top() + gpui_kit::px(2.)
                })
            });
            if !over_another {
                continue;
            }
            // It carries the Inbox's surface, opaque, under its own text.
            let k = trek.window(cx, |w, _| w.scale_factor());
            let filled = trek.window(cx, |w, _| {
                w.painted_quads().iter().any(|q| {
                    q.background.as_solid().is_some_and(|c| c.a >= 0.999)
                        && (q.bounds.origin.y.0 / k - row.top().as_f32()).abs() < 1.
                        && (q.bounds.size.height.0 / k - row.size.height.as_f32()).abs() < 1.
                        && q.bounds.origin.x.0 / k <= row.left().as_f32() + 0.5
                        && q.bounds.right().0 / k >= row.right().as_f32() - 0.5
                })
            });
            assert!(filled, "an opaque fill under the rising row at {row:?}");
            passing.push((row, text_over(&trek, cx, row)));
        }
        assert!(
            passing.len() >= 2,
            "it passed other rows for a few frames: {}",
            passing.len()
        );

        // At rest, the text in its box is its own: mid-flight, nothing else was painted there.
        frame(&trek, cx, 1_000);
        let rest = trek
            .bounds(cx, format!("inbox-{hero}"))
            .expect("at the top of the Inbox");
        assert!(rest.top() < trek.bounds(cx, format!("inbox-{}", done[0])).unwrap().top());
        let own = text_over(&trek, cx, rest);
        assert!(!own.is_empty(), "its title and why are painted");
        let near = |a: &Glyph, b: &Glyph| {
            (a.0 - b.0).abs() < 1.
                && (a.1 - b.1).abs() < 1.
                && (a.2 - b.2).abs() < 1.
                && (a.3 - b.3).abs() < 1.
        };
        for (row, text) in &passing {
            for glyph in text {
                assert!(
                    own.iter().any(|g| near(g, glyph)),
                    "another row's text painted inside the rising row at {row:?}: {glyph:?}"
                );
            }
        }
    });
}

/// A box: left, top, right, bottom (window px).
type Area = (f32, f32, f32, f32);

/// A band down the sidebar: top, bottom (window px).
type Span = (f32, f32);

/// Something done to the window between frames.
type Step<'a> = &'a dyn Fn(&Trek, &mut TestAppContext);

/// What one frame of the sidebar painted: each glyph's (or icon's) box as painted (inside its
/// mask), each row's band as drawn and as painted (`Sidebar::painted_bands`), and the horizontal
/// edges of the wide bordered boxes (the Inbox's card) as painted.
struct Painted {
    glyphs: Vec<Area>,
    rows: Vec<(String, Span, Span)>,
    edges: Vec<Area>,
}

fn painted(trek: &Trek, cx: &mut TestAppContext) -> Painted {
    let side = trek.bounds(cx, "sidebar").expect("the sidebar");
    let (left, right) = (side.left().as_f32(), side.right().as_f32());
    let sidebar = cx.read(|cx| trek.root.read(cx).sidebar.clone());
    let rows = sidebar.read_with(cx, |s, _| s.painted_bands());
    let (glyphs, edges) = trek.window(cx, |window, _| {
        let k = window.scale_factor();
        let visible = |b: gpui_kit::Bounds<gpui_kit::ScaledPixels>,
                       m: gpui_kit::Bounds<gpui_kit::ScaledPixels>|
         -> Option<Area> {
            let a = (
                b.origin.x.0.max(m.origin.x.0) / k,
                b.origin.y.0.max(m.origin.y.0) / k,
                b.right().0.min(m.right().0) / k,
                b.bottom().0.min(m.bottom().0) / k,
            );
            (a.2 > a.0 && a.3 > a.1 && a.0 >= left - 0.5 && a.2 <= right + 0.5).then_some(a)
        };
        let glyphs = window
            .painted_monochrome_sprites()
            .iter()
            .filter_map(|s| visible(s.bounds, s.content_mask.bounds))
            .collect();
        let mut edges = vec![];
        for q in window.painted_quads() {
            let (b, w) = (q.bounds, q.border_widths);
            if q.border_color.a == 0. || b.size.width.0 / k < (right - left) * 0.8 {
                continue;
            }
            for (y0, y1) in [
                (b.origin.y, b.origin.y + w.top),
                (b.bottom() - w.bottom, b.bottom()),
            ] {
                if y1 > y0 {
                    let line = gpui_kit::Bounds::from_corners(
                        gpui_kit::point(b.origin.x, y0),
                        gpui_kit::point(b.right(), y1),
                    );
                    edges.extend(visible(line, q.content_mask.bounds));
                }
            }
        }
        (glyphs, edges)
    });
    Painted {
        glyphs,
        rows,
        edges,
    }
}

/// What's wrong with a frame: two rows painted over the same pixels, a glyph painted across rows
/// (outside the band its row may paint), or the card's edge painted through a row.
fn overprint(p: &Painted) -> Option<String> {
    for (i, (ka, _, (a0, a1))) in p.rows.iter().enumerate() {
        for (kb, _, (b0, b1)) in &p.rows[i + 1..] {
            if a1.min(*b1) - a0.max(*b0) > 0.5 {
                return Some(format!(
                    "{ka} ({a0}..{a1}) and {kb} ({b0}..{b1}) painted over the same pixels"
                ));
            }
        }
    }
    for g in &p.glyphs {
        let across: Vec<&String> = p
            .rows
            .iter()
            .filter(|(_, _, (t, b))| g.3.min(*b) - g.1.max(*t) > 0.5)
            .map(|(k, _, _)| k)
            .collect();
        if across.len() > 1 {
            return Some(format!("a glyph {g:?} painted across rows {across:?}"));
        }
    }
    for e in &p.edges {
        for (k, _, (t, b)) in &p.rows {
            if e.3.min(*b) - e.1.max(*t) > 0.25 {
                return Some(format!(
                    "the card's edge {e:?} painted through {k} ({t}..{b})"
                ));
            }
        }
    }
    None
}

/// Rows drawn over each other this frame (before they're cut back): the frame tests something.
fn passing(p: &Painted) -> bool {
    p.rows.iter().enumerate().any(|(i, (_, (a0, a1), _))| {
        p.rows[i + 1..]
            .iter()
            .any(|(_, (b0, b1), _)| a1.min(*b1) - a0.max(*b0) > 2.)
    })
}

/// Draw frames 8 ms apart until nothing in the sidebar moves, checking each; `mid` runs after the
/// third (a fold mid-move). `riser`, rising past the others, is never cut by any of them. How many
/// frames had rows drawn over each other.
fn step_clean(
    trek: &Trek,
    cx: &mut TestAppContext,
    what: &str,
    riser: Option<&str>,
    mid: Option<Step>,
) -> usize {
    trek.render(cx);
    let mut passed = 0;
    for i in 0..90 {
        frame(trek, cx, 8);
        let p = painted(trek, cx);
        if let Some(wrong) = overprint(&p) {
            panic!("{what}, frame {i}: {wrong}");
        }
        if let Some((_, drawn, cut)) = riser.and_then(|r| p.rows.iter().find(|(k, _, _)| k == r)) {
            assert_eq!(
                drawn, cut,
                "{what}, frame {i}: the rising row is painted whole"
            );
        }
        passed += usize::from(passing(&p));
        if i == 2
            && let Some(mid) = mid
        {
            mid(trek, cx);
        }
    }
    passed
}

/// Every kind of reorder, frame by frame, in `look`: no row's text is ever painted over another's.
fn reorders_paint_clean(theme: trek_core::settings::ThemeChoice, glass: bool) {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        cx.update(|cx| trek_app::apply_theme(theme, None, cx));
        trek.update(cx, |ws, cx| {
            ws.settings.appearance.glass = glass;
            cx.notify();
        });
        assert_eq!(trek.read(cx, |ws, _| ws.see_through()), glass);
        let done: Vec<String> = ["Explain the cart code", "Check the page for typos"]
            .iter()
            .enumerate()
            .map(|(i, title)| quiet(&trek, cx, title, (i as i64 + 1) * 60_000))
            .collect();
        for id in &done {
            set_state(&trek, cx, id, RunState::Idle, true);
        }
        let quiet_ones: Vec<String> = (0..3)
            .map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i + 8) * 60_000))
            .collect();
        let heroes: Vec<String> = ["Fix the checkout total", "Tidy the styles"]
            .iter()
            .enumerate()
            .map(|(i, title)| quiet(&trek, cx, title, (i as i64 + 4) * 60_000))
            .collect();
        for id in &heroes {
            set_state(&trek, cx, id, RunState::Working, false);
        }
        let other = elsewhere(&trek, cx, "Elsewhere", 20 * 60_000);
        trek.render(cx);
        frame(&trek, cx, 1_000);
        let at_rest = painted(&trek, cx);
        assert!(at_rest.glyphs.len() > 20 && at_rest.rows.len() > 8);
        assert_eq!(overprint(&at_rest), None, "at rest");
        let pid = pid(&trek, cx, &heroes[0]);
        let fold = move |trek: &Trek, cx: &mut TestAppContext| {
            trek.click(cx, format!("live-proj-head-{pid}"));
        };
        let mut passed = 0;

        // One rises to the top of the Inbox past the rows between; they're pushed down, the
        // project's close the gap.
        set_state(&trek, cx, &heroes[0], RunState::NeedsYou, false);
        passed += step_clean(&trek, cx, "a row rising", Some(&heroes[0]), None);
        // Answered, it goes back down to its project.
        set_state(&trek, cx, &heroes[0], RunState::Working, false);
        passed += step_clean(&trek, cx, "a row leaving the Inbox", None, None);
        // Two start to wait in the same frame.
        trek.update(cx, |ws, cx| {
            for id in &heroes {
                ws.store
                    .update_thread(id, |t| t.run_state = RunState::NeedsYou)
                    .unwrap();
            }
            ws.reload(cx);
        });
        passed += step_clean(&trek, cx, "two rising at once", None, None);
        // Both answered; the project folds as they go down to it, then unfolds as one rises.
        trek.update(cx, |ws, cx| {
            for id in &heroes {
                ws.store
                    .update_thread(id, |t| t.run_state = RunState::Working)
                    .unwrap();
            }
            ws.reload(cx);
        });
        passed += step_clean(&trek, cx, "folding mid-move", None, Some(&fold));
        set_state(&trek, cx, &heroes[1], RunState::NeedsYou, false);
        passed += step_clean(&trek, cx, "unfolding mid-move", None, Some(&fold));
        // A quiet one finishes unread: up into the Inbox's news, under what calls for the user.
        set_state(&trek, cx, &quiet_ones[2], RunState::Idle, true);
        passed += step_clean(&trek, cx, "a row joining the news", None, None);
        // The other project's thread is archived: it fades where it was as the rest close up.
        trek.update(cx, |ws, cx| ws.archive(&other, cx));
        passed += step_clean(&trek, cx, "a row fading away", None, None);
        // A thread just started (its row still fading in) that asks at once: it rises whole.
        let newcomer = quiet(&trek, cx, "Newcomer", 1_000);
        set_state(&trek, cx, &newcomer, RunState::Working, false);
        frame(&trek, cx, 8);
        set_state(&trek, cx, &newcomer, RunState::NeedsYou, false);
        passed += step_clean(&trek, cx, "a new row rising", Some(&newcomer), None);
        assert!(
            passed >= 10,
            "rows were drawn over each other mid-move in a few frames: {passed}"
        );
    });
}

#[test]
fn every_reorder_paints_each_row_clear_of_the_others_in_night() {
    reorders_paint_clean(trek_core::settings::ThemeChoice::Night, false);
}

#[test]
fn every_reorder_paints_each_row_clear_of_the_others_in_paper() {
    reorders_paint_clean(trek_core::settings::ThemeChoice::Paper, false);
}

#[test]
fn every_reorder_paints_each_row_clear_of_the_others_on_glass() {
    reorders_paint_clean(trek_core::settings::ThemeChoice::Night, true);
}

#[test]
fn every_reorder_paints_each_row_clear_of_the_others_on_glass_in_paper() {
    reorders_paint_clean(trek_core::settings::ThemeChoice::Paper, true);
}

#[test]
fn the_palette_stays_while_it_fades_then_goes() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        trek.press(cx, "secondary-k");
        frame(&trek, cx, 1_000);
        assert!(trek.visible(cx, "palette"));
        trek.press(cx, "escape");
        trek.render(cx);
        assert!(!cx.read(|cx| trek.root.read(cx).palette.read(cx).open));
        assert!(trek.visible(cx, "palette"), "still drawn as it leaves");
        frame(&trek, cx, 1_000);
        assert!(!trek.visible(cx, "palette"), "unmounted once it has gone");
    });
}

#[test]
fn a_toast_stays_mounted_through_its_exit_then_unmounts() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        cx.update(|cx| trek_app::root::init(trek.ws.clone(), cx));
        trek.window(cx, |window, _| window.activate_window());
        cx.run_until_parked();
        let toasts = |trek: &Trek, cx: &mut TestAppContext| {
            trek.window(cx, |window, cx| trek_app::toast::count(window, cx))
        };
        trek.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::Toast {
                message: "Saved".into(),
                undo: None,
            })
        });
        trek.render(cx);
        assert_eq!(toasts(&trek, cx), 1);
        // Its time is up: it starts to go, and stays mounted while it does.
        frame(
            &trek,
            cx,
            trek_app::root::toast_lifetime("Saved", false).as_millis() as u64 + 10,
        );
        assert_eq!(toasts(&trek, cx), 1, "still there, on its way out");
        frame(&trek, cx, 400);
        assert_eq!(toasts(&trek, cx), 0, "gone once it has left");
    });
}

/// A `w`×`h` PNG in the project.
fn png(trek: &Trek, name: &str, w: u32, h: u32) -> PathBuf {
    let path = trek.project.join(name);
    image::RgbaImage::from_pixel(w, h, image::Rgba([90, 140, 200, 255]))
        .save(&path)
        .expect("png");
    path
}

#[test]
fn the_preview_grows_out_of_its_thumbnail_and_shrinks_back_into_it() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        let image = png(&trek, "shot.png", 1600, 1000);
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        composer.update(cx, |c, cx| c.attach_image(image.clone(), cx));
        trek.render(cx);
        let thumb = trek
            .bounds(cx, ("attachment", 0usize))
            .expect("a thumbnail");
        let preview = cx.read(|cx| trek.root.read(cx).preview.clone());

        trek.click(cx, ("attachment", 0usize));
        trek.render(cx);
        let from = preview
            .read_with(cx, |p, _| p.source())
            .expect("it knows where it came from");
        // (Inside its 1 px border.)
        assert!(
            (from.origin.x - thumb.origin.x).abs() <= px(1.)
                && (from.size.width - thumb.size.width).abs() <= px(2.),
            "{from:?} vs {thumb:?}"
        );
        let opening = trek.bounds(cx, "preview-image").unwrap();
        assert!(
            opening.size.width < thumb.size.width + px(40.),
            "it starts at the thumbnail's size: {opening:?}"
        );
        frame(&trek, cx, 1_000);
        let open_at = trek.bounds(cx, "preview-image").unwrap();
        assert!(
            open_at.size.width > px(600.),
            "then fills the stage: {open_at:?}"
        );

        // Esc: it shrinks back toward the thumbnail, mounted until it's there.
        trek.press(cx, "escape");
        frame(&trek, cx, 60);
        assert!(preview.read_with(cx, |p, _| !p.is_open() && p.is_mounted()));
        let closing = trek.bounds(cx, "preview-image").unwrap();
        assert!(
            closing.size.width < open_at.size.width && closing.size.width > thumb.size.width,
            "{closing:?}"
        );
        frame(&trek, cx, 1_000);
        assert!(preview.read_with(cx, |p, _| !p.is_mounted()));
        assert!(!trek.visible(cx, "attachment-preview"));
    });
}

#[test]
fn the_add_agent_sheet_leaves_rather_than_vanishing() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        let ws = trek.ws.clone();
        trek.window(cx, |window, cx| {
            trek_app::add_agent::open(ws, trek_app::add_agent::Tab::Command, window, cx)
        });
        frame(&trek, cx, 1_000);
        assert!(trek.visible(cx, "add-agent-sheet"));
        // Esc is a real close path: the dialog's on_close runs sheet_left at once, so the leave
        // is registered before the next frame draws. A pointer click on the cancel button is
        // hit-tested against the live tree and a loaded runner has missed it.
        trek.press(cx, "escape");
        trek.render(cx);
        assert!(
            trek.window(cx, |window, cx| !window.has_active_dialog(cx)),
            "the dialog is down"
        );
        assert!(
            trek.visible(cx, "sheet-leaving"),
            "its sheet is drawn once more as it goes"
        );
        frame(&trek, cx, 1_000);
        assert!(!trek.visible(cx, "sheet-leaving"));
    });
}

#[test]
fn basecamps_bars_grow_in_one_after_another() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, trek_core::RunState::Idle).await;
        moving(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        let basecamp = cx.read(|cx| trek.root.read(cx).basecamp.clone());
        for _ in 0..500 {
            cx.run_until_parked();
            if basecamp.read_with(cx, |b, _| b.summary().is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        frame(&trek, cx, 1_000);
        let bar = (0..24usize)
            .find(|i| trek.visible(cx, ("basecamp-bar-fill", *i)))
            .expect("a bar for the turn");
        let height = |trek: &Trek, cx: &mut TestAppContext| {
            trek.bounds(cx, ("basecamp-bar-fill", bar))
                .map_or(px(0.), |b| b.size.height)
        };
        let full = height(&trek, cx);
        assert!(
            full > px(50.),
            "the busiest bar near the chart's height: {full:?}"
        );

        // Back to Basecamp later, the chart grows in again.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx));
        trek.render(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        trek.render(cx);
        assert!(height(&trek, cx) < px(3.), "it starts flat");
        frame(&trek, cx, 300);
        let growing = height(&trek, cx);
        assert!(
            growing > px(0.) && growing < full,
            "on its way: {growing:?} of {full:?}"
        );
        frame(&trek, cx, 1_000);
        assert_eq!(height(&trek, cx), full);
    });
}

/// A frame as the platform draws one: the clock moves on, the frames asked for come, and only
/// what changed is drawn again (cached views replay their last painting, unlike `Trek::render`).
fn live_frame(trek: &Trek, cx: &mut TestAppContext, ms: u64) {
    cx.executor().advance_clock(Duration::from_millis(ms));
    trek.window(cx, |window, cx| {
        window.simulate_next_frame(cx);
    });
    cx.run_until_parked();
}

/// The opacity of the fill painted over exactly `b` (an element's own background) in the last
/// frame, if one was.
fn fill_alpha(
    trek: &Trek,
    cx: &mut TestAppContext,
    b: gpui_kit::Bounds<gpui_kit::Pixels>,
) -> Option<f32> {
    trek.window(cx, |window, _| {
        let k = window.scale_factor();
        let near = |a: f32, b: f32| (a - b * k).abs() < 1.;
        window
            .painted_quads()
            .iter()
            .filter(|q| {
                near(q.bounds.origin.x.0, b.origin.x.as_f32())
                    && near(q.bounds.origin.y.0, b.origin.y.as_f32())
                    && near(q.bounds.size.width.0, b.size.width.as_f32())
                    && near(q.bounds.size.height.0, b.size.height.as_f32())
            })
            .filter_map(|q| q.background.as_solid().map(|c| c.a))
            .reduce(f32::max)
    })
}

/// While the window crosses between modes, `b`'s fill (in a cached view on the side that's
/// going) fades with it, frame by frame, as the platform draws them.
fn fades_with_the_crossing(
    trek: &Trek,
    cx: &mut TestAppContext,
    b: gpui_kit::Bounds<gpui_kit::Pixels>,
    full: f32,
    going: impl Fn(f32) -> f32,
) {
    let mut frames = 0;
    for _ in 0..60 {
        live_frame(trek, cx, 16);
        let shown = going(editor_in(trek, cx));
        if shown == 0. {
            break;
        }
        frames += 1;
        let alpha = fill_alpha(trek, cx, b).unwrap_or(0.);
        assert!(
            (alpha - full * shown).abs() < 0.01,
            "frame {frames}: drawn at {alpha}, the rest of its side at {}",
            full * shown
        );
    }
    assert!(frames > 5, "it took a few frames: {frames}");
}

#[test]
fn the_editor_fades_out_whole_on_the_way_back_to_agents() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("one.rs");
        std::fs::write(&file, "1\n").unwrap();
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project),
                },
                cx,
            )
        });
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        trek.render(cx);
        // The file's row in the Explorer (a cached view) is highlighted: a fill to follow.
        trek.click(cx, file.display().to_string());
        moving(cx);
        frame(&trek, cx, 1_000);
        let row = trek
            .bounds(cx, file.display().to_string())
            .expect("the file's row");
        let full = fill_alpha(&trek, cx, row).expect("its highlight");

        trek.update(cx, |ws, cx| ws.set_mode(Mode::Agents, cx));
        fades_with_the_crossing(&trek, cx, row, full, |e| e);
        // Across: nothing of the editor is drawn, without a refresh to clear it.
        live_frame(&trek, cx, 500);
        live_frame(&trek, cx, 16);
        assert_eq!(
            fill_alpha(&trek, cx, row),
            None,
            "the tree is gone with the rest of the editor"
        );
        assert!(!trek.visible(cx, "ide-workbench") && !trek.visible(cx, "explorer-tree"));
    });
}

#[test]
fn agents_fade_out_whole_on_the_way_to_the_editor() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        moving(cx);
        frame(&trek, cx, 1_000);
        // The open thread's row in the sidebar (a cached view) is highlighted.
        let row = trek.bounds(cx, format!("live-line-{id}")).expect("its row");
        let full = fill_alpha(&trek, cx, row).expect("its highlight");

        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        fades_with_the_crossing(&trek, cx, row, full, |e| 1. - e);
        live_frame(&trek, cx, 500);
        live_frame(&trek, cx, 16);
        assert_eq!(fill_alpha(&trek, cx, row), None);
        assert!(!trek.visible(cx, "sidebar"));
    });
}

/// A seen, unsettled thread in a project of its own, `older_by` ms old.
fn elsewhere(trek: &Trek, cx: &mut TestAppContext, title: &str, older_by: i64) -> String {
    trek.update(cx, |ws, cx| {
        let other = common::new_project("elsewhere");
        ws.store.ensure_project(&other).unwrap();
        let mut t = ws
            .store
            .create_thread(
                Some(&other),
                mock(),
                None,
                Effort::Medium,
                HandHolding::Auto,
            )
            .expect("thread");
        t.title = title.into();
        t.updated_at = now_ms() - older_by;
        t.last_seen_at = t.updated_at;
        ws.store.save_thread(&t).expect("save");
        ws.reload(cx);
        t.id
    })
}

fn pid(trek: &Trek, cx: &TestAppContext, id: &str) -> String {
    trek.read(cx, |ws, _| {
        ws.thread(id).unwrap().project_id.clone().unwrap()
    })
}

#[test]
fn groups_trading_places_pass_whole_the_one_going_up_over_the_other() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        let a = quiet(&trek, cx, "Here", 60_000);
        let b = elsewhere(&trek, cx, "There", 120_000);
        let (pa, pb) = (pid(&trek, cx, &a), pid(&trek, cx, &b));
        // `a` open: its row has a fill to follow.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        frame(&trek, cx, 1_000);
        let head = |trek: &Trek, cx: &mut TestAppContext, p: &str| {
            trek.bounds(cx, format!("live-proj-head-{p}"))
                .expect("a header")
        };
        let (ha, hb) = (head(&trek, cx, &pa), head(&trek, cx, &pb));
        assert!(ha.origin.y < hb.origin.y, "the newer project first");
        let row_a = trek.bounds(cx, format!("live-line-{a}")).unwrap();

        // News in `b`: its project goes first. Each group slides as one; the rows in them don't
        // move against their headers.
        bump(&trek, cx, &b);
        trek.render(cx);
        let group = |trek: &Trek, cx: &mut TestAppContext, p: &str| {
            offset(trek, cx, &format!("live-g-{p}")).unwrap()
        };
        assert!(
            group(&trek, cx, &pb) > 20. && group(&trek, cx, &pa) < -20.,
            "{} {}",
            group(&trek, cx, &pb),
            group(&trek, cx, &pa)
        );
        for key in [
            a.clone(),
            b.clone(),
            format!("live-h-{pa}"),
            format!("live-h-{pb}"),
        ] {
            assert_eq!(
                offset(&trek, cx, &key),
                Some(0.),
                "{key} rides with its group"
            );
        }
        assert_eq!(
            head(&trek, cx, &pb).origin.y,
            hb.origin.y,
            "drawn where it was"
        );

        // Passing: `a`'s row isn't painted under `b`'s group, and `b` takes the click there.
        let mut passed = false;
        for _ in 0..50 {
            frame(&trek, cx, 8);
            let (top, row_b) = (
                head(&trek, cx, &pb),
                trek.bounds(cx, format!("live-line-{b}")).unwrap(),
            );
            let band = (top.origin.y.as_f32(), row_b.bottom().as_f32());
            let k = trek.window(cx, |w, _| w.scale_factor());
            let fills: Vec<(f32, f32, f32, f32)> = trek.window(cx, |w, _| {
                w.painted_quads()
                    .iter()
                    .filter(|q| {
                        (q.bounds.size.height.0 - row_a.size.height.as_f32() * k).abs() < 1.
                            && (q.bounds.size.width.0 - row_a.size.width.as_f32() * k).abs() < 1.
                    })
                    .filter(|q| q.background.as_solid().is_some_and(|c| c.a > 0.))
                    .map(|q| {
                        (
                            q.bounds.origin.y.0 / k,
                            q.bounds.bottom().0 / k,
                            q.content_mask.bounds.origin.y.0 / k,
                            q.content_mask.bounds.bottom().0 / k,
                        )
                    })
                    .collect()
            });
            let Some(&(y0, y1, m0, m1)) = fills.first() else {
                continue;
            };
            // What's painted is the fill where its mask lets it be: a row clear of the band
            // needs no mask to stay out of it.
            for (f0, f1, m0, m1) in &fills {
                let painted = (f0.max(*m0).max(band.0), f1.min(*m1).min(band.1));
                assert!(
                    painted.1 - painted.0 < 0.5,
                    "`a` painted under `b`'s group: fill {f0}..{f1}, mask {m0}..{m1}, group {band:?}"
                );
            }
            let mid = row_b.center().y.as_f32();
            if !passed && y0 < mid && mid < y1 && band.0 < band.1 {
                assert!(m1 <= band.0 + 0.5 || m0 >= band.1 - 0.5);
                passed = true;
                trek.click(cx, format!("live-line-{b}"));
                assert_eq!(
                    trek.read(cx, |ws, _| ws.route.clone()),
                    Route::Thread(b.clone()),
                    "the click lands on the group on top"
                );
                break;
            }
        }
        assert!(passed, "they passed each other");
        frame(&trek, cx, 1_000);
        assert!(head(&trek, cx, &pb).origin.y < head(&trek, cx, &pa).origin.y);
        assert_eq!(group(&trek, cx, &pa), 0.);
    });
}

#[test]
fn a_click_on_a_row_as_it_fades_away_does_nothing() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        let ids: Vec<String> = (0..3)
            .map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i + 1) * 60_000))
            .collect();
        frame(&trek, cx, 1_000);
        let route = trek.read(cx, |ws, _| ws.route.clone());
        trek.update(cx, |ws, cx| ws.archive(&ids[2], cx));
        frame(&trek, cx, 60);
        assert_eq!(ghosts(&trek, cx), [ids[2].clone()]);
        trek.click(cx, format!("live-line-{}", ids[2]));
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            route,
            "the archived thread doesn't open"
        );

        // The sidebar folding away (⌘B) is out of reach too.
        trek.press(cx, "secondary-b");
        frame(&trek, cx, 60);
        assert!(trek.visible(cx, "sidebar"));
        trek.click(cx, format!("live-line-{}", ids[0]));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), route);
    });
}

#[test]
fn a_click_on_the_palette_as_it_goes_does_nothing() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        quiet(&trek, cx, "Recent", 60_000);
        let route = trek.read(cx, |ws, _| ws.route.clone());
        trek.press(cx, "secondary-k");
        frame(&trek, cx, 1_000);
        trek.press(cx, "escape");
        frame(&trek, cx, 40);
        assert!(
            trek.visible(cx, ("palette-row", 0usize)),
            "still drawn as it leaves"
        );
        trek.click(cx, ("palette-row", 0usize));
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            route,
            "nothing ran"
        );
        assert!(!cx.read(|cx| trek.root.read(cx).palette.read(cx).open));
    });
}

#[test]
fn a_click_as_the_preview_shrinks_back_goes_to_what_is_under_it() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        let id = quiet(&trek, cx, "Under the preview", 60_000);
        // Bigger than the stage at 2x: a press zooms it.
        let image = png(&trek, "big.png", 4000, 2500);
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        composer.update(cx, |c, cx| c.attach_image(image.clone(), cx));
        trek.render(cx);
        trek.click(cx, ("attachment", 0usize));
        frame(&trek, cx, 1_000);
        trek.press(cx, "escape");
        frame(&trek, cx, 40);
        let preview = cx.read(|cx| trek.root.read(cx).preview.clone());
        assert!(preview.read_with(cx, |p, _| !p.is_open() && p.is_mounted()));
        // A press on the image would zoom it to actual pixels; going, it doesn't.
        trek.click(cx, "preview-image");
        assert!(preview.read_with(cx, |p, _| !p.is_actual()));
        // The sidebar's row, under the preview's backdrop.
        trek.click(cx, format!("live-line-{id}"));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(id));
        assert!(
            preview.read_with(cx, |p, _| !p.is_open()),
            "and the preview didn't take it"
        );
    });
}

#[test]
fn a_click_on_a_sheet_as_it_leaves_does_nothing() {
    use gpui_kit::test::TestWindowExt as _;
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        let ws = trek.ws.clone();
        trek.window(cx, |window, cx| {
            trek_app::add_agent::open(ws, trek_app::add_agent::Tab::Command, window, cx)
        });
        frame(&trek, cx, 1_000);
        trek.window(cx, |window, cx| {
            // Esc is a real close path: the dialog's on_close runs sheet_left at once, so the
            // leave is registered before the next frame draws. A pointer click on the cancel
            // button is hit-tested against the live tree and a loaded runner has missed it;
            // close_dialog pops the dialog without running on_close.
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(
                window.find("sheet-leaving").visible(),
                "the sheet is on its way out"
            );
            // "Add agent" on the empty form would complain; on its way out it does nothing.
            window.click("command-add", cx);
            window.render_frame(cx);
            assert!(
                !window
                    .try_find("command-error")
                    .is_some_and(|e| e.visible())
            );
        });
        assert!(trek.window(cx, |window, cx| !window.has_active_dialog(cx)));
    });
}

#[test]
fn undo_on_a_toast_as_it_goes_does_nothing() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        let id = trek.quiet_thread(cx);
        cx.update(|cx| trek_app::root::init(trek.ws.clone(), cx));
        trek.window(cx, |window, _| window.activate_window());
        cx.run_until_parked();
        trek.press(cx, "secondary-e");
        let settled = |trek: &Trek, cx: &mut TestAppContext| {
            trek.read(cx, |ws, _| {
                ws.thread(&id).is_some_and(|t| t.settled_at.is_some())
            })
        };
        assert!(settled(&trek, cx));
        frame(&trek, cx, 1_000);
        trek.click(cx, "undo");
        assert!(!settled(&trek, cx), "Undo brings it back");
        // Settled again some other way while the toast goes: its Undo is spent.
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&id, |t| t.settled_at = Some(now_ms()))
                .unwrap();
            ws.reload(cx);
        });
        trek.render(cx);
        assert!(trek.visible(cx, "undo"), "still drawn as it goes");
        trek.click(cx, "undo");
        assert!(settled(&trek, cx), "a second Undo as it goes does nothing");
    });
}

/// Something under the toasts that counts what reaches it.
#[derive(Default)]
struct Probe {
    clicks: usize,
    hovered: bool,
    scrolls: usize,
}

impl gpui_kit::Render for Probe {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{
            InteractiveElement as _, StatefulInteractiveElement as _, Styled as _,
            TestSupportExt as _,
        };
        gpui_kit::div()
            .id("probe")
            .test_support()
            .size_full()
            .on_click(cx.listener(|p, _, _, _| p.clicks += 1))
            .on_hover(cx.listener(|p, on: &bool, _, _| p.hovered = *on))
            .on_scroll_wheel(cx.listener(|p, _, _, _| p.scrolls += 1))
    }
}

/// A window of its own with only `Probe` in it, and the toast layer over that.
fn probe_window(cx: &mut TestAppContext) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<Probe>) {
    use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, point, size};
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(800.), px(600.)),
        })),
        ..Default::default()
    };
    cx.update(|cx| gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Probe::default())))
        .expect("probe window")
}

fn in_window<R>(
    cx: &mut TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    f: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App) -> R,
) -> R {
    let r = gpui_kit::AppContext::update_window(cx, window, |_, window, cx| f(window, cx))
        .expect("window");
    cx.run_until_parked();
    r
}

/// Move the clock on `ms` and draw `window`.
fn frame_in(cx: &mut TestAppContext, window: gpui_kit::AnyWindowHandle, ms: u64) {
    use gpui_kit::test::TestWindowExt as _;
    cx.executor().advance_clock(Duration::from_millis(ms));
    cx.run_until_parked();
    in_window(cx, window, |w, cx| w.render_frame(cx));
}

#[test]
fn a_toast_on_its_way_out_lets_the_pointer_through() {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{ScrollDelta, point};
    use trek_app::toast::{Toast, count};
    run(async |cx| {
        let _trek = open(cx);
        moving(cx);
        let (window, probe) = probe_window(cx);
        let opened = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = opened.clone();
        in_window(cx, window, |w, cx| {
            trek_app::toast::push(
                w,
                Toast::new("Finished: Fix the login bug")
                    .on_click(move |_, _, _| seen.set(seen.get() + 1)),
                cx,
            )
        });
        frame_in(cx, window, 1_000);
        // Up: it takes the pointer, and a click on it does what it says.
        in_window(cx, window, |w, cx| w.hover("notification", cx));
        let close = in_window(cx, window, |w, cx| {
            w.render_frame(cx);
            let (face, close) = (w.find("notification"), w.find("toast-close"));
            assert!(close.visible(), "the pointer on it shows its close button");
            close.bounds().center() - face.bounds().origin
        });
        assert!(!probe.read_with(cx, |p, _| p.hovered));
        in_window(cx, window, |w, cx| w.click("notification", cx));
        assert_eq!((opened.get(), probe.read_with(cx, |p, _| p.clicks)), (1, 0));

        // On its way out (clicked away): still drawn, but out of reach.
        frame_in(cx, window, 40);
        assert_eq!(
            in_window(cx, window, |w, cx| count(w, cx)),
            1,
            "still mounted as it goes"
        );
        assert!(in_window(cx, window, |w, _| w
            .find("notification")
            .visible()));
        in_window(cx, window, |w, cx| w.hover("notification", cx));
        assert!(
            probe.read_with(cx, |p, _| p.hovered),
            "the pointer on it hovers what's under it"
        );
        in_window(cx, window, |w, cx| w.click("notification", cx));
        assert_eq!(
            (opened.get(), probe.read_with(cx, |p, _| p.clicks)),
            (1, 1),
            "a click reaches what's under it"
        );
        in_window(cx, window, |w, cx| {
            w.scroll(
                "notification",
                ScrollDelta::Pixels(point(px(0.), px(-40.))),
                cx,
            )
        });
        assert_eq!(
            probe.read_with(cx, |p, _| p.scrolls),
            1,
            "and so does the wheel"
        );
        // Where its close button was: what's under it gets the click.
        in_window(cx, window, |w, cx| w.click_at("notification", close, cx));
        assert_eq!(probe.read_with(cx, |p, _| p.clicks), 2);
        assert!(!in_window(cx, window, |w, _| w
            .find("toast-close")
            .visible()));
        frame_in(cx, window, 1_000);
        assert_eq!(
            in_window(cx, window, |w, cx| count(w, cx)),
            0,
            "gone once it has left"
        );
    });
}

#[test]
fn a_live_toast_closes_and_times_out_and_reduce_motion_takes_it_at_once() {
    use gpui_kit::test::TestWindowExt as _;
    use trek_app::toast::{Toast, count};
    run(async |cx| {
        let _trek = open(cx);
        moving(cx);
        let (window, probe) = probe_window(cx);
        let toasts = |cx: &mut TestAppContext| in_window(cx, window, |w, cx| count(w, cx));
        // Its close button takes it down, and the click goes no further.
        in_window(cx, window, |w, cx| trek_app::toast::push(w, "Saved", cx));
        frame_in(cx, window, 1_000);
        in_window(cx, window, |w, cx| w.hover("notification", cx));
        in_window(cx, window, |w, cx| w.click("toast-close", cx));
        assert_eq!(probe.read_with(cx, |p, _| p.clicks), 0);
        frame_in(cx, window, 20);
        assert_eq!(toasts(cx), 1, "on its way out");
        frame_in(cx, window, 1_000);
        assert_eq!(toasts(cx), 0);

        // Left alone, it goes once its time is up.
        in_window(cx, window, |w, cx| {
            trek_app::toast::push(w, Toast::new("Saved").lifetime(Duration::from_secs(3)), cx)
        });
        frame_in(cx, window, 2_900);
        assert_eq!(toasts(cx), 1);
        frame_in(cx, window, 140); // 40 ms into its exit (QUICK: about 150 ms all told)
        assert_eq!(toasts(cx), 1, "still drawn as it goes");
        assert!(in_window(cx, window, |w, _| w
            .find("notification")
            .visible()));
        frame_in(cx, window, 1_000);
        assert_eq!(toasts(cx), 0, "gone after its time");

        // With Reduce Motion it's gone the moment it's taken down.
        cx.update(|cx| cx.set_reduce_motion(true));
        let key = Toast::new_key();
        in_window(cx, window, |w, cx| {
            trek_app::toast::push(w, Toast::new("Saved").key(key.clone()), cx)
        });
        frame_in(cx, window, 0);
        assert!(
            in_window(cx, window, |w, _| w.find("notification").visible()),
            "in at once"
        );
        in_window(cx, window, |w, cx| trek_app::toast::dismiss(w, &key, cx));
        assert_eq!(toasts(cx), 0, "out at once");
        frame_in(cx, window, 0);
        assert!(in_window(cx, window, |w, _| w
            .try_find("notification")
            .is_none_or(|t| !t.visible())));
    });
}

#[test]
fn a_click_on_a_toast_as_it_goes_opens_the_row_under_it() {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Point, point, size};
    use trek_app::toast::{Toast, count};
    run(async |cx| {
        let trek = open(cx);
        // Narrow and short: the toast, centred at the bottom, is over the sidebar's rows.
        cx.simulate_window_resize(trek.window, size(px(600.), px(290.)));
        let ids: Vec<String> = (0..4)
            .map(|i| quiet(&trek, cx, &format!("Thread {i}"), 60_000 * (i + 1)))
            .collect();
        moving(cx);
        let key = Toast::new_key();
        trek.window(cx, |w, cx| {
            trek_app::toast::push(w, Toast::new("Saved").key(key.clone()), cx)
        });
        frame(&trek, cx, 1_000);
        let face = trek.bounds(cx, "notification").expect("the toast");
        // A row under the toast's left end, and a point on both.
        let (id, at): (String, Point<gpui_kit::Pixels>) = ids
            .iter()
            .find_map(|id| {
                let row = trek.bounds(cx, format!("live-line-{id}"))?;
                let at = point(face.left() + px(24.), row.center().y);
                (row.contains(&at) && face.contains(&at) && (at.y - face.top()).abs() > px(4.))
                    .then(|| (id.clone(), at))
            })
            .unwrap_or_else(|| {
                panic!(
                    "no row under the toast at {face:?}: {:?}",
                    ids.iter()
                        .map(|id| trek.bounds(cx, format!("live-line-{id}")))
                        .collect::<Vec<_>>()
                )
            });
        let before = trek.read(cx, |ws, _| ws.route.clone());
        // Up, it covers the row.
        trek.window(cx, |w, cx| w.click_at("notification", at - face.origin, cx));
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), before);
        // On its way out, the row takes the click.
        trek.window(cx, |w, cx| trek_app::toast::dismiss(w, &key, cx));
        frame(&trek, cx, 8); // a frame into its exit, still over the row
        assert_eq!(
            trek.window(cx, |w, cx| count(w, cx)),
            1,
            "still drawn as it goes"
        );
        let face = trek.bounds(cx, "notification").expect("still drawn");
        assert!(face.contains(&at), "still over the row");
        trek.window(cx, |w, cx| w.click_at("notification", at - face.origin, cx));
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(id));
    });
}

#[test]
fn a_row_leaving_its_group_for_another_goes_from_where_it_was() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        trek.update(cx, |ws, cx| {
            ws.settled_open = true;
            cx.notify();
        });
        quiet(&trek, cx, "Stays", 60_000);
        // Alone in the second project's group: settling it takes the group away.
        let a = elsewhere(&trek, cx, "Settles", 120_000);
        frame(&trek, cx, 1_000);
        let before = row_y(&trek, cx, &a).unwrap();
        trek.update(cx, |ws, cx| ws.settle(&a, cx));
        trek.render(cx);
        let title = |trek: &Trek, cx: &mut TestAppContext| {
            trek.bounds(cx, format!("line-{a}"))
                .expect("in the history")
                .origin
                .y
                .as_f32()
        };
        let first = title(&trek, cx);
        frame(&trek, cx, 1_000);
        let after = title(&trek, cx);
        assert!(
            (first - before).abs() < 8.,
            "drawn where it was: {before} → {first}"
        );
        assert!(after > first + 20., "then down into the history: {after}");
    });
}

// ---------- the motion pass: frequency, exits, the tools panel, one policy ----------

/// How far the tools panel is out of the window's right edge (1 out), and how far it has grown
/// from its side to fill the window (1 filling it).
fn panel(trek: &Trek, cx: &mut TestAppContext) -> (f32, f32) {
    let now = cx.executor().now();
    cx.read(|cx| {
        let root = trek.root.read(cx);
        (root.panel_motion.value(now), root.panel_fill.value(now))
    })
}

/// Values read a frame (16 ms) apart for `frames` frames.
fn samples(
    trek: &Trek,
    cx: &mut TestAppContext,
    frames: usize,
    read: impl Fn(&Trek, &mut TestAppContext) -> f32,
) -> Vec<f32> {
    (0..frames)
        .map(|_| {
            frame(trek, cx, 16);
            read(trek, cx)
        })
        .collect()
}

fn monotonic(values: &[f32], rising: bool) -> bool {
    values
        .windows(2)
        .all(|w| if rising { w[1] >= w[0] } else { w[1] <= w[0] })
}

/// The palette is opened all day, mostly from ⌘K: it's there whole in the frame the key lands
/// in (no fade, no drop), and it leaves on the quick spring, gone in about a quarter second
/// where it used to take about four tenths.
#[test]
fn the_palette_opens_at_once_and_leaves_quicker_than_it_used_to() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        trek.render(cx);
        trek.press(cx, "secondary-k");
        trek.render(cx);
        let first = trek.bounds(cx, "palette").expect("open in the first frame");
        frame(&trek, cx, 1_000);
        let settled = trek.bounds(cx, "palette").unwrap();
        assert_eq!(
            first.origin, settled.origin,
            "no drop-in: it's where it stays"
        );

        trek.press(cx, "escape");
        frame(&trek, cx, 16);
        assert!(
            trek.visible(cx, "palette"),
            "it fades out rather than blinking away"
        );
        frame(&trek, cx, 250);
        assert!(!trek.visible(cx, "palette"), "gone by a quarter second");

        // Reopened mid-exit: back whole at once, not fading up from where it was.
        trek.press(cx, "secondary-k");
        frame(&trek, cx, 1_000);
        trek.press(cx, "escape");
        frame(&trek, cx, 40);
        trek.press(cx, "secondary-k");
        trek.render(cx);
        assert_eq!(trek.bounds(cx, "palette").unwrap().origin, settled.origin);
    });
}

/// ⌘B is a keyboard toggle done tens of times a day: it folds on the quick spring. The title
/// bar's button is the pointer's, and keeps the calmer surface spring. Both start in the first
/// frame, never overshoot, and settle.
#[test]
fn the_sidebar_folds_quicker_from_its_key_than_from_its_button() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        trek.render(cx);
        trek.press(cx, "secondary-b");
        frame(&trek, cx, 100);
        let by_key = sidebar(&trek, cx);
        frame(&trek, cx, 1_000);
        assert_eq!(sidebar(&trek, cx), 0.);
        trek.press(cx, "secondary-b");
        frame(&trek, cx, 1_000);
        assert_eq!(sidebar(&trek, cx), 1.);

        trek.click(cx, "toggle-sidebar");
        frame(&trek, cx, 16);
        let first = sidebar(&trek, cx);
        assert!(first < 1., "moving from the first frame: {first}");
        frame(&trek, cx, 84);
        let by_pointer = sidebar(&trek, cx);
        assert!(
            by_key < by_pointer - 0.1,
            "100 ms in, the key's fold is further along: {by_key} vs {by_pointer}"
        );
        let rest = samples(&trek, cx, 40, sidebar);
        assert!(monotonic(&rest, false), "{rest:?}");
        assert!(rest.iter().all(|v| *v >= 0.), "no overshoot: {rest:?}");
        assert_eq!(rest.last().copied(), Some(0.));
    });
}

/// The tools panel from the title bar's button comes out of the window's right edge, its left
/// edge leading, and goes back the same way; turned round mid-way it goes back from where it is.
/// ⌘J (the keyboard's way, all day) puts it there at once.
#[test]
fn the_tools_panel_slides_out_from_the_pointer_and_is_there_at_once_from_its_key() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        trek.render(cx);
        trek.click(cx, "toggle-tools");
        frame(&trek, cx, 16);
        let (first, _) = panel(&trek, cx);
        assert!(
            first > 0. && first < 1.,
            "moving from the first frame: {first}"
        );
        assert!(
            trek.visible(cx, "tools-sliding"),
            "drawn as far as it's out"
        );
        frame(&trek, cx, 44);
        let (mid, _) = panel(&trek, cx);
        let shown = trek
            .bounds(cx, "tools-sliding")
            .map(|b| b.size.width.as_f32())
            .unwrap_or(0.);
        assert!(shown > 20., "{shown}");

        // Closed again mid-way: back from where it is, no jump.
        trek.click(cx, "toggle-tools");
        trek.render(cx);
        let (turned, _) = panel(&trek, cx);
        assert!((turned - mid).abs() < 0.02, "{mid} → {turned}");
        let back = samples(&trek, cx, 40, |t, cx| panel(t, cx).0);
        assert!(
            back[0] >= turned - 0.2,
            "no jump back: {turned} → {}",
            back[0]
        );
        assert!(back.iter().all(|v| *v >= 0.), "{back:?}");
        assert_eq!(back.last().copied(), Some(0.));
        assert!(!trek.visible(cx, "tools-sliding") && !trek.visible(cx, "tools-out"));

        // Open to the end: then it's simply there, laid out whole.
        trek.click(cx, "toggle-tools");
        frame(&trek, cx, 1_000);
        assert_eq!(panel(&trek, cx).0, 1.);
        assert!(!trek.visible(cx, "tools-sliding") && trek.visible(cx, "tools-out"));
        trek.click(cx, "toggle-tools");
        frame(&trek, cx, 1_000);

        // ⌘J: at once both ways.
        trek.press(cx, "secondary-j");
        trek.render(cx);
        assert_eq!(panel(&trek, cx).0, 1.);
        assert!(!trek.visible(cx, "tools-sliding") && trek.visible(cx, "tools-out"));
        trek.press(cx, "secondary-j");
        trek.render(cx);
        assert_eq!(panel(&trek, cx).0, 0.);
        assert!(!trek.visible(cx, "tools-out"));
    });
}

/// "Fill the window" grows the panel from its side to the window's whole width over the chat,
/// and back.
#[test]
fn filling_the_window_grows_the_panel_from_its_side_and_back() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        trek.press(cx, "secondary-j");
        frame(&trek, cx, 16);
        let side = trek.bounds(cx, "tools-out").unwrap().size.width.as_f32();
        let whole = trek.window(cx, |window, _| window.viewport_size().width.as_f32());
        let width = |trek: &Trek, cx: &mut TestAppContext| {
            trek.bounds(cx, "tools-overlay")
                .map(|b| b.size.width.as_f32())
                .unwrap_or(0.)
        };
        trek.click(cx, "panel-fill");
        frame(&trek, cx, 16);
        let first = width(&trek, cx);
        assert!(
            first > side - 1. && first < whole - 1.,
            "growing from its side: {side} < {first} < {whole}"
        );
        let grow = samples(&trek, cx, 40, width);
        assert!(monotonic(&grow, true), "{grow:?}");
        assert!(
            (grow.last().unwrap() - whole).abs() < 1.,
            "{grow:?} of {whole}"
        );

        trek.click(cx, "panel-fill");
        frame(&trek, cx, 16);
        let shrinking = width(&trek, cx);
        assert!(shrinking < whole - 1. && shrinking > side, "{shrinking}");
        frame(&trek, cx, 1_000);
        assert!(!trek.visible(cx, "tools-overlay"), "back at the side");
        let back = trek.bounds(cx, "tools-out").unwrap().size.width.as_f32();
        assert!((back - side).abs() < 1., "{back} vs {side}");
    });
}

/// A toast comes up on the surface spring and goes on the quick one: an exit is quicker than its
/// entrance.
#[test]
fn a_toast_leaves_quicker_than_it_came() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        cx.update(|cx| trek_app::root::init(trek.ws.clone(), cx));
        trek.window(cx, |window, _| window.activate_window());
        cx.run_until_parked();
        let toasts = |trek: &Trek, cx: &mut TestAppContext| {
            trek.window(cx, |window, cx| trek_app::toast::count(window, cx))
        };
        trek.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::Toast {
                message: "Saved".into(),
                undo: None,
            })
        });
        trek.render(cx);
        frame(
            &trek,
            cx,
            trek_app::root::toast_lifetime("Saved", false).as_millis() as u64 + 10,
        );
        assert_eq!(toasts(&trek, cx), 1, "on its way out");
        frame(&trek, cx, 260);
        assert_eq!(toasts(&trek, cx), 0, "gone in a quarter second");
    });
}

/// Trek's Reduce motion: the tools panel and its fill are where they go at once, opened from the
/// pointer as from the keys.
#[test]
fn reduce_motion_puts_the_tools_panel_and_its_fill_there_at_once() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.appearance.reduce_motion = true);
        moving(cx);
        trek.render(cx);
        trek.click(cx, "toggle-tools");
        trek.render(cx);
        assert_eq!(panel(&trek, cx), (1., 0.));
        assert!(!trek.visible(cx, "tools-sliding"));
        trek.click(cx, "panel-fill");
        trek.render(cx);
        assert_eq!(panel(&trek, cx), (1., 1.));
        let whole = trek.window(cx, |window, _| window.viewport_size().width);
        let over = trek.bounds(cx, "tools-overlay").unwrap();
        assert!((over.size.width - whole).abs() < px(1.), "{over:?}");
        trek.click(cx, "panel-fill");
        trek.click(cx, "toggle-sidebar");
        trek.render(cx);
        assert_eq!(sidebar(&trek, cx), 0., "the button's fold is at once too");
    });
}

/// A "Still" Trek theme (motion scale 0) is as good as Reduce motion: the policy says no, and
/// the app's flag, which GPUI's and the components' own animations read, is set.
#[test]
fn a_motion_scale_of_zero_stills_everything_reduce_motion_does() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.appearance.motion_scale = 0.);
        moving(cx);
        assert!(!trek.read(cx, |ws, cx| ws.motion(cx)));
        assert!(!cx.update(|cx| trek_app::motion::enabled(cx)));
        cx.update(|cx| trek_app::system::reduce_motion_as(false, &trek.ws, cx));
        assert!(cx.update(|cx| cx.reduce_motion()));
        trek.press(cx, "secondary-b");
        assert_eq!(sidebar(&trek, cx), 0., "folded at once");
        trek.update(cx, |ws, _| ws.settings.appearance.motion_scale = 1.35);
        cx.update(|cx| trek_app::system::reduce_motion_as(false, &trek.ws, cx));
        assert!(trek.read(cx, |ws, cx| ws.motion(cx)));
        assert!(!cx.update(|cx| cx.reduce_motion()));
    });
}

/// Every animation goes through the one motion policy: a fixed-time one takes its `Animation`
/// from `motion::timed` or `motion::looping` (the named times at the motion scale), nothing but
/// the policy reads the reduce-motion flag, and a view that draws itself frame by frame on its
/// own clock is one of those checked to ask the policy. A new one fails here until it does.
#[test]
fn every_animation_goes_through_the_motion_policy() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // Hand clocks: each asks `motion::enabled` (or `Workspace::motion`) and takes its time from
    // the named times, or asks for frames for something other than motion (a re-measure).
    const OWN_FRAMES: &[&str] = &[
        // The expanded card's dialog follows the window: not motion.
        "artifact/mod.rs",
        // The chart's entrance: Workspace::motion, visualization::entering.
        "basecamp.rs",
        // The status chip's fade: motion::enabled, motion::FADE.
        "composer.rs",
        // The Compact shimmer, still without motion.
        "context_popover.rs",
        // The toolbar re-measured: not motion.
        "notes.rs",
        // The welcome's entrance: Workspace::motion.
        "onboarding/steps.rs",
        // Drawn in only with Workspace::motion.
        "panels/branch_graph.rs",
        // The page's load progress: a status.
        "panels/browser.rs",
        // The tab strip re-measured: not motion.
        "panels/mod.rs",
        // The busy ring: motion::enabled.
        "pr_card.rs",
        // A new title's reveal: Workspace::motion, at the motion scale.
        "root.rs",
        "sidebar.rs",
        "thread_window.rs",
        // The strip re-measured: not motion.
        "tabs.rs",
        // Entrances and the hover lift: motion::enabled, at the motion scale.
        "visualization/mod.rs",
    ];
    let mut wrong = Vec::new();
    let mut dirs = vec![src.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let rel = path
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&path).unwrap();
            for (n, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                let at = format!("{rel}:{}", n + 1);
                if rel != "motion.rs" && code.contains("Animation::new(") {
                    wrong.push(format!(
                        "{at}: Animation::new — use motion::timed or motion::looping"
                    ));
                }
                if rel != "motion.rs"
                    && rel != "system.rs"
                    && (code.contains(".reduce_motion()") || code.contains("set_reduce_motion("))
                {
                    wrong.push(format!(
                        "{at}: the reduce-motion flag — ask motion::enabled"
                    ));
                }
                if code.contains("request_animation_frame") && !OWN_FRAMES.contains(&rel.as_str()) {
                    wrong.push(format!(
                        "{at}: frames on its own clock — ask motion::enabled, take the time from \
                         motion::scaled, then list the file in OWN_FRAMES"
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Only the panel's own buttons slide it: a tool an agent or a link opens (`open_tool`) is there
/// at once, so nothing beside the panel re-renders while it would have slid. And once a slide
/// has settled it costs nothing: frames after it redraw neither the window nor the transcript.
#[test]
fn a_tool_opened_by_code_is_there_at_once_and_a_settled_slide_costs_nothing() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        trek.render(cx);
        let tools = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.window(cx, |window, cx| {
            tools.update(cx, |p, cx| {
                p.open_tool(trek_app::workspace::PanelTool::Git, window, cx)
            })
        });
        trek.render(cx);
        assert_eq!(panel(&trek, cx), (1., 0.), "there at once");
        assert!(!trek.visible(cx, "tools-sliding"));

        // From the title bar: it slides, then settles, and then no frame redraws anything.
        trek.click(cx, "toggle-tools");
        frame(&trek, cx, 1_000);
        assert_eq!(panel(&trek, cx).0, 0.);
        trek.click(cx, "toggle-tools");
        frame(&trek, cx, 16);
        assert!(
            trek.visible(cx, "tools-sliding"),
            "the pointer's open slides"
        );
        frame(&trek, cx, 1_000);
        cx.run_until_parked();
        trek_app::test_support::take_renders();
        for _ in 0..10 {
            cx.executor().advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
        }
        let renders = trek_app::test_support::take_renders();
        for view in ["TrekWindow", "ThreadView", "RightPanel", "Composer"] {
            assert_eq!(
                renders.get(view).copied().unwrap_or(0),
                0,
                "{view} re-rendered after the slide settled: {renders:?}"
            );
        }
    });
}
