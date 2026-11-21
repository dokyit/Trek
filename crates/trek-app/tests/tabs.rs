//! Tabs along the top of the chat, threads in no project, and the Notes screen. The tools panel's
//! own tabs, and its clicks kept off the strip and the sidebar it can lie over.

mod common;

use common::{launch, new_project, open, run, settings};
use gpui_kit::{px, size};
use trek_app::workspace::{NO_PROJECT, PanelTool, Route};
use trek_core::RunState;

fn tab_ids(trek: &common::Trek, cx: &gpui_kit::TestAppContext) -> Vec<String> {
    trek.read(cx, |ws, _| {
        ws.tabs_here().into_iter().map(|t| t.id.clone()).collect()
    })
}

#[test]
fn threads_open_as_tabs_in_their_projects_strip() {
    run(async |cx| {
        let trek = open(cx);
        let a = trek.send(cx, "first");
        trek.wait_done(cx, &a, RunState::Idle).await;
        // ⌘N starts in no project; the strip's + starts another thread in this one.
        trek.update(cx, |ws, cx| ws.new_thread(cx));
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Draft { project: None }
        );
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        trek.render(cx);
        trek.click(cx, "tab-new");
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Draft {
                project: Some(trek.project.clone())
            }
        );
        trek.render(cx);
        assert!(
            trek.visible(cx, "tab-strip"),
            "a draft beside an open tab shows the strip"
        );
        assert!(
            trek.visible(cx, "tab-draft"),
            "with the draft as a tab of its own"
        );
        let b = trek.send(cx, "second");
        trek.wait_done(cx, &b, RunState::Idle).await;
        assert_eq!(
            tab_ids(&trek, cx),
            [a.clone(), b.clone()],
            "the draft's tab became the thread's"
        );
        trek.render(cx);
        assert!(trek.visible(cx, format!("tab-{a}")) && trek.visible(cx, format!("tab-{b}")));

        // Another project's thread has a strip of its own.
        let other = new_project("other");
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(other),
                },
                cx,
            )
        });
        let c = trek.send(cx, "elsewhere");
        trek.wait_done(cx, &c, RunState::Idle).await;
        assert_eq!(tab_ids(&trek, cx), [c.clone()]);

        // Back in the first project: its tabs, and ⌃Tab goes round them.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        assert_eq!(tab_ids(&trek, cx), [a.clone(), b.clone()]);
        trek.update(cx, |ws, cx| ws.cycle_tab(1, cx));
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(b.clone())
        );
        trek.update(cx, |ws, cx| ws.cycle_tab(1, cx));
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(a.clone()),
            "round the strip"
        );

        // Moving a tab.
        trek.update(cx, |ws, cx| ws.move_tab(&a, 0, cx));
        assert_eq!(tab_ids(&trek, cx), [a.clone(), b.clone()]);
        trek.update(cx, |ws, cx| ws.move_tab(&a, 1, cx));
        assert_eq!(tab_ids(&trek, cx), [b.clone(), a.clone()]);

        // A full strip lets go of the tab looked at longest ago.
        let mut extra = vec![];
        for i in 0..trek_app::workspace::MAX_TABS {
            let project = trek.project.clone();
            trek.update(cx, |ws, cx| {
                ws.navigate(
                    Route::Draft {
                        project: Some(project),
                    },
                    cx,
                )
            });
            let id = trek.send(cx, &format!("more {i}"));
            trek.wait_done(cx, &id, RunState::Idle).await;
            extra.push(id);
        }
        let strip = tab_ids(&trek, cx);
        assert_eq!(strip.len(), trek_app::workspace::MAX_TABS, "{strip:?}");
        assert!(!strip.contains(&b), "b was looked at longest ago");
        assert!(
            trek.read(cx, |ws, _| ws.thread(&b).is_some()),
            "its thread stays"
        );
        for id in &extra {
            trek.update(cx, |ws, cx| ws.close_tab(id, cx));
        }
        // Both first tabs went as the strip filled; they come back as they're opened again.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b.clone()), cx));
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        assert_eq!(tab_ids(&trek, cx), [b.clone(), a.clone()]);

        // Closing the tab in front brings its neighbour forward; the thread stays.
        trek.update(cx, |ws, cx| ws.close_tab(&a, cx));
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(b.clone())
        );
        assert!(
            trek.read(cx, |ws, _| ws.thread(&a).is_some()),
            "closing a tab doesn't touch the thread"
        );
        // The last one: a new thread in the project.
        trek.update(cx, |ws, cx| ws.close_tab(&b, cx));
        let project = trek.project.clone();
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Draft {
                project: Some(project)
            }
        );
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
        let t = trek
            .read(cx, |ws, _| ws.thread(&id).cloned())
            .expect("thread");
        assert_eq!(t.project_id, None, "it belongs to no project");
        let cwd = t.cwd.expect("a folder to work in");
        assert!(
            trek_core::paths::is_chat_dir(&cwd) && cwd.is_dir(),
            "{cwd:?}"
        );
        assert!(
            cwd.join(".git").exists(),
            "a repository, so its turns can be rewound"
        );
        assert!(
            !cwd.to_string_lossy().contains(' '),
            "no space in the path for shells to trip on: {cwd:?}"
        );
        assert!(
            trek.read(cx, |ws, _| ws.project_dir(&t_of(ws, &id)).is_none()),
            "and has no project folder"
        );

        // ⌘N from it: another thread in no project, not the first project.
        trek.update(cx, |ws, cx| ws.new_thread(cx));
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Draft { project: None }
        );

        // The project filter's "No project" keeps just it.
        trek.update(cx, |ws, _| ws.project_filter = Some(NO_PROJECT.to_string()));
        let listed: Vec<String> = trek.read(cx, |ws, _| {
            ws.sections()
                .into_iter()
                .flat_map(|(_, v)| v.into_iter().map(|t| t.id.clone()))
                .collect()
        });
        assert_eq!(listed, [id.clone()]);

        // Its tab is in the no-project strip, not the project's.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        assert_eq!(tab_ids(&trek, cx), [id.clone()]);

        // Deleting the thread takes its folder with it.
        trek.update(cx, |ws, cx| ws.delete_thread(&id, cx));
        assert!(!cwd.exists(), "{cwd:?}");
    });
}

