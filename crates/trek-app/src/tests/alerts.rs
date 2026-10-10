//! What Trek tells the system: alerts (toasts, banners, sounds) and where clicking them leads,
//! the Dock badge, and keeping the Mac awake while agents work. Banners go to the test platform,
//! sounds and the badge to `system`'s test hooks: nothing reaches the real system.

use super::harness::{Trek, open_with, run};
use crate::system::{DOCK_BADGE, SOUNDS};
use crate::workspace::{Route, WorkspaceEvent};
use gpui_kit::{SystemNotificationResponse, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;
use trek_agents::{AgentEvent, Decision};
use trek_core::settings::NotifyMode;
use trek_core::RunState;

/// A Trek window with the app-wide alert handling the app installs at launch.
fn alerting(cx: &mut TestAppContext, mode: NotifyMode) -> Trek {
    let trek = open_with(cx, |s| s.notifications.mode = mode);
    cx.update(|cx| {
        // Banners are only recorded once the app has an identity, as on a real system.
        cx.set_app_identity("dev.trek.Trek", "Trek");
        crate::root::init(trek.ws.clone(), cx);
    });
    SOUNDS.with(|n| n.set(0));
    trek
}

/// Attention messages the workspace raises from now on.
fn attention(trek: &Trek, cx: &mut TestAppContext) -> Rc<RefCell<Vec<String>>> {
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

fn titled(trek: &Trek, cx: &mut TestAppContext, title: &str) -> String {
    let id = trek.quiet_thread(cx);
    trek.update(cx, |ws, cx| ws.rename(&id, title.into(), cx));
    id
}

fn finish(trek: &Trek, cx: &mut TestAppContext, id: &str, error: Option<&str>) {
    let events = vec![AgentEvent::TextDelta("Done.".into()), AgentEvent::TurnComplete { error: error.map(String::from) }];
    trek.update(cx, |ws, cx| ws.apply_events(id, events, cx));
}

fn ask(trek: &Trek, cx: &mut TestAppContext, id: &str) {
    let ask = AgentEvent::PermissionRequest { request_id: format!("{id}-ask"), title: "Run command".into(), detail: "make deploy".into(), prompt: None };
    trek.update(cx, |ws, cx| ws.apply_events(id, vec![AgentEvent::TextDelta("Deploying.".into()), ask], cx));
}

fn banners(cx: &TestAppContext) -> Vec<(String, String)> {
    cx.shown_system_notifications().into_iter().map(|n| (n.tag.to_string(), n.title.to_string())).collect()
}

fn toasts(trek: &Trek, cx: &mut TestAppContext) -> usize {
    trek.window(cx, |window, cx| crate::toast::count(window, cx))
}

/// Let the toast's entrance (real time, 400 ms) finish.
fn settled_in(trek: &Trek, cx: &mut TestAppContext) {
    std::thread::sleep(std::time::Duration::from_millis(450));
    trek.render(cx);
}

fn sounds() -> usize {
    SOUNDS.with(|n| n.get())
}

fn focus(trek: &Trek, cx: &mut TestAppContext) {
    trek.window(cx, |window, _| window.activate_window());
    cx.run_until_parked();
}

/// Another app comes to the front.
fn blur(trek: &Trek, cx: &mut TestAppContext) {
    VisualTestContext::from_window(trek.window, cx).deactivate_window();
}

#[test]
fn alerts_follow_focus_and_what_is_on_screen() {
    run(async |cx| {
        let trek = alerting(cx, NotifyMode::BannerAndSound);
        let said = attention(&trek, cx);
        let away = titled(&trek, cx, "Fix the login bug");
        let here = titled(&trek, cx, "Write the docs");
        focus(&trek, cx);

        // Looking at it in the focused window: nothing at all.
        finish(&trek, cx, &here, None);
        assert_eq!(*said.borrow(), ["Finished: Write the docs"]);
        assert_eq!((toasts(&trek, cx), banners(cx).len(), sounds()), (0, 0, 0));

        // Another thread while Trek is in front: a toast only (banners wait for the background).
        finish(&trek, cx, &away, None);
        assert_eq!(said.borrow().last().map(String::as_str), Some("Finished: Fix the login bug"));
        assert_eq!((toasts(&trek, cx), banners(cx).len(), sounds()), (1, 0, 0));
        // Clicking the toast opens that thread (once it has slid into place: mid-slide, the
        // press and the release land on different frames and make no click).
        settled_in(&trek, cx);
        trek.click(cx, "notification");
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(away.clone()));

        // In the background: a banner and the sound, even for the thread on screen.
        blur(&trek, cx);
        ask(&trek, cx, &away);
        assert_eq!(said.borrow().last().map(String::as_str), Some("Needs your approval: Run command"));
        assert_eq!(banners(cx), [(format!("trek-attention-{away}"), "Needs your approval: Run command".to_string())]);
        assert_eq!(sounds(), 1);

        // A failed turn says so; a turn the user stopped says nothing.
        finish(&trek, cx, &here, Some("The agent crashed"));
        assert_eq!(said.borrow().last().map(String::as_str), Some("Failed: Write the docs"));
        assert_eq!(banners(cx).last(), Some(&(format!("trek-attention-{here}"), "Failed: Write the docs".to_string())));
        let before = said.borrow().len();
        trek.update(cx, |ws, cx| ws.apply_events(&away, vec![AgentEvent::TurnComplete { error: Some("Interrupted".into()) }], cx));
        assert_eq!(said.borrow().len(), before, "no alert for a stop");

        // Clicking a banner brings Trek forward on its thread.
        cx.simulate_system_notification_response(SystemNotificationResponse { tag: format!("trek-attention-{here}").into(), action_id: None });
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(here.clone()));
        assert_eq!(cx.update(|cx| cx.active_window()), trek.read(cx, |ws, _| ws.main_window), "the main window came forward");
        assert_eq!(cx.update(|cx| cx.windows().len()), 1, "no second main window");
        // Tags Trek didn't post are left alone.
        cx.simulate_system_notification_response(SystemNotificationResponse { tag: "something-else".into(), action_id: None });
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(here.clone()));
    });
}

