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
use trek_core::store::{Item, Section, Store, ToolStatus};
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
        assert!(trek.visible(cx, format!("live-line-at-work-{id}")));
        trek.wait(cx, "the server's output", |ws| ws.live.values().any(|l| l.background.iter().any(|b| b.last_line().is_some_and(|l| l.contains("localhost"))))).await;
        trek.render(cx);
        assert!(trek.visible(cx, "background-strip"));
        assert_eq!(strip(&trek, cx), ["npm run dev — ➜  Local:   http://localhost:5173/ (stop)"]);
        // The end of its output, with Stop, and the whole of it a click away (the agent says where).
        trek.click(cx, ("bg-task", 0usize));
        assert!(cx.read(|cx| trek.root.read(cx).background_strip.read(cx).output_open()).is_some());
        trek.render(cx);
        assert!(trek.visible(cx, "bg-output-card") && trek.visible(cx, "bg-output-text"));
        assert!(trek.visible(cx, "bg-output-full"));
        trek.click(cx, "bg-output-stop");
        let t = id.clone();
        trek.wait(cx, "the server to stop", move |ws| ws.live[&t].background.is_empty()).await;
        trek.render(cx);
        assert!(!trek.visible(cx, "background-strip") && !trek.visible(cx, format!("live-line-at-work-{id}")));
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
        // Watching through the checks below: a loaded runner can take seconds to reach them.
        let id = trek.send(cx, "mock:watch 8s");
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
fn turns_a_watcher_wakes_the_agent_for_raise_one_alert_a_while() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let seen = alerts(&trek, cx);
        // A watcher wakes the agent, again and again: one turn of its own after another.
        let woken = |text: &str, error: Option<&str>| {
            vec![trek_agents::AgentEvent::TextDelta(text.into()), trek_agents::AgentEvent::TurnComplete { error: error.map(str::to_string) }]
        };
        trek.update(cx, |ws, cx| ws.apply_events(&id, woken("The watcher caught a failure; fixed it.", None), cx));
        trek.update(cx, |ws, cx| ws.apply_events(&id, woken("Another one; fixed too.", None), cx));
        assert_eq!(seen.borrow().len(), 1, "one alert for the two: {:?}", seen.borrow());
        assert!(seen.borrow()[0].starts_with("Finished"));
        // One that fails says so whatever.
        trek.update(cx, |ws, cx| ws.apply_events(&id, woken("Trying again.", Some("Claude Code crashed")), cx));
        assert!(seen.borrow().last().is_some_and(|m| m.starts_with("Failed")), "{:?}", seen.borrow());
        // The user's own turns raise theirs as ever.
        trek.send(cx, "thanks");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(seen.borrow().len() == 3 && seen.borrow()[2].starts_with("Finished"), "{:?}", seen.borrow());
    });
}

#[test]
fn a_settled_thread_with_work_in_the_background_says_so_in_its_line() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        let quiet = trek.send(cx, "hello");
        assert_ne!(quiet, id);
        trek.wait_done(cx, &quiet, RunState::Idle).await;
        trek.update(cx, |ws, cx| {
            ws.settle(&id, cx);
            ws.settle(&quiet, cx);
            ws.settled_open = true;
            ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx);
        });
        trek.render(cx);
        assert_eq!(section(&trek, cx, &id), Some(Section::Settled), "settled, it stays settled");
        assert!(trek.visible(cx, format!("line-at-work-{id}")), "its line has the dot");
        assert!(!trek.visible(cx, format!("line-at-work-{quiet}")));
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
        assert!(trek.visible(cx, format!("live-line-at-work-{id}")));
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
        // Still working through the checks below: a loaded runner can take seconds to reach them.
        let id = trek.send(cx, "mock:delegate mock:long 8s");
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
        // The sub-agent is stopped with its parent a moment later (its mock work runs on the
        // runtime's threads): the wait line goes once it has.
        let p = id.clone();
        trek.wait(cx, "the sub-agent to stop", move |ws| !ws.waiting(&p) && ws.children(&p).iter().all(|c| ws.task_state(&c.id) != TaskState::Running)).await;
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
        // Still running until the parent's ask lands: its report is what's held meanwhile.
        let id = trek.send(cx, "mock:delegate mock:long 5s");
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
        let id = trek.send(cx, "mock:delegate mock:long 8s");
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
        let id = trek.send(cx, "mock:delegate mock:long 8s");
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
        trek.wait_done(cx, &id, RunState::Idle).await;
        // Nor does the follow-up, written for the turn that failed, go out after the wake-up's.
        cx.run_until_parked();
        let sent = |trek: &Trek, cx: &TestAppContext| trek.items(cx, &id).iter().any(|i| matches!(i, Item::User { text, .. } if text == "and then this"));
        assert!(!sent(&trek, cx), "not sent unattended");
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1);
        // On screen again, it's back in the composer.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 0);
        assert!(trek.composer_text(cx).contains("and then this"), "{}", trek.composer_text(cx));
        assert!(!sent(&trek, cx));
    });
}

