//! Where features meet: a secret's card with the phone, the side bars, a popped-out window, a
//! switch of agent or account, the queue, a split and a restart; rules that ask every time with
//! every way of answering; diff comments with the agent's own cards; `/compact` with a cut-off
//! chat and a switch; what "Needs you" makes of a chat cut off while it asked; and what the phone
//! is told of all of it.

mod common;

use common::{Trek, mock, open, run, toasts};
use gpui_kit::{AppContext as _, TestAppContext};
use std::sync::{Mutex, MutexGuard};
use trek_agents::{Command, Decision};
use trek_app::ipc::Reply;
use trek_app::workspace::split::Pane;
use trek_app::workspace::{PendingPermission, Prefs, Route, Scope, WorkspaceEvent};
use trek_core::permission_rules::{Call, PermissionRule, RuleAction, RuleKind};
use trek_core::settings::FollowUp;
use trek_core::store::Item;
use trek_core::{AgentId, RunState};
use trek_remote as tr;

/// Secrets given and the rules in force are the process's, and every workspace that opens or
/// quits forgets the first: one test at a time.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static ONE: Mutex<()> = Mutex::new(());
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}

const PASTED: &str = "ghp_pasted-in-the-wrong-place";

/// A thread with its turn over.
async fn quiet(trek: &Trek, cx: &mut TestAppContext) -> String {
    let id = trek.send(cx, "hello");
    trek.wait_done(cx, &id, RunState::Idle).await;
    id
}

/// The agent's side of a request for a secret: its reply when it comes.
fn ask_secret(trek: &Trek, cx: &mut TestAppContext, id: &str) -> async_channel::Receiver<Reply> {
    let (tx, rx) = async_channel::bounded(1);
    trek.update(cx, |ws, cx| {
        ws.ask_for_secret(id, "GITHUB_TOKEN", "To open the pull request.", tx, cx)
    });
    trek.render(cx);
    rx
}

/// The secret, typed into its card.
fn give_secret(trek: &Trek, cx: &mut TestAppContext, id: &str, value: &str) {
    let request = trek.request(cx, id);
    trek.update(cx, |ws, cx| {
        ws.answer(id, &request, vec![(String::new(), value.to_string())], cx)
    });
}

fn secret_card_up(trek: &Trek, cx: &TestAppContext, id: &str) -> bool {
    trek.read(cx, |ws, _| ws.asks_for_secret(id))
}

/// Nowhere it could be kept: the transcript, the store, the queue and the unsent messages.
fn kept_nowhere(trek: &Trek, cx: &mut TestAppContext, id: &str, value: &str) {
    let items = format!("{:?}", trek.items(cx, id));
    assert!(!items.contains(value), "in the transcript: {items}");
    trek.update(cx, |ws, cx| ws.persist_for_test(id, cx));
    let stored = trek.read(cx, |ws, _| ws.store.items(id).unwrap());
    assert!(
        !format!("{stored:?}").contains(value),
        "in the store: {stored:?}"
    );
    let queued = trek.read(cx, |ws, _| {
        format!("{:?}", ws.live.get(id).map(|l| &l.queued))
    });
    assert!(!queued.contains(value), "queued: {queued}");
    trek.update(cx, |ws, _| ws.save_unsent());
    let unsent = trek.read(cx, |ws, _| ws.store.unsent_messages().unwrap());
    assert!(!format!("{unsent:?}").contains(value), "unsent: {unsent:?}");
    let title = trek.read(cx, |ws, _| ws.thread(id).map(|t| t.title.clone()));
    assert!(!title.unwrap_or_default().contains(value));
}

/// Make a request the way the phone's server does and take the reply.
fn ask<T>(
    trek: &Trek,
    cx: &mut TestAppContext,
    make: impl FnOnce(tr::Reply<T>) -> tr::HostRequest,
) -> tr::HostResult<T> {
    let (reply, mut rx) = tokio::sync::oneshot::channel();
    trek.update(cx, |ws, cx| ws.remote_request(make(reply), cx));
    cx.run_until_parked();
    rx.try_recv().expect("answered")
}

fn phone_send(
    id: &str,
    text: &str,
    mode: Option<tr::SendMode>,
) -> impl FnOnce(tr::Reply<Option<tr::Open>>) -> tr::HostRequest {
    let req = tr::SendRequest {
        thread_id: id.to_string(),
        text: text.to_string(),
        mode,
        images: vec![],
        consult: None,
        files: vec![],
    };
    move |reply| tr::HostRequest::Send { req, reply }
}

fn transcript(
    id: &str,
    after_seq: Option<u64>,
) -> impl FnOnce(tr::Reply<tr::Transcript>) -> tr::HostRequest {
    let thread_id = id.to_string();
    move |reply| tr::HostRequest::Transcript {
        thread_id,
        after_seq,
        limit: None,
        reply,
    }
}

/// What a thread's session is sent from here on, in place of its agent. The agent's own end is
/// kept open meanwhile: closed, it would end the session.
struct FakeSession {
    rx: async_channel::Receiver<Command>,
    _real: Option<async_channel::Sender<Command>>,
}

impl FakeSession {
    fn try_recv(&self) -> Result<Command, async_channel::TryRecvError> {
        self.rx.try_recv()
    }
}

fn fake_session(trek: &Trek, cx: &mut TestAppContext, id: &str) -> FakeSession {
    let (tx, rx) = async_channel::unbounded();
    let real = trek.update(cx, |ws, _| {
        ws.live.get_mut(id).unwrap().commands.replace(tx)
    });
    FakeSession { rx, _real: real }
}

