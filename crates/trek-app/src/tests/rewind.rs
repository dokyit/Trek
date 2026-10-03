//! Taking a conversation back: rewinding to a message, undoing and retrying a turn, editing and
//! resending a message, and forking, with the files checkpointed in a git repo along the way. The
//! mock agent keeps a history like a real agent's session, so `recall` shows what it knows.

use super::harness::{Trek, mock, open, open_with, run};
use crate::workspace::{ForkAt, ItemRef, Route, Scope};
use gpui_kit::{AppContext as _, TestAppContext};
use gpui_kit::test::TestWindowExt as _;
use std::path::Path;
use std::time::{Duration, Instant};
use trek_core::rewind::Reopen;
use trek_core::store::{Item, ResumePoint};
use trek_core::{AgentId, Effort, HandHolding, RunState, ThreadSource};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Make the project a git repo with one commit: `notes.txt` saying "v1".
fn git_project(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("notes.txt"), "v1\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgSign=false", "commit", "-qm", "init"]);
}

fn read(dir: &Path, file: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(file)).ok()
}

/// The user's messages in `id`, in order.
fn said(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<String> {
    trek.items(cx, id).into_iter().filter_map(|i| if let Item::User { text, .. } = i { Some(text) } else { None }).collect()
}

fn user_ix(trek: &Trek, cx: &TestAppContext, id: &str, text: &str) -> usize {
    trek.item_ix(cx, id, |i| matches!(i, Item::User { text: t, .. } if t == text))
}

fn last_end(trek: &Trek, cx: &TestAppContext, id: &str) -> usize {
    trek.items(cx, id).iter().rposition(|i| matches!(i, Item::TurnEnd { .. })).expect("a finished turn")
}

/// Scroll the transcript to message `ix` and put the pointer on it (its actions show).
fn hover_message(trek: &Trek, cx: &mut TestAppContext, id: &str, ix: usize) {
    trek.update(cx, |ws, cx| ws.open_thread_at(id, ItemRef::Position(ix), cx));
    trek.render(cx);
    trek.window(cx, |window, cx| window.hover(("user-msg", ix), cx));
}

fn set_composer(trek: &Trek, cx: &mut TestAppContext, text: &str) {
    let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
    trek.window(cx, |window, cx| composer.update(cx, |c, cx| c.set_text(text, window, cx)));
    cx.run_until_parked();
}

/// Send `text` from the main window (starting a thread on a draft) and wait for its turn to
/// finish. Returns the thread.
async fn turn(trek: &Trek, cx: &mut TestAppContext, text: &str) -> String {
    trek.update(cx, |ws, cx| ws.send(text.into(), vec![], cx));
    let id = trek.thread_id(cx);
    trek.wait_done(cx, &id, RunState::Idle).await;
    id
}

/// What the agent remembers of the conversation, as it says when asked.
async fn recall(trek: &Trek, cx: &mut TestAppContext, id: &str) -> String {
    trek.update(cx, |ws, cx| ws.send_to(id, "recall".into(), vec![], cx));
    trek.wait_done(cx, id, RunState::Idle).await;
    let answers = trek.answers(cx, id);
    answers.lines().last().unwrap_or_default().to_string()
}

/// Wait (in real time: git runs on other threads) until the open confirmation has checked the files.
async fn files_checked(trek: &Trek, cx: &mut TestAppContext) -> String {
    let view = trek.thread_view(cx);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        match view.read_with(cx, |v, _| v.confirm_files()) {
            Some(f) if f != "checking" => return f,
            None => panic!("no confirmation open"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "timed out checking the files");
        cx.background_executor.timer(Duration::from_millis(5)).await;
    }
}

/// Wait until `f` holds for the project's files.
async fn files(trek: &Trek, cx: &mut TestAppContext, what: &str, f: impl Fn(&Path) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !f(&trek.project) {
        cx.run_until_parked();
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        cx.background_executor.timer(Duration::from_millis(5)).await;
    }
}

/// Two turns ("apple", then "banana") in a git project, with the files changed after each as
/// the agent would have: after the first, notes.txt says "v2" and new.txt exists; after the
/// second, notes.txt says "v3", extra.txt exists and new.txt is gone.
async fn two_turns(trek: &Trek, cx: &mut TestAppContext) -> String {
    git_project(&trek.project);
    let id = turn(trek, cx, "apple").await;
    std::fs::write(trek.project.join("notes.txt"), "v2\n").unwrap();
    std::fs::write(trek.project.join("new.txt"), "made by turn one\n").unwrap();
    turn(trek, cx, "banana").await;
    std::fs::write(trek.project.join("notes.txt"), "v3\n").unwrap();
    std::fs::write(trek.project.join("extra.txt"), "made by turn two\n").unwrap();
    std::fs::remove_file(trek.project.join("new.txt")).unwrap();
    id
}

#[test]
fn rewinding_takes_back_the_conversation_the_files_and_what_the_agent_knows() {
    run(async |cx| {
        let trek = open(cx);
        let id = two_turns(&trek, cx).await;
        // Each turn's first message got a checkpoint, in the repo and in the store.
        let checkpoints = trek.read(cx, |ws, _| ws.store.checkpoints(&id).unwrap());
        assert_eq!(checkpoints.len(), 2);
        assert_eq!(git(&trek.project, &["for-each-ref", "--format=%(refname)", "refs/trek/checkpoints/"]).lines().count(), 2);
        let head = git(&trek.project, &["rev-parse", "HEAD"]);

        // "Rewind to here" on the second message: the popover lists what restoring changes.
        let banana = user_ix(&trek, cx, &id, "banana");
        hover_message(&trek, cx, &id, banana);
        trek.click(cx, ("rewind-user", banana));
        assert!(trek.visible(cx, "confirm-card"));
        assert_eq!(files_checked(&trek, cx).await, "changes: extra.txt new.txt notes.txt");
        trek.click(cx, "confirm-go");

        files(&trek, cx, "the files as they were before \"banana\"", |p| {
            read(p, "notes.txt").as_deref() == Some("v2\n") && read(p, "new.txt").is_some() && read(p, "extra.txt").is_none()
        })
        .await;
        assert_eq!(said(&trek, cx, &id), ["apple"]);
        assert!(matches!(trek.items(cx, &id).last(), Some(Item::TurnEnd { .. })), "the first turn is left whole");
        assert_eq!(trek.composer_text(cx), "banana", "the message is back in the composer");
        assert_eq!(git(&trek.project, &["rev-parse", "HEAD"]), head, "HEAD untouched");
        // The rewound message's checkpoint went with it.
        let left: Vec<String> = trek.read(cx, |ws, _| ws.store.checkpoints(&id).unwrap()).into_iter().map(|c| c.item_id).collect();
        assert_eq!(left, [checkpoints[0].item_id.clone()]);
        // The agent was taken back too (its own session, cut back): it never heard "banana".
        assert!(matches!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().reopen.clone()), Some(Reopen::Native { fork: false, .. })));
        assert_eq!(recall(&trek, cx, &id).await, "I remember: apple");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().reopen.clone()), None, "picked up by the new session");
        // What was saved is what's on screen.
        assert_eq!(trek.read(cx, |ws, _| ws.store.items(&id).unwrap()), trek.items(cx, &id));
    });
}

