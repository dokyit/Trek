//! Motion (`crate::motion`): springs that turn round where they are, rows that slide to new
//! places, and what goes away animating out before it unmounts. Time is the test scheduler's
//! clock, moved on by hand; without motion everything is where it's going at once.

use super::harness::{Trek, mock, open, open_with, run};
use crate::workspace::{Mode, Route, WorkspaceEvent};
use gpui_kit::component::WindowExt as _;
use gpui_kit::{TestAppContext, px};
use std::path::PathBuf;
use std::time::Duration;
use trek_core::store::now_ms;
use trek_core::{Effort, HandHolding};

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
        trek.press(cx, "cmd-b");
        frame(&trek, cx, 60);
        let folding = sidebar(&trek, cx);
        assert!(folding > 0.1 && folding < 0.9, "part-way: {folding}");
        // Drawn part-way out of the window's left edge, not gone.
        let x = trek.bounds(cx, "sidebar").expect("still drawn while it folds").origin.x;
        assert!(x < px(0.) && x > px(-crate::root::SIDEBAR_WIDTH), "{x:?}");

        // ⌘B again, mid-fold: it turns round from where it is, at the speed it had.
        trek.press(cx, "cmd-b");
        trek.render(cx);
        let turned = sidebar(&trek, cx);
        assert!((turned - folding).abs() < 0.01, "no jump at the turn: {folding} → {turned}");
        frame(&trek, cx, 16);
        assert!(sidebar(&trek, cx) < turned, "still folding for a moment: it was moving that way");
        let samples: Vec<f32> = (0..40).map(|_| {
            frame(&trek, cx, 16);
            sidebar(&trek, cx)
        }).collect();
        assert!(samples.windows(2).all(|w| w[1] >= w[0] - 1e-4), "then it comes back out steadily: {samples:?}");
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
        assert!(trek.visible(cx, "sidebar"), "Agents is still drawn, fading out under the editor");

        trek.update(cx, |ws, cx| ws.set_mode(Mode::Agents, cx));
        trek.render(cx);
        assert!((editor_in(&trek, cx) - crossing).abs() < 0.01, "back from where it was");
        frame(&trek, cx, 1_000);
        assert_eq!(editor_in(&trek, cx), 0.);
        assert!(trek.visible(cx, "sidebar"));

        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        frame(&trek, cx, 1_000);
        assert_eq!(editor_in(&trek, cx), 1.);
        assert!(!trek.visible(cx, "sidebar"), "once across, only the editor is drawn");
    });
}

#[test]
fn reduce_motion_puts_everything_where_it_goes_at_once() {
    run(async |cx| {
        // Trek's own setting, with the system's off.
        let trek = open_with(cx, |s| s.appearance.reduce_motion = true);
        moving(cx);
        trek.press(cx, "cmd-b");
        assert_eq!(sidebar(&trek, cx), 0.);
        assert!(!trek.visible(cx, "sidebar"));
        trek.press(cx, "cmd-b");
        assert!(trek.visible(cx, "sidebar"));

        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        trek.render(cx);
        assert_eq!(editor_in(&trek, cx), 1.);
        assert!(!trek.visible(cx, "sidebar"));
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Agents, cx));
        trek.render(cx);

        trek.press(cx, "cmd-k");
        assert!(trek.visible(cx, "palette"));
        trek.press(cx, "escape");
        assert!(!trek.visible(cx, "palette"), "gone at once, nothing left to fade");

        // A row that moves takes its new place at once.
        let ids: Vec<String> = (0..3).map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i + 1) * 60_000)).collect();
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
        let mut t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
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
    trek.bounds(cx, format!("card-{id}")).or_else(|| trek.bounds(cx, format!("live-line-{id}"))).map(|b| b.origin.y.as_f32())
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
        let ids: Vec<String> = (0..3).map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i + 1) * 60_000)).collect();
        trek.render(cx);
        frame(&trek, cx, 1_000);
        let before: Vec<f32> = ids.iter().map(|id| row_y(&trek, cx, id).expect("a row")).collect();
        assert!(before.windows(2).all(|w| w[0] < w[1]), "newest first: {before:?}");

        // The oldest gets news and goes to the top. The first frame draws every row where it
        // was (the same row, under its own id), each sprung toward its new place.
        bump(&trek, cx, &ids[2]);
        trek.render(cx);
        assert!(trek.visible(cx, format!("live-line-{}", ids[2])), "the row that moved is the same row");
        let first: Vec<f32> = ids.iter().map(|id| row_y(&trek, cx, id).unwrap()).collect();
        for (b, f) in before.iter().zip(&first) {
            assert!((b - f).abs() < 1., "drawn where it was: {before:?} → {first:?}");
        }
        assert!(offset(&trek, cx, &ids[2]).unwrap() > 20., "the risen row is a long way below its place");
        assert!(offset(&trek, cx, &ids[0]).unwrap() < 0., "the others are above theirs");

        frame(&trek, cx, 80);
        let mid = row_y(&trek, cx, &ids[2]).unwrap();
        assert!(mid < before[2] && mid > before[0], "on its way up: {mid}");

        frame(&trek, cx, 1_000);
        let after: Vec<f32> = ids.iter().map(|id| row_y(&trek, cx, id).unwrap()).collect();
        assert_eq!(after[2], before[0], "at the top");
        assert_eq!((after[0], after[1]), (before[1], before[2]), "the rest one down");
        assert!(ids.iter().all(|id| offset(&trek, cx, id) == Some(0.)));
    });
}

