//! Threads in worktrees of their own, through the real window: starting one, the branch chips,
//! the review in the Git tool, merging and removing, a worktree that vanished, and the warning
//! for threads sharing a folder. Git runs for real, in repos under the test's temp folder.

use super::harness::{Trek, open, run};
use crate::workspace::{PanelTool, Route, Scope};
use crate::worktree_ui::{Leave, confirm_leave};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use std::path::Path;
use trek_core::RunState;
use trek_core::settings::ProjectAction;
use trek_core::store::Item;
use trek_core::worktree::{self, git};

/// Make the test's project a git repo on `main` with one commit, an ignored `.env`, and a draft
/// in it again (so the project's defaults and git state are read anew).
fn make_repo(trek: &Trek, cx: &mut TestAppContext) {
    let dir = &trek.project;
    for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@example.com"], &["config", "user.name", "T"], &["config", "commit.gpgsign", "false"]] {
        git(dir, args).expect("git");
    }
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    std::fs::write(dir.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(dir.join(".env"), "TOKEN=1\n").unwrap();
    git(dir, &["add", "-A"]).unwrap();
    git(dir, &["commit", "-qm", "init"]).unwrap();
    let project = dir.clone();
    trek.update(cx, |ws, cx| {
        ws.reload(cx);
        ws.navigate(Route::Draft { project: None }, cx);
        ws.navigate(Route::Draft { project: Some(project) }, cx);
    });
    trek.render(cx);
}

/// Pick "New worktree" for the draft, as its menu does.
fn use_worktree(trek: &Trek, cx: &mut TestAppContext) {
    trek.update(cx, |ws, cx| {
        let mut p = ws.prefs_in(&Scope::Main);
        p.worktree = true;
        ws.set_prefs_in(&Scope::Main, p, cx);
    });
}

fn clean(dir: &Path) -> bool {
    git(dir, &["status", "--porcelain"]).unwrap().trim().is_empty()
}

/// A thread in a worktree that has added a note (uncommitted) there.
async fn worktree_thread(trek: &Trek, cx: &mut TestAppContext) -> (String, worktree::Worktree) {
    make_repo(trek, cx);
    use_worktree(trek, cx);
    let id = trek.send(cx, "mock:write");
    trek.wait_done(cx, &id, RunState::Idle).await;
    let wt = trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.worktree.clone())).expect("a worktree");
    (id, wt)
}

#[test]
fn a_new_thread_can_run_in_a_worktree_of_its_own() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        // The draft offers where to run once the project's git state is in.
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| ws.refresh_git_at(project, cx));
        trek.render(cx);
        assert!(trek.visible(cx, "env-place"));
        use_worktree(&trek, cx);
        let id = trek.send(cx, "mock:write");
        trek.wait_done(cx, &id, RunState::Idle).await;

        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned()).unwrap();
        let wt = t.worktree.clone().expect("a worktree");
        assert_eq!((wt.branch.as_str(), wt.base.as_str()), ("trek/mock-write", "main"));
        assert!(wt.path.starts_with(worktree::worktrees_dir()));
        assert_eq!(t.cwd.as_ref(), Some(&wt.path), "the agent ran in the worktree");
        assert_eq!(trek.read(cx, |ws, _| ws.project_dir(&t)), Some(trek.project.clone()), "still the project's thread");
        // The agent's change landed there, not in the project folder; the env file came along.
        assert!(wt.path.join("NOTES.md").exists() && !trek.project.join("NOTES.md").exists());
        assert!(clean(&trek.project));
        assert_eq!(std::fs::read_to_string(wt.path.join(".env")).unwrap(), "TOKEN=1\n");
        assert_eq!(git(&wt.path, &["branch", "--show-current"]).unwrap().trim(), "trek/mock-write");
        assert!(trek.answers(cx, &id).contains("NOTES.md"));
        // Saved with the thread.
        assert_eq!(trek.read(cx, |ws, _| ws.store.thread(&id).unwrap().unwrap().worktree), Some(wt.clone()));

        // The branch shows on its card, in the title bar and in its own window; the composer
        // says where it runs.
        trek.render(cx);
        assert!(trek.visible(cx, format!("card-branch-{id}")));
        assert!(trek.visible(cx, "title-branch"));
        assert!(trek.visible(cx, "env-worktree"));
        let own = trek.open_thread_window(cx, &id);
        assert!(trek.visible_in(cx, own, "title-branch"));

        // The next new thread from here starts in the project folder again, not in this worktree.
        trek.update(cx, |ws, cx| ws.new_thread(cx));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: Some(trek.project.clone()) });
    });
}