fn prompts(session: &FakeSession) -> Vec<String> {
    std::iter::from_fn(|| session.try_recv().ok())
        .filter_map(|c| match c {
            Command::Prompt { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

/// An approval card on `id`, for running `command`.
fn approval(trek: &Trek, cx: &mut TestAppContext, id: &str, request_id: &str, command: &str) {
    trek.update(cx, |ws, cx| {
        let live = ws.live.get_mut(id).unwrap();
        live.permissions.push(PendingPermission {
            request_id: request_id.into(),
            title: "Run command".into(),
            detail: command.into(),
            prompt: None,
            after_turn: false,
            call: Some(Call::command("Bash", command)),
        });
        live.revision += 1;
        cx.notify();
    });
}

fn ask_rule(trek: &Trek, cx: &mut TestAppContext, pattern: &str) {
    trek.update(cx, |ws, cx| {
        ws.settings.permissions.rules = vec![PermissionRule::new(
            RuleKind::Command,
            pattern,
            RuleAction::Ask,
        )];
        ws.save_settings(cx);
    });
}

fn no_rules(trek: &Trek, cx: &mut TestAppContext) {
    trek.update(cx, |ws, cx| {
        ws.settings.permissions.rules.clear();
        ws.save_settings(cx);
    });
}

/// The events `trek`'s workspace emits from now on that put words back into a composer.
fn put_back(trek: &Trek, cx: &mut TestAppContext) -> std::rc::Rc<std::cell::RefCell<Vec<String>>> {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
    let sink = seen.clone();
    cx.update(|cx| {
        cx.subscribe(&trek.ws, move |_, event: &WorkspaceEvent, _| match event {
            WorkspaceEvent::RestoreQueued { thread, text, .. } => {
                sink.borrow_mut().push(format!("restore {thread}: {text}"))
            }
            WorkspaceEvent::InsertIntoComposer(text) => {
                sink.borrow_mut().push(format!("insert: {text}"))
            }
            _ => {}
        })
        .detach()
    });
    seen
}

fn relay() -> AgentId {
    AgentId::Direct(trek_core::catalog::MOCK_RELAY_PROVIDER.into())
}

/// A pick in `scope`'s model menu, as the composer makes it.
fn pick(trek: &Trek, cx: &mut TestAppContext, scope: &Scope, f: impl FnOnce(&mut Prefs)) {
    let scope = scope.clone();
    trek.update(cx, |ws, cx| {
        let mut prefs = ws.chosen_prefs_in(&scope);
        f(&mut prefs);
        ws.choose_prefs_in(&scope, prefs, cx);
    });
}

// ---------- a secret's card ----------

#[test]
fn a_phone_message_sent_with_a_secrets_card_up_goes_nowhere() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let reply = ask_secret(&trek, cx, &id);
        trek.update(cx, |ws, _| ws.start_test_remote());
        // Pasted into the phone's composer: refused, steered, queued or as a command alike.
        for (text, mode) in [
            (PASTED.to_string(), None),
            (PASTED.to_string(), Some(tr::SendMode::Queue)),
            (PASTED.to_string(), Some(tr::SendMode::Steer)),
            (format!("/consult {PASTED}"), None),
        ] {
            let sent = ask(&trek, cx, phone_send(&id, &text, mode));
            let err = sent.expect_err("refused");
            assert_eq!(err.code, tr::ErrorCode::Conflict, "{err:?}");
            assert!(err.message.contains("card"), "{}", err.message);
        }
        assert!(secret_card_up(&trek, cx, &id), "the card is still up");
        assert!(reply.try_recv().is_err(), "nothing went to the agent");
        kept_nowhere(&trek, cx, &id, PASTED);
        assert_eq!(
            trek.composer_text(cx),
            "",
            "nor onto this computer's composer"
        );
        // Once it's given, the phone's messages go again.
        give_secret(&trek, cx, &id, "ghp_the-real-one");
        ask(&trek, cx, phone_send(&id, "thanks", None)).unwrap();
        assert!(
            trek.items(cx, &id)
                .iter()
                .any(|i| matches!(i, Item::User { text, .. } if text == "thanks"))
        );
    });
}

#[test]
fn a_plans_feedback_from_the_phone_waits_for_a_secrets_card_too() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        trek.update(cx, |ws, _| {
            ws.live
                .get_mut(&id)
                .unwrap()
                .permissions
                .push(PendingPermission {
                    request_id: "plan-1".into(),
                    title: "Plan".into(),
                    detail: String::new(),
                    prompt: Some(trek_agents::Prompt::Plan("1. Do it".into())),
                    after_turn: false,
                    call: None,
                });
        });
        let _reply = ask_secret(&trek, cx, &id);
        trek.update(cx, |ws, _| ws.start_test_remote());
        let answer = ask(&trek, cx, |reply| tr::HostRequest::Answer {
            req: tr::AnswerRequest {
                thread_id: id.clone(),
                request_id: "plan-1".into(),
                response: tr::AnswerResponse::Plan {
                    approve: false,
                    feedback: Some(PASTED.into()),
                },
            },
            reply,
        });
        assert_eq!(answer.unwrap_err().code, tr::ErrorCode::Conflict);
        assert!(
            trek.read(cx, |ws, _| ws.live[&id]
                .permissions
                .iter()
                .any(|p| p.request_id == "plan-1")),
            "the plan is still up, unanswered"
        );
        kept_nowhere(&trek, cx, &id, PASTED);
    });
}

