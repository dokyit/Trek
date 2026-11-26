//! What Trek tells the system: alerts (toasts, banners, sounds) and where clicking them leads,
//! the Dock badge, and keeping the Mac awake while agents work. Banners go to the test platform,
//! sounds and the badge to `system`'s test hooks: nothing reaches the real system.

mod common;

use common::{Trek, open_with, run};
use gpui_kit::{SystemNotificationResponse, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;
use trek_agents::{AgentEvent, Decision};
use trek_app::system::{DOCK_BADGE, SOUNDS};
use trek_app::workspace::{Route, WorkspaceEvent};
use trek_core::RunState;
use trek_core::settings::NotifyMode;

/// A Trek window with the app-wide alert handling the app installs at launch.
fn alerting(cx: &mut TestAppContext, mode: NotifyMode) -> Trek {
    let trek = open_with(cx, |s| s.notifications.mode = mode);
    cx.update(|cx| {
        // Banners are only recorded once the app has an identity, as on a real system.
        cx.set_app_identity("dev.trek.Trek", "Trek");
        trek_app::root::init(trek.ws.clone(), cx);
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
    let events = vec![
        AgentEvent::TextDelta("Done.".into()),
        AgentEvent::TurnComplete {
            error: error.map(String::from),
        },
    ];
    trek.update(cx, |ws, cx| ws.apply_events(id, events, cx));
}

fn ask(trek: &Trek, cx: &mut TestAppContext, id: &str) {
    let ask = AgentEvent::PermissionRequest {
        request_id: format!("{id}-ask"),
        title: "Run command".into(),
        detail: "make deploy".into(),
        prompt: None,
        call: None,
    };
    trek.update(cx, |ws, cx| {
        ws.apply_events(
            id,
            vec![AgentEvent::TextDelta("Deploying.".into()), ask],
            cx,
        )
    });
}

fn banners(cx: &TestAppContext) -> Vec<(String, String)> {
    cx.shown_system_notifications()
        .into_iter()
        .map(|n| (n.tag.to_string(), n.title.to_string()))
        .collect()
}

fn toasts(trek: &Trek, cx: &mut TestAppContext) -> usize {
    trek.window(cx, |window, cx| trek_app::toast::count(window, cx))
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
        assert_eq!(
            said.borrow().last().map(String::as_str),
            Some("Finished: Fix the login bug")
        );
        assert_eq!((toasts(&trek, cx), banners(cx).len(), sounds()), (1, 0, 0));
        // Clicking the toast opens that thread (once it has slid into place: mid-slide, the
        // press and the release land on different frames and make no click).
        settled_in(&trek, cx);
        trek.click(cx, "notification");
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(away.clone())
        );

        // In the background: a banner and the sound, even for the thread on screen.
        blur(&trek, cx);
        ask(&trek, cx, &away);
        assert_eq!(
            said.borrow().last().map(String::as_str),
            Some("Waiting: run make deploy? · Fix the login bug")
        );
        assert_eq!(
            banners(cx),
            [(
                format!("trek-attention-{away}"),
                "Waiting: run make deploy? · Fix the login bug".to_string()
            )]
        );
        assert_eq!(sounds(), 1);

        // A failed turn says so; a turn the user stopped says nothing.
        finish(&trek, cx, &here, Some("The agent crashed"));
        assert_eq!(
            said.borrow().last().map(String::as_str),
            Some("Failed: Write the docs")
        );
        assert_eq!(
            banners(cx).last(),
            Some(&(
                format!("trek-attention-{here}"),
                "Failed: Write the docs".to_string()
            ))
        );
        let before = said.borrow().len();
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &away,
                vec![AgentEvent::TurnComplete {
                    error: Some("Interrupted".into()),
                }],
                cx,
            )
        });
        assert_eq!(said.borrow().len(), before, "no alert for a stop");

        // Clicking a banner brings Trek forward on its thread.
        cx.simulate_system_notification_response(SystemNotificationResponse {
            tag: format!("trek-attention-{here}").into(),
            action_id: None,
        });
        cx.run_until_parked();
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(here.clone())
        );
        assert_eq!(
            cx.update(|cx| cx.active_window()),
            trek.read(cx, |ws, _| ws.main_window),
            "the main window came forward"
        );
        assert_eq!(
            cx.update(|cx| cx.windows().len()),
            1,
            "no second main window"
        );
        // Tags Trek didn't post are left alone.
        cx.simulate_system_notification_response(SystemNotificationResponse {
            tag: "something-else".into(),
            action_id: None,
        });
        cx.run_until_parked();
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(here.clone())
        );
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
        cx.simulate_system_notification_response(SystemNotificationResponse {
            tag: format!("trek-attention-{popped}").into(),
            action_id: None,
        });
        cx.run_until_parked();
        assert_eq!(
            cx.update(|cx| cx.active_window()),
            Some(own),
            "its own window comes forward"
        );
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(other.clone()),
            "the main window is left as it was"
        );

        // With the main window closed, a banner still goes out, and clicking it reopens the
        // main window on the thread.
        trek.window(cx, |window, _| window.remove_window());
        cx.run_until_parked();
        VisualTestContext::from_window(own, cx).deactivate_window();
        finish(&trek, cx, &other, Some("Out of credits"));
        assert_eq!(
            banners(cx).last().map(|(_, t)| t.as_str()),
            Some("Failed: Stays in main")
        );
        cx.simulate_system_notification_response(SystemNotificationResponse {
            tag: format!("trek-attention-{other}").into(),
            action_id: None,
        });
        cx.run_until_parked();
        let main = trek
            .read(cx, |ws, _| ws.main_window)
            .expect("the main window is back");
        assert_ne!(main, trek.window, "a new main window");
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(other.clone())
        );
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
        cx.update(|cx| trek_app::system::init(trek.ws.clone(), cx));
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
        trek.update(cx, |ws, cx| {
            ws.respond(&asking, &format!("{asking}-ask"), Decision::Allow, cx)
        });
        assert_eq!(badge(), 1);
        trek.update(cx, |ws, cx| ws.settle(&failing, cx));
        assert_eq!(badge(), 0);
        trek.update(cx, |ws, cx| {
            ws.undo(
                trek_app::workspace::UndoAction::Unsettle(failing.clone()),
                cx,
            )
        });
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
    use trek_app::system::BADGE_ON;
    run(async |cx| {
        let trek = open_with(cx, |s| s.notifications.dock_badge = true);
        // The app-wide handling `main` installs: the main window's closing and reopening.
        cx.update(|cx| {
            trek_app::root::init(trek.ws.clone(), cx);
            trek_app::system::init(trek.ws.clone(), cx);
        });
        let first = trek
            .read(cx, |ws, _| ws.main_window)
            .expect("a main window");
        let asking = trek.quiet_thread(cx);
        // Another thread on screen: the one asking is waiting off to the side.
        trek.quiet_thread(cx);
        ask(&trek, cx, &asking);
        assert_eq!((badge(), BADGE_ON.with(|w| w.get())), (1, Some(first)));

        // Close the main window and open another: a taskbar button belongs to its window, so the
        // new one gets the badge; the Dock tile is the app's, and nothing is asked again.
        trek.window(cx, |window, _| window.remove_window());
        cx.run_until_parked();
        cx.update(|cx| trek_app::root::show_main(trek.ws.clone(), cx));
        cx.run_until_parked();
        let second = trek
            .read(cx, |ws, _| ws.main_window)
            .expect("a new main window");
        assert_ne!(first, second);
        assert_eq!(
            BADGE_ON.with(|w| w.get()),
            Some(if cfg!(windows) { second } else { first })
        );
        assert_eq!(badge(), 1);

        // Answered: cleared, on the window it's on.
        trek.update(cx, |ws, cx| {
            ws.respond(&asking, &format!("{asking}-ask"), Decision::Allow, cx)
        });
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
        let [banner] = shown.as_slice() else {
            panic!("one banner, got {}", shown.len())
        };
        // What Windows (and macOS) show: the title, and nothing to press but the banner itself.
        assert_eq!(banner.title.as_ref(), "Finished: Fix the login bug");
        assert!(banner.body.is_empty() && banner.actions.is_empty());
        // Windows reports a click on the toast body with no action.
        cx.simulate_system_notification_response(SystemNotificationResponse {
            tag: banner.tag.clone(),
            action_id: None,
        });
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(id));
    });
}

