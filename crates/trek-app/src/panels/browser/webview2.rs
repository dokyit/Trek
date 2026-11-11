//! What WebView2 needs from a browser that wry leaves out (Windows), as `webkit.rs` is for WebKit.
//! Each view's events are hooked as it's made (`hook`), before its first page:
//!
//! - `alert`, `confirm` and `prompt` go to the bar above the page (`asks.rs`) in place of WebView2's
//!   own dialogs; the page waits on the dialog's deferral, kept here, until the user answers
//!   (`answer`). "Leave this page?" lets the page go, as WebKit does without asking.
//! - Camera, microphone and location ask on the bar too, for that request only (WebView2 would keep
//!   an answer in the profile for good); notifications are refused quietly, as WebKit refuses them.
//! - A link only another app opens is asked about (`LaunchingExternalUriScheme`), in place of
//!   WebView2's own dialog; one no app on the PC takes, which WebView2 drops without a word, says
//!   so (`NavigationStarting`).
//! - `window.open` and `target=_blank` get a real view (`new_window`): wry asks for it on a thread of
//!   its own while WebView2 waits, the panel makes it on the opener's environment, and wry hands it
//!   to WebView2 (`SetNewWindow`), so the popup keeps `window.opener` and a sign-in can post its
//!   answer back. `window.close()` closes the tab.

use super::Msg;
use super::asks::{AskKind, LinkKind, Reply, link_kind, scheme_of, site_of_url};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_CAMERA,
    COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
    COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS, COREWEBVIEW2_PERMISSION_STATE_ALLOW,
    COREWEBVIEW2_PERMISSION_STATE_DENY, COREWEBVIEW2_SCRIPT_DIALOG_KIND,
    COREWEBVIEW2_SCRIPT_DIALOG_KIND_ALERT, COREWEBVIEW2_SCRIPT_DIALOG_KIND_CONFIRM,
    COREWEBVIEW2_SCRIPT_DIALOG_KIND_PROMPT, ICoreWebView2, ICoreWebView2_18, ICoreWebView2Deferral,
    ICoreWebView2LaunchingExternalUriSchemeEventHandler,
    ICoreWebView2NavigationStartingEventHandler, ICoreWebView2PermissionRequestedEventArgs,
    ICoreWebView2PermissionRequestedEventArgs3, ICoreWebView2PermissionRequestedEventHandler,
    ICoreWebView2ScriptDialogOpeningEventArgs, ICoreWebView2ScriptDialogOpeningEventHandler,
};
use webview2_com::{
    LaunchingExternalUriSchemeEventHandler, NavigationStartingEventHandler,
    PermissionRequestedEventHandler, ScriptDialogOpeningEventHandler,
    WindowCloseRequestedEventHandler,
};
use windows_61::core::{HSTRING, Interface as _, PCWSTR, PWSTR};
use wry::WebViewExtWindows as _;

/// A page's request waiting for the user's answer: what to answer, and the deferral that holds
/// the page (a dialog) or the request (a permission) until then.
enum Waiting {
    Dialog(
        ICoreWebView2ScriptDialogOpeningEventArgs,
        ICoreWebView2Deferral,
    ),
    /// One or two requests (camera and microphone come one right after the other: one ask).
    Permission {
        site: String,
        kind: AskKind,
        requests: Vec<(
            ICoreWebView2PermissionRequestedEventArgs,
            ICoreWebView2Deferral,
        )>,
    },
}

thread_local! {
    /// What each ask waits on, by its id, with its tab.
    static WAITING: RefCell<HashMap<u64, (u64, Waiting)>> = RefCell::default();
}

static NEXT_ASK: AtomicU64 = AtomicU64::new(1);

