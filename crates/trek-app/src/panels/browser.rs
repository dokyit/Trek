//! Browser: tabbed embedded web views (WKWebView on macOS, WebView2 on Windows) with a themed
//! start page that finds local dev servers, element picking and page screenshots for the
//! composer, devtools, and zoom.

use crate::assets::Lucide;
use crate::ui;
use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_wry::WebView;
use std::borrow::Cow;
use std::cell::RefCell;
use std::path::PathBuf;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Trek's internal pages (start page, load errors) live on this scheme.
const START_URL: &str = "trek://newtab/";
const ERROR_URL: &str = "trek://newtab/error";
const START_HTML: &str = include_str!("browser_start.html");

/// Ports dev servers commonly listen on (Next, Vite, Angular, Django, Tauri, Storybook…).
const DEV_PORTS: [u16; 13] = [3000, 3001, 4173, 4200, 5000, 5173, 5174, 8000, 8080, 8081, 8888, 1420, 6006];

const ZOOM_STEPS: [f64; 9] = [0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 2.0];

/// Runs in every page's main frame: reports title/favicon, SPA URL changes.
const INIT_JS: &str = r#"(() => {
  if (window.__trekInit) return;
  window.__trekInit = true;
  const post = (o) => { try { window.ipc.postMessage(JSON.stringify(o)); } catch (_) {} };
  const icon = () => {
    const links = Array.from(document.querySelectorAll('link[rel~="icon"], link[rel="shortcut icon"], link[rel~="apple-touch-icon"]')).filter((l) => l.href);
    const score = (l) => { const t = (l.type || '').toLowerCase(); if (t.includes('svg') || /\.svg(\?|$)/.test(l.href)) return 1; if (l.rel.includes('apple')) return 2; return 3; };
    links.sort((a, b) => score(b) - score(a));
    if (links[0]) return links[0].href;
    return /^https?:$/.test(location.protocol) ? location.origin + '/favicon.ico' : '';
  };
  const meta = () => post({ t: 'meta', url: location.href, title: document.title, icon: icon() });
  document.addEventListener('DOMContentLoaded', meta);
  window.addEventListener('load', meta);
  const changed = () => post({ t: 'url', url: location.href });
  for (const k of ['pushState', 'replaceState']) {
    const orig = history[k];
    history[k] = function () { const r = orig.apply(this, arguments); changed(); return r; };
  }
  window.addEventListener('popstate', changed);
  window.addEventListener('hashchange', changed);
})();"#;

/// Element picker: highlight on hover, report the clicked element, Escape cancels.
const PICK_JS: &str = r#"(() => {
  if (window.__trekPick) return;
  const post = (o) => { try { window.ipc.postMessage(JSON.stringify(o)); } catch (_) {} };
  const box = document.createElement('div');
  box.style.cssText = 'position:fixed;pointer-events:none;z-index:2147483647;border:2px solid #FF6A3D;background:rgba(255,106,61,.12);border-radius:3px;transition:all 60ms ease-out;display:none';
  const tag = document.createElement('div');
  tag.style.cssText = 'position:fixed;pointer-events:none;z-index:2147483647;background:#FF6A3D;color:#fff;font:600 11px ui-monospace,SFMono-Regular,Menlo,monospace;padding:2px 6px;border-radius:4px;white-space:nowrap;display:none';
  document.documentElement.append(box, tag);
  const cursor = document.documentElement.style.cursor;
  document.documentElement.style.cursor = 'crosshair';
  const part = (el) => {
    let s = el.tagName.toLowerCase();
    if (el.id) return /^[A-Za-z][\w-]*$/.test(el.id) ? s + '#' + el.id : s + '[id="' + el.id.replace(/"/g, '\\"') + '"]';
    const cls = Array.from(el.classList).filter((c) => !/[:\[\]\/]/.test(c)).slice(0, 2);
    if (cls.length) s += '.' + cls.map((c) => CSS.escape(c)).join('.');
    const p = el.parentElement;
    if (p) {
      const same = Array.from(p.children).filter((c) => c.tagName === el.tagName);
      if (same.length > 1) s += ':nth-of-type(' + (same.indexOf(el) + 1) + ')';
    }
    return s;
  };
  const selector = (el) => {
    const parts = [];
    for (let e = el; e && e.nodeType === 1 && e !== document.documentElement && parts.length < 5; e = e.parentElement) {
      parts.unshift(part(e));
      if (e.id) break;
    }
    return parts.join(' > ');
  };
  let current = null;
  const move = (e) => {
    const el = e.target;
    if (!el || el === box || el === tag || el.nodeType !== 1) return;
    current = el;
    const r = el.getBoundingClientRect();
    Object.assign(box.style, { display: 'block', left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px' });
    tag.textContent = part(el) + '  ' + Math.round(r.width) + '×' + Math.round(r.height);
    tag.style.display = 'block';
    const top = r.top > 24 ? r.top - 22 : r.bottom + 4;
    Object.assign(tag.style, { left: Math.max(4, Math.min(r.left, innerWidth - tag.offsetWidth - 4)) + 'px', top: top + 'px' });
  };
  const block = (e) => { e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation(); };
  const click = (e) => {
    block(e);
    const el = current || e.target;
    stop();
    if (!el || el.nodeType !== 1) return;
    post({ t: 'pick', url: location.href, selector: selector(el), tag: el.tagName.toLowerCase(),
      text: (el.innerText || '').trim().replace(/\s+/g, ' ').slice(0, 200), html: el.outerHTML.slice(0, 1500) });
  };
  const key = (e) => { if (e.key === 'Escape') { block(e); stop(); post({ t: 'pick-cancel' }); } };
  const opts = { capture: true };
  const stop = () => {
    document.removeEventListener('mousemove', move, opts);
    document.removeEventListener('click', click, opts);
    document.removeEventListener('keydown', key, opts);
    ['mousedown', 'mouseup', 'pointerdown', 'pointerup', 'dblclick'].forEach((n) => document.removeEventListener(n, block, opts));
    box.remove(); tag.remove();
    document.documentElement.style.cursor = cursor;
    delete window.__trekPick;
  };
  document.addEventListener('mousemove', move, opts);
  document.addEventListener('click', click, opts);
  document.addEventListener('keydown', key, opts);
  ['mousedown', 'mouseup', 'pointerdown', 'pointerup', 'dblclick'].forEach((n) => document.addEventListener(n, block, opts));
  window.__trekPick = { stop };
})();"#;

const PICK_STOP_JS: &str = "window.__trekPick && window.__trekPick.stop();";

/// Where to get the WebView2 runtime when a Windows system has none.
const WEBVIEW2_DOWNLOAD: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";

/// The scheme Trek's own pages are registered under, which Trek spells `trek://`. WebView2 can't
/// load a custom scheme, so on Windows wry serves `<scheme>://newtab/…` as `http://<scheme>.newtab/…`
/// and answers every `http://<scheme>.*` request itself: nothing else can be at such an address.
/// So there the scheme is one no web site uses (`http://trek.localhost`, say, is somebody's dev
/// server, not Trek's start page).
const PAGES_SCHEME: &str = if cfg!(windows) { "trek-pages" } else { "trek" };
const WEBVIEW2_TREK: &str = "http://trek-pages.";

/// Trek's form of a page address for WebView2's (`trek://newtab/` is `http://trek-pages.newtab/`),
/// whatever case the scheme was typed in.
fn to_webview2(url: &str) -> Cow<'_, str> {
    const TREK: &str = "trek://";
    match url.get(..TREK.len()) {
        Some(scheme) if scheme.eq_ignore_ascii_case(TREK) => Cow::Owned(format!("{WEBVIEW2_TREK}{}", &url[TREK.len()..])),
        _ => Cow::Borrowed(url),
    }
}

fn from_webview2(url: String) -> String {
    match url.strip_prefix(WEBVIEW2_TREK) {
        Some(rest) => format!("trek://{rest}"),
        None => url,
    }
}

/// A URL on its way into a view, in the view's form (see `WEBVIEW2_TREK`).
fn to_view(url: &str) -> Cow<'_, str> {
    if cfg!(windows) { to_webview2(url) } else { Cow::Borrowed(url) }
}

/// A URL a view reported, in Trek's form.
fn from_view(url: String) -> String {
    if cfg!(windows) { from_webview2(url) } else { url }
}

/// Native WKWebView controls wry doesn't wrap.
#[cfg(target_os = "macos")]
mod native {
    use wry::WebViewExtMacOS as _;

    pub fn can_go_back(wv: &wry::WebView) -> bool {
        unsafe { wv.webview().canGoBack() }
    }
    pub fn can_go_forward(wv: &wry::WebView) -> bool {
        unsafe { wv.webview().canGoForward() }
    }
    pub fn is_loading(wv: &wry::WebView) -> bool {
        unsafe { wv.webview().isLoading() }
    }
    pub fn go_back(wv: &wry::WebView) {
        let _ = unsafe { wv.webview().goBack() };
    }
    pub fn go_forward(wv: &wry::WebView) {
        let _ = unsafe { wv.webview().goForward() };
    }
    pub fn stop(wv: &wry::WebView) {
        unsafe { wv.webview().stopLoading() }
    }
    pub fn hard_reload(wv: &wry::WebView) {
        let _ = unsafe { wv.webview().reloadFromOrigin() };
    }
    pub fn window_number(wv: &wry::WebView) -> Option<isize> {
        Some(wv.ns_window().windowNumber())
    }
}

/// Native WebView2 controls wry doesn't wrap. WebView2 has no loading flag to poll; it says
/// itself when a load ends on its error page (`watch`), so `watch_load` doesn't run here.
#[cfg(windows)]
mod native {
    use super::Msg;
    use webview2_com::Microsoft::Web::WebView2::Win32::{COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, ICoreWebView2};
    use webview2_com::{CallDevToolsProtocolMethodCompletedHandler, CapturePreviewCompletedHandler, ContentLoadingEventHandler, HistoryChangedEventHandler};
    use windows_61::Win32::System::Com::{IStream, STREAM_SEEK_SET};
    use windows_61::Win32::UI::Shell::SHCreateMemStream;
    use windows_61::core::{BOOL, PWSTR, w};
    use wry::WebViewExtWindows as _;

