//! "Needs you": what waits on the user sits on top of the sidebar and says why, two keys go
//! through it, and a thread's state stays true after an agent's crash, a stop and a restart.

mod common;

use common::{Trek, mock, new_project, open, run, settings, toasts};
use gpui_kit::{AppContext as _, TestAppContext};
use trek_agents::{AgentEvent, Decision};
use trek_app::keys::{self, Id};
use trek_app::workspace::attention::Why;
use trek_app::workspace::{Route, Workspace};
use trek_core::store::{Item, Store, now_ms};
use trek_core::{Effort, HandHolding, RunState};

/// The sidebar's id for its "Needs you" group, in the ids of its footer.
const GROUP: &str = "\u{0}needs";

/// A seen thread in the harness project (not on screen), `older_by` ms old.
fn thread(trek: &Trek, cx: &mut TestAppContext, title: &str, older_by: i64) -> String {
    let project = trek.project.clone();
    thread_in(trek, cx, &project, title, older_by)
}

fn thread_in(
    trek: &Trek,
    cx: &mut TestAppContext,
    project: &std::path::Path,
    title: &str,
    older_by: i64,
) -> String {
    trek.update(cx, |ws, cx| {
        ws.store.ensure_project(project).expect("project");
        let mut t = ws
            .store
            .create_thread(
                Some(project),
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

fn ask(trek: &Trek, cx: &mut TestAppContext, id: &str, command: &str) {
    let event = AgentEvent::PermissionRequest {
        request_id: format!("{id}-ask"),
        title: "Run command".into(),
        detail: command.into(),
        prompt: None,
        call: None,
    };
    trek.update(cx, |ws, cx| ws.apply_events(id, vec![event], cx));
}

fn end_turn(trek: &Trek, cx: &mut TestAppContext, id: &str, error: Option<&str>) {
    let events = vec![
        AgentEvent::TextDelta("Done.".into()),
        AgentEvent::TurnComplete {
            error: error.map(String::from),
        },
    ];
    trek.update(cx, |ws, cx| ws.apply_events(id, events, cx));
}

fn why(trek: &Trek, cx: &TestAppContext, id: &str) -> Option<String> {
    trek.read(cx, |ws, _| ws.why_needs_you(id).map(|w| w.words()))
}

fn y_of(trek: &Trek, cx: &mut TestAppContext, id: &str) -> gpui_kit::Pixels {
    trek.bounds(cx, format!("card-{id}"))
        .unwrap_or_else(|| panic!("no row for {id}"))
        .origin
        .y
}

fn route(trek: &Trek, cx: &TestAppContext) -> Route {
    trek.read(cx, |ws, _| ws.route.clone())
}

fn on_thread(id: &str) -> Route {
    Route::Thread(id.to_string())
}

/// The keys, as the app handles them wherever the focus is (`main` installs this).
fn with_keys(cx: &mut TestAppContext) {
    cx.update(trek_app::app_actions);
}

#[test]
fn what_waits_on_you_sits_on_top_says_why_and_takes_no_room_otherwise() {
    run(async |cx| {
        let trek = open(cx);
        let quiet = thread(&trek, cx, "Quiet", 3_600_000);
        trek.render(cx);
        assert!(
            !trek.visible(cx, "needs-you"),
            "nothing needs you: no heading, no room"
        );

        // An approval in a project folded away, a failure, one finished unread, one pinned.
        let elsewhere = new_project("elsewhere");
        let asking = thread_in(&trek, cx, &elsewhere, "E2E tests", 60_000);
        let also_there = thread_in(&trek, cx, &elsewhere, "Also there", 60_000);
        let failed = thread(&trek, cx, "Nightly export", 120_000);
        let finished = thread(&trek, cx, "Fix the docs", 180_000);
        let pinned = thread(&trek, cx, "Pinned one", 240_000);
        trek.update(cx, |ws, cx| ws.toggle_pin(&pinned, cx));
        trek.render(cx);
        let there = trek.read(cx, |ws, _| {
            ws.thread(&asking).unwrap().project_id.clone().unwrap()
        });
        trek.click(cx, format!("live-fold-{there}"));
        // Off the sidebar: under the pointer, its rows hold their places.
        trek.window(cx, |window, cx| {
            window.dispatch_event(
                gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                    position: gpui_kit::point(gpui_kit::px(900.), gpui_kit::px(400.)),
                    pressed_button: None,
                    modifiers: Default::default(),
                }),
                cx,
            )
        });
        ask(&trek, cx, &asking, "cd /work/atlas && cargo test");
        ask(&trek, cx, &pinned, "make deploy");
        end_turn(&trek, cx, &failed, Some("Rate limit reached\nretry later"));
        end_turn(&trek, cx, &finished, None);
        trek.render(cx);

        assert!(trek.visible(cx, "needs-you"));
        for id in [&asking, &pinned, &failed] {
            assert!(trek.visible(cx, format!("card-{id}")), "{id} is listed");
        }
        // What only finished isn't waiting on anyone: it stays in its project, with its dot.
        assert!(!trek.visible(cx, format!("card-{finished}")));
        assert!(trek.visible(cx, format!("live-line-unseen-{finished}")));
        assert!(
            !trek.visible(cx, format!("live-line-{also_there}")),
            "the folded project's quiet thread stays folded"
        );
        // What's asking, newest first; then the failure.
        let (ys, quiet_y) = (
            [&asking, &pinned, &failed].map(|id| y_of(&trek, cx, id)),
            trek.bounds(cx, format!("live-line-{quiet}"))
                .unwrap()
                .origin
                .y,
        );
        assert!(ys.windows(2).all(|p| p[0] < p[1]), "{ys:?}");
        assert!(ys[2] < quiet_y, "above the projects");

        // Each says why, in the words its alert uses.
        assert_eq!(
            why(&trek, cx, &asking).as_deref(),
            Some("Waiting: run cargo test?")
        );
        assert_eq!(
            why(&trek, cx, &failed).as_deref(),
            Some("Failed: Rate limit reached")
        );
        assert_eq!(why(&trek, cx, &finished).as_deref(), Some("Finished"));
        assert_eq!(why(&trek, cx, &quiet), None);
        assert!(trek.visible(cx, format!("card-needs-{asking}")));
        assert!(trek.visible(cx, format!("card-failed-{failed}")));

        // Answered, settled and read, the group is gone; the pinned thread goes back to Pinned.
        for id in [&asking, &pinned] {
            let request = format!("{id}-ask");
            trek.update(cx, |ws, cx| {
                ws.respond(id, &request, Decision::Allow, cx);
                ws.apply_events(id, vec![AgentEvent::TurnComplete { error: None }], cx);
                ws.mark_all_read(cx);
            });
        }
        trek.update(cx, |ws, cx| {
            ws.settle(&failed, cx);
            ws.mark_all_read(cx);
        });
        trek.render(cx);
        assert!(!trek.visible(cx, "needs-you"));
        assert!(trek.visible(cx, format!("card-{pinned}")));
        assert_eq!(trek.read(cx, |ws, _| ws.needing_you().len()), 0);
    });
}

#[test]
fn a_finished_thread_waits_in_its_project_with_its_dot_till_it_is_opened() {
    run(async |cx| {
        let trek = open(cx);
        let done = thread(&trek, cx, "Done", 60_000);
        end_turn(&trek, cx, &done, None);
        trek.render(cx);
        assert!(!trek.visible(cx, "needs-you"));
        assert!(!trek.visible(cx, format!("card-finished-{done}")));
        assert!(trek.visible(cx, format!("live-line-unseen-{done}")));
        trek.update(cx, |ws, cx| ws.navigate(on_thread(&done), cx));
        trek.render(cx);
        assert!(trek.visible(cx, format!("live-line-{done}")));
        assert!(!trek.visible(cx, format!("live-line-unseen-{done}")));
    });
}

#[test]
fn nothing_waiting_ever_folds_and_a_run_of_finished_threads_stays_in_its_project() {
    run(async |cx| {
        let trek = open(cx);
        let finished: Vec<String> = (0..5)
            .map(|i| thread(&trek, cx, &format!("Done {i}"), i as i64 * 60_000 + 60_000))
            .collect();
        for id in &finished {
            end_turn(&trek, cx, id, None);
        }
        let asking: Vec<String> = (0..5)
            .map(|i| {
                thread(
                    &trek,
                    cx,
                    &format!("Asking {i}"),
                    i as i64 * 60_000 + 60_000,
                )
            })
            .collect();
        for id in &asking {
            ask(&trek, cx, id, "cargo test");
        }
        trek.render(cx);
        for id in &asking {
            assert!(trek.visible(cx, format!("card-needs-{id}")));
        }
        assert!(!trek.visible(cx, format!("live-more-{GROUP}")));
        // Unread, every one shows in its project, past the three a project shows of the rest.
        for id in &finished {
            assert!(!trek.visible(cx, format!("card-finished-{id}")));
            assert!(trek.visible(cx, format!("live-line-unseen-{id}")));
        }
    });
}

#[test]
fn the_row_says_what_the_banner_said() {
    run(async |cx| {
        let trek = open(cx);
        let said = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let sink = said.clone();
        cx.update(|cx| {
            cx.subscribe(
                &trek.ws,
                move |_, event: &trek_app::workspace::WorkspaceEvent, _| {
                    if let trek_app::workspace::WorkspaceEvent::Attention { message, .. } = event {
                        sink.borrow_mut().push(message.clone());
                    }
                },
            )
            .detach()
        });
        let id = thread(&trek, cx, "Fix the login bug", 60_000);
        ask(&trek, cx, &id, "make deploy");
        let row = why(&trek, cx, &id).unwrap();
        assert_eq!(row, "Waiting: run make deploy?");
        assert_eq!(
            said.borrow().last().map(String::as_str),
            Some("Waiting: run make deploy? · Fix the login bug")
        );
    });
}

#[test]
fn the_jump_key_goes_through_what_needs_you_and_round() {
    run(async |cx| {
        let trek = open(cx);
        with_keys(cx);
        let said = toasts(&trek, cx);
        let next = keys::keystroke(Id::NextNeedsYou);

        // Nothing needs you: it says so, and stays.
        let start = route(&trek, cx);
        trek.press(cx, &next);
        assert_eq!(route(&trek, cx), start);
        assert_eq!(said.borrow().last().unwrap().0, "Nothing needs you.");

        let first = thread(&trek, cx, "First", 60_000);
        let second = thread(&trek, cx, "Second", 120_000);
        let failed = thread(&trek, cx, "Failed", 180_000);
        let done = thread(&trek, cx, "Done", 240_000);
        let later = thread(&trek, cx, "Later", 300_000);
        ask(&trek, cx, &first, "one");
        ask(&trek, cx, &second, "two");
        end_turn(&trek, cx, &failed, Some("boom"));
        end_turn(&trek, cx, &later, None);
        end_turn(&trek, cx, &done, None);

        // In the order of the list. Each `apply_events` made its thread the newest.
        let order: Vec<String> = trek.read(cx, |ws, _| {
            ws.needing_you().iter().map(|(t, _)| t.id.clone()).collect()
        });
        assert_eq!(order.len(), 5);
        assert_eq!(&order[2], &failed);
        for id in &order[..3] {
            trek.press(cx, &next);
            assert_eq!(route(&trek, cx), on_thread(id));
        }
        // The two finished ones leave the list as they're opened: the key carries on to the next
        // rather than starting over.
        trek.press(cx, &next);
        assert_eq!(route(&trek, cx), on_thread(&order[3]));
        trek.press(cx, &next);
        assert_eq!(route(&trek, cx), on_thread(&order[4]));
        // And round: what still waits comes up again.
        trek.press(cx, &next);
        assert_eq!(route(&trek, cx), on_thread(&order[0]));
        trek.press(cx, &next);
        assert_eq!(route(&trek, cx), on_thread(&order[1]));

        // Down to the one on screen: it says so.
        let (a, b) = (order[0].clone(), order[1].clone());
        trek.update(cx, |ws, cx| {
            ws.respond(&a, &format!("{a}-ask"), Decision::Allow, cx);
            ws.apply_events(&a, vec![AgentEvent::TurnComplete { error: None }], cx);
            ws.settle(&failed, cx);
            ws.mark_all_read(cx);
        });
        assert_eq!(route(&trek, cx), on_thread(&b));
        trek.press(cx, &next);
        assert_eq!(route(&trek, cx), on_thread(&b));
        assert_eq!(
            said.borrow().last().unwrap().0,
            "Only this thread needs you."
        );
    });
}

#[test]
fn the_jump_key_counts_from_the_thread_in_its_own_window() {
    run(async |cx| {
        let trek = open(cx);
        with_keys(cx);
        let (a, b) = (
            thread(&trek, cx, "A", 60_000),
            thread(&trek, cx, "B", 120_000),
        );
        ask(&trek, cx, &b, "two");
        ask(&trek, cx, &a, "one");
        let order: Vec<String> = trek.read(cx, |ws, _| {
            ws.needing_you().iter().map(|(t, _)| t.id.clone()).collect()
        });
        assert_eq!(order, [a.clone(), b.clone()]);
        // `a` has a window of its own, in front: from there the next is `b`, not the first on the
        // list, whatever the main window shows.
        let own = trek.open_thread_window(cx, &a);
        own.update(cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.run_until_parked();
        cx.update(trek_app::root::jump_to_needs_you);
        cx.run_until_parked();
        assert_eq!(route(&trek, cx), on_thread(&b));
    });
}

#[test]
fn the_other_key_goes_back_to_where_you_last_sent_a_prompt() {
    run(async |cx| {
        let trek = open(cx);
        with_keys(cx);
        let said = toasts(&trek, cx);
        let back = keys::keystroke(Id::BackToLastPrompt);
        trek.press(cx, &back);
        assert_eq!(
            said.borrow().last().unwrap().0,
            "No prompt sent since Trek opened."
        );

        let other = thread(&trek, cx, "Other", 60_000);
        let id = trek.send(cx, "hello there");
        // An agent or a queued follow-up sending to a thread isn't the user's prompt.
        trek.update(cx, |ws, cx| {
            ws.send_to(&other, "from an agent".into(), vec![], cx)
        });
        trek.update(cx, |ws, cx| ws.navigate(on_thread(&other), cx));
        trek.press(cx, &back);
        assert_eq!(route(&trek, cx), on_thread(&id));
        trek.press(cx, &back);
        assert_eq!(route(&trek, cx), on_thread(&id));
        assert_eq!(
            said.borrow().last().unwrap().0,
            "You last sent a prompt here."
        );

        // Archived since, it's gone.
        trek.update(cx, |ws, cx| ws.archive(&id, cx));
        trek.press(cx, &back);
        assert_eq!(
            said.borrow().last().unwrap().0,
            "The thread you last sent a prompt to is gone."
        );
    });
}

#[test]
fn a_session_that_dies_on_a_stop_is_stopped_not_failed() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let start = AgentEvent::TextDelta("Working on it.".into());
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![start.clone()], cx));
        assert_eq!(trek.run_state(cx, &id), RunState::Working);
        trek.update(cx, |ws, cx| {
            ws.interrupt(&id, cx);
            ws.apply_events(&id, vec![AgentEvent::Exited], cx)
        });
        assert_eq!(trek.run_state(cx, &id), RunState::Idle);
        let items = trek.items(cx, &id);
        assert!(
            !items.iter().any(|i| matches!(i, Item::Error { .. })),
            "{items:?}"
        );
        assert!(matches!(items.last(), Some(Item::Notice { text }) if text == "Interrupted"));
        assert_eq!(why(&trek, cx, &id), None);

        // The same for an agent that words its stop as an error.
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![start.clone()], cx));
        trek.update(cx, |ws, cx| {
            ws.interrupt(&id, cx);
            ws.apply_events(
                &id,
                vec![AgentEvent::TurnComplete {
                    error: Some("exit status 130".into()),
                }],
                cx,
            )
        });
        assert_eq!(trek.run_state(cx, &id), RunState::Idle);

        // A later turn's failure is still one: the stop was for that turn.
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![start], cx));
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::Exited], cx)
        });
        assert_eq!(trek.run_state(cx, &id), RunState::Failed);
        assert_eq!(
            why(&trek, cx, &id).as_deref(),
            Some("Failed: The agent stopped unexpectedly.")
        );
    });
}

