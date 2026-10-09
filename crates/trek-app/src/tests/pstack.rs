//! pstack-style workflows: restating a request before any work, a design arena across model
//! families with a blind judge, and a verification skill per project that every agent is told
//! about. The mock agent plays every side.

use super::harness::{Trek, open, run};
use crate::workspace::{Route, SettingsPage, TaskState};
use gpui_kit::TestAppContext;
use trek_core::orchestrate::{Style, split_consult};
use trek_core::store::Item;
use trek_core::RunState;

/// The last message the user sent in `id`.
fn last_message(trek: &Trek, cx: &TestAppContext, id: &str) -> String {
    trek.items(cx, id).into_iter().rev().find_map(|i| if let Item::User { text, .. } = i { Some(text) } else { None }).unwrap()
}

fn restating(trek: &Trek, cx: &TestAppContext) -> bool {
    cx.read(|cx| trek.root.read(cx).composer.read(cx).restating())
}

fn last_end(trek: &Trek, cx: &TestAppContext, id: &str) -> usize {
    trek.items(cx, id).iter().rposition(|i| matches!(i, Item::TurnEnd { .. })).expect("a finished turn")
}

/// `find <dir> -exec touch -t 202001010000 {} +`: every file under it reads as from 2020.
fn age(dir: &std::path::Path) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            age(&path);
        } else {
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_577_836_800)).unwrap();
        }
    }
}

#[test]
fn restate_first_says_it_back_then_goes_ahead_on_a_yes() {
    run(async |cx| {
        let trek = open(cx);
        // `/restate` alone turns it on for the next message (picked from the commands, then
        // sent); a pill says so.
        trek.type_text(cx, "/restate");
        trek.press(cx, "enter");
        assert_eq!(trek.composer_text(cx), "/restate ", "picked");
        trek.press(cx, "enter");
        trek.render(cx);
        assert!(restating(&trek, cx) && trek.composer_text(cx).is_empty());
        assert!(trek.visible(cx, "restate-pill"));

        let id = trek.send(cx, "Fix the flaky upload test.");
        let sent = last_message(&trek, cx, &id);
        assert_eq!(trek_core::restate::split_restate(&sent), ("Fix the flaky upload test.", true));
        assert!(!restating(&trek, cx), "only for that message");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone())), Some("Fix the flaky upload test.".into()));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("you'd like me to fix the flaky upload test."));
        // The message shows as written, noting the request; the turn asks for a yes or a correction.
        let user = trek.item_ix(cx, &id, |i| matches!(i, Item::User { .. }));
        let end = last_end(&trek, cx, &id);
        trek.render(cx);
        assert!(trek.visible(cx, ("restating", user)));
        assert!(trek.visible(cx, ("restate-yes", end)) && trek.visible(cx, ("restate-no", end)));
        assert!(!trek.read(cx, |ws, _| ws.transcript_markdown(&id)).contains("trek-restate"), "copied as written");

        trek.click(cx, ("restate-yes", end));
        assert_eq!(last_message(&trek, cx, &id), trek_core::restate::GO_AHEAD);
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        assert!(!trek.visible(cx, ("restate-yes", end)), "answered");
        assert!(!trek.visible(cx, ("restate-yes", last_end(&trek, cx, &id))), "the go-ahead's turn asks nothing");
    });
}

#[test]
fn not_quite_puts_the_correction_in_the_composer_restated_too() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "/restate Make the sidebar faster");
        assert!(trek_core::restate::split_restate(&last_message(&trek, cx, &id)).1);
        trek.wait_done(cx, &id, RunState::Idle).await;
        let end = last_end(&trek, cx, &id);
        trek.render(cx);
        trek.click(cx, ("restate-no", end));
        assert!(restating(&trek, cx), "the correction is restated too");
        trek.send(cx, "Not the sidebar: the inbox search.");
        assert_eq!(trek_core::restate::split_restate(&last_message(&trek, cx, &id)), ("Not the sidebar: the inbox search.", true));
        trek.wait_done(cx, &id, RunState::Idle).await;
        // It works the same in a window of its own.
        let window = trek.open_thread_window(cx, &id);
        let end = last_end(&trek, cx, &id);
        assert!(trek.visible_in(cx, window, ("restate-yes", end)));
    });
}

