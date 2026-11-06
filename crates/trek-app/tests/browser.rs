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
