//! Adding agents beyond the built-in ones (Settings › Agents › Add agent): the ACP Registry's list
//! and its search, and an agent of the user's own command, which then shows on the Agents page
//! and is still known after a restart. The registry is a fixture; nothing is fetched or installed.

use super::harness::{Trek, launch, open, run};
use crate::workspace::{AgentInstall, Route, SettingsPage};
use gpui_kit::TestAppContext;
use trek_core::AgentId;
use trek_core::detect::Availability;
use trek_core::settings::Settings;
use trek_core::store::Store;

/// The registry as if it had just been fetched, and Settings › Agents on screen.
fn on_agents_page(trek: &Trek, cx: &mut TestAppContext) {
    trek.update(cx, |ws, cx| {
        let mut r = trek_core::registry::parse(include_str!("../../../trek-core/fixtures/acp-registry.json")).unwrap();
        r.fetched_at = trek_core::store::now_ms();
        ws.added_agents.registry = Some(r);
        ws.navigate(Route::Settings(SettingsPage::Agents), cx);
    });
    trek.render(cx);
}

/// Open the sheet and let it finish animating in: clicks go where the last frame drew things.
fn open_sheet(trek: &Trek, cx: &mut TestAppContext) {
    trek.click(cx, "add-agent");
    trek.render(cx);
    std::thread::sleep(std::time::Duration::from_millis(400));
    trek.render(cx);
    assert!(trek.visible(cx, "add-agent-sheet"));
}

#[test]
fn the_registry_lists_agents_marks_built_in_ones_and_search_filters_it() {
    run(async |cx| {
        let trek = open(cx);
        on_agents_page(&trek, cx);
        open_sheet(&trek, cx);
        assert!(trek.visible(cx, "registry-row-auggie"));
        assert!(trek.visible(cx, "registry-add-auggie"), "one Trek doesn't have: Add");
        assert!(trek.visible(cx, "registry-built-in-devin"), "Devin is built in: no Add");
        assert!(!trek.visible(cx, "registry-add-devin"));
        assert!(!trek.visible(cx, "registry-row-not-valid"), "entries the format doesn't allow are left out");

        // The search has focus as the sheet opens.
        trek.type_text(cx, "augment");
        trek.render(cx);
        assert!(trek.visible(cx, "registry-row-auggie"));
        assert!(!trek.visible(cx, "registry-row-devin"));
        trek.type_text(cx, "zzz");
        trek.render(cx);
        assert!(trek.visible(cx, "registry-empty"), "nothing matches: it says so");

        // A test process never downloads: the row says why, and offers it again.
        for _ in "augmentzzz".chars() {
            trek.press(cx, "backspace");
        }
        trek.render(cx);
        trek.click(cx, "registry-add-auggie");
        trek.wait(cx, "the install to be turned down", |ws| matches!(ws.added_agents.installs.get("auggie"), Some(AgentInstall::Failed(_)))).await;
        trek.render(cx);
        assert!(trek.visible(cx, "registry-add-auggie"), "Retry");
        assert!(trek.read(cx, |ws, _| ws.settings.added_agents.is_empty()));
    });
}

#[test]
fn a_custom_command_becomes_an_agent_on_the_agents_page_and_survives_a_restart() {
    run(async |cx| {
        let trek = open(cx);
        on_agents_page(&trek, cx);
        open_sheet(&trek, cx);
        trek.click(cx, ("add-agent-tab", 1usize));
        trek.render(cx);
        assert!(trek.visible(cx, "command-form"));

        // Nothing typed: it says what's missing and adds nothing.
        trek.click(cx, "command-add");
        trek.render(cx);
        assert!(trek.visible(cx, "command-error"));

        // The name field has focus on this tab.
        trek.click(cx, "command-name");
        trek.type_text(cx, "Local Bot");
        trek.click(cx, "command-program");
        trek.type_text(cx, "/bin/sh");
        trek.click(cx, "arg-add");
        trek.render(cx);
        trek.type_text(cx, "-c");
        trek.click(cx, "command-add");
        let agent = AgentId::Acp("local-bot".into());
        let key = agent.key();
        let found = agent.clone();
        trek.wait(cx, "the agent to be found", move |ws| ws.agents.iter().any(|a| a.agent == found && a.availability == Availability::Ready)).await;
        trek.render(cx);
        assert!(!trek.visible(cx, "add-agent-sheet"), "the sheet closes");
        assert!(trek.visible(cx, gpui_kit::SharedString::from(format!("enable-{key}"))), "a row on the Agents page");
        let added = trek.read(cx, |ws, _| ws.settings.added_agents.clone());
        assert_eq!(added.len(), 1);
        assert_eq!((added[0].name.as_str(), added[0].command.as_str(), added[0].args.as_slice()), ("Local Bot", "/bin/sh", &["-c".to_string()][..]));
        assert_eq!(agent.display_name(), "Local Bot");

        // Saved: a Trek started afresh (nothing known in this process) knows it by name.
        trek_core::catalog::set_added_agents(&[]);
        assert_eq!(agent.display_name(), "local-bot");
        let (ws2, _, _) = launch(cx, Store::in_memory().unwrap(), Settings::load());
        cx.run_until_parked();
        assert_eq!(agent.display_name(), "Local Bot");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !ws2.read_with(cx, |ws, _| ws.agents.iter().any(|a| a.agent == agent)) {
            assert!(std::time::Instant::now() < deadline, "the restarted Trek finds it");
            cx.run_until_parked();
            cx.background_executor.timer(std::time::Duration::from_millis(5)).await;
        }

        // Removed: gone from settings and the page.
        trek.update(cx, |ws, cx| ws.remove_added_agent("local-bot", cx));
        let gone = agent.clone();
        trek.wait(cx, "the agent to go", move |ws| !ws.agents.iter().any(|a| a.agent == gone)).await;
        trek.render(cx);
        assert!(!trek.visible(cx, gpui_kit::SharedString::from(format!("enable-{key}"))));
        assert!(trek.read(cx, |ws, _| ws.settings.added_agents.is_empty()));
    });
}
