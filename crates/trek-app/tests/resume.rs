//! Not losing running work to a restart: updates that wait for it, chats a quit cut off offered
//! back at the next launch, and messages not sent yet kept across it. A relaunch here is a new
//! window over the same database, as Trek's launch sees what the last run left.

mod common;

use common::{Trek, launch, mock, new_project, open, run, settings, store_items};
use gpui_kit::TestAppContext;
use trek_agents::mock as mock_agent;
use trek_app::updater::UpdateAction;
use trek_app::workspace::resume::ResumeState;
use trek_app::workspace::{Route, UpdateStatus};
use trek_core::settings::Settings;
use trek_core::store::{Item, Store, ToolStatus};
use trek_core::{Effort, HandHolding, RunState};

fn user(text: &str) -> Item {
    Item::User {
        text: text.into(),
        images: vec![],
        at: Some(1),
        resume: None,
        aside: false,
    }
}

fn tool(status: ToolStatus) -> Item {
    Item::Tool {
        id: "t1".into(),
        title: "Bash".into(),
        detail: "npm publish".into(),
        output: String::new(),
        status,
    }
}

/// A thread as the last run left it.
fn left_by_last_run(
    store: &Store,
    project: &std::path::Path,
    state: RunState,
    native: Option<&str>,
    items: Vec<Item>,
) -> String {
    let mut t = store
        .create_thread(
            Some(project),
            mock(),
            None,
            Effort::Medium,
            HandHolding::Auto,
        )
        .expect("thread");
    t.title = "Publish it".into();
    t.run_state = state;
    t.native_id = native.map(str::to_string);
    store.save_thread(&t).expect("save");
    store_items(store, &t.id, items);
    t.id
}

/// The folder a run's database goes in, and the project its threads are in.
struct Run {
    db: std::path::PathBuf,
    project: std::path::PathBuf,
}

impl Run {
    fn new(name: &str) -> Self {
        let dir = new_project(name);
        Run {
            db: dir.join("trek.sqlite"),
            project: new_project("project"),
        }
    }

    fn store(&self) -> Store {
        Store::open(&self.db).expect("store")
    }

    fn launch(&self, cx: &mut TestAppContext, tweak: impl FnOnce(&mut Settings)) -> Trek {
        let mut s = settings();
        s.user_projects.push(self.project.display().to_string());
        tweak(&mut s);
        let (ws, root, window) = launch(cx, self.store(), s);
        let trek = Trek {
            ws,
            root,
            window,
            project: self.project.clone(),
        };
        trek.render(cx);
        trek
    }
}

fn ready(trek: &Trek, cx: &mut TestAppContext) {
    let staged = common::data_dir().join("no-such-update/Trek.app");
    trek.update(cx, |ws, cx| {
        ws.blocked = Some(None);
        ws.updater.status = UpdateStatus::Ready {
            version: "9.9.9".into(),
            staged,
        };
        cx.notify();
    });
}

fn status(trek: &Trek, cx: &TestAppContext) -> UpdateStatus {
    trek.read(cx, |ws, _| ws.updater.status.clone())
}

fn view(trek: &Trek, cx: &TestAppContext) -> trek_app::updater::UpdateView {
    trek.read(cx, |ws, app| ws.update_view(app))
}

/// A turn that keeps going until it's stopped, in a thread of its own.
async fn long_turn(trek: &Trek, cx: &mut TestAppContext) -> String {
    trek.update(cx, |ws, cx| {
        ws.navigate(
            Route::Draft {
                project: Some(trek.project.clone()),
            },
            cx,
        )
    });
    let id = trek.send(cx, "mock:long 600s");
    let tid = id.clone();
    trek.wait(cx, "the long build", |ws| {
        ws.live
            .get(&tid)
            .is_some_and(|l| l.turn_started.is_some() && l.commands.is_some())
    })
    .await;
    id
}

fn install_failed(trek: &Trek, cx: &TestAppContext) -> bool {
    matches!(status(trek, cx), UpdateStatus::Failed(e) if e.contains(common::INSTALL_BLOCKED))
}

// ---------- updates that wait ----------

