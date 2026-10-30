//! The Browser's extensions on macOS: WebKit's own (`WKWebExtension`, macOS 15.4 and later), which
//! runs Chrome and Firefox extensions from their folders: background service workers, content
//! scripts, declarativeNetRequest rules, storage, popups and options pages. One controller serves
//! every Browser view (each is made with it in its configuration: `configuration`); Trek tells it
//! which tabs there are and which is in front (`sync_tabs`), shows an extension's popup under its
//! toolbar button and its pages in a window of their own, and asks the user before an extension
//! gets more than it was installed with (`prompts`, answered in the Browser).
//!
//! What's installed is Trek's list (`registry`); WebKit keeps each extension's storage, under the
//! extension's id. Everything here is the main thread's: WebKit calls the delegate there, and the
//! state is a thread local that no WebKit call is made while borrowing (a call can come back in).

use super::{Extension, extensions_dir, manifest, registry};
use block2::{DynBlock, RcBlock};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
use objc2_foundation::{
    NSArray, NSDate, NSError, NSPoint, NSRect, NSRectEdge, NSSet, NSSize, NSString, NSURL,
    NSURLRequest, NSUUID,
};
use objc2_web_kit::{
    WKWebExtension, WKWebExtensionAction, WKWebExtensionContext,
    WKWebExtensionContextPermissionStatus, WKWebExtensionController,
    WKWebExtensionControllerConfiguration, WKWebExtensionControllerDelegate,
    WKWebExtensionMatchPattern, WKWebExtensionPermission, WKWebExtensionTab,
    WKWebExtensionTabChangedProperties, WKWebExtensionTabConfiguration, WKWebExtensionWindow,
    WKWebExtensionWindowConfiguration, WKWebExtensionWindowState, WKWebExtensionWindowType,
    WKWebView, WKWebViewConfiguration,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Whether this macOS has WebKit's extensions (15.4 and later).
pub fn supported() -> bool {
    objc2::available!(macos = 15.4)
}

struct State {
    controller: Retained<WKWebExtensionController>,
    _delegate: Retained<Delegate>,
    /// Loaded extensions, by id.
    contexts: HashMap<String, Retained<WKWebExtensionContext>>,
    /// Why an extension didn't load, by id.
    failed: HashMap<String, String>,
    /// Each loaded extension's toolbar icon, from its manifest (read once, not as it draws).
    icons: HashMap<String, Option<PathBuf>>,
    /// Each loaded extension's place in the installed list (its toolbar button's).
    rank: HashMap<String, usize>,
    /// One per Browser panel, by its key.
    windows: Vec<Retained<ExtWindow>>,
    focused: Option<u64>,
    /// Opens an address in a new Browser tab (the last Browser synced).
    open_tab: Option<Rc<dyn Fn(String)>>,
    /// Where the next popup shows: under the toolbar button just clicked.
    anchor: Option<(Retained<NSView>, NSRect)>,
    /// Extension pages (options) open in windows of their own.
    pages: Vec<Retained<NSWindow>>,
    prompts: Vec<Pending>,
    /// The popup last shown, for the shots driver to read.
    popup: Option<Retained<WKWebView>>,
    /// When each extension was last unloaded, and those waiting to load again (`load_when_free`).
    unloaded: HashMap<String, Instant>,
    reloads: Vec<registry::Installed>,
    next_prompt: u64,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// Bumped on every change the Browser shows (an extension loaded, an action's badge, a prompt).
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    static WAKER: RefCell<Option<async_channel::Sender<()>>> = const { RefCell::new(None) };
}

/// `f` on the state, once there is one (`ensure`); never call WebKit inside.
fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with_borrow_mut(|s| s.as_mut().map(f))
}

/// Something the Browser shows changed: draw again.
fn changed() {
    GENERATION.set(GENERATION.get() + 1);
    WAKER.with_borrow(|w| {
        if let Some(w) = w {
            let _ = w.try_send(());
        }
    });
}

pub fn generation() -> u64 {
    GENERATION.get()
}

/// Wake `tx` on each change (`generation`): the app redraws.
pub fn set_waker(tx: async_channel::Sender<()>) {
    WAKER.set(Some(tx));
}

pub fn has_waker() -> bool {
    WAKER.with_borrow(|w| w.is_some())
}