#[test]
fn the_git_tool_reviews_commits_and_merges_a_worktree_thread() {
    run(async |cx| {
        let trek = open(cx);
        let (id, wt) = worktree_thread(&trek, cx).await;
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.window(cx, |window, cx| panel.update(cx, |p, cx| p.open_tool(PanelTool::Git, window, cx)));
        // The review is read off the UI thread; a frame after it's in shows it.
        trek.render(cx);
        assert!(trek.visible(cx, "gf-NOTES.md"), "the thread's new file is listed");
        assert!(trek.visible(cx, "git-merge"));

        // Uncommitted work blocks the merge, and says so.
        let blocked = trek.update(cx, |ws, cx| ws.merge_worktree(&id, cx)).await.unwrap();
        assert_eq!(blocked, Err(worktree::MergeBlock::Uncommitted(1)));
        assert!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().settled_at.is_none()));

        worktree::commit(&wt.path, "Add a note").unwrap();
        let task = trek.update(cx, |ws, cx| ws.merge_worktree(&id, cx));
        let tid = id.clone();
        trek.wait(cx, "the merge to settle the thread", |ws| ws.thread(&tid).is_some_and(|t| t.settled_at.is_some())).await;
        drop(task);
        assert!(trek.project.join("NOTES.md").exists(), "merged into the project folder");
        assert!(worktree::is_merged(&trek.project, &wt));

        // Removing a merged worktree takes its branch too; the thread moves to the project folder.
        let task = trek.update(cx, |ws, cx| ws.remove_worktree(&id, 0, false, cx));
        trek.wait(cx, "the worktree to go", |ws| ws.thread(&tid).is_some_and(|t| t.worktree.is_none())).await;
        drop(task);
        assert!(!wt.path.exists());
        assert!(git(&trek.project, &["show-ref", "--verify", "--quiet", "refs/heads/trek/mock-write"]).is_err());
        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned()).unwrap();
        assert_eq!((t.cwd, t.native_id), (Some(trek.project.clone()), None));
        assert!(matches!(trek.items(cx, &id).last(), Some(Item::Notice { text }) if text.contains("worktree was removed")));
    });
}

/// Open the Git tool on the thread on screen.
fn open_git(trek: &Trek, cx: &mut TestAppContext) {
    let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
    trek.window(cx, |window, cx| panel.update(cx, |p, cx| p.open_tool(PanelTool::Git, window, cx)));
    trek.render(cx);
}

/// Let a dialog that just opened finish animating in: a click goes where the last frame drew its
/// buttons, and they move while it scales in.
fn dialog_open(trek: &Trek, cx: &mut TestAppContext) {
    trek.render(cx);
    std::thread::sleep(std::time::Duration::from_millis(400));
    trek.render(cx);
}

fn last_subject(dir: &Path) -> String {
    git(dir, &["log", "-1", "--format=%s"]).unwrap().trim().to_string()
}

