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
        // The message shows who designs and who judges.
        let user = trek.item_ix(cx, &id, |i| matches!(i, Item::User { .. }));
        cx.simulate_window_resize(trek.window, gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(2400.)));
        trek.render(cx);
        assert!(trek.visible(cx, ("consulting", user)));
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

        // Set up: a thread in the project, following the guide Trek ships.
        trek.click(cx, "verify-setup");
        let id = trek.thread_id(cx);
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

        // A skill that's gone is forgotten.
        std::fs::remove_dir_all(&v.skill).unwrap();
        trek.update(cx, |ws, cx| ws.refresh_verification(&project, cx));
        assert_eq!(trek.read(cx, |ws, _| ws.verification(&project)), None);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Settings(SettingsPage::Project));
    });
}

/// Restate first and the project notes with a real agent: Claude Code (claude-haiku-4-5, low
/// effort) restates a request without touching anything, and a new thread in a project with a
/// verification skill knows its CLI. Not run by default (two tiny turns):
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
        let done = async |cx: &mut TestAppContext, id: &str| {
            let deadline = Instant::now() + Duration::from_secs(180);
            loop {
                cx.run_until_parked();
                let (state, running, asked) = trek.read(cx, |ws, _| (ws.thread(id).map(|t| t.run_state), ws.turn_running(id), ws.pending_request(id).map(|p| p.request_id.clone())));
                // Anything it asks to do is declined: restating needs nothing done.
                if let Some(rid) = asked {
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

        // A new thread: the project's verification skill reached Claude Code's system prompt.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project.clone()) }, cx));
        let id = trek.send(cx, "Without running or reading anything: what command runs this project's verification CLI, as you were told? Reply with just the command.");
        done(cx, &id).await;
        let answer = trek.answers(cx, &id);
        println!("told: {answer}");
        assert!(answer.contains("notesctl"), "{answer}");
    });
}