/// A Windows banner carries its thread's `trek://` link, which Windows opens when it's clicked:
/// that starts Trek after it has quit, where the click on its own reaches no one. Only from the
/// Trek the links open (a trial build's would open the user's own Trek); a Mac's goes as it did.
#[test]
fn a_banner_opens_its_thread_s_link_only_from_the_trek_the_links_open() {
    run(async |cx| {
        let trek = alerting(cx, NotifyMode::Banner);
        let id = titled(&trek, cx, "Fix the login bug");
        let tag = format!("trek-attention-{id}");
        blur(&trek, cx);
        trek_app::winsys::set_owns_links(false);
        finish(&trek, cx, &id, None);
        assert_eq!(banners(cx).len(), 1);
        assert_eq!(
            cx.system_notification_launch(&tag),
            None,
            "not this Trek's links"
        );

        trek_app::winsys::set_owns_links(true);
        ask(&trek, cx, &id);
        assert_eq!(banners(cx).len(), 2);
        let link = cx.system_notification_launch(&tag).map(|l| l.to_string());
        trek_app::winsys::set_owns_links(false);
        if cfg!(windows) {
            assert_eq!(link, Some(format!("trek://thread/{id}")));
        } else {
            assert_eq!(link, None);
        }
    });
}

/// What the link does when Windows launches Trek on it: a Trek that had quit opens on the thread,
/// and a running one is handed the link and shows the thread as a click on the banner would, in
/// its own window when it has one.
#[test]
fn a_banner_s_link_reveals_its_thread_at_launch_and_when_handed_over() {
    use trek_app::single_instance::Forwarded;
    use trek_ipc::instance::Open;
    run(async |cx| {
        let trek = alerting(cx, NotifyMode::Banner);
        let quiet = titled(&trek, cx, "Quiet");
        let finished = titled(&trek, cx, "Finished one");
        let popped = titled(&trek, cx, "Popped out");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(quiet.clone()), cx));
        let (links, heard) = async_channel::unbounded();
        let (tx, rx) = async_channel::unbounded();
        let ws = trek.ws.clone();
        // Started by the click: the link is the launch's own, opened once the window is up.
        cx.update(|cx| {
            trek_app::single_instance::hear(
                Some(rx),
                vec![format!("trek://thread/{finished}")],
                ws,
                links,
                cx,
            )
        });
        trek.update(cx, |ws, cx| ws.hear_deep_links(heard, cx));
        cx.run_until_parked();
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(finished.clone())
        );

        // Running, with the thread in a window of its own: that window comes forward.
        let own = trek.open_thread_window(cx, &popped);
        blur(&trek, cx);
        VisualTestContext::from_window(own, cx).deactivate_window();
        let (done, answer) = async_channel::bounded(1);
        tx.try_send(Forwarded {
            open: Open {
                args: vec![format!("trek://thread/{popped}")],
                background: false,
            },
            done,
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(answer.try_recv(), Ok(Ok(())));
        assert_eq!(cx.update(|cx| cx.active_window()), Some(own));
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(finished.clone()),
            "the main window is left as it was"
        );
    });
}

