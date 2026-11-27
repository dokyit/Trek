//! The web view an artifact runs in: WKWebView on macOS, WebView2 on Windows, through wry.
//!
//! It sits in a container of its own laid over the window (an `NSView` that clips its subviews; a
//! child window on Windows, whose children Windows clips to it): the container's frame is the part
//! of the card a reader can see (`Placement::clip`) and the web view keeps the card's whole frame
//! inside it, offset, so scrolling moves the page under the transcript's edges without reflowing it.
//!
//! The sandbox: the document is served under a scheme and origin of its own with Trek's CSP
//! (`trek_core::artifact::net`: loads from the open sources and the hosts the user allowed, no
//! others), the frame may not navigate away, new windows and downloads are refused, website data
//! isn't kept (each view a non-persistent store on macOS, InPrivate on Windows), WebRTC is off
//! where the engine has a switch for it, and there's no devtools outside debug builds. The page's
//! one channel to Trek is the bootstrap's messages.
//!
//! A link or window the page asks for is reported with its address (`Event::Wants`) and never
//! followed here. Whether the user clicked for it is for the app to see, not for the page to say:
//!
//! - macOS: every mouse button AppKit hands the app is noted before it's dispatched (a
//!   local event monitor: `platform::Clip::clicked` says when one last went to this view). A
//!   page's script can't make one. Keys don't count: the page's own script could spend a "click"
//!   on an address the key had nothing to do with.
//! - Linux: GTK's own button signals on the web view's widget, likewise.
//! - Windows: the page's view takes its input in WebView2's own processes, where the app sees
//!   none of it. WebView2 says of each new window and each navigation whether a user gesture
//!   started it (`IsUserInitiated`): that is what's reported (`Event::Wants`'s `engine`).

use super::surface::{Placement, Shot, Surface};
use gpui_kit::*;
use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

/// What a view reports, bridged to the card's task.
pub enum Event {
    /// A message from the page (unchecked: `trek_core::artifact::parse_message` checks it).
    Message(String),
    /// The document finished loading.
    Loaded,
    /// The page asked to go somewhere else: a link the user clicked, a new window (a link with
    /// `target=_blank`, `window.open`), or a script sending the frame away. It went nowhere;
    /// `url` is where. `engine`: what the engine itself says about a user gesture having started
    /// it (Windows); `None` where the app watches the clicks itself (`Surface::clicked`).
    Wants { url: String, engine: Option<bool> },
}

/// What a view is made with.
pub struct Spec {
    /// Its document's origin (`trek_core::artifact::origin`).
    pub origin: String,
    /// The document as served; replaced in place (a theme switch), it's what a reload gets.
    pub document: Rc<RefCell<String>>,
    /// The policy it's served under; replaced in place when the user allows it a host (a reload
    /// then gets the new one).
    pub csp: Rc<RefCell<String>>,
    /// The `Connection-Allowlist` header it's served with (`net::connection_allowlist`), kept in
    /// step with the policy; empty for none.
    pub connections: Rc<RefCell<String>>,
    /// A video file a note plays: served, in pieces as the player asks, at `MEDIA_PATH`.
    pub media: Option<std::path::PathBuf>,
    /// The bootstrap script, with the view's secret.
    pub bootstrap: String,
    /// The card's colour: what shows where the page draws nothing (Windows; macOS is see-through).
    pub background: (u8, u8, u8),
    pub events: async_channel::Sender<Event>,
}

/// Serve the document at `/` and nothing anywhere else, with the CSP and the list of what it
/// may connect to (`connections`, if any), never cached.
fn respond(
    document: &str,
    csp: &str,
    connections: &str,
    media: Option<&std::path::Path>,
    path: &str,
    range: Option<&str>,
) -> wry::http::Response<Cow<'static, [u8]>> {
    let base = wry::http::Response::builder()
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff");
    if path == "/" || path.is_empty() {
        let base = if connections.is_empty() {
            base
        } else {
            base.header("Connection-Allowlist", connections)
        };
        base.header("Content-Type", "text/html; charset=utf-8")
            .header("Content-Security-Policy", csp)
            .header("Referrer-Policy", "no-referrer")
            .body(Cow::Owned(document.as_bytes().to_vec()))
            .unwrap_or_default()
    } else if let (Some(file), true) = (media, path == trek_core::artifact::MEDIA_PATH) {
        crate::ide::preview::serve(file, range).map(Cow::Owned)
    } else if let Some((kind, bytes)) = path
        .strip_prefix(trek_core::artifact::KATEX_PATH)
        .and_then(trek_assets::assets::katex_file)
    {
        // The KaTeX a study card draws its formulas with: bundled, nothing fetched.
        base.header("Content-Type", kind)
            .body(bytes)
            .unwrap_or_default()
    } else {
        base.status(404)
            .body(Cow::Borrowed(&[][..]))
            .unwrap_or_default()
    }
}

/// A native artifact view and its container.
pub struct NativeSurface {
    // Declared (and so dropped) before the container it sits in.
    web: wry::WebView,
    clip: platform::Clip,
    shown: Option<Placement>,
    /// Linux: when GTK last handed the view a button or a key.
    #[cfg(target_os = "linux")]
    clicked: Rc<std::cell::Cell<Option<std::time::Instant>>>,
}