#[test]
fn an_update_ready_says_how_many_chats_run_and_installs_when_the_last_one_ends() {
    run(async |cx| {
        let trek = open(cx);
        let one = long_turn(&trek, cx).await;
        let two = long_turn(&trek, cx).await;
        ready(&trek, cx);

        let v = view(&trek, cx);
        assert_eq!(v.line, "Trek 9.9.9 is ready. 2 threads are running.");
        let c = v.choices.expect("the choices");
        assert_eq!(c.wait, "Installs when 2 threads finish");
        assert_eq!(c.now.as_deref(), Some("Install now stops 2 threads first."));
        assert!(c.later);

        // The default: wait.
        trek.update(cx, |ws, cx| {
            ws.run_update_action(v.action.expect("an action"), cx)
        });
        assert!(matches!(
            status(&trek, cx),
            UpdateStatus::RestartPending { .. }
        ));
        assert_eq!(
            view(&trek, cx).line,
            "Trek 9.9.9 installs when 2 threads finish."
        );

        trek.update(cx, |ws, cx| ws.interrupt(&one, cx));
        trek.wait_done(cx, &one, RunState::Idle).await;
        assert_eq!(
            view(&trek, cx).line,
            "Trek 9.9.9 installs when 1 thread finishes.",
            "it follows what's left"
        );
        cx.executor()
            .advance_clock(trek_app::workspace::RESTART_GRACE * 2);
        cx.run_until_parked();
        assert!(matches!(
            status(&trek, cx),
            UpdateStatus::RestartPending { .. }
        ));

        trek.update(cx, |ws, cx| ws.interrupt(&two, cx));
        trek.wait_done(cx, &two, RunState::Idle).await;
        // Warned first: a countdown that can be called off.
        assert!(matches!(
            status(&trek, cx),
            UpdateStatus::RestartPending { .. }
        ));
        cx.executor()
            .advance_clock(trek_app::workspace::RESTART_GRACE);
        cx.run_until_parked();
        assert!(install_failed(&trek, cx), "{:?}", status(&trek, cx));
    });
}

#[test]
fn install_now_goes_at_once_and_later_leaves_it_for_the_quit() {
    run(async |cx| {
        let trek = open(cx);
        let id = long_turn(&trek, cx).await;
        ready(&trek, cx);
        assert_eq!(view(&trek, cx).secondary, Some(UpdateAction::InstallNow));

        trek.update(cx, |ws, cx| ws.run_update_action(UpdateAction::Later, cx));
        assert!(matches!(status(&trek, cx), UpdateStatus::Ready { .. }));
        let v = view(&trek, cx);
        assert_eq!(
            v.line,
            "Trek 9.9.9 installs when you quit. 1 thread is running."
        );
        assert!(!v.choices.expect("still choices").later);
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        cx.executor()
            .advance_clock(trek_app::workspace::RESTART_GRACE * 2);
        cx.run_until_parked();
        assert!(
            matches!(status(&trek, cx), UpdateStatus::Ready { .. }),
            "Later doesn't install when the chat ends"
        );
        assert_eq!(
            view(&trek, cx).line,
            "Trek 9.9.9 installs when you quit. Restart whenever suits."
        );

        let id = long_turn(&trek, cx).await;
        trek.update(cx, |ws, cx| {
            ws.run_update_action(UpdateAction::InstallNow, cx)
        });
        assert!(
            install_failed(&trek, cx),
            "it went for it, chat running: {:?}",
            status(&trek, cx)
        );
        assert!(trek.read(cx, |ws, _| ws.turn_running(&id)));
    });
}

#[test]
fn someone_typing_isnt_interrupted_and_what_they_typed_is_kept() {
    run(async |cx| {
        let trek = open(cx);
        let id = long_turn(&trek, cx).await;
        ready(&trek, cx);
        trek.update(cx, |ws, cx| ws.restart_to_update(cx));

        // The chat ends while the user is writing the next message.
        trek.type_text(cx, "and then deploy");
        assert!(trek.read(cx, |ws, app| ws.typing_now(app)));
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(
            view(&trek, cx).line,
            "Trek 9.9.9 installs once you've stopped typing."
        );
        cx.executor()
            .advance_clock(trek_app::workspace::RESTART_GRACE * 2);
        cx.run_until_parked();
        assert!(
            matches!(status(&trek, cx), UpdateStatus::RestartPending { .. }),
            "no countdown over their typing: {:?}",
            status(&trek, cx)
        );

        // They stop, and the chats are done: it warns and goes, and the message is kept.
        cx.executor()
            .advance_clock(trek_app::workspace::unsent::TYPING_QUIET);
        assert!(!trek.read(cx, |ws, app| ws.typing_now(app)));
        assert_eq!(view(&trek, cx).line, "Trek 9.9.9 installs in a moment.");
    });
}