#[test]
fn an_arena_drafts_designs_across_families_and_has_them_judged() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "consult-pill");
        trek.click(cx, ("consult-style", 2usize));
        // One candidate per model family on offer (the mock and the relay mock), and a judge of
        // another family than the thread's own.
        let picked = cx.read(|cx| trek.root.read(cx).composer.read(cx).consultants()).0;
        assert_eq!(picked, ["direct:mock/mock-swift/high", "direct:mock-relay/relay-swift/high"]);
        let judge = cx.read(|cx| trek.root.read(cx).composer.read(cx).judge(cx));
        assert_eq!(judge.as_deref(), Some("direct:mock-relay/relay-swift/high"));
        trek.render(cx);
        assert!(trek.visible(cx, "consult-judge"));
        trek.click(cx, ("consult-then", 1usize));
        trek.press(cx, "escape");

        let id = trek.send(cx, "Rate-limit the webhooks");
        let sent = last_message(&trek, cx, &id);
        let (said, consult) = split_consult(&sent);
        let consult = consult.expect("the arena went with it");
        assert_eq!((said, consult.style, consult.implement), ("Rate-limit the webhooks", Style::Arena, false));
        assert_eq!(consult.judge.map(|j| j.key()).as_deref(), Some("direct:mock-relay/relay-swift/high"));
        trek.wait_done(cx, &id, RunState::Idle).await;
        // As agents whose tool calls run one at a time do it: both designs start without waiting
        // and are collected with `task_result`, which waits. Read in the turn, their answers don't
        // wake the thread again after it.
        let items = trek.items(cx, &id);
        let calls: Vec<&str> = items.iter().filter_map(|i| if let Item::Tool { title, .. } = i { title.split_once("__").and_then(|(_, t)| t.split_once("__")).map(|(_, t)| t) } else { None }).collect();
        assert_eq!(calls, ["delegate_task", "delegate_task", "task_result", "task_result", "delegate_task"]);
        assert!(!items.iter().any(|i| matches!(i, Item::User { text, .. } if trek_core::orchestrate::is_wake(text))));

        // Two designs, then the judge: sub-agents of the thread, each on its own model.
        let mut kids: Vec<(String, String, String)> = trek.read(cx, |ws, _| ws.children(&id).into_iter().map(|t| (t.id.clone(), t.title.clone(), t.model.clone().unwrap_or_default())).collect());
        // The designs start together, in no set order.
        kids.sort_by(|a, b| a.1.cmp(&b.1));
        let titles: Vec<(&str, &str)> = kids.iter().map(|(_, t, m)| (t.as_str(), m.as_str())).collect();
        assert_eq!(titles, [("Design A", "mock-swift"), ("Design B", "relay-swift"), ("Judge the designs", "relay-swift")]);
        for (kid, _, _) in &kids {
            assert_eq!(trek.read(cx, |ws, _| ws.task_state(kid)), TaskState::Done);
        }
        let judged = trek.items(cx, &kids[2].0).into_iter().find_map(|i| if let Item::User { text, .. } = i { Some(text) } else { None }).unwrap();
        assert!(judged.contains("Design A:") && judged.contains("Design B:") && !judged.contains("mock-swift"), "judged blind: {judged}");
        assert!(trek.answers(cx, &id).contains("Design B won"), "{}", trek.answers(cx, &id));
        assert!(trek.answers(cx, &id).contains("I haven't changed any files"));
        let rows = trek.rows(cx);
        assert_eq!(rows.iter().filter(|r| r.starts_with("subagent:")).count(), 3, "{rows:?}");
        assert!(rows.contains(&"subagent: Relay Swift: Judge the designs (Done)".to_string()), "{rows:?}");
        // The agent waited for every answer in its turn: no report comes back to wake it again.
        cx.run_until_parked();
        assert!(trek.items(cx, &id).iter().all(|i| !matches!(i, Item::User { text, .. } if trek_core::orchestrate::is_wake(text))), "{:?}", trek.items(cx, &id));
        assert!(trek.read(cx, |ws, _| ws.store.held_reports().unwrap()).is_empty(), "nor after a restart");
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_some_and(|t| t.run_state == RunState::Idle)));
        // The message shows who designs and who judges.
        let user = trek.item_ix(cx, &id, |i| matches!(i, Item::User { .. }));
        cx.simulate_window_resize(trek.window, gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(2400.)));
        trek.render(cx);
        assert!(trek.visible(cx, ("consulting", user)));
        // A design's row is a sub-agent row like any other: it opens on its package.
        let design = trek.item_ix(cx, &id, |i| matches!(i, Item::Tool { id, detail, .. } if trek_core::orchestrate::task_of_row(id).is_some() && detail.contains("Design A")));
        assert!(trek.visible(cx, ("subagent-open", design)));
        trek.click(cx, ("subagent", design));
        trek.render(cx);
        let drawn = trek.drawn_markdown(cx);
        assert!(drawn.iter().any(|(ix, md)| *ix == design && md.contains("Core types and signatures")), "{drawn:?}");
    });
}

