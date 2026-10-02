//! Browser: an embedded WebKit view for previewing local servers and docs.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use gpui_wry::WebView;

pub struct BrowserPanel {
    webview: Option<Entity<WebView>>,
    address: Entity<InputState>,
    visible: bool,
    _subscription: Subscription,
}

/// "3000" → http://localhost:3000, "example.com" → https://example.com.
fn normalize(input: &str) -> String {
    let t = input.trim();
    if t.chars().all(|c| c.is_ascii_digit()) && !t.is_empty() {
        return format!("http://localhost:{t}");
    }
    if t.starts_with("localhost") || t.starts_with("127.0.0.1") {
        return format!("http://{t}");
    }
    if t.contains("://") || t.starts_with("about:") {
        return t.to_string();
    }
    format!("https://{t}")
}

impl BrowserPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Enter a URL or a localhost port (3000)"));
        let built = {
            use raw_window_handle::HasWindowHandle;
            window.window_handle().ok().and_then(|handle| wry::WebViewBuilder::new().with_url("about:blank").build_as_child(&handle).ok())
        };
        let webview = built.map(|wv| cx.new(|cx| WebView::new(wv, window, cx)));
        let sub = cx.subscribe(&address, |this: &mut Self, input, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let url = normalize(&input.read(cx).value());
                if let Some(wv) = &this.webview {
                    wv.update(cx, |v, _| v.load_url(&url));
                }
            }
        });
        Self { webview, address, visible: true, _subscription: sub }
    }

    /// The native view floats above GPUI; hide it whenever its tab isn't on screen.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if let Some(wv) = &self.webview {
            wv.update(cx, |v, _| if visible { v.show() } else { v.hide() });
        }
    }
}

impl Render for BrowserPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let wv = self.webview.clone();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_2()
                    .h(px(40.))
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(crate::ui::icon_button("browser-back", IconName::ArrowLeft, "Back").on_click({
                        let wv = wv.clone();
                        move |_, _, cx| {
                            if let Some(wv) = &wv {
                                wv.update(cx, |v, _| {
                                    let _ = v.back();
                                });
                            }
                        }
                    }))
                    .child(crate::ui::icon_button("browser-reload", IconName::RefreshCw, "Reload").on_click({
                        let wv = wv.clone();
                        let address = self.address.clone();
                        move |_, _, cx| {
                            let url = normalize(&address.read(cx).value());
                            if let Some(wv) = &wv {
                                wv.update(cx, |v, _| v.load_url(&url));
                            }
                        }
                    }))
                    .child(div().flex_1().child(Input::new(&self.address).small().prefix(Icon::new(IconName::Globe).xsmall().text_color(theme.muted_foreground)))),
            )
            .child(match wv {
                Some(wv) => div().flex_1().min_h_0().child(wv).into_any_element(),
                None => super::empty("The embedded browser isn't available on this system.", cx).into_any_element(),
            })
    }
}