/// Answer ask `id` (a no-op once answered: a deferral is completed exactly once).
pub fn answer(id: u64, reply: &Reply) {
    let Some((_, waiting)) = WAITING.with(|w| w.borrow_mut().remove(&id)) else {
        return;
    };
    let yes = !matches!(reply, Reply::No);
    // SAFETY: COM calls on the thread WebView2 handed the arguments over on (the views'); a
    // request whose page has gone answers with an error, which changes nothing.
    unsafe {
        match waiting {
            Waiting::Dialog(args, deferral) => {
                if yes {
                    // A `prompt`'s answer, "" for a plain OK (as WebKit's); other dialogs ignore it.
                    let text = match reply {
                        Reply::Text(t) => t.as_str(),
                        _ => "",
                    };
                    let _ = args.SetResultText(&HSTRING::from(text));
                    let _ = args.Accept();
                }
                let _ = deferral.Complete();
            }
            Waiting::Permission { requests, .. } => {
                let state = if yes {
                    COREWEBVIEW2_PERMISSION_STATE_ALLOW
                } else {
                    COREWEBVIEW2_PERMISSION_STATE_DENY
                };
                for (args, deferral) in requests {
                    let _ = args.SetState(state);
                    let _ = deferral.Complete();
                }
            }
        }
    }
}

/// Tab `tab` is gone: answer everything it still waits on with "no".
pub fn unhook(tab: u64) {
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

/// A permission request for the bar: one more ask, or the second half of one already waiting
/// (the microphone right after the camera), which the bar then asks as one.
fn ask_permission(
    tab: u64,
    tx: &async_channel::Sender<Msg>,
    site: String,
    kind: AskKind,
    request: (
        ICoreWebView2PermissionRequestedEventArgs,
        ICoreWebView2Deferral,
    ),
) {
    let joined = WAITING.with(|w| {
        let mut w = w.borrow_mut();
        w.iter_mut().find_map(|(id, (t, waiting))| match waiting {
            Waiting::Permission {
                site: s,
                kind: k,
                requests,
            } if *t == tab && *s == site => {
                let both = k.joined(&kind)?;
                *k = both.clone();
                requests.push(request.clone());
                Some((*id, both))
            }
            _ => None,
        })
    });
    match joined {
        // The bar's ask by that id now asks for both.
        Some((id, kind)) => {
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
        None => ask(
            tab,
            tx,
            site.clone(),
            kind.clone(),
            Waiting::Permission {
                site,
                kind,
                requests: vec![request],
            },
        ),
    }
}

/// A string WebView2 hands back through `get` ("" when it gives none).
fn text(get: impl FnOnce(*mut PWSTR) -> windows_61::core::Result<()>) -> String {
    let mut s = PWSTR::null();
    match get(&mut s) {
        Ok(()) => webview2_com::take_pwstr(s),
        Err(_) => String::new(),
    }
}

/// Feed view `wv` (tab `tab`) to the panel: its dialogs, permission requests and links for other
/// apps (the bar), and its `window.close()`. Called as the view is made: WebView2 reads whether
/// to draw its own dialogs as each page loads, and a popup's settings must be in place before it's
/// handed over.
pub fn hook(wv: &wry::WebView, tab: u64, tx: &async_channel::Sender<Msg>) {
    let webview = wv.webview();
    let mut token = 0;
    // SAFETY: COM calls on the view's own thread, which WebView2 calls the handlers back on; an
    // older runtime without an interface answers with an error and keeps its own behaviour.
    unsafe {
        if webview
            .add_ScriptDialogOpening(&dialogs(tab, tx.clone()), &mut token)
            .is_ok()
            && let Ok(settings) = webview.Settings()
        {
            let _ = settings.SetAreDefaultScriptDialogsEnabled(false);
        }
        let _ = webview.add_PermissionRequested(&permissions(tab, tx.clone()), &mut token);
        // wry's own handler destroys the view's window; the panel closes its tab.
        let closed = tx.clone();
        let _ = webview.add_WindowCloseRequested(
            &WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
                let _ = closed.try_send(Msg::Closed(tab));
                Ok(())
            })),
            &mut token,
        );
        if let Ok(webview) = webview.cast::<ICoreWebView2_18>() {
            let _ = webview.add_LaunchingExternalUriScheme(&apps(tab, tx.clone()), &mut token);
        }
        let _ = webview.add_NavigationStarting(&no_app(tab, tx.clone()), &mut token);
    }
}

