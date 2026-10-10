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
        let toasts = |trek: &Trek, cx: &mut TestAppContext| trek.window(cx, |window, cx| crate::toast::count(window, cx));
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
fn fill_alpha(trek: &Trek, cx: &mut TestAppContext, b: gpui_kit::Bounds<gpui_kit::Pixels>) -> Option<f32> {
    trek.window(cx, |window, _| {
        let k = window.scale_factor();
        let near = |a: f32, b: f32| (a - b * k).abs() < 1.;
        window
            .painted_quads()
            .iter()
            .filter(|q| near(q.bounds.origin.x.0, b.origin.x.as_f32()) && near(q.bounds.origin.y.0, b.origin.y.as_f32()) && near(q.bounds.size.width.0, b.size.width.as_f32()) && near(q.bounds.size.height.0, b.size.height.as_f32()))
            .filter_map(|q| q.background.as_solid().map(|c| c.a))
            .reduce(f32::max)
    })
}

/// While the window crosses between modes, `b`'s fill (in a cached view on the side that's
/// going) fades with it, frame by frame, as the platform draws them.
fn fades_with_the_crossing(trek: &Trek, cx: &mut TestAppContext, b: gpui_kit::Bounds<gpui_kit::Pixels>, full: f32, going: impl Fn(f32) -> f32) {
    let mut frames = 0;
    for _ in 0..60 {
        live_frame(trek, cx, 16);
        let shown = going(editor_in(trek, cx));
        if shown == 0. {
            break;
        }
        frames += 1;
        let alpha = fill_alpha(trek, cx, b).unwrap_or(0.);
        assert!((alpha - full * shown).abs() < 0.01, "frame {frames}: drawn at {alpha}, the rest of its side at {}", full * shown);
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
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project) }, cx));
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        trek.render(cx);
        // The file's row in the Explorer (a cached view) is highlighted: a fill to follow.
        trek.click(cx, file.display().to_string());
        moving(cx);
        frame(&trek, cx, 1_000);
        let row = trek.bounds(cx, file.display().to_string()).expect("the file's row");
        let full = fill_alpha(&trek, cx, row).expect("its highlight");

        trek.update(cx, |ws, cx| ws.set_mode(Mode::Agents, cx));
        fades_with_the_crossing(&trek, cx, row, full, |e| e);
        // Across: nothing of the editor is drawn, without a refresh to clear it.
        live_frame(&trek, cx, 500);
        live_frame(&trek, cx, 16);
        assert_eq!(fill_alpha(&trek, cx, row), None, "the tree is gone with the rest of the editor");
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
        let other = super::harness::new_project("elsewhere");
        ws.store.ensure_project(&other).unwrap();
        let mut t = ws.store.create_thread(Some(&other), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
        t.title = title.into();
        t.updated_at = now_ms() - older_by;
        t.last_seen_at = t.updated_at;
        ws.store.save_thread(&t).expect("save");
        ws.reload(cx);
        t.id
    })
}