#[test]
fn background_sub_agents_hold_an_update_and_count_as_their_chat() {
    run(async |cx| {
        let trek = open(cx);
        // Its turn is over, and its agent's own sub-agents are still out.
        let own = trek.send(cx, "subagents 600s");
        let tid = own.clone();
        trek.wait(cx, "its sub-agents out", move |ws| {
            ws.live
                .get(&tid)
                .is_some_and(|l| l.turn_started.is_none() && l.background_agents().count() == 2)
        })
        .await;
        ready(&trek, cx);
        assert_eq!(
            view(&trek, cx).line,
            "Trek 9.9.9 is ready. 1 thread is running.",
            "two sub-agents, one chat"
        );
        assert!(trek.read(cx, |ws, _| ws.work_in_flight()));

        // A sub-agent Trek sent out, working on with its parent's turn over: its parent's chat.
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(trek.project.clone()),
                },
                cx,
            )
        });
        let parent = trek.send(cx, "mock:delegate subagents 600s");
        let p = parent.clone();
        trek.wait(cx, "the sub-agent out", move |ws| {
            ws.children(&p)
                .first()
                .and_then(|c| ws.live.get(&c.id))
                .is_some_and(|l| l.background_agents().count() == 2)
        })
        .await;
        assert_eq!(
            view(&trek, cx).line,
            "Trek 9.9.9 is ready. 2 threads are running."
        );
        // A shell left running doesn't hold it: `tests/background.rs` has that.
    });
}

#[test]
fn an_iphone_update_counts_as_running_work() {
    run(async |cx| {
        let trek = open(cx);
        ready(&trek, cx);
        trek.update(cx, |ws, _| ws.phone_update.running = true);
        let v = view(&trek, cx);
        assert_eq!(v.line, "Trek 9.9.9 is ready. The iPhone update is running.");
        assert_eq!(
            v.choices.expect("choices").wait,
            "Installs when the iPhone update finishes"
        );
        trek.update(cx, |ws, cx| ws.restart_to_update(cx));
        assert!(matches!(
            status(&trek, cx),
            UpdateStatus::RestartPending { .. }
        ));
    });
}

#[test]
fn edits_not_saved_hold_a_restart_and_going_now_says_what_it_costs() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), Some(1), cx));
        common::loaded(&trek, cx).await;
        let editor = trek
            .root
            .read_with(cx, |r, cx| r.editor(cx))
            .expect("editor view");
        trek.window(cx, |window, cx| {
            editor.update(cx, |e, cx| {
                e.text_state()
                    .update(cx, |s, cx| s.insert("let x = 1;\n", window, cx))
            });
        });
        cx.run_until_parked();
        ready(&trek, cx);
        let v = view(&trek, cx);
        let c = v.choices.expect("the choices");
        assert_eq!(c.wait, "Installs once your edits are saved");
        assert_eq!(
            c.now.as_deref(),
            Some("With Install now, unsaved edits in 1 file are lost.")
        );
        // Restart doesn't go over them.
        trek.update(cx, |ws, cx| ws.restart_to_update(cx));
        assert!(matches!(
            status(&trek, cx),
            UpdateStatus::RestartPending { .. }
        ));
        cx.executor()
            .advance_clock(trek_app::workspace::RESTART_GRACE * 2);
        cx.run_until_parked();
        assert!(matches!(
            status(&trek, cx),
            UpdateStatus::RestartPending { .. }
        ));
        // Saved, nothing holds it.
        editor.update(cx, |e, cx| e.save_now(cx));
        assert_eq!(view(&trek, cx).line, "Trek 9.9.9 installs in a moment.");
    });
}