impl NativeSurface {
    /// A view for `spec` over the window whose handle is `handle`, hidden until placed. Never
    /// from inside an update (see `ArtifactState::make`).
    pub fn new(spec: Spec, handle: raw_window_handle::RawWindowHandle) -> Result<Self, String> {
        let clip = platform::Clip::new(handle).ok_or("Couldn't make the web view's container.")?;
        let parent = clip.handle();
        let Spec {
            origin,
            document,
            csp,
            connections,
            media,
            bootstrap,
            background,
            events,
        } = spec;
        #[cfg(windows)]
        let watched = (origin.clone(), events.clone());
        let (t1, t2, t3, t4) = (events.clone(), events.clone(), events.clone(), events);
        let nav_origin = origin.clone();
        // A profile of the artifacts' own, beside the Browser panel's and not in it: a page an
        // agent wrote shares no cookies, logins or storage with the user's own browsing.
        #[cfg(windows)]
        let mut context = wry::WebContext::new(Some(
            crate::panels::browser::webview_data_dir().with_file_name("WebView2-pages"),
        ));
        // It mustn't take the keys from Trek: a click on it does.
        #[cfg(windows)]
        let builder = wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_focused(false)
            .with_background_color((background.0, background.1, background.2, 255));
        #[cfg(target_os = "macos")]
        let builder = {
            let _ = background;
            wry::WebViewBuilder::new().with_transparent(true)
        };
        // Linux: an X11 window inside GPUI's shows nothing of GPUI's through it, so the page
        // stands on the card's colour, as on Windows.
        #[cfg(not(any(target_os = "macos", windows)))]
        let builder = wry::WebViewBuilder::new()
            .with_focused(false)
            .with_background_color((background.0, background.1, background.2, 255));
        let web = builder
            .with_url(format!("{origin}/"))
            .with_visible(true)
            .with_devtools(cfg!(debug_assertions))
            .with_incognito(true)
            .with_accept_first_mouse(true)
            .with_back_forward_navigation_gestures(false)
            .with_autoplay(false)
            .with_hotkeys_zoom(false)
            .with_initialization_script_for_main_only(bootstrap, true)
            .with_custom_protocol(trek_core::artifact::SCHEME.into(), move |_, request| {
                respond(
                    &document.borrow(),
                    &csp.borrow(),
                    &connections.borrow(),
                    media.as_deref(),
                    request.uri().path(),
                    request.headers().get("range").and_then(|r| r.to_str().ok()),
                )
            })
            .with_ipc_handler(move |request| {
                let _ = t1.try_send(Event::Message(request.into_body()));
            })
            .with_navigation_handler(move |url| {
                let ok = trek_core::artifact::navigation_allowed(&url, &nav_origin);
                if !ok {
                    tracing::debug!("artifact: navigation refused");
                    // On Windows the engine reports it, with what it knows of a gesture
                    // (`platform::watch`).
                    if !cfg!(windows) {
                        let _ = t4.try_send(Event::Wants { url, engine: None });
                    }
                }
                ok
            })
            .with_new_window_req_handler(move |url, _| {
                if !cfg!(windows) {
                    let _ = t2.try_send(Event::Wants { url, engine: None });
                }
                wry::NewWindowResponse::Deny
            })
            .with_download_started_handler(|_, _| false)
            .with_on_page_load_handler(move |event, _| {
                if matches!(event, wry::PageLoadEvent::Finished) {
                    let _ = t3.try_send(Event::Loaded);
                }
            })
            .build_as_child(&parent)
            .map_err(|e| e.to_string())?;
        // Before the page's first script: its own load is still on its way.
        platform::harden(&web);
        #[cfg(windows)]
        platform::watch(&web, watched.0, watched.1);
        #[cfg(target_os = "linux")]
        let clicked = platform::watch_clicks(&web);
        Ok(Self {
            web,
            clip,
            shown: None,
            #[cfg(target_os = "linux")]
            clicked,
        })
    }
}

impl Surface for NativeSurface {
    fn place(&mut self, at: Option<Placement>, window: &Window) {
        if self.shown == at {
            return;
        }
        match at {
            Some(p) => {
                // The window's points (the layout's times the interface zoom) and the display's
                // own scale: what a native view is placed in.
                let z = window.ui_zoom();
                let p = Placement {
                    body: window.zoom_bounds(p.body),
                    clip: window.zoom_bounds(p.clip),
                    round_bottom: p.round_bottom,
                };
                let s = window.scale_factor() / z;
                self.clip.place(&p, s);
                let inner = p.body.origin - p.clip.origin;
                let rect = if cfg!(windows) {
                    wry::Rect {
                        position: wry::dpi::Position::Physical(wry::dpi::PhysicalPosition::new(
                            (f32::from(inner.x) * s).round() as i32,
                            (f32::from(inner.y) * s).round() as i32,
                        )),
                        size: wry::dpi::Size::Physical(wry::dpi::PhysicalSize::new(
                            (f32::from(p.body.size.width) * s).round() as u32,
                            (f32::from(p.body.size.height) * s).round() as u32,
                        )),
                    }
                } else {
                    wry::Rect {
                        position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(
                            f32::from(inner.x) as f64,
                            f32::from(inner.y) as f64,
                        )),
                        size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(
                            f32::from(p.body.size.width) as f64,
                            f32::from(p.body.size.height) as f64,
                        )),
                    }
                };
                let _ = self.web.set_bounds(rect);
            }
            None => self.clip.hide(),
        }
        self.shown = at;
    }

    fn eval(&self, js: &str) {
        let _ = self.web.evaluate_script(js);
    }

    fn reload(&self) {
        let _ = self.web.reload();
    }

    fn snapshot(&self) -> Option<async_channel::Receiver<Result<Vec<u8>, String>>> {
        platform::snapshot(&self.web)
    }

    fn release_keys(&self) {
        self.clip.release_keys();
    }

    #[cfg(any(test, feature = "test-support", feature = "shots"))]
    fn take_keys(&self) {
        let _ = self.web.focus();
    }

    fn set_background(&self, rgb: (u8, u8, u8)) {
        if !cfg!(target_os = "macos") {
            let _ = self.web.set_background_color((rgb.0, rgb.1, rgb.2, 255));
        }
    }

    fn clicked(&self) -> Option<std::time::Instant> {
        #[cfg(target_os = "macos")]
        {
            self.clip.clicked()
        }
        #[cfg(target_os = "linux")]
        {
            self.clicked.get()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            None
        }
    }

    #[cfg(feature = "shots")]
    fn click_at(&self, x: f64, y: f64) -> Result<(), String> {
        platform::click_at(&self.web, x, y)
    }
}

impl Drop for NativeSurface {
    fn drop(&mut self) {
        self.clip.hide();
    }
}

/// Decode an image a platform gave (TIFF, PNG) into BGRA rows.
pub fn decode(bytes: &[u8]) -> Result<Shot, String> {
    let picture = image::load_from_memory(bytes)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    let (width, height) = picture.dimensions();
    let mut bgra = picture.into_raw();
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Ok(Shot {
        width,
        height,
        bgra,
    })
}

/// A raw handle (the container's) as wry wants a parent.
struct Parent(raw_window_handle::RawWindowHandle);

impl raw_window_handle::HasWindowHandle for Parent {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: the handle is the container's, which outlives the borrow (it's built with it).
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.0) })
    }
}

#[cfg(feature = "shots")]
pub use platform::{keys_holder, shots_keys};