fn pid(trek: &Trek, cx: &TestAppContext, id: &str) -> String {
    trek.read(cx, |ws, _| ws.thread(id).unwrap().project_id.clone().unwrap())
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
        let head = |trek: &Trek, cx: &mut TestAppContext, p: &str| trek.bounds(cx, format!("live-proj-head-{p}")).expect("a header");
        let (ha, hb) = (head(&trek, cx, &pa), head(&trek, cx, &pb));
        assert!(ha.origin.y < hb.origin.y, "the newer project first");
        let row_a = trek.bounds(cx, format!("live-line-{a}")).unwrap();

        // News in `b`: its project goes first. Each group slides as one; the rows in them don't
        // move against their headers.
        bump(&trek, cx, &b);
        trek.render(cx);
        let group = |trek: &Trek, cx: &mut TestAppContext, p: &str| offset(trek, cx, &format!("live-g-{p}")).unwrap();
        assert!(group(&trek, cx, &pb) > 20. && group(&trek, cx, &pa) < -20., "{} {}", group(&trek, cx, &pb), group(&trek, cx, &pa));
        for key in [a.clone(), b.clone(), format!("live-h-{pa}"), format!("live-h-{pb}")] {
            assert_eq!(offset(&trek, cx, &key), Some(0.), "{key} rides with its group");
        }
        assert_eq!(head(&trek, cx, &pb).origin.y, hb.origin.y, "drawn where it was");

        // Passing: `a`'s row isn't painted under `b`'s group, and `b` takes the click there.
        let mut passed = false;
        for _ in 0..50 {
            frame(&trek, cx, 8);
            let (top, row_b) = (head(&trek, cx, &pb), trek.bounds(cx, format!("live-line-{b}")).unwrap());
            let band = (top.origin.y.as_f32(), row_b.bottom().as_f32());
            let k = trek.window(cx, |w, _| w.scale_factor());
            let fills: Vec<(f32, f32, f32, f32)> = trek.window(cx, |w, _| {
                w.painted_quads()
                    .iter()
                    .filter(|q| (q.bounds.size.height.0 - row_a.size.height.as_f32() * k).abs() < 1. && (q.bounds.size.width.0 - row_a.size.width.as_f32() * k).abs() < 1.)
                    .filter(|q| q.background.as_solid().is_some_and(|c| c.a > 0.))
                    .map(|q| (q.bounds.origin.y.0 / k, q.bounds.bottom().0 / k, q.content_mask.bounds.origin.y.0 / k, q.content_mask.bounds.bottom().0 / k))
                    .collect()
            });
            let Some(&(y0, y1, m0, m1)) = fills.first() else { continue };
            for (_, _, m0, m1) in &fills {
                let painted = (m0.max(band.0), m1.min(band.1));
                assert!(painted.1 - painted.0 < 0.5, "`a` painted under `b`'s group: mask {m0}..{m1}, group {band:?}");
            }
            let mid = row_b.center().y.as_f32();
            if !passed && y0 < mid && mid < y1 && band.0 < band.1 {
                assert!(m1 <= band.0 + 0.5 || m0 >= band.1 - 0.5);
                passed = true;
                trek.click(cx, format!("live-line-{b}"));
                assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(b.clone()), "the click lands on the group on top");
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
        let ids: Vec<String> = (0..3).map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i + 1) * 60_000)).collect();
        frame(&trek, cx, 1_000);
        let route = trek.read(cx, |ws, _| ws.route.clone());
        trek.update(cx, |ws, cx| ws.archive(&ids[2], cx));
        frame(&trek, cx, 60);
        assert_eq!(ghosts(&trek, cx), [ids[2].clone()]);
        trek.click(cx, format!("live-line-{}", ids[2]));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), route, "the archived thread doesn't open");

        // The sidebar folding away (⌘B) is out of reach too.
        trek.press(cx, "cmd-b");
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
        trek.press(cx, "cmd-k");
        frame(&trek, cx, 1_000);
        trek.press(cx, "escape");
        frame(&trek, cx, 40);
        assert!(trek.visible(cx, ("palette-row", 0usize)), "still drawn as it leaves");
        trek.click(cx, ("palette-row", 0usize));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), route, "nothing ran");
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
        assert!(preview.read_with(cx, |p, _| !p.is_open()), "and the preview didn't take it");
    });
}

