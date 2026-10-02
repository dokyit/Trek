//! End-to-end flows through the real window: composer, transcript, cards, working bar, shortcuts.
//! The mock agent plays each script; see `trek_agents::mock` for what each keyword does.

use super::harness::{open, open_with, run};
use crate::workspace::{Route, SettingsPage};
use trek_core::settings::FollowUp;
use trek_core::store::{Item, ToolStatus};
use trek_core::{HandHolding, RunState};

fn turn_ends(items: &[Item]) -> usize {
    items.iter().filter(|i| matches!(i, Item::TurnEnd { .. })).count()
}

#[test]
fn a_new_thread_streams_an_answer_and_ends_with_a_footer() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "explain the startup");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone())), Some("explain the startup".into()));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let items = trek.items(cx, &id);
        assert!(matches!(&items[0], Item::User { text, .. } if text == "explain the startup"));
        assert!(matches!(items.last(), Some(Item::TurnEnd { .. })), "{items:?}");
        assert!(trek.answers(cx, &id).starts_with("## How the app starts"));
        // The thought before the answer folds away; the answer and its footer show.
        assert_eq!(trek.rows(cx), ["user", "group: Thought it through", "assistant", "end"]);
        let end = items.len() - 1;
        assert!(trek.visible(cx, ("copy-turn", end)), "the footer row is drawn by the (cached) transcript");
        assert_eq!(trek.working_bar(cx), None);
        assert_eq!(trek.composer_text(cx), "", "the composer clears on send");
        // Context usage reached the thread.
        assert!(trek.read(cx, |ws, _| ws.live[&id].context.is_some_and(|(used, window)| used > 0 && window == 200_000)));
    });
}

#[test]
fn tool_calls_fold_into_summary_rows() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "use the tools");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(
            trek.rows(cx),
            ["user", "group: Ran 1 command, read 1 file, and ran 1 search", "assistant", "group: Ran 1 command and edited 2 files", "assistant", "end"]
        );
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| (t.additions, t.deletions))), Some((14, 3)));
        // Opening a group lists its calls; each one opens to its output.
        let first = trek.item_ix(cx, &id, |i| matches!(i, Item::Reasoning { .. }));
        trek.click(cx, ("tool-group", first));
        assert_eq!(&trek.rows(cx)[1..6], ["group: Ran 1 command, read 1 file, and ran 1 search", "  thought", "  tool", "  tool", "  tool"]);
        let ls = trek.item_ix(cx, &id, |i| matches!(i, Item::Tool { detail, .. } if detail == "ls -la"));
        assert!(!trek.visible(cx, ("tool-out", ls)));
        trek.click(cx, ("tool", ls));
        assert!(trek.visible(cx, ("tool-out", ls)));
        // Tool calls stay out of the Markdown export.
        let md = trek.read(cx, |ws, _| ws.transcript_markdown(&id));
        assert!(md.starts_with("# use the tools\n\n## You\n\nuse the tools\n"));
        assert!(md.contains("All 14 tests pass."));
        assert!(!md.contains("ls -la") && !md.contains("Thought"));
    });
}