/// `alert`, `confirm`, `prompt` to the bar; "Leave this page?" says yes.
fn dialogs(
    tab: u64,
    tx: async_channel::Sender<Msg>,
) -> ICoreWebView2ScriptDialogOpeningEventHandler {
    ScriptDialogOpeningEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut kind = COREWEBVIEW2_SCRIPT_DIALOG_KIND::default();
        // SAFETY: the event's own arguments, on the thread it's raised on.
        unsafe { args.Kind(&mut kind)? };
        let message = text(|p| unsafe { args.Message(p) });
        let kind = match kind {
            COREWEBVIEW2_SCRIPT_DIALOG_KIND_ALERT => AskKind::Alert(message),
            COREWEBVIEW2_SCRIPT_DIALOG_KIND_CONFIRM => AskKind::Confirm(message),
            COREWEBVIEW2_SCRIPT_DIALOG_KIND_PROMPT => AskKind::Prompt {
                message,
                default: text(|p| unsafe { args.DefaultText(p) }),
            },
            // `beforeunload`: WebKit leaves without asking, and so does Trek here.
            _ => return unsafe { args.Accept() },
        };
        let site = site_of_url(&text(|p| unsafe { args.Uri(p) }));
        let deferral = unsafe { args.GetDeferral()? };
        ask(tab, &tx, site, kind, Waiting::Dialog(args, deferral));
        Ok(())
    }))
}

/// Camera, microphone and location to the bar, for this request only; notifications refused.
/// The rest (clipboard, MIDI, …) as WebView2 decides.
fn permissions(
    tab: u64,
    tx: async_channel::Sender<Msg>,
) -> ICoreWebView2PermissionRequestedEventHandler {
    PermissionRequestedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
        // SAFETY: the event's own arguments, on the thread it's raised on.
        unsafe { args.PermissionKind(&mut kind)? };
        let kind = match kind {
            COREWEBVIEW2_PERMISSION_KIND_CAMERA => AskKind::Camera,
            COREWEBVIEW2_PERMISSION_KIND_MICROPHONE => AskKind::Microphone,
            COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION => AskKind::Location,
            // As WebKit answers them: the site carries on without.
            COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS => {
                this_time_only(&args);
                return unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY) };
            }
            _ => return Ok(()),
        };
        this_time_only(&args);
        let site = site_of_url(&text(|p| unsafe { args.Uri(p) }));
        let deferral = unsafe { args.GetDeferral()? };
        ask_permission(tab, &tx, site, kind, (args, deferral));
        Ok(())
    }))
}

/// The answer holds for this request alone, as WebKit's does: WebView2 would otherwise keep it in
/// the profile and never ask again.
fn this_time_only(args: &ICoreWebView2PermissionRequestedEventArgs) {
    if let Ok(args) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
        // SAFETY: the event's own arguments, on the thread it's raised on.
        let _ = unsafe { args.SetSavesInProfile(false) };
    }
}

/// A link only another app opens: the bar asks (`Msg::External`), and opens it itself.
fn apps(
    tab: u64,
    tx: async_channel::Sender<Msg>,
) -> ICoreWebView2LaunchingExternalUriSchemeEventHandler {
    LaunchingExternalUriSchemeEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        // SAFETY: the event's own arguments, on the thread it's raised on.
        unsafe { args.SetCancel(true)? };
        let url = text(|p| unsafe { args.Uri(p) });
        let _ = tx.try_send(Msg::External { tab, url });
        Ok(())
    }))
}

/// The page itself (not a frame in it: those are WebView2's `FrameNavigationStarting`) going to
/// an address no app on this PC opens, a link or a redirect: WebView2 would drop it without a word
/// (it raises `LaunchingExternalUriScheme` only for a scheme an app took), so the bar says so, as
/// on the Mac.
fn no_app(tab: u64, tx: async_channel::Sender<Msg>) -> ICoreWebView2NavigationStartingEventHandler {
    NavigationStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        // SAFETY: the event's own arguments, on the thread it's raised on.
        let url = text(|p| unsafe { args.Uri(p) });
        if !view_loads(&url) && app_for(&url).is_none() {
            unsafe { args.SetCancel(true)? };
            let _ = tx.try_send(Msg::External { tab, url });
        }
        Ok(())
    }))
}