fn t_of(ws: &trek_app::workspace::Workspace, id: &str) -> trek_core::store::Thread {
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
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(600));
            cx.run_until_parked();
            trek_core::notes::list_in(&trek_core::notes::notes_dir())
                .first()
                .map(|n| n.body.clone())
                .unwrap_or_default()
        };
        trek.type_text(cx, "milk");
        assert_eq!(saved(cx), "milk");
        assert!(trek.visible(cx, "note-undo") && trek.visible(cx, "note-redo"));
        // A formatting command, from the toolbar: one step.
        trek.render(cx);
        trek.press(cx, "secondary-a");
        trek.click(cx, "note-bold");
        assert_eq!(saved(cx), "**milk**");
        // ⌘Z takes it back, then the typing; ⇧⌘Z puts them back (Ctrl+Y outside macOS).
        let redo = if cfg!(target_os = "macos") {
            "cmd-shift-z"
        } else {
            "ctrl-y"
        };
        trek.press(cx, "secondary-z");
        assert_eq!(saved(cx), "milk");
        trek.press(cx, "secondary-z");
        assert_eq!(saved(cx), "");
        trek.press(cx, redo);
        assert_eq!(saved(cx), "milk");
        trek.press(cx, redo);
        assert_eq!(saved(cx), "**milk**");
        // The toolbar's buttons do the same.
        trek.click(cx, "note-undo");
        assert_eq!(saved(cx), "milk");
        trek.click(cx, "note-redo");
        assert_eq!(saved(cx), "**milk**");
    });
}

#[test]
fn a_full_tab_strip_scrolls_fades_and_lists_every_tab() {
    run(async |cx| {
        let trek = open(cx);
        let ids: Vec<String> = (0..trek_app::workspace::MAX_TABS)
            .map(|i| {
                trek.update(cx, |ws, cx| {
                    let mut t = ws
                        .store
                        .create_thread(
                            Some(&trek.project),
                            common::mock(),
                            None,
                            trek_core::Effort::Medium,
                            trek_core::HandHolding::Auto,
                        )
                        .expect("thread");
                    t.title = format!(
                        "Thread {i}: fix the flaky integration test in the payments service"
                    );
                    ws.store.save_thread(&t).expect("save");
                    ws.reload(cx);
                    ws.navigate(Route::Thread(t.id.clone()), cx);
                    t.id
                })
            })
            .collect();
        // Laid out once, the strip knows it overflows.
        trek.render(cx);
        trek.render(cx);
        assert_eq!(tab_ids(&trek, cx).len(), ids.len());
        assert!(
            trek.visible(cx, "tab-overflow"),
            "a menu lists the tabs that don't fit"
        );
        let strip = trek.bounds(cx, "tab-strip-tabs").expect("the strip");
        let inside = |trek: &common::Trek, cx: &mut gpui_kit::TestAppContext, id: &str| {
            let tab = trek.bounds(cx, format!("tab-{id}")).expect("the tab");
            tab.left() >= strip.left() - gpui_kit::px(0.5)
                && tab.right() <= strip.right() + gpui_kit::px(0.5)
        };
        let last = ids.last().unwrap();
        assert!(
            inside(&trek, cx, last),
            "the tab in front is scrolled into view"
        );
        assert!(
            trek.visible(cx, "tab-fade-start"),
            "tabs cut off at the start fade out there"
        );
        // No tab is squeezed below its minimum.
        for id in &ids {
            let w = trek.bounds(cx, format!("tab-{id}")).map(|b| b.size.width);
            assert!(
                w.is_none_or(|w| w >= gpui_kit::px(trek_app::tabs::TAB_MIN_WIDTH - 0.5)),
                "{w:?}"
            );
        }
        // Bringing the first tab forward scrolls back to it.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(ids[0].clone()), cx));
        trek.render(cx);
        trek.render(cx);
        assert!(inside(&trek, cx, &ids[0]));
        assert!(trek.visible(cx, "tab-fade-end") && !trek.visible(cx, "tab-fade-start"));
    });
}

