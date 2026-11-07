//! The sidebar, inbox first: what needs the user and what finished unread sit in the Inbox under
//! the nav, two lines each; every other thread is a line in its project (at work with its clock),
//! the quiet ones capped until opened. Projects sit under their own label; Study's sessions,
//! other machines and the phone each have a section of their own; every row's count or time ends
//! on one right-hand column.

mod common;

use common::{Trek, mock, open, run};
use gpui_kit::TestAppContext;
use trek_app::workspace::Route;
use trek_core::store::now_ms;
use trek_core::{Effort, HandHolding, RunState};

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

fn project_of(trek: &Trek, cx: &TestAppContext, id: &str) -> String {
    trek.read(cx, |ws, _| {
        ws.thread(id).unwrap().project_id.clone().unwrap()
    })
}

#[test]
fn quiet_threads_fold_into_a_project_group_capped_at_three() {
    run(async |cx| {
        let trek = open(cx);
        let ids: Vec<String> = (0..5)
            .map(|i| quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000))
            .collect();
        trek.render(cx);
        let pid = project_of(&trek, cx, &ids[0]);
        // The newest three stay out; the rest wait behind "Show 2 more". None is a card.
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(
                trek.visible(cx, format!("live-line-{id}")),
                i < 3,
                "quiet {i}"
            );
            assert!(
                !trek.visible(cx, format!("inbox-{id}")),
                "quiet {i} isn't in the Inbox"
            );
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
fn what_needs_you_goes_to_the_inbox_and_what_works_stays_in_its_project() {
    run(async |cx| {
        let trek = open(cx);
        for i in 0..4 {
            quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000);
        }
        let asking = quiet(&trek, cx, "Asking", 10_000);
        let working = quiet(&trek, cx, "Working", 20_000);
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&asking, |t| t.run_state = RunState::NeedsYou)
                .unwrap();
            ws.store
                .update_thread(&working, |t| t.run_state = RunState::Working)
                .unwrap();
            ws.reload(cx);
        });
        trek.render(cx);
        assert!(
            trek.visible(cx, format!("inbox-{asking}")),
            "the approval is in the Inbox"
        );
        assert!(
            !trek.visible(cx, format!("live-line-{asking}")),
            "and out of its project while it's there"
        );
        assert!(
            trek.visible(cx, format!("live-line-{working}")),
            "the running turn stays in its project"
        );
        assert!(
            trek.visible(cx, format!("live-line-working-{working}")),
            "with its clock"
        );
        assert!(!trek.visible(cx, format!("inbox-{working}")));
        // At work, it's on show past the three quiet lines a project shows.
        let pid = project_of(&trek, cx, &working);
        assert!(trek.visible(cx, format!("live-more-{pid}")));
        let head = trek
            .bounds(cx, format!("live-proj-head-{pid}"))
            .unwrap()
            .origin
            .y;
        let row = trek
            .bounds(cx, format!("live-line-{working}"))
            .unwrap()
            .origin
            .y;
        assert!(head < row, "under its project's header");
    });
}

#[test]
fn unread_and_open_lines_are_never_hidden() {
    run(async |cx| {
        let trek = open(cx);
        let ids: Vec<String> = (0..5)
            .map(|i| quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000))
            .collect();
        // The oldest has news: it's in the Inbox, whatever the three a project shows.
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&ids[4], |t| t.last_seen_at = t.updated_at - 1)
                .unwrap();
            ws.reload(cx);
        });
        // The open thread stays out even though it's seen and old.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(ids[3].clone()), cx));
        trek.render(cx);
        assert!(
            trek.visible(cx, format!("inbox-finished-{}", ids[4])),
            "unread is never capped away"
        );
        assert!(
            trek.visible(cx, format!("live-line-{}", ids[3])),
            "the open thread stays"
        );
        assert!(
            !trek.visible(cx, format!("live-line-{}", ids[2])),
            "a seen old one hides"
        );
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
                ws.store
                    .update_thread(&id, |t| t.settled_at = Some(t.updated_at))
                    .unwrap();
                ws.reload(cx);
            });
        }
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&settled, |t| t.settled_at = Some(t.updated_at))
                .unwrap();
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
            ws.threads
                .iter()
                .filter(|t| t.settled_at.is_some() && t.project_id.as_deref() == Some(pid.as_str()))
                .map(|t| t.id.clone())
                .collect::<Vec<_>>()
        });
        let shown = settled_ids
            .iter()
            .filter(|id| trek.visible(cx, format!("line-{id}")))
            .count();
        assert_eq!(shown, 5, "the settled group still caps at five");
    });
}