// ---------- messages not sent ----------

#[test]
fn a_message_not_sent_comes_back_after_a_relaunch() {
    run(async |cx| {
        let run_ = Run::new("unsent");
        let id = left_by_last_run(
            &run_.store(),
            &run_.project,
            RunState::Idle,
            Some("s1"),
            vec![user("hello")],
        );
        let trek = run_.launch(cx, |_| {});
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        trek.type_text(cx, "half a thought");
        // Another thread on screen: what was typed went with the thread it was typed for.
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(trek.project.clone()),
                },
                cx,
            )
        });
        trek.render(cx);
        trek.type_text(cx, "a new thread's message");
        trek.update(cx, |ws, _| ws.save_unsent());
        trek.window(cx, |window, _| window.remove_window());

        let again = run_.launch(cx, |_| {});
        again.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        again.render(cx);
        assert_eq!(again.composer_text(cx), "half a thought");
        again.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(again.project.clone()),
                },
                cx,
            )
        });
        again.render(cx);
        assert_eq!(again.composer_text(cx), "a new thread's message");
    });
}

#[test]
fn whats_in_the_prompt_as_trek_quits_is_kept_and_a_message_sent_isnt() {
    run(async |cx| {
        let run_ = Run::new("quit-unsent");
        let trek = run_.launch(cx, |_| {});
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.type_text(cx, "half a thought");
        // Sent and gone: it isn't kept.
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(trek.project.clone()),
                },
                cx,
            )
        });
        trek.render(cx);
        let other = trek.send(cx, "sent already");
        assert_ne!(other, id);
        cx.quit();
        let kept = run_.store().unsent_messages().unwrap();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(kept[0].key, format!("thread:{id}"));
        assert_eq!(kept[0].text, "half a thought");
    });
}

#[test]
fn messages_queued_behind_a_turn_come_back_unsent() {
    run(async |cx| {
        let run_ = Run::new("queued");
        let trek = run_.launch(cx, |s| {
            s.general.follow_up = trek_core::settings::FollowUp::Queue
        });
        let id = long_turn(&trek, cx).await;
        trek.send(cx, "then deploy");
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1);
        trek.update(cx, |ws, _| ws.save_unsent());
        trek.window(cx, |window, _| window.remove_window());

        let again = run_.launch(cx, |_| {});
        again.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        again.render(cx);
        assert_eq!(again.composer_text(cx), "then deploy");
    });
}

// ---------- chats a quit cut off ----------

#[test]
fn chats_cut_off_are_offered_back_and_nothing_runs_by_itself() {
    run(async |cx| {
        let run_ = Run::new("offered");
        let store = run_.store();
        let working = left_by_last_run(
            &store,
            &run_.project,
            RunState::Working,
            Some("sess-1"),
            vec![user("publish"), tool(ToolStatus::Running)],
        );
        let also = left_by_last_run(
            &store,
            &run_.project,
            RunState::Working,
            Some("sess-2"),
            vec![user("build")],
        );
        let asking = left_by_last_run(
            &store,
            &run_.project,
            RunState::NeedsYou,
            Some("sess-3"),
            vec![user("deploy"), tool(ToolStatus::Running)],
        );
        let stopped = left_by_last_run(
            &store,
            &run_.project,
            RunState::Idle,
            Some("sess-4"),
            vec![
                user("nope"),
                Item::Notice {
                    text: "Interrupted".into(),
                },
            ],
        );
        drop(store);
        let trek = run_.launch(cx, |_| {});
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(60));
        cx.run_until_parked();

        let notice = trek.read(cx, |ws, _| ws.interrupted_notice());
        assert_eq!(
            notice.headline().as_deref(),
            Some("2 threads were interrupted")
        );
        assert_eq!(
            notice.waiting_too().as_deref(),
            Some("1 is waiting for you")
        );
        assert_eq!(notice.working.len(), 2);
        assert_eq!(notice.waiting, vec![asking.clone()]);
        // Stopped on purpose: nothing about it.
        assert!(trek.read(cx, |ws, _| ws.interrupted_offer(&stopped).is_none()));
        // Nothing ran: no session, no new message, no state but idle.
        for id in [&working, &also, &asking, &stopped] {
            assert!(
                trek.read(cx, |ws, _| ws
                    .live
                    .get(id.as_str())
                    .is_none_or(|l| l.commands.is_none())),
                "{id}: no session"
            );
            assert_eq!(trek.run_state(cx, id), RunState::Idle);
            let stored = trek.read(cx, |ws, _| ws.store.items(id).unwrap());
            assert_eq!(
                stored
                    .iter()
                    .filter(|i| matches!(i, Item::User { .. }))
                    .count(),
                1,
                "{id}: nothing sent"
            );
        }
        // What the card was about is kept for its bar.
        assert_eq!(
            trek.read(cx, |ws, _| ws
                .interrupted_offer(&asking)
                .and_then(|o| o.turn.asked.clone())),
            Some("Bash npm publish".to_string())
        );
        // The sidebar says so, and each row carries its own way back.
        trek.render(cx);
        assert!(trek.visible(cx, "interrupted-notice"));
        assert!(trek.visible(cx, "resume-all"));
        assert!(trek.visible(cx, format!("inbox-resume-{working}")));
        assert!(
            !trek.visible(cx, format!("inbox-resume-{asking}")),
            "one waiting isn't resumed from its row"
        );
    });
}

