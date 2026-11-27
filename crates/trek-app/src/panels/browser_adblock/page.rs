//! Trek's ad blocker in the Browser's pages (macOS). Each tab's view is told, as it starts to go
//! to a page (its main frame's navigation, `browser/webkit.rs`), what that page gets:
//!
//! - **WebKit's content rules** (the lists' network filters, `engine::content_rules`), on the
//!   view unless the site is one the user let through (Allow ads on this site): then they come
//!   off for that page, at once, with nothing to compile.
//! - **The page's own scripts**, set on the view before its document exists, as Brave's iOS
//!   browser sets them: the scriptlets uBlock Origin's lists ask for on that site, in the page's
//!   world at its start (they must run before the page's own scripts), and Trek's cosmetic
//!   script (`cosmetic.js`) with the site's hiding and procedural filters, in a world of its own
//!   that the page can't see, which asks for the lists' generic hiding as classes and ids appear
//!   (a message to Trek, answered from the engine).
//!
//! Everything here is the main thread's.

use super::engine;
use block2::{Block, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSError, NSString};
use objc2_web_kit::{
    WKContentRuleList, WKContentWorld, WKScriptMessage, WKScriptMessageHandlerWithReply,
    WKUserContentController, WKUserScript, WKUserScriptInjectionTime, WKWebView,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// The content world Trek's cosmetic script runs in, and its handler's name there.
const WORLD: &str = "TrekAdblock";
const HANDLER: &str = "trekAdblock";
/// What marks the scripts Trek sets for a page (so the next page's replace them).
const MARK: &str = "/*trek-adblock*/";
const COSMETIC_JS: &str = include_str!("cosmetic.js");

#[derive(Default)]
struct State {
    /// The blocker is on.
    on: bool,
    /// Sites let through (`site_of`).
    allowed: Vec<String>,
    engine: Option<Arc<engine::Pages>>,
    lists: Vec<Retained<WKContentRuleList>>,
    /// Each tab's view, and whether the lists are on it now.
    views: HashMap<u64, (Retained<WKWebView>, bool)>,
    handlers: HashMap<u64, Retained<Handler>>,
    /// What the last pages got, for the shots driver (tab: site, hide, procedural, script bytes).
    last: HashMap<u64, Value>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with_borrow_mut(f)
}

/// Whether the blocker is on and has its engine.
pub fn ready() -> bool {
    with(|s| s.on && s.engine.is_some())
}

/// The engine, for the driver and Settings' numbers.
pub fn engine() -> Option<Arc<engine::Pages>> {
    with(|s| s.engine.clone())
}

/// What the blocker works with now: on or off, the sites let through, the engine and the
/// compiled lists. The views take the lists (or lose them) at once; their pages' scripts follow
/// from their next page.
pub fn set(
    on: bool,
    allowed: Vec<String>,
    engine: Option<Arc<engine::Pages>>,
    lists: Vec<Retained<WKContentRuleList>>,
) {
    let (old, views) = with(|s| {
        s.on = on;
        s.allowed = allowed;
        if engine.is_some() {
            s.engine = engine;
        }
        let old = std::mem::replace(&mut s.lists, lists);
        let views: Vec<(u64, Retained<WKWebView>)> =
            s.views.iter().map(|(k, (v, _))| (*k, v.clone())).collect();
        (old, views)
    });
    for (tab, view) in views {
        let url = unsafe { view.URL() }
            .and_then(|u| u.absoluteString())
            .map(|s| s.to_string())
            .unwrap_or_default();
        // Off the old lists first, then the current ones for its page.
        take_lists(&view, &old);
        with(|s| {
            if let Some(v) = s.views.get_mut(&tab) {
                v.1 = false;
            }
        });
        lists_for(tab, &view, &url);
    }
}

fn take_lists(view: &WKWebView, lists: &[Retained<WKContentRuleList>]) {
    let controller = unsafe { view.configuration().userContentController() };
    for l in lists {
        unsafe { controller.removeContentRuleList(l) };
    }
}

/// Whether a page on `url` is blocked on: the blocker on, and its site not let through.
fn blocks(s: &State, url: &str) -> bool {
    if !s.on {
        return false;
    }
    match super::site_of(url) {
        Some(site) => !s.allowed.iter().any(|a| a == &site),
        // Not a web page (Trek's own, a file): nothing to block or hide.
        None => false,
    }
}

/// Put the lists on tab `tab`'s view, or take them off, for a page on `url`.
fn lists_for(tab: u64, view: &WKWebView, url: &str) {
    let (want, has, lists) = with(|s| {
        (
            blocks(s, url) || (s.on && super::site_of(url).is_none() && !url.is_empty()),
            s.views.get(&tab).is_some_and(|v| v.1),
            s.lists.clone(),
        )
    });
    if want == has {
        return;
    }
    let controller = unsafe { view.configuration().userContentController() };
    for l in &lists {
        unsafe {
            if want {
                controller.addContentRuleList(l);
            } else {
                controller.removeContentRuleList(l);
            }
        }
    }
    with(|s| {
        if let Some(v) = s.views.get_mut(&tab) {
            v.1 = want;
        }
    });
}

/// Give tab `tab`'s view (`wv`) the blocker: its handler, and the lists for its first page.
pub fn install(wv: &wry::WebView, tab: u64) {
    use wry::WebViewExtMacOS as _;
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let view: Retained<WKWebView> = Retained::into_super(wv.webview());
    let handler: Retained<Handler> = {
        let this = mtm.alloc::<Handler>().set_ivars(Ivars { tab });
        unsafe { msg_send![super(this), init] }
    };
    unsafe {
        let controller = view.configuration().userContentController();
        let world = WKContentWorld::worldWithName(&NSString::from_str(WORLD), mtm);
        let name = NSString::from_str(HANDLER);
        controller.removeScriptMessageHandlerForName_contentWorld(&name, &world);
        controller.addScriptMessageHandlerWithReply_contentWorld_name(
            ProtocolObject::from_ref(&*handler),
            &world,
            &name,
        );
    }
    with(|s| {
        s.views.insert(tab, (view, false));
        s.handlers.insert(tab, handler);
    });
}

/// Tab `tab` is gone.
pub fn remove(tab: u64) {
    with(|s| {
        s.views.remove(&tab);
        s.handlers.remove(&tab);
        s.last.remove(&tab);
    });
}

/// Tab `tab`'s view starts to go to `url` (its main frame, before the document exists): the
/// lists on or off for it, and the page's scripts.
pub fn navigating(tab: u64, view: &WKWebView, url: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if !with(|s| s.views.contains_key(&tab)) {
        return;
    }
    lists_for(tab, view, url);
    let (on, engine) = with(|s| (blocks(s, url), s.engine.clone()));
    let started = std::time::Instant::now();
    let page = match (on, engine) {
        (true, Some(pages)) => Some(engine::page_split(&pages, url)),
        _ => None,
    };
    let lookup = started.elapsed();
    let controller = unsafe { view.configuration().userContentController() };
    // The other scripts (wry's, passkeys', the agent's) stay; the last page's of Trek's go. Every
    // one of Trek's starts with `MARK`, scriptlets included: a script without it stayed, so each
    // navigation added another copy of the site's scriptlets (some 90 KB a page) until the pile
    // made each load stall the main thread for seconds and WebKit died sending it to a new web
    // process. Only the start of each source is looked at, not the whole of it copied out.
    let mark = NSString::from_str(MARK);
    let others: Vec<Retained<WKUserScript>> = unsafe { controller.userScripts() }
        .iter()
        .filter(|s| !unsafe { s.source() }.hasPrefix(&mark))
        .collect();
    let had = unsafe { controller.userScripts() }.count() != others.len();
    if !had && page.is_none() {
        return;
    }
    unsafe {
        controller.removeAllUserScripts();
        for s in &others {
            controller.addUserScript(s);
        }
    }
    let Some((page, isolated_scriptlets)) = page else {
        with(|s| s.last.remove(&tab));
        return;
    };
    let script = |source: String, world: &WKContentWorld, main_only: bool| unsafe {
        let source = marked(source);
        WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
            WKUserScript::alloc(mtm),
            &NSString::from_str(&source),
            WKUserScriptInjectionTime::AtDocumentStart,
            main_only,
            world,
        )
    };
    let scriptlets = engine::wrap_script(&page.script);
    let data = json!({
        "hide": page.hide,
        "procedural": page.procedural,
        "exceptions": page.exceptions,
        "generichide": page.generichide,
    });
    unsafe {
        if !scriptlets.is_empty() {
            // The site's scriptlets are the main frame's: a frame of another site has its own.
            controller.addUserScript(&script(
                scriptlets.clone(),
                &WKContentWorld::pageWorld(mtm),
                true,
            ));
        }
        let world = WKContentWorld::worldWithName(&NSString::from_str(WORLD), mtm);
        // uBlock Origin's isolated-world scriptlets: in Trek's world, where the page can't see
        // or change them.
        let isolated = engine::wrap_script(&isolated_scriptlets);
        if !isolated.is_empty() {
            controller.addUserScript(&script(isolated, &world, true));
        }
        controller.addUserScript(&script(
            format!("globalThis.__trekAdblock = {data};\n{COSMETIC_JS}"),
            &world,
            true,
        ));
    }
    with(|s| {
        s.last.insert(
            tab,
            json!({
                "url": url,
                "hide": page.hide.len(),
                "procedural": page.procedural.len(),
                "generichide": page.generichide,
                "scriptlets_bytes": scriptlets.len(),
                "isolated_bytes": isolated_scriptlets.len(),
                // What the engine took for this page, and setting its scripts.
                "lookup_us": lookup.as_micros(),
                // Trek's scripts on the view now: one set per page, never a pile.
                "trek_scripts": unsafe { controller.userScripts() }
                    .iter()
                    .filter(|s| unsafe { s.source() }.hasPrefix(&mark))
                    .count(),
                "total_us": started.elapsed().as_micros(),
            }),
        )
    });
}

