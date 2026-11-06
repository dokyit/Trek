//! Three things above the prompt: a move to another agent that waits for the next message, the
//! queue's rows, and the Compact chip for a large thread whose prompt cache has gone.

mod common;

use common::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{ElementId, TestAppContext};
use std::time::Duration;
use trek_agents::Billing;
use trek_app::workspace::{Clock, Prefs, Scope};
use trek_core::settings::FollowUp;
use trek_core::store::{Item, now_ms};
use trek_core::{AgentId, RunState};

fn relay() -> AgentId {
    AgentId::Direct(trek_core::catalog::MOCK_RELAY_PROVIDER.into())
}

/// A pick in the composer's model menu.
fn pick(trek: &Trek, cx: &mut TestAppContext, agent: AgentId, model: Option<&str>) {
    let model = model.map(String::from);
    trek.update(cx, |ws, cx| {
        let prefs = Prefs {
            agent,
            model,
            ..ws.chosen_prefs_in(&Scope::Main)
        };
        ws.choose_prefs_in(&Scope::Main, prefs, cx);
    });
}

fn agent_of(trek: &Trek, cx: &TestAppContext, id: &str) -> AgentId {
    trek.read(cx, |ws, _| ws.thread(id).unwrap().agent.clone())
}

fn handoffs(trek: &Trek, cx: &TestAppContext, id: &str) -> usize {
    trek.items(cx, id)
        .iter()
        .filter(|i| matches!(i, Item::Handoff { .. }))
        .count()
}

async fn working(trek: &Trek, cx: &mut TestAppContext, prompt: &str) -> String {
    let id = trek.send(cx, prompt);
    let tid = id.clone();
    trek.wait(cx, "the turn to start", move |ws| {
        ws.live
            .get(&tid)
            .is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::Tool { .. })))
    })
    .await;
    id
}

/// Click `target`, a control that shows with the pointer on `over`.
fn click_hovered(
    trek: &Trek,
    cx: &mut TestAppContext,
    over: impl Into<ElementId>,
    target: impl Into<ElementId>,
) {
    let over = over.into();
    trek.window(cx, |window, cx| window.hover(over, cx));
    trek.render(cx);
    trek.click(cx, target);
}

fn queued(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<String> {
    trek.read(cx, |ws, _| {
        ws.live[id].queued.iter().map(|(t, _)| t.clone()).collect()
    })
}

// ---------- switching ----------

#[test]
fn picking_another_agent_waits_for_the_next_message() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "remember APPLE");
        trek.wait_done(cx, &id, RunState::Idle).await;
        pick(&trek, cx, relay(), None);
        // Nothing has moved: the same agent, its session, and no row in the transcript.
        assert_eq!(agent_of(&trek, cx, &id), mock());
        assert_eq!(handoffs(&trek, cx, &id), 0);
        assert!(trek.read(cx, |ws, _| ws.live[&id].commands.is_some()));
        // The pill and its menus show the pick.
        assert_eq!(
            trek.read(cx, |ws, _| ws.chosen_prefs_in(&Scope::Main).agent),
            relay()
        );
        assert_eq!(
            trek.read(cx, |ws, _| ws.prefs_in(&Scope::Main).agent),
            mock()
        );
        trek.render(cx);
        for el in ["switch-bar", "switch-stay", "switch-preview"] {
            assert!(trek.visible(cx, el), "{el}");
        }

        // The next message moves it, with the conversation.
        trek.send(cx, "recall");
        assert_eq!(agent_of(&trek, cx, &id), relay());
        assert_eq!(handoffs(&trek, cx, &id), 1);
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        assert!(!trek.visible(cx, "switch-bar"));
        let answer = trek.answers(cx, &id);
        assert!(
            answer
                .lines()
                .last()
                .is_some_and(|l| l.contains("remember APPLE")),
            "{answer}"
        );
    });
}