    pub fn can_go_back(wv: &wry::WebView) -> bool {
        let mut can = BOOL::default();
        unsafe { wv.webview().CanGoBack(&mut can) }.is_ok() && can.as_bool()
    }
    pub fn can_go_forward(wv: &wry::WebView) -> bool {
        let mut can = BOOL::default();
        unsafe { wv.webview().CanGoForward(&mut can) }.is_ok() && can.as_bool()
    }
    pub fn is_loading(_: &wry::WebView) -> bool {
        false
    }
    pub fn go_back(wv: &wry::WebView) {
        let _ = unsafe { wv.webview().GoBack() };
    }
    pub fn go_forward(wv: &wry::WebView) {
        let _ = unsafe { wv.webview().GoForward() };
    }
    pub fn stop(wv: &wry::WebView) {
        let _ = unsafe { wv.webview().Stop() };
    }
    /// Reload past the cache, as WKWebView's `reloadFromOrigin` does.
    pub fn hard_reload(wv: &wry::WebView) {
        let done = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(())));
        if unsafe { wv.webview().CallDevToolsProtocolMethod(w!("Page.reload"), w!(r#"{"ignoreCache":true}"#), &done) }.is_err() {
            let _ = wv.reload();
        }
    }

    fn source(wv: &ICoreWebView2) -> String {
        let mut url = PWSTR::null();
        match unsafe { wv.Source(&mut url) } {
            Ok(()) => webview2_com::take_pwstr(url),
            Err(_) => String::new(),
        }
    }

    /// Report what the page-load handler doesn't: a load that ended on WebView2's own error page
    /// (Trek shows its error page instead, as on macOS), and history moving within a page (a
    /// fragment, `pushState`), which changes back and forward without a new load.
    pub fn watch(wv: &wry::WebView, tab: u64, tx: &async_channel::Sender<Msg>) {
        let webview = wv.webview();
        let failed = tx.clone();
        let on_error = ContentLoadingEventHandler::create(Box::new(move |wv, args| {
            let (Some(wv), Some(args)) = (wv, args) else { return Ok(()) };
            let mut error = BOOL::default();
            unsafe { args.IsErrorPage(&mut error)? };
            if error.as_bool() {
                let _ = failed.try_send(Msg::Failed { tab, url: source(&wv) });
            }
            Ok(())
        }));
        let moved = tx.clone();
        let on_history = HistoryChangedEventHandler::create(Box::new(move |_, _| {
            let _ = moved.try_send(Msg::History(tab));
            Ok(())
        }));
        let mut token = 0;
        unsafe {
            let _ = webview.add_ContentLoading(&on_error, &mut token);
            let _ = webview.add_HistoryChanged(&on_history, &mut token);
        }
    }

    /// The page as the view shows it: PNG bytes at the view's size in device pixels, which is what
    /// the macOS crop of the window comes to. Answers once WebView2 has drawn it (on this thread's
    /// message loop); a view closed first drops the sender, so the receiver ends instead of hanging.
    pub fn capture_png(wv: &wry::WebView) -> async_channel::Receiver<Result<Vec<u8>, String>> {
        let (tx, rx) = async_channel::bounded(1);
        let started = (|| -> Result<(), String> {
            let stream = unsafe { SHCreateMemStream(None) }.ok_or("Couldn't make room for the screenshot.")?;
            let (out, done) = (stream.clone(), tx.clone());
            let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
                let png = result.map_err(|e| e.message()).and_then(|()| read_all(&out));
                let _ = done.try_send(png);
                Ok(())
            }));
            unsafe { wv.webview().CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, &stream, &handler) }.map_err(|e| e.message())
        })();
        if let Err(e) = started {
            let _ = tx.try_send(Err(e));
        }
        rx
    }

    fn read_all(stream: &IStream) -> Result<Vec<u8>, String> {
        unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }.map_err(|e| e.message())?;
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            unsafe { stream.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read)) }.ok().map_err(|e| e.message())?;
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read as usize]);
        }
        if out.is_empty() { Err("The page didn't draw.".into()) } else { Ok(out) }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod native {
    pub fn can_go_back(_: &wry::WebView) -> bool {
        true
    }
    pub fn can_go_forward(_: &wry::WebView) -> bool {
        true
    }
    pub fn is_loading(_: &wry::WebView) -> bool {
        false
    }
    pub fn go_back(wv: &wry::WebView) {
        let _ = wv.evaluate_script("history.back()");
    }
    pub fn go_forward(wv: &wry::WebView) {
        let _ = wv.evaluate_script("history.forward()");
    }
    pub fn stop(wv: &wry::WebView) {
        let _ = wv.evaluate_script("window.stop()");
    }
    pub fn hard_reload(wv: &wry::WebView) {
        let _ = wv.reload();
    }
    pub fn window_number(_: &wry::WebView) -> Option<isize> {
        None
    }
}

/// Events from the native views, bridged onto the GPUI thread.
enum Msg {
    Title(u64, String),
    Load { tab: u64, finished: bool, url: String },
    Ipc { tab: u64, origin: String, body: String },
    NewWindow(String),
    /// WebView2 is showing its own error page for `url`: the load failed.
    #[cfg(windows)]
    Failed { tab: u64, url: String },
    /// History moved without a load (a fragment, `pushState`): back and forward may have changed.
    #[cfg(windows)]
    History(u64),
}

struct Tab {
    id: u64,
    view: Entity<WebView>,
    zoom: f64,
    page: Page,
}

/// What a tab knows of its page, kept up to date from its view's events. Nothing here touches
/// the view, so the bookkeeping is the same on every platform (and tested without one).
struct Page {
    /// The page's URL (`trek://newtab/…` for internal pages).
    url: String,
    /// The address that failed to load, while the error page is showing.
    failed: Option<String>,
    title: String,
    favicon: Option<Arc<Image>>,
    favicon_url: Option<String>,
    loading: bool,
    can_back: bool,
    can_forward: bool,
    /// Hidden until the first page paints, so a new view never flashes white.
    ready: bool,
    /// Navigation bookkeeping for load-failure detection.
    nav_seq: u64,
    committed_seq: u64,
}

impl Page {
    /// A view just created on `url`, its first load under way.
    fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            failed: None,
            title: String::new(),
            favicon: None,
            favicon_url: None,
            loading: true,
            can_back: false,
            can_forward: false,
            ready: false,
            nav_seq: 1,
            committed_seq: 0,
        }
    }

    /// The panel asked for a new load; its number, for `watch_load`.
    fn navigating(&mut self) -> u64 {
        self.nav_seq += 1;
        self.loading = true;
        self.failed = None;
        self.nav_seq
    }

    /// The view started (`finished` false) or finished loading `url`.
    fn loaded(&mut self, finished: bool, url: String) {
        let internal = url.starts_with("trek://");
        if finished {
            self.loading = false;
            self.ready = true;
        } else {
            if host_of(&url) != host_of(&self.url) {
                self.favicon = None;
                self.favicon_url = None;
            }
            self.committed_seq = self.nav_seq;
            self.loading = true;
            if !url.starts_with(ERROR_URL) {
                self.failed = None;
            }
        }
        self.url = url;
        if internal {
            self.title.clear();
        }
    }

    /// Loading `url` failed: the error page takes over. Returns its address, for the view.
    fn failed_to_load(&mut self, url: String) -> String {
        let error_page = format!("{ERROR_URL}#{}", percent_encode(&url));
        self.loading = false;
        self.failed = Some(url);
        self.nav_seq += 1;
        self.ready = true;
        error_page
    }

    /// The user stopped the load.
    fn stopped(&mut self) {
        self.loading = false;
        self.nav_seq += 1;
    }

    fn is_start(&self) -> bool {
        self.url.starts_with("trek://") && self.failed.is_none()
    }

    /// The address shown in the URL bar ("" on the start page).
    fn address(&self) -> String {
        match &self.failed {
            Some(u) => u.clone(),
            None if self.url.starts_with("trek://") => String::new(),
            None => self.url.clone(),
        }
    }

    fn label(&self) -> String {
        if self.is_start() {
            return "New Tab".into();
        }
        if self.failed.is_some() {
            return "Can’t reach page".into();
        }
        if !self.title.trim().is_empty() {
            return self.title.trim().to_string();
        }
        host_of(&self.url).unwrap_or_else(|| self.url.clone())
    }
}

/// How long a page screenshot may take before Trek gives up on it.
#[cfg(windows)]
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// The page screenshots asked for, one at a time. WebView2 never answers one for a view it isn't
/// drawing, so each is given a number and the first of its answer and its timeout wins.
#[cfg(windows)]
#[derive(Default)]
struct Captures {
    last: u64,
    waiting: Option<u64>,
}

#[cfg(windows)]
impl Captures {
    /// A screenshot starts; its number.
    fn begin(&mut self) -> u64 {
        self.last += 1;
        self.waiting = Some(self.last);
        self.last
    }

    /// Screenshot `id` answered or ran out of time: whether it was still being waited for.
    fn end(&mut self, id: u64) -> bool {
        self.waiting.take_if(|waiting| *waiting == id).is_some()
    }
}

pub struct BrowserPanel {
    workspace: Entity<Workspace>,
    tabs: Vec<Tab>,
    active: u64,
    next_id: u64,
    address: Entity<InputState>,
    editing: bool,
    /// Set by the right panel: false when the Browser tool isn't on screen.
    visible: bool,
    menu_open: bool,
    picking: bool,
    capturing: bool,
    #[cfg(windows)]
    captures: Captures,
    unavailable: bool,
    tx: async_channel::Sender<Msg>,
    /// CSS custom properties for internal pages, shared with every view's protocol handler.
    page_vars: Rc<RefCell<String>>,
    /// Decoded favicons by URL (`None` when the icon couldn't be fetched or decoded).
    favicons: HashMap<String, Option<Arc<Image>>>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
    (!host.is_empty()).then(|| host.strip_prefix("www.").unwrap_or(host).to_string())
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// "3000" → http://localhost:3000, "example.com" → https://example.com, anything else searches Google.
fn normalize(input: &str) -> String {
    let t = input.trim();
    if t.is_empty() {
        return START_URL.into();
    }
    if t.chars().all(|c| c.is_ascii_digit()) && t.len() <= 5 {
        return format!("http://localhost:{t}");
    }
    let port = t.strip_prefix(':').map(|r| r.split(['/', '?', '#']).next().unwrap_or(""));
    if port.is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        return format!("http://localhost{t}");
    }
    if ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"].iter().any(|h| t.starts_with(h)) {
        return format!("http://{t}");
    }
    if t.contains("://") || t.starts_with("about:") || t.starts_with("data:") {
        return t.to_string();
    }
    let host = t.split(['/', '?', '#']).next().unwrap_or("");
    let looks_like_host = !t.contains(char::is_whitespace) && (host.contains('.') && !host.ends_with('.') || host.contains(':'));
    if looks_like_host {
        return format!("https://{t}");
    }
    format!("https://www.google.com/search?q={}", percent_encode(t))
}