#[test]
fn sub_agents_report_progress_and_keep_the_turn_open() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "send subagents 3s");
        let tid = id.clone();
        trek.wait(cx, "the first turn to end with agents still out", |ws| {
            let l = &ws.live[&tid];
            l.background == 2 && l.active_tasks() == 2 && l.items.iter().any(|i| matches!(i, Item::Assistant { text } if text.contains("Both scouts are out")))
        })
        .await;
        // The agent's turn ended, but the thread works on until the agents are back.
        assert_eq!(trek.run_state(cx, &id), RunState::Working);
        assert_eq!(turn_ends(&trek.items(cx, &id)), 0);
        assert!(trek.working_bar(cx).is_some_and(|l| l.ends_with("· 2 agents out")), "{:?}", trek.working_bar(cx));
        assert!(trek.visible(cx, "working-bar"));
        // Their calls stay "running" (the tool call returned at once; the agent is still out).
        assert!(trek.items(cx, &id).iter().filter(|i| matches!(i, Item::Tool { title, status: ToolStatus::Running, .. } if title == "Subagent")).count() == 2);
        let group = trek.item_ix(cx, &id, |i| matches!(i, Item::Tool { title, .. } if title == "Subagent"));
        assert!(trek.rows(cx).contains(&"group: Started 2 agents (running)".to_string()), "{:?}", trek.rows(cx));
        trek.click(cx, ("tool-group", group));
        trek.wait(cx, "progress from a sub-agent", |ws| ws.live[&tid].tasks.iter().any(|t| t.activity == "Reading src/routes.rs")).await;
        assert!(trek.rows(cx).contains(&"  tool (Reading src/routes.rs · 2 steps)".to_string()), "{:?}", trek.rows(cx));

        trek.wait_done(cx, &id, RunState::Idle).await;
        let items = trek.items(cx, &id);
        assert!(trek.read(cx, |ws, _| ws.live[&id].tasks.iter().all(|t| t.done == Some(true))));
        assert!(items.iter().filter(|i| matches!(i, Item::Tool { status: ToolStatus::Done, .. })).count() == 2);
        assert!(trek.answers(cx, &id).contains("Both scouts reported back"));
        assert_eq!(turn_ends(&items), 1);
        assert!(matches!(items.last(), Some(Item::TurnEnd { .. })));
        assert_eq!(trek.working_bar(cx), None);
        assert!(!trek.visible(cx, "working-bar"));
    });
}

#[test]
fn permission_cards_allow_deny_and_always_allow() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:permission");
        trek.wait_needs_you(cx, &id).await;
        assert!(trek.read(cx, |ws, _| ws.needs_you_count() == 1));
        assert!(trek.visible(cx, "allow"), "the approval card shows");
        assert_eq!(trek.working_bar(cx), None, "the card takes the working bar's place");
        trek.click(cx, "allow");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("Migrations applied"));
        assert!(!trek.visible(cx, "allow"));

        trek.send(cx, "permission again");
        trek.wait_needs_you(cx, &id).await;
        trek.click(cx, "deny");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("won't run it"));
        let last_tool = trek.items(cx, &id).into_iter().rev().find_map(|i| if let Item::Tool { status, .. } = i { Some(status) } else { None });
        assert_eq!(last_tool, Some(ToolStatus::Failed));

        trek.send(cx, "permission once more");
        trek.wait_needs_you(cx, &id).await;
        trek.click(cx, "allow-session");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // Always allowed for this session: the next one runs without asking.
        trek.send(cx, "permission, last time");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(turn_ends(&trek.items(cx, &id)), 4);
    });
}

#[test]
fn stopping_while_a_card_is_up_clears_it() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "permission");
        trek.wait_needs_you(cx, &id).await;
        assert!(trek.visible(cx, "deny"));
        trek.press(cx, "cmd-.");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(!trek.visible(cx, "deny"));
        assert!(trek.read(cx, |ws, _| ws.live[&id].permissions.is_empty() && ws.needs_you_count() == 0));
        assert!(matches!(trek.items(cx, &id).last(), Some(Item::Notice { text }) if text == "Interrupted"));
    });
}

#[test]
fn the_working_bar_follows_the_thread_on_screen() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:long 30s");
        let tid = id.clone();
        trek.wait(cx, "the build to start", |ws| ws.live[&tid].items.iter().any(|i| matches!(i, Item::Tool { .. }))).await;
        assert!(trek.visible(cx, "working-bar"));
        trek.press(cx, "cmd-n");
        assert_eq!(trek.working_bar(cx), None, "a draft has no working bar");
        let other = trek.send(cx, "meanwhile, explain");
        trek.wait_done(cx, &other, RunState::Idle).await;
        assert_eq!(trek.working_bar(cx), None, "the thread on screen is idle");
        assert_eq!(trek.run_state(cx, &id), RunState::Working, "the other one still works");
        let section = trek.read(cx, |ws, _| ws.sections().into_iter().find(|(_, t)| t.iter().any(|t| t.id == id)).map(|(s, _)| s));
        assert_eq!(section, Some(trek_core::store::Section::Working));
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        assert!(trek.working_bar(cx).is_some());
        assert!(trek.visible(cx, "working-bar"));
        // Deleting a working thread ends its session.
        trek.update(cx, |ws, cx| ws.delete_thread(&id, cx));
        assert!(trek.read(cx, |ws, _| !ws.any_turn_running()));
        assert_eq!(trek.working_bar(cx), None);
    });
}

