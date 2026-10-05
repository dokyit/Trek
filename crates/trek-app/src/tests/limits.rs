//! Usage limits: a thread its agent's limit stopped pauses with one transcript row and a bar
//! above the composer; it resumes at the reset (with what was typed meanwhile, in order),
//! snoozes until it, or moves to another agent with a handoff divider. The mock's `mock:limit`
//! plays the limit; the workspace's clock follows the test clock, so the reset can be skipped to.

use super::harness::{Trek, launch, mock, new_project, open, open_with, run, settings};
use crate::workspace::{Clock, Prefs, Route, Scope, WorkspaceEvent};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, ElementId, SharedString, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use trek_core::limit::{CONTINUE, LimitScope, Pause, RESUME_GRACE_MS};
use trek_core::settings::OnUsageLimit;
use trek_core::store::{Item, Section, Store, now_ms};
use trek_core::{AgentId, Effort, HandHolding, RunState};

/// Run Trek's usage-limit clock on the test clock: wall time moves as the executor's does.
fn test_clock(trek: &Trek, cx: &mut TestAppContext) {
    let exec = cx.executor();
    let (start, base) = (exec.now(), now_ms());
    trek.update(cx, move |ws, _| ws.clock = Clock::new(move || base + (exec.now() - start).as_millis() as i64));
}

/// Move the clock on by `secs`, a few seconds at a time, letting timers fire on the way.
fn skip(cx: &mut TestAppContext, secs: u64) {
    for _ in 0..secs.div_ceil(5) {
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
    }
}

fn pause(trek: &Trek, cx: &TestAppContext, id: &str) -> Option<Pause> {
    trek.read(cx, |ws, _| ws.pause(id).cloned())
}

/// Send `text` from the composer and wait until the limit has paused the thread.
async fn hit_limit(trek: &Trek, cx: &mut TestAppContext, text: &str) -> String {
    let id = trek.send(cx, text);
    let thread = id.clone();
    trek.wait(cx, "the limit to pause the thread", |ws| ws.pause(&thread).is_some() && !ws.turn_running(&thread)).await;
    id
}

/// The thread's messages, in order.
fn said(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<String> {
    trek.items(cx, id).into_iter().filter_map(|i| if let Item::User { text, .. } = i { Some(text) } else { None }).collect()
}

/// Lift the mock's limit on `id`'s session, as if it had reset.
fn lift(trek: &Trek, cx: &TestAppContext, id: &str) {
    let session = trek.read(cx, |ws, _| ws.thread(id).and_then(|t| t.native_id.clone())).expect("a session");
    trek_agents::mock::lift_limit(&session);
}

fn events(trek: &Trek, cx: &mut TestAppContext) -> Rc<RefCell<Vec<String>>> {
    let seen = Rc::new(RefCell::new(vec![]));
    let sink = seen.clone();
    cx.update(|cx| {
        cx.subscribe(&trek.ws, move |_, event: &WorkspaceEvent, _| match event {
            WorkspaceEvent::Attention { message, .. } | WorkspaceEvent::Toast { message, .. } => sink.borrow_mut().push(message.clone()),
            _ => {}
        })
        .detach()
    });
    seen
}

fn paused_status(id: &str) -> ElementId {
    ElementId::Name(SharedString::from(format!("paused-{id}")))
}

#[test]
fn a_limit_pauses_the_thread_with_one_row_and_a_bar() {
    run(async |cx| {
        let trek = open(cx);
        let seen = events(&trek, cx);
        let id = hit_limit(&trek, cx, "mock:limit 5s").await;
        let items = trek.items(cx, &id);
        // One row says it: no error, and the agent's limit message isn't an answer.
        assert_eq!(items.iter().filter(|i| matches!(i, Item::Limit { .. })).count(), 1, "{items:?}");
        assert!(!items.iter().any(|i| matches!(i, Item::Error { .. } | Item::Assistant { .. })), "{items:?}");
        assert!(matches!(items.last(), Some(Item::Limit { resets_at: Some(_), scope: LimitScope::Session, .. })));
        assert!(trek.rows(cx).iter().any(|r| r == "limit"));
        // Paused, not failed: nothing for the user to fix, no turn to keep the Mac awake for.
        assert_eq!(trek.run_state(cx, &id), RunState::Idle);
        assert!(trek.read(cx, |ws, _| ws.needs_you_count() == 0 && !ws.any_turn_running()));
        let p = pause(&trek, cx, &id).expect("paused");
        assert!(!p.resume, "Ask is the default");
        assert!(seen.borrow().iter().any(|m| m.starts_with("Usage limit reached, resets ")), "{:?}", seen.borrow());
        trek.render(cx);
        for el in ["limit-bar", "limit-resume", "limit-snooze", "limit-switch"] {
            assert!(trek.visible(cx, el), "{el}");
        }
        assert!(!trek.visible(cx, "limit-cancel"));
        assert!(trek.visible(cx, paused_status(&id)), "the sidebar card says it's paused");
        // Kept with the thread.
        assert_eq!(trek.read(cx, |ws, _| ws.store.thread(&id).unwrap().unwrap().paused), Some(p));
        // "Switch agent…" opens the model picker.
        trek.click(cx, "limit-switch");
        trek.render(cx);
        assert!(trek.visible(cx, "model-menu-body"));
        trek.press(cx, "escape");

        // The same bar in the thread's own window.
        let window = trek.open_thread_window(cx, &id);
        cx.update_window(window, |_, window, cx| window.render_frame(cx)).ok();
        assert!(trek.visible_in(cx, window, "limit-bar"));
    });
}

#[test]
fn resume_at_reset_sends_what_was_typed_meanwhile_in_order() {
    run(async |cx| {
        let trek = open(cx);
        test_clock(&trek, cx);
        let seen = events(&trek, cx);
        let id = hit_limit(&trek, cx, "mock:limit 5s").await;
        trek.click(cx, "limit-resume");
        assert!(pause(&trek, cx, &id).is_some_and(|p| p.resume));
        trek.render(cx);
        assert!(trek.visible(cx, "limit-cancel") && !trek.visible(cx, "limit-resume"));

        // Typed while paused: queued for the reset, not sent.
        trek.send(cx, "first follow-up");
        trek.send(cx, "second follow-up");
        let queued: Vec<String> = pause(&trek, cx, &id).unwrap().queued.into_iter().map(|q| q.text).collect();
        assert_eq!(queued, ["first follow-up", "second follow-up"]);
        assert_eq!(said(&trek, cx, &id), ["mock:limit 5s"]);
        trek.render(cx);
        assert!(trek.visible(cx, ("limit-queued", 0usize)) && trek.visible(cx, ("limit-queued", 1usize)));

        lift(&trek, cx, &id);
        let resets = pause(&trek, cx, &id).unwrap().resets_at.unwrap();
        // Not before the reset and the minute after it.
        let now = trek.read(cx, |ws, _| ws.now());
        skip(cx, ((resets - now) / 1000) as u64 + 30);
        assert!(pause(&trek, cx, &id).is_some(), "still waiting for the grace minute");
        skip(cx, (RESUME_GRACE_MS / 1000) as u64);
        let thread = id.clone();
        trek.wait(cx, "both queued messages to go", |ws| ws.live.get(&thread).is_some_and(|l| l.queued.is_empty() && l.turn_started.is_none()) && ws.pause(&thread).is_none()).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(said(&trek, cx, &id), ["mock:limit 5s", "first follow-up", "second follow-up"]);
        assert!(seen.borrow().iter().any(|m| m.starts_with("Resumed: ")), "{:?}", seen.borrow());
        trek.render(cx);
        assert!(!trek.visible(cx, "limit-bar"));
        assert_eq!(trek.read(cx, |ws, _| ws.store.thread(&id).unwrap().unwrap().paused), None);
    });
}

#[test]
fn resuming_automatically_continues_and_cancel_hands_messages_back() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.on_usage_limit = OnUsageLimit::Resume);
        test_clock(&trek, cx);
        let id = hit_limit(&trek, cx, "mock:limit 5s").await;
        assert!(pause(&trek, cx, &id).is_some_and(|p| p.resume && p.queued.is_empty()));
        trek.render(cx);
        assert!(trek.visible(cx, "limit-cancel") && !trek.visible(cx, "limit-resume"));
        // Cancel: back to asking, and what was queued comes back to the composer.
        trek.send(cx, "then run the tests");
        trek.click(cx, "limit-cancel");
        assert!(pause(&trek, cx, &id).is_some_and(|p| !p.resume && p.queued.is_empty()));
        assert_eq!(trek.composer_text(cx), "then run the tests");
        trek.render(cx);
        assert!(trek.visible(cx, "limit-resume"));

        // Nothing scheduled: at the reset the pause simply ends, and nothing is sent.
        let resets = pause(&trek, cx, &id).unwrap().resets_at.unwrap();
        let now = trek.read(cx, |ws, _| ws.now());
        skip(cx, ((resets - now) / 1000) as u64 + 10);
        assert!(pause(&trek, cx, &id).is_none());
        assert_eq!(said(&trek, cx, &id), ["mock:limit 5s"]);

        // Again, resuming on its own this time: "continue" goes, with no message of the user's.
        // (The mock still reports its first reset, gone by this clock: Trek tries again in a while.)
        trek.update(cx, |ws, cx| ws.send_to(&id, "mock:limit 5s".into(), vec![], cx));
        let thread = id.clone();
        trek.wait(cx, "the limit again", |ws| ws.pause(&thread).is_some() && !ws.turn_running(&thread)).await;
        let p = pause(&trek, cx, &id).unwrap();
        assert!(p.resume && p.resets_at.is_some_and(|at| at > trek.read(cx, |ws, _| ws.now())), "{p:?}");
        lift(&trek, cx, &id);
        skip(cx, (trek_core::limit::LATE_RESET_RETRY_MS + RESUME_GRACE_MS) as u64 / 1000 + 10);
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(said(&trek, cx, &id).last().map(String::as_str), Some(CONTINUE));
        assert!(pause(&trek, cx, &id).is_none());
    });
}