#[test]
fn folding_a_group_hides_its_rows_but_not_what_waits_on_you() {
    run(async |cx| {
        let trek = open(cx);
        let ids: Vec<String> = (0..4)
            .map(|i| quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000))
            .collect();
        let asking = quiet(&trek, cx, "Asking", 10_000);
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&asking, |t| t.run_state = RunState::NeedsYou)
                .unwrap();
            ws.store
                .update_thread(&ids[3], |t| t.last_seen_at = t.updated_at - 1)
                .unwrap();
            ws.reload(cx);
        });
        let pid = project_of(&trek, cx, &asking);
        trek.render(cx);
        // The row sits in the Inbox, above the projects; its project keeps the quiet lines.
        assert!(trek.visible(cx, format!("inbox-{asking}")));
        assert!(trek.visible(cx, format!("live-proj-head-{pid}")));
        trek.click(cx, format!("live-fold-{pid}"));
        trek.render(cx);
        // Folded: the project's rows away, the header still there, and what asks still asks.
        for id in &ids {
            assert!(!trek.visible(cx, format!("live-line-{id}")));
        }
        assert!(trek.visible(cx, format!("inbox-{asking}")));
        assert!(trek.visible(cx, format!("live-proj-head-{pid}")));
        // What's at work stays in view in a folded project (off the sidebar, so its rows don't
        // hold their places under the pointer).
        move_pointer(
            &trek,
            cx,
            gpui_kit::point(gpui_kit::px(900.), gpui_kit::px(400.)),
        );
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&ids[0], |t| t.run_state = RunState::Working)
                .unwrap();
            ws.reload(cx);
        });
        trek.render(cx);
        assert!(trek.visible(cx, format!("live-line-working-{}", ids[0])));
        assert!(!trek.visible(cx, format!("live-line-{}", ids[1])));
        trek.click(cx, format!("live-fold-{pid}"));
        trek.render(cx);
        assert!(trek.visible(cx, format!("inbox-{asking}")));
        assert!(trek.visible(cx, format!("live-line-{}", ids[0])));
    });
}

#[test]
fn a_second_project_gets_its_own_group() {
    run(async |cx| {
        let trek = open(cx);
        let here = quiet(&trek, cx, "In this project", 0);
        let elsewhere = trek.update(cx, |ws, cx| {
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
        let (p1, p2) = (
            project_of(&trek, cx, &here),
            project_of(&trek, cx, &elsewhere),
        );
        assert_ne!(p1, p2);
        assert!(
            trek.visible(cx, format!("live-proj-head-{p1}"))
                && trek.visible(cx, format!("live-proj-head-{p2}"))
        );
    });
}

#[test]
fn a_later_turn_doesnt_retitle_a_thread_whose_first_turn_was_cut_off() {
    run(async |cx| {
        let trek = common::open_with(cx, |s| s.general.auto_title = true);
        // A first turn Trek quit in the middle of: no `TurnEnd`, and the title still the first message.
        let first = "We need permission to migrate";
        let id = trek.update(cx, |ws, cx| {
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
            t.title = trek_core::import_title(first);
            ws.store.save_thread(&t).expect("save");
            common::store_items(
                &ws.store,
                &t.id,
                vec![
                    trek_core::store::Item::User {
                        text: first.into(),
                        images: vec![],
                        at: Some(now_ms()),
                        resume: None,
                        aside: false,
                    },
                    trek_core::store::Item::Notice {
                        text: trek_core::store::INTERRUPTED_BY_QUIT.into(),
                    },
                ],
            );
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            t.id
        });
        let before = trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone());
        assert_eq!(trek.send(cx, "mock:long 1ms"), id);
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone()),
            before,
            "the second prompt doesn't name the thread"
        );
    });
}

