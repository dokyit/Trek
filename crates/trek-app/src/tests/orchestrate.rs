//! Sub-agents: an agent hands work to another through Trek's orchestration tools. The mock agent
//! plays both sides (`mock:consult` waits for its sub-agent, `mock:delegate` doesn't), calling
//! Trek over the same socket `trek-mcp orchestrate` uses; other tests call the tools directly.

use super::harness::{Trek, mock, open, run};
use crate::workspace::TaskState;
use gpui_kit::TestAppContext;
use serde_json::json;
use trek_core::store::{Item, ToolStatus};
use trek_core::{HandHolding, RunState};

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
