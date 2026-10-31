//! Threads an agent starts as the user's own (`start_thread`) and hands its work to (`handoff`).
//! The mock agent calls Trek over the same socket `trek-mcp orchestrate` uses
//! (`mock:start-thread`, `mock:handoff`); other tests call the tools directly.

mod common;

use common::{Trek, mock, open, run};
use gpui_kit::TestAppContext;
use serde_json::json;
use std::cell::RefCell;
use std::rc::Rc;
use trek_app::workspace::{Route, ThreadState, UndoAction, WorkspaceEvent};
use trek_core::lineage::{self, Link};
use trek_core::store::Item;
use trek_core::{HandHolding, RunState, ThreadSource};

/// The threads `caller`'s agent started or handed off to, oldest first.
fn started(trek: &Trek, cx: &TestAppContext, caller: &str) -> Vec<String> {
    trek.read(cx, |ws, _| {
        ws.started_threads(caller)
            .into_iter()
            .map(|t| t.id.clone())
            .collect()
    })
}

fn state(trek: &Trek, cx: &TestAppContext, id: &str) -> ThreadState {
    trek.read(cx, |ws, _| ws.thread_state(id))
}

async fn wait_state(trek: &Trek, cx: &mut TestAppContext, id: &str, want: ThreadState) {
    let id = id.to_string();
    trek.wait(cx, &format!("the thread to be {want:?}"), move |ws| {
        ws.thread_state(&id) == want
    })
    .await;
}