#[test]
fn restating_an_arena_holds_it_until_the_go_ahead() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "consult-pill");
        trek.click(cx, ("consult-style", 2usize));
        trek.press(cx, "escape");
        let id = trek.send(cx, "/restate Rate-limit the webhooks");
        let sent = last_message(&trek, cx, &id);
        assert!(split_consult(&sent).1.is_some_and(|c| c.style == Style::Arena) && trek_core::restate::split_restate(split_consult(&sent).0).1);
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| ws.children(&id).is_empty()), "only restated: no designs yet");
        assert!(cx.read(|cx| trek.root.read(cx).composer.read(cx).consultants()).0.is_empty(), "the picks went with the message");

        // The go-ahead carries the arena that waited for it.
        let end = last_end(&trek, cx, &id);
        trek.render(cx);
        trek.click(cx, ("restate-yes", end));
        let ahead = last_message(&trek, cx, &id);
        let (said, consult) = split_consult(&ahead);
        assert_eq!((said, consult.map(|c| c.style)), (trek_core::restate::GO_AHEAD, Some(Style::Arena)));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let titles: Vec<String> = trek.read(cx, |ws, _| ws.children(&id).into_iter().map(|t| t.title.clone()).collect());
        assert!(titles.iter().any(|t| t == "Judge the designs") && titles.iter().filter(|t| t.starts_with("Design ")).count() == 2, "{titles:?}");
        assert!(trek.answers(cx, &id).contains("Design B won"), "{}", trek.answers(cx, &id));
    });
}

#[test]
fn a_corrected_restatement_keeps_the_arena_for_the_go_ahead() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "consult-pill");
        trek.click(cx, ("consult-style", 2usize));
        trek.press(cx, "escape");
        let id = trek.send(cx, "/restate Rate-limit the webhooks");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // Not quite: the correction is restated, without the arena (its picks went with the
        // first message)…
        trek.render(cx);
        trek.click(cx, ("restate-no", last_end(&trek, cx, &id)));
        trek.send(cx, "Per customer, not per endpoint.");
        let correction = last_message(&trek, cx, &id);
        assert!(split_consult(&correction).1.is_none() && trek_core::restate::split_restate(&correction).1);
        trek.wait_done(cx, &id, RunState::Idle).await;
        // …and the go-ahead brings it back.
        trek.render(cx);
        trek.click(cx, ("restate-yes", last_end(&trek, cx, &id)));
        let ahead = last_message(&trek, cx, &id);
        assert_eq!(split_consult(&ahead).1.map(|c| c.style), Some(Style::Arena), "{ahead}");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| ws.children(&id).iter().any(|t| t.title == "Judge the designs")));
    });
}

#[test]
fn restate_alone_in_a_thread_under_way_restates_the_thread() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "Fix the flaky upload test.");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.type_text(cx, "/restate");
        trek.press(cx, "enter");
        trek.press(cx, "enter");
        let sent = last_message(&trek, cx, &id);
        assert_eq!(trek_core::restate::split_restate(&sent), (trek_core::restate::THREAD, true));
        assert!(!restating(&trek, cx) && trek.composer_text(cx).is_empty());
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("you'd like me to look at what we have so far"), "{}", trek.answers(cx, &id));
        trek.render(cx);
        assert!(trek.visible(cx, ("restate-yes", last_end(&trek, cx, &id))));
    });
}

#[test]
fn an_arena_drafts_no_more_designs_than_run_at_once() {
    run(async |cx| {
        let trek = open(cx);
        // Five designs can't all run at once: the command stays in the composer, unsent.
        let five = "/consult arena swift low, swift high, deep low, deep high, relay: Rate-limit the webhooks";
        trek.type_text(cx, five);
        trek.press(cx, "enter");
        assert_eq!(trek.composer_text(cx), five);
        assert!(trek.read(cx, |ws, _| ws.threads.iter().all(|t| !t.title.contains("Rate-limit"))), "not sent");
        // Four can.
        trek.window(cx, |window, cx| trek.root.read(cx).composer.clone().update(cx, |c, cx| c.set_text("", window, cx)));
        trek.type_text(cx, "/consult arena swift low, swift high, deep low, relay");
        trek.press(cx, "enter");
        let picked = cx.read(|cx| trek.root.read(cx).composer.read(cx).consultants()).0;
        assert_eq!(picked.len(), trek_core::orchestrate::MAX_RUNNING, "{picked:?}");

        // Five picked for advice, then an arena: the first four draft.
        let clear = |trek: &Trek, cx: &mut TestAppContext| trek.window(cx, |window, cx| trek.root.read(cx).composer.clone().update(cx, |c, cx| c.set_text("", window, cx)));
        trek.press(cx, "escape");
        clear(&trek, cx);
        trek.type_text(cx, "/consult advise swift low, swift high, deep low, deep high, relay");
        trek.press(cx, "enter");
        assert_eq!(cx.read(|cx| trek.root.read(cx).composer.read(cx).consultants()).0.len(), 5);
        trek.press(cx, "escape");
        clear(&trek, cx);
        let id = trek.send(cx, "/consult arena: Rate-limit the webhooks");
        let consult = split_consult(&last_message(&trek, cx, &id)).1.expect("an arena");
        assert_eq!((consult.style, consult.consultants.len()), (Style::Arena, trek_core::orchestrate::MAX_RUNNING));

        // One design isn't an arena: the message stays in the composer, unsent.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        clear(&trek, cx);
        let one = "/consult arena relay: Cache the avatars";
        trek.type_text(cx, one);
        trek.press(cx, "enter");
        assert_eq!(trek.composer_text(cx), one);
        assert!(trek.read(cx, |ws, _| ws.threads.iter().all(|t| !t.title.contains("avatars"))), "not sent");
    });
}

