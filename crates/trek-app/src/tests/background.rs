//! Work an agent leaves running after it has answered, and sub-agents a thread waits on. A shell
//! left running (`mock:server`, `mock:watch`) doesn't keep a thread among the working: it's
//! answered, with a quiet line on its card and a strip above the composer. Sub-agents it waits
//! on do: the thread stays in the Working group, its header says on what, and it wakes when they
//! report: whatever it was doing meanwhile, after a relaunch, together when they finish together.

use super::harness::{Trek, launch, mock, new_project, open, run, settings, store_items};
use crate::workspace::{Route, Scope, TaskState, WorkspaceEvent};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext};
use serde_json::json;
use std::cell::RefCell;
use std::rc::Rc;
use trek_core::orchestrate::{Outcome, Report};
use trek_core::store::{Item, Section, Store};
use trek_core::{Effort, HandHolding, RunState};

/// The section `id` is listed in.
fn section(trek: &Trek, cx: &TestAppContext, id: &str) -> Option<Section> {
    trek.read(cx, |ws, _| ws.sections().into_iter().find(|(_, ts)| ts.iter().any(|t| t.id == id)).map(|(s, _)| s))
}

/// Every thread the sidebar lists, in any section.
fn listed(trek: &Trek, cx: &TestAppContext) -> Vec<String> {
    trek.read(cx, |ws, _| ws.sections().into_iter().flat_map(|(_, ts)| ts.into_iter().map(|t| t.id.clone())).collect())
}

fn children(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<String> {
    trek.read(cx, |ws, _| ws.children(id).into_iter().map(|t| t.id.clone()).collect())
}

/// The wake-up messages `id` got.
fn wakes(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<String> {
    trek.items(cx, id).into_iter().filter_map(|i| if let Item::User { text, .. } = i { trek_core::orchestrate::is_wake(&text).then_some(text) } else { None }).collect()
}

async fn wait_woken(trek: &Trek, cx: &mut TestAppContext, id: &str) {
    let p = id.to_string();
    trek.wait(cx, "the wake-up", move |ws| ws.live.get(&p).is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::User { text, .. } if trek_core::orchestrate::is_wake(text))))).await;
}

/// Attention messages (alerts) the workspace raises from now on.
fn alerts(trek: &Trek, cx: &mut TestAppContext) -> Rc<RefCell<Vec<String>>> {
    let seen = Rc::new(RefCell::new(vec![]));
    let sink = seen.clone();
    cx.update(|cx| {
        cx.subscribe(&trek.ws, move |_, event: &WorkspaceEvent, _| {
            if let WorkspaceEvent::Attention { message, .. } = event {
                sink.borrow_mut().push(message.clone());
            }
        })
        .detach()
    });
    seen
}

fn strip(trek: &Trek, cx: &TestAppContext) -> Vec<String> {
    cx.read(|cx| trek.root.read(cx).background_strip.read(cx).rows())
}

#[test]
fn an_answer_with_a_server_left_running_is_done_not_working() {
    run(async |cx| {
        let trek = open(cx);
        let seen = alerts(&trek, cx);
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // Answered: out of the Working group, nothing waited on, no turn to keep the Mac awake.
        assert!(trek.answers(cx, &id).contains("http://localhost:5173"));
        assert_eq!(trek.read(cx, |ws, _| ws.live[&id].background.len()), 1, "the server runs on");
        assert!(!trek.read(cx, |ws, _| ws.waiting(&id)));
        assert_eq!(section(&trek, cx, &id), Some(Section::Inbox));
        assert!(!trek.read(cx, |ws, _| ws.any_turn_running()), "a server alone doesn't keep the Mac awake");
        assert!(!trek.read(cx, |ws, _| ws.work_in_flight()), "nor does it hold back an update");
        assert_eq!(trek.working_bar(cx), None);
        assert!(seen.borrow().iter().any(|m| m.starts_with("Finished: ")), "{:?}", seen.borrow());
        // Its card says so, quietly; the strip above the composer shows it with its last line.
        trek.render(cx);
        assert!(trek.visible(cx, format!("card-background-{id}")));
        trek.wait(cx, "the server's output", |ws| ws.live.values().any(|l| l.background.iter().any(|b| b.last_line().is_some_and(|l| l.contains("localhost"))))).await;
        trek.render(cx);
        assert!(trek.visible(cx, "background-strip"));
        assert_eq!(strip(&trek, cx), ["npm run dev — ➜  Local:   http://localhost:5173/ (stop)"]);
        // Its output, in full, with Stop.
        trek.click(cx, ("bg-task", 0usize));
        assert!(cx.read(|cx| trek.root.read(cx).background_strip.read(cx).output_open()).is_some());
        trek.render(cx);
        assert!(trek.visible(cx, "bg-output-card") && trek.visible(cx, "bg-output-text"));
        trek.click(cx, "bg-output-stop");
        let t = id.clone();
        trek.wait(cx, "the server to stop", move |ws| ws.live[&t].background.is_empty()).await;
        trek.render(cx);
        assert!(!trek.visible(cx, "background-strip") && !trek.visible(cx, format!("card-background-{id}")));
        assert_eq!(trek.run_state(cx, &id), RunState::Idle);
    });
}

