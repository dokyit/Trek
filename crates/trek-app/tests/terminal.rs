//! The terminal as a terminal: ⌘-clicking a link opens it where the user chose (asked the first
//! time), dragging selects and ⌘C copies, new output doesn't scramble a selection, the wheel goes
//! to full-screen programs, and the bell, the title, the text size and the way back down from
//! the scrollback show.

mod common;

use common::{Trek, open, open_with, run};
use gpui_kit::{
    Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollDelta, TestAppContext, point, px,
};
use trek_app::panels::terminal::TerminalPanel;
use trek_app::workspace::PanelTool;
use trek_core::settings::OpenLinks;

const VITE: &[u8] = b"\r\n  VITE v5.4.2  ready in 312 ms\r\n\r\n  \xe2\x9e\x9c  Local:   http://localhost:5173/\r\n  \xe2\x9e\x9c  Network: use --host to expose\r\n";

/// The dock's shell, shown.
fn dock_terminal(trek: &Trek, cx: &mut TestAppContext) -> Entity<TerminalPanel> {
    trek.click(cx, "open-terminal");
    trek.render(cx);
    let dock = trek.root.read_with(cx, |r, _| r.terminal_dock.clone());
    dock.read_with(cx, |d, cx| d.terminals(cx)[0].clone())
}

fn print(trek: &Trek, cx: &mut TestAppContext, term: &Entity<TerminalPanel>, bytes: &[u8]) {
    term.update(cx, |t, cx| t.print(bytes, cx));
    trek.render(cx);
}

fn dispatch(trek: &Trek, cx: &mut TestAppContext, event: gpui_kit::PlatformInput) {
    trek.window(cx, |window, cx| {
        window.dispatch_event(event, cx);
    });
    trek.render(cx);
}

fn move_to(trek: &Trek, cx: &mut TestAppContext, position: Point<Pixels>, modifiers: Modifiers) {
    dispatch(
        trek,
        cx,
        gpui_kit::PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers,
        }),
    );
}

fn click(
    trek: &Trek,
    cx: &mut TestAppContext,
    position: Point<Pixels>,
    modifiers: Modifiers,
    click_count: usize,
) {
    move_to(trek, cx, position, modifiers);
    dispatch(
        trek,
        cx,
        gpui_kit::PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count,
            first_mouse: false,
        }),
    );
    dispatch(
        trek,
        cx,
        gpui_kit::PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count,
        }),
    );
}

/// Where the n-th link on screen (from 1) is drawn, and its address.
fn link(
    term: &Entity<TerminalPanel>,
    cx: &mut TestAppContext,
    n: usize,
) -> (Point<Pixels>, String) {
    term.read_with(cx, |t, _| {
        let link = t.links().into_iter().nth(n - 1).expect("a link on screen");
        (t.cell_center(link.row, link.cols.start + 3), link.url)
    })
}

fn open_links(trek: &Trek, cx: &TestAppContext) -> OpenLinks {
    trek.read(cx, |ws, _| ws.settings.terminal.open_links)
}

/// The Browser tool is open in the tools panel.
fn browser_open(trek: &Trek, cx: &TestAppContext) -> bool {
    let panel = trek.root.read_with(cx, |r, _| r.right_panel.clone());
    panel.read_with(cx, |p, _| p.open && p.browser().is_some())
}

