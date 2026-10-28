//! The editor's way out and Format Document: a file shown on its own (Agents mode, from the
//! Explorer) closes with its ×, ⌘W / Ctrl+W and the palette and goes back to where it came from;
//! the editor's last tab leaves a way back to Agents; and Format document runs the language's
//! formatter, replacing the text only when it succeeds (the fixtures stand in for `rustfmt`).

mod common;

use common::loaded;
use common::{REAL_TIME_LIMIT, Trek, open, run};
use gpui_kit::{Entity, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;
use trek_app::workspace::{Mode, Route};

fn open_tabs(trek: &Trek, cx: &TestAppContext) -> usize {
    let ide = cx.read(|cx| trek.root.read(cx).ide.clone());
    cx.read(|cx| ide.read(cx).tab_paths(cx).len())
}

fn route(trek: &Trek, cx: &TestAppContext) -> Route {
    trek.read(cx, |ws, _| ws.route.clone())
}

/// A file opened on its own from a draft in the project: Agents mode, the harness's lone editor.
async fn file_on_its_own(
    trek: &Trek,
    cx: &mut TestAppContext,
    name: &str,
) -> (
    std::path::PathBuf,
    Route,
    Entity<trek_app::editor::EditorView>,
) {
    let file = trek.project.join(name);
    std::fs::write(&file, "[a]\nb = 1\n").unwrap();
    let project = trek.project.clone();
    trek.update(cx, |ws, cx| {
        ws.navigate(
            Route::Draft {
                project: Some(project),
            },
            cx,
        )
    });
    let from = route(trek, cx);
    trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
    let editor = loaded(trek, cx).await;
    assert!(
        matches!(route(trek, cx), Route::Editor { .. }),
        "the file is on its own"
    );
    (file, from, editor)
}

#[test]
fn a_file_shown_on_its_own_has_a_close_button_that_goes_back_to_where_it_came_from() {
    run(async |cx| {
        let trek = open(cx);
        let (_, from, _) = file_on_its_own(&trek, cx, "launcher_imgui.ini").await;
        assert_eq!(open_tabs(&trek, cx), 1);
        // The header Tobias had (name, Save, open externally) has the way out too.
        assert!(trek.visible(cx, "editor-close"));
        assert!(trek.visible(cx, "editor-save") && trek.visible(cx, "editor-open-default"));
        trek.click(cx, "editor-close");
        trek.render(cx);
        assert!(
            route(&trek, cx) == from,
            "back on the draft it was opened from"
        );
        assert_eq!(open_tabs(&trek, cx), 0, "closed, not parked behind");
        assert!(!trek.visible(cx, "editor-close"));
    });
}

#[test]
fn the_shortcuts_and_the_palette_close_a_file_shown_on_its_own() {
    run(async |cx| {
        let trek = open(cx);
        // A thread, then a file from its project: closing goes back to the thread.
        let id = trek.send(cx, "hello");
        assert!(route(&trek, cx) == Route::Thread(id.clone()));
        let file = trek.project.join("a.ini");
        std::fs::write(&file, "[a]\n").unwrap();
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
        loaded(&trek, cx).await;
        assert!(matches!(route(&trek, cx), Route::Editor { .. }));
        trek.press(cx, "secondary-w");
        trek.render(cx);
        assert!(
            route(&trek, cx) == Route::Thread(id.clone()),
            "⌘W / Ctrl+W: back to the thread"
        );
        assert_eq!(open_tabs(&trek, cx), 0);

        // Ctrl+F4 as well, on Windows.
        if cfg!(windows) {
            trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
            loaded(&trek, cx).await;
            trek.press(cx, "ctrl-f4");
            trek.render(cx);
            assert!(route(&trek, cx) == Route::Thread(id.clone()), "Ctrl+F4");
        }

        // And "Close file" in the palette.
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
        loaded(&trek, cx).await;
        trek.press(cx, "secondary-k");
        trek.type_text(cx, "Close file");
        trek.press(cx, "enter");
        trek.render(cx);
        assert!(route(&trek, cx) == Route::Thread(id), "the palette");
        assert_eq!(open_tabs(&trek, cx), 0);
    });
}

#[test]
fn closing_a_file_shown_on_its_own_asks_about_unsaved_edits_and_stays_when_cancelled() {
    run(async |cx| {
        let trek = open(cx);
        let (file, from, editor) = file_on_its_own(&trek, cx, "edited.ini").await;
        trek.window(cx, |window, cx| {
            editor.update(cx, |e, cx| {
                e.text_state().update(cx, |s, cx| s.insert("x", window, cx))
            })
        });
        cx.run_until_parked();
        trek.click(cx, "editor-close");
        assert!(cx.has_pending_prompt(), "unsaved edits: asks first");
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert!(
            matches!(route(&trek, cx), Route::Editor { .. }) && open_tabs(&trek, cx) == 1,
            "Cancel keeps it"
        );
        trek.press(cx, "secondary-w");
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Don't Save");
        cx.run_until_parked();
        trek.render(cx);
        assert!(route(&trek, cx) == from);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "[a]\nb = 1\n",
            "nothing written"
        );
    });
}