#[test]
fn a_skill_without_a_cli_says_why_no_turn_is_verified() {
    run(async |cx| {
        let trek = open(cx);
        let project = trek.project.clone();
        // Found by its name and Feature Map, with no CLI named or bundled.
        let skill = project.join(".claude/skills/verify-app");
        std::fs::create_dir_all(skill.join("references/features")).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: verify-app\ndescription: Check the app.\n---\n").unwrap();
        std::fs::write(skill.join("references/features/README.md"), "# Features\n").unwrap();
        trek.update(cx, |ws, cx| ws.refresh_verification(&project, cx));
        assert!(trek.read(cx, |ws, _| ws.verification(&project)).is_some_and(|v| v.cli.is_none()));
        let pid = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == project).map(|p| p.id.clone()));
        trek.update(cx, |ws, cx| ws.open_project_settings(pid, cx));
        cx.simulate_window_resize(trek.window, gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(1800.)));
        trek.render(cx);
        assert!(trek.visible(cx, "verify-no-cli") && !trek.visible(cx, "verify-cli"));
        // Maintain asks for one.
        trek.click(cx, "verify-maintain");
        let id = trek.thread_id(cx);
        assert!(last_message(&trek, cx, &id).contains("names no CLI yet"));
    });
}

#[test]
fn a_thread_in_a_worktree_verifies_its_own_folder() {
    run(async |cx| {
        let trek = open(cx);
        super::worktrees::make_repo(&trek, cx);
        let project = trek.project.clone();
        // A skill made in the project folder and not committed yet.
        let skill = project.join(".agents/skills/verify-app");
        std::fs::create_dir_all(skill.join("scripts")).unwrap();
        std::fs::create_dir_all(skill.join("references/features")).unwrap();
        // Windows has no execute bit: a program is a file with a program's extension (app.cmd).
        let script = if cfg!(windows) { "app.cmd" } else { "app" };
        let cli = format!("./.agents/skills/verify-app/scripts/{script}");
        let program = if cfg!(windows) { "@echo off\r\necho ok\r\n" } else { "#!/bin/sh\necho ok\n" };
        std::fs::write(skill.join("SKILL.md"), format!("---\nname: verify-app\nmetadata:\n  trek: verification\n  cli: {cli}\n---\n")).unwrap();
        std::fs::write(skill.join("references/features/README.md"), "# Features\n").unwrap();
        std::fs::write(skill.join("scripts").join(script), program).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(skill.join("scripts").join(script), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        trek.update(cx, |ws, cx| ws.refresh_verification(&project, cx));
        assert!(trek.read(cx, |ws, _| ws.verification(&project)).is_some());

        // The worktree has no copy: the agent reads the main checkout's, and runs its CLI by its
        // full path from the worktree, where its changes are.
        super::worktrees::use_worktree(&trek, cx);
        let id = trek.send(cx, "mock:verify the notes change");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let wt = trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.worktree.clone())).expect("a worktree");
        assert!(!wt.path.join(".agents").exists());
        let full = skill.join("scripts").join(script);
        assert!(trek.answers(cx, &id).contains(&format!("verified it with `{} check`", full.display())), "{}", trek.answers(cx, &id));
        let end = last_end(&trek, cx, &id);
        trek.render(cx);
        assert!(trek.visible(cx, ("verified", end)));

        // Committed, a new worktree has its own copy, and runs that.
        trek_core::worktree::git(&project, &["add", "-A"]).unwrap();
        trek_core::worktree::git(&project, &["commit", "-qm", "Add the verification skill"]).unwrap();
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project.clone()) }, cx));
        super::worktrees::use_worktree(&trek, cx);
        let id = trek.send(cx, "mock:verify the notes change");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.worktree.clone())).is_some_and(|w| w.path.join(".agents/skills/verify-app/SKILL.md").exists()));
        assert!(trek.answers(cx, &id).contains(&format!("verified it with `{cli} check`")), "{}", trek.answers(cx, &id));
    });
}

