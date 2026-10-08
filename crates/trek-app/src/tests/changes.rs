//! What a finished turn changed, under its answer: counted from the git checkpoints (changes made
//! outside the agent's edit tools too), a turn at a time, for live turns and history alike; from
//! the agent's edits outside git; no card for a turn that changed nothing. Folding it, its diff in
//! the Git tool. And the recap Trek asks agents for, and of whom.

use super::harness::{Trek, launch, mock, new_project, open, open_with, run, settings};
use super::worktrees::make_repo;
use gpui_kit::{SharedString, TestAppContext};
use std::process::Command;
use trek_core::store::{Item, Store, ToolStatus};
use trek_core::{Effort, HandHolding, RunState};

/// Where each of `id`'s turns ends.
fn ends(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<usize> {
    trek.items(cx, id).iter().enumerate().filter(|(_, i)| matches!(i, Item::TurnEnd { .. })).map(|(ix, _)| ix).collect()
}

/// Draw the thread (its turns' changes are worked out as their footers are drawn) and wait until
/// every turn's are in.
async fn settled(trek: &Trek, cx: &mut TestAppContext, id: &str) {
    for _ in 0..3 {
        trek.render(cx);
        let (id2, ends) = (id.to_string(), ends(trek, cx, id));
        trek.wait(cx, "the turns' changes", move |ws| ends.iter().all(|e| ws.turn_changes_settled(&id2, *e))).await;
    }
    trek.render(cx);
}

/// The turns' changes as their cards list them (`ThreadView::describe_changes`).
fn change_rows(trek: &Trek, cx: &TestAppContext) -> Vec<String> {
    trek.thread_view(cx).read_with(cx, |v, cx| v.describe_changes(cx))
}

fn row(end: usize, file: usize) -> SharedString {
    format!("changed-{end}-{file}").into()
}

#[test]
fn a_turn_that_changed_files_lists_them_under_its_answer() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        let id = trek.send(cx, "mock:write");
        trek.wait_done(cx, &id, RunState::Idle).await;
        settled(&trek, cx, &id).await;
        assert_eq!(change_rows(&trek, cx), ["changes (1): +3 −0", "  NOTES.md new +3 −0"]);
        let first = ends(&trek, cx, &id)[0];
        assert!(trek.visible(cx, ("turn-changes", first)) && trek.visible(cx, row(first, 0)));
        let card = trek.read(cx, |ws, _| ws.turn_changes(&id, first)).expect("the bridge sees it too");
        assert_eq!(card.root, trek.project.canonicalize().unwrap());

        // A second turn counts only its own change; the first still counts its own, now up to
        // the second's checkpoint rather than the files as they are.
        trek.update(cx, |ws, cx| ws.send_to(&id, "mock:write".into(), vec![], cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        settled(&trek, cx, &id).await;
        assert_eq!(change_rows(&trek, cx), ["changes (1): +3 −0", "  NOTES.md new +3 −0", "changes (1): +1 −0", "  NOTES.md changed +1 −0"]);
    });
}

#[test]
fn changes_made_outside_the_agents_edit_tools_count_too() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        let id = trek.send(cx, "mock:long 2s");
        trek.wait(cx, "the turn to start", |ws| ws.live.get(&id).is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::Tool { .. })))).await;
        // What a shell command of the turn's would do: a file written, one deleted.
        std::fs::create_dir_all(trek.project.join("src")).unwrap();
        std::fs::write(trek.project.join("src/gen.rs"), "pub fn a() {}\npub fn b() {}\n").unwrap();
        std::fs::remove_file(trek.project.join("README.md")).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        settled(&trek, cx, &id).await;
        assert_eq!(change_rows(&trek, cx), ["changes (2): +2 −1", "  README.md deleted +0 −1", "  src/gen.rs new +2 −0"]);
    });
}