/// The lineage rows in `id`'s transcript: (link, thread).
fn links(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<(Link, String)> {
    trek.items(cx, id)
        .into_iter()
        .filter_map(|i| match i {
            Item::Tool { id, .. } => lineage::of_row(&id).map(|(l, t)| (l, t.to_string())),
            _ => None,
        })
        .collect()
}

/// Toasts and alerts seen so far: (message, its button).
type Notices = Rc<RefCell<Vec<(String, Option<String>)>>>;

/// Every event the workspace raises from now on that's a toast or an alert.
fn notices(trek: &Trek, cx: &mut TestAppContext) -> Notices {
    let seen = Rc::new(RefCell::new(vec![]));
    let sink = seen.clone();
    cx.update(|cx| {
        cx.subscribe(&trek.ws, move |_, event: &WorkspaceEvent, _| match event {
            WorkspaceEvent::Attention {
                message, button, ..
            } => sink
                .borrow_mut()
                .push((message.clone(), button.map(str::to_string))),
            WorkspaceEvent::Toast { message, undo } => sink
                .borrow_mut()
                .push((message.clone(), undo.as_ref().map(|u| format!("{u:?}")))),
            _ => {}
        })
        .detach()
    });
    seen
}

/// A thread in the project made straight in the store, shaped by `f`, as an earlier run left it.
fn stored_thread(
    trek: &Trek,
    cx: &mut TestAppContext,
    f: impl FnOnce(&mut trek_core::store::Thread),
) -> String {
    trek.update(cx, |ws, cx| {
        let mut t = ws
            .store
            .create_thread(
                Some(&trek.project),
                mock(),
                None,
                trek_core::Effort::Medium,
                HandHolding::Auto,
            )
            .expect("thread");
        f(&mut t);
        ws.store.save_thread(&t).expect("save");
        ws.reload(cx);
        t.id
    })
}

#[test]
fn an_agent_starts_a_thread_of_the_users_own() {
    run(async |cx| {
        let trek = open(cx);
        let seen = notices(&trek, cx);
        let id = trek.send(cx, "mock:start-thread explain the startup");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let kids = started(&trek, cx, &id);
        assert_eq!(kids.len(), 1, "one thread");
        let new = &kids[0];
        let (t, caller) = trek.read(cx, |ws, _| {
            (
                ws.thread(new).cloned().unwrap(),
                ws.thread(&id).cloned().unwrap(),
            )
        });
        assert_eq!(
            (
                t.started_by.as_deref(),
                t.parent_id.as_deref(),
                t.title.as_str()
            ),
            (Some(id.as_str()), None, "Speed up the slow tests")
        );
        assert_eq!(
            (t.agent.clone(), t.model.as_deref(), t.source),
            (mock(), Some("mock-swift"), ThreadSource::Trek)
        );
        assert_eq!(
            t.hand_holding, caller.hand_holding,
            "no more access than the thread that started it"
        );
        assert_eq!(
            t.cwd.as_deref(),
            Some(trek.project.as_path()),
            "in the same project"
        );
        // A thread like any other: in the sidebar, not under its starter.
        let listed: Vec<String> = trek.read(cx, |ws, _| {
            ws.sections()
                .into_iter()
                .flat_map(|(_, ts)| ts.into_iter().map(|t| t.id.clone()))
                .collect()
        });
        assert!(listed.contains(new) && listed.contains(&id), "{listed:?}");
        // The user stays where they were.
        assert_eq!(trek.thread_id(cx), id);
        // Both transcripts link the two; the starter's row stands for the call.
        assert_eq!(links(&trek, cx, &id), [(Link::Started, new.clone())]);
        assert!(
            trek.rows(cx)
                .iter()
                .any(|r| r.starts_with("lineage: Started thread: Speed up the slow tests")),
            "{:?}",
            trek.rows(cx)
        );
        assert!(
            !trek.rows(cx).iter().any(|r| r.contains("group")),
            "the call has no row of its own: {:?}",
            trek.rows(cx)
        );
        assert_eq!(links(&trek, cx, new), [(Link::StartedBy, id.clone())]);
        let asked = trek
            .items(cx, new)
            .into_iter()
            .find_map(|i| {
                if let Item::User { text, .. } = i {
                    Some(text)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(
            asked, "explain the startup",
            "its first message is the prompt as given"
        );
        // It runs, and when it's done the user hears so, with a way to it.
        wait_state(&trek, cx, new, ThreadState::Finished).await;
        trek.wait_done(cx, new, RunState::Idle).await;
        assert!(trek.answers(cx, new).contains("How the app starts"));
        let alert = seen
            .borrow()
            .iter()
            .find(|(m, _)| m.starts_with("Thread completed"))
            .cloned();
        assert_eq!(
            alert,
            Some((
                "Thread completed · Speed up the slow tests".into(),
                Some("Open thread".into())
            )),
            "{:?}",
            seen.borrow()
        );
        // The starter's agent can follow it.
        let status = trek.read(cx, |ws, _| ws.thread_status(&id, new)).unwrap();
        assert_eq!(
            (status["status"].as_str(), status["title"].as_str()),
            (Some("finished"), Some("Speed up the slow tests"))
        );
        assert!(
            status["preview"]
                .as_str()
                .unwrap()
                .contains("How the app starts")
        );
        let read = trek.read(cx, |ws, _| ws.read_thread(&id, new)).unwrap();
        assert!(read["answer"].as_str().unwrap().contains("```rust"));
        let list = trek.read(cx, |ws, _| ws.list_threads(&id));
        assert_eq!(list["threads"][0]["id"].as_str(), Some(new.as_str()));
        assert_eq!(list["threads"][0]["relation"], "started");
        assert_eq!(
            list["threads"][0]["link"].as_str(),
            Some(format!("trek://thread/{new}").as_str()),
            "a link that opens it"
        );
        assert_eq!(list["this_thread"]["can_start_threads"], true);
    });
}

#[test]
fn an_agent_waits_for_a_thread_it_started_and_reads_its_answer() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:start-thread-wait mock:long 1s");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let new = started(&trek, cx, &id).pop().expect("a thread");
        assert_eq!(state(&trek, cx, &new), ThreadState::Finished);
        assert!(
            trek.answers(cx, &id)
                .contains("The thread is finished: The full suite passed"),
            "{}",
            trek.answers(cx, &id)
        );
    });
}

#[test]
fn a_handoff_moves_the_work_and_the_user_to_a_fresh_thread() {
    run(async |cx| {
        let trek = open(cx);
        let seen = notices(&trek, cx);
        // The screen may have moved on to the new thread by the time `send` looks.
        trek.send(cx, "mock:handoff Run the tests.");
        trek.wait(cx, "the handoff", |ws| {
            ws.threads.iter().any(|t| t.continued_from.is_some())
        })
        .await;
        let id = trek
            .read(cx, |ws, _| {
                ws.threads.iter().find_map(|t| t.continued_from.clone())
            })
            .unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        let new = started(&trek, cx, &id).pop().expect("a new thread");
        let (t, old) = trek.read(cx, |ws, _| {
            (
                ws.thread(&new).cloned().unwrap(),
                ws.thread(&id).cloned().unwrap(),
            )
        });
        assert_eq!(
            (
                t.continued_from.as_deref(),
                t.started_by.as_deref(),
                t.title.as_str()
            ),
            (Some(id.as_str()), None, "Finish the parser refactor")
        );
        assert_eq!(
            (t.cwd.clone(), t.worktree.clone(), t.hand_holding),
            (old.cwd.clone(), old.worktree.clone(), old.hand_holding),
            "the same folder and access"
        );
        // The user was looking at the old thread: they follow the work.
        assert_eq!(trek.thread_id(cx), new);
        assert_eq!(links(&trek, cx, &id), [(Link::ContinuedIn, new.clone())]);
        assert_eq!(links(&trek, cx, &new), [(Link::ContinuedFrom, id.clone())]);
        assert!(
            trek.rows(cx)[0].starts_with("lineage: Continued from: "),
            "{:?}",
            trek.rows(cx)
        );
        let brief = trek
            .items(cx, &new)
            .into_iter()
            .find_map(|i| {
                if let Item::User { text, .. } = i {
                    Some(text)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(
            brief.contains("taking over from another thread")
                && brief.contains("<brief>\nGoal: refactor the request parser")
                && brief.contains("Next: Run the tests.\n</brief>"),
            "{brief}"
        );
        // And where the old thread's whole transcript is, to search for anything exact.
        let log = trek_core::paths::transcripts_dir().join(format!("{id}.log"));
        assert!(
            brief.contains(&format!(
                "That thread's whole conversation is in its transcript at \"{}\"",
                log.display()
            )) && brief.contains(&format!("transcript_search with thread \"{id}\"")),
            "{brief}"
        );
        assert!(
            std::fs::read_to_string(&log)
                .unwrap()
                .contains("mock:handoff Run the tests."),
            "the log has the old thread's messages"
        );
        trek.wait_done(cx, &new, RunState::Idle).await;
        // The old thread's agent is told to stop there, and it did.
        assert!(
            trek.answers(cx, &id)
                .ends_with("Handed off: the work goes on in the new thread."),
            "{}",
            trek.answers(cx, &id)
        );
        // Its turn ending is no news: the work went on elsewhere. Nor is it unread: the user saw
        // it hand off, and its last words are the agent's own.
        assert!(
            !seen.borrow().iter().any(|(m, _)| m.starts_with("Finished")),
            "{:?}",
            seen.borrow()
        );
        assert!(
            !trek.read(cx, |ws, _| ws.thread(&id).unwrap().is_unseen()),
            "the old thread is read"
        );
        // A handoff's thread wakes nobody: the thread it continues is done.
        assert!(!trek.items(cx, &id).iter().any(
            |i| matches!(i, Item::User { text, .. } if trek_core::orchestrate::is_wake(text))
        ));
        // Back in the old thread, the row says where the work went.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        assert!(
            trek.rows(cx)
                .iter()
                .any(|r| r.starts_with("lineage: Continued in: Finish the parser refactor")),
            "{:?}",
            trek.rows(cx)
        );
        // One handoff a request: a second says where the first went.
        let again = trek
            .update(cx, |ws, cx| {
                ws.handoff(&id, &json!({ "brief": "more" }), cx)
            })
            .unwrap_err();
        assert!(
            again.contains("already handed off") && again.contains(&new),
            "{again}"
        );
    });
}

#[test]
fn a_handoff_the_user_isnt_looking_at_leaves_them_where_they_are() {
    run(async |cx| {
        let trek = open(cx);
        let seen = notices(&trek, cx);
        let away = trek.quiet_thread(cx);
        let here = trek.quiet_thread(cx);
        let new = trek
            .update(cx, |ws, cx| {
                ws.handoff(&away, &json!({ "brief": "Goal: x. Next: y." }), cx)
            })
            .unwrap();
        assert_eq!(trek.thread_id(cx), here, "no focus stolen");
        let toast = seen
            .borrow()
            .iter()
            .find(|(m, _)| m.starts_with("Handed off"))
            .cloned()
            .expect("a toast");
        assert_eq!(
            toast.1,
            Some(format!("{:?}", UndoAction::OpenThread(new.clone())))
        );
        // Its button opens the thread.
        trek.update(cx, |ws, cx| {
            ws.undo(UndoAction::OpenThread(new.clone()), cx)
        });
        assert_eq!(trek.thread_id(cx), new);
        trek.wait_done(cx, &new, RunState::Idle).await;
    });
}

#[test]
fn thread_tools_reach_only_threads_the_agent_started() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let other = trek.quiet_thread(cx);
        let new = trek
            .update(cx, |ws, cx| {
                ws.start_agent_thread(
                    &id,
                    &json!({ "prompt": "mock:long 30s", "title": "Long one" }),
                    cx,
                )
            })
            .unwrap();
        wait_state(&trek, cx, &new, ThreadState::Working).await;
        // Another thread's agent can't see it, nor message it.
        assert!(
            trek.read(cx, |ws, _| ws.thread_status(&other, &new))
                .unwrap_err()
                .contains("didn't start")
        );
        assert!(
            trek.read(cx, |ws, _| ws.read_thread(&other, &new))
                .unwrap_err()
                .contains("didn't start")
        );
        assert!(
            trek.update(cx, |ws, cx| ws.send_to_thread(&other, &new, Some("hi"), cx))
                .unwrap_err()
                .contains("didn't start")
        );
        // Nor can the starter reach a thread of the user's it didn't start.
        assert!(
            trek.update(cx, |ws, cx| ws.send_to_thread(&id, &other, Some("hi"), cx))
                .unwrap_err()
                .contains("didn't start")
        );
        // While it works, a follow-up waits for its turn.
        let sent = trek
            .update(cx, |ws, cx| {
                ws.send_to_thread(&id, &new, Some("and then the docs"), cx)
            })
            .unwrap();
        assert!(sent["note"].as_str().unwrap().contains("waits"), "{sent}");
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&new)), 1);
        let status = trek.read(cx, |ws, _| ws.thread_status(&id, &new)).unwrap();
        assert_eq!(status["status"], "working");
        assert!(
            trek.read(cx, |ws, _| ws.read_thread(&id, &new)).unwrap()["note"]
                .as_str()
                .unwrap()
                .contains("wait true")
        );
        // A thread Trek imported never takes one, whoever's it seems to be.
        trek.update(cx, |ws, cx| ws.interrupt(&new, cx));
        trek.wait_done(cx, &new, RunState::Idle).await;
        let n = new.clone();
        trek.update(cx, |ws, cx| {
            let _ = ws
                .store
                .update_thread(&n, |t| t.imported_title = Some("Long one".into()));
            ws.reload(cx);
        });
        let refused = trek
            .update(cx, |ws, cx| ws.send_to_thread(&id, &new, Some("hi"), cx))
            .unwrap_err();
        assert!(
            refused.contains("only message threads you started"),
            "{refused}"
        );
        // Nor over an approval only the user can give: the message would answer it.
        let asking = trek
            .update(cx, |ws, cx| {
                ws.start_agent_thread(&id, &json!({ "prompt": "permission" }), cx)
            })
            .unwrap();
        trek.wait_needs_you(cx, &asking).await;
        assert_eq!(state(&trek, cx, &asking), ThreadState::NeedsApproval);
        assert!(
            trek.update(cx, |ws, cx| ws.send_to_thread(
                &id,
                &asking,
                Some("yes"),
                cx
            ))
            .unwrap_err()
            .contains("only they can answer")
        );
        trek.update(cx, |ws, cx| ws.interrupt(&asking, cx));
    });
}