#[test]
fn a_project_s_verification_skill_is_set_up_told_to_agents_and_maintained() {
    run(async |cx| {
        let trek = open(cx);
        let project = trek.project.clone();
        let pid = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == project).map(|p| p.id.clone())).expect("project");
        trek.update(cx, |ws, cx| ws.open_project_settings(Some(pid.clone()), cx));
        cx.simulate_window_resize(trek.window, gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(1800.)));
        trek.render(cx);
        assert!(trek.visible(cx, "verify-setup"));
        assert!(!trek.visible(cx, "verify-maintain"));

        // Set up: a thread in the project, following the guide Trek ships, with the user's
        // default agent (not one the project picked for its threads).
        let relay = trek_core::AgentId::Direct(trek_core::catalog::MOCK_RELAY_PROVIDER.into());
        trek.update(cx, |ws, cx| ws.update_project_prefs(&project, |p| p.agent = Some(relay.key()), cx));
        trek.click(cx, "verify-setup");
        let id = trek.thread_id(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.agent.clone())), Some(super::harness::mock()));
        trek.update(cx, |ws, cx| ws.update_project_prefs(&project, |p| p.agent = None, cx));
        let asked = last_message(&trek, cx, &id);
        let guide = trek_core::skills::shipped_root().join(trek_core::skills::CREATE_VERIFICATION).join("SKILL.md");
        assert!(asked.contains(&guide.display().to_string()) && guide.exists(), "{asked}");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.cwd.clone())), Some(project.clone()), "in the project folder itself");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let v = trek.read(cx, |ws, _| ws.verification(&project)).expect("recorded");
        assert_eq!((v.name.as_str(), v.cli.as_deref()), ("verify-app", Some("./.agents/skills/verify-app/scripts/app")));
        assert!(std::path::Path::new(&v.skill).join("references/features/README.md").exists());
        let first = v.maintained_at.expect("when it was made");

        // Settings says it's ready, with Maintain and the reminder.
        trek.update(cx, |ws, cx| ws.open_project_settings(Some(pid.clone()), cx));
        trek.render(cx);
        assert!(trek.visible(cx, "verify-maintain") && trek.visible(cx, "verify-remind") && !trek.visible(cx, "verify-setup"));

        // A new thread's agent is told about it, runs its CLI, and the turn is marked Verified.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project.clone()) }, cx));
        let id = trek.send(cx, "mock:verify the notes change");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("verified it with `./.agents/skills/verify-app/scripts/app check`"), "{}", trek.answers(cx, &id));
        let end = last_end(&trek, cx, &id);
        trek.render(cx);
        assert!(trek.visible(cx, ("verified", end)));
        // A turn that didn't run it isn't.
        trek.send(cx, "explain the startup");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        assert!(!trek.visible(cx, ("verified", last_end(&trek, cx, &id))));
        // A change the CLI catches: its run really fails, and the turn says so.
        trek.send(cx, "mock:verify broken");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        let end = last_end(&trek, cx, &id);
        assert!(trek.visible(cx, ("verify-failed", end)) && !trek.visible(cx, ("verified", end)));
        assert!(trek.answers(cx, &id).contains("`./.agents/skills/verify-app/scripts/app check` fails"));
        // Fixed, it passes again.
        trek.send(cx, "mock:verify the fix");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        assert!(trek.visible(cx, ("verified", last_end(&trek, cx, &id))));

        // Maintain runs the maintain guide, and counts as maintenance.
        trek.update(cx, |ws, cx| ws.open_project_settings(Some(pid.clone()), cx));
        trek.render(cx);
        trek.click(cx, "verify-maintain");
        let id = trek.thread_id(cx);
        assert!(last_message(&trek, cx, &id).contains(trek_core::skills::MAINTAIN_VERIFICATION));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let v = trek.read(cx, |ws, _| ws.verification(&project)).unwrap();
        assert!(v.maintained_at.unwrap() >= first);
        assert!(std::fs::read_to_string(std::path::Path::new(&v.skill).join("references/features/README.md")).unwrap().contains("settings.md"));

        // A Maintain run counts once it has changed the skill or run its CLI: one that only read
        // it (here it found no Feature Map to work on) doesn't, nor does a later turn that leaves
        // it alone.
        let old = trek_core::store::now_ms() - 3 * trek_core::verification::WEEK_MS;
        age(std::path::Path::new(&v.skill));
        trek.update(cx, |ws, cx| ws.update_project_prefs(&project, |p| p.verification.iter_mut().for_each(|v| v.maintained_at = Some(old)), cx));
        let map = std::path::Path::new(&v.skill).join("references/features/README.md");
        std::fs::rename(&map, map.with_extension("md.away")).unwrap();
        trek.update(cx, |ws, cx| ws.open_project_settings(Some(pid.clone()), cx));
        trek.render(cx);
        trek.click(cx, "verify-maintain");
        let id = trek.thread_id(cx);
        trek.wait_done(cx, &id, RunState::Idle).await;
        let maintained = |trek: &super::harness::Trek, cx: &TestAppContext| trek.read(cx, |ws, _| ws.verification(&project).and_then(|v| v.maintained_at));
        assert!(trek.answers(cx, &id).contains("no verification skill here"));
        assert_eq!(maintained(&trek, cx), Some(old), "it didn't touch the skill");
        std::fs::rename(map.with_extension("md.away"), &map).unwrap();
        trek.send(cx, "thanks");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(maintained(&trek, cx), Some(old), "a turn that left it alone");

        // The weekly reminder: once it's a week old, once a week.
        trek.update(cx, |ws, cx| ws.open_project_settings(Some(pid.clone()), cx));
        trek.render(cx);
        trek.click(cx, "verify-remind");
        let now = trek_core::store::now_ms();
        let week = trek_core::verification::WEEK_MS;
        trek.update(cx, |ws, cx| ws.update_project_prefs(&project, |p| p.verification.iter_mut().for_each(|v| v.maintained_at = Some(now - week - 1)), cx));
        let toasts = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
        let seen = toasts.clone();
        let _sub = cx.update(|cx| {
            cx.subscribe(&trek.ws, move |_, e: &crate::workspace::WorkspaceEvent, _| {
                if let crate::workspace::WorkspaceEvent::Toast { message, undo } = e {
                    seen.borrow_mut().push((message.clone(), matches!(undo, Some(crate::workspace::UndoAction::MaintainVerification(_)))));
                }
            })
        });
        trek.update(cx, |ws, cx| ws.remind_verification(now, cx));
        trek.update(cx, |ws, cx| ws.remind_verification(now + 60_000, cx));
        let name = project.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(*toasts.borrow(), [(format!("{name}'s verification skill was last maintained 7 days ago."), true)]);
        assert_eq!(trek.read(cx, |ws, _| ws.verification(&project).and_then(|v| v.reminded_at)), Some(now));

        // A skill that's gone is forgotten; back (a branch with it checked out again), it has the
        // user's choices again.
        let kept = trek.read(cx, |ws, _| ws.verification(&project)).unwrap();
        let aside = project.with_extension("skill-aside");
        std::fs::rename(&v.skill, &aside).unwrap();
        trek.update(cx, |ws, cx| ws.refresh_verification(&project, cx));
        assert_eq!(trek.read(cx, |ws, _| ws.verification(&project)), None);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Settings(SettingsPage::Project));
        std::fs::rename(&aside, &v.skill).unwrap();
        trek.update(cx, |ws, cx| ws.refresh_verification(&project, cx));
        cx.run_until_parked();
        let back = trek.read(cx, |ws, _| ws.verification(&project)).expect("found again");
        assert_eq!((back.remind_weekly, back.maintained_at, back.reminded_at), (true, kept.maintained_at, kept.reminded_at));
        // A project folder that isn't there (a drive not mounted) says nothing about its skill.
        let away = project.with_extension("unmounted");
        trek.update(cx, |ws, cx| ws.update_project_prefs(&away, |p| p.verification = Some(kept.clone()), cx));
        trek.update(cx, |ws, cx| ws.refresh_verification(&away, cx));
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.verification(&away)), Some(kept));
    });
}