/// A popup's view on its way from the panel to wry's thread, which hands it back to WebView2 on
/// the views' thread (`SetNewWindow`).
pub struct NewView(pub ICoreWebView2);

// SAFETY: only carried across: WebView2 is called on the thread that made the view.
unsafe impl Send for NewView {}

/// How long a page's `window.open` waits for the panel to make the popup's view (the page is held
/// meanwhile); past it, the page gets no popup.
const POPUP_WAIT: Duration = Duration::from_secs(20);

/// wry's call, on a thread of its own while WebView2 holds the request, for the view a page in tab
/// `opener` opens: the panel makes it (`Msg::OpenPopup`) and sends it back here. No view (the
/// opener's gone, an address another app opens, no answer in time) refuses it: the page's
/// `window.open` gets `null`.
pub fn new_window(
    opener: u64,
    url: String,
    features: wry::NewWindowFeatures,
    tx: &async_channel::Sender<Msg>,
) -> wry::NewWindowResponse {
    let size = features.size.map(|s| (s.width, s.height));
    let (made, view) = std::sync::mpsc::sync_channel(1);
    let asked = tx.try_send(Msg::OpenPopup {
        opener,
        url,
        size,
        from: features.opener,
        made,
    });
    if asked.is_err() {
        return wry::NewWindowResponse::Deny;
    }
    match view.recv_timeout(POPUP_WAIT) {
        Ok(Some(NewView(webview))) => wry::NewWindowResponse::Create { webview },
        _ => wry::NewWindowResponse::Deny,
    }
}

/// Whether WebView2 shows `url` itself: the schemes `link_kind` names, and WebView2's own (an
/// extension's pages, its developer tools, a page's source).
pub fn view_loads(url: &str) -> bool {
    const OWN: [&str; 4] = ["chrome-extension", "devtools", "edge", "view-source"];
    link_kind(url) == LinkKind::View
        || scheme_of(url).is_some_and(|s| OWN.iter().any(|own| s.eq_ignore_ascii_case(own)))
}

/// The app that opens `url` on this PC, by its name (its scheme's association); `None` when none
/// does (Windows would only offer to look for one).
pub fn app_for(url: &str) -> Option<String> {
    use windows_61::Win32::UI::Shell::{
        ASSOCF_IS_PROTOCOL, ASSOCSTR, ASSOCSTR_EXECUTABLE, ASSOCSTR_FRIENDLYAPPNAME,
        AssocQueryStringW,
    };
    let scheme = HSTRING::from(scheme_of(url)?);
    let query = |what: ASSOCSTR| -> Option<String> {
        let mut len = 0u32;
        // SAFETY: a length query, then a buffer of that length.
        let _ = unsafe {
            AssocQueryStringW(
                ASSOCF_IS_PROTOCOL,
                what,
                &scheme,
                PCWSTR::null(),
                None,
                &mut len,
            )
        };
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        let got = unsafe {
            AssocQueryStringW(
                ASSOCF_IS_PROTOCOL,
                what,
                &scheme,
                PCWSTR::null(),
                Some(PWSTR(buf.as_mut_ptr())),
                &mut len,
            )
        };
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let text = String::from_utf16_lossy(&buf[..end]).trim().to_string();
        (got.is_ok() && !text.is_empty()).then_some(text)
    };
    // "Pick an app": an association to Windows' app picker is no app.
    let exe = query(ASSOCSTR_EXECUTABLE);
    if exe
        .as_deref()
        .is_some_and(|e| e.to_ascii_lowercase().ends_with("\\openwith.exe"))
    {
        return None;
    }
    query(ASSOCSTR_FRIENDLYAPPNAME).or_else(|| exe.map(|_| "its app".to_string()))
}
