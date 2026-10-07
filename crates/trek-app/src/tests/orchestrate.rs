//! Sub-agents: an agent hands work to another through Trek's orchestration tools. The mock agent
//! plays both sides (`mock:consult` waits for its sub-agent, `mock:delegate` doesn't), calling
//! Trek over the same socket `trek-mcp orchestrate` uses; other tests call the tools directly.

use super::harness::{Trek, mock, open, open_with, run, store_items};
use crate::workspace::{Route, TaskState, WorkspaceEvent};
use gpui_kit::TestAppContext;
use serde_json::json;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use trek_core::store::{Item, ToolStatus, now_ms};
use trek_core::{AgentId, Effort, HandHolding, RunState};

/// The sub-agents of `id`, oldest first.
fn children(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<String> {
    trek.read(cx, |ws, _| ws.children(id).into_iter().map(|t| t.id.clone()).collect())
}

fn state(trek: &Trek, cx: &TestAppContext, child: &str) -> TaskState {
    trek.read(cx, |ws, _| ws.task_state(child))
}

/// The sub-agent row of `child` in its parent's transcript: (status, output).
fn task_row(trek: &Trek, cx: &TestAppContext, parent: &str, child: &str) -> (ToolStatus, String) {
    let row = trek_core::orchestrate::task_row(child);
    trek.items(cx, parent)
        .into_iter()
        .find_map(|i| match i {
            Item::Tool { id, status, output, .. } if id == row => Some((status, output)),
            _ => None,
        })
        .expect("a row for the sub-agent")
}

async fn wait_task(trek: &Trek, cx: &mut TestAppContext, child: &str, want: TaskState) {
    let c = child.to_string();
    trek.wait(cx, &format!("the sub-agent to be {want:?}"), move |ws| ws.task_state(&c) == want).await;
}

#[test]
fn consulting_waits_for_the_sub_agent_and_uses_its_answer() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:consult explain the startup");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let kids = children(&trek, cx, &id);
        assert_eq!(kids.len(), 1, "one sub-agent");
        let child = &kids[0];
        let t = trek.read(cx, |ws, _| ws.thread(child).cloned()).unwrap();
        assert_eq!((t.parent_id.as_deref(), t.title.as_str(), t.agent.clone()), (Some(id.as_str()), "Second opinion", mock()));
        assert_eq!(t.hand_holding, HandHolding::Supervised, "advising, it works read-only");
        assert_eq!(t.cwd, trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.cwd.clone())), "in its parent's folder");
        assert_eq!(state(&trek, cx, child), TaskState::Done);
        // Its answer came back to the parent, which used it.
        let (status, output) = task_row(&trek, cx, &id, child);
        assert_eq!(status, ToolStatus::Done);
        assert!(output.starts_with("## How the app starts"), "{output}");
        assert!(trek.answers(cx, &id).contains("The second opinion is in: ## How the app starts"), "{}", trek.answers(cx, &id));
        // The sub-agent was told what it is, and its session ended with its task.
        let asked = trek.items(cx, child).into_iter().find_map(|i| if let Item::User { text, .. } = i { Some(text) } else { None }).unwrap();
        assert!(asked.contains("consultant") && asked.contains("<task>\nexplain the startup\n</task>"), "{asked}");
        trek.wait(cx, "the sub-agent's session to end", |ws| ws.live.get(&kids[0]).is_some_and(|l| l.commands.is_none())).await;
        // It shows inline in its parent, not in the inbox; no wake-up was needed.
        // In order: the sub-agent's row comes after what the agent said before calling it (its own
        // `delegate_task` row has none: the sub-agent's stands for it).
        assert_eq!(trek.rows(cx), ["user", "assistant", "subagent: Mock Swift: Second opinion (Done)", "assistant", "end"]);
        assert!(!trek.rows(cx).contains(&"wake".to_string()));
        let listed: Vec<String> = trek.read(cx, |ws, _| ws.sections().into_iter().flat_map(|(_, ts)| ts.into_iter().map(|t| t.id.clone())).collect());
        assert!(listed.contains(&id) && !listed.contains(child), "{listed:?}");
    });
}

#[test]
fn a_sub_agent_that_runs_on_its_own_wakes_its_parent() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:delegate mock:long 300ms");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let child = children(&trek, cx, &id).pop().expect("a sub-agent");
        assert!(trek.answers(cx, &id).contains("pick its answer up when it reports back"));
        // The parent's turn is over; the sub-agent works on, and its row with it.
        assert_eq!(state(&trek, cx, &child), TaskState::Running);
        assert_eq!(task_row(&trek, cx, &id, &child).0, ToolStatus::Running, "the parent's turn ending doesn't end its row");
        trek.wait(cx, "the wake-up", |ws| ws.live[&id].items.iter().any(|i| matches!(i, Item::User { text, .. } if trek_core::orchestrate::is_wake(text)))).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("The sub-agent reported back: The full suite passed"), "{}", trek.answers(cx, &id));
        assert_eq!(state(&trek, cx, &child), TaskState::Done);
        assert!(trek.rows(cx).contains(&"wake".to_string()));
    });
}