#[test]
fn undoing_a_turn_can_leave_the_files_alone() {
    run(async |cx| {
        let trek = open(cx);
        let id = two_turns(&trek, cx).await;
        let end = last_end(&trek, cx, &id);
        // The latest turn's actions are always on show.
        trek.click(cx, ("undo-turn", end));
        assert_eq!(files_checked(&trek, cx).await, "changes: extra.txt new.txt notes.txt");
        trek.click(cx, "restore-files");
        trek.click(cx, "confirm-go");
        assert_eq!(said(&trek, cx, &id), ["apple"]);
        assert_eq!(trek.composer_text(cx), "banana");
        // Nothing to wait for: no restore was queued.
        assert_eq!(read(&trek.project, "notes.txt").as_deref(), Some("v3\n"));
        assert!(read(&trek.project, "extra.txt").is_some());
        assert_eq!(recall(&trek, cx, &id).await, "I remember: apple");
    });
}

#[test]
fn edit_and_resend_replaces_the_message() {
    run(async |cx| {
        let trek = open(cx);
        let id = two_turns(&trek, cx).await;
        set_composer(&trek, cx, "a draft of mine");
        let apple = user_ix(&trek, cx, &id, "apple");
        hover_message(&trek, cx, &id, apple);
        trek.click(cx, ("edit-user", apple));
        assert!(trek.visible(cx, "edit-banner"));
        assert_eq!(trek.composer_text(cx), "apple");
        // Esc puts the draft back.
        trek.press(cx, "escape");
        assert!(!trek.visible(cx, "edit-banner"));
        assert_eq!(trek.composer_text(cx), "a draft of mine");

        hover_message(&trek, cx, &id, apple);
        trek.click(cx, ("edit-user", apple));
        set_composer(&trek, cx, "cherry");
        trek.press(cx, "enter");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // The edit took the first message's place; the files went back to before it.
        files(&trek, cx, "the files as they were before \"apple\"", |p| read(p, "notes.txt").as_deref() == Some("v1\n") && read(p, "extra.txt").is_none()).await;
        assert_eq!(said(&trek, cx, &id), ["cherry"]);
        assert!(!trek.visible(cx, "edit-banner"));
        assert_eq!(trek.composer_text(cx), "a draft of mine", "the draft from before the edit is back");
        // The first message started the session: the edit starts a new one.
        assert_eq!(recall(&trek, cx, &id).await, "I remember: cherry");
        // The edited message has a checkpoint of its own.
        assert_eq!(trek.read(cx, |ws, _| ws.store.checkpoints(&id).unwrap().len()), 2);
    });
}