#[test]
fn background_work_that_ends_quietly_changes_nothing() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:server 300ms");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let before = trek.items(cx, &id).len();
        let t = id.clone();
        trek.wait(cx, "the server to exit", move |ws| ws.live[&t].background.is_empty()).await;
        cx.run_until_parked();
        assert_eq!(trek.run_state(cx, &id), RunState::Idle);
        assert_eq!(trek.items(cx, &id).len(), before, "no turn, no rows");
        assert_eq!(section(&trek, cx, &id), Some(Section::Inbox));
    });
}

#[test]
fn background_work_that_reports_makes_the_agent_work_again() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:watch 600ms");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(section(&trek, cx, &id), Some(Section::Inbox), "the watcher runs; the thread is answered");
        assert!(!trek.read(cx, |ws, _| ws.waiting(&id)));
        trek.render(cx);
        assert_eq!(strip(&trek, cx).len(), 1);
        // The watcher catches something: the agent takes a turn of its own, and works again.
        let t = id.clone();
        trek.wait(cx, "the agent's own turn", move |ws| ws.thread(&t).is_some_and(|t| t.run_state == RunState::Working)).await;
        assert_eq!(section(&trek, cx, &id), Some(Section::Working));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("caught a failure"), "{}", trek.answers(cx, &id));
        assert!(trek.read(cx, |ws, _| ws.live[&id].background.is_empty()));
        trek.render(cx);
        assert!(strip(&trek, cx).is_empty());
    });
}

#[test]
fn several_background_tasks_fold_into_one_row_until_opened() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.send(cx, "mock:watch 30s");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        assert_eq!(strip(&trek, cx).len(), 1, "one row, with a count");
        assert!(trek.visible(cx, "bg-toggle"));
        trek.click(cx, "bg-toggle");
        trek.render(cx);
        let rows = strip(&trek, cx);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows[1].starts_with("cargo watch -x test — "), "{rows:?}");
        assert!(trek.visible(cx, ("bg-task", 1usize)));
        trek.render(cx);
        assert!(trek.visible(cx, format!("card-background-{id}")));
    });
}

#[test]
fn the_strip_is_in_thread_windows_too() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let window = trek.open_thread_window(cx, &id);
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.render_frame(cx)).unwrap();
        assert!(trek.visible_in(cx, window, "background-strip"));
        assert!(trek.visible_in(cx, window, ("bg-task", 0usize)));
    });
}

