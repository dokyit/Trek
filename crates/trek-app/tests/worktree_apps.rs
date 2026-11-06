//! Coming from another app, through the real window: what's offered and only when, bringing it
//! over, and that a worktree brought over stays that app's whatever is done here. Git runs for
//! real, in a made-up home folder under the test's temp folder.

mod common;

use common::{Trek, canonical, new_project, open, run};
use gpui_kit::TestAppContext;
use std::path::{Path, PathBuf};
use trek_app::thread_menu::Facts;
use trek_app::workspace::{Route, SettingsPage};
use trek_core::import::worktree_apps::WorktreeApp;
use trek_core::worktree::git;

/// A home folder with a repository and a Conductor worktree of it on `feat/login`.
fn conductor_home() -> (PathBuf, PathBuf, PathBuf) {
    let home = canonical(&new_project("home"));
    let repo = home.join("code/acme");
    std::fs::create_dir_all(&repo).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@example.com"],
        &["config", "user.name", "T"],
        &["commit", "-q", "--allow-empty", "-m", "first"],
    ] {
        git(&repo, args).unwrap();
    }
    let wt = home.join("conductor/workspaces/acme/kyoto");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/login",
            wt.to_str().unwrap(),
        ],
    )
    .unwrap();
    (home, repo, wt)
}

async fn scan(trek: &Trek, home: &Path, cx: &mut TestAppContext) {
    let home = home.to_path_buf();
    trek.update(cx, |ws, cx| {
        ws.worktree_apps.fixture_home = Some(home);
        ws.scan_worktree_apps(cx);
    });
    trek.wait(cx, "the look to finish", |ws| !ws.worktree_apps.scanning)
        .await;
}

async fn adopt(trek: &Trek, cx: &mut TestAppContext) {
    trek.update(cx, |ws, cx| ws.adopt_from(WorktreeApp::Conductor, cx));
    trek.wait(cx, "the bring-over to finish", |ws| {
        ws.worktree_apps.adopting.is_none() && !ws.worktree_apps.scanning
    })
    .await;
}

fn thread_in(trek: &Trek, wt: &Path, cx: &TestAppContext) -> String {
    trek.read(cx, |ws, _| {
        ws.threads
            .iter()
            .find(|t| t.worktree.as_ref().is_some_and(|w| w.path == wt))
            .map(|t| t.id.clone())
            .expect("a thread in the worktree")
    })
}

#[test]
fn a_test_or_the_isolated_app_never_looks_in_the_real_home() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.scan_worktree_apps(cx));
        assert!(trek.read(cx, |ws, _| {
            !ws.worktree_apps.scanning && ws.worktree_apps.plans.is_empty()
        }));
    });
}

#[test]
fn nothing_shows_on_the_page_when_no_app_left_anything() {
    run(async |cx| {
        let trek = open(cx);
        let empty = canonical(&new_project("empty-home"));
        scan(&trek, &empty, cx).await;
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Import), cx)
        });
        trek.render(cx);
        assert!(!trek.visible(cx, "worktree-app-conductor"));
    });
}

#[test]
fn the_page_offers_what_conductor_left_and_shows_it_before_anything_happens() {
    run(async |cx| {
        let trek = open(cx);
        let (home, _repo, _wt) = conductor_home();
        let projects_before = trek.read(cx, |ws, _| ws.projects.len());
        trek.update(cx, |ws, cx| {
            ws.worktree_apps.fixture_home = Some(home);
            ws.navigate(Route::Settings(SettingsPage::Import), cx)
        });
        trek.wait(cx, "the look to finish", |ws| {
            !ws.worktree_apps.scanning && !ws.worktree_apps.plans.is_empty()
        })
        .await;
        trek.render(cx);
        assert!(trek.visible(cx, "worktree-app-conductor"));

        trek.click(cx, "worktree-app-conductor");
        trek.render(cx);
        assert!(trek.visible(cx, "worktree-app-review"), "the dialog shows");
        assert!(trek.visible(cx, "worktree-app-counts"));
        // It's only been shown: nothing came across yet.
        assert_eq!(trek.read(cx, |ws, _| ws.projects.len()), projects_before);

        trek.click(cx, "worktree-app-adopt");
        trek.wait(cx, "the bring-over to finish", |ws| {
            ws.worktree_apps.adopting.is_none() && ws.worktree_apps.plans.is_empty()
        })
        .await;
        assert_eq!(
            trek.read(cx, |ws, _| ws.projects.len()),
            projects_before + 1
        );
    });
}

