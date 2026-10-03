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
            pause.queued.push(trek_core::limit::Queued { text: "and then the docs".into(), images: vec![] });
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
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        assert!(pause(&trek, cx, &id).is_some(), "not at once");
        skip(cx, 10);
        trek.wait_done(cx, &id, RunState::Idle).await;
        let thread = id.clone();
        trek.wait(cx, "the queued message", |ws| ws.live.get(&thread).is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::User { text, .. } if text == "and then the docs")))).await;
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
        // There and back before sending: nothing to show.
        set(&trek, cx, relay.clone(), None);
        set(&trek, cx, mock(), Some("mock-deep"));
        assert!(handoffs(&trek, cx).is_empty());
        set(&trek, cx, relay.clone(), Some("relay-swift"));
        assert_eq!(
            handoffs(&trek, cx),
            [Item::Handoff { from: mock().key(), from_model: Some("mock-deep".into()), to: relay.key(), to_model: Some("relay-swift".into()) }]
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
