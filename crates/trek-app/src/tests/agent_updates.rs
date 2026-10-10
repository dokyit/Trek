//! Agent CLI updates: the sidebar's card and Settings > Agents, updates that wait for the agent's
//! running turns, failures with their output, and "Update all". The check itself is
//! `trek_core::agent_update`'s (recorded feeds there); here the found versions are set up
//! directly and updates run through a fake runner, so nothing is installed or fetched.

use super::harness::{Trek, mock, open, open_with, run};
use crate::agent_updates::{Job, Runner};
use crate::workspace::{Route, SettingsPage, UpdateStatus};
use gpui_kit::TestAppContext;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use trek_core::agent_update::{AgentVersion, Outcome, mock_versions};
use trek_core::types::RunState;
use trek_core::settings::{FollowUp, ThemeChoice};

/// MonoCode's three (Codex, OpenCode, Pi) and the mock agent itself, so a thread can be mid-turn
/// in an agent with an update out. Pi's update fails; the rest reach their latest version.
fn found(trek: &Trek, cx: &mut TestAppContext) -> Arc<AtomicUsize> {
    let runs = Arc::new(AtomicUsize::new(0));
    let counted = runs.clone();
    trek.update(cx, |ws, cx| {
        let mut found = mock_versions();
        let row = crate::agent_updates::mock_agent_row();
        assert_eq!(row.agent, mock().key());
        found.push(row);
        let u = &mut ws.agent_updates;
        u.found = found;
        u.checked_at = trek_core::store::now_ms();
        u.runner = Runner::Fake(Arc::new(move |v: &AgentVersion| {
            counted.fetch_add(1, Ordering::SeqCst);
            match v.id.as_str() {
                "pi" => Outcome::Failed { summary: "npm install -g @earendil-works/pi-coding-agent@latest stopped with code 243.".into(), output: "npm error code EACCES\nnpm error path /usr/local/lib/node_modules".into() },
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
        for agent in ["codex", "opencode", "pi", "mock"] {
            assert!(trek.visible(cx, gpui_kit::SharedString::from(format!("agent-update-row-{agent}"))), "{agent}");
        }
        trek.click(cx, "agent-update-codex");
        trek.wait(cx, "Codex to update", |ws| matches!(ws.agent_updates.job("codex"), Some(Job::Updated { .. }))).await;
        trek.render(cx);
        assert!(trek.visible(cx, "agent-updated-codex"));
        assert_eq!(trek.read(cx, |ws, _| ws.agent_updates.pending()), 3);
        assert_eq!(job(&trek, cx, "codex"), Some(Job::Updated { from: Some("0.159.2".into()), to: "0.160.0".into(), output: "updated Codex".into() }));
        // Cached at its new version: the next launch doesn't offer it again.
        let cached = trek_core::agent_update::Snapshot::load();
        assert_eq!(cached.agents.iter().find(|v| v.id == "codex").and_then(|v| v.installed.as_deref()), Some("0.160.0"));

        // A failure says why, keeps what the command printed one click away, and can be retried.
        trek.click(cx, "agent-update-pi");
        trek.wait(cx, "Pi's update to fail", |ws| matches!(ws.agent_updates.job("pi"), Some(Job::Failed { .. }))).await;
        trek.render(cx);
        assert!(trek.visible(cx, "agent-update-pi"), "Retry");
        assert!(!trek.visible(cx, "agent-update-log-pi"));
        trek.click(cx, "agent-update-output-pi");
        trek.render(cx);
        assert!(trek.visible(cx, "agent-update-log-pi"));
        // In Paper too.
        cx.update(|cx| crate::apply_theme(ThemeChoice::Paper, None, cx));
        trek.render(cx);
        assert!(trek.visible(cx, "agent-update-log-pi"));
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
        trek.update(cx, |ws, cx| ws.update_agent("mock", cx));
        assert_eq!(job(&trek, cx, "mock"), Some(Job::Queued));
        // The card says it's waiting for the turn.
        trek.click(cx, "agent-updates");
        trek.render(cx);
        assert!(trek.visible(cx, "agent-update-waiting-mock"));
        // Other agents don't wait on it.
        trek.update(cx, |ws, cx| ws.update_agent("opencode", cx));
        trek.wait(cx, "OpenCode to update", |ws| matches!(ws.agent_updates.job("opencode"), Some(Job::Updated { .. }))).await;
        assert_eq!(runs.load(Ordering::SeqCst), 1, "only OpenCode ran");
        assert!(trek.read(cx, |ws, _| ws.turn_running(&id)));
        assert_eq!(job(&trek, cx, "mock"), Some(Job::Queued));
        // The turn ends: the mock agent's update goes.
        trek.wait(cx, "the mock agent to update", |ws| matches!(ws.agent_updates.job("mock"), Some(Job::Updated { .. }))).await;
        assert!(trek.read(cx, |ws, _| !ws.turn_running(&id)));
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        // Its idle session was shut down as the update started: the next message starts the
        // updated CLI.
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
        // Nor badges for what the last check found: nothing would refresh them. The page still lists it.
        trek.render(cx);
        assert!(!trek.visible(cx, "agents-update-count"));
        assert!(!trek.visible(cx, "agent-updates"), "no sidebar pill");
        assert!(trek.visible(cx, "agent-update-row-codex"));
        // Update all: one after another, each to its end.
        trek.click(cx, "agent-updates-all");
        trek.wait(cx, "every update to finish", |ws| ws.agent_updates.found.iter().all(|v| matches!(ws.agent_updates.job(&v.id), Some(Job::Updated { .. } | Job::Failed { .. })))).await;
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.agent_updates.pending()), 1, "Pi failed; the rest are in");
        assert!(trek.visible(cx, "agent-update-pi"), "Retry");
        assert!(trek.visible(cx, "agent-updates-all"), "one left to retry: still there for it");
        trek.update(cx, |ws, _| ws.agent_updates.runner = Runner::Fake(Arc::new(|v: &AgentVersion| Outcome::Updated { version: v.latest.clone().unwrap(), output: String::new() })));
        trek.click(cx, "agent-updates-all");
        trek.wait(cx, "Pi to update", |ws| matches!(ws.agent_updates.job("pi"), Some(Job::Updated { .. }))).await;
        trek.render(cx);
        assert!(!trek.visible(cx, "agent-updates-all"), "nothing left to update all of");
    });
}

#[test]
fn while_an_agent_updates_no_session_of_it_starts_and_messages_wait() {
    run(async |cx| {
        let trek = open(cx);
        found(&trek, cx);
        let (release, gate) = async_channel::bounded::<()>(1);
        trek.update(cx, |ws, _| ws.agent_updates.runner = Runner::Gated(gate));
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| ws.live[&id].commands.is_some()), "an idle session");

        // It starts at once (nothing's running) and the idle session goes before the files change.
        trek.update(cx, |ws, cx| ws.update_agent("mock", cx));
        assert_eq!(job(&trek, cx, "mock"), Some(Job::Running));
        assert!(trek.read(cx, |ws, _| ws.agent_updating(&mock().key()) && ws.live[&id].commands.is_none()));
        assert!(trek.read(cx, |ws, _| ws.work_in_flight()), "Trek doesn't restart over it");

        // A message waits, and no session starts for it, nor ahead of it.
        trek.update(cx, |ws, cx| ws.send_to(&id, "after the update".into(), vec![], cx));
        trek.update(cx, |ws, cx| ws.warm_up(cx));
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| !ws.turn_running(&id) && ws.live[&id].commands.is_none() && ws.queued(&id) == 1));
        assert!(trek.visible(cx, "composer-agent-updating"), "the composer says why");

        // Done: the message goes, to a session of the new version.
        release.send(()).await.unwrap();
        trek.wait(cx, "the update to finish", |ws| matches!(ws.agent_updates.job("mock"), Some(Job::Updated { .. }))).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| ws.queued(&id) == 0 && !ws.agent_updating(&mock().key())));
        assert!(trek.items(cx, &id).iter().any(|i| matches!(i, trek_core::store::Item::User { text, .. } if text == "after the update")));
        assert!(trek.answers(cx, &id).lines().count() >= 2, "both turns answered: {}", trek.answers(cx, &id));
        trek.render(cx);
        assert!(!trek.visible(cx, "composer-agent-updating"));
    });
}

