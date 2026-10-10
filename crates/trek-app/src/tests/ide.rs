//! Trek IDE (Editor mode): the switch and what it carries across, the AI side bar's chats (and
//! that they're ordinary threads), the editor's tabs, Quick Open and the Explorer's cache.

use super::harness::{Trek, open, run};
use crate::ide::IdeWorkbench;
use crate::workspace::{IdeTab, Mode, Route, Scope};
use trek_core::store::Item;
use gpui_kit::{Entity, TestAppContext};
use gpui_kit::test::TestWindowExt as _;
use trek_core::RunState;

fn ide(trek: &Trek, cx: &TestAppContext) -> Entity<IdeWorkbench> {
    cx.read(|cx| trek.root.read(cx).ide.clone())
}

/// The editor's tabs: (file, preview).
fn tabs(trek: &Trek, cx: &TestAppContext) -> Vec<(std::path::PathBuf, bool)> {
    let ide = ide(trek, cx);
    cx.read(|cx| ide.read(cx).tab_paths(cx))
}

fn active_file(trek: &Trek, cx: &TestAppContext) -> Option<std::path::PathBuf> {
    trek.root.read_with(cx, |r, cx| r.editor(cx).map(|e| e.read(cx).path.clone()))
}

/// The harness on a draft in the project, then the editor.
fn editor_on_draft(trek: &Trek, cx: &mut TestAppContext) {
    let project = trek.project.clone();
    trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project) }, cx));
    trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
    trek.render(cx);
}

#[test]
fn switching_modes_keeps_the_harness_route_and_the_editor_state() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("kept.rs");
        std::fs::write(&file, "fn kept() {}\n").unwrap();
        editor_on_draft(&trek, cx);
        assert_eq!(trek.read(cx, |ws, _| ws.ide_root.clone()), Some(trek.project.clone()), "the draft's project is the IDE folder");
        assert!(trek.visible(cx, "ide-workbench"));
        assert!(!trek.visible(cx, "settle"), "no harness chrome in the editor's title bar");

        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
        trek.render(cx);
        assert_eq!(active_file(&trek, cx).as_deref(), Some(file.as_path()));

        // ⌥⌘E back to Agents: the harness is where it was.
        trek.press(cx, "alt-cmd-e");
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| (ws.mode, ws.route.clone())), (Mode::Agents, Route::Draft { project: Some(trek.project.clone()) }));
        assert!(!trek.visible(cx, "ide-workbench"));

        // And into the editor again: the file is still open there.
        trek.press(cx, "alt-cmd-e");
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.mode), Mode::Editor);
        assert_eq!(tabs(&trek, cx), vec![(file.clone(), false)]);
        assert!(trek.visible(cx, ("ide-tab", 0usize)));
    });
}

#[test]
fn a_new_chat_in_the_editor_starts_a_thread_without_moving_the_harness() {
    run(async |cx| {
        let trek = open(cx);
        editor_on_draft(&trek, cx);
        let route = trek.read(cx, |ws, _| ws.route.clone());
        let before = trek.read(cx, |ws, _| ws.threads.len());
        assert!(trek.visible(cx, "ai-draft"), "the AI side bar opens on a new chat");

        // Typed into the AI side bar's input.
        let ide = ide(&trek, cx);
        let input = cx.read(|cx| ide.read(cx).ai.read(cx).input.clone());
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.type_text(cx, "explain the startup");
        trek.press(cx, "enter");

        let (id, after_route, tab_strip) = trek.read(cx, |ws, _| (ws.ide_chat.active_thread().map(str::to_string), ws.route.clone(), ws.tabs.clone()));
        let id = id.expect("the chat tab became a thread");
        assert_eq!(after_route, route, "the harness didn't move");
        assert!(!tab_strip.contains(&id), "nor did it get a tab there");
        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned()).expect("an ordinary thread");
        assert_eq!(t.cwd.as_deref(), Some(trek.project.as_path()), "it works in the IDE folder");
        assert_eq!(trek.read(cx, |ws, _| ws.threads.len()), before + 1);
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(!trek.answers(cx, &id).is_empty(), "the agent answered it");
        // It's in the harness's inbox, like a thread started there.
        assert!(trek.read(cx, |ws, _| ws.sections().iter().any(|(_, list)| list.iter().any(|t| t.id == id))));
        // On screen in the editor: no toast or badge for it, and window actions act on it.
        assert_eq!(trek.read(cx, |ws, _| ws.shown_in(&id)), Some(Scope::Ide));
        assert_eq!(trek.read(cx, |ws, _| ws.focused_thread().map(str::to_string)), Some(id.clone()));
    });
}

#[test]
fn chat_tabs_open_switch_and_close_without_losing_threads() {
    run(async |cx| {
        let trek = open(cx);
        let a = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string)), Some(a.clone()));
        assert!(trek.visible(cx, ("ai-tab", 1usize)));

        // + goes to the (one) new chat; a click switches back.
        trek.click(cx, "ai-new-chat");
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.ide_chat.is_draft()));
        trek.click(cx, ("ai-tab", 1usize));
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string)), Some(a.clone()));

        // × on the new chat's tab closes it without bringing it forward first.
        trek.click(cx, ("ai-tab-close", 0usize));
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.ide_chat.tabs.clone()), vec![IdeTab::Thread(a.clone())]);
        // Closing the thread's tab leaves a new chat; the thread lives on.
        trek.click(cx, ("ai-tab-close", 0usize));
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.ide_chat.tabs.clone()), vec![IdeTab::Draft]);
        assert!(trek.read(cx, |ws, _| ws.thread(&a).is_some()));
    });
}

#[test]
fn the_switch_carries_the_thread_both_ways() {
    run(async |cx| {
        let trek = open(cx);
        let elsewhere = super::harness::new_project("elsewhere");
        trek.update(cx, |ws, cx| ws.set_ide_root(elsewhere.clone(), cx));
        // Harness → editor with a thread open: its folder and its chat come along.
        let a = trek.quiet_thread(cx);
        trek.click(cx, "mode-editor");
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.ide_root.clone()), Some(trek.project.clone()));
        assert_eq!(trek.read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string)), Some(a.clone()));

        // Another thread in the AI side bar; the window's actions act on it, not the harness's.
        let b = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&trek.project), super::harness::mock(), None, trek_core::Effort::Medium, trek_core::HandHolding::Auto).unwrap();
            ws.reload(cx);
            ws.ide_open_thread(&t.id, cx);
            t.id
        });
        assert_eq!(trek.read(cx, |ws, _| (ws.route.clone(), ws.focused_thread().map(str::to_string))), (Route::Thread(a.clone()), Some(b.clone())));
        assert_eq!(trek.read(cx, |ws, _| (ws.shown_in(&a), ws.shown_in(&b))), (None, Some(Scope::Ide)));

        // Editor → harness: the side bar's chat opens there.
        trek.click(cx, "mode-agents");
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| (ws.mode, ws.route.clone())), (Mode::Agents, Route::Thread(b.clone())));

        // Unless following is off: then the harness stays where it was.
        trek.update(cx, |ws, cx| {
            ws.settings.ide.follow_active_chat = false;
            ws.navigate(Route::Thread(a.clone()), cx);
            ws.set_mode(Mode::Editor, cx);
            ws.ide_open_thread(&b, cx);
            ws.set_mode(Mode::Agents, cx);
        });
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(a));
    });
}

#[test]
fn closing_a_dirty_tab_asks_first_and_the_close_button_only_closes() {
    run(async |cx| {
        let trek = open(cx);
        let a = trek.project.join("a.rs");
        let b = trek.project.join("b.rs");
        std::fs::write(&a, "a\n").unwrap();
        std::fs::write(&b, "b\n").unwrap();
        editor_on_draft(&trek, cx);
        trek.update(cx, |ws, cx| ws.open_editor(a.clone(), None, cx));
        trek.update(cx, |ws, cx| ws.open_editor(b.clone(), None, cx));
        trek.render(cx);
        assert_eq!(active_file(&trek, cx).as_deref(), Some(b.as_path()));

        // × on the tab behind closes it; the one in front stays in front.
        trek.click(cx, ("ide-tab-close", 0usize));
        trek.render(cx);
        assert_eq!(tabs(&trek, cx), vec![(b.clone(), false)]);
        assert_eq!(active_file(&trek, cx).as_deref(), Some(b.as_path()));

        // Unsaved edits: ⌘W asks. Cancel keeps the tab, Don't Save drops the edits.
        let editor = trek.root.read_with(cx, |r, cx| r.editor(cx)).unwrap();
        trek.window(cx, |window, cx| editor.update(cx, |e, cx| e.text_state().update(cx, |s, cx| s.insert("edit ", window, cx))));
        cx.run_until_parked();
        trek.press(cx, "cmd-w");
        assert!(cx.has_pending_prompt(), "closing a dirty tab asks first");
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(tabs(&trek, cx).len(), 1, "Cancel keeps it");
        trek.press(cx, "cmd-w");
        cx.simulate_prompt_answer("Don't Save");
        cx.run_until_parked();
        assert!(tabs(&trek, cx).is_empty());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b\n", "nothing written");

        // Save writes it, then closes.
        trek.update(cx, |ws, cx| ws.open_editor(a.clone(), None, cx));
        let editor = trek.root.read_with(cx, |r, cx| r.editor(cx)).unwrap();
        trek.window(cx, |window, cx| editor.update(cx, |e, cx| e.text_state().update(cx, |s, cx| s.insert("saved ", window, cx))));
        cx.run_until_parked();
        trek.press(cx, "cmd-w");
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();
        assert!(tabs(&trek, cx).is_empty());
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "saved a\n");
    });
}