#[test]
fn threads_an_agent_starts_never_get_more_access_than_it_has() {
    run(async |cx| {
        let trek = open(cx);
        let id = stored_thread(&trek, cx, |t| t.hand_holding = HandHolding::Supervised);
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.update(cx, |ws, _| {
            ws.live.entry(id.clone()).or_default().plan = true
        });
        let new = trek
            .update(cx, |ws, cx| {
                ws.start_agent_thread(&id, &json!({ "prompt": "plan" }), cx)
            })
            .unwrap();
        let t = trek.read(cx, |ws, _| ws.thread(&new).cloned().unwrap());
        assert_eq!(t.hand_holding, HandHolding::Supervised);
        assert!(
            trek.read(cx, |ws, _| ws.live[&new].plan),
            "planning, it plans too"
        );
        let free = stored_thread(&trek, cx, |t| t.hand_holding = HandHolding::FullAccess);
        let new = trek
            .update(cx, |ws, cx| {
                ws.start_agent_thread(&free, &json!({ "prompt": "hi" }), cx)
            })
            .unwrap();
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&new).map(|t| t.hand_holding)),
            Some(HandHolding::FullAccess),
            "full access only from full access"
        );
        // Sub-agents and side chats start none.
        let kid = stored_thread(&trek, cx, |t| t.parent_id = Some(id.clone()));
        assert!(
            trek.update(cx, |ws, cx| ws.start_agent_thread(
                &kid,
                &json!({ "prompt": "hi" }),
                cx
            ))
            .unwrap_err()
            .contains("sub-agent")
        );
        assert!(
            trek.update(cx, |ws, cx| ws.handoff(&kid, &json!({ "brief": "hi" }), cx))
                .unwrap_err()
                .contains("sub-agent")
        );
        // What it's asked is checked.
        assert!(
            trek.update(cx, |ws, cx| ws.start_agent_thread(&id, &json!({}), cx))
                .unwrap_err()
                .contains("needs a prompt")
        );
        assert!(
            trek.update(cx, |ws, cx| ws.handoff(&id, &json!({}), cx))
                .unwrap_err()
                .contains("needs a brief")
        );
        assert!(
            trek.update(cx, |ws, cx| ws.start_agent_thread(
                &id,
                &json!({ "prompt": "x", "project": "nowhere" }),
                cx
            ))
            .unwrap_err()
            .contains("knows no project")
        );
        assert!(
            trek.update(cx, |ws, cx| ws.start_agent_thread(
                &id,
                &json!({ "prompt": "x", "worktree": true }),
                cx
            ))
            .unwrap_err()
            .contains("isn't a git repository")
        );
        assert!(
            trek.update(cx, |ws, cx| ws.start_agent_thread(
                &id,
                &json!({ "prompt": "x", "agent": "nope" }),
                cx
            ))
            .unwrap_err()
            .contains("can't run “nope”")
        );
    });
}