fn css_color(c: Hsla) -> String {
    let c = c.to_rgb();
    format!("rgba({:.0},{:.0},{:.0},{:.3})", c.r * 255., c.g * 255., c.b * 255., c.a)
}

/// Trek's theme as CSS variables for the internal pages.
fn page_vars(cx: &App) -> String {
    let theme = cx.theme();
    let dark = theme.mode.is_dark();
    let accent = crate::palette::ember(cx);
    let ok = crate::palette::emerald(cx);
    let fg = theme.foreground;
    [
        ("--bg", css_color(theme.background)),
        ("--fg", css_color(fg)),
        ("--muted", css_color(theme.muted_foreground)),
        ("--border", css_color(theme.border)),
        ("--border-strong", css_color(fg.opacity(0.16))),
        ("--card", css_color(fg.opacity(if dark { 0.035 } else { 0.025 }))),
        ("--card-strong", css_color(fg.opacity(if dark { 0.055 } else { 0.035 }))),
        ("--chip", css_color(fg.opacity(0.07))),
        ("--accent", css_color(accent)),
        ("--accent-soft", css_color(accent.opacity(0.6))),
        ("--accent-ring", css_color(accent.opacity(0.16))),
        ("--ok", css_color(ok)),
        ("--ok-ring", css_color(ok.opacity(0.18))),
        ("color-scheme", (if dark { "dark" } else { "light" }).to_string()),
    ]
    .iter()
    .map(|(k, v)| format!("{k}:{v};"))
    .collect()
}

fn internal_page(path: &str, vars: &str) -> (&'static str, Cow<'static, [u8]>) {
    match path {
        "/mark.png" => ("image/png", Cow::Owned(crate::assets::brand_bytes("brand/mark.png").unwrap_or_default())),
        _ => ("text/html; charset=utf-8", Cow::Owned(START_HTML.replace("/*VARS*/", vars).into_bytes())),
    }
}

/// Probe common dev ports on loopback and read each server's <title>.
fn probe_servers() -> Vec<(u16, Option<String>)> {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    let probe = |port: u16| -> Option<(u16, Option<String>)> {
        let addrs: [SocketAddr; 2] = [([127, 0, 0, 1], port).into(), (std::net::Ipv6Addr::LOCALHOST, port).into()];
        let mut stream = addrs.iter().find_map(|a| TcpStream::connect_timeout(a, Duration::from_millis(150)).ok())?;
        let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));
        let req = format!("GET / HTTP/1.0\r\nHost: localhost:{port}\r\nAccept: text/html\r\nUser-Agent: Trek\r\nConnection: close\r\n\r\n");
        if stream.write_all(req.as_bytes()).is_err() {
            return Some((port, None));
        }
        let mut buf = Vec::new();
        let _ = stream.take(96 * 1024).read_to_end(&mut buf);
        let text = String::from_utf8_lossy(&buf);
        // macOS's AirPlay receiver squats on 5000/7000; it isn't a dev server.
        if text.lines().take(20).any(|l| l.to_ascii_lowercase().starts_with("server: airtunes")) {
            return None;
        }
        Some((port, extract_title(&text)))
    };
    let handles: Vec<_> = DEV_PORTS.iter().map(|&p| std::thread::spawn(move || probe(p))).collect();
    let mut found: Vec<_> = handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect();
    found.sort_by_key(|(p, _)| *p);
    found
}

/// The largest icon Trek downloads.
const FAVICON_MAX_BYTES: usize = 1_000_000;

/// The icon to show for a page: the one it names, if that's on the web, else its site's
/// `/favicon.ico`. None for Trek's own pages, and for icons only the page can reach (`data:`,
/// `blob:`), which the page script never names anyway.
fn favicon_url(icon: &str, page: &str) -> Option<String> {
    if page.starts_with("trek://") {
        return None;
    }
    if icon.starts_with("http://") || icon.starts_with("https://") {
        return Some(icon.to_string());
    }
    if !icon.is_empty() {
        return None;
    }
    let (scheme, rest) = page.split_once("://").filter(|(s, _)| *s == "http" || *s == "https")?;
    let host = rest.split(['/', '?', '#']).next().filter(|h| !h.is_empty())?;
    Some(format!("{scheme}://{host}/favicon.ico"))
}

/// Fetch an icon (six seconds, `FAVICON_MAX_BYTES` at most, redirects followed) on Trek's tokio
/// runtime; the bytes come back over the channel.
fn fetch_favicon(url: String) -> async_channel::Receiver<Option<Vec<u8>>> {
    let (tx, rx) = async_channel::bounded(1);
    trek_core::runtime().spawn(async move {
        let _ = tx.send(download_favicon(&url).await).await;
    });
    rx
}

async fn download_favicon(url: &str) -> Option<Vec<u8>> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(6)).user_agent("Mozilla/5.0 Trek").build().ok()?;
    let mut response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    if response.content_length().is_some_and(|n| n > FAVICON_MAX_BYTES as u64) {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        body.extend_from_slice(&chunk);
        if body.len() > FAVICON_MAX_BYTES {
            return None;
        }
    }
    (!body.is_empty()).then_some(body)
}

/// The image in an icon download, by its first bytes (servers often send the wrong type).
fn decode_favicon(b: Vec<u8>) -> Option<Image> {
    let format = if b.starts_with(b"\x89PNG") {
        ImageFormat::Png
    } else if b.starts_with(&[0xFF, 0xD8]) {
        ImageFormat::Jpeg
    } else if b.starts_with(b"GIF8") {
        ImageFormat::Gif
    } else if b.len() > 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" {
        ImageFormat::Webp
    } else if b.starts_with(&[0, 0, 1, 0]) {
        ImageFormat::Ico
    } else if b.starts_with(b"BM") {
        ImageFormat::Bmp
    } else if String::from_utf8_lossy(&b[..b.len().min(1024)]).contains("<svg") {
        ImageFormat::Svg
    } else {
        return None;
    };
    Some(Image::from_bytes(format, b))
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = start + lower[start..].find('>')? + 1;
    let close = open_end + lower[open_end..].find("</title")?;
    let raw = html.get(open_end..close)?.trim();
    let title = raw
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!title.is_empty()).then_some(title)
}

/// Where WebView2 keeps its profile (cookies, cache): `%LOCALAPPDATA%\Trek\WebView2`, beside the
/// logs, or in the data folder a test or `TREK_DATA_DIR` moved. Left to itself it would write
/// next to `trek.exe`, a folder an update replaces and that may not be writable.
#[cfg(windows)]
fn webview_data_dir() -> PathBuf {
    if trek_core::paths::isolated() {
        return trek_core::paths::data_dir().join("WebView2");
    }
    crate::logging::log_dir().with_file_name("WebView2")
}