#[test]
fn a_worktree_brought_over_is_on_its_branch_where_it_was_under_its_repository() {
    run(async |cx| {
        let trek = open(cx);
        let (home, repo, wt) = conductor_home();
        scan(&trek, &home, cx).await;
        adopt(&trek, cx).await;

        let id = thread_in(&trek, &wt, cx);
        trek.read(cx, |ws, _| {
            let t = ws.thread(&id).unwrap();
            let w = t.worktree.as_ref().unwrap();
            assert_eq!(t.cwd.as_deref(), Some(wt.as_path()));
            assert_eq!(w.branch, "feat/login");
            assert_eq!(w.base, "main");
            assert_eq!(t.title, "kyoto");
            let project = t.project_id.as_deref().and_then(|p| ws.project(p)).unwrap();
            assert_eq!(project.path, repo);
            assert_eq!(ws.worktree_owner(&id), Some(WorktreeApp::Conductor));
        });
        assert_eq!(
            git(&wt, &["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap()
                .trim(),
            "feat/login"
        );
        // It shows in the sidebar, as the work in progress it is, and not as unread.
        trek.read(cx, |ws, _| {
            let t = ws.thread(&id).unwrap();
            assert_eq!(t.settled_at, None);
            assert!(!t.is_unseen());
        });
        // Offered once.
        assert!(trek.read(cx, |ws, _| ws.worktree_apps.plans.is_empty()));
    });
}

#[test]
fn nothing_in_trek_removes_a_worktree_another_app_keeps() {
    run(async |cx| {
        let trek = open(cx);
        let (home, _repo, wt) = conductor_home();
        scan(&trek, &home, cx).await;
        adopt(&trek, cx).await;
        let id = thread_in(&trek, &wt, cx);

        // The menus don't ask about it: the thread just goes.
        let facts = trek.read(cx, |ws, _| Facts::read(ws, &id)).unwrap();
        assert!(!facts.in_worktree);

        // Reached any way at all, removing is refused.
        let refused = trek
            .update(cx, |ws, cx| ws.remove_worktree(&id, 99, true, cx))
            .await;
        let why = refused.unwrap_err().to_string();
        assert!(why.contains("Conductor"), "{why}");
        assert!(wt.join(".git").exists());
        assert!(
            git(&wt, &["rev-parse", "--verify", "feat/login"]).is_ok(),
            "its branch stays"
        );

        // Nor does it make the folder again if it goes: that's Conductor's to do.
        std::fs::rename(&wt, wt.with_extension("moved")).unwrap();
        trek.update(cx, |ws, cx| ws.recreate_worktree(&id, cx));
        cx.run_until_parked();
        assert!(!wt.exists());
        std::fs::rename(wt.with_extension("moved"), &wt).unwrap();

        // Archiving, then deleting, take the thread and leave the folder.
        trek.update(cx, |ws, cx| ws.archive(&id, cx));
        cx.run_until_parked();
        assert!(wt.join(".git").exists());
        trek.update(cx, |ws, cx| ws.delete_thread(&id, cx));
        cx.run_until_parked();
        assert!(wt.join(".git").exists());
        assert!(
            trek.read(cx, |ws, _| ws.worktree_apps.owner(&wt)).is_some(),
            "still known to be Conductor's"
        );
    });
}

#[test]
fn a_project_removed_earlier_comes_back_when_it_is_asked_for() {
    run(async |cx| {
        let trek = open(cx);
        let (home, repo, wt) = conductor_home();
        let key = repo.display().to_string();
        trek.update(cx, |ws, _| ws.settings.hidden_projects.push(key.clone()));
        scan(&trek, &home, cx).await;
        adopt(&trek, cx).await;
        assert!(trek.read(cx, |ws, _| !ws.settings.hidden_projects.contains(&key)));
        let id = thread_in(&trek, &wt, cx);
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_some()));
    });
}
