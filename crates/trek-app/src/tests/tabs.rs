//! Tabs along the top of the chat, threads in no project, and the Notes screen.

use super::harness::{new_project, open, run};
use crate::workspace::{NO_PROJECT, Route};
use trek_core::RunState;

fn tab_ids(trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext) -> Vec<String> {
    trek.read(cx, |ws, _| ws.tabs_here().into_iter().map(|t| t.id.clone()).collect())
}

#[test]
fn threads_open_as_tabs_in_their_projects_strip() {
    run(async |cx| {
        let trek = open(cx);
        let a = trek.send(cx, "first");
        trek.wait_done(cx, &a, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.new_thread(cx));
        trek.render(cx);
        assert!(trek.visible(cx, "tab-strip"), "a draft beside an open tab shows the strip");
        assert!(trek.visible(cx, "tab-draft"), "with the draft as a tab of its own");
        let b = trek.send(cx, "second");
        trek.wait_done(cx, &b, RunState::Idle).await;
        assert_eq!(tab_ids(&trek, cx), [a.clone(), b.clone()], "the draft's tab became the thread's");
        trek.render(cx);
        assert!(trek.visible(cx, format!("tab-{a}")) && trek.visible(cx, format!("tab-{b}")));

        // Another project's thread has a strip of its own.
        let other = new_project("other");
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(other) }, cx));
        let c = trek.send(cx, "elsewhere");
        trek.wait_done(cx, &c, RunState::Idle).await;
        assert_eq!(tab_ids(&trek, cx), [c.clone()]);

        // Back in the first project: its tabs, and ⌃Tab goes round them.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        assert_eq!(tab_ids(&trek, cx), [a.clone(), b.clone()]);
        trek.update(cx, |ws, cx| ws.cycle_tab(1, cx));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(b.clone()));
        trek.update(cx, |ws, cx| ws.cycle_tab(1, cx));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(a.clone()), "round the strip");

        // Moving a tab.
        trek.update(cx, |ws, cx| ws.move_tab(&a, 0, cx));
        assert_eq!(tab_ids(&trek, cx), [a.clone(), b.clone()]);
        trek.update(cx, |ws, cx| ws.move_tab(&a, 1, cx));
        assert_eq!(tab_ids(&trek, cx), [b.clone(), a.clone()]);

        // A full strip lets go of the tab looked at longest ago.
        let mut extra = vec![];
        for i in 0..crate::workspace::MAX_TABS {
            trek.update(cx, |ws, cx| ws.new_thread(cx));
            let id = trek.send(cx, &format!("more {i}"));
            trek.wait_done(cx, &id, RunState::Idle).await;
            extra.push(id);
        }
        let strip = tab_ids(&trek, cx);
        assert_eq!(strip.len(), crate::workspace::MAX_TABS, "{strip:?}");
        assert!(!strip.contains(&b), "b was looked at longest ago");
        assert!(trek.read(cx, |ws, _| ws.thread(&b).is_some()), "its thread stays");
        for id in &extra {
            trek.update(cx, |ws, cx| ws.close_tab(id, cx));
        }
        // Both first tabs went as the strip filled; they come back as they're opened again.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b.clone()), cx));
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        assert_eq!(tab_ids(&trek, cx), [b.clone(), a.clone()]);

        // Closing the tab in front brings its neighbour forward; the thread stays.
        trek.update(cx, |ws, cx| ws.close_tab(&a, cx));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(b.clone()));
        assert!(trek.read(cx, |ws, _| ws.thread(&a).is_some()), "closing a tab doesn't touch the thread");
        // The last one: a new thread in the project.
        trek.update(cx, |ws, cx| ws.close_tab(&b, cx));
        let project = trek.project.clone();
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: Some(project) });
        trek.render(cx);
        assert!(!trek.visible(cx, "tab-strip"), "no tabs left: no strip");
    });
}

