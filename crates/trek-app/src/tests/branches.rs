//! Checking out another branch from the composer's branch chip: never under a working agent,
//! never a branch another worktree has, and what's on screen follows (open files, the chip).

use super::harness::{Trek, open, run};
use gpui_kit::TestAppContext;
use std::path::Path;
use trek_core::RunState;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo on `main` with a `feature` branch: `a.rs` says "main" on one and "feature" on the other.
fn two_branches(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.name", "t"), ("user.email", "t@t"), ("commit.gpgSign", "false"), ("core.autocrlf", "false")] {
        git(dir, &["config", k, v]);
    }
    std::fs::write(dir.join("a.rs"), "main\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "init"]);
    git(dir, &["switch", "-qc", "feature"]);
    std::fs::write(dir.join("a.rs"), "feature\n").unwrap();
    git(dir, &["commit", "-qam", "feature"]);
    git(dir, &["switch", "-q", "main"]);
}

async fn on_branch(trek: &Trek, cx: &mut TestAppContext, branch: &str) {
    let (dir, want) = (trek.project.clone(), Some(branch.to_string()));
    trek.wait(cx, &format!("the chip to say {branch}"), |ws| ws.git_info.get(&dir).is_some_and(|g| g.branch == want)).await;
    trek.render(cx);
}

#[test]
fn open_files_follow_a_branch_switch_and_unsaved_edits_are_not_saved_over_it_unasked() {
    run(async |cx| {
        let trek = open(cx);
        two_branches(&trek.project);
        let file = trek.project.join("a.rs");
        trek.update(cx, |ws, cx| ws.refresh_git_at(trek.project.clone(), cx));
        on_branch(&trek, cx, "main").await;
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
        trek.render(cx);
        let editor = trek.root.read_with(cx, |r, cx| r.editor(cx)).expect("editor view");
        let text = |cx: &mut TestAppContext| editor.read_with(cx, |e, cx| e.text_state().read(cx).value().to_string());
        assert_eq!(text(cx), "main\n");

        trek.update(cx, |ws, cx| ws.switch_branch(trek.project.clone(), "feature".into(), cx));
        on_branch(&trek, cx, "feature").await;
        assert_eq!(text(cx), "feature\n", "a clean tab shows the branch checked out");

        // Edited, not saved, and the branch changes under it: the edit stays, and the first save
        // says the file moved on instead of writing the other branch's version back.
        trek.window(cx, |window, cx| editor.update(cx, |e, cx| e.text_state().update(cx, |s, cx| s.insert("// mine\n", window, cx))));
        cx.run_until_parked();
        trek.update(cx, |ws, cx| ws.switch_branch(trek.project.clone(), "main".into(), cx));
        on_branch(&trek, cx, "main").await;
        assert!(editor.read_with(cx, |e, _| e.dirty()));
        assert_eq!(text(cx), "// mine\nfeature\n");
        editor.update(cx, |e, cx| e.save_now(cx));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "main\n", "not written over unasked");
        editor.update(cx, |e, cx| e.save_now(cx));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "// mine\nfeature\n", "saving again writes it");
    });
}

#[test]
fn no_branch_is_switched_under_a_working_agent() {
    run(async |cx| {
        let trek = open(cx);
        two_branches(&trek.project);
        trek.update(cx, |ws, cx| ws.send("mock:long 600s".into(), vec![], cx));
        let id = trek.thread_id(cx);
        trek.wait(cx, "the long turn", |ws| ws.turn_running(&id)).await;
        assert!(trek.read(cx, |ws, _| ws.switch_blocked(&trek.project)).is_some());
        // A subfolder of the checkout counts too.
        std::fs::create_dir_all(trek.project.join("sub")).unwrap();
        assert!(trek.read(cx, |ws, _| ws.switch_blocked(&trek.project.join("sub"))).is_some());
        trek.update(cx, |ws, cx| ws.switch_branch(trek.project.clone(), "feature".into(), cx));
        cx.run_until_parked();
        assert_eq!(git(&trek.project, &["branch", "--show-current"]), "main");

        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| ws.switch_blocked(&trek.project)).is_none());
        trek.update(cx, |ws, cx| ws.switch_branch(trek.project.clone(), "feature".into(), cx));
        on_branch(&trek, cx, "feature").await;
    });
}

#[test]
fn branches_other_worktrees_have_are_named_and_every_branch_is_listed() {
    let dir = super::harness::new_project("branches");
    two_branches(&dir);
    for n in 0..25 {
        git(&dir, &["branch", &format!("b{n:02}")]);
    }
    let wt = dir.with_extension("wt");
    git(&dir, &["worktree", "add", "-q", "-b", "elsewhere", wt.to_str().unwrap()]);
    let info = crate::workspace::read_git_info(&dir);
    assert_eq!(info.elsewhere, ["elsewhere"]);
    assert_eq!(info.branches.len(), 28, "{:?}", info.branches);
    assert!(info.head.is_some());
    let _ = std::fs::remove_dir_all(&wt);
}

#[test]
fn git_errors_read_as_sentences_with_the_files_they_name() {
    use crate::workspace::git_error;
    let dirty = "error: Your local changes to the following files would be overwritten by checkout:\n\ta.rs\n\tb.rs\nPlease commit your changes or stash them before you switch branches.\nAborting\n";
    assert_eq!(git_error(dirty, "x"), "Your local changes to the following files would be overwritten by checkout a.rs, b.rs. Commit or stash them first.");
    assert_eq!(git_error("fatal: 'wtb' is already used by worktree at '/tmp/w'\n", "x"), "'wtb' is already used by worktree at '/tmp/w'");
    assert_eq!(git_error("", "git switch failed"), "git switch failed");
}