#[test]
fn resume_all_reopens_each_session_with_a_message_and_a_note_and_leaves_the_one_waiting() {
    run(async |cx| {
        let run_ = Run::new("resume-all");
        let store = run_.store();
        // The session has to be one the mock knows, as a real agent has its session on disk.
        let trek0 = run_.launch(cx, |_| {});
        let earlier = trek0.send(cx, "hello");
        trek0.wait_done(cx, &earlier, RunState::Idle).await;
        let native = trek0
            .read(cx, |ws, _| {
                ws.thread(&earlier).and_then(|t| t.native_id.clone())
            })
            .expect("a session");
        trek0.window(cx, |window, _| window.remove_window());
        store
            .update_thread(&earlier, |t| t.run_state = RunState::Working)
            .unwrap();
        let mut items = store.items(&earlier).unwrap();
        items.push(tool(ToolStatus::Running));
        store_items(&store, &earlier, items);
        let asking = left_by_last_run(
            &store,
            &run_.project,
            RunState::NeedsYou,
            Some("sess-3"),
            vec![user("deploy"), tool(ToolStatus::Running)],
        );
        drop(store);

        let trek = run_.launch(cx, |_| {});
        cx.run_until_parked();
        trek.click(cx, "resume-all");
        // Right away, and before the agent has said anything: "Resuming…" somewhere it shows.
        assert!(matches!(
            trek.read(cx, |ws, _| ws
                .interrupted_offer(&earlier)
                .map(|o| o.state.clone())),
            Some(ResumeState::Resuming) | Some(ResumeState::Going)
        ));
        assert_eq!(trek.run_state(cx, &earlier), RunState::Working);
        trek.wait_done(cx, &earlier, RunState::Idle).await;

        // The same session of the agent's.
        assert_eq!(
            trek.read(cx, |ws, _| ws
                .thread(&earlier)
                .and_then(|t| t.native_id.clone())),
            Some(native.clone())
        );
        // The transcript has the message; the agent had the note under it too.
        let items = trek.items(cx, &earlier);
        assert!(
            items
                .iter()
                .any(|i| matches!(i, Item::User { text, .. } if text == "Resume working")),
            "{items:?}"
        );
        let said = mock_agent::remembered(&native);
        let last = said.last().expect("a message");
        assert!(
            last.starts_with("Resume working\n\n") && last.contains("none were repeated"),
            "{last}"
        );
        // Tool calls that may have run are left as failed, and none is sent again.
        assert!(!items.iter().any(|i| matches!(
            i,
            Item::Tool {
                status: ToolStatus::Running,
                ..
            }
        )));

        // Over: the offer's gone for good.
        assert!(trek.read(cx, |ws, _| ws.interrupted_offer(&earlier).is_none()));
        assert!(
            trek.read(cx, |ws, _| ws.store.interrupted_turns().unwrap())
                .iter()
                .all(|t| t.thread != earlier)
        );
        // The one waiting wasn't touched.
        assert!(trek.read(cx, |ws, _| {
            ws.live.get(&asking).is_none_or(|l| l.commands.is_none())
        }));
        assert_eq!(
            trek.read(cx, |ws, _| ws.interrupted_notice().waiting),
            vec![asking]
        );
    });
}