#[test]
fn the_mac_stays_awake_exactly_while_agents_work() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.prevent_sleep_while_running = true);
        cx.update(|cx| trek_app::system::init(trek.ws.clone(), cx));
        assert_eq!(cx.active_idle_sleep_preventions(), 0);

        // Working through the checks below: a loaded runner can take seconds to reach them.
        let id = trek.send(cx, "mock:long 8s");
        let tid = id.clone();
        trek.wait(cx, "the build to start", |ws| {
            ws.any_turn_running()
                && ws.live[&tid]
                    .items
                    .iter()
                    .any(|i| matches!(i, trek_core::store::Item::Tool { .. }))
        })
        .await;
        assert_eq!(
            cx.active_idle_sleep_preventions(),
            1,
            "held while the turn runs"
        );
        // A second running turn shares it.
        let other = trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(trek.project.clone()),
                },
                cx,
            );
            ws.route.clone()
        });
        assert!(matches!(other, Route::Draft { .. }));
        let second = trek.send(cx, "mock:long 5s");
        let sid = second.clone();
        trek.wait(cx, "the second build", |ws| {
            ws.live.get(&sid).is_some_and(|l| l.turn_started.is_some())
        })
        .await;
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
        assert_eq!(
            cx.active_idle_sleep_preventions(),
            0,
            "released when every turn is over"
        );

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
    use trek_app::system::APP_ICON;
    use trek_app::workspace::SettingsPage;
    use trek_core::settings::AppIcon;
    run(async |cx| {
        let trek = open_with(cx, |_| {});
        APP_ICON.with(|i| i.set(None));
        cx.update(|cx| trek_app::system::init(trek.ws.clone(), cx));
        // The default (Night) goes up at launch, whatever the bundle's icon is.
        assert_eq!(APP_ICON.with(|i| i.get()), Some(AppIcon::Night));
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Appearance), cx)
        });
        trek.render(cx);
        // The app icons are under the theme gallery: scrolled to, as a user would.
        let page = trek.bounds(cx, "settings-content").expect("the page");
        let wheel = gpui_kit::ScrollWheelEvent {
            position: page.center(),
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                gpui_kit::px(-600.),
            )),
            ..Default::default()
        };
        trek.window(cx, |window, cx| {
            _ = window.dispatch_event(gpui_kit::InputEvent::to_platform_input(wheel), cx)
        });
        trek.render(cx);
        trek.click(cx, "app-icon-Paper");
        cx.run_until_parked();
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.appearance.app_icon),
            AppIcon::Paper
        );
        assert_eq!(APP_ICON.with(|i| i.get()), Some(AppIcon::Paper));
        trek.click(cx, "app-icon-Glass");
        cx.run_until_parked();
        assert_eq!(APP_ICON.with(|i| i.get()), Some(AppIcon::Glass));
        trek.click(cx, "app-icon-Night");
        cx.run_until_parked();
        assert_eq!(APP_ICON.with(|i| i.get()), Some(AppIcon::Night));
    });
}