#[test]
fn command_click_opens_a_dev_servers_link_and_a_plain_click_does_not() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.terminal.open_links = OpenLinks::Default);
        let term = dock_terminal(&trek, cx);
        print(&trek, cx, &term, VITE);
        let (at, url) = link(&term, cx, 1);
        assert_eq!(url, "http://localhost:5173/");

        // A plain click is a click: the keys come here, nothing opens, nothing is underlined.
        click(&trek, cx, at, Modifiers::default(), 1);
        assert_eq!(cx.opened_url(), None);
        assert_eq!(term.read_with(cx, |t, _| t.hovered_link()), None);
        assert!(trek.window(cx, |window, cx| {
            term.read(cx).focus_handle().is_focused(window)
        }));

        // ⌘ held over it: underlined (a hand for a pointer); ⌘-click opens it.
        move_to(&trek, cx, at, Modifiers::secondary_key());
        assert_eq!(
            term.read_with(cx, |t, _| t.hovered_link()).as_deref(),
            Some("http://localhost:5173/")
        );
        move_to(&trek, cx, at, Modifiers::default());
        assert_eq!(
            term.read_with(cx, |t, _| t.hovered_link()),
            None,
            "let go of ⌘: plain text again"
        );
        click(&trek, cx, at, Modifiers::secondary_key(), 1);
        assert_eq!(cx.opened_url().as_deref(), Some("http://localhost:5173/"));
        assert!(!browser_open(&trek, cx));
        assert_eq!(
            term.read_with(cx, |t, _| t.selected()),
            None,
            "a link's click selects nothing"
        );
    });
}

#[test]
fn links_open_in_the_built_in_browser_from_the_dock_and_the_tools_panels_terminal() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.terminal.open_links = OpenLinks::BuiltIn);
        let term = dock_terminal(&trek, cx);
        print(&trek, cx, &term, VITE);
        let (at, _) = link(&term, cx, 1);
        click(&trek, cx, at, Modifiers::secondary_key(), 1);
        assert!(browser_open(&trek, cx), "the Browser tool beside the chat");
        assert_eq!(cx.opened_url(), None);

        // The tools panel's own Terminal tab goes the same way.
        let panel = trek.root.read_with(cx, |r, _| r.right_panel.clone());
        trek.window(cx, |window, cx| {
            panel.update(cx, |p, cx| p.open_tool(PanelTool::Terminal, window, cx))
        });
        trek.render(cx);
        let tab = panel
            .read_with(cx, |p, _| p.shown_terminal())
            .expect("the panel's terminal");
        print(&trek, cx, &tab, b"docs at https://example.com/guide.\r\n");
        let opened = trek
            .window(cx, |window, cx| {
                tab.update(cx, |t, cx| t.open_nth_link(1, window, cx))
            })
            .unwrap();
        assert_eq!(
            opened, "https://example.com/guide",
            "the full stop is the sentence's"
        );
        trek.render(cx);
        assert_eq!(
            panel.read_with(cx, |p, _| p.shown_terminal()),
            None,
            "the Browser came to the front"
        );
        assert!(browser_open(&trek, cx));
        assert_eq!(cx.opened_url(), None);
    });
}

#[test]
fn the_first_link_asks_where_links_open_and_the_answer_is_kept() {
    run(async |cx| {
        let trek = open(cx);
        assert_eq!(open_links(&trek, cx), OpenLinks::Ask);
        let term = dock_terminal(&trek, cx);
        print(&trek, cx, &term, VITE);
        print(
            &trek,
            cx,
            &term,
            b"Uvicorn running on http://127.0.0.1:8000 (Press CTRL+C to quit)\r\n",
        );
        let (at, _) = link(&term, cx, 1);

        // Cancelled: nothing opens, and it asks again next time.
        click(&trek, cx, at, Modifiers::secondary_key(), 1);
        assert!(
            trek.visible(cx, "terminal-links-builtin")
                && trek.visible(cx, "terminal-links-default")
        );
        trek.click(cx, "terminal-links-cancel");
        trek.render(cx);
        assert!(!trek.visible(cx, "terminal-links-default"));
        assert_eq!(
            (open_links(&trek, cx), cx.opened_url()),
            (OpenLinks::Ask, None)
        );

        click(&trek, cx, at, Modifiers::secondary_key(), 1);
        assert_eq!(cx.opened_url(), None, "nothing opens before the answer");
        trek.click(cx, "terminal-links-default");
        trek.render(cx);
        assert_eq!(
            open_links(&trek, cx),
            OpenLinks::Default,
            "kept in settings"
        );
        assert_eq!(cx.opened_url().as_deref(), Some("http://localhost:5173/"));
        assert!(!trek.visible(cx, "terminal-links-default"));

        // From now on it doesn't ask.
        let (at, url) = link(&term, cx, 2);
        assert_eq!(url, "http://127.0.0.1:8000");
        click(&trek, cx, at, Modifiers::secondary_key(), 1);
        assert!(!trek.visible(cx, "terminal-links-default"));
        assert_eq!(cx.opened_url().as_deref(), Some("http://127.0.0.1:8000"));
    });
}