#[test]
fn a_first_message_waiting_for_its_worktree_goes_once_an_update_that_started_meanwhile_ends() {
    run(async |cx| {
        let trek = open(cx);
        found(&trek, cx);
        super::worktrees::make_repo(&trek, cx);
        super::worktrees::use_worktree(&trek, cx);
        let (release, gate) = async_channel::bounded::<()>(1);
        trek.update(cx, |ws, _| ws.agent_updates.runner = Runner::Gated(gate));
        // The message waits for its worktree; before that's made, the agent's update starts
        // (no turn of it is running).
        trek.ws.update(cx, |ws, cx| {
            ws.send("hello".into(), vec![], cx);
            ws.update_agent("mock", cx);
        });
        let id = trek.thread_id(cx);
        assert_eq!(job(&trek, cx, "mock"), Some(Job::Running));
        let tid = id.clone();
        trek.wait(cx, "the worktree", move |ws| !ws.live[&tid].preparing).await;
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1, "held for the update");
        assert!(trek.read(cx, |ws, _| !ws.turn_running(&id)));
        release.send(()).await.unwrap();
        trek.wait(cx, "the update to finish", |ws| matches!(ws.agent_updates.job("mock"), Some(Job::Updated { .. }))).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 0);
        assert!(!trek.answers(cx, &id).is_empty(), "the agent answered it");
    });
}