#[test]
fn the_editor_side_bar_keeps_a_secret_back_as_the_composer_does() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.send_ide("hello".into(), vec![], FollowUp::Steer, cx)
        });
        let id = trek
            .read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string))
            .expect("the side bar's chat");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let reply = ask_secret(&trek, cx, &id);
        let back = put_back(&trek, cx);
        for follow in [FollowUp::Queue, FollowUp::Steer] {
            trek.update(cx, |ws, cx| ws.send_ide(PASTED.into(), vec![], follow, cx));
        }
        assert!(secret_card_up(&trek, cx, &id));
        assert!(reply.try_recv().is_err());
        kept_nowhere(&trek, cx, &id, PASTED);
        assert_eq!(
            back.borrow().as_slice(),
            [
                format!("restore {id}: {PASTED}"),
                format!("restore {id}: {PASTED}")
            ],
            "back to the composer showing the thread, not 'the composer'"
        );
    });
}

#[test]
fn a_pending_switch_doesnt_take_the_secrets_card_down() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let reply = ask_secret(&trek, cx, &id);
        // Another agent picked: it waits for the next message.
        pick(&trek, cx, &Scope::Main, |p| {
            p.agent = relay();
            p.model = None;
        });
        assert!(trek.read(cx, |ws, _| ws.pending_switch(&id).is_some()));
        trek.send(cx, PASTED);
        assert!(secret_card_up(&trek, cx, &id), "the card is still up");
        assert!(reply.try_recv().is_err(), "nothing went to either agent");
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().agent.clone()),
            mock(),
            "no move yet"
        );
        assert!(trek.read(cx, |ws, _| ws.pending_switch(&id).is_some()));
        kept_nowhere(&trek, cx, &id, PASTED);
        assert_eq!(trek.composer_text(cx), PASTED, "back in the composer");
    });
}

#[test]
fn in_a_thread_window_the_guard_puts_it_back_in_that_window() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let own = trek.open_thread_window(cx, &id);
        // The main window goes on to a new thread's draft.
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project),
                },
                cx,
            )
        });
        trek.render(cx);
        let reply = ask_secret(&trek, cx, &id);
        let composer = cx
            .update_window(own, |root, _, cx| {
                let view = root
                    .downcast::<gpui_kit::component::Root>()
                    .expect("root")
                    .read(cx)
                    .view()
                    .clone();
                view.downcast::<trek_app::thread_window::ThreadWindow>()
                    .expect("a thread window")
                    .read(cx)
                    .composer()
                    .clone()
            })
            .expect("window");
        cx.update_window(own, |_, window, cx| {
            composer.update(cx, |c, cx| {
                c.set_text(PASTED, window, cx);
                c.send_now(window, cx);
            })
        })
        .expect("window");
        cx.run_until_parked();
        assert!(secret_card_up(&trek, cx, &id));
        assert!(reply.try_recv().is_err());
        assert_eq!(
            cx.read(|cx| composer.read(cx).text(cx)),
            PASTED,
            "back where it was typed"
        );
        assert_eq!(trek.composer_text(cx), "", "not in the new thread's draft");
        // Return there now starts no thread with it.
        let threads = trek.read(cx, |ws, _| ws.threads.len());
        trek.press(cx, "enter");
        assert_eq!(trek.read(cx, |ws, _| ws.threads.len()), threads);
        kept_nowhere(&trek, cx, &id, PASTED);
    });
}

#[test]
fn a_token_the_guard_puts_back_isnt_kept_for_the_next_launch() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        trek.type_text(cx, "a draft from before");
        trek.update(cx, |ws, _| ws.save_unsent());
        let _reply = ask_secret(&trek, cx, &id);
        // Select what's there and paste over it: nothing of it is noted while the card is up.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        trek.window(cx, |window, cx| {
            composer.update(cx, |c, cx| c.set_text("", window, cx))
        });
        trek.send(cx, PASTED);
        assert_eq!(trek.composer_text(cx), PASTED, "put back");
        trek.type_text(cx, " and more");
        trek.update(cx, |ws, _| ws.save_unsent());
        let unsent = trek.read(cx, |ws, _| ws.store.unsent_messages().unwrap());
        assert!(!format!("{unsent:?}").contains(PASTED), "{unsent:?}");
        // What was kept from before the card is still kept.
        assert!(
            unsent.iter().any(|m| m.text == "a draft from before"),
            "{unsent:?}"
        );
        // Given in the card, the value is taken out of anything kept from then on.
        give_secret(&trek, cx, &id, PASTED);
        trek.type_text(cx, "!");
        trek.update(cx, |ws, _| ws.save_unsent());
        let unsent = trek.read(cx, |ws, _| ws.store.unsent_messages().unwrap());
        assert!(!format!("{unsent:?}").contains(PASTED), "{unsent:?}");
        assert!(
            format!("{unsent:?}").contains("[GITHUB_TOKEN hidden]"),
            "{unsent:?}"
        );
        // Even as Trek quits, after the secrets' own files have gone.
        cx.quit();
        let unsent = trek.read(cx, |ws, _| ws.store.unsent_messages().unwrap());
        assert!(!format!("{unsent:?}").contains(PASTED), "{unsent:?}");
    });
}