#[test]
fn a_turn_that_changed_nothing_has_no_card() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        let id = trek.send(cx, "explain the startup");
        trek.wait_done(cx, &id, RunState::Idle).await;
        settled(&trek, cx, &id).await;
        assert!(change_rows(&trek, cx).is_empty());
        let end = ends(&trek, cx, &id)[0];
        assert!(trek.visible(cx, ("copy-turn", end)) && !trek.visible(cx, ("turn-changes", end)));
        assert_eq!(trek.read(cx, |ws, _| ws.turn_changes(&id, end)), None);
    });
}

#[test]
fn outside_git_the_agents_edits_are_counted() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:write");
        trek.wait_done(cx, &id, RunState::Idle).await;
        settled(&trek, cx, &id).await;
        assert_eq!(change_rows(&trek, cx), ["changes (1): +3 −0", "  NOTES.md changed +3 −0"]);
        let end = ends(&trek, cx, &id)[0];
        let counted = trek.read(cx, |ws, _| ws.turn_changes(&id, end).map(|c| c.counted));
        assert_eq!(counted, Some(trek_core::changes::Counted::EditTools));
        // No diff to show: no Review.
        assert!(trek.visible(cx, ("turn-changes", end)) && !trek.visible(cx, ("changes-review", end)));
    });
}

#[test]
fn a_rewind_counts_the_latest_turn_again() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        let id = trek.send(cx, "mock:write");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.send_to(&id, "mock:write".into(), vec![], cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        settled(&trek, cx, &id).await;
        // Taking the second turn back without its files: the first is the latest again, and runs
        // to the files as they are now (its note and the second's).
        let second = trek.items(cx, &id).iter().enumerate().filter(|(_, i)| matches!(i, Item::User { .. })).map(|(ix, _)| ix).nth(1).unwrap();
        let item = trek.read(cx, |ws, _| ws.live.get(&id).and_then(|l| l.items.id_at(second).map(str::to_string))).unwrap();
        trek.update(cx, |ws, cx| _ = ws.rewind(&id, &item, false, cx));
        settled(&trek, cx, &id).await;
        assert_eq!(change_rows(&trek, cx), ["changes (1): +4 −0", "  NOTES.md new +4 −0"]);
    });
}

#[test]
fn review_opens_the_turns_diff_and_a_row_opens_its_file() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        let id = trek.send(cx, "mock:long 2s");
        trek.wait(cx, "the turn to start", |ws| ws.live.get(&id).is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::Tool { .. })))).await;
        std::fs::create_dir_all(trek.project.join("src")).unwrap();
        std::fs::write(trek.project.join("src/gen.rs"), "pub fn a() {}\n").unwrap();
        std::fs::write(trek.project.join("README.md"), "hello\nworld\n").unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        settled(&trek, cx, &id).await;
        let end = ends(&trek, cx, &id)[0];
        assert!(trek.visible(cx, row(end, 0)) && trek.visible(cx, row(end, 1)));

        // Review: the Git tool shows the turn's files, the first one's diff open; a file's row
        // opens its own.
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.click(cx, ("changes-review", end));
        trek.render(cx);
        let shown = cx.read(|cx| panel.read(cx).git_turn(cx));
        assert_eq!(shown, Some((vec!["README.md".to_string(), "src/gen.rs".to_string()], Some("README.md".to_string()))));
        assert!(trek.visible(cx, "git-turn"));
        trek.click(cx, row(end, 1));
        trek.render(cx);
        let shown = cx.read(|cx| panel.read(cx).git_turn(cx));
        assert_eq!(shown.and_then(|(_, selected)| selected), Some("src/gen.rs".to_string()));
        // Back to the working tree.
        trek.click(cx, "git-turn-close");
        trek.render(cx);
        assert_eq!(cx.read(|cx| panel.read(cx).git_turn(cx)), None);
    });
}

