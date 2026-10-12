//! The Browser tool in the headless harness. GPUI's test window has no native window for a web
//! view to live in, so the tool shows what a system without one shows (on Windows, a missing
//! WebView2 runtime), and every path that hides or shows the native view must cope with none.

use super::harness::{open, run};
use crate::workspace::PanelTool;

#[test]
fn without_a_web_view_the_browser_says_so_and_offers_webview2_on_windows() {
    run(async |cx| {
        let trek = open(cx);
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.window(cx, |window, cx| panel.update(cx, |p, cx| p.open_tool(PanelTool::Browser, window, cx)));
        trek.render(cx);
        let browser = cx.read(|cx| panel.read(cx).browser()).expect("the Browser tool");
        assert!(cx.read(|cx| browser.read(cx).unavailable()));
        assert!(!trek.visible(cx, "browser-url"), "no toolbar for a browser that isn't there");
        assert_eq!(trek.visible(cx, "browser-get-webview2"), cfg!(windows), "the download link is Windows' alone");

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