/// Restate first and the project notes with a real agent: Claude Code (claude-haiku-4-5, low
/// effort) restates a request without touching anything, and a new thread in a project with a
/// verification skill knows its CLI and reads Trek's guides without asking. Not run by default
/// (two tiny turns):
/// `TREK_LIVE_AGENT=claude cargo test -p trek-app live_restate -- --ignored`. Works in
/// /tmp/trek-pstack-e2e; remove ~/.claude/projects/-private-tmp-trek-pstack-e2e afterwards.
#[test]
#[ignore = "live: runs a real agent"]
fn live_restate_first_and_project_notes() {
    use std::time::{Duration, Instant};
    run(async |cx| {
        let trek = super::harness::open_with(cx, |s| {
            s.general.default_agent = trek_core::AgentId::ClaudeCode.key();
            s.general.default_model = Some("claude-haiku-4-5".into());
            s.general.default_effort = trek_core::Effort::Low;
        });
        let project = std::path::PathBuf::from("/tmp/trek-pstack-e2e");
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(project.join(".agents/skills/control-app")).unwrap();
        std::fs::write(project.join("README.md"), "# Notes\n\nA tiny notes app.\n").unwrap();
        std::fs::write(
            project.join(".agents/skills/control-app/SKILL.md"),
            "---\nname: control-app\ndescription: Drive and verify the notes app.\nmetadata:\n  trek: verification\n  cli: ./tools/notesctl\n---\n\nRun `./tools/notesctl check`.\n",
        )
        .unwrap();
        trek.update(cx, |ws, cx| {
            ws.store.ensure_project(&project).unwrap();
            ws.reload(cx);
            ws.navigate(Route::Draft { project: Some(project.clone()) }, cx);
        });
        assert!(trek.read(cx, |ws, _| ws.verification(&project)).is_some(), "found as the draft opened");
        let asked_any = std::cell::Cell::new(false);
        let done = async |cx: &mut TestAppContext, id: &str| {
            let deadline = Instant::now() + Duration::from_secs(180);
            loop {
                cx.run_until_parked();
                let (state, running, asked) = trek.read(cx, |ws, _| (ws.thread(id).map(|t| t.run_state), ws.turn_running(id), ws.pending_request(id).map(|p| p.request_id.clone())));
                // Anything it asks to do is declined: restating needs nothing done.
                if let Some(rid) = asked {
                    asked_any.set(true);
                    trek.update(cx, |ws, cx| ws.respond(id, &rid, trek_agents::Decision::Deny, cx));
                }
                match (state, running) {
                    (Some(RunState::Idle), false) => return,
                    (Some(RunState::Failed), _) => panic!("the turn failed: {:?}", trek.items(cx, id)),
                    _ => {}
                }
                assert!(Instant::now() < deadline, "timed out");
                cx.background_executor.timer(Duration::from_millis(50)).await;
            }
        };
        let id = trek.send(cx, "/restate The notes app loses the last edit when its window closes. Fix it.");
        done(cx, &id).await;
        let answer = trek.answers(cx, &id);
        println!("restated: {answer}");
        assert!(!answer.trim().is_empty());
        let changed = trek.items(cx, &id).iter().any(|i| matches!(i, Item::Tool { title, .. } if title.starts_with("Edit") || title.starts_with("Wr")));
        assert!(!changed, "nothing changed before the go-ahead");
        assert!(!project.join("src").exists() && std::fs::read_dir(&project).unwrap().count() == 2, "the folder is as it was");
        let end = last_end(&trek, cx, &id);
        trek.render(cx);
        assert!(trek.visible(cx, ("restate-yes", end)));

        // A new thread: the project's verification skill reached Claude Code's system prompt, and
        // Trek's guides (outside the project) are read without asking.
        let guide = trek_core::skills::shipped(trek_core::skills::CREATE_VERIFICATION).unwrap();
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project.clone()) }, cx));
        asked_any.set(false);
        let id = trek.send(
            cx,
            &format!("Without running anything: what command runs this project's verification CLI, as you were told? Then read {} and give its `name:` line. Reply with just those two lines.", guide.display()),
        );
        done(cx, &id).await;
        let answer = trek.answers(cx, &id);
        println!("told: {answer}");
        assert!(answer.contains("notesctl") && answer.contains("create-verification-skill"), "{answer}");
        assert!(!asked_any.get(), "reading the guide needed no permission");
    });
}