#[test]
fn retry_sends_the_message_again_with_the_model_asked_for() {
    run(async |cx| {
        let trek = open(cx);
        let id = two_turns(&trek, cx).await;
        let answers_before = trek.answers(cx, &id);
        let end = last_end(&trek, cx, &id);
        trek.click(cx, ("retry-turn", end));
        files_checked(&trek, cx).await;
        trek.click(cx, "confirm-go");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(said(&trek, cx, &id), ["apple", "banana"]);
        assert_eq!(trek.answers(cx, &id), answers_before, "the same message, answered again");
        files(&trek, cx, "the files restored", |p| read(p, "notes.txt").as_deref() == Some("v2\n")).await;

        // "Retry with…" another model of the same agent, from the chevron's menu.
        let end = last_end(&trek, cx, &id);
        let other = trek.read(cx, |ws, _| {
            // The thread has no model of its own: it runs the agent's default, which the menu
            // leaves out (picking it would be a plain retry).
            let models = ws.models_for(&mock());
            let current = ws.thread(&id).unwrap().model.clone().or_else(|| crate::composer::default_model(&models).map(|m| m.id.clone())).unwrap();
            models.into_iter().find(|m| !crate::composer::same_model(&current, &m.id)).unwrap().id
        });
        trek.click(cx, ("retry-with", end));
        // The menu's first row is its label; the models follow.
        trek.window(cx, |window, cx| window.within("popup-menu").click(1usize, cx));
        cx.run_until_parked();
        assert!(trek.visible(cx, "confirm-card"));
        // The files are as they were when it was sent (the retry above put them back).
        assert_eq!(files_checked(&trek, cx).await, "changes: ");
        trek.click(cx, "confirm-go");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().model.clone()), Some(other));
        assert_eq!(said(&trek, cx, &id), ["apple", "banana"]);
        assert_eq!(recall(&trek, cx, &id).await, "I remember: apple | banana");
    });
}

#[test]
fn forking_copies_the_conversation_and_leaves_the_original_alone() {
    run(async |cx| {
        let trek = open(cx);
        let id = two_turns(&trek, cx).await;
        let first_end = trek.item_ix(cx, &id, |i| matches!(i, Item::TurnEnd { .. }));
        let title = trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone());
        // An earlier turn's actions show while the pointer is on its footer.
        trek.window(cx, |window, cx| window.hover(("copy-turn", first_end), cx));
        trek.click(cx, ("fork-turn", first_end));
        let fork = trek.thread_id(cx);
        assert_ne!(fork, id, "the fork opens");
        let t = trek.read(cx, |ws, _| ws.thread(&fork).cloned().unwrap());
        assert_eq!((t.title.as_str(), t.agent.clone(), t.cwd.as_deref()), (format!("{title} (fork)").as_str(), mock(), Some(trek.project.as_path())));
        assert_eq!(said(&trek, cx, &fork), ["apple"]);
        assert!(matches!(t.reopen, Some(Reopen::Native { fork: true, .. })));
        // Its message keeps its checkpoint (the fork can take its files back too).
        trek.wait(cx, "the fork's checkpoints", |ws| ws.store.checkpoints(&fork).is_ok_and(|c| c.len() == 1)).await;
        let link = trek.read(cx, |ws, _| ws.store.checkpoints(&fork).unwrap()[0].clone());
        assert_eq!(git(&trek.project, &["rev-parse", &trek_core::checkpoint::ref_name(&fork, &link.item_id)]), link.sha);
        // The agent's session was forked at that point; the original's is as it was.
        assert_eq!(recall(&trek, cx, &fork).await, "I remember: apple");
        assert_eq!(said(&trek, cx, &id), ["apple", "banana"]);
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        assert_eq!(recall(&trek, cx, &id).await, "I remember: apple | banana");

        // From a message: the conversation before it, and the message waiting in the composer.
        let banana = user_ix(&trek, cx, &id, "banana");
        let before = trek.update(cx, |ws, cx| ws.fork_thread(&id, ForkAt::Before(ws.live[&id].items.id_at(banana).unwrap().to_string()), &Scope::Main, cx)).unwrap();
        assert_eq!(said(&trek, cx, &before), ["apple"]);
        assert_eq!(trek.composer_text(cx), "banana");

        // "Fork thread" from the sidebar's menu: all of it.
        set_composer(&trek, cx, "");
        trek.render(cx);
        trek.window(cx, |window, cx| window.right_click(format!("card-{id}"), cx));
        cx.run_until_parked();
        // After "Open in new window".
        trek.window(cx, |window, cx| window.within("popup-menu").click(1usize, cx));
        cx.run_until_parked();
        let whole = trek.thread_id(cx);
        assert!(whole != id && whole != fork && whole != before, "the fork opens");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&whole).unwrap().title.clone()), format!("{title} (fork)"));
        assert_eq!(said(&trek, cx, &whole), said(&trek, cx, &id));
        assert_eq!(recall(&trek, cx, &whole).await, "I remember: apple | banana | recall");
    });
}

