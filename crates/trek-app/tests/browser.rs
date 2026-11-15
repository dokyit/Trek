//! The Browser tool in the headless harness. GPUI's test window has no native window for a web
//! view to live in, so the tool shows what a system without one shows (on Windows, a missing
//! WebView2 runtime), and every path that hides or shows the native view must cope with none.

mod common;

use common::{open, run};
use trek_app::workspace::PanelTool;

#[test]
fn without_a_web_view_the_browser_says_so_and_offers_webview2_on_windows() {
    run(async |cx| {
        let trek = open(cx);
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.window(cx, |window, cx| {
            panel.update(cx, |p, cx| p.open_tool(PanelTool::Browser, window, cx))
        });
        trek.render(cx);
        let browser = cx
            .read(|cx| panel.read(cx).browser())
            .expect("the Browser tool");
        assert!(cx.read(|cx| browser.read(cx).unavailable()));
        assert!(
            !trek.visible(cx, "browser-url"),
            "no toolbar for a browser that isn't there"
        );
        assert_eq!(
            trek.visible(cx, "browser-get-webview2"),
            cfg!(windows),
            "the download link is Windows' alone"
        );

        // The palette over it and gone, the panel shut and opened again: nothing native to hide.
        trek.press(cx, "secondary-k");
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.overlay_open));
        trek.press(cx, "escape");
        trek.render(cx);
        cx.update(|cx| panel.update(cx, |p, cx| p.toggle(cx)));
        trek.render(cx);
        assert!(!cx.read(|cx| panel.read(cx).open));
        cx.update(|cx| panel.update(cx, |p, cx| p.toggle(cx)));
        trek.render(cx);
        assert_eq!(trek.visible(cx, "browser-get-webview2"), cfg!(windows));
    });
}

#[test]
fn a_draft_with_a_command_or_a_mention_leaves_the_page_on_show() {
    run(async |cx| {
        let trek = open(cx);
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.window(cx, |window, cx| {
            panel.update(cx, |p, cx| p.open_tool(PanelTool::Browser, window, cx))
        });
        trek.render(cx);
        let covered = |trek: &common::Trek, cx: &mut gpui_kit::TestAppContext| {
            trek.read(cx, |ws, _| ws.overlay_open)
        };
        // The list for `/`, `@` or a path sits over the composer, not the tools panel: the page
        // stays, while it's up and after the composer loses the keys with the word still there.
        for word in ["/", " @", " src/main.rs"] {
            trek.type_text(cx, word);
            trek.render(cx);
            assert!(!covered(&trek, cx), "{word:?} hid the page");
        }
        trek.window(cx, |window, cx| window.blur(cx));
        trek.render(cx);
        assert!(!covered(&trek, cx), "a draft left the page hidden");
        // A menu that opens over the panel still hides it, and gives it back.
        trek.click(cx, "model-pill");
        trek.render(cx);
        assert!(
            covered(&trek, cx),
            "the model menu would sit under the page"
        );
        trek.press(cx, "escape");
        trek.render(cx);
        assert!(!covered(&trek, cx));
    });
}

/// A download that lands says where, with a button to show the file; the same file downloaded
/// twice is said twice, each by its own name; one that fails says so.
#[test]
fn a_finished_download_offers_to_show_the_file() {
    use trek_app::workspace::UndoAction;
    run(async |cx| {
        let trek = open(cx);
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.window(cx, |window, cx| {
            panel.update(cx, |p, cx| p.open_tool(PanelTool::Browser, window, cx))
        });
        trek.render(cx);
        let browser = cx
            .read(|cx| panel.read(cx).browser())
            .expect("the Browser tool");
        let seen = common::toasts(&trek, cx);
        let downloads = trek.project.join("Downloads");
        let file = downloads.join("report.pdf");
        let again = downloads.join("report (1).pdf");
        for (path, ok) in [(&file, true), (&again, true), (&file, false)] {
            trek.window(cx, |window, cx| {
                browser.update(cx, |b, cx| {
                    let url = "https://example.com/report.pdf";
                    b.download_for_test(url, path.clone(), ok, window, cx)
                })
            });
        }
        let seen = seen.borrow();
        let done: Vec<&(String, Option<UndoAction>)> = seen
            .iter()
            .filter(|(m, _)| !m.starts_with("Downloading"))
            .collect();
        assert_eq!(done[0].0, "Downloaded report.pdf to Downloads");
        assert!(matches!(&done[0].1, Some(UndoAction::RevealFile(p)) if *p == file));
        assert_eq!(done[1].0, "Downloaded report (1).pdf to Downloads");
        assert_eq!(
            done[2].0,
            "Couldn’t download report.pdf. Try the link again."
        );
        assert!(done[2].1.is_none());
        assert_eq!(
            trek_app::root::toast_button_label(done[0].1.as_ref().unwrap()),
            trek_app::words::words().show_in_file_manager
        );
    });
}