#[test]
fn a_click_on_a_sheet_as_it_leaves_does_nothing() {
    use gpui_kit::test::TestWindowExt as _;
    run(async |cx| {
        let trek = open(cx);
        moving(cx);
        let ws = trek.ws.clone();
        trek.window(cx, |window, cx| crate::add_agent::open(ws, crate::add_agent::Tab::Command, window, cx));
        frame(&trek, cx, 1_000);
        trek.window(cx, |window, cx| {
            // Esc is a real close path: the dialog's on_close runs sheet_left at once, so the
            // leave is registered before the next frame draws. A pointer click on the cancel
            // button is hit-tested against the live tree and a loaded runner has missed it;
            // close_dialog pops the dialog without running on_close.
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(window.find("sheet-leaving").visible(), "the sheet is on its way out");
            // "Add agent" on the empty form would complain; on its way out it does nothing.
            window.click("command-add", cx);
            window.render_frame(cx);
            assert!(!window.try_find("command-error").is_some_and(|e| e.visible()));
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
        cx.update(|cx| crate::root::init(trek.ws.clone(), cx));
        trek.window(cx, |window, _| window.activate_window());
        cx.run_until_parked();
        trek.press(cx, "cmd-e");
        let settled = |trek: &Trek, cx: &mut TestAppContext| trek.read(cx, |ws, _| ws.thread(&id).is_some_and(|t| t.settled_at.is_some()));
        assert!(settled(&trek, cx));
        frame(&trek, cx, 1_000);
        trek.click(cx, "undo");
        assert!(!settled(&trek, cx), "Undo brings it back");
        // Settled again some other way while the toast goes: its Undo is spent.
        trek.update(cx, |ws, cx| {
            ws.store.update_thread(&id, |t| t.settled_at = Some(now_ms())).unwrap();
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
    fn render(&mut self, _: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<Self>) -> impl gpui_kit::IntoElement {
        use gpui_kit::{InteractiveElement as _, StatefulInteractiveElement as _, Styled as _, TestSupportExt as _};
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
    let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.), px(0.)), size: size(px(800.), px(600.)) })), ..Default::default() };
    cx.update(|cx| gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Probe::default()))).expect("probe window")
}

fn in_window<R>(cx: &mut TestAppContext, window: gpui_kit::AnyWindowHandle, f: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App) -> R) -> R {
    let r = gpui_kit::AppContext::update_window(cx, window, |_, window, cx| f(window, cx)).expect("window");
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
    use crate::toast::{Toast, count};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{ScrollDelta, point};
    run(async |cx| {
        let _trek = open(cx);
        moving(cx);
        let (window, probe) = probe_window(cx);
        let opened = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = opened.clone();
        in_window(cx, window, |w, cx| crate::toast::push(w, Toast::new("Finished: Fix the login bug").on_click(move |_, _, _| seen.set(seen.get() + 1)), cx));
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
        assert_eq!(in_window(cx, window, |w, cx| count(w, cx)), 1, "still mounted as it goes");
        assert!(in_window(cx, window, |w, _| w.find("notification").visible()));
        in_window(cx, window, |w, cx| w.hover("notification", cx));
        assert!(probe.read_with(cx, |p, _| p.hovered), "the pointer on it hovers what's under it");
        in_window(cx, window, |w, cx| w.click("notification", cx));
        assert_eq!((opened.get(), probe.read_with(cx, |p, _| p.clicks)), (1, 1), "a click reaches what's under it");
        in_window(cx, window, |w, cx| w.scroll("notification", ScrollDelta::Pixels(point(px(0.), px(-40.))), cx));
        assert_eq!(probe.read_with(cx, |p, _| p.scrolls), 1, "and so does the wheel");
        // Where its close button was: what's under it gets the click.
        in_window(cx, window, |w, cx| w.click_at("notification", close, cx));
        assert_eq!(probe.read_with(cx, |p, _| p.clicks), 2);
        assert!(!in_window(cx, window, |w, _| w.find("toast-close").visible()));
        frame_in(cx, window, 1_000);
        assert_eq!(in_window(cx, window, |w, cx| count(w, cx)), 0, "gone once it has left");
    });
}