#[test]
fn stop_ends_the_sub_agents_too() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:consult mock:long 30s");
        trek.wait(cx, "a sub-agent at work", |ws| ws.children(&id).first().is_some_and(|c| ws.task_state(&c.id) == TaskState::Running)).await;
        let child = children(&trek, cx, &id).pop().unwrap();
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        wait_task(&trek, cx, &child, TaskState::Cancelled).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(task_row(&trek, cx, &id, &child).0, ToolStatus::Denied);
        assert!(trek.items(cx, &child).iter().any(|i| matches!(i, Item::Notice { text } if text == "Interrupted")));
        // Stopped on its parent's behalf: nobody is woken.
        trek.wait(cx, "the sub-agent's session to end", |ws| ws.live.get(&child).is_some_and(|l| l.commands.is_none())).await;
        assert!(!trek.items(cx, &id).iter().any(|i| matches!(i, Item::User { text, .. } if trek_core::orchestrate::is_wake(text))));
        assert!(!trek.read(cx, |ws, _| ws.work_in_flight()), "nothing left running");
    });
}

#[test]
fn sub_agents_go_two_levels_deep_and_no_further() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:consult mock:consult mock:consult explain");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let child = children(&trek, cx, &id).pop().expect("a sub-agent");
        let grandchild = children(&trek, cx, &child).pop().expect("its own sub-agent");
        assert!(children(&trek, cx, &grandchild).is_empty(), "no third level");
        assert_eq!(trek.read(cx, |ws, _| (ws.depth(&id), ws.depth(&child), ws.depth(&grandchild))), (0, 1, 2));
        // The grandchild was told why, and said so; that came back up the chain.
        assert!(trek.answers(cx, &grandchild).contains("levels down"), "{}", trek.answers(cx, &grandchild));
        assert!(trek.answers(cx, &id).contains("levels down"), "{}", trek.answers(cx, &id));
    });
}

#[test]
fn a_thread_runs_at_most_four_sub_agents_at_once() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let start = |trek: &Trek, cx: &mut TestAppContext| trek.update(cx, |ws, cx| ws.delegate(&id, &json!({ "title": "Wait", "prompt": "mock:long 30s", "agent": "direct:mock" }), cx));
        for _ in 0..4 {
            start(&trek, cx).expect("room for it");
        }
        let err = start(&trek, cx).unwrap_err();
        assert!(err.contains("4 sub-agents running") && err.contains("cancel_task"), "{err}");
        // One stops: there's room again.
        let first = children(&trek, cx, &id)[0].clone();
        let caller = id.clone();
        trek.update(cx, |ws, cx| ws.cancel_task(&caller, &first, cx)).unwrap();
        wait_task(&trek, cx, &first, TaskState::Cancelled).await;
        start(&trek, cx).expect("room again");
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        let tid = id.clone();
        trek.wait(cx, "every sub-agent to stop", |ws| ws.running_children(&tid).is_empty()).await;
    });
}

#[test]
fn delegate_task_checks_what_it_is_asked() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let ask = |trek: &Trek, cx: &mut TestAppContext, params: serde_json::Value| trek.update(cx, |ws, cx| ws.delegate(&id, &params, cx));
        assert!(ask(&trek, cx, json!({ "prompt": "x" })).unwrap_err().contains("needs a title"));
        assert!(ask(&trek, cx, json!({ "title": "x" })).unwrap_err().contains("needs a prompt"));
        assert!(ask(&trek, cx, json!({ "title": "x", "prompt": "y", "agent": "nope" })).unwrap_err().contains("can't run “nope”"));
        assert!(ask(&trek, cx, json!({ "title": "x", "prompt": "y", "model": "mock-giant" })).unwrap_err().contains("mock-swift (Mock Swift)"));
        assert!(ask(&trek, cx, json!({ "title": "x", "prompt": "y", "mode": "yolo" })).unwrap_err().contains("advise"));
        assert!(ask(&trek, cx, json!({ "title": "x", "prompt": "y", "effort": "extreme" })).unwrap_err().contains("effort"));
        // An agent at its usage limit takes no more.
        trek.update(cx, |ws, _| {
            let limit = trek_agents::UsageLimit { label: "5-hour limit".into(), percent: 100., resets_at: None, window: "5h".into() };
            ws.agent_status.insert(mock().key(), trek_agents::AgentStatus { limits: vec![limit], ..Default::default() });
        });
        assert!(ask(&trek, cx, json!({ "title": "x", "prompt": "y" })).unwrap_err().contains("used up its 5-hour limit"));
        trek.update(cx, |ws, _| _ = ws.agent_status.remove(&mock().key()));
        // Names work as well as ids; efforts are clamped to the model's.
        let child = ask(&trek, cx, json!({ "title": "x", "prompt": "explain", "model": "Mock Deep", "effort": "max", "mode": "implement" })).unwrap();
        let t = trek.read(cx, |ws, _| ws.thread(&child).cloned()).unwrap();
        assert_eq!((t.model.as_deref(), t.effort, t.hand_holding), (Some("mock-deep"), trek_core::Effort::Max, HandHolding::Auto), "implementing, it has its parent's access");
        wait_task(&trek, cx, &child, TaskState::Done).await;

        // Only a thread's own sub-agents answer to it.
        let other = trek.quiet_thread(cx);
        let (c, o) = (child.clone(), other.clone());
        assert!(trek.read(cx, |ws, _| ws.task_status(&o, &c)).unwrap_err().contains("no sub-agent"));
        let status = trek.read(cx, |ws, _| ws.task_status(&id, &c)).unwrap();
        assert_eq!((status["status"].as_str(), status["mode"].as_str(), status["model"].as_str()), (Some("done"), Some("implement"), Some("Mock Deep")));
        assert!(status["preview"].as_str().unwrap().starts_with("## How the app starts"));
        let result = trek.read(cx, |ws, _| ws.task_result(&id, &c)).unwrap();
        assert!(result["result"].as_str().unwrap().contains("```rust"));
        let cancel = trek.update(cx, |ws, cx| ws.cancel_task(&id, &c, cx)).unwrap();
        assert_eq!(cancel["note"], "It had already ended.");
    });
}