/// The log's lines for alerts decided since the last look.
fn alert_log() -> Vec<String> {
    trek_app::system::ALERT_LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

/// A thread another thread's agent started (`start_thread`), as the store has one.
fn started_by(trek: &Trek, cx: &mut TestAppContext, parent: &str, title: &str) -> String {
    trek.update(cx, |ws, cx| {
        let mut t = ws
            .store
            .create_thread(
                Some(&trek.project),
                common::mock(),
                None,
                trek_core::Effort::Medium,
                trek_core::HandHolding::Auto,
            )
            .expect("thread");
        t.title = title.into();
        t.started_by = Some(parent.to_string());
        ws.store.save_thread(&t).expect("save");
        ws.reload(cx);
        t.id
    })
}

/// The user sent `id` a prompt: a turn of theirs is open. (A turn the agent starts by itself,
/// woken by its background work, alerts once in a while at most; theirs always does.)
fn prompted(trek: &Trek, cx: &mut TestAppContext, id: &str) {
    trek.update(cx, |ws, cx| {
        ws.load_for_view(id, cx);
        ws.live.get_mut(id).expect("live").turn_started = Some(std::time::Instant::now());
    });
}

/// `finish`, for a turn the user started.
fn finish_prompted(trek: &Trek, cx: &mut TestAppContext, id: &str, error: Option<&str>) {
    prompted(trek, cx, id);
    finish(trek, cx, id, error);
}

/// A shell command the agent left running between turns.
fn background_command(trek: &Trek, cx: &mut TestAppContext, id: &str) {
    let task = trek_agents::BackgroundTask {
        id: "bg-1".into(),
        kind: trek_agents::BackgroundKind::Shell,
        title: "npm run dev".into(),
        call: None,
        readable: true,
        stoppable: true,
    };
    trek.update(cx, |ws, cx| {
        ws.apply_events(id, vec![AgentEvent::Background(vec![task])], cx)
    });
}

/// The report this answers, as it happened on a Mac with 0.4.64, alerts set to banner and sound
/// and only in the background: two turns ended while Trek was the app in front, and the Mac made
/// no banner and no sound while the phone got its note both times. That was the setting at work,
/// which nothing said: now the log has a line for each, saying where the user was and why the
/// Mac stayed quiet. The same two turns with another app in front post a banner and play the
/// sound, as the one alert Trek did post that morning (with another app in front) had.
#[test]
fn the_reported_turns_were_quiet_because_trek_was_in_front_and_the_log_says_so() {
    run(async |cx| {
        let trek = alerting(cx, NotifyMode::BannerAndSound);
        let said = attention(&trek, cx);
        let here = titled(&trek, cx, "Trek dev");
        focus(&trek, cx);
        alert_log();

        // Test 1: the turn of the thread on screen ends with a command of its still running in
        // the background. The turn is over, so it's "Finished", command or no command; and it's
        // the thread being looked at, in the window in front: nothing at all.
        background_command(&trek, cx, &here);
        finish_prompted(&trek, cx, &here, None);
        assert_eq!(*said.borrow(), ["Finished: Trek dev"]);
        assert_eq!((toasts(&trek, cx), banners(cx).len(), sounds()), (0, 0, 0));
        let short = &here[..8];
        assert_eq!(
            alert_log(),
            [format!(
                "alert thread={short} event=finished focused=true viewing=true app_active=true key_window=true away=false mode=banner-and-sound only_when_unfocused=true -> no banner or sound: the thread is on screen in the window in front"
            )]
        );

        // Test 2: a thread that one started (`start_thread`) finishes, Trek still in front: a
        // note in the window with the way to it, and no banner or sound.
        let child = started_by(&trek, cx, &here, "Notification test");
        finish_prompted(&trek, cx, &child, None);
        assert_eq!(
            said.borrow().last().map(String::as_str),
            Some("Thread completed · Notification test")
        );
        assert_eq!((toasts(&trek, cx), banners(cx).len(), sounds()), (1, 0, 0));
        let log = alert_log();
        assert_eq!(log.len(), 1, "{log:?}");
        assert!(
            log[0].ends_with(
                "focused=true viewing=false app_active=true key_window=true away=false mode=banner-and-sound only_when_unfocused=true -> no banner or sound: Trek is in front and alerts wait for the background, a note in the window"
            ),
            "{log:?}"
        );
        assert!(
            !log[0].contains("Notification test") && !log[0].contains(&child[8..]),
            "no title, and the id cut short: {log:?}"
        );

        // The same two with another app in front: a banner and the sound each, and the log has
        // what the system said to each banner.
        blur(&trek, cx);
        finish_prompted(&trek, cx, &child, None);
        assert_eq!(
            banners(cx).last(),
            Some(&(
                format!("trek-attention-{child}"),
                "Thread completed · Notification test".to_string()
            ))
        );
        assert_eq!(sounds(), 1);
        let child_short = &child[..8];
        assert_eq!(
            alert_log(),
            [
                format!(
                    "alert thread={child_short} event=finished focused=false viewing=false app_active=true key_window=false away=false mode=banner-and-sound only_when_unfocused=true -> banner and sound, a note in the window"
                ),
                format!(
                    "alert thread={child_short} banner: {} took it (allowed)",
                    trek_app::banner::system_name()
                ),
            ]
        );
        finish_prompted(&trek, cx, &here, None);
        assert_eq!(
            banners(cx).last(),
            Some(&(
                format!("trek-attention-{here}"),
                "Finished: Trek dev".to_string()
            ))
        );
        assert_eq!(sounds(), 2, "with its command still running");
        alert_log();

        // A banner the system turns down is said too, with its reason.
        trek_app::banner::fake::refuse(Some("Notifications are not allowed for this application"));
        finish_prompted(&trek, cx, &here, Some("The agent crashed"));
        let log = alert_log();
        assert!(
            log[0].starts_with(&format!("alert thread={short} event=failed ")),
            "{log:?}"
        );
        assert_eq!(
            log[1],
            format!(
                "alert thread={short} banner: {} refused it (allowed): Notifications are not allowed for this application",
                trek_app::banner::system_name()
            )
        );
        trek_app::banner::fake::refuse(None);
    });
}

/// "In front" is the user being in Trek, which takes more than a window that says it has the
/// keys: the system's word that Trek is the app in front, and someone at the computer.
#[test]
fn a_trek_nobody_is_in_alerts_as_one_in_the_background() {
    run(async |cx| {
        let trek = alerting(cx, NotifyMode::BannerAndSound);
        let away = titled(&trek, cx, "Fix the login bug");
        titled(&trek, cx, "Write the docs");
        focus(&trek, cx);
        alert_log();

        // In Trek: a note in the window, as before.
        finish_prompted(&trek, cx, &away, None);
        assert_eq!((banners(cx).len(), sounds()), (0, 0));

        // Another app is in front while Trek's window still counts itself as having the keys
        // (its word that it lost them never came, or a panel of Trek's took them): a banner.
        trek_app::system::fake_presence(false, false);
        finish_prompted(&trek, cx, &away, None);
        assert_eq!((banners(cx).len(), sounds()), (1, 1));
        assert!(alert_log().iter().any(|l| {
            l.contains("focused=false viewing=false app_active=false key_window=true away=false")
        }));

        // Trek left in front of a locked screen, or of a keyboard nobody has touched for a
        // while: nobody is looking, so the banner (it waits in the notification center and on
        // the lock screen) and the sound go out.
        trek_app::system::fake_presence(true, true);
        ask(&trek, cx, &away);
        assert_eq!((banners(cx).len(), sounds()), (2, 2));
        assert!(alert_log().iter().any(|l| l.contains(
            "event=needs-you focused=false viewing=false app_active=true key_window=true away=true"
        )));

        // Back at the keyboard, in Trek: quiet again.
        trek_app::system::fake_presence(true, false);
        finish_prompted(&trek, cx, &away, None);
        assert_eq!((banners(cx).len(), sounds()), (2, 2));
    });
}

/// What "finished" means while work goes on after the turn. A command left running (a server,
/// a watcher) doesn't hold the alert back: the turn is over and the agent has said its piece.
/// Sub-agents of the agent's own still out do: the thread isn't done until the turn their
/// reports start has ended, and says so then.
#[test]
fn a_turn_is_finished_with_a_command_still_running_but_not_with_sub_agents_out() {
    run(async |cx| {
        let trek = alerting(cx, NotifyMode::BannerAndSound);
        let said = attention(&trek, cx);
        let id = titled(&trek, cx, "Build the importer");
        blur(&trek, cx);

        background_command(&trek, cx, &id);
        finish_prompted(&trek, cx, &id, None);
        assert_eq!(*said.borrow(), ["Finished: Build the importer"]);
        assert_eq!((banners(cx).len(), sounds()), (1, 1));

        // A sub-agent of its own goes on in the background as the next turn ends: no alert.
        let scout = |done| AgentEvent::Task {
            id: "toolu_scout".into(),
            description: Some("Scout the parser".into()),
            activity: None,
            tool_uses: None,
            done,
        };
        let out = trek_agents::BackgroundTask {
            id: "agent-1".into(),
            kind: trek_agents::BackgroundKind::Agent,
            title: "Scout the parser".into(),
            call: Some("toolu_scout".into()),
            readable: false,
            stoppable: true,
        };
        prompted(&trek, cx, &id);
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    AgentEvent::TextDelta("Scouting.".into()),
                    scout(None),
                    AgentEvent::Background(vec![out]),
                    AgentEvent::TurnComplete { error: None },
                ],
                cx,
            )
        });
        assert_eq!(said.borrow().len(), 1, "it waits on its sub-agent");
        assert_eq!((banners(cx).len(), sounds()), (1, 1));

        // The sub-agent reports, the agent takes its turn for that, and that turn's end is the
        // thread's: now it's finished.
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    scout(Some(true)),
                    AgentEvent::Background(vec![]),
                    AgentEvent::TextDelta("The parser is fine.".into()),
                    AgentEvent::TurnComplete { error: None },
                ],
                cx,
            )
        });
        assert_eq!(said.borrow().len(), 2);
        assert_eq!(
            said.borrow().last().map(String::as_str),
            Some("Finished: Build the importer")
        );
        assert_eq!((banners(cx).len(), sounds()), (2, 2));
    });
}