/// Make the controller, the first time, and load what's installed and on. False where WebKit
/// has no extensions.
pub fn ensure() -> bool {
    if !supported() {
        return false;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let fresh = STATE.with_borrow_mut(|s| {
        if s.is_some() {
            return false;
        }
        // SAFETY: main thread; plain object creation and a delegate WebKit holds weakly (kept in
        // the state for as long as the controller).
        unsafe {
            let config = if trek_core::paths::isolated() {
                // A test profile keeps its extensions' storage apart from the user's.
                let id = NSUUID::from_bytes(uuid_bytes(&trek_core::paths::data_dir()));
                WKWebExtensionControllerConfiguration::configurationWithIdentifier(&id, mtm)
            } else {
                WKWebExtensionControllerConfiguration::defaultConfiguration(mtm)
            };
            let controller = WKWebExtensionController::initWithConfiguration(
                WKWebExtensionController::alloc(mtm),
                &config,
            );
            let delegate: Retained<Delegate> = msg_send![Delegate::alloc(mtm), init];
            controller.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            *s = Some(State {
                controller,
                _delegate: delegate,
                contexts: HashMap::new(),
                failed: HashMap::new(),
                icons: HashMap::new(),
                rank: HashMap::new(),
                windows: vec![],
                focused: None,
                open_tab: None,
                anchor: None,
                pages: vec![],
                prompts: vec![],
                popup: None,
                unloaded: HashMap::new(),
                reloads: vec![],
                next_prompt: 1,
            });
        }
        true
    });
    if fresh {
        for entry in registry::load(&extensions_dir()) {
            if entry.enabled {
                load(entry);
            }
        }
    }
    true
}

fn uuid_bytes(path: &std::path::Path) -> [u8; 16] {
    use sha2::Digest as _;
    let hash = sha2::Sha256::digest(path.to_string_lossy().as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    bytes
}

/// A configuration for a new Browser view, with the extensions' controller in it (`None` where
/// WebKit has none). A fresh one per view: wry registers each view's own handlers on it.
pub fn configuration() -> Option<Retained<WKWebViewConfiguration>> {
    if !ensure() {
        return None;
    }
    let mtm = MainThreadMarker::new()?;
    let controller = with(|s| s.controller.clone())?;
    // SAFETY: main thread; the configuration is new and the controller live.
    unsafe {
        let config = WKWebViewConfiguration::new(mtm);
        config.setWebExtensionController(Some(&controller));
        Some(config)
    }
}

/// Put the extensions' controller in `config` (one WebKit made for a page's popup) unless it
/// already has one: WebKit copies the opener's configuration, so it normally does.
pub fn attach_controller(config: &WKWebViewConfiguration) {
    if !ensure() {
        return;
    }
    let Some(controller) = with(|s| s.controller.clone()) else {
        return;
    };
    // SAFETY: main thread (the popup's view is made there); the view isn't made yet.
    unsafe {
        if config.webExtensionController().is_none() {
            config.setWebExtensionController(Some(&controller));
        }
    }
}

/// Load installed extension `entry` (WebKit reads its folder, then it runs).
fn load(entry: registry::Installed) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let folder = entry.folder_in(&extensions_dir());
    // WebKit takes an absolute file URL (and stops the app on any other).
    let Ok(folder) = folder.canonicalize() else {
        with(|s| {
            s.failed
                .insert(entry.id.clone(), "Its folder is gone.".to_string())
        });
        changed();
        return;
    };
    let url =
        NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&folder.to_string_lossy()), true);
    let done = RcBlock::new(move |ext: *mut WKWebExtension, error: *mut NSError| {
        // SAFETY: WebKit hands a valid extension or nil, on the main thread.
        match unsafe { Retained::retain(ext) } {
            Some(ext) => attach(&entry, &ext),
            None => {
                let why = unsafe { error.as_ref() }
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_else(|| "WebKit couldn't read it.".to_string());
                with(|s| s.failed.insert(entry.id.clone(), why));
                changed();
            }
        }
    });
    // SAFETY: main thread; WebKit copies the block.
    unsafe { WKWebExtension::extensionWithResourceBaseURL_completionHandler(&url, &done, mtm) };
}

/// Run `ext` as installed extension `entry`: with the permissions it was installed with (the user
/// agreed to them then) and those allowed since.
fn attach(entry: &registry::Installed, ext: &WKWebExtension) {
    let Some(controller) = with(|s| s.controller.clone()) else {
        return;
    };
    let granted = WKWebExtensionContextPermissionStatus::GrantedExplicitly;
    // SAFETY: main thread; a new context for a live extension.
    let ctx = unsafe {
        let ctx = WKWebExtensionContext::contextForExtension(ext);
        // Its id keeps its storage and its pages' address from one run to the next.
        ctx.setUniqueIdentifier(&NSString::from_str(&entry.id));
        if let Some(base) = NSURL::URLWithString(&NSString::from_str(&format!(
            "webkit-extension://{}/",
            entry.id
        ))) {
            ctx.setBaseURL(&base);
        }
        ctx.setInspectable(true);
        for p in ext.requestedPermissions().iter() {
            ctx.setPermissionStatus_forPermission(granted, &p);
        }
        for p in &entry.allowed {
            ctx.setPermissionStatus_forPermission(granted, &NSString::from_str(p));
        }
        for m in ext.allRequestedMatchPatterns().iter() {
            ctx.setPermissionStatus_forMatchPattern(granted, &m);
        }
        let mtm = MainThreadMarker::new_unchecked();
        for site in &entry.allowed_sites {
            if let Some(m) =
                WKWebExtensionMatchPattern::matchPatternWithString(&NSString::from_str(site), mtm)
            {
                ctx.setPermissionStatus_forMatchPattern(granted, &m);
            }
        }
        ctx
    };
    let icon = manifest::read(&entry.folder_in(&extensions_dir()))
        .ok()
        .and_then(|m| m.icon);
    let rank = registry::load(&extensions_dir())
        .iter()
        .position(|e| e.id == entry.id)
        .unwrap_or(usize::MAX);
    // SAFETY: main thread, no state borrowed (loading calls the delegate).
    let loaded = unsafe { controller.loadExtensionContext_error(&ctx) };
    with(|s| match loaded {
        Ok(()) => {
            s.failed.remove(&entry.id);
            s.icons.insert(entry.id.clone(), icon);
            s.rank.insert(entry.id.clone(), rank);
            s.contexts.insert(entry.id.clone(), ctx);
        }
        Err(e) => {
            s.failed
                .insert(entry.id.clone(), e.localizedDescription().to_string());
        }
    });
    changed();
}

fn unload(id: &str) {
    let Some((controller, ctx)) =
        with(|s| Some((s.controller.clone(), s.contexts.remove(id)?))).flatten()
    else {
        return;
    };
    // SAFETY: main thread, no state borrowed.
    let _ = unsafe { controller.unloadExtensionContext_error(&ctx) };
    with(|s| s.unloaded.insert(id.to_string(), Instant::now()));
    changed();
}

/// How long WebKit takes to let go of an unloaded extension: one loaded again sooner (turned
/// off and on, installed again) finds its background's address still taken, and its background
/// never answers (measured: at once fails, four seconds works).
const RELOAD_AFTER: Duration = Duration::from_secs(2);

/// Load `entry`, or wait until WebKit has let go of it when it was unloaded just now.
fn load_when_free(entry: registry::Installed) {
    let wait = with(|s| {
        let left = s
            .unloaded
            .get(&entry.id)
            .map(|at| RELOAD_AFTER.saturating_sub(at.elapsed()))
            .filter(|left| !left.is_zero())?;
        s.reloads.push(entry.clone());
        Some((left, s._delegate.clone()))
    })
    .flatten();
    match wait {
        None => load(entry),
        Some((left, delegate)) => {
            // SAFETY: main thread; the delegate answers `trekLoadWaiting` (below).
            let _: () = unsafe {
                msg_send![&*delegate, performSelector: objc2::sel!(trekLoadWaiting), withObject: std::ptr::null::<NSObject>(), afterDelay: left.as_secs_f64()]
            };
        }
    }
}

