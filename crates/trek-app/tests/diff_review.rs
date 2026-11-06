//! Reviewing an agent's changes from its diff: comments that wait on the thread and go as one
//! message through the normal send path, the keys and where they stop, and viewed files; in the
//! Git tool and in the editor's diff tabs.

mod common;

use common::{Trek, make_repo, open, open_with, run, toasts};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use trek_app::ide::diff_view::{DiffSource, DiffView};
use trek_app::panels::git::GitPanel;
use trek_app::workspace::{Mode, PanelTool, Route};
use trek_core::RunState;
use trek_core::settings::FollowUp;
use trek_core::store::{Item, Store};
use trek_core::worktree::git as run_git;

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    run_git(dir, args).expect("git")
}

/// A change on disk, as the file watcher reports one.
fn files_moved(trek: &Trek, cx: &mut TestAppContext) {
    trek.update(cx, |ws, cx| {
        ws.files_epoch += 1;
        cx.notify();
    });
}

/// A control that shows while its row is hovered.
fn hover_click(
    trek: &Trek,
    cx: &mut TestAppContext,
    row: impl Into<gpui_kit::ElementId>,
    id: impl Into<gpui_kit::ElementId>,
) {
    let row = row.into();
    trek.window(cx, |window, cx| window.hover(row, cx));
    trek.render(cx);
    trek.click(cx, id);
}

fn open_git(trek: &Trek, cx: &mut TestAppContext) -> Entity<GitPanel> {
    let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
    trek.window(cx, |window, cx| {
        panel.update(cx, |p, cx| p.open_tool(PanelTool::Git, window, cx))
    });
    trek.render(cx);
    cx.read(|cx| panel.read(cx).git_panel())
        .expect("the Git tool")
}

/// Wait (in real time) until `f` holds.
async fn until(cx: &mut TestAppContext, what: &str, f: impl Fn(&mut TestAppContext) -> bool) {
    let deadline = std::time::Instant::now() + common::REAL_TIME_LIMIT;
    loop {
        cx.run_until_parked();
        if f(cx) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        cx.background_executor
            .timer(std::time::Duration::from_millis(5))
            .await;
    }
}