#[test]
fn a_sub_agent_inherits_its_parents_model() {
    run(async |cx| {
        let trek = open(cx);
        // A thread on a chosen model: "Mock Deep", not the mock agent's first.
        let id = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&trek.project), mock(), Some("mock-deep".into()), Effort::Medium, HandHolding::Auto).expect("thread");
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            t.id
        });
        // delegate_task with no model runs the caller's model, not the agent's default.
        let child = trek.update(cx, |ws, cx| ws.delegate(&id, &json!({ "title": "x", "prompt": "y" }), cx)).unwrap();
        let t = trek.read(cx, |ws, _| ws.thread(&child).cloned()).unwrap();
        assert_eq!(t.model.as_deref(), Some("mock-deep"), "no model asked for: the parent's");
        wait_task(&trek, cx, &child, TaskState::Done).await;
    });
}

#[test]
fn an_advising_sub_agent_is_never_allowed_to_act() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let child = trek.update(cx, |ws, cx| ws.delegate(&id, &json!({ "title": "Migrate", "prompt": "mock:permission" }), cx)).unwrap();
        wait_task(&trek, cx, &child, TaskState::Done).await;
        // The request was declined for it: nobody was asked.
        assert!(trek.answers(cx, &child).contains("won't run it"), "{}", trek.answers(cx, &child));
        assert_eq!(trek.read(cx, |ws, _| ws.needs_you_count()), 0);

        // Implementing (with its parent's Supervised access), it asks the user like any thread.
        let parent = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.set_hand_holding(Some(&parent), HandHolding::Supervised, cx)).unwrap();
        let doer = trek.update(cx, |ws, cx| ws.delegate(&parent, &json!({ "title": "Migrate", "prompt": "mock:permission", "mode": "implement" }), cx)).unwrap();
        wait_task(&trek, cx, &doer, TaskState::NeedsYou).await;
        assert_eq!(trek.read(cx, |ws, _| ws.needs_you_count()), 1, "it waits on the user");
        let rid = trek.request(cx, &doer);
        trek.update(cx, |ws, cx| ws.respond(&doer, &rid, trek_agents::Decision::Allow, cx));
        wait_task(&trek, cx, &doer, TaskState::Done).await;
        assert!(task_row(&trek, cx, &parent, &doer).1.contains("Migrations applied"));
    });
}

#[test]
fn trek_s_socket_answers_its_sessions_only() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let (good, stranger) = trek.read(cx, |ws, _| {
            let ipc = ws.ipc.as_ref().expect("listening");
            let key = ipc.open_session(Some(&id));
            (ipc.client(&key), trek_ipc::Client { token: "f".repeat(64), ..ipc.client(&key) })
        });
        // A real socket: the call runs off the test's thread while the workspace answers here.
        let call = std::thread::spawn(move || (good.call("list_models", &json!({})), stranger.call("list_models", &json!({}))));
        let answered = trek_core::runtime().spawn_blocking(move || call.join().unwrap());
        let (ok, refused) = loop {
            cx.run_until_parked();
            if answered.is_finished() {
                break trek_core::runtime().block_on(answered).unwrap();
            }
            cx.background_executor.timer(std::time::Duration::from_millis(5)).await;
        };
        let models = ok.unwrap();
        assert!(models["agents"].as_array().unwrap().iter().any(|a| a["agent"] == "direct:mock"), "{models}");
        assert_eq!(models["sub_agents"]["max_running"], 4);
        assert_eq!(models["sub_agents"]["can_delegate"], true);
        assert!(refused.unwrap_err().contains("wrong token"));
    });
}