#[test]
fn a_titled_thread_keeps_its_title_after_more_turns() {
    run(async |cx| {
        let trek = common::open_with(cx, |s| s.general.auto_title = true);
        let id = trek.send(cx, "mock:permission please");
        trek.wait_needs_you(cx, &id).await;
        let request = trek.request(cx, &id);
        trek.update(cx, |ws, cx| {
            ws.respond(&id, &request, trek_agents::Decision::Allow, cx)
        });
        trek.wait_done(cx, &id, RunState::Idle).await;
        let titled = trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone());
        assert_eq!(
            titled, "Apply the schema migrations",
            "the first turn names it"
        );
        trek.send(cx, "explain the startup");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone()),
            titled
        );
    });
}

/// Where `id`'s row is drawn, whichever kind of row it has.
fn row_y(trek: &Trek, cx: &mut TestAppContext, id: &str) -> Option<gpui_kit::Pixels> {
    trek.bounds(cx, format!("inbox-{id}"))
        .or_else(|| trek.bounds(cx, format!("live-line-{id}")))
        .map(|b| b.origin.y)
}

fn move_pointer(trek: &Trek, cx: &mut TestAppContext, at: gpui_kit::Point<gpui_kit::Pixels>) {
    trek.window(cx, |window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                position: at,
                pressed_button: None,
                modifiers: Default::default(),
            }),
            cx,
        )
    });
    cx.run_until_parked();
}

#[test]
fn rows_hold_still_under_the_pointer_and_move_once_it_leaves() {
    run(async |cx| {
        let trek = open(cx);
        let quiet_ids: Vec<String> = (0..3)
            .map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i as i64 + 1) * 60_000))
            .collect();
        // The oldest thread asks: it's on top, the quiet three below it.
        let asking = quiet(&trek, cx, "Asking", 30 * 60_000);
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&asking, |t| t.run_state = RunState::NeedsYou)
                .unwrap();
            ws.reload(cx);
        });
        trek.render(cx);
        let before: Vec<_> = [&asking]
            .into_iter()
            .chain(&quiet_ids)
            .map(|id| row_y(&trek, cx, id).expect("a row"))
            .collect();
        assert!(
            before.windows(2).all(|w| w[0] < w[1]),
            "what asks comes first: {before:?}"
        );

        // The pointer rests on the sidebar; meanwhile the question is answered elsewhere, and
        // another thread starts asking.
        let at = trek
            .bounds(cx, format!("inbox-{}", quiet_ids[0]))
            .or_else(|| trek.bounds(cx, format!("live-line-{}", quiet_ids[0])))
            .unwrap()
            .center();
        move_pointer(&trek, cx, at);
        let newcomer = quiet(&trek, cx, "Newcomer", 5_000);
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&asking, |t| t.run_state = RunState::Idle)
                .unwrap();
            ws.store
                .update_thread(&newcomer, |t| t.run_state = RunState::NeedsYou)
                .unwrap();
            ws.reload(cx);
        });
        trek.render(cx);
        let held: Vec<_> = [&asking]
            .into_iter()
            .chain(&quiet_ids)
            .map(|id| row_y(&trek, cx, id).expect("still a row"))
            .collect();
        assert_eq!(held, before, "no row moves under the pointer");
        assert!(
            row_y(&trek, cx, &newcomer).is_none(),
            "the newcomer waits for the pointer to leave"
        );
        assert!(
            trek.visible(cx, format!("inbox-{asking}")),
            "the answered thread stays in the Inbox, in its place"
        );

        // The pointer leaves for the transcript: the list catches up.
        move_pointer(
            &trek,
            cx,
            gpui_kit::point(gpui_kit::px(900.), gpui_kit::px(400.)),
        );
        trek.render(cx);
        let newcomer_y = row_y(&trek, cx, &newcomer).expect("the newcomer shows");
        assert!(newcomer_y <= before[0], "and comes first");
        assert!(
            row_y(&trek, cx, &asking).is_none(),
            "the answered thread, oldest of four quiet ones, folds behind Show more"
        );
    });
}