/// Two files changed in the project, as an agent's work leaves them: `a.txt` with two hunks.
fn change_two_files(trek: &Trek) {
    let dir = &trek.project;
    let lines: String = (1..=30).map(|n| format!("line {n}\n")).collect();
    std::fs::write(dir.join("a.txt"), &lines).unwrap();
    std::fs::write(dir.join("b.txt"), "one\ntwo\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "files"]);
    let changed = lines
        .replace("line 3\n", "line three\n")
        .replace("line 25\n", "line twenty-five\n");
    std::fs::write(dir.join("a.txt"), changed).unwrap();
    std::fs::write(dir.join("b.txt"), "one\nTWO\n").unwrap();
}

/// A quiet thread on the project, its changes in the Git tool with `a.txt` selected and the keys
/// on the diff.
async fn thread_with_a_diff(trek: &Trek, cx: &mut TestAppContext) -> (String, Entity<GitPanel>) {
    make_repo(trek, cx);
    let id = trek.send(cx, "explain the startup");
    trek.wait_done(cx, &id, RunState::Idle).await;
    change_two_files(trek);
    let panel = open_git(trek, cx);
    files_moved(trek, cx);
    until(cx, "the changed files", |cx| {
        trek.render(cx);
        trek.visible(cx, "gf-a.txt") && trek.visible(cx, "gf-b.txt")
    })
    .await;
    select(trek, &panel, cx, "a.txt").await;
    (id, panel)
}

async fn select(trek: &Trek, panel: &Entity<GitPanel>, cx: &mut TestAppContext, path: &str) {
    trek.click(cx, gpui_kit::SharedString::from(format!("gf-{path}")));
    let shown = panel.clone();
    let want = path.to_string();
    until(cx, "the file's diff", move |cx| {
        shown.read_with(cx, |p, _| {
            p.selected_file().as_deref() == Some(want.as_str()) && !p.describe_diff().is_empty()
        })
    })
    .await;
    // The rows are drawn for the first time now.
    trek.render(cx);
}

fn drafts(trek: &Trek, cx: &TestAppContext, key: &str) -> Vec<trek_app::diff_review::Draft> {
    trek.read(cx, |ws, _| {
        ws.review_notes
            .get(key)
            .map(|n| n.drafts.clone())
            .unwrap_or_default()
    })
}

fn user_messages(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<String> {
    trek.items(cx, id)
        .into_iter()
        .filter_map(|i| match i {
            Item::User { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

/// Write a comment on the line under the cursor, by keyboard.
fn comment(trek: &Trek, cx: &mut TestAppContext, text: &str) {
    trek.press(cx, "c");
    trek.render(cx);
    assert!(trek.visible(cx, "diff-comment-input"));
    trek.type_text(cx, text);
    trek.press(cx, "enter");
    trek.render(cx);
}

#[test]
fn comments_go_to_the_agent_as_one_message_with_file_line_code_and_words() {
    run(async |cx| {
        let trek = open(cx);
        let (id, panel) = thread_with_a_diff(&trek, cx).await;
        // The cursor comes to the first change as the file opens: its removed line, then its new one.
        assert_eq!(
            panel.read_with(cx, |p, _| p.cursor_line()).as_deref(),
            Some("-line 3")
        );
        trek.press(cx, "j");
        assert_eq!(
            panel.read_with(cx, |p, _| p.cursor_line()).as_deref(),
            Some("+line three")
        );
        comment(&trek, cx, "Why spell it out?");
        assert!(panel.read_with(cx, |p, _| p.describe_diff().contains(&"card".to_string())));
        assert!(trek.visible(cx, "diff-comments-send"), "the strip is up");

        // Over to the next hunk for a range: both removed and new lines, numbered as the new file.
        trek.press(cx, "n");
        assert_eq!(
            panel.read_with(cx, |p, _| p.cursor_line()).as_deref(),
            Some("-line 25")
        );
        trek.press(cx, "shift-j");
        comment(&trek, cx, "And this?");
        // In another file.
        trek.press(cx, "]");
        let shown = panel.clone();
        until(cx, "the next file's diff", move |cx| {
            shown.read_with(cx, |p, _| {
                p.selected_file().as_deref() == Some("b.txt") && !p.describe_diff().is_empty()
            })
        })
        .await;
        trek.render(cx);
        trek.press(cx, "j");
        comment(&trek, cx, "Shouting.");

        assert_eq!(drafts(&trek, cx, &id).len(), 3);
        let before = user_messages(&trek, cx, &id).len();
        trek.click(cx, "diff-comments-send");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let sent = user_messages(&trek, cx, &id);
        assert_eq!(sent.len(), before + 1, "one message: {sent:?}");
        assert_eq!(
            sent.last().unwrap(),
            "I left 3 comments on your changes. Please address them together:\n\
             \n1. a.txt:3\n> +line three\nWhy spell it out?\n\
             \n2. a.txt:25\n> -line 25\n> +line twenty-five\nAnd this?\n\
             \n3. b.txt:2\n> +TWO\nShouting."
        );
        assert!(drafts(&trek, cx, &id).is_empty(), "sent, they're gone");
        trek.render(cx);
        assert!(!trek.visible(cx, "diff-comments-send"));
    });
}

#[test]
fn sending_while_the_agent_works_queues_like_any_message() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let (id, _panel) = thread_with_a_diff(&trek, cx).await;
        comment(&trek, cx, "Look again.");
        trek.update(cx, |ws, cx| {
            ws.send_to(&id, "mock:long 8s".into(), vec![], cx)
        });
        trek.wait(cx, "the turn to start", |ws| ws.turn_running(&id))
            .await;
        trek.render(cx);
        trek.click(cx, "diff-comments-send");
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1, "queued for later");
        assert!(
            !user_messages(&trek, cx, &id)
                .iter()
                .any(|m| m.contains("Look again.")),
            "not sent over the running turn"
        );
        assert!(drafts(&trek, cx, &id).is_empty());
    });
}

#[test]
fn keys_stop_at_the_diff_and_never_reach_a_text_field_as_commands() {
    run(async |cx| {
        let trek = open(cx);
        let (id, panel) = thread_with_a_diff(&trek, cx).await;
        let line = |cx: &TestAppContext| panel.read_with(cx, |p, _| p.cursor_line());
        let on = line(cx);

        // In the comment box they're letters.
        trek.press(cx, "c");
        trek.render(cx);
        trek.type_live(cx, "njkvsc");
        assert_eq!(line(cx), on, "the cursor didn't move");
        trek.press(cx, "enter");
        trek.render(cx);
        let saved = drafts(&trek, cx, &id);
        assert_eq!(saved[0].body, "njkvsc");
        assert!(!trek.read(cx, |ws, _| ws.is_viewed(&id, "a.txt")));

        // The diff has the keys back.
        trek.press(cx, "j");
        assert_ne!(line(cx), on);
        let here = line(cx);

        // In the composer they're letters too, and the diff stays put.
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        trek.window(cx, |window, cx| {
            composer.update(cx, |c, cx| c.focus(window, cx))
        });
        trek.render(cx);
        trek.type_live(cx, "njk");
        assert_eq!(line(cx), here);
        assert_eq!(trek.composer_text(cx), "njk");

        // Escape in the diff: a selection goes, then the cursor, then the keys go to the composer.
        let file = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        select(&trek, &panel, cx, "b.txt").await;
        assert!(cx.read(|cx| file.read(cx).git_panel().is_some()));
        trek.press(cx, "shift-j");
        trek.press(cx, "escape");
        assert!(line(cx).is_some(), "the selection, not the cursor");
        trek.press(cx, "escape");
        assert_eq!(line(cx), None);
        trek.render(cx);
        trek.type_live(cx, "x");
        assert_eq!(trek.composer_text(cx), "njkx");
    });
}

#[test]
fn dragging_over_lines_selects_them_and_the_gutters_plus_comments_on_them() {
    run(async |cx| {
        let trek = open(cx);
        let (id, panel) = thread_with_a_diff(&trek, cx).await;
        let row = |cx: &TestAppContext, text: &str| {
            panel.read_with(cx, |p, _| {
                p.describe_diff()
                    .iter()
                    .position(|r| r == text)
                    .expect(text)
            })
        };
        let (from, to) = (row(cx, "-line 3"), row(cx, "+line three"));
        let spot = |trek: &Trek, cx: &mut TestAppContext, ix: usize| {
            let b = trek.bounds(cx, ("git-diff-row", ix)).expect("the row");
            gpui_kit::point(b.origin.x + gpui_kit::px(120.), b.center().y)
        };
        let (start, end) = (spot(&trek, cx, from), spot(&trek, cx, to));
        trek.window(cx, |window, cx| window.drag(start, end, cx));
        trek.render(cx);
        assert_eq!(
            panel.read_with(cx, |p, _| p.cursor_line()).as_deref(),
            Some("+line three")
        );

        // Its last line keeps its "+": what's selected goes in one comment.
        trek.click(cx, ("diff-line-plus", to));
        trek.render(cx);
        assert!(trek.visible(cx, "diff-comment-input"));
        trek.type_text(cx, "Both of these.");
        trek.click(cx, "diff-comment-save");
        trek.render(cx);
        let saved = drafts(&trek, cx, &id);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].place(), "a.txt:3");
        assert_eq!(saved[0].quote, ["-line 3", "+line three"]);

        // On another line, hovering brings its "+", for that line alone.
        let other = row(cx, " line 2");
        hover_click(
            &trek,
            cx,
            ("git-diff-row", other),
            ("diff-line-plus", other),
        );
        trek.render(cx);
        trek.type_text(cx, "Context too.");
        trek.press(cx, "enter");
        let saved = drafts(&trek, cx, &id);
        assert_eq!(saved[1].place(), "a.txt:2");
        assert_eq!(saved[1].quote, [" line 2"]);
    });
}