/// Capture `rect` (window points) of a window by number into a PNG at `dest`.
#[cfg(not(windows))]
fn capture_window_rect(window_number: isize, window_width: f32, rect: Bounds<Pixels>, dest: &std::path::Path) -> Result<(), String> {
    use std::io::BufReader;
    let tmp = dest.with_extension("full.png");
    let status = std::process::Command::new("/usr/sbin/screencapture")
        .args(["-x", "-o", &format!("-l{window_number}")])
        .arg(&tmp)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() || !tmp.exists() {
        return Err("Screen capture failed — check Screen Recording permission for Trek.".into());
    }
    let result = (|| {
        let file = std::fs::File::open(&tmp).map_err(|e| e.to_string())?;
        let mut decoder = png::Decoder::new(BufReader::new(file));
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        let scale = info.width as f32 / window_width.max(1.);
        let bpp = info.line_size / info.width as usize;
        let x0 = ((f32::from(rect.origin.x) * scale).round().max(0.) as u32).min(info.width);
        let y0 = ((f32::from(rect.origin.y) * scale).round().max(0.) as u32).min(info.height);
        let w = ((f32::from(rect.size.width) * scale).round() as u32).min(info.width - x0);
        let h = ((f32::from(rect.size.height) * scale).round() as u32).min(info.height - y0);
        if w == 0 || h == 0 {
            return Err("Nothing to capture.".to_string());
        }
        let mut out = Vec::with_capacity((w * h) as usize * bpp);
        for row in y0..y0 + h {
            let start = row as usize * info.line_size + x0 as usize * bpp;
            out.extend_from_slice(&buf[start..start + w as usize * bpp]);
        }
        let file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        encoder.set_color(info.color_type);
        encoder.set_depth(info.bit_depth);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&out).map_err(|e| e.to_string())?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

impl BrowserPanel {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Search or enter address"));
        let (tx, rx) = async_channel::unbounded::<Msg>();
        let pump = cx.spawn_in(window, async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                if this.update_in(cx, |this, window, cx| this.handle(msg, window, cx)).is_err() {
                    break;
                }
            }
        });
        let subscriptions = vec![cx.subscribe_in(&address, window, |this: &mut Self, input, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => {
                let url = normalize(&input.read(cx).value());
                this.editing = false;
                this.navigate(url, cx);
                // WKWebView takes the keys once an address is entered. WebView2 would keep them from
                // Trek's own shortcuts (Ctrl+K, Ctrl+W…: none reach Trek while the page has them)
                // until the page is clicked, so on Windows they stay where they were.
                #[cfg(not(windows))]
                {
                    if let Some(tab) = this.active_tab() {
                        let _ = tab.view.read(cx).raw().focus();
                    }
                }
                let _ = window;
                cx.notify();
            }
            InputEvent::Blur => {
                this.editing = false;
                cx.notify();
            }
            _ => {}
        })];
        let mut this = Self {
            workspace,
            tabs: vec![],
            active: 0,
            next_id: 1,
            address,
            editing: false,
            visible: true,
            menu_open: false,
            picking: false,
            capturing: false,
            #[cfg(windows)]
            captures: Captures::default(),
            unavailable: false,
            tx,
            page_vars: Rc::new(RefCell::new(page_vars(cx))),
            favicons: HashMap::new(),
            _tasks: vec![pump],
            _subscriptions: subscriptions,
        };
        this.new_tab(START_URL, window, cx);
        this
    }

    /// No web view could be made, so the tool says so (and on Windows offers WebView2).
    #[cfg(test)]
    pub(crate) fn unavailable(&self) -> bool {
        self.unavailable && self.tabs.is_empty()
    }

    /// The native view floats above GPUI; hide it whenever its tab isn't on screen.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible {
            self.menu_open = false;
        }
        self.sync_views(cx);
    }

    /// Show exactly one native view: the active tab's, when nothing GPUI-drawn needs that space.
    fn sync_views(&mut self, cx: &mut Context<Self>) {
        let show_active = self.visible && !self.menu_open;
        for tab in &self.tabs {
            let show = show_active && tab.page.ready && tab.id == self.active;
            tab.view.update(cx, |v, _| {
                if show && !v.visible() {
                    v.show();
                } else if !show && v.visible() {
                    v.hide();
                }
            });
        }
        cx.notify();
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == self.active)
    }

    fn tab_mut(&mut self, id: u64) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    fn build_view(&self, id: u64, url: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<WebView>> {
        let built = {
            use raw_window_handle::HasWindowHandle;
            let handle = window.window_handle().ok()?;
            let (t1, t2, t3, t4) = (self.tx.clone(), self.tx.clone(), self.tx.clone(), self.tx.clone());
            let vars = self.page_vars.clone();
            #[cfg(windows)]
            let mut context = wry::WebContext::new(Some(webview_data_dir()));
            // A new view mustn't take the keys from Trek (WebView2 focuses it by default): it gets
            // them when the page is clicked, or after the address bar sends it somewhere.
            #[cfg(windows)]
            let builder = wry::WebViewBuilder::new_with_web_context(&mut context).with_focused(false);
            #[cfg(not(windows))]
            let builder = wry::WebViewBuilder::new();
            builder
                .with_url(to_view(url))
                .with_visible(false)
                .with_devtools(true)
                .with_accept_first_mouse(true)
                .with_back_forward_navigation_gestures(true)
                .with_initialization_script_for_main_only(INIT_JS, true)
                .with_custom_protocol(PAGES_SCHEME.into(), move |_, request| {
                    let (mime, body) = internal_page(request.uri().path(), &vars.borrow());
                    wry::http::Response::builder().header("Content-Type", mime).header("Cache-Control", "no-store").body(body).unwrap_or_default()
                })
                .with_ipc_handler(move |request| {
                    let origin = request.uri().to_string();
                    let _ = t1.try_send(Msg::Ipc { tab: id, origin, body: request.into_body() });
                })
                .with_document_title_changed_handler(move |title| {
                    let _ = t2.try_send(Msg::Title(id, title));
                })
                .with_on_page_load_handler(move |event, url| {
                    let _ = t3.try_send(Msg::Load { tab: id, finished: matches!(event, wry::PageLoadEvent::Finished), url });
                })
                .with_new_window_req_handler(move |url, _| {
                    let _ = t4.try_send(Msg::NewWindow(url));
                    wry::NewWindowResponse::Deny
                })
                .build_as_child(&handle)
                .inspect_err(|e| tracing::warn!("browser: no web view: {e}"))
                .ok()?
        };
        #[cfg(windows)]
        native::watch(&built, id, &self.tx);
        Some(cx.new(|cx| WebView::new(built, window, cx)))
    }

    fn new_tab(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.next_id;
        self.next_id += 1;
        let Some(view) = self.build_view(id, url, window, cx) else {
            self.unavailable = true;
            cx.notify();
            return;
        };
        self.stop_picking(cx);
        self.tabs.push(Tab { id, view, zoom: 1.0, page: Page::new(url) });
        self.active = id;
        // Reveal after a moment even if the first load never reports in.
        self._tasks.push(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(tab) = this.tab_mut(id).filter(|t| !t.page.ready) {
                    tab.page.ready = true;
                    this.sync_views(cx);
                }
            });
        }));
        self.watch_load(id, 1, url.to_string(), cx);
        if url == START_URL {
            self.start_editing(window, cx);
        } else {
            self.editing = false;
        }
        self.sync_views(cx);
    }

    fn activate(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.active == id {
            return;
        }
        self.stop_picking(cx);
        self.active = id;
        self.editing = false;
        self.sync_views(cx);
    }

    fn close_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.tabs.iter().position(|t| t.id == id) else { return };
        if self.active == id {
            self.stop_picking(cx);
        }
        let tab = self.tabs.remove(ix);
        tab.view.update(cx, |v, _| v.hide());
        if self.tabs.is_empty() {
            self.new_tab(START_URL, window, cx);
            return;
        }
        if self.active == id {
            self.active = self.tabs[ix.min(self.tabs.len() - 1)].id;
            self.editing = false;
        }
        self.sync_views(cx);
    }

    /// Load `url` in the active tab and watch for it failing to load.
    fn navigate(&mut self, url: String, cx: &mut Context<Self>) {
        self.stop_picking(cx);
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == self.active) else { return };
        let (id, seq) = (tab.id, tab.page.navigating());
        let sent = tab.view.read(cx).raw().load_url(&to_view(&url));
        // WebView2 turns an address it can't parse down at once, and then reports nothing.
        if sent.is_err() && cfg!(windows) {
            self.show_load_error(id, url, cx);
        } else {
            self.watch_load(id, seq, url, cx);
        }
        cx.notify();
    }

    /// Loading `url` in tab `id` failed: Trek's error page takes over.
    fn show_load_error(&mut self, id: u64, url: String, cx: &mut Context<Self>) {
        if let Some(tab) = self.tab_mut(id) {
            let error_page = tab.page.failed_to_load(url);
            let _ = tab.view.read(cx).raw().load_url(&to_view(&error_page));
        }
        self.sync_views(cx);
    }

    /// WKWebView shows nothing when a load fails, so notice it and show Trek's error page.
    /// (WebView2 shows an error page of its own, and says so: see `native::watch`.)
    fn watch_load(&mut self, id: u64, seq: u64, url: String, cx: &mut Context<Self>) {
        if url.starts_with("trek://") || cfg!(windows) {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let mut seen_loading = false;
            loop {
                cx.background_executor().timer(Duration::from_millis(150)).await;
                let state = this.update(cx, |this, cx| {
                    let tab = this.tab_mut(id)?;
                    if tab.page.nav_seq != seq || tab.page.committed_seq >= seq {
                        return None;
                    }
                    Some(native::is_loading(tab.view.read(cx).raw()))
                });
                let Ok(Some(loading)) = state else { return };
                seen_loading |= loading;
                if loading && started.elapsed() < Duration::from_secs(90) || !seen_loading && started.elapsed() < Duration::from_millis(1500) {
                    continue;
                }
                let _ = this.update(cx, |this, cx| this.show_load_error(id, url, cx));
                return;
            }
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    fn refresh_nav(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else { return };
        let raw = tab.view.read(cx).raw();
        tab.page.can_back = native::can_go_back(raw);
        tab.page.can_forward = native::can_go_forward(raw);
    }

    fn handle(&mut self, msg: Msg, window: &mut Window, cx: &mut Context<Self>) {
        match msg {
            Msg::Title(id, title) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.page.title = title;
                }
            }
            Msg::Load { tab: id, finished, url } => {
                let Some(tab) = self.tab_mut(id) else { return };
                tab.page.loaded(finished, from_view(url));
                if !finished && self.active == id {
                    self.picking = false;
                }
                self.refresh_nav(id, cx);
                self.sync_views(cx);
            }
            Msg::NewWindow(url) => {
                let url = from_view(url);
                if !url.is_empty() && url != "about:blank" {
                    self.new_tab(&url, window, cx);
                }
            }
            Msg::Ipc { tab, origin, body } => self.handle_ipc(tab, from_view(origin), body, window, cx),
            #[cfg(windows)]
            Msg::Failed { tab: id, url } => {
                let Some(tab) = self.tab_mut(id) else { return };
                let url = if url.is_empty() { tab.page.url.clone() } else { from_view(url) };
                self.show_load_error(id, url, cx);
            }
            #[cfg(windows)]
            Msg::History(id) => self.refresh_nav(id, cx),
        }
        cx.notify();
    }

    fn handle_ipc(&mut self, id: u64, origin: String, body: String, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&body) else { return };
        let str_of = |k: &str| msg.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let internal = origin.starts_with("trek://");
        match msg.get("t").and_then(|v| v.as_str()).unwrap_or_default() {
            "meta" => {
                if let Some(icon) = favicon_url(&str_of("icon"), &origin) {
                    self.load_favicon(id, icon, cx);
                }
                if let Some(tab) = self.tab_mut(id) {
                    let title = str_of("title");
                    if !title.is_empty() && !internal {
                        tab.page.title = title;
                    }
                }
            }
            "url" => {
                let url = str_of("url");
                if let Some(tab) = self.tab_mut(id) {
                    if !url.is_empty() && !internal {
                        tab.page.url = url;
                    }
                }
                self.refresh_nav(id, cx);
            }
            // The start page's search box, server rows, quick links and error page buttons.
            "go" if internal => {
                if self.active == id {
                    self.navigate(normalize(&str_of("q")), cx);
                }
            }
            "servers" if internal => self.scan_servers(id, cx),
            "pick" if self.picking && self.active == id => {
                self.picking = false;
                let text = describe_pick(&msg);
                self.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::InsertIntoComposer(text)));
            }
            "pick-cancel" => self.picking = false,
            _ => {}
        }
        let _ = window;
        cx.notify();
    }

    /// GPUI has no HTTP client here, so fetch the icon ourselves and hand it over as image bytes.
    fn load_favicon(&mut self, id: u64, url: String, cx: &mut Context<Self>) {
        let Some(tab) = self.tab_mut(id) else { return };
        if tab.page.favicon_url.as_deref() == Some(url.as_str()) {
            return;
        }
        tab.page.favicon_url = Some(url.clone());
        if let Some(cached) = self.favicons.get(&url).cloned() {
            if let Some(tab) = self.tab_mut(id) {
                tab.page.favicon = cached;
            }
            return;
        }
        let bytes = fetch_favicon(url.clone());
        cx.spawn(async move |this, cx| {
            let image = bytes.recv().await.ok().flatten().and_then(decode_favicon).map(Arc::new);
            let _ = this.update(cx, |this, cx| {
                this.favicons.insert(url.clone(), image.clone());
                for tab in this.tabs.iter_mut().filter(|t| t.page.favicon_url.as_deref() == Some(url.as_str())) {
                    tab.page.favicon = image.clone();
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn scan_servers(&mut self, id: u64, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move { probe_servers() }).await;
            let list: Vec<serde_json::Value> = found.into_iter().map(|(port, title)| serde_json::json!({ "port": port, "title": title })).collect();
            let json = serde_json::Value::Array(list).to_string();
            let _ = this.update(cx, |this, cx| {
                let Some(tab) = this.tab_mut(id) else { return };
                let raw = tab.view.read(cx).raw();
                // Only hand the list to Trek's own page, never to a site the tab has since moved to.
                if raw.url().map(|u| from_view(u).starts_with("trek://")).unwrap_or(false) {
                    let _ = raw.evaluate_script(&format!("window.__trekServers && window.__trekServers({json});"));
                }
            });
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    fn start_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.active_tab().map(|t| t.page.address()).unwrap_or_default();
        self.editing = true;
        self.address.update(cx, |s, cx| {
            s.set_value(value, window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    fn with_active(&self, cx: &App, f: impl FnOnce(&wry::WebView)) {
        if let Some(tab) = self.active_tab() {
            f(tab.view.read(cx).raw());
        }
    }

    fn reload_or_stop(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else { return };
        if tab.page.loading {
            native::stop(tab.view.read(cx).raw());
            let id = tab.id;
            if let Some(tab) = self.tab_mut(id) {
                tab.page.stopped();
            }
        } else if let Some(url) = tab.page.failed.clone() {
            self.navigate(url, cx);
        } else {
            let _ = tab.view.read(cx).raw().reload();
        }
        cx.notify();
    }

    fn toggle_picking(&mut self, cx: &mut Context<Self>) {
        if self.picking {
            self.stop_picking(cx);
        } else if self.active_tab().is_some_and(|t| !t.page.url.starts_with("trek://")) {
            self.picking = true;
            self.with_active(cx, |wv| {
                let _ = wv.evaluate_script(PICK_JS);
                let _ = wv.focus();
            });
        }
        cx.notify();
    }

    fn stop_picking(&mut self, cx: &mut Context<Self>) {
        if self.picking {
            self.picking = false;
            self.with_active(cx, |wv| {
                let _ = wv.evaluate_script(PICK_STOP_JS);
            });
        }
    }

    /// WebView2 draws the page into a PNG itself, at the size the macOS window crop comes to.
    #[cfg(windows)]
    fn screenshot(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else { return };
        // WebView2 draws nothing for a view that's hidden, and never says so.
        if self.capturing || !self.visible || !tab.page.ready || !tab.view.read(cx).visible() {
            return;
        }
        let png = native::capture_png(tab.view.read(cx).raw());
        let id = self.captures.begin();
        self.capturing = true;
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let dest: PathBuf = crate::mentions::snapshot_path();
            let result = match png.recv().await {
                Ok(Ok(bytes)) => {
                    let out = dest.clone();
                    cx.background_executor().spawn(async move { std::fs::write(&out, bytes).map_err(|e| e.to_string()) }).await.map(|()| dest)
                }
                Ok(Err(message)) => Err(format!("Couldn't capture the page: {message}")),
                Err(_) => Err("The page closed before it was captured.".to_string()),
            };
            let _ = this.update(cx, |this, cx| this.captured(id, result, cx));
        });
        // A view that stops drawing mid-capture (a crashed page, a minimised window) never answers:
        // give up, or the button would spin for good.
        let timeout = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CAPTURE_TIMEOUT).await;
            let _ = this.update(cx, |this, cx| this.captured(id, Err("Couldn't capture the page: it didn't draw.".to_string()), cx));
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
        self._tasks.push(timeout);
    }

    /// Screenshot `id` ended (an image to attach, or why not), unless it already did.
    #[cfg(windows)]
    fn captured(&mut self, id: u64, result: Result<PathBuf, String>, cx: &mut Context<Self>) {
        if !self.captures.end(id) {
            return;
        }
        self.capturing = false;
        let event = match result {
            Ok(dest) => WorkspaceEvent::AttachImage(dest),
            Err(message) => WorkspaceEvent::Toast { message, undo: None },
        };
        self.workspace.update(cx, |_, cx| cx.emit(event));
        cx.notify();
    }

    #[cfg(not(windows))]
    fn screenshot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else { return };
        if self.capturing || !self.visible {
            return;
        }
        let view = tab.view.read(cx);
        let rect = view.bounds();
        let Some(number) = native::window_number(view.raw()) else { return };
        if rect.size.width <= px(0.) || rect.size.height <= px(0.) {
            return;
        }
        let window_width = f32::from(window.bounds().size.width);
        let dest: PathBuf = crate::mentions::snapshot_path();
        self.capturing = true;
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let out = dest.clone();
            let result = cx.background_executor().spawn(async move { capture_window_rect(number, window_width, rect, &out) }).await;
            let _ = this.update(cx, |this, cx| {
                this.capturing = false;
                let event = match result {
                    Ok(()) => WorkspaceEvent::AttachImage(dest),
                    Err(message) => WorkspaceEvent::Toast { message, undo: None },
                };
                this.workspace.update(cx, |_, cx| cx.emit(event));
                cx.notify();
            });
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    fn set_zoom(&mut self, zoom: f64, cx: &mut Context<Self>) {
        let id = self.active;
        if let Some(tab) = self.tab_mut(id) {
            tab.zoom = zoom.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
            let z = tab.zoom;
            let _ = tab.view.read(cx).raw().zoom(z);
        }
        cx.notify();
    }

    fn step_zoom(&mut self, up: bool, cx: &mut Context<Self>) {
        let current = self.active_tab().map(|t| t.zoom).unwrap_or(1.0);
        let next = if up {
            ZOOM_STEPS.iter().copied().find(|z| *z > current + 0.001)
        } else {
            ZOOM_STEPS.iter().rev().copied().find(|z| *z < current - 0.001)
        };
        if let Some(z) = next {
            self.set_zoom(z, cx);
        }
    }

    fn set_menu_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.menu_open = open;
        if open {
            self.stop_picking(cx);
        }
        self.sync_views(cx);
    }

    /// Keep internal pages in step with Trek's theme.
    fn sync_theme(&mut self, cx: &mut Context<Self>) {
        let vars = page_vars(cx);
        if *self.page_vars.borrow() == vars {
            return;
        }
        *self.page_vars.borrow_mut() = vars.clone();
        for tab in &self.tabs {
            let raw = tab.view.read(cx).raw();
            if raw.url().map(|u| from_view(u).starts_with("trek://")).unwrap_or(false) {
                let _ = raw.evaluate_script(&format!("document.documentElement.style.cssText = {};", serde_json::Value::String(vars.clone())));
            }
        }
    }

    fn more_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let (is_page, zoom) = self.active_tab().map(|t| (!t.page.url.starts_with("trek://"), t.zoom)).unwrap_or((false, 1.0));
        let row = |id: &'static str, icon: Icon, label: &'static str, enabled: bool, cx: &mut Context<Self>, f: fn(&mut Self, &mut Context<Self>)| {
            ui::menu_row(id, false, cx)
                .child(icon.small().text_color(muted))
                .child(div().flex_1().child(label))
                .when(!enabled, |el| el.opacity(0.45).cursor_default())
                .when(enabled, |el| {
                    el.on_click(cx.listener(move |this, _, _, cx| {
                        f(this, cx);
                        this.set_menu_open(false, cx);
                    }))
                })
                .into_any_element()
        };
        let zoom_button = |id: &'static str, icon: Lucide, up: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.foreground.opacity(0.08)))
                .child(Icon::new(icon).xsmall())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.step_zoom(up, cx);
                }))
        };
        let zoom_row = h_flex()
            .px(px(10.))
            .h(px(34.))
            .gap(px(10.))
            .text_sm()
            .child(Icon::new(Lucide::ZoomIn).small().text_color(muted))
            .child(div().flex_1().child("Zoom"))
            .child(zoom_button("browser-zoom-out", Lucide::Minus, false, cx))
            .child(
                div()
                    .id("browser-zoom-reset")
                    .w(px(44.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .text_xs()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.foreground.opacity(0.08)))
                    .child(format!("{:.0}%", zoom * 100.))
                    .on_click(cx.listener(|this, _, _, cx| this.set_zoom(1.0, cx))),
            )
            .child(zoom_button("browser-zoom-in", Lucide::Plus, true, cx))
            .into_any_element();
        let separator = || div().my(px(4.)).mx(px(6.)).h(px(1.)).bg(theme.border).into_any_element();

        ui::menu_surface(cx)
            .w(px(250.))
            .child(row("browser-copy-url", Icon::new(Lucide::Copy), "Copy URL", is_page, cx, |this, cx| {
                if let Some(tab) = this.active_tab() {
                    cx.write_to_clipboard(ClipboardItem::new_string(tab.page.address()));
                }
            }))
            .child(row("browser-hard-reload", Icon::new(Lucide::RotateCw), "Hard reload", is_page, cx, |this, cx| {
                this.with_active(cx, native::hard_reload);
            }))
            .child(separator())
            .child(zoom_row)
            .child(separator())
            .child(row("browser-clear-data", Icon::new(Lucide::Eraser), "Clear cookies and site data", true, cx, |this, cx| {
                this.with_active(cx, |wv| {
                    let _ = wv.clear_all_browsing_data();
                });
                this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message: "Cleared browsing data".into(), undo: None }));
            }))
            .into_any_element()
    }

    fn render_tab(&self, tab: &Tab, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = tab.id;
        let active = id == self.active;
        let muted = theme.muted_foreground;
        let page = &tab.page;
        let icon: AnyElement = if page.loading && !page.is_start() {
            Spinner::new().xsmall().color(muted).into_any_element()
        } else if page.is_start() {
            Icon::new(Lucide::Sparkle).xsmall().text_color(crate::palette::ember(cx)).into_any_element()
        } else if let Some(image) = page.favicon.clone() {
            img(image)
                .size(px(14.))
                .flex_none()
                .rounded(px(3.))
                .with_fallback(move || Icon::new(Lucide::Globe).xsmall().text_color(muted).into_any_element())
                .into_any_element()
        } else {
            Icon::new(Lucide::Globe).xsmall().text_color(muted).into_any_element()
        };
        h_flex()
            .id(("browser-tab", id as usize))
            .group("browser-tab")
            .relative()
            .flex_1()
            .min_w(px(72.))
            .max_w(px(220.))
            .h(px(28.))
            .pl(px(10.))
            .pr(px(4.))
            .gap(px(7.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(12.5))
            .border_1()
            .when(active, |el| el.bg(theme.popover).border_color(theme.border).text_color(theme.foreground).shadow_xs())
            .when(!active, |el| el.border_color(transparent_black()).text_color(muted).hover(|s| s.bg(theme.foreground.opacity(0.05)).text_color(theme.foreground)))
            .child(div().flex_none().size(px(14.)).flex().items_center().justify_center().child(icon))
            .child(div().flex_1().min_w_0().truncate().child(page.label()))
            .child(
                div()
                    .id(("browser-tab-close", id as usize))
                    .flex_none()
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .when(!active, |el| el.invisible().group_hover("browser-tab", |s| s.visible()))
                    .hover(|s| s.bg(theme.foreground.opacity(0.1)))
                    .child(Icon::new(IconName::Close).xsmall())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_tab(id, window, cx);
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.activate(id, cx)))
            .into_any_element()
    }

    fn render_url_pill(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let tab = self.active_tab();
        let zoom = tab.map(|t| t.zoom).unwrap_or(1.0);
        let pill = h_flex()
            .id("browser-url")
            .flex_1()
            .min_w_0()
            .h(px(28.))
            .px(px(10.))
            .gap(px(6.))
            .rounded(px(8.))
            .text_size(px(12.5))
            .border_1();
        if self.editing {
            return pill
                .bg(theme.foreground.opacity(0.08))
                .border_color(crate::palette::ember(cx).opacity(0.55))
                .child(Icon::new(Lucide::Search).xsmall().text_color(muted))
                .child(div().flex_1().min_w_0().child(Input::new(&self.address).small().appearance(false)))
                .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.editing = false;
                        window.blur(cx);
                        cx.notify();
                    }
                }))
                .into_any_element();
        }
        let address = tab.map(|t| t.page.address()).unwrap_or_default();
        let content: AnyElement = if address.is_empty() {
            h_flex()
                .gap(px(6.))
                .child(Icon::new(Lucide::Search).xsmall().text_color(muted))
                .child(div().text_color(muted).child("Search or enter address"))
                .into_any_element()
        } else {
            let secure = address.starts_with("https://");
            let failed = tab.is_some_and(|t| t.page.failed.is_some());
            let (host, rest) = match address.split_once("://") {
                Some((_, after)) => match after.find(['/', '?', '#']) {
                    Some(i) => (after[..i].to_string(), after[i..].to_string()),
                    None => (after.to_string(), String::new()),
                },
                None => (address.clone(), String::new()),
            };
            let rest = if rest == "/" { String::new() } else { rest };
            h_flex()
                .min_w_0()
                .gap(px(6.))
                .child(
                    Icon::new(if failed {
                        Lucide::CircleAlert
                    } else if secure {
                        Lucide::Lock
                    } else {
                        Lucide::Globe
                    })
                    .xsmall()
                    .text_color(if failed { crate::palette::red(cx) } else { muted }),
                )
                .child(
                    h_flex()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(div().flex_none().text_color(theme.foreground).child(host))
                        .child(div().min_w_0().truncate().text_color(muted).child(rest)),
                )
                .into_any_element()
        };
        pill.bg(theme.foreground.opacity(0.06))
            .border_color(transparent_black())
            .cursor_text()
            .hover(|s| s.bg(theme.foreground.opacity(0.09)))
            .child(div().flex_1().min_w_0().overflow_hidden().child(content))
            .when((zoom - 1.0).abs() > 0.001, |el| {
                el.child(
                    div()
                        .id("browser-zoom-chip")
                        .flex_none()
                        .px(px(6.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .text_xs()
                        .text_color(muted)
                        .bg(theme.foreground.opacity(0.07))
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.foreground))
                        .child(format!("{:.0}%", zoom * 100.))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_zoom(1.0, cx);
                        })),
                )
            })
            .on_click(cx.listener(|this, _, window, cx| this.start_editing(window, cx)))
            .into_any_element()
    }
}

