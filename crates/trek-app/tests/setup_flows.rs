//! Settings and onboarding choices that must stick, and what the pages say when something can't
//! be done: onboarding re-runs, agents' install lines, keys, local servers.

mod common;

use common::{open, run, toasts};
use trek_app::onboarding::Step;
use trek_app::workspace::{Route, SettingsPage};
use trek_core::detect::{Availability, DetectedAgent};
use trek_core::{AgentId, HandHolding};

#[test]
fn running_onboarding_again_and_skipping_leaves_the_defaults_alone() {
    run(async |cx| {
        let trek = open(cx);
        // A one-off choice in the draft: another agent, and a looser level.
        trek.update(cx, |ws, cx| {
            ws.settings.general.hand_holding = HandHolding::Supervised;
            ws.settings.general.default_agent = AgentId::ClaudeCode.key();
            ws.draft_prefs.hand_holding = HandHolding::Auto;
            ws.draft_prefs.agent = AgentId::Codex;
            ws.open_onboarding(None, cx);
        });
        trek.render(cx);
        trek.update(cx, |ws, cx| ws.finish_onboarding(None, cx));
        trek.read(cx, |ws, _| {
            assert_eq!(ws.settings.general.hand_holding, HandHolding::Supervised);
            assert_eq!(ws.settings.general.default_agent, AgentId::ClaudeCode.key());
        });
    });
}

#[test]
fn the_approvals_step_saves_the_level_it_is_given() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.settings.general.hand_holding = HandHolding::Supervised;
            ws.open_onboarding(Some(Step::Approvals), cx);
        });
        trek.render(cx);
        trek.click(cx, "ob-hh-Auto");
        trek.read(cx, |ws, _| {
            assert_eq!(ws.settings.general.hand_holding, HandHolding::Auto);
        });
    });
}

fn detected(agent: AgentId, availability: Availability) -> DetectedAgent {
    DetectedAgent {
        name: agent.display_name(),
        agent,
        path: None,
        version: None,
        availability,
        models: vec![],
        install_hint: None,
    }
}

#[test]
fn a_local_server_switched_off_can_be_switched_back_on_in_settings() {
    run(async |cx| {
        let trek = open(cx);
        let ollama = AgentId::Direct("ollama".into());
        trek.update(cx, |ws, cx| {
            let mut up = detected(ollama.clone(), Availability::Ready);
            up.models = vec!["qwen3-coder".into()];
            ws.agents = vec![up];
            ws.navigate(Route::Settings(SettingsPage::LocalModels), cx)
        });
        trek_app::settings_view::shown::take();
        trek.render(cx);
        cx.run_until_parked();
        trek.render(cx);
        // Running: its models, a switch, and a way to look again even though one is up.
        assert!(trek.visible(cx, "local-ollama-on"));
        assert!(trek.visible(cx, "rescan-local"));
        assert!(trek_app::settings_view::shown::take().iter().any(|t| t == "qwen3-coder"));
        trek.click(cx, "local-ollama-on");
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.settings.disabled_agents.contains(&ollama.key())));
        // Off, and then not found running: the switch is still there to bring it back.
        trek.update(cx, |ws, _| {
            ws.agents = vec![detected(ollama.clone(), Availability::Offline)];
        });
        trek.render(cx);
        assert!(trek.visible(cx, "local-ollama-on"), "still offered while it is off");
        trek.click(cx, "local-ollama-on");
        trek.render(cx);
        assert!(!trek.read(cx, |ws, _| ws.settings.disabled_agents.contains(&ollama.key())));
        // A server that isn't installed says where to get it.
        assert!(trek.visible(cx, "local-lmstudio-get") || trek.visible(cx, "local-lmstudio-start"));
    });
}

#[test]
fn the_phone_step_offers_a_phones_browser_and_never_a_repo_path() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.start_test_remote();
            ws.settings.mobile.enabled = true;
            if let Some(remote) = ws.remote.as_mut() {
                remote.offer = Some(trek_remote::PairingOffer {
                    code: "ABCD-1234".into(),
                    expires_at: trek_core::store::now_ms() + 600_000,
                    url: "trek://pair?host=x&code=ABCD-1234".into(),
                    web_url: Some("https://trekdesktop.com/remote/#p=abc".into()),
                    fingerprint: None,
                });
            }
            ws.open_onboarding(Some(Step::Phone), cx);
        });
        trek.render(cx);
        assert!(trek.visible(cx, "ob-pairing"));
        assert!(trek.visible(cx, ("ob-pair-for", 0usize)), "iPhone or a phone's browser");
        if !cfg!(windows) {
            assert!(!trek.visible(cx, "ob-pair-web-link"), "the iPhone's code first on a Mac");
        }
        trek.click(cx, ("ob-pair-for", 1usize));
        trek.render(cx);
        assert!(trek.visible(cx, "ob-pair-web-link"), "the web remote's link");
    });
}