#[test]
fn a_card_shows_eight_files_then_offers_the_rest() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        let id = trek.send(cx, "mock:long 2s");
        trek.wait(cx, "the turn to start", |ws| ws.live.get(&id).is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::Tool { .. })))).await;
        std::fs::create_dir_all(trek.project.join("src")).unwrap();
        for i in 0..10 {
            std::fs::write(trek.project.join(format!("src/f{i:02}.rs")), "fn x() {}\n").unwrap();
        }
        trek.wait_done(cx, &id, RunState::Idle).await;
        settled(&trek, cx, &id).await;
        let end = ends(&trek, cx, &id)[0];
        assert!(trek.visible(cx, row(end, 0)) && trek.visible(cx, row(end, 7)) && !trek.visible(cx, row(end, 8)));
        trek.click(cx, ("changes-more", end));
        trek.render(cx);
        assert!(trek.visible(cx, row(end, 8)) && trek.visible(cx, row(end, 9)), "the rest show");
        trek.click(cx, ("changes-more", end));
        trek.render(cx);
        assert!(!trek.visible(cx, row(end, 8)), "and fold away again");
    });
}

#[test]
fn the_cards_undo_asks_the_same_confirmation() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        let id = trek.send(cx, "mock:write");
        trek.wait_done(cx, &id, RunState::Idle).await;
        settled(&trek, cx, &id).await;
        let end = ends(&trek, cx, &id)[0];
        trek.click(cx, ("changes-undo", end));
        trek.render(cx);
        assert!(trek.visible(cx, "confirm-card"), "the card's Undo opens the turn's confirmation");
        // The confirmation opens below the turn-end row, not over the answer above it.
        let pop = trek.bounds(cx, "confirm-card").expect("the confirmation");
        let row = trek.bounds(cx, ("undo-turn", end)).expect("the undo button");
        assert!(pop.top() >= row.bottom(), "the popover ({pop:?}) must not rise over the row ({row:?})");
        trek.click(cx, "confirm-cancel");
        trek.render(cx);

        // A turn with many files ends on a row at the transcript's foot — as near the window's
        // bottom as a row gets. Its confirmation lists those files: too tall for the room below,
        // so it flips over the row rather than clamping back over it.
        trek.update(cx, |ws, cx| ws.send_to(&id, "mock:long 2s".into(), vec![], cx));
        trek.wait(cx, "the turn to start", |ws| ws.live.get(&id).is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::Tool { .. })))).await;
        std::fs::create_dir_all(trek.project.join("src")).unwrap();
        for i in 0..10 {
            std::fs::write(trek.project.join(format!("src/f{i:02}.rs")), "fn x() {}\n").unwrap();
        }
        trek.wait_done(cx, &id, RunState::Idle).await;
        settled(&trek, cx, &id).await;
        let end = *ends(&trek, cx, &id).last().unwrap();
        trek.click(cx, ("changes-undo", end));
        trek.render(cx);
        trek.render(cx);
        let pop = trek.bounds(cx, "confirm-card").expect("the confirmation");
        let row = trek.bounds(cx, ("undo-turn", end)).expect("the undo button");
        assert!(pop.bottom() <= row.top(), "the popover ({pop:?}) must not cover the row ({row:?})");
        trek.click(cx, "confirm-cancel");
    });
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(dir).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn history_from_the_store_has_its_cards() {
    run(async |cx| {
        let project = new_project("history");
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@example.com"], &["config", "user.name", "T"], &["config", "commit.gpgsign", "false"]] {
            git(&project, args);
        }
        std::fs::write(project.join("README.md"), "hello\n").unwrap();
        git(&project, &["add", "-A"]);
        git(&project, &["commit", "-qm", "init"]);
        // Two turns a Trek before this one ran and kept: their checkpoints, then what each did.
        let db = new_project("history-data").join("trek.sqlite");
        let id = {
            let store = Store::open(&db).expect("store");
            let thread = store.create_thread(Some(&project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            let user = |t: &str| Item::User { text: t.into(), images: vec![], at: Some(1), resume: None, aside: false };
            let edit = Item::Tool { id: "e1".into(), title: "Edit".into(), detail: "NOTES.md".into(), output: String::new(), status: ToolStatus::Done };
            let mut transcript = trek_core::transcript::Transcript::unsaved(vec![
                user("write a note"),
                edit,
                Item::Assistant { text: "Done.".into() },
                Item::TurnEnd { at: 2, took_secs: 3 },
                user("and tidy up"),
                Item::Assistant { text: "Done.".into() },
                Item::TurnEnd { at: 4, took_secs: 3 },
            ]);
            let ids = transcript.ids().to_vec();
            store.save_transcript(&thread.id, &mut transcript).expect("items");
            let repo = trek_core::checkpoint::Repo::find(&project).expect("repo");
            let checkpoint = |item: &str| store.add_checkpoint(&thread.id, item, &repo.top, &repo.snapshot(&thread.id, item).unwrap()).unwrap();
            checkpoint(&ids[0]);
            std::fs::write(project.join("NOTES.md"), "# Notes\n\n- Note 1\n").unwrap();
            checkpoint(&ids[4]);
            // The second turn ran a command: README.md went.
            std::fs::remove_file(project.join("README.md")).unwrap();
            thread.id
        };
        let mut s = settings();
        s.user_projects.push(project.display().to_string());
        let (ws, root, window) = launch(cx, Store::open(&db).expect("store"), s);
        let trek = Trek { ws, root, window, project };
        trek.update(cx, |ws, cx| ws.navigate(crate::workspace::Route::Thread(id.clone()), cx));
        trek.wait(cx, "the thread to load", |ws| ws.live.get(&id).is_some_and(|l| l.loaded)).await;
        settled(&trek, cx, &id).await;
        assert_eq!(change_rows(&trek, cx), ["changes (1): +3 −0", "  NOTES.md new +3 −0", "changes (1): +0 −1", "  README.md deleted +0 −1"]);
    });
}

#[test]
fn agents_are_asked_for_a_recap_unless_it_doesnt_fit() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:told");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let told = trek.answers(cx, &id);
        assert!(told.contains(trek_core::changes::RECAP), "{told}");
        // A side chat changes nothing: it isn't asked.
        let side = trek.update(cx, |ws, cx| ws.create_side_chat(cx)).expect("side chat");
        trek.update(cx, |ws, cx| ws.send_to(&side, "mock:told".into(), vec![], cx));
        trek.wait_done(cx, &side, RunState::Idle).await;
        let told = trek.answers(cx, &side);
        assert!(told.contains(trek_core::visualization::AGENT_INSTRUCTIONS), "{told}");
        assert!(!told.contains(trek_core::changes::RECAP), "{told}");
        // Nor is a sub-agent that only advises.
        trek.update(cx, |ws, cx| ws.navigate(crate::workspace::Route::Draft { project: Some(trek.project.clone()) }, cx));
        let parent = trek.send(cx, "mock:consult mock:told");
        trek.wait_done(cx, &parent, RunState::Idle).await;
        let child = trek.read(cx, |ws, _| ws.children(&parent).first().map(|t| t.id.clone())).expect("a sub-agent");
        let told = trek.answers(cx, &child);
        assert!(told.contains(trek_core::visualization::AGENT_INSTRUCTIONS), "{told}");
        assert!(!told.contains(trek_core::changes::RECAP), "{told}");
    });
}

#[test]
fn with_the_recap_off_agents_arent_asked_for_a_recap() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.ask_recap = false);
        let id = trek.send(cx, "mock:told");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let told = trek.answers(cx, &id);
        assert!(told.contains(trek_core::visualization::AGENT_INSTRUCTIONS), "{told}");
        assert!(!told.contains(trek_core::changes::RECAP), "{told}");
    });
}
