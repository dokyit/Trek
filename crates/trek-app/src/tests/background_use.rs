//! Background computer use, watched: `trek-mcp computer` tells Trek each step it takes behind the
//! user's windows (`trek_ipc::COMPUTER_ACTIVITY`), and Trek shows it in a picture-in-picture
//! window the user can put away and bring back, and in the thread's working bar.

use super::harness::{Trek, open, open_with, run};
use crate::ipc::{Call, Reply};
use gpui_kit::TestAppContext;
use serde_json::{Value, json};
use std::time::Duration;
use trek_core::RunState;

/// A step as `trek-mcp computer` reports it, in Safari's 800x600 window.
fn step(action: &str, detail: &str, point: Option<(f64, f64)>) -> Value {
    json!({
        "action": action,
        "detail": detail,
        "window": {"id": 4242, "pid": 42, "app": "Safari", "title": "Docs", "x": 0.0, "y": 0.0, "width": 800.0, "height": 600.0},
        "point": point.map(|(x, y)| json!({"x": x, "y": y})),
        "from": null,
    })
}

/// Report `params` from `thread`'s session, as the computer server does; Trek's answer.
fn report(trek: &Trek, cx: &mut TestAppContext, thread: &str, params: Value) -> Reply {
    let (reply, answer) = async_channel::unbounded();
    let call = Call { thread: thread.to_string(), method: trek_ipc::COMPUTER_ACTIVITY.into(), params, reply };
    trek.update(cx, |ws, cx| ws.handle_call(call, cx));
    cx.run_until_parked();
    answer.try_recv().expect("an answer at once")
}

/// (sessions, the thread and app on show, where the pointer is, the picture showing).
fn watched(trek: &Trek, cx: &TestAppContext) -> (usize, Option<(String, String, Option<(f64, f64)>)>, bool) {
    trek.read(cx, |ws, cx| {
        let b = ws.background_use.read(cx);
        (b.sessions.len(), b.current().map(|s| (s.thread.clone(), s.window.app.clone(), s.pointer)), b.showing())
    })
}

async fn working(trek: &Trek, cx: &mut TestAppContext) -> String {
    let id = trek.send(cx, "mock:long 30s");
    let t = id.clone();
    trek.wait(cx, "the turn to start", move |ws| ws.thread(&t).is_some_and(|t| t.run_state == RunState::Working)).await;
    id
}

#[test]
fn each_step_is_shown_until_the_turn_ends() {
    run(async |cx| {
        let trek = open(cx);
        let id = working(&trek, cx).await;
        assert_eq!(watched(&trek, cx), (0, None, false), "nothing until the computer is used");
        assert_eq!(report(&trek, cx, &id, step("click", "Clicking", Some((20., 30.)))), Reply::Done(Ok(json!({}))));
        assert_eq!(watched(&trek, cx), (1, Some((id.clone(), "Safari".into(), Some((20., 30.)))), true), "the picture opens");
        // Typing points nowhere: the pointer stays where it was left.
        report(&trek, cx, &id, step("type", "Typing “hi”", None));
        assert_eq!(watched(&trek, cx).1.unwrap().2, Some((20., 30.)));
        trek.read(cx, |ws, cx| assert_eq!(ws.background_use.read(cx).current().unwrap().step.detail, "Typing “hi”"));
        // The working bar says where, with a button to put the picture away and bring it back.
        trek.render(cx);
        assert!(trek.visible(cx, "background-use-chip"));
        trek.click(cx, "background-use-toggle");
        assert!(!watched(&trek, cx).2, "put away");
        // Put away, it stays away as the agent goes on.
        report(&trek, cx, &id, step("scroll", "Scrolling down", Some((400., 300.))));
        assert!(!watched(&trek, cx).2);
        trek.render(cx);
        trek.click(cx, "background-use-toggle");
        assert!(watched(&trek, cx).2, "brought back");
        // A report that isn't one (no window) changes nothing.
        report(&trek, cx, &id, json!({"action": "click"}));
        assert_eq!(watched(&trek, cx).0, 1);
        // The turn ends: a moment later the session ends and the picture closes.
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while watched(&trek, cx) != (0, None, false) {
            assert!(std::time::Instant::now() < deadline, "the session should end with its turn: {:?}", watched(&trek, cx));
            cx.background_executor.timer(Duration::from_millis(20)).await;
            cx.run_until_parked();
        }
        trek.render(cx);
        assert!(!trek.visible(cx, "background-use-chip"));
    });
}