#[test]
fn a_thread_waiting_on_you_is_two_lines_high_and_says_what_it_waits_for() {
    run(async |cx| {
        let trek = open(cx);
        let plain = quiet(&trek, cx, "Plain", 60_000);
        let id = trek.send(cx, "mock:permission please");
        trek.wait_needs_you(cx, &id).await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(plain.clone()), cx));
        trek.render(cx);
        let (row, line) = (
            trek.bounds(cx, format!("inbox-{id}")).expect("its row"),
            trek.bounds(cx, format!("live-line-{plain}"))
                .expect("a quiet line"),
        );
        assert!(
            row.size.height > line.size.height && row.size.height < line.size.height * 2.,
            "a title and what it waits for: taller than a quiet line, but not two of them"
        );
        assert!(trek.visible(cx, format!("inbox-needs-{id}")));
        assert_eq!(
            trek.read(cx, |ws, _| ws.why_needs_you(&id).map(|w| w.words())),
            Some("Waiting: run ./scripts/migrate.sh --apply?".to_string())
        );
    });
}

#[test]
fn a_pointer_taken_off_the_window_lets_the_held_rows_go() {
    run(async |cx| {
        let trek = open(cx);
        let quiet_ids: Vec<String> = (0..3)
            .map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i as i64 + 1) * 60_000))
            .collect();
        trek.render(cx);
        // The pointer rests on the sidebar and a thread starts asking: its row waits.
        let at = trek
            .bounds(cx, format!("inbox-{}", quiet_ids[0]))
            .or_else(|| trek.bounds(cx, format!("live-line-{}", quiet_ids[0])))
            .unwrap()
            .center();
        move_pointer(&trek, cx, at);
        let newcomer = quiet(&trek, cx, "Newcomer", 5_000);
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&newcomer, |t| t.run_state = RunState::NeedsYou)
                .unwrap();
            ws.reload(cx);
        });
        trek.render(cx);
        assert!(
            row_y(&trek, cx, &newcomer).is_none(),
            "the newcomer waits for the pointer to leave"
        );
        // The pointer goes off the window's edge: Windows says so with no move, and the window takes
        // GPUI's pointer off (`chrome::let_go_of_hover`), as an event off the edge would.
        trek.window(cx, |window, cx| {
            trek_app::chrome::pointer_off_window(window, cx)
        });
        cx.run_until_parked();
        trek.render(cx);
        assert!(
            row_y(&trek, cx, &newcomer).is_some(),
            "the list caught up with it"
        );
        // And the pointer is outside the window now, so the next frame doesn't move it again.
        assert!(
            !trek.window(cx, |window, _| trek_app::chrome::pointer_inside(window)),
            "moved off once, not on every frame"
        );
    });
}

/// A Study session, as `start_study` leaves it: in no project, working in the study folder.
fn study_session(trek: &Trek, cx: &mut TestAppContext, title: &str) -> String {
    trek.update(cx, |ws, cx| {
        let mut t = ws
            .store
            .create_thread(None, mock(), None, Effort::Medium, HandHolding::Auto)
            .expect("thread");
        t.title = title.into();
        t.cwd = Some(trek_core::study::dir());
        t.last_seen_at = t.updated_at;
        ws.store.save_thread(&t).expect("save");
        ws.reload(cx);
        t.id
    })
}

#[test]
fn a_study_session_is_listed_under_study_not_as_a_thread_without_a_project() {
    run(async |cx| {
        let trek = open(cx);
        let session = study_session(&trek, cx, "Learn · the chain rule");
        let loose = trek.update(cx, |ws, cx| {
            let mut t = ws
                .store
                .create_thread(None, mock(), None, Effort::Medium, HandHolding::Auto)
                .expect("thread");
            t.title = "Name ideas for the CLI".into();
            t.last_seen_at = t.updated_at;
            ws.store.save_thread(&t).unwrap();
            ws.reload(cx);
            t.id
        });
        trek.render(cx);
        assert!(trek.visible(cx, "study-sessions"), "Study has its section");
        assert!(trek.visible(cx, format!("live-line-{session}")));
        // The thread with no project keeps its group; the session isn't in it.
        assert!(trek.visible(cx, format!("live-line-{loose}")));
        assert!(trek.visible(cx, "live-proj-head-"));
        let mut under = |id: &str| {
            trek.bounds(cx, format!("live-line-{id}"))
                .map(|b| b.origin.y)
                .unwrap()
        };
        let (study_y, loose_y) = (under(&session), under(&loose));
        let study_head = trek.bounds(cx, "study-sessions").unwrap().origin.y;
        assert!(
            loose_y < study_head && study_head < study_y,
            "the session sits under Study, after the projects"
        );
        // It's one: its row is the topic, the kind at its end.
        let title = trek.read(cx, |ws, _| ws.thread(&session).unwrap().title.clone());
        assert_eq!(
            title, "Learn · the chain rule",
            "the thread keeps its title"
        );
    });
}