/// Load the extensions waiting for WebKit (`load_when_free`) whose time has come.
fn load_waiting() {
    let due = with(|s| {
        let (due, wait): (Vec<_>, Vec<_>) =
            std::mem::take(&mut s.reloads).into_iter().partition(|e| {
                s.unloaded
                    .get(&e.id)
                    .is_none_or(|at| at.elapsed() >= RELOAD_AFTER)
            });
        s.reloads = wait;
        due
    })
    .unwrap_or_default();
    for entry in due {
        // Still on (it may have been turned off again meanwhile).
        let on = registry::load(&extensions_dir())
            .iter()
            .any(|e| e.id == entry.id && e.enabled);
        if on && with(|s| s.contexts.contains_key(&entry.id)) == Some(false) {
            load(entry);
        }
    }
}

/// What's installed, for the Extensions page.
pub fn list() -> Vec<Extension> {
    ensure();
    let dir = extensions_dir();
    registry::load(&dir)
        .into_iter()
        .map(|entry| {
            let m = manifest::read(&entry.folder_in(&dir)).unwrap_or_default();
            let (ctx, failed) = with(|s| {
                (
                    s.contexts.get(&entry.id).cloned(),
                    s.failed.get(&entry.id).cloned(),
                )
            })
            .unwrap_or_default();
            // It didn't load, or its background stopped (else an error is a page's or a
            // script's, and the rest of it runs).
            let mut broken = failed.is_some();
            let mut problems: Vec<String> = failed.into_iter().collect();
            if let Some(ctx) = ctx {
                // SAFETY: main thread, a live context.
                for e in unsafe { ctx.errors() }.iter() {
                    // WKWebExtensionContextErrorBackgroundContentFailedToLoad.
                    if e.code() == 6 && e.domain().to_string().contains("WebExtensionContext") {
                        broken = true;
                    }
                    let e = e.localizedDescription().to_string();
                    if !problems.contains(&e) {
                        problems.push(e);
                    }
                }
            }
            Extension {
                name: if m.name.is_empty() {
                    entry.id.clone()
                } else {
                    m.name.clone()
                },
                enabled: entry.enabled,
                version: m.version.clone(),
                icon: m.icon.clone(),
                has_options: m.has_options,
                permissions: manifest::describe(&m),
                problems,
                broken,
                id: entry.id,
            }
        })
        .collect()
}

/// Install the unpacked extension in `folder` as `id` (its files stay where they are), on.
pub fn add(folder: &std::path::Path, id: &str) -> Result<Extension, String> {
    if !ensure() {
        return Err(unsupported());
    }
    let dir = extensions_dir();
    let m = manifest::read(folder)?;
    unload(id);
    let own = folder.starts_with(&dir);
    let entry = registry::Installed {
        id: id.to_string(),
        folder: (!own).then(|| folder.to_path_buf()),
        enabled: true,
        ..Default::default()
    };
    registry::update(&dir, |all| registry::put(all, entry.clone()));
    let entry = registry::load(&dir)
        .into_iter()
        .find(|e| e.id == id)
        .unwrap_or(entry);
    with(|s| s.failed.remove(id));
    load_when_free(entry);
    Ok(Extension {
        id: id.to_string(),
        name: m.name,
        enabled: true,
        ..Default::default()
    })
}

pub fn unsupported() -> String {
    "Extensions need macOS 15.4 or later.".to_string()
}

/// Turn extension `id` on or off (`Some`), or remove it (`None`): its files too, when Trek
/// unpacked them, and what it stored.
pub fn change(id: &str, enable: Option<bool>) -> Result<(), String> {
    if !ensure() {
        return Err(unsupported());
    }
    let dir = extensions_dir();
    let Some(entry) = registry::load(&dir).into_iter().find(|e| e.id == id) else {
        return Err("That extension is gone.".to_string());
    };
    match enable {
        Some(on) => {
            registry::update(&dir, |all| {
                if let Some(e) = all.iter_mut().find(|e| e.id == id) {
                    e.enabled = on;
                }
            });
            unload(id);
            if on {
                load_when_free(registry::Installed {
                    enabled: true,
                    ..entry
                });
            }
        }
        None => {
            remove_data(id);
            unload(id);
            registry::update(&dir, |all| all.retain(|e| e.id != id));
            if entry.folder.is_none() {
                let _ = std::fs::remove_dir_all(dir.join(id));
            }
            with(|s| {
                s.failed.remove(id);
                s.prompts.retain(|p| p.ext != id);
            });
        }
    }
    changed();
    Ok(())
}

/// Drop what extension `id` stored (its storage areas), while it's still loaded.
fn remove_data(id: &str) {
    let Some((controller, ctx)) =
        with(|s| Some((s.controller.clone(), s.contexts.get(id)?.clone()))).flatten()
    else {
        return;
    };
    let types = unsafe {
        WKWebExtensionController::allExtensionDataTypes(MainThreadMarker::new_unchecked())
    };
    let remover = controller.clone();
    let all_types = types.clone();
    let found = RcBlock::new(
        move |record: *mut objc2_web_kit::WKWebExtensionDataRecord| {
            // SAFETY: WebKit hands a valid record or nil, on the main thread.
            if let Some(record) = unsafe { Retained::retain(record) } {
                let records = NSArray::from_retained_slice(&[record]);
                let done = RcBlock::new(|| {});
                unsafe {
                    remover.removeDataOfTypes_fromDataRecords_completionHandler(
                        &all_types, &records, &done,
                    )
                };
            }
        },
    );
    // SAFETY: main thread; WebKit copies the block.
    unsafe {
        controller
            .fetchDataRecordOfTypes_forExtensionContext_completionHandler(&types, &ctx, &found)
    };
}

