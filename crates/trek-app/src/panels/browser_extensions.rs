//! The Browser's extensions, and the Extensions page that finds, installs and manages them. The
//! page is the same everywhere (and tested on a Mac); `backend` is each system's own:
//!
//! - Windows: WebView2 runs unpacked Chromium extensions, loaded into the browser's profile,
//!   where they stay from one run to the next (`AddBrowserExtension`, then each one listed,
//!   turned on or off, or removed). It shows no toolbar for an extension's own button or popup;
//!   what runs is what works without them (content scripts: ad blockers, dark modes).
//! - macOS 15.4 and later: WebKit's own Web Extensions (`webkit`), which run the same Chrome
//!   packages: background service workers, content scripts, declarativeNetRequest rules, an
//!   extension's toolbar button and popup, its options page. Trek keeps what's installed
//!   (`registry`) and asks before an extension gets more than it was installed with.
//!
//! Extensions come from a Chrome Web Store (or Edge Add-ons) link or id, whose CRX the store
//! sends and Trek unpacks; a `.zip` or `.crx` file; or an unpacked folder, run where it is. The
//! page's starter list (`catalog`) says which well-known ones work here. Nothing ships with Trek.
//! Before anything installs, the page shows what it may do (`manifest`), and waits for Install.

pub mod catalog;
pub mod manifest;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // WebView2 keeps its own list.
pub mod registry;
#[cfg(target_os = "macos")]
pub mod webkit;

use crate::ui;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_wry::WebView;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The Extensions page's address: the Browser opens the page for it (Settings links here).
pub const PAGE_URL: &str = "trek://extensions";

/// A store an extension can be installed from, by its link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    Chrome,
    Edge,
}

/// An installed extension, as the backend lists it (WebView2 tells only its id, name and state).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extension {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub version: String,
    pub icon: Option<PathBuf>,
    pub has_options: bool,
    /// What it may do, in words (`manifest::describe`).
    pub permissions: Vec<String>,
    /// What went wrong running it (WebKit's errors: an API it lacks stops a background).
    pub problems: Vec<String>,
    /// It doesn't run: it didn't load, or its background stopped at an error.
    pub broken: bool,
}

/// A store's extension ids: 32 letters a–p (a key's hash, in Chromium's own alphabet).
fn is_extension_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

/// The store and extension id in what was typed: a Chrome Web Store or Edge Add-ons link
/// (`…/detail/<name>/<id>`), or a bare id (taken as Chrome's).
pub fn store_id(input: &str) -> Option<(Store, String)> {
    let input = input.trim();
    if is_extension_id(input) {
        return Some((Store::Chrome, input.to_string()));
    }
    let rest = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))
        .unwrap_or(input);
    let (host, path) = rest.split_once('/')?;
    let store = match host.to_ascii_lowercase().as_str() {
        "chromewebstore.google.com" | "chrome.google.com" => Store::Chrome,
        "microsoftedge.microsoft.com" => Store::Edge,
        _ => return None,
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    let mut parts = path.split('/').filter(|p| !p.is_empty());
    parts.position(|p| p == "detail")?;
    parts
        .find(|p| is_extension_id(p))
        .map(|id| (store, id.to_string()))
}

/// Where the store sends the extension's package (a CRX), following its redirect.
pub fn crx_url(store: Store, id: &str) -> String {
    match store {
        // `prodversion`: the browser asking, which must be new enough for the extension.
        Store::Chrome => format!(
            "https://clients2.google.com/service/update2/crx?response=redirect&prodversion=140.0.0.0\
             &acceptformat=crx2,crx3&x=id%3D{id}%26uc"
        ),
        Store::Edge => format!(
            "https://edge.microsoft.com/extensionwebstorebase/v1/crx?response=redirect\
             &x=id%3D{id}%26installsource%3Dondemand%26uc"
        ),
    }
}

/// The zip archive in a CRX: after its header (`Cr24`, the version, then for CRX3 the header's
/// length, for CRX2 its key's and signature's). A plain zip is taken as it is.
pub fn crx_zip(bytes: &[u8]) -> Result<&[u8], String> {
    if bytes.starts_with(b"PK\x03\x04") {
        return Ok(bytes);
    }
    let word = |at: usize| -> Result<usize, String> {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(|| "The download is cut short.".to_string())
    };
    if !bytes.starts_with(b"Cr24") {
        return Err("The download isn't an extension package.".into());
    }
    let start = match word(4)? {
        3 => 12 + word(8)?,
        2 => 16 + word(8)? + word(12)?,
        v => return Err(format!("Extension packages of version {v} aren't known.")),
    };
    bytes
        .get(start..)
        .filter(|zip| zip.starts_with(b"PK\x03\x04"))
        .ok_or_else(|| "The extension package is damaged.".to_string())
}

/// Where extensions from a store are unpacked: one folder each, by id, beside WebView2's data.
/// WebView2 runs them from there, so they stay.
#[cfg(windows)]
pub fn extensions_dir() -> PathBuf {
    super::browser::webview_data_dir().with_file_name("WebView2-extensions")
}

/// Where extensions are unpacked (one folder each, by id) and listed (`registry`): Trek's data.
#[cfg(not(windows))]
pub fn extensions_dir() -> PathBuf {
    trek_core::paths::data_dir().join("browser-extensions")
}

/// Where a download or a package waits, unpacked, for the user to install it.
fn incoming_dir() -> PathBuf {
    extensions_dir().join(".incoming")
}

/// Unpack the CRX `bytes` into `dest` (replacing what was there). Chromium refuses to load a
/// folder with a `_metadata` folder (the store's signatures, which are only checked for packed
/// extensions), so it goes.
pub fn unpack_crx(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let zip = crx_zip(bytes)?;
    let staging = dest.with_extension("partial");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let archive = staging.with_extension("zip");
    let unpacked = std::fs::write(&archive, zip)
        .map_err(|e| e.to_string())
        .and_then(|()| {
            trek_core::registry::unpack_zip(&archive, &staging).map_err(|e| e.to_string())
        });
    let _ = std::fs::remove_file(&archive);
    if let Err(e) = unpacked {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(staging.join("_metadata"));
    if !staging.join("manifest.json").is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("The package has no manifest.json.".into());
    }
    let _ = std::fs::remove_dir_all(dest);
    std::fs::rename(&staging, dest).map_err(|e| e.to_string())
}

/// Download extension `id` from `store` and unpack it to wait for the user (`incoming_dir`), on
/// Trek's runtime; the folder.
fn download(store: Store, id: String) -> async_channel::Receiver<Result<PathBuf, String>> {
    const MAX_BYTES: usize = 64 << 20;
    let (tx, rx) = async_channel::bounded(1);
    trek_core::runtime().spawn(async move {
        let result = async {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .map_err(|e| e.to_string())?;
            let response = client
                .get(crx_url(store, &id))
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(|e| format!("The store didn't send the extension ({e})."))?;
            let bytes = response.bytes().await.map_err(|e| e.to_string())?;
            if bytes.len() > MAX_BYTES {
                return Err("The extension is too large.".to_string());
            }
            let dest = incoming_dir().join(&id);
            unpack_crx(&bytes, &dest)?;
            Ok(dest)
        }
        .await;
        let _ = tx.send(result).await;
    });
    rx
}