#[test]
fn a_parent_in_a_thread_window_wakes_and_shows_its_wait_there() {
    run(async |cx| {
        let trek = open(cx);
        // Still working when its window renders: a loaded runner can take seconds to get there
        // (a window, a navigation and a frame), so it works for fifteen seconds.
        let id = trek.send(cx, "mock:delegate mock:long 15s");
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
        let (parent, done, cut, busy, busy_done, nested) = {
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
            // A parent cut off mid-turn itself (it was waiting for an answer in a call) isn't woken,
            // though one of its sub-agents had finished: the user picks it up.
            let busy = thread("Mid-turn", None);
            store_items(&store, &busy, vec![Item::User { text: "go".into(), images: vec![], at: Some(1), resume: None, aside: false }]);
            store.update_thread(&busy, |t| t.run_state = RunState::Working).unwrap();
            let busy_kid = thread("Its helper", Some(&busy));
            store.await_report(&busy, &busy_kid).unwrap();
            let busy_done = thread("Its scout", Some(&busy));
            store.hold_report(&busy, &Report { id: busy_done.clone(), title: "Its scout".into(), model: "Mock Swift".into(), outcome: Outcome::Done("Found it.".into()) }).unwrap();
            // Nor is a sub-agent: its answer would have nowhere to go.
            let nested = thread("Nested", Some(&parent));
            let grandkid = thread("Deep", Some(&nested));
            store.await_report(&nested, &grandkid).unwrap();
            (parent, done, cut, busy, busy_done, nested)
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
        assert!(!trek.read(cx, |ws, _| ws.waiting(&busy)), "it waits on the user, not on its sub-agents");
        assert_eq!(section(&trek, cx, &busy), Some(Section::Inbox));
        assert!(wakes(&trek, cx, &nested).is_empty());
        assert_eq!(trek.read(cx, |ws, _| ws.store.held_reports().unwrap().len()), 2, "the cut-off parent's are kept");
        // And its transcript says so, under the turn the quit cut off.
        let said = trek.read(cx, |ws, _| ws.store.items(&busy).unwrap());
        assert!(matches!(&said[said.len() - 2..], [Item::Notice { text: cut }, Item::Notice { text: kept }] if cut == trek_core::store::INTERRUPTED_BY_QUIT && kept.starts_with("2 sub-agents' reports are kept")), "{said:?}");
        // The user picks it up, and its agent collects the scout's answer itself.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(busy.clone()), cx));
        let (reply, answer) = async_channel::bounded(1);
        let call = crate::ipc::Call { thread: busy.clone(), method: "task_result".into(), params: json!({ "id": busy_done }), reply };
        trek.update(cx, |ws, cx| ws.handle_call(call, cx));
        let got = answer.try_recv();
        assert!(matches!(&got, Ok(crate::ipc::Reply::Done(Ok(_)))), "{got:?}");
        assert_eq!(trek.read(cx, |ws, _| ws.store.held_reports().unwrap().len()), 1, "read, it isn't kept");
        // Once that turn is over, it hears the rest.
        trek.send(cx, "carry on");
        wait_woken(&trek, cx, &busy).await;
        trek.wait_done(cx, &busy, RunState::Idle).await;
        let woke = wakes(&trek, cx, &busy);
        assert_eq!(woke.len(), 1, "{woke:?}");
        assert!(!woke[0].contains("Found it.") && woke[0].contains(trek_core::orchestrate::CUT_OFF), "{}", woke[0]);
        let items = trek.items(cx, &busy);
        let carry = items.iter().position(|i| matches!(i, Item::User { text, .. } if text == "carry on")).unwrap();
        let wake = items.iter().position(|i| matches!(i, Item::User { text, .. } if trek_core::orchestrate::is_wake(text))).unwrap();
        assert!(carry < wake, "after the user's own turn");
        assert!(trek.read(cx, |ws, _| ws.store.held_reports().unwrap()).is_empty(), "nothing left to deliver");
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn reports_parked_by_a_quit_stay_parked_through_another_relaunch() {
    run(async |cx| {
        let dir = new_project("parked-relaunch");
        let db = dir.join("trek.sqlite");
        let project = new_project("project");
        let busy = {
            let store = Store::open(&db).expect("store");
            let mut t = store.create_thread(Some(&project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            t.title = "Mid-turn".into();
            t.run_state = RunState::Working;
            store.save_thread(&t).unwrap();
            store_items(&store, &t.id, vec![Item::User { text: "go".into(), images: vec![], at: Some(1), resume: None, aside: false }]);
            let mut kid = store.create_thread(Some(&project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            kid.parent_id = Some(t.id.clone());
            store.save_thread(&kid).unwrap();
            store.await_report(&t.id, &kid.id).unwrap();
            t.id
        };
        let mut s = settings();
        s.user_projects.push(project.display().to_string());
        // The launch after the quit parks the report; the one after that, with the user not back
        // in the thread yet, mustn't start a turn for it.
        for launch_no in 0..2 {
            let (ws, root, window) = launch(cx, Store::open(&db).expect("store"), s.clone());
            let trek = Trek { ws, root, window, project: project.clone() };
            // (Launch wake-ups go out at once under test.)
            cx.background_executor.timer(std::time::Duration::from_millis(200)).await;
            cx.run_until_parked();
            assert!(wakes(&trek, cx, &busy).is_empty(), "launch {launch_no}: no wake-up before the user's next message");
            assert_eq!(trek.read(cx, |ws, _| ws.store.held_reports().unwrap().len()), 1, "launch {launch_no}");
            let kept = trek.read(cx, |ws, _| ws.store.items(&busy).unwrap()).iter().filter(|i| matches!(i, Item::Notice { text } if text.contains("kept for this thread"))).count();
            assert_eq!(kept, 1, "launch {launch_no}: said once");
            if launch_no == 1 {
                trek.update(cx, |ws, cx| ws.navigate(Route::Thread(busy.clone()), cx));
                trek.send(cx, "carry on");
                wait_woken(&trek, cx, &busy).await;
                trek.wait_done(cx, &busy, RunState::Idle).await;
                assert!(trek.read(cx, |ws, _| ws.store.held_reports().unwrap()).is_empty());
            }
            trek.window(cx, |window, _| window.remove_window());
        }
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn sub_agents_out_in_the_background_when_trek_quit_end_with_it() {
    run(async |cx| {
        let dir = new_project("orphans");
        let db = dir.join("trek.sqlite");
        let project = new_project("project");
        // Answered, with the agent's own scout still out: stored idle, its row still running.
        let id = {
            let store = Store::open(&db).expect("store");
            let t = store.create_thread(Some(&project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            store_items(&store, &t.id, vec![
                Item::User { text: "send a scout".into(), images: vec![], at: Some(1), resume: None, aside: false },
                Item::Tool { id: "agent-1".into(), title: "Subagent".into(), detail: "Map the HTTP routes".into(), output: "Async agent launched successfully.".into(), status: ToolStatus::Running },
                Item::Assistant { text: "The scout is out.".into() },
            ]);
            t.id
        };
        let mut s = settings();
        s.user_projects.push(project.display().to_string());
        let (ws, root, window) = launch(cx, Store::open(&db).expect("store"), s);
        let trek = Trek { ws, root, window, project };
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        // No session of this run has it: it ended with the last one, and the transcript says so.
        for items in [trek.items(cx, &id), trek.read(cx, |ws, _| ws.store.items(&id).unwrap())] {
            assert!(items.iter().any(|i| matches!(i, Item::Tool { id, status: ToolStatus::Failed, .. } if id == "agent-1")), "{items:?}");
            assert!(matches!(items.last(), Some(Item::Notice { text }) if text == crate::workspace::ORPHANS_ENDED), "{items:?}");
        }
        assert!(!trek.read(cx, |ws, _| ws.waiting(&id)));
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
        let seen = alerts(&trek, cx);
        let id = trek.send(cx, "send subagents 30s");
        let p = id.clone();
        trek.wait(cx, "the answer with agents out", move |ws| ws.live[&p].turn_started.is_none() && ws.live[&p].background.len() == 2).await;
        assert!(trek.read(cx, |ws, _| ws.waiting(&id)));
        trek.render(cx);
        seen.borrow_mut().clear();
        trek.click(cx, "stop");
        let p = id.clone();
        trek.wait(cx, "the agents to stop", move |ws| ws.live[&p].background.is_empty()).await;
        // Claude Code takes a turn of its own to say they were stopped (the mock does as it
        // does): it shows, but it's no news.
        let p = id.clone();
        trek.wait(cx, "the agent saying so", move |ws| ws.live[&p].items.iter().any(|i| matches!(i, Item::Assistant { text } if text.contains("was stopped")))).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        cx.run_until_parked();
        assert!(seen.borrow().is_empty(), "no alert for it: {:?}", seen.borrow());
        assert!(!trek.read(cx, |ws, _| ws.waiting(&id)));
        assert_eq!(section(&trek, cx, &id), Some(Section::Inbox));
        // The next turn the user asks for is news again.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        trek.update(cx, |ws, cx| ws.send_to(&id, "hello".into(), vec![], cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(seen.borrow().iter().any(|m| m.starts_with("Finished")), "{:?}", seen.borrow());
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

/// Turn fast mode on or off for `scope`: settings the agent reads at launch, so its session
/// restarts when nothing would be cut short.
fn toggle_fast(trek: &Trek, cx: &mut TestAppContext, scope: &Scope) {
    trek.update(cx, |ws, cx| {
        let mut p = ws.prefs_in(scope);
        p.fast = !p.fast;
        ws.set_prefs_in(scope, p, cx);
    });
}

fn session_alive(trek: &Trek, cx: &TestAppContext, id: &str) -> bool {
    trek.read(cx, |ws, _| ws.live.get(id).is_some_and(|l| l.commands.is_some()))
}

#[test]
fn a_setting_read_at_launch_reaches_a_session_with_background_work_without_restarting_it() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // A restart would end the server: the session is told the new setting as it runs.
        trek.update(cx, |ws, cx| {
            let mut p = ws.prefs_in(&Scope::Main);
            p.plan = true;
            ws.set_prefs_in(&Scope::Main, p, cx);
        });
        cx.run_until_parked();
        assert!(session_alive(&trek, cx, &id), "not restarted under the server");
        assert_eq!(trek.read(cx, |ws, _| ws.live[&id].background.len()), 1);
        // The next message runs in plan mode: a plan to approve, nothing changed.
        trek.send(cx, "refactor the router");
        trek.wait_needs_you(cx, &id).await;
        assert!(trek.read(cx, |ws, _| ws.live[&id].permissions.iter().any(|p| matches!(p.prompt, Some(trek_agents::Prompt::Plan(_))))), "a plan, in plan mode");
        assert_eq!(trek.read(cx, |ws, _| ws.live[&id].background.len()), 1, "the server is still up");
    });
}

#[test]
fn a_setting_changed_mid_turn_with_background_work_reaches_the_session_after_the_turn() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.send(cx, "mock:long 600s");
        let t = id.clone();
        trek.wait(cx, "the long turn", move |ws| ws.turn_running(&t)).await;
        trek.update(cx, |ws, cx| {
            let mut p = ws.prefs_in(&Scope::Main);
            p.plan = true;
            ws.set_prefs_in(&Scope::Main, p, cx);
        });
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        cx.run_until_parked();
        assert!(session_alive(&trek, cx, &id) && trek.read(cx, |ws, _| ws.live[&id].background.len()) == 1, "the server outlives the change");
        trek.send(cx, "refactor the router");
        trek.wait_needs_you(cx, &id).await;
        assert!(trek.read(cx, |ws, _| ws.live[&id].permissions.iter().any(|p| matches!(p.prompt, Some(trek_agents::Prompt::Plan(_))))));
    });
}

#[test]
fn a_setting_read_at_launch_doesn_t_strand_a_thread_waiting_on_its_agent_s_own_sub_agents() {
    run(async |cx| {
        let trek = open(cx);
        // Agents out long enough that a loaded runner still finds them out (`600ms` can be
        // over before the first poll), and back well inside a `wait`.
        let id = trek.send(cx, "send subagents 8s");
        let p = id.clone();
        trek.wait(cx, "the answer with agents out", move |ws| ws.live[&p].turn_started.is_none() && ws.live[&p].background.len() == 2).await;
        toggle_fast(&trek, cx, &Scope::Main);
        cx.run_until_parked();
        assert!(session_alive(&trek, cx, &id), "its sub-agents would never report");
        assert!(trek.read(cx, |ws, _| ws.waiting(&id)));
        // It took the setting as it runs; they report, and the agent takes its turn in the same
        // session.
        let p = id.clone();
        trek.wait(cx, "the scouts' report", move |ws| ws.live[&p].items.iter().any(|i| matches!(i, Item::Assistant { text } if text.contains("reported back")))).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(session_alive(&trek, cx, &id), "no restart");
        assert!(!trek.read(cx, |ws, _| ws.waiting(&id)));
        assert_eq!(section(&trek, cx, &id), Some(Section::Inbox));
    });
}

#[test]
fn a_sub_agent_finishing_while_its_parent_s_session_restarts_still_wakes_it() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:delegate mock:long 8s");
        let p = id.clone();
        trek.wait(cx, "the parent's answer", move |ws| ws.live[&p].turn_started.is_none()).await;
        // Idle with nothing in the background: the setting restarts its session at once.
        toggle_fast(&trek, cx, &Scope::Main);
        cx.run_until_parked();
        assert!(!session_alive(&trek, cx, &id), "restarted");
        assert!(trek.read(cx, |ws, _| ws.waiting(&id)), "still waiting, between sessions");
        wait_woken(&trek, cx, &id).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(wakes(&trek, cx, &id).len(), 1);
        assert!(trek.answers(cx, &id).contains("The sub-agent reported back"), "{}", trek.answers(cx, &id));
    });
}

/// A sub-agent of Trek's, sent out by a parent, whose own turn has ended with its agent's own
/// sub-agents still out: (parent, child).
async fn delegate_with_agents_out(trek: &Trek, cx: &mut TestAppContext) -> (String, String) {
    let id = trek.send(cx, "mock:delegate subagents 30s");
    let p = id.clone();
    trek.wait(cx, "the sub-agent with agents of its own out", move |ws| {
        ws.children(&p).first().and_then(|c| ws.live.get(&c.id)).is_some_and(|l| l.turn_started.is_none() && l.background_agents().count() == 2)
    })
    .await;
    let child = children(trek, cx, &id).pop().unwrap();
    assert_eq!(trek.read(cx, |ws, _| ws.task_state(&child)), TaskState::Running, "it waits on them: not done yet");
    (id, child)
}

#[test]
fn a_sub_agent_whose_session_dies_with_its_own_sub_agents_out_reports_failing() {
    run(async |cx| {
        let trek = open(cx);
        let (id, child) = delegate_with_agents_out(&trek, cx).await;
        trek.update(cx, |ws, cx| ws.apply_events(&child, vec![trek_agents::AgentEvent::Exited], cx));
        assert_eq!(trek.read(cx, |ws, _| ws.task_state(&child)), TaskState::Failed);
        wait_woken(&trek, cx, &id).await;
        let woke = wakes(&trek, cx, &id);
        assert!(trek_core::orchestrate::wake_summary(&woke[0]).contains("failed on"), "{woke:?}");
        assert!(woke[0].contains("own sub-agents were still at work"), "{}", woke[0]);
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(!trek.read(cx, |ws, _| ws.waiting(&id)));
    });
}

#[test]
fn a_sub_agent_the_user_stops_while_its_own_sub_agents_are_out_reports_stopped() {
    run(async |cx| {
        let trek = open(cx);
        let (id, child) = delegate_with_agents_out(&trek, cx).await;
        // From its own window, say: Stop between its turns.
        trek.update(cx, |ws, cx| ws.interrupt(&child, cx));
        assert_eq!(trek.read(cx, |ws, _| ws.task_state(&child)), TaskState::Cancelled);
        wait_woken(&trek, cx, &id).await;
        let woke = wakes(&trek, cx, &id);
        assert!(woke[0].contains("was stopped before it finished"), "{woke:?}");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let c = child.clone();
        trek.wait(cx, "its agents gone with its session", move |ws| ws.live[&c].background.is_empty()).await;
        assert!(!trek.read(cx, |ws, _| ws.waiting(&id)));
    });
}

#[test]
fn a_sub_agent_whose_own_sub_agents_end_with_no_turn_after_is_done_after_a_grace() {
    run(async |cx| {
        let trek = open(cx);
        let (id, child) = delegate_with_agents_out(&trek, cx).await;
        // They end, and the agent takes no turn for them (it might, for a while).
        trek.update(cx, |ws, cx| ws.apply_events(&child, vec![trek_agents::AgentEvent::Background(vec![])], cx));
        assert_eq!(trek.read(cx, |ws, _| ws.task_state(&child)), TaskState::Running, "it may still take its turn");
        cx.executor().advance_clock(std::time::Duration::from_secs(16));
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.task_state(&child)), TaskState::Done);
        wait_woken(&trek, cx, &id).await;
    });
}

#[test]
fn an_imported_parent_hears_its_reports_after_its_history_has_loaded() {
    run(async |cx| {
        let dir = new_project("imported-parent");
        let db = dir.join("trek.sqlite");
        let project = new_project("project");
        // A Claude Code session picked up in Trek: its history stays in Claude's files.
        let native = "bg-imported-parent";
        let folder = trek_core::paths::home().join(".claude/projects/-Users-me-code-trek-bg-imported");
        std::fs::create_dir_all(&folder).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let lines = [
            json!({"type":"user","message":{"role":"user","content":"Plan the trip"},"timestamp":now,"cwd":"/Users/me/code/trek-bg-imported","sessionId":native,"entrypoint":"cli"}),
            json!({"type":"assistant","message":{"role":"assistant","model":"claude-x","content":[{"type":"text","text":"Asked a scout."}],"stop_reason":"end_turn"},"timestamp":now,"cwd":"/Users/me/code/trek-bg-imported","sessionId":native,"entrypoint":"cli"}),
        ];
        std::fs::write(folder.join(format!("{native}.jsonl")), lines.map(|l| l.to_string()).join("\n") + "\n").unwrap();
        let parent = {
            let store = Store::open(&db).expect("store");
            let mut t = store.create_thread(Some(&project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            t.title = "Plan the trip".into();
            t.source = trek_core::ThreadSource::ClaudeCode;
            t.native_id = Some(native.into());
            store.save_thread(&t).unwrap();
            let kid = store.create_thread(Some(&project), mock(), None, Effort::Medium, HandHolding::Auto).expect("kid");
            store.update_thread(&kid.id, |k| k.parent_id = Some(t.id.clone())).unwrap();
            store.hold_report(&t.id, &Report { id: kid.id.clone(), title: "Scout".into(), model: "Mock Swift".into(), outcome: Outcome::Done("Book the hut.".into()) }).unwrap();
            t.id
        };
        let mut s = settings();
        s.user_projects.push(project.display().to_string());
        let (ws, root, window) = launch(cx, Store::open(&db).expect("store"), s);
        let trek = Trek { ws, root, window, project };
        wait_woken(&trek, cx, &parent).await;
        let items = trek.items(cx, &parent);
        let wake = items.iter().position(|i| matches!(i, Item::User { text, .. } if trek_core::orchestrate::is_wake(text))).unwrap();
        assert!(matches!(&items[..wake], [Item::User { text, .. }, Item::Assistant { text: a }, ..] if text == "Plan the trip" && a == "Asked a scout."), "{items:?}");
        assert!(items[wake].clone() != items[0] && wakes(&trek, cx, &parent)[0].contains("Book the hut."));
        trek.wait_done(cx, &parent, RunState::Idle).await;
        let _ = std::fs::remove_dir_all(folder);
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn a_background_server_doesn_t_redraw_the_sidebar() {
    run(async |cx| {
        let trek = open(cx);
        // In front, with motion on: whatever animates, animates.
        trek.window(cx, |window, _| window.activate_window());
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let t = id.clone();
        trek.wait(cx, "the server's output", move |ws| ws.live[&t].background.iter().any(|b| b.output.is_some())).await;
        trek.render(cx);
        assert!(trek.visible(cx, format!("live-line-at-work-{id}")));
        cx.run_until_parked();
        super::take_renders();
        // The strip reads the server's output and moves its clock on (the test platform draws
        // as soon as anything is dirty); the sidebar keeps its cache.
        for _ in 0..3 {
            cx.executor().advance_clock(std::time::Duration::from_secs(2));
            cx.run_until_parked();
        }
        let renders = super::take_renders();
        assert!(renders.get("BackgroundStrip").is_some_and(|n| *n > 0), "the strip ticks: {renders:?}");
        assert_eq!(renders.get("Sidebar"), None, "{renders:?}");
    });
}

#[test]
fn a_task_that_won_t_stop_can_be_asked_again() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // A task the agent doesn't stop (it couldn't, and said so).
        let mut set: Vec<trek_agents::BackgroundTask> = trek.read(cx, |ws, _| ws.live[&id].background.iter().map(|b| b.task.clone()).collect());
        set.push(trek_agents::BackgroundTask { id: "stuck".into(), kind: trek_agents::BackgroundKind::Shell, title: "tail -f log".into(), call: None, readable: false, stoppable: true });
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![trek_agents::AgentEvent::Background(set)], cx));
        trek.update(cx, |ws, cx| ws.stop_background(&id, "stuck", cx));
        let stopping = |trek: &Trek, cx: &TestAppContext| trek.read(cx, |ws, _| ws.live[&id].background.iter().find(|b| b.task.id == "stuck").map(|b| b.stopping));
        assert_eq!(stopping(&trek, cx), Some(true));
        cx.executor().advance_clock(std::time::Duration::from_secs(11));
        cx.run_until_parked();
        assert_eq!(stopping(&trek, cx), Some(false), "Stop is offered again");
    });
}

#[test]
fn a_session_that_ends_says_what_it_took_with_it() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![trek_agents::AgentEvent::Exited], cx));
        assert!(trek.items(cx, &id).iter().any(|i| matches!(i, Item::Notice { text } if text == "npm run dev stopped with the session")), "{:?}", trek.items(cx, &id));
        assert!(trek.read(cx, |ws, _| ws.live[&id].background.is_empty()));
    });
}