#[test]
fn a_waiting_move_shows_the_accounts_of_the_agent_it_goes_to() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.add_account(&mock(), cx).expect("an account")
        });
        let id = trek.send(cx, "remember APPLE");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        assert!(trek.visible(cx, "model-pill-account"), "the mock has two");
        pick(&trek, cx, relay(), None);
        trek.render(cx);
        // The pill shows the relay, which has one login: no account beside it, and none to pick.
        assert!(trek.visible(cx, "switch-bar"));
        assert!(!trek.visible(cx, "model-pill-account"));
        trek.click(cx, "model-pill");
        trek.render(cx);
        assert!(!trek.visible(cx, "mm-account"));
        // The thread itself is still on the mock's default until the next message.
        assert!(
            trek.read(cx, |ws, _| ws.account_label_in(&Scope::Main))
                .is_some()
        );
    });
}

#[test]
fn stay_on_takes_the_pick_back() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "remember APPLE");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let session = trek.read(cx, |ws, _| ws.thread(&id).unwrap().native_id.clone());
        pick(&trek, cx, relay(), None);
        trek.render(cx);
        trek.click(cx, "switch-stay");
        trek.render(cx);
        assert!(!trek.visible(cx, "switch-bar"));
        assert_eq!(
            trek.read(cx, |ws, _| ws.chosen_prefs_in(&Scope::Main).agent),
            mock()
        );
        trek.send(cx, "go on");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(agent_of(&trek, cx, &id), mock());
        assert_eq!(handoffs(&trek, cx, &id), 0);
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().native_id.clone()),
            session,
            "the same session"
        );
    });
}

#[test]
fn picking_the_threads_own_agent_again_drops_the_pick() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "remember APPLE");
        trek.wait_done(cx, &id, RunState::Idle).await;
        pick(&trek, cx, relay(), None);
        pick(&trek, cx, mock(), Some("mock-deep"));
        assert_eq!(trek.read(cx, |ws, _| ws.pending_switch(&id).cloned()), None);
        // And that model applies now, in place.
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().model.clone()),
            Some("mock-deep".to_string())
        );
    });
}

/// Another model of the same agent has never been a move: a Direct thread keeps its conversation
/// in place.
#[test]
fn another_model_of_the_same_agent_changes_at_once() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "remember APPLE");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(matches!(agent_of(&trek, cx, &id), AgentId::Direct(_)));
        let session = trek.read(cx, |ws, _| ws.thread(&id).unwrap().native_id.clone());
        pick(&trek, cx, mock(), Some("mock-deep"));
        assert_eq!(trek.read(cx, |ws, _| ws.pending_switch(&id).cloned()), None);
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().model.clone()),
            Some("mock-deep".to_string())
        );
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().native_id.clone()),
            session
        );
        assert_eq!(handoffs(&trek, cx, &id), 0);
        trek.render(cx);
        assert!(!trek.visible(cx, "switch-bar"));
    });
}

#[test]
fn a_thread_with_nothing_said_changes_agent_at_once() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        pick(&trek, cx, relay(), None);
        assert_eq!(agent_of(&trek, cx, &id), relay());
        assert_eq!(trek.read(cx, |ws, _| ws.pending_switch(&id).cloned()), None);
        trek.render(cx);
        assert!(!trek.visible(cx, "switch-bar"));
    });
}