#[test]
fn explorer_clicks_preview_and_edits_keep_the_tab() {
    run(async |cx| {
        let trek = open(cx);
        let a = trek.project.join("one.rs");
        let b = trek.project.join("two.rs");
        std::fs::write(&a, "1\n").unwrap();
        std::fs::write(&b, "2\n").unwrap();
        editor_on_draft(&trek, cx);
        trek.click(cx, a.display().to_string());
        trek.render(cx);
        assert_eq!(tabs(&trek, cx), vec![(a.clone(), true)], "a single click previews");
        trek.click(cx, b.display().to_string());
        trek.render(cx);
        assert_eq!(tabs(&trek, cx), vec![(b.clone(), true)], "the next preview takes its place");

        let editor = trek.root.read_with(cx, |r, cx| r.editor(cx)).unwrap();
        trek.window(cx, |window, cx| editor.update(cx, |e, cx| e.text_state().update(cx, |s, cx| s.insert("x", window, cx))));
        cx.run_until_parked();
        trek.click(cx, a.display().to_string());
        trek.render(cx);
        assert_eq!(tabs(&trek, cx), vec![(b, false), (a, true)], "an edited preview stays");
    });
}

#[test]
fn cmd_p_lists_the_folders_files() {
    run(async |cx| {
        let trek = open(cx);
        std::fs::create_dir_all(trek.project.join("src")).unwrap();
        std::fs::write(trek.project.join("src/needle.rs"), "fn needle() {}\n").unwrap();
        std::fs::write(trek.project.join("README.md"), "# hi\n").unwrap();
        editor_on_draft(&trek, cx);

        trek.press(cx, "cmd-p");
        trek.render(cx);
        let palette = cx.read(|cx| trek.root.read(cx).palette.clone());
        assert!(palette.read_with(cx, |p, _| p.open && p.files_only), "⌘P opens Quick Open");
        let labels = |cx: &mut TestAppContext| palette.read_with(cx, |p, cx| p.entries(cx).iter().map(|e| e.label.to_string()).collect::<Vec<_>>());
        let all = labels(cx);
        assert!(all.contains(&"needle.rs".to_string()) && all.contains(&"README.md".to_string()), "the folder's files, got {all:?}");

        trek.type_live(cx, "needl");
        trek.render(cx);
        assert_eq!(labels(cx), vec!["needle.rs".to_string()], "files only: no threads or commands");
        trek.press(cx, "enter");
        trek.render(cx);
        assert_eq!(active_file(&trek, cx), Some(trek.project.join("src/needle.rs")));

        // `>` lists commands.
        trek.press(cx, "cmd-p");
        trek.type_live(cx, ">switch");
        trek.render(cx);
        assert!(labels(cx).contains(&"Switch to Agents".to_string()));
    });
}

#[test]
fn the_explorer_reads_folders_once_and_again_when_files_change() {
    run(async |cx| {
        let trek = open(cx);
        std::fs::write(trek.project.join("first.rs"), "\n").unwrap();
        editor_on_draft(&trek, cx);
        assert!(trek.visible(cx, trek.project.join("first.rs").display().to_string()));

        // A file made behind its back doesn't show from a redraw alone: frames draw what was read.
        let later = trek.project.join("later.rs");
        std::fs::write(&later, "\n").unwrap();
        trek.update(cx, |_, cx| cx.notify());
        trek.render(cx);
        assert!(!trek.visible(cx, later.display().to_string()));

        // Refresh reads it again; so does a finished turn or a checkout (files_epoch).
        trek.click(cx, "ide-explorer-refresh");
        trek.render(cx);
        assert!(trek.visible(cx, later.display().to_string()));
        let gone = trek.project.join("first.rs");
        std::fs::remove_file(&gone).unwrap();
        trek.update(cx, |ws, cx| {
            ws.files_epoch += 1;
            cx.notify();
        });
        trek.render(cx);
        assert!(!trek.visible(cx, gone.display().to_string()));
    });
}

#[test]
fn layout_toggles_hide_regions_and_are_kept() {
    run(async |cx| {
        let trek = open(cx);
        editor_on_draft(&trek, cx);
        assert!(trek.visible(cx, "ide-primary") && trek.visible(cx, "ide-ai") && !trek.visible(cx, "ide-panel"));
        trek.press(cx, "cmd-b");
        trek.press(cx, "alt-cmd-b");
        trek.render(cx);
        assert!(!trek.visible(cx, "ide-primary") && !trek.visible(cx, "ide-ai"));
        trek.click(cx, "toggle-panel");
        trek.render(cx);
        assert!(trek.visible(cx, "ide-panel"));
        let layout = trek.read(cx, |ws, _| ws.settings.ide.layout.clone());
        assert!(!layout.primary_open && !layout.ai_open && layout.panel_open, "kept in settings: {layout:?}");
        // ⌘B and ⌘J are the harness's own again there.
        trek.press(cx, "alt-cmd-e");
        trek.press(cx, "cmd-b");
        assert!(trek.read(cx, |ws, _| ws.sidebar_collapsed));
    });
}

// ---------- the AI side bar ----------

fn ai_input(trek: &Trek, cx: &TestAppContext) -> Entity<crate::ide::ai::AiInput> {
    let ide = ide(trek, cx);
    cx.read(|cx| ide.read(cx).ai.read(cx).input.clone())
}

/// The AI side bar's blocks (see `AiTranscript::describe`).
fn ai_rows(trek: &Trek, cx: &TestAppContext) -> Vec<String> {
    let ide = ide(trek, cx);
    cx.read(|cx| ide.read(cx).ai.read(cx).transcript.read(cx).describe(cx))
}

/// Type `text` in the AI side bar's input and press `key` (Return, ⌘Return). Returns the chat's
/// thread.
fn ai_send(trek: &Trek, cx: &mut TestAppContext, text: &str, key: &str) -> String {
    let input = ai_input(trek, cx);
    trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
    trek.type_text(cx, text);
    trek.press(cx, key);
    trek.read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string)).expect("the chat has a thread")
}

/// The user's messages in `id`, as sent.
fn sent(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<String> {
    trek.items(cx, id).into_iter().filter_map(|i| if let Item::User { text, .. } = i { Some(text) } else { None }).collect()
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// The project as a git repository with one commit (`README.md`).
fn make_repo(trek: &Trek) {
    for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@example.com"], &["config", "user.name", "T"], &["config", "commit.gpgsign", "false"], &["config", "core.autocrlf", "false"]] {
        git(&trek.project, args);
    }
    std::fs::write(trek.project.join("README.md"), "hello\n").unwrap();
    git(&trek.project, &["add", "-A"]);
    git(&trek.project, &["commit", "-qm", "init"]);
}

fn pending(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<String> {
    trek.read(cx, |ws, _| ws.pending_files(id).iter().map(|f| f.path.clone()).collect())
}

#[test]
fn the_ai_input_sends_the_current_file_and_picked_lines_and_continues_the_chat() {
    run(async |cx| {
        let trek = open(cx);
        std::fs::create_dir_all(trek.project.join("src")).unwrap();
        let util = trek.project.join("src/util.rs");
        std::fs::write(trek.project.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(&util, "fn greet() {}\nfn wave() {}\nfn nod() {}\n").unwrap();
        editor_on_draft(&trek, cx);
        trek.update(cx, |ws, cx| ws.open_editor(util.clone(), None, cx));
        trek.render(cx);
        assert!(trek.visible(cx, "ai-chip-current"), "the file in front is attached by itself");

        // Lines 2–3 picked, ⌘⇧L: a chip for them.
        let editor = trek.root.read_with(cx, |r, cx| r.editor(cx)).unwrap();
        let state = editor.read_with(cx, |e, _| e.text_state());
        let (from, to) = ("fn greet() {}\n".len(), "fn greet() {}\nfn wave() {}\nfn nod() {}".len());
        state.update(cx, |s, cx| s.set_selected_range(from..to, cx));
        trek.window(cx, |window, cx| editor.update(cx, |e, cx| e.focus(window, cx)));
        trek.press(cx, "cmd-shift-l");
        trek.render(cx);
        assert!(trek.visible(cx, "ai-chip-0"), "the picked lines' chip");
        let attached = ai_input(&trek, cx).read_with(cx, |i, _| i.attached());
        assert_eq!(attached.iter().map(|c| c.label()).collect::<Vec<_>>(), ["util.rs", "util.rs:2–3"]);

        let id = ai_send(&trek, cx, "explain the startup", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let first = sent(&trek, cx, &id).remove(0);
        assert!(first.starts_with("explain the startup\n\n<trek-context>"), "{first}");
        assert!(first.contains("\n@src/util.rs\n"), "the file as a mention: {first}");
        assert!(first.contains("\nsrc/util.rs:2-3\n```rs\nfn wave() {}\nfn nod() {}\n```\n"), "the lines, quoted: {first}");
        assert_eq!(ai_rows(&trek, cx)[0], "user: explain the startup [util.rs, util.rs:2–3]", "the transcript shows the message, the chips named");

        // Sent: the picked lines go; the current file stays attached. Removing it sends none.
        assert_eq!(ai_input(&trek, cx).read_with(cx, |i, _| i.attached().len()), 1);
        trek.click(cx, "ai-chip-current-off");
        let again = ai_send(&trek, cx, "and the shutdown", "enter");
        assert_eq!(again, id, "the same chat goes on");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(sent(&trek, cx, &id)[1], "and the shutdown");
    });
}

#[test]
fn return_queues_while_a_turn_runs_and_cmd_return_steers() {
    run(async |cx| {
        let trek = open(cx);
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "mock:long 8s", "enter");
        trek.wait(cx, "the turn to start", |ws| ws.live.get(&id).is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::Tool { .. })))).await;
        ai_send(&trek, cx, "then the docs", "enter");
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1, "Return queues it for after the turn");
        trek.render(cx);
        assert!(trek.visible(cx, ("ai-queued", 0usize)));
        ai_send(&trek, cx, "use the fast path", "secondary-enter");
        assert_eq!(sent(&trek, cx, &id).last().map(String::as_str), Some("use the fast path"), "⌘Return steers the turn now");
        assert!(trek.read(cx, |ws, _| ws.turn_running(&id)));
        // ⌘⇧⌫ with nothing typed and nothing pending stops the turn; what was queued for after
        // it comes back to the input.
        trek.press(cx, "cmd-shift-backspace");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        assert_eq!(ai_input(&trek, cx).read_with(cx, |i, cx| i.text(cx)), "then the docs");
    });
}