#[test]
fn a_reviews_files_fold_as_theyre_viewed_and_its_comments_go_to_its_chat() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        editor_on_the_project(&trek, cx);
        trek.update(cx, |ws, cx| {
            ws.send_in(
                &trek_app::workspace::Scope::Ide,
                "mock:write one.md".into(),
                vec![],
                cx,
            )
        });
        let id = trek
            .read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string))
            .expect("a chat");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| {
            ws.send_to(&id, "mock:write two.md".into(), vec![], cx)
        });
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.wait(cx, "both pending", |ws| ws.pending_files(&id).len() == 2)
            .await;

        let ide = cx.read(|cx| trek.root.read(cx).ide.clone());
        let source = DiffSource::Review { thread: id.clone() };
        let view = trek.window(cx, |window, cx| {
            ide.update(cx, |ide, cx| ide.open_diff_view(source, false, window, cx))
        });
        let shown = view.clone();
        until(cx, "the review's files", move |cx| {
            shown.read_with(cx, |d, _| {
                d.loaded() && d.describe().iter().any(|r| r.starts_with("file A two.md"))
            })
        })
        .await;
        trek.render(cx);
        let rows = |cx: &TestAppContext| view.read_with(cx, |d, _| d.describe());
        assert_eq!(
            view.read_with(cx, |d, _| d.cursor_line()).as_deref(),
            Some("+# Notes"),
            "the first change"
        );

        // A comment by keyboard: the chat's own.
        comment(&trek, cx, "Say more.");
        assert_eq!(drafts(&trek, cx, &id)[0].place(), "one.md:1");

        // `v`: the file folds (its comment stays counted), the count moves, the cursor goes on.
        trek.press(cx, "v");
        trek.render(cx);
        assert_eq!(view.read_with(cx, |d, cx| d.viewed_count(cx)), (1, 2));
        let shown = rows(cx);
        let second = shown
            .iter()
            .position(|r| r.starts_with("file A two.md"))
            .unwrap();
        assert!(
            shown[..second].iter().all(|r| !r.starts_with("+ ")),
            "the first file is folded: {shown:?}"
        );
        assert!(shown[second..].iter().any(|r| r.starts_with("+ ")));
        assert!(trek.visible(cx, "diff-comments-toggle"));
        assert_eq!(
            view.read_with(cx, |d, _| d.cursor_line()).as_deref(),
            Some("+# Notes"),
            "the next file's first change"
        );

        // `[` back to the folded file's head, `]` on again.
        trek.press(cx, "[");
        assert_eq!(view.read_with(cx, |d, _| d.cursor_line()), None, "its head");
        trek.press(cx, "]");
        assert_eq!(
            view.read_with(cx, |d, _| d.cursor_line()).as_deref(),
            Some("+# Notes")
        );

        // Editing a comment in a folded file opens the file for it.
        trek.click(cx, "diff-comments-toggle");
        trek.render(cx);
        hover_click(
            &trek,
            cx,
            ("diff-comments-row", 0usize),
            ("diff-comments-edit", 0usize),
        );
        trek.render(cx);
        assert!(rows(cx).iter().any(|r| r == "card"));
        assert!(trek.visible(cx, "diff-comment-input"));
        trek.press(cx, "escape");
        trek.render(cx);

        // Sent to the chat the review is of.
        let before = user_messages(&trek, cx, &id).len();
        trek.press(cx, &trek_app::keys::keystroke(trek_app::keys::Id::DiffSend));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let sent = user_messages(&trek, cx, &id);
        assert_eq!(sent.len(), before + 1);
        assert!(sent.last().unwrap().contains("one.md:1"));
        assert!(sent.last().unwrap().contains("Say more."));
        assert!(drafts(&trek, cx, &id).is_empty());
    });
}