#[test]
fn the_git_tools_buttons_revert_commit_merge_and_remove() {
    run(async |cx| {
        let trek = open(cx);
        let (id, wt) = worktree_thread(&trek, cx).await;
        std::fs::write(wt.path.join("scratch.txt"), "tmp\n").unwrap();
        open_git(&trek, cx);
        assert!(trek.visible(cx, "gf-scratch.txt"));

        // Revert shows on hover, and asks first.
        trek.window(cx, |window, cx| window.hover("gf-scratch.txt", cx));
        trek.render(cx);
        trek.click(cx, "gf-revert-scratch.txt");
        dialog_open(&trek, cx);
        assert!(wt.path.join("scratch.txt").exists(), "nothing reverted before the answer");
        trek.click(cx, "ok");
        let scratch = wt.path.join("scratch.txt");
        trek.wait(cx, "the revert", |_| !scratch.exists()).await;
        assert!(wt.path.join("NOTES.md").exists(), "only that file");

        // Merging with the note uncommitted changes nothing.
        trek.render(cx);
        trek.click(cx, "git-merge");
        assert!(!trek.project.join("NOTES.md").exists());

        // Commit with a typed message.
        trek.render(cx);
        trek.click(cx, "git-message");
        trek.type_text(cx, "Add a note");
        trek.click(cx, "git-commit");
        let dir = wt.path.clone();
        trek.wait(cx, "the commit", |_| clean(&dir)).await;
        assert_eq!(last_subject(&wt.path), "Add a note");

        // Merge into the base: the project folder gets it, and the thread settles.
        trek.render(cx);
        trek.click(cx, "git-merge");
        let tid = id.clone();
        trek.wait(cx, "the merge to settle the thread", |ws| ws.thread(&tid).is_some_and(|t| t.settled_at.is_some())).await;
        assert!(trek.project.join("NOTES.md").exists());

        // Remove, through its dialog: merged, so the branch goes too.
        trek.render(cx);
        trek.click(cx, "git-remove");
        dialog_open(&trek, cx);
        assert!(wt.path.exists(), "nothing removed before the answer");
        trek.click(cx, "wt-remove");
        trek.wait(cx, "the worktree to go", |ws| ws.thread(&tid).is_some_and(|t| t.worktree.is_none())).await;
        assert!(!wt.path.exists());
        assert!(git(&trek.project, &["show-ref", "--verify", "--quiet", "refs/heads/trek/mock-write"]).is_err());
    });
}

#[test]
fn archiving_can_take_the_worktree_and_keeps_unmerged_commits() {
    run(async |cx| {
        let trek = open(cx);
        let (id, wt) = worktree_thread(&trek, cx).await;
        worktree::commit(&wt.path, "Add a note").unwrap();
        std::fs::write(wt.path.join("scratch.txt"), "wip\n").unwrap();
        let removal = trek.update(cx, |ws, cx| ws.worktree_removal(&id, cx)).await;
        assert_eq!(removal, Some(worktree::Removal { uncommitted: 1, unmerged: 1, missing: false }));
        // Archive and remove, through the dialog that names the uncommitted file.
        let (ws, tid) = (trek.ws.clone(), id.clone());
        trek.window(cx, |window, cx| confirm_leave(ws, tid, Leave::Archive, window, cx));
        dialog_open(&trek, cx);
        assert!(trek.visible(cx, "wt-keep"));
        trek.click(cx, "wt-remove");
        let tid = id.clone();
        trek.wait(cx, "the folder to go", move |ws| ws.store.thread(&tid).ok().flatten().is_some_and(|t| t.worktree.is_none())).await;
        assert!(!wt.path.exists());
        // The unmerged commit is still on its branch.
        assert!(git(&trek.project, &["show-ref", "--verify", "--quiet", "refs/heads/trek/mock-write"]).is_ok());
        let stored = trek.read(cx, |ws, _| ws.store.thread(&id).unwrap().unwrap());
        assert!(stored.archived_at.is_some());
        assert_eq!(stored.cwd, Some(trek.project.clone()));
    });
}