#[test]
fn approvals_and_questions_are_answered_from_the_side_bar() {
    run(async |cx| {
        let trek = open(cx);
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "permission", "enter");
        trek.wait_needs_you(cx, &id).await;
        trek.render(cx);
        assert!(trek.visible(cx, "ai-approval"));
        trek.click(cx, "ai-allow");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("Migrations applied"));

        ai_send(&trek, cx, "question", "enter");
        trek.wait_needs_you(cx, &id).await;
        trek.render(cx);
        assert!(trek.visible(cx, "ai-question"));
        let rid = trek.request(cx, &id);
        trek.click(cx, format!("q-{rid}-0-1"));
        trek.click(cx, format!("q-{rid}-1-0"));
        trek.render(cx);
        trek.click(cx, "ai-q-send");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(sent(&trek, cx, &id).iter().any(|t| t.contains("Postgres")), "the answers went as the user's message");
    });
}

#[test]
fn the_mode_pill_sets_plan_and_ask() {
    run(async |cx| {
        let trek = open(cx);
        editor_on_draft(&trek, cx);
        trek.click(cx, "ai-mode-pill");
        trek.render(cx);
        assert!(trek.visible(cx, "ai-mode-body"));
        trek.click(cx, "ai-mode-plan");
        assert!(trek.read(cx, |ws, _| ws.ide_chat.draft_prefs.plan), "Plan is plan mode");
        assert_eq!(trek.read(cx, |ws, _| ws.chat_mode_in(&Scope::Ide)), crate::workspace::ChatMode::Plan);

        // A plan comes back as a card; Build starts the work.
        let id = ai_send(&trek, cx, "tidy the routes", "enter");
        assert!(trek.read(cx, |ws, _| ws.live.get(&id).is_some_and(|l| l.plan)), "the thread starts in plan mode");
        trek.wait_needs_you(cx, &id).await;
        trek.render(cx);
        assert!(trek.visible(cx, "ai-plan"));
        trek.click(cx, "ai-plan-build");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains("Implemented the plan"));

        // Ask: the thread is held to reading, and its messages say so.
        trek.click(cx, "ai-mode-pill");
        trek.render(cx);
        trek.click(cx, "ai-mode-ask");
        assert!(trek.read(cx, |ws, _| ws.live.get(&id).is_some_and(|l| l.ask && !l.plan)));
        ai_send(&trek, cx, "how does it start", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let last = sent(&trek, cx, &id).pop().unwrap();
        assert!(last.starts_with("how does it start\n\n<trek-ask>"), "{last}");
        assert_eq!(ai_rows(&trek, cx).iter().filter(|r| r.starts_with("user:")).last().map(String::as_str), Some("user: how does it start [ask]"));
        // ⇧Tab cycles on: Ask → Agent.
        let input = ai_input(&trek, cx);
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.press(cx, "shift-tab");
        assert_eq!(trek.read(cx, |ws, _| ws.chat_mode_in(&Scope::Ide)), crate::workspace::ChatMode::Agent);
    });
}

#[test]
fn keep_and_undo_review_an_agents_changes_file_by_file() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "mock:write one.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.wait(cx, "one.md pending", |ws| ws.pending_files(&id).len() == 1).await;
        ai_send(&trek, cx, "mock:write two.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.wait(cx, "both pending", |ws| ws.pending_files(&id).len() == 2).await;
        assert_eq!(pending(&trek, cx, &id), ["one.md", "two.md"], "both turns' files, counted from the first turn's checkpoint");
        trek.render(cx);
        assert!(trek.visible(cx, "ai-pending"), "the pending bar");

        // The agent's file open in the editor is marked until it's kept.
        let two = trek.project.join("two.md");
        trek.update(cx, |ws, cx| ws.open_editor(two.clone(), None, cx));
        trek.render(cx);
        let tab = tabs(&trek, cx).iter().position(|(p, _)| *p == two).unwrap();
        assert!(trek.visible(cx, ("ide-tab-agent", tab)));
        trek.update(cx, |ws, cx| ws.keep_files(&id, Some(vec!["two.md".into()]), cx));
        trek.wait(cx, "two.md kept", |ws| ws.pending_files(&id).len() == 1 && ws.review_settled(&id)).await;
        assert_eq!(pending(&trek, cx, &id), ["one.md"]);
        trek.render(cx);
        assert!(!trek.visible(cx, ("ide-tab-agent", tab)), "kept: no mark");

        // Undo one.md: it was new, so it goes. Nothing's left: the review is over.
        trek.update(cx, |ws, cx| ws.undo_files(&id, Some(vec!["one.md".into()]), cx));
        trek.wait(cx, "one.md undone", |ws| ws.review(&id).is_none()).await;
        assert!(!trek.project.join("one.md").exists());
        assert_eq!(std::fs::read_to_string(&two).unwrap(), "# Notes\n\n- Note 1\n", "the kept file stays");

        // Another edit to the kept file: pending again, against what was kept.
        ai_send(&trek, cx, "mock:write two.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.wait(cx, "two.md pending again", |ws| ws.pending_files(&id).len() == 1).await;
        assert_eq!(trek.read(cx, |ws, _| ws.pending_files(&id).iter().map(|f| (f.added, f.removed)).collect::<Vec<_>>()), [(1, 0)]);

        // No Undo while a turn runs: it would race the agent.
        ai_send(&trek, cx, "mock:long 8s", "enter");
        trek.wait(cx, "the turn to start", |ws| ws.turn_running(&id)).await;
        trek.update(cx, |ws, cx| ws.undo_files(&id, None, cx));
        assert_eq!(std::fs::read_to_string(&two).unwrap(), "# Notes\n\n- Note 1\n- Note 2\n", "refused while the turn runs");
        assert_eq!(pending(&trek, cx, &id), ["two.md"]);
        trek.wait_done(cx, &id, RunState::Idle).await;

        // Undo all, from the bar, asks first: once arms it, again does it. Back to the kept
        // version; the editor shows it.
        trek.render(cx);
        trek.click(cx, "ai-undo-all");
        assert!(trek.read(cx, |ws, _| ws.review(&id).is_some_and(|r| r.undo_all_armed())), "asked, not done");
        assert_eq!(std::fs::read_to_string(&two).unwrap(), "# Notes\n\n- Note 1\n- Note 2\n");
        trek.render(cx);
        trek.click(cx, "ai-undo-all");
        trek.wait(cx, "all undone", |ws| ws.review(&id).is_none()).await;
        assert_eq!(std::fs::read_to_string(&two).unwrap(), "# Notes\n\n- Note 1\n");
        trek.render(cx);
        let editor = trek.root.read_with(cx, |r, cx| r.editor(cx)).unwrap();
        assert_eq!(editor.read_with(cx, |e, cx| e.text_state().read(cx).value().to_string()), "# Notes\n\n- Note 1\n", "a clean buffer reloads");
    });
}

/// The diff tab in front.
fn diff_tab(trek: &Trek, cx: &TestAppContext) -> Option<Entity<crate::ide::diff_view::DiffView>> {
    let ide = ide(trek, cx);
    cx.read(|cx| {
        let ide = ide.read(cx);
        ide.tabs.get(ide.active).and_then(|t| t.diff().cloned())
    })
}

/// Wait (in real time) until `f` holds.
async fn until(cx: &mut TestAppContext, what: &str, f: impl Fn(&mut TestAppContext) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if f(cx) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        cx.background_executor.timer(std::time::Duration::from_millis(5)).await;
    }
}

/// The diff tab in front's rows, once it has read its files and `f` holds for them.
async fn diff_rows(trek: &Trek, cx: &mut TestAppContext, what: &str, f: impl Fn(&[String]) -> bool) -> Vec<String> {
    until(cx, what, |cx| diff_tab(trek, cx).is_some_and(|d| d.read_with(cx, |d, _| d.loaded() && f(&d.describe())))).await;
    diff_tab(trek, cx).unwrap().read_with(cx, |d, _| d.describe())
}