/// Show extension `id`'s options page, in a window of its own.
pub fn open_options(id: &str) {
    let Some(ctx) = with(|s| s.contexts.get(id).cloned()).flatten() else {
        return;
    };
    // SAFETY: main thread, a live context.
    if let Some(url) = unsafe { ctx.optionsPageURL() } {
        open_page(&ctx, &url);
    }
}

/// Open extension page `url` (options, or one it opens as a tab) in a window of its own: such a
/// page needs its extension's own web view configuration, which a Browser tab hasn't.
fn open_page(ctx: &WKWebExtensionContext, url: &NSURL) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // SAFETY: main thread; a new window and view, kept in the state while open.
    unsafe {
        let Some(config) = ctx.webViewConfiguration() else {
            return;
        };
        let name = ctx
            .webExtension()
            .displayName()
            .map(|n| n.to_string())
            .unwrap_or_default();
        let rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(780., 620.));
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect,
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::Miniaturizable,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setReleasedWhenClosed(false);
        window.setTitle(&NSString::from_str(&name));
        let view = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), rect, &config);
        view.loadRequest(&NSURLRequest::requestWithURL(url));
        window.setContentView(Some(&view));
        window.center();
        window.makeKeyAndOrderFront(None);
        with(|s| {
            s.pages.retain(|w| w.isVisible());
            s.pages.push(window);
        });
    }
}

/// An extension's toolbar button.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub badge: String,
    pub icon: Option<PathBuf>,
    pub enabled: bool,
}

/// The toolbar buttons of the extensions that have one, for the tab in front.
pub fn actions() -> Vec<Action> {
    type Loaded = (String, Retained<WKWebExtensionContext>, Option<PathBuf>);
    let Some((contexts, tab)) = with(|s| {
        let mut contexts: Vec<Loaded> = s
            .contexts
            .iter()
            .map(|(k, v)| (k.clone(), v.clone(), s.icons.get(k).cloned().flatten()))
            .collect();
        // In the order they were installed.
        contexts.sort_by_key(|c| (s.rank.get(&c.0).copied().unwrap_or(usize::MAX), c.0.clone()));
        (contexts, focused_tab(s))
    }) else {
        return vec![];
    };
    contexts
        .into_iter()
        .filter_map(|(id, ctx, icon)| {
            let tab = tab.as_deref().map(|t| ProtocolObject::from_ref(t));
            // SAFETY: main thread, a live context.
            let action = unsafe { ctx.actionForTab(tab) }?;
            // SAFETY: main thread, a live action.
            unsafe {
                Some(Action {
                    label: action.label().to_string(),
                    badge: action.badgeText().to_string(),
                    enabled: action.isEnabled(),
                    icon,
                    id,
                })
            }
        })
        .collect()
}

fn focused_tab(s: &State) -> Option<Retained<ExtTab>> {
    let window = s
        .windows
        .iter()
        .find(|w| Some(w.ivars().key) == s.focused)?;
    window.active_tab()
}

/// Click extension `id`'s toolbar button: its popup shows under `anchor` (a rectangle in `view`,
/// from the top left), or the extension hears the click.
pub fn perform_action(id: &str, view: Retained<NSView>, anchor: NSRect) {
    let Some((ctx, tab)) = with(|s| Some((s.contexts.get(id)?.clone(), focused_tab(s)))).flatten()
    else {
        return;
    };
    // AppKit's views count up from the bottom unless flipped.
    let rect = if view.isFlipped() {
        anchor
    } else {
        let height = view.bounds().size.height;
        NSRect::new(
            NSPoint::new(
                anchor.origin.x,
                height - anchor.origin.y - anchor.size.height,
            ),
            anchor.size,
        )
    };
    with(|s| s.anchor = Some((view, rect)));
    let tab = tab.as_deref().map(|t| ProtocolObject::from_ref(t));
    // SAFETY: main thread, no state borrowed (the popup comes back through the delegate).
    unsafe { ctx.performActionForTab(tab) };
}

/// Show `action`'s popup under the button that asked for it (or the top right of the page).
fn present_popup(action: &WKWebExtensionAction) -> Result<(), String> {
    // SAFETY: main thread, a live action.
    let popover = unsafe { action.popupPopover() }.ok_or("The extension has no popup.")?;
    let anchor = with(|s| {
        s.anchor.take().or_else(|| {
            let view = focused_tab(s)?.ivars().view.clone();
            let bounds = view.bounds();
            let at = NSRect::new(
                NSPoint::new(
                    bounds.size.width - 40.,
                    if view.isFlipped() {
                        0.
                    } else {
                        bounds.size.height - 1.
                    },
                ),
                NSSize::new(24., 1.),
            );
            Some((Retained::into_super(view), at))
        })
    })
    .flatten();
    let (view, rect) = anchor.ok_or("There's no Browser to show it in.")?;
    let edge = if view.isFlipped() {
        NSRectEdge::MaxY
    } else {
        NSRectEdge::MinY
    };
    // In Trek's scheme, not the system's (Trek's theme may differ).
    {
        use objc2_app_kit::NSAppearanceCustomization as _;
        popover.setAppearance(Some(&view.effectiveAppearance()));
    }
    popover.showRelativeToRect_ofView_preferredEdge(rect, &view, edge);
    // SAFETY: main thread, a live action.
    let page = unsafe { action.popupWebView() };
    with(|s| s.popup = page);
    Ok(())
}