/// A design arena with real agents: Claude Code (claude-haiku-4-5, low effort) grounds a tiny
/// problem and runs the arena through `delegate_task`, with designs from itself and Codex
/// (gpt-5.6-luna, low) and Codex judging; then it reports. Not run by default (a handful of tiny
/// turns), and needs `trek-mcp` next to the test binary:
/// `cargo build -p trek-mcp && cp target/debug/trek-mcp target/debug/deps/ &&
/// TREK_LIVE_AGENT=arena cargo test -p trek-app live_arena -- --ignored`. Works in
/// /tmp/trek-pstack-e2e; remove ~/.claude/projects/-private-tmp-trek-pstack-e2e and the Codex
/// sessions whose cwd is that folder afterwards.
#[test]
#[ignore = "live: runs real agents"]
fn live_arena_with_real_agents() {
    use std::time::{Duration, Instant};
    use trek_core::orchestrate::{Consult, Consultant, consult_prompt};
    use trek_core::{AgentId, Effort};
    run(async |cx| {
        let trek = super::harness::open_with(cx, |s| {
            s.general.default_agent = AgentId::ClaudeCode.key();
            s.general.default_model = Some("claude-haiku-4-5".into());
            s.general.default_effort = Effort::Low;
        });
        let agents = trek_core::runtime().block_on(trek_core::detect::detect_all());
        trek.update(cx, |ws, _| ws.agents = agents);
        assert!(trek.read(cx, |ws, _| ws.consult_unavailable(&AgentId::ClaudeCode)).is_none(), "trek-mcp next to the test binary, Claude Code and Codex installed");
        let project = std::path::PathBuf::from("/tmp/trek-pstack-e2e");
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("src/lib.rs"), "//! Timeouts for a tiny job runner.\n\npub struct Job {\n    pub name: String,\n}\n\npub fn run(job: &Job) {\n    println!(\"running {}\", job.name);\n}\n").unwrap();
        trek.update(cx, |ws, cx| {
            ws.store.ensure_project(&project).unwrap();
            ws.reload(cx);
            ws.navigate(Route::Draft { project: Some(project.clone()) }, cx);
        });
        let low = |agent: AgentId, model: &str| Consultant { agent, model: model.into(), effort: Effort::Low };
        let consult = Consult {
            consultants: vec![low(AgentId::ClaudeCode, "claude-haiku-4-5"), low(AgentId::Codex, "gpt-5.6-luna")],
            style: Style::Arena,
            implement: false,
            judge: Some(low(AgentId::Codex, "gpt-5.6-luna")),
        };
        let ask = "Add a per-job timeout to src/lib.rs's runner. Keep every design package under 25 lines.";
        trek.update(cx, |ws, cx| ws.send(consult_prompt(ask, &consult, |c| c.model.clone()), vec![], cx));
        let id = trek.thread_id(cx);
        let deadline = Instant::now() + Duration::from_secs(600);
        // Whether the designs ever drafted at the same time: the agent's tool calls run one at a
        // time, so they do only if it started them without waiting and collected them after.
        let mut together = false;
        loop {
            cx.run_until_parked();
            together |= trek.read(cx, |ws, _| ws.children(&id).iter().filter(|t| t.title.starts_with("Design") && ws.task_state(&t.id).live()).count() >= 2);
            // The orchestration tools, and anything else it asks for, are allowed: it reports only.
            for t in std::iter::once(id.clone()).chain(trek.read(cx, |ws, _| ws.children(&id).into_iter().map(|t| t.id.clone()).collect::<Vec<_>>())) {
                if let Some(rid) = trek.read(cx, |ws, _| ws.pending_request(&t).map(|p| p.request_id.clone())) {
                    trek.update(cx, |ws, cx| ws.respond(&t, &rid, trek_agents::Decision::Allow, cx));
                }
            }
            let (state, running) = trek.read(cx, |ws, _| (ws.thread(&id).map(|t| t.run_state), ws.turn_running(&id) || !ws.running_children(&id).is_empty()));
            match (state, running) {
                (Some(RunState::Idle), false) => break,
                (Some(RunState::Failed), _) => panic!("the turn failed: {:?}", trek.items(cx, &id)),
                _ => {}
            }
            assert!(Instant::now() < deadline, "timed out: {:?}", trek.items(cx, &id));
            cx.background_executor.timer(Duration::from_millis(100)).await;
        }
        for i in trek.items(cx, &id) {
            match i {
                Item::Tool { title, detail, output, status, .. } => println!("tool {title} {:?}: {} -> {}", status, orch_preview(&detail), orch_preview(&output)),
                Item::User { text, .. } => println!("user: {}", orch_preview(&text)),
                Item::Assistant { text } => println!("said: {}", orch_preview(&text)),
                _ => {}
            }
        }
        let kids: Vec<(String, String, String, String)> =
            trek.read(cx, |ws, _| ws.children(&id).into_iter().map(|t| (t.id.clone(), t.title.clone(), t.agent.key(), t.model.clone().unwrap_or_default())).collect());
        for (kid, title, agent, model) in &kids {
            let brief = trek.items(cx, kid).into_iter().find_map(|i| if let Item::User { text, .. } = i { Some(text) } else { None }).unwrap_or_default();
            println!("sub-agent {title} on {agent}/{model} ({:?}): {}", trek.read(cx, |ws, _| ws.task_state(kid)), orch_preview(&brief));
            println!("  said: {}", orch_preview(&trek.answers(cx, kid)));
        }
        // Titled as asked (an agent may add a few words: "Design A: Per-job timeout").
        let titles: Vec<&str> = kids.iter().map(|k| k.1.as_str()).collect();
        let on = |title: &str| kids.iter().find(|k| k.1.starts_with(title)).map(|k| (k.2.clone(), k.3.clone())).unwrap_or_else(|| panic!("no {title}: {titles:?}"));
        assert_eq!(on("Judge the designs"), ("codex".to_string(), "gpt-5.6-luna".to_string()));
        let judged = kids.iter().find(|k| k.1.starts_with("Judge")).map(|k| trek.items(cx, &k.0)).unwrap_or_default();
        let brief = judged.iter().find_map(|i| if let Item::User { text, .. } = i { Some(text.to_lowercase()) } else { None }).unwrap_or_default();
        assert!(!["haiku", "luna", "claude", "codex"].iter().any(|w| brief.contains(w)), "judged blind: {brief}");
        let mut designers = vec![on("Design A"), on("Design B")];
        designers.sort();
        assert_eq!(designers, [("claude-code".to_string(), "claude-haiku-4-5".to_string()), ("codex".to_string(), "gpt-5.6-luna".to_string())]);
        assert!(together, "the designs drafted one after the other");
        let answer = trek.answers(cx, &id);
        println!("synthesis: {answer}");
        assert!(answer.contains("Design"), "{answer}");
        assert_eq!(std::fs::read_to_string(project.join("src/lib.rs")).unwrap().lines().count(), 9, "reported only: nothing changed");
        trek.render(cx);
        assert_eq!(trek.rows(cx).iter().filter(|r| r.starts_with("subagent:")).count(), kids.len());
    });
}

fn orch_preview(text: &str) -> String {
    trek_core::orchestrate::preview(text, 160)
}