#[test]
fn archiving_or_deleting_a_parent_takes_its_sub_agents_with_it() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let busy = trek.update(cx, |ws, cx| ws.delegate(&id, &json!({ "title": "Busy", "prompt": "mock:long 30s" }), cx)).unwrap();
        trek.update(cx, |ws, cx| ws.archive(&id, cx));
        assert!(trek.read(cx, |ws, _| ws.thread(&busy).is_none()), "archived with it");
        assert!(trek.read(cx, |ws, _| ws.live.get(&busy).is_none_or(|l| l.commands.is_none())), "and stopped");
        trek.update(cx, |ws, cx| ws.undo(crate::workspace::UndoAction::Unarchive(id.clone()), cx));
        assert!(trek.read(cx, |ws, _| ws.thread(&busy).is_some()), "back with it");
        trek.update(cx, |ws, cx| ws.delete_thread(&id, cx));
        assert!(trek.read(cx, |ws, _| ws.store.thread(&busy).unwrap().is_none()), "deleted with it");
    });
}

fn consultants(trek: &Trek, cx: &TestAppContext) -> (Vec<String>, bool) {
    cx.read(|cx| trek.root.read(cx).composer.read(cx).consultants())
}

/// The last message the user sent in `id`.
fn last_message(trek: &Trek, cx: &TestAppContext, id: &str) -> String {
    trek.items(cx, id).into_iter().rev().find_map(|i| if let Item::User { text, .. } = i { Some(text) } else { None }).unwrap()
}

#[test]
fn the_consult_menu_picks_models_and_the_message_tells_the_agent_to_ask_them() {
    run(async |cx| {
        let trek = open(cx);
        assert!(trek.visible(cx, "consult-pill"), "next to the model picker");
        trek.click(cx, "consult-pill");
        assert!(trek.visible(cx, "consult-menu-body"));
        // It opens on another agent than the thread's (the relay mock); the thread's own is a tab away.
        assert!(trek.visible(cx, "consult-add-direct:mock-relay-relay-swift"));
        trek.click(cx, "consult-rail-direct:mock");
        trek.click(cx, "consult-add-direct:mock-mock-deep");
        assert_eq!(consultants(&trek, cx).0, ["direct:mock/mock-deep/high"], "added at High");
        // Its effort, from the side panel; then Discuss, and report only.
        trek.click(cx, ("consultant-effort", 0usize));
        assert!(trek.visible(cx, "consult-eff-max"));
        trek.click(cx, "consult-eff-max");
        assert_eq!(consultants(&trek, cx).0, ["direct:mock/mock-deep/max"]);
        trek.click(cx, ("consult-style", 1usize));
        trek.click(cx, ("consult-then", 1usize));
        trek.press(cx, "escape");
        trek.render(cx);

        let id = trek.send(cx, "How does the app start?");
        let sent = last_message(&trek, cx, &id);
        let (said, consult) = trek_core::orchestrate::split_consult(&sent);
        assert_eq!(said, "How does the app start?");
        let consult = consult.expect("the instructions went with it");
        assert_eq!((consult.style, consult.implement), (trek_core::orchestrate::Style::Discuss, false));
        assert!(sent.contains("Mock Deep · Max: agent \"direct:mock\", model \"mock-deep\", effort \"max\""), "{sent}");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone())), Some("How does the app start?".into()), "titled by what the user wrote");
        // The transcript shows the message as written, with who it consults.
        let user = trek.item_ix(cx, &id, |i| matches!(i, Item::User { .. }));
        trek.render(cx);
        assert!(trek.visible(cx, ("consulting", user)));
        let markdown = trek.read(cx, |ws, _| ws.transcript_markdown(&id));
        assert!(markdown.contains("How does the app start?") && !markdown.contains("trek-consult"), "copied as written");
        // Not pinned: they clear for the next message.
        assert!(consultants(&trek, cx).0.is_empty());
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