#[test]
fn removing_loses_no_more_than_the_dialog_said() {
    run(async |cx| {
        let trek = open(cx);
        let (id, wt) = worktree_thread(&trek, cx).await;
        // The dialog was asked with one uncommitted file (the note); the agent wrote another since.
        std::fs::write(wt.path.join("later.txt"), "later\n").unwrap();
        let result = trek.update(cx, |ws, cx| ws.remove_worktree(&id, 1, false, cx)).await;
        assert!(result.unwrap_err().to_string().contains("2 uncommitted changes now"));
        assert!(wt.path.join("later.txt").exists() && wt.path.join("NOTES.md").exists());
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.worktree.clone())), Some(wt.clone()));

        // Deleting the thread takes its worktree, once it may: the thread goes with it.
        let (ws, tid) = (trek.ws.clone(), id.clone());
        trek.window(cx, |window, cx| confirm_leave(ws, tid, Leave::Delete, window, cx));
        dialog_open(&trek, cx);
        assert!(!trek.visible(cx, "wt-keep"), "a deleted thread doesn't leave its worktree behind");
        trek.click(cx, "wt-leave");
        let tid = id.clone();
        trek.wait(cx, "the thread to go", move |ws| ws.thread(&tid).is_none() && ws.store.thread(&tid).ok().flatten().is_none()).await;
        assert!(!wt.path.exists());
        // Its commits-free branch went too.
        assert!(git(&trek.project, &["show-ref", "--verify", "--quiet", "refs/heads/trek/mock-write"]).is_err());
    });
}

#[test]
fn a_draft_set_to_a_worktree_says_so_even_where_one_cant_start() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| ws.update_project_prefs(&project, |p| p.run_in = trek_core::settings::RunIn::Worktree, cx));
        git(&trek.project, &["checkout", "-q", "--detach"]).unwrap();
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Draft { project: None }, cx);
            ws.navigate(Route::Draft { project: Some(project.clone()) }, cx);
            ws.refresh_git_at(project.clone(), cx);
        });
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.prefs_in(&Scope::Main).worktree));
        // The menu is there (not a plain "Local"), with the base it would start from.
        assert!(trek.visible(cx, "env-place"));
        assert!(trek.visible(cx, "env-base"));
        assert!(!trek.visible(cx, "branch-chip"), "no branch switcher for a worktree draft");
        // Sending can't make a worktree: nothing starts, and the text comes back.
        trek.type_text(cx, "fix the build");
        trek.press(cx, "enter");
        assert!(matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { .. }));
        assert!(trek.composer_text(cx).contains("fix the build"));
    });
}

#[test]
fn messages_wait_while_a_worktree_is_missing() {
    run(async |cx| {
        let trek = open(cx);
        let (id, wt) = worktree_thread(&trek, cx).await;
        worktree::commit(&wt.path, "Add a note").unwrap();
        std::fs::remove_dir_all(&wt.path).unwrap();
        trek.render(cx);
        assert!(trek.visible(cx, "worktree-missing"));
        // A message waits for a folder to run in.
        trek.send(cx, "explain the startup");
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1);
        assert_eq!(trek.items(cx, &id).iter().filter(|i| matches!(i, Item::User { .. })).count(), 1);

        // Recreated from its branch: the message goes out there.
        trek.click(cx, "wt-recreate");
        let tid = id.clone();
        trek.wait(cx, "the waiting message's answer", |ws| ws.live[&tid].items.iter().filter(|i| matches!(i, Item::TurnEnd { .. })).count() == 2).await;
        assert!(!wt.is_missing() && wt.path.join("NOTES.md").exists(), "back with the thread's commits");
        assert!(!trek.visible(cx, "worktree-missing"));

        // Gone again; this time the thread moves to the project folder.
        std::fs::remove_dir_all(&wt.path).unwrap();
        trek.render(cx);
        trek.send(cx, "explain the startup");
        trek.click(cx, "wt-local");
        trek.wait(cx, "the answer in the project folder", |ws| ws.live[&tid].items.iter().filter(|i| matches!(i, Item::TurnEnd { .. })).count() == 3).await;
        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned()).unwrap();
        assert_eq!((t.worktree, t.cwd), (None, Some(trek.project.clone())));
    });
}

