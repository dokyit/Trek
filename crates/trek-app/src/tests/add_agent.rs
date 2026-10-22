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
        let mut r = trek_core::registry::parse(include_str!(
            "../../../trek-core/fixtures/acp-registry.json"
        ))
        .unwrap();
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
        assert!(
            trek.visible(cx, "registry-add-auggie"),
            "one Trek doesn't have: Add"
        );
        assert!(
            trek.visible(cx, "registry-built-in-devin"),
            "Devin is built in: no Add"
        );
        assert!(!trek.visible(cx, "registry-add-devin"));
        assert!(
            !trek.visible(cx, "registry-row-not-valid"),
            "entries the format doesn't allow are left out"
        );

        // The search has focus as the sheet opens.
        trek.type_text(cx, "augment");
        trek.render(cx);
        assert!(trek.visible(cx, "registry-row-auggie"));
        assert!(!trek.visible(cx, "registry-row-devin"));
        trek.type_text(cx, "zzz");
        trek.render(cx);
        assert!(
            trek.visible(cx, "registry-empty"),
            "nothing matches: it says so"
        );

        // A test process never downloads: the row says why, and offers it again.
        for _ in "augmentzzz".chars() {
            trek.press(cx, "backspace");
        }
        trek.render(cx);
        trek.click(cx, "registry-add-auggie");
        trek.wait(cx, "the install to be turned down", |ws| {
            matches!(
                ws.added_agents.installs.get("auggie"),
                Some(AgentInstall::Failed(_))
            )
        })
        .await;
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

        // The name field has focus on this tab. The program is a real binary so it's found.
        let fake = trek_test_fixtures::bin("fake-acp").display().to_string();
        trek.click(cx, "command-name");
        trek.type_text(cx, "Local Bot");
        trek.click(cx, "command-program");
        trek.type_text(cx, &fake);
        trek.click(cx, "arg-add");
        trek.render(cx);
        trek.type_text(cx, "-c");
        trek.click(cx, "command-add");
        let agent = AgentId::Acp("local-bot".into());
        let key = agent.key();
        let found = agent.clone();
        trek.wait(cx, "the agent to be found", move |ws| {
            ws.agents
                .iter()
                .any(|a| a.agent == found && a.availability == Availability::Ready)
        })
        .await;
        trek.render(cx);
        assert!(!trek.visible(cx, "add-agent-sheet"), "the sheet closes");
        assert!(
            trek.visible(cx, gpui_kit::SharedString::from(format!("enable-{key}"))),
            "a row on the Agents page"
        );
        let added = trek.read(cx, |ws, _| ws.settings.added_agents.clone());
        assert_eq!(added.len(), 1);
        assert_eq!(
            (
                added[0].name.as_str(),
                added[0].command.as_str(),
                added[0].args.as_slice()
            ),
            ("Local Bot", fake.as_str(), &["-c".to_string()][..])
        );
        assert_eq!(agent.display_name(), "Local Bot");

        // Saved: a Trek started afresh (nothing known in this process) knows it by name.
        trek_core::catalog::set_added_agents(&[]);
        assert_eq!(agent.display_name(), "local-bot");
        let (ws2, _, _) = launch(cx, Store::in_memory().unwrap(), Settings::load());
        cx.run_until_parked();
        assert_eq!(agent.display_name(), "Local Bot");
        let deadline = std::time::Instant::now() + super::harness::REAL_TIME_LIMIT;
        while !ws2.read_with(cx, |ws, _| ws.agents.iter().any(|a| a.agent == agent)) {
            assert!(
                std::time::Instant::now() < deadline,
                "the restarted Trek finds it"
            );
            cx.run_until_parked();
            cx.background_executor
                .timer(std::time::Duration::from_millis(5))
                .await;
        }

        // Removed: gone from settings and the page.
        trek.update(cx, |ws, cx| ws.remove_added_agent("local-bot", cx));
        let gone = agent.clone();
        trek.wait(cx, "the agent to go", move |ws| {
            !ws.agents.iter().any(|a| a.agent == gone)
        })
        .await;
        trek.render(cx);
        assert!(!trek.visible(cx, gpui_kit::SharedString::from(format!("enable-{key}"))));
        assert!(trek.read(cx, |ws, _| ws.settings.added_agents.is_empty()));
    });
}

#[test]
fn running_in_terminal_from_settings_redirects_and_opens_dock() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(
                crate::workspace::Route::Settings(crate::workspace::SettingsPage::Agents),
                cx,
            )
        });
        trek.render(cx);
        assert!(matches!(
            trek.read(cx, |ws, _| ws.route.clone()),
            crate::workspace::Route::Settings(_)
        ));

        trek.update(cx, |_, cx| {
            cx.emit(crate::workspace::WorkspaceEvent::RunInTerminal {
                command: "echo test".into(),
                cwd: None,
            })
        });
        trek.render(cx);

        assert!(
            !matches!(
                trek.read(cx, |ws, _| ws.route.clone()),
                crate::workspace::Route::Settings(_)
            ),
            "navigates out of settings"
        );
        assert!(trek.visible(cx, "terminal-dock"), "opens terminal dock");
    });
}

#[test]
fn an_agent_removed_while_its_update_installs_stays_removed() {
    use crate::workspace::InstallStep;
    use trek_core::registry::{AddedAgent, AgentSource, Progress};
    run(async |cx| {
        let trek = open(cx);
        let agent = |version: &str| AddedAgent {
            id: "auggie".into(),
            name: "Auggie".into(),
            command: "auggie".into(),
            args: vec![],
            env: vec![],
            source: AgentSource::Registry,
            version: Some(version.into()),
            distribution: Some("binary".into()),
            description: String::new(),
            website: None,
            icon: None,
        };
        trek.update(cx, |ws, cx| ws.add_custom_agent(agent("1.0"), vec![], cx))
            .unwrap();
        // Its update is under way when the user removes it.
        trek.update(cx, |ws, cx| {
            ws.added_agents
                .installs
                .insert("auggie".into(), AgentInstall::Running(None));
            ws.remove_added_agent("auggie", cx);
            ws.install_step("auggie", InstallStep::Progress(Progress::Unpacking), cx);
            ws.install_step("auggie", InstallStep::Done(Ok(agent("1.2"))), cx);
        });
        assert!(trek.read(cx, |ws, _| ws.settings.added_agents.is_empty()
            && ws.added_agents.installs.is_empty()));
        // An update that fails after it's gone says nothing about it either.
        trek.update(cx, |ws, cx| {
            ws.install_step("auggie", InstallStep::Done(Err("offline".into())), cx)
        });
        assert!(trek.read(cx, |ws, _| ws.added_agents.installs.is_empty()));
    });
}
