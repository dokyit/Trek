//! WebKit's side of `bridge` (macOS): the messages and ports WebKit hands Trek for an
//! extension's `runtime.sendNativeMessage` / `connectNative`, answered by Trek ([`bridge::APP`])
//! or by a native messaging host the user allowed.
//!
//! Everything here is the main thread's, as WebKit is. A host's output is read on a thread of its
//! own, and what it says is handed to the main thread as a job ([`run_jobs`], run by the
//! Browser's wake loop in `browser_extensions`).

use super::bridge::{self, Notice};
use super::webkit;
use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    ClassType as _, DefinedClass, MainThreadMarker, MainThreadOnly, Message as _, define_class,
    msg_send,
};
use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowDelegate, NSWindowStyleMask};
use objc2_foundation::{
    NSData, NSError, NSJSONSerialization, NSNotification, NSPoint, NSRect, NSSize, NSString, NSURL,
    NSURLRequest,
};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationDelegate, WKWebExtensionContext, WKWebExtensionMessagePort,
    WKWebView, WKWebViewConfiguration,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write as _;
use std::sync::Mutex;

pub(super) type Job = Box<dyn FnOnce() + Send>;

static JOBS: Mutex<Vec<Job>> = Mutex::new(Vec::new());
static WAKE: Mutex<Option<async_channel::Sender<()>>> = Mutex::new(None);

/// Run `job` on the main thread, soon (from any thread).
pub(super) fn on_main(job: Job) {
    if let Ok(mut jobs) = JOBS.lock() {
        jobs.push(job);
    }
    if let Ok(wake) = WAKE.lock()
        && let Some(tx) = wake.as_ref()
    {
        let _ = tx.try_send(());
    }
}

/// Where to wake the main thread for jobs (the Browser's wake loop).
pub fn set_wake(tx: async_channel::Sender<()>) {
    if let Ok(mut wake) = WAKE.lock() {
        *wake = Some(tx);
    }
}

/// Run the jobs other threads left (main thread).
pub fn run_jobs() {
    let jobs = JOBS
        .lock()
        .map(|mut j| std::mem::take(&mut *j))
        .unwrap_or_default();
    for job in jobs {
        job();
    }
}

/// A port WebKit opened for an extension: to Trek, or to a host.
struct Port {
    key: u64,
    ext: String,
    port: Retained<WKWebExtensionMessagePort>,
    /// A host's input (`None`: the port is Trek's).
    host: Option<std::sync::Arc<Mutex<std::process::ChildStdin>>>,
    child: Option<std::sync::Arc<Mutex<std::process::Child>>>,
}

#[derive(Default)]
struct State {
    ports: Vec<Port>,
    next_port: u64,
    /// Notifications shown, by extension and id.
    notices: HashMap<(String, String), Notice>,
    /// Banners to post or take down (`take_banners`), for the app.
    banners: Vec<Banner>,
    /// Events for an extension with no port open yet (its background is starting).
    queued: HashMap<String, Vec<Value>>,
    /// Each extension's offscreen page.
    offscreen: HashMap<String, Retained<WKWebView>>,
    /// Open sign-in windows (`identity.launchWebAuthFlow`).
    auth: Vec<Retained<AuthWindow>>,
    /// `sendNativeMessage` calls waiting for their host's answer.
    waiting: HashMap<u64, Answer>,
}

type Answer = Box<dyn FnOnce(Result<Value, String>)>;

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with_borrow_mut(f)
}

/// A banner for the app to post (`Some`) or take down.
#[derive(Debug, Clone, PartialEq)]
pub enum Banner {
    Show(Notice),
    Hide(String),
}

/// The banners to post or take down since last asked.
pub fn take_banners() -> Vec<Banner> {
    with(|s| std::mem::take(&mut s.banners))
}

/// The notifications extensions show now (for the shots driver).
pub fn notices() -> Vec<Notice> {
    let mut all: Vec<Notice> = with(|s| s.notices.values().cloned().collect());
    all.sort_by(|a, b| (&a.ext, &a.id).cmp(&(&b.ext, &b.id)));
    all
}