/// Run `js` in the popup last shown (the shots driver's `ext-popup`): its answer, as text.
pub fn popup_eval(js: &str) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let Some(page) = with(|s| s.popup.clone()).flatten() else {
        let _ = tx.try_send(Err("No popup has been shown.".to_string()));
        return rx;
    };
    let done = RcBlock::new(
        move |value: *mut objc2::runtime::AnyObject, error: *mut NSError| {
            // SAFETY: WebKit hands a valid value or nil, and an error or nil, on the main thread.
            let answer = match unsafe { (value.as_ref(), error.as_ref()) } {
                (_, Some(e)) => Err(e.localizedDescription().to_string()),
                (Some(v), None) => {
                    let text: Retained<NSString> = unsafe { msg_send![v, description] };
                    Ok(text.to_string())
                }
                (None, None) => Ok(String::new()),
            };
            let _ = tx.try_send(answer);
        },
    );
    // SAFETY: main thread, a live view; WebKit copies the block.
    unsafe { page.evaluateJavaScript_completionHandler(&NSString::from_str(js), Some(&done)) };
    rx
}

/// What an extension asks for, waiting for the user's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub id: u64,
    pub extension: String,
    pub name: String,
    /// What it asks for, in words.
    pub asks: Vec<String>,
}

type Answer<T> = RcBlock<dyn Fn(NonNull<NSSet<T>>, *mut NSDate)>;

enum Reply {
    Permissions(
        Answer<WKWebExtensionPermission>,
        Retained<NSSet<WKWebExtensionPermission>>,
    ),
    Urls(Answer<NSURL>, Retained<NSSet<NSURL>>),
    Patterns(
        Answer<WKWebExtensionMatchPattern>,
        Retained<NSSet<WKWebExtensionMatchPattern>>,
    ),
}

struct Pending {
    prompt: Prompt,
    ext: String,
    reply: Reply,
}

pub fn prompts() -> Vec<Prompt> {
    with(|s| s.prompts.iter().map(|p| p.prompt.clone()).collect()).unwrap_or_default()
}

/// Queue `reply` for the user: extension `ctx` asks for `permissions` and `hosts`.
fn ask(ctx: &WKWebExtensionContext, permissions: Vec<String>, hosts: Vec<String>, reply: Reply) {
    // SAFETY: main thread, a live context.
    let (ext, name) = unsafe {
        (
            ctx.uniqueIdentifier().to_string(),
            ctx.webExtension()
                .displayName()
                .map(|n| n.to_string())
                .unwrap_or_default(),
        )
    };
    let mut asks = manifest::describe(&manifest::Manifest {
        permissions: permissions.clone(),
        hosts,
        ..Default::default()
    });
    if asks.is_empty() {
        asks = permissions;
    }
    with(|s| {
        let id = s.next_prompt;
        s.next_prompt += 1;
        s.prompts.push(Pending {
            prompt: Prompt {
                id,
                extension: ext.clone(),
                name,
                asks,
            },
            ext,
            reply,
        });
    });
    changed();
}

/// Answer prompt `id`: everything it asked for (`allow`), or nothing. What's allowed is kept for
/// its next run.
pub fn answer(id: u64, allow: bool) {
    let Some(pending) = with(|s| {
        let at = s.prompts.iter().position(|p| p.prompt.id == id)?;
        Some(s.prompts.remove(at))
    })
    .flatten() else {
        return;
    };
    let mut allowed: Vec<String> = vec![];
    let mut sites: Vec<String> = vec![];
    match &pending.reply {
        Reply::Permissions(done, asked) => {
            let given = if allow { asked.clone() } else { NSSet::new() };
            allowed.extend(given.iter().map(|p| p.to_string()));
            done.call((NonNull::from(&*given), std::ptr::null_mut()));
        }
        Reply::Urls(done, asked) => {
            let given = if allow { asked.clone() } else { NSSet::new() };
            for url in given.iter() {
                if let (Some(scheme), Some(host)) = (url.scheme(), url.host()) {
                    sites.push(format!("{scheme}://{host}/*"));
                }
            }
            done.call((NonNull::from(&*given), std::ptr::null_mut()));
        }
        Reply::Patterns(done, asked) => {
            let given = if allow { asked.clone() } else { NSSet::new() };
            // SAFETY: main thread, live patterns.
            sites.extend(given.iter().map(|m| unsafe { m.string() }.to_string()));
            done.call((NonNull::from(&*given), std::ptr::null_mut()));
        }
    }
    if !allowed.is_empty() || !sites.is_empty() {
        registry::update(&extensions_dir(), |all| {
            if let Some(e) = all.iter_mut().find(|e| e.id == pending.ext) {
                for p in allowed {
                    if !e.allowed.contains(&p) {
                        e.allowed.push(p);
                    }
                }
                for site in sites {
                    if !e.allowed_sites.contains(&site) {
                        e.allowed_sites.push(site);
                    }
                }
            }
        });
    }
    changed();
}