#[test]
fn a_running_turn_goes_on_until_the_message_goes() {
    run(async |cx| {
        let trek = open(cx);
        let id = working(&trek, cx, "mock:long 30s").await;
        pick(&trek, cx, relay(), None);
        assert!(
            trek.read(cx, |ws, _| ws.turn_running(&id)),
            "picking stops nothing"
        );
        assert!(
            !trek
                .items(cx, &id)
                .iter()
                .any(|i| matches!(i, Item::Notice { text } if text == "Interrupted"))
        );
        trek.render(cx);
        assert!(trek.visible(cx, "switch-bar"));

        trek.send(cx, "carry on");
        assert_eq!(agent_of(&trek, cx, &id), relay());
        // The message said it would stop the turn, and it did; the new agent has the message.
        let items = trek.items(cx, &id);
        let at = |f: &dyn Fn(&Item) -> bool| items.iter().position(f).expect("in the transcript");
        let stopped = at(&|i| matches!(i, Item::Notice { text } if text == "Interrupted"));
        let moved = at(&|i| matches!(i, Item::Handoff { .. }));
        let said = at(&|i| matches!(i, Item::User { text, .. } if text == "carry on"));
        assert!(stopped < moved && moved < said, "{items:?}");
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

#[test]
fn trek_own_commands_dont_move_the_chat() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "remember APPLE");
        trek.wait_done(cx, &id, RunState::Idle).await;
        pick(&trek, cx, relay(), None);
        trek.send(cx, "/cost");
        assert_eq!(agent_of(&trek, cx, &id), mock());
        assert!(trek.read(cx, |ws, _| ws.pending_switch(&id).is_some()));
    });
}

#[test]
fn what_the_preview_shows_is_what_goes_and_secrets_stay_behind() {
    run(async |cx| {
        let trek = open(cx);
        let key = "sk-ant-abcdefghijklmnopqrstuvwxyz";
        let id = trek.send(cx, &format!("deploy with the key {key}"));
        trek.wait_done(cx, &id, RunState::Idle).await;
        pick(&trek, cx, relay(), None);
        let shown = trek
            .read(cx, |ws, _| ws.pending_handover(&id))
            .expect("a handover");
        assert!(!shown.text.contains(key), "{}", shown.text);
        assert!(shown.text.contains("[redacted]"), "{}", shown.text);
        assert_eq!(shown.left_out.secrets, 1);
        assert!(shown.tokens > 0 && shown.tokens <= shown.budget);
        assert!(shown.text.contains("You are taking this conversation over"));

        // The card shows it.
        trek.render(cx);
        trek.click(cx, "switch-preview");
        trek.render(cx);
        for el in [
            "switch-preview-body",
            "switch-preview-size",
            "switch-preview-text",
            "switch-preview-left-out",
        ] {
            assert!(trek.visible(cx, el), "{el}");
        }
        trek.press(cx, "escape");

        // The new agent got that, and nothing else of the key.
        trek.send(cx, "recall");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let answer = trek.answers(cx, &id);
        let recalled = answer.lines().last().unwrap_or_default();
        assert!(recalled.contains("[redacted]"), "{answer}");
        assert!(!recalled.contains(key), "{answer}");
    });
}

/// What the transcript tools answer `thread`'s agent.
async fn read_transcript(trek: &Trek, cx: &mut TestAppContext, thread: &str) -> String {
    let (reply, answer) = async_channel::unbounded();
    trek.update(cx, |ws, cx| {
        ws.handle_call(
            trek_app::ipc::Call {
                thread: thread.to_string(),
                method: "transcript_read".into(),
                params: serde_json::json!({ "start": 1, "end": 40 }),
                reply,
            },
            cx,
        )
    });
    match answer.recv().await {
        Ok(trek_app::ipc::Reply::Done(Ok(read))) => read.as_str().unwrap().to_string(),
        other => panic!("{other:?}"),
    }
}

/// The preview says the new agent can search the transcript with secrets removed: it can, and they are.
#[test]
fn a_moved_threads_transcript_tools_hand_over_no_secrets() {
    run(async |cx| {
        let trek = open(cx);
        let key = "sk-ant-abcdefghijklmnopqrstuvwxyz";
        let id = trek.send(cx, &format!("deploy with the key {key}"));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let read = read_transcript(&trek, cx, &id).await;
        assert!(read.contains(key), "word for word on its own agent: {read}");

        pick(&trek, cx, relay(), None);
        let read = read_transcript(&trek, cx, &id).await;
        assert!(read.contains(key), "nothing has moved yet: {read}");

        trek.send(cx, "carry on");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let read = read_transcript(&trek, cx, &id).await;
        assert!(!read.contains(key), "{read}");
        assert!(read.contains("[redacted]"), "{read}");
    });
}