#[test]
fn a_parent_waiting_on_its_sub_agent_is_at_work_not_in_the_inbox() {
    run(async |cx| {
        let trek = open(cx);
        let seen = alerts(&trek, cx);
        let id = trek.send(cx, "mock:delegate mock:long 2s");
        let p = id.clone();
        trek.wait(cx, "the parent's answer", move |ws| ws.live[&p].turn_started.is_none() && ws.live[&p].items.iter().any(|i| matches!(i, Item::TurnEnd { .. }))).await;
        let child = children(&trek, cx, &id).pop().unwrap();
        // Its turn is over, but it waits: Working group, its header, its card, its Stop.
        assert_eq!(trek.run_state(cx, &id), RunState::Idle);
        assert!(trek.read(cx, |ws, _| ws.waiting(&id)));
        assert_eq!(section(&trek, cx, &id), Some(Section::Working));
        assert!(trek.read(cx, |ws, _| ws.ready_for_review().iter().all(|t| t.id != id)), "not ready for review yet");
        assert!(trek.working_bar(cx).is_some_and(|l| l.starts_with("Waiting on Mock Swift · ")), "{:?}", trek.working_bar(cx));
        trek.render(cx);
        assert!(trek.visible(cx, format!("card-waiting-{id}")));
        assert!(trek.visible(cx, format!("card-kids-{id}")));
        assert!(trek.visible(cx, "stop"));
        assert!(!seen.borrow().iter().any(|m| m.starts_with("Finished")), "not finished while it waits: {:?}", seen.borrow());
        // The sub-agent has no card of its own, ever.
        assert!(!listed(&trek, cx).contains(&child));
        // It reports: the parent wakes, answers, and is done.
        wait_woken(&trek, cx, &id).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(!trek.read(cx, |ws, _| ws.waiting(&id)));
        assert_eq!(section(&trek, cx, &id), Some(Section::Inbox));
        assert_eq!(seen.borrow().iter().filter(|m| m.starts_with("Finished")).count(), 1, "{:?}", seen.borrow());
        assert!(!listed(&trek, cx).contains(&child));
    });
}

#[test]
fn a_turn_blocked_on_its_sub_agent_says_whom_it_waits_on() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:consult mock:long 30s");
        let p = id.clone();
        trek.wait(cx, "the sub-agent at work", move |ws| ws.children(&p).first().is_some_and(|c| ws.task_state(&c.id) == TaskState::Running)).await;
        assert_eq!(trek.run_state(cx, &id), RunState::Working);
        assert!(trek.working_bar(cx).is_some_and(|l| l.starts_with("Waiting on Mock Swift")), "{:?}", trek.working_bar(cx));
        trek.render(cx);
        assert!(trek.visible(cx, "waiting-on"));
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(trek.working_bar(cx), None);
    });
}

#[test]
fn two_sub_agents_finishing_together_wake_their_parent_once() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:pair mock:long 400ms");
        wait_woken(&trek, cx, &id).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        let p = id.clone();
        trek.wait(cx, "both sub-agents done", move |ws| ws.children(&p).iter().all(|c| ws.task_state(&c.id) == TaskState::Done)).await;
        cx.run_until_parked();
        let woke = wakes(&trek, cx, &id);
        assert_eq!(woke.len(), 1, "one message for both: {woke:?}");
        assert_eq!(trek_core::orchestrate::wake_summary(&woke[0]), "2 sub-agents reported back");
        assert!(trek.read(cx, |ws, _| ws.store.held_reports().unwrap()).is_empty(), "delivered: nothing kept");
    });
}

#[test]
fn a_failing_sub_agent_wakes_its_parent() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:delegate error");
        wait_woken(&trek, cx, &id).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        let woke = wakes(&trek, cx, &id);
        assert!(trek_core::orchestrate::wake_summary(&woke[0]).contains("failed on"), "{woke:?}");
        assert!(!trek.read(cx, |ws, _| ws.waiting(&id)));
    });
}

#[test]
fn a_parent_asking_the_user_hears_its_sub_agent_once_answered() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:delegate mock:long 300ms");
        let p = id.clone();
        trek.wait(cx, "the parent's answer", move |ws| ws.live[&p].turn_started.is_none()).await;
        let child = children(&trek, cx, &id).pop().unwrap();
        // Meanwhile the parent asks the user something, and the sub-agent finishes.
        trek.send(cx, "mock:permission");
        trek.wait_needs_you(cx, &id).await;
        let c = child.clone();
        trek.wait(cx, "the sub-agent to finish", move |ws| ws.task_state(&c) == TaskState::Done).await;
        cx.run_until_parked();
        assert!(wakes(&trek, cx, &id).is_empty(), "held: the parent is asking");
        assert_eq!(trek.read(cx, |ws, _| ws.store.held_reports().unwrap().len()), 1, "kept until it's heard");
        let request = trek.request(cx, &id);
        trek.update(cx, |ws, cx| ws.respond(&id, &request, trek_agents::Decision::Allow, cx));
        wait_woken(&trek, cx, &id).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| ws.store.held_reports().unwrap()).is_empty());
    });
}