#[test]
fn agents_that_cant_rewind_their_session_get_a_recap() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.default_model = Some(trek_agents::mock::RECAP_MODEL.into()));
        let id = turn(&trek, cx, "apple").await;
        turn(&trek, cx, "banana").await;
        turn(&trek, cx, "cherry").await;
        let cherry = trek.read(cx, |ws, _| ws.live[&id].items.ids()[user_ix(&trek, cx, &id, "cherry")].clone());
        let back = trek.update(cx, |ws, cx| ws.rewind(&id, &cherry, true, cx));
        assert_eq!(back.map(|(t, _)| t).as_deref(), Some("cherry"));
        // Said in the transcript, so the user knows.
        assert!(matches!(trek.items(cx, &id).last(), Some(Item::Notice { text }) if text.contains("recap")));
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().reopen.clone()), Some(Reopen::Recap));
        // A new session, told what was said before.
        assert_eq!(recall(&trek, cx, &id).await, "I remember: apple | banana");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().reopen.clone()), None);
    });
}

#[test]
fn a_session_with_no_history_of_its_own_restarts_with_a_recap() {
    run(async |cx| {
        let trek = open(cx);
        let id = turn(&trek, cx, "apple").await;
        turn(&trek, cx, "banana").await;
        // Like a direct model's (it keeps none), and the session ends: idle, or a relaunch.
        trek.update(cx, |ws, _| {
            ws.threads.iter_mut().find(|t| t.id == id).unwrap().native_id = None;
            let _ = ws.live.get_mut(&id).unwrap().commands.take().unwrap().try_send(trek_agents::Command::Shutdown);
        });
        assert_eq!(recall(&trek, cx, &id).await, "I remember: apple | banana");
    });
}

#[test]
fn settings_read_at_launch_apply_once_the_turn_is_over() {
    run(async |cx| {
        let trek = open(cx);
        let id = turn(&trek, cx, "apple").await;
        trek.update(cx, |ws, cx| ws.send_to(&id, "mock:long 600s".into(), vec![], cx));
        trek.wait(cx, "the long turn", |ws| ws.turn_running(&id)).await;
        // Plan mode is read at launch: the running turn isn't cut short for it.
        trek.update(cx, |ws, cx| {
            let mut p = ws.prefs_in(&Scope::Main);
            p.plan = true;
            ws.set_prefs_in(&Scope::Main, p, cx);
        });
        assert!(trek.read(cx, |ws, _| ws.live[&id].commands.is_some() && ws.turn_running(&id)));
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| ws.live[&id].commands.is_none()), "the session restarts after the turn");
        // It resumes: the conversation goes on.
        assert_eq!(recall(&trek, cx, &id).await, "I remember: apple | mock:long 600s");
    });
}

#[test]
fn nothing_is_taken_back_while_a_turn_runs() {
    run(async |cx| {
        let trek = open(cx);
        let id = turn(&trek, cx, "apple").await;
        trek.update(cx, |ws, cx| ws.send_to(&id, "mock:long 600s".into(), vec![], cx));
        trek.wait(cx, "the long turn", |ws| ws.turn_running(&id)).await;
        let apple = user_ix(&trek, cx, &id, "apple");
        let item = trek.read(cx, |ws, _| ws.live[&id].items.ids()[apple].clone());
        assert!(trek.update(cx, |ws, cx| ws.rewind(&id, &item, false, cx)).is_none());
        hover_message(&trek, cx, &id, apple);
        trek.click(cx, ("rewind-user", apple));
        assert_eq!(trek.thread_view(cx).read_with(cx, |v, _| v.confirm_files()), None, "nothing to confirm");
        assert_eq!(said(&trek, cx, &id).len(), 2);
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.update(cx, |ws, cx| ws.rewind(&id, &item, false, cx)).is_some());
        assert!(said(&trek, cx, &id).is_empty());
    });
}