// ---------- the queue's rows ----------

#[test]
fn each_queued_message_has_a_row_and_can_be_moved() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let id = working(&trek, cx, "mock:long 30s").await;
        trek.render(cx);
        assert!(!trek.visible(cx, "queue-bar"), "nothing queued yet");
        for text in ["one", "two", "three"] {
            trek.send(cx, text);
        }
        trek.render(cx);
        for ix in 0..3usize {
            assert!(trek.visible(cx, ("queue-row", ix)), "{ix}");
        }
        trek.update(cx, |ws, cx| ws.queue_move(&id, 2, 0, cx));
        assert_eq!(queued(&trek, cx, &id), ["three", "one", "two"]);
        trek.update(cx, |ws, cx| ws.queue_move(&id, 1, 2, cx));
        assert_eq!(queued(&trek, cx, &id), ["three", "two", "one"]);
        // Past the end is no move.
        trek.update(cx, |ws, cx| ws.queue_move(&id, 0, 3, cx));
        assert_eq!(queued(&trek, cx, &id), ["three", "two", "one"]);
    });
}

#[test]
fn a_reordered_queue_goes_out_in_its_new_order() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let id = working(&trek, cx, "mock:long 2s").await;
        trek.send(cx, "one");
        trek.send(cx, "two");
        trek.update(cx, |ws, cx| ws.queue_move(&id, 1, 0, cx));
        let tid = id.clone();
        trek.wait(cx, "the queue to go", move |ws| {
            ws.live[&tid].queued.is_empty() && ws.live[&tid].turn_started.is_none()
        })
        .await;
        let said: Vec<String> = trek
            .items(cx, &id)
            .into_iter()
            .filter_map(|i| match i {
                Item::User { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(said, ["mock:long 2s", "two", "one"]);
    });
}

#[test]
fn removing_a_queued_message_can_be_undone() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let seen = toasts(&trek, cx);
        let id = working(&trek, cx, "mock:long 30s").await;
        trek.send(cx, "one");
        trek.send(cx, "two");
        trek.update(cx, |ws, cx| ws.queue_remove(&id, 0, cx));
        assert_eq!(queued(&trek, cx, &id), ["two"]);
        cx.run_until_parked();
        let (message, undo) = seen.borrow().last().cloned().expect("a toast");
        assert_eq!(message, "Removed from the queue");
        trek.update(cx, |ws, cx| ws.undo(undo.unwrap(), cx));
        assert_eq!(queued(&trek, cx, &id), ["one", "two"], "back in its place");
    });
}

#[test]
fn an_edited_queued_message_goes_back_in_its_place() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let id = working(&trek, cx, "mock:long 30s").await;
        trek.send(cx, "one");
        trek.send(cx, "two");
        trek.type_text(cx, "my draft");
        trek.render(cx);
        click_hovered(&trek, cx, ("queue-row", 0usize), ("queue-edit", 0usize));
        trek.render(cx);
        assert_eq!(trek.composer_text(cx), "one", "in the prompt to change");
        assert_eq!(queued(&trek, cx, &id), ["two"], "and off the queue");
        assert!(trek.visible(cx, "queue-editing"));

        trek.type_text(cx, " more");
        trek.press(cx, "enter");
        assert_eq!(
            queued(&trek, cx, &id),
            ["one more", "two"],
            "changed in place"
        );
        assert_eq!(trek.composer_text(cx), "my draft", "what was there is back");
        trek.render(cx);
        assert!(!trek.visible(cx, "queue-editing"));
        assert!(
            trek.read(cx, |ws, _| ws.turn_running(&id)),
            "nothing was sent"
        );
    });
}

#[test]
fn escape_leaves_a_queued_message_as_it_was() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let id = working(&trek, cx, "mock:long 30s").await;
        trek.send(cx, "one");
        trek.send(cx, "two");
        trek.render(cx);
        click_hovered(&trek, cx, ("queue-row", 1usize), ("queue-edit", 1usize));
        assert_eq!(trek.composer_text(cx), "two");
        trek.type_text(cx, " changed");
        trek.press(cx, "escape");
        assert_eq!(queued(&trek, cx, &id), ["one", "two"]);
        assert_eq!(trek.composer_text(cx), "");
    });
}