/// Unpack the `.zip` or `.crx` at `path` to wait for the user (`incoming_dir`), under the id its
/// manifest's key gives it (else one from its bytes); the folder.
fn unpack_file(path: PathBuf) -> async_channel::Receiver<Result<PathBuf, String>> {
    let (tx, rx) = async_channel::bounded(1);
    trek_core::runtime().spawn(async move {
        let result = (|| {
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            let staging = incoming_dir().join(format!("file-{}", std::process::id()));
            unpack_crx(&bytes, &staging)?;
            let m = manifest::read(&staging)?;
            let id = m.key_id.clone().unwrap_or_else(|| {
                use sha2::Digest as _;
                let hash = sha2::Sha256::digest(&bytes);
                hash[..16]
                    .iter()
                    .flat_map(|b| [b >> 4, b & 0xf])
                    .map(|n| (b'a' + n) as char)
                    .collect()
            });
            let dest = incoming_dir().join(&id);
            registry::replace_dir(&staging, &dest)?;
            Ok(dest)
        })();
        let _ = tx.send(result).await;
    });
    rx
}

/// WebView2's extension calls, on the profile of the view given. Each answers once WebView2 has
/// (on this thread's message loop): COM's objects stay on this thread, so one call that needs
/// another (turning an extension on finds it first) makes it in the first one's handler.
#[cfg(windows)]
mod native {
    use super::Extension;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2_13, ICoreWebView2BrowserExtension,
        ICoreWebView2BrowserExtensionList, ICoreWebView2Profile7,
    };
    use webview2_com::{
        BrowserExtensionEnableCompletedHandler, BrowserExtensionRemoveCompletedHandler,
        ProfileAddBrowserExtensionCompletedHandler, ProfileGetBrowserExtensionsCompletedHandler,
    };
    use windows_61::core::{BOOL, HSTRING, Interface as _, PWSTR};
    use wry::WebViewExtWindows as _;

    pub type Reply<T> = async_channel::Receiver<Result<T, String>>;

    fn profile(wv: &ICoreWebView2) -> Result<ICoreWebView2Profile7, String> {
        const OLD: &str = "This WebView2 can't run extensions: update the Microsoft Edge \
                           WebView2 Runtime.";
        let wv13 = wv.cast::<ICoreWebView2_13>().map_err(|_| OLD.to_string())?;
        unsafe { wv13.Profile() }
            .map_err(|e| e.message())?
            .cast::<ICoreWebView2Profile7>()
            .map_err(|_| OLD.to_string())
    }

    fn text(read: impl FnOnce(*mut PWSTR) -> windows_61::core::Result<()>) -> String {
        let mut s = PWSTR::null();
        match read(&mut s) {
            Ok(()) => webview2_com::take_pwstr(s),
            Err(_) => String::new(),
        }
    }

    fn describe(ext: &ICoreWebView2BrowserExtension) -> Extension {
        let mut enabled = BOOL::default();
        let _ = unsafe { ext.IsEnabled(&mut enabled) };
        Extension {
            id: text(|p| unsafe { ext.Id(p) }),
            name: text(|p| unsafe { ext.Name(p) }),
            enabled: enabled.as_bool(),
            ..Default::default()
        }
    }

    fn entries(
        list: Option<ICoreWebView2BrowserExtensionList>,
    ) -> Result<Vec<ICoreWebView2BrowserExtension>, String> {
        let list = list.ok_or("WebView2 listed nothing.")?;
        let mut count = 0u32;
        unsafe { list.Count(&mut count) }.map_err(|e| e.message())?;
        (0..count)
            .map(|i| unsafe { list.GetValueAtIndex(i) }.map_err(|e| e.message()))
            .collect()
    }

    /// Ask for every extension; `then` gets them, in WebView2's handler.
    fn with_all(
        wv: &ICoreWebView2,
        then: impl FnOnce(Result<Vec<ICoreWebView2BrowserExtension>, String>) + 'static,
    ) {
        let then = std::rc::Rc::new(std::cell::RefCell::new(Some(then)));
        let later = then.clone();
        let handler =
            ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |result, list| {
                if let Some(then) = later.borrow_mut().take() {
                    then(result.map_err(|e| e.message()).and_then(|()| entries(list)));
                }
                Ok(())
            }));
        let started = profile(wv)
            .and_then(|p| unsafe { p.GetBrowserExtensions(&handler) }.map_err(|e| e.message()));
        if let Err(e) = started
            && let Some(then) = then.borrow_mut().take()
        {
            then(Err(e));
        }
    }

    pub fn list(wv: &wry::WebView) -> Reply<Vec<Extension>> {
        let (tx, rx) = async_channel::bounded(1);
        with_all(&wv.webview(), move |all| {
            let _ = tx.try_send(all.map(|all| all.iter().map(describe).collect()));
        });
        rx
    }

    pub fn add(wv: &wry::WebView, folder: &std::path::Path) -> Reply<Extension> {
        let (tx, rx) = async_channel::bounded(1);
        let done = tx.clone();
        let handler =
            ProfileAddBrowserExtensionCompletedHandler::create(Box::new(move |result, ext| {
                let added = result.map_err(|e| e.message()).and_then(|()| {
                    ext.map(|e| describe(&e))
                        .ok_or_else(|| "WebView2 added nothing.".to_string())
                });
                let _ = done.try_send(added);
                Ok(())
            }));
        let path = HSTRING::from(folder);
        let started = profile(&wv.webview()).and_then(|p| {
            unsafe { p.AddBrowserExtension(&path, &handler) }.map_err(|e| e.message())
        });
        if let Err(e) = started {
            let _ = tx.try_send(Err(e));
        }
        rx
    }

    /// Turn extension `id` on or off (`Some`), or remove it (`None`).
    pub fn change(wv: &wry::WebView, id: String, enable: Option<bool>) -> Reply<()> {
        let (tx, rx) = async_channel::bounded(1);
        with_all(&wv.webview(), move |all| {
            let ext = match all {
                Ok(all) => all.into_iter().find(|e| describe(e).id == id),
                Err(e) => {
                    let _ = tx.try_send(Err(e));
                    return;
                }
            };
            let Some(ext) = ext else {
                let _ = tx.try_send(Err("That extension is gone.".into()));
                return;
            };
            let done = tx.clone();
            let started = match enable {
                Some(on) => {
                    let handler =
                        BrowserExtensionEnableCompletedHandler::create(Box::new(move |result| {
                            let _ = done.try_send(result.map_err(|e| e.message()));
                            Ok(())
                        }));
                    unsafe { ext.Enable(on, &handler) }
                }
                None => {
                    let handler =
                        BrowserExtensionRemoveCompletedHandler::create(Box::new(move |result| {
                            let _ = done.try_send(result.map_err(|e| e.message()));
                            Ok(())
                        }));
                    unsafe { ext.Remove(&handler) }
                }
            };
            if let Err(e) = started {
                let _ = tx.try_send(Err(e.message()));
            }
        });
        rx
    }
}

/// Each system's extension calls, answered on a channel (WebView2's once it has, WebKit's at once).
/// WebView2's are on a Browser view's profile, so they need one; WebKit's are the app's.
mod backend {
    use super::Extension;
    use std::path::Path;

    pub type Reply<T> = async_channel::Receiver<Result<T, String>>;

    #[cfg_attr(windows, allow(dead_code))]
    fn now<T>(answer: Result<T, String>) -> Reply<T> {
        let (tx, rx) = async_channel::bounded(1);
        let _ = tx.try_send(answer);
        rx
    }

    #[cfg(windows)]
    const NO_VIEW: &str = "Open a page in the Browser first.";

