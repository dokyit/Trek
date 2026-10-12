//! The main window's composer: drafts kept per thread, delivery keys, the pills it shows.

use super::harness::*;
use crate::workspace::{Route, WorkspaceEvent};

/// What's typed stays with the thread (or new-thread draft) it was typed for: moving to another
/// thread shows that thread's own draft, and coming back brings the text back.
#[test]
fn drafts_stay_with_their_thread() {
    run(async |cx| {
        let trek = open(cx);
        let project = trek.project.clone();
        trek.type_text(cx, "for the new thread");
        let a = trek.quiet_thread(cx);
        assert_eq!(trek.composer_text(cx), "", "a thread doesn't show the new thread's draft");
        trek.type_text(cx, "for A");
        let b = trek.quiet_thread(cx);
        assert_eq!(trek.composer_text(cx), "", "B doesn't show A's draft");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        assert_eq!(trek.composer_text(cx), "for A");
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project) }, cx));
        assert_eq!(trek.composer_text(cx), "for the new thread");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b), cx));
        assert_eq!(trek.composer_text(cx), "");
    });
}

/// A message a rewind puts back lands in its own thread's composer and doesn't follow the user
/// to another thread.
#[test]
fn a_message_put_back_stays_in_its_thread() {
    run(async |cx| {
        let trek = open(cx);
        let a = trek.quiet_thread(cx);
        let b = trek.quiet_thread(cx);
        // Navigating and handing the message over in one go, as a fork does.
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Thread(a.clone()), cx);
            cx.emit(WorkspaceEvent::ComposeIn { scope: crate::workspace::Scope::Main, thread: a.clone(), text: "How does the app start up?".into(), images: vec![], edit: None });
        });
        assert_eq!(trek.composer_text(cx), "How does the app start up?");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b), cx));
        assert_eq!(trek.composer_text(cx), "", "the message stays with A");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a), cx));
        assert_eq!(trek.composer_text(cx), "How does the app start up?");
    });
}

/// While a turn runs, ↩ goes the way Settings › General says (steer, by default) and ⌥↩ the
/// other way: here, queued for after the turn.
#[test]
fn option_return_sends_the_other_way_mid_turn() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:long 30s");
        let tid = id.clone();
        trek.wait(cx, "the turn to start", move |ws| ws.live.get(&tid).is_some_and(|l| l.turn_started.is_some() && l.commands.is_some())).await;
        trek.type_text(cx, "and then this");
        trek.press(cx, "alt-enter");
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1, "queued behind the running turn");
        assert_eq!(trek.composer_text(cx), "");
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
    });
}

/// The send button says which keys do what, mid-turn too.
#[test]
fn the_send_hint_names_the_keys() {
    use crate::composer::delivery_hint;
    use trek_core::settings::FollowUp;
    crate::keys::with_mac(true, || {
        assert_eq!(delivery_hint(false, FollowUp::Steer, false), "Send ↩ · New line ⇧↩");
        assert_eq!(delivery_hint(true, FollowUp::Steer, false), "Send ⌘↩ · New line ↩");
        assert_eq!(delivery_hint(false, FollowUp::Steer, true), "↩ steers the running turn · ⌥↩ queues for after it");
        assert_eq!(delivery_hint(false, FollowUp::Queue, true), "↩ queues for after the turn · ⌥↩ steers it now");
    });
    crate::keys::with_mac(false, || {
        assert_eq!(delivery_hint(false, FollowUp::Steer, false), "Send Enter · New line Shift+Enter");
        assert_eq!(delivery_hint(true, FollowUp::Steer, false), "Send Ctrl+Enter · New line Enter");
        assert_eq!(delivery_hint(false, FollowUp::Steer, true), "Enter steers the running turn · Alt+Enter queues for after it");
        assert_eq!(delivery_hint(false, FollowUp::Queue, true), "Enter queues for after the turn · Alt+Enter steers it now");
    });
}

/// The context ring stays out of the way early in a thread and shows once there's something to
/// watch. The Plan pill is in a thread's composer as on a new thread's.
#[test]
fn the_context_ring_waits_and_plan_is_always_there() {
    run(async |cx| {
        let trek = open(cx);
        assert!(trek.visible(cx, "plan-pill"), "on a new thread");
        let id = trek.quiet_thread(cx);
        trek.render(cx);
        assert!(trek.visible(cx, "plan-pill"), "in a thread under way");
        let set = |trek: &Trek, cx: &mut gpui_kit::TestAppContext, used: u64| {
            let id = id.clone();
            trek.update(cx, move |ws, cx| {
                ws.live.entry(id).or_default().context = Some((used, 200_000));
                cx.notify();
            });
            trek.render(cx);
        };
        set(&trek, cx, 20_000);
        assert!(!trek.visible(cx, "context-ring"), "10% isn't worth a ring");
        set(&trek, cx, 100_000);
        assert!(trek.visible(cx, "context-ring"), "50% is");
    });
}