#[test]
fn chains_of_started_threads_go_three_deep_and_no_further() {
    run(async |cx| {
        let trek = open(cx);
        let top = trek.quiet_thread(cx);
        let one = stored_thread(&trek, cx, |t| t.started_by = Some(top.clone()));
        let two = stored_thread(&trek, cx, |t| t.started_by = Some(one.clone()));
        let three = stored_thread(&trek, cx, |t| t.started_by = Some(two.clone()));
        assert_eq!(
            trek.read(cx, |ws, _| [&top, &one, &two, &three]
                .map(|t| ws.lineage_depth(t))),
            [0, 1, 2, 3]
        );
        let err = trek
            .update(cx, |ws, cx| {
                ws.start_agent_thread(&three, &json!({ "prompt": "hi" }), cx)
            })
            .unwrap_err();
        assert!(err.contains("3 levels down"), "{err}");
        assert_eq!(
            trek.read(cx, |ws, _| ws.list_threads(&three))["this_thread"]["can_start_threads"],
            false
        );
        let fourth = trek
            .update(cx, |ws, cx| {
                ws.start_agent_thread(&two, &json!({ "prompt": "hi" }), cx)
            })
            .expect("one level up there's room");
        assert_eq!(trek.read(cx, |ws, _| ws.lineage_depth(&fourth)), 3);
        // A handoff keeps its place in the chain: it's the same work going on.
        let next = trek
            .update(cx, |ws, cx| {
                ws.handoff(&three, &json!({ "brief": "Goal: x." }), cx)
            })
            .unwrap();
        assert_eq!(
            trek.read(cx, |ws, _| (
                ws.lineage_depth(&next),
                ws.thread(&next).and_then(|t| t.started_by.clone())
            )),
            (3, Some(two.clone()))
        );
        // And the thread that started the work follows its continuation too.
        assert!(started(&trek, cx, &two).contains(&next));
        trek.wait_done(cx, &fourth, RunState::Idle).await;
        trek.wait_done(cx, &next, RunState::Idle).await;
        // The handoff's thread, though the thread that started the work follows it, wakes no one.
        let t2 = two.clone();
        trek.wait(cx, "the wake-up for the thread `two` started", move |ws| {
            wakes_in(ws, &t2) == 1
        })
        .await;
        assert_eq!(
            trek.read(cx, |ws, _| wakes_in(ws, &two)),
            1,
            "only `fourth`'s"
        );
    });
}

