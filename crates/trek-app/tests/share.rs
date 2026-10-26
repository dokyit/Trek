//! The thread's "⋯" menu in the title bar, its copies (menu, shortcuts), and the Share sheet.

mod common;

use common::{Trek, mock, open, run, store_items, toasts};
use gpui_kit::{AppContext as _, ClipboardItem, TestAppContext};
use trek_app::keys::{self, Id};
use trek_app::share::gist::{self, Gist};
use trek_app::thread_menu::{Copy, copy_text};
use trek_app::workspace::Route;
use trek_core::store::{Item, ToolStatus};
use trek_core::{Effort, HandHolding};

/// A thread in the project with a short conversation (a question, a tool call, an answer), on
/// screen.
fn talked_thread(trek: &Trek, cx: &mut TestAppContext) -> String {
    let main_rs = trek.project.join("src/main.rs").display().to_string();
    trek.update(cx, |ws, cx| {
        let t = ws
            .store
            .create_thread(
                Some(&trek.project),
                mock(),
                None,
                Effort::Medium,
                HandHolding::Auto,
            )
            .expect("thread");
        store_items(
            &ws.store,
            &t.id,
            vec![
                Item::User {
                    text: format!("Fix the bug in {main_rs}"),
                    images: vec![],
                    at: None,
                    resume: None,
                    aside: false,
                },
                Item::Tool {
                    id: "t1".into(),
                    title: "Read main.rs".into(),
                    detail: main_rs.clone(),
                    output: "fn main() {}".into(),
                    status: ToolStatus::Done,
                },
                Item::Assistant {
                    text: "Fixed it.".into(),
                },
            ],
        );
        ws.reload(cx);
        ws.navigate(Route::Thread(t.id.clone()), cx);
        t.id
    })
}

fn clipboard(cx: &mut TestAppContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|c| c.text())
}

#[test]
fn the_thread_menu_sits_before_the_tools_toggle_only_on_a_thread() {
    run(async |cx| {
        let trek = open(cx);
        trek.render(cx);
        assert!(
            !trek.visible(cx, "thread-menu"),
            "a draft has no thread menu"
        );
        talked_thread(&trek, cx);
        trek.render(cx);
        let menu = trek
            .bounds(cx, "thread-menu")
            .expect("⋯ is in the title bar");
        let tools = trek.bounds(cx, "toggle-tools").expect("tools toggle");
        assert!(
            menu.origin.x + menu.size.width < tools.origin.x,
            "⋯ comes before the tools toggle: {menu:?} {tools:?}"
        );
        // In the area the window's drag leaves alone, as every title bar control is.
        assert!(trek.visible(cx, "no-drag-thread-menu"));
    });
}

#[test]
fn the_menu_has_share_copy_places_and_actions() {
    run(async |cx| {
        let trek = open(cx);
        talked_thread(&trek, cx);
        trek.render(cx);
        trek.click(cx, "thread-menu");
        trek.render(cx);
        for id in [
            "thread-share",
            "thread-open-window",
            "thread-to-phone",
            "thread-fork",
            "thread-rename",
            "thread-pin",
            "thread-archive",
        ] {
            assert!(trek.visible(cx, id), "{id} is in the menu");
        }
    });
}