#[test]
fn a_limit_still_there_at_the_reset_moves_the_resume_on() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.on_usage_limit = OnUsageLimit::Resume);
        test_clock(&trek, cx);
        let id = hit_limit(&trek, cx, "mock:limit 5s").await;
        let first = pause(&trek, cx, &id).unwrap().resets_at.unwrap();
        // The mock's limit doesn't lift: the resume meets it again.
        skip(cx, 75);
        let thread = id.clone();
        trek.wait(cx, "the resume to meet the limit", |ws| ws.live.get(&thread).is_some_and(|l| l.items.iter().filter(|i| matches!(i, Item::Limit { .. })).count() == 2) && !ws.turn_running(&thread) && ws.pause(&thread).is_some())
            .await;
        let p = pause(&trek, cx, &id).unwrap();
        let now = trek.read(cx, |ws, _| ws.now());
        assert!(p.resume, "it still resumes");
        assert!(p.resets_at.is_some_and(|at| at > now && at > first), "{p:?} at {now}");
        assert_eq!(said(&trek, cx, &id), ["mock:limit 5s", CONTINUE]);
        assert!(!trek.items(cx, &id).iter().any(|i| matches!(i, Item::Error { .. })));
    });
}

#[test]
fn snooze_until_reset_brings_the_thread_back_then() {
    run(async |cx| {
        let trek = open(cx);
        let seen = events(&trek, cx);
        let id = hit_limit(&trek, cx, "mock:limit 2m").await;
        let resets = pause(&trek, cx, &id).unwrap().resets_at.unwrap();
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        trek.click(cx, "limit-snooze");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.snoozed_until)), Some(resets));
        let section = |at: i64| trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.section(at)));
        assert_eq!(section(now_ms()), Some(Section::Snoozed));
        assert_eq!(section(resets + 1), Some(Section::Inbox));
        assert!(seen.borrow().iter().any(|m| m.starts_with("Snoozed until ")), "{:?}", seen.borrow());
        trek.render(cx);
        assert!(!trek.visible(cx, "limit-snooze"), "snoozed already");
    });
}

#[test]
fn messages_queued_for_the_reset_can_be_taken_back() {
    run(async |cx| {
        let trek = open(cx);
        let id = hit_limit(&trek, cx, "mock:limit 2m").await;
        trek.send(cx, "one");
        trek.send(cx, "two");
        trek.render(cx);
        trek.click(cx, ("limit-unqueue", 0usize));
        let p = pause(&trek, cx, &id).unwrap();
        assert_eq!(p.queued.iter().map(|q| q.text.as_str()).collect::<Vec<_>>(), ["two"]);
        assert!(p.resume, "the rest still go at the reset");
        assert_eq!(trek.composer_text(cx), "one");
    });
}