#[test]
fn a_thread_runs_at_most_eight_threads_it_started_at_once() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let start = |trek: &Trek, cx: &mut TestAppContext| {
            trek.update(cx, |ws, cx| {
                ws.start_agent_thread(&id, &json!({ "prompt": "mock:long 30s" }), cx)
            })
        };
        for _ in 0..lineage::MAX_RUNNING {
            start(&trek, cx).expect("room for it");
        }
        let err = start(&trek, cx).unwrap_err();
        assert!(err.contains("8 threads it started at work"), "{err}");
        // One finishes: there's room again.
        let first = started(&trek, cx, &id)[0].clone();
        trek.update(cx, |ws, cx| ws.interrupt(&first, cx));
        trek.wait_done(cx, &first, RunState::Idle).await;
        start(&trek, cx).expect("room again");
        for t in started(&trek, cx, &id) {
            trek.update(cx, |ws, cx| ws.interrupt(&t, cx));
        }
        let tid = id.clone();
        trek.wait(cx, "every thread to stop", move |ws| {
            ws.started_threads(&tid)
                .iter()
                .all(|t| ws.thread_state(&t.id) != ThreadState::Working)
        })
        .await;
    });
}

#[test]
fn an_agent_starts_at_most_sixteen_threads_a_request_and_hands_off_three_times_in_a_row() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        // Threads it started earlier that are done by now: they still count until the user speaks.
        for _ in 0..lineage::MAX_PER_REQUEST {
            stored_thread(&trek, cx, |t| t.started_by = Some(id.clone()));
        }
        let err = trek
            .update(cx, |ws, cx| {
                ws.start_agent_thread(&id, &json!({ "prompt": "hi" }), cx)
            })
            .unwrap_err();
        assert!(
            err.contains("16 threads since the user's last message"),
            "{err}"
        );
        // Handed on three times with no word from the user: the fourth is refused.
        let a = trek.quiet_thread(cx);
        let b = stored_thread(&trek, cx, |t| t.continued_from = Some(a.clone()));
        let c = stored_thread(&trek, cx, |t| t.continued_from = Some(b.clone()));
        let d = stored_thread(&trek, cx, |t| t.continued_from = Some(c.clone()));
        let err = trek
            .update(cx, |ws, cx| ws.handoff(&d, &json!({ "brief": "x" }), cx))
            .unwrap_err();
        assert!(err.contains("3 times in a row"), "{err}");
        // The user saying something in between starts the count again.
        trek.update(cx, |ws, _| {
            let live = ws.live.entry(c.clone()).or_default();
            live.loaded = true;
            for text in ["the brief", "and also this"] {
                live.items.push(Item::User {
                    text: text.into(),
                    images: vec![],
                    at: Some(1),
                    resume: None,
                    aside: false,
                });
            }
        });
        let next = trek
            .update(cx, |ws, cx| ws.handoff(&d, &json!({ "brief": "x" }), cx))
            .expect("allowed again");
        trek.wait_done(cx, &next, RunState::Idle).await;
    });
}