#[test]
fn threads_editing_one_folder_are_warned_and_offered_a_worktree() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        let first = trek.send(cx, "mock:long 20s");
        trek.update(cx, |ws, cx| ws.new_thread(cx));
        let second = trek.send(cx, "mock:long 20s");
        assert_ne!(first, second);
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.sharing_folder(&first) && ws.sharing_folder(&second)));
        assert!(trek.visible(cx, "next-in-worktree"));
        // The link opens a new thread for the project, in a worktree.
        trek.click(cx, "next-in-worktree");
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: Some(trek.project.clone()) });
        assert!(trek.read(cx, |ws, _| ws.prefs_in(&Scope::Main).worktree));
        // One stops: the other is alone again.
        trek.update(cx, |ws, cx| ws.interrupt(&first, cx));
        trek.wait_done(cx, &first, RunState::Idle).await;
        assert!(!trek.read(cx, |ws, _| ws.sharing_folder(&second)));
        trek.update(cx, |ws, cx| ws.interrupt(&second, cx));
        trek.wait_done(cx, &second, RunState::Idle).await;
    });
}

#[test]
fn a_merge_from_the_git_tool_leaves_a_pinned_thread_where_it_is() {
    run(async |cx| {
        let trek = open(cx);
        let (id, wt) = worktree_thread(&trek, cx).await;
        worktree::commit(&wt.path, "Add a note").unwrap();
        // Pinned, and watched for a merge of its branch (as after a turn that committed).
        trek.update(cx, |ws, cx| {
            ws.store.update_thread(&id, |t| t.branch = Some(wt.branch.clone())).unwrap();
            ws.reload(cx);
            ws.toggle_pin(&id, cx);
        });
        let task = trek.update(cx, |ws, cx| ws.merge_worktree(&id, cx));
        let tid = id.clone();
        trek.wait(cx, "the merge to land", |ws| ws.thread(&tid).is_some_and(|t| t.branch.is_none())).await;
        drop(task);
        assert!(trek.project.join("NOTES.md").exists(), "merged into the project folder");
        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned()).unwrap();
        assert!(t.settled_at.is_none() && t.pinned_at.is_some(), "stays pinned, as a merge seen by the watch would leave it");
    });
}

#[test]
fn a_worktree_threads_project_actions_run_in_its_worktree() {
    run(async |cx| {
        let trek = open(cx);
        let (id, wt) = worktree_thread(&trek, cx).await;
        let ran = super::screens::runs(&trek, cx);
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| ws.update_project_prefs(&project, |p| p.actions = vec![ProjectAction { name: "Check".into(), command: "true".into() }], cx));
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        trek.click(cx, "run-actions");
        trek.press(cx, "down");
        trek.press(cx, "enter");
        assert_eq!(*ran.borrow(), [("true".to_string(), Some(wt.path.clone()))], "on the thread's copy of the code");

        // Once its folder is gone, they run in the project folder.
        std::fs::remove_dir_all(&wt.path).unwrap();
        trek.update(cx, |_, cx| cx.notify());
        trek.render(cx);
        trek.click(cx, "run-actions");
        trek.press(cx, "down");
        trek.press(cx, "enter");
        assert_eq!(ran.borrow().last(), Some(&("true".to_string(), Some(project))));
    });
}

fn checkpoint_refs(dir: &Path) -> usize {
    git(dir, &["for-each-ref", "--format=%(refname)", "refs/trek/checkpoints/"]).unwrap().lines().count()
}