#[test]
fn comments_outlast_switching_files_closing_the_panel_and_the_app() {
    run(async |cx| {
        let trek = open(cx);
        let (id, panel) = thread_with_a_diff(&trek, cx).await;
        comment(&trek, cx, "Keep me.");
        let row_count = |panel: &Entity<GitPanel>, cx: &TestAppContext| {
            panel.read_with(cx, |p, _| {
                p.describe_diff().iter().filter(|r| *r == "card").count()
            })
        };
        assert!(row_count(&panel, cx) > 0);

        // Away to another file: its diff has no card, the strip still counts it.
        select(&trek, &panel, cx, "b.txt").await;
        assert_eq!(row_count(&panel, cx), 0);
        assert!(trek.visible(cx, "diff-comments-toggle"));
        // And back: the card is under its line again.
        select(&trek, &panel, cx, "a.txt").await;
        assert!(row_count(&panel, cx) > 0);

        // The panel closed and opened again.
        trek.press(
            cx,
            &trek_app::keys::keystroke(trek_app::keys::Id::ToggleRightPanel),
        );
        trek.render(cx);
        assert!(!trek.visible(cx, "diff-comments-toggle"));
        open_git(&trek, cx);
        assert_eq!(drafts(&trek, cx, &id).len(), 1);
        assert!(trek.visible(cx, "diff-comments-toggle"));

        // The store has them for the next launch.
        let kept = trek.read(cx, |ws, _| ws.store.review_notes().unwrap());
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].0, id);
        assert!(kept[0].1.contains("Keep me."));
    });
}