#[test]
fn projects_sit_under_their_own_label_after_the_inbox() {
    run(async |cx| {
        let trek = open(cx);
        let quiet_id = quiet(&trek, cx, "Quiet", 60_000);
        let working = quiet(&trek, cx, "Working", 10_000);
        let asking = quiet(&trek, cx, "Asking", 20_000);
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&working, |t| t.run_state = RunState::Working)
                .unwrap();
            ws.store
                .update_thread(&asking, |t| t.run_state = RunState::NeedsYou)
                .unwrap();
            ws.reload(cx);
        });
        trek.render(cx);
        let pid = project_of(&trek, cx, &quiet_id);
        let mut y = |id: String| trek.bounds(cx, id).map(|b| b.origin.y).unwrap();
        let (inbox, label, head, row) = (
            y(format!("inbox-{asking}")),
            y("section-projects".to_string()),
            y(format!("live-proj-head-{pid}")),
            y(format!("live-line-{working}")),
        );
        assert!(
            inbox < label && label < head && head < row,
            "the Inbox, the Projects label, the project, its thread at work: {inbox:?} {label:?} {head:?} {row:?}"
        );
        assert!(!trek.visible(cx, "section-active"), "no Active section");
    });
}

#[test]
fn a_paired_phone_is_a_device_that_opens_its_settings() {
    run(async |cx| {
        let trek = open(cx);
        quiet(&trek, cx, "Quiet", 60_000);
        trek.render(cx);
        assert!(!trek.visible(cx, "devices"), "no devices, no section");
        trek.update(cx, |ws, cx| {
            ws.start_test_remote();
            let remote = ws.remote.as_mut().unwrap();
            remote.devices.push(trek_remote::DeviceInfo {
                device_id: "phone".into(),
                name: "Test iPhone".into(),
                paired_at: now_ms() - 86_400_000,
                last_seen_at: None,
                app_version: None,
                platform: Some("ios".into()),
            });
            // A machine paired the other way round isn't a phone.
            remote.devices.push(trek_remote::DeviceInfo {
                device_id: "mac".into(),
                name: "Studio Mac".into(),
                paired_at: now_ms(),
                last_seen_at: None,
                app_version: None,
                platform: Some("macos".into()),
            });
            cx.notify();
        });
        trek.render(cx);
        assert!(trek.visible(cx, "devices"));
        assert!(trek.visible(cx, "phone-phone"));
        assert!(!trek.visible(cx, "phone-mac"));
        trek.click(cx, "phone-phone");
        trek.render(cx);
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Settings(trek_app::workspace::SettingsPage::Mobile)
        );
    });
}

#[test]
fn an_empty_inbox_and_an_empty_search_each_say_so() {
    run(async |cx| {
        let trek = open(cx);
        trek.render(cx);
        assert!(trek.visible(cx, "inbox-zero"));
        quiet(&trek, cx, "Profile the sidebar", 60_000);
        trek.render(cx);
        assert!(!trek.visible(cx, "inbox-zero"));
        trek.update(cx, |ws, cx| ws.set_search("zzqx nothing".into(), cx));
        trek.render(cx);
        assert!(trek.visible(cx, "search-empty"));
        trek.update(cx, |ws, cx| ws.set_search(String::new(), cx));
        trek.render(cx);
        assert!(!trek.visible(cx, "search-empty"));
    });
}

/// `id` asks for an approval, has failed, or has finished unread.
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