#[test]
fn an_agent_starts_a_thread_in_a_worktree_or_another_project() {
    run(async |cx| {
        let trek = open(cx);
        common::make_repo(&trek, cx);
        let id = trek.send(cx, "mock:start-worktree-thread explain the startup");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let new = started(&trek, cx, &id).pop().expect("a thread");
        trek.wait_done(cx, &new, RunState::Idle).await;
        let t = trek.read(cx, |ws, _| ws.thread(&new).cloned().unwrap());
        let wt = t.worktree.clone().expect("a worktree of its own");
        assert!(
            wt.branch.contains("speed-up-the-slow-tests"),
            "named after its title: {}",
            wt.branch
        );
        assert_eq!(t.cwd.as_deref(), Some(wt.path.as_path()));
        assert!(wt.path.exists(), "made for real");
        assert!(trek.answers(cx, &new).contains("How the app starts"));
        // Another project Trek knows, by name.
        let elsewhere = common::new_project("elsewhere");
        let name = elsewhere.file_name().unwrap().to_string_lossy().to_string();
        trek.update(cx, |ws, cx| {
            ws.store.ensure_project(&elsewhere).unwrap();
            ws.reload(cx);
        });
        let there = trek
            .update(cx, |ws, cx| {
                ws.start_agent_thread(&id, &json!({ "prompt": "hi", "project": name }), cx)
            })
            .unwrap();
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&there).and_then(|t| t.cwd.clone())),
            Some(elsewhere)
        );
        trek.wait_done(cx, &there, RunState::Idle).await;
    });
}