#[test]
fn choosing_the_built_in_browser_when_asked_keeps_it_and_opens_there() {
    run(async |cx| {
        let trek = open(cx);
        let term = dock_terminal(&trek, cx);
        print(&trek, cx, &term, VITE);
        let (at, _) = link(&term, cx, 1);
        click(&trek, cx, at, Modifiers::secondary_key(), 1);
        trek.click(cx, "terminal-links-builtin");
        trek.render(cx);
        assert_eq!(open_links(&trek, cx), OpenLinks::BuiltIn);
        assert!(browser_open(&trek, cx));
        assert_eq!(cx.opened_url(), None);
    });
}

#[test]
fn dragging_selects_command_c_copies_and_a_click_lets_go() {
    run(async |cx| {
        let trek = open(cx);
        let term = dock_terminal(&trek, cx);
        // Rows 0 and 1 are the test shell's ("$", "(no shell in tests)").
        print(&trek, cx, &term, b"alpha beta\r\ngamma delta\r\n");
        let sent = term.update(cx, |t, _| t.capture_input());
        // From the boundary before "beta" to the one after "gamma".
        let (from, to) = term.read_with(cx, |t, _| (t.cell_start(2, 6), t.cell_start(3, 5)));
        trek.window(cx, |window, cx| {
            use gpui_kit::test::TestWindowExt as _;
            window.drag(from, to, cx);
        });
        trek.render(cx);
        assert_eq!(
            term.read_with(cx, |t, _| t.selected()).as_deref(),
            Some("beta\ngamma")
        );

        let copy = if cfg!(windows) {
            "ctrl-shift-c"
        } else {
            "cmd-c"
        };
        trek.press(cx, copy);
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("beta\ngamma")
        );
        assert!(
            sent.lock().unwrap().is_empty(),
            "⌘C with a selection copies and sends the shell nothing"
        );

        // A click without a drag lets it go; ⌘C then does nothing here.
        click(&trek, cx, from, Modifiers::default(), 1);
        assert_eq!(term.read_with(cx, |t, _| t.selected()), None);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("before".into()));
        trek.press(cx, copy);
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("before")
        );
        assert!(sent.lock().unwrap().is_empty());

        // A double click picks a word, a triple click the row.
        let word = term.read_with(cx, |t, _| t.cell_center(3, 8));
        click(&trek, cx, word, Modifiers::default(), 1);
        dispatch(
            &trek,
            cx,
            gpui_kit::PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: word,
                modifiers: Modifiers::default(),
                click_count: 2,
                first_mouse: false,
            }),
        );
        assert_eq!(
            term.read_with(cx, |t, _| t.selected()).as_deref(),
            Some("delta")
        );
        dispatch(
            &trek,
            cx,
            gpui_kit::PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: word,
                modifiers: Modifiers::default(),
                click_count: 3,
                first_mouse: false,
            }),
        );
        assert_eq!(
            term.read_with(cx, |t, _| t.selected()).as_deref(),
            Some("gamma delta")
        );
    });
}

