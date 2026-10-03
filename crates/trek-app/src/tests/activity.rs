//! Live activity: the running turn's group of tool calls in the working bar (it appears, grows,
//! and folds into a transcript summary row when it ends), and titles that animate in only when
//! they change.

use super::harness::{Trek, open, open_with, run};
use gpui_kit::TestAppContext;
use std::time::{Duration, Instant};
use trek_agents::AgentEvent;
use trek_core::RunState;

fn feed(trek: &Trek, cx: &mut TestAppContext, id: &str, events: Vec<AgentEvent>) {
    trek.update(cx, |ws, cx| ws.apply_events(id, events, cx));
}

fn start(id: &str, title: &str, detail: &str) -> AgentEvent {
    AgentEvent::ToolStarted { id: id.into(), title: title.into(), detail: detail.into() }
}

fn done(id: &str) -> AgentEvent {
    AgentEvent::ToolFinished { id: id.into(), output: "ok".into(), ok: true }
}

/// Motion on (the harness turns it off), with the window in front so the bar animates.
fn with_motion(trek: &Trek, cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(false));
    trek.window(cx, |window, _| window.activate_window());
    cx.run_until_parked();
}

fn item_id(trek: &Trek, cx: &TestAppContext, thread: &str, ix: usize) -> String {
    trek.read(cx, |ws, _| ws.live[thread].items.id_at(ix).map(str::to_string)).expect("item")
}

#[test]
fn the_live_group_appears_grows_and_folds_into_its_summary() {
    run(async |cx| {
        let trek = open(cx);
        with_motion(&trek, cx);
        let id = trek.quiet_thread(cx);
        feed(&trek, cx, &id, vec![AgentEvent::TextDelta("Let me look around.".into())]);
        assert_eq!(trek.live_group(cx), None, "text alone isn't a group");
        assert!(trek.working_bar(cx).is_some_and(|l| l.starts_with("Mock Swift working")), "{:?}", trek.working_bar(cx));

        // The first call: the group appears in the bar, not the transcript.
        let project = trek.project.display().to_string();
        feed(&trek, cx, &id, vec![start("t1", "Read", &format!("{project}/src/main.rs"))]);
        assert_eq!(trek.live_group(cx), Some(vec!["Read 1 file · Exploring the project".into(), "Read src/main.rs (running)".into()]));
        assert!(trek.visible(cx, "live-group"));
        let first = item_id(&trek, cx, &id, 1);
        assert!(trek.visible(cx, format!("live-row-{first}")));
        assert_eq!(trek.rows(cx), ["assistant"]);

        // It grows with each call, says what it's doing, and counts changed lines.
        feed(&trek, cx, &id, vec![done("t1"), start("t2", "Edit", "src/main.rs"), AgentEvent::ToolLines { id: "t2".into(), added: 12, removed: 3 }]);
        feed(&trek, cx, &id, vec![done("t2"), start("t3", "Run command", "cargo test")]);
        assert_eq!(
            trek.live_group(cx),
            Some(vec![
                "Ran 1 command, edited 1 file, and read 1 file · Running tests".into(),
                "Read src/main.rs".into(),
                "Edit src/main.rs +12 −3".into(),
                "cargo test (running)".into(),
            ])
        );
        // Past six calls, the oldest fold into "+N earlier".
        for n in 4..=8 {
            feed(&trek, cx, &id, vec![start(&format!("t{n}"), "Search", &format!("pattern {n}"))]);
        }
        let group = trek.live_group(cx).expect("a live group");
        assert_eq!(group.len(), 1 + 1 + crate::working_bar::ROWS, "{group:?}");
        assert_eq!(group[1], "+2 earlier");
        assert_eq!(trek.rows(cx), ["assistant"], "still only in the bar");

        // Text ends the group: it folds away in the bar while the transcript holds it back…
        feed(&trek, cx, &id, vec![AgentEvent::TextDelta("Found it.".into())]);
        assert_eq!(trek.live_group(cx), None);
        assert_eq!(trek.folding(cx).as_deref(), Some("Ran 1 command, edited 1 file, read 1 file, and ran 5 searches"));
        assert!(trek.visible(cx, "live-fold"));
        assert_eq!(trek.rows(cx), ["assistant"]);
        // …then it's a summary row there, and what followed shows below it.
        let tid = id.clone();
        trek.wait(cx, "the fold to finish", |ws| ws.live[&tid].fold.is_none()).await;
        trek.render(cx);
        assert_eq!(trek.rows(cx), ["assistant", "group: Ran 1 command, edited 1 file, read 1 file, and ran 5 searches (running)", "assistant"]);
        assert_eq!(trek.folding(cx), None);
        assert!(!trek.visible(cx, "live-fold"));

        // The turn's end takes the bar away; the group stays a summary row you can open.
        feed(&trek, cx, &id, vec![AgentEvent::TurnComplete { cost_usd: None, error: None }]);
        assert_eq!(trek.working_bar(cx), None);
        assert_eq!(trek.rows(cx), ["assistant", "group: Ran 1 command, edited 1 file, read 1 file, and ran 5 searches", "assistant", "end"]);
        trek.click(cx, ("tool-group", 1usize));
        assert_eq!(trek.rows(cx)[2..4], ["  tool".to_string(), "  tool".to_string()]);
    });
}

