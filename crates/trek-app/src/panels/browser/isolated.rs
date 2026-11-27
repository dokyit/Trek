//! Running Trek's own script in a page where the page can't reach it: a script world of Trek's
//! own, which shares the page's document and nothing else. What the page puts on `window`, and
//! what it does to `JSON`, `Element.prototype` and the rest, stays in the page's world; Trek's
//! objects stay in Trek's. Browser use runs there (`agent.rs`), because what its script reports
//! decides whether the user is asked before an agent types a password or sends a form away.
//!
//! - macOS: a `WKContentWorld` of Trek's own (`evaluateJavaScript:inFrame:inContentWorld:`).
//! - Linux: a WebKitGTK script world, by name (`webkit_web_view_evaluate_javascript`).
//! - Windows: WebView2's `ExecuteScript` only knows the page's world, so the world is made over
//!   the DevTools protocol (`Page.createIsolatedWorld`, the world content scripts get) and the
//!   script run in it (`Runtime.evaluate`). Should the protocol refuse (it shouldn't, and a page
//!   can't make it), the script runs in the page's world after all, written for that case: it
//!   uses nothing it finds on `window` (`agent.js`, not `isolated`). The page's own `JSON` and
//!   DOM prototypes are then the ones it calls, so the probe can't be trusted there: the tab is
//!   marked and every click, type and press in it asks the user (`agent.rs`).

/// The world's name: one for every page, kept for the life of each document.
const WORLD: &str = "trek-agent";

/// What `done` is handed: the script's value as text (JSON, or JSON of a JSON string as wry's own
/// evaluation gives it; `agent_core::answer` reads both), or nothing for a script that didn't
/// run.
pub type Done = Box<dyn FnOnce(String)>;