/// An image (or a PDF, a file that isn't text) shown on its own is a preview, not an editor: it
/// has the same way out, and closing it goes back as an editor's does instead of the screen
/// making its tab again.
#[test]
fn a_picture_shown_on_its_own_closes_like_a_file_does() {
    run(async |cx| {
        let trek = open(cx);
        let png = trek.project.join("chart.png");
        image::RgbaImage::from_pixel(40, 30, image::Rgba([200, 80, 40, 255]))
            .save(&png)
            .unwrap();
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project),
                },
                cx,
            )
        });
        let from = route(&trek, cx);
        for close in ["the ×", "⌘W / Ctrl+W", "the palette"] {
            trek.update(cx, |ws, cx| ws.open_editor(png.clone(), None, cx));
            trek.render(cx);
            assert!(
                matches!(route(&trek, cx), Route::Editor { .. }),
                "{close}: the picture is on its own"
            );
            assert!(
                trek.visible(cx, "ide-preview-image") && trek.visible(cx, "editor-close"),
                "{close}: the picture, and the way out"
            );
            match close {
                "the ×" => trek.click(cx, "editor-close"),
                "⌘W / Ctrl+W" => trek.press(cx, "secondary-w"),
                _ => {
                    trek.press(cx, "secondary-k");
                    trek.type_text(cx, "Close file");
                    trek.press(cx, "enter");
                }
            }
            trek.render(cx);
            assert!(route(&trek, cx) == from, "{close}: back on the draft");
            assert_eq!(open_tabs(&trek, cx), 0, "{close}: closed, not made again");
        }
    });
}

#[test]
fn the_last_tab_closed_in_the_editor_leaves_its_watermark_and_the_switch_goes_back() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("only.rs");
        std::fs::write(&file, "fn only() {}\n").unwrap();
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project),
                },
                cx,
            )
        });
        trek.update(cx, |ws, cx| ws.set_mode(Mode::Editor, cx));
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
        trek.render(cx);
        assert_eq!(open_tabs(&trek, cx), 1);
        // The tab's × is there for the tab in front, without hovering.
        assert!(trek.visible(cx, ("ide-tab-close", 0usize)));
        trek.press(cx, "secondary-w");
        trek.render(cx);
        assert_eq!(open_tabs(&trek, cx), 0);
        // An editor of its own: no button back to Agents, the title bar's switch is the way.
        assert!(trek.visible(cx, "ide-watermark") && !trek.visible(cx, "ide-back-to-agents"));
        trek.click(cx, "mode-agents");
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.mode), Mode::Agents);
    });
}

// ───────────────────────── Format document ─────────────────────────

/// What `trek_app::format::resolve` finds, in place of a formatter on the PATH, until dropped.
struct Formatting;

impl Formatting {
    fn with(args: &[&str]) -> Self {
        let program = trek_test_fixtures::bin("fixture");
        let args = args.iter().map(|a| a.to_string()).collect();
        trek_app::format::tests::OVERRIDE.with(|o| *o.borrow_mut() = Some(Some((program, args))));
        Formatting
    }

    fn none() -> Self {
        trek_app::format::tests::OVERRIDE.with(|o| *o.borrow_mut() = Some(None));
        Formatting
    }
}

impl Drop for Formatting {
    fn drop(&mut self) {
        trek_app::format::tests::OVERRIDE.with(|o| *o.borrow_mut() = None);
    }
}

