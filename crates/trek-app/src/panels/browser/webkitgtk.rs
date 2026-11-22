//! Linux: what WebKitGTK tells about a tab's page, as `webkit.rs` hears WebKit on the Mac and
//! `webview2.rs` WebView2 on Windows. wry covers navigation, titles, IPC and downloads; this
//! adds the rest:
//!
//! - A page's `alert`, `confirm` and `prompt` (and its "leave this page?") go to Trek's bar.
//!   Left to itself WebKitGTK draws its own dialog inside the page, and the page runs nothing (an
//!   agent's scripts included) until someone clicks it. WebKitGTK lets a `script-dialog` handler
//!   keep the dialog and close it later with the answer, so the page waits for the bar as it
//!   waits for its own dialog.
//! - A load that fails says why, on Trek's page (`Msg::LoadFailed`): WebKitGTK's GLib errors
//!   (GIO's, the resolver's, WebKit's own) given as the Mac's codes they mean, so `asks::load_failure`
//!   words them once for both.
//! - A page whose web process ends says so (`Msg::Crashed`).

use super::Msg;
use super::asks::{AskKind, Reply};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use webkit2gtk::{ScriptDialog, ScriptDialogType, WebViewExt as _};

/// Ask ids: their own numbers, as each platform's are.
static NEXT_ASK: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// The dialogs a page waits on, by ask id. GTK's: the main thread's alone.
    static WAITING: RefCell<HashMap<u64, ScriptDialog>> = RefCell::new(HashMap::new());
}

/// Tell the panel what tab `tab`'s page does that wry doesn't say.
pub fn hook(built: &wry::WebView, tab: u64, tx: async_channel::Sender<Msg>) {
    use wry::WebViewExtUnix as _;
    let view = built.webview();
    // A page's `window.open` reaches wry's new-window handler (a tab of its own), as WKWebView's
    // does on the Mac: WebKitGTK otherwise drops one no click started.
    if let Some(settings) = webkit2gtk::WebViewExt::settings(&view) {
        use webkit2gtk::SettingsExt as _;
        settings.set_javascript_can_open_windows_automatically(true);
    }
    let dialogs = tx.clone();
    view.connect_script_dialog(move |view, dialog| {
        let message = dialog.message().map(|m| m.to_string()).unwrap_or_default();
        let kind = match dialog.dialog_type() {
            ScriptDialogType::Alert => AskKind::Alert(message),
            ScriptDialogType::Prompt => AskKind::Prompt {
                message,
                default: dialog
                    .prompt_get_default_text()
                    .map(|d| d.to_string())
                    .unwrap_or_default(),
            },
            // A confirm, and the page asking to be let go of.
            _ => AskKind::Confirm(message),
        };
        let site = view.uri().map(|u| site_of(&u)).unwrap_or_default();
        let id = NEXT_ASK.fetch_add(1, Ordering::Relaxed);
        WAITING.with(|w| w.borrow_mut().insert(id, dialog.clone()));
        if dialogs
            .try_send(Msg::Ask {
                tab,
                id,
                site,
                kind,
            })
            .is_err()
        {
            // No panel to ask: WebKitGTK's own dialog, as before.
            WAITING.with(|w| w.borrow_mut().remove(&id));
            return false;
        }
        true
    });
    let failures = tx.clone();
    view.connect_load_failed(move |_, _, url, error| {
        let (domain, code) = as_webkit_error(error.domain().as_str(), error_code(error));
        // WebKitGTK's own error page stays away: Trek's says why instead (or nothing, for a load
        // that was only cancelled or replaced).
        failures
            .try_send(Msg::LoadFailed {
                tab,
                url: url.to_string(),
                domain: domain.to_string(),
                code,
            })
            .is_ok()
    });
    view.connect_web_process_terminated(move |_, reason| {
        tracing::warn!("browser: tab {tab}'s web process ended ({reason:?})");
        let _ = tx.try_send(Msg::Crashed(tab));
    });
}

/// Answer ask `id`, and let its page go on.
pub fn answer(id: u64, reply: &Reply) {
    let Some(dialog) = WAITING.with(|w| w.borrow_mut().remove(&id)) else {
        return;
    };
    match dialog.dialog_type() {
        ScriptDialogType::Alert => {}
        ScriptDialogType::Prompt => match reply {
            // Not set: the page's `prompt` returns null.
            Reply::No => {}
            Reply::Yes => dialog.prompt_set_text(""),
            Reply::Text(t) => dialog.prompt_set_text(t),
        },
        _ => dialog.confirm_set_confirmed(!matches!(reply, Reply::No)),
    }
    dialog.close();
}