#[test]
fn drafts_come_back_with_the_app() {
    run(async |cx| {
        let store = Store::in_memory().unwrap();
        let project = common::new_project("kept");
        store.ensure_project(&project).unwrap();
        let t = store
            .create_thread(
                None,
                common::mock(),
                None,
                trek_core::Effort::High,
                trek_core::HandHolding::Auto,
            )
            .unwrap();
        let notes = trek_app::diff_review::Notes {
            drafts: vec![trek_app::diff_review::Draft {
                id: 4,
                path: "a.rs".into(),
                side: trek_app::diff_review::Side::New,
                first: 1,
                last: 1,
                quote: vec![],
                body: "Still here".into(),
            }],
            viewed: Default::default(),
            next_id: 4,
        };
        store
            .set_review_notes(&t.id, Some(&serde_json::to_string(&notes).unwrap()))
            .unwrap();
        store
            .set_review_notes("gone-thread", Some(&serde_json::to_string(&notes).unwrap()))
            .unwrap();
        let (ws, _root, _window) = common::launch(cx, store, common::settings());
        cx.run_until_parked();
        ws.read_with(cx, |ws, _| {
            assert_eq!(ws.review_notes.get(&t.id).map(|n| n.drafts.len()), Some(1));
            assert!(!ws.review_notes.contains_key("gone-thread"));
            assert_eq!(ws.store.review_notes().unwrap().len(), 1);
        });
    });
}

#[test]
fn each_comment_can_be_edited_or_removed_and_clearing_can_be_undone() {
    run(async |cx| {
        let trek = open(cx);
        let (id, _panel) = thread_with_a_diff(&trek, cx).await;
        comment(&trek, cx, "First words.");
        trek.press(cx, "n");
        comment(&trek, cx, "Second.");
        assert_eq!(drafts(&trek, cx, &id).len(), 2);

        // Edit: the box holds what was written, and saving replaces it.
        trek.click(cx, "diff-comments-toggle");
        trek.render(cx);
        hover_click(
            &trek,
            cx,
            ("diff-comments-row", 0usize),
            ("diff-comments-edit", 0usize),
        );
        trek.render(cx);
        trek.press(cx, "secondary-a");
        trek.type_text(cx, "Better words.");
        trek.press(cx, "enter");
        trek.render(cx);
        let bodies: Vec<String> = drafts(&trek, cx, &id).into_iter().map(|d| d.body).collect();
        assert_eq!(bodies, ["Better words.", "Second."]);

        // Escape leaves the box and changes nothing.
        hover_click(
            &trek,
            cx,
            ("diff-comments-row", 1usize),
            ("diff-comments-edit", 1usize),
        );
        trek.render(cx);
        trek.type_text(cx, " Not this.");
        trek.press(cx, "escape");
        trek.render(cx);
        assert!(!trek.visible(cx, "diff-comment-input"));
        assert_eq!(drafts(&trek, cx, &id)[1].body, "Second.");

        // Remove takes the one.
        hover_click(
            &trek,
            cx,
            ("diff-comments-row", 0usize),
            ("diff-comments-remove", 0usize),
        );
        trek.render(cx);
        assert_eq!(drafts(&trek, cx, &id).len(), 1);

        // Clear takes the rest, and says how to get them back.
        let said = toasts(&trek, cx);
        trek.click(cx, "diff-comments-clear");
        trek.render(cx);
        assert!(drafts(&trek, cx, &id).is_empty());
        assert!(!trek.visible(cx, "diff-comments-send"));
        let (message, undo) = said.borrow().last().cloned().expect("a toast");
        assert_eq!(message, "Cleared 1 comment");
        trek.update(cx, |ws, cx| ws.undo(undo.expect("an undo"), cx));
        trek.render(cx);
        assert_eq!(drafts(&trek, cx, &id)[0].body, "Second.");
        assert!(trek.visible(cx, "diff-comments-send"));
    });
}