    #[cfg(windows)]
    pub fn list(wv: Option<&wry::WebView>) -> Reply<Vec<Extension>> {
        wv.map(super::native::list)
            .unwrap_or_else(|| now(Err(NO_VIEW.into())))
    }
    #[cfg(windows)]
    pub fn add(wv: Option<&wry::WebView>, folder: &Path, _id: &str) -> Reply<Extension> {
        wv.map(|wv| super::native::add(wv, folder))
            .unwrap_or_else(|| now(Err(NO_VIEW.into())))
    }
    #[cfg(windows)]
    pub fn change(wv: Option<&wry::WebView>, id: String, enable: Option<bool>) -> Reply<()> {
        wv.map(|wv| super::native::change(wv, id, enable))
            .unwrap_or_else(|| now(Err(NO_VIEW.into())))
    }

    #[cfg(target_os = "macos")]
    pub fn list(_: Option<&wry::WebView>) -> Reply<Vec<Extension>> {
        now(if super::webkit::ensure() {
            Ok(super::webkit::list())
        } else {
            Err(super::webkit::unsupported())
        })
    }
    #[cfg(target_os = "macos")]
    pub fn add(_: Option<&wry::WebView>, folder: &Path, id: &str) -> Reply<Extension> {
        now(super::webkit::add(folder, id))
    }
    #[cfg(target_os = "macos")]
    pub fn change(_: Option<&wry::WebView>, id: String, enable: Option<bool>) -> Reply<()> {
        now(super::webkit::change(&id, enable))
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    pub fn list(_: Option<&wry::WebView>) -> Reply<Vec<Extension>> {
        now(Err("This Browser runs no extensions.".into()))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    pub fn add(_: Option<&wry::WebView>, _: &Path, _: &str) -> Reply<Extension> {
        now(Err("This Browser runs no extensions.".into()))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    pub fn change(_: Option<&wry::WebView>, _: String, _: Option<bool>) -> Reply<()> {
        now(Err("This Browser runs no extensions.".into()))
    }
}

/// Install the unpacked extension in `folder` into `wv`'s profile (Windows; the imported ad
/// blocker, `browser_adblock`).
#[cfg(windows)]
pub fn install(
    wv: &wry::WebView,
    folder: &Path,
) -> async_channel::Receiver<Result<Extension, String>> {
    native::add(wv, folder)
}

/// Whether this Browser can run extensions (and offers the page): WebView2's can, and WebKit's
/// (the page says so where macOS is older than 15.4).
pub const AVAILABLE: bool = cfg!(any(windows, target_os = "macos"));

/// Settings › Browser's line on extensions: where they come from, and (macOS) what's installed.
pub fn settings_summary() -> String {
    let what = "Chrome Web Store extensions in the Browser: find, install and manage them on its \
                Extensions page (also in the Browser's ⋯ menu).";
    #[cfg(target_os = "macos")]
    {
        if !webkit::supported() {
            return format!("{what} {}", webkit::unsupported());
        }
        let all = registry::load(&extensions_dir());
        let on = all.iter().filter(|e| e.enabled).count();
        match all.len() {
            0 => what.to_string(),
            n => format!("{what} {n} installed, {on} on."),
        }
    }
    #[cfg(not(target_os = "macos"))]
    what.to_string()
}

/// Whether extensions run here now: WebKit's need macOS 15.4.
fn supported() -> bool {
    #[cfg(target_os = "macos")]
    return webkit::supported();
    #[cfg(not(target_os = "macos"))]
    return AVAILABLE;
}

/// A new Browser view's WebKit configuration, with the extensions in it (macOS 15.4 and later).
#[cfg(target_os = "macos")]
pub fn webview_configuration() -> Option<objc2::rc::Retained<objc2_web_kit::WKWebViewConfiguration>>
{
    webkit::configuration()
}

/// A popup's configuration (WebKit's, from its opener) with the extensions in it too.
#[cfg(target_os = "macos")]
pub fn with_extensions(
    config: objc2::rc::Retained<objc2_web_kit::WKWebViewConfiguration>,
) -> objc2::rc::Retained<objc2_web_kit::WKWebViewConfiguration> {
    if supported() {
        webkit::attach_controller(&config);
    }
    config
}

/// Tell the extensions about Browser `key`'s tabs and which is in front (macOS: `tabs.query`,
/// `tabs.onActivated` and the like). `open_tab` opens an address in a new tab of it.
#[cfg(target_os = "macos")]
pub fn sync_tabs<'a>(
    key: u64,
    tabs: impl Iterator<Item = (u64, &'a wry::WebView)>,
    active: u64,
    open_tab: impl Fn(String) + 'static,
) {
    use wry::WebViewExtMacOS as _;
    if !webkit::supported() {
        return;
    }
    let tabs = tabs
        .map(|(id, wv)| (id, objc2::rc::Retained::into_super(wv.webview())))
        .collect();
    webkit::sync_tabs(key, tabs, active, std::rc::Rc::new(open_tab));
}

/// Redraw the app when an extension changes something the Browser shows (macOS: WebKit tells
/// Trek outside GPUI). Once.
#[cfg(target_os = "macos")]
fn wake_on_change(cx: &mut App) {
    if webkit::has_waker() {
        return;
    }
    let (tx, rx) = async_channel::unbounded();
    webkit::set_waker(tx);
    cx.spawn(async move |cx| {
        while rx.recv().await.is_ok() {
            while rx.try_recv().is_ok() {}
            cx.update(|cx| cx.refresh_windows());
        }
    })
    .detach();
}

/// The toolbar's room for extensions' buttons: past it the address field gets too narrow in a
/// panel beside the chat, so the rest are behind one button (`more`: the Extensions page, where
/// each one's icon opens its popup too).
const TOOLBAR_BUTTONS: usize = 2;

/// The extensions' own toolbar buttons, for the Browser's toolbar (macOS): each opens its popup
/// under it, as WebKit presents it, or tells the extension it was clicked. `more` opens the
/// Extensions page, for the buttons that don't fit.
pub fn toolbar(
    window: &mut Window,
    cx: &mut App,
    more: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Vec<AnyElement> {
    #[cfg(target_os = "macos")]
    {
        if !webkit::supported() {
            return vec![];
        }
        wake_on_change(cx);
        webkit::sync_tab_properties();
        let theme = cx.theme().clone();
        let _ = window;
        let actions = webkit::actions();
        let hidden = actions.len().saturating_sub(TOOLBAR_BUTTONS);
        let overflow = (hidden > 0).then(|| {
            div()
                .id("browser-extensions-more")
                .test_support()
                .child(
                    Button::new("browser-extensions-more-button")
                        .ghost()
                        .small()
                        .w(px(28.))
                        .h(px(28.))
                        .icon(Icon::new(crate::assets::Lucide::Puzzle))
                        .tooltip(format!("{hidden} more extensions"))
                        .on_click(more),
                )
                .into_any_element()
        });
        actions
            .into_iter()
            .take(TOOLBAR_BUTTONS)
            .enumerate()
            .map(|(ix, action)| {
                let id = action.id.clone();
                let icon: AnyElement = match &action.icon {
                    Some(path) => img(path.clone()).size(px(16.)).into_any_element(),
                    None => Icon::new(crate::assets::Lucide::Puzzle)
                        .small()
                        .into_any_element(),
                };
                let badge = (!action.badge.is_empty()).then(|| {
                    div()
                        .absolute()
                        .bottom(px(1.))
                        .right(px(0.))
                        .px(px(3.))
                        .rounded(px(4.))
                        .bg(crate::palette::ember(cx))
                        .text_color(gpui::white())
                        .text_size(px(8.5))
                        .line_height(px(11.))
                        .child(action.badge.clone())
                });
                div()
                    .id(("browser-extension-action", ix))
                    .test_support()
                    .relative()
                    .child(
                        Button::new(("browser-extension-action-button", ix))
                            .ghost()
                            .small()
                            .w(px(28.))
                            .h(px(28.))
                            .tooltip(if action.label.is_empty() {
                                id.clone()
                            } else {
                                action.label.clone()
                            })
                            .disabled(!action.enabled)
                            .child(icon)
                            .on_click(move |event, window, _| {
                                let at = event.position();
                                click_action(&id, at, window);
                            }),
                    )
                    .children(badge)
                    .text_color(theme.foreground)
                    .into_any_element()
            })
            .chain(overflow)
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, cx, more);
        vec![]
    }
}

/// Click extension `id`'s toolbar button, pressed at `at` (window points): its popup shows under
/// the button.
#[cfg(target_os = "macos")]
fn click_action(id: &str, at: Point<Pixels>, window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // SAFETY: `ns_view` is the live NSView GPUI made for this window, on the main thread.
    let Some(view) = (unsafe {
        objc2::rc::Retained::retain(appkit.ns_view.as_ptr().cast::<objc2_app_kit::NSView>())
    }) else {
        return;
    };
    // The button's square, from where it was pressed: 28 points, centred on the toolbar's row.
    let rect = objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(
            f64::from(f32::from(at.x)) - 14.,
            f64::from(f32::from(at.y)) - 14.,
        ),
        objc2_foundation::NSSize::new(28., 28.),
    );
    webkit::perform_action(id, view, rect);
}

/// Extensions' requests for more than they were installed with, as strips above the page: what
/// it asks, Don't allow and Allow (macOS; WebKit waits for the answer).
pub fn prompt_strips(cx: &mut App) -> Vec<AnyElement> {
    #[cfg(target_os = "macos")]
    {
        if !webkit::supported() {
            return vec![];
        }
        let theme = cx.theme().clone();
        let amber = crate::palette::amber(cx);
        webkit::prompts()
            .into_iter()
            .map(|p| {
                let (deny, allow) = (p.id, p.id);
                h_flex()
                    .id(("browser-extension-prompt", p.id))
                    .min_h(px(36.))
                    .px(px(12.))
                    .py(px(6.))
                    .gap(px(10.))
                    .text_xs()
                    .bg(amber.opacity(0.1))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Icon::new(crate::assets::Lucide::ShieldCheck)
                            .xsmall()
                            .text_color(amber),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_medium().child(format!("{} wants to:", p.name)))
                            .children(p.asks.iter().map(|a| {
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("• {a}"))
                            })),
                    )
                    .child(
                        Button::new(("browser-extension-deny", p.id))
                            .small()
                            .ghost()
                            .label("Don't allow")
                            .on_click(move |_, _, cx| {
                                webkit::answer(deny, false);
                                cx.refresh_windows();
                            }),
                    )
                    .child(
                        Button::new(("browser-extension-allow", p.id))
                            .small()
                            .primary()
                            .label("Allow")
                            .on_click(move |_, _, cx| {
                                webkit::answer(allow, true);
                                cx.refresh_windows();
                            }),
                    )
                    .into_any_element()
            })
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cx;
        vec![]
    }
}

