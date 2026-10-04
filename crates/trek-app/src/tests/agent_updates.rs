//! Agent CLI updates: the sidebar's card and Settings > Agents, updates that wait for the agent's
//! running turns, failures with their output, and "Update all". The check itself is
//! `trek_core::agent_update`'s (recorded feeds there); here the found versions are set up
//! directly and updates run through a fake runner, so nothing is installed or fetched.

use super::harness::{Trek, mock, open, run};
use crate::agent_updates::{Job, Runner};
use crate::workspace::{Route, SettingsPage};
use gpui_kit::TestAppContext;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use trek_core::agent_update::{AgentVersion, Install, Outcome, UpdateCommand, mock_versions};
use trek_core::settings::ThemeChoice;

/// MonoCode's three (Codex, OpenCode, Pi) and the mock agent itself, so a thread can be mid-turn
/// in an agent with an update out. Pi's update fails; the rest reach their latest version.
fn found(trek: &Trek, cx: &mut TestAppContext) -> Arc<AtomicUsize> {
    let runs = Arc::new(AtomicUsize::new(0));
    let counted = runs.clone();
    trek.update(cx, |ws, cx| {
        let mut found = mock_versions();
        found.push(AgentVersion {
            agent: mock().key(),
            name: "Mock agent".into(),
            binary: "/usr/local/bin/mock".into(),
            installed: Some("1.0.0".into()),
            latest: Some("1.1.0".into()),
            install: Install::Native,
            command: Some(UpdateCommand { program: "/usr/local/bin/mock".into(), args: vec!["update".into()] }),
            error: None,
        });
        let u = &mut ws.agent_updates;
        u.found = found;
        u.checked_at = trek_core::store::now_ms();
        u.runner = Runner::Fake(Arc::new(move |v: &AgentVersion| {
            counted.fetch_add(1, Ordering::SeqCst);
            match v.agent.as_str() {
                "acp:pi" => Outcome::Failed { summary: "npm install -g @mariozechner/pi-coding-agent@latest stopped with code 243.".into(), output: "npm error code EACCES\nnpm error path /usr/local/lib/node_modules".into() },
                _ => Outcome::Updated { version: v.latest.clone().unwrap(), output: format!("updated {}", v.name) },
            }
        }));
        cx.notify();
    });
    trek.render(cx);
    runs
}

fn job(trek: &Trek, cx: &TestAppContext, agent: &str) -> Option<Job> {
    trek.read(cx, |ws, _| ws.agent_updates.job(agent).cloned())
}

#[test]
fn the_sidebar_card_updates_an_agent_and_shows_a_failure_with_its_output() {
    run(async |cx| {
        let trek = open(cx);
        assert!(!trek.visible(cx, "agent-updates"), "no pill while nothing's out");
        found(&trek, cx);
        assert!(trek.visible(cx, "agent-updates"));
        trek.click(cx, "agent-updates");
        trek.render(cx);
        assert!(trek.visible(cx, "agent-updates-card"));
        for agent in ["codex", "opencode", "acp:pi", "direct:mock"] {
            assert!(trek.visible(cx, gpui_kit::SharedString::from(format!("agent-update-row-{agent}"))), "{agent}");
        }
        trek.click(cx, "agent-update-codex");
        trek.wait(cx, "Codex to update", |ws| matches!(ws.agent_updates.job("codex"), Some(Job::Updated { .. }))).await;
        trek.render(cx);
        assert!(trek.visible(cx, "agent-updated-codex"));
        assert_eq!(trek.read(cx, |ws, _| ws.agent_updates.pending()), 3);
        assert_eq!(job(&trek, cx, "codex"), Some(Job::Updated { from: Some("0.159.2".into()), to: "0.160.0".into(), output: "updated Codex".into() }));

        // A failure says why, keeps what the command printed one click away, and can be retried.
        trek.click(cx, "agent-update-acp:pi");
        trek.wait(cx, "Pi's update to fail", |ws| matches!(ws.agent_updates.job("acp:pi"), Some(Job::Failed { .. }))).await;
        trek.render(cx);
        assert!(trek.visible(cx, "agent-update-acp:pi"), "Retry");
        assert!(!trek.visible(cx, "agent-update-log-acp:pi"));
        trek.click(cx, "agent-update-output-acp:pi");
        trek.render(cx);
        assert!(trek.visible(cx, "agent-update-log-acp:pi"));
        // In Paper too.
        cx.update(|cx| crate::apply_theme(ThemeChoice::Paper, None, cx));
        trek.render(cx);
        assert!(trek.visible(cx, "agent-update-log-acp:pi"));
        assert!(trek.visible(cx, "agent-updated-codex"));
    });
}

#[test]
fn an_update_waits_for_its_agents_running_turn() {
    run(async |cx| {
        let trek = open(cx);
        let runs = found(&trek, cx);
        let id = trek.send(cx, "mock:long 2s");
        let tid = id.clone();
        trek.wait(cx, "the turn to start", |ws| ws.turn_running(&tid)).await;
        trek.update(cx, |ws, cx| ws.update_agent(&mock().key(), cx));
        assert_eq!(job(&trek, cx, "direct:mock"), Some(Job::Queued));
        // The card says it's waiting for the turn.
        trek.click(cx, "agent-updates");
        trek.render(cx);
        assert!(trek.visible(cx, "agent-update-waiting-direct:mock"));
        // Other agents don't wait on it.
        trek.update(cx, |ws, cx| ws.update_agent("opencode", cx));
        trek.wait(cx, "OpenCode to update", |ws| matches!(ws.agent_updates.job("opencode"), Some(Job::Updated { .. }))).await;
        assert_eq!(runs.load(Ordering::SeqCst), 1, "only OpenCode ran");
        assert!(trek.read(cx, |ws, _| ws.turn_running(&id)));
        assert_eq!(job(&trek, cx, "direct:mock"), Some(Job::Queued));
        // The turn ends: the mock agent's update goes.
        trek.wait(cx, "the mock agent to update", |ws| matches!(ws.agent_updates.job("direct:mock"), Some(Job::Updated { .. }))).await;
        assert!(trek.read(cx, |ws, _| !ws.turn_running(&id)));
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        // Its idle session was shut down: the next message starts the updated CLI.
        assert!(trek.read(cx, |ws, _| ws.live[&id].commands.is_none()));
    });
}

#[test]
fn settings_has_the_switch_the_updates_and_update_all() {
    run(async |cx| {
        let trek = open(cx);
        found(&trek, cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::Agents), cx));
        trek.render(cx);
        assert!(trek.visible(cx, "agents-update-count"), "a count beside Agents in the nav");
        assert!(trek.visible(cx, "agent-update-row-codex"));
        assert!(trek.read(cx, |ws, _| ws.settings.updates.check_agents));
        trek.click(cx, "agent-updates-auto");
        assert!(!trek.read(cx, |ws, _| ws.settings.updates.check_agents));
        assert!(!trek.read(cx, |ws, _| ws.agent_updates.due(ws.settings.updates.check_agents, i64::MAX)), "no background checks while off");
        // Update all: one after another, each to its end.
        trek.click(cx, "agent-updates-all");
        trek.wait(cx, "every update to finish", |ws| ws.agent_updates.found.iter().all(|v| matches!(ws.agent_updates.job(&v.agent), Some(Job::Updated { .. } | Job::Failed { .. })))).await;
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.agent_updates.pending()), 1, "Pi failed; the rest are in");
        assert!(trek.visible(cx, "agent-update-acp:pi"), "Retry");
        assert!(!trek.visible(cx, "agent-updates-all"), "nothing left to update all of");
    });
}