#[test]
fn a_stop_from_elsewhere_is_no_news() {
    run(async |cx| {
        let trek = open(cx);
        let id = thread(&trek, cx, "Stopped from the phone", 60_000);
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::TextDelta("Hm.".into())], cx);
            ws.interrupt(&id, cx);
            ws.apply_events(
                &id,
                vec![AgentEvent::TurnComplete {
                    error: Some("Interrupted".into()),
                }],
                cx,
            );
        });
        assert_eq!(trek.run_state(cx, &id), RunState::Idle);
        assert_eq!(
            why(&trek, cx, &id),
            None,
            "the user stopped it: not \"Finished\""
        );
    });
}

#[test]
fn a_session_gone_with_no_turn_open_leaves_nothing_marked_running() {
    run(async |cx| {
        let trek = open(cx);
        // A live session, its turn over; then it offers a question after the turn.
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let question = trek_agents::Question {
            question: "Which region first?".into(),
            header: "Region".into(),
            options: vec![("EU".into(), String::new())],
            multi: false,
            secret: false,
        };
        let asked = AgentEvent::PermissionRequest {
            request_id: "q1".into(),
            title: "Question".into(),
            detail: String::new(),
            prompt: Some(trek_agents::Prompt::Questions(vec![question.clone()])),
            call: None,
        };
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![asked], cx));
        assert_eq!(trek.run_state(cx, &id), RunState::NeedsYou);
        assert_eq!(
            why(&trek, cx, &id).as_deref(),
            Some("Waiting: Which region first?")
        );
        // Answered, it's marked at work ahead of the agent's first word...
        trek.update(cx, |ws, cx| {
            ws.answer(
                &id,
                "q1",
                vec![("Which region first?".into(), "EU".into())],
                cx,
            )
        });
        assert_eq!(trek.run_state(cx, &id), RunState::Working);
        // ...and then the session dies with no turn open: nothing is running.
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::Exited], cx)
        });
        assert_eq!(trek.run_state(cx, &id), RunState::Idle);
    });
}

