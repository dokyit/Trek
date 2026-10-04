//! End-to-end flows through the real window: composer, transcript, cards, working bar, shortcuts.
//! The mock agent plays each script; see `trek_agents::mock` for what each keyword does.

use super::harness::{Trek, open, open_with, run};
use crate::workspace::{Route, SettingsPage, UpdateStatus};
use gpui_kit::{Focusable as _, TestAppContext};
use trek_agents::AgentEvent;
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
fn mock_threads_title_themselves_without_a_model() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.auto_title = true);
        // Claude Code looks ready, as in the real app; titles would normally go through it.
        trek.update(cx, |ws, _| {
            ws.agents.push(trek_core::detect::DetectedAgent {
                agent: trek_core::AgentId::ClaudeCode,
                name: "Claude Code".into(),
                path: None,
                version: None,
                availability: trek_core::detect::Availability::Ready,
                models: vec![],
                install_hint: None,
            })
        });
        let id = trek.send(cx, "explain the startup");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // Named by the mock as the turn ends: no title task, no `claude -p`.
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone())), Some("How the app starts".into()));
        // Only the first turn names the thread.
        trek.update(cx, |ws, cx| ws.rename(&id, "My title".into(), cx));
        trek.send(cx, "mock:long 1ms");
        let tid = id.clone();
        trek.wait(cx, "the second turn", |ws| turn_ends(&ws.live[&tid].items) == 2).await;
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone())), Some("My title".into()));
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
fn sub_agents_out_after_the_answer_keep_the_thread_waiting_until_they_report() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "send subagents 3s");
        let tid = id.clone();
        trek.wait(cx, "the first turn to end with agents still out", |ws| {
            let l = &ws.live[&tid];
            l.turn_started.is_none() && l.background.len() == 2 && l.active_tasks() == 2 && l.items.iter().any(|i| matches!(i, Item::Assistant { text } if text.contains("Both scouts are out")))
        })
        .await;
        // The agent's answer is in (its turn has its footer), but it waits on the agents it sent:
        // it's at work, in the Working group, with the header saying on what.
        assert_eq!(trek.run_state(cx, &id), RunState::Idle);
        assert_eq!(turn_ends(&trek.items(cx, &id)), 1);
        assert!(trek.read(cx, |ws, _| ws.waiting(&id)));
        assert!(trek.read(cx, |ws, _| ws.sections().iter().any(|(s, ts)| *s == trek_core::store::Section::Working && ts.iter().any(|t| t.id == id))));
        assert!(trek.working_bar(cx).is_some_and(|l| l.starts_with("Waiting on 2 sub-agents · ")), "{:?}", trek.working_bar(cx));
        trek.render(cx);
        assert!(trek.visible(cx, "working-bar") && trek.visible(cx, "waiting-on"));
        assert!(trek.read(cx, |ws, _| ws.any_turn_running()), "the Mac stays awake while they work");
        // Their calls stay "running" (the tool call returned at once; the agent is still out).
        assert!(trek.items(cx, &id).iter().filter(|i| matches!(i, Item::Tool { title, status: ToolStatus::Running, .. } if title == "Subagent")).count() == 2);
        // Each is a row of its own, as sub-agents Trek runs are, with what it's doing now.
        let first = trek.item_ix(cx, &id, |i| matches!(i, Item::Tool { title, .. } if title == "Subagent"));
        let rows = trek.rows(cx);
        assert!(rows.contains(&"subagent: Map the HTTP routes (Running)".to_string()) && rows.contains(&"subagent: Audit error handling (Running)".to_string()), "{rows:?}");
        assert!(trek.visible(cx, ("subagent", first)));
        trek.wait(cx, "progress from a sub-agent", |ws| ws.live[&tid].tasks.iter().any(|t| t.activity == "Reading src/routes.rs")).await;
        assert!(trek.rows(cx).contains(&"subagent: Map the HTTP routes (Reading src/routes.rs · 2 steps)".to_string()), "{:?}", trek.rows(cx));
        // Opened, the row shows the calls it made.
        trek.click(cx, ("subagent", first));
        assert!(trek.visible(cx, ("subagent-activity", first)));
        let rows = trek.rows(cx);
        let at = rows.iter().position(|r| r.starts_with("subagent: Map the HTTP routes")).unwrap();
        assert_eq!(rows[at + 1..at + 3], ["  Read 1 file · Exploring the project".to_string(), "  Read src/routes.rs".to_string()], "{rows:?}");

        // They report: the agent takes a turn of its own with what they found, and is done.
        trek.wait(cx, "the agent's own turn after the report", |ws| ws.live[&tid].items.iter().any(|i| matches!(i, Item::Assistant { text } if text.contains("Both scouts reported back")))).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        let items = trek.items(cx, &id);
        assert!(trek.read(cx, |ws, _| ws.live[&id].tasks.iter().all(|t| t.done == Some(true))));
        assert!(items.iter().filter(|i| matches!(i, Item::Tool { status: ToolStatus::Done, .. })).count() == 2);
        assert_eq!(turn_ends(&items), 2);
        assert!(matches!(items.last(), Some(Item::TurnEnd { .. })));
        assert!(!trek.read(cx, |ws, _| ws.waiting(&id)));
        assert_eq!(trek.working_bar(cx), None);
        trek.render(cx);
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
        // The build shows live in the bar, not (yet) in the transcript.
        assert!(trek.visible(cx, "live-group"));
        assert_eq!(trek.rows(cx), ["user"]);
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
fn thread_windows_have_their_own_working_bar() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:long 30s");
        let tid = id.clone();
        trek.wait(cx, "the build to start", |ws| ws.live[&tid].items.iter().any(|i| matches!(i, Item::Tool { .. }))).await;
        let own = trek.open_thread_window(cx, &id);
        assert!(trek.visible_in(cx, own, "working-bar"));
        // The main window moves on to a new thread; the thread's own window keeps its bar.
        trek.update(cx, |ws, cx| ws.new_thread(cx));
        assert!(!trek.visible(cx, "working-bar"));
        assert!(trek.visible_in(cx, own, "working-bar"));
        // Its bar shows the live group too; the transcript has it once it's over.
        assert!(trek.visible_in(cx, own, "live-group"));
        let group = trek.item_ix(cx, &id, |i| matches!(i, Item::Reasoning { .. }));
        assert!(!trek.visible_in(cx, own, ("tool-group", group)));
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(!trek.visible_in(cx, own, "working-bar"));
        // Its transcript is cached too, and still draws what changed.
        assert!(trek.visible_in(cx, own, ("tool-group", group)));
        let notice = trek.item_ix(cx, &id, |i| matches!(i, Item::Notice { text } if text == "Interrupted"));
        assert_eq!(notice, trek.items(cx, &id).len() - 1);
    });
}