#[test]
fn consult_by_command_and_pinned_consultants_stay() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "/consult mock deep max, swift: how does it start?");
        let sent = last_message(&trek, cx, &id);
        let (said, consult) = trek_core::orchestrate::split_consult(&sent);
        assert_eq!(said, "how does it start?");
        let keys: Vec<String> = consult.unwrap().consultants.iter().map(|c| c.key()).collect();
        assert_eq!(keys, ["direct:mock/mock-deep/max", "direct:mock/mock-swift/high"]);
        trek.wait_done(cx, &id, RunState::Idle).await;
        // A name it doesn't know: nothing is sent.
        trek.type_text(cx, "/consult giant: hi");
        trek.press(cx, "enter");
        assert_eq!(trek.composer_text(cx), "/consult giant: hi", "kept to fix");
        assert_eq!(trek.items(cx, &id).iter().filter(|i| matches!(i, Item::User { .. })).count(), 1);
        trek.update(cx, |_, cx| cx.notify());
        trek.window(cx, |window, cx| trek.root.read(cx).composer.clone().update(cx, |c, cx| c.set_text("", window, cx)));
        // Just the command: the menu opens with them picked; pinned, they stay after sending.
        trek.type_text(cx, "/consult swift");
        trek.press(cx, "enter");
        trek.render(cx);
        assert!(trek.visible(cx, "consult-menu-body"));
        assert_eq!(trek.composer_text(cx), "", "the command itself isn't sent");
        assert_eq!(consultants(&trek, cx).0, ["direct:mock/mock-swift/high"]);
        trek.click(cx, "consult-pin");
        trek.press(cx, "escape");
        trek.send(cx, "and the config?");
        assert!(trek_core::orchestrate::split_consult(&last_message(&trek, cx, &id)).1.is_some());
        assert_eq!(consultants(&trek, cx), (vec!["direct:mock/mock-swift/high".to_string()], true), "pinned");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // Editing the message puts it back as written, with its consultants.
        let last = trek.items(cx, &id).iter().rposition(|i| matches!(i, Item::User { .. })).unwrap();
        trek.click(cx, "consult-pill");
        trek.render(cx);
        trek.click(cx, "consult-clear");
        trek.press(cx, "escape");
        assert!(consultants(&trek, cx).0.is_empty());
        // As the message's Edit button does.
        let (item, text) = trek.read(cx, |ws, _| (ws.live[&id].item_ids()[last].clone(), ws.live[&id].items[last].clone()));
        let Item::User { text, .. } = text else { panic!() };
        let thread = id.clone();
        trek.update(cx, |_, cx| cx.emit(crate::workspace::WorkspaceEvent::ComposeIn { scope: crate::workspace::Scope::Main, thread, text, images: vec![], edit: Some(item) }));
        assert_eq!(trek.composer_text(cx), "and the config?");
        assert_eq!(consultants(&trek, cx).0, ["direct:mock/mock-swift/high"]);
    });
}

#[test]
fn sub_agents_show_inline_and_on_their_parent_s_card() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:delegate mock:long 30s");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let child = children(&trek, cx, &id).pop().unwrap();
        trek.render(cx);
        let row = trek.item_ix(cx, &id, |i| matches!(i, Item::Tool { id, .. } if trek_core::orchestrate::task_of_row(id).is_some()));
        assert!(trek.visible(cx, ("subagent", row)), "its row");
        assert!(trek.rows(cx).iter().any(|r| r.starts_with("subagent: Mock Swift: Second opinion (Running")), "{:?}", trek.rows(cx));
        // The agent's own call to delegate_task has no row: the sub-agent's stands for it.
        assert!(!trek.rows(cx).iter().any(|r| r.starts_with("group")), "{:?}", trek.rows(cx));
        assert!(trek.visible(cx, format!("card-kids-{id}")), "the parent's card shows who's at work");
        assert!(trek.read(cx, |ws, _| ws.any_turn_running()), "the Mac stays awake while it works");
        // The sub-agent isn't in the inbox, but a search finds it.
        assert!(!trek.visible(cx, format!("card-{child}")));
        trek.update(cx, |ws, cx| ws.set_search("Second opinion".into(), cx));
        let c = child.clone();
        trek.wait(cx, "the search", move |ws| ws.sections().iter().any(|(_, ts)| ts.iter().any(|t| t.id == c))).await;
        trek.update(cx, |ws, cx| ws.set_search(String::new(), cx));
        // Its chevron opens its thread in a window of its own.
        trek.click(cx, ("subagent-open", row));
        assert!(trek.read(cx, |ws, _| ws.thread_windows.contains_key(&child)));
        // Stop in the parent (idle now) still ends it.
        assert!(trek.visible(cx, "stop"), "the parent offers Stop while its sub-agent works");
        trek.click(cx, "stop");
        wait_task(&trek, cx, &child, TaskState::Cancelled).await;
        trek.render(cx);
        assert!(!trek.visible(cx, format!("card-kids-{id}")));
        assert!(trek.rows(cx).iter().any(|r| r == "subagent: Mock Swift: Second opinion (Stopped)"), "{:?}", trek.rows(cx));
    });
}

#[test]
fn a_sub_agent_s_row_stays_in_the_transcript_while_its_parent_waits() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:consult mock:explore 30s");
        let p = id.clone();
        trek.wait(cx, "the sub-agent to read a file", move |ws| {
            ws.children(&p).first().and_then(|c| ws.live.get(&c.id)).is_some_and(|l| matches!(l.items.iter().rev().find(|i| matches!(i, Item::Tool { .. })), Some(Item::Tool { title, .. }) if title == "Read"))
        })
        .await;
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.turn_running(&id)), "the parent waits on it");
        // The live group in the working bar ends at the sub-agent: its row, ticking, stays here.
        let row = trek.item_ix(cx, &id, |i| matches!(i, Item::Tool { id, .. } if trek_core::orchestrate::task_of_row(id).is_some()));
        assert!(trek.visible(cx, ("subagent", row)));
        assert!(trek.rows(cx).iter().any(|r| r.starts_with("subagent: Mock Swift: Second opinion (Running · Read src/")), "{:?}", trek.rows(cx));
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

/// Whether `id`'s agent has ended the turn in which it started a sub-agent of its own.
fn turn_ended(ws: &crate::workspace::Workspace, id: &str) -> bool {
    ws.live.get(id).is_some_and(|l| l.turn_started.is_none() && l.items.iter().any(|i| matches!(i, Item::Assistant { text } if text.contains("pick its answer up"))))
}