#[test]
fn resume_on_a_row_resumes_it_and_leaves_the_user_where_they_are() {
    run(async |cx| {
        let run_ = Run::new("row");
        let id = left_by_last_run(
            &run_.store(),
            &run_.project,
            RunState::Working,
            None,
            vec![user("publish")],
        );
        let trek = run_.launch(cx, |_| {});
        cx.run_until_parked();
        trek.render(cx);
        let before = trek.read(cx, |ws, _| ws.route.clone());
        trek.click(cx, format!("inbox-resume-{id}"));
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            before,
            "the row's own click doesn't open it"
        );
        // Resuming shows on the row, then the turn runs and ends.
        trek.render(cx);
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| ws.interrupted_offer(&id).is_none()));
        assert_eq!(
            trek.read(cx, |ws, _| ws.interrupted_notice().headline()),
            None
        );
    });
}

#[test]
fn a_direct_thread_with_no_session_comes_back_from_a_recap() {
    run(async |cx| {
        let run_ = Run::new("recap");
        let id = left_by_last_run(
            &run_.store(),
            &run_.project,
            RunState::Working,
            None,
            vec![
                user("remember the word APPLE"),
                Item::Assistant {
                    text: "Noted.".into(),
                },
            ],
        );
        let trek = run_.launch(cx, |_| {});
        cx.run_until_parked();
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        trek.click(cx, "resume-thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let native = trek
            .read(cx, |ws, _| ws.thread(&id).and_then(|t| t.native_id.clone()))
            .expect("the new session");
        // The mock takes the recap's messages in as its session's history, as an agent would read it.
        let said = mock_agent::remembered(&native);
        assert!(said.iter().any(|m| m.contains("APPLE")), "{said:?}");
        assert!(
            said.last()
                .expect("a message")
                .contains("none were repeated"),
            "{said:?}"
        );
    });
}

#[test]
fn the_composers_bar_offers_resume_and_edit_and_dismiss_leaves_it_stopped() {
    run(async |cx| {
        let run_ = Run::new("bar");
        let id = left_by_last_run(
            &run_.store(),
            &run_.project,
            RunState::Working,
            Some("gone"),
            vec![user("publish")],
        );
        let trek = run_.launch(cx, |_| {});
        cx.run_until_parked();
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        assert!(trek.visible(cx, "interrupted-bar"));

        // Edit message: it's in the prompt, for the user to change and send.
        trek.click(cx, "resume-edit");
        trek.render(cx);
        assert_eq!(trek.composer_text(cx), "Resume working");
        assert_eq!(trek.run_state(cx, &id), RunState::Idle, "nothing went yet");
        // What's typed already isn't overwritten.
        trek.type_text(cx, " on the docs");
        trek.click(cx, "resume-edit");
        trek.render(cx);
        assert_eq!(trek.composer_text(cx), "Resume working on the docs");
        trek.press(cx, "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let items = trek.items(cx, &id);
        assert!(
            items.iter().any(
                |i| matches!(i, Item::User { text, .. } if text == "Resume working on the docs")
            ),
            "{items:?}"
        );
        assert!(trek.read(cx, |ws, _| ws.interrupted_offer(&id).is_none()));
        assert!(!trek.visible(cx, "interrupted-bar"));

        // Dismiss: it stays stopped, through another launch too.
        let other = left_by_last_run(
            &run_.store(),
            &run_.project,
            RunState::Working,
            Some("gone-2"),
            vec![user("build")],
        );
        trek.window(cx, |window, _| window.remove_window());
        let again = run_.launch(cx, |_| {});
        cx.run_until_parked();
        again.update(cx, |ws, cx| ws.navigate(Route::Thread(other.clone()), cx));
        again.render(cx);
        again.click(cx, "resume-dismiss");
        again.render(cx);
        assert!(!again.visible(cx, "interrupted-bar"));
        assert_eq!(again.run_state(cx, &other), RunState::Idle);
        again.window(cx, |window, _| window.remove_window());
        let third = run_.launch(cx, |_| {});
        cx.run_until_parked();
        assert!(third.read(cx, |ws, _| ws.interrupted_offer(&other).is_none()));
    });
}

#[test]
fn a_chat_waiting_on_approval_comes_back_waiting_and_asking_again_is_the_users_call() {
    run(async |cx| {
        let run_ = Run::new("waiting");
        let id = left_by_last_run(
            &run_.store(),
            &run_.project,
            RunState::NeedsYou,
            None,
            vec![user("deploy"), tool(ToolStatus::Running)],
        );
        // Even with resuming at launch on.
        let trek = run_.launch(cx, |s| s.general.resume_interrupted = true);
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(60));
        cx.run_until_parked();
        assert!(
            trek.read(cx, |ws, _| ws
                .live
                .get(&id)
                .is_none_or(|l| l.commands.is_none())),
            "no session"
        );
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        assert!(trek.visible(cx, "interrupted-bar"));
        assert!(!trek.visible(cx, "resume-edit"));
        trek.click(cx, "resume-thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let native = trek
            .read(cx, |ws, _| ws.thread(&id).and_then(|t| t.native_id.clone()))
            .expect("a session");
        let said = mock_agent::remembered(&native);
        assert!(
            said.last()
                .expect("a message")
                .contains("Nothing was answered"),
            "{said:?}"
        );
    });
}

#[test]
fn resuming_at_launch_only_when_turned_on_and_not_for_ever() {
    run(async |cx| {
        let run_ = Run::new("auto");
        let id = left_by_last_run(
            &run_.store(),
            &run_.project,
            RunState::Working,
            None,
            vec![user("publish")],
        );
        let tries_out = left_by_last_run(
            &run_.store(),
            &run_.project,
            RunState::Working,
            None,
            vec![user("crashes Trek every time")],
        );
        {
            let store = run_.store();
            store.note_interrupted(&tries_out, false, None, 1).unwrap();
            for _ in 0..trek_core::interrupted::MAX_AUTO_RESUMES {
                store.interrupted_tried(&tries_out).unwrap();
            }
        }
        let trek = run_.launch(cx, |s| s.general.resume_interrupted = true);
        // Not at once: the windows come up and the agents are found first.
        assert!(trek.read(cx, |ws, _| {
            ws.live.get(&id).is_none_or(|l| l.commands.is_none())
        }));
        let seen = common::toasts(&trek, cx);
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(10));
        cx.run_until_parked();
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(
            trek.items(cx, &id)
                .iter()
                .any(|i| matches!(i, Item::User { text, .. } if text == "Resume working"))
        );
        assert!(
            seen.borrow()
                .iter()
                .any(|(t, _)| t.contains("Resuming 1 interrupted thread")),
            "{:?}",
            seen.borrow()
        );
        // Resumed twice already, and Trek went down each time: it waits to be asked.
        assert!(trek.read(cx, |ws, _| {
            ws.live.get(&tries_out).is_none_or(|l| l.commands.is_none())
        }));
        assert_eq!(
            trek.read(cx, |ws, _| ws.interrupted_notice().working),
            vec![tries_out]
        );
    });
}

#[test]
fn a_failed_resume_says_why_and_retry_goes_again() {
    run(async |cx| {
        let run_ = Run::new("retry");
        let id = left_by_last_run(
            &run_.store(),
            &run_.project,
            RunState::Working,
            None,
            vec![user("publish")],
        );
        // The mock's turn that fails.
        let trek = run_.launch(cx, |s| s.general.resume_message = "error".into());
        cx.run_until_parked();
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        trek.click(cx, "resume-thread");
        trek.wait_done(cx, &id, RunState::Failed).await;
        let state = trek.read(cx, |ws, _| {
            ws.interrupted_offer(&id).map(|o| o.state.clone())
        });
        assert!(
            matches!(&state, Some(ResumeState::Failed(why)) if !why.is_empty()),
            "{state:?}"
        );
        trek.render(cx);
        assert!(trek.visible(cx, "resume-retry"));

        trek.update(cx, |ws, _| {
            ws.settings.general.resume_message = "go on".into()
        });
        trek.click(cx, "resume-retry");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| ws.interrupted_offer(&id).is_none()));
    });
}