/// Whether a native view inside `window` (a web page: the Browser's, a card's) has the keys,
/// rather than Trek's own view.
pub fn page_has_keys(window: &gpui_kit::Window) -> bool {
    platform::page_has_keys(window)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{Parent, Placement};
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, MainThreadOnly as _};
    use objc2_app_kit::NSView;
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use std::ptr::NonNull;

    /// kCALayerMinXMinYCorner | kCALayerMaxXMinYCorner: the bottom corners of a layer in a view
    /// that isn't flipped.
    const BOTTOM_CORNERS: usize = 1 | 2;
    /// The card's corner radius, less its 1 px edge.
    // scale: ok, as in `super`
    const RADIUS: f64 = (crate::scale::radius::CARD - 1.) as f64;

    /// An `NSView` over the window's GPUI view that clips the web view in it to its frame.
    pub struct Clip {
        view: Retained<NSView>,
        host: Retained<NSView>,
    }

    /// A mouse button or a key the user pressed, as AppKit handed it to the app: in which
    /// window, where (a button, in the window's coordinates) and when.
    #[derive(Clone, Copy)]
    struct Press {
        window: isize,
        at: Option<NSPoint>,
        when: std::time::Instant,
    }

    thread_local! {
        static WATCHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static PRESSED: std::cell::Cell<Option<Press>> = const { std::cell::Cell::new(None) };
    }

    /// NSEventType: left, right and other buttons going down and up. Keys aren't here: a key
    /// the user presses in the page isn't a click on anything (the page's own script can spend
    /// a "click" the gate counts on an address the key had nothing to do with), so only a mouse
    /// button opens a link (`Clip::clicked`).
    const LEFT_DOWN: usize = 1;
    const LEFT_UP: usize = 2;
    const RIGHT_DOWN: usize = 3;
    const RIGHT_UP: usize = 4;
    const OTHER_DOWN: usize = 25;
    const OTHER_UP: usize = 26;

    /// Note every mouse button AppKit hands the app, before it's dispatched (once, for the
    /// app's life): what `Clip::clicked` answers from. These are the system's events; nothing a
    /// page runs can make one, or reach this.
    fn watch_presses() {
        if WATCHING.replace(true) {
            return;
        }
        let mask: u64 = [
            LEFT_DOWN, LEFT_UP, RIGHT_DOWN, RIGHT_UP, OTHER_DOWN, OTHER_UP,
        ]
        .iter()
        .fold(0, |m, t| m | (1u64 << t));
        let note = block2::RcBlock::new(
            |event: NonNull<objc2_app_kit::NSEvent>| -> *mut objc2_app_kit::NSEvent {
                // SAFETY: AppKit's own event, valid for the call.
                let e = unsafe { event.as_ref() };
                PRESSED.set(Some(Press {
                    window: e.windowNumber(),
                    at: Some(e.locationInWindow()),
                    when: std::time::Instant::now(),
                }));
                event.as_ptr()
            },
        );
        // SAFETY: AppKit copies the block and keeps the monitor until it's removed (never).
        unsafe {
            let _: *mut AnyObject = msg_send![objc2::class!(NSEvent), addLocalMonitorForEventsMatchingMask: mask, handler: &*note];
        }
    }

    impl Clip {
        pub fn new(handle: raw_window_handle::RawWindowHandle) -> Option<Self> {
            let raw_window_handle::RawWindowHandle::AppKit(h) = handle else {
                return None;
            };
            let mtm = MainThreadMarker::new()?;
            watch_presses();
            // SAFETY: GPUI's view, alive as long as its window (which outlives every card in it).
            let host: Retained<NSView> = unsafe { Retained::retain(h.ns_view.as_ptr().cast())? };
            let view = NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.)),
            );
            view.setWantsLayer(true);
            // SAFETY: a layer-backed view has a CALayer; these are its documented properties.
            unsafe {
                let layer: *mut AnyObject = msg_send![&*view, layer];
                if let Some(layer) = layer.as_ref() {
                    let _: () = msg_send![layer, setMasksToBounds: true];
                }
            }
            view.setHidden(true);
            host.addSubview(&view);
            Some(Self { view, host })
        }

        pub fn handle(&self) -> Parent {
            let ptr = NonNull::from(&*self.view).cast();
            Parent(raw_window_handle::RawWindowHandle::AppKit(
                raw_window_handle::AppKitWindowHandle::new(ptr),
            ))
        }

        /// Frame the visible part (window px, from the top left) and show it.
        pub fn place(&self, p: &Placement, _scale: f32) {
            let host = self.host.frame();
            let (x, w, h) = (
                f32::from(p.clip.origin.x) as f64,
                f32::from(p.clip.size.width) as f64,
                f32::from(p.clip.size.height) as f64,
            );
            let top = f32::from(p.clip.origin.y) as f64;
            let y = if self.host.isFlipped() {
                top
            } else {
                host.size.height - top - h
            };
            self.view
                .setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)));
            // SAFETY: as in `new`.
            unsafe {
                let layer: *mut AnyObject = msg_send![&*self.view, layer];
                if let Some(layer) = layer.as_ref() {
                    let radius = if p.round_bottom { RADIUS } else { 0. };
                    let _: () = msg_send![layer, setCornerRadius: radius];
                    let _: () = msg_send![layer, setMaskedCorners: BOTTOM_CORNERS];
                }
            }
            if self.view.isHidden() {
                self.view.setHidden(false);
            }
        }

        pub fn hide(&self) {
            if !self.view.isHidden() {
                self.give_back_held_keys();
                self.view.setHidden(true);
            }
        }

        /// When the user last pressed a mouse button over this view, as AppKit handed it to the
        /// app (`watch_presses`). `None`: the last press went elsewhere, or there was none.
        pub fn clicked(&self) -> Option<std::time::Instant> {
            let press = PRESSED.get()?;
            let window = self.host.window()?;
            if window.windowNumber() != press.window || self.view.isHidden() {
                return None;
            }
            let at = press.at?;
            let frame = self.view.convertRect_toView(self.view.bounds(), None);
            let ours = at.x >= frame.origin.x
                && at.x <= frame.origin.x + frame.size.width
                && at.y >= frame.origin.y
                && at.y <= frame.origin.y + frame.size.height;
            ours.then_some(press.when)
        }

        /// Whether a view inside the container is the window's first responder.
        fn holds_keys(&self, window: &objc2_app_kit::NSWindow) -> bool {
            // SAFETY: plain AppKit getters on the main thread; the responder is the window's
            // own, alive for the call.
            unsafe {
                let responder: *mut AnyObject = msg_send![window, firstResponder];
                match responder.as_ref() {
                    Some(r) => {
                        let is_view: bool = msg_send![r, isKindOfClass: objc2::class!(NSView)];
                        is_view && {
                            let inside: bool = msg_send![r, isDescendantOf: &*self.view];
                            inside
                        }
                    }
                    None => false,
                }
            }
        }

        /// Make GPUI's view the window's first responder again.
        pub fn release_keys(&self) {
            if let Some(window) = self.host.window() {
                window.makeFirstResponder(Some(&self.host));
            }
        }

        /// The keys back to GPUI's view if the page (or anything else in the container) has
        /// them, whatever the card thought: AppKit leaves a hidden or removed first responder's
        /// keys with the window itself, which beeps at every key, and GPUI's view doesn't take
        /// them back when it's clicked.
        fn give_back_held_keys(&self) {
            let Some(window) = self.host.window() else {
                return;
            };
            if self.holds_keys(&window) {
                window.makeFirstResponder(Some(&self.host));
            }
        }
    }

    impl Drop for Clip {
        fn drop(&mut self) {
            self.give_back_held_keys();
            self.view.removeFromSuperview();
        }
    }

    /// Whether a view inside the window's own has the keys (a web view), not GPUI's view.
    pub fn page_has_keys(window: &gpui_kit::Window) -> bool {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return false;
        };
        let RawWindowHandle::AppKit(h) = handle.as_raw() else {
            return false;
        };
        let view: *mut AnyObject = h.ns_view.as_ptr().cast();
        // SAFETY: GPUI's live view and its window; plain AppKit getters on the main thread.
        unsafe {
            let ns_window: *mut AnyObject = msg_send![view, window];
            if ns_window.is_null() {
                return false;
            }
            let responder: *mut AnyObject = msg_send![ns_window, firstResponder];
            if responder.is_null() || responder == view {
                return false;
            }
            let is_view: bool = msg_send![responder, isKindOfClass: objc2::class!(NSView)];
            let content: *mut AnyObject = msg_send![ns_window, contentView];
            if !is_view || content.is_null() {
                return false;
            }
            msg_send![responder, isDescendantOf: content]
        }
    }

    /// Who has the keys in `window`: `trek` (GPUI's view), the class of a native view inside it
    /// (`WKWebView`), the class of anything else (the window itself), or `nothing`.
    #[cfg(feature = "shots")]
    pub fn keys_holder(window: &gpui_kit::Window) -> String {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return "nothing".into();
        };
        let RawWindowHandle::AppKit(h) = handle.as_raw() else {
            return "nothing".into();
        };
        let view: *mut AnyObject = h.ns_view.as_ptr().cast();
        // SAFETY: GPUI's live view and its window; plain AppKit getters on the main thread.
        unsafe {
            let ns_window: *mut AnyObject = msg_send![view, window];
            if ns_window.is_null() {
                return "nothing".into();
            }
            let responder: *mut AnyObject = msg_send![ns_window, firstResponder];
            if responder.is_null() {
                return "nothing".into();
            }
            if responder == view {
                return "trek".into();
            }
            (*responder).class().name().to_string_lossy().into_owned()
        }
    }

    /// The capture build's `artifacts lose-keys`: the window takes the keys itself, as AppKit
    /// leaves them when a view that had them goes; `artifacts press-trek`: a press on GPUI's own
    /// view where the composer is, delivered as AppKit delivers one (`mouseDown:`, `mouseUp:`);
    /// `type-native`: a key (`a`) handed to the window as AppKit hands one.
    #[cfg(feature = "shots")]
    pub fn shots_keys(window: &gpui_kit::Window, what: &str) -> Result<(), String> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = HasWindowHandle::window_handle(window).map_err(|e| e.to_string())?;
        let RawWindowHandle::AppKit(h) = handle.as_raw() else {
            return Err("not an AppKit window".into());
        };
        let view: *mut AnyObject = h.ns_view.as_ptr().cast();
        // SAFETY: GPUI's live view and its window, on the main thread; the events are made with
        // AppKit's own constructor and handed to the view as AppKit hands them.
        unsafe {
            let ns_window: *mut AnyObject = msg_send![view, window];
            if ns_window.is_null() {
                return Err("no window".into());
            }
            match what {
                "lose-keys" => {
                    let none: *mut AnyObject = std::ptr::null_mut();
                    let _: bool = msg_send![ns_window, makeFirstResponder: none];
                }
                "press-trek" => {
                    let frame: NSRect = msg_send![view, frame];
                    // The composer's text, a little above the window's bottom edge.
                    let at = NSPoint::new(frame.size.width / 2., 110.);
                    let number: isize = msg_send![ns_window, windowNumber];
                    let context: *mut AnyObject = std::ptr::null_mut();
                    let class = objc2::class!(NSEvent);
                    for kind in [1usize, 2] {
                        let event: *mut AnyObject = msg_send![class, mouseEventWithType: kind, location: at, modifierFlags: 0usize, timestamp: 0f64, windowNumber: number, context: context, eventNumber: 0isize, clickCount: 1isize, pressure: 1f32];
                        if event.is_null() {
                            return Err("AppKit made no event".into());
                        }
                        if kind == 1 {
                            let _: () = msg_send![view, mouseDown: event];
                        } else {
                            let _: () = msg_send![view, mouseUp: event];
                        }
                    }
                }
                "type-native" => {
                    // `a`, down and up, to the window as AppKit hands it a key: to its first
                    // responder, or the window itself when no view has the keys.
                    let chars = objc2_foundation::NSString::from_str("a");
                    let number: isize = msg_send![ns_window, windowNumber];
                    let context: *mut AnyObject = std::ptr::null_mut();
                    let class = objc2::class!(NSEvent);
                    for kind in [10usize, 11] {
                        let event: *mut AnyObject = msg_send![class, keyEventWithType: kind, location: NSPoint::new(0., 0.), modifierFlags: 0usize, timestamp: 0f64, windowNumber: number, context: context, characters: &*chars, charactersIgnoringModifiers: &*chars, isARepeat: false, keyCode: 0u16];
                        if event.is_null() {
                            return Err("AppKit made no event".into());
                        }
                        let _: () = msg_send![ns_window, sendEvent: event];
                    }
                }
                other => return Err(format!("{other}?")),
            }
        }
        Ok(())
    }

    /// What a page's view does without that no policy directive stops in WebKit, by WebKit's own
    /// name for each switch: WebRTC (a peer connection reaches whatever server it's given), and
    /// `<link rel="preconnect">` and its kin (a connection, and a DNS lookup, to any host named).
    /// DNS prefetching too: WebKit dropped the privacy-invasive kinds itself, and the
    /// `<link rel="dns-prefetch">` switch (`LinkDNSPrefetchEnabled`, gone again in WebKit's
    /// 2026 releases, where the feature is simply on) is named under both names it had.
    const OFF: &[&str] = &[
        "PeerConnectionEnabled",
        "LinkPreconnect",
        "LinkPreconnectEarlyHintsEnabled",
        "LinkPrefetchEnabled",
        "SpeculationRulesPrefetchEnabled",
        "LinkDNSPrefetchEnabled",
        "DNSPrefetchingEnabled",
    ];

    /// Turn `OFF` off in this view (its preferences are its own: the Browser's pages keep
    /// theirs). The switches are WebKit's own, not in its public headers, so each is used only
    /// if this WebKit has it; one it lacks is logged, and for WebRTC the bootstrap's removal of
    /// the constructors still stands.
    pub fn harden(web: &wry::WebView) {
        use wry::WebViewExtMacOS as _;
        let view = web.webview();
        let mut off: Vec<String> = Vec::new();
        // SAFETY: the view's own configuration and preferences, on the main thread; each
        // selector is called only if WebKit answers to it, with the types its headers give.
        unsafe {
            let config: *mut AnyObject = msg_send![&*view, configuration];
            if config.is_null() {
                return;
            }
            let prefs: *mut AnyObject = msg_send![config, preferences];
            let Some(prefs) = prefs.as_ref() else {
                return;
            };
            let has: bool =
                msg_send![prefs, respondsToSelector: objc2::sel!(_setPeerConnectionEnabled:)];
            if has {
                let _: () = msg_send![prefs, _setPeerConnectionEnabled: false];
                off.push(OFF[0].to_string());
            }
            let class = objc2::class!(WKPreferences);
            let lists: bool = msg_send![class, respondsToSelector: objc2::sel!(_features)];
            let sets: bool =
                msg_send![prefs, respondsToSelector: objc2::sel!(_setEnabled:forFeature:)];
            if lists && sets {
                let features: *mut AnyObject = msg_send![class, _features];
                let count: usize = if features.is_null() {
                    0
                } else {
                    msg_send![features, count]
                };
                for i in 0..count {
                    let feature: *mut AnyObject = msg_send![features, objectAtIndex: i];
                    let key: *mut objc2_foundation::NSString = msg_send![feature, key];
                    let Some(key) = key.as_ref().map(|k| k.to_string()) else {
                        continue;
                    };
                    if OFF.contains(&key.as_str()) {
                        let _: () = msg_send![prefs, _setEnabled: false, forFeature: feature];
                        if !off.contains(&key) {
                            off.push(key);
                        }
                    }
                }
            }
        }
        for name in OFF.iter().filter(|n| !off.iter().any(|o| o == *n)) {
            tracing::warn!("artifact: this WebKit has no switch named {name}");
        }
    }

    /// The capture build's `artifacts click-page x y`: a click at (`x`, `y`) of the page (CSS px
    /// from its top left), made as AppKit makes one and handed to the app as AppKit hands one
    /// (`sendEvent:`), so it passes the event monitor and reaches the page as the user's own.
    #[cfg(feature = "shots")]
    pub fn click_at(web: &wry::WebView, x: f64, y: f64) -> Result<(), String> {
        use wry::WebViewExtMacOS as _;
        let view = web.webview();
        // SAFETY: the live view and its window on the main thread; the events are made with
        // AppKit's own constructor.
        unsafe {
            let window: *mut AnyObject = msg_send![&*view, window];
            if window.is_null() {
                return Err("the page's view is in no window".into());
            }
            let flipped: bool = msg_send![&*view, isFlipped];
            let bounds: NSRect = msg_send![&*view, bounds];
            let local = NSPoint::new(x, if flipped { y } else { bounds.size.height - y });
            let none: *mut AnyObject = std::ptr::null_mut();
            let at: NSPoint = msg_send![&*view, convertPoint: local, toView: none];
            let number: isize = msg_send![window, windowNumber];
            let app: *mut AnyObject = msg_send![objc2::class!(NSApplication), sharedApplication];
            for kind in [LEFT_DOWN, LEFT_UP] {
                let event: *mut AnyObject = msg_send![objc2::class!(NSEvent), mouseEventWithType: kind, location: at, modifierFlags: 0usize, timestamp: 0f64, windowNumber: number, context: none, eventNumber: 0isize, clickCount: 1isize, pressure: 1f32];
                if event.is_null() {
                    return Err("AppKit made no event".into());
                }
                let _: () = msg_send![app, sendEvent: event];
            }
        }
        Ok(())
    }

    /// The page as WebKit draws it: a TIFF.
    pub fn snapshot(
        web: &wry::WebView,
    ) -> Option<async_channel::Receiver<Result<Vec<u8>, String>>> {
        use objc2_app_kit::NSImage;
        use objc2_foundation::NSError;
        use wry::WebViewExtMacOS as _;
        let (tx, rx) = async_channel::bounded(1);
        let done = block2::RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
            // SAFETY: WebKit hands a valid image or nil.
            let tiff = unsafe { image.as_ref() }
                .and_then(|i| i.TIFFRepresentation())
                .map(|d| d.to_vec());
            let _ = tx.try_send(tiff.ok_or_else(|| "WebKit took no snapshot".to_string()));
        });
        let view = web.webview();
        // SAFETY: called on the main thread with a live view; WebKit copies the block.
        unsafe { view.takeSnapshotWithConfiguration_completionHandler(None, &done) };
        Some(rx)
    }
}