#[test]
fn each_copy_puts_the_right_text_on_the_clipboard() {
    run(async |cx| {
        let trek = open(cx);
        let id = talked_thread(&trek, cx);
        let shown = toasts(&trek, cx);
        let text =
            |what: Copy, cx: &mut TestAppContext| trek.read(cx, |ws, _| copy_text(ws, &id, what));
        assert_eq!(text(Copy::Link, cx), Some(format!("trek://thread/{id}")));
        assert_eq!(text(Copy::Id, cx), Some(id.clone()));
        assert_eq!(text(Copy::LastAnswer, cx).as_deref(), Some("Fixed it."));
        assert_eq!(
            text(Copy::Folder, cx),
            Some(trek.project.display().to_string())
        );
        // The mock agent has no CLI to resume in.
        assert_eq!(text(Copy::Resume, cx), None);
        let md = text(Copy::Markdown, cx).unwrap();
        assert!(md.contains("## You") && md.contains("Fixed it."), "{md}");
        assert!(
            !md.contains("Read main.rs"),
            "Copy as Markdown leaves tool calls out"
        );
        let plain = text(Copy::PlainText, cx).unwrap();
        assert!(
            plain.contains("You:\nFix the bug") && plain.contains(":\nFixed it."),
            "{plain}"
        );

        // The shortcuts copy the thread on screen.
        cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
        trek.press(cx, &keys::keystroke(Id::CopyThreadLink));
        assert_eq!(clipboard(cx), Some(format!("trek://thread/{id}")));
        trek.press(cx, &keys::keystroke(Id::CopyThreadMarkdown));
        assert_eq!(clipboard(cx), Some(md));
        if cfg!(target_os = "macos") {
            trek.press(cx, &keys::keystroke(Id::CopyThreadFolder));
            assert_eq!(clipboard(cx), Some(trek.project.display().to_string()));
        }
        assert!(
            shown
                .borrow()
                .iter()
                .any(|(m, _)| m == "Copied the thread's link"),
            "{:?}",
            shown.borrow()
        );
    });
}

#[test]
fn share_opens_the_sheet_and_its_tabs() {
    run(async |cx| {
        let trek = open(cx);
        talked_thread(&trek, cx);
        trek.render(cx);
        trek.click(cx, "thread-menu");
        trek.render(cx);
        trek.click(cx, "thread-share");
        trek.render(cx);
        assert!(trek.visible(cx, "share-sheet"));
        for id in [
            "share-public",
            "share-tools",
            "share-thinking",
            "share-redact",
        ] {
            assert!(trek.visible(cx, id), "Link has {id}");
        }
        assert!(!trek.visible(cx, "share-url"), "no link made yet");
        trek.click(cx, ("share-tab", 1usize));
        trek.render(cx);
        for i in 0..3usize {
            assert!(
                trek.visible(cx, ("share-format", i)),
                "Export offers format {i}"
            );
        }
        trek.click(cx, ("share-tab", 2usize));
        trek.render(cx);
        assert!(trek.visible(cx, "share-to-phone"));
        assert_eq!(
            trek.visible(cx, "share-to-system"),
            cfg!(target_os = "macos"),
            "More… is the Mac's share menu"
        );
    });
}

#[test]
fn a_remembered_link_shows_again() {
    run(async |cx| {
        let trek = open(cx);
        let id = talked_thread(&trek, cx);
        gist::remember(
            &id,
            Some(Gist {
                id: "feed1234".into(),
                url: "https://gist.github.com/me/feed1234".into(),
                filename: "fix.md".into(),
                at: trek_core::store::now_ms(),
            }),
        );
        trek.render(cx);
        cx.update_window(trek.window, |_, window, cx| {
            trek_app::share::open(trek.ws.clone(), &id, window, cx)
        })
        .unwrap();
        trek.render(cx);
        assert!(trek.visible(cx, "share-url"));
        assert!(trek.visible(cx, "share-url-copy"));
        assert!(trek.visible(cx, "share-delete"));
        trek.click(cx, "share-url-copy");
        assert_eq!(
            clipboard(cx).as_deref(),
            Some("https://gist.github.com/me/feed1234")
        );
        gist::remember(&id, None);
        assert_eq!(gist::remembered(&id), None);
    });
}

#[test]
fn a_thread_window_has_the_menu_too() {
    run(async |cx| {
        let trek = open(cx);
        let id = talked_thread(&trek, cx);
        let own = trek.open_thread_window(cx, &id);
        cx.update_window(own, |_, window, cx| {
            use gpui_kit::test::TestWindowExt as _;
            window.render_frame(cx)
        })
        .unwrap();
        assert!(trek.visible_in(cx, own, "thread-menu"));
    });
}