/// Tell WebKit about Browser `key`'s tabs (`tabs`, the one in front `active`): new ones opened,
/// gone ones closed, a new one in front. `open_tab` opens an address in a new tab of it.
pub fn sync_tabs(
    key: u64,
    tabs: Vec<(u64, Retained<WKWebView>)>,
    active: u64,
    open_tab: Rc<dyn Fn(String)>,
) {
    if !ensure() {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some((controller, window, new_window, focus)) = with(|s| {
        s.open_tab = Some(open_tab);
        let found = s.windows.iter().find(|w| w.ivars().key == key).cloned();
        let new_window = found.is_none();
        let window = found.unwrap_or_else(|| {
            let w = ExtWindow::new(mtm, key);
            s.windows.push(w.clone());
            w
        });
        let focus = s.focused.replace(key) != Some(key);
        (s.controller.clone(), window, new_window, focus)
    }) else {
        return;
    };
    let old: Vec<Retained<ExtTab>> = window.ivars().tabs.borrow().clone();
    let mut next = vec![];
    let mut opened = vec![];
    for (id, view) in tabs {
        match old
            .iter()
            .find(|t| t.ivars().id == id && t.ivars().view == view)
        {
            Some(t) => next.push(t.clone()),
            None => {
                let t = ExtTab::new(mtm, id, view, &window);
                opened.push(t.clone());
                next.push(t);
            }
        }
    }
    let closed: Vec<Retained<ExtTab>> = old
        .into_iter()
        .filter(|t| {
            !next
                .iter()
                .any(|n| Retained::as_ptr(n) == Retained::as_ptr(t))
        })
        .collect();
    *window.ivars().tabs.borrow_mut() = next;
    let was = window.ivars().active.replace(Some(active));
    let switched = was != Some(active);
    // The tab in front before, unless it just closed.
    let previous = was.filter(|_| switched).and_then(|p| window.tab(p));
    // SAFETY: main thread, no state borrowed (each call may ask the delegate back).
    unsafe {
        if new_window {
            controller.didOpenWindow(ProtocolObject::from_ref(&*window));
        }
        for t in &opened {
            controller.didOpenTab(ProtocolObject::from_ref(&**t));
        }
        for t in &closed {
            controller.didCloseTab_windowIsClosing(ProtocolObject::from_ref(&**t), false);
        }
        if switched && let Some(t) = window.tab(active) {
            controller.didActivateTab_previousActiveTab(
                ProtocolObject::from_ref(&*t),
                previous.as_deref().map(|p| ProtocolObject::from_ref(p)),
            );
        }
        if focus {
            controller.didFocusWindow(Some(ProtocolObject::from_ref(&*window)));
        }
    }
}

/// Tell WebKit which tabs' address, title or loading changed since last asked (it tells the
/// extensions: `tabs.onUpdated`). Called as the Browser draws.
pub fn sync_tab_properties() {
    let Some((controller, tabs)) = with(|s| {
        let tabs: Vec<Retained<ExtTab>> = s
            .windows
            .iter()
            .flat_map(|w| w.ivars().tabs.borrow().clone())
            .collect();
        (s.controller.clone(), tabs)
    }) else {
        return;
    };
    for t in tabs {
        let changed = t.changed_properties();
        if !changed.is_empty() {
            // SAFETY: main thread, no state borrowed.
            unsafe {
                controller.didChangeTabProperties_forTab(changed, ProtocolObject::from_ref(&*t))
            };
        }
    }
}

fn error(text: &str) -> Retained<NSError> {
    let _ = text;
    NSError::new(1, &NSString::from_str("Trek"))
}

struct WindowIvars {
    key: u64,
    tabs: RefCell<Vec<Retained<ExtTab>>>,
    active: Cell<Option<u64>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars drop themselves.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TrekWebExtensionWindow"]
    #[ivars = WindowIvars]
    struct ExtWindow;

    unsafe impl NSObjectProtocol for ExtWindow {}

    unsafe impl WKWebExtensionWindow for ExtWindow {
        #[unsafe(method_id(tabsForWebExtensionContext:))]
        fn tabs_for(
            &self,
            _context: &WKWebExtensionContext,
        ) -> Retained<NSArray<ProtocolObject<dyn WKWebExtensionTab>>> {
            let tabs: Vec<Retained<ProtocolObject<dyn WKWebExtensionTab>>> = self
                .ivars()
                .tabs
                .borrow()
                .iter()
                .map(|t| ProtocolObject::from_retained(t.clone()))
                .collect();
            NSArray::from_retained_slice(&tabs)
        }

        #[unsafe(method_id(activeTabForWebExtensionContext:))]
        fn active_tab_for(
            &self,
            _context: &WKWebExtensionContext,
        ) -> Option<Retained<ProtocolObject<dyn WKWebExtensionTab>>> {
            self.active_tab().map(ProtocolObject::from_retained)
        }

        #[unsafe(method(windowTypeForWebExtensionContext:))]
        fn kind(&self, _context: &WKWebExtensionContext) -> WKWebExtensionWindowType {
            WKWebExtensionWindowType::Normal
        }

        #[unsafe(method(windowStateForWebExtensionContext:))]
        fn state(&self, _context: &WKWebExtensionContext) -> WKWebExtensionWindowState {
            WKWebExtensionWindowState::Normal
        }

        #[unsafe(method(isPrivateForWebExtensionContext:))]
        fn is_private(&self, _context: &WKWebExtensionContext) -> bool {
            false
        }

        #[unsafe(method(frameForWebExtensionContext:))]
        fn frame(&self, _context: &WKWebExtensionContext) -> NSRect {
            self.ns_window().map(|w| w.frame()).unwrap_or(NSRect::ZERO)
        }

        #[unsafe(method(focusForWebExtensionContext:completionHandler:))]
        fn focus(&self, _context: &WKWebExtensionContext, done: &DynBlock<dyn Fn(*mut NSError)>) {
            done.call((std::ptr::null_mut(),));
        }
    }
);

impl ExtWindow {
    fn new(mtm: MainThreadMarker, key: u64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WindowIvars {
            key,
            tabs: RefCell::new(vec![]),
            active: Cell::new(None),
        });
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }

    fn tab(&self, id: u64) -> Option<Retained<ExtTab>> {
        self.ivars()
            .tabs
            .borrow()
            .iter()
            .find(|t| t.ivars().id == id)
            .cloned()
    }

    fn active_tab(&self) -> Option<Retained<ExtTab>> {
        self.tab(self.ivars().active.get()?)
    }

    fn ns_window(&self) -> Option<Retained<NSWindow>> {
        self.ivars().tabs.borrow().first()?.ivars().view.window()
    }
}

struct TabIvars {
    id: u64,
    view: Retained<WKWebView>,
    window: Weak<ExtWindow>,
    /// The address, title and loading last told WebKit (`changed_properties`).
    last: RefCell<(String, String, bool)>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars drop themselves.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TrekWebExtensionTab"]
    #[ivars = TabIvars]
    struct ExtTab;

    unsafe impl NSObjectProtocol for ExtTab {}

    unsafe impl WKWebExtensionTab for ExtTab {
        #[unsafe(method_id(windowForWebExtensionContext:))]
        fn window_for(
            &self,
            _context: &WKWebExtensionContext,
        ) -> Option<Retained<ProtocolObject<dyn WKWebExtensionWindow>>> {
            self.ivars()
                .window
                .load()
                .map(ProtocolObject::from_retained)
        }