#[test]
fn a_parent_whose_session_was_reaped_still_wakes() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:delegate mock:long 1500ms");
        let p = id.clone();
        trek.wait(cx, "the parent's answer", move |ws| ws.live[&p].turn_started.is_none()).await;
        // Off screen and idle a while: its session is let go while its sub-agent works.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        let later = cx.executor().now() + std::time::Duration::from_secs(20 * 60);
        trek.update(cx, |ws, _| ws.reap_idle_sessions(later));
        let p = id.clone();
        trek.wait(cx, "the parent's session to end", move |ws| ws.live[&p].commands.is_none()).await;
        assert!(trek.read(cx, |ws, _| ws.waiting(&id)), "still waiting, with no session");
        // The report starts it again.
        wait_woken(&trek, cx, &id).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("The sub-agent reported back"), "{}", trek.answers(cx, &id));
    });
}

#[test]
fn a_parent_whose_turn_failed_with_messages_left_queued_still_wakes() {
    run(async |cx| {
        let trek = super::harness::open_with(cx, |s| s.general.follow_up = trek_core::settings::FollowUp::Queue);
        let id = trek.send(cx, "mock:delegate mock:long 1500ms");
        let p = id.clone();
        trek.wait(cx, "the parent's answer", move |ws| ws.live[&p].turn_started.is_none()).await;
        // Another turn, a follow-up queued behind it, and the session dies under it while the
        // thread is off screen: the follow-up stays queued.
        trek.send(cx, "mock:long 30s");
        trek.send(cx, "and then this");
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![trek_agents::AgentEvent::Exited], cx));
        assert_eq!(trek.run_state(cx, &id), RunState::Failed);
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1);
        // The report isn't held back by that.
        wait_woken(&trek, cx, &id).await;
    });
}

#[test]
fn a_parent_in_a_thread_window_wakes_and_shows_its_wait_there() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:delegate mock:long 1500ms");
        let p = id.clone();
        trek.wait(cx, "the parent's answer", move |ws| ws.live[&p].turn_started.is_none()).await;
        let window = trek.open_thread_window(cx, &id);
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        cx.update_window(window, |_, window, cx| window.render_frame(cx)).unwrap();
        assert!(trek.visible_in(cx, window, "waiting-on"), "its window says what it waits on");
        assert_eq!(trek.read(cx, |ws, _| ws.shown_in(&id)), Some(Scope::Thread(id.clone())));
        wait_woken(&trek, cx, &id).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        cx.update_window(window, |_, window, cx| window.render_frame(cx)).unwrap();
        assert!(!trek.visible_in(cx, window, "waiting-on"));
    });
}