/// The shots harness's `browser` command (see `shots.rs`): the tool driven without a pointer,
/// and where its native view sits, for checking it against the panel at any scale.
#[cfg(feature = "shots")]
impl BrowserPanel {
    pub fn shots(&mut self, cmd: &str, arg: &str, cx: &mut Context<Self>) -> anyhow::Result<()> {
        match cmd {
            "go" => self.navigate(normalize(arg), cx),
            "back" => self.with_active(cx, native::go_back),
            "forward" => self.with_active(cx, native::go_forward),
            "reload" => self.reload_or_stop(cx),
            _ => anyhow::bail!("browser: unknown command {cmd:?} (go <address>|back|forward|reload|state|snap <name>)"),
        }
        Ok(())
    }

    /// The active page, the panel's area for the view (logical px), the native view's rectangle
    /// (device px, from the OS), the window's scale, and which window has the keys.
    pub fn shots_state(&self, window: &Window, cx: &App) -> serde_json::Value {
        let scale = window.scale_factor() as f64;
        let tab = self.active_tab();
        let view = tab.map(|t| t.view.read(cx));
        let area = view.map(|v| {
            let b = v.bounds();
            serde_json::json!([f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)])
        });
        let native = view.and_then(|v| v.raw().bounds().ok()).map(|r| {
            let (p, s) = (r.position.to_physical::<i32>(scale), r.size.to_physical::<i32>(scale));
            serde_json::json!([p.x, p.y, s.width, s.height])
        });
        serde_json::json!({
            "tabs": self.tabs.len(),
            "unavailable": self.unavailable,
            "url": tab.map(|t| t.page.url.clone()),
            "label": tab.map(|t| t.page.label()),
            "failed": tab.and_then(|t| t.page.failed.clone()),
            "loading": tab.map(|t| t.page.loading),
            "can_back": tab.map(|t| t.page.can_back),
            "can_forward": tab.map(|t| t.page.can_forward),
            "ready": tab.map(|t| t.page.ready),
            "favicon": tab.map(|t| t.page.favicon.is_some()),
            "panel_visible": self.visible,
            "view_shown": view.map(|v| v.visible()),
            "scale": scale,
            "area": area,
            "native": native,
            "focus": shots_focus(window),
        })
    }

    /// The page as `screenshot` would capture it.
    #[cfg(windows)]
    pub fn shots_capture(&self, cx: &App) -> Option<async_channel::Receiver<Result<Vec<u8>, String>>> {
        self.active_tab().map(|t| native::capture_png(t.view.read(cx).raw()))
    }
}