#[test]
fn a_queued_message_changed_with_a_secrets_card_up_isnt_queued_with_it() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.settings.general.follow_up = FollowUp::Queue;
            ws.save_settings(cx);
        });
        let id = trek.send(cx, "mock:long 600s");
        let tid = id.clone();
        trek.wait(cx, "the long turn", move |ws| {
            ws.live
                .get(&tid)
                .is_some_and(|l| l.turn_started.is_some() && l.commands.is_some())
        })
        .await;
        trek.send(cx, "then deploy");
        let _reply = ask_secret(&trek, cx, &id);
        trek.update(cx, |ws, _| ws.start_test_remote());
        // The phone's edit of the row.
        let edit = ask(&trek, cx, |reply| tr::HostRequest::QueueAction {
            req: tr::QueueActionRequest {
                thread_id: id.clone(),
                index: 0,
                reset: false,
                text: "then deploy".into(),
                action: tr::QueueAction::Edit {
                    text: PASTED.into(),
                },
            },
            reply,
        });
        assert_eq!(edit.unwrap_err().code, tr::ErrorCode::Conflict);
        // The Mac's: the row taken into the prompt, the token pasted over it, Return.
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        trek.window(cx, |window, cx| {
            composer.update(cx, |c, cx| {
                c.edit_queued_for_test(&id, 0, window, cx);
                c.set_text(PASTED, window, cx);
            })
        });
        trek.press(cx, "enter");
        assert_eq!(trek.composer_text(cx), PASTED, "it stays in the prompt");
        kept_nowhere(&trek, cx, &id, PASTED);
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
    });
}

#[test]
fn a_given_secret_stays_out_of_messages_and_titles_from_anywhere() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let _reply = ask_secret(&trek, cx, &id);
        let value = "ghp_handed-on-by-an-agent";
        give_secret(&trek, cx, &id, value);
        // Another agent's message to this thread (`send_to_thread`, a sub-agent's report).
        trek.update(cx, |ws, cx| {
            ws.send_to(&id, format!("Use {value} for it"), vec![], cx)
        });
        kept_nowhere(&trek, cx, &id, value);
        // A new thread an agent starts with it in its brief: its first message and its title.
        let project = trek.project.clone();
        let other = trek.update(cx, |ws, cx| {
            let prefs = ws.prefs_in(&Scope::Main);
            let text = format!("{value} deploy");
            let other = ws
                .start_thread(Some(project), prefs, &text, &[], cx)
                .expect("a thread");
            ws.send_to(&other, text, vec![], cx);
            other
        });
        kept_nowhere(&trek, cx, &other, value);
    });
}

#[test]
fn a_phone_cant_download_a_secrets_file_an_answer_links() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let _reply = ask_secret(&trek, cx, &id);
        give_secret(&trek, cx, &id, "ghp_in-a-file");
        let file = trek_core::paths::data_dir()
            .join("session-secrets")
            .join(&id)
            .join("GITHUB_TOKEN");
        assert!(file.is_file(), "{file:?}");
        // A link to it in the project, where links can be made without asking.
        #[cfg(unix)]
        std::os::unix::fs::symlink(&file, trek.project.join("token.png")).unwrap();
        trek.update(cx, |ws, _| {
            let live = ws.live.get_mut(&id).unwrap();
            live.items.push(Item::Assistant {
                text: format!(
                    "It's in [the file]({}), and ![here](token.png).",
                    file.display()
                ),
            });
            live.revision += 1;
            ws.start_test_remote();
        });
        let mut asked = vec![file.display().to_string()];
        if cfg!(unix) {
            asked.push("token.png".into());
        }
        for path in asked {
            let (reply, mut rx) = tokio::sync::oneshot::channel();
            let req = tr::MediaRequest {
                thread_id: id.clone(),
                path: path.clone(),
                max_px: None,
                offset: 0,
                length: None,
            };
            trek.update(cx, |ws, cx| {
                ws.remote_request(tr::HostRequest::Media { req, reply }, cx)
            });
            let deadline = std::time::Instant::now() + common::REAL_TIME_LIMIT;
            let answer = loop {
                cx.run_until_parked();
                if let Ok(answer) = rx.try_recv() {
                    break answer;
                }
                assert!(std::time::Instant::now() < deadline, "no answer");
                cx.background_executor
                    .timer(std::time::Duration::from_millis(5))
                    .await;
            };
            assert!(answer.is_err(), "{path}: {answer:?}");
        }
    });
}

#[test]
fn the_phone_is_told_a_secrets_card_has_closed() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let _reply = ask_secret(&trek, cx, &id);
        trek.update(cx, |ws, _| ws.start_test_remote());
        let request = trek.request(cx, &id);
        let first = ask(&trek, cx, transcript(&id, None)).unwrap();
        let card = |t: &tr::Transcript| {
            t.items
                .iter()
                .find(|i| i.id == format!("r{request}"))
                .map(|i| i.body.clone())
        };
        let Some(tr::ItemBody::Notice { text }) = card(&first) else {
            panic!("{first:?}");
        };
        assert!(text.contains("Enter it on"), "{text}");
        give_secret(&trek, cx, &id, "ghp_given-on-the-mac");
        use trek_app::remote::tests::EVENTS_PUSHED;
        EVENTS_PUSHED.with(|e| e.borrow_mut().clear());
        trek.update(cx, |ws, cx| ws.push_remote_changes(cx));
        let closed = EVENTS_PUSHED.with(|e| {
            e.borrow().iter().find_map(|event| match event {
                tr::HostEvent::Item { item, .. } if item.id == format!("r{request}") => {
                    Some(item.body.clone())
                }
                _ => None,
            })
        });
        let Some(tr::ItemBody::Notice { text }) = closed else {
            panic!("the card's row closes: {closed:?}");
        };
        assert!(!text.contains("Enter it on"), "{text}");
        assert!(text.contains("GITHUB_TOKEN"), "{text}");
        assert!(!text.contains("ghp_given"), "{text}");
    });
}