/// Markdown with a heading, a list, a code block and multi-byte characters.
const STREAMED: &str = "## Café startup — what happens\n\nStartup lives in `src/main.rs`:\n\n- **Flags** are parsed first — naïvely, but fast.\n- Settings load from `config.toml`.\n\n```rust\nfn main() {\n    app::run();\n}\n```\n\nThat's all ✓.";

/// Every document the transcript drew holds exactly the text of the item it's drawn for.
fn assert_documents_match(trek: &Trek, cx: &mut TestAppContext, id: &str) {
    let items = trek.items(cx, id);
    let docs = trek.drawn_markdown(cx);
    assert!(!docs.is_empty(), "no documents drawn");
    for (ix, source) in docs {
        if let Some(Item::Assistant { text } | Item::Reasoning { text }) = items.get(ix) {
            assert_eq!(&source, text, "document for item {ix}");
        }
    }
}

#[test]
fn streamed_answers_draw_their_full_text() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::ReasoningDelta("Looking at the entry point first.".into())], cx));
        // A batch of tokens at a time, each drawn before the next arrives, as while streaming.
        let chars: Vec<char> = STREAMED.chars().collect();
        for (n, chunk) in chars.chunks(7).enumerate() {
            let delta: String = chunk.iter().collect();
            trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TextDelta(delta)], cx));
            if n == 4 {
                // Built on its first frame, mid-stream, from the text so far.
                let answer = trek.item_ix(cx, &id, |i| matches!(i, Item::Assistant { .. }));
                assert!(trek.drawn_markdown(cx).iter().any(|(ix, _)| *ix == answer));
                assert_documents_match(&trek, cx, &id);
            }
        }
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TextDone(STREAMED.into()), AgentEvent::TurnComplete { error: None }], cx));
        assert_eq!(trek.answers(cx, &id), STREAMED);
        assert_documents_match(&trek, cx, &id);
    });
}