#[cfg(windows)]
mod platform {
    use super::{Parent, Placement};
    use std::num::NonZeroIsize;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetParent, HWND_TOP, IsChild, IsWindowVisible, SW_HIDE,
        SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow, WS_CHILD, WS_CLIPCHILDREN,
        WS_CLIPSIBLINGS,
    };

    /// A child window over GPUI's that clips the web view in it (Windows clips a child window's
    /// children to it).
    pub struct Clip {
        hwnd: HWND,
    }

    impl Clip {
        pub fn new(handle: raw_window_handle::RawWindowHandle) -> Option<Self> {
            let raw_window_handle::RawWindowHandle::Win32(h) = handle else {
                return None;
            };
            let parent = h.hwnd.get() as HWND;
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: a plain child window of GPUI's, on its thread; destroyed in `drop`.
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                    0,
                    0,
                    0,
                    0,
                    parent,
                    std::ptr::null_mut(),
                    GetModuleHandleW(std::ptr::null()),
                    std::ptr::null(),
                )
            };
            (!hwnd.is_null()).then_some(Self { hwnd })
        }

        pub fn handle(&self) -> Parent {
            let hwnd = NonZeroIsize::new(self.hwnd as isize).expect("a window");
            Parent(raw_window_handle::RawWindowHandle::Win32(
                raw_window_handle::Win32WindowHandle::new(hwnd),
            ))
        }

        /// Frame the visible part (window px from the top left, in GPUI's logical px) and show it.
        pub fn place(&self, p: &Placement, scale: f32) {
            let at = |v: gpui_kit::Pixels| (f32::from(v) * scale).round() as i32;
            let (x, y) = (at(p.clip.origin.x), at(p.clip.origin.y));
            let (w, h) = (
                at(p.clip.origin.x + p.clip.size.width) - x,
                at(p.clip.origin.y + p.clip.size.height) - y,
            );
            // SAFETY: our own child window.
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    HWND_TOP,
                    x,
                    y,
                    w,
                    h,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
            };
        }

        pub fn hide(&self) {
            // SAFETY: our own child window.
            unsafe {
                if IsWindowVisible(self.hwnd) != 0 {
                    self.give_back_held_keys();
                    ShowWindow(self.hwnd, SW_HIDE);
                }
            }
        }

        /// The keys back to GPUI's window if the page (or anything else in the container) has
        /// them, whatever the card thought: a hidden window keeps the keys, and a destroyed one
        /// leaves them with no window at all, so nothing types until something takes them.
        fn give_back_held_keys(&self) {
            // SAFETY: plain queries of our own child window and the thread's focus.
            unsafe {
                let focus = GetFocus();
                if !focus.is_null() && (focus == self.hwnd || IsChild(self.hwnd, focus) != 0) {
                    self.release_keys();
                }
            }
        }

        /// The keys back to GPUI's window.
        pub fn release_keys(&self) {
            // SAFETY: our parent, GPUI's window, on its thread.
            unsafe {
                let parent = GetParent(self.hwnd);
                if !parent.is_null() {
                    SetFocus(parent);
                }
            }
        }
    }

    impl Drop for Clip {
        fn drop(&mut self) {
            self.give_back_held_keys();
            // SAFETY: our own child window; the web view in it went first.
            unsafe { DestroyWindow(self.hwnd) };
        }
    }

    /// Whether a window inside GPUI's has the keys (a page), not GPUI's own.
    pub fn page_has_keys(window: &gpui_kit::Window) -> bool {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return false;
        };
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return false;
        };
        let hwnd = h.hwnd.get() as HWND;
        // SAFETY: queries of the thread's focus and GPUI's own window.
        unsafe {
            let focus = GetFocus();
            !focus.is_null() && focus != hwnd && IsChild(hwnd, focus) != 0
        }
    }

    /// Who has the keys in `window`: `trek` (GPUI's window), `child` (a window inside it, a
    /// page), `other`, or `nothing`.
    #[cfg(feature = "shots")]
    pub fn keys_holder(window: &gpui_kit::Window) -> String {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return "nothing".into();
        };
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return "nothing".into();
        };
        let hwnd = h.hwnd.get() as HWND;
        // SAFETY: queries of the thread's focus and GPUI's own window.
        unsafe {
            let focus = GetFocus();
            if focus.is_null() {
                "nothing".into()
            } else if focus == hwnd {
                "trek".into()
            } else if IsChild(hwnd, focus) != 0 {
                "child".into()
            } else {
                "other".into()
            }
        }
    }

    #[cfg(feature = "shots")]
    pub fn shots_keys(_: &gpui_kit::Window, what: &str) -> Result<(), String> {
        Err(format!("{what}: macOS only"))
    }

    /// Nothing to switch off here: WebView2 has no setting for WebRTC. The policy's `webrtc`
    /// directive and the bootstrap's removal of the constructors are what stand.
    pub fn harden(_: &wry::WebView) {}

    /// Report what the page asks to open, with what WebView2 says of a user gesture: a new
    /// window (`NewWindowRequested`; never made, whoever else hears the event) and a navigation
    /// away from the document at `origin` (`NavigationStarting`; cancelled here as wry's own
    /// handler cancels it).
    pub fn watch(web: &wry::WebView, origin: String, events: async_channel::Sender<super::Event>) {
        use webview2_com::{NavigationStartingEventHandler, NewWindowRequestedEventHandler};
        use windows_61::core::{BOOL, PWSTR};
        use wry::WebViewExtWindows as _;
        fn text(get: impl FnOnce(*mut PWSTR) -> windows_61::core::Result<()>) -> String {
            let mut s = PWSTR::null();
            match get(&mut s) {
                Ok(()) => webview2_com::take_pwstr(s),
                Err(_) => String::new(),
            }
        }
        let webview = web.webview();
        let mut token = 0;
        let (opened, away) = (events.clone(), events);
        // SAFETY: COM calls on the view's own thread, which WebView2 calls the handlers back
        // on; the arguments are each event's own.
        unsafe {
            let _ = webview.add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let _ = args.SetHandled(true);
                    let mut user = BOOL::default();
                    let said = args.IsUserInitiated(&mut user).is_ok();
                    let url = text(|p| args.Uri(p));
                    let _ = opened.try_send(super::Event::Wants {
                        url,
                        engine: Some(said && user.as_bool()),
                    });
                    Ok(())
                })),
                &mut token,
            );
            let _ = webview.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let url = text(|p| args.Uri(p));
                    if trek_core::artifact::navigation_allowed(&url, &origin) {
                        return Ok(());
                    }
                    let _ = args.SetCancel(true);
                    let mut user = BOOL::default();
                    let said = args.IsUserInitiated(&mut user).is_ok();
                    let _ = away.try_send(super::Event::Wants {
                        url,
                        engine: Some(said && user.as_bool()),
                    });
                    Ok(())
                })),
                &mut token,
            );
        }
    }

    #[cfg(feature = "shots")]
    pub fn click_at(_: &wry::WebView, _: f64, _: f64) -> Result<(), String> {
        Err("click-page: macOS only".into())
    }

    /// The page as WebView2 draws it (CapturePreview): a PNG.
    pub fn snapshot(
        web: &wry::WebView,
    ) -> Option<async_channel::Receiver<Result<Vec<u8>, String>>> {
        Some(crate::panels::browser::capture_png(web))
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{Parent, Placement};
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{
        ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, InputFocus, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;

    /// An X11 child window over GPUI's that clips the web view in it (X clips a child window's
    /// children to it), as Windows' child window does. Made on a connection of its own, which
    /// lives as long as it: X destroys a client's windows when its connection closes.
    pub struct Clip {
        conn: RustConnection,
        window: u32,
        parent: u32,
    }

    impl Clip {
        pub fn new(handle: raw_window_handle::RawWindowHandle) -> Option<Self> {
            let raw_window_handle::RawWindowHandle::Xlib(h) =
                crate::linux_gtk::web_view_parent(handle)?
            else {
                return None;
            };
            let parent = u32::try_from(h.window).ok()?;
            let (conn, _) = x11rb::connect(None).ok()?;
            // GPUI's window may be 32 bits deep (see-through): the child takes its depth, visual
            // and colormap, or X refuses it.
            let attrs = conn.get_window_attributes(parent).ok()?.reply().ok()?;
            let depth = conn.get_geometry(parent).ok()?.reply().ok()?.depth;
            let window = conn.generate_id().ok()?;
            conn.create_window(
                depth,
                window,
                parent,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                attrs.visual,
                &CreateWindowAux::new()
                    .colormap(attrs.colormap)
                    .border_pixel(0)
                    .background_pixel(0),
            )
            .ok()?;
            // A round trip, so the server has made it before wry (on GTK's connection) puts the
            // web view's window inside it.
            conn.get_input_focus().ok()?.reply().ok()?;
            Some(Self {
                conn,
                window,
                parent,
            })
        }

        pub fn handle(&self) -> Parent {
            Parent(raw_window_handle::RawWindowHandle::Xlib(
                raw_window_handle::XlibWindowHandle::new(std::ffi::c_ulong::from(self.window)),
            ))
        }

        /// Frame the visible part (window px from the top left, in GPUI's logical px) and show it.
        pub fn place(&self, p: &Placement, scale: f32) {
            let at = |v: gpui_kit::Pixels| (f32::from(v) * scale).round() as i32;
            let (x, y) = (at(p.clip.origin.x), at(p.clip.origin.y));
            let (w, h) = (
                at(p.clip.origin.x + p.clip.size.width) - x,
                at(p.clip.origin.y + p.clip.size.height) - y,
            );
            let _ = self.conn.configure_window(
                self.window,
                &ConfigureWindowAux::new()
                    .x(x)
                    .y(y)
                    .width(w.max(1) as u32)
                    .height(h.max(1) as u32)
                    .stack_mode(x11rb::protocol::xproto::StackMode::ABOVE),
            );
            let _ = self.conn.map_window(self.window);
            let _ = self.conn.flush();
        }

        pub fn hide(&self) {
            let _ = self.conn.unmap_window(self.window);
            let _ = self.conn.flush();
        }

        /// The keys back to GPUI's window.
        pub fn release_keys(&self) {
            let _ = self
                .conn
                .set_input_focus(InputFocus::PARENT, self.parent, x11rb::CURRENT_TIME);
            let _ = self.conn.flush();
        }
    }

    impl Drop for Clip {
        fn drop(&mut self) {
            let _ = self.conn.destroy_window(self.window);
            let _ = self.conn.flush();
        }
    }

    pub fn page_has_keys(_: &gpui_kit::Window) -> bool {
        false
    }

    /// What a page's view does without that no policy directive stops in WebKit, by WebKit's own
    /// name for each switch (as on the Mac): WebRTC, and `<link rel="preconnect">` and its kin.
    const OFF: &[&str] = &[
        "PeerConnectionEnabled",
        "LinkPreconnect",
        "LinkPreconnectEarlyHintsEnabled",
        "LinkPrefetchEnabled",
        "SpeculationRulesPrefetchEnabled",
    ];

    /// No WebRTC in a page's view, no window a script opens by itself (both WebKitGTK's
    /// defaults, set here so they don't depend on it), no DNS prefetching, and `OFF` switched
    /// off where this WebKitGTK has its feature switches.
    pub fn harden(web: &wry::WebView) {
        use webkit2gtk::{SettingsExt as _, WebViewExt as _};
        use wry::WebViewExtUnix as _;
        let Some(settings) = web.webview().settings() else {
            return;
        };
        settings.set_enable_webrtc(false);
        settings.set_javascript_can_open_windows_automatically(false);
        settings.set_enable_dns_prefetching(false);
        let off = features_off(&settings);
        for name in OFF.iter().skip(1).filter(|n| !off.iter().any(|o| o == *n)) {
            tracing::warn!("artifact: this WebKitGTK has no switch named {name}");
        }
    }

    /// Switch `OFF` off in `settings` through WebKitGTK's feature list (2.42 and later), and
    /// say which were found. The bindings Trek builds with stop at 2.40, so the six functions
    /// are looked up in the running library by name: an older WebKitGTK has none of them, and
    /// nothing is called.
    fn features_off(settings: &webkit2gtk::Settings) -> Vec<String> {
        use gtk::glib::object::ObjectType as _;
        use std::ffi::{CStr, c_char, c_int, c_void};
        /// The function named `name` in the running process, as a pointer to cast.
        fn find(name: &CStr) -> Option<*mut c_void> {
            // SAFETY: a lookup by name among the libraries already loaded; no call is made.
            let found = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
            (!found.is_null()).then_some(found)
        }
        type All = unsafe extern "C" fn() -> *mut c_void;
        type Length = unsafe extern "C" fn(*mut c_void) -> usize;
        type Get = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;
        type Identifier = unsafe extern "C" fn(*mut c_void) -> *const c_char;
        type Set = unsafe extern "C" fn(*mut c_void, *mut c_void, c_int);
        type Unref = unsafe extern "C" fn(*mut c_void);
        let (Some(all), Some(length), Some(get), Some(identifier), Some(set), Some(unref)) = (
            find(c"webkit_settings_get_all_features"),
            find(c"webkit_feature_list_get_length"),
            find(c"webkit_feature_list_get"),
            find(c"webkit_feature_get_identifier"),
            find(c"webkit_settings_set_feature_enabled"),
            find(c"webkit_feature_list_unref"),
        ) else {
            return Vec::new();
        };
        let mut off = Vec::new();
        // SAFETY: WebKitGTK's own functions, found by their documented names and called with
        // their documented signatures (`WebKitFeatureList* webkit_settings_get_all_features(void)`,
        // `gsize webkit_feature_list_get_length(WebKitFeatureList*)`, `WebKitFeature*
        // webkit_feature_list_get(WebKitFeatureList*, gsize)`, `const char*
        // webkit_feature_get_identifier(WebKitFeature*)`, `void
        // webkit_settings_set_feature_enabled(WebKitSettings*, WebKitFeature*, gboolean)`, `void
        // webkit_feature_list_unref(WebKitFeatureList*)`), on GTK's thread, with the view's own
        // settings; the list is held until the loop is done, and each name is copied out.
        unsafe {
            let all: All = std::mem::transmute(all);
            let length: Length = std::mem::transmute(length);
            let get: Get = std::mem::transmute(get);
            let identifier: Identifier = std::mem::transmute(identifier);
            let set: Set = std::mem::transmute(set);
            let unref: Unref = std::mem::transmute(unref);
            let list = all();
            if list.is_null() {
                return off;
            }
            for i in 0..length(list) {
                let feature = get(list, i);
                if feature.is_null() {
                    continue;
                }
                let name = identifier(feature);
                if name.is_null() {
                    continue;
                }
                let name = CStr::from_ptr(name).to_string_lossy().into_owned();
                if OFF.contains(&name.as_str()) {
                    set(settings.as_ptr().cast(), feature, 0);
                    off.push(name);
                }
            }
            unref(list);
        }
        off
    }

    /// Note when GTK hands the page's view a mouse button: the system's own events, which
    /// nothing a page runs can make. Keys aren't clicks (as on the Mac): the page's own script
    /// could spend one on an address the key had nothing to do with.
    pub fn watch_clicks(
        web: &wry::WebView,
    ) -> std::rc::Rc<std::cell::Cell<Option<std::time::Instant>>> {
        use gtk::prelude::WidgetExt as _;
        use wry::WebViewExtUnix as _;
        let clicked = std::rc::Rc::new(std::cell::Cell::new(None));
        let view = web.webview();
        let (down, up) = (clicked.clone(), clicked.clone());
        view.connect_button_press_event(move |_, _| {
            down.set(Some(std::time::Instant::now()));
            gtk::glib::Propagation::Proceed
        });
        view.connect_button_release_event(move |_, _| {
            up.set(Some(std::time::Instant::now()));
            gtk::glib::Propagation::Proceed
        });
        clicked
    }

    #[cfg(feature = "shots")]
    pub fn click_at(_: &wry::WebView, _: f64, _: f64) -> Result<(), String> {
        Err("click-page: macOS only".into())
    }

    #[cfg(feature = "shots")]
    pub fn keys_holder(_: &gpui_kit::Window) -> String {
        "trek".into()
    }

    #[cfg(feature = "shots")]
    pub fn shots_keys(_: &gpui_kit::Window, what: &str) -> Result<(), String> {
        Err(format!("{what}: macOS only"))
    }

    pub fn snapshot(_: &wry::WebView) -> Option<async_channel::Receiver<Result<Vec<u8>, String>>> {
        None
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod platform {
    use super::{Parent, Placement};

    pub struct Clip;

    impl Clip {
        pub fn new(_: raw_window_handle::RawWindowHandle) -> Option<Self> {
            None
        }
        pub fn handle(&self) -> Parent {
            unreachable!("no container is made here")
        }
        pub fn place(&self, _: &Placement, _: f32) {}
        pub fn hide(&self) {}
        pub fn release_keys(&self) {}
    }

    pub fn page_has_keys(_: &gpui_kit::Window) -> bool {
        false
    }

    pub fn harden(_: &wry::WebView) {}

    #[cfg(feature = "shots")]
    pub fn click_at(_: &wry::WebView, _: f64, _: f64) -> Result<(), String> {
        Err("click-page: macOS only".into())
    }

    #[cfg(feature = "shots")]
    pub fn keys_holder(_: &gpui_kit::Window) -> String {
        "trek".into()
    }

    #[cfg(feature = "shots")]
    pub fn shots_keys(_: &gpui_kit::Window, what: &str) -> Result<(), String> {
        Err(format!("{what}: macOS only"))
    }

    pub fn snapshot(_: &wry::WebView) -> Option<async_channel::Receiver<Result<Vec<u8>, String>>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::respond;

    #[test]
    fn the_document_is_served_at_its_root_with_the_csp_and_nothing_else_is() {
        let page = respond(
            "<p>hi</p>",
            &trek_core::artifact::net::page_csp(&[]),
            "",
            None,
            "/",
            None,
        );
        assert_eq!(page.status(), 200);
        assert_eq!(
            page.headers()["content-security-policy"],
            &trek_core::artifact::net::page_csp(&[])
        );
        assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
        assert_eq!(page.headers()["cache-control"], "no-store");
        assert_eq!(&page.body()[..], b"<p>hi</p>");
        for other in [
            "/favicon.ico",
            "/../../etc/passwd",
            "/x.js",
            "/_trek/katex/../../secret",
            "/_trek/katex/fonts/../katex.min.js",
            "/_trek/katex/nothing.js",
        ] {
            let r = respond(
                "<p>hi</p>",
                &trek_core::artifact::net::page_csp(&[]),
                "",
                None,
                other,
                None,
            );
            assert_eq!(r.status(), 404, "{other}");
            assert!(r.body().is_empty());
        }
    }

    #[test]
    fn the_bundled_katex_is_served_from_the_documents_own_origin() {
        let path = |rel: &str| format!("{}{rel}", trek_core::artifact::KATEX_PATH);
        for (rel, kind) in [
            ("katex.min.js", "text/javascript"),
            ("auto-render.min.js", "text/javascript"),
            ("katex.min.css", "text/css"),
            ("fonts/KaTeX_Main-Regular.woff2", "font/woff2"),
        ] {
            let r = respond(
                "<p>hi</p>",
                &trek_core::artifact::net::study_csp(&[]),
                "",
                None,
                &path(rel),
                None,
            );
            assert_eq!(r.status(), 200, "{rel}");
            assert!(r.body().len() > 1000, "{rel}");
            assert_eq!(r.headers()["content-type"], kind);
        }
    }

    #[test]
    fn a_notes_video_is_served_in_pieces_at_its_path_and_nothing_else() {
        let file = std::env::temp_dir().join(format!("trek-media-{}.mp4", std::process::id()));
        std::fs::write(&file, (0..=255u8).collect::<Vec<u8>>()).unwrap();
        let csp = trek_core::artifact::VIDEO_CSP;
        let media = trek_core::artifact::MEDIA_PATH;
        let part = respond("<video>", csp, "", Some(&file), media, Some("bytes=10-19"));
        assert_eq!(part.status(), 206);
        assert_eq!(&part.body()[..], &(10..20u8).collect::<Vec<u8>>()[..]);
        assert_eq!(part.headers()["content-range"], "bytes 10-19/256");
        assert_eq!(part.headers()["content-type"], "video/mp4");
        assert_eq!(
            respond("<video>", csp, "", Some(&file), media, None).status(),
            200
        );
        // The page carries the video policy; another path, or a document with no file, is a 404.
        assert_eq!(
            respond("<video>", csp, "", Some(&file), "/", None).headers()["content-security-policy"],
            csp
        );
        assert_eq!(
            respond("<video>", csp, "", Some(&file), "/other.mp4", None).status(),
            404
        );
        assert_eq!(
            respond(
                "<p>hi</p>",
                &trek_core::artifact::net::page_csp(&[]),
                "",
                None,
                media,
                Some("bytes=0-1")
            )
            .status(),
            404
        );
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn pictures_are_handed_on_as_bgra() {
        let mut png = Vec::new();
        let picture =
            image::RgbaImage::from_raw(2, 1, vec![10, 20, 30, 255, 1, 2, 3, 128]).unwrap();
        picture
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let shot = super::decode(&png).unwrap();
        assert_eq!((shot.width, shot.height), (2, 1));
        assert_eq!(shot.bgra, vec![30, 20, 10, 255, 3, 2, 1, 128]);
    }
}