/// Answer the first waiting prompt (the shots driver's `ext-allow` / `ext-deny`).
pub fn answer_prompt(allow: bool) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let p = webkit::prompts()
            .into_iter()
            .next()
            .ok_or("No extension is asking.")?;
        webkit::answer(p.id, allow);
        Ok(p.name)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = allow;
        Err("Extensions ask nothing here.".into())
    }
}

/// Install from what the shots driver names: a folder, a `.zip`/`.crx` file, or a store link or
/// id (downloaded). No prompt: the driver stands in for the user's Install. The extension's name.
pub async fn install_for_driver(
    spec: String,
    view: Option<Entity<WebView>>,
    cx: &mut AsyncApp,
) -> Result<String, String> {
    let path = PathBuf::from(&spec);
    let folder = if path.is_dir() {
        path
    } else if path.is_file() {
        unpack_file(path)
            .recv()
            .await
            .map_err(|e| e.to_string())??
    } else {
        let (store, id) = store_id(&spec).ok_or("Not a folder, a package or a store id.")?;
        download(store, id)
            .recv()
            .await
            .map_err(|e| e.to_string())??
    };
    let m = manifest::read(&folder)?;
    let (folder, id) = settle(&folder, &m)?;
    let reply = cx.update(|cx| backend::add(view.as_ref().map(|v| v.read(cx).raw()), &folder, &id));
    let ext = reply.recv().await.map_err(|e| e.to_string())??;
    Ok(if ext.name.is_empty() {
        m.name
    } else {
        ext.name
    })
}

/// The installed list as JSON lines for the shots driver's `ext-list`.
pub async fn list_for_driver(
    view: Option<Entity<WebView>>,
    cx: &mut AsyncApp,
) -> Result<serde_json::Value, String> {
    let reply = cx.update(|cx| backend::list(view.as_ref().map(|v| v.read(cx).raw())));
    let items = reply.recv().await.map_err(|e| e.to_string())??;
    #[cfg(target_os = "macos")]
    let actions = webkit::actions();
    Ok(serde_json::Value::Array(
        items
            .into_iter()
            .map(|e| {
                #[cfg(target_os = "macos")]
                let action = actions.iter().find(|a| a.id == e.id).map(|a| {
                    serde_json::json!({ "label": a.label, "badge": a.badge, "enabled": a.enabled })
                });
                #[cfg(not(target_os = "macos"))]
                let action: Option<serde_json::Value> = None;
                serde_json::json!({
                    "id": e.id,
                    "name": e.name,
                    "version": e.version,
                    "enabled": e.enabled,
                    "has_options": e.has_options,
                    "permissions": e.permissions,
                    "problems": e.problems, "broken": e.broken,
                    "action": action,
                })
            })
            .collect(),
    ))
}

/// Turn extension `id` (or the first whose name has it) on or off, or remove it, for the driver.
pub async fn change_for_driver(
    spec: String,
    enable: Option<bool>,
    view: Option<Entity<WebView>>,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    let reply = cx.update(|cx| backend::list(view.as_ref().map(|v| v.read(cx).raw())));
    let items = reply.recv().await.map_err(|e| e.to_string())??;
    let needle = spec.to_lowercase();
    let ext = items
        .iter()
        .find(|e| e.id == spec)
        .or_else(|| {
            items
                .iter()
                .find(|e| e.name.to_lowercase().contains(&needle))
        })
        .ok_or_else(|| format!("No extension {spec:?} is installed."))?;
    let id = ext.id.clone();
    let reply =
        cx.update(|cx| backend::change(view.as_ref().map(|v| v.read(cx).raw()), id, enable));
    reply.recv().await.map_err(|e| e.to_string())?
}

/// Open the extension pages of extension `spec` (its options), for the driver.
#[cfg(target_os = "macos")]
pub fn options_for_driver(spec: &str) -> Result<(), String> {
    let needle = spec.to_lowercase();
    let list = webkit::list();
    let ext = list
        .iter()
        .find(|e| e.id == spec)
        .or_else(|| {
            list.iter()
                .find(|e| e.name.to_lowercase().contains(&needle))
        })
        .ok_or_else(|| format!("No extension {spec:?} is installed."))?;
    webkit::open_options(&ext.id);
    Ok(())
}