#[test]
fn new_output_keeps_a_selection_on_its_lines_or_lets_it_go() {
    run(async |cx| {
        let trek = open(cx);
        let term = dock_terminal(&trek, cx);
        let output: String = (1..=100).map(|i| format!("line {i}\r\n")).collect();
        print(&trek, cx, &term, output.as_bytes());
        let triple = |trek: &Trek, cx: &mut TestAppContext, row: u16| {
            let at = term.read_with(cx, |t, _| t.cell_center(row, 1));
            for n in 1..=3 {
                dispatch(
                    trek,
                    cx,
                    gpui_kit::PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position: at,
                        modifiers: Modifiers::default(),
                        click_count: n,
                        first_mouse: false,
                    }),
                );
                dispatch(
                    trek,
                    cx,
                    gpui_kit::PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position: at,
                        modifiers: Modifiers::default(),
                        click_count: n,
                    }),
                );
            }
        };

        // Scrolled back: the view stays on its lines as output comes in, and so does the selection.
        trek.window(cx, |window, cx| {
            use gpui_kit::test::TestWindowExt as _;
            window.scroll(
                "terminal",
                ScrollDelta::Pixels(point(px(0.), px(10. * 18.))),
                cx,
            );
        });
        trek.render(cx);
        triple(&trek, cx, 2);
        let picked = term
            .read_with(cx, |t, _| t.selected())
            .expect("a row picked");
        let rows = term.read_with(cx, |t, _| t.visible_rows());
        assert_eq!(rows[2], picked);
        print(&trek, cx, &term, b"line 101\r\nline 102\r\n");
        assert_eq!(term.read_with(cx, |t, _| t.visible_rows()), rows);
        assert_eq!(
            term.read_with(cx, |t, _| t.selected()),
            Some(picked.clone()),
            "still the same line"
        );

        // The wheel moves the view, and the selection with its line.
        trek.window(cx, |window, cx| {
            use gpui_kit::test::TestWindowExt as _;
            window.scroll(
                "terminal",
                ScrollDelta::Pixels(point(px(0.), px(-3. * 18.))),
                cx,
            );
        });
        trek.render(cx);
        assert_eq!(term.read_with(cx, |t, _| t.selected()), Some(picked));

        // At the bottom, output moves the lines under it: it goes rather than cover other text.
        trek.window(cx, |window, cx| {
            use gpui_kit::test::TestWindowExt as _;
            window.scroll(
                "terminal",
                ScrollDelta::Pixels(point(px(0.), px(-100. * 18.))),
                cx,
            );
        });
        trek.render(cx);
        assert_eq!(term.read_with(cx, |t, _| t.scrolled_back()), 0);
        triple(&trek, cx, 3);
        assert!(term.read_with(cx, |t, _| t.selected()).is_some());
        print(&trek, cx, &term, b"line 103\r\n");
        assert_eq!(term.read_with(cx, |t, _| t.selected()), None);
    });
}

#[test]
fn the_wheel_goes_to_full_screen_programs_and_keys_send_what_shells_expect() {
    run(async |cx| {
        let trek = open(cx);
        let term = dock_terminal(&trek, cx);
        let sent = term.update(cx, |t, _| t.capture_input());
        let wheel = |trek: &Trek, cx: &mut TestAppContext, lines: f32| {
            trek.window(cx, |window, cx| {
                use gpui_kit::test::TestWindowExt as _;
                window.scroll(
                    "terminal",
                    ScrollDelta::Pixels(point(px(0.), px(lines * 18.))),
                    cx,
                );
            });
        };
        let take = |sent: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>| {
            std::mem::take(&mut *sent.lock().unwrap())
        };

        // less, man: the alternate screen and no mouse asked for: arrow keys.
        print(&trek, cx, &term, b"\x1b[?1049h");
        wheel(&trek, cx, 3.);
        assert_eq!(take(&sent), b"\x1b[A\x1b[A\x1b[A");
        print(&trek, cx, &term, b"\x1b[?1h");
        wheel(&trek, cx, -1.);
        assert_eq!(
            take(&sent),
            b"\x1bOB",
            "application cursor keys when the program asked for them"
        );
        // vim with `mouse=a`, htop: the wheel as the mouse report it asked for.
        print(&trek, cx, &term, b"\x1b[?1000h\x1b[?1006h");
        wheel(&trek, cx, -1.);
        let report = take(&sent);
        assert!(
            report.starts_with(b"\x1b[<65;") && report.ends_with(b"M"),
            "{:?}",
            String::from_utf8_lossy(&report)
        );
        // Back at the prompt the wheel scrolls the view and sends nothing.
        print(
            &trek,
            cx,
            &term,
            b"\x1b[?1006l\x1b[?1000l\x1b[?1l\x1b[?1049l",
        );
        wheel(&trek, cx, 1.);
        assert!(take(&sent).is_empty());

        // Shift+Enter: a new line in the prompt (Meta+Return), not a run.
        trek.press(cx, "shift-enter");
        assert_eq!(take(&sent), b"\x1b\r");
        trek.press(cx, "enter");
        assert_eq!(take(&sent), b"\r");
        // ⌥←/→ a word, ⌘←/→ the line's ends and ⌘⌫ the line, as Terminal and iTerm send them.
        trek.press(cx, "alt-left");
        assert_eq!(take(&sent), b"\x1bb");
        if !cfg!(windows) {
            trek.press(cx, "cmd-left");
            assert_eq!(take(&sent), b"\x01");
            trek.press(cx, "cmd-right");
            assert_eq!(take(&sent), b"\x05");
            trek.press(cx, "cmd-backspace");
            assert_eq!(take(&sent), b"\x15");
        }
    });
}

