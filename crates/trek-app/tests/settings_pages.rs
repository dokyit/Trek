//! Settings pages: the Agents page's Sign in and Install buttons, the agent list's cached keys,
//! the pairing code's lifetime and the scroll container the driver reaches.

mod common;

use common::runs;
use common::{Trek, open, run};
use gpui_kit::TestAppContext;
use std::time::Duration;
use trek_app::workspace::{Route, SettingsPage};
use trek_core::AgentId;
use trek_core::detect::{Availability, DetectedAgent};

fn detected(agent: AgentId, name: &str, availability: Availability) -> DetectedAgent {
    DetectedAgent {
        agent,
        name: name.into(),
        path: None,
        version: None,
        availability,
        models: vec![],
        install_hint: None,
    }
}

/// The Agents page, drawn: OpenCode installed and not signed in, Droid not installed. A command
/// sent to the terminal leaves Settings, so each visit starts here.
fn agents_page(trek: &Trek, cx: &mut TestAppContext, opencode: Availability) {
    trek.update(cx, |ws, cx| {
        ws.agents = vec![
            detected(AgentId::OpenCode, "OpenCode", opencode),
            detected(AgentId::Droid, "Droid", Availability::NotInstalled),
        ];
        ws.navigate(Route::Settings(SettingsPage::Agents), cx);
    });
    trek.render(cx);
}

#[test]
fn sign_in_and_install_can_be_tried_again_after_an_attempt_that_went_nowhere() {
    run(async |cx| {
        let trek = open(cx);
        let ran = runs(&trek, cx);
        agents_page(&trek, cx, Availability::NeedsLogin);
        for (button, count) in [("login-opencode", 1), ("install-droid", 2)] {
            trek.click(cx, button);
            assert_eq!(ran.borrow().len(), count, "{button} runs its command");
            // The command is in a terminal Trek can't see the end of: the button waits, and
            // clicking it meanwhile does nothing more.
            agents_page(&trek, cx, Availability::NeedsLogin);
            trek.click(cx, button);
            assert_eq!(ran.borrow().len(), count, "{button} waits for the attempt");
        }
        // Scan again forgets the attempts: the buttons work.
        agents_page(&trek, cx, Availability::NeedsLogin);
        trek.click(cx, "rescan");
        for (button, count) in [("login-opencode", 3), ("install-droid", 4)] {
            agents_page(&trek, cx, Availability::NeedsLogin);
            trek.click(cx, button);
            assert_eq!(ran.borrow().len(), count, "{button} after Scan again");
        }
    });
}

#[test]
fn signing_in_is_forgotten_once_the_agent_is_signed_in() {
    run(async |cx| {
        let trek = open(cx);
        let ran = runs(&trek, cx);
        agents_page(&trek, cx, Availability::NeedsLogin);
        trek.click(cx, "login-opencode");
        assert_eq!(ran.borrow().len(), 1);
        // Signed in, then signed out again some time later: Sign in is there to use.
        agents_page(&trek, cx, Availability::Ready);
        assert!(!trek.visible(cx, "login-opencode"));
        agents_page(&trek, cx, Availability::NeedsLogin);
        trek.click(cx, "login-opencode");
        assert_eq!(ran.borrow().len(), 2);
    });
}

#[test]
fn the_agents_a_key_makes_ready_are_found_once_until_a_key_changes() {
    run(async |cx| {
        let trek = open(cx);
        let ready = |trek: &Trek, cx: &mut TestAppContext| {
            trek.read(cx, |ws, _| {
                ws.ready_agents()
                    .contains(&AgentId::Direct("openrouter".into()))
            })
        };
        assert!(!ready(&trek, cx));
        // What was found is what the pages (drawn often) use...
        trek.read(cx, |ws, _| {
            *ws.keyed_providers.borrow_mut() = Some(vec!["openrouter"])
        });
        assert!(ready(&trek, cx), "the Keychain isn't asked again");
        // ...until a key is saved or removed.
        trek.read(cx, |ws, _| ws.forget_keyed_providers());
        assert!(!ready(&trek, cx));
    });
}

#[test]
fn a_key_an_earlier_copy_saved_shows_as_locked_until_the_user_allows_access() {
    run(async |cx| {
        // Saved by "another build": macOS would only hand it over with the user's password.
        trek_core::keychain::plant_locked("deepseek", "sk-from-before");
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::ApiKeys), cx)
        });
        trek_app::settings_view::shown::take();
        trek.render(cx);
        // Opening Trek and the page read it quietly: locked, not handed to anything.
        assert_eq!(trek_core::settings::secrets::api_key("deepseek"), None);
        assert!(
            trek.visible(cx, "allow-deepseek"),
            "Allow access is offered"
        );
        assert!(
            trek_app::settings_view::shown::take()
                .iter()
                .any(|t| t.contains("Trek never asks for them on its own")),
            "Trek says what macOS will ask before it asks"
        );
        // Allow access (the isolated store stands for the user allowing it at the prompt).
        trek.click(cx, "allow-deepseek");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while trek.visible(cx, "allow-deepseek") {
            assert!(std::time::Instant::now() < deadline, "never unlocked");
            cx.background_executor.timer(Duration::from_millis(5)).await;
            trek.render(cx);
        }
        assert_eq!(
            trek_core::settings::secrets::api_key("deepseek").as_deref(),
            Some("sk-from-before")
        );
        assert!(
            trek.visible(cx, "del-deepseek"),
            "now a saved key, Remove offered"
        );
    });
}

