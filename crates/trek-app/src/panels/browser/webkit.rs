//! What WebKit needs from a browser that wry leaves out (macOS). Each view's UI and navigation
//! delegates are wrapped in Trek's own, which answer what they know and pass every other call to
//! wry's (`forwardingTargetForSelector:`):
//!
//! - `window.open` and `target=_blank` get a real view, made from the configuration WebKit hands
//!   over (same website data, and the page that opened it as `window.opener`), so a sign-in popup
//!   can post its answer back. wry's own answer is a separate window, or none.
//! - `window.close()` from a page its opener opened closes its tab.
//! - `alert`, `confirm`, `prompt`, camera, microphone and location go to the bar above the page
//!   (`asks.rs`); WebKit waits on the handler kept here until the user answers (`answer`).
//! - A link only another app opens (`mailto:`, `zoommtg:`…) is asked about rather than failing;
//!   ⌘-click and middle-click open a link in a new tab.
//! - A load that fails says why (`Msg::LoadFailed`), and a page whose process ends says so.
//!
//! WebKit reads which methods a delegate has when it's set, so the wrappers are set after wry's,
//! as each view is made (`hook`).

use super::Msg;
use super::asks::{AskKind, LinkKind, Reply, link_kind, scheme_of};
use block2::{Block, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{
    DefinedClass as _, MainThreadMarker, MainThreadOnly, Message as _, define_class, msg_send,
};
use objc2_app_kit::NSEventModifierFlags;
use objc2_foundation::{NSError, NSString, NSURL};
use objc2_web_kit::{
    WKFrameInfo, WKNavigation, WKNavigationAction, WKNavigationDelegate, WKSecurityOrigin,
    WKUIDelegate, WKUserContentController, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use wry::WebViewExtMacOS as _;

/// Makes the view a page opens (`window.open`): its URL, WebKit's configuration for it, and the
/// size the page asked for. `None` refuses it (the page's `window.open` returns `null`).
pub type MakePopup = Box<
    dyn Fn(
        String,
        Retained<WKWebViewConfiguration>,
        Option<(f64, f64)>,
    ) -> Option<Retained<WKWebView>>,
>;

/// WebKit's `WKPermissionDecision` and `WKNavigationActionPolicy` (both `NSInteger`).
const GRANT: isize = 1;
const DENY: isize = 2;
const POLICY_CANCEL: isize = 0;
const POLICY_ALLOW: isize = 1;

/// A WebKit handler waiting for the user's answer, by the ask's id.
enum Waiting {
    Alert(RcBlock<dyn Fn()>),
    Confirm(RcBlock<dyn Fn(Bool)>),
    Prompt(RcBlock<dyn Fn(*mut NSString)>),
    Permission(RcBlock<dyn Fn(isize)>),
}

thread_local! {
    static WAITING: RefCell<HashMap<u64, (u64, Waiting)>> = RefCell::default();
    /// Each tab's wrappers: WebKit holds a delegate weakly, so they're kept here while the tab is.
    static HOOKS: RefCell<HashMap<u64, (Retained<TrekUiDelegate>, Retained<TrekNavigationDelegate>)>> =
        RefCell::default();
}

static NEXT_ASK: AtomicU64 = AtomicU64::new(1);

/// Answer ask `id` (a no-op once answered: WebKit's handler is called exactly once).
pub fn answer(id: u64, reply: &Reply) {
    let Some((_, waiting)) = WAITING.with(|w| w.borrow_mut().remove(&id)) else {
        return;
    };
    let yes = !matches!(reply, Reply::No);
    match waiting {
        Waiting::Alert(done) => done.call(()),
        Waiting::Confirm(done) => done.call((Bool::new(yes),)),
        Waiting::Prompt(done) => match reply {
            Reply::No => done.call((std::ptr::null_mut(),)),
            Reply::Yes => {
                let text = NSString::new();
                done.call((Retained::as_ptr(&text).cast_mut(),));
            }
            Reply::Text(t) => {
                let text = NSString::from_str(t);
                done.call((Retained::as_ptr(&text).cast_mut(),));
            }
        },
        Waiting::Permission(done) => done.call((if yes { GRANT } else { DENY },)),
    }
}

/// Answer everything tab `tab` still waits on with "no" (its tab closed, or its page is going).
pub fn answer_all(tab: u64) {
    let ids: Vec<u64> = WAITING.with(|w| {
        w.borrow()
            .iter()
            .filter(|(_, (t, _))| *t == tab)
            .map(|(id, _)| *id)
            .collect()
    });
    for id in ids {
        answer(id, &Reply::No);
    }
}

/// Keep `waiting` for the user and tell the panel; if the panel's gone, the answer is "no" now.
fn ask(tab: u64, tx: &async_channel::Sender<Msg>, site: String, kind: AskKind, waiting: Waiting) {
    let id = NEXT_ASK.fetch_add(1, Ordering::Relaxed);
    WAITING.with(|w| w.borrow_mut().insert(id, (tab, waiting)));
    if tx
        .try_send(Msg::Ask {
            tab,
            id,
            site,
            kind,
        })
        .is_err()
    {
        answer(id, &Reply::No);
    }
}

/// The site a frame's page is on, as the bar names it: its host (and port, when it has one of
/// its own), or its scheme (`file`).
fn site_of(origin: &WKSecurityOrigin) -> String {
    let host = unsafe { origin.host() }.to_string();
    let port = unsafe { origin.port() };
    match (host.is_empty(), port) {
        (true, _) => unsafe { origin.protocol() }.to_string(),
        (false, 0) => host,
        (false, port) => format!("{host}:{port}"),
    }
}

/// The address a navigation action asks for ("" when WebKit gives none: `window.open()`).
fn action_url(action: &WKNavigationAction) -> String {
    unsafe { action.request().URL() }
        .and_then(|u| u.absoluteString())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// Whether WebKit shows `url` itself: the schemes `link_kind` names, any WebKit handles, and
/// Trek's own pages.
fn view_loads(url: &str, mtm: MainThreadMarker) -> bool {
    link_kind(url) == LinkKind::View
        || scheme_of(url).is_some_and(|s| {
            !s.eq_ignore_ascii_case("trek")
                && unsafe { WKWebView::handlesURLScheme(&NSString::from_str(s), mtm) }
        })
}

pub struct Ivars {
    tab: u64,
    tx: async_channel::Sender<Msg>,
    /// wry's delegate, which gets every call this one doesn't answer.
    inner: Retained<AnyObject>,
    popup: Option<MakePopup>,
}

impl Ivars {
    /// `respondsToSelector:` for the pair: this class's own methods, then wry's.
    fn inner_responds(&self, sel: Sel) -> bool {
        unsafe { msg_send![&*self.inner, respondsToSelector: sel] }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars are dropped with the object.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TrekBrowserUIDelegate"]
    #[ivars = Ivars]
    pub struct TrekUiDelegate;

    impl TrekUiDelegate {
        #[unsafe(method(respondsToSelector:))]
        fn responds_to(&self, sel: Sel) -> bool {
            let own: bool = unsafe { msg_send![super(self), respondsToSelector: sel] };
            own || self.ivars().inner_responds(sel)
        }

        #[unsafe(method(forwardingTargetForSelector:))]
        fn forwarding_target(&self, _sel: Sel) -> *mut AnyObject {
            Retained::as_ptr(&self.ivars().inner).cast_mut()
        }

        /// Location (WebKit's own SPI: the public delegate has no such method on macOS).
        #[unsafe(method(_webView:requestGeolocationPermissionForOrigin:initiatedByFrame:decisionHandler:))]
        fn location(
            &self,
            _webview: &WKWebView,
            origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            decide: &Block<dyn Fn(isize)>,
        ) {
            let v = self.ivars();
            tracing::info!("browser: {} asks for the location", site_of(origin));
            ask(v.tab, &v.tx, site_of(origin), AskKind::Location, Waiting::Permission(decide.copy()));
        }
    }

    unsafe impl NSObjectProtocol for TrekUiDelegate {}

    unsafe impl WKUIDelegate for TrekUiDelegate {
        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_web_view(
            &self,
            _webview: &WKWebView,
            configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            self.popup(configuration, action, features)
        }

        #[unsafe(method(webViewDidClose:))]
        fn did_close(&self, _webview: &WKWebView) {
            let _ = self.ivars().tx.try_send(Msg::Closed(self.ivars().tab));
        }

        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn alert(
            &self,
            _webview: &WKWebView,
            message: &NSString,
            frame: &WKFrameInfo,
            done: &Block<dyn Fn()>,
        ) {
            let v = self.ivars();
            let site = site_of(&*unsafe { frame.securityOrigin() });
            ask(v.tab, &v.tx, site, AskKind::Alert(message.to_string()), Waiting::Alert(done.copy()));
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn confirm(
            &self,
            _webview: &WKWebView,
            message: &NSString,
            frame: &WKFrameInfo,
            done: &Block<dyn Fn(Bool)>,
        ) {
            let v = self.ivars();
            let site = site_of(&*unsafe { frame.securityOrigin() });
            ask(v.tab, &v.tx, site, AskKind::Confirm(message.to_string()), Waiting::Confirm(done.copy()));
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        fn prompt(
            &self,
            _webview: &WKWebView,
            message: &NSString,
            default: Option<&NSString>,
            frame: &WKFrameInfo,
            done: &Block<dyn Fn(*mut NSString)>,
        ) {
            let v = self.ivars();
            let site = site_of(&*unsafe { frame.securityOrigin() });
            let kind = AskKind::Prompt {
                message: message.to_string(),
                default: default.map(|d| d.to_string()).unwrap_or_default(),
            };
            ask(v.tab, &v.tx, site, kind, Waiting::Prompt(done.copy()));
        }

        /// Camera and microphone: Trek's bar asks (wry grants every request without a word).
        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        fn media(
            &self,
            _webview: &WKWebView,
            origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            kind: isize,
            decide: &Block<dyn Fn(isize)>,
        ) {
            let v = self.ivars();
            tracing::info!("browser: {} asks for capture device {kind}", site_of(origin));
            let kind = match kind {
                0 => AskKind::Camera,
                1 => AskKind::Microphone,
                _ => AskKind::CameraAndMicrophone,
            };
            ask(v.tab, &v.tx, site_of(origin), kind, Waiting::Permission(decide.copy()));
        }
    }
);

impl TrekUiDelegate {
    /// The view for a page's `window.open` (`createWebView…`): one `MakePopup` makes, or none
    /// for an address another app opens (the bar asks about that instead).
    fn popup(
        &self,
        configuration: &WKWebViewConfiguration,
        action: &WKNavigationAction,
        features: &WKWindowFeatures,
    ) -> Option<Retained<WKWebView>> {
        let v = self.ivars();
        let url = action_url(action);
        if !url.is_empty() && !view_loads(&url, self.mtm()) {
            let _ = v.tx.try_send(Msg::External { tab: v.tab, url });
            return None;
        }
        let size = unsafe { features.width().zip(features.height()) }
            .map(|(w, h)| (w.doubleValue(), h.doubleValue()));
        let url = if url.is_empty() {
            "about:blank".into()
        } else {
            url
        };
        // WebKit checks the view was made from this very configuration (its opener is in it).
        (v.popup.as_ref()?)(url, configuration.retain(), size)
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars are dropped with the object.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TrekBrowserNavigationDelegate"]
    #[ivars = Ivars]
    pub struct TrekNavigationDelegate;

    impl TrekNavigationDelegate {
        #[unsafe(method(respondsToSelector:))]
        fn responds_to(&self, sel: Sel) -> bool {
            let own: bool = unsafe { msg_send![super(self), respondsToSelector: sel] };
            own || self.ivars().inner_responds(sel)
        }

        #[unsafe(method(forwardingTargetForSelector:))]
        fn forwarding_target(&self, _sel: Sel) -> *mut AnyObject {
            Retained::as_ptr(&self.ivars().inner).cast_mut()
        }
    }

    unsafe impl NSObjectProtocol for TrekNavigationDelegate {}

    unsafe impl WKNavigationDelegate for TrekNavigationDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_action(
            &self,
            webview: &WKWebView,
            action: &WKNavigationAction,
            decide: &Block<dyn Fn(isize)>,
        ) {
            let v = self.ivars();
            let url = action_url(action);
            // wry reads every action's URL as there; one without (WebKit's own) is let through.
            if url.is_empty() {
                decide.call((POLICY_ALLOW,));
                return;
            }
            let main = unsafe { action.targetFrame() }.is_none_or(|f| unsafe { f.isMainFrame() });
            if !view_loads(&url, self.mtm()) {
                decide.call((POLICY_CANCEL,));
                // A frame inside the page opening an app is how pages spring apps on people:
                // only the page itself may ask.
                if main {
                    let _ = v.tx.try_send(Msg::External { tab: v.tab, url });
                }
                return;
            }
            // ⌘-click and middle-click open a link beside the page, as in Safari (no opener).
            let link = unsafe { action.navigationType() } == objc2_web_kit::WKNavigationType::LinkActivated;
            let command = unsafe { action.modifierFlags() }.contains(NSEventModifierFlags::Command);
            let middle = unsafe { action.buttonNumber() } == 2;
            if main && link && (command || middle) && url.starts_with("http") {
                decide.call((POLICY_CANCEL,));
                let _ = v.tx.try_send(Msg::NewWindow(url));
                return;
            }
            unsafe {
                let _: () = msg_send![
                    &*v.inner,
                    webView: webview,
                    decidePolicyForNavigationAction: action,
                    decisionHandler: decide
                ];
            }
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn failed(&self, webview: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            let v = self.ivars();
            let url = failing_url(error).or_else(|| {
                unsafe { webview.URL() }
                    .and_then(|u| u.absoluteString())
                    .map(|s| s.to_string())
            });
            let _ = v.tx.try_send(Msg::LoadFailed {
                tab: v.tab,
                url: url.unwrap_or_default(),
                domain: error.domain().to_string(),
                code: error.code() as i64,
            });
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn crashed(&self, _webview: &WKWebView) {
            let _ = self.ivars().tx.try_send(Msg::Crashed(self.ivars().tab));
        }
    }
);

/// The address a load failed on, from its error (`NSURLErrorFailingURLErrorKey`, an NSURL; or
/// the older string key).
fn failing_url(error: &NSError) -> Option<String> {
    let info = error.userInfo();
    let value = |key: &str| -> Option<Retained<AnyObject>> {
        let key = NSString::from_str(key);
        unsafe { msg_send![&*info, objectForKey: &*key] }
    };
    if let Some(url) = value("NSErrorFailingURLKey").and_then(|v| v.downcast::<NSURL>().ok()) {
        return url.absoluteString().map(|s| s.to_string());
    }
    value("NSErrorFailingURLStringKey")
        .and_then(|v| v.downcast::<NSString>().ok())
        .map(|s| s.to_string())
}

/// Wrap the delegates wry gave view `wv` (tab `tab`) in Trek's. `popup` makes the views its pages
/// open; `None` refuses them.
pub fn hook(wv: &wry::WebView, tab: u64, tx: async_channel::Sender<Msg>, popup: Option<MakePopup>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let view = wv.webview();
    let (Some(ui), Some(nav)) = (unsafe { view.UIDelegate() }, unsafe {
        view.navigationDelegate()
    }) else {
        return;
    };
    let ivars = |inner: Retained<AnyObject>, popup| Ivars {
        tab,
        tx: tx.clone(),
        inner,
        popup,
    };
    let ui: Retained<TrekUiDelegate> = {
        let this = mtm
            .alloc::<TrekUiDelegate>()
            .set_ivars(ivars(into_object(ui), popup));
        unsafe { msg_send![super(this), init] }
    };
    let nav: Retained<TrekNavigationDelegate> = {
        let this = mtm
            .alloc::<TrekNavigationDelegate>()
            .set_ivars(ivars(into_object(nav), None));
        unsafe { msg_send![super(this), init] }
    };
    unsafe {
        view.setUIDelegate(Some(ProtocolObject::from_ref(&*ui)));
        view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*nav)));
    }
    HOOKS.with(|h| h.borrow_mut().insert(tab, (ui, nav)));
}

fn into_object<T: objc2::Message>(p: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: a protocol object is an Objective-C object.
    unsafe { Retained::cast_unchecked(p) }
}

/// Tab `tab` is gone: let go of its wrappers, and answer what it waited on.
pub fn unhook(tab: u64) {
    answer_all(tab);
    HOOKS.with(|h| h.borrow_mut().remove(&tab));
}

/// Give a popup's configuration (WebKit's copy of its opener's) a content controller of its own.
/// The copy shares the opener's, and wry fills a view's controller with its scripts and its
/// `ipc` handler: shared, the opener's pages would run every script twice, its messages would
/// come in as the popup's, and closing the popup would take the opener's handler with it. The
/// website data (cookies, storage) stays shared: that's the configuration's data store.
pub fn own_scripts(config: &WKWebViewConfiguration) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    unsafe { config.setUserContentController(&WKUserContentController::new(mtm)) };
}

/// A view's WKWebView, for WebKit's `createWebView…` answer.
pub fn web_view(wv: &wry::WebView) -> Retained<WKWebView> {
    Retained::into_super(wv.webview())
}

/// The app that opens `url` on this Mac, by its name.
pub fn app_for(url: &str) -> Option<String> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSFileManager, NSURL};
    let url = NSURL::URLWithString(&NSString::from_str(url))?;
    let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url)?;
    let path = app.path()?;
    let name = NSFileManager::defaultManager()
        .displayNameAtPath(&path)
        .to_string();
    Some(name.strip_suffix(".app").unwrap_or(&name).to_string())
}