#[test]
fn a_message_being_edited_doesnt_go_out_as_the_turn_ends() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let id = working(&trek, cx, "mock:long 2s").await;
        trek.send(cx, "one");
        trek.render(cx);
        click_hovered(&trek, cx, ("queue-row", 0usize), ("queue-edit", 0usize));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let said = trek
            .items(cx, &id)
            .iter()
            .filter(|i| matches!(i, Item::User { .. }))
            .count();
        assert_eq!(said, 1, "it waited for the user");
        // Its turn over, it's an ordinary message now.
        assert_eq!(trek.composer_text(cx), "one");
        trek.press(cx, "enter");
        assert!(
            trek.items(cx, &id)
                .iter()
                .any(|i| matches!(i, Item::User { text, .. } if text == "one"))
        );
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

// ---------- the Compact chip ----------

/// `id` as billed `billing`, `used` tokens of 200k in its context, on a clock tests can move.
fn set_up_chip(trek: &Trek, cx: &mut TestAppContext, id: &str, used: u64, billing: Billing) {
    let exec = cx.executor();
    let (start, base) = (exec.now(), now_ms());
    trek.update(cx, move |ws, cx| {
        ws.clock = Clock::new(move || base + (exec.now() - start).as_millis() as i64);
        let live = ws.live.get_mut(id).unwrap();
        live.context = Some((used, 200_000));
        live.billing = Some(billing);
        cx.notify();
    });
}

fn skip_minutes(cx: &mut TestAppContext, minutes: u64) {
    cx.executor()
        .advance_clock(Duration::from_secs(minutes * 60 + 1));
    cx.run_until_parked();
}

#[test]
fn the_chip_comes_once_a_large_threads_cache_has_gone() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        set_up_chip(&trek, cx, &id, 90_000, Billing::Metered);
        trek.render(cx);
        assert!(!trek.visible(cx, "compact-chip"), "the cache is still warm");
        skip_minutes(cx, 4);
        trek.render(cx);
        assert!(!trek.visible(cx, "compact-chip"));
        skip_minutes(cx, 2);
        trek.render(cx);
        assert!(
            trek.visible(cx, "compact-chip"),
            "five minutes on an API key"
        );
    });
}

#[test]
fn the_chip_appears_by_itself_on_an_idle_thread() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        set_up_chip(&trek, cx, &id, 90_000, Billing::Metered);
        trek.render(cx);
        skip_minutes(cx, 6);
        // No render asked for: the composer's own timer draws it, on screen as in the app.
        assert!(trek.visible(cx, "compact-chip"));
    });
}

#[test]
fn a_subscription_cache_lasts_an_hour() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        set_up_chip(&trek, cx, &id, 90_000, Billing::Plan(None));
        skip_minutes(cx, 59);
        trek.render(cx);
        assert!(!trek.visible(cx, "compact-chip"));
        skip_minutes(cx, 2);
        trek.render(cx);
        assert!(trek.visible(cx, "compact-chip"));
    });
}

#[test]
fn a_small_thread_gets_no_chip() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        set_up_chip(&trek, cx, &id, 79_000, Billing::Metered);
        skip_minutes(cx, 120);
        trek.render(cx);
        assert!(!trek.visible(cx, "compact-chip"), "under 40% of the window");
    });
}

#[test]
fn a_local_model_has_no_cache_to_lose() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        set_up_chip(&trek, cx, &id, 90_000, Billing::Local);
        skip_minutes(cx, 120);
        trek.render(cx);
        assert!(!trek.visible(cx, "compact-chip"));
    });
}