#[test]
fn documents_follow_answers_that_move() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        // Agents that hide their reasoning leave empty thoughts, dropped when the turn ends; the
        // answers after them move up. Once to a longer answer's place, once to one of equal length.
        for (first, second) in [("First answer.", "Second, longer answer here."), ("Alpha", "Omega")] {
            for answer in [first, second] {
                let events = vec![AgentEvent::ReasoningDelta(String::new()), AgentEvent::TextDelta(answer.into()), AgentEvent::TextDone(answer.into())];
                trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
            }
            trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TurnComplete { error: None }], cx));
            assert_documents_match(&trek, cx, &id);
        }
        assert_eq!(trek.answers(cx, &id), "First answer.\nSecond, longer answer here.\nAlpha\nOmega");
        assert_eq!(trek.rows(cx).iter().filter(|r| *r == "assistant").count(), 4);
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
        // Picks live in the workspace and don't change the transcript; the (cached) card still
        // redraws for them, even when no pointer movement redraws the window.
        super::take_renders();
        trek.update(cx, |ws, cx| {
            ws.live.get_mut(&id).expect("live").picks.insert((rid.clone(), 0), vec!["SQLite".into()]);
            cx.notify();
        });
        assert!(super::take_renders().get("ThreadView").is_some_and(|n| *n > 0), "the card redrew for a pick");
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
fn secret_answers_are_typed_into_the_cached_card() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let question = trek_agents::Question { question: "Paste your deploy token".into(), header: "Token".into(), options: vec![], multi: false, secret: true };
        let ask = AgentEvent::PermissionRequest { request_id: "token".into(), title: "AskUserQuestion".into(), detail: String::new(), prompt: Some(trek_agents::Prompt::Questions(vec![question])) };
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![ask], cx));
        assert!(trek.visible(cx, "q-send"));
        let field = trek.thread_view(cx).read_with(cx, |v, _| v.secret_field(0));
        trek.window(cx, |window, cx| field.read(cx).focus_handle(cx).focus(window, cx));
        cx.run_until_parked();
        // Each key redraws the (cached) card, without a full refresh.
        super::take_renders();
        trek.type_live(cx, "s3cret");
        assert!(super::take_renders().get("ThreadView").is_some_and(|n| *n > 0), "the card redrew as the secret was typed");
        assert_eq!(field.read_with(cx, |f, _| f.value().to_string()), "s3cret");
        // So does moving the cursor, which changes no text.
        super::take_renders();
        trek.press_live(cx, "left");
        assert!(super::take_renders().get("ThreadView").is_some_and(|n| *n > 0), "the card redrew for the cursor");
        trek.press_live(cx, "enter");
        assert!(trek.read(cx, |ws, _| ws.live[&id].permissions.is_empty()), "answered");
        assert!(!trek.items(cx, &id).iter().any(|i| format!("{i:?}").contains("s3cret")), "a secret is never kept");
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
fn a_message_while_sub_agents_are_out_is_a_turn_of_its_own_and_they_stay_tracked() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "send subagents 3s");
        let tid = id.clone();
        trek.wait(cx, "the answer with agents out", |ws| ws.live[&tid].turn_started.is_none() && ws.live[&tid].active_tasks() == 2).await;
        trek.send(cx, "check the auth routes too");
        trek.read(cx, |ws, _| {
            let live = &ws.live[&id];
            assert!(live.turn_started.is_some(), "the agent is free: the message starts a turn");
            assert_eq!(live.active_tasks(), 2, "the agents it sent are still tracked");
        });
        trek.wait(cx, "progress after the message", |ws| ws.live[&tid].tasks.iter().any(|t| !t.activity.is_empty())).await;
        trek.wait(cx, "the report", |ws| ws.live[&tid].items.iter().any(|i| matches!(i, Item::Assistant { text } if text.contains("Both scouts reported back")))).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
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
        assert!(trek.visible(cx, "settle"), "the title bar offers Settle");
        trek.press(cx, "cmd-e");
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_some_and(|t| t.settled_at.is_some())));
        assert!(!trek.visible(cx, "settle"));
        // The title bar is a cached view: a workspace change alone brings the button back.
        trek.update(cx, |ws, cx| ws.unsettle(&id, cx));
        assert!(trek.visible(cx, "settle"));

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

/// An update staged at a path that doesn't exist: installing it can only fail, here as in a test
/// binary, which isn't an app bundle anyway.
fn stage_fake_update(trek: &Trek, cx: &mut TestAppContext) {
    let staged = super::harness::data_dir().join("no-such-update/Trek.app");
    trek.update(cx, |ws, cx| {
        ws.updater.status = UpdateStatus::Ready { version: "9.9.9".into(), staged };
        ws.restart_to_update(cx);
    });
}

fn update_status(trek: &Trek, cx: &TestAppContext) -> UpdateStatus {
    trek.read(cx, |ws, _| ws.updater.status.clone())
}