#[test]
fn banners_open_threads_in_their_own_window_or_a_reopened_main_window() {
    run(async |cx| {
        let trek = alerting(cx, NotifyMode::Banner);
        let popped = titled(&trek, cx, "Popped out");
        let other = titled(&trek, cx, "Stays in main");
        let own = trek.open_thread_window(cx, &popped);
        blur(&trek, cx);
        VisualTestContext::from_window(own, cx).deactivate_window();
        finish(&trek, cx, &popped, None);
        assert_eq!(sounds(), 0, "banner only");
        cx.simulate_system_notification_response(SystemNotificationResponse { tag: format!("trek-attention-{popped}").into(), action_id: None });
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| cx.active_window()), Some(own), "its own window comes forward");
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(other.clone()), "the main window is left as it was");

        // With the main window closed, a banner still goes out, and clicking it reopens the
        // main window on the thread.
        trek.window(cx, |window, _| window.remove_window());
        cx.run_until_parked();
        VisualTestContext::from_window(own, cx).deactivate_window();
        finish(&trek, cx, &other, Some("Out of credits"));
        assert_eq!(banners(cx).last().map(|(_, t)| t.as_str()), Some("Failed: Stays in main"));
        cx.simulate_system_notification_response(SystemNotificationResponse { tag: format!("trek-attention-{other}").into(), action_id: None });
        cx.run_until_parked();
        let main = trek.read(cx, |ws, _| ws.main_window).expect("the main window is back");
        assert_ne!(main, trek.window, "a new main window");
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(other.clone()));
    });
}

#[test]
fn alerts_off_still_toast_threads_off_screen() {
    run(async |cx| {
        let trek = alerting(cx, NotifyMode::Off);
        let away = titled(&trek, cx, "Background job");
        titled(&trek, cx, "On screen");
        blur(&trek, cx);
        finish(&trek, cx, &away, None);
        assert_eq!((toasts(&trek, cx), banners(cx).len(), sounds()), (1, 0, 0));
    });
}

fn badge() -> usize {
    DOCK_BADGE.with(|b| b.get())
}