#[test]
fn a_turn_blocked_on_its_agent_s_own_sub_agent_says_it_waits_on_it() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:task 30s");
        let p = id.clone();
        trek.wait(cx, "the sub-agent at work", move |ws| ws.live[&p].active_tasks() == 1).await;
        assert_eq!(trek.run_state(cx, &id), RunState::Working);
        assert!(trek.working_bar(cx).is_some_and(|l| l.starts_with("Waiting on a sub-agent")), "{:?}", trek.working_bar(cx));
        // Its card names it on hover.
        let (kids, background) = cx.read(|cx| {
            let t = trek.ws.read(cx).thread(&id).cloned().unwrap();
            trek.root.read(cx).sidebar.read(cx).at_work(&t, cx)
        });
        assert_eq!(crate::sidebar::card_tip(&kids, &background).as_deref(), Some("Sub-agents at work:\nSurvey the test suite"));
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

#[test]
fn a_codex_turn_in_its_wait_call_says_it_waits_on_its_sub_agent() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:long 600s");
        let t = id.clone();
        trek.wait(cx, "the turn", move |ws| ws.turn_running(&t)).await;
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        // As Codex's driver reports a turn that spawns a sub-agent and then waits for it: the spawn
        // call returns at once (its row runs on while its task does), and `wait` gets no row.
        use trek_agents::AgentEvent;
        let events = vec![
            AgentEvent::TextDelta("Spawning a helper.".into()),
            AgentEvent::ToolStarted { id: "call1".into(), title: "Subagent".into(), detail: "Count the files".into() },
            AgentEvent::Task { id: "call1".into(), description: Some("Count the files".into()), activity: None, tool_uses: None, done: None },
            AgentEvent::ToolFinished { id: "call1".into(), output: String::new(), ok: true },
            AgentEvent::Task { id: "call1".into(), description: None, activity: Some("Running ls".into()), tool_uses: Some(1), done: None },
        ];
        trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
        trek.render(cx);
        assert!(trek.working_bar(cx).is_some_and(|l| l.starts_with("Waiting on a sub-agent")), "{:?}", trek.working_bar(cx));
        // Going on with its own work instead (it didn't wait), it isn't blocked on it.
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::ReasoningDelta("Meanwhile, the tests.".into())], cx));
        trek.render(cx);
        assert!(trek.working_bar(cx).is_some_and(|l| !l.starts_with("Waiting")), "{:?}", trek.working_bar(cx));
        // `wait` reports it done: the turn goes on with its own work.
        let done = vec![
            AgentEvent::Task { id: "call1".into(), description: None, activity: None, tool_uses: None, done: Some(true) },
            AgentEvent::ToolFinished { id: "call1".into(), output: "3 files".into(), ok: true },
        ];
        trek.update(cx, |ws, cx| ws.apply_events(&id, done, cx));
        trek.render(cx);
        assert!(trek.working_bar(cx).is_some_and(|l| !l.starts_with("Waiting")), "{:?}", trek.working_bar(cx));
    });
}