/// Where the unpacked extension in `folder` installs, and its id: a download or package waiting
/// in `incoming_dir` moves to its own folder by id; a folder of the user's stays where it is
/// (Chromium's id for it, from its path or its key).
fn settle(folder: &Path, m: &manifest::Manifest) -> Result<(PathBuf, String), String> {
    if folder.starts_with(incoming_dir()) {
        let id = folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dest = extensions_dir().join(&id);
        registry::replace_dir(folder, &dest)?;
        Ok((dest, id))
    } else {
        Ok((folder.to_path_buf(), manifest::folder_id(folder, m)))
    }
}

pub enum ExtensionsEvent {
    Close,
    /// Open an address in a Browser tab (the store's search).
    Open(String),
}

/// An extension unpacked and read, waiting for the user's Install.
struct Pending {
    folder: PathBuf,
    manifest: manifest::Manifest,
    /// The starter list's word on it, when it's there.
    fit: Option<catalog::Fit>,
}

/// The Extensions page, in the Browser's page area while it's open.
pub struct ExtensionsPage {
    /// A view of the Browser's (WebView2: they share one profile, and so its extensions).
    view: Option<Entity<WebView>>,
    search: Entity<InputState>,
    query: String,
    items: Vec<Extension>,
    loaded: bool,
    /// The backend's changes seen (macOS: `webkit::generation`), to list again after one.
    seen: u64,
    /// What's under way ("Downloading…"), while something is.
    busy: Option<&'static str>,
    error: Option<String>,
    pending: Option<Pending>,
    /// The installed extension whose permissions are shown.
    expanded: Option<String>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ExtensionsEvent> for ExtensionsPage {}

impl ExtensionsPage {
    /// `view`: a Browser view (WebView2 needs one); `address`: the page in front, whose store
    /// link (an extension's page on the Chrome Web Store) fills the search.
    pub fn new(
        view: Option<Entity<WebView>>,
        address: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search, or paste a Chrome Web Store link or extension id")
        });
        if let Some(address) = address.filter(|a| store_id(a).is_some()) {
            search.update(cx, |s, cx| s.set_value(address, window, cx));
        }
        let subscriptions = vec![cx.subscribe_in(
            &search,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                InputEvent::Change => {
                    this.query = this.search.read(cx).value().to_string();
                    cx.notify();
                }
                _ => {}
            },
        )];
        let query = search.read(cx).value().to_string();
        let mut this = Self {
            view,
            search,
            query,
            items: vec![],
            loaded: false,
            seen: 0,
            busy: None,
            error: None,
            pending: None,
            expanded: None,
            _tasks: vec![],
            _subscriptions: subscriptions,
        };
        if supported() {
            this.refresh(cx);
        }
        this
    }

    fn raw<'a>(&self, cx: &'a App) -> Option<&'a wry::WebView> {
        self.view.as_ref().map(|v| v.read(cx).raw())
    }

    /// Wait for `reply`, saying `what` meanwhile; then `done` with its answer, or the error shown.
    fn run<T: 'static>(
        &mut self,
        what: &'static str,
        reply: async_channel::Receiver<Result<T, String>>,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        self.busy = Some(what);
        self.error = None;
        let task = cx.spawn(async move |this, cx| {
            let answer = reply
                .recv()
                .await
                .unwrap_or_else(|_| Err("The Browser didn't answer.".to_string()));
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match answer {
                    Ok(value) => done(this, value, cx),
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            self.seen = webkit::generation();
        }
        let reply = backend::list(self.raw(cx));
        self.run("Loading…", reply, cx, |this, items, _| {
            this.items = items;
            this.loaded = true;
        });
    }

    /// Enter in the search: a store link or id installs; a name only filters the list.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.search.read(cx).value().to_string();
        if let Some((store, id)) = store_id(&typed) {
            self.search.update(cx, |s, cx| s.set_value("", window, cx));
            self.query.clear();
            self.install_from_store(store, id, cx);
        } else if let [only] = catalog::search(&typed)[..]
            && !typed.trim().is_empty()
        {
            self.install_from_store(Store::Chrome, only.id.to_string(), cx);
        }
    }

    fn install_from_store(&mut self, store: Store, id: String, cx: &mut Context<Self>) {
        self.pending = None;
        let fit = catalog::by_id(&id).map(catalog::fit);
        self.run(
            "Downloading…",
            download(store, id),
            cx,
            move |this, folder, cx| this.review(folder, fit, cx),
        );
    }

    fn pick(&mut self, folders: bool, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: !folders,
            directories: folders,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            if let Some(path) = paths.into_iter().next() {
                let _ = this.update(cx, |this, cx| {
                    if folders {
                        this.review(path, None, cx);
                    } else {
                        this.run("Unpacking…", unpack_file(path), cx, |this, folder, cx| {
                            this.review(folder, None, cx)
                        });
                    }
                });
            }
        })
        .detach();
    }

    /// Show what the extension in `folder` may do, and wait for Install.
    fn review(&mut self, folder: PathBuf, fit: Option<catalog::Fit>, cx: &mut Context<Self>) {
        match manifest::read(&folder) {
            Ok(manifest) => {
                self.pending = Some(Pending {
                    folder,
                    manifest,
                    fit,
                })
            }
            Err(e) => {
                self.discard(&folder);
                self.error = Some(e);
            }
        }
        cx.notify();
    }

    /// Drop a download or package that waited (never a folder of the user's).
    fn discard(&self, folder: &Path) {
        if folder.starts_with(incoming_dir()) {
            let _ = std::fs::remove_dir_all(folder);
        }
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.pending.take() {
            self.discard(&p.folder);
        }
        cx.notify();
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.pending.take() else {
            return;
        };
        let (folder, id) = match settle(&p.folder, &p.manifest) {
            Ok(settled) => settled,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let reply = backend::add(self.raw(cx), &folder, &id);
        self.run("Installing…", reply, cx, |this, _, cx| this.refresh(cx));
    }

    /// Turn extension `id` on or off (`Some`), or remove it (`None`).
    fn change(&mut self, id: String, enable: Option<bool>, cx: &mut Context<Self>) {
        let reply = backend::change(self.raw(cx), id, enable);
        let what = if enable.is_some() {
            "Saving…"
        } else {
            "Removing…"
        };
        self.run(what, reply, cx, |this, (), cx| this.refresh(cx));
    }

    fn icon(path: Option<&PathBuf>, name: &str, cx: &App) -> AnyElement {
        let theme = cx.theme();
        match path {
            Some(path) => img(path.clone())
                .size(px(24.))
                .flex_none()
                .into_any_element(),
            None => div()
                .size(px(24.))
                .flex_none()
                .rounded(px(6.))
                .bg(theme.muted)
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .font_semibold()
                .text_color(theme.muted_foreground)
                .child(
                    name.chars()
                        .find(|c| c.is_alphanumeric())
                        .unwrap_or('?')
                        .to_uppercase()
                        .to_string(),
                )
                .into_any_element(),
        }
    }

    fn fit_label(fit: catalog::Fit, cx: &App) -> (Hsla, &'static str, Option<&'static str>) {
        match fit {
            catalog::Fit::Works => (crate::palette::emerald(cx), "Works here", None),
            catalog::Fit::Limited(why) => (crate::palette::amber(cx), "Partly works", Some(why)),
            catalog::Fit::Broken(why) => (crate::palette::red(cx), "Doesn't run here", Some(why)),
        }
    }

    fn installed_row(&self, ext: &Extension, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (on_id, off_id, perm_id) = (ext.id.clone(), ext.id.clone(), ext.id.clone());
        let expanded = self.expanded.as_deref() == Some(&ext.id);
        let busy = self.busy.is_some();
        // A broken one says why (the error, not WebKit's "failed to load" after it); a page's
        // script errors wait under Permissions.
        let why = ext
            .problems
            .iter()
            .find(|p| !p.contains("failed to load"))
            .or(ext.problems.first());
        let detail = match why {
            Some(problem) if ext.enabled && ext.broken => div()
                .text_xs()
                .text_color(theme.danger)
                .truncate()
                .child(format!("Doesn't run: {problem}")),
            _ => div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .truncate()
                .child(if ext.version.is_empty() {
                    ext.id.clone()
                } else {
                    format!("{} · {}", ext.version, ext.id)
                }),
        };
        #[cfg(target_os = "macos")]
        let options = (ext.has_options && ext.enabled).then(|| {
            let id = ext.id.clone();
            ui::icon_button(
                ("browser-extension-options", ix),
                IconName::Settings2,
                "Options",
            )
            .on_click(move |_, _, _| webkit::open_options(&id))
        });
        #[cfg(not(target_os = "macos"))]
        let options: Option<Button> = None;
        // Its icon is its toolbar button too (macOS): the toolbar shows only a few.
        let icon = Self::icon(ext.icon.as_ref(), &ext.name, cx);
        #[cfg(target_os = "macos")]
        let icon = if ext.enabled {
            let id = ext.id.clone();
            div()
                .id(("browser-extension-row-action", ix))
                .test_support()
                .cursor_pointer()
                .child(icon)
                .on_click(move |event, window, _| click_action(&id, event.position(), window))
                .into_any_element()
        } else {
            icon
        };
        v_flex()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .id(("browser-extension", ix))
                    .px(px(12.))
                    .py(px(8.))
                    .gap(px(10.))
                    .child(icon)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().child(if ext.name.is_empty() {
                                ext.id.clone()
                            } else {
                                ext.name.clone()
                            }))
                            .child(detail),
                    )
                    .children(options)
                    .when(
                        !ext.permissions.is_empty() || !ext.problems.is_empty(),
                        |el| {
                            el.child(
                                ui::icon_button(
                                    ("browser-extension-permissions", ix),
                                    Icon::new(crate::assets::Lucide::ShieldCheck),
                                    "Permissions",
                                )
                                .selected(expanded)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.expanded =
                                            if this.expanded.as_deref() == Some(&perm_id) {
                                                None
                                            } else {
                                                Some(perm_id.clone())
                                            };
                                        cx.notify();
                                    },
                                )),
                            )
                        },
                    )
                    .child(
                        Switch::new(("browser-extension-on", ix))
                            .checked(ext.enabled)
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                this.change(on_id.clone(), Some(*on), cx)
                            })),
                    )
                    .child(
                        ui::icon_button(
                            ("browser-extension-remove", ix),
                            Icon::new(crate::assets::Lucide::Trash),
                            "Remove",
                        )
                        .disabled(busy)
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.change(off_id.clone(), None, cx)
                            }),
                        ),
                    ),
            )
            .when(expanded, |el| {
                el.child(
                    v_flex()
                        .pl(px(46.))
                        .pr(px(12.))
                        .pb(px(10.))
                        .gap(px(2.))
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .when(!ext.permissions.is_empty(), |el| {
                            el.child(div().text_color(theme.foreground).child("It can:"))
                                .children(
                                    ext.permissions
                                        .iter()
                                        .map(|p| div().child(format!("• {p}"))),
                                )
                        })
                        .when(!ext.problems.is_empty(), |el| {
                            el.child(
                                div()
                                    .pt(px(4.))
                                    .text_color(theme.foreground)
                                    .child("What went wrong running it:"),
                            )
                            .children(
                                ext.problems.iter().take(6).map(|p| {
                                    div().text_color(theme.danger).child(format!("• {p}"))
                                }),
                            )
                        }),
                )
            })
            .into_any_element()
    }

    fn starter_row(
        &self,
        e: &'static catalog::Entry,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let installed = self.items.iter().any(|i| i.id == e.id);
        let (color, verdict, why) = Self::fit_label(catalog::fit(e), cx);
        h_flex()
            .id(("browser-extension-starter", ix))
            .px(px(12.))
            .py(px(8.))
            .gap(px(10.))
            .border_b_1()
            .border_color(theme.border)
            .child(Self::icon(None, e.name, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(div().text_sm().child(e.name))
                            .child(
                                div()
                                    .px(px(6.))
                                    .rounded(px(4.))
                                    .text_size(px(10.5))
                                    .bg(color.opacity(0.14))
                                    .text_color(color)
                                    .child(verdict),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(match why {
                                Some(why) => format!("{} It {why}.", e.blurb),
                                None => e.blurb.to_string(),
                            }),
                    ),
            )
            .child(
                Button::new(("browser-extension-get", ix))
                    .small()
                    .when(!installed, |b| b.outline())
                    .when(installed, |b| b.ghost())
                    .label(if installed { "Installed" } else { "Install" })
                    .disabled(installed || self.busy.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.install_from_store(Store::Chrome, e.id.to_string(), cx)
                    })),
            )
            .into_any_element()
    }

    fn review_card(&self, p: &Pending, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let m = &p.manifest;
        let asks = manifest::describe(m);
        let fit = p.fit.map(|f| Self::fit_label(f, cx));
        v_flex()
            .id("browser-extension-review")
            .p(px(12.))
            .gap(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(Self::icon(m.icon.as_ref(), &m.name, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .child(format!("Install {}?", m.name)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .truncate()
                                    .child(if m.version.is_empty() {
                                        m.description.clone()
                                    } else {
                                        format!("{} · {}", m.version, m.description)
                                    }),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap(px(2.))
                    .text_xs()
                    .child(div().child(if asks.is_empty() {
                        "It asks for no permissions.".to_string()
                    } else {
                        "It will be able to:".to_string()
                    }))
                    .children(asks.iter().map(|a| {
                        div()
                            .text_color(theme.muted_foreground)
                            .child(format!("• {a}"))
                    })),
            )
            .when_some(
                fit.and_then(|(c, v, why)| why.map(|w| (c, v, w))),
                |el, (color, verdict, why)| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(color)
                            .child(format!("{verdict}: it {why}.")),
                    )
                },
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(
                        Button::new("browser-extension-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                    )
                    .child(
                        Button::new("browser-extension-install")
                            .small()
                            .primary()
                            .label("Install")
                            .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                    ),
            )
            .into_any_element()
    }
}

impl Render for ExtensionsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // WebKit changed something (an extension loaded or failed): list again.
        #[cfg(target_os = "macos")]
        if self.loaded && self.busy.is_none() && webkit::generation() != self.seen {
            self.seen = webkit::generation();
            self.items = webkit::list();
        }
        let theme = cx.theme().clone();
        let busy = self.busy.is_some();
        let installed: Vec<AnyElement> = self
            .items
            .clone()
            .iter()
            .enumerate()
            .map(|(ix, ext)| self.installed_row(ext, ix, cx))
            .collect();
        let starters: Vec<AnyElement> = catalog::search(&self.query)
            .into_iter()
            .enumerate()
            .map(|(ix, e)| self.starter_row(e, ix, cx))
            .collect();
        let review = self
            .pending
            .as_ref()
            .map(|p| (p.folder.clone(), p.manifest.clone(), p.fit))
            .map(|(folder, manifest, fit)| {
                self.review_card(
                    &Pending {
                        folder,
                        manifest,
                        fit,
                    },
                    cx,
                )
            });
        let query = self.query.trim().to_string();
        let searching_name = !query.is_empty() && store_id(&query).is_none();
        let heading = |text: &'static str| {
            div()
                .pt(px(6.))
                .text_xs()
                .font_medium()
                .text_color(theme.muted_foreground)
                .child(text)
        };
        let about = if cfg!(windows) {
            "Chromium extensions from the Chrome Web Store or Edge Add-ons, a .zip or .crx, or an \
             unpacked folder. They run in every page; WebView2 shows no toolbar button or popup \
             for them."
        } else {
            "Chrome extensions from the Chrome Web Store, a .zip or .crx, or an unpacked folder, \
             run by WebKit: content scripts, blocking rules, toolbar buttons and popups. Some need \
             Chrome APIs WebKit doesn't have; the list below says which."
        };
        v_flex()
            .id("browser-extensions")
            .size_full()
            .p(px(16.))
            .gap(px(12.))
            .overflow_y_scroll()
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(Icon::new(crate::assets::Lucide::Puzzle).small())
                    .child(div().flex_1().text_base().child("Extensions"))
                    .child(
                        div().id("browser-extensions-close").test_support().child(
                            ui::icon_button(
                                "browser-extensions-close-button",
                                IconName::Close,
                                "Close",
                            )
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ExtensionsEvent::Close))),
                        ),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(about),
            )
            .when(!supported(), |el| {
                #[cfg(target_os = "macos")]
                let why = webkit::unsupported();
                #[cfg(not(target_os = "macos"))]
                let why = String::new();
                el.child(div().text_sm().text_color(theme.danger).child(why))
            })
            .when(supported(), |el| {
                el.child(
                    h_flex()
                        .gap(px(6.))
                        .child(div().flex_1().child(Input::new(&self.search).small()))
                        .child(
                            Button::new("browser-extensions-folder")
                                .small()
                                .label("Folder…")
                                .disabled(busy)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.pick(true, window, cx)),
                                ),
                        )
                        .child(
                            Button::new("browser-extensions-file")
                                .small()
                                .label("File…")
                                .disabled(busy)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.pick(false, window, cx)),
                                ),
                        ),
                )
                .when_some(self.busy, |el, what| {
                    el.child(
                        h_flex()
                            .gap(px(8.))
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(Spinner::new().small())
                            .child(what),
                    )
                })
                .when_some(self.error.clone(), |el, e| {
                    el.child(div().text_xs().text_color(theme.danger).child(e))
                })
                .children(review)
                .child(heading("Installed"))
                .child(
                    v_flex()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(theme.border)
                        .when(installed.is_empty(), |el| {
                            el.child(
                                div()
                                    .p(px(12.))
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(if self.loaded {
                                        "No extensions yet."
                                    } else {
                                        "Loading…"
                                    }),
                            )
                        })
                        .children(installed),
                )
                .child(heading("From the Chrome Web Store"))
                .child(
                    v_flex()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(theme.border)
                        .children(starters)
                        .when(searching_name, |el| {
                            let url = catalog::store_search_url(&query);
                            el.child(
                                h_flex()
                                    .id("browser-extensions-store-search")
                                    .px(px(12.))
                                    .py(px(8.))
                                    .gap(px(8.))
                                    .text_xs()
                                    .text_color(crate::palette::link(cx))
                                    .cursor_pointer()
                                    .child(Icon::new(crate::assets::Lucide::Search).xsmall())
                                    .child(format!(
                                        "Search the Chrome Web Store for “{query}”, then paste the \
                                         extension's link here"
                                    ))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(ExtensionsEvent::Open(url.clone()))
                                    })),
                            )
                        }),
                )
            })
    }
}