fn toasts(trek: &Trek, cx: &mut TestAppContext) -> Rc<RefCell<Vec<String>>> {
    let seen = Rc::new(RefCell::new(vec![]));
    let sink = seen.clone();
    cx.update(|cx| {
        cx.subscribe(
            &trek.ws,
            move |_, event: &trek_app::workspace::WorkspaceEvent, _| {
                if let trek_app::workspace::WorkspaceEvent::Toast { message, .. } = event {
                    sink.borrow_mut().push(message.clone());
                }
            },
        )
        .detach()
    });
    seen
}

fn buffer_of(editor: &Entity<trek_app::editor::EditorView>, cx: &TestAppContext) -> String {
    editor.read_with(cx, |e, cx| e.text_state().read(cx).value().to_string())
}

fn focus_editor_text(
    trek: &Trek,
    cx: &mut TestAppContext,
    editor: &Entity<trek_app::editor::EditorView>,
) {
    trek.window(cx, |window, cx| {
        editor.update(cx, |e, cx| e.focus(window, cx))
    });
    cx.run_until_parked();
}

/// Wait (the formatter is a real process) until `until` says the buffer or the toasts show its answer.
async fn wait_for(
    cx: &mut TestAppContext,
    editor: &Entity<trek_app::editor::EditorView>,
    toasts: &Rc<RefCell<Vec<String>>>,
    until: impl Fn(&str, &[String]) -> bool,
) {
    let deadline = std::time::Instant::now() + REAL_TIME_LIMIT;
    loop {
        cx.run_until_parked();
        if until(&buffer_of(editor, cx), &toasts.borrow()) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for the formatter: buffer {:?}, toasts {:?}",
            buffer_of(editor, cx),
            toasts.borrow()
        );
        cx.background_executor
            .timer(std::time::Duration::from_millis(5))
            .await;
    }
}

fn format(trek: &Trek, cx: &mut TestAppContext, editor: &Entity<trek_app::editor::EditorView>) {
    trek.window(cx, |window, cx| {
        editor.update(cx, |e, cx| e.format(window, cx))
    });
}

#[test]
fn format_document_replaces_the_text_with_what_the_formatter_prints_and_undo_takes_it_back() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("main.rs");
        std::fs::write(&file, "fn  main( ) {}\n").unwrap();
        let _formatter = Formatting::with(&["upper"]);
        let toasts = toasts(&trek, cx);
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
        let editor = loaded(&trek, cx).await;
        // The caret after `main`, to see it kept near.
        let at = "fn  main".len();
        trek.window(cx, |_, cx| {
            editor.update(cx, |e, cx| {
                e.text_state()
                    .update(cx, |s, cx| s.set_selected_range(at..at, cx))
            })
        });
        format(&trek, cx, &editor);
        wait_for(cx, &editor, &toasts, |text, _| text == "FN  MAIN( ) {}\n").await;
        assert!(
            editor.read_with(cx, |e, _| e.dirty()),
            "it's an edit, saved when the user says so"
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "fn  main( ) {}\n",
            "nothing written to the file"
        );
        assert_eq!(
            editor.read_with(cx, |e, cx| e.text_state().read(cx).cursor()),
            at,
            "the caret stays after `main`"
        );
        assert!(toasts.borrow().is_empty(), "{:?}", toasts.borrow());
        // One edit: undo puts the text back.
        focus_editor_text(&trek, cx, &editor);
        trek.press(cx, "secondary-z");
        assert_eq!(buffer_of(&editor, cx), "fn  main( ) {}\n");
        assert!(
            !editor.read_with(cx, |e, _| e.dirty()),
            "undone, so as saved"
        );
    });
}

#[test]
fn format_document_runs_from_its_shortcut_and_leaves_a_file_a_formatter_refused_alone() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("main.rs");
        std::fs::write(&file, "fn main( ) {}\n").unwrap();
        let toasts = toasts(&trek, cx);
        // The harness binds the app's keys; the editor's (⌘S, ⇧⌥F) are bound beside them in `main`.
        cx.update(|cx| cx.bind_keys(trek_app::editor::key_bindings()));
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
        let editor = loaded(&trek, cx).await;
        focus_editor_text(&trek, cx, &editor);

        // The formatter fails: its first line is the toast, the text is as it was.
        let failing = Formatting::with(&["fail", "1", "error: expected `;`", "more"]);
        trek.press(cx, "alt-shift-f");
        wait_for(cx, &editor, &toasts, |_, t| !t.is_empty()).await;
        assert_eq!(
            *toasts.borrow(),
            vec!["fixture couldn't format it: error: expected `;`".to_string()]
        );
        assert_eq!(buffer_of(&editor, cx), "fn main( ) {}\n");
        assert!(!editor.read_with(cx, |e, _| e.dirty()));
        drop(failing);

        // Output equal to the input: said so, nothing changes.
        let _same = Formatting::with(&["cat"]);
        format(&trek, cx, &editor);
        wait_for(cx, &editor, &toasts, |_, t| t.len() == 2).await;
        assert_eq!(toasts.borrow()[1], "Already formatted");
        assert!(!editor.read_with(cx, |e, _| e.dirty()));
    });
}