#[test]
fn a_review_holds_the_turns_changes_not_what_came_after_and_undo_keeps_later_edits() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "mock:write one.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        ai_send(&trek, cx, "mock:write two.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.wait(cx, "both pending", |ws| ws.pending_files(&id).len() == 2).await;

        // After the turns: the user edits a file the agent never touched, and one it wrote.
        std::fs::write(trek.project.join("README.md"), "hello\nmine\n").unwrap();
        let one = trek.project.join("one.md");
        std::fs::write(&one, "# Notes\n\n- Note 1\n- mine\n").unwrap();
        trek.update(cx, |ws, cx| ws.review_moved(&id, cx));
        trek.wait(cx, "counted again", |ws| ws.review_settled(&id)).await;
        assert_eq!(pending(&trek, cx, &id), ["one.md", "two.md"], "the user's README.md isn't the chat's");

        // Undo both: two.md goes; one.md changed since its turn ended, so it stays as it is.
        let shown = super::rewind::toasts(&trek, cx);
        trek.update(cx, |ws, cx| ws.undo_files(&id, Some(vec!["one.md".into(), "two.md".into()]), cx));
        trek.wait(cx, "undone", |ws| ws.review_settled(&id)).await;
        assert!(!trek.project.join("two.md").exists());
        assert_eq!(std::fs::read_to_string(&one).unwrap(), "# Notes\n\n- Note 1\n- mine\n", "the user's edit is still there");
        assert_eq!(std::fs::read_to_string(trek.project.join("README.md")).unwrap(), "hello\nmine\n");
        assert_eq!(pending(&trek, cx, &id), ["one.md"]);
        let (message, undo) = shown.borrow().last().cloned().expect("a toast");
        assert!(message.starts_with("Undid two.md · one.md was changed since the turn ended"), "{message}");
        // Its Undo brings two.md back, and nothing else moves.
        trek.update(cx, |ws, cx| ws.undo(undo.expect("an Undo"), cx));
        let two = trek.project.join("two.md");
        trek.wait(cx, "two.md back", move |_| two.exists()).await;
        assert_eq!(std::fs::read_to_string(&one).unwrap(), "# Notes\n\n- Note 1\n- mine\n");

        // Keep while the files are being counted again: the keep isn't dropped half done, and
        // the review doesn't stay busy.
        trek.update(cx, |ws, cx| {
            ws.keep_files(&id, None, cx);
            ws.review_moved(&id, cx);
        });
        trek.wait(cx, "all kept", |ws| ws.review(&id).is_none()).await;
    });
}

#[test]
fn an_edited_chip_counts_lines_as_the_turns_card_does() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        editor_on_draft(&trek, cx);
        // Over the turn something more than the agent's edit lands in the file (a formatter, a
        // command): the edit tool reported 3 lines, the file gained 4.
        super::rewind::during_next_turn(&trek, cx, |p| {
            let one = p.join("one.md");
            let text = std::fs::read_to_string(&one).unwrap();
            std::fs::write(&one, format!("{text}- tidied\n")).unwrap();
        });
        let id = ai_send(&trek, cx, "mock:write one.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let end = trek.items(cx, &id).iter().rposition(|i| matches!(i, Item::TurnEnd { .. })).unwrap();
        for _ in 0..3 {
            trek.render(cx);
            let id2 = id.clone();
            trek.wait(cx, "the turn counted", move |ws| ws.turn_changes_settled(&id2, end)).await;
        }
        let card: Vec<(String, u32, u32)> = trek.read(cx, |ws, _| ws.turn_changes(&id, end)).expect("a card").files.into_iter().map(|f| (f.path, f.added, f.removed)).collect();
        assert_eq!(card, [("one.md".to_string(), 4, 0)]);
        trek.render(cx);
        assert!(ai_rows(&trek, cx).contains(&"edits: one.md +4 −0".to_string()), "the chip agrees with the card: {:?}", ai_rows(&trek, cx));
    });
}

#[test]
fn review_opens_the_pending_diff_in_a_diff_tab_and_keep_all_clears_it() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "mock:write one.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.wait(cx, "one.md pending", |ws| ws.pending_files(&id).len() == 1).await;
        trek.render(cx);
        trek.click(cx, "ai-review");
        let rows = diff_rows(&trek, cx, "the review's rows", |r| !r.is_empty()).await;
        assert_eq!(rows, ["file A one.md +3 −0", "hunk @@ -0,0 +1,3 @@", "+ 1 # Notes", "+ 2 ", "+ 3 - Note 1"], "a new file: every line added, numbered");
        trek.render(cx);
        assert!(trek.visible(cx, "diff-view") && trek.visible(cx, ("diff-keep-file", 0usize)) && trek.visible(cx, ("diff-keep-hunk", 0usize)), "Keep and Undo on the file and the hunk");
        assert_eq!(tabs(&trek, cx), vec![], "a diff tab, not a text buffer");
        assert_eq!(trek.read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string)), Some(id.clone()));

        // ⌘Return with nothing typed keeps everything; the tab says there's nothing left.
        let input = ai_input(&trek, cx);
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.press(cx, "secondary-enter");
        trek.wait(cx, "all kept", |ws| ws.review(&id).is_none()).await;
        assert!(trek.project.join("one.md").exists());
        diff_rows(&trek, cx, "the review to empty", |r| r.is_empty()).await;
    });
}

#[test]
fn the_review_diff_keeps_and_undoes_one_hunk_at_a_time() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        let body: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        std::fs::write(trek.project.join("README.md"), &body).unwrap();
        commit_all(&trek, "readme");
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "mock:write README.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.wait(cx, "README.md pending", |ws| ws.pending_files(&id).len() == 1).await;
        // The agent changed a line near the top and one near the end: two hunks, far apart.
        let changed = body.replace("line 2\n", "line two\n").replace("line 28\n", "line twenty-eight\n");
        std::fs::write(trek.project.join("README.md"), &changed).unwrap();
        trek.update(cx, |ws, cx| ws.review_moved(&id, cx));
        trek.wait(cx, "settled", |ws| ws.review_settled(&id)).await;
        trek.update(cx, |ws, cx| ws.open_review(&id, cx));
        let rows = diff_rows(&trek, cx, "two hunks", |r| r.iter().filter(|l| l.starts_with("hunk")).count() == 2).await;
        assert!(rows.contains(&"- 2 line 2".to_string()) && rows.contains(&"+ 2 line two".to_string()), "{rows:?}");
        assert!(rows.contains(&"  1 1 line 1".to_string()), "context with both numbers: {rows:?}");
        let fold = rows.iter().find(|r| r.starts_with("fold")).cloned().expect("the stretch between the hunks folds");
        assert_eq!(fold, "fold 19 at 6", "{rows:?}");

        // Opened, the stretch shows its lines.
        let diff = diff_tab(&trek, cx).unwrap();
        diff.update(cx, |d, cx| d.open_fold_for_test(6, cx));
        assert!(diff.read_with(cx, |d, _| d.describe().contains(&"  15 15 line 15".to_string())));
        // Side by side, a changed line is one row.
        diff.update(cx, |d, cx| d.set_split_for_test(true, cx));
        assert!(diff.read_with(cx, |d, _| d.describe().contains(&"pair 2 line 2 | 2 line two".to_string())), "{:?}", diff.read_with(cx, |d, _| d.describe()));
        diff.update(cx, |d, cx| d.set_split_for_test(false, cx));

        // Undo the second hunk: it's out of the file; the first stays pending.
        diff.update(cx, |d, cx| d.act_on_hunk(0, 1, false, cx));
        trek.wait(cx, "the hunk undone", |_| std::fs::read_to_string(trek.project.join("README.md")).unwrap().contains("line 28\n")).await;
        assert!(std::fs::read_to_string(trek.project.join("README.md")).unwrap().contains("line two\n"));
        trek.wait(cx, "settled", |ws| ws.review_settled(&id)).await;
        let rows = diff_rows(&trek, cx, "one hunk left", |r| r.iter().filter(|l| l.starts_with("hunk")).count() == 1).await;
        assert!(rows.contains(&"+ 2 line two".to_string()));
        // Keep it: nothing's left to review.
        let diff = diff_tab(&trek, cx).unwrap();
        diff.update(cx, |d, cx| d.act_on_hunk(0, 0, true, cx));
        trek.wait(cx, "kept", |ws| ws.review(&id).is_none()).await;
        assert!(std::fs::read_to_string(trek.project.join("README.md")).unwrap().contains("line two\n"));
    });
}

#[test]
fn outside_git_the_agents_edits_are_kept_but_not_undone() {
    run(async |cx| {
        let trek = open(cx);
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "mock:write one.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.wait(cx, "one.md pending", |ws| ws.pending_files(&id).len() == 1).await;
        trek.render(cx);
        assert!(trek.visible(cx, "ai-keep-all") && !trek.visible(cx, "ai-undo-all"), "no Undo without checkpoints");
        trek.click(cx, "ai-keep-all");
        trek.wait(cx, "kept", |ws| ws.review(&id).is_none()).await;
    });
}

// ---------- the editor and the AI side bar together ----------

fn editor(trek: &Trek, cx: &TestAppContext) -> Entity<crate::editor::EditorView> {
    trek.root.read_with(cx, |r, cx| r.editor(cx)).expect("a file open")
}

/// Wait until the editor in front says `until`.
async fn wait_editor(trek: &Trek, cx: &mut TestAppContext, what: &str, until: impl Fn(&crate::editor::EditorView, &gpui_kit::App) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        let e = editor(trek, cx);
        if e.read_with(cx, |e, cx| until(e, cx)) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        cx.background_executor.timer(std::time::Duration::from_millis(5)).await;
    }
}

