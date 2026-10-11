//! The in-app editor: a project file opens editable, marks dirty on change, and saves back.

use super::harness::{open, run};
use gpui_kit::test::TestWindowExt as _;
use crate::workspace::{PanelTool, Route};

#[test]
fn a_file_opens_in_the_editor_marks_dirty_and_saves() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        // As the Explorer's "Edit in Trek" does.
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), Some(1), cx));
        trek.render(cx);
        assert!(matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Editor { .. }));

        let editor = trek.root.read_with(cx, |r, cx| r.editor(cx)).expect("editor view");
        assert!(!editor.read_with(cx, |e, _| e.dirty()));

        // Typing dirties the buffer; saving clears it and lands the file.
        trek.window(cx, |window, cx| {
            editor.update(cx, |e, cx| e.text_state().update(cx, |s, cx| s.insert("let x = 1;\n", window, cx)));
        });
        cx.run_until_parked();
        assert!(editor.read_with(cx, |e, _| e.dirty()));

        editor.update(cx, |e, cx| e.save_now(cx));
        assert!(!editor.read_with(cx, |e, _| e.dirty()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "let x = 1;\nfn main() {}\n");
    });
}

#[test]
fn a_gone_file_opens_read_only_with_its_reason() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("missing.rs");

        trek.update(cx, |ws, cx| ws.open_editor(file, None, cx));
        trek.render(cx);

        let editor = trek.root.read_with(cx, |r, cx| r.editor(cx)).expect("editor view");
        assert!(!editor.read_with(cx, |e, _| e.dirty()));
    });
}

#[test]
fn a_deep_link_opens_the_editor_on_the_line() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("deep.rs");
        std::fs::write(&file, "a\nb\nc\nd\n").unwrap();

        // trek://edit?path=…&line=3 — as the editor extension sends it.
        let url = format!("trek://edit?path={}&line=3", file.display());
        cx.update(|cx| crate::deep_link::open(&url, cx));
        trek.render(cx);

        assert!(matches!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Editor { path } if path == file
        ));
    });
}

#[test]
fn the_ide_toggle_parks_the_chat_and_brings_it_back() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("from-ide.rs");
        std::fs::write(&file, "fn ide() {}\n").unwrap();

        // Chat is on this draft when the IDE takes over.
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project.clone()) }, cx));
        trek.update(cx, |ws, cx| ws.toggle_ide(cx));
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.ide()), "the toggle turns IDE mode on");

        // The file tree is the left column; clicking a file opens an editor tab while the
        // chat route stays put — it renders as the IDE's right column.
        trek.click(cx, file.display().to_string());
        trek.render(cx);
        assert!(
            trek.read(cx, |ws, _| ws.ide() && matches!(ws.route, Route::Draft { .. })),
            "a file click in IDE mode leaves the chat route alone"
        );
        let open = trek.root.read_with(cx, |r, cx| r.editor(cx).map(|e| e.read(cx).path.clone()));
        assert_eq!(open.as_deref(), Some(file.as_path()), "the file is an open editor tab");

        // Toggling back shows the same chat — it never went anywhere.
        trek.update(cx, |ws, cx| ws.toggle_ide(cx));
        trek.render(cx);
        assert_eq!(
            trek.read(cx, |ws, _| (ws.ide(), ws.route.clone())),
            (false, Route::Draft { project: Some(project) })
        );
    });
}

#[test]
fn cmd_k_in_the_ide_finds_files() {
    run(async |cx| {
        let trek = open(cx);
        std::fs::write(trek.project.join("a-needle-file.rs"), "fn x() {}\n").unwrap();
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        trek.update(cx, |ws, cx| ws.toggle_ide(cx));
        trek.render(cx);

        trek.press(cx, "secondary-k");
        trek.render(cx);
        trek.type_live(cx, "needle");
        trek.render(cx);

        let palette = cx.read(|cx| trek.root.read(cx).palette.clone());
        let labels: Vec<String> = palette.read_with(cx, |p, cx| p.entries(cx).iter().map(|e| e.label.to_string()).collect());
        assert!(labels.iter().any(|l| l == "a-needle-file.rs"), "file rows lead the IDE palette, got {labels:?}");
    });
}

#[test]
fn cmd_shift_f_searches_the_folder() {
    run(async |cx| {
        let trek = open(cx);
        std::fs::write(trek.project.join("searchable.txt"), "the quick brown fox\n").unwrap();
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        trek.update(cx, |ws, cx| ws.toggle_ide(cx));
        trek.press(cx, "secondary-shift-f");
        trek.render(cx);
        trek.type_live(cx, "quick brown");
        // The scan runs on a worker thread — poll for the hit row.
        let mut found = false;
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            cx.run_until_parked();
            trek.render(cx);
            if trek.visible(cx, ("ide-search-hit", 0usize)) {
                found = true;
                break;
            }
        }
        assert!(found, "a hit row should render for the match");
    });
}