#[test]
fn format_document_says_when_there_is_no_formatter_and_when_it_is_not_installed() {
    run(async |cx| {
        let trek = open(cx);
        let toasts = toasts(&trek, cx);
        // A language with none.
        let cs = trek.project.join("Program.cs");
        std::fs::write(&cs, "class A {}\n").unwrap();
        trek.update(cx, |ws, cx| ws.open_editor(cs.clone(), None, cx));
        let editor = loaded(&trek, cx).await;
        format(&trek, cx, &editor);
        assert_eq!(
            *toasts.borrow(),
            vec!["No formatter for .cs on this computer".to_string()]
        );
        // One that has, but not on this computer.
        let rs = trek.project.join("main.rs");
        std::fs::write(&rs, "fn main() {}\n").unwrap();
        let _nothing_installed = Formatting::none();
        trek.update(cx, |ws, cx| ws.open_editor(rs.clone(), None, cx));
        let editor = loaded(&trek, cx).await;
        format(&trek, cx, &editor);
        assert_eq!(
            toasts.borrow()[1],
            "Format Document needs rustfmt for .rs files, and it isn't on this computer"
        );
        assert_eq!(buffer_of(&editor, cx), "fn main() {}\n");
    });
}

#[test]
fn text_typed_while_the_formatter_ran_is_not_overwritten() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        // Slow enough to type in the meantime, and different from what's there when it answers.
        let _formatter = Formatting::with(&["cat", "--delay", "1"]);
        let toasts = toasts(&trek, cx);
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
        let editor = loaded(&trek, cx).await;
        format(&trek, cx, &editor);
        trek.window(cx, |window, cx| {
            editor.update(cx, |e, cx| {
                e.text_state()
                    .update(cx, |s, cx| s.insert("// mine\n", window, cx))
            })
        });
        wait_for(cx, &editor, &toasts, |_, t| !t.is_empty()).await;
        assert_eq!(
            *toasts.borrow(),
            vec![
                "The file changed while it was being formatted, so nothing was replaced"
                    .to_string()
            ]
        );
        assert_eq!(buffer_of(&editor, cx), "// mine\nfn main() {}\n");
    });
}

#[test]
fn format_document_is_in_the_palette_beside_its_keys() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
        loaded(&trek, cx).await;
        trek.press(cx, "secondary-k");
        trek.type_text(cx, "Format document");
        trek.render(cx);
        let palette = cx.read(|cx| trek.root.read(cx).palette.clone());
        let rows: Vec<(String, String)> = palette.read_with(cx, |p, cx| {
            p.entries(cx)
                .iter()
                .map(|e| {
                    (
                        e.label.to_string(),
                        e.hint.as_ref().map(|h| h.to_string()).unwrap_or_default(),
                    )
                })
                .collect()
        });
        let want = trek_app::keys::hint(trek_app::keys::Id::FormatDocument);
        assert!(
            rows.iter()
                .any(|(label, hint)| label == "Format document" && *hint == want),
            "{rows:?}"
        );
    });
}

// ───────────────────────── tabs or spaces ─────────────────────────

#[test]
fn the_tab_key_types_a_tab_in_a_makefile_and_go_and_spaces_elsewhere() {
    run(async |cx| {
        let trek = open(cx);
        for (name, typed) in [("Makefile", "\t"), ("main.go", "\t"), ("main.rs", "    ")] {
            let file = trek.project.join(name);
            std::fs::write(&file, "x\n").unwrap();
            trek.update(cx, |ws, cx| ws.open_editor(file.clone(), None, cx));
            let editor = loaded(&trek, cx).await;
            focus_editor_text(&trek, cx, &editor);
            trek.press(cx, "tab");
            assert_eq!(
                buffer_of(&editor, cx),
                format!("{typed}x\n"),
                "Tab in {name}"
            );
        }
    });
}