#[test]
fn a_stop_between_the_answer_and_the_agents_first_word_ends_it() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let asked = AgentEvent::PermissionRequest {
            request_id: "q1".into(),
            title: "Question".into(),
            detail: String::new(),
            prompt: Some(trek_agents::Prompt::Questions(vec![
                trek_agents::Question {
                    question: "Which?".into(),
                    header: "Which".into(),
                    options: vec![("One".into(), String::new())],
                    multi: false,
                    secret: false,
                },
            ])),
            call: None,
        };
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![asked], cx);
            ws.answer(&id, "q1", vec![("Which?".into(), "One".into())], cx);
        });
        assert_eq!(trek.run_state(cx, &id), RunState::Working);
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        assert_eq!(trek.run_state(cx, &id), RunState::Idle);
    });
}

#[test]
fn an_agents_crash_with_a_card_up_fails_the_thread_and_clears_the_card() {
    run(async |cx| {
        let trek = open(cx);
        let id = thread(&trek, cx, "Crashes", 60_000);
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::TextDelta("On it.".into())], cx)
        });
        ask(&trek, cx, &id, "make deploy");
        assert_eq!(
            why(&trek, cx, &id).as_deref(),
            Some("Waiting: run make deploy?")
        );
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::Exited], cx)
        });
        assert_eq!(trek.run_state(cx, &id), RunState::Failed);
        assert_eq!(
            why(&trek, cx, &id).as_deref(),
            Some("Failed: The agent stopped unexpectedly.")
        );
        assert!(trek.read(cx, |ws, _| ws.pending_request(&id).is_none()));
    });
}