#[test]
fn raising_hand_holding_approves_what_the_new_level_covers() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.permissions.full_access_unlocked = true);
        let id = trek.send(cx, "permission");
        trek.wait_needs_you(cx, &id).await;
        trek.update(cx, |ws, cx| ws.set_hand_holding(Some(&id), HandHolding::FullAccess, cx)).expect("unlocked");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("Migrations applied"));
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.hand_holding)), Some(HandHolding::FullAccess));
    });
}

#[test]
fn question_cards_take_picks_typed_answers_or_a_skip() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "ask me a question");
        trek.wait_needs_you(cx, &id).await;
        let rid = trek.request(cx, &id);
        assert!(trek.visible(cx, "q-send"));
        trek.click(cx, format!("q-{rid}-0-1")); // Postgres
        trek.click(cx, format!("q-{rid}-1-0")); // Migrations
        trek.click(cx, format!("q-{rid}-1-2")); // Backups
        trek.click(cx, format!("q-{rid}-1-0")); // …and not Migrations after all
        trek.click(cx, "q-send");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let answer = trek.answers(cx, &id);
        assert!(answer.contains("Which database should the service use? **Postgres**"), "{answer}");
        assert!(answer.contains("What should ship with it? **Backups**"), "{answer}");

        // Typing in the composer answers in the user's own words.
        trek.send(cx, "another question");
        trek.wait_needs_you(cx, &id).await;
        trek.send(cx, "SQLite, keep it simple");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("**SQLite, keep it simple**"));
        assert!(trek.items(cx, &id).iter().any(|i| matches!(i, Item::User { text, .. } if text == "SQLite, keep it simple")));

        trek.send(cx, "one more question");
        trek.wait_needs_you(cx, &id).await;
        trek.click(cx, "q-skip");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("No problem"));
    });
}

#[test]
fn plans_can_be_sent_back_or_approved() {
    run(async |cx| {
        let trek = open(cx);
        // ⇧⇥ in the composer turns plan mode on for the new thread.
        trek.press(cx, "shift-tab");
        assert!(trek.read(cx, |ws, _| ws.draft_prefs.plan));
        let id = trek.send(cx, "add auth");
        trek.wait_needs_you(cx, &id).await;
        assert!(trek.visible(cx, "plan-approve"));
        trek.click(cx, "plan-revise");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("keep refining the plan"));
        assert!(trek.read(cx, |ws, _| ws.live[&id].plan), "still planning");

        trek.send(cx, "drop the 401 body");
        trek.wait_needs_you(cx, &id).await;
        trek.click(cx, "plan-approve");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("Implemented the plan"));
        assert!(!trek.read(cx, |ws, _| ws.live[&id].plan), "approving leaves plan mode");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.additions)), Some(31));
    });
}

#[test]
fn queued_follow_ups_wait_for_the_turn() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let id = trek.send(cx, "mock:long 3s");
        let tid = id.clone();
        trek.wait(cx, "the build to start", |ws| ws.live[&tid].items.iter().any(|i| matches!(i, Item::Tool { .. }))).await;
        trek.send(cx, "then summarize");
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1);
        assert!(!trek.items(cx, &id).iter().any(|i| matches!(i, Item::User { text, .. } if text == "then summarize")));
        trek.wait(cx, "both turns", |ws| turn_ends(&ws.live[&tid].items) == 2).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        let items = trek.items(cx, &id);
        let first_end = items.iter().position(|i| matches!(i, Item::TurnEnd { .. })).unwrap();
        let follow_up = items.iter().position(|i| matches!(i, Item::User { text, .. } if text == "then summarize")).unwrap();
        assert!(follow_up > first_end, "sent after the first turn finished");
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 0);
    });
}

