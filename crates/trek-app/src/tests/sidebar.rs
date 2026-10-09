//! The sidebar's live area: threads that need eyes on them keep their cards; the quiet rest
//! group by project as compact lines, capped until opened.

use super::harness::{Trek, mock, open, run};
use crate::workspace::Route;
use gpui_kit::TestAppContext;
use trek_core::store::now_ms;
use trek_core::{Effort, HandHolding, RunState};

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

fn project_of(trek: &Trek, cx: &TestAppContext, id: &str) -> String {
    trek.read(cx, |ws, _| ws.thread(id).unwrap().project_id.clone().unwrap())
}

#[test]
fn quiet_threads_fold_into_a_project_group_capped_at_three() {
    run(async |cx| {
        let trek = open(cx);
        let ids: Vec<String> = (0..5).map(|i| quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000)).collect();
        trek.render(cx);
        let pid = project_of(&trek, cx, &ids[0]);
        // The newest three stay out; the rest wait behind "Show 2 more". None is a card.
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(trek.visible(cx, format!("live-line-{id}")), i < 3, "quiet {i}");
            assert!(!trek.visible(cx, format!("card-{id}")), "quiet {i} keeps no card");
        }
        assert!(trek.visible(cx, format!("live-proj-head-{pid}")));
        assert!(trek.visible(cx, format!("live-more-{pid}")));
        trek.click(cx, format!("live-more-{pid}"));
        trek.render(cx);
        for id in &ids {
            assert!(trek.visible(cx, format!("live-line-{id}")));
        }
        trek.click(cx, format!("live-less-{pid}"));
        trek.render(cx);
        assert!(!trek.visible(cx, format!("live-line-{}", ids[4])));
    });
}

#[test]
fn what_needs_you_keeps_its_card_over_quiet_lines() {
    run(async |cx| {
        let trek = open(cx);
        for i in 0..4 {
            quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000);
        }
        let asking = quiet(&trek, cx, "Asking", 10_000);
        let working = quiet(&trek, cx, "Working", 20_000);
        trek.update(cx, |ws, cx| {
            ws.store.update_thread(&asking, |t| t.run_state = RunState::NeedsYou).unwrap();
            ws.store.update_thread(&working, |t| t.run_state = RunState::Working).unwrap();
            ws.reload(cx);
        });
        trek.render(cx);
        assert!(trek.visible(cx, format!("card-{asking}")), "the approval keeps its card");
        assert!(trek.visible(cx, format!("card-{working}")), "the running turn keeps its card");
        assert!(!trek.visible(cx, format!("live-line-{asking}")));
        assert!(!trek.visible(cx, format!("live-line-{working}")));
    });
}

#[test]
fn unread_and_open_lines_are_never_hidden() {
    run(async |cx| {
        let trek = open(cx);
        let ids: Vec<String> = (0..5).map(|i| quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000)).collect();
        // The oldest has news: unread lines always show, wherever they'd sort.
        trek.update(cx, |ws, cx| {
            ws.store.update_thread(&ids[4], |t| t.last_seen_at = t.updated_at - 1).unwrap();
            ws.reload(cx);
        });
        // The open thread stays out even though it's seen and old.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(ids[3].clone()), cx));
        trek.render(cx);
        assert!(trek.visible(cx, format!("live-line-{}", ids[4])), "unread is never capped away");
        assert!(trek.visible(cx, format!("live-line-unseen-{}", ids[4])));
        assert!(trek.visible(cx, format!("live-line-{}", ids[3])), "the open thread stays");
        assert!(!trek.visible(cx, format!("live-line-{}", ids[2])), "a seen old one hides");
    });
}

#[test]
fn opening_a_live_group_doesnt_open_the_settled_one() {
    run(async |cx| {
        let trek = open(cx);
        for i in 0..4 {
            quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000);
        }
        let settled = quiet(&trek, cx, "Settled", 5 * 60_000);
        for i in 0..5 {
            let id = quiet(&trek, cx, &format!("Settled {i}"), (6 + i) as i64 * 60_000);
            trek.update(cx, |ws, cx| {
                ws.store.update_thread(&id, |t| t.settled_at = Some(t.updated_at)).unwrap();
                ws.reload(cx);
            });
        }
        trek.update(cx, |ws, cx| {
            ws.store.update_thread(&settled, |t| t.settled_at = Some(t.updated_at)).unwrap();
            ws.reload(cx);
            ws.settled_open = true;
            cx.notify();
        });
        let pid = project_of(&trek, cx, &settled);
        trek.render(cx);
        // Settled shows its five; opening the live group leaves it capped.
        assert!(trek.visible(cx, format!("live-more-{pid}")));
        trek.click(cx, format!("live-more-{pid}"));
        trek.render(cx);
        let settled_ids = trek.read(cx, |ws, _| {
            ws.threads.iter().filter(|t| t.settled_at.is_some() && t.project_id.as_deref() == Some(pid.as_str())).map(|t| t.id.clone()).collect::<Vec<_>>()
        });
        let shown = settled_ids.iter().filter(|id| trek.visible(cx, format!("line-{id}"))).count();
        assert_eq!(shown, 5, "the settled group still caps at five");
    });
}