#[test]
fn without_motion_a_finished_group_goes_straight_to_the_transcript() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        feed(&trek, cx, &id, vec![start("t1", "Run command", "ls -la")]);
        assert_eq!(trek.live_group(cx), Some(vec!["Ran 1 command · Exploring the project".into(), "ls -la (running)".into()]));
        assert!(trek.rows(cx).is_empty());
        feed(&trek, cx, &id, vec![done("t1"), AgentEvent::TextDelta("Two files.".into())]);
        assert_eq!(trek.folding(cx), None);
        assert_eq!(trek.rows(cx), ["group: Ran 1 command", "assistant"]);
    });
}

#[test]
fn a_card_shows_the_group_in_the_transcript_until_it_is_answered() {
    run(async |cx| {
        let trek = open(cx);
        with_motion(&trek, cx);
        let id = trek.quiet_thread(cx);
        feed(&trek, cx, &id, vec![start("t1", "Run command", "make deploy")]);
        assert_eq!(trek.rows(cx), Vec::<String>::new());
        let ask = AgentEvent::PermissionRequest { request_id: "ask".into(), title: "Run command".into(), detail: "make deploy".into(), prompt: None };
        feed(&trek, cx, &id, vec![ask]);
        // The card takes the bar's place: the group is in the transcript, and nothing folds.
        assert_eq!(trek.working_bar(cx), None);
        assert_eq!(trek.folding(cx), None);
        assert_eq!(trek.rows(cx), ["group: Ran 1 command (running)"]);
        // The agent moves on (an answer through the card works the same way).
        feed(&trek, cx, &id, vec![AgentEvent::PermissionResolved { request_id: "ask".into() }]);
        assert_eq!(trek.run_state(cx, &id), RunState::Working);
        assert!(trek.live_group(cx).is_some(), "back in the bar");
        assert!(trek.rows(cx).is_empty());
    });
}

#[test]
fn the_mock_explores_live_for_every_window() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:explore 1500ms");
        let tid = id.clone();
        trek.wait(cx, "a few calls", |ws| ws.live[&tid].items.iter().filter(|i| matches!(i, trek_core::store::Item::Tool { .. })).count() >= 3).await;
        let own = trek.open_thread_window(cx, &id);
        assert!(trek.visible_in(cx, own, "live-group"));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(trek.live_group(cx), None);
        assert!(!trek.visible_in(cx, own, "live-group"));
        let rows = trek.rows(cx);
        assert!(rows.iter().filter(|r| r.starts_with("group: ")).count() >= 2, "{rows:?}");
        assert_eq!(rows.last().map(String::as_str), Some("end"));
    });
}

#[test]
fn titles_animate_in_only_when_they_change() {
    run(async |cx| {
        let trek = open(cx);
        with_motion(&trek, cx);
        let id = trek.quiet_thread(cx);
        let card = format!("card-title-{id}");
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.title_reveal(&id).is_none()), "a thread opening isn't a change");
        assert!(!trek.visible(cx, card.clone()));

        trek.update(cx, |ws, cx| ws.rename(&id, "Animate the title".into(), cx));
        assert_eq!(trek.read(cx, |ws, _| ws.title_reveal(&id).map(|(_, old)| old.to_string())), Some("New thread".to_string()));
        assert!(trek.visible(cx, card.clone()), "the sidebar card animates it");
        assert!(trek.visible(cx, "window-title"), "and the title bar");

        // Once it's in, redraws leave it be, and the same title again changes nothing.
        trek.update(cx, |ws, _| ws.retitled.get_mut(&id).expect("retitled").1 = Instant::now() - Duration::from_secs(2));
        trek.render(cx);
        assert!(!trek.visible(cx, card.clone()));
        trek.update(cx, |ws, cx| ws.rename(&id, "Animate the title".into(), cx));
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.title_reveal(&id).is_none()));
        assert!(!trek.visible(cx, card.clone()));

        // A thread window's title animates too.
        let own = trek.open_thread_window(cx, &id);
        trek.update(cx, |ws, cx| ws.rename(&id, "Animate titles everywhere".into(), cx));
        assert!(trek.visible_in(cx, own, "window-title"));
        assert_eq!(trek.read(cx, |ws, _| ws.title_reveal(&id).map(|(_, old)| old.to_string())), Some("Animate the title".into()));
    });
}

#[test]
fn reduced_motion_titles_change_at_once() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.appearance.reduce_motion = true);
        cx.update(|cx| cx.set_reduce_motion(false));
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.rename(&id, "Straight in".into(), cx));
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.title_reveal(&id).is_none()));
        assert!(!trek.visible(cx, format!("card-title-{id}")));
    });
}