#[test]
fn the_card_tip_names_sub_agents_and_background_work() {
    let mock = trek_core::AgentId::Direct("mock".into());
    assert_eq!(crate::sidebar::card_tip(&[], &[]), None);
    assert_eq!(
        crate::sidebar::card_tip(&[(mock.clone(), "Sol: Review the cache".into())], &["npm run dev".into(), "cargo watch".into()]).as_deref(),
        Some("Sub-agents at work:\nSol: Review the cache\n\nRunning in the background:\nnpm run dev\ncargo watch")
    );
}

#[test]
fn the_strip_can_be_put_away() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        trek.click(cx, "bg-hide");
        trek.render(cx);
        assert_eq!(strip(&trek, cx), ["Background · 1 running"]);
        assert!(trek.visible(cx, "bg-show") && !trek.visible(cx, ("bg-task", 0usize)));
        trek.click(cx, "bg-show");
        trek.render(cx);
        assert!(trek.visible(cx, ("bg-task", 0usize)));
    });
}

#[test]
fn the_strip_lines_up_with_the_wider_column_above_the_composer() {
    run(async |cx| {
        let trek = open(cx);
        cx.simulate_window_resize(trek.window, gpui_kit::size(gpui_kit::px(1900.), gpui_kit::px(1000.)));
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        let column = trek.read(cx, |ws, _| ws.column());
        let row = trek.bounds(cx, ("bg-task", 0usize)).expect("the strip's row");
        let composer = trek.bounds(cx, "send").expect("composer");
        // As wide as the transcript's column, its right edge on the composer's, just above it.
        assert!((row.size.width - column).abs() < gpui_kit::px(1.), "{row:?} vs {column:?}");
        assert!((row.right() - composer.right()).abs() < gpui_kit::px(16.), "{row:?} vs {composer:?}");
        assert!(row.bottom() <= composer.top(), "{row:?} vs {composer:?}");
        // A larger text size widens it in step, as it does the transcript.
        trek.update(cx, |ws, cx| {
            ws.settings.appearance.transcript_font_size = 17.5;
            ws.save_settings(cx);
        });
        trek.render(cx);
        let wider = trek.bounds(cx, ("bg-task", 0usize)).expect("the strip's row");
        assert!((wider.size.width - trek.read(cx, |ws, _| ws.column())).abs() < gpui_kit::px(1.), "{wider:?}");
        assert!(wider.size.width > row.size.width);
    });
}