/// Wake-up blocks about threads it started in `id`'s transcript.
fn wakes_in(ws: &trek_app::workspace::Workspace, id: &str) -> usize {
    ws.live.get(id).map_or(0, |l| {
        l.items
            .iter()
            .filter_map(|i| {
                if let Item::User { text, .. } = i {
                    Some(text)
                } else {
                    None
                }
            })
            .filter(|t| trek_core::orchestrate::is_thread_wake(t))
            .map(|t| t.matches("[Trek] Thread “").count())
            .sum()
    })
}

#[test]
fn a_thread_the_agent_started_wakes_it_when_it_finishes() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:start-thread mock:long 1s");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let new = started(&trek, cx, &id).pop().expect("a thread");
        // It finishes; its starter takes a turn for the news, as it does for a sub-agent's.
        let caller = id.clone();
        trek.wait(cx, "the wake-up", move |ws| wakes_in(ws, &caller) == 1)
            .await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        let wake = trek.items(cx, &id).into_iter().find_map(|i| match i {
            Item::User { text, .. } if trek_core::orchestrate::is_thread_wake(&text) => Some(text),
            _ => None,
        });
        let wake = wake.unwrap();
        assert!(
            wake.contains("“Speed up the slow tests”")
                && wake.contains(&new)
                && wake.contains(") finished:")
                && wake.contains("The full suite passed")
                && wake.contains("read_thread"),
            "{wake}"
        );
        assert!(
            trek.answers(cx, &id)
                .contains("The thread reported back: The full suite passed"),
            "{}",
            trek.answers(cx, &id)
        );
        assert!(
            trek.rows(cx).contains(&"wake".to_string()),
            "{:?}",
            trek.rows(cx)
        );
        // Once: its own turn ending wakes nothing more.
        cx.background_executor
            .timer(std::time::Duration::from_millis(1500))
            .await;
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| wakes_in(ws, &id)), 1);
        assert!(
            trek.read(cx, |ws, _| ws.store.held_reports().unwrap().is_empty()),
            "nothing left held"
        );
    });
}

#[test]
fn threads_wake_their_starter_as_asked_and_again_after_each_message() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        // notify false: nobody is woken.
        let quiet = trek
            .update(cx, |ws, cx| {
                ws.start_agent_thread(&id, &json!({ "prompt": "hi", "notify": false }), cx)
            })
            .unwrap();
        trek.wait_done(cx, &quiet, RunState::Idle).await;
        cx.background_executor
            .timer(std::time::Duration::from_millis(1500))
            .await;
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| wakes_in(ws, &id)), 0);
        // A thread that needs the user's approval says so.
        let asking = trek
            .update(cx, |ws, cx| {
                ws.start_agent_thread(&id, &json!({ "prompt": "permission" }), cx)
            })
            .unwrap();
        trek.wait_needs_you(cx, &asking).await;
        let caller = id.clone();
        trek.wait(cx, "the wake-up", move |ws| wakes_in(ws, &caller) == 1)
            .await;
        let last_wake = |trek: &Trek, cx: &TestAppContext| {
            trek.items(cx, &id)
                .into_iter()
                .rev()
                .find_map(|i| match i {
                    Item::User { text, .. } if trek_core::orchestrate::is_thread_wake(&text) => {
                        Some(text)
                    }
                    _ => None,
                })
                .unwrap()
        };
        assert!(
            last_wake(&trek, cx).contains("is waiting for the user's approval: "),
            "{}",
            last_wake(&trek, cx)
        );
        trek.update(cx, |ws, cx| ws.interrupt(&asking, cx));
        trek.wait_done(cx, &asking, RunState::Idle).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        // Stopped by the user: no news. A message from its starter wakes it again when it's done.
        assert_eq!(trek.read(cx, |ws, _| wakes_in(ws, &id)), 1);
        trek.update(cx, |ws, cx| {
            ws.send_to_thread(&id, &asking, Some("explain the startup instead"), cx)
        })
        .unwrap();
        let caller = id.clone();
        trek.wait(cx, "the second wake-up", move |ws| {
            wakes_in(ws, &caller) == 2
        })
        .await;
        assert!(
            last_wake(&trek, cx).contains(") finished:"),
            "{}",
            last_wake(&trek, cx)
        );
        trek.wait_done(cx, &id, RunState::Idle).await;
        // Past the cap since the user's last message, threads wake it no more.
        trek.update(cx, |ws, _| {
            let live = ws.live.get_mut(&id).unwrap();
            for _ in 0..lineage::MAX_PER_REQUEST {
                live.items.push(Item::User {
                    text: "[Trek] Thread “x” (Sol, thread t) finished:\n\nok".into(),
                    images: vec![],
                    at: None,
                    resume: None,
                    aside: false,
                });
            }
        });
        let before = trek.read(cx, |ws, _| wakes_in(ws, &id));
        let more = trek
            .update(cx, |ws, cx| {
                ws.start_agent_thread(&id, &json!({ "prompt": "hi" }), cx)
            })
            .unwrap();
        trek.wait_done(cx, &more, RunState::Idle).await;
        cx.background_executor
            .timer(std::time::Duration::from_millis(1500))
            .await;
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| wakes_in(ws, &id)), before, "capped");
    });
}