#[test]
fn a_sub_agent_waiting_on_a_secret_isnt_an_approval_on_the_phone() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let parent = quiet(&trek, cx).await;
        let kid = trek.update(cx, |ws, cx| {
            let mut t = ws
                .store
                .create_thread(
                    Some(&trek.project),
                    mock(),
                    None,
                    trek_core::Effort::Medium,
                    trek_core::HandHolding::Auto,
                )
                .unwrap();
            t.parent_id = Some(parent.clone());
            ws.store.save_thread(&t).unwrap();
            ws.reload(cx);
            ws.live.entry(t.id.clone()).or_default();
            t.id
        });
        let _reply = ask_secret(&trek, cx, &kid);
        trek.update(cx, |ws, _| ws.start_test_remote());
        let snapshot = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap();
        let row = snapshot.threads.iter().find(|t| t.id == parent).unwrap();
        assert_eq!(row.needs_thread.as_deref(), Some(kid.as_str()));
        assert_eq!(
            row.needs.as_ref().map(|n| n.kind),
            Some(tr::NeedsKind::Question)
        );
    });
}

/// The isolated app's `control` (built with `shots`: `cargo test --features shots`).
#[cfg(feature = "shots")]
#[test]
fn control_approve_on_a_secrets_card_says_why_rather_than_crash() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let _reply = ask_secret(&trek, cx, &id);
        for verb in ["approve", "answer"] {
            let said = trek.update(cx, |ws, cx| {
                trek_app::shots::control_card(ws, verb, "1", cx)
            });
            let why = said.expect_err(verb).to_string();
            assert!(why.contains("in the card"), "{why}");
        }
        trek.update(cx, |ws, cx| {
            trek_app::shots::control_card(ws, "deny", "", cx)
        })
        .unwrap();
        assert!(!secret_card_up(&trek, cx, &id));
    });
}

// ---------- rules that ask every time ----------

#[test]
fn allow_for_session_is_once_where_a_rule_asks_every_time() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        ask_rule(&trek, cx, "./deploy.sh");
        let session = fake_session(&trek, cx, &id);
        // The editor's card and anything else that answers through `respond`.
        approval(&trek, cx, &id, "r1", "./deploy.sh --prod");
        assert!(!trek.read(cx, |ws, _| ws.session_approval_offered(&id, "r1")));
        assert!(
            trek.read(cx, |ws, _| ws.rule_asking(&id, "r1"))
                .is_some_and(|r| r.contains("./deploy.sh"))
        );
        trek.update(cx, |ws, cx| {
            ws.respond(&id, "r1", Decision::AllowForSession, cx)
        });
        let sent: Vec<Decision> = std::iter::from_fn(|| session.try_recv().ok())
            .filter_map(|c| match c {
                Command::Respond { decision, .. } => Some(decision),
                _ => None,
            })
            .collect();
        assert_eq!(sent, [Decision::Allow], "once, not for the session");

        // The phone: told there's no session's yes, and refused one from a phone that offers it.
        trek.update(cx, |ws, cx| {
            ws.settings.mobile.session_approvals = true;
            ws.save_settings(cx);
            ws.start_test_remote();
        });
        approval(&trek, cx, &id, "r2", "./deploy.sh --prod");
        let t = ask(&trek, cx, transcript(&id, None)).unwrap();
        let asked_by = t.items.iter().find_map(|i| match &i.body {
            tr::ItemBody::Approval {
                request_id,
                asked_by,
                ..
            } if request_id == "r2" => Some(asked_by.clone()),
            _ => None,
        });
        assert!(
            asked_by
                .flatten()
                .is_some_and(|r| r.contains("./deploy.sh")),
            "{t:?}"
        );
        let answer = ask(&trek, cx, |reply| tr::HostRequest::Answer {
            req: tr::AnswerRequest {
                thread_id: id.clone(),
                request_id: "r2".into(),
                response: tr::AnswerResponse::Approval {
                    decision: tr::Decision::AllowForSession,
                },
            },
            reply,
        });
        assert_eq!(answer.unwrap_err().code, tr::ErrorCode::BadRequest);
        assert!(trek.read(cx, |ws, _| {
            ws.live[&id]
                .permissions
                .iter()
                .any(|p| p.request_id == "r2")
        }));

        // Without the rule, the session's yes is the session's.
        no_rules(&trek, cx);
        assert!(trek.read(cx, |ws, _| ws.session_approval_offered(&id, "r2")));
        trek.update(cx, |ws, cx| {
            ws.respond(&id, "r2", Decision::AllowForSession, cx)
        });
        let sent: Vec<Decision> = std::iter::from_fn(|| session.try_recv().ok())
            .filter_map(|c| match c {
                Command::Respond { decision, .. } => Some(decision),
                _ => None,
            })
            .collect();
        assert_eq!(sent, [Decision::AllowForSession]);
    });
}

// ---------- the phone ----------