#[test]
fn overdue_resumes_go_after_a_relaunch() {
    run(async |cx| {
        let dir = new_project("relaunch");
        let db = dir.join("trek.sqlite");
        let project = new_project("project");
        let id = {
            let store = Store::open(&db).expect("store");
            let mut t = store.create_thread(Some(&project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            t.title = "Refactor the parser".into();
            store.save_thread(&t).unwrap();
            let user = Item::User { text: "mock:limit 5s".into(), images: vec![], at: Some(1), resume: None, aside: false };
            let limit = Item::Limit { text: "You've hit your session limit · resets 2:10am".into(), resets_at: Some(now_ms() - 120_000), scope: LimitScope::Session };
            super::harness::store_items(&store, &t.id, vec![user, limit]);
            // Trek quit while the thread waited for its reset, which has passed since.
            let mut pause = Pause::new("You've hit your session limit · resets 2:10am".into(), Some(now_ms() - 120_000), LimitScope::Session, 0, true);
            pause.queued.push(trek_core::limit::Queued { text: "recall".into(), images: vec![] });
            store.update_thread(&t.id, |t| t.paused = Some(pause)).unwrap();
            t.id
        };
        let mut s = settings();
        s.user_projects.push(project.display().to_string());
        let (ws, root, window) = launch(cx, Store::open(&db).expect("store"), s);
        let trek = Trek { ws, root, window, project };
        let seen = events(&trek, cx);
        cx.run_until_parked();
        assert!(seen.borrow().iter().any(|m| m.contains("resumes in a moment")), "{:?}", seen.borrow());
        assert!(pause(&trek, cx, &id).is_some(), "not at once");
        // The thread isn't opened: the resume reads its history itself.
        skip(cx, 10);
        let thread = id.clone();
        trek.wait(cx, "the queued message", |ws| ws.live.get(&thread).is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::User { text, .. } if text == "recall")))).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(said(&trek, cx, &id), ["mock:limit 5s", "recall"]);
        // The new session started from a recap of that history.
        let answer = trek.answers(cx, &id);
        assert!(answer.lines().last().is_some_and(|l| l.contains("mock:limit 5s")), "{answer}");
        assert!(pause(&trek, cx, &id).is_none());
        assert_eq!(trek.read(cx, |ws, _| ws.store.thread(&id).unwrap().unwrap().paused), None);
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn another_agent_takes_over_with_a_handoff_and_a_recap() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "remember APPLE");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let relay = AgentId::Direct(trek_core::catalog::MOCK_RELAY_PROVIDER.into());
        let set = |trek: &Trek, cx: &mut TestAppContext, agent: AgentId, model: Option<&str>| {
            let model = model.map(String::from);
            trek.update(cx, |ws, cx| {
                let prefs = Prefs { agent, model, ..ws.prefs_in(&Scope::Main) };
                ws.set_prefs_in(&Scope::Main, prefs, cx)
            });
        };
        let handoffs = |trek: &Trek, cx: &TestAppContext| trek.items(cx, &id).into_iter().filter(|i| matches!(i, Item::Handoff { .. })).collect::<Vec<_>>();
        // Another model of the same agent is no handoff.
        set(&trek, cx, mock(), Some("mock-deep"));
        assert!(handoffs(&trek, cx).is_empty());
        // There and back before sending: nothing to show. The default model is written down.
        set(&trek, cx, relay.clone(), None);
        assert!(matches!(handoffs(&trek, cx).as_slice(), [Item::Handoff { to_model: Some(m), .. }] if m == "relay-swift"));
        set(&trek, cx, mock(), Some("mock-deep"));
        assert!(handoffs(&trek, cx).is_empty());
        set(&trek, cx, relay.clone(), Some("relay-swift"));
        assert_eq!(
            handoffs(&trek, cx),
            [Item::Handoff {
                from: mock().key(),
                from_model: Some("mock-deep".into()),
                to: relay.key(),
                to_model: Some("relay-swift".into()),
                from_name: Some("Mock Deep".into()),
                to_name: Some("Relay Swift".into())
            }]
        );
        assert!(trek.rows(cx).iter().any(|r| r == "handoff"));
        let ix = trek.item_ix(cx, &id, |i| matches!(i, Item::Handoff { .. }));
        trek.render(cx);
        assert!(trek.visible(cx, ("handoff-row", ix)));
        // Saved with the transcript.
        assert!(trek.read(cx, |ws, _| ws.store.items(&id).unwrap().iter().any(|i| matches!(i, Item::Handoff { .. }))));
        // The new agent knows the conversation: it got the recap with its first message.
        trek.update(cx, |ws, cx| ws.send_to(&id, "recall".into(), vec![], cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let answer = trek.answers(cx, &id);
        assert!(answer.lines().last().is_some_and(|l| l.contains("remember APPLE")), "{answer}");
    });
}

#[test]
fn switching_agents_ends_a_pause_and_hands_its_messages_back() {
    run(async |cx| {
        let trek = open(cx);
        let id = hit_limit(&trek, cx, "mock:limit 2m").await;
        trek.send(cx, "keep going");
        let relay = AgentId::Direct(trek_core::catalog::MOCK_RELAY_PROVIDER.into());
        trek.update(cx, |ws, cx| {
            let prefs = Prefs { agent: relay, model: None, ..ws.prefs_in(&Scope::Main) };
            ws.set_prefs_in(&Scope::Main, prefs, cx)
        });
        assert!(pause(&trek, cx, &id).is_none());
        assert_eq!(trek.composer_text(cx), "keep going");
        assert!(matches!(trek.items(cx, &id).last(), Some(Item::Handoff { .. })));
        trek.render(cx);
        assert!(!trek.visible(cx, "limit-bar"));
    });
}