fn buffer(trek: &Trek, cx: &TestAppContext) -> String {
    editor(trek, cx).read_with(cx, |e, cx| e.text_state().read(cx).value().to_string())
}

fn focus_editor(trek: &Trek, cx: &mut TestAppContext) {
    let e = editor(trek, cx);
    trek.window(cx, |window, cx| e.update(cx, |e, cx| e.focus(window, cx)));
    cx.run_until_parked();
}

fn commit_all(trek: &Trek, message: &str) {
    git(&trek.project, &["add", "-A"]);
    git(&trek.project, &["commit", "-qm", message]);
}

#[test]
fn the_editor_shows_a_reviews_hunks_and_keeps_or_undoes_them_one_at_a_time() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        let notes = trek.project.join("notes.md");
        std::fs::write(&notes, (1..=12).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
        commit_all(&trek, "notes");
        editor_on_draft(&trek, cx);
        // Open (and clean) before the agent writes: the buffer follows its edit.
        trek.update(cx, |ws, cx| ws.open_editor(notes.clone(), None, cx));
        trek.render(cx);
        let id = ai_send(&trek, cx, "mock:write notes.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.wait(cx, "notes.md pending", |ws| ws.pending_files(&id).len() == 1).await;
        wait_editor(&trek, cx, "the agent's line in the clean buffer", |e, cx| e.text_state().read(cx).value().ends_with("line 12\n- Note 1\n")).await;
        wait_editor(&trek, cx, "one hunk against the baseline", |e, _| e.pending_hunks() == 1).await;
        assert_eq!(editor(&trek, cx).read_with(cx, |e, _| e.fills()), [(12, 13, "added")], "the new line, against the review's baseline");

        // The agent changes the first line too: two hunks.
        std::fs::write(&notes, std::fs::read_to_string(&notes).unwrap().replacen("line 1\n", "LINE ONE\n", 1)).unwrap();
        trek.update(cx, |ws, cx| {
            ws.agent_edits += 1;
            ws.review_moved(&id, cx);
            cx.notify();
        });
        wait_editor(&trek, cx, "two hunks", |e, _| e.pending_hunks() == 2).await;
        assert_eq!(editor(&trek, cx).read_with(cx, |e, _| e.fills()), [(0, 1, "added"), (12, 13, "added")]);
        trek.render(cx);
        assert!(trek.visible(cx, "editor-hunk-bar") && trek.visible(cx, "editor-file-bar"), "the hunk bar and the file bar");

        // ⌘Y on the first line keeps that hunk: the baseline takes it, the file stays.
        let e = editor(&trek, cx);
        trek.window(cx, |window, cx| e.update(cx, |e, cx| e.goto_line(1, window, cx)));
        focus_editor(&trek, cx);
        trek.press(cx, "cmd-y");
        wait_editor(&trek, cx, "one hunk left", |e, _| e.pending_hunks() == 1).await;
        assert!(std::fs::read_to_string(&notes).unwrap().starts_with("LINE ONE\n"));
        assert_eq!(pending(&trek, cx, &id), ["notes.md"]);

        // ⌘N over the hunks is a new chat, as everywhere: it undoes nothing.
        let before = std::fs::read_to_string(&notes).unwrap();
        trek.press(cx, "cmd-n");
        assert_eq!(trek.read(cx, |ws, _| ws.ide_chat.tabs.len()), 2, "a new chat");
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), before, "nothing undone");
        trek.update(cx, |ws, cx| ws.ide_select_chat(0, cx));
        wait_editor(&trek, cx, "the hunk back in view", |e, _| e.pending_hunks() == 1).await;
        focus_editor(&trek, cx);

        // ⌥⌘⌫ undoes the other: out of the file, and with nothing left the review is over. The
        // toast can put it back.
        let shown = super::rewind::toasts(&trek, cx);
        trek.press(cx, "alt-cmd-backspace");
        trek.wait(cx, "the review to close", |ws| ws.review(&id).is_none()).await;
        let expected = format!("LINE ONE\n{}", (2..=12).map(|i| format!("line {i}\n")).collect::<String>());
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), expected);
        wait_editor(&trek, cx, "the buffer to follow", |e, cx| e.text_state().read(cx).value() == expected.as_str()).await;
        let (message, undo) = shown.borrow().last().cloned().expect("a toast");
        assert_eq!(message, "Undid a change to notes.md");
        assert!(matches!(undo, Some(crate::workspace::UndoAction::Unrestore { .. })), "it can be undone");
        // Back to HEAD's fills: the kept line differs from the commit.
        wait_editor(&trek, cx, "HEAD's fills", |e, _| e.pending_hunks() == 0 && e.fills() == [(0, 1, "changed")]).await;
        // The toast's Undo puts the undone change back.
        trek.update(cx, |ws, cx| ws.undo(undo.unwrap(), cx));
        let notes2 = notes.clone();
        trek.wait(cx, "the change back", move |_| std::fs::read_to_string(&notes2).is_ok_and(|t| t.ends_with("line 12\n- Note 1\n"))).await;
    });
}

#[test]
fn the_file_bar_steps_through_the_reviews_files_and_keeps_one() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "mock:write one.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        ai_send(&trek, cx, "mock:write Two.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.wait(cx, "both pending", |ws| ws.pending_files(&id).len() == 2).await;
        assert_eq!(pending(&trek, cx, &id), ["Two.md", "one.md"], "the file's name as written");
        trek.update(cx, |ws, cx| ws.open_editor(trek.project.join("Two.md"), None, cx));
        wait_editor(&trek, cx, "the hunk", |e, _| e.pending_hunks() == 1).await;
        trek.render(cx);
        assert!(trek.visible(cx, "editor-file-bar"));

        trek.click(cx, "editor-file-next");
        trek.render(cx);
        assert_eq!(active_file(&trek, cx), Some(trek.project.join("one.md")), "› opens the next pending file");
        wait_editor(&trek, cx, "one.md's hunk", |e, _| e.pending_hunks() == 1).await;

        trek.click(cx, "editor-file-keep");
        trek.wait(cx, "one.md kept", |ws| ws.pending_files(&id).len() == 1 && ws.review_settled(&id)).await;
        assert_eq!(pending(&trek, cx, &id), ["Two.md"]);
        trek.render(cx);
        assert!(!trek.visible(cx, "editor-file-bar"), "a kept file has no bar");
    });
}

#[test]
fn chat_tabs_and_reviews_outlive_a_restart() {
    run(async |cx| {
        let project = super::harness::new_project("restart");
        let db = super::harness::new_project("restart-data").join("trek.sqlite");
        let mut s = super::harness::settings();
        s.user_projects.push(project.display().to_string());
        let (ws, root, window) = super::harness::launch(cx, trek_core::store::Store::open(&db).expect("store"), s.clone());
        let trek = Trek { ws, root, window, project: project.clone() };
        make_repo(&trek);
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "mock:write one.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.wait(cx, "one.md pending", |ws| ws.pending_files(&id).len() == 1).await;
        trek.update(cx, |ws, cx| ws.ide_new_chat(cx));
        assert_eq!(trek.read(cx, |ws, _| ws.ide_chat.tabs.clone()), [IdeTab::Thread(id.clone()), IdeTab::Draft]);

        // Trek again, over the same data.
        let (ws, root, window) = super::harness::launch(cx, trek_core::store::Store::open(&db).expect("store"), s);
        let again = Trek { ws, root, window, project: project.clone() };
        again.wait(cx, "the review back", |ws| ws.pending_files(&id).len() == 1 && ws.review_settled(&id)).await;
        assert_eq!(pending(&again, cx, &id), ["one.md"]);
        again.update(cx, |ws, cx| ws.set_ide_root(project.clone(), cx));
        assert_eq!(again.read(cx, |ws, _| (ws.ide_chat.tabs.clone(), ws.ide_chat.active)), (vec![IdeTab::Thread(id.clone()), IdeTab::Draft], 1), "the folder's chats as they were left");
        // Keep it there: the review goes, for good.
        again.update(cx, |ws, cx| ws.keep_files(&id, None, cx));
        again.wait(cx, "kept", |ws| ws.review(&id).is_none()).await;
        assert!(again.read(cx, |ws, _| ws.store.reviews().unwrap().is_empty()));
    });
}

#[test]
fn add_to_chat_from_the_explorer_makes_a_chip() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("lib.rs");
        std::fs::write(&file, "pub fn x() {}\n").unwrap();
        editor_on_draft(&trek, cx);
        let ide = ide(&trek, cx);
        ide.update(cx, |i, _| i.layout.ai_open = false);
        trek.render(cx);
        trek.window(cx, |window, cx| window.right_click(file.display().to_string(), cx));
        cx.run_until_parked();
        // "Add to Chat" is the menu's first item.
        trek.window(cx, |window, cx| window.within("popup-menu").click(0usize, cx));
        trek.render(cx);
        let attached = ai_input(&trek, cx).read_with(cx, |i, _| i.attached());
        assert_eq!(attached.iter().map(|c| c.label()).collect::<Vec<_>>(), ["lib.rs"]);
        assert!(trek.visible(cx, "ide-ai") && trek.visible(cx, "ai-chip-0"), "the side bar opens on it");
    });
}