#[test]
fn folding_a_group_hides_its_rows_but_keeps_its_badges() {
    run(async |cx| {
        let trek = open(cx);
        let ids: Vec<String> = (0..4).map(|i| quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000)).collect();
        let asking = quiet(&trek, cx, "Asking", 10_000);
        trek.update(cx, |ws, cx| {
            ws.store.update_thread(&asking, |t| t.run_state = RunState::NeedsYou).unwrap();
            ws.store.update_thread(&ids[3], |t| t.last_seen_at = t.updated_at - 1).unwrap();
            ws.reload(cx);
        });
        let pid = project_of(&trek, cx, &asking);
        trek.render(cx);
        // The card sits inside its project's group now, with the quiet lines after it.
        assert!(trek.visible(cx, format!("card-{asking}")));
        assert!(trek.visible(cx, format!("live-proj-head-{pid}")));
        trek.click(cx, format!("live-fold-{pid}"));
        trek.render(cx);
        // Folded: every row away, the header still there with its needs-you badge.
        for id in ids.iter().chain([&asking]) {
            assert!(!trek.visible(cx, format!("live-line-{id}")) && !trek.visible(cx, format!("card-{id}")));
        }
        assert!(trek.visible(cx, format!("live-proj-head-{pid}")));
        trek.click(cx, format!("live-fold-{pid}"));
        trek.render(cx);
        assert!(trek.visible(cx, format!("card-{asking}")));
        assert!(trek.visible(cx, format!("live-line-{}", ids[0])));
    });
}

#[test]
fn a_second_project_gets_its_own_group() {
    run(async |cx| {
        let trek = open(cx);
        let here = quiet(&trek, cx, "In this project", 0);
        let elsewhere = trek.update(cx, |ws, cx| {
            let other = super::harness::new_project("elsewhere");
            ws.store.ensure_project(&other).unwrap();
            let mut t = ws.store.create_thread(Some(&other), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            t.title = "Elsewhere".into();
            t.last_seen_at = t.updated_at;
            ws.store.save_thread(&t).unwrap();
            ws.reload(cx);
            t.id
        });
        trek.render(cx);
        for id in [&here, &elsewhere] {
            assert!(trek.visible(cx, format!("live-line-{id}")));
        }
        let (p1, p2) = (project_of(&trek, cx, &here), project_of(&trek, cx, &elsewhere));
        assert_ne!(p1, p2);
        assert!(trek.visible(cx, format!("live-proj-head-{p1}")) && trek.visible(cx, format!("live-proj-head-{p2}")));
    });
}

#[test]
fn a_later_turn_doesnt_retitle_a_thread_whose_first_turn_was_cut_off() {
    run(async |cx| {
        let trek = super::harness::open_with(cx, |s| s.general.auto_title = true);
        // A first turn Trek quit in the middle of: no `TurnEnd`, and the title still the first message.
        let first = "We need permission to migrate";
        let id = trek.update(cx, |ws, cx| {
            let mut t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            t.title = trek_core::import_title(first);
            ws.store.save_thread(&t).expect("save");
            super::harness::store_items(
                &ws.store,
                &t.id,
                vec![
                    trek_core::store::Item::User { text: first.into(), images: vec![], at: Some(now_ms()), resume: None, aside: false },
                    trek_core::store::Item::Notice { text: trek_core::store::INTERRUPTED_BY_QUIT.into() },
                ],
            );
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            t.id
        });
        let before = trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone());
        assert_eq!(trek.send(cx, "mock:long 1ms"), id);
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone()), before, "the second prompt doesn't name the thread");
    });
}