/// After a toast offering an undo, there's time to reach it.
#[test]
fn undo_toasts_stay_long_enough_to_reach() {
    use crate::root::toast_lifetime;
    use std::time::Duration;
    assert!(toast_lifetime("Settled", true) >= Duration::from_secs(10));
    assert_eq!(toast_lifetime("Settled", false), Duration::from_secs(5));
    let long = "Titles are written with Claude Code, which isn't available. Used the first message instead.";
    assert!(toast_lifetime(long, false) > Duration::from_secs(5), "a long one stays longer");
    assert!(toast_lifetime(&long.repeat(10), false) <= Duration::from_secs(12));
}

/// ⌘E settles the thread on screen and says which, with an undo.
#[test]
fn command_e_settles_with_an_undo() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.rename(&id, "Add a note".into(), cx));
        let seen = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
        let sink = seen.clone();
        cx.update(|cx| {
            cx.subscribe(&trek.ws, move |_, event: &WorkspaceEvent, _| {
                if let WorkspaceEvent::Toast { message, undo } = event {
                    sink.borrow_mut().push((message.clone(), undo.is_some()));
                }
            })
            .detach()
        });
        // Trek in front, so the toast has a window to show in.
        cx.update(|cx| crate::root::init(trek.ws.clone(), cx));
        trek.window(cx, |window, _| window.activate_window());
        cx.run_until_parked();
        trek.press(cx, "secondary-e");
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_some_and(|t| t.settled_at.is_some())));
        assert_eq!(*seen.borrow(), [("Settled “Add a note”".to_string(), true)]);
        // The toast is on screen, with its Undo.
        assert_eq!(trek.window(cx, |window, cx| crate::toast::count(window, cx)), 1);
        std::thread::sleep(std::time::Duration::from_millis(450));
        trek.render(cx);
        trek.click(cx, "undo");
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_some_and(|t| t.settled_at.is_none())), "Undo brings it back");
        // Another one, left alone: it goes once its time is up.
        trek.update(cx, |ws, cx| ws.settle(&id, cx));
        assert!(trek.window(cx, |window, cx| crate::toast::count(window, cx)) >= 1);
        cx.executor().advance_clock(std::time::Duration::from_secs(12));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(300));
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(trek.window(cx, |window, cx| crate::toast::count(window, cx)), 0, "gone after its time");
    });
}

/// A window too narrow for the sidebar, the chat and the tools panel: the sidebar steps aside
/// (at the smallest window the chat and panel then fit side by side); past that the panel floats.
#[test]
fn a_narrow_window_makes_room_for_the_tools_panel() {
    use crate::root::{SIDEBAR_WIDTH, overlay_panel, sidebar_yields};
    assert!(!sidebar_yields(1280., true, false));
    assert!(sidebar_yields(760., true, false));
    assert!(!sidebar_yields(760., false, false), "only for the panel");
    assert!(!sidebar_yields(760., true, true), "settings have no panel");
    assert!(!overlay_panel(760. - 8. - 16.), "the smallest window fits chat and panel once the sidebar steps aside");
    assert!(overlay_panel(760. - SIDEBAR_WIDTH - 16.));
}

/// OpenCode 2's effort levels are each model's own. Once a session says which its model offers,
/// the effort menu shows those, and a thread on a level the model doesn't have moves to the one
/// the session is on (the model's default); a level it has stays.
#[test]
fn a_models_own_efforts_replace_the_shared_ones() {
    use trek_agents::{AcpInfo, AgentEvent};
    use trek_core::catalog::ModelInfo;
    use trek_core::{AgentId, Effort, HandHolding};
    run(async |cx| {
        let trek = open(cx);
        let shared = vec![Effort::Low, Effort::Medium, Effort::High];
        let id = trek.update(cx, |ws, cx| {
            let model = |id: &str| ModelInfo { id: id.into(), name: id.into(), efforts: shared.clone(), tier: 0, fast: None };
            ws.acp_info.insert("opencode".into(), Ok(AcpInfo { models: vec![model("opencode/a"), model("opencode/b")], ..Default::default() }));
            let t = ws.store.create_thread(Some(&trek.project), AgentId::OpenCode, Some("opencode/b".into()), Effort::Low, HandHolding::Auto).unwrap();
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            t.id
        });
        let said = |effort| AgentEvent::Efforts { model: "opencode/b".into(), efforts: vec![Effort::Medium, Effort::High], effort };
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![said(Some(Effort::Medium))], cx));
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().effort), Effort::Medium, "B has no Low: its default");
        let efforts = |cx: &mut gpui_kit::TestAppContext, m: &str| trek.read(cx, |ws, _| ws.models_for(&AgentId::OpenCode).into_iter().find(|i| i.id == m).unwrap().efforts);
        assert_eq!(efforts(cx, "opencode/b"), vec![Effort::Medium, Effort::High]);
        assert_eq!(efforts(cx, "opencode/a"), shared, "another model's are as they were");
        trek.render(cx);
        trek.click(cx, "model-pill");
        trek.render(cx);
        trek.click(cx, "mm-effort");
        trek.render(cx);
        assert!(trek.visible(cx, "eff-medium") && trek.visible(cx, "eff-high") && !trek.visible(cx, "eff-low"));
        // Picked from those, it stays whatever level the session reports next.
        trek.click(cx, "eff-high");
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![said(Some(Effort::Medium))], cx));
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().effort), Effort::High);
    });
}