        #[unsafe(method(indexInWindowForWebExtensionContext:))]
        fn index(&self, _context: &WKWebExtensionContext) -> usize {
            self.ivars()
                .window
                .load()
                .and_then(|w| {
                    w.ivars()
                        .tabs
                        .borrow()
                        .iter()
                        .position(|t| t.ivars().id == self.ivars().id)
                })
                .unwrap_or(0)
        }

        #[unsafe(method_id(webViewForWebExtensionContext:))]
        fn web_view(&self, _context: &WKWebExtensionContext) -> Option<Retained<WKWebView>> {
            Some(self.ivars().view.clone())
        }

        #[unsafe(method_id(titleForWebExtensionContext:))]
        fn title(&self, _context: &WKWebExtensionContext) -> Option<Retained<NSString>> {
            // SAFETY: main thread, a live view.
            unsafe { self.ivars().view.title() }
        }

        #[unsafe(method_id(urlForWebExtensionContext:))]
        fn url(&self, _context: &WKWebExtensionContext) -> Option<Retained<NSURL>> {
            // SAFETY: main thread, a live view.
            unsafe { self.ivars().view.URL() }
        }

        #[unsafe(method(isLoadingCompleteForWebExtensionContext:))]
        fn loaded(&self, _context: &WKWebExtensionContext) -> bool {
            // SAFETY: main thread, a live view.
            !unsafe { self.ivars().view.isLoading() }
        }

        #[unsafe(method(isSelectedForWebExtensionContext:))]
        fn selected(&self, _context: &WKWebExtensionContext) -> bool {
            self.ivars()
                .window
                .load()
                .is_some_and(|w| w.ivars().active.get() == Some(self.ivars().id))
        }

        #[unsafe(method(loadURL:forWebExtensionContext:completionHandler:))]
        fn load_url(
            &self,
            url: &NSURL,
            _context: &WKWebExtensionContext,
            done: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            // SAFETY: main thread, a live view.
            unsafe {
                self.ivars()
                    .view
                    .loadRequest(&NSURLRequest::requestWithURL(url))
            };
            done.call((std::ptr::null_mut(),));
        }

        #[unsafe(method(reloadFromOrigin:forWebExtensionContext:completionHandler:))]
        fn reload(
            &self,
            from_origin: bool,
            _context: &WKWebExtensionContext,
            done: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            // SAFETY: main thread, a live view.
            unsafe {
                if from_origin {
                    self.ivars().view.reloadFromOrigin();
                } else {
                    self.ivars().view.reload();
                }
            }
            done.call((std::ptr::null_mut(),));
        }

        #[unsafe(method(goBackForWebExtensionContext:completionHandler:))]
        fn back(&self, _context: &WKWebExtensionContext, done: &DynBlock<dyn Fn(*mut NSError)>) {
            // SAFETY: main thread, a live view.
            unsafe { self.ivars().view.goBack() };
            done.call((std::ptr::null_mut(),));
        }

        #[unsafe(method(goForwardForWebExtensionContext:completionHandler:))]
        fn forward(&self, _context: &WKWebExtensionContext, done: &DynBlock<dyn Fn(*mut NSError)>) {
            // SAFETY: main thread, a live view.
            unsafe { self.ivars().view.goForward() };
            done.call((std::ptr::null_mut(),));
        }
    }
);