#[test]
fn the_tools_panel_opens_on_a_grid_of_tools_and_never_clips_a_tab() {
    run(async |cx| {
        let trek = open(cx);
        trek.press(cx, "secondary-j");
        trek.render(cx);
        for tool in trek_app::workspace::PanelTool::offered() {
            assert!(
                trek.visible(cx, format!("tool-tile-{}", tool.label())),
                "{}",
                tool.label()
            );
        }
        trek.click(cx, "tool-tile-Terminal");
        trek.render(cx);
        assert!(
            !trek.visible(cx, "tool-tile-Terminal"),
            "the tool opened in a tab"
        );
        // Many tools at the panel's default width: the tabs not in front drop to their icons,
        // then the strip scrolls with a menu of every tab; the one in front stays whole. (The
        // panel's own terminals stack from its launcher; the event's terminal is the dock under
        // the chat.)
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        for tool in [
            trek_app::workspace::PanelTool::Git,
            trek_app::workspace::PanelTool::Explorer,
            trek_app::workspace::PanelTool::SideChat,
            trek_app::workspace::PanelTool::Terminal,
            trek_app::workspace::PanelTool::Terminal,
            trek_app::workspace::PanelTool::Terminal,
            trek_app::workspace::PanelTool::Terminal,
            trek_app::workspace::PanelTool::Terminal,
            trek_app::workspace::PanelTool::Terminal,
        ] {
            trek.window(cx, |window, cx| {
                panel.update(cx, |p, cx| p.open_tool(tool, window, cx))
            });
        }
        // The frames the panel asks for once it has measured its strip.
        for _ in 0..3 {
            trek.render(cx);
            trek.window(cx, |window, cx| window.simulate_next_frame(cx));
        }
        trek.render(cx);
        let strip = trek.bounds(cx, "panel-tabs").expect("the tab strip");
        let front = cx
            .read(|cx| trek.root.read(cx).right_panel.read(cx).active_id())
            .expect("a tab in front");
        let tab = trek
            .bounds(cx, ("panel-tab", front as usize))
            .expect("the tab in front");
        assert!(
            tab.left() >= strip.left() - gpui_kit::px(0.5)
                && tab.right() <= strip.right() + gpui_kit::px(0.5),
            "{tab:?} in {strip:?}"
        );
        assert!(trek.visible(cx, "panel-tabs-all"), "a menu lists every tab");
    });
}

#[test]
fn trek_opens_on_a_thread_in_no_project() {
    run(async |cx| {
        let project = new_project("first");
        let mut s = settings();
        s.user_projects.push(project.display().to_string());
        let store = trek_core::store::Store::in_memory().expect("store");
        store.ensure_project(&project).expect("project");
        let (ws, _, _) = launch(cx, store, s);
        assert_eq!(
            ws.read_with(cx, |ws, _| ws.route.clone()),
            Route::Draft { project: None },
            "not whichever project is first"
        );
    });
}

#[test]
fn close_other_tabs_keeps_the_one_asked_about_and_every_thread() {
    run(async |cx| {
        let trek = open(cx);
        let mut ids = vec![];
        for text in ["first", "second", "third"] {
            let project = trek.project.clone();
            trek.update(cx, |ws, cx| {
                ws.navigate(
                    Route::Draft {
                        project: Some(project),
                    },
                    cx,
                )
            });
            let id = trek.send(cx, text);
            trek.wait_done(cx, &id, RunState::Idle).await;
            ids.push(id);
        }
        assert_eq!(tab_ids(&trek, cx), ids);
        let keep = ids[1].clone();
        trek.update(cx, |ws, cx| ws.close_other_tabs(&keep, cx));
        assert_eq!(tab_ids(&trek, cx), [keep.clone()]);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(keep));
        assert!(
            trek.read(cx, |ws, _| ids.iter().all(|id| ws.thread(id).is_some())),
            "closing a tab leaves its thread alone"
        );
    });
}