#[test]
fn a_new_row_settles_in_and_an_archived_one_folds_away() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        let ids: Vec<String> = (0..3).map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i + 1) * 60_000)).collect();
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
        assert!(closing < before[2] && closing > before[1], "the row under it closes the gap: {closing}");
        assert_eq!(row_y(&trek, cx, &ids[1]), Some(before[1]), "fading where it was");
        frame(&trek, cx, 1_000);
        assert!(ghosts(&trek, cx).is_empty());
        assert!(!trek.visible(cx, format!("live-line-{}", ids[1])), "and it's gone");
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

#[test]
fn the_palette_stays_while_it_fades_then_goes() {
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        trek.press(cx, "cmd-k");
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
        cx.update(|cx| crate::root::init(trek.ws.clone(), cx));
        trek.window(cx, |window, _| window.activate_window());
        cx.run_until_parked();
        let toasts = |trek: &Trek, cx: &mut TestAppContext| trek.window(cx, |window, cx| window.notifications(cx).len());
        trek.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message: "Saved".into(), undo: None }));
        trek.render(cx);
        assert_eq!(toasts(&trek, cx), 1);
        // Its time is up: it starts to go, and stays mounted while it does.
        frame(&trek, cx, crate::root::toast_lifetime("Saved", false).as_millis() as u64 + 10);
        assert_eq!(toasts(&trek, cx), 1, "still there, on its way out");
        frame(&trek, cx, 400);
        assert_eq!(toasts(&trek, cx), 0, "gone once it has left");
    });
}

/// A `w`×`h` PNG in the project.
fn png(trek: &Trek, name: &str, w: u32, h: u32) -> PathBuf {
    let path = trek.project.join(name);
    image::RgbaImage::from_pixel(w, h, image::Rgba([90, 140, 200, 255])).save(&path).expect("png");
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
        let thumb = trek.bounds(cx, ("attachment", 0usize)).expect("a thumbnail");
        let preview = cx.read(|cx| trek.root.read(cx).preview.clone());

        trek.click(cx, ("attachment", 0usize));
        trek.render(cx);
        let from = preview.read_with(cx, |p, _| p.source()).expect("it knows where it came from");
        // (Inside its 1 px border.)
        assert!((from.origin.x - thumb.origin.x).abs() <= px(1.) && (from.size.width - thumb.size.width).abs() <= px(2.), "{from:?} vs {thumb:?}");
        let opening = trek.bounds(cx, "preview-image").unwrap();
        assert!(opening.size.width < thumb.size.width + px(40.), "it starts at the thumbnail's size: {opening:?}");
        frame(&trek, cx, 1_000);
        let open_at = trek.bounds(cx, "preview-image").unwrap();
        assert!(open_at.size.width > px(600.), "then fills the stage: {open_at:?}");

        // Esc: it shrinks back toward the thumbnail, mounted until it's there.
        trek.press(cx, "escape");
        frame(&trek, cx, 60);
        assert!(preview.read_with(cx, |p, _| !p.is_open() && p.is_mounted()));
        let closing = trek.bounds(cx, "preview-image").unwrap();
        assert!(closing.size.width < open_at.size.width && closing.size.width > thumb.size.width, "{closing:?}");
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
        trek.window(cx, |window, cx| crate::add_agent::open(ws, crate::add_agent::Tab::Command, window, cx));
        frame(&trek, cx, 1_000);
        assert!(trek.visible(cx, "add-agent-sheet"));
        trek.click(cx, "command-cancel");
        trek.render(cx);
        assert!(trek.window(cx, |window, cx| !window.has_active_dialog(cx)), "the dialog is down");
        assert!(trek.visible(cx, "sheet-leaving"), "its sheet is drawn once more as it goes");
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
        let bar = (0..24usize).find(|i| trek.visible(cx, ("basecamp-bar-fill", *i))).expect("a bar for the turn");
        let height = |trek: &Trek, cx: &mut TestAppContext| trek.bounds(cx, ("basecamp-bar-fill", bar)).map_or(px(0.), |b| b.size.height);
        let full = height(&trek, cx);
        assert!(full > px(50.), "the busiest bar near the chart's height: {full:?}");

        // Back to Basecamp later, the chart grows in again.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx));
        trek.render(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        trek.render(cx);
        assert!(height(&trek, cx) < px(3.), "it starts flat");
        frame(&trek, cx, 300);
        let growing = height(&trek, cx);
        assert!(growing > px(0.) && growing < full, "on its way: {growing:?} of {full:?}");
        frame(&trek, cx, 1_000);
        assert_eq!(height(&trek, cx), full);
    });
}