/// Which window of this thread has the keys: Trek's own (`trek`), one inside it (the web view),
/// or none.
#[cfg(all(feature = "shots", windows))]
fn shots_focus(window: &Window) -> String {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GUITHREADINFO, GetClassNameW, GetGUIThreadInfo};
    let main = match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    };
    unsafe {
        let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..std::mem::zeroed() };
        GetGUIThreadInfo(windows_sys::Win32::System::Threading::GetCurrentThreadId(), &mut info);
        if info.hwndFocus.is_null() {
            return "none".into();
        }
        if info.hwndFocus as isize == main {
            return "trek".into();
        }
        let mut class = [0u16; 128];
        let n = GetClassNameW(info.hwndFocus, class.as_mut_ptr(), class.len() as i32);
        format!("child:{}", String::from_utf16_lossy(&class[..n.max(0) as usize]))
    }
}

#[cfg(all(feature = "shots", not(windows)))]
fn shots_focus(_: &Window) -> String {
    "unknown".into()
}

/// What the agent sees when an element is picked.
fn describe_pick(msg: &serde_json::Value) -> String {
    let get = |k: &str| msg.get(k).and_then(|v| v.as_str()).unwrap_or_default();
    let mut out = format!("Element `{}` on {}\n", get("selector"), get("url"));
    let text = get("text");
    if !text.is_empty() {
        out.push_str(&format!("Text: \"{text}\"\n"));
    }
    out.push_str(&format!("```html\n{}\n```\n", get("html")));
    out
}

/// The line under the toolbar while a page loads: a bar sliding along it, or, with `still` (reduced
/// motion), the whole line, dimmer. Sliding is what says "loading"; a sliding bar held at its
/// start would be off the line altogether.
pub(crate) fn loading_line(still: bool, ember: Hsla) -> impl IntoElement {
    let bar = div().id("browser-loading-bar").test_support().absolute().top_0().h_full().rounded(px(1.)).bg(ember);
    let bar = if still {
        bar.left_0().w_full().opacity(0.45).into_any_element()
    } else {
        bar.w(relative(0.32))
            .with_animation("browser-loading", Animation::new(Duration::from_millis(1100)).repeat().with_easing(ease_in_out), |el, t| el.left(relative(-0.32 + 1.32 * t)))
            .into_any_element()
    };
    div().id("browser-loading").test_support().absolute().left_0().bottom(px(-1.)).w_full().h(px(2.)).overflow_hidden().child(bar)
}