#[test]
fn a_titled_thread_keeps_its_title_after_more_turns() {
    run(async |cx| {
        let trek = super::harness::open_with(cx, |s| s.general.auto_title = true);
        let id = trek.send(cx, "mock:permission please");
        trek.wait_needs_you(cx, &id).await;
        let request = trek.request(cx, &id);
        trek.update(cx, |ws, cx| ws.respond(&id, &request, trek_agents::Decision::Allow, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let titled = trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone());
        assert_eq!(titled, "Apply the schema migrations", "the first turn names it");
        trek.send(cx, "explain the startup");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone()), titled);
    });
}

/// Where `id`'s row is drawn, whichever kind of row it has.
fn row_y(trek: &Trek, cx: &mut TestAppContext, id: &str) -> Option<gpui_kit::Pixels> {
    trek.bounds(cx, format!("card-{id}")).or_else(|| trek.bounds(cx, format!("live-line-{id}"))).map(|b| b.origin.y)
}

fn move_pointer(trek: &Trek, cx: &mut TestAppContext, at: gpui_kit::Point<gpui_kit::Pixels>) {
    trek.window(cx, |window, cx| {
        window.dispatch_event(gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent { position: at, pressed_button: None, modifiers: Default::default() }), cx)
    });
    cx.run_until_parked();
}

#[test]
fn rows_hold_still_under_the_pointer_and_move_once_it_leaves() {
    run(async |cx| {
        let trek = open(cx);
        let quiet_ids: Vec<String> = (0..3).map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i as i64 + 1) * 60_000)).collect();
        // The oldest thread asks: it's on top, the quiet three below it.
        let asking = quiet(&trek, cx, "Asking", 30 * 60_000);
        trek.update(cx, |ws, cx| {
            ws.store.update_thread(&asking, |t| t.run_state = RunState::NeedsYou).unwrap();
            ws.reload(cx);
        });
        trek.render(cx);
        let before: Vec<_> = [&asking].into_iter().chain(&quiet_ids).map(|id| row_y(&trek, cx, id).expect("a row")).collect();
        assert!(before.windows(2).all(|w| w[0] < w[1]), "what asks comes first: {before:?}");

        // The pointer rests on the sidebar; meanwhile the question is answered elsewhere, and
        // another thread starts asking.
        let at = trek.bounds(cx, format!("card-{}", quiet_ids[0])).or_else(|| trek.bounds(cx, format!("live-line-{}", quiet_ids[0]))).unwrap().center();
        move_pointer(&trek, cx, at);
        let newcomer = quiet(&trek, cx, "Newcomer", 5_000);
        trek.update(cx, |ws, cx| {
            ws.store.update_thread(&asking, |t| t.run_state = RunState::Idle).unwrap();
            ws.store.update_thread(&newcomer, |t| t.run_state = RunState::NeedsYou).unwrap();
            ws.reload(cx);
        });
        trek.render(cx);
        let held: Vec<_> = [&asking].into_iter().chain(&quiet_ids).map(|id| row_y(&trek, cx, id).expect("still a row")).collect();
        assert_eq!(held, before, "no row moves under the pointer");
        assert!(row_y(&trek, cx, &newcomer).is_none(), "the newcomer waits for the pointer to leave");
        assert!(trek.visible(cx, format!("live-line-{asking}")), "the answered thread draws as a quiet line, in its place");

        // The pointer leaves for the transcript: the list catches up.
        move_pointer(&trek, cx, gpui_kit::point(gpui_kit::px(900.), gpui_kit::px(400.)));
        trek.render(cx);
        let newcomer_y = row_y(&trek, cx, &newcomer).expect("the newcomer shows");
        assert!(newcomer_y <= before[0], "and comes first");
        assert!(row_y(&trek, cx, &asking).is_none(), "the answered thread, oldest of four quiet ones, folds behind Show more");
    });
}

#[test]
fn a_thread_waiting_on_you_is_a_line_high_and_says_what_it_waits_for() {
    run(async |cx| {
        let trek = open(cx);
        let plain = quiet(&trek, cx, "Plain", 60_000);
        let id = trek.send(cx, "mock:permission please");
        trek.wait_needs_you(cx, &id).await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(plain.clone()), cx));
        trek.render(cx);
        let (row, line) = (trek.bounds(cx, format!("card-{id}")).expect("its row"), trek.bounds(cx, format!("live-line-{plain}")).expect("a quiet line"));
        assert_eq!(row.size.height, line.size.height, "the same height as a quiet line");
        assert!(trek.visible(cx, format!("card-needs-{id}")));
        assert_eq!(trek.read(cx, |ws, _| crate::sidebar::needs_label(ws, &id)), "Needs approval");
    });
}