#[test]
fn a_live_toast_closes_and_times_out_and_reduce_motion_takes_it_at_once() {
    use crate::toast::{Toast, count};
    use gpui_kit::test::TestWindowExt as _;
    run(async |cx| {
        let _trek = open(cx);
        moving(cx);
        let (window, probe) = probe_window(cx);
        let toasts = |cx: &mut TestAppContext| in_window(cx, window, |w, cx| count(w, cx));
        // Its close button takes it down, and the click goes no further.
        in_window(cx, window, |w, cx| crate::toast::push(w, "Saved", cx));
        frame_in(cx, window, 1_000);
        in_window(cx, window, |w, cx| w.hover("notification", cx));
        in_window(cx, window, |w, cx| w.click("toast-close", cx));
        assert_eq!(probe.read_with(cx, |p, _| p.clicks), 0);
        frame_in(cx, window, 20);
        assert_eq!(toasts(cx), 1, "on its way out");
        frame_in(cx, window, 1_000);
        assert_eq!(toasts(cx), 0);

        // Left alone, it goes once its time is up.
        in_window(cx, window, |w, cx| crate::toast::push(w, Toast::new("Saved").lifetime(Duration::from_secs(3)), cx));
        frame_in(cx, window, 2_900);
        assert_eq!(toasts(cx), 1);
        frame_in(cx, window, 200);
        assert_eq!(toasts(cx), 1, "still drawn as it goes");
        assert!(in_window(cx, window, |w, _| w.find("notification").visible()));
        frame_in(cx, window, 1_000);
        assert_eq!(toasts(cx), 0, "gone after its time");

        // With Reduce Motion it's gone the moment it's taken down.
        cx.update(|cx| cx.set_reduce_motion(true));
        let key = Toast::new_key();
        in_window(cx, window, |w, cx| crate::toast::push(w, Toast::new("Saved").key(key.clone()), cx));
        frame_in(cx, window, 0);
        assert!(in_window(cx, window, |w, _| w.find("notification").visible()), "in at once");
        in_window(cx, window, |w, cx| crate::toast::dismiss(w, &key, cx));
        assert_eq!(toasts(cx), 0, "out at once");
        frame_in(cx, window, 0);
        assert!(in_window(cx, window, |w, _| w.try_find("notification").is_none_or(|t| !t.visible())));
    });
}

#[test]
fn a_click_on_a_toast_as_it_goes_opens_the_row_under_it() {
    use crate::toast::{Toast, count};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Point, point, size};
    run(async |cx| {
        let trek = open(cx);
        // Narrow and short: the toast, centred at the bottom, is over the sidebar's rows.
        cx.simulate_window_resize(trek.window, size(px(600.), px(290.)));
        let ids: Vec<String> = (0..4).map(|i| quiet(&trek, cx, &format!("Thread {i}"), 60_000 * (i + 1))).collect();
        moving(cx);
        let key = Toast::new_key();
        trek.window(cx, |w, cx| crate::toast::push(w, Toast::new("Saved").key(key.clone()), cx));
        frame(&trek, cx, 1_000);
        let face = trek.bounds(cx, "notification").expect("the toast");
        // A row under the toast's left end, and a point on both.
        let (id, at): (String, Point<gpui_kit::Pixels>) = ids
            .iter()
            .find_map(|id| {
                let row = trek.bounds(cx, format!("live-line-{id}"))?;
                let at = point(face.left() + px(24.), row.center().y);
                (row.contains(&at) && face.contains(&at) && (at.y - face.top()).abs() > px(4.)).then(|| (id.clone(), at))
            })
            .unwrap_or_else(|| panic!("no row under the toast at {face:?}: {:?}", ids.iter().map(|id| trek.bounds(cx, format!("live-line-{id}"))).collect::<Vec<_>>()));
        let before = trek.read(cx, |ws, _| ws.route.clone());
        // Up, it covers the row.
        trek.window(cx, |w, cx| w.click_at("notification", at - face.origin, cx));
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), before);
        // On its way out, the row takes the click.
        trek.window(cx, |w, cx| crate::toast::dismiss(w, &key, cx));
        frame(&trek, cx, 30);
        assert_eq!(trek.window(cx, |w, cx| count(w, cx)), 1, "still drawn as it goes");
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
        let title = |trek: &Trek, cx: &mut TestAppContext| trek.bounds(cx, format!("line-{a}")).expect("in the history").origin.y.as_f32();
        let first = title(&trek, cx);
        frame(&trek, cx, 1_000);
        let after = title(&trek, cx);
        assert!((first - before).abs() < 8., "drawn where it was: {before} → {first}");
        assert!(after > first + 20., "then down into the history: {after}");
    });
}