#[test]
fn the_dock_badge_counts_threads_waiting_on_the_user() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.notifications.dock_badge = true);
        DOCK_BADGE.with(|b| b.set(0));
        cx.update(|cx| crate::system::init(trek.ws.clone(), cx));
        let asking = trek.quiet_thread(cx);
        let failing = trek.quiet_thread(cx);
        let finished = trek.quiet_thread(cx);
        assert_eq!(badge(), 0);
        ask(&trek, cx, &asking);
        assert_eq!(badge(), 1);
        finish(&trek, cx, &failing, Some("boom"));
        assert_eq!(badge(), 2);
        // A turn that finished, read or not, doesn't call for the user.
        finish(&trek, cx, &finished, None);
        assert_eq!(badge(), 2);
        trek.update(cx, |ws, cx| ws.mark_unread(&finished, cx));
        assert_eq!(badge(), 2);

        // Answering clears it; so does settling a failure (it's been seen to). Undone, the
        // failure is back.
        trek.update(cx, |ws, cx| ws.respond(&asking, &format!("{asking}-ask"), Decision::Allow, cx));
        assert_eq!(badge(), 1);
        trek.update(cx, |ws, cx| ws.settle(&failing, cx));
        assert_eq!(badge(), 0);
        trek.update(cx, |ws, cx| ws.undo(crate::workspace::UndoAction::Unsettle(failing.clone()), cx));
        assert_eq!(badge(), 1);
        assert_eq!(trek.run_state(cx, &failing), RunState::Failed);
        trek.update(cx, |ws, cx| ws.settle(&failing, cx));
        assert_eq!(badge(), 0);

        // A settled thread that asks again is back in the inbox, and on the badge.
        ask(&trek, cx, &failing);
        assert_eq!(badge(), 1);
        // Archived ones aren't.
        trek.update(cx, |ws, cx| ws.archive(&failing, cx));
        assert_eq!(badge(), 0);

        // The setting hides and shows it.
        ask(&trek, cx, &finished);
        assert_eq!(badge(), 1);
        trek.update(cx, |ws, cx| {
            ws.settings.notifications.dock_badge = false;
            cx.notify();
        });
        assert_eq!(badge(), 0);
        trek.update(cx, |ws, cx| {
            ws.settings.notifications.dock_badge = true;
            cx.notify();
        });
        assert_eq!(badge(), 1);
    });
}

#[test]
fn the_taskbar_badge_follows_the_main_window_where_the_button_is_the_windows() {
    use crate::system::BADGE_ON;
    run(async |cx| {
        let trek = open_with(cx, |s| s.notifications.dock_badge = true);
        // The app-wide handling `main` installs: the main window's closing and reopening.
        cx.update(|cx| {
            crate::root::init(trek.ws.clone(), cx);
            crate::system::init(trek.ws.clone(), cx);
        });
        let first = trek.read(cx, |ws, _| ws.main_window).expect("a main window");
        let asking = trek.quiet_thread(cx);
        // Another thread on screen: the one asking is waiting off to the side.
        trek.quiet_thread(cx);
        ask(&trek, cx, &asking);
        assert_eq!((badge(), BADGE_ON.with(|w| w.get())), (1, Some(first)));

        // Close the main window and open another: a taskbar button belongs to its window, so the
        // new one gets the badge; the Dock tile is the app's, and nothing is asked again.
        trek.window(cx, |window, _| window.remove_window());
        cx.run_until_parked();
        cx.update(|cx| crate::root::show_main(trek.ws.clone(), cx));
        cx.run_until_parked();
        let second = trek.read(cx, |ws, _| ws.main_window).expect("a new main window");
        assert_ne!(first, second);
        assert_eq!(BADGE_ON.with(|w| w.get()), Some(if cfg!(windows) { second } else { first }));
        assert_eq!(badge(), 1);

        // Answered: cleared, on the window it's on.
        trek.update(cx, |ws, cx| ws.respond(&asking, &format!("{asking}-ask"), Decision::Allow, cx));
        assert_eq!((badge(), BADGE_ON.with(|w| w.get())), (0, Some(second)));
    });
}