/// The handoff with real agents: Claude Code (claude-haiku-4-5) hears a word, then the thread
/// moves to Codex (gpt-5.6-luna), which must know it from the recap alone. Not run by default (two
/// tiny turns): `TREK_LIVE_AGENT=handoff cargo test -p trek-app live_handoff -- --ignored`. Works
/// in /tmp/trek-limits-e2e.
#[test]
#[ignore = "live: runs real agents"]
fn live_handoff_carries_the_conversation() {
    run(async |cx| {
        let trek = open_with(cx, |s| {
            s.general.default_agent = AgentId::ClaudeCode.key();
            s.general.default_model = Some("claude-haiku-4-5".into());
            s.general.default_effort = Effort::Low;
        });
        let project = std::path::PathBuf::from("/tmp/trek-limits-e2e");
        std::fs::create_dir_all(&project).unwrap();
        trek.update(cx, |ws, cx| {
            ws.store.ensure_project(&project).unwrap();
            ws.reload(cx);
            ws.navigate(Route::Draft { project: Some(project.clone()) }, cx);
        });
        let done = async |cx: &mut TestAppContext, id: &str| {
            let deadline = std::time::Instant::now() + Duration::from_secs(240);
            loop {
                cx.run_until_parked();
                let state = trek.read(cx, |ws, _| (ws.thread(id).map(|t| t.run_state), ws.turn_running(id)));
                match state {
                    (Some(RunState::Idle), false) => return,
                    (Some(RunState::Failed), _) => panic!("the turn failed: {:?}", trek.items(cx, id)),
                    _ => {}
                }
                assert!(std::time::Instant::now() < deadline, "timed out");
                cx.background_executor.timer(Duration::from_millis(50)).await;
            }
        };
        trek.update(cx, |ws, cx| ws.send("Just for this conversation (don't save it to memory or to any file), remember the word APPLE. Reply with just OK.".into(), vec![], cx));
        let id = trek.thread_id(cx);
        done(cx, &id).await;
        trek.update(cx, |ws, cx| {
            let prefs = Prefs { agent: AgentId::Codex, model: Some("gpt-5.6-luna".into()), effort: Effort::Low, ..ws.prefs_in(&Scope::Main) };
            ws.set_prefs_in(&Scope::Main, prefs, cx)
        });
        assert!(matches!(trek.items(cx, &id).last(), Some(Item::Handoff { .. })));
        trek.update(cx, |ws, cx| ws.send_to(&id, "Which word did I ask you to remember? Reply with just the word.".into(), vec![], cx));
        done(cx, &id).await;
        let last = trek.answers(cx, &id).to_uppercase().lines().last().unwrap_or_default().to_string();
        assert!(last.contains("APPLE"), "Codex knows the conversation from the recap: {last}");
        println!("live handoff: ok ({last})");
    });
}

#[test]
fn a_resume_that_meets_the_limit_again_keeps_its_messages_and_waits_longer() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.on_usage_limit = OnUsageLimit::Resume);
        test_clock(&trek, cx);
        let seen = events(&trek, cx);
        let id = hit_limit(&trek, cx, "mock:limit 5s").await;
        trek.send(cx, "first follow-up");
        trek.send(cx, "second follow-up");
        // The mock's limit doesn't lift: the resume sends the first message into it.
        skip(cx, 75);
        let thread = id.clone();
        let limits = |ws: &crate::workspace::Workspace, n: usize| ws.live.get(&thread).is_some_and(|l| l.items.iter().filter(|i| matches!(i, Item::Limit { .. })).count() == n);
        trek.wait(cx, "the resume to meet the limit", |ws| limits(ws, 2) && !ws.turn_running(&thread) && ws.pause(&thread).is_some()).await;
        let p = pause(&trek, cx, &id).unwrap();
        // Nothing the user wrote is lost: the message it sent goes again, first.
        assert_eq!(p.queued.iter().map(|q| q.text.as_str()).collect::<Vec<_>>(), ["first follow-up", "second follow-up"]);
        assert_eq!((p.resume, p.tries), (true, 1));
        // The mock's reset is long gone by this clock: tried again in five minutes, then ten.
        let now = trek.read(cx, |ws, _| ws.now());
        assert!(p.resets_at.is_some_and(|at| (at - now - trek_core::limit::late_retry(1)).abs() < 10_000), "{p:?} at {now}");
        let alerts = |seen: &Rc<RefCell<Vec<String>>>, prefix: &str| seen.borrow().iter().filter(|m| m.starts_with(prefix)).count();
        // One alert for the limit and one for the resume; the second meeting is quiet.
        assert_eq!((alerts(&seen, "Paused until "), alerts(&seen, "Usage limit reached"), alerts(&seen, "Resumed: ")), (1, 0, 1), "{:?}", seen.borrow());

        let wait = ((p.resets_at.unwrap() - now) / 1000) as u64 + RESUME_GRACE_MS as u64 / 1000 + 10;
        skip(cx, wait);
        let thread = id.clone();
        trek.wait(cx, "the next try to meet the limit", |ws| limits(ws, 3) && !ws.turn_running(&thread) && ws.pause(&thread).is_some()).await;
        let p = pause(&trek, cx, &id).unwrap();
        assert_eq!((p.tries, p.queued.len()), (2, 2));
        assert_eq!(alerts(&seen, "Resumed: "), 1, "no news: {:?}", seen.borrow());

        // Then it lifts: both messages go, in order.
        lift(&trek, cx, &id);
        let now = trek.read(cx, |ws, _| ws.now());
        skip(cx, ((p.resets_at.unwrap() - now) / 1000) as u64 + RESUME_GRACE_MS as u64 / 1000 + 10);
        let thread = id.clone();
        trek.wait(cx, "both messages to go", |ws| ws.pause(&thread).is_none() && ws.live.get(&thread).is_some_and(|l| l.queued.is_empty() && l.turn_started.is_none())).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        let all = said(&trek, cx, &id);
        assert_eq!(all[all.len() - 2..], ["first follow-up", "second follow-up"], "{all:?}");
    });
}

/// A usage report for the probe to answer with: `(label, window, percent, resets in ms from now)`.
fn usage(now: i64, windows: &[(&str, &str, f32, i64)]) -> Option<trek_agents::AgentStatus> {
    let limits = windows.iter().map(|(label, window, percent, after)| trek_agents::UsageLimit { label: (*label).into(), percent: *percent, resets_at: Some(now + after), window: (*window).into() }).collect();
    Some(trek_agents::AgentStatus { limits, ..Default::default() })
}