fn y(trek: &Trek, cx: &mut TestAppContext, id: String) -> gpui_kit::Pixels {
    trek.bounds(cx, id.clone())
        .unwrap_or_else(|| panic!("{id} isn't drawn"))
        .origin
        .y
}

#[test]
fn the_inbox_lists_what_needs_you_then_what_finished_and_is_gone_when_empty() {
    run(async |cx| {
        let trek = open(cx);
        let plain = quiet(&trek, cx, "Plain", 300_000);
        trek.render(cx);
        assert!(!trek.visible(cx, "inbox"), "nothing in it: no Inbox at all");

        // Newest first would put the finished one on top: what needs you comes first anyway.
        let finished = quiet(&trek, cx, "Finished", 10_000);
        let failed = quiet(&trek, cx, "Failed", 20_000);
        let asking = quiet(&trek, cx, "Asking", 30_000);
        set_state(&trek, cx, &finished, RunState::Idle, true);
        set_state(&trek, cx, &failed, RunState::Failed, false);
        set_state(&trek, cx, &asking, RunState::NeedsYou, false);
        trek.render(cx);
        assert!(trek.visible(cx, "inbox"));
        let ys = [&asking, &failed, &finished].map(|id| y(&trek, cx, format!("inbox-{id}")));
        assert!(ys.windows(2).all(|p| p[0] < p[1]), "{ys:?}");
        // Each says why in its alert's words, and where it's from.
        assert!(trek.visible(cx, format!("inbox-needs-{asking}")));
        assert!(trek.visible(cx, format!("inbox-failed-{failed}")));
        assert!(trek.visible(cx, format!("inbox-finished-{finished}")));
        for id in [&asking, &failed, &finished] {
            assert!(
                !trek.visible(cx, format!("live-line-{id}")),
                "in the Inbox, out of its project"
            );
        }
        // Above the projects.
        let pid = project_of(&trek, cx, &plain);
        assert!(ys[2] < y(&trek, cx, format!("live-proj-head-{pid}")));

        // Answered, settled and read, it's gone again.
        set_state(&trek, cx, &asking, RunState::Idle, false);
        trek.update(cx, |ws, cx| {
            ws.settle(&failed, cx);
            ws.navigate(Route::Thread(finished.clone()), cx);
        });
        trek.render(cx);
        assert!(!trek.visible(cx, "inbox"));
        assert!(trek.visible(cx, format!("live-line-{finished}")));
    });
}

#[test]
fn mark_all_read_reads_what_finished_and_leaves_what_needs_you() {
    run(async |cx| {
        let trek = open(cx);
        let done: Vec<String> = (0..2)
            .map(|i| quiet(&trek, cx, &format!("Done {i}"), (i + 1) * 60_000))
            .collect();
        let asking = quiet(&trek, cx, "Asking", 10_000);
        for id in &done {
            set_state(&trek, cx, id, RunState::Idle, true);
        }
        set_state(&trek, cx, &asking, RunState::NeedsYou, false);
        trek.render(cx);
        assert!(trek.visible(cx, "inbox-mark-read"));
        trek.click(cx, "inbox-mark-read");
        trek.render(cx);
        // What the user did here shows at once, under the pointer or not.
        for id in &done {
            assert!(!trek.read(cx, |ws, _| ws.thread(id).unwrap().is_unseen()));
            assert!(!trek.visible(cx, format!("inbox-{id}")));
            assert!(
                trek.visible(cx, format!("live-line-{id}")),
                "back in its project"
            );
        }
        assert!(
            trek.visible(cx, format!("inbox-{asking}")),
            "what needs you stays"
        );
        assert!(
            !trek.visible(cx, "inbox-mark-read"),
            "nothing left for it to read"
        );
    });
}

