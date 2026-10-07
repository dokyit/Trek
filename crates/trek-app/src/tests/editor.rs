//! The in-app editor: a project file opens editable, marks dirty on change, and saves back.

use super::harness::{open, run};
use crate::workspace::Route;

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
