//! The in-app editor: a project file opens editable, marks dirty on change, and saves back.

use super::harness::{open, run};
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

        trek.press(cx, "cmd-k");
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
        trek.press(cx, "cmd-shift-f");
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
        // A pipe nobody writes to: reading it would wait for ever.
        let fifo = trek.project.join("pipe");
        let c = std::ffi::CString::new(fifo.display().to_string()).unwrap();
        // SAFETY: a plain path, a plain mode.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        let big = trek.project.join("big.log");
        std::fs::File::create(&big).unwrap().set_len(2_000_001).unwrap();
        for (path, why) in [(fifo, "Not a file"), (std::path::PathBuf::from("/dev/zero"), "Not a file"), (big, "Too big")] {
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
    use std::os::unix::fs::PermissionsExt as _;
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("run.sh");
        std::fs::write(&file, "echo one\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = trek.project.join("link.sh");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        // Edited through the link.
        trek.update(cx, |ws, cx| ws.open_editor(link.clone(), None, cx));
        let editor = loaded(&trek, cx).await;
        trek.window(cx, |window, cx| editor.update(cx, |e, cx| e.text_state().update(cx, |s, cx| s.insert("set -e\n", window, cx))));
        cx.run_until_parked();
        editor.update(cx, |e, cx| e.save_now(cx));
        assert!(!editor.read_with(cx, |e, _| e.dirty()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "set -e\necho one\n");
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o755, "still runs");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link is still a link");
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