#[test]
fn fix_with_agent_sends_the_problem_and_its_lines() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("main.rs");
        std::fs::write(&file, "fn main() {\n    let x: u32 = \"no\";\n}\n").unwrap();
        editor_on_draft(&trek, cx);
        let diag = lsp_types::Diagnostic {
            range: lsp_types::Range { start: lsp_types::Position { line: 1, character: 17 }, end: lsp_types::Position { line: 1, character: 21 } },
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            message: "mismatched types".into(),
            ..Default::default()
        };
        trek.update(cx, |ws, cx| ws.set_diagnostics(file.clone(), vec![diag], cx));
        let ide = ide(&trek, cx);
        trek.window(cx, |window, cx| ide.update(cx, |i, cx| i.show_panel(crate::ide::PanelTab::Problems, window, cx)));
        trek.render(cx);
        trek.click(cx, ("ide-problem-fix", 0usize));
        let id = trek.read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string)).expect("a chat for it");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let first = sent(&trek, cx, &id).remove(0);
        assert!(first.starts_with("Fix this problem.\n\n<trek-context>"), "{first}");
        assert!(first.contains("\nProblem at main.rs:2 (error): mismatched types\nmain.rs:1-3\n```rs\nfn main() {\n    let x: u32 = \"no\";\n}\n```"), "{first}");
        assert_eq!(ai_rows(&trek, cx)[0], "user: Fix this problem. [⚠ main.rs:2]");
    });
}

#[test]
fn cmd_k_in_the_editor_asks_for_an_inline_edit_that_comes_back_as_a_hunk() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        let util = trek.project.join("util.rs");
        std::fs::write(&util, "fn greet() {}\nfn wave() {}\nfn nod() {}\n").unwrap();
        commit_all(&trek, "util");
        editor_on_draft(&trek, cx);
        // ⌘K outside the editor is still the palette.
        let input = ai_input(&trek, cx);
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.press(cx, "cmd-k");
        let palette = cx.read(|cx| trek.root.read(cx).palette.clone());
        assert!(palette.read_with(cx, |p, _| p.open));
        trek.press(cx, "escape");

        trek.update(cx, |ws, cx| ws.open_editor(util.clone(), None, cx));
        trek.render(cx);
        let state = editor(&trek, cx).read_with(cx, |e, _| e.text_state());
        state.update(cx, |s, cx| s.set_selected_range("fn greet() {}\n".len().."fn greet() {}\nfn wave() {}\nfn nod() {}".len(), cx));
        focus_editor(&trek, cx);
        trek.press(cx, "cmd-k");
        trek.render(cx);
        assert!(editor(&trek, cx).read_with(cx, |e, _| e.inline_open()), "the prompt card opens");
        assert!(trek.visible(cx, "editor-inline"));
        assert!(!palette.read_with(cx, |p, _| p.open), "not the palette");
        trek.type_text(cx, "mark them");
        trek.press(cx, "enter");
        trek.render(cx);
        assert!(!trek.visible(cx, "editor-inline"), "it closes as the request goes");

        let id = trek.read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string)).expect("a new chat for it");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let asked = sent(&trek, cx, &id).remove(0);
        assert!(asked.starts_with("mark them\n\n<trek-context>"), "{asked}");
        assert!(asked.contains("\nutil.rs:2-3\n```rs\nfn wave() {}\nfn nod() {}\n```"), "the lines go along: {asked}");
        assert_eq!(trek_core::inline_edit::parse(&asked), Some(("util.rs".to_string(), (2, 3))));
        assert_eq!(ai_rows(&trek, cx)[0], "user: mark them [⌘K edit, util.rs:2–3]");
        assert_eq!(std::fs::read_to_string(&util).unwrap(), "fn greet() {}\nfn wave() {} // edited\nfn nod() {} // edited\n");
        trek.wait(cx, "util.rs pending", |ws| ws.pending_files(&id).len() == 1).await;
        wait_editor(&trek, cx, "the edit as a hunk", |e, _| e.pending_hunks() == 1).await;
        assert!(buffer(&trek, cx).contains("fn wave() {} // edited"), "the clean buffer shows it");
        assert_eq!(editor(&trek, cx).read_with(cx, |e, _| e.fills()), [(1, 3, "added")]);
    });
}

#[test]
fn a_dirty_buffer_keeps_its_edits_when_an_agent_writes_the_file() {
    run(async |cx| {
        let trek = open(cx);
        let notes = trek.project.join("NOTES.md");
        std::fs::write(&notes, "# Notes\n\n- Note 1\n").unwrap();
        editor_on_draft(&trek, cx);
        trek.update(cx, |ws, cx| ws.open_editor(notes.clone(), None, cx));
        trek.render(cx);
        let e = editor(&trek, cx);
        trek.window(cx, |window, cx| e.update(cx, |e, cx| e.text_state().update(cx, |s, cx| s.insert("mine ", window, cx))));
        cx.run_until_parked();
        let id = ai_send(&trek, cx, "mock:write", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "# Notes\n\n- Note 1\n- Note 2\n");
        assert!(buffer(&trek, cx).starts_with("mine "), "unsaved edits stay");
        assert!(e.read_with(cx, |e, _| e.dirty()));
        // ⌘S warns first: the file changed under it.
        focus_editor(&trek, cx);
        trek.press(cx, "cmd-s");
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "# Notes\n\n- Note 1\n- Note 2\n", "not written over yet");
    });
}

#[test]
fn the_explorer_leaves_out_what_git_ignores_and_opens_folders_with_their_files() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        std::fs::write(trek.project.join(".gitignore"), "build/\n*.log\n").unwrap();
        for f in ["build/out.o", "debug.log", "src/a.rs", "target/keep.txt"] {
            let p = trek.project.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x\n").unwrap();
        }
        editor_on_draft(&trek, cx);
        trek.update(cx, |ws, cx| ws.open_editor(trek.project.join("src/a.rs"), None, cx));
        trek.render(cx);
        let shown = |trek: &Trek, cx: &mut TestAppContext, f: &str| trek.visible(cx, super::harness::join(&trek.project, f).display().to_string());
        assert!(shown(&trek, cx, "src") && shown(&trek, cx, "src/a.rs"), "the opened file's folder shows open, with its files");
        assert!(shown(&trek, cx, ".gitignore") && shown(&trek, cx, "target"), "only what git ignores goes");
        assert!(!shown(&trek, cx, "build") && !shown(&trek, cx, "debug.log"));
    });
}

// ---------- Source Control ----------

fn scm(trek: &Trek, cx: &mut TestAppContext) -> Entity<crate::ide::scm::ScmView> {
    let ide = ide(trek, cx);
    trek.window(cx, |window, cx| ide.update(cx, |ide, cx| ide.show_view(crate::ide::SideView::Scm, window, cx)));
    trek.render(cx);
    cx.read(|cx| ide.read(cx).scm.clone()).expect("Source Control")
}

async fn scm_rows(cx: &mut TestAppContext, view: &Entity<crate::ide::scm::ScmView>, what: &str, want: &[&str]) {
    let want: Vec<String> = want.iter().map(|s| s.to_string()).collect();
    until(cx, what, |cx| view.read_with(cx, |v, _| v.idle() && v.describe() == want)).await;
}

#[test]
fn source_control_stages_unstages_discards_and_commits_with_status_caught_up_at_once() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        std::fs::write(trek.project.join("README.md"), "hello\nthere\n").unwrap();
        std::fs::write(trek.project.join("new.txt"), "a\nb\n").unwrap();
        editor_on_draft(&trek, cx);
        let view = scm(&trek, cx);
        scm_rows(cx, &view, "the changes", &["C M README.md", "C U new.txt"]).await;
        assert_eq!(view.read_with(cx, |v, _| v.file(false, "new.txt").map(|f| (f.added, f.removed))), Some((2, 0)));

        // Stage one: it moves to Staged Changes; unstage it again.
        trek.window(cx, |window, cx| view.update(cx, |v, cx| v.stage(vec!["README.md".into()], window, cx)));
        scm_rows(cx, &view, "README staged", &["S M README.md", "C U new.txt"]).await;
        trek.render(cx);
        assert!(trek.visible(cx, "scm-file-staged-README.md") && trek.visible(cx, "scm-section-staged"));
        trek.window(cx, |window, cx| view.update(cx, |v, cx| v.unstage(vec!["README.md".into()], window, cx)));
        scm_rows(cx, &view, "README unstaged", &["C M README.md", "C U new.txt"]).await;

        // A click opens the file's diff in an editor tab (not in the side bar).
        trek.click(cx, "scm-file-changes-README.md");
        let rows = diff_rows(&trek, cx, "README's diff", |r| !r.is_empty()).await;
        assert_eq!(rows, ["file M README.md +1 −0", "hunk @@ -1 +1,2 @@", "  1 1 hello", "+ 2 there"]);
        assert_eq!(diff_tab(&trek, cx).unwrap().read_with(cx, |d, cx| d.title(cx)), "README.md (Working Tree)");

        // Discard asks first; Cancel leaves it, Discard puts it back.
        trek.window(cx, |window, cx| view.update(cx, |v, cx| {
            let f = v.file(false, "README.md").unwrap();
            v.discard(vec![f], window, cx)
        }));
        assert!(cx.has_pending_prompt(), "discarding asks first");
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(trek.project.join("README.md")).unwrap(), "hello\nthere\n");
        trek.window(cx, |window, cx| view.update(cx, |v, cx| {
            let f = v.file(false, "README.md").unwrap();
            v.discard(vec![f], window, cx)
        }));
        cx.simulate_prompt_answer("Discard");
        scm_rows(cx, &view, "README discarded", &["C U new.txt"]).await;
        assert_eq!(std::fs::read_to_string(trek.project.join("README.md")).unwrap(), "hello\n");
        // The open diff read its file again: nothing left in it.
        diff_rows(&trek, cx, "the diff to empty", |r| r.is_empty()).await;

        // Commit (all: nothing's staged): the list, the status bar and the badge catch up at once.
        trek.update(cx, |ws, cx| ws.refresh_git_at(trek.project.clone(), cx));
        trek.wait(cx, "git read", |ws| ws.git_info.get(&trek.project).is_some_and(|g| g.changed == 1)).await;
        trek.window(cx, |window, cx| view.update(cx, |v, cx| v.set_message("Add new.txt", window, cx)));
        trek.window(cx, |window, cx| view.update(cx, |v, cx| v.commit(window, cx)));
        scm_rows(cx, &view, "the commit", &[]).await;
        trek.wait(cx, "the status bar's count", |ws| ws.git_info.get(&trek.project).is_some_and(|g| g.changed == 0)).await;
        let log = std::process::Command::new("git").args(["log", "-1", "--format=%s"]).current_dir(&trek.project).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "Add new.txt");
    });
}