/// The tools panel, open on `tool` in `id`'s thread with the window `width` wide, at the side or
/// filling the window.
fn tools_open(
    trek: &common::Trek,
    cx: &mut gpui_kit::TestAppContext,
    id: &str,
    width: f32,
    tool: PanelTool,
    full: bool,
) -> gpui_kit::Entity<trek_app::panels::RightPanel> {
    cx.simulate_window_resize(trek.window, size(px(width), px(820.)));
    trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.to_string()), cx));
    let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
    trek.window(cx, |window, cx| {
        panel.update(cx, |p, cx| {
            p.open_tool(tool, window, cx);
            p.full = full;
            cx.notify();
        })
    });
    trek.render(cx);
    panel
}

/// Over the chat (a narrow window) or filling the window, the panel's header lies over the tab
/// strip, its hide button right on the strip's +: a click there hides the panel and nothing else.
/// It went through to the + as well, and a new thread opened behind the panel.
#[test]
fn hiding_the_tools_panel_opens_no_thread_under_it() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let threads = trek.read(cx, |ws, _| ws.threads.len());
        // Docked beside the chat, over the chat, and filling the window wide and narrow.
        for (width, full, over) in [
            (1280., false, false),
            (600., false, true),
            (1280., true, true),
            (600., true, true),
        ] {
            let state = format!("{width} wide, filling the window: {full}");
            for control in ["panel-close", "toggle-tools", "panel-fill"] {
                let panel = tools_open(&trek, cx, &id, width, PanelTool::Git, full);
                assert_eq!(trek.visible(cx, "tools-overlay"), over, "{state}");
                let (close, plus) = (
                    trek.bounds(cx, "panel-close").expect("hide button"),
                    trek.bounds(cx, "tab-new").expect("the strip's +"),
                );
                // What makes it a test of the click: over the chat the two are in one place.
                assert_eq!(close.intersects(&plus), over, "{state}: {close:?} {plus:?}");
                trek.click(cx, control);
                trek.render(cx);
                let (open, now_full) = cx.read(|cx| (panel.read(cx).open, panel.read(cx).full));
                if control == "panel-fill" {
                    assert!(open, "{state}: {control} keeps it open");
                    assert_eq!(now_full, !full, "{state}: {control} changes its size");
                } else {
                    assert!(!open, "{state}: {control} hides it");
                }
                assert_eq!(
                    trek.read(cx, |ws, _| ws.route.clone()),
                    Route::Thread(id.clone()),
                    "{state}: {control} leaves the thread on screen"
                );
                assert_eq!(tab_ids(&trek, cx), vec![id.clone()], "{state}: {control}");
                assert_eq!(
                    trek.read(cx, |ws, _| ws.threads.len()),
                    threads,
                    "{state}: {control} starts no thread"
                );
            }
        }
    });
}

/// Filling the window, the panel lies over the sidebar: a click on a tool where the sidebar's
/// Basecamp, Notes, Study or new-thread button is under it is the tool's alone. It went through,
/// and the thread was left for Study behind the panel.
#[test]
fn the_tools_panel_filling_the_window_keeps_clicks_off_the_sidebar() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first");
        trek.wait_done(cx, &id, RunState::Idle).await;
        for tool in [
            PanelTool::Git,
            PanelTool::Explorer,
            PanelTool::Terminal,
            PanelTool::Browser,
        ] {
            for row in ["open-study", "open-notes", "open-basecamp", "new-thread"] {
                let panel = tools_open(&trek, cx, &id, 1280., tool, true);
                let over = trek
                    .bounds(cx, "tools-overlay")
                    .expect("filling the window");
                let at = trek.bounds(cx, row).expect("the sidebar under it");
                assert!(over.contains(&at.center()), "{tool:?}: {row} {at:?}");
                // A click at the row's place, which is the panel's.
                trek.click(cx, row);
                trek.render(cx);
                assert_eq!(
                    trek.read(cx, |ws, _| ws.route.clone()),
                    Route::Thread(id.clone()),
                    "{tool:?}: a click where {row} is under the panel"
                );
                assert!(cx.read(|cx| panel.read(cx).open), "{tool:?}: {row}");
                // And hiding it, or putting it back at the side, comes back to that thread.
                trek.click(cx, "panel-fill");
                trek.click(cx, "panel-close");
                trek.render(cx);
                assert_eq!(
                    trek.read(cx, |ws, _| ws.route.clone()),
                    Route::Thread(id.clone()),
                    "{tool:?}: {row}"
                );
            }
        }
    });
}
