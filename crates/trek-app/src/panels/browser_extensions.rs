//! The Browser's extensions (Windows). WebView2 runs unpacked Chromium extensions, loaded into
//! the browser's profile, where they stay from one run to the next: `AddBrowserExtension` from a
//! folder, then each one listed, turned on or off, or removed. There's no store in WebView2 and
//! no toolbar for an extension's own button or popup; what runs is what works without them
//! (content scripts: ad blockers, dark modes, password fillers' autofill). So this page takes a
//! Chrome Web Store or Edge Add-ons link (the store's CRX download, unpacked here) or a folder.
//!
//! WKWebView (macOS) can't run extensions of any kind: Safari Web Extensions load in Safari only.
//! The page's logic is the same everywhere (and tested on a Mac); only `native` talks to WebView2.

use crate::ui;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_wry::WebView;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A store an extension can be installed from, by its link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    Chrome,
    Edge,
}

/// An installed extension, as WebView2 lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
    pub id: String,
    pub name: String,
    pub enabled: bool,
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

/// (Nothing installs elsewhere: the page is offered on Windows only.)
#[cfg(not(windows))]
pub fn extensions_dir() -> PathBuf {
    std::env::temp_dir().join("trek-browser-extensions")
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

/// Download extension `id` from `store` and unpack it into its folder (`extensions_dir`), on
/// Trek's runtime; the folder, for WebView2.
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
            let dest = extensions_dir().join(&id);
            unpack_crx(&bytes, &dest)?;
            Ok(dest)
        }
        .await;
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

/// Elsewhere there are no extensions (and the page isn't offered: see `available`).
#[cfg(not(windows))]
mod native {
    use super::Extension;

    pub type Reply<T> = async_channel::Receiver<Result<T, String>>;

    fn none<T>() -> Reply<T> {
        let (tx, rx) = async_channel::bounded(1);
        let _ = tx.try_send(Err("Extensions run on Windows only.".to_string()));
        rx
    }
    pub fn list(_: &wry::WebView) -> Reply<Vec<Extension>> {
        none()
    }
    pub fn add(_: &wry::WebView, _: &std::path::Path) -> Reply<Extension> {
        none()
    }
    pub fn change(_: &wry::WebView, _: String, _: Option<bool>) -> Reply<()> {
        none()
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

/// Whether this Browser can run extensions (and offers the page): WebView2's can.
pub const AVAILABLE: bool = cfg!(windows);

pub enum ExtensionsEvent {
    Close,
}

/// The Extensions page, in the Browser's page area while it's open.
pub struct ExtensionsPage {
    /// A view of the Browser's (they share one profile, and so its extensions).
    view: Entity<WebView>,
    link: Entity<InputState>,
    items: Vec<Extension>,
    loaded: bool,
    /// What's under way ("Downloading…"), while something is.
    busy: Option<&'static str>,
    error: Option<String>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ExtensionsEvent> for ExtensionsPage {}

impl ExtensionsPage {
    pub fn new(view: Entity<WebView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let link = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Chrome Web Store or Edge Add-ons link")
        });
        let subscriptions = vec![cx.subscribe_in(
            &link,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.install_link(window, cx);
                }
            },
        )];
        let mut this = Self {
            view,
            link,
            items: vec![],
            loaded: false,
            busy: None,
            error: None,
            _tasks: vec![],
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
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
                .unwrap_or_else(|_| Err("WebView2 didn't answer.".to_string()));
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
        let reply = native::list(self.view.read(cx).raw());
        self.run("Loading…", reply, cx, |this, items, _| {
            this.items = items;
            this.loaded = true;
        });
    }

    fn install_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.link.read(cx).value().to_string();
        let Some((store, id)) = store_id(&typed) else {
            self.error = Some(
                "Paste an extension's page from the Chrome Web Store \
                 (chromewebstore.google.com/detail/…) or Edge Add-ons."
                    .into(),
            );
            cx.notify();
            return;
        };
        self.link.update(cx, |s, cx| s.set_value("", window, cx));
        self.run(
            "Downloading…",
            download(store, id),
            cx,
            |this, folder, cx| this.install_folder(folder, cx),
        );
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Install".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            if let Some(folder) = paths.into_iter().next() {
                let _ = this.update(cx, |this, cx| this.install_folder(folder, cx));
            }
        })
        .detach();
    }

    fn install_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        let reply = native::add(self.view.read(cx).raw(), &folder);
        self.run("Installing…", reply, cx, |this, _, cx| this.refresh(cx));
    }

    /// Turn extension `id` on or off (`Some`), or remove it (`None`).
    fn change(&mut self, id: String, enable: Option<bool>, cx: &mut Context<Self>) {
        let reply = native::change(self.view.read(cx).raw(), id, enable);
        let what = if enable.is_some() {
            "Saving…"
        } else {
            "Removing…"
        };
        self.run(what, reply, cx, |this, (), cx| this.refresh(cx));
    }

    fn row(&self, ext: &Extension, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (on_id, off_id) = (ext.id.clone(), ext.id.clone());
        h_flex()
            .id(("browser-extension", ix))
            .px(px(12.))
            .py(px(8.))
            .gap(px(10.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().truncate().child(if ext.name.is_empty() {
                        ext.id.clone()
                    } else {
                        ext.name.clone()
                    }))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(ext.id.clone()),
                    ),
            )
            .child(
                Switch::new(("browser-extension-on", ix))
                    .checked(ext.enabled)
                    .small()
                    .disabled(self.busy.is_some())
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
                .disabled(self.busy.is_some())
                .on_click(cx.listener(move |this, _, _, cx| this.change(off_id.clone(), None, cx))),
            )
            .into_any_element()
    }
}

impl Render for ExtensionsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rows: Vec<AnyElement> = self
            .items
            .clone()
            .iter()
            .enumerate()
            .map(|(ix, ext)| self.row(ext, ix, cx))
            .collect();
        let busy = self.busy.is_some();
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
            .child(div().text_xs().text_color(theme.muted_foreground).child(
                "Chromium extensions from the Chrome Web Store or Edge Add-ons, or an \
                         unpacked folder. They run in every page; WebView2 shows no toolbar \
                         button or popup for them.",
            ))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(div().flex_1().child(Input::new(&self.link).small()))
                    .child(
                        Button::new("browser-extensions-install")
                            .small()
                            .primary()
                            .label("Install")
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.install_link(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("browser-extensions-folder")
                            .small()
                            .label("Folder…")
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_folder(window, cx)),
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
            .when(self.loaded || !rows.is_empty(), |el| {
                el.child(
                    v_flex()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(theme.border)
                        .when(rows.is_empty() && self.loaded, |el| {
                            el.child(
                                div()
                                    .p(px(12.))
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("No extensions yet."),
                            )
                        })
                        .children(rows),
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