#[test]
fn folders_outside_git_rewind_without_files() {
    run(async |cx| {
        let trek = open(cx);
        let id = turn(&trek, cx, "apple").await;
        turn(&trek, cx, "banana").await;
        assert!(trek.read(cx, |ws, _| ws.store.checkpoints(&id).unwrap().is_empty()));
        let end = last_end(&trek, cx, &id);
        trek.click(cx, ("undo-turn", end));
        assert_eq!(files_checked(&trek, cx).await, "unavailable");
        trek.click(cx, "confirm-go");
        assert_eq!(said(&trek, cx, &id), ["apple"]);
    });
}

#[test]
fn edits_go_to_the_window_they_were_asked_in() {
    run(async |cx| {
        let trek = open(cx);
        let id = turn(&trek, cx, "apple").await;
        let own = trek.open_thread_window(cx, &id);
        let apple = user_ix(&trek, cx, &id, "apple");
        cx.update_window(own, |_, window, cx| {
            window.hover(("user-msg", apple), cx);
            window.click(("edit-user", apple), cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(trek.visible_in(cx, own, "edit-banner"));
        assert!(!trek.visible(cx, "edit-banner"), "not the main window's composer");
        cx.update_window(own, |_, window, cx| {
            window.input(" pie", cx);
            window.press("enter", cx);
        })
        .unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(said(&trek, cx, &id), ["apple pie"]);
    });
}

#[test]
fn commands_trek_answers_are_neither_turns_nor_rewind_points() {
    run(async |cx| {
        let trek = open(cx);
        let id = turn(&trek, cx, "apple").await;
        trek.update(cx, |ws, cx| ws.send_to(&id, "/model".into(), vec![], cx));
        let model = user_ix(&trek, cx, &id, "/model");
        turn(&trek, cx, "banana").await;
        // Undoing the last turn takes back "banana", not the command before it.
        let end = last_end(&trek, cx, &id);
        let end_id = trek.read(cx, |ws, _| ws.live[&id].items.id_at(end).unwrap().to_string());
        let start = trek.read(cx, |ws, _| ws.turn_start_item(&id, &end_id)).unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.live[&id].items.position(&start)), Some(user_ix(&trek, cx, &id, "banana")));
        trek.click(cx, ("undo-turn", end));
        files_checked(&trek, cx).await;
        trek.click(cx, "confirm-go");
        assert_eq!(said(&trek, cx, &id), ["apple", "/model"]);
        assert_eq!(trek.composer_text(cx), "banana");
        // The agent's own session was cut back, not swapped for a recap.
        assert!(matches!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().reopen.clone()), Some(Reopen::Native { fork: false, .. })));
        assert_eq!(recall(&trek, cx, &id).await, "I remember: apple");
        // The command's row has only copy.
        hover_message(&trek, cx, &id, model);
        assert!(trek.visible(cx, ("copy-user", model)));
        assert!(!trek.visible(cx, ("rewind-user", model)) && !trek.visible(cx, ("edit-user", model)));
    });
}

#[test]
fn threads_that_dont_know_where_their_session_stands_find_out_before_sending() {
    run(async |cx| {
        let trek = open(cx);
        let id = turn(&trek, cx, "apple").await;
        // As an imported thread, or one an older Trek kept: no point recorded.
        trek.update(cx, |ws, cx| {
            ws.live.get_mut(&id).unwrap().mark = None;
            ws.store.update_thread(&id, |t| t.native_at = None).unwrap();
            ws.reload(cx);
        });
        turn(&trek, cx, "banana").await;
        let banana = user_ix(&trek, cx, &id, "banana");
        assert!(
            matches!(&trek.items(cx, &id)[banana], Item::User { resume: Some(ResumePoint { after: Some(_), .. }), .. }),
            "read from the agent's session before the message went"
        );
        let item = trek.read(cx, |ws, _| ws.live[&id].items.ids()[banana].clone());
        assert!(trek.update(cx, |ws, cx| ws.rewind(&id, &item, false, cx)).is_some());
        assert!(matches!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().reopen.clone()), Some(Reopen::Native { fork: false, .. })));
        assert_eq!(recall(&trek, cx, &id).await, "I remember: apple");
    });
}