#[test]
fn a_pairing_code_goes_from_settings_when_it_expires() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.start_test_remote();
            ws.offer_pairing(cx);
        });
        let offered = |trek: &Trek, cx: &TestAppContext| {
            trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().offer.is_some())
        };
        assert!(offered(&trek, cx));
        cx.executor().advance_clock(Duration::from_secs(60));
        cx.run_until_parked();
        assert!(offered(&trek, cx), "still good a minute on");
        // A new code replaces it, and the first one's timer doesn't take the new one away.
        trek.update(cx, |ws, cx| ws.offer_pairing(cx));
        cx.executor()
            .advance_clock(Duration::from_secs(9 * 60 + 30));
        cx.run_until_parked();
        assert!(offered(&trek, cx), "the new code runs its own time");
        cx.executor().advance_clock(Duration::from_secs(60));
        cx.run_until_parked();
        assert!(!offered(&trek, cx), "expired, so no longer shown as good");
    });
}

/// Scroll Settings' page down by `by` points, as a wheel would.
fn scroll_settings(trek: &Trek, cx: &mut TestAppContext, by: f32) {
    let page = trek.bounds(cx, "settings-content").expect("the page");
    let wheel = gpui_kit::ScrollWheelEvent {
        position: page.center(),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-by))),
        ..Default::default()
    };
    trek.window(cx, |window, cx| {
        _ = window.dispatch_event(gpui_kit::InputEvent::to_platform_input(wheel), cx)
    });
    trek.render(cx);
}

#[test]
fn the_pairing_codes_fingerprint_stays_inside_its_card() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.start_test_remote();
            ws.offer_pairing(cx);
            // As a server with its certificate has it (the test one has none): a link that long
            // makes the QR code as large as a real one.
            let offer = ws.remote.as_mut().unwrap().offer.as_mut().unwrap();
            offer.code = "GBFV-23F4".into();
            offer.fingerprint = Some("6586-47C9-BF84-93CD".into());
            offer.url = format!(
                "trek://pair?host=127.0.0.1:7431&code=GBFV-23F4&name=Test%E2%80%99s%20MacBook%20Air&hid={}&fp={}",
                "0".repeat(36),
                "0".repeat(64)
            );
            ws.navigate(Route::Settings(SettingsPage::Mobile), cx)
        });
        trek.render(cx);
        scroll_settings(&trek, cx, 500.);
        let card = trek.bounds(cx, "mobile-pairing").expect("the pairing card");
        let fingerprint = trek
            .bounds(cx, "pair-fingerprint")
            .expect("the fingerprint");
        assert!(
            fingerprint.right() <= card.right() && fingerprint.bottom() <= card.bottom(),
            "{fingerprint:?} in {card:?}"
        );
    });
}

#[test]
fn an_isolated_run_shows_no_iphone_install_of_the_users() {
    use trek_app::remote::phone_update;
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Mobile), cx)
        });
        trek.render(cx);
        // Whatever the phone updater of this Mac recorded of the user's iPhone stays out.
        assert_eq!(phone_update::last_attempt(), None);
        assert!(
            !trek.visible(cx, "mobile-update-phone"),
            "no iPhone to speak of"
        );
        // The run's own record, in its data folder, is the one read.
        let dir = trek_core::paths::data_dir().join("phone-update");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("last"),
            "at=1793390000\nresult=ok\nline=Installed Trek 0.4.27 on the iPhone (Test iPhone)\n",
        )
        .unwrap();
        assert_eq!(
            phone_update::last_attempt().map(|a| a.line),
            Some("Installed Trek 0.4.27 on the iPhone (Test iPhone)".into())
        );
        // And it never builds for or installs on the iPhone this Mac reaches.
        assert!(trek.update(cx, |ws, cx| ws.update_phone(None, cx)).is_err());
        assert!(!trek.read(cx, |ws, _| ws.phone_update.running));
    });
}