/// Ad and tracker networks the built-in blocker (macOS; see `blocker`) stops pages loading from:
/// the big ad exchanges and analytics beacons, by their own domains. A short list on purpose:
/// what's here is never a site's own content, so blocking it breaks nothing a page needs.
const TRACKERS: &[&str] = &[
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "google-analytics.com",
    "googletagmanager.com",
    "googletagservices.com",
    "adservice.google.com",
    "pagead2.googlesyndication.com",
    "amazon-adsystem.com",
    "adsrvr.org",
    "adnxs.com",
    "criteo.com",
    "criteo.net",
    "taboola.com",
    "outbrain.com",
    "rubiconproject.com",
    "pubmatic.com",
    "openx.net",
    "casalemedia.com",
    "smartadserver.com",
    "moatads.com",
    "scorecardresearch.com",
    "quantserve.com",
    "quantcount.com",
    "hotjar.com",
    "mouseflow.com",
    "fullstory.com",
    "mixpanel.com",
    "segment.io",
    "segment.com",
    "amplitude.com",
    "branch.io",
    "adjust.com",
    "appsflyer.com",
    "connect.facebook.net",
    "ads-twitter.com",
    "analytics.twitter.com",
    "ads.linkedin.com",
    "px.ads.linkedin.com",
    "bat.bing.com",
    "clarity.ms",
    "yandex.ru/metrika",
    "mc.yandex.ru",
    "advertising.com",
    "media.net",
    "zedo.com",
    "yieldmo.com",
    "sharethrough.com",
    "teads.tv",
    "3lift.com",
    "bidswitch.net",
    "contextweb.com",
    "lijit.com",
    "sovrn.com",
    "indexww.com",
    "chartbeat.com",
    "newrelic.com/nr-",
    "nr-data.net",
    "adform.net",
    "serving-sys.com",
    "everesttech.net",
    "demdex.net",
    "omtrdc.net",
    "krxd.net",
    "bluekai.com",
    "exelator.com",
    "agkn.com",
    "rlcdn.com",
    "tapad.com",
    "crwdcntrl.net",
];