#[test]
fn a_restore_can_be_undone() {
    run(async |cx| {
        let trek = open(cx);
        let id = two_turns(&trek, cx).await;
        let end = last_end(&trek, cx, &id);
        trek.click(cx, ("undo-turn", end));
        files_checked(&trek, cx).await;
        trek.click(cx, "confirm-go");
        files(&trek, cx, "the files restored", |p| read(p, "notes.txt").as_deref() == Some("v2\n")).await;
        // The toast's Undo puts back what the restore replaced.
        let sha = git(&trek.project, &["rev-parse", &trek_core::checkpoint::undo_ref(&id)]);
        trek.update(cx, |ws, cx| ws.undo(crate::workspace::UndoAction::Unrestore { thread: id.clone(), repo: trek.project.clone(), sha }, cx));
        files(&trek, cx, "the files as they were", |p| read(p, "notes.txt").as_deref() == Some("v3\n") && read(p, "extra.txt").is_some() && read(p, "new.txt").is_none()).await;
    });
}

#[test]
fn failed_and_stopped_turns_can_be_retried() {
    run(async |cx| {
        let trek = open(cx);
        let id = turn(&trek, cx, "apple").await;
        trek.update(cx, |ws, cx| ws.send_to(&id, "mock:error".into(), vec![], cx));
        trek.wait_done(cx, &id, RunState::Failed).await;
        let failed = trek.item_ix(cx, &id, |i| matches!(i, Item::Error { .. }));
        trek.click(cx, ("retry-turn", failed));
        files_checked(&trek, cx).await;
        trek.click(cx, "confirm-go");
        trek.wait_done(cx, &id, RunState::Failed).await;
        assert_eq!(said(&trek, cx, &id), ["apple", "mock:error"], "sent again in its place");

        // A stopped turn: undo from its "Interrupted" line.
        let failed = trek.item_ix(cx, &id, |i| matches!(i, Item::Error { .. }));
        trek.click(cx, ("undo-turn", failed));
        files_checked(&trek, cx).await;
        trek.click(cx, "confirm-go");
        assert_eq!(trek.composer_text(cx), "mock:error");
        set_composer(&trek, cx, "");
        trek.update(cx, |ws, cx| ws.send_to(&id, "mock:long 600s".into(), vec![], cx));
        trek.wait(cx, "the long turn", |ws| ws.turn_running(&id)).await;
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let stopped = trek.item_ix(cx, &id, |i| matches!(i, Item::Notice { text } if text == "Interrupted"));
        trek.render(cx);
        trek.click(cx, ("undo-turn", stopped));
        files_checked(&trek, cx).await;
        trek.click(cx, "confirm-go");
        assert_eq!(said(&trek, cx, &id), ["apple"]);
        assert_eq!(trek.composer_text(cx), "mock:long 600s");
    });
}

#[test]
fn a_stop_while_the_checkpoint_is_taken_keeps_the_message_from_the_agent() {
    run(async |cx| {
        let trek = open(cx);
        git_project(&trek.project);
        let id = turn(&trek, cx, "apple").await;
        // A clean filter slow enough that the stop lands while the checkpoint is being taken.
        git(&trek.project, &["config", "filter.slow.clean", "sleep 1; cat"]);
        std::fs::write(trek.project.join(".gitattributes"), "*.big filter=slow\n").unwrap();
        std::fs::write(trek.project.join("data.big"), "lots\n").unwrap();
        // Stopped before the git work gets to run.
        trek.update(cx, |ws, cx| {
            ws.send_to(&id, "banana".into(), vec![], cx);
            ws.interrupt(&id, cx);
        });
        assert!(!trek.read(cx, |ws, _| ws.turn_running(&id)), "stopped at once");
        assert!(matches!(trek.items(cx, &id).last(), Some(Item::Notice { text }) if text == "Interrupted"));
        // The checkpoint still lands; the agent never heard "banana".
        trek.wait(cx, "the checkpoint", |ws| ws.store.checkpoints(&id).is_ok_and(|c| c.len() == 2)).await;
        assert_eq!(recall(&trek, cx, &id).await, "I remember: apple");
    });
}

#[test]
fn deleting_a_thread_drops_its_checkpoints() {
    run(async |cx| {
        let trek = open(cx);
        let id = two_turns(&trek, cx).await;
        let refs = |p: &Path| git(p, &["for-each-ref", "--format=%(refname)", "refs/trek/"]);
        assert!(!refs(&trek.project).is_empty());
        trek.update(cx, |ws, cx| ws.delete_thread(&id, cx));
        files(&trek, cx, "the refs to go", |p| refs(p).is_empty()).await;
        assert!(trek.read(cx, |ws, _| ws.store.checkpoints(&id).unwrap().is_empty()));
    });
}

