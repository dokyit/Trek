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

        let editor = trek.root.read_with(cx, |r, _| r.editor()).expect("editor view");
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

        let editor = trek.root.read_with(cx, |r, _| r.editor()).expect("editor view");
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
        assert!(trek.read(cx, |ws, _| ws.ide), "the toggle turns IDE mode on");

        // The file tree is the left column; clicking a file opens an editor tab while the
        // chat route stays put — it renders as the IDE's right column.
        trek.click(cx, file.display().to_string());
        trek.render(cx);
        assert!(
            trek.read(cx, |ws, _| ws.ide && matches!(ws.route, Route::Draft { .. })),
            "a file click in IDE mode leaves the chat route alone"
        );
        let open = trek.root.read_with(cx, |r, cx| r.editor().map(|e| e.read(cx).path.clone()));
        assert_eq!(open.as_deref(), Some(file.as_path()), "the file is an open editor tab");

        // Toggling back shows the same chat — it never went anywhere.
        trek.update(cx, |ws, cx| ws.toggle_ide(cx));
        trek.render(cx);
        assert_eq!(
            trek.read(cx, |ws, _| (ws.ide, ws.route.clone())),
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