/// `source` as one of Trek's page scripts: it starts with `MARK`, which is how `navigating` knows
/// to take it away when the tab goes to the next page.
fn marked(source: String) -> String {
    format!("{MARK}\n{source}")
}

/// What tab `tab`'s page got (the shots driver's `adblock-state`).
pub fn last(tab: u64) -> Option<Value> {
    with(|s| s.last.get(&tab).cloned())
}

/// Answer a page's question: the generic selectors for its classes and ids.
fn answer(body: &str) -> Value {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return Value::Null;
    };
    let strings = |k: &str| -> Vec<String> {
        v[k].as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .take(5000)
                    .collect()
            })
            .unwrap_or_default()
    };
    match v["kind"].as_str() {
        Some("generic") => {
            let Some(engine) = with(|s| s.engine.clone()) else {
                return json!([]);
            };
            json!(engine::generic(
                &engine.engine,
                &strings("classes"),
                &strings("ids"),
                &strings("exceptions"),
            ))
        }
        _ => Value::Null,
    }
}

struct Ivars {
    #[allow(dead_code)]
    tab: u64,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars are dropped with the object.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TrekAdblockHandler"]
    #[ivars = Ivars]
    struct Handler;

    unsafe impl NSObjectProtocol for Handler {}

    unsafe impl WKScriptMessageHandlerWithReply for Handler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:replyHandler:))]
        fn received(
            &self,
            _controller: &WKUserContentController,
            message: &WKScriptMessage,
            done: &Block<dyn Fn(*mut AnyObject, *mut NSString)>,
        ) {
            let body = unsafe { message.body() }
                .downcast::<NSString>()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let text = NSString::from_str(&answer(&body).to_string());
            done.call((
                Retained::as_ptr(&text).cast_mut().cast(),
                std::ptr::null_mut(),
            ));
        }
    }
);