#[test]
fn a_relaunch_delivers_reports_held_and_says_what_quitting_cut_off() {
    run(async |cx| {
        let dir = new_project("relaunch");
        let db = dir.join("trek.sqlite");
        let project = new_project("project");
        let (parent, done, cut, busy, nested) = {
            let store = Store::open(&db).expect("store");
            let thread = |title: &str, parent: Option<&str>| {
                let mut t = store.create_thread(Some(&project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
                t.title = title.into();
                t.parent_id = parent.map(str::to_string);
                store.save_thread(&t).unwrap();
                t.id
            };
            let parent = thread("Plan the release", None);
            store_items(&store, &parent, vec![Item::User { text: "go".into(), images: vec![], at: Some(1), resume: None, aside: false }, Item::Assistant { text: "Asked two models.".into() }]);
            // One finished before the quit, and its parent hadn't heard; one was cut off by it.
            let done = thread("Review the plan", Some(&parent));
            store.hold_report(&parent, &Report { id: done.clone(), title: "Review the plan".into(), model: "Mock Swift".into(), outcome: Outcome::Done("Ship it.".into()) }).unwrap();
            let cut = thread("Check the changelog", Some(&parent));
            store.await_report(&parent, &cut).unwrap();
            store.update_thread(&cut, |t| t.run_state = RunState::Working).unwrap();
            // A parent cut off mid-turn itself (it was waiting for an answer in a call) isn't woken.
            let busy = thread("Mid-turn", None);
            store.update_thread(&busy, |t| t.run_state = RunState::Working).unwrap();
            let busy_kid = thread("Its helper", Some(&busy));
            store.await_report(&busy, &busy_kid).unwrap();
            // Nor is a sub-agent: its answer would have nowhere to go.
            let nested = thread("Nested", Some(&parent));
            let grandkid = thread("Deep", Some(&nested));
            store.await_report(&nested, &grandkid).unwrap();
            (parent, done, cut, busy, nested)
        };
        let mut s = settings();
        s.user_projects.push(project.display().to_string());
        let (ws, root, window) = launch(cx, Store::open(&db).expect("store"), s);
        let trek = Trek { ws, root, window, project };
        wait_woken(&trek, cx, &parent).await;
        trek.wait_done(cx, &parent, RunState::Idle).await;
        let woke = wakes(&trek, cx, &parent);
        assert_eq!(woke.len(), 1, "{woke:?}");
        assert!(woke[0].contains(&format!("task {done}")) && woke[0].contains("Ship it."), "{}", woke[0]);
        assert!(woke[0].contains(&format!("task {cut}")) && woke[0].contains(trek_core::orchestrate::CUT_OFF), "{}", woke[0]);
        // What it said before is all still there, ahead of the wake-up.
        let items = trek.items(cx, &parent);
        assert!(matches!(&items[..2], [Item::User { text, .. }, Item::Assistant { text: a }] if text == "go" && a == "Asked two models."), "{items:?}");
        assert!(wakes(&trek, cx, &busy).is_empty() && trek.read(cx, |ws, _| ws.live.get(&busy).is_none_or(|l| l.commands.is_none())));
        assert!(wakes(&trek, cx, &nested).is_empty());
        assert!(trek.read(cx, |ws, _| ws.store.held_reports().unwrap()).is_empty(), "nothing left to deliver");
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn sub_agents_never_get_inbox_cards() {
    run(async |cx| {
        let trek = open(cx);
        // Trek's own, whatever state they're in: at work, asking, failed.
        let id = trek.quiet_thread(cx);
        let asking = trek.update(cx, |ws, cx| ws.delegate(&id, &json!({ "title": "Write it", "prompt": "mock:permission", "mode": "implement" }), cx)).unwrap();
        let failing = trek.update(cx, |ws, cx| ws.delegate(&id, &json!({ "title": "Break it", "prompt": "error" }), cx)).unwrap();
        let a = asking.clone();
        trek.wait(cx, "the sub-agent to ask", move |ws| ws.task_state(&a) == TaskState::NeedsYou).await;
        let f = failing.clone();
        trek.wait(cx, "the other to fail", move |ws| ws.task_state(&f) == TaskState::Failed).await;
        let shown = listed(&trek, cx);
        assert!(shown.contains(&id) && !shown.contains(&asking) && !shown.contains(&failing), "{shown:?}");
        assert_eq!(trek.read(cx, |ws, _| ws.needs_you_count()), 1, "the parent's card carries the request");
        trek.render(cx);
        assert!(!trek.visible(cx, format!("card-{asking}")) && !trek.visible(cx, format!("card-{failing}")));
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        // The agent's own sub-agents aren't threads at all.
        let before = trek.read(cx, |ws, _| ws.threads.len());
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        let other = trek.send(cx, "send subagents 300ms");
        let o = other.clone();
        trek.wait(cx, "the scouts' report", move |ws| ws.live[&o].items.iter().any(|i| matches!(i, Item::Assistant { text } if text.contains("reported back")))).await;
        trek.wait_done(cx, &other, RunState::Idle).await;
        assert_eq!(trek.read(cx, |ws, _| ws.threads.len()), before + 1, "only the thread that sent them");
        // Imported history: a sub-agent's transcript next to its session is put away, not listed.
        let dir = trek_core::paths::home().join(".claude/projects/-Users-me-code-trek-bg-import");
        std::fs::create_dir_all(&dir).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let lines = |side: bool| {
            [
                json!({"type":"user","message":{"role":"user","content":"Plan the trip"},"timestamp":now,"cwd":"/Users/me/code/trek-bg-import","sessionId":"bg-main","isSidechain":side,"entrypoint":"cli"}),
                json!({"type":"assistant","message":{"role":"assistant","model":"claude-x","content":[{"type":"text","text":"Planned."}],"stop_reason":"end_turn"},"timestamp":now,"cwd":"/Users/me/code/trek-bg-import","sessionId":"bg-main","isSidechain":side,"entrypoint":"cli"}),
            ]
            .map(|l| l.to_string())
            .join("\n")
                + "\n"
        };
        std::fs::write(dir.join("bg-main.jsonl"), lines(false)).unwrap();
        std::fs::write(dir.join("agent-bg-scout.jsonl"), lines(true)).unwrap();
        trek.update(cx, |ws, cx| {
            let settings = ws.settings.import.clone();
            trek_core::import::import_all(&ws.store, &settings);
            ws.reload(cx);
        });
        let native = |n: &str| trek.read(cx, |ws, _| ws.threads.iter().find(|t| t.native_id.as_deref() == Some(n)).map(|t| t.id.clone()));
        let main = native("bg-main").expect("the session is imported");
        let shown = listed(&trek, cx);
        assert!(shown.contains(&main), "{shown:?}");
        assert!(native("agent-bg-scout").is_none_or(|kid| !shown.contains(&kid)), "the sub-agent's transcript isn't a card");
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn stop_on_a_parent_waiting_on_its_agent_s_own_sub_agents_stops_them() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "send subagents 30s");
        let p = id.clone();
        trek.wait(cx, "the answer with agents out", move |ws| ws.live[&p].turn_started.is_none() && ws.live[&p].background.len() == 2).await;
        assert!(trek.read(cx, |ws, _| ws.waiting(&id)));
        trek.render(cx);
        trek.click(cx, "stop");
        let p = id.clone();
        trek.wait(cx, "the agents to stop", move |ws| ws.live[&p].background.is_empty()).await;
        assert!(!trek.read(cx, |ws, _| ws.waiting(&id)));
        assert_eq!(section(&trek, cx, &id), Some(Section::Inbox));
    });
}

#[test]
fn a_sub_agent_s_row_opens_on_what_it_is_doing() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:delegate mock:explore 30s");
        let p = id.clone();
        trek.wait(cx, "the sub-agent to read a file", move |ws| {
            ws.children(&p).first().and_then(|c| ws.live.get(&c.id)).is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::Tool { title, .. } if title == "Read")))
        })
        .await;
        trek.render(cx);
        let row = trek.item_ix(cx, &id, |i| matches!(i, Item::Tool { id, .. } if trek_core::orchestrate::task_of_row(id).is_some()));
        assert_eq!(cx.read(|cx| trek.root.read(cx).thread_view.read(cx).ticker()), Some(false), "its dot breathes, slowly");
        assert!(!trek.visible(cx, ("subagent-activity", row)));
        // Opened: its live group, as the working bar shows its parent's, and a brisker redraw.
        trek.click(cx, ("subagent", row));
        trek.render(cx);
        assert!(trek.visible(cx, ("subagent-activity", row)));
        let rows = trek.rows(cx);
        let at = rows.iter().position(|r| r.starts_with("subagent: Mock Swift: Second opinion")).unwrap();
        assert!(rows.get(at + 1).is_some_and(|l| l.starts_with("  ") && l.contains("·")), "its summary: {rows:?}");
        assert!(rows[at + 2..].iter().take_while(|l| l.starts_with("  ")).any(|l| l.starts_with("  Read ")), "{rows:?}");
        assert_eq!(cx.read(|cx| trek.root.read(cx).thread_view.read(cx).ticker()), Some(true));
        // Its own thread still opens in a window.
        assert!(trek.visible(cx, ("subagent-open", row)));
        trek.click(cx, ("subagent", row));
        trek.render(cx);
        assert!(!trek.visible(cx, ("subagent-activity", row)));
        assert_eq!(cx.read(|cx| trek.root.read(cx).thread_view.read(cx).ticker()), Some(false));
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
    });
}