#[test]
fn a_worktree_thread_checkpoints_rewinds_and_forks_in_its_worktree() {
    run(async |cx| {
        let trek = open(cx);
        let (id, wt) = worktree_thread(&trek, cx).await;
        trek.update(cx, |ws, cx| ws.send_to(&id, "mock:write".into(), vec![], cx));
        trek.wait(cx, "the second note", |ws| ws.live[&id].items.iter().filter(|i| matches!(i, Item::TurnEnd { .. })).count() == 2).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        let notes = || std::fs::read_to_string(wt.path.join("NOTES.md")).unwrap();
        assert_eq!(notes(), "# Notes\n\n- Note 1\n- Note 2\n");
        // Checkpoints of the worktree, their refs in the repository it shares with the project.
        assert!(trek.read(cx, |ws, _| ws.store.checkpoints(&id).unwrap()).len() == 2);
        assert_eq!(checkpoint_refs(&trek.project), 2);

        // Rewinding puts the worktree's files back; the project folder is untouched.
        let second = trek.read(cx, |ws, _| {
            let items = &ws.live[&id].items;
            let ix = items.iter().rposition(|i| matches!(i, Item::User { .. })).unwrap();
            items.ids()[ix].clone()
        });
        assert!(trek.update(cx, |ws, cx| ws.rewind(&id, &second, true, cx)).is_some());
        let path = wt.path.join("NOTES.md");
        trek.wait(cx, "the restore", move |_| std::fs::read_to_string(&path).is_ok_and(|n| n == "# Notes\n\n- Note 1\n")).await;
        assert!(clean(&trek.project) && !trek.project.join("NOTES.md").exists());

        // A fork stays in the worktree, with the checkpoints of the messages it copied.
        let fork = trek.update(cx, |ws, cx| ws.fork_thread(&id, crate::workspace::ForkAt::End, &Scope::Main, cx)).expect("a fork");
        trek.wait(cx, "the fork's checkpoints", |ws| ws.live[&fork].checkpointed.len() == 1).await;
        let f = trek.read(cx, |ws, _| ws.thread(&fork).cloned()).unwrap();
        assert_eq!((f.worktree.as_ref(), f.cwd.as_ref()), (Some(&wt), Some(&wt.path)));
        assert_eq!(trek.read(cx, |ws, _| ws.project_dir(&f)), Some(trek.project.clone()));
        assert_eq!(trek.read(cx, |ws, _| ws.worktree_sharers(&id)), [fork.clone()]);
        assert_eq!(checkpoint_refs(&trek.project), 2);

        // Removing the worktree moves both threads to the project folder. Their checkpoints were
        // of the worktree, so they go.
        let lose = trek.update(cx, |ws, cx| ws.worktree_removal(&id, cx)).await.unwrap().uncommitted;
        trek.update(cx, |ws, cx| ws.remove_worktree(&id, lose, false, cx)).await.unwrap();
        for t in [&id, &fork] {
            let t = trek.read(cx, |ws, _| ws.thread(t).cloned()).unwrap();
            assert_eq!((t.worktree, t.cwd, t.native_id), (None, Some(trek.project.clone()), None));
        }
        let (a, b) = (id.clone(), fork.clone());
        trek.wait(cx, "the checkpoints to go", move |ws| ws.store.checkpoints(&a).unwrap().is_empty() && ws.store.checkpoints(&b).unwrap().is_empty()).await;
        assert_eq!(checkpoint_refs(&trek.project), 0);
        // Earlier messages say why their files can't come back.
        let why = trek.read(cx, |ws, _| crate::workspace::NoCheckpoint::of(&ws.live[&id], 0, true));
        assert_eq!(why, Some(crate::workspace::NoCheckpoint::Missing));
    });
}

/// What the open rewind/undo confirmation says about the files, once it has checked them.
async fn confirm_files(trek: &Trek, cx: &mut TestAppContext) -> String {
    let view = trek.thread_view(cx);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        match view.read_with(cx, |v, _| v.confirm_files()) {
            Some(f) if f != "checking" => return f,
            None => panic!("no confirmation open"),
            _ => {}
        }
        assert!(std::time::Instant::now() < deadline, "timed out checking the files");
        cx.background_executor.timer(std::time::Duration::from_millis(5)).await;
    }
}