/// Let the user pick an element on tab `tab`'s page (`cosmetic.js`'s picker): the selector they
/// picked, or `None` when they stopped.
pub fn pick(tab: u64) -> async_channel::Receiver<Option<String>> {
    let (tx, rx) = async_channel::bounded(1);
    let Some(view) = with(|s| s.views.get(&tab).map(|v| v.0.clone())) else {
        let _ = tx.try_send(None);
        return rx;
    };
    let Some(mtm) = MainThreadMarker::new() else {
        let _ = tx.try_send(None);
        return rx;
    };
    let done = RcBlock::new(move |value: *mut AnyObject, _error: *mut NSError| {
        let picked = unsafe { value.as_ref() }
            .and_then(|v| v.downcast_ref::<NSString>())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        let _ = tx.try_send(picked);
    });
    unsafe {
        let world = WKContentWorld::worldWithName(&NSString::from_str(WORLD), mtm);
        let body = NSString::from_str(
            "return globalThis.__trekAdblockPick ? await globalThis.__trekAdblockPick() : null;",
        );
        let args = objc2_foundation::NSDictionary::<NSString, AnyObject>::new();
        view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &body,
            Some(&args),
            None,
            &world,
            Some(&done),
        );
    }
    rx
}

/// Hide `selector` on tab `tab`'s page at once (what the user just picked).
pub fn hide_now(tab: u64, selector: &str) {
    let Some(view) = with(|s| s.views.get(&tab).map(|v| v.0.clone())) else {
        return;
    };
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let js = format!(
        "globalThis.__trekAdblockHide && globalThis.__trekAdblockHide({})",
        serde_json::to_string(selector).unwrap_or_default()
    );
    unsafe {
        let world = WKContentWorld::worldWithName(&NSString::from_str(WORLD), mtm);
        view.evaluateJavaScript_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(&js),
            None,
            &world,
            None,
        );
    }
}

/// Evaluate `script` (a plain script that ends in a JSON string) in an unseen view of its own,
/// with nothing of the user's in it (no website data kept): its answer. For uBlock Origin's
/// scriptlets (`scriptlets::bundle`), which only a JavaScript engine can read.
pub fn evaluate(script: String) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let Some(mtm) = MainThreadMarker::new() else {
        let _ = tx.try_send(Err("Not on the main thread.".into()));
        return rx;
    };
    unsafe {
        let config = objc2_web_kit::WKWebViewConfiguration::new(mtm);
        config.setWebsiteDataStore(&objc2_web_kit::WKWebsiteDataStore::nonPersistentDataStore(
            mtm,
        ));
        let view = WKWebView::initWithFrame_configuration(
            WKWebView::alloc(mtm),
            objc2_foundation::NSRect::ZERO,
            &config,
        );
        let keep = RefCell::new(Some(view.clone()));
        let done = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            let answer = match (value.as_ref(), error.as_ref()) {
                (_, Some(e)) => Err(e.localizedDescription().to_string()),
                (Some(v), None) => v
                    .downcast_ref::<NSString>()
                    .map(|s| s.to_string())
                    .ok_or_else(|| "It didn't answer with text.".to_string()),
                (None, None) => Err("No answer.".to_string()),
            };
            let _ = tx.try_send(answer);
            keep.borrow_mut().take();
        });
        view.evaluateJavaScript_completionHandler(&NSString::from_str(&script), Some(&done));
    }
    rx
}