#[test]
fn the_usage_check_before_a_resume_waits_quietly_and_moves_the_resume_on() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.on_usage_limit = OnUsageLimit::Resume);
        test_clock(&trek, cx);
        // Asked about its usage, the agent answers when the test says.
        type Answer = async_channel::Sender<Option<trek_agents::AgentStatus>>;
        let asked: Rc<RefCell<Vec<Answer>>> = Rc::default();
        let calls = asked.clone();
        trek.update(cx, |ws, _| {
            ws.usage_probe = Some(Rc::new(move |_, _| {
                let (tx, rx) = async_channel::bounded(1);
                calls.borrow_mut().push(tx);
                rx
            }))
        });
        let id = hit_limit(&trek, cx, "mock:limit 5s").await;
        skip(cx, 75);
        assert_eq!(asked.borrow().len(), 1, "asked once at the reset");
        // While it answers, no timer spins: nothing else is paused.
        assert!(trek.read(cx, |ws, _| ws.limit_timer.is_none()));
        skip(cx, 60);
        assert_eq!(asked.borrow().len(), 1);
        assert_eq!(said(&trek, cx, &id), ["mock:limit 5s"]);

        // The session window is still used up for an hour; Fable's week is no matter to this model.
        let now = trek.read(cx, |ws, _| ws.now());
        let answer = asked.borrow()[0].clone();
        answer.try_send(usage(now, &[("5-hour limit", "5h", 100., 3_600_000), ("Weekly · Fable", "7d", 100., 3 * 86_400_000)])).unwrap();
        cx.run_until_parked();
        let p = pause(&trek, cx, &id).expect("still paused");
        assert_eq!(p.resets_at, Some(now + 3_600_000));
        assert!(p.resume);
        assert!(trek.read(cx, |ws, _| ws.limit_timer.is_some()), "waiting for the new reset");

        // At that reset it's clear: "continue" goes.
        lift(&trek, cx, &id);
        skip(cx, 3_600 + RESUME_GRACE_MS as u64 / 1000 + 10);
        assert_eq!(asked.borrow().len(), 2);
        let now = trek.read(cx, |ws, _| ws.now());
        let answer = asked.borrow()[1].clone();
        answer.try_send(usage(now, &[("5-hour limit", "5h", 3., 5 * 3_600_000), ("Weekly · Fable", "7d", 100., 2 * 86_400_000)])).unwrap();
        cx.run_until_parked();
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(said(&trek, cx, &id), ["mock:limit 5s", CONTINUE]);
        assert!(pause(&trek, cx, &id).is_none());
    });
}

#[test]
fn retrying_or_editing_a_paused_turn_sends_it_now() {
    run(async |cx| {
        let trek = open(cx);
        let id = hit_limit(&trek, cx, "mock:limit 2m").await;
        trek.send(cx, "waiting for the reset");
        let item_id = |trek: &Trek, cx: &TestAppContext, f: fn(&Item) -> bool| {
            let ix = trek.item_ix(cx, &id, f);
            trek.read(cx, |ws, _| ws.live[&id].items.id_at(ix).map(str::to_string)).expect("an item id")
        };
        // Retry with another model: sent at once, not queued for the reset.
        let end = item_id(&trek, cx, |i| matches!(i, Item::Limit { .. }));
        let before = pause(&trek, cx, &id).unwrap();
        trek.update(cx, |ws, cx| ws.retry(&id, &end, Some("mock-deep".into()), false, cx));
        assert_eq!(trek.composer_text(cx), "waiting for the reset", "what waited comes back to the composer");
        let thread = id.clone();
        trek.wait(cx, "the retried turn", |ws| ws.pause(&thread).is_some() && !ws.turn_running(&thread)).await;
        // It went to the agent (and ran into the limit, still there), rather than into the queue.
        assert_eq!(said(&trek, cx, &id), ["mock:limit 2m"]);
        assert_eq!(trek.items(cx, &id).iter().filter(|i| matches!(i, Item::Limit { .. })).count(), 1);
        let after = pause(&trek, cx, &id).unwrap();
        assert!(after.queued.is_empty() && after.since >= before.since && !after.resume, "{after:?}");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.model.clone())).as_deref(), Some("mock-deep"));

        // Edit and send again, once the limit has lifted: it goes, and the thread carries on.
        lift(&trek, cx, &id);
        let user = item_id(&trek, cx, |i| matches!(i, Item::User { .. }));
        assert!(trek.update(cx, |ws, cx| ws.edit_and_resend(&id, &user, "explain the startup".into(), vec![], false, cx)));
        assert!(pause(&trek, cx, &id).is_none());
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(said(&trek, cx, &id), ["explain the startup"]);
        assert!(!trek.items(cx, &id).iter().any(|i| matches!(i, Item::Limit { .. })));
    });
}

#[test]
fn without_a_reset_time_the_next_message_tries_again() {
    use trek_agents::AgentEvent;
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let user = Item::User { text: "explain the startup".into(), images: vec![], at: Some(1), resume: None, aside: false };
        trek.update(cx, |ws, cx| {
            ws.live.get_mut(&id).unwrap().items.push(user);
            let limit = AgentEvent::LimitReached { message: "API Error: 429 rate_limit_error".into(), resets_at: None, scope: LimitScope::Other };
            ws.apply_events(&id, vec![limit, AgentEvent::TurnComplete { error: Some("API Error: 429 rate_limit_error".into()) }], cx)
        });
        assert!(pause(&trek, cx, &id).is_some_and(|p| p.resets_at.is_none()));
        trek.render(cx);
        assert!(trek.visible(cx, "limit-retry") && !trek.visible(cx, "limit-resume"));
        // Nothing would ever send it at a reset nobody knows: it goes now.
        trek.send(cx, "try once more");
        assert!(pause(&trek, cx, &id).is_none());
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(said(&trek, cx, &id), ["explain the startup", "try once more"]);
    });
}

#[test]
fn a_side_chat_shows_its_limit_and_carries_on() {
    run(async |cx| {
        let trek = open(cx);
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.window(cx, |window, cx| panel.update(cx, |p, cx| p.open_tool(crate::workspace::PanelTool::SideChat, window, cx)));
        trek.click(cx, "side-input");
        trek.type_live(cx, "mock:limit 2m");
        trek.press_live(cx, "enter");
        let side = |ws: &crate::workspace::Workspace| ws.threads.iter().find(|t| t.side_of.is_some()).map(|t| t.id.clone());
        trek.wait(cx, "the side chat's limit", |ws| side(ws).is_some_and(|id| !ws.turn_running(&id) && ws.live.get(&id).is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::Limit { .. }))))).await;
        let id = trek.read(cx, |ws, _| side(ws)).unwrap();
        // No pause and no bar to wait behind: the panel shows the limit, once.
        assert!(pause(&trek, cx, &id).is_none());
        let items = trek.items(cx, &id);
        assert!(!items.iter().any(|i| matches!(i, Item::Error { .. })), "{items:?}");
        let ix = trek.item_ix(cx, &id, |i| matches!(i, Item::Limit { .. }));
        trek.render(cx);
        assert!(trek.visible(cx, ("side-limit", ix)));
        // The next message goes to the agent (which meets its limit again), not to a queue.
        trek.update(cx, |ws, cx| ws.send_to(&id, "explain the startup".into(), vec![], cx));
        let thread = id.clone();
        trek.wait(cx, "the second message", |ws| !ws.turn_running(&thread) && ws.live.get(&thread).is_some_and(|l| l.items.iter().filter(|i| matches!(i, Item::Limit { .. })).count() == 2)).await;
        assert_eq!(said(&trek, cx, &id), ["mock:limit 2m", "explain the startup"]);
        assert!(pause(&trek, cx, &id).is_none());
    });
}