// ---- Foundation objects as JSON -----------------------------------------------------------

fn to_json(object: &AnyObject) -> Value {
    // NSJSONWritingFragmentsAllowed: a message may be a bare string or number.
    let data: Option<Retained<NSData>> = unsafe {
        msg_send![NSJSONSerialization::class(), dataWithJSONObject: object, options: 4usize, error: std::ptr::null_mut::<*mut NSError>()]
    };
    data.and_then(|d| serde_json::from_slice(&d.to_vec()).ok())
        .unwrap_or(Value::Null)
}

fn from_json(value: &Value) -> Option<Retained<AnyObject>> {
    let bytes = serde_json::to_vec(value).ok()?;
    let data = NSData::with_bytes(&bytes);
    // NSJSONReadingFragmentsAllowed.
    unsafe {
        msg_send![NSJSONSerialization::class(), JSONObjectWithData: &*data, options: 4usize, error: std::ptr::null_mut::<*mut NSError>()]
    }
}

fn ns_error(text: &str) -> Retained<NSError> {
    use objc2_foundation::{NSDictionary, NSLocalizedDescriptionKey};
    let info = NSDictionary::from_retained_objects(
        &[unsafe { NSLocalizedDescriptionKey }],
        &[Retained::into_super(Retained::into_super(
            NSString::from_str(text),
        ))],
    );
    unsafe { NSError::errorWithDomain_code_userInfo(&NSString::from_str("Trek"), 1, Some(&info)) }
}

fn ext_id(ctx: &WKWebExtensionContext) -> String {
    unsafe { ctx.uniqueIdentifier() }.to_string()
}

fn ext_name(ctx: &WKWebExtensionContext) -> String {
    unsafe { ctx.webExtension().displayName() }
        .map(|n| n.to_string())
        .unwrap_or_default()
}

// ---- WebKit's calls -----------------------------------------------------------------------

/// `runtime.sendNativeMessage(app, message)`: Trek's answer, or one host's.
pub fn send(
    ctx: &WKWebExtensionContext,
    message: &AnyObject,
    app: Option<&NSString>,
    reply: &DynBlock<dyn Fn(*mut AnyObject, *mut NSError)>,
) {
    let message = to_json(message);
    let app = app.map(|a| a.to_string()).unwrap_or_default();
    let reply = reply.copy();
    let answer = move |result: Result<Value, String>| match result {
        Ok(v) => match from_json(&v) {
            Some(o) => reply.call((Retained::as_ptr(&o).cast_mut(), std::ptr::null_mut())),
            None => reply.call((std::ptr::null_mut(), std::ptr::null_mut())),
        },
        Err(e) => reply.call((
            std::ptr::null_mut(),
            Retained::as_ptr(&ns_error(&e)).cast_mut(),
        )),
    };
    if app == bridge::APP {
        answer_call(ctx, message, Box::new(move |v| answer(Ok(v))));
        return;
    }
    let id = ext_id(ctx);
    let host = match bridge::find_host(&app, &id, &bridge::host_dirs()) {
        Ok(h) => h,
        Err(e) => {
            log_host_once(&id, &app, &e);
            return answer(Err(e));
        }
    };
    // Kept here (WebKit's reply lives on the main thread) until the host answers.
    let key = with(|s| {
        s.next_port += 1;
        s.waiting.insert(s.next_port, Box::new(answer));
        s.next_port
    });
    let run = {
        let (host, id) = (host.clone(), id.clone());
        move || {
            std::thread::spawn(move || {
                let result = bridge::one_message(&host, &id, &message);
                on_main(Box::new(move || {
                    if let Some(f) = with(|s| s.waiting.remove(&key)) {
                        f(result);
                    }
                }));
            });
        }
    };
    let denied = move || {
        if let Some(f) = with(|s| s.waiting.remove(&key)) {
            f(Err(
                "The user didn't allow it to start that program.".to_string()
            ));
        }
    };
    when_allowed(ctx, &host, Box::new(run), Box::new(denied));
}