// ---------- the Explorer's file actions ----------

#[test]
fn the_explorer_makes_renames_and_trashes_files_and_tabs_follow() {
    run(async |cx| {
        let trek = open(cx);
        std::fs::create_dir_all(trek.project.join("src")).unwrap();
        std::fs::write(trek.project.join("src/a.rs"), "fn a() {}\n").unwrap();
        editor_on_draft(&trek, cx);
        let ide = ide(&trek, cx);
        let explorer = cx.read(|cx| ide.read(cx).explorer.clone());

        // New File in src: the name is typed in the tree, then the file is made and opened.
        let src = trek.project.join("src");
        trek.window(cx, |window, cx| explorer.update(cx, |e, cx| e.begin_new(src.clone(), false, window, cx)));
        trek.render(cx);
        assert!(trek.visible(cx, "explorer-edit"));
        trek.type_text(cx, "b.rs");
        trek.press(cx, "enter");
        trek.render(cx);
        let b = super::harness::join(&trek.project, "src/b.rs");
        assert!(b.is_file(), "made");
        assert_eq!(active_file(&trek, cx).as_deref(), Some(b.as_path()), "and opened");
        assert!(trek.visible(cx, b.display().to_string()));

        // A name that's taken is refused (the input stays up); New Folder makes a folder.
        trek.window(cx, |window, cx| explorer.update(cx, |e, cx| e.begin_new(src.clone(), false, window, cx)));
        trek.type_text(cx, "a.rs");
        trek.press(cx, "enter");
        trek.render(cx);
        assert!(trek.visible(cx, "explorer-edit"), "a.rs exists");
        trek.press(cx, "escape");
        trek.render(cx);
        assert!(!trek.visible(cx, "explorer-edit"));
        let root = trek.project.clone();
        trek.window(cx, |window, cx| explorer.update(cx, |e, cx| e.begin_new(root.clone(), true, window, cx)));
        trek.type_text(cx, "docs");
        trek.press(cx, "enter");
        assert!(trek.project.join("docs").is_dir());

        // Rename: the open tab follows the file.
        trek.window(cx, |window, cx| explorer.update(cx, |e, cx| e.begin_rename(b.clone(), window, cx)));
        trek.press(cx, "secondary-a");
        trek.type_text(cx, "c.rs");
        trek.press(cx, "enter");
        trek.render(cx);
        let c = trek.project.join("src/c.rs");
        assert!(c.is_file() && !b.exists());
        assert_eq!(tabs(&trek, cx).iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(), vec![c.clone()], "the tab is on the new name");

        // Delete asks, then the file goes (to the Trash outside tests), its tab with it.
        trek.window(cx, |window, cx| explorer.update(cx, |e, cx| e.delete(c.clone(), window, cx)));
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Move to Trash");
        cx.run_until_parked();
        assert!(!c.exists());
        assert!(tabs(&trek, cx).is_empty());
    });
}

// ---------- the AI side bar's tabs, ⌘K's chat, drafts and edits ----------

#[test]
fn chat_tabs_keep_their_width_list_in_a_menu_and_rename() {
    run(async |cx| {
        let trek = open(cx);
        let ids: Vec<String> = (0..5).map(|_| trek.quiet_thread(cx)).collect();
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        for id in &ids {
            trek.update(cx, |ws, cx| ws.ide_open_thread(id, cx));
        }
        trek.render(cx);
        let tab = trek.bounds(cx, ("ai-tab", trek.read(cx, |ws, _| ws.ide_chat.active))).expect("the active tab in view");
        assert!(tab.size.width >= gpui_kit::px(119.), "never squeezed to a few letters: {:?}", tab.size.width);
        assert!(trek.visible(cx, "ai-tabs-menu"), "⌄ lists them all");

        // A double click renames: the title is typed in the tab.
        let active = trek.read(cx, |ws, _| ws.ide_chat.active);
        let ide = ide(&trek, cx);
        let pane = cx.read(|cx| ide.read(cx).ai.clone());
        trek.window(cx, |window, cx| pane.update(cx, |p, cx| p.begin_rename(active, window, cx)));
        trek.render(cx);
        assert!(trek.visible(cx, ("ai-tab-rename", active)));
        trek.type_text(cx, "Parser work");
        trek.press(cx, "enter");
        let id = trek.read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string)).unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone())), Some("Parser work".to_string()));
    });
}

#[test]
fn cmd_k_edits_go_to_the_files_own_chat_and_leave_the_open_one_alone() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        let util = trek.project.join("util.rs");
        std::fs::write(&util, "fn greet() {}\nfn wave() {}\n").unwrap();
        commit_all(&trek, "util");
        editor_on_draft(&trek, cx);
        // A conversation is open in the side bar.
        let other = ai_send(&trek, cx, "what does this do?", "enter");
        trek.wait_done(cx, &other, RunState::Idle).await;
        let other_title = trek.read(cx, |ws, _| ws.thread(&other).map(|t| t.title.clone()));
        let other_items = trek.items(cx, &other).len();

        trek.update(cx, |ws, cx| ws.open_editor(util.clone(), None, cx));
        trek.render(cx);
        focus_editor(&trek, cx);
        trek.press(cx, "cmd-k");
        trek.render(cx);
        assert!(trek.visible(cx, "editor-inline-chat"));
        assert_eq!(trek.read(cx, |ws, _| ws.inline_target(&util).0), "Inline edits · util.rs", "the card says where it goes");
        trek.type_text(cx, "mark it");
        trek.press(cx, "enter");
        let id = trek.read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string)).unwrap();
        assert_ne!(id, other, "not the open conversation");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone())), Some("Inline edits · util.rs".to_string()));
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&other).map(|t| t.title.clone())), other_title, "nor retitled");
        assert_eq!(trek.items(cx, &other).len(), other_items, "nothing added to it");

        // The next ⌘K on the file goes to the same chat, whatever is in front.
        trek.update(cx, |ws, cx| ws.ide_open_thread(&other, cx));
        trek.update(cx, |ws, cx| ws.inline_edit(&util, (2, 2), "fn wave() {}".into(), "and this", cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(sent(&trek, cx, &id).len(), 2);
        assert_eq!(trek.read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string)), Some(id.clone()), "its chat comes to the front, with the hunk");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone())), Some("Inline edits · util.rs".to_string()), "a later turn doesn't rename it");
    });
}

#[test]
fn each_chat_keeps_its_own_draft() {
    run(async |cx| {
        let trek = open(cx);
        let a = trek.quiet_thread(cx);
        let b = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        trek.update(cx, |ws, cx| ws.ide_open_thread(&a, cx));
        let input = ai_input(&trek, cx);
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.type_text(cx, "for a");
        trek.update(cx, |ws, cx| ws.ide_open_thread(&b, cx));
        cx.run_until_parked();
        assert_eq!(input.read_with(cx, |c, cx| c.text(cx)), "", "b's draft is its own");
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.type_text(cx, "for b");
        trek.update(cx, |ws, cx| ws.ide_open_thread(&a, cx));
        cx.run_until_parked();
        assert_eq!(input.read_with(cx, |c, cx| c.text(cx)), "for a");
        // Return sends a's text to a, not b.
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.press(cx, "enter");
        trek.wait_done(cx, &a, RunState::Idle).await;
        assert!(sent(&trek, cx, &a).last().is_some_and(|t| t.starts_with("for a")));
        assert!(!sent(&trek, cx, &b).iter().any(|t| t.starts_with("for")));
        trek.update(cx, |ws, cx| ws.ide_open_thread(&b, cx));
        cx.run_until_parked();
        assert_eq!(input.read_with(cx, |c, cx| c.text(cx)), "for b");
    });
}