/// The sub-agent row of `child` in `parent`'s transcript: (status, output).
fn task_row(trek: &Trek, cx: &TestAppContext, parent: &str, child: &str) -> (trek_core::store::ToolStatus, String) {
    let row = trek_core::orchestrate::task_row(child);
    trek.items(cx, parent)
        .into_iter()
        .find_map(|i| match i {
            Item::Tool { id, status, output, .. } if id == row => Some((status, output)),
            _ => None,
        })
        .expect("a row for the sub-agent")
}

fn wakes(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<String> {
    said(trek, cx, id).into_iter().filter(|t| trek_core::orchestrate::is_wake(t)).collect()
}

#[test]
fn a_sub_agent_at_its_limit_reports_it_and_takes_its_task_back_up_at_the_reset() {
    use crate::workspace::TaskState;
    run(async |cx| {
        let trek = open(cx);
        test_clock(&trek, cx);
        let seen = events(&trek, cx);
        let id = trek.quiet_thread(cx);
        let child = trek.update(cx, |ws, cx| ws.delegate(&id, &serde_json::json!({ "title": "Refactor", "prompt": "mock:limit 5s", "mode": "implement" }), cx)).unwrap();
        let c = child.clone();
        trek.wait(cx, "the sub-agent to stop at its limit", |ws| ws.task_state(&c) == TaskState::Failed).await;
        let (status, why) = task_row(&trek, cx, &id, &child);
        assert_eq!(status, trek_core::store::ToolStatus::Failed);
        assert!(why.starts_with("It hit its 5-hour limit, which resets "), "{why}");
        assert!(pause(&trek, cx, &child).is_some(), "paused, to resume at the reset");
        // Its parent hears it (woken: it wasn't waiting); the user isn't alerted for a sub-agent.
        let p = id.clone();
        trek.wait(cx, "the parent to hear it", |ws| !ws.turn_running(&p) && ws.live[&p].items.iter().any(|i| matches!(i, Item::User { text, .. } if text.contains("failed: It hit its 5-hour limit")))).await;
        assert!(!seen.borrow().iter().any(|m| m.contains("Refactor")), "{:?}", seen.borrow());
        // The agent takes no more work until then.
        let again = trek.update(cx, |ws, cx| ws.delegate(&id, &serde_json::json!({ "title": "More", "prompt": "explain" }), cx));
        assert!(again.unwrap_err().contains("used up its 5-hour limit (it resets "));

        // Resumed at the reset (from its own thread), it picks the task back up and reports.
        trek.update(cx, |ws, cx| ws.resume_at_reset(&child, cx));
        lift(&trek, cx, &child);
        let resets = pause(&trek, cx, &child).unwrap().resets_at.unwrap();
        let now = trek.read(cx, |ws, _| ws.now());
        let waited = ((resets - now) / 1000) as u64 + (RESUME_GRACE_MS / 1000) as u64 + 10;
        skip(cx, waited);
        trek.wait(cx, "the sub-agent to finish its task", |ws| ws.task_state(&c) == TaskState::Done).await;
        // Its time is the time it worked: the wait for the reset isn't part of it.
        let took = trek.read(cx, |ws, _| ws.task_elapsed(&child));
        assert!(took < Duration::from_secs(waited / 2), "{took:?}");
        assert!(task_row(&trek, cx, &id, &child).1.starts_with("## How the app starts"));
        trek.wait(cx, "the parent to hear the answer", |ws| ws.live[&p].items.iter().any(|i| matches!(i, Item::User { text, .. } if text.contains("finished:")))).await;
        assert_eq!(wakes(&trek, cx, &id).len(), 2, "once at the limit, once with its answer");
    });
}

#[test]
fn a_parent_paused_at_its_limit_hears_its_sub_agents_once_the_limit_lifts() {
    use crate::workspace::TaskState;
    run(async |cx| {
        let trek = open(cx);
        test_clock(&trek, cx);
        let id = hit_limit(&trek, cx, "mock:limit 5s").await;
        // Its own agent is at its limit: the sub-agent runs on another.
        let mine = trek.update(cx, |ws, cx| ws.delegate(&id, &serde_json::json!({ "title": "Same", "prompt": "explain" }), cx));
        assert!(mine.unwrap_err().contains("used up its 5-hour limit"));
        let child = trek.update(cx, |ws, cx| ws.delegate(&id, &serde_json::json!({ "title": "Review", "prompt": "explain", "agent": "direct:mock-relay" }), cx)).unwrap();
        let c = child.clone();
        trek.wait(cx, "the sub-agent to finish", |ws| ws.task_state(&c) == TaskState::Done).await;
        cx.run_until_parked();
        assert!(wakes(&trek, cx, &id).is_empty(), "held while paused: it would only meet the limit");
        lift(&trek, cx, &id);
        let resets = pause(&trek, cx, &id).unwrap().resets_at.unwrap();
        let now = trek.read(cx, |ws, _| ws.now());
        skip(cx, ((resets - now) / 1000) as u64 + 10);
        assert!(pause(&trek, cx, &id).is_none(), "the limit lifted");
        let p = id.clone();
        trek.wait(cx, "the wake-up", |ws| !ws.turn_running(&p) && ws.live[&p].items.iter().any(|i| matches!(i, Item::User { text, .. } if trek_core::orchestrate::is_wake(text)))).await;
        assert!(trek.answers(cx, &id).contains("The sub-agent reported back"), "{}", trek.answers(cx, &id));
    });
}

#[test]
fn a_parent_paused_at_its_limit_hears_its_sub_agents_when_it_moves_to_another_agent() {
    use crate::workspace::TaskState;
    run(async |cx| {
        let trek = open(cx);
        let id = hit_limit(&trek, cx, "mock:limit 2m").await;
        let child = trek.update(cx, |ws, cx| ws.delegate(&id, &serde_json::json!({ "title": "Review", "prompt": "explain", "agent": "direct:mock-relay" }), cx)).unwrap();
        let c = child.clone();
        trek.wait(cx, "the sub-agent to finish", |ws| ws.task_state(&c) == TaskState::Done).await;
        cx.run_until_parked();
        assert!(wakes(&trek, cx, &id).is_empty(), "held while paused");
        // Another agent takes over: the pause ends, and the report goes to it at once.
        let relay = AgentId::Direct(trek_core::catalog::MOCK_RELAY_PROVIDER.into());
        trek.update(cx, |ws, cx| {
            let prefs = Prefs { agent: relay, model: None, ..ws.prefs_in(&Scope::Main) };
            ws.set_prefs_in(&Scope::Main, prefs, cx)
        });
        assert!(pause(&trek, cx, &id).is_none());
        let p = id.clone();
        trek.wait(cx, "the wake-up", |ws| !ws.turn_running(&p) && ws.live[&p].items.iter().any(|i| matches!(i, Item::User { text, .. } if trek_core::orchestrate::is_wake(text)))).await;
        assert_eq!(wakes(&trek, cx, &id).len(), 1);
    });
}

#[test]
fn a_sub_agent_paused_when_trek_quit_is_not_resumed_on_its_own() {
    run(async |cx| {
        let dir = new_project("relaunch");
        let db = dir.join("trek.sqlite");
        let project = new_project("project");
        let reset = now_ms() - 120_000;
        let (parent, child) = {
            let store = Store::open(&db).expect("store");
            let p = store.create_thread(Some(&project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            let mut c = store.create_thread(Some(&project), mock(), None, Effort::Medium, HandHolding::Supervised).expect("thread");
            c.parent_id = Some(p.id.clone());
            c.title = "Second opinion".into();
            store.save_thread(&c).unwrap();
            // Its parent heard it failed at its limit; Trek quit before the reset, with the
            // sub-agent set to resume then.
            let why = "It hit its 5-hour limit, which resets 2:10 AM. Ask another model, or let it resume from its thread at the reset.";
            let row = Item::Tool { id: trek_core::orchestrate::task_row(&c.id), title: "Sub-agent".into(), detail: c.title.clone(), output: why.into(), status: trek_core::store::ToolStatus::Failed };
            super::harness::store_items(&store, &p.id, vec![Item::User { text: "go".into(), images: vec![], at: Some(1), resume: None, aside: false }, row]);
            let asked = Item::User { text: "mock:limit 5s".into(), images: vec![], at: Some(1), resume: None, aside: false };
            let limit = Item::Limit { text: "You've hit your session limit".into(), resets_at: Some(reset), scope: LimitScope::Session };
            super::harness::store_items(&store, &c.id, vec![asked, limit]);
            let pause = Pause::new("You've hit your session limit".into(), Some(reset), LimitScope::Session, 0, true);
            store.update_thread(&c.id, |t| t.paused = Some(pause)).unwrap();
            (p.id, c.id)
        };
        let mut s = settings();
        s.general.on_usage_limit = OnUsageLimit::Resume;
        s.user_projects.push(project.display().to_string());
        let (ws, root, window) = launch(cx, Store::open(&db).expect("store"), s);
        let trek = Trek { ws, root, window, project };
        let seen = events(&trek, cx);
        cx.run_until_parked();
        assert!(!seen.borrow().iter().any(|m| m.contains("resume")), "{:?}", seen.borrow());
        skip(cx, 15);
        // The limit has gone, and the pause with it; nothing was sent for a task nobody waits on.
        assert!(pause(&trek, cx, &child).is_none());
        assert_eq!(trek.read(cx, |ws, _| ws.store.thread(&child).unwrap().unwrap().paused), None);
        let sent = trek.read(cx, |ws, _| ws.store.items(&child).unwrap());
        assert!(!sent.iter().any(|i| matches!(i, Item::User { text, .. } if text == CONTINUE)), "{sent:?}");
        assert!(!trek.read(cx, |ws, _| ws.turn_running(&child)));
        assert_eq!(trek.read(cx, |ws, _| ws.store.items(&parent).unwrap().len()), 2, "the parent isn't woken");
        let _ = std::fs::remove_dir_all(dir);
    });
}

fn wrap_up_notes(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<String> {
    trek.items(cx, id).into_iter().filter_map(|i| if let Item::Notice { text } = i { Some(text) } else { None }).filter(|t| t.contains("asked the agent to wrap up")).collect()
}

fn answers(trek: &Trek, cx: &TestAppContext, id: &str) -> String {
    trek.items(cx, id).into_iter().filter_map(|i| if let Item::Assistant { text } = i { Some(text) } else { None }).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_turn_near_its_limit_wraps_up_and_the_thread_waits_for_the_reset() {
    run(async |cx| {
        let trek = open(cx);
        test_clock(&trek, cx);
        let seen = events(&trek, cx);
        // The mock says its 5-hour window is 96% used partway through the turn.
        let id = trek.send(cx, "mock:nearlimit 10m");
        let thread = id.clone();
        trek.wait(cx, "the wrapped-up turn to pause the thread", |ws| ws.pause(&thread).is_some() && !ws.turn_running(&thread)).await;
        // Trek told the agent, once, and a note says so; it's no message of the user's.
        let notes = wrap_up_notes(&trek, cx, &id);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with("5-hour limit 96% used (resets "), "{notes:?}");
        assert_eq!(said(&trek, cx, &id), ["mock:nearlimit 10m"]);
        let session = trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.native_id.clone())).expect("a session");
        let told = trek_agents::mock::remembered(&session);
        assert_eq!(told.iter().filter(|m| m.starts_with(trek_core::limit::WRAP_UP_TAG)).count(), 1, "{told:?}");
        assert!(told.iter().any(|m| m.contains("Your 5-hour limit is 96% used") && m.contains("start nothing new")), "{told:?}");
        // The agent stopped by itself and said where: an answer, no limit row and no error.
        let items = trek.items(cx, &id);
        assert!(answers(&trek, cx, &id).contains("Stopping here, ahead of the limit"), "{items:?}");
        assert!(!items.iter().any(|i| matches!(i, Item::Limit { .. } | Item::Error { .. })), "{items:?}");
        assert!(matches!(items.last(), Some(Item::TurnEnd { .. })), "{items:?}");
        // The thread waits for the reset like one the limit stopped: paused, idle, and asking.
        let p = pause(&trek, cx, &id).unwrap();
        assert!(p.wrapped && !p.resume, "{p:?}");
        assert_eq!((p.scope.clone(), p.message.as_str()), (LimitScope::Session, "Wrapped up before the 5-hour limit (96% used)"));
        assert!(p.resets_at.is_some_and(|at| at > trek.read(cx, |ws, _| ws.now())));
        assert_eq!(trek.run_state(cx, &id), RunState::Idle);
        // The finished turn is the news; the pause adds no alert of its own.
        assert!(!seen.borrow().iter().any(|m| m.contains("Usage limit reached")), "{:?}", seen.borrow());
        trek.render(cx);
        assert!(trek.visible(cx, "limit-resume") && trek.visible(cx, "limit-snooze") && trek.visible(cx, "limit-switch"));
        // Resumed at the reset, it carries on from where it stopped.
        trek.update(cx, |ws, cx| ws.resume_at_reset(&id, cx));
        skip(cx, 600 + RESUME_GRACE_MS as u64 / 1000 + 10);
        trek.wait(cx, "the resume", |ws| ws.pause(&thread).is_none()).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(said(&trek, cx, &id), ["mock:nearlimit 10m", CONTINUE]);
        assert_eq!(wrap_up_notes(&trek, cx, &id).len(), 1);
    });
}

#[test]
fn messages_queued_behind_a_wrapped_up_turn_wait_for_the_reset() {
    run(async |cx| {
        let trek = open_with(cx, |s| {
            s.general.follow_up = trek_core::settings::FollowUp::Queue;
            s.general.on_usage_limit = OnUsageLimit::Resume;
        });
        test_clock(&trek, cx);
        let id = trek.send(cx, "mock:nearlimit 10m");
        let thread = id.clone();
        trek.wait(cx, "the turn to start", |ws| ws.turn_running(&thread)).await;
        trek.send(cx, "then update the docs");
        trek.wait(cx, "the wrapped-up turn to pause the thread", |ws| ws.pause(&thread).is_some() && !ws.turn_running(&thread)).await;
        // Sent now, the follow-up would meet the limit: it waits, and the resume is on by itself.
        let p = pause(&trek, cx, &id).unwrap();
        assert!(p.wrapped && p.resume, "{p:?}");
        assert_eq!(p.queued.iter().map(|q| q.text.as_str()).collect::<Vec<_>>(), ["then update the docs"]);
        assert_eq!(said(&trek, cx, &id), ["mock:nearlimit 10m"]);
        skip(cx, 600 + RESUME_GRACE_MS as u64 / 1000 + 10);
        trek.wait(cx, "the resume", |ws| ws.pause(&thread).is_none()).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(said(&trek, cx, &id), ["mock:nearlimit 10m", "then update the docs"]);
    });
}

#[test]
fn with_wrap_ups_off_a_turn_near_its_limit_runs_on() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.wrap_up_near_limit = false);
        let id = trek.send(cx, "mock:nearlimit 10m");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(pause(&trek, cx, &id).is_none());
        assert!(wrap_up_notes(&trek, cx, &id).is_empty());
        assert!(answers(&trek, cx, &id).contains("The parser refactor is finished"));
    });
}