/// A transcript call from `thread`'s agent, as `trek-mcp orchestrate` passes it on; its answer.
async fn transcript_call(
    trek: &Trek,
    cx: &mut TestAppContext,
    thread: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let (reply, answer) = async_channel::unbounded();
    trek.update(cx, |ws, cx| {
        ws.handle_call(
            trek_app::ipc::Call {
                thread: thread.to_string(),
                method: method.into(),
                params,
                reply,
            },
            cx,
        )
    });
    match answer.recv().await {
        Ok(trek_app::ipc::Reply::Done(result)) => result,
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_agent_can_search_its_thread_s_transcript_word_for_word() {
    run(async |cx| {
        // With the sub-agent tools off, the transcript's are still there.
        let trek = common::open_with(cx, |s| s.tools.orchestration = false);
        let id = trek.send(
            cx,
            "Why does the build fail with E0599-frobnicate in src/parser.rs?",
        );
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.send(cx, "mock:transcript frobnicate parser.rs");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let said = trek.answers(cx, &id);
        assert!(
            said.contains("The transcript says:")
                && said.contains("Why does the build fail with E0599-frobnicate in src/parser.rs?")
                && said.contains("matching line"),
            "{said}"
        );
        // The log is plain text, one entry per item, each under a numbered header.
        let log = trek_core::paths::transcripts_dir().join(format!("{id}.log"));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.starts_with("### #1 L1 turn 1 "), "{text}");
        assert!(
            text.contains(" assistant\n") && text.contains(" turn end (took "),
            "{text}"
        );
        // Lines by number, as the search gave them.
        let read = transcript_call(
            &trek,
            cx,
            &id,
            "transcript_read",
            json!({ "start": 1, "end": 2 }),
        )
        .await
        .unwrap();
        assert!(
            read.as_str()
                .unwrap()
                .contains("2\tWhy does the build fail with E0599-frobnicate"),
            "{read}"
        );
        // Another project's thread is out of reach; a made-up one doesn't exist.
        let other = common::new_project("elsewhere");
        let theirs = trek.read(cx, |ws, _| {
            ws.store
                .create_thread(
                    Some(&other),
                    mock(),
                    None,
                    trek_core::Effort::Medium,
                    HandHolding::Supervised,
                )
                .unwrap()
                .id
        });
        let refused = transcript_call(
            &trek,
            cx,
            &id,
            "transcript_search",
            json!({ "query": "x", "thread": theirs }),
        )
        .await
        .unwrap_err();
        assert!(refused.contains("another project"), "{refused}");
        let missing = transcript_call(
            &trek,
            cx,
            &id,
            "transcript_search",
            json!({ "query": "x", "thread": "no-such-thread" }),
        )
        .await
        .unwrap_err();
        assert!(missing.contains("no thread"), "{missing}");
        // The notes every session gets say so.
        let notes = trek.read(cx, |ws, _| {
            ws.session_notes(None, std::path::Path::new("/"), false)
                .unwrap()
        });
        assert!(notes.contains(trek_core::session_log::AGENT_NOTE));
    });
}