/// `runtime.connectNative(app)`: a port to Trek, or to a host started for it.
pub fn connect(
    ctx: &WKWebExtensionContext,
    port: &WKWebExtensionMessagePort,
    done: &DynBlock<dyn Fn(*mut NSError)>,
) {
    let app = unsafe { port.applicationIdentifier() }
        .map(|a| a.to_string())
        .unwrap_or_default();
    let id = ext_id(ctx);
    let port: Retained<WKWebExtensionMessagePort> = port.retain();
    if app == bridge::APP {
        let key = add_port(&id, port.clone(), None, None);
        listen(&port, key);
        done.call((std::ptr::null_mut(),));
        return;
    }
    let host = match bridge::find_host(&app, &id, &bridge::host_dirs()) {
        Ok(h) => h,
        Err(e) => {
            log_host_once(&id, &app, &e);
            done.call((Retained::as_ptr(&ns_error(&e)).cast_mut(),));
            return;
        }
    };
    // WebKit waits for the answer before the port is open: answered once the user has.
    let done = std::rc::Rc::new(RefCell::new(Some(done.copy())));
    let open = {
        let (done, port, host, id) = (done.clone(), port.clone(), host.clone(), id.clone());
        move || {
            let Some(done) = done.borrow_mut().take() else {
                return;
            };
            match bridge::start(&host, &id) {
                Ok(run) => {
                    let bridge::Running {
                        child,
                        input,
                        mut output,
                    } = run;
                    let input = std::sync::Arc::new(Mutex::new(input));
                    let child = std::sync::Arc::new(Mutex::new(child));
                    let key = add_port(&id, port.clone(), Some(input), Some(child));
                    listen(&port, key);
                    done.call((std::ptr::null_mut(),));
                    std::thread::spawn(move || {
                        loop {
                            match bridge::read_message(&mut output) {
                                Ok(Some(message)) => {
                                    on_main(Box::new(move || to_port(key, message)))
                                }
                                Ok(None) | Err(_) => {
                                    on_main(Box::new(move || close_port(key, true)));
                                    break;
                                }
                            }
                        }
                    });
                }
                Err(e) => done.call((Retained::as_ptr(&ns_error(&e)).cast_mut(),)),
            }
        }
    };
    let denied = move || {
        if let Some(done) = done.borrow_mut().take() {
            let e = ns_error("The user didn't allow it to start that program.");
            done.call((Retained::as_ptr(&e).cast_mut(),));
        }
    };
    when_allowed(ctx, &host, Box::new(open), Box::new(denied));
}

/// Say why a host can't be used, once a run per extension and host (an extension that looks for
/// its desktop app asks again every few seconds).
fn log_host_once(id: &str, app: &str, why: &str) {
    thread_local! {
        static SAID: RefCell<std::collections::HashSet<String>> = RefCell::default();
    }
    if SAID.with_borrow_mut(|s| s.insert(format!("{id} {app}"))) {
        tracing::info!("browser extensions: {id}: native host {app}: {why}");
    }
}

/// Run `allowed` once the user lets extension `ctx` start `host` (asked once, then kept), else
/// `denied`.
fn when_allowed(
    ctx: &WKWebExtensionContext,
    host: &bridge::Host,
    allowed: Box<dyn FnOnce()>,
    denied: Box<dyn FnOnce()>,
) {
    let id = ext_id(ctx);
    if webkit::host_allowed(&id, &host.name) {
        allowed();
        return;
    }
    webkit::ask_host(
        ctx,
        host.name.clone(),
        bridge::ask_line(host),
        Box::new(move |yes| if yes { allowed() } else { denied() }),
    );
}