#[test]
fn stopping_a_resumed_chat_or_archiving_one_ends_the_offer() {
    run(async |cx| {
        let run_ = Run::new("stopped");
        let store = run_.store();
        let id = left_by_last_run(
            &store,
            &run_.project,
            RunState::Working,
            None,
            vec![user("publish")],
        );
        let archived = left_by_last_run(
            &store,
            &run_.project,
            RunState::Working,
            None,
            vec![user("build")],
        );
        drop(store);
        let trek = run_.launch(cx, |s| s.general.resume_message = "mock:long 600s".into());
        cx.run_until_parked();
        trek.update(cx, |ws, cx| ws.archive(&archived, cx));
        assert!(trek.read(cx, |ws, _| ws.interrupted_offer(&archived).is_none()));
        trek.update(cx, |ws, cx| ws.resume_interrupted(&id, cx));
        let tid = id.clone();
        trek.wait(cx, "the resumed turn", |ws| ws.turn_running(&tid))
            .await;
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(
            trek.read(cx, |ws, _| ws.interrupted_offer(&id).is_none()),
            "stopped on purpose stays stopped"
        );
        assert!(
            trek.read(cx, |ws, _| ws.store.interrupted_turns().unwrap())
                .is_empty()
        );
    });
}

#[test]
fn a_stop_still_under_way_as_trek_quits_isnt_offered_back() {
    run(async |cx| {
        let run_ = Run::new("quit-stop");
        let trek = run_.launch(cx, |_| {});
        let stopping = long_turn(&trek, cx).await;
        let cut_off = long_turn(&trek, cx).await;
        // Asked to stop, and the quit comes before the agent has said it did.
        trek.update(cx, |ws, cx| {
            ws.interrupt(&stopping, cx);
        });
        cx.quit();
        let store = run_.store();
        assert_eq!(
            store.thread(&stopping).unwrap().unwrap().run_state,
            RunState::Idle
        );
        assert!(matches!(
            store.items(&stopping).unwrap().last(),
            Some(Item::Notice { text }) if text == "Interrupted"
        ));
        assert_eq!(
            store.thread(&cut_off).unwrap().unwrap().run_state,
            RunState::Working,
            "this one the next launch closes and offers"
        );
        let closed = store.close_interrupted_turns().unwrap();
        assert_eq!(closed, vec![cut_off.clone()]);
        let turns = store.interrupted_turns().unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].thread, cut_off);
    });
}