#[test]
fn a_banner_is_the_message_alone_and_clicking_it_needs_no_button() {
    run(async |cx| {
        let trek = alerting(cx, NotifyMode::Banner);
        let id = titled(&trek, cx, "Fix the login bug");
        blur(&trek, cx);
        finish(&trek, cx, &id, None);
        let shown = cx.shown_system_notifications();
        let [banner] = shown.as_slice() else { panic!("one banner, got {}", shown.len()) };
        // What Windows (and macOS) show: the title, and nothing to press but the banner itself.
        assert_eq!(banner.title.as_ref(), "Finished: Fix the login bug");
        assert!(banner.body.is_empty() && banner.actions.is_empty());
        // Windows reports a click on the toast body with no action.
        cx.simulate_system_notification_response(SystemNotificationResponse { tag: banner.tag.clone(), action_id: None });
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(id));
    });
}

#[test]
fn the_mac_stays_awake_exactly_while_agents_work() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.prevent_sleep_while_running = true);
        cx.update(|cx| crate::system::init(trek.ws.clone(), cx));
        assert_eq!(cx.active_idle_sleep_preventions(), 0);

        let id = trek.send(cx, "mock:long 2s");
        let tid = id.clone();
        trek.wait(cx, "the build to start", |ws| ws.any_turn_running() && ws.live[&tid].items.iter().any(|i| matches!(i, trek_core::store::Item::Tool { .. }))).await;
        assert_eq!(cx.active_idle_sleep_preventions(), 1, "held while the turn runs");
        // A second running turn shares it.
        let other = trek.update(cx, |ws, cx| {
            ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx);
            ws.route.clone()
        });
        assert!(matches!(other, Route::Draft { .. }));
        let second = trek.send(cx, "mock:long 1s");
        let sid = second.clone();
        trek.wait(cx, "the second build", |ws| ws.live.get(&sid).is_some_and(|l| l.turn_started.is_some())).await;
        assert_eq!(cx.active_idle_sleep_preventions(), 1);

        // Turned off mid-turn: let go at once; back on: held again.
        trek.update(cx, |ws, cx| {
            ws.settings.general.prevent_sleep_while_running = false;
            cx.notify();
        });
        assert_eq!(cx.active_idle_sleep_preventions(), 0);
        trek.update(cx, |ws, cx| {
            ws.settings.general.prevent_sleep_while_running = true;
            cx.notify();
        });
        assert_eq!(cx.active_idle_sleep_preventions(), 1);

        trek.wait_done(cx, &second, RunState::Idle).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(cx.active_idle_sleep_preventions(), 0, "released when every turn is over");

        // Waiting on the user isn't working: the Mac may sleep.
        let asking = trek.send(cx, "needs permission");
        trek.wait_needs_you(cx, &asking).await;
        assert_eq!(cx.active_idle_sleep_preventions(), 0);
        let rid = trek.request(cx, &asking);
        trek.update(cx, |ws, cx| ws.respond(&asking, &rid, Decision::Allow, cx));
        assert_eq!(cx.active_idle_sleep_preventions(), 1, "working again");
        trek.wait_done(cx, &asking, RunState::Idle).await;
        assert_eq!(cx.active_idle_sleep_preventions(), 0);
    });
}

#[test]
fn the_dock_shows_the_app_icon_picked_in_appearance() {
    use crate::system::APP_ICON;
    use crate::workspace::SettingsPage;
    use trek_core::settings::AppIcon;
    run(async |cx| {
        let trek = open_with(cx, |_| {});
        APP_ICON.with(|i| i.set(None));
        cx.update(|cx| crate::system::init(trek.ws.clone(), cx));
        // The cairn on Trek orange goes up at launch, whatever the bundle's icon is.
        assert_eq!(APP_ICON.with(|i| i.get()), Some(AppIcon::Ember));
        trek.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::Appearance), cx));
        trek.render(cx);
        trek.click(cx, "app-icon-Night");
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.settings.appearance.app_icon), AppIcon::Night);
        assert_eq!(APP_ICON.with(|i| i.get()), Some(AppIcon::Night));
        trek.click(cx, "app-icon-Glass");
        cx.run_until_parked();
        assert_eq!(APP_ICON.with(|i| i.get()), Some(AppIcon::Glass));
    });
}
