//! The title bar's hub: the button between the title and the tools toggle, and the card it opens
//! with where the thread works, its git, its context and usage, what it used and where it came
//! from. Run and Open live in it now.

mod common;

use common::{open, run};
use std::path::Path;
use trek_app::workspace::{Route, Scope};
use trek_core::settings::ProjectAction;
use trek_core::store::{Item, ToolStatus};

fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .expect("git")
        .success();
    assert!(ok, "git {args:?}");
}

fn tool(id: &str, title: &str, detail: &str) -> Item {
    Item::Tool {
        id: id.into(),
        title: title.into(),
        detail: detail.into(),
        output: String::new(),
        status: ToolStatus::Done,
    }
}

#[test]
fn the_hub_holds_open_in_and_run_and_closes_when_one_is_picked() {
    run(async |cx| {
        let trek = open(cx);
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| {
            ws.update_project_prefs(
                &project,
                |p| {
                    p.actions = vec![ProjectAction {
                        name: "Check".into(),
                        command: "true".into(),
                    }]
                },
                cx,
            )
        });
        trek.quiet_thread(cx);
        trek.render(cx);
        // Run and Open are no longer buttons of their own.
        assert!(!trek.visible(cx, "run-actions"));
        assert!(!trek.visible(cx, "open-in"));
        assert!(trek.visible(cx, "hub"));
        assert!(!trek.visible(cx, "hub-card"));

        trek.click(cx, "hub");
        assert!(trek.visible(cx, "hub-card"));
        assert!(trek.visible(cx, "hub-where"));
        assert!(trek.visible(cx, "hub-open"));
        assert!(trek.visible(cx, "hub-run"));
        // The caret lists the rest of Open's apps (the terminal, at least) in the card itself.
        assert!(!trek.visible(cx, ("hub-open-in", 1usize)));
        trek.click(cx, "hub-open-more");
        assert!(trek.visible(cx, ("hub-open-in", 1usize)));
        assert!(
            trek.visible(cx, "hub-card"),
            "unfolding keeps the card open"
        );

        // Opened again, it starts folded.
        trek.press(cx, "escape");
        assert!(!trek.visible(cx, "hub-card"));
        trek.click(cx, "hub");
        assert!(!trek.visible(cx, ("hub-open-in", 1usize)));

        // The project's settings, behind the Where row's ⋯.
        trek.click(cx, "hub-project-settings");
        assert!(!trek.visible(cx, "hub-card"));
        assert!(matches!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Settings(_)
        ));
    });
}

#[test]
fn a_draft_has_where_and_no_thread_sections() {
    run(async |cx| {
        let trek = open(cx);
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project.clone()),
                },
                cx,
            )
        });
        trek.render(cx);
        trek.click(cx, "hub");
        assert!(trek.visible(cx, "hub-where"));
        assert!(trek.visible(cx, "hub-open"));
        assert!(trek.visible(cx, "hub-run"), "Add a project action…");
        for id in ["hub-context", "hub-usage", "hub-subagents", "hub-pr"] {
            assert!(!trek.visible(cx, id), "{id} isn't a draft's");
        }
    });
}

#[test]
fn the_git_section_names_the_branch_and_switches_it() {
    run(async |cx| {
        let trek = open(cx);
        let project = trek.project.clone();
        git(&project, &["init", "-q", "-b", "main"]);
        std::fs::write(project.join("a.txt"), "a").unwrap();
        git(&project, &["add", "."]);
        git(&project, &["commit", "-q", "-m", "first"]);
        git(&project, &["branch", "other"]);
        trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.refresh_git_at(project.clone(), cx));
        cx.run_until_parked();
        trek.render(cx);
        trek.click(cx, "hub");
        assert!(trek.visible(cx, "hub-branch"));
        assert!(trek.visible(cx, "hub-pull"));
        trek.click(cx, "hub-branch");
        assert!(trek.visible(cx, ("hub-branch-to", 0usize)));
        assert!(trek.visible(cx, ("hub-branch-to", 1usize)));
        // Branches come most recently committed first; both are at the same commit, so find
        // "other" by trying the one that isn't checked out.
        let other = trek
            .read(cx, |ws, _| {
                ws.git_info
                    .get(&project)
                    .map(|g| g.branches.iter().position(|b| b == "other"))
            })
            .flatten()
            .expect("other is listed");
        trek.click(cx, ("hub-branch-to", other));
        assert!(!trek.visible(cx, "hub-card"));
        cx.run_until_parked();
        let head = std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(&project)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "other");
    });
}

#[test]
fn the_thread_section_opens_the_context_card_and_shows_the_pull_request() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| {
            let live = ws.live.entry(id.clone()).or_default();
            // Little of the window used: the composer's ring stays hidden until asked for.
            live.context = Some((12_000, 200_000));
            live.items.push(Item::Tool {
                id: trek_app::pr_card::row_id("demo"),
                title: "Pull request".into(),
                detail: serde_json::to_string(&trek_app::pr_card::Card {
                    title: "Refresh sessions".into(),
                    number: Some(128),
                    url: Some("https://github.com/o/r/pull/128".into()),
                    phase: trek_app::pr_card::Phase::Ready,
                    ..Default::default()
                })
                .unwrap(),
                output: String::new(),
                status: ToolStatus::Done,
            });
            live.revision += 1;
            cx.notify();
        });
        trek.render(cx);
        assert!(!trek.visible(cx, "context-ring"));
        trek.click(cx, "hub");
        assert!(trek.visible(cx, "hub-pr"));
        assert!(trek.visible(cx, "hub-context"));
        trek.click(cx, "hub-context");
        trek.render(cx);
        assert!(!trek.visible(cx, "hub-card"));
        assert!(
            trek.visible(cx, "context-compact"),
            "the context card is open"
        );
        assert!(trek.visible(cx, "context-ring"));
    });
}

#[test]
fn sources_list_three_then_all_and_lineage_opens_the_thread_it_names() {
    run(async |cx| {
        let trek = open(cx);
        let starter = trek.quiet_thread(cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| {
            let mut t = ws.thread(&id).cloned().unwrap();
            t.started_by = Some(starter.clone());
            ws.store.save_thread(&t).unwrap();
            ws.reload(cx);
            let live = ws.live.entry(id.clone()).or_default();
            live.items.push(tool("r1", "Read", "src/a.rs"));
            live.items.push(tool("r2", "Read", "src/b.rs"));
            live.items.push(tool("e1", "Edit", "src/c.rs"));
            live.items
                .push(tool("f1", "Fetch", "https://example.com/page"));
            live.items.push(tool("r3", "Read", "src/a.rs"));
            live.revision += 1;
            cx.notify();
        });
        trek.render(cx);
        trek.click(cx, "hub");
        for i in 0..3usize {
            assert!(trek.visible(cx, ("hub-source", i)));
        }
        assert!(!trek.visible(cx, ("hub-source", 3usize)));
        trek.click(cx, "hub-sources-all");
        assert!(trek.visible(cx, ("hub-source", 3usize)), "four, a.rs once");
        assert!(!trek.visible(cx, ("hub-source", 4usize)));

        assert!(trek.visible(cx, ("hub-lineage", 0usize)));
        trek.click(cx, ("hub-lineage", 0usize));
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(starter)
        );
    });
}

#[test]
fn a_thread_window_has_the_hub_too() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let window = trek.open_thread_window(cx, &id);
        assert!(trek.visible_in(cx, window, "hub"));
        assert!(!trek.visible_in(cx, window, "open-in"));
        assert!(trek.read(cx, |ws, _| trek_app::hub::shown(
            ws,
            &Scope::Thread(id.clone())
        )));
    });
}