#[test]
fn updates_wait_for_a_turn_paused_on_a_card() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:permission");
        trek.wait_needs_you(cx, &id).await;
        // Waiting for the user isn't idle: restarting would end the turn behind the card.
        stage_fake_update(&trek, cx);
        assert!(matches!(update_status(&trek, cx), UpdateStatus::RestartPending { .. }), "{:?}", update_status(&trek, cx));
        trek.click(cx, "allow");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // With the turn over, the restart counts down first: the user may be typing by now.
        assert!(matches!(update_status(&trek, cx), UpdateStatus::RestartPending { .. }), "{:?}", update_status(&trek, cx));
        cx.executor().advance_clock(crate::workspace::RESTART_GRACE);
        cx.run_until_parked();
        // Then the install went ahead (and stopped, as no app bundle runs here).
        let status = update_status(&trek, cx);
        assert!(matches!(&status, UpdateStatus::Failed(e) if e.contains("not running from an app bundle")), "{status:?}");
    });
}

#[test]
fn a_restart_counting_down_can_be_called_off() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:permission");
        trek.wait_needs_you(cx, &id).await;
        stage_fake_update(&trek, cx);
        trek.click(cx, "allow");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.undo(crate::workspace::UndoAction::CancelRestart, cx));
        cx.executor().advance_clock(crate::workspace::RESTART_GRACE * 2);
        cx.run_until_parked();
        // Nothing installed: it waits for a click on Restart, or for Trek to quit.
        assert!(matches!(update_status(&trek, cx), UpdateStatus::Ready { .. }), "{:?}", update_status(&trek, cx));
    });
}

#[test]
fn updates_wait_for_a_plan_offered_after_its_turn() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        // Codex offers its plan once the turn is over; the card lives only in memory.
        let plan = AgentEvent::PermissionRequest {
            request_id: "codex-plan-turn-1".into(),
            title: "Plan".into(),
            detail: String::new(),
            prompt: Some(trek_agents::Prompt::Plan("1. Add the route\n2. Test it".into())),
        };
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::TextDelta("Here's the plan.".into())], cx);
            ws.apply_events(&id, vec![AgentEvent::TurnComplete { error: None }, plan], cx);
        });
        assert_eq!(trek.run_state(cx, &id), RunState::NeedsYou);
        stage_fake_update(&trek, cx);
        assert!(matches!(update_status(&trek, cx), UpdateStatus::RestartPending { .. }), "{:?}", update_status(&trek, cx));
        // Answered, there's nothing left a restart would lose.
        trek.update(cx, |ws, cx| ws.respond(&id, "codex-plan-turn-1", trek_agents::Decision::Deny, cx));
        assert!(!trek.read(cx, |ws, _| ws.work_in_flight()));
    });
}

#[test]
fn transcripts_are_saved_as_turns_pause_soon_after_changes_and_on_quit() {
    run(async |cx| {
        let trek = open(cx);
        let store = trek.read(cx, |ws, _| ws.store.clone());
        let id = trek.send(cx, "mock:permission");
        trek.wait_needs_you(cx, &id).await;
        // Everything up to the card is stored as the turn stops for it: the text, the command.
        let stored = store.items(&id).expect("items");
        assert_eq!(stored, trek.items(cx, &id));
        assert!(stored.iter().any(|i| matches!(i, Item::Tool { status: ToolStatus::Running, .. })), "{stored:?}");

        // Streamed text is saved a second after it arrives.
        let quiet = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.apply_events(&quiet, vec![AgentEvent::TextDelta("Half an ans".into())], cx));
        assert!(store.items(&quiet).expect("items").is_empty(), "not on every batch");
        trek.wait(cx, "the save", |ws| ws.store.items(&quiet).is_ok_and(|i| i == vec![Item::Assistant { text: "Half an ans".into() }])).await;

        // Whatever came since is saved when Trek quits.
        trek.update(cx, |ws, cx| ws.apply_events(&quiet, vec![AgentEvent::TextDelta("wer".into())], cx));
        assert_eq!(store.items(&quiet).expect("items"), vec![Item::Assistant { text: "Half an ans".into() }]);
        cx.quit();
        assert_eq!(store.items(&quiet).expect("items"), vec![Item::Assistant { text: "Half an answer".into() }]);
    });
}

#[test]
fn tests_run_none_of_the_users_agents_or_shells() {
    run(async |cx| {
        let trek = open(cx);
        // Detection would run every agent CLI on this Mac, and then their sign-ins and probes.
        trek.update(cx, |ws, cx| ws.detect_agents(cx));
        assert!(trek.read(cx, |ws, _| !ws.detecting && ws.agents.is_empty()));
        assert_eq!(std::env::var_os("HOME").map(std::path::PathBuf::from), Some(super::harness::data_dir().join("home")));
        // A project action opens its terminal tab without running anything.
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| ws.run_project_action(project, "touch ran.txt".into(), cx));
        trek.render(cx);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!trek.project.join("ran.txt").exists());
    });
}