#[test]
fn clicking_a_file_in_the_explorer_opens_it_in_the_editor() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("clicked.rs");
        std::fs::write(&file, "fn clicked() {}\n").unwrap();

        // The Explorer tool shows the file tree.
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.window(cx, |window, cx| panel.update(cx, |p, cx| p.open_tool(PanelTool::Explorer, window, cx)));
        trek.render(cx);

        trek.render(cx);
        assert!(trek.visible(cx, file.display().to_string()), "the file row should show");
        trek.click(cx, file.display().to_string());
        trek.render(cx);

        assert!(matches!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Editor { path } if path == file
        ));
    });
}

/// The editor in front, once its file is read (off the main thread).
async fn loaded(trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext) -> gpui_kit::Entity<crate::editor::EditorView> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        trek.render(cx);
        if let Some(e) = trek.root.read_with(cx, |r, cx| r.editor(cx)).filter(|e| e.read_with(cx, |e, _| !e.loading())) {
            return e;
        }
        assert!(std::time::Instant::now() < deadline, "timed out reading the file");
        cx.background_executor.timer(std::time::Duration::from_millis(5)).await;
    }
}

#[test]
fn pipes_devices_and_huge_files_open_read_only_without_hanging() {
    run(async |cx| {
        let trek = open(cx);
        let big = trek.project.join("big.log");
        std::fs::File::create(&big).unwrap().set_len(2_000_001).unwrap();
        let mut cases = Vec::new();
        // Unix only: Windows has no FIFOs or device files to open by path.
        #[cfg(unix)]
        {
            // A pipe nobody writes to: reading it would wait for ever.
            let fifo = trek.project.join("pipe");
            let c = std::ffi::CString::new(fifo.display().to_string()).unwrap();
            // SAFETY: a plain path, a plain mode.
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
            cases.push((fifo, "Not a file"));
            cases.push((std::path::PathBuf::from("/dev/zero"), "Not a file"));
        }
        cases.push((big, "Too big"));
        for (path, why) in cases {
            trek.update(cx, |ws, cx| ws.open_editor(path.clone(), None, cx));
            let editor = loaded(&trek, cx).await;
            let (problem, text) = editor.read_with(cx, |e, cx| (e.problem().map(str::to_string), e.text_state().read(cx).value().to_string()));
            assert!(problem.as_deref().is_some_and(|p| p.starts_with(why)), "{}: {problem:?}", path.display());
            assert!(text.is_empty());
        }
    });
}

#[test]
fn a_deep_link_lands_on_its_line_once_the_file_is_read() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("lines.rs");
        std::fs::write(&file, "a\nb\nc\nd\n").unwrap();
        cx.update(|cx| crate::deep_link::open(&format!("trek://edit?path={}&line=3", file.display()), cx));
        let editor = loaded(&trek, cx).await;
        assert_eq!(editor.read_with(cx, |e, cx| e.caret(cx)).0, 3);
    });
}

#[test]
fn saving_replaces_the_file_whole_and_keeps_its_permissions_and_links() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("run.sh");
        std::fs::write(&file, "echo one\n").unwrap();
        // The executable bit is Unix's: a Windows file has none to keep.
        #[cfg(unix)]
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let link = trek.project.join("link.sh");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&file, &link).unwrap();
        // Making a link takes Developer Mode or an elevated process on Windows: without either, the
        // file is edited directly and the link checks are skipped.
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&file, &link).is_ok();
        #[cfg(unix)]
        let linked = true;
        // Edited through the link.
        let opened = if linked { link.clone() } else { file.clone() };
        trek.update(cx, |ws, cx| ws.open_editor(opened.clone(), None, cx));
        let editor = loaded(&trek, cx).await;
        trek.window(cx, |window, cx| editor.update(cx, |e, cx| e.text_state().update(cx, |s, cx| s.insert("set -e\n", window, cx))));
        cx.run_until_parked();
        editor.update(cx, |e, cx| e.save_now(cx));
        assert!(!editor.read_with(cx, |e, _| e.dirty()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "set -e\necho one\n");
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&file).unwrap().permissions()) & 0o777, 0o755, "still runs");
        if linked {
            assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link is still a link");
        }
        let left: Vec<String> = std::fs::read_dir(&trek.project).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.contains("trek-save")).collect();
        assert!(left.is_empty(), "no temporary file left behind: {left:?}");
    });
}

