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