#[test]
fn a_message_typed_on_the_phone_declines_the_card_as_on_the_mac() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let session = fake_session(&trek, cx, &id);
        approval(&trek, cx, &id, "r1", "rm -rf build");
        trek.update(cx, |ws, cx| {
            ws.live.get_mut(&id).unwrap().turn_started = Some(std::time::Instant::now());
            ws.settings.general.follow_up = FollowUp::Queue;
            ws.save_settings(cx);
            ws.start_test_remote();
        });
        ask(
            &trek,
            cx,
            phone_send(&id, "Don't, clean it with make instead", None),
        )
        .unwrap();
        let sent: Vec<Command> = std::iter::from_fn(|| session.try_recv().ok()).collect();
        assert!(
            sent.iter().any(|c| matches!(c, Command::Respond { request_id, decision: Decision::Deny } if request_id == "r1")),
            "{sent:?}"
        );
        assert!(
            sent.iter().any(
                |c| matches!(c, Command::Prompt { text, .. } if text.contains("make instead"))
            ),
            "the words go now, not behind the card they answer: {sent:?}"
        );
        assert!(trek.read(cx, |ws, _| ws.live[&id].permissions.is_empty()));
        assert!(trek.read(cx, |ws, _| ws.live[&id].queued.is_empty()));
    });
}

#[test]
fn a_one_tap_allow_only_for_a_command_sent_whole() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let long = format!("./migrate.sh {} && rm -rf /", "--table x ".repeat(400));
        approval(&trek, cx, &id, "r1", &long);
        trek.update(cx, |ws, _| ws.start_test_remote());
        let row = |trek: &Trek, cx: &mut TestAppContext| {
            let snapshot = ask(trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap();
            snapshot.threads.into_iter().find(|t| t.id == id).unwrap()
        };
        assert_eq!(
            row(&trek, cx).ask,
            None,
            "its middle would be cut out to fit"
        );
        trek.update(cx, |ws, _| {
            let live = ws.live.get_mut(&id).unwrap();
            live.permissions.clear();
            live.revision += 1;
        });
        approval(&trek, cx, &id, "r2", "cargo test");
        assert_eq!(
            row(&trek, cx).ask.map(|a| (a.request_id, a.detail)),
            Some(("r2".to_string(), "cargo test".to_string()))
        );
    });
}

#[test]
fn a_thread_started_on_the_phone_runs_on_the_agents_own_login() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let work = trek.update(cx, |ws, cx| {
            let id = ws.add_account(&mock(), cx).expect("an account");
            ws.rename_account(&id, "Work", cx);
            id
        });
        // The Mac's draft is on the Work account.
        trek.update(cx, |ws, cx| {
            let prefs = Prefs {
                account: Some(work.clone()),
                ..ws.prefs_in(&Scope::Main)
            };
            ws.set_prefs_in(&Scope::Main, prefs, cx);
        });
        trek.update(cx, |ws, _| ws.start_test_remote());
        let id = ask(&trek, cx, |reply| tr::HostRequest::NewThread {
            req: tr::NewThreadRequest {
                project_id: String::new(),
                agent: mock().key(),
                model: None,
                text: "from the phone".into(),
                worktree: false,
                effort: None,
                access: None,
                plan: false,
                images: vec![],
                files: vec![],
            },
            reply,
        })
        .unwrap();
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().account.clone()),
            None
        );
        assert_eq!(
            trek.read(cx, |ws, _| ws.prefs_in(&Scope::Main).account),
            Some(work),
            "the Mac's draft keeps its pick"
        );
    });
}

// ---------- switching agent and account ----------

#[test]
fn an_account_picked_with_a_switch_waiting_goes_with_the_switch() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        // A thread on the relay, moving to the mock agent, which has a Work account.
        trek.update(cx, |ws, _| ws.draft_prefs.agent = relay());
        let id = quiet(&trek, cx).await;
        let work = trek.update(cx, |ws, cx| {
            let id = ws.add_account(&mock(), cx).expect("an account");
            ws.rename_account(&id, "Work", cx);
            id
        });
        pick(&trek, cx, &Scope::Main, |p| {
            p.agent = mock();
            p.model = None;
        });
        // The menu offers the mock agent's accounts, not the relay's (none).
        let choices = trek.read(cx, |ws, _| {
            let prefs = ws.chosen_prefs_in(&Scope::Main);
            ws.account_choices_for(&Scope::Main, &prefs.agent)
        });
        assert!(
            choices
                .iter()
                .any(|c| c.id.as_deref() == Some(work.as_str())),
            "{choices:?}"
        );
        pick(&trek, cx, &Scope::Main, |p| p.account = Some(work.clone()));
        assert_eq!(
            trek.read(cx, |ws, _| ws
                .pending_switch(&id)
                .map(|p| (p.agent.clone(), p.account.clone()))),
            Some((mock(), Some(work.clone()))),
            "the pick is kept"
        );
        assert_eq!(
            trek.read(cx, |ws, _| ws.account_label_for(
                &Scope::Main,
                &ws.chosen_prefs_in(&Scope::Main)
            )),
            Some("Work".to_string())
        );
        trek.send(cx, "go on");
        assert_eq!(
            trek.read(cx, |ws, _| {
                let t = ws.thread(&id).unwrap();
                (t.agent.clone(), t.account.clone())
            }),
            (mock(), Some(work))
        );
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