#[test]
fn follow_ups_a_failed_turn_left_queued_arent_sent_by_an_update() {
    use trek_agents::AgentEvent;
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        found(&trek, cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TextDelta("Working on it".into())], cx));
        trek.update(cx, |ws, _| ws.live.get_mut(&id).unwrap().queued.push(("stale follow-up".into(), vec![])));
        // Its turn fails off screen: the follow-up waits to be handed back, not sent.
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project) }, cx));
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TurnComplete { error: Some("boom".into()) }], cx));
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1);
        let before = trek.items(cx, &id).len();
        trek.update(cx, |ws, cx| ws.update_agent("mock", cx));
        trek.wait(cx, "the mock agent to update", |ws| matches!(ws.agent_updates.job("mock"), Some(Job::Updated { .. }))).await;
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1, "still the user's to have back");
        assert_eq!(trek.items(cx, &id).len(), before, "nothing went to the agent");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        assert_eq!(trek.composer_text(cx), "stale follow-up");
    });
}

#[test]
fn a_message_sent_during_an_update_goes_without_the_follow_up_a_failed_turn_left() {
    use trek_agents::AgentEvent;
    use trek_core::store::Item;
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        found(&trek, cx);
        let (release, gate) = async_channel::bounded::<()>(1);
        trek.update(cx, |ws, _| ws.agent_updates.runner = Runner::Gated(gate));
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TextDelta("Working on it".into())], cx));
        trek.update(cx, |ws, _| ws.live.get_mut(&id).unwrap().queued.push(("stale follow-up".into(), vec![])));
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project) }, cx));
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TurnComplete { error: Some("boom".into()) }], cx));
        trek.update(cx, |ws, cx| ws.update_agent("mock", cx));
        assert_eq!(job(&trek, cx, "mock"), Some(Job::Running));
        // Opened while the agent updates: the left-over follow-up comes back to the composer.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        assert_eq!(trek.composer_text(cx), "stale follow-up");
        // The user writes something new instead; it waits for the update, and only it goes.
        trek.update(cx, |ws, cx| ws.send_to(&id, "something new instead".into(), vec![], cx));
        assert_eq!(trek.read(cx, |ws, _| ws.live[&id].queued.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>()), ["something new instead"]);
        release.send(()).await.unwrap();
        trek.wait(cx, "the update to finish", |ws| matches!(ws.agent_updates.job("mock"), Some(Job::Updated { .. }))).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        let users: Vec<String> = trek.items(cx, &id).into_iter().filter_map(|i| if let Item::User { text, .. } = i { Some(text) } else { None }).collect();
        assert_eq!(users, ["something new instead"]);
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 0);
    });
}