#[test]
fn phone_notifications_say_a_phones_browser_needs_the_phone_server() {
    run(async |cx| {
        let trek = common::open_with(cx, |s| {
            s.mobile.enabled = false;
            s.mobile.push = true;
        });
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Mobile), cx)
        });
        trek.render(cx);
        scroll_settings(&trek, cx, 300.);
        assert!(trek.visible(cx, "push-web-off"), "says why none gets them");
        assert!(!trek.visible(cx, "push-web-none"));
        // With phones let in, it says how to pair one instead.
        trek.update(cx, |ws, cx| {
            ws.settings.mobile.enabled = true;
            ws.start_test_remote();
            cx.notify();
        });
        trek.render(cx);
        assert!(!trek.visible(cx, "push-web-off"));
        assert!(trek.visible(cx, "push-web-none"));
    });
}

#[test]
fn the_settings_scroll_container_can_be_found_by_the_driver() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Tools), cx)
        });
        trek.render(cx);
        assert!(trek.visible(cx, "settings-content"));
    });
}

/// Settings › Tools › Video says what Trek downloads for the video watcher (no Homebrew or winget
/// needed), how far a download has got, and why one couldn't be made, with a way to try again.
#[test]
fn the_video_tools_say_what_trek_downloads_and_how_far_it_has_got() {
    use trek_app::workspace::MediaSetup;
    use trek_core::media_tools::{Progress, Tool};
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Tools), cx)
        });
        cx.simulate_window_resize(
            trek.window,
            gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(4000.)),
        );
        trek_app::settings_view::shown::take();
        trek.render(cx);
        trek.render(cx);
        let said = trek_app::settings_view::shown::take();
        let plan = trek_core::media_tools::plan(Tool::Ffmpeg).expect("a build for this computer");
        assert!(
            said.iter().any(|t| t.contains(&format!(
                "Trek downloads ffmpeg ({}) the first time an agent watches a video",
                plan.size_text()
            ))),
            "{said:?}"
        );
        assert!(
            said.iter()
                .any(|t| t.contains("Trek downloads yt-dlp (about"))
        );
        // Downloading: how far, in words.
        trek.update(cx, |ws, cx| {
            ws.media_setup = Some(MediaSetup::Downloading {
                tool: Tool::Ffmpeg,
                progress: Some(Progress::Downloading {
                    done: 42,
                    total: 100,
                }),
            });
            cx.notify();
        });
        trek.render(cx);
        assert!(trek.visible(cx, "ffmpeg-media-status"));
        // Failed: why, and again.
        trek.update(cx, |ws, cx| {
            ws.media_setup = Some(MediaSetup::Failed {
                tool: Tool::Ffmpeg,
                error: "Couldn't reach the download.".into(),
            });
            cx.notify();
        });
        trek.render(cx);
        assert!(trek.visible(cx, "ffmpeg-media-status"));
        assert!(trek.visible(cx, "install-ffmpeg"), "Try again");
        assert!(
            !trek.visible(cx, "yt-dlp-media-status"),
            "the other tool says nothing"
        );
    });
}

#[test]
fn a_saved_key_can_be_copied_and_says_where_it_lives() {
    run(async |cx| {
        // Gemini: a provider the other tests here leave alone (the store is shared in-process),
        // and a row near the top, in view without scrolling.
        trek_core::settings::secrets::set_api_key("google", "sk-test-copy").unwrap();
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::ApiKeys), cx)
        });
        trek_app::settings_view::shown::take();
        trek.render(cx);
        assert!(trek.visible(cx, "copy-google"), "Copy is offered");
        assert!(trek.visible(cx, "del-google"), "Remove is offered");
        // The status says which item holds it, so Keychain Access or Credential Manager can find
        // it: the Keychain's service, or the credential's name (`google.dev.trek.Trek`).
        let item = if cfg!(windows) {
            format!("google.{}", trek_core::keychain::SERVICE)
        } else {
            trek_core::keychain::SERVICE.to_string()
        };
        assert!(
            trek_app::settings_view::shown::take()
                .iter()
                .any(|t| t.contains(&item)),
            "the status names the item that holds the key"
        );
        trek.click(cx, "copy-google");
        trek.render(cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("sk-test-copy")
        );
    });
}

#[test]
fn a_trek_replaced_on_disk_offers_a_restart_in_place_of_allow() {
    // The Mac's permission rows; Windows asks for none.
    if trek_app::words::is_windows() {
        return;
    }
    run(async |cx| {
        use trek_app::permissions::{Permission, Status, fake};
        fake::set(Permission::Accessibility, Status::Replaced);
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Tools), cx)
        });
        // Tall enough for the computer use rows without scrolling.
        cx.simulate_window_resize(
            trek.window,
            gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(2400.)),
        );
        trek.render(cx);
        trek.render(cx);
        assert!(
            trek.visible(cx, "ax-perm-restart"),
            "a permission macOS set aside offers Restart Trek, not the pane"
        );
        assert!(
            trek.visible(cx, "sr-perm"),
            "one that was never granted still offers Allow"
        );
    });
}