fn add_port(
    ext: &str,
    port: Retained<WKWebExtensionMessagePort>,
    host: Option<std::sync::Arc<Mutex<std::process::ChildStdin>>>,
    child: Option<std::sync::Arc<Mutex<std::process::Child>>>,
) -> u64 {
    let trek = host.is_none();
    let key = with(|s| {
        s.next_port += 1;
        let key = s.next_port;
        s.ports.push(Port {
            key,
            ext: ext.to_string(),
            port,
            host,
            child,
        });
        key
    });
    // Events kept while its background started go to the first port to Trek.
    if trek {
        let queued = with(|s| s.queued.remove(ext)).unwrap_or_default();
        for e in queued {
            to_port(key, e);
        }
    }
    key
}

/// Take what the extension says on port `key`, and its closing.
fn listen(port: &WKWebExtensionMessagePort, key: u64) {
    let on_message = RcBlock::new(move |message: *mut AnyObject, _error: *mut NSError| {
        let Some(message) = (unsafe { message.as_ref() }) else {
            return;
        };
        let message = to_json(message);
        let input = with(|s| {
            s.ports
                .iter()
                .find(|p| p.key == key)
                .map(|p| p.host.clone())
        });
        match input {
            // To Trek: a page saying hello (its events start flowing; nothing to answer).
            Some(None) | None => {}
            Some(Some(input)) => {
                let bytes = bridge::encode(&message);
                std::thread::spawn(move || {
                    if let (Ok(bytes), Ok(mut input)) = (bytes, input.lock()) {
                        let _ = input.write_all(&bytes);
                        let _ = input.flush();
                    }
                });
            }
        }
    });
    let on_close = RcBlock::new(move |_error: *mut NSError| close_port(key, false));
    unsafe {
        port.setMessageHandler(Some(&on_message));
        port.setDisconnectHandler(Some(&on_close));
    }
}

fn to_port(key: u64, message: Value) {
    let Some(port) = with(|s| {
        s.ports
            .iter()
            .find(|p| p.key == key)
            .map(|p| p.port.clone())
    }) else {
        return;
    };
    if let Some(object) = from_json(&message) {
        unsafe { port.sendMessage_completionHandler(Some(&object), None) };
    }
}

/// Port `key` is closed (by the extension, or its host ended: `ended`).
fn close_port(key: u64, ended: bool) {
    let Some(port) = with(|s| {
        let at = s.ports.iter().position(|p| p.key == key)?;
        Some(s.ports.remove(at))
    }) else {
        return;
    };
    if let Some(child) = &port.child
        && let Ok(mut child) = child.lock()
    {
        let _ = child.kill();
        let _ = child.wait();
    }
    if ended {
        unsafe { port.port.disconnect() };
    }
}

/// Send `event` to extension `ext`'s pages: down its ports to Trek, or kept until one opens
/// (its background is started for it).
fn send_event(ext: &str, event: Value) {
    let keys: Vec<u64> = with(|s| {
        s.ports
            .iter()
            .filter(|p| p.ext == ext && p.host.is_none())
            .map(|p| p.key)
            .collect()
    });
    if !keys.is_empty() {
        for key in keys {
            to_port(key, event.clone());
        }
        return;
    }
    with(|s| s.queued.entry(ext.to_string()).or_default().push(event));
    if let Some(ctx) = webkit::context(ext) {
        let done = RcBlock::new(|_: *mut NSError| {});
        unsafe { ctx.loadBackgroundContentWithCompletionHandler(&done) };
    }
}

// ---- Trek's answers -----------------------------------------------------------------------