/// Run `isolated` in Trek's world of the page `wv` shows, and hand `done` its value. `page` is
/// the same script written for the page's own world, for a platform that can't have one of
/// Trek's (see the module's notes).
#[cfg(target_os = "macos")]
pub fn eval(
    wv: &wry::WebView,
    isolated: &str,
    _page: &str,
    _untrusted: Option<std::rc::Rc<std::cell::Cell<bool>>>,
    done: Done,
) -> Result<(), String> {
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSError, NSString};
    use objc2_web_kit::WKContentWorld;
    use wry::WebViewExtMacOS as _;
    let mtm = MainThreadMarker::new().ok_or("The page can't run scripts off the main thread.")?;
    let view = wv.webview();
    let done = std::cell::Cell::new(Some(done));
    let handler = RcBlock::new(move |value: *mut AnyObject, _: *mut NSError| {
        // SAFETY: WebKit hands the script's value (a string, here) or nil, valid for the call.
        let text = unsafe { value.as_ref() }
            .and_then(|v| v.downcast_ref::<NSString>())
            .map(|s| s.to_string())
            .unwrap_or_default();
        if let Some(done) = done.take() {
            done(text);
        }
    });
    // SAFETY: on the main thread, with a live view; WebKit copies the block.
    unsafe {
        let world = WKContentWorld::worldWithName(&NSString::from_str(WORLD), mtm);
        view.evaluateJavaScript_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(isolated),
            None,
            &world,
            Some(&handler),
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn eval(
    wv: &wry::WebView,
    isolated: &str,
    _page: &str,
    _untrusted: Option<std::rc::Rc<std::cell::Cell<bool>>>,
    done: Done,
) -> Result<(), String> {
    use javascriptcore::ValueExt as _;
    use webkit2gtk::WebViewExt as _;
    use wry::WebViewExtUnix as _;
    wv.webview().evaluate_javascript(
        isolated,
        Some(WORLD),
        None,
        None::<&gtk::gio::Cancellable>,
        move |result| {
            // The script's value is a string (its answer as JSON).
            let text = result
                .ok()
                .filter(|v| v.is_string())
                .map(|v| v.to_str().to_string())
                .unwrap_or_default();
            done(text);
        },
    );
    Ok(())
}

#[cfg(windows)]
pub fn eval(
    wv: &wry::WebView,
    isolated: &str,
    page: &str,
    untrusted: Option<std::rc::Rc<std::cell::Cell<bool>>>,
    done: Done,
) -> Result<(), String> {
    use serde_json::json;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use webview2_com::ExecuteScriptCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
    use windows_61::core::HSTRING;
    use wry::WebViewExtWindows as _;

    static SAID: AtomicBool = AtomicBool::new(false);
    let webview: ICoreWebView2 = wv.webview();
    let done = Rc::new(Cell::new(Some(done)));
    // The page's own world, when Trek's can't be had: said once, since it isn't expected.
    let in_page = {
        let (webview, done, page, untrusted) = (
            webview.clone(),
            done.clone(),
            page.to_string(),
            untrusted.clone(),
        );
        Rc::new(move |why: &str| {
            // The probe ran in the page's own world: the caller marks the tab so every action
            // there asks (`agent.rs`).
            if let Some(u) = &untrusted {
                u.set(true);
            }
            if !SAID.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "browser use: no script world of Trek's own ({why}); its script runs in the page's"
                );
            }
            let Some(done) = done.take() else { return };
            let done = Rc::new(Cell::new(Some(done)));
            let answered = done.clone();
            let handler = ExecuteScriptCompletedHandler::create(Box::new(move |_, value| {
                if let Some(done) = answered.take() {
                    done(value);
                }
                Ok(())
            }));
            // SAFETY: a COM call on the view's own thread.
            if unsafe { webview.ExecuteScript(&HSTRING::from(page.as_str()), &handler) }.is_err()
                && let Some(done) = done.take()
            {
                done(String::new());
            }
        })
    };
    let (for_world, for_run) = (webview.clone(), webview.clone());
    let (no_frame, no_world, no_run) = (in_page.clone(), in_page.clone(), in_page);
    let isolated = isolated.to_string();
    cdp(
        &webview,
        "Page.getFrameTree",
        "{}".into(),
        Box::new(move |tree| {
            let Some(frame) = tree
                .as_ref()
                .and_then(|t| t["frameTree"]["frame"]["id"].as_str())
            else {
                return no_frame("the page's frame isn't known");
            };
            let params = json!({ "frameId": frame, "worldName": WORLD }).to_string();
            cdp(
                &for_world,
                "Page.createIsolatedWorld",
                params,
                Box::new(move |made| {
                    let Some(context) =
                        made.as_ref().and_then(|m| m["executionContextId"].as_i64())
                    else {
                        return no_world("the world wasn't made");
                    };
                    let params = json!({ "expression": isolated, "contextId": context, "returnByValue": true }).to_string();
                    cdp(
                        &for_run,
                        "Runtime.evaluate",
                        params,
                        Box::new(move |ran| {
                            let Some(ran) = ran else {
                                return no_run("the script wasn't run");
                            };
                            // A script that threw (or didn't parse) has no answer.
                            let text = if ran["exceptionDetails"].is_object() {
                                String::new()
                            } else {
                                ran["result"]["value"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string()
                            };
                            if let Some(done) = done.take() {
                                done(text);
                            }
                        }),
                    );
                }),
            );
        }),
    );
    Ok(())
}

/// One DevTools protocol call on `webview`: `then` gets its answer, or `None` if it failed.
#[cfg(windows)]
fn cdp(
    webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    method: &str,
    params: String,
    then: Box<dyn FnOnce(Option<serde_json::Value>)>,
) {
    use std::cell::Cell;
    use std::rc::Rc;
    use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
    use windows_61::core::HSTRING;
    let then = Rc::new(Cell::new(Some(then)));
    let answered = then.clone();
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, answer| {
            let value = result
                .ok()
                .and_then(|()| serde_json::from_str::<serde_json::Value>(&answer).ok());
            if let Some(then) = answered.take() {
                then(value);
            }
            Ok(())
        }));
    // SAFETY: a COM call on the view's own thread, which the handler is called back on.
    let started = unsafe {
        webview.CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            &HSTRING::from(params.as_str()),
            &handler,
        )
    };
    if started.is_err()
        && let Some(then) = then.take()
    {
        then(None);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn eval(
    wv: &wry::WebView,
    _isolated: &str,
    page: &str,
    untrusted: Option<std::rc::Rc<std::cell::Cell<bool>>>,
    done: Done,
) -> Result<(), String> {
    if let Some(u) = untrusted {
        u.set(true);
    }
    wv.evaluate_script_with_callback(page, done)
        .map_err(|e| e.to_string())
}