#[test]
fn an_agent_that_cant_compact_gets_no_chip() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| {
            let prefs = Prefs {
                agent: AgentId::Acp("cursor".into()),
                model: None,
                ..ws.prefs_in(&Scope::Main)
            };
            ws.set_prefs_in(&Scope::Main, prefs, cx);
        });
        set_up_chip(&trek, cx, &id, 90_000, Billing::Metered);
        skip_minutes(cx, 120);
        assert_eq!(
            trek.read(cx, |ws, _| ws.compact_offer(&id, &Scope::Main)),
            None
        );
    });
}

#[test]
fn codex_and_direct_threads_can_compact_now() {
    run(async |cx| {
        let trek = open(cx);
        for agent in [AgentId::Codex, AgentId::Direct("openrouter".into())] {
            assert!(
                trek.read(cx, |ws, _| ws.can_compact(&Scope::Main, &agent)),
                "{agent:?}"
            );
        }
        assert!(!trek.read(cx, |ws, _| {
            ws.can_compact(&Scope::Main, &AgentId::Acp("cursor".into()))
        }));
    });
}

#[test]
fn one_click_compacts_and_the_chip_goes() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        set_up_chip(&trek, cx, &id, 90_000, Billing::Metered);
        skip_minutes(cx, 6);
        trek.render(cx);
        trek.click(cx, "compact-chip");
        assert!(
            trek.items(cx, &id)
                .iter()
                .any(|i| matches!(i, Item::User { text, .. } if text == "/compact")),
            "the agent was asked"
        );
        trek.render(cx);
        assert!(!trek.visible(cx, "compact-chip"), "gone while it runs");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        assert!(!trek.visible(cx, "compact-chip"), "and after");
    });
}

#[test]
fn a_dismissed_chip_stays_gone_until_the_thread_has_been_active() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        set_up_chip(&trek, cx, &id, 90_000, Billing::Metered);
        skip_minutes(cx, 6);
        trek.render(cx);
        trek.click(cx, "compact-chip-dismiss");
        trek.render(cx);
        assert!(!trek.visible(cx, "compact-chip"));
        assert!(
            !trek
                .items(cx, &id)
                .iter()
                .any(|i| matches!(i, Item::User { text, .. } if text == "/compact")),
            "dismissing doesn't compact"
        );
        skip_minutes(cx, 60);
        trek.render(cx);
        assert!(!trek.visible(cx, "compact-chip"), "no nagging");
        // Another message, another break: it's a new situation.
        trek.send(cx, "more");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, _| {
            ws.live.get_mut(&id).unwrap().context = Some((90_000, 200_000));
        });
        skip_minutes(cx, 6);
        trek.render(cx);
        assert!(trek.visible(cx, "compact-chip"));
    });
}

/// The state the chip comes in always has the context ring out, which takes room from the pills:
/// in a narrow window they give way sooner, and send stays in the card.
#[test]
fn a_narrow_composer_keeps_send_in_view_with_the_ring_out() {
    run(async |cx| {
        let trek = open(cx);
        cx.simulate_window_resize(
            trek.window,
            gpui_kit::size(gpui_kit::px(800.), gpui_kit::px(640.)),
        );
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        set_up_chip(&trek, cx, &id, 90_000, Billing::Metered);
        // A second frame: the first measures the card.
        trek.render(cx);
        trek.render(cx);
        let send = trek.bounds(cx, "send").expect("the send button");
        // The strip under the card ends 6 px short of the card's edge.
        let edge = trek.bounds(cx, "open-terminal").expect("the strip").right() + gpui_kit::px(6.);
        assert!(trek.visible(cx, "context-ring"));
        assert!(
            send.right() <= edge,
            "send {send:?} runs past the card, at {edge:?}"
        );
    });
}

#[test]
fn the_chip_stays_away_while_a_switch_waits() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        set_up_chip(&trek, cx, &id, 90_000, Billing::Metered);
        skip_minutes(cx, 6);
        pick(&trek, cx, relay(), None);
        trek.render(cx);
        assert!(
            !trek.visible(cx, "compact-chip"),
            "the next session starts small"
        );
    });
}