/// Answer a call on Trek's channel (see `bridge`).
fn answer_call(ctx: &WKWebExtensionContext, message: Value, reply: Box<dyn FnOnce(Value)>) {
    let call: bridge::Call = match serde_json::from_value(message) {
        Ok(c) => c,
        Err(_) => return reply(bridge::err("Trek didn't understand that call.")),
    };
    let id = ext_id(ctx);
    let a = &call.args;
    let text = |k: &str| a[k].as_str().unwrap_or("").to_string();
    match call.method.as_str() {
        "notifications.create" | "notifications.update" => {
            let nid = text("id");
            let update = call.method.ends_with("update");
            let before = with(|s| s.notices.get(&(id.clone(), nid.clone())).cloned());
            if update && before.is_none() {
                return reply(bridge::ok(json!(false)));
            }
            let notice = before
                .unwrap_or_else(|| Notice {
                    ext: id.clone(),
                    id: nid.clone(),
                    from: ext_name(ctx),
                    ..Default::default()
                })
                .with_options(&a["options"]);
            with(|s| {
                s.notices.insert((id.clone(), nid.clone()), notice.clone());
                s.banners.push(Banner::Show(notice));
            });
            webkit::changed();
            reply(bridge::ok(if update { json!(true) } else { json!(nid) }))
        }
        "notifications.clear" => {
            let nid = text("id");
            let gone = with(|s| s.notices.remove(&(id.clone(), nid.clone())));
            if let Some(n) = &gone {
                with(|s| s.banners.push(Banner::Hide(n.tag())));
                webkit::changed();
            }
            reply(bridge::ok(json!(gone.is_some())))
        }
        "notifications.getAll" => {
            let all: serde_json::Map<String, Value> = with(|s| {
                s.notices
                    .keys()
                    .filter(|(e, _)| e == &id)
                    .map(|(_, n)| (n.clone(), json!(true)))
                    .collect()
            });
            reply(bridge::ok(Value::Object(all)))
        }
        "offscreen.create" => {
            if with(|s| s.offscreen.contains_key(&id)) {
                return reply(bridge::err(
                    "Only a single offscreen document may be created.",
                ));
            }
            match make_offscreen(ctx, &text("url")) {
                Ok(view) => {
                    with(|s| s.offscreen.insert(id, view));
                    reply(bridge::ok(Value::Null))
                }
                Err(e) => reply(bridge::err(e)),
            }
        }
        "offscreen.close" => {
            let gone = with(|s| s.offscreen.remove(&id));
            match gone {
                Some(view) => {
                    unsafe { view.stopLoading() };
                    reply(bridge::ok(Value::Null))
                }
                None => reply(bridge::err("No offscreen document is open.")),
            }
        }
        "offscreen.has" => reply(bridge::ok(json!(with(|s| s.offscreen.contains_key(&id))))),
        "runtime.getContexts" => {
            let wants = |kind: &str| {
                a["contextTypes"]
                    .as_array()
                    .is_none_or(|t| t.iter().any(|t| t == kind))
            };
            let mut out = vec![];
            if wants("OFFSCREEN_DOCUMENT")
                && let Some(view) = with(|s| s.offscreen.get(&id).cloned())
            {
                let url = unsafe { view.URL() }
                    .and_then(|u| u.absoluteString())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                out.push(json!({
                    "contextType": "OFFSCREEN_DOCUMENT", "documentUrl": url,
                    "documentOrigin": format!("webkit-extension://{id}"), "contextId": format!("offscreen-{id}"),
                    "tabId": -1, "windowId": -1, "frameId": -1, "incognito": false,
                }));
            }
            reply(bridge::ok(Value::Array(out)))
        }
        "identity.launchWebAuthFlow" => {
            let url = text("url");
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return reply(bridge::err("The sign-in address must be a web page."));
            }
            open_auth_window(&id, &ext_name(ctx), &url, reply);
        }
        "idle.query" => {
            let threshold = a["seconds"].as_f64().unwrap_or(60.).max(15.);
            reply(bridge::ok(json!(bridge::idle_state(
                idle_seconds(),
                screen_locked(),
                threshold
            ))))
        }
        "downloads.save" => {
            use base64::Engine as _;
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(text("data")) else {
                return reply(bridge::err("The download's data was damaged."));
            };
            let dir = bridge::downloads_dir();
            let _ = std::fs::create_dir_all(&dir);
            let path = bridge::download_path(&dir, &text("name"));
            match std::fs::write(&path, &bytes) {
                Ok(()) => {
                    let n = DOWNLOADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    webkit::say(format!(
                        "{} downloaded {} to Downloads",
                        ext_name(ctx),
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                    reply(bridge::ok(json!({
                        "id": n, "path": path.to_string_lossy(), "size": bytes.len(),
                    })))
                }
                Err(e) => reply(bridge::err(format!("The download couldn't be saved: {e}"))),
            }
        }
        "downloads.show" => {
            let path = text("path");
            let target = if path.is_empty() {
                bridge::downloads_dir()
            } else {
                std::path::PathBuf::from(path)
            };
            // Only what's in Downloads.
            if target.starts_with(bridge::downloads_dir()) {
                let _ = std::process::Command::new("open")
                    .arg("-R")
                    .arg(&target)
                    .spawn();
            }
            reply(bridge::ok(Value::Null))
        }
        "sidePanel.open" => {
            let url = text("url");
            match NSURL::URLWithString(&NSString::from_str(&url)) {
                Some(u) if !url.is_empty() => {
                    webkit::open_page(ctx, &u);
                    reply(bridge::ok(Value::Null))
                }
                _ => reply(bridge::err("It has no side panel page.")),
            }
        }
        "log.shim" => {
            tracing::info!("browser extensions: {id}: shim in {}: {}", text("where"), a);
            reply(bridge::ok(Value::Null))
        }
        "log.error" => {
            tracing::info!(
                "browser extensions: {id} ({}) said: {}",
                text("where"),
                text("text")
            );
            reply(bridge::ok(Value::Null))
        }
        other => reply(bridge::err(format!("Trek doesn't answer {other}."))),
    }
}

static DOWNLOADS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn idle_seconds() -> f64 {
    crate::push::idle_seconds()
}

fn screen_locked() -> bool {
    crate::push::screen_locked()
}

/// The user clicked one of an extension's banners (`action`: a button's id, or the banner).
/// Whether the tag was an extension's.
pub fn clicked(tag: &str, action: Option<&str>) -> bool {
    let Some((ext, id)) = bridge::parse_tag(tag) else {
        return false;
    };
    let event = match action.and_then(bridge::button_index) {
        Some(i) => bridge::event("notifications.onButtonClicked", vec![json!(id), json!(i)]),
        None => bridge::event("notifications.onClicked", vec![json!(id)]),
    };
    send_event(ext, event);
    true
}

/// The user (or the driver) closed notification `id` of extension `ext`.
pub fn closed(ext: &str, id: &str) {
    if with(|s| s.notices.remove(&(ext.to_string(), id.to_string()))).is_some() {
        send_event(
            ext,
            bridge::event("notifications.onClosed", vec![json!(id), json!(true)]),
        );
        webkit::changed();
    }
}

/// Extension `ext` stopped (off or removed): its ports, pages and windows go.
pub fn unloaded(ext: &str) {
    let ports: Vec<u64> = with(|s| {
        s.offscreen.remove(ext);
        s.queued.remove(ext);
        s.ports
            .iter()
            .filter(|p| p.ext == ext)
            .map(|p| p.key)
            .collect()
    });
    for key in ports {
        close_port(key, true);
    }
    let gone: Vec<Notice> = with(|s| {
        let keys: Vec<_> = s
            .notices
            .keys()
            .filter(|(e, _)| e == ext)
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|k| s.notices.remove(&k))
            .collect()
    });
    with(|s| s.banners.extend(gone.iter().map(|n| Banner::Hide(n.tag()))));
}