#[test]
fn a_message_sent_off_screen_during_an_update_goes_without_the_follow_up_a_failed_turn_left() {
    use trek_agents::AgentEvent;
    use trek_core::store::Item;
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        found(&trek, cx);
        let (release, gate) = async_channel::bounded::<()>(1);
        trek.update(cx, |ws, _| ws.agent_updates.runner = Runner::Gated(gate));
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TextDelta("Working on it".into())], cx));
        trek.update(cx, |ws, _| ws.live.get_mut(&id).unwrap().queued.push(("stale follow-up".into(), vec![])));
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project) }, cx));
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TurnComplete { error: Some("boom".into()) }], cx));
        trek.update(cx, |ws, cx| ws.update_agent("mock", cx));
        // A message reaches it while it's off screen (not from its composer).
        trek.update(cx, |ws, cx| ws.send_to(&id, "something new instead".into(), vec![], cx));
        release.send(()).await.unwrap();
        trek.wait(cx, "the update to finish", |ws| matches!(ws.agent_updates.job("mock"), Some(Job::Updated { .. }))).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        let users: Vec<String> = trek.items(cx, &id).into_iter().filter_map(|i| if let Item::User { text, .. } = i { Some(text) } else { None }).collect();
        assert_eq!(users, ["something new instead"], "the left-over follow-up isn't sent");
        // It's still the user's to have back.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        assert_eq!(trek.composer_text(cx), "stale follow-up");
    });
}

#[test]
fn a_check_that_lands_mid_update_doesnt_strand_it() {
    run(async |cx| {
        let trek = open(cx);
        found(&trek, cx);
        let (release, gate) = async_channel::bounded::<()>(1);
        trek.update(cx, |ws, _| ws.agent_updates.runner = Runner::Gated(gate));
        trek.update(cx, |ws, cx| ws.update_agent("codex", cx));
        trek.update(cx, |ws, cx| ws.update_agent("opencode", cx));
        assert_eq!(job(&trek, cx, "codex"), Some(Job::Running));
        // A check comes back while Codex's binary is being swapped: no Codex in it, nor (now
        // uninstalled) OpenCode.
        trek.update(cx, |ws, _| {
            let fresh = ws.agent_updates.found.iter().filter(|v| v.id != "codex" && v.id != "opencode").cloned().collect();
            ws.agent_updates.checked(fresh, trek_core::store::now_ms());
        });
        assert!(trek.read(cx, |ws, _| ws.agent_updates.found.iter().any(|v| v.id == "codex")), "its row stays while it runs");
        assert_eq!(job(&trek, cx, "opencode"), None, "nothing left to update");
        release.send(()).await.unwrap();
        trek.wait(cx, "Codex to update", |ws| matches!(ws.agent_updates.job("codex"), Some(Job::Updated { .. }))).await;
        assert!(trek.read(cx, |ws, _| !ws.agent_updates.running() && !ws.work_in_flight()));
        // Later updates still go.
        release.send(()).await.unwrap();
        trek.update(cx, |ws, cx| ws.update_agent("mock", cx));
        trek.wait(cx, "the mock agent to update", |ws| matches!(ws.agent_updates.job("mock"), Some(Job::Updated { .. }))).await;
    });
}

#[test]
fn titles_wait_while_claude_code_updates() {
    run(async |cx| {
        let trek = open(cx);
        found(&trek, cx);
        let (release, gate) = async_channel::bounded::<()>(1);
        trek.update(cx, |ws, _| {
            let mut claude = crate::agent_updates::mock_agent_row();
            (claude.id, claude.agent, claude.name) = ("claude".into(), trek_core::AgentId::ClaudeCode.key(), "Claude Code".into());
            ws.agent_updates.found.push(claude);
            ws.agent_updates.runner = Runner::Gated(gate);
        });
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.rename(&id, "placeholder".into(), cx));
        trek.update(cx, |ws, cx| ws.update_agent("claude", cx));
        // Claude Code writes titles: not while its CLI is being replaced.
        trek.update(cx, |ws, cx| ws.regenerate_title(&id, true, cx));
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone())).as_deref(), Some("placeholder"));
        assert_eq!(trek.read(cx, |ws, _| ws.agent_updates.titles.clone()), [(id.clone(), true)]);
        release.send(()).await.unwrap();
        trek.wait(cx, "Claude Code to update", |ws| matches!(ws.agent_updates.job("claude"), Some(Job::Updated { .. }))).await;
        assert!(trek.read(cx, |ws, _| ws.agent_updates.titles.is_empty()));
        assert_ne!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone())).as_deref(), Some("placeholder"), "written once it's back");
    });
}