#[test]
fn a_sub_agent_waiting_on_its_own_reports_once_they_have() {
    run(async |cx| {
        let trek = open(cx);
        // The root waits on its sub-agent, which starts one of its own and ends its turn.
        let id = trek.send(cx, "mock:consult mock:delegate mock:long 800ms");
        trek.wait(cx, "a grandchild", |ws| ws.children(&id).first().is_some_and(|c| !ws.children(&c.id).is_empty())).await;
        let child = children(&trek, cx, &id)[0].clone();
        let grandchild = children(&trek, cx, &child)[0].clone();
        let c = child.clone();
        trek.wait(cx, "the sub-agent's first turn to end", move |ws| turn_ended(ws, &c)).await;
        // It isn't done: what it waits on hasn't reported. Its session stays for the wake-up.
        assert_eq!(state(&trek, cx, &child), TaskState::Running);
        assert_eq!(state(&trek, cx, &grandchild), TaskState::Running);
        assert_eq!(trek.run_state(cx, &id), RunState::Working, "the root still waits");
        assert!(trek.read(cx, |ws, _| ws.running_children(&id).len()) == 1, "the root's card and Stop still count it");
        assert!(trek.read(cx, |ws, _| ws.live[&child].commands.is_some()));
        trek.wait_done(cx, &id, RunState::Idle).await;
        // The answer the root got is the one after the wake-up, not the placeholder.
        assert_eq!(state(&trek, cx, &grandchild), TaskState::Done);
        assert_eq!(state(&trek, cx, &child), TaskState::Done);
        let (status, output) = task_row(&trek, cx, &id, &child);
        assert_eq!(status, ToolStatus::Done);
        assert!(output.starts_with("The sub-agent reported back: The full suite passed"), "{output}");
        assert!(trek.answers(cx, &id).contains("The second opinion is in: The sub-agent reported back"), "{}", trek.answers(cx, &id));
    });
}

#[test]
fn stop_reaches_a_sub_agent_waiting_between_turns_and_its_own() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:delegate mock:delegate mock:long 30s");
        trek.wait(cx, "a grandchild at work", |ws| ws.children(&id).first().is_some_and(|c| ws.children(&c.id).first().is_some_and(|g| ws.task_state(&g.id) == TaskState::Running))).await;
        let child = children(&trek, cx, &id)[0].clone();
        let grandchild = children(&trek, cx, &child)[0].clone();
        let c = child.clone();
        trek.wait(cx, "the sub-agent's turn to end", move |ws| turn_ended(ws, &c)).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(trek.read(cx, |ws, _| ws.running_children(&id).iter().map(|t| t.id.clone()).collect::<Vec<_>>()), [child.clone()]);
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        wait_task(&trek, cx, &child, TaskState::Cancelled).await;
        wait_task(&trek, cx, &grandchild, TaskState::Cancelled).await;
        trek.wait(cx, "both sessions to end", |ws| [&child, &grandchild].iter().all(|k| ws.live.get(*k).is_none_or(|l| l.commands.is_none()))).await;
        assert!(!trek.items(cx, &id).iter().any(|i| matches!(i, Item::User { text, .. } if trek_core::orchestrate::is_wake(text))), "stopped from above: nobody is woken");
        assert!(!trek.read(cx, |ws, _| ws.work_in_flight()), "nothing left running");
    });
}

#[test]
fn a_sub_agent_cut_off_by_quitting_failed() {
    run(async |cx| {
        let trek = open(cx);
        let (parent, child) = trek.update(cx, |ws, cx| {
            let p = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).unwrap();
            let mut c = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Supervised).unwrap();
            c.parent_id = Some(p.id.clone());
            c.title = "Review the cache".into();
            ws.store.save_thread(&c).unwrap();
            let row = Item::Tool { id: trek_core::orchestrate::task_row(&c.id), title: "Sub-agent".into(), detail: "Review the cache".into(), output: String::new(), status: ToolStatus::Running };
            store_items(&ws.store, &p.id, vec![Item::User { text: "go".into(), images: vec![], at: Some(1), resume: None, aside: false }, row]);
            ws.reload(cx);
            (p.id, c.id)
        });
        // An idle parent wasn't mid-turn at the quit, so only reading it shows the row's stale.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(parent.clone()), cx));
        assert_eq!(task_row(&trek, cx, &parent, &child), (ToolStatus::Failed, trek_core::orchestrate::CUT_OFF.to_string()));
        assert_eq!(state(&trek, cx, &child), TaskState::Failed);
        let v = trek.read(cx, |ws, _| ws.task_result(&parent, &child)).unwrap();
        assert_eq!((v["status"].as_str(), v["error"].as_str()), (Some("failed"), Some(trek_core::orchestrate::CUT_OFF)));
        let stored = trek.read(cx, |ws, _| ws.store.items(&parent).unwrap());
        assert!(stored.iter().any(|i| matches!(i, Item::Tool { status: ToolStatus::Failed, .. })), "saved settled");
    });
}