#[test]
fn a_thread_whose_worktree_is_missing_rewinds_only_the_conversation() {
    run(async |cx| {
        let trek = open(cx);
        let (id, wt) = worktree_thread(&trek, cx).await;
        trek.update(cx, |ws, cx| ws.send_to(&id, "mock:write".into(), vec![], cx));
        trek.wait(cx, "the second note", |ws| ws.live[&id].items.iter().filter(|i| matches!(i, Item::TurnEnd { .. })).count() == 2).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        worktree::commit(&wt.path, "Add notes").unwrap();
        assert_eq!(checkpoint_refs(&trek.project), 2);
        std::fs::remove_dir_all(&wt.path).unwrap();
        trek.render(cx);
        let toasts = |trek: &Trek, cx: &mut TestAppContext| trek.window(cx, |window, cx| gpui_kit::component::WindowExt::notifications(window, cx).len());
        let before = toasts(&trek, cx);

        // Undoing the last turn says the files can't come back, and doesn't try.
        let end = trek.items(cx, &id).iter().rposition(|i| matches!(i, Item::TurnEnd { .. })).unwrap();
        let why = trek.read(cx, |ws, _| {
            let start = ws.turn_start_item(&id, &ws.live[&id].items.ids()[end]).unwrap();
            ws.no_checkpoint(&id, &start)
        });
        assert_eq!(why, Some(crate::workspace::NoCheckpoint::WorktreeMissing));
        trek.click(cx, ("undo-turn", end));
        assert_eq!(confirm_files(&trek, cx).await, "unavailable");
        trek.click(cx, "confirm-go");
        assert_eq!(trek.items(cx, &id).iter().filter(|i| matches!(i, Item::User { .. })).count(), 1);
        assert_eq!(trek.composer_text(cx), "mock:write");
        // The undone message's checkpoint goes, from the repository the worktree shared; the
        // first one's stays for when the worktree is back.
        let tid = id.clone();
        trek.wait(cx, "the undone checkpoint to go", move |ws| ws.store.checkpoints(&tid).unwrap().len() == 1).await;
        assert_eq!(checkpoint_refs(&trek.project), 1);
        assert_eq!(toasts(&trek, cx), before, "no failed restore");

        // Recreated from its branch, the first message's files can be put back again.
        trek.click(cx, "wt-recreate");
        let tid = id.clone();
        trek.wait(cx, "the worktree back", move |ws| !ws.live[&tid].preparing && ws.thread(&tid).and_then(|t| t.worktree.clone()).is_some_and(|w| !w.is_missing())).await;
        trek.render(cx);
        let end = trek.items(cx, &id).iter().rposition(|i| matches!(i, Item::TurnEnd { .. })).unwrap();
        trek.click(cx, ("undo-turn", end));
        assert_eq!(confirm_files(&trek, cx).await, "changes: NOTES.md");
    });
}

#[test]
fn a_fork_sharing_a_worktree_leaves_without_it() {
    run(async |cx| {
        let trek = open(cx);
        let (id, wt) = worktree_thread(&trek, cx).await;
        let fork = trek.update(cx, |ws, cx| ws.fork_thread(&id, crate::workspace::ForkAt::End, &Scope::Main, cx)).expect("a fork");
        // Deleting the fork asks only about the thread: the worktree is the other's too.
        let (ws, tid) = (trek.ws.clone(), fork.clone());
        trek.window(cx, |window, cx| confirm_leave(ws, tid, Leave::Delete, window, cx));
        dialog_open(&trek, cx);
        assert!(!trek.visible(cx, "wt-keep"));
        trek.click(cx, "wt-leave");
        let tid = fork.clone();
        trek.wait(cx, "the fork to go", move |ws| ws.thread(&tid).is_none()).await;
        assert!(wt.path.join("NOTES.md").exists(), "the worktree stays");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.worktree.clone())), Some(wt.clone()));
        // Alone in it again, the thread is asked as before.
        let (ws, tid) = (trek.ws.clone(), id.clone());
        trek.window(cx, |window, cx| confirm_leave(ws, tid, Leave::Archive, window, cx));
        dialog_open(&trek, cx);
        assert!(trek.visible(cx, "wt-keep"));
    });
}