#[test]
fn a_comment_moves_with_its_code_and_goes_to_the_list_when_the_code_is_gone() {
    run(async |cx| {
        let trek = open(cx);
        let (id, panel) = thread_with_a_diff(&trek, cx).await;
        trek.press(cx, "j");
        comment(&trek, cx, "On the new line.");
        assert_eq!(drafts(&trek, cx, &id)[0].first, 3);

        // Two lines come in above it: it's on line 5 now, and its message will say so.
        let file = trek.project.join("a.txt");
        let moved = format!("new 1\nnew 2\n{}", std::fs::read_to_string(&file).unwrap());
        std::fs::write(&file, &moved).unwrap();
        files_moved(&trek, cx);
        let (t, i) = (trek.ws.clone(), id.clone());
        until(cx, "the comment to move", move |cx| {
            t.read_with(cx, |ws, _| ws.review_notes[&i].drafts[0].first == 5)
        })
        .await;
        assert_eq!(drafts(&trek, cx, &id)[0].last, 5);

        // The line goes: still a draft, no card, and the list still has it.
        std::fs::write(&file, moved.replace("line three\n", "line 3\n")).unwrap();
        files_moved(&trek, cx);
        let shown = panel.clone();
        until(cx, "the card to go", move |cx| {
            shown.read_with(cx, |p, _| {
                !p.describe_diff().is_empty() && !p.describe_diff().contains(&"card".to_string())
            })
        })
        .await;
        assert_eq!(drafts(&trek, cx, &id).len(), 1);
        trek.render(cx);
        assert!(trek.visible(cx, "diff-comments-toggle"));
    });
}

#[test]
fn a_viewed_file_is_ticked_counted_and_comes_back_when_it_changes() {
    run(async |cx| {
        let trek = open(cx);
        let (id, panel) = thread_with_a_diff(&trek, cx).await;
        // `v`: viewed, and on to the next.
        trek.press(cx, "v");
        let shown = panel.clone();
        until(cx, "the next file", move |cx| {
            shown.read_with(cx, |p, _| p.selected_file().as_deref() == Some("b.txt"))
        })
        .await;
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.is_viewed(&id, "a.txt")));
        assert_eq!(panel.read_with(cx, |p, cx| p.viewed_shown(cx)), (1, 2));

        // Marked from the list: the mark learns what the file says.
        trek.press(cx, "v");
        trek.render(cx);
        assert_eq!(panel.read_with(cx, |p, cx| p.viewed_shown(cx)), (2, 2));
        assert_eq!(
            panel.read_with(cx, |p, _| p.selected_file()),
            None,
            "nothing left to review"
        );

        // The agent changes a viewed file: it's back to being reviewed.
        std::fs::write(trek.project.join("b.txt"), "one\nTWO!\n").unwrap();
        files_moved(&trek, cx);
        let (t, i) = (trek.ws.clone(), id.clone());
        until(cx, "the mark to go", move |cx| {
            t.read_with(cx, |ws, _| !ws.is_viewed(&i, "b.txt"))
        })
        .await;
        assert!(
            trek.read(cx, |ws, _| ws.is_viewed(&id, "a.txt")),
            "the other stays"
        );
        assert_eq!(panel.read_with(cx, |p, cx| p.viewed_shown(cx)), (1, 2));
    });
}

/// The editor on the project, `rel`'s changes in a diff tab, with its keys.
async fn diff_tab(
    trek: &Trek,
    cx: &mut TestAppContext,
    rel: &str,
    staged: bool,
) -> Entity<DiffView> {
    let top = common::canonical(&trek.project);
    let source = DiffSource::Git {
        top,
        rel: rel.to_string(),
        staged,
    };
    let ide = cx.read(|cx| trek.root.read(cx).ide.clone());
    let view = trek.window(cx, |window, cx| {
        ide.update(cx, |ide, cx| ide.open_diff_view(source, false, window, cx))
    });
    let shown = view.clone();
    until(cx, "the diff tab", move |cx| {
        shown.read_with(cx, |d, _| d.loaded() && !d.describe().is_empty())
    })
    .await;
    trek.render(cx);
    view
}

fn editor_on_the_project(trek: &Trek, cx: &mut TestAppContext) {
    let project = trek.project.clone();
    trek.update(cx, |ws, cx| {
        ws.navigate(
            Route::Draft {
                project: Some(project),
            },
            cx,
        )
    });
    trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
    trek.render(cx);
}