#[test]
fn usage_read_while_a_turn_runs_asks_it_to_wrap_up_once() {
    run(async |cx| {
        let trek = open(cx);
        test_clock(&trek, cx);
        let id = trek.send(cx, "mock:long 2s");
        let thread = id.clone();
        trek.wait(cx, "the turn to start", |ws| ws.turn_running(&thread) && ws.live.get(&thread).is_some_and(|l| l.commands.is_some())).await;
        let now = trek.read(cx, |ws, _| ws.now());
        let read = |trek: &Trek, cx: &mut TestAppContext, windows: &[(&str, &str, f32, i64)]| {
            let status = usage(now, windows).unwrap();
            trek.update(cx, |ws, cx| {
                ws.agent_status.insert(mock().key(), status);
                ws.wrap_up_where_due(&mock(), cx);
            })
        };
        // Room left in every window that applies: nothing is said. Another model's doesn't apply.
        read(&trek, cx, &[("5-hour limit", "5h", 94.0, 3_600_000), ("Weekly limit", "7d", 97.0, 86_400_000), ("Weekly · Fable", "7d", 99.0, 86_400_000)]);
        assert!(wrap_up_notes(&trek, cx, &id).is_empty());
        // Two windows close to their limits: the thread will wait for the later reset. Read
        // again, the turn isn't told twice.
        let close = [("5-hour limit", "5h", 97.0, 3_600_000), ("Weekly limit", "7d", 98.5, 86_400_000)];
        read(&trek, cx, &close);
        read(&trek, cx, &close);
        let notes = wrap_up_notes(&trek, cx, &id);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with("Weekly limit 98% used"), "{notes:?}");
        trek.wait(cx, "the turn to end and the thread to pause", |ws| ws.pause(&thread).is_some() && !ws.turn_running(&thread)).await;
        let p = pause(&trek, cx, &id).unwrap();
        assert_eq!((p.wrapped, p.scope.clone(), p.resets_at), (true, LimitScope::Weekly, Some(now + 86_400_000)));
        // The next turn starts afresh: near the limit still, it's asked again.
        trek.update(cx, |ws, cx| ws.end_pause(&id, true, cx));
        trek.wait(cx, "the next turn to start", |ws| ws.turn_running(&thread) && ws.live.get(&thread).is_some_and(|l| l.commands.is_some())).await;
        read(&trek, cx, &close);
        assert_eq!(wrap_up_notes(&trek, cx, &id).len(), 2);
    });
}

#[test]
fn a_stopped_or_failed_turn_asked_to_wrap_up_does_not_pause() {
    run(async |cx| {
        let trek = open(cx);
        test_clock(&trek, cx);
        let id = trek.send(cx, "mock:long 30s");
        let thread = id.clone();
        trek.wait(cx, "the turn to start", |ws| ws.turn_running(&thread) && ws.live.get(&thread).is_some_and(|l| l.commands.is_some())).await;
        let now = trek.read(cx, |ws, _| ws.now());
        trek.update(cx, |ws, cx| {
            ws.agent_status.insert(mock().key(), usage(now, &[("5-hour limit", "5h", 96.0, 3_600_000)]).unwrap());
            ws.wrap_up_where_due(&mock(), cx);
        });
        assert_eq!(wrap_up_notes(&trek, cx, &id).len(), 1);
        // The user stops it: they've taken over, and nothing waits for a reset.
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(pause(&trek, cx, &id).is_none());
    });
}