#[test]
fn a_thread_in_no_project_runs_in_a_folder_of_its_own() {
    run(async |cx| {
        let trek = open(cx);
        let in_project = trek.send(cx, "in the project");
        trek.wait_done(cx, &in_project, RunState::Idle).await;

        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx));
        let id = trek.send(cx, "a quick thought");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned()).expect("thread");
        assert_eq!(t.project_id, None, "it belongs to no project");
        let cwd = t.cwd.expect("a folder to work in");
        assert!(trek_core::paths::is_chat_dir(&cwd) && cwd.is_dir(), "{cwd:?}");
        assert!(cwd.join(".git").exists(), "a repository, so its turns can be rewound");
        assert!(!cwd.to_string_lossy().contains(' '), "no space in the path for shells to trip on: {cwd:?}");
        assert!(trek.read(cx, |ws, _| ws.project_dir(&t_of(ws, &id)).is_none()), "and has no project folder");

        // ⌘N from it: another thread in no project, not the first project.
        trek.update(cx, |ws, cx| ws.new_thread(cx));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: None });

        // The project filter's "No project" keeps just it.
        trek.update(cx, |ws, _| ws.project_filter = Some(NO_PROJECT.to_string()));
        let listed: Vec<String> = trek.read(cx, |ws, _| ws.sections().into_iter().flat_map(|(_, v)| v.into_iter().map(|t| t.id.clone())).collect());
        assert_eq!(listed, [id.clone()]);

        // Its tab is in the no-project strip, not the project's.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        assert_eq!(tab_ids(&trek, cx), [id.clone()]);

        // Deleting the thread takes its folder with it.
        trek.update(cx, |ws, cx| ws.delete_thread(&id, cx));
        assert!(!cwd.exists(), "{cwd:?}");
    });
}

fn t_of(ws: &crate::workspace::Workspace, id: &str) -> trek_core::store::Thread {
    ws.thread(id).cloned().expect("thread")
}

#[test]
fn notes_open_from_the_sidebar() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-notes");
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Notes);
        assert!(trek.visible(cx, "notes"));
        assert!(trek.visible(cx, "note-toolbar") && trek.visible(cx, "note-list"));
        // Typing starts a note; Return carries a list on.
        trek.type_text(cx, "- milk");
        trek.press(cx, "enter");
        trek.type_text(cx, "eggs");
        trek.render(cx);
        assert!(trek.visible(cx, "note-delete-zone"), "a note now");
    });
}

#[test]
fn notes_undo_and_redo_typing_and_formatting() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-notes");
        trek.render(cx);
        // What the note holds on disk, once what's typed has been saved.
        let saved = |cx: &mut gpui_kit::TestAppContext| {
            cx.executor().advance_clock(std::time::Duration::from_millis(600));
            cx.run_until_parked();
            trek_core::notes::list_in(&trek_core::notes::notes_dir()).first().map(|n| n.body.clone()).unwrap_or_default()
        };
        trek.type_text(cx, "milk");
        assert_eq!(saved(cx), "milk");
        assert!(trek.visible(cx, "note-undo") && trek.visible(cx, "note-redo"));
        // A formatting command, from the toolbar: one step.
        trek.render(cx);
        trek.press(cx, "cmd-a");
        trek.click(cx, "note-bold");
        assert_eq!(saved(cx), "**milk**");
        // ⌘Z takes it back, then the typing; ⇧⌘Z puts them back.
        trek.press(cx, "cmd-z");
        assert_eq!(saved(cx), "milk");
        trek.press(cx, "cmd-z");
        assert_eq!(saved(cx), "");
        trek.press(cx, "cmd-shift-z");
        assert_eq!(saved(cx), "milk");
        trek.press(cx, "cmd-shift-z");
        assert_eq!(saved(cx), "**milk**");
        // The toolbar's buttons do the same.
        trek.click(cx, "note-undo");
        assert_eq!(saved(cx), "milk");
        trek.click(cx, "note-redo");
        assert_eq!(saved(cx), "**milk**");
    });
}