/// The tracker list as WebKit's content rules: one block rule per entry, for loads from it on
/// another site's page (third-party), so the networks' own sites still open.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // Only WebKit takes it; tested everywhere.
fn content_rules() -> String {
    let rules: Vec<serde_json::Value> = TRACKERS
        .iter()
        .map(|entry| {
            let (host, path) = entry.split_once('/').unwrap_or((entry, ""));
            let mut filter = format!("^https?://([^/]+\\.)?{}[:/]", regex_escape(host));
            if !path.is_empty() {
                filter.push_str(&regex_escape(path));
            }
            serde_json::json!({
                "trigger": { "url-filter": filter, "load-type": ["third-party"] },
                "action": { "type": "block" },
            })
        })
        .collect();
    serde_json::Value::Array(rules).to_string()
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn regex_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            let special = matches!(
                c,
                '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\'
            );
            special
                .then_some('\\')
                .into_iter()
                .chain(std::iter::once(c))
        })
        .collect()
}

/// The built-in ad and tracker blocker (macOS): WKWebView runs no extensions, but takes content
/// rules (Safari's own content-blocker format), compiled once and added to each view's
/// controller. They apply from the next load on.
#[cfg(target_os = "macos")]
pub mod blocker {
    use objc2::rc::Retained;
    use objc2_foundation::{NSError, NSString};
    use objc2_web_kit::{WKContentRuleList, WKContentRuleListStore, WKWebView};
    use std::cell::RefCell;

    /// The compiled list once WebKit has it, and the views (with on/off) waiting for it.
    #[derive(Default)]
    struct State {
        list: Option<Retained<WKContentRuleList>>,
        compiling: bool,
        waiting: Vec<(Retained<WKWebView>, bool)>,
    }

    thread_local! {
        static STATE: RefCell<State> = RefCell::default();
    }

    fn apply(view: &WKWebView, list: &WKContentRuleList, on: bool) {
        // SAFETY: main thread (the views live there), a live view and list.
        unsafe {
            let controller = view.configuration().userContentController();
            controller.removeContentRuleList(list);
            if on {
                controller.addContentRuleList(list);
            }
        }
    }