impl Render for BrowserPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_theme(cx);
        let theme = cx.theme().clone();
        if self.unavailable && self.tabs.is_empty() {
            // Windows 11 ships the WebView2 runtime, but it can be removed (and older systems lack it).
            return super::empty(crate::words::words().browser_unavailable, cx)
                .id("browser-unavailable")
                .gap(px(12.))
                .when(cfg!(windows), |el| {
                    el.child(
                        gpui_kit::component::button::Button::new("browser-get-webview2")
                            .small()
                            .label("Get WebView2")
                            .on_click(|_, _, cx| cx.open_url(WEBVIEW2_DOWNLOAD)),
                    )
                })
                .into_any_element();
        }
        let tab = self.active_tab();
        let loading = tab.is_some_and(|t| t.page.loading && !t.page.is_start());
        let still = !self.workspace.read(cx).motion(cx);
        let can_back = tab.is_some_and(|t| t.page.can_back);
        let can_forward = tab.is_some_and(|t| t.page.can_forward);
        let is_page = tab.is_some_and(|t| !t.page.url.starts_with("trek://"));
        let failed = tab.is_some_and(|t| t.page.failed.is_some());
        let ember = crate::palette::ember(cx);
        let active_view = tab.map(|t| t.view.clone());
        // The native view only draws once the first page paints; until then, a hint fills the
        // void. Hidden while a menu is open (or the panel is) is suppression, not empty.
        let blank = tab.is_none_or(|t| !t.page.ready);

        let strip = h_flex()
            .h(px(36.))
            .px(px(6.))
            .gap(px(4.))
            .child(
                h_flex()
                    .id("browser-tabs")
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .overflow_x_scroll()
                    .children(self.tabs.iter().map(|t| self.render_tab(t, cx)).collect::<Vec<_>>())
                    .child(
                        ui::icon_button("browser-new-tab", IconName::Plus, "New tab")
                            .on_click(cx.listener(|this, _, window, cx| this.new_tab(START_URL, window, cx))),
                    ),
            );

        let entity = cx.entity();
        let more = Popover::new("browser-more")
            .anchor(Anchor::TopRight)
            .appearance(false)
            .open(self.menu_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| this.set_menu_open(*open, cx)))
            .trigger(ui::icon_button("browser-more-button", IconName::Ellipsis, "More"))
            .content(move |_, _, cx| entity.update(cx, |p, cx| p.more_menu(cx)));

        let nav_button = |id: &'static str, icon: Lucide, tooltip: &'static str, enabled: bool| ui::icon_button(id, icon, tooltip).disabled(!enabled);
        let toolbar = h_flex()
            .relative()
            .h(px(40.))
            .px(px(6.))
            .gap(px(2.))
            .border_b_1()
            .border_color(theme.border)
            .child(nav_button("browser-back", Lucide::ArrowLeft, "Back", can_back).on_click(cx.listener(|this, _, _, cx| {
                this.stop_picking(cx);
                this.with_active(cx, native::go_back);
            })))
            .child(nav_button("browser-forward", Lucide::ArrowRight, "Forward", can_forward).on_click(cx.listener(|this, _, _, cx| {
                this.stop_picking(cx);
                this.with_active(cx, native::go_forward);
            })))
            .child(
                ui::icon_button("browser-reload", if loading { Lucide::X } else { Lucide::RotateCw }, if loading { "Stop" } else { "Reload" })
                    .on_click(cx.listener(|this, _, _, cx| this.reload_or_stop(cx))),
            )
            .child(div().w(px(4.)))
            .child(self.render_url_pill(cx))
            .child(div().w(px(4.)))
            .child(
                ui::icon_button("browser-pick", Lucide::Crosshair, "Pick an element for the chat")
                    .selected(self.picking)
                    .disabled(!is_page)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_picking(cx))),
            )
            .child(
                ui::icon_button("browser-screenshot", Lucide::Camera, "Screenshot the page into the chat")
                    .loading(self.capturing)
                    .on_click(cx.listener(|this, _, window, cx| this.screenshot(window, cx))),
            )
            .child(ui::icon_button("browser-devtools", Lucide::CodeXml, "Open devtools").on_click(cx.listener(|this, _, _, cx| {
                this.with_active(cx, |wv| wv.open_devtools());
            })))
            .child(
                ui::icon_button("browser-external", Lucide::ExternalLink, "Open in default browser")
                    .disabled(!is_page && !failed)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(url) = this.active_tab().filter(|t| t.page.failed.is_some() || !t.page.url.starts_with("trek://")).map(|t| t.page.address()) {
                            cx.open_url(&url);
                        }
                    })),
            )
            .child(more)
            .when(loading, |el| el.child(loading_line(still, ember)));

        let picking_hint = self.picking.then(|| {
            h_flex()
                .h(px(28.))
                .px(px(12.))
                .gap(px(8.))
                .text_xs()
                .bg(ember.opacity(0.1))
                .border_b_1()
                .border_color(theme.border)
                .child(Icon::new(Lucide::Crosshair).xsmall().text_color(ember))
                .child(div().flex_1().child("Click an element on the page to add it to the chat"))
                .child(div().text_color(theme.muted_foreground).child("Esc to cancel"))
        });

        v_flex()
            .size_full()
            .child(strip)
            .child(toolbar)
            .children(picking_hint)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .bg(theme.background)
                    .when(blank, |el| el.child(super::empty("Search or enter an address", cx).absolute().top_0().left_0()))
                    .children(active_view),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: that would bring GPUI's `test` attribute over the standard one.
    use super::{
        ERROR_URL, FAVICON_MAX_BYTES, Page, START_URL, decode_favicon, download_favicon, extract_title, favicon_url, from_view, from_webview2, host_of, normalize, to_view,
        to_webview2,
    };
    use gpui_kit::ImageFormat;
    #[cfg(windows)]
    use super::{Captures, Msg, PAGES_SCHEME, native};

    #[test]
    fn trek_pages_cross_into_webview2_and_back() {
        assert_eq!(to_webview2("trek://newtab/"), "http://trek-pages.newtab/");
        assert_eq!(to_webview2("trek://newtab/error#http%3A%2F%2Fx"), "http://trek-pages.newtab/error#http%3A%2F%2Fx");
        assert_eq!(from_webview2("http://trek-pages.newtab/mark.png".into()), "trek://newtab/mark.png");
        // A scheme is the same scheme in any case: WebView2 mustn't be handed `TREK://`, which it
        // would take for an app to launch.
        assert_eq!(to_webview2("TREK://newtab/"), "http://trek-pages.newtab/");
        assert_eq!(to_webview2("Trek://newtab/error"), "http://trek-pages.newtab/error");
        // Everything else passes untouched, either way, including sites named like Trek's pages.
        for url in ["https://example.com/", "http://localhost:3000/", "https://trek.dev/", "http://trek.localhost:3000/", "http://trek/", "trek:", "trek:/x", "trek:/é", "tré","about:blank", ""] {
            assert_eq!(to_webview2(url), url);
            assert_eq!(from_webview2(url.into()), url);
        }
        // The platform's own form: a round trip always lands back on Trek's.
        assert_eq!(from_view(to_view(START_URL).into_owned()), START_URL);
        assert_eq!(to_view(START_URL) == START_URL, !cfg!(windows));
    }

    #[cfg(windows)]
    #[test]
    fn a_screenshot_is_given_up_on_once_and_a_late_answer_is_ignored() {
        let mut captures = Captures::default();
        let first = captures.begin();
        assert!(captures.end(first), "the timeout ends it");
        assert!(!captures.end(first), "its late answer finds it gone");
        let second = captures.begin();
        assert!(!captures.end(first), "an old answer doesn't end the next screenshot");
        assert!(captures.end(second));
    }

    #[test]
    fn a_new_tab_is_the_start_page_until_it_loads() {
        let page = Page::new(START_URL);
        assert!(page.is_start() && page.loading && !page.ready);
        assert_eq!((page.address(), page.label()), (String::new(), "New Tab".to_string()));
    }

    #[test]
    fn a_load_reports_its_url_title_and_progress() {
        let mut page = Page::new(START_URL);
        let seq = page.navigating();
        assert_eq!(seq, 2);
        page.loaded(false, "https://www.example.com/docs".into());
        assert!(page.loading && page.committed_seq == seq, "the load committed");
        assert_eq!(page.label(), "example.com", "the host until there's a title");
        page.title = "Example Docs".into();
        page.loaded(true, "https://www.example.com/docs".into());
        assert!(!page.loading && page.ready);
        assert_eq!((page.address(), page.label()), ("https://www.example.com/docs".to_string(), "Example Docs".to_string()));
        // Back to an internal page: its title isn't the site's.
        page.loaded(false, START_URL.into());
        assert!(page.title.is_empty() && page.is_start());
    }

    #[test]
    fn the_favicon_goes_when_the_site_changes() {
        let mut page = Page::new("https://a.dev/");
        page.loaded(false, "https://a.dev/".into());
        page.favicon_url = Some("https://a.dev/favicon.ico".into());
        page.loaded(false, "https://a.dev/next".into());
        assert!(page.favicon_url.is_some(), "same site, same icon");
        page.loaded(false, "https://b.dev/".into());
        assert_eq!(page.favicon_url, None);
    }

    #[test]
    fn a_failed_load_shows_the_error_page_with_the_address_kept() {
        let mut page = Page::new(START_URL);
        page.navigating();
        let error_page = page.failed_to_load("http://localhost:9/app".into());
        assert_eq!(error_page, format!("{ERROR_URL}#http%3A%2F%2Flocalhost%3A9%2Fapp"));
        assert!(!page.loading && page.ready);
        // The error page loading keeps the failure on show.
        page.loaded(false, error_page.clone());
        page.loaded(true, error_page);
        assert!(!page.is_start());
        assert_eq!((page.address(), page.label()), ("http://localhost:9/app".to_string(), "Can’t reach page".to_string()));
        // A new address clears it.
        page.navigating();
        assert_eq!(page.failed, None);
    }

    #[test]
    fn stopping_ends_the_load_and_its_watch() {
        let mut page = Page::new("https://slow.dev/");
        let seq = page.nav_seq;
        page.stopped();
        assert!(!page.loading && page.nav_seq > seq, "the load watcher sees a newer load and stands down");
    }

    #[test]
    fn favicons_come_from_the_page_or_its_site() {
        assert_eq!(favicon_url("https://cdn.x.dev/icon.svg", "https://x.dev/a"), Some("https://cdn.x.dev/icon.svg".into()));
        assert_eq!(favicon_url("", "http://localhost:5173/app?x=1#y"), Some("http://localhost:5173/favicon.ico".into()));
        assert_eq!(favicon_url("", "https://user@x.dev"), Some("https://user@x.dev/favicon.ico".into()));
        assert_eq!(favicon_url("data:image/png;base64,AAAA", "https://x.dev/"), None, "not fetchable");
        assert_eq!(favicon_url("https://x.dev/i.png", "trek://newtab/"), None, "Trek's own pages have none");
        assert_eq!(favicon_url("", "about:blank"), None);
        assert_eq!(favicon_url("", "file:///C:/a.html"), None);
    }

    #[test]
    fn favicons_are_read_by_their_bytes() {
        let png = decode_favicon(b"\x89PNG\r\n\x1a\n....".to_vec()).expect("a PNG");
        assert_eq!(png.format, ImageFormat::Png);
        let ico = decode_favicon(vec![0, 0, 1, 0, 1, 0]).expect("an ICO");
        assert_eq!(ico.format, ImageFormat::Ico);
        let svg = decode_favicon(b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec()).expect("an SVG");
        assert_eq!(svg.format, ImageFormat::Svg);
        assert!(decode_favicon(b"<html>Not found</html>".to_vec()).is_none());
    }

    /// One HTTP answer on a loopback port; the URL to ask it at.
    fn serve_once(status: &'static str, body: Vec<u8>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = listener.local_addr().expect("its address").port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request);
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}/favicon.ico")
    }

    #[test]
    fn favicons_download_over_http_within_their_limit() {
        let png = b"\x89PNG\r\n\x1a\nicon".to_vec();
        let got = trek_core::runtime().block_on(download_favicon(&serve_once("200 OK", png.clone())));
        assert_eq!(got, Some(png));
        let missing = trek_core::runtime().block_on(download_favicon(&serve_once("404 Not Found", b"nope".to_vec())));
        assert_eq!(missing, None, "an error page isn't an icon");
        let huge = trek_core::runtime().block_on(download_favicon(&serve_once("200 OK", vec![0; FAVICON_MAX_BYTES + 1])));
        assert_eq!(huge, None, "too big to be an icon");
    }

    /// A real WebView2 in a real window, for the `#[ignore]`d tests below: off screen, never
    /// activated, on a data folder of its own.
    #[cfg(windows)]
    mod real {
        use super::{Msg, PAGES_SCHEME, from_view, native};
        use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
        use std::borrow::Cow;
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};
        use windows_sys::Win32::UI::WindowsAndMessaging::*;

        pub struct Parent(pub isize);
        impl HasWindowHandle for Parent {
            fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
                let hwnd = std::num::NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?;
                Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd))) })
            }
        }

        pub fn pump() {
            unsafe {
                let mut msg = std::mem::zeroed::<MSG>();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }

        pub fn wait_for<T>(what: &str, mut poll: impl FnMut() -> Option<T>) -> T {
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                pump();
                if let Some(v) = poll() {
                    return v;
                }
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// A plain STATIC window as the parent: off screen, no taskbar button, never activated.
        pub fn parent_window() -> isize {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let parent = unsafe {
                CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), std::ptr::null(), WS_POPUP | WS_CLIPCHILDREN, -32000, -32000, 640, 480, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null())
            };
            assert!(!parent.is_null(), "a parent window");
            unsafe { ShowWindow(parent, SW_SHOWNOACTIVATE) };
            parent as isize
        }

        /// The window in front, whoever's it is.
        pub fn foreground() -> isize {
            unsafe { GetForegroundWindow() as isize }
        }

        /// The window of this thread with the keys (0 for none).
        pub fn focus() -> isize {
            unsafe {
                let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..std::mem::zeroed() };
                GetGUIThreadInfo(windows_sys::Win32::System::Threading::GetCurrentThreadId(), &mut info);
                info.hwndFocus as isize
            }
        }

        /// A view as the Browser tool makes one (Trek's pages under `PAGES_SCHEME`), with what it reports.
        pub struct View {
            pub wv: wry::WebView,
            pub rx: async_channel::Receiver<Msg>,
            context: wry::WebContext,
            data: PathBuf,
        }

        impl View {
            pub fn open(parent: isize) -> View {
                static N: AtomicUsize = AtomicUsize::new(0);
                let data = std::env::temp_dir().join(format!("trek-webview2-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
                let mut context = wry::WebContext::new(Some(data.clone()));
                let (tx, rx) = async_channel::unbounded::<Msg>();
                let (t1, t2) = (tx.clone(), tx.clone());
                let wv = wry::WebViewBuilder::new_with_web_context(&mut context)
                    .with_focused(false)
                    .with_url("about:blank")
                    .with_bounds(wry::Rect { position: wry::dpi::LogicalPosition::new(0, 0).into(), size: wry::dpi::LogicalSize::new(400, 300).into() })
                    .with_custom_protocol(PAGES_SCHEME.into(), |_, _| {
                        wry::http::Response::builder().header("Content-Type", "text/html").body(Cow::Borrowed(&b"<title>Trek page</title>"[..])).unwrap_or_default()
                    })
                    .with_document_title_changed_handler(move |title| {
                        let _ = t1.try_send(Msg::Title(1, title));
                    })
                    .with_on_page_load_handler(move |event, url| {
                        let _ = t2.try_send(Msg::Load { tab: 1, finished: matches!(event, wry::PageLoadEvent::Finished), url });
                    })
                    .build_as_child(&Parent(parent))
                    .expect("a WebView2 (is the runtime installed?)");
                native::watch(&wv, 1, &tx);
                View { wv, rx, context, data }
            }

            /// Wait for `want` (in Trek's form) to finish loading.
            pub fn loaded(&self, want: &str) {
                wait_for(&format!("{want} to load"), || match self.rx.try_recv() {
                    Ok(Msg::Load { finished: true, url, .. }) if from_view(url.clone()) == want => Some(()),
                    _ => None,
                })
            }

            /// Wait for the page's title to be `want`.
            pub fn titled(&self, want: &str) {
                wait_for(&format!("the title {want:?}"), || match self.rx.try_recv() {
                    Ok(Msg::Title(_, title)) if title == want => Some(()),
                    _ => None,
                })
            }

            /// Close the view, then the data folder it kept.
            pub fn finish(self) {
                let View { wv, context, data, .. } = self;
                drop(wv);
                pump();
                drop(context);
                let _ = std::fs::remove_dir_all(&data);
            }
        }
    }

    /// A real WebView2 in a real window (off screen, never activated): the native calls the
    /// headless harness can't reach. Run by hand: `cargo test -p trek-app a_real_webview2 -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "creates a real WebView2 window, which needs a desktop session and the WebView2 runtime (CI runs without one)"]
    fn a_real_webview2_loads_reports_captures_and_outlives_its_window() {
        use real::{View, focus, parent_window, pump, wait_for};
        use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;

        let parent = parent_window();
        let view = View::open(parent);
        let (wv, rx) = (&view.wv, &view.rx);
        let loaded = |want: &str| view.loaded(want);
        loaded("about:blank");
        // Created without the keys: nothing in this thread took the focus.
        let keys = focus();
        assert!(keys == 0 || keys == parent, "the new view took the focus");

        // A titled page: the title arrives, and history now goes back.
        wv.load_url("data:text/html,<title>Trek%20test</title><p>hello").expect("navigate");
        wait_for("the title", || match rx.try_recv() {
            Ok(Msg::Title(_, title)) if title == "Trek test" => Some(()),
            _ => None,
        });
        wait_for("history", || native::can_go_back(&wv).then_some(()));
        assert!(!native::can_go_forward(&wv));

        // Trek's own pages, through wry's http://trek-pages. stand-in, come back as trek://.
        wv.load_url(&to_view(START_URL)).expect("navigate");
        loaded(START_URL);

        // The screenshot: a PNG the size of the view in device pixels.
        let shot = native::capture_png(&wv);
        let png = wait_for("the capture", || shot.try_recv().ok()).expect("a capture");
        let info = png::Decoder::new(std::io::Cursor::new(&png)).read_info().expect("a PNG").info().clone();
        let size = wv.bounds().expect("bounds").size.to_physical::<u32>(1.0);
        assert_eq!((info.width, info.height), (size.width, size.height));

        // A load that fails ends on WebView2's error page, which Trek is told of.
        wv.load_url("http://127.0.0.1:9/").expect("navigate");
        let failed = wait_for("the failure", || match rx.try_recv() {
            Ok(Msg::Failed { url, .. }) => Some(url),
            _ => None,
        });
        assert!(failed.starts_with("http://127.0.0.1:9"), "{failed}");

        // The window goes first (a closed Trek window), then the view: no crash.
        unsafe { DestroyWindow(parent as _) };
        pump();
        view.finish();
    }

    /// Only Trek's pages are at Trek's address, hiding the view leaves the windows alone, and a
    /// hidden view answers no capture.
    #[cfg(windows)]
    #[test]
    #[ignore = "creates a real WebView2 window, which needs a desktop session and the WebView2 runtime (CI runs without one)"]
    fn a_real_webview2_serves_sites_named_like_trek_and_keeps_the_front_window_when_hidden() {
        use real::{View, foreground, parent_window, pump, wait_for};
        use std::io::{Read, Write};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};
        use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;

        // A dev server on loopback that answers anything with a titled page.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = listener.local_addr().expect("its address").port();
        listener.set_nonblocking(true).expect("non-blocking");
        let stop = Arc::new(AtomicBool::new(false));
        let server = std::thread::spawn({
            let stop = stop.clone();
            move || {
                let page = "<title>Dev server</title>";
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            let _ = stream.read(&mut [0u8; 4096]);
                            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len());
                            let _ = stream.write_all(head.as_bytes());
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            }
        });

        let parent = parent_window();
        let view = View::open(parent);
        let wv = &view.wv;
        view.loaded("about:blank");

        // `trek.localhost` is a dev server's name (Caddy, portless…), not Trek's start page.
        wv.load_url(&to_view(&format!("http://trek.localhost:{port}/"))).expect("navigate");
        view.titled("Dev server");
        wv.load_url(&to_view(START_URL)).expect("navigate");
        view.titled("Trek page");

        // Handing the keys back and hiding the view move nothing in front of anything.
        let front = foreground();
        wv.focus_parent().expect("focus parent");
        wv.set_visible(false).expect("hide");
        pump();
        wv.set_visible(true).expect("show");
        pump();
        assert_eq!(foreground(), front, "the view changed which window is in front");

        // A view that's hidden draws nothing and answers no capture (hence the panel's timeout and
        // its refusing to ask); shown again, it answers.
        wv.set_visible(false).expect("hide");
        let hidden = native::capture_png(wv);
        let until = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < until {
            pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(hidden.try_recv().is_err(), "a hidden view drew a capture: the panel's guard is not needed");
        wv.set_visible(true).expect("show");
        let shot = native::capture_png(wv);
        let png = wait_for("a capture", || shot.try_recv().ok()).expect("a capture");
        assert!(png.starts_with(b"\x89PNG"));

        unsafe { DestroyWindow(parent as _) };
        pump();
        view.finish();
        stop.store(true, Ordering::Relaxed);
        server.join().expect("the server");
    }

    #[test]
    fn normalizes_addresses() {
        assert_eq!(normalize("3000"), "http://localhost:3000");
        assert_eq!(normalize(":5173/app"), "http://localhost:5173/app");
        assert_eq!(normalize("localhost:8080"), "http://localhost:8080");
        assert_eq!(normalize("example.com/a"), "https://example.com/a");
        assert_eq!(normalize("https://x.dev"), "https://x.dev");
        assert_eq!(normalize("rust borrow checker"), "https://www.google.com/search?q=rust%20borrow%20checker");
        assert_eq!(normalize(""), START_URL);
    }

    #[test]
    fn reads_titles_and_hosts() {
        assert_eq!(extract_title("<html><head><TITLE> Vite &amp; React </TITLE>"), Some("Vite & React".into()));
        assert_eq!(extract_title("<html>"), None);
        assert_eq!(host_of("https://www.github.com/a?b"), Some("github.com".into()));
        assert_eq!(host_of("http://localhost:3000"), Some("localhost:3000".into()));
    }
}