#[test]
fn advising_stays_read_only_down_the_chain_and_starts_are_bounded() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:delegate mock:long 30s");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let child = children(&trek, cx, &id)[0].clone();
        assert!(trek.read(cx, |ws, _| ws.advising(&child)));
        let c = child.clone();
        let err = trek.update(cx, |ws, cx| ws.delegate(&c, &json!({ "title": "Fix it", "prompt": "fix", "agent": "direct:mock", "mode": "implement" }), cx)).unwrap_err();
        assert!(err.contains("advising") && err.contains("\"advise\""), "{err}");
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        wait_task(&trek, cx, &child, TaskState::Cancelled).await;
        assert!(!trek.read(cx, |ws, _| ws.advising(&child)), "once it's done, the user may carry on in it as they like");

        // A dozen sub-agents between two messages from the user, and no more.
        let q = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| {
            for n in 0..trek_core::orchestrate::MAX_PER_REQUEST {
                let mut t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Low, HandHolding::Supervised).unwrap();
                t.parent_id = Some(q.clone());
                t.title = format!("Round {n}");
                ws.store.save_thread(&t).unwrap();
            }
            ws.reload(cx);
        });
        let start = |trek: &Trek, cx: &mut TestAppContext, q: &str| {
            let q = q.to_string();
            trek.update(cx, move |ws, cx| ws.delegate(&q, &json!({ "title": "One more", "prompt": "mock:long 30s", "agent": "direct:mock" }), cx))
        };
        let err = start(&trek, cx, &q).unwrap_err();
        assert!(err.contains("since the user's last message"), "{err}");
        let at = now_ms();
        trek.wait(cx, "a new millisecond", move |_| now_ms() > at).await;
        trek.update(cx, |ws, cx| ws.send_to(&q, "and now?".into(), vec![], cx));
        trek.wait_done(cx, &q, RunState::Idle).await;
        start(&trek, cx, &q).expect("the user asked again: room again");
        trek.update(cx, |ws, cx| ws.interrupt(&q, cx));
        let tid = q.clone();
        trek.wait(cx, "the sub-agent to stop", move |ws| ws.running_children(&tid).is_empty()).await;
    });
}

#[test]
fn acp_agents_wait_less_than_their_clients_do() {
    run(async |cx| {
        let trek = open(cx);
        let (acp, mock_id) = trek.update(cx, |ws, cx| {
            let a = ws.store.create_thread(Some(&trek.project), AgentId::OpenCode, None, Effort::Medium, HandHolding::Auto).unwrap();
            let m = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).unwrap();
            ws.reload(cx);
            (a.id, m.id)
        });
        assert!(trek.read(cx, |ws, _| ws.longest_wait(&acp)) < Duration::from_secs(60));
        assert_eq!(trek.read(cx, |ws, _| ws.longest_wait(&mock_id)), Duration::MAX);
    });
}

#[test]
fn a_parent_with_sub_agents_at_work_still_takes_a_message() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:delegate mock:long 30s");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let child = children(&trek, cx, &id)[0].clone();
        trek.render(cx);
        assert!(trek.visible(cx, "stop") && !trek.visible(cx, "send"), "an empty composer offers to stop the sub-agent");
        trek.type_text(cx, "meanwhile, what's next?");
        trek.render(cx);
        assert!(trek.visible(cx, "send") && !trek.visible(cx, "stop"), "with a message, the button sends it");
        trek.click(cx, "send");
        assert_eq!(trek.items(cx, &id).iter().filter(|i| matches!(i, Item::User { .. })).count(), 2);
        assert_eq!(state(&trek, cx, &child), TaskState::Running, "sending didn't stop it");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        wait_task(&trek, cx, &child, TaskState::Cancelled).await;
    });
}

#[test]
fn consulting_is_offered_only_where_the_agent_gets_the_tools() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.tools.orchestration = false);
        let seen = Rc::new(RefCell::new(Vec::<String>::new()));
        let sink = seen.clone();
        cx.update(|cx| {
            cx.subscribe(&trek.ws, move |_, event: &WorkspaceEvent, _| {
                if let WorkspaceEvent::Toast { message, .. } = event {
                    sink.borrow_mut().push(message.clone());
                }
            })
            .detach()
        });
        trek.render(cx);
        trek.click(cx, "consult-pill");
        trek.render(cx);
        assert!(!trek.visible(cx, "consult-menu-body"), "the pill doesn't open");
        trek.type_text(cx, "/consult swift: hi");
        trek.press(cx, "enter");
        assert_eq!(trek.composer_text(cx), "/consult swift: hi", "nothing was sent");
        assert!(seen.borrow().last().is_some_and(|m| m.contains("Sub-agent tools are off")), "{:?}", seen.borrow());
        assert!(consultants(&trek, cx).0.is_empty());
        // Picked while on, then turned off: the message waits rather than going without them.
        trek.update(cx, |ws, cx| {
            ws.settings.tools.orchestration = true;
            cx.notify();
        });
        trek.window(cx, |window, cx| trek.root.read(cx).composer.clone().update(cx, |c, cx| c.set_text("/consult swift", window, cx)));
        trek.press(cx, "enter");
        trek.press(cx, "escape");
        assert_eq!(consultants(&trek, cx).0, ["direct:mock/mock-swift/high"]);
        trek.update(cx, |ws, cx| {
            ws.settings.tools.orchestration = false;
            cx.notify();
        });
        trek.type_text(cx, "how does it start?");
        trek.press(cx, "enter");
        assert_eq!(trek.composer_text(cx), "how does it start?");
        assert_eq!(consultants(&trek, cx).0, ["direct:mock/mock-swift/high"], "kept for when it can go");
        assert!(trek.read(cx, |ws, _| ws.consult_unavailable(&AgentId::Direct("openai".into()))).is_some());
    });
}