#[test]
fn a_file_that_changes_on_disk_again_warns_again_before_its_overwritten() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("notes.md");
        std::fs::write(&file, "mine\n").unwrap();
        let toasts = std::rc::Rc::new(std::cell::RefCell::new(0));
        let sink = toasts.clone();
        cx.update(|cx| {
            cx.subscribe(&trek.ws, move |_, event: &crate::workspace::WorkspaceEvent, _| {
                if let crate::workspace::WorkspaceEvent::Toast { message, .. } = event {
                    if message.contains("Save again") {
                        *sink.borrow_mut() += 1;
                    }
                }
            })
            .detach()
        });
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
        let editor = loaded(&trek, cx).await;
        trek.window(cx, |window, cx| editor.update(cx, |e, cx| e.text_state().update(cx, |s, cx| s.insert("edited ", window, cx))));
        cx.run_until_parked();
        // Changed on disk under the edit: the first save warns.
        std::fs::write(&file, "theirs, once\n").unwrap();
        editor.update(cx, |e, cx| e.save_now(cx));
        assert_eq!((*toasts.borrow(), std::fs::read_to_string(&file).unwrap().as_str()), (1, "theirs, once\n"));
        // Changed again: the next save warns again rather than writing over what's new.
        std::fs::write(&file, "theirs, twice\n").unwrap();
        editor.update(cx, |e, cx| e.save_now(cx));
        assert_eq!((*toasts.borrow(), std::fs::read_to_string(&file).unwrap().as_str()), (2, "theirs, twice\n"));
        // Saved again with nothing new on disk: the user's version goes.
        editor.update(cx, |e, cx| e.save_now(cx));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "edited mine\n");
        assert!(!editor.read_with(cx, |e, _| e.dirty()));
    });
}

/// The rows on screen and the scroll offset (px, down is positive) of `editor`'s text.
fn on_screen(editor: &gpui_kit::Entity<crate::editor::EditorView>, cx: &gpui_kit::TestAppContext) -> (std::ops::Range<usize>, f32) {
    editor.read_with(cx, |e, cx| {
        let state = e.text_state();
        let state = state.read(cx);
        (state.visible_row_range().expect("laid out"), -state.scroll_offset().y.as_f32())
    })
}

/// Open `file` at `line` and come back with the rows on screen once the editor has settled.
/// `painted_first`: a frame is drawn while the file is still being read, so the editor has been
/// laid out (empty) by the time the text arrives; otherwise the text arrives before the first frame.
async fn opened_at(trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext, file: &std::path::Path, line: u32, painted_first: bool) -> (std::ops::Range<usize>, f32) {
    trek.update(cx, |ws, cx| ws.open_editor(file.to_path_buf(), Some(line), cx));
    if painted_first {
        trek.window(cx, |window, cx| window.render_frame(cx));
    }
    let editor = loaded(trek, cx).await;
    for _ in 0..4 {
        trek.render(cx);
    }
    on_screen(&editor, cx)
}

#[test]
fn opening_at_a_line_shows_it_whichever_comes_first_the_frame_or_the_file() {
    run(async |cx| {
        let trek = open(cx);
        let mut seen = vec![];
        for painted_first in [false, true] {
            // A file of its own each time round, so the editor is a fresh one.
            let file = trek.project.join(format!("long-{painted_first}.rs"));
            std::fs::write(&file, (1..=200).map(|n| format!("// line {n}\n")).collect::<String>()).unwrap();
            // Line 40 is past the first screen: it comes up in the middle, neither at the bottom
            // edge (as when the editor had been laid out by the time the text arrived) nor off
            // screen (as when it hadn't).
            let (rows, offset) = opened_at(&trek, cx, &file, 40, painted_first).await;
            assert!(rows.contains(&39), "painted_first={painted_first}: line 40 is on screen, rows {rows:?}");
            assert!(39 - rows.start > 5 && rows.end - 39 > 5, "painted_first={painted_first}: line 40 is clear of both edges, rows {rows:?}");
            seen.push((rows.clone(), offset));
            // Asked for again on a line that's on screen already, nothing moves.
            let (again, _) = opened_at(&trek, cx, &file, 41, painted_first).await;
            assert_eq!(again, rows, "painted_first={painted_first}");
            // Near the top of the file the top is as high as it goes; at the end, the end.
            let (top, offset) = opened_at(&trek, cx, &file, 3, painted_first).await;
            assert_eq!((top.start, offset), (0, 0.), "painted_first={painted_first}");
            let (end, _) = opened_at(&trek, cx, &file, 200, painted_first).await;
            assert!(end.contains(&199), "painted_first={painted_first}: the last line is on screen, rows {end:?}");
            // The caret is on the line, as ever.
            let editor = trek.root.read_with(cx, |r, cx| r.editor(cx)).expect("editor");
            assert_eq!(editor.read_with(cx, |e, cx| e.caret(cx)).0, 200);
        }
        assert_eq!(seen[0], seen[1], "the same scroll whichever came first");
    });
}

#[test]
fn quitting_with_a_file_open_at_a_line_leaves_no_handle_on_its_editor() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("quit.rs");
        std::fs::write(&file, (1..=200).map(|n| format!("// line {n}\n")).collect::<String>()).unwrap();
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), Some(40), cx));
        let editor = loaded(&trek, cx).await;
        // A frame that asks for the scroll, and the quit before it is taken up.
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), Some(150), cx));
        trek.window(cx, |window, cx| window.render_frame(cx));
        let (state, ws) = (editor.read_with(cx, |e, _| e.text_state().downgrade()), trek.ws.downgrade());
        drop((editor, trek));
        cx.update(|cx| {
            cx.shutdown();
            cx.clear_globals();
        });
        cx.run_until_parked();
        state.assert_released();
        ws.assert_released();
    });
}