#[test]
fn a_hunk_stages_and_unstages_from_the_keyboard_in_an_editor_diff() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        change_two_files(&trek);
        editor_on_the_project(&trek, cx);
        let dir = trek.project.clone();

        let view = diff_tab(&trek, cx, "a.txt", false).await;
        // The cursor is on the first hunk, shown on its head's row and its lines.
        assert_eq!(
            view.read_with(cx, |d, _| d.cursor_line()).as_deref(),
            Some("-line 3")
        );
        trek.press(cx, "n");
        assert_eq!(
            view.read_with(cx, |d, _| d.cursor_line()).as_deref(),
            Some("-line 25")
        );
        trek.press(cx, "s");
        let d = dir.clone();
        until(cx, "the second hunk staged", move |_| {
            run_git(&d, &["diff", "--cached", "--no-color"])
                .is_ok_and(|s| s.contains("+line twenty-five"))
        })
        .await;
        let left = git(&dir, &["diff", "--no-color", "--", "a.txt"]);
        assert!(left.contains("+line three") && !left.contains("+line twenty-five"));
        let shown = view.clone();
        until(cx, "the hunk to leave the diff", move |cx| {
            shown.read_with(cx, |d, _| {
                d.loaded() && !d.describe().iter().any(|r| r.contains("twenty-five"))
            })
        })
        .await;

        // In the index's diff, the same key takes it back out.
        let staged = diff_tab(&trek, cx, "a.txt", true).await;
        assert_eq!(
            staged.read_with(cx, |d, _| d.cursor_line()).as_deref(),
            Some("-line 25")
        );
        trek.press(cx, "s");
        let d = dir.clone();
        until(cx, "the hunk unstaged", move |_| {
            run_git(&d, &["diff", "--cached", "--no-color"]).is_ok_and(|s| s.is_empty())
        })
        .await;
    });
}

#[test]
fn an_editor_diffs_comments_stay_with_the_folder_and_go_to_the_side_bars_chat() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        change_two_files(&trek);
        editor_on_the_project(&trek, cx);
        diff_tab(&trek, cx, "a.txt", false).await;
        trek.press(cx, "j");
        comment(&trek, cx, "From the editor.");
        let key = trek_app::diff_review::folder_key(&common::canonical(&trek.project));
        assert_eq!(drafts(&trek, cx, &key).len(), 1);
        assert_eq!(drafts(&trek, cx, &key)[0].place(), "a.txt:3");

        trek.press(cx, &trek_app::keys::keystroke(trek_app::keys::Id::DiffSend));
        cx.run_until_parked();
        let chat = trek
            .read(cx, |ws, _| ws.ide_chat.active_thread().map(str::to_string))
            .expect("a chat started in the side bar");
        trek.wait_done(cx, &chat, RunState::Idle).await;
        assert!(user_messages(&trek, cx, &chat)[0].contains("From the editor."));
        assert!(drafts(&trek, cx, &key).is_empty());
    });
}

#[test]
fn a_viewed_file_folds_in_a_review_and_opens_again_on_a_click() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        change_two_files(&trek);
        editor_on_the_project(&trek, cx);
        let view = diff_tab(&trek, cx, "a.txt", false).await;
        let lines = |cx: &TestAppContext| {
            view.read_with(cx, |d, _| {
                d.describe().iter().filter(|r| r.starts_with("+ ")).count()
            })
        };
        assert_eq!(lines(cx), 2);
        assert_eq!(view.read_with(cx, |d, cx| d.viewed_count(cx)).0, 0);

        trek.click(cx, ("diff-viewed-file", 0usize));
        trek.render(cx);
        assert_eq!(lines(cx), 0, "folded");
        assert_eq!(view.read_with(cx, |d, cx| d.viewed_count(cx)).0, 1);
        assert!(trek.visible(cx, ("diff-file", 0usize)));

        // Open it: still viewed.
        trek.click(cx, ("diff-file", 0usize));
        trek.render(cx);
        assert_eq!(lines(cx), 2);
        assert_eq!(view.read_with(cx, |d, cx| d.viewed_count(cx)).0, 1);

        // Unmarked, it's as it was.
        trek.click(cx, ("diff-viewed-file", 0usize));
        trek.render(cx);
        assert_eq!(lines(cx), 2);
        assert_eq!(view.read_with(cx, |d, cx| d.viewed_count(cx)).0, 0);
    });
}

#[test]
fn the_keyboard_shortcuts_page_lists_a_diffs_keys() {
    let groups = trek_app::keys::shortcut_groups(trek_app::keys::is_mac(), false);
    let (name, rows) = groups.last().unwrap();
    assert_eq!(*name, "Diff review");
    for row in [
        "Next hunk",
        "Next file",
        "Comment on the line",
        "Mark the file viewed",
        "Stage or unstage the hunk",
    ] {
        assert!(rows.iter().any(|(n, _)| *n == row), "{row}");
    }
}