#[test]
fn what_a_restart_cut_short_says_so_and_stops_counting_once_read() {
    run(async |cx| {
        let store = Store::in_memory().expect("store");
        let project = new_project("project");
        let left = |state: RunState, title: &str| {
            let mut t = store
                .create_thread(
                    Some(&project),
                    mock(),
                    None,
                    Effort::Medium,
                    HandHolding::Supervised,
                )
                .expect("thread");
            t.title = title.into();
            t.run_state = state;
            store.save_thread(&t).expect("save");
            t.id
        };
        let (working, asking, failed, quiet) = (
            left(RunState::Working, "Working"),
            left(RunState::NeedsYou, "Asking"),
            left(RunState::Failed, "Failed"),
            left(RunState::Idle, "Quiet"),
        );

        let ws = cx.new(|cx| Workspace::with(store.clone(), settings(), cx));
        ws.update(cx, |ws, cx| {
            assert!(!ws.thread(&quiet).unwrap().is_unseen());
            for id in [&working, &asking] {
                assert_eq!(ws.thread(id).unwrap().run_state, RunState::Idle);
            }
            assert_eq!(
                ws.why_needs_you(&working),
                Some(Why::CutShort),
                "{:?}",
                ws.thread(&working)
            );
            // Cut off while it waited on the user, it waits on them still: not news.
            assert_eq!(
                ws.why_needs_you(&asking),
                Some(Why::Waiting(None)),
                "{:?}",
                ws.thread(&asking)
            );
            assert_eq!(ws.why_needs_you(&failed), Some(Why::Failed(None)));
            assert_eq!(ws.why_needs_you(&quiet), None);
            assert_eq!(
                ws.needs_you_count(),
                2,
                "the badge counts what's waiting (the one that asked, the failure): not what's only news"
            );
            // Read, it's off the list; a later turn that finishes is simply finished.
            ws.navigate(on_thread(&working), cx);
            assert_eq!(ws.why_needs_you(&working), None);
            ws.navigate(on_thread(&quiet), cx);
        });
        // Read and finished within one millisecond would look read.
        std::thread::sleep(std::time::Duration::from_millis(2));
        ws.update(cx, |ws, cx| {
            ws.apply_events(
                &working,
                vec![
                    AgentEvent::TextDelta("Back at it.".into()),
                    AgentEvent::TurnComplete { error: None },
                ],
                cx,
            );
            assert_eq!(ws.why_needs_you(&working), Some(Why::Finished));
        });
    });
}