#[test]
fn a_finished_sub_agent_keeps_its_time() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:consult explain the startup");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let child = children(&trek, cx, &id)[0].clone();
        let took = trek.read(cx, |ws, _| ws.task_elapsed(&child));
        let at = now_ms();
        trek.wait(cx, "a second to pass", move |_| now_ms() > at + 1100).await;
        // The user carries on in it: its run for the parent took what it took.
        trek.update(cx, |ws, cx| ws.send_to(&child, "thanks".into(), vec![], cx));
        trek.wait_done(cx, &child, RunState::Idle).await;
        assert_eq!(trek.read(cx, |ws, _| ws.task_elapsed(&child)), took);
    });
}

#[test]
fn a_sub_agent_row_goes_after_its_call_when_text_follows_in_the_same_batch() {
    use trek_agents::AgentEvent;
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        // A turn is open, and Trek hears of the sub-agent before the agent's report of its call.
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TextDelta("I'll get a second opinion.".into()), AgentEvent::TextDone("I'll get a second opinion.".into())], cx));
        let child = trek.update(cx, |ws, cx| ws.delegate(&id, &json!({ "title": "Second opinion", "prompt": "mock:long 30s" }), cx)).unwrap();
        let row = trek_core::orchestrate::task_row(&child);
        assert!(!trek.items(cx, &id).iter().any(|i| matches!(i, Item::Tool { id, .. } if *id == row)), "it waits for the call");
        // The call, its result and the start of the next message, all at once.
        let call = format!("mcp__{}__delegate_task", trek_agents::mock::ORCHESTRATE_SERVER);
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    AgentEvent::ToolStarted { id: "call-1".into(), title: call, detail: "Second opinion".into() },
                    AgentEvent::ToolFinished { id: "call-1".into(), output: "{}".into(), ok: true },
                    AgentEvent::TextDelta("It's ".into()),
                    AgentEvent::TextDelta("on it.".into()),
                    AgentEvent::TextDone("It's on it.".into()),
                ],
                cx,
            )
        });
        let items = trek.items(cx, &id);
        let assistants: Vec<&str> = items.iter().filter_map(|i| if let Item::Assistant { text } = i { Some(text.as_str()) } else { None }).collect();
        assert_eq!(assistants, ["I'll get a second opinion.", "It's on it."], "{items:?}");
        let call_ix = items.iter().position(|i| matches!(i, Item::Tool { id, .. } if id == "call-1")).unwrap();
        assert!(matches!(&items[call_ix + 1], Item::Tool { id, .. } if *id == row), "{items:?}");
        assert!(matches!(&items[call_ix + 2], Item::Assistant { text } if text == "It's on it."), "{items:?}");
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
    });
}

#[test]
fn a_sub_agent_waiting_on_an_approval_shows_on_its_parent_everywhere() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let child = trek.update(cx, |ws, cx| ws.delegate(&id, &json!({ "title": "Write it", "prompt": "mock:permission", "mode": "implement" }), cx)).unwrap();
        wait_task(&trek, cx, &child, TaskState::NeedsYou).await;
        // The Dock badge counts one thing to do, and the inbox and Basecamp both show where it is.
        assert_eq!(trek.read(cx, |ws, _| ws.needs_you_count()), 1);
        assert_eq!(trek.read(cx, |ws, _| ws.ready_for_review().iter().map(|t| t.id.clone()).collect::<Vec<_>>()), [id.clone()]);
        trek.render(cx);
        assert!(trek.visible(cx, format!("card-sub-needs-{id}")), "the parent's card says it");
        assert!(!trek.visible(cx, format!("card-{child}")), "the sub-agent has no card of its own");
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        trek.render(cx);
        assert!(trek.visible(cx, format!("review-needs-{id}")));
        assert_eq!(crate::basecamp::Waiting::SubAgent.label(), "Sub-agent needs you");
        // Stopped, it asks nothing any more.
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        wait_task(&trek, cx, &child, TaskState::Cancelled).await;
        assert_eq!(trek.read(cx, |ws, _| ws.needs_you_count()), 0);
        trek.render(cx);
        assert!(!trek.visible(cx, format!("review-needs-{id}")));
    });
}