#[test]
fn another_account_of_the_same_agent_waits_for_the_next_message() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let work = trek.update(cx, |ws, cx| {
            let id = ws.add_account(&mock(), cx).expect("an account");
            ws.rename_account(&id, "Work", cx);
            id
        });
        let id = trek.send(cx, "mock:long 600s");
        let tid = id.clone();
        trek.wait(cx, "the long turn", move |ws| {
            ws.live
                .get(&tid)
                .is_some_and(|l| l.turn_started.is_some() && l.commands.is_some())
        })
        .await;
        pick(&trek, cx, &Scope::Main, |p| p.account = Some(work.clone()));
        assert!(
            trek.read(cx, |ws, _| ws.turn_running(&id)),
            "the turn goes on"
        );
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().account.clone()),
            None
        );
        trek.render(cx);
        assert!(
            trek.visible(cx, "switch-bar"),
            "and the bar says what's waiting"
        );
        // Back to where it is: nothing waits.
        pick(&trek, cx, &Scope::Main, |p| p.account = None);
        assert!(trek.read(cx, |ws, _| ws.pending_switch(&id).is_none()));
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
    });
}

#[test]
fn compact_with_a_switch_waiting_compacts_and_the_switch_waits_on() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        pick(&trek, cx, &Scope::Main, |p| {
            p.agent = relay();
            p.model = None;
        });
        let session = fake_session(&trek, cx, &id);
        trek.send(cx, "/compact");
        assert_eq!(prompts(&session), ["/compact"], "to the session there is");
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().agent.clone()),
            mock()
        );
        assert!(trek.read(cx, |ws, _| ws.pending_switch(&id).is_some()));
    });
}

#[test]
fn a_switch_waiting_is_still_there_after_a_restart() {
    let _one = one_at_a_time();
    run(async |cx| {
        let dir = common::new_project("switch-restart");
        let db = dir.join("trek.sqlite");
        let launch = |cx: &mut TestAppContext| {
            let mut s = common::settings();
            s.user_projects.push(dir.display().to_string());
            let (ws, root, window) =
                common::launch(cx, trek_core::store::Store::open(&db).unwrap(), s);
            Trek {
                ws,
                root,
                window,
                project: dir.clone(),
            }
        };
        let trek = launch(cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(dir.clone()),
                },
                cx,
            )
        });
        trek.render(cx);
        let id = quiet(&trek, cx).await;
        pick(&trek, cx, &Scope::Main, |p| {
            p.agent = relay();
            p.model = None;
        });
        trek.update(cx, |ws, _| ws.save_unsent());
        trek.window(cx, |window, _| window.remove_window());

        let again = launch(cx);
        assert_eq!(
            again.read(cx, |ws, _| ws.pending_switch(&id).map(|p| p.agent.clone())),
            Some(relay())
        );
        again.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        again.render(cx);
        assert!(again.visible(cx, "switch-bar"));
    });
}

// ---------- diff comments ----------

#[test]
fn diff_comments_wait_for_a_question_and_make_the_switch() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let shown = toasts(&trek, cx);
        trek.update(cx, |ws, _| {
            ws.live
                .get_mut(&id)
                .unwrap()
                .permissions
                .push(PendingPermission {
                    request_id: "q1".into(),
                    title: "Question".into(),
                    detail: String::new(),
                    prompt: Some(trek_agents::Prompt::Questions(vec![
                        trek_agents::Question {
                            header: "DB".into(),
                            question: "Which database?".into(),
                            options: vec![("Postgres".into(), String::new())],
                            multi: false,
                            secret: false,
                        },
                    ])),
                    after_turn: false,
                    call: None,
                });
        });
        let comments = "Comments on the diff:\n\n`src/a.rs:3`\nRename this.";
        let scope = Scope::Thread(id.clone());
        let went = trek.update(cx, |ws, cx| {
            ws.send_comments_in(&scope, comments.into(), cx)
        });
        assert!(!went, "not the question's answer");
        assert!(
            trek.read(cx, |ws, _| ws.live[&id].permissions.len()) == 1,
            "the question is still up"
        );
        assert!(
            !trek
                .items(cx, &id)
                .iter()
                .any(|i| matches!(i, Item::User { text, .. } if text.contains("Rename this"))),
        );
        assert!(
            shown
                .borrow()
                .iter()
                .any(|(m, _)| m.contains("Answer it first")),
            "{:?}",
            shown.borrow()
        );
        // Answered, they go; and a switch waiting for the next message happens with them.
        trek.update(cx, |ws, _| {
            ws.live.get_mut(&id).unwrap().permissions.clear()
        });
        pick(&trek, cx, &Scope::Main, |p| {
            p.agent = relay();
            p.model = None;
        });
        let went = trek.update(cx, |ws, cx| {
            ws.send_comments_in(&scope, comments.into(), cx)
        });
        assert!(went);
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().agent.clone()),
            relay()
        );
        assert!(trek.read(cx, |ws, _| ws.pending_switch(&id).is_none()));
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

// ---------- a restart ----------