#[test]
fn a_parent_left_waiting_on_sub_agents_is_offered_not_woken() {
    run(async |cx| {
        let run_ = Run::new("parent");
        let store = run_.store();
        let parent = left_by_last_run(
            &store,
            &run_.project,
            RunState::Idle,
            Some("p"),
            vec![user("ask two models")],
        );
        let mut kid = store
            .create_thread(
                Some(&run_.project),
                mock(),
                None,
                Effort::Medium,
                HandHolding::Auto,
            )
            .unwrap();
        kid.parent_id = Some(parent.clone());
        kid.run_state = RunState::Working;
        store.save_thread(&kid).unwrap();
        store.await_report(&parent, &kid.id).unwrap();
        drop(store);

        let trek = run_.launch(cx, |_| {});
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(60));
        cx.run_until_parked();
        assert!(
            trek.read(cx, |ws, _| ws
                .live
                .get(&parent)
                .is_none_or(|l| l.commands.is_none())),
            "not woken without the setting"
        );
        assert_eq!(
            trek.read(cx, |ws, _| ws.interrupted_notice().working),
            vec![parent.clone()],
            "the sub-agent isn't a chat of its own"
        );
        let kept = trek.read(cx, |ws, _| ws.store.items(&parent).unwrap());
        assert!(
            kept.iter().any(
                |i| matches!(i, Item::Notice { text } if text.contains("kept for this thread"))
            ),
            "{kept:?}"
        );
    });
}