/// End the page's web process, as a crash would (the capture build's `browser crash`).
#[cfg(feature = "shots")]
pub fn end_page_process(built: &wry::WebView) -> Result<usize, String> {
    use wry::WebViewExtUnix as _;
    built.webview().terminate_web_process();
    Ok(1)
}

/// A GLib error's code, which glib's `Error` keeps to itself (it compares one against a known
/// domain's enum only).
fn error_code(error: &gtk::glib::Error) -> i32 {
    use gtk::glib::translate::ToGlibPtr as _;
    let raw: *const gtk::glib::ffi::GError = error.to_glib_none().0;
    // SAFETY: a pointer to the error's own GError, valid while `error` is borrowed.
    unsafe { (*raw).code }
}

/// A WebKitGTK load error (a GLib error's domain and code) as the error WebKit gives on the Mac
/// for the same failure, which `asks::load_failure` puts in Trek's words. Unknown ones go as they
/// are: Trek's page then says the page couldn't be reached.
fn as_webkit_error(domain: &str, code: i32) -> (&str, i64) {
    const NS: &str = "NSURLErrorDomain";
    const WEBKIT: &str = "WebKitErrorDomain";
    match (domain, code) {
        // WebKitNetworkError: CANCELLED.
        ("WebKitNetworkError", 302) => (NS, -999),
        // WebKitPolicyError: FRAME_LOAD_INTERRUPTED_BY_POLICY_CHANGE (a download, a link handed
        // to another app), CANNOT_SHOW_URI, CANNOT_SHOW_MIME_TYPE, CANNOT_USE_RESTRICTED_PORT:
        // the codes WebKit has on the Mac too.
        ("WebKitPolicyError", code @ (100 | 101 | 102 | 103)) => (WEBKIT, i64::from(code)),
        // WebKitNetworkError: UNKNOWN_PROTOCOL; FILE_DOES_NOT_EXIST.
        ("WebKitNetworkError", 301) => (NS, -1002),
        ("WebKitNetworkError", 303) => (NS, -1100),
        // GResolverError: NOT_FOUND, TEMPORARY_FAILURE.
        ("g-resolver-error-quark", 0 | 1) => (NS, -1003),
        // GIOError: CONNECTION_REFUSED, TIMED_OUT, HOST_UNREACHABLE, NETWORK_UNREACHABLE.
        ("g-io-error-quark", 39) => (NS, -1004),
        ("g-io-error-quark", 24) => (NS, -1001),
        ("g-io-error-quark", 37 | 38) => (NS, -1009),
        // GTlsError: the certificate (WebKitGTK says most of these through its own signal, which
        // shows its page).
        ("g-tls-error-quark", _) => (NS, -1202),
        (domain, code) => (domain, i64::from(code)),
    }
}

/// The site a page is on, as the bar names it: its host (and port, when it has one of its own),
/// or its scheme (`file`), as `webkit.rs` names it from WebKit's origin.
fn site_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or((url, ""));
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    if host.is_empty() {
        scheme.to_string()
    } else {
        host.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{as_webkit_error, site_of};

    #[test]
    fn a_dialog_names_the_site_its_page_is_on() {
        assert_eq!(site_of("https://example.com/a?b#c"), "example.com");
        assert_eq!(site_of("http://localhost:3000/"), "localhost:3000");
        assert_eq!(site_of("https://user@host.dev/x"), "host.dev");
        assert_eq!(site_of("file:///tmp/a.html"), "file");
    }

    #[test]
    fn a_failed_load_says_why_in_the_words_it_has_on_the_mac() {
        let words = |domain: &str, code: i32, url: &str| {
            let (domain, code) = as_webkit_error(domain, code);
            super::super::asks::load_failure(domain, code, url).map(|(_, why)| why)
        };
        assert!(
            words("g-io-error-quark", 39, "http://127.0.0.1:39999/")
                .is_some_and(|w| w.contains("running")),
            "a refused connection: is the server running"
        );
        assert!(
            words("WebKitPolicyError", 103, "http://127.0.0.1:9/")
                .is_some_and(|w| w.contains("kept")),
            "a port browsers keep pages off"
        );
        assert!(words("g-resolver-error-quark", 0, "https://nowhere.invalid/").is_some());
        assert_eq!(
            words("WebKitNetworkError", 302, "https://example.com/"),
            None,
            "cancelled"
        );
        assert_eq!(
            words("WebKitPolicyError", 102, "https://example.com/"),
            None,
            "a download"
        );
    }
}