#[test]
fn compact_as_the_first_message_after_a_restart_goes_as_typed() {
    use trek_app::workspace::resume::{Offer, ResumeState};
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        trek.update(cx, |ws, _| {
            ws.interrupted.insert(
                id.clone(),
                Offer {
                    turn: trek_core::interrupted::Interrupted {
                        thread: id.clone(),
                        waiting: false,
                        asked: None,
                        at: 1,
                        tries: 0,
                    },
                    state: ResumeState::Offered,
                },
            );
        });
        let session = fake_session(&trek, cx, &id);
        trek.send(cx, "/compact");
        assert_eq!(prompts(&session), ["/compact"]);
        assert_eq!(
            trek.read(cx, |ws, _| ws
                .interrupted_offer(&id)
                .map(|o| o.state.clone())),
            Some(ResumeState::Offered),
            "still on offer: the next message carries the note"
        );
        trek.update(cx, |ws, cx| {
            let live = ws.live.get_mut(&id).unwrap();
            live.turn_started = None;
            cx.notify();
        });
        trek.send(cx, "carry on");
        let sent = prompts(&session);
        assert!(
            sent.len() == 1 && sent[0].starts_with("carry on") && sent[0].contains("Trek closed"),
            "{sent:?}"
        );
    });
}

#[test]
fn a_direct_thread_with_no_session_offers_no_compact() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, _| ws.draft_prefs.agent = relay());
        let id = quiet(&trek, cx).await;
        trek.update(cx, |ws, cx| {
            let live = ws.live.get_mut(&id).unwrap();
            live.context = Some((150_000, 200_000));
            live.billing = Some(trek_agents::Billing::Metered);
            cx.notify();
        });
        // With its session, a large thread may compact (once its cache has gone).
        let has_session = trek.read(cx, |ws, _| ws.live[&id].commands.is_some());
        assert!(has_session);
        let offer = trek.read(cx, |ws, _| ws.compact_offer(&id, &Scope::Main));
        assert!(offer.is_some(), "a session to compact");
        // Its session gone (a restart, the idle shutdown, a plan toggle): nothing to compact.
        trek.update(cx, |ws, _| ws.live.get_mut(&id).unwrap().commands = None);
        assert_eq!(
            trek.read(cx, |ws, _| ws.compact_offer(&id, &Scope::Main)),
            None
        );
    });
}

// ---------- the queue and a split ----------

#[test]
fn a_message_sent_from_the_pane_it_moved_to_isnt_kept_as_unsent() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let a = quiet(&trek, cx).await;
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project),
                },
                cx,
            )
        });
        trek.render(cx);
        let b = quiet(&trek, cx).await;
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Thread(a.clone()), cx);
            ws.open_beside(&b, cx);
        });
        trek.render(cx);
        trek.type_text(cx, "right's message");
        trek.update(cx, |ws, cx| ws.close_pane(Pane::Left, cx));
        trek.render(cx);
        assert_eq!(trek.composer_text(cx), "right's message");
        trek.press(cx, "enter");
        trek.wait_done(cx, &b, RunState::Idle).await;
        trek.update(cx, |ws, _| ws.save_unsent());
        let unsent = trek.read(cx, |ws, _| ws.store.unsent_messages().unwrap());
        assert!(
            !unsent.iter().any(|m| m.text.contains("right's message")),
            "sent, so not to come back: {unsent:?}"
        );
    });
}

#[test]
fn undoing_a_removal_after_the_turn_with_the_thread_away_keeps_the_message() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.settings.general.follow_up = FollowUp::Queue;
            ws.save_settings(cx);
        });
        let id = trek.send(cx, "mock:long 600s");
        let tid = id.clone();
        trek.wait(cx, "the long turn", move |ws| {
            ws.live
                .get(&tid)
                .is_some_and(|l| l.turn_started.is_some() && l.commands.is_some())
        })
        .await;
        trek.send(cx, "then deploy");
        let shown = toasts(&trek, cx);
        trek.update(cx, |ws, cx| ws.queue_remove(&id, 0, cx));
        let undo = shown
            .borrow()
            .iter()
            .find_map(|(_, u)| u.clone())
            .expect("an undo");
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project),
                },
                cx,
            )
        });
        trek.render(cx);
        trek.update(cx, |ws, cx| ws.undo(undo, cx));
        // Kept for the next launch meanwhile, and in the thread's composer once it's back.
        trek.update(cx, |ws, _| ws.save_unsent());
        let unsent = trek.read(cx, |ws, _| ws.store.unsent_messages().unwrap());
        assert!(unsent.iter().any(|m| m.text == "then deploy"), "{unsent:?}");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        assert_eq!(trek.composer_text(cx), "then deploy");
    });
}

#[test]
fn quitting_while_changing_a_queued_message_keeps_what_the_prompt_held() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.settings.general.follow_up = FollowUp::Queue;
            ws.save_settings(cx);
        });
        let id = trek.send(cx, "mock:long 600s");
        let tid = id.clone();
        trek.wait(cx, "the long turn", move |ws| {
            ws.live
                .get(&tid)
                .is_some_and(|l| l.turn_started.is_some() && l.commands.is_some())
        })
        .await;
        trek.send(cx, "then deploy");
        trek.type_text(cx, "half a thought");
        // The queued row taken back into the prompt to change.
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        trek.window(cx, |window, cx| {
            composer.update(cx, |c, cx| c.edit_queued_for_test(&id, 0, window, cx))
        });
        assert_eq!(trek.composer_text(cx), "then deploy");
        trek.type_text(cx, " now");
        cx.quit();
        let unsent = trek.read(cx, |ws, _| ws.store.unsent_messages().unwrap());
        let kept = unsent
            .iter()
            .find(|m| m.key == format!("thread:{id}"))
            .map(|m| m.text.clone())
            .unwrap_or_default();
        assert!(kept.contains("then deploy now"), "{kept}");
        assert!(kept.contains("half a thought"), "{kept}");
    });
}