#[test]
fn an_update_waits_for_its_agents_background_work() {
    run(async |cx| {
        let trek = open(cx);
        let runs = found(&trek, cx);
        // Answered, with a dev server left running: replacing the CLI would end it.
        let id = trek.send(cx, "mock:server 800ms");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| !ws.live[&id].background.is_empty() && !ws.turn_running(&id)));
        trek.update(cx, |ws, cx| ws.update_agent("mock", cx));
        assert_eq!(job(&trek, cx, "mock"), Some(Job::Queued));
        trek.click(cx, "agent-updates");
        trek.render(cx);
        assert!(trek.visible(cx, "agent-update-waiting-mock"));
        // The server exits between turns: the update goes then, not before.
        let t = id.clone();
        trek.wait(cx, "the mock agent to update", |ws| matches!(ws.agent_updates.job("mock"), Some(Job::Updated { .. }))).await;
        assert!(trek.read(cx, |ws, _| ws.live[&t].background.is_empty()));
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(trek.read(cx, |ws, _| ws.live[&id].commands.is_none()), "its idle session went before the files changed");
    });
}

#[test]
fn an_update_waiting_on_a_dev_server_doesnt_hold_back_trek_restarting() {
    run(async |cx| {
        let trek = open(cx);
        found(&trek, cx);
        // A dev server with no end, and the agent's update waiting on it.
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.update_agent("mock", cx));
        assert_eq!(job(&trek, cx, "mock"), Some(Job::Queued));
        assert!(!trek.read(cx, |ws, _| ws.work_in_flight()), "it isn't about to start");
        // Trek's own update counts down and goes ahead; the restart ends the server anyway.
        let staged = super::harness::data_dir().join("no-such-update/Trek.app");
        trek.update(cx, |ws, cx| {
            ws.updater.status = UpdateStatus::Ready { version: "9.9.9".into(), staged };
            ws.restart_to_update(cx);
        });
        cx.executor().advance_clock(crate::workspace::RESTART_GRACE);
        cx.run_until_parked();
        let status = trek.read(cx, |ws, _| ws.updater.status.clone());
        assert!(matches!(&status, UpdateStatus::Failed(e) if e.contains(super::harness::INSTALL_BLOCKED)), "{status:?}");
        assert!(trek.read(cx, |ws, _| !ws.live[&id].background.is_empty()), "the server was still running");
    });
}

#[test]
fn a_parent_whose_agent_updated_while_its_sub_agent_reported_still_wakes() {
    run(async |cx| {
        let trek = open(cx);
        found(&trek, cx);
        let (release, gate) = async_channel::bounded::<()>(1);
        trek.update(cx, |ws, _| ws.agent_updates.runner = Runner::Gated(gate));
        let id = trek.send(cx, "mock:delegate mock:long 600ms");
        let p = id.clone();
        trek.wait(cx, "the parent's answer", move |ws| ws.live[&p].turn_started.is_none() && !ws.children(&p).is_empty()).await;
        // Asked for while the sub-agent (the same agent) works: it starts as that turn ends, so
        // the report arrives while the agent's CLI is being replaced and has to wait.
        trek.update(cx, |ws, cx| ws.update_agent("mock", cx));
        assert_eq!(job(&trek, cx, "mock"), Some(Job::Queued));
        trek.wait(cx, "the update to start", |ws| matches!(ws.agent_updates.job("mock"), Some(Job::Running))).await;
        let p = id.clone();
        trek.wait(cx, "the report to be held", move |ws| ws.wakes_held(&p)).await;
        assert!(trek.read(cx, |ws, _| ws.waiting(&id) && matches!(ws.agent_updates.job("mock"), Some(Job::Running))), "still waiting, its report held");
        assert!(!trek.items(cx, &id).iter().any(|i| matches!(i, trek_core::store::Item::User { text, .. } if trek_core::orchestrate::is_wake(text))));
        // Done: the report goes to a session of the new version.
        release.send(()).await.unwrap();
        let p = id.clone();
        trek.wait(cx, "the wake-up", move |ws| ws.live[&p].items.iter().any(|i| matches!(i, trek_core::store::Item::User { text, .. } if trek_core::orchestrate::is_wake(text)))).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(!trek.read(cx, |ws, _| ws.waiting(&id)));
        assert!(trek.read(cx, |ws, _| ws.store.held_reports().unwrap()).is_empty());
    });
}