#[test]
fn steered_follow_ups_join_the_running_turn() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:long 3s");
        let tid = id.clone();
        trek.wait(cx, "the build to start", |ws| ws.live[&tid].items.iter().any(|i| matches!(i, Item::Tool { .. }))).await;
        trek.send(cx, "use tabs");
        // Sent at once, mid-turn.
        assert!(trek.items(cx, &id).iter().any(|i| matches!(i, Item::User { text, .. } if text == "use tabs")));
        assert_eq!(trek.run_state(cx, &id), RunState::Working);
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("Noted — use tabs"));
        assert_eq!(turn_ends(&trek.items(cx, &id)), 1);
    });
}

#[test]
fn steering_keeps_the_turn_clock_and_sub_agents() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "send subagents 3s");
        let tid = id.clone();
        trek.wait(cx, "agents out", |ws| ws.live[&tid].active_tasks() == 2).await;
        let started = trek.read(cx, |ws, _| ws.live[&id].turn_started);
        trek.send(cx, "check the auth routes too");
        trek.read(cx, |ws, _| {
            let live = &ws.live[&id];
            assert_eq!(live.turn_started, started, "the turn goes on; its clock doesn't restart");
            assert_eq!(live.active_tasks(), 2, "the agents it sent are still tracked");
        });
        trek.wait(cx, "progress after steering", |ws| ws.live[&tid].tasks.iter().any(|t| !t.activity.is_empty())).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("Noted — check the auth routes too"));
        assert!(trek.read(cx, |ws, _| ws.live[&id].tasks.iter().all(|t| t.done == Some(true))));
    });
}

#[test]
fn stopping_a_turn_returns_queued_follow_ups_to_the_composer() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let id = trek.send(cx, "mock:long 30s");
        let tid = id.clone();
        trek.wait(cx, "the build to start", |ws| ws.live[&tid].items.iter().any(|i| matches!(i, Item::Tool { .. }))).await;
        assert!(trek.working_bar(cx).is_some());
        trek.send(cx, "and deploy it");
        trek.press(cx, "cmd-.");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let items = trek.items(cx, &id);
        assert!(matches!(items.last(), Some(Item::Notice { text }) if text == "Interrupted"), "{items:?}");
        assert!(items.iter().any(|i| matches!(i, Item::Tool { status: ToolStatus::Failed, .. })));
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 0);
        assert_eq!(trek.composer_text(cx), "and deploy it");
        assert_eq!(trek.working_bar(cx), None);
    });
}

#[test]
fn a_failed_turn_needs_the_user() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:error");
        trek.wait_done(cx, &id, RunState::Failed).await;
        assert!(matches!(trek.items(cx, &id).last(), Some(Item::Error { .. })));
        assert!(trek.rows(cx).ends_with(&["error".to_string()]));
        assert_eq!(trek.read(cx, |ws, _| ws.needs_you_count()), 1);
        let inbox = trek.read(cx, |ws, _| ws.sections().into_iter().find(|(s, _)| *s == trek_core::store::Section::Inbox).map(|(_, t)| t.len()));
        assert_eq!(inbox, Some(1));
    });
}

#[test]
fn keyboard_shortcuts_reach_their_actions() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.permissions.full_access_unlocked = false);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;

        trek.press(cx, "cmd-shift-a");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.hand_holding)), Some(HandHolding::AutoAcceptEdits));
        trek.press(cx, "cmd-e");
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_some_and(|t| t.settled_at.is_some())));

        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.press(cx, "cmd-j");
        assert!(panel.read_with(cx, |p, _| p.open));
        trek.press(cx, "cmd-j");
        assert!(!panel.read_with(cx, |p, _| p.open));

        trek.press(cx, "cmd-b");
        assert!(trek.read(cx, |ws, _| ws.sidebar_collapsed));
        trek.press(cx, "cmd-b");

        trek.press(cx, "cmd-n");
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: Some(trek.project.clone()) });

        trek.press(cx, "cmd-,");
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Settings(SettingsPage::General));
    });
}