#[test]
fn the_picture_can_be_turned_off_and_reports_from_gone_threads_are_refused() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.tools.background_pip = false);
        let id = working(&trek, cx).await;
        report(&trek, cx, &id, step("click", "Clicking", Some((1., 2.))));
        let (n, _, showing) = watched(&trek, cx);
        assert_eq!((n, showing), (1, false), "followed, but not shown");
        trek.render(cx);
        assert!(trek.visible(cx, "background-use-chip"), "the working bar still says where");
        // Shown on request all the same.
        trek.read(cx, |ws, _| ws.background_use.clone()).update(cx, |b, cx| b.show(cx));
        cx.run_until_parked();
        assert!(watched(&trek, cx).2);
        // The agent's next step doesn't take it away again.
        report(&trek, cx, &id, step("scroll", "Scrolling down", Some((400., 300.))));
        assert!(watched(&trek, cx).2, "still shown after another step");
        // Put away, it goes, and the steps after that don't bring it back.
        trek.read(cx, |ws, _| ws.background_use.clone()).update(cx, |b, cx| b.tuck(cx));
        cx.run_until_parked();
        report(&trek, cx, &id, step("click", "Clicking", Some((3., 4.))));
        assert!(!watched(&trek, cx).2, "put away");
        trek.read(cx, |ws, _| ws.background_use.clone()).update(cx, |b, cx| b.show(cx));
        cx.run_until_parked();
        assert!(matches!(report(&trek, cx, "no-such-thread", step("click", "Clicking", None)), Reply::Done(Err(_))));
        // A session's key for reporting doesn't bring the sub-agent tools when they're off.
        trek.update(cx, |ws, _| ws.settings.tools.orchestration = false);
        let (reply, answer) = async_channel::unbounded();
        trek.update(cx, |ws, cx| ws.handle_call(Call { thread: id.clone(), method: "list_models".into(), params: json!({}), reply }, cx));
        assert!(matches!(answer.try_recv(), Ok(Reply::Done(Err(e))) if e.contains("Sub-agent tools are off")));
        assert_eq!(report(&trek, cx, &id, step("key", "Pressing return", None)), Reply::Done(Ok(json!({}))), "reports still come in");
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

#[test]
fn the_chip_fits_a_narrow_thread_pane() {
    run(async |cx| {
        let trek = open(cx);
        let id = working(&trek, cx).await;
        report(&trek, cx, &id, step("click", "Clicking", Some((20., 30.))));
        // The sidebar and a tools panel leave the thread a few hundred points.
        cx.simulate_window_resize(trek.window, gpui_kit::size(gpui_kit::px(800.), gpui_kit::px(700.)));
        trek.render(cx);
        let (bar, chip) = (trek.bounds(cx, "working-bar").expect("the bar"), trek.bounds(cx, "background-use-chip").expect("the chip"));
        assert!(chip.right() <= bar.right(), "the chip ends inside the bar: {chip:?} in {bar:?}");
        assert!(trek.visible(cx, "background-use-toggle"), "and its button is on screen");
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

#[test]
fn once_computer_use_is_on_it_works_in_the_background_watched() {
    let tools = trek_core::settings::Tools::default();
    assert!(!tools.computer_use, "computer use itself stays off until the user turns it on");
    assert!(tools.computer_background && tools.background_pip, "once on, it works in the background, watched");
}

#[test]
fn the_tools_page_switches_background_use_and_its_picture() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.navigate(crate::workspace::Route::Settings(crate::workspace::SettingsPage::Tools), cx));
        // Tall enough for the computer use rows without scrolling.
        cx.simulate_window_resize(trek.window, gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(2400.)));
        trek.render(cx);
        if crate::words::is_windows() {
            assert!(!trek.visible(cx, "computer-background"), "Windows' computer use works in front");
            return;
        }
        assert!(trek.visible(cx, "computer-background") && trek.visible(cx, "background-pip"));
        trek.click(cx, "background-pip");
        trek.render(cx);
        assert!(!trek.read(cx, |ws, _| ws.settings.tools.background_pip));
        // Working in front, there's no picture to show.
        trek.click(cx, "computer-background");
        trek.render(cx);
        assert!(!trek.read(cx, |ws, _| ws.settings.tools.computer_background));
        assert!(!trek.visible(cx, "background-pip"));
    });
}