#[test]
fn the_bell_flashes_the_title_names_the_shell_and_the_text_sizes() {
    run(async |cx| {
        let trek = open(cx);
        let term = dock_terminal(&trek, cx);
        assert!(!trek.visible(cx, "terminal-bell") && !trek.visible(cx, "terminal-dock-title"));
        print(&trek, cx, &term, b"\x07");
        assert!(trek.visible(cx, "terminal-bell"));
        // A title set with OSC 0 (its BEL ends it, and isn't a bell).
        print(&trek, cx, &term, b"\x1b]0;npm run dev\x07");
        assert_eq!(
            term.read_with(cx, |t, _| t.title().map(str::to_string))
                .as_deref(),
            Some("npm run dev")
        );
        assert!(trek.visible(cx, "terminal-dock-title"));

        let rows = term.read_with(cx, |t, _| t.visible_rows().len());
        let (bigger, actual) = if cfg!(windows) {
            ("ctrl-=", "ctrl-0")
        } else {
            ("cmd-=", "cmd-0")
        };
        trek.press(cx, bigger);
        trek.press(cx, bigger);
        trek.render(cx);
        assert_eq!(term.read_with(cx, |t, _| t.font_size()), 14.5);
        assert!(
            term.read_with(cx, |t, _| t.visible_rows().len()) < rows,
            "bigger text, fewer rows"
        );
        trek.press(cx, actual);
        trek.render(cx);
        assert_eq!(term.read_with(cx, |t, _| t.font_size()), 12.5);
        assert_eq!(term.read_with(cx, |t, _| t.visible_rows().len()), rows);
    });
}

#[test]
fn scrolled_back_a_pill_says_so_and_takes_the_view_down() {
    run(async |cx| {
        let trek = open(cx);
        let term = dock_terminal(&trek, cx);
        let output: String = (1..=100).map(|i| format!("line {i}\r\n")).collect();
        print(&trek, cx, &term, output.as_bytes());
        assert!(!trek.visible(cx, "terminal-to-bottom"));
        trek.window(cx, |window, cx| {
            use gpui_kit::test::TestWindowExt as _;
            window.scroll(
                "terminal",
                ScrollDelta::Pixels(point(px(0.), px(5. * 18.))),
                cx,
            );
        });
        trek.render(cx);
        assert!(trek.visible(cx, "terminal-to-bottom"));
        trek.click(cx, "terminal-to-bottom");
        trek.render(cx);
        assert_eq!(term.read_with(cx, |t, _| t.scrolled_back()), 0);
        assert!(!trek.visible(cx, "terminal-to-bottom"));
        assert_eq!(
            term.read_with(cx, |t, _| t.selected()),
            None,
            "the pill's click is its own"
        );
    });
}