/// Settings › Notifications' "Send a test": five seconds after the click, the banner and the
/// sound the settings ask for, whatever is in front, and under the button what the system says.
#[test]
fn the_test_in_settings_sends_a_real_banner_and_says_what_the_system_made_of_it() {
    use trek_app::banner::{self, Os};
    use trek_app::settings_view::shown;
    use trek_app::workspace::SettingsPage;
    let said = |trek: &Trek, cx: &mut TestAppContext| -> Vec<String> {
        shown::take();
        trek.render(cx);
        shown::take()
    };
    let has = |texts: &[String], part: &str| texts.iter().any(|t| t.contains(part));
    run(async |cx| {
        let trek = alerting(cx, NotifyMode::BannerAndSound);
        focus(&trek, cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Notifications), cx)
        });
        let system = banner::system_name();

        // What "only in the background" comes to is said under it, in plain words.
        let texts = said(&trek, cx);
        assert!(
            has(&texts, "Trek stays quiet on") && has(&texts, "while its window is in front"),
            "{texts:?}"
        );
        assert!(has(&texts, &format!("{system} allows Trek's banners.")));
        assert!(!trek.visible(cx, "banner-settings"));

        // The click sends nothing yet: there's time to switch to another app.
        alert_log();
        trek.click(cx, "banner-test");
        assert!(has(&said(&trek, cx), "Sending in 5 seconds."));
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(4));
        cx.run_until_parked();
        assert_eq!((banners(cx).len(), sounds()), (0, 0));
        // Then it goes, though Trek is in front and alerts wait for the background.
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(2));
        cx.run_until_parked();
        assert_eq!(
            banners(cx),
            [(
                "trek-test-banner".to_string(),
                "Trek's banners work".to_string()
            )]
        );
        assert_eq!(sounds(), 1);
        assert!(has(
            &said(&trek, cx),
            &format!("Sent, and {system} took it.")
        ));
        let log = alert_log();
        assert!(
            log[0].starts_with("alert thread=- event=test ")
                && log[0].ends_with("-> banner and sound"),
            "{log:?}"
        );
        assert_eq!(
            log[1],
            format!("alert thread=- banner: {system} took it (allowed)")
        );

        // The system has Trek's notifications off: the row says so, the test says why nothing
        // showed, and a button opens the system's settings for it.
        banner::fake::set(Os::Denied);
        banner::fake::refuse(Some("Notifications are not allowed for this application"));
        trek.click(cx, "banner-test");
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(6));
        cx.run_until_parked();
        let texts = said(&trek, cx);
        assert!(
            has(
                &texts,
                "Not shown: Notifications are not allowed for this application. Denied:"
            ),
            "{texts:?}"
        );
        assert_eq!(banners(cx).len(), 1, "none posted");
        trek.click(cx, "banner-settings");
        assert_eq!(
            banner::fake::opened(),
            [trek_app::permissions::Permission::Notifications
                .pane()
                .to_string()]
        );
        banner::fake::refuse(None);

        // A Focus holds banners back without refusing them: said by name, and its settings open.
        banner::fake::set(Os::Focus(Some("Do Not Disturb".into())));
        trek.click(cx, "banner-test");
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(6));
        cx.run_until_parked();
        assert!(has(&said(&trek, cx), "Sent. Focus is on (Do Not Disturb):"));
        // Delivered quietly: allowed, with no banner style.
        banner::fake::set(Os::Quiet);
        assert!(has(&said(&trek, cx), "Delivered quietly:"));
        assert!(trek.visible(cx, "banner-settings"));
        banner::fake::set(Os::Allowed);

        // Sound alone: the test plays it and posts nothing. Off: nothing to send.
        trek.update(cx, |ws, cx| {
            ws.settings.notifications.mode = NotifyMode::Sound;
            cx.notify()
        });
        let (posted, played) = (banners(cx).len(), sounds());
        trek.click(cx, "banner-test");
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(6));
        cx.run_until_parked();
        assert_eq!((banners(cx).len(), sounds()), (posted, played + 1));
        assert!(has(&said(&trek, cx), "Played the sound."));
        trek.update(cx, |ws, cx| {
            ws.settings.notifications.mode = NotifyMode::Off;
            cx.notify()
        });
        assert!(has(&said(&trek, cx), "Alerts are off"));
        trek.click(cx, "banner-test");
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(6));
        cx.run_until_parked();
        assert_eq!((banners(cx).len(), sounds()), (posted, played + 1));

        // With the switch off, the words under it say what that comes to.
        trek.update(cx, |ws, cx| {
            ws.settings.notifications.mode = NotifyMode::BannerAndSound;
            ws.settings.notifications.only_when_unfocused = false;
            cx.notify()
        });
        assert!(has(&said(&trek, cx), "while you're in Trek too"));
    });
}