impl ExtTab {
    fn new(
        mtm: MainThreadMarker,
        id: u64,
        view: Retained<WKWebView>,
        window: &Retained<ExtWindow>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TabIvars {
            id,
            view,
            window: Weak::from_retained(window),
            last: RefCell::default(),
        });
        // SAFETY: NSObject's init.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.changed_properties();
        this
    }

    /// What changed in the view since last asked: its address, title, loading.
    fn changed_properties(&self) -> WKWebExtensionTabChangedProperties {
        let view = &self.ivars().view;
        // SAFETY: main thread, a live view.
        let now = unsafe {
            (
                view.URL()
                    .and_then(|u| u.absoluteString())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                view.title().map(|t| t.to_string()).unwrap_or_default(),
                view.isLoading(),
            )
        };
        let last = self.ivars().last.replace(now.clone());
        let mut changed = WKWebExtensionTabChangedProperties::empty();
        if last.0 != now.0 {
            changed |= WKWebExtensionTabChangedProperties::URL;
        }
        if last.1 != now.1 {
            changed |= WKWebExtensionTabChangedProperties::Title;
        }
        if last.2 != now.2 {
            changed |= WKWebExtensionTabChangedProperties::Loading;
        }
        changed
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class adds no ivars.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TrekWebExtensionDelegate"]
    struct Delegate;

    impl Delegate {
        /// AppKit's timer for `load_when_free`.
        #[unsafe(method(trekLoadWaiting))]
        fn load_waiting_now(&self) {
            load_waiting();
        }
    }

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl WKWebExtensionControllerDelegate for Delegate {
        #[unsafe(method_id(webExtensionController:openWindowsForExtensionContext:))]
        fn open_windows(
            &self,
            _controller: &WKWebExtensionController,
            _context: &WKWebExtensionContext,
        ) -> Retained<NSArray<ProtocolObject<dyn WKWebExtensionWindow>>> {
            let windows: Vec<Retained<ProtocolObject<dyn WKWebExtensionWindow>>> = with(|s| {
                // The focused one first: extensions take the first as the current window.
                let mut all = s.windows.clone();
                all.sort_by_key(|w| Some(w.ivars().key) != s.focused);
                all.into_iter().map(ProtocolObject::from_retained).collect()
            })
            .unwrap_or_default();
            NSArray::from_retained_slice(&windows)
        }

        #[unsafe(method_id(webExtensionController:focusedWindowForExtensionContext:))]
        fn focused_window(
            &self,
            _controller: &WKWebExtensionController,
            _context: &WKWebExtensionContext,
        ) -> Option<Retained<ProtocolObject<dyn WKWebExtensionWindow>>> {
            with(|s| {
                s.windows
                    .iter()
                    .find(|w| Some(w.ivars().key) == s.focused)
                    .cloned()
            })
            .flatten()
            .map(ProtocolObject::from_retained)
        }

        #[unsafe(method(webExtensionController:openNewTabUsingConfiguration:forExtensionContext:completionHandler:))]
        fn open_new_tab(
            &self,
            controller: &WKWebExtensionController,
            configuration: &WKWebExtensionTabConfiguration,
            _context: &WKWebExtensionContext,
            done: &DynBlock<dyn Fn(*mut ProtocolObject<dyn WKWebExtensionTab>, *mut NSError)>,
        ) {
            // SAFETY: main thread, a live configuration.
            let url = unsafe { configuration.url() };
            open_url(controller, url.as_deref());
            done.call((std::ptr::null_mut(), std::ptr::null_mut()));
        }

        #[unsafe(method(webExtensionController:openNewWindowUsingConfiguration:forExtensionContext:completionHandler:))]
        fn open_new_window(
            &self,
            controller: &WKWebExtensionController,
            configuration: &WKWebExtensionWindowConfiguration,
            _context: &WKWebExtensionContext,
            done: &DynBlock<dyn Fn(*mut ProtocolObject<dyn WKWebExtensionWindow>, *mut NSError)>,
        ) {
            // Trek has one Browser per window: a window's pages open as its tabs.
            // SAFETY: main thread, a live configuration.
            for url in unsafe { configuration.tabURLs() }.iter() {
                open_url(controller, Some(&url));
            }
            done.call((std::ptr::null_mut(), std::ptr::null_mut()));
        }

        #[unsafe(method(webExtensionController:openOptionsPageForExtensionContext:completionHandler:))]
        fn open_options_page(
            &self,
            _controller: &WKWebExtensionController,
            context: &WKWebExtensionContext,
            done: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            // SAFETY: main thread, a live context.
            match unsafe { context.optionsPageURL() } {
                Some(url) => {
                    open_page(context, &url);
                    done.call((std::ptr::null_mut(),));
                }
                None => done.call((Retained::as_ptr(&error("no options page")).cast_mut(),)),
            }
        }

        #[unsafe(method(webExtensionController:promptForPermissions:inTab:forExtensionContext:completionHandler:))]
        fn prompt_permissions(
            &self,
            _controller: &WKWebExtensionController,
            permissions: &NSSet<WKWebExtensionPermission>,
            _tab: Option<&ProtocolObject<dyn WKWebExtensionTab>>,
            context: &WKWebExtensionContext,
            done: &DynBlock<dyn Fn(NonNull<NSSet<WKWebExtensionPermission>>, *mut NSDate)>,
        ) {
            let asked: Vec<String> = permissions.iter().map(|p| p.to_string()).collect();
            ask(
                context,
                asked,
                vec![],
                Reply::Permissions(
                    done.copy(),
                    NSSet::from_retained_slice(&permissions.to_vec()),
                ),
            );
        }

        #[unsafe(method(webExtensionController:promptForPermissionToAccessURLs:inTab:forExtensionContext:completionHandler:))]
        fn prompt_urls(
            &self,
            _controller: &WKWebExtensionController,
            urls: &NSSet<NSURL>,
            _tab: Option<&ProtocolObject<dyn WKWebExtensionTab>>,
            context: &WKWebExtensionContext,
            done: &DynBlock<dyn Fn(NonNull<NSSet<NSURL>>, *mut NSDate)>,
        ) {
            let hosts: Vec<String> = urls
                .iter()
                .filter_map(|u| Some(format!("{}://{}/*", u.scheme()?, u.host()?)))
                .collect();
            ask(
                context,
                vec![],
                hosts,
                Reply::Urls(done.copy(), NSSet::from_retained_slice(&urls.to_vec())),
            );
        }

        #[unsafe(method(webExtensionController:promptForPermissionMatchPatterns:inTab:forExtensionContext:completionHandler:))]
        fn prompt_patterns(
            &self,
            _controller: &WKWebExtensionController,
            patterns: &NSSet<WKWebExtensionMatchPattern>,
            _tab: Option<&ProtocolObject<dyn WKWebExtensionTab>>,
            context: &WKWebExtensionContext,
            done: &DynBlock<dyn Fn(NonNull<NSSet<WKWebExtensionMatchPattern>>, *mut NSDate)>,
        ) {
            // SAFETY: main thread, live patterns.
            let hosts: Vec<String> = patterns
                .iter()
                .map(|m| unsafe { m.string() }.to_string())
                .collect();
            ask(
                context,
                vec![],
                hosts,
                Reply::Patterns(done.copy(), NSSet::from_retained_slice(&patterns.to_vec())),
            );
        }

        #[unsafe(method(webExtensionController:didUpdateAction:forExtensionContext:))]
        fn did_update_action(
            &self,
            _controller: &WKWebExtensionController,
            _action: &WKWebExtensionAction,
            _context: &WKWebExtensionContext,
        ) {
            changed();
        }

        #[unsafe(method(webExtensionController:presentPopupForAction:forExtensionContext:completionHandler:))]
        fn present_popup_for(
            &self,
            _controller: &WKWebExtensionController,
            action: &WKWebExtensionAction,
            _context: &WKWebExtensionContext,
            done: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            match present_popup(action) {
                Ok(()) => done.call((std::ptr::null_mut(),)),
                Err(e) => {
                    tracing::warn!("browser extensions: no popup: {e}");
                    done.call((Retained::as_ptr(&error(&e)).cast_mut(),));
                }
            }
        }
    }
);

/// Open `url` for an extension: its own page in a window of its own, any other in a Browser tab.
fn open_url(controller: &WKWebExtensionController, url: Option<&NSURL>) {
    let Some(url) = url else {
        return;
    };
    // SAFETY: main thread, a live controller.
    if let Some(ctx) = unsafe { controller.extensionContextForURL(url) } {
        open_page(&ctx, url);
        return;
    }
    let Some(address) = url.absoluteString().map(|s| s.to_string()) else {
        return;
    };
    if let Some(open) = with(|s| s.open_tab.clone()).flatten() {
        open(address);
    }
}