fn make_offscreen(ctx: &WKWebExtensionContext, url: &str) -> Result<Retained<WKWebView>, String> {
    let mtm = MainThreadMarker::new().ok_or("Not on the main thread.")?;
    let base = unsafe { ctx.baseURL() }
        .absoluteString()
        .map(|s| s.to_string())
        .unwrap_or_default();
    // Its own page only.
    if !url.starts_with(&base) {
        return Err("An offscreen document must be one of the extension's own pages.".into());
    }
    let url = NSURL::URLWithString(&NSString::from_str(url)).ok_or("That isn't an address.")?;
    unsafe {
        let config = ctx
            .webViewConfiguration()
            .ok_or("It has no pages of its own.")?;
        let rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(400., 300.));
        let view = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), rect, &config);
        view.loadRequest(&NSURLRequest::requestWithURL(&url));
        Ok(view)
    }
}

// ---- Sign-in windows ----------------------------------------------------------------------

struct AuthIvars {
    ext: String,
    window: RefCell<Option<Retained<NSWindow>>>,
    reply: RefCell<Option<Box<dyn FnOnce(Value)>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars drop themselves.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TrekExtensionAuthWindow"]
    #[ivars = AuthIvars]
    struct AuthWindow;

    unsafe impl NSObjectProtocol for AuthWindow {}