    /// Block trackers in `wv`'s pages from its next load (`on`), or stop.
    pub fn set(wv: &wry::WebView, on: bool) {
        use wry::WebViewExtMacOS as _;
        let view: Retained<WKWebView> = Retained::into_super(wv.webview());
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        let compile = STATE.with_borrow_mut(|state| {
            if let Some(list) = &state.list {
                apply(&view, list, on);
                return false;
            }
            state.waiting.retain(|(v, _)| *v != view);
            state.waiting.push((view, on));
            !std::mem::replace(&mut state.compiling, true)
        });
        if !compile {
            return;
        }
        let done = block2::RcBlock::new(|list: *mut WKContentRuleList, error: *mut NSError| {
            // SAFETY: WebKit hands a valid list or nil, on the main thread.
            let list = unsafe { Retained::retain(list) };
            if list.is_none() {
                let why = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
                tracing::warn!("browser: the tracker list didn't compile: {why:?}");
            }
            STATE.with_borrow_mut(|state| {
                state.compiling = false;
                let waiting = std::mem::take(&mut state.waiting);
                if let Some(list) = list {
                    for (view, on) in waiting {
                        apply(&view, &list, on);
                    }
                    state.list = Some(list);
                }
            });
        });
        // SAFETY: main thread; WebKit copies the block.
        unsafe {
            if let Some(store) = WKContentRuleListStore::defaultStore(mtm) {
                store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
                    Some(&NSString::from_str("trek-trackers-1")),
                    Some(&NSString::from_str(&super::content_rules())),
                    Some(&done),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Store, TRACKERS, content_rules, crx_url, crx_zip, store_id, unpack_crx};
    use std::io::Write as _;

    const UBOL: &str = "ddkjiahejlhfcafbddmgiahcphecmpfh";

    #[test]
    fn store_links_name_their_extension() {
        for link in [
            "https://chromewebstore.google.com/detail/ublock-origin-lite/ddkjiahejlhfcafbddmgiahcphecmpfh",
            "https://chromewebstore.google.com/detail/ddkjiahejlhfcafbddmgiahcphecmpfh?hl=en",
            "chromewebstore.google.com/detail/ublock-origin-lite/ddkjiahejlhfcafbddmgiahcphecmpfh/reviews",
            "https://chrome.google.com/webstore/detail/ublock-origin-lite/ddkjiahejlhfcafbddmgiahcphecmpfh",
            "  ddkjiahejlhfcafbddmgiahcphecmpfh  ",
        ] {
            assert_eq!(store_id(link), Some((Store::Chrome, UBOL.into())), "{link}");
        }
        assert_eq!(
            store_id(
                "https://microsoftedge.microsoft.com/addons/detail/ublock-origin-lite/cimighlppcgcoapaliogpjjdehbnofhn"
            ),
            Some((Store::Edge, "cimighlppcgcoapaliogpjjdehbnofhn".into()))
        );
        for not in [
            "https://example.com/detail/x/ddkjiahejlhfcafbddmgiahcphecmpfh",
            "https://chromewebstore.google.com/category/extensions",
            // Not the store's alphabet (a–p), or not 32 letters.
            "https://chromewebstore.google.com/detail/x/zzkjiahejlhfcafbddmgiahcphecmpfh",
            "ddkjiahejlhfcafbddmgiahcphecmpf",
            "",
        ] {
            assert_eq!(store_id(not), None, "{not}");
        }
    }

    #[test]
    fn the_store_sends_the_package_for_the_id() {
        assert!(crx_url(Store::Chrome, UBOL).contains(&format!("x=id%3D{UBOL}%26uc")));
        assert!(crx_url(Store::Chrome, UBOL).starts_with("https://clients2.google.com/"));
        assert!(crx_url(Store::Edge, UBOL).starts_with("https://edge.microsoft.com/"));
    }

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in files {
            out.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            out.write_all(body.as_bytes()).unwrap();
        }
        out.finish().unwrap().into_inner()
    }

    fn crx3(zip: &[u8]) -> Vec<u8> {
        let header = [7u8; 20];
        let mut crx = b"Cr24".to_vec();
        crx.extend(3u32.to_le_bytes());
        crx.extend((header.len() as u32).to_le_bytes());
        crx.extend(header);
        crx.extend(zip);
        crx
    }

    #[test]
    fn a_package_is_its_zip_after_the_header() {
        let zip = zip_of(&[("manifest.json", "{}")]);
        assert_eq!(crx_zip(&crx3(&zip)).unwrap(), &zip[..]);
        let mut crx2 = b"Cr24".to_vec();
        crx2.extend(2u32.to_le_bytes());
        crx2.extend(3u32.to_le_bytes()); // key
        crx2.extend(2u32.to_le_bytes()); // signature
        crx2.extend([1, 2, 3, 4, 5]);
        crx2.extend(&zip);
        assert_eq!(crx_zip(&crx2).unwrap(), &zip[..]);
        assert_eq!(crx_zip(&zip).unwrap(), &zip[..]);
        assert!(crx_zip(b"<html>not found</html>").is_err());
        assert!(crx_zip(b"Cr24\x03\0\0\0\xff\xff\0\0").is_err());
        assert!(crx_zip(b"Cr24\x09\0\0\0\0\0\0\0").is_err());
    }

    #[test]
    fn a_package_unpacks_without_the_stores_signatures() {
        let dir = std::env::temp_dir().join(format!("trek-ext-test-{}", std::process::id()));
        let dest = dir.join(UBOL);
        let zip = zip_of(&[
            ("manifest.json", r#"{"name":"x","manifest_version":3}"#),
            ("js/content.js", "1"),
            ("_metadata/verified_contents.json", "[]"),
        ]);
        unpack_crx(&crx3(&zip), &dest).unwrap();
        assert!(dest.join("manifest.json").is_file());
        assert!(dest.join("js/content.js").is_file());
        assert!(!dest.join("_metadata").exists());
        // Again, over the last one: it's replaced.
        unpack_crx(&crx3(&zip_of(&[("manifest.json", "{}")])), &dest).unwrap();
        assert!(!dest.join("js").exists());
        // No manifest: refused, and what was there stays.
        assert!(unpack_crx(&crx3(&zip_of(&[("a.js", "1")])), &dest).is_err());
        assert!(dest.join("manifest.json").is_file());
        // A name reaching out of the folder: refused.
        assert!(unpack_crx(&crx3(&zip_of(&[("../evil.js", "1")])), &dest).is_err());
        assert!(!dir.join("evil.js").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tracker_list_is_one_third_party_block_rule_each() {
        let rules: Vec<serde_json::Value> = serde_json::from_str(&content_rules()).unwrap();
        assert_eq!(rules.len(), TRACKERS.len());
        let first = &rules[0];
        assert_eq!(first["action"]["type"], "block");
        assert_eq!(first["trigger"]["load-type"][0], "third-party");
        assert_eq!(
            first["trigger"]["url-filter"],
            "^https?://([^/]+\\.)?doubleclick\\.net[:/]"
        );
        // A path narrows the rule to it.
        let metrika = rules
            .iter()
            .find(|r| {
                r["trigger"]["url-filter"]
                    .as_str()
                    .unwrap()
                    .contains("yandex\\.ru")
            })
            .unwrap();
        assert!(
            metrika["trigger"]["url-filter"]
                .as_str()
                .unwrap()
                .ends_with("[:/]metrika")
        );
    }
}