#[test]
fn opening_a_finished_thread_reads_it_and_sends_it_back_to_its_project() {
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx, "Done", 60_000);
        quiet(&trek, cx, "Other", 120_000);
        set_state(&trek, cx, &id, RunState::Idle, true);
        trek.render(cx);
        let at = y(&trek, cx, format!("inbox-{id}"));
        trek.click(cx, format!("inbox-{id}"));
        trek.render(cx);
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(id.clone())
        );
        assert!(!trek.read(cx, |ws, _| ws.thread(&id).unwrap().is_unseen()));
        // Under the pointer, it holds its place.
        assert_eq!(y(&trek, cx, format!("inbox-{id}")), at);
        move_pointer(
            &trek,
            cx,
            gpui_kit::point(gpui_kit::px(900.), gpui_kit::px(400.)),
        );
        trek.render(cx);
        assert!(!trek.visible(cx, "inbox"), "the Inbox had only it");
        assert!(trek.visible(cx, format!("live-line-{id}")));
    });
}

#[test]
fn a_pinned_thread_stays_on_top_of_its_project_with_its_pin() {
    run(async |cx| {
        let trek = open(cx);
        let ids: Vec<String> = (0..5)
            .map(|i| quiet(&trek, cx, &format!("Quiet {i}"), (i + 1) * 60_000))
            .collect();
        trek.update(cx, |ws, cx| ws.toggle_pin(&ids[4], cx));
        trek.render(cx);
        assert!(trek.visible(cx, format!("live-line-pinned-{}", ids[4])));
        assert!(!trek.visible(cx, "section-pinned"), "no section of its own");
        assert!(
            y(&trek, cx, format!("live-line-{}", ids[4]))
                < y(&trek, cx, format!("live-line-{}", ids[0])),
            "first in its project, the oldest though it is"
        );
        // And on show past the three quiet ones.
        assert!(trek.visible(cx, format!("live-line-{}", ids[2])));
        assert!(!trek.visible(cx, format!("live-line-{}", ids[3])));
    });
}

#[test]
fn counts_times_and_devices_end_on_one_right_hand_column() {
    run(async |cx| {
        let trek = open(cx);
        let read = quiet(&trek, cx, "Read", 60_000);
        let working = quiet(&trek, cx, "Working", 20_000);
        let asking = quiet(&trek, cx, "Asking", 30_000);
        let done = quiet(&trek, cx, "Done", 40_000);
        let settled = quiet(&trek, cx, "Settled", 90_000);
        set_state(&trek, cx, &working, RunState::Working, false);
        set_state(&trek, cx, &asking, RunState::NeedsYou, false);
        set_state(&trek, cx, &done, RunState::Idle, true);
        trek.update(cx, |ws, cx| {
            ws.settle(&settled, cx);
            ws.settled_open = true;
            ws.start_test_remote();
            let remote = ws.remote.as_mut().unwrap();
            remote.devices.push(trek_remote::DeviceInfo {
                device_id: "phone".into(),
                name: "Test iPhone".into(),
                paired_at: now_ms() - 86_400_000,
                last_seen_at: Some(now_ms() - 120_000),
                app_version: None,
                platform: Some("ios".into()),
            });
            cx.notify();
        });
        trek.render(cx);
        let pid = project_of(&trek, cx, &read);
        let column: Vec<(String, f32)> = [
            format!("live-proj-count-{pid}"),
            format!("live-line-time-{read}"),
            format!("live-line-working-{working}"),
            format!("inbox-time-{asking}"),
            "inbox-mark-read".to_string(),
            "phone-seen-phone".to_string(),
            "settled-count".to_string(),
            format!("settled-proj-count-{pid}"),
            format!("line-time-{settled}"),
        ]
        .into_iter()
        .map(|id| {
            let right = trek
                .bounds(cx, id.clone())
                .unwrap_or_else(|| panic!("{id} isn't drawn"))
                .right()
                .as_f32();
            (id, right)
        })
        .collect();
        let edge = column[0].1;
        for (id, right) in &column {
            assert!(
                (right - edge).abs() < 0.5,
                "{id} ends at {right}, the column at {edge}: {column:?}"
            );
        }
        // On a project's header, "New thread here" lies over the count: nothing moves for it.
        let head = trek.bounds(cx, format!("live-proj-head-{pid}")).unwrap();
        move_pointer(&trek, cx, head.center());
        trek.render(cx);
        assert!(trek.visible(cx, format!("live-proj-new-{pid}")));
        let right = trek
            .bounds(cx, format!("live-line-time-{read}"))
            .unwrap()
            .right()
            .as_f32();
        assert!((right - edge).abs() < 0.5);
    });
}