    unsafe impl WKNavigationDelegate for AuthWindow {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide(
            &self,
            _webview: &WKWebView,
            action: &WKNavigationAction,
            decide: &DynBlock<dyn Fn(isize)>,
        ) {
            let url = unsafe { action.request().URL() }
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string())
                .unwrap_or_default();
            if bridge::is_auth_redirect(&url, &self.ivars().ext) {
                decide.call((0,));
                self.finish(Ok(url));
                return;
            }
            decide.call((1,));
        }
    }

    unsafe impl NSWindowDelegate for AuthWindow {
        #[unsafe(method(windowWillClose:))]
        fn will_close(&self, _note: &NSNotification) {
            self.finish(Err("The user closed the sign-in window.".into()));
        }
    }
);

impl AuthWindow {
    fn finish(&self, result: Result<String, String>) {
        let Some(reply) = self.ivars().reply.borrow_mut().take() else {
            return;
        };
        reply(match result {
            Ok(url) => bridge::ok(json!(url)),
            Err(e) => bridge::err(e),
        });
        if let Some(window) = self.ivars().window.borrow_mut().take() {
            window.setDelegate(None);
            window.close();
        }
        let me = self as *const Self;
        with(|s| s.auth.retain(|w| !std::ptr::eq(Retained::as_ptr(w), me)));
    }
}

/// `identity.launchWebAuthFlow`: `url` in a window of its own, with the Browser's cookies, until
/// it goes to the extension's redirect address (that address is the answer) or is closed.
fn open_auth_window(ext: &str, name: &str, url: &str, reply: Box<dyn FnOnce(Value)>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return reply(bridge::err("Not on the main thread."));
    };
    let Some(address) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return reply(bridge::err("That isn't an address."));
    };
    let this = mtm.alloc::<AuthWindow>().set_ivars(AuthIvars {
        ext: ext.to_string(),
        window: RefCell::new(None),
        reply: RefCell::new(Some(reply)),
    });
    let this: Retained<AuthWindow> = unsafe { msg_send![super(this), init] };
    unsafe {
        let rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(520., 680.));
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect,
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setReleasedWhenClosed(false);
        window.setTitle(&NSString::from_str(&format!("{name}: sign in")));
        // The Browser's website data (the default store): a session there counts here.
        let config = WKWebViewConfiguration::new(mtm);
        config.setApplicationNameForUserAgent(Some(&NSString::from_str(
            &crate::panels::browser::safari_application_name(),
        )));
        let view = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), rect, &config);
        view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*this)));
        view.loadRequest(&NSURLRequest::requestWithURL(&address));
        window.setContentView(Some(&view));
        window.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        window.center();
        window.makeKeyAndOrderFront(None);
        *this.ivars().window.borrow_mut() = Some(window);
    }
    with(|s| s.auth.push(this));
}

/// The sign-in windows open now, by extension (for the shots driver).
pub fn auth_windows() -> Vec<String> {
    with(|s| s.auth.iter().map(|w| w.ivars().ext.clone()).collect())
}