#[test]
fn an_install_that_ends_is_said_to_have_ended_and_how_it_went() {
    run(async |cx| {
        let trek = open(cx);
        let said = toasts(&trek, cx);
        let line = trek_core::catalog::install_hint("claude-code")
            .unwrap()
            .here();
        // Settings' button stops reading "Installing…" the moment the terminal's command ends.
        let before = std::time::Instant::now();
        trek.update(cx, |ws, cx| {
            assert!(ws.setup_ended.get(line).is_none());
            ws.setup_finished(line, "", cx);
            assert!(ws.setup_ended.get(line).is_some_and(|at| *at >= before));
        });
        // It didn't take: the toast says so.
        trek.update(cx, |ws, cx| {
            ws.agents = vec![detected(AgentId::ClaudeCode, Availability::NotInstalled)];
            ws.say_how_installs_went(cx);
        });
        assert_eq!(
            said.borrow().last().map(|(m, _)| m.as_str()),
            Some("Claude Code didn't install. The terminal shows why.")
        );
        // It took, and wants signing in to; a second time, no repeat of the first.
        trek.update(cx, |ws, cx| {
            ws.setup_finished(line, "", cx);
            ws.agents = vec![detected(AgentId::ClaudeCode, Availability::NeedsLogin)];
            ws.say_how_installs_went(cx);
            ws.say_how_installs_went(cx);
        });
        let said = said.borrow();
        assert_eq!(said.len(), 2, "{said:?}");
        assert_eq!(
            said[1].0,
            "Claude Code is installed. Sign in to start using it."
        );
    });
}

#[test]
fn a_key_the_provider_refuses_says_so_beside_it() {
    run(async |cx| {
        // Mistral: a provider the other key tests leave alone (the store is shared in-process).
        trek_core::settings::secrets::set_api_key("mistral", "sk-wrong").unwrap();
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::ApiKeys), cx)
        });
        trek.render(cx);
        assert!(!trek.visible(cx, "key-check-mistral"), "nothing asked yet");
        // What `save_api_key` starts: the provider is asked (not in an isolated process), and
        // answers.
        trek.update(cx, |ws, _| {
            ws.key_checks.insert("mistral".into(), None);
        });
        trek.render(cx);
        assert!(trek.visible(cx, "key-check-mistral"));
        assert_eq!(
            trek.read(cx, |ws, _| ws.key_check_note("mistral")),
            Some(("Checking…".to_string(), false))
        );
        let said = toasts(&trek, cx);
        trek.update(cx, |ws, cx| {
            ws.key_checked("mistral", trek_agents::KeyCheck::Refused(401), cx)
        });
        trek.render(cx);
        assert_eq!(
            said.borrow().last().map(|(m, _)| m.as_str()),
            Some("Mistral says this key isn't valid. It's saved; paste another to replace it.")
        );
        assert_eq!(
            trek.read(cx, |ws, _| ws.key_check_note("mistral")),
            Some(("Mistral says it isn't valid".to_string(), true))
        );
        // A good one, and one that couldn't be asked.
        trek.update(cx, |ws, cx| {
            ws.key_checked("mistral", trek_agents::KeyCheck::Works(12), cx)
        });
        assert_eq!(
            trek.read(cx, |ws, _| ws.key_check_note("mistral")),
            Some(("Works · 12 models".to_string(), false))
        );
        trek.update(cx, |ws, cx| {
            ws.key_checked(
                "mistral",
                trek_agents::KeyCheck::Unchecked("offline".into()),
                cx,
            )
        });
        assert_eq!(
            trek.read(cx, |ws, _| ws.key_check_note("mistral")),
            Some(("Saved, not checked".to_string(), false))
        );
        // Removed meanwhile: a late answer says nothing.
        trek.update(cx, |ws, cx| {
            ws.remove_api_key("mistral", cx).unwrap();
            ws.key_checked("mistral", trek_agents::KeyCheck::Refused(401), cx)
        });
        assert_eq!(trek.read(cx, |ws, _| ws.key_check_note("mistral")), None);
    });
}