#[test]
fn imported_threads_rewind_and_fork_their_agents_session() {
    run(async |cx| {
        let trek = open(cx);
        // A Claude Code session imported from its history, with where each message stood in it.
        let point = |after: Option<&str>| Some(ResumePoint { session: "claude-s1".into(), after: after.map(String::from) });
        let user = |t: &str, after: Option<&str>| Item::User { text: t.into(), images: vec![], at: None, resume: point(after), aside: false };
        let id = trek.update(cx, |ws, cx| {
            let mut t = ws.store.create_thread(Some(&trek.project), AgentId::ClaudeCode, None, Effort::Low, HandHolding::Auto).unwrap();
            t.source = ThreadSource::ClaudeCode;
            t.native_id = Some("claude-s1".into());
            ws.store.save_thread(&t).unwrap();
            super::harness::store_items(&ws.store, &t.id, vec![user("one", None), Item::Assistant { text: "1".into() }, user("two", Some("msg-a")), Item::Assistant { text: "2".into() }]);
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            t.id
        });
        trek.render(cx);
        let two = trek.read(cx, |ws, _| ws.live[&id].items.ids()[2].clone());
        // A fork from there resumes a copy of the session cut back to the message before it.
        let fork = trek.update(cx, |ws, cx| ws.fork_thread(&id, ForkAt::Before(two.clone()), &Scope::Main, cx)).unwrap();
        let f = trek.read(cx, |ws, _| ws.thread(&fork).cloned().unwrap());
        assert_eq!(f.reopen, Some(Reopen::Native { session: "claude-s1".into(), at: Some("msg-a".into()), fork: true }));
        assert_eq!(f.source, ThreadSource::Trek);
        // Rewinding the original cuts its own session back in place; past messages have no
        // checkpoints, so there are no files to restore.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        let two_ix = 2usize;
        hover_message(&trek, cx, &id, two_ix);
        trek.click(cx, ("rewind-user", two_ix));
        assert_eq!(files_checked(&trek, cx).await, "unavailable");
        trek.click(cx, "confirm-go");
        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned().unwrap());
        assert_eq!(t.reopen, Some(Reopen::Native { session: "claude-s1".into(), at: Some("msg-a".into()), fork: false }));
        assert_eq!((t.native_id.as_deref(), t.source), (Some("claude-s1"), ThreadSource::ClaudeCode));
        assert_eq!(said(&trek, cx, &id), ["one"]);
        // "two" is back in the composer, which warms a session up for it: in tests that's never
        // the real Claude Code, and the thread says so.
        assert!(trek.composer_text(cx).ends_with("two"));
        trek.wait(cx, "the warm-up to end", |ws| ws.live[&id].commands.is_none()).await;
        assert!(trek.items(cx, &id).iter().any(|i| matches!(i, Item::Error { text } if text == "Claude Code isn't started in tests.")), "{:?}", trek.items(cx, &id));
    });
}