#[test]
fn editing_a_message_asks_about_files_puts_the_draft_aside_and_keeps_text_on_failure() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "mock:write one.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let message = trek.read(cx, |ws, _| ws.live.get(&id).and_then(|l| l.items.id_at(0)).map(str::to_string)).unwrap();
        let input = ai_input(&trek, cx);
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.type_text(cx, "half a thought");

        // Edit: the message comes in, the draft goes aside; the banner says which files go back.
        let (text, images) = trek.read(cx, |ws, _| match &ws.live.get(&id).unwrap().items[0] {
            Item::User { text, .. } => (text.clone(), vec![]),
            _ => unreachable!(),
        });
        trek.update(cx, |_, cx| cx.emit(crate::workspace::WorkspaceEvent::ComposeIn { scope: Scope::Ide, thread: id.clone(), text, images, edit: Some(message.clone()) }));
        cx.run_until_parked();
        assert_eq!(input.read_with(cx, |c, cx| c.text(cx)), "mock:write one.md");
        trek.render(cx);
        assert!(trek.visible(cx, "ai-editing") && trek.visible(cx, "ai-edit-restore"));
        // Esc: the draft is back.
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.press(cx, "escape");
        assert_eq!(input.read_with(cx, |c, cx| c.text(cx)), "half a thought");

        // Again, with the files left as they are this time.
        let (text, images) = trek.read(cx, |ws, _| match &ws.live.get(&id).unwrap().items[0] {
            Item::User { text, .. } => (text.clone(), vec![]),
            _ => unreachable!(),
        });
        trek.update(cx, |_, cx| cx.emit(crate::workspace::WorkspaceEvent::ComposeIn { scope: Scope::Ide, thread: id.clone(), text, images, edit: Some(message.clone()) }));
        cx.run_until_parked();
        trek.render(cx);
        trek.click(cx, "ai-edit-restore");
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.press(cx, "secondary-a");
        trek.type_text(cx, "mock:write two.md");
        trek.press(cx, "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.project.join("one.md").exists(), "Restore files was off: the first file stays");
        assert_eq!(sent(&trek, cx, &id), ["mock:write two.md"], "sent in place of the old message");
        assert_eq!(input.read_with(cx, |c, cx| c.text(cx)), "half a thought", "the draft came back");
    });
}

#[test]
fn trek_commands_and_send_with_cmd_enter_work_in_the_side_bar() {
    run(async |cx| {
        let trek = open(cx);
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "hello", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // /restate <message> asks for a restatement first; the finished turn asks back.
        let input = ai_input(&trek, cx);
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.type_text(cx, "/restate tidy the parser");
        trek.press(cx, "escape");
        trek.press(cx, "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let last = sent(&trek, cx, &id).last().cloned().unwrap();
        assert!(trek_core::restate::split_restate(&last).1, "{last}");
        trek.render(cx);
        assert!(ai_rows(&trek, cx).contains(&"user: tidy the parser".to_string()), "{:?}", ai_rows(&trek, cx));
        let n = ai_rows(&trek, cx).len();
        let _ = n;
        assert!((0..40usize).any(|i| trek.visible(cx, ("ai-restate-yes", i))), "That's right — go ahead");
        // An unknown model to consult says so, and sends nothing.
        let before = sent(&trek, cx, &id).len();
        let input = ai_input(&trek, cx);
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.type_text(cx, "/consult nosuchmodel: hi");
        trek.press(cx, "escape");
        trek.press(cx, "enter");
        cx.run_until_parked();
        assert_eq!(sent(&trek, cx, &id).len(), before);
        // /new: a new chat here, not a draft in the harness.
        let input = ai_input(&trek, cx);
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.set_text_for_test("", window, cx)));
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.type_text(cx, "/new");
        // The `/` picker is open on it: Esc closes it, Return sends the command.
        trek.press(cx, "escape");
        trek.press(cx, "enter");
        assert!(trek.read(cx, |ws, _| ws.ide_chat.is_draft()), "a new chat in the side bar");
        assert_eq!(trek.read(cx, |ws, _| ws.mode), Mode::Editor);

        // Send with ⌘↩: Return is a new line.
        trek.update(cx, |ws, cx| {
            ws.settings.general.send_with_cmd_enter = true;
            cx.notify();
        });
        cx.run_until_parked();
        trek.window(cx, |window, cx| input.update(cx, |c, cx| c.focus(window, cx)));
        trek.type_text(cx, "two");
        trek.press(cx, "enter");
        trek.type_text(cx, "lines");
        assert!(trek.read(cx, |ws, _| ws.ide_chat.is_draft()), "Return didn't send");
        assert_eq!(input.read_with(cx, |c, cx| c.text(cx)), "two\nlines");
        trek.press(cx, "secondary-enter");
        let new = trek.read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string)).expect("⌘↩ sent it");
        trek.wait_done(cx, &new, RunState::Idle).await;
        assert!(sent(&trek, cx, &new)[0].starts_with("two\nlines"));
    });
}

#[test]
fn a_turn_is_undone_from_the_side_bar_after_saying_which_files_go_back() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "mock:write one.md", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.project.join("one.md").exists());
        trek.render(cx);
        let n = (0..40usize).find(|i| trek.visible(cx, ("ai-undo-turn", *i))).expect("the last turn's Undo shows");
        trek.click(cx, ("ai-undo-turn", n));
        trek.render(cx);
        assert!(trek.visible(cx, "ai-confirm"), "asks first");
        until(cx, "the files checked", |cx| {
            trek.render(cx);
            trek.visible(cx, "ai-confirm-restore")
        })
        .await;
        trek.click(cx, "ai-confirm-go");
        trek.wait(cx, "the turn gone", |ws| ws.live.get(&id).is_some_and(|l| l.items.is_empty())).await;
        assert!(!trek.project.join("one.md").exists(), "its file went with it");
        assert_eq!(ai_input(&trek, cx).read_with(cx, |c, cx| c.text(cx)), "mock:write one.md", "the message is back in the input");
    });
}

#[test]
fn the_ai_transcript_only_redraws_for_its_own_chat() {
    run(async |cx| {
        let trek = open(cx);
        let shown = trek.quiet_thread(cx);
        let other = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        trek.update(cx, |ws, cx| ws.ide_open_thread(&shown, cx));
        trek.render(cx);
        cx.run_until_parked();
        crate::tests::take_renders();
        // Another thread streams (its transcript events, the test platform drawing whatever went
        // dirty): the side bar's transcript isn't drawn again.
        for _ in 0..5 {
            trek.update(cx, |ws, cx| {
                if let Some(l) = ws.live.get_mut(&other) {
                    l.revision += 1;
                }
                cx.emit(crate::workspace::WorkspaceEvent::Transcript { id: other.clone(), appended: true });
            });
            cx.run_until_parked();
        }
        assert_eq!(crate::tests::take_renders().get("AiTranscript"), None);
        // Its own chat streaming is.
        trek.update(cx, |ws, cx| {
            if let Some(l) = ws.live.get_mut(&shown) {
                l.revision += 1;
            }
            cx.emit(crate::workspace::WorkspaceEvent::Transcript { id: shown.clone(), appended: true });
        });
        cx.run_until_parked();
        assert!(crate::tests::take_renders().get("AiTranscript").is_some_and(|n| *n > 0));
    });
}

#[test]
fn project_search_skips_what_git_ignores_and_runs_only_when_asked() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek);
        std::fs::create_dir_all(trek.project.join("web/node_modules")).unwrap();
        std::fs::write(trek.project.join("web/.gitignore"), "node_modules/\n").unwrap();
        std::fs::write(trek.project.join("web/node_modules/lib.js"), "needle\n").unwrap();
        std::fs::write(trek.project.join("web/app.js"), "a\nneedle here\n").unwrap();
        let listed = crate::panels::ide_search::list_files(&trek.project);
        assert!(listed.contains(&"web/app.js".to_string()) && !listed.iter().any(|f| f.contains("node_modules")), "{listed:?}");
        editor_on_draft(&trek, cx);
        let ide = ide(&trek, cx);
        trek.window(cx, |window, cx| ide.update(cx, |ide, cx| ide.show_view(crate::ide::SideView::Search, window, cx)));
        trek.type_text(cx, "needle");
        let search = cx.read(|cx| ide.read(cx).search.clone());
        until(cx, "the hits", |cx| search.read_with(cx, |s, _| !s.searching())).await;
        assert_eq!(search.read_with(cx, |s, _| s.hits()), ["web/app.js:2"]);
    });
}

#[test]
fn the_output_panel_lists_language_servers_and_each_chats_agent_log() {
    run(async |cx| {
        let trek = open(cx);
        editor_on_draft(&trek, cx);
        let id = ai_send(&trek, cx, "hello", "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let ide = ide(&trek, cx);
        trek.window(cx, |window, cx| ide.update(cx, |ide, cx| ide.show_panel(crate::ide::PanelTab::Output, window, cx)));
        trek.render(cx);
        // The chat in front's agent: what its processes said on stderr (the mock says nothing).
        assert!(trek.visible(cx, "ide-output-channel"));
        assert!(trek.read(cx, |ws, _| ws.live.get(&id).is_some_and(|l| l.stderr.is_some())), "the session's log is kept");
        ide.update(cx, |ide, cx| {
            ide.output_channel = Some(crate::ide::OutputChannel::Servers);
            cx.notify();
        });
        trek.render(cx);
        assert!(trek.visible(cx, "ide-output"));
    });
}

#[test]
fn a_narrow_ai_side_bar_keeps_send_inside_the_input() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, _| ws.settings.ide.layout.ai_width = 340.);
        editor_on_draft(&trek, cx);
        trek.render(cx);
        let input = trek.bounds(cx, "ai-input").expect("the AI input");
        let send = trek.bounds(cx, "ai-send").expect("the send button");
        assert!(send.right() <= input.right(), "send {send:?} sticks out of the input {input:?}");
        assert!(send.left() >= input.left());
    });
}