#[test]
fn the_keys_are_in_the_shortcuts_list_and_the_palette() {
    run(async |cx| {
        assert_eq!(keys::hint_for(Id::NextNeedsYou, true), "⌘⇧I");
        assert_eq!(keys::hint_for(Id::NextNeedsYou, false), "Ctrl+Shift+I");
        assert_eq!(keys::hint_for(Id::BackToLastPrompt, true), "⌘⇧B");
        assert_eq!(keys::hint_for(Id::BackToLastPrompt, false), "Ctrl+Shift+B");
        for mac in [true, false] {
            let groups = keys::shortcut_groups(mac, false);
            let threads = &groups.iter().find(|(g, _)| *g == "Threads").unwrap().1;
            for name in [
                "Next thread that needs you",
                "Back to where you last sent a prompt",
            ] {
                let row = threads.iter().find(|(n, _)| *n == name);
                assert!(row.is_some_and(|(_, chips)| !chips.is_empty()), "{name}");
            }
        }

        let trek = open(cx);
        with_keys(cx);
        let id = thread(&trek, cx, "Asking", 60_000);
        ask(&trek, cx, &id, "make deploy");
        trek.press(cx, &keys::keystroke(Id::OpenPalette));
        trek.type_text(cx, "needs you");
        trek.render(cx);
        let palette = cx.read(|cx| trek.root.read(cx).palette.clone());
        let labels: Vec<String> = palette.read_with(cx, |p, cx| {
            p.entries(cx).iter().map(|e| e.label.to_string()).collect()
        });
        assert!(
            labels.iter().any(|l| l == "Next thread that needs you"),
            "{labels:?}"
        );
        trek.press(cx, "enter");
        cx.run_until_parked();
        assert_eq!(route(&trek, cx), on_thread(&id));
    });
}