/// The whole path with a real agent: checkpoints, a rewind that restores files and cuts the
/// agent's session back, and a fork. Not run by default (it costs a few tiny turns):
/// `TREK_LIVE_AGENT=claude` (claude-haiku-4-5) or `codex` (gpt-5.6-luna), with
/// `cargo test -p trek-app live_rewind -- --ignored`. Works in /tmp/trek-timetravel-e2e.
#[test]
#[ignore = "live: runs a real agent"]
fn live_rewind_and_fork() {
    let (agent, model) = match std::env::var("TREK_LIVE_AGENT").as_deref() {
        Ok("codex") => (AgentId::Codex, "gpt-5.6-luna"),
        _ => (AgentId::ClaudeCode, "claude-haiku-4-5"),
    };
    run(async |cx| {
        let trek = open_with(cx, |s| {
            s.general.default_agent = agent.key();
            s.general.default_model = Some(model.into());
            s.general.default_effort = Effort::Low;
        });
        let project = std::path::PathBuf::from("/tmp/trek-timetravel-e2e").join(format!("app-{}", agent.key()));
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(&project).unwrap();
        git_project(&project);
        trek.update(cx, |ws, cx| {
            ws.store.ensure_project(&project).unwrap();
            ws.reload(cx);
            ws.navigate(Route::Draft { project: Some(project.clone()) }, cx);
        });
        let done = async |cx: &mut TestAppContext, id: &str| {
            let deadline = Instant::now() + Duration::from_secs(240);
            loop {
                cx.run_until_parked();
                let state = trek.read(cx, |ws, _| (ws.thread(id).map(|t| t.run_state), ws.turn_running(id)));
                match state {
                    (Some(RunState::Idle), false) => return,
                    (Some(RunState::Failed), _) => panic!("the turn failed: {:?}", trek.items(cx, id)),
                    _ => {}
                }
                assert!(Instant::now() < deadline, "timed out");
                cx.background_executor.timer(Duration::from_millis(50)).await;
            }
        };
        trek.update(cx, |ws, cx| ws.send("Just for this conversation (don't save it to memory or to any file), remember the word APPLE. Reply with just OK.".into(), vec![], cx));
        let id = trek.thread_id(cx);
        done(cx, &id).await;
        // Forget where the session stands, as an imported thread (or one an older Trek kept)
        // would: the next message reads it from the agent's own files.
        trek.update(cx, |ws, cx| {
            ws.live.get_mut(&id).unwrap().mark = None;
            ws.store.update_thread(&id, |t| t.native_at = None).unwrap();
            ws.reload(cx);
        });
        std::fs::write(project.join("notes.txt"), "v2\n").unwrap();
        let banana_text = "Just for this conversation (don't save it to memory or to any file), also remember the word BANANA. Reply with just OK.";
        trek.update(cx, |ws, cx| ws.send_to(&id, banana_text.into(), vec![], cx));
        done(cx, &id).await;
        std::fs::write(project.join("notes.txt"), "v3\n").unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.store.checkpoints(&id).unwrap().len()), 2);

        let banana = trek.read(cx, |ws, _| ws.live[&id].items.ids()[user_ix(&trek, cx, &id, banana_text)].clone());
        let point = trek.read(cx, |ws, _| ws.live[&id].items.get(ws.live[&id].items.position(&banana).unwrap()).cloned());
        assert!(matches!(point, Some(Item::User { resume: Some(ResumePoint { after: Some(_), .. }), .. })), "found in the agent's files: {point:?}");
        assert!(trek.update(cx, |ws, cx| ws.rewind(&id, &banana, true, cx)).is_some());
        files(&trek, cx, "notes.txt restored", |_| read(&project, "notes.txt").as_deref() == Some("v2\n")).await;
        assert!(matches!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().reopen.clone()), Some(Reopen::Native { fork: false, .. })));
        let ask = "List every word I asked you to remember, comma separated, nothing else.";
        trek.update(cx, |ws, cx| ws.send_to(&id, ask.into(), vec![], cx));
        done(cx, &id).await;
        let answer = trek.answers(cx, &id).to_uppercase();
        let last = answer.lines().last().unwrap_or_default().to_string();
        assert!(last.contains("APPLE") && !last.contains("BANANA"), "the agent forgot the rewound turn: {last}");

        let fork = trek.update(cx, |ws, cx| ws.fork_thread(&id, ForkAt::End, &Scope::Main, cx)).unwrap();
        trek.update(cx, |ws, cx| ws.send_to(&fork, "Just for this conversation (don't save it anywhere), also remember the word CHERRY. Then list every word I asked you to remember, comma separated, nothing else.".into(), vec![], cx));
        done(cx, &fork).await;
        let last = trek.answers(cx, &fork).to_uppercase().lines().last().unwrap_or_default().to_string();
        assert!(last.contains("APPLE") && last.contains("CHERRY") && !last.contains("BANANA"), "the fork carries on the conversation: {last}");
        let (a, b) = trek.read(cx, |ws, _| (ws.thread(&id).unwrap().native_id.clone(), ws.thread(&fork).unwrap().native_id.clone()));
        assert!(a.is_some() && b.is_some() && a != b, "the fork has a session of its own");
        println!("live {}: ok (sessions {a:?}, fork {b:?})", agent.key());
    });
}

#[test]
fn undoing_a_restore_waits_for_the_running_turn() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let toasts = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let sink = toasts.clone();
        cx.update(|cx| {
            cx.subscribe(&trek.ws, move |_, event: &crate::workspace::WorkspaceEvent, _| {
                if let crate::workspace::WorkspaceEvent::Toast { message, .. } = event {
                    sink.borrow_mut().push(message.clone());
                }
            })
            .detach()
        });
        // The resent turn has started when "Restored N files · Undo" is clicked.
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![trek_agents::AgentEvent::TextDelta("Editing the parser".into())], cx));
        let undo = crate::workspace::UndoAction::Unrestore { thread: id.clone(), repo: trek.project.clone(), sha: "0".repeat(40) };
        trek.update(cx, |ws, cx| ws.undo(undo, cx));
        assert_eq!(*toasts.borrow(), ["Stop the running turn first."], "nothing was put back under the agent");
    });
}
