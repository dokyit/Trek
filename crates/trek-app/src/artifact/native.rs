//! The web view an artifact runs in: WKWebView on macOS, WebView2 on Windows, through wry.
//!
//! It sits in a container of its own laid over the window (an `NSView` that clips its subviews; a
//! child window on Windows, whose children Windows clips to it): the container's frame is the part
//! of the card a reader can see (`Placement::clip`) and the web view keeps the card's whole frame
//! inside it, offset, so scrolling moves the page under the transcript's edges without reflowing it.
//!
//! The sandbox: the document is served under a scheme and origin of its own with Trek's CSP
//! (`trek_core::artifact::CSP`: https loads only), the frame may not navigate away, new windows and
//! downloads are refused (a link the reader clicks goes to their browser, through Trek), website
//! data isn't kept (each view a non-persistent store on macOS, InPrivate on Windows), and there's
//! no devtools outside debug builds. The page's one channel to Trek is the bootstrap's messages.

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
    /// The page asked for a new window (a link with `target=_blank`, `window.open`): the URL.
    Popup(String),
}

/// What a view is made with.
pub struct Spec {
    /// Its document's origin (`trek_core::artifact::origin`).
    pub origin: String,
    /// The document as served; replaced in place (a theme switch), it's what a reload gets.
    pub document: Rc<RefCell<String>>,
    /// The bootstrap script, with the view's secret.
    pub bootstrap: String,
    /// The card's colour: what shows where the page draws nothing (Windows; macOS is see-through).
    pub background: (u8, u8, u8),
    pub events: async_channel::Sender<Event>,
}

/// Serve the document at `/` and nothing anywhere else, with the CSP, never cached.
fn respond(document: &str, path: &str) -> wry::http::Response<Cow<'static, [u8]>> {
    let base = wry::http::Response::builder().header("Cache-Control", "no-store").header("X-Content-Type-Options", "nosniff");
    if path == "/" || path.is_empty() {
        base.header("Content-Type", "text/html; charset=utf-8")
            .header("Content-Security-Policy", trek_core::artifact::CSP)
            .body(Cow::Owned(document.as_bytes().to_vec()))
            .unwrap_or_default()
    } else {
        base.status(404).body(Cow::Borrowed(&[][..])).unwrap_or_default()
    }
}

/// A native artifact view and its container.
pub struct NativeSurface {
    // Declared (and so dropped) before the container it sits in.
    web: wry::WebView,
    clip: platform::Clip,
    shown: Option<Placement>,
}

impl NativeSurface {
    /// A view for `spec` over `window`, hidden until placed.
    pub fn new(spec: Spec, window: &Window) -> Result<Self, String> {
        let handle = raw_window_handle::HasWindowHandle::window_handle(window).map_err(|e| e.to_string())?.as_raw();
        let clip = platform::Clip::new(handle).ok_or("Couldn't make the web view's container.")?;
        let parent = clip.handle();
        let Spec { origin, document, bootstrap, background, events } = spec;
        let (t1, t2, t3) = (events.clone(), events.clone(), events);
        let nav_origin = origin.clone();
        #[cfg(windows)]
        let mut context = wry::WebContext::new(Some(crate::panels::browser::webview_data_dir()));
        // It mustn't take the keys from Trek: a click on it does.
        #[cfg(windows)]
        let builder = wry::WebViewBuilder::new_with_web_context(&mut context).with_focused(false).with_background_color((background.0, background.1, background.2, 255));
        #[cfg(not(windows))]
        let builder = {
            let _ = background;
            wry::WebViewBuilder::new().with_transparent(true)
        };
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
            .with_custom_protocol(trek_core::artifact::SCHEME.into(), move |_, request| respond(&document.borrow(), request.uri().path()))
            .with_ipc_handler(move |request| {
                let _ = t1.try_send(Event::Message(request.into_body()));
            })
            .with_navigation_handler(move |url| {
                let ok = trek_core::artifact::navigation_allowed(&url, &nav_origin);
                if !ok {
                    tracing::debug!("artifact: navigation to {url} refused");
                }
                ok
            })
            .with_new_window_req_handler(move |url, _| {
                let _ = t2.try_send(Event::Popup(url));
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
        Ok(Self { web, clip, shown: None })
    }
}

impl Surface for NativeSurface {
    fn place(&mut self, at: Option<Placement>, window: &Window) {
        if self.shown == at {
            return;
        }
        match at {
            Some(p) => {
                self.clip.place(&p, window.scale_factor());
                let inner = p.body.origin - p.clip.origin;
                let rect = if cfg!(windows) {
                    let s = window.scale_factor();
                    wry::Rect {
                        position: wry::dpi::Position::Physical(wry::dpi::PhysicalPosition::new((f32::from(inner.x) * s).round() as i32, (f32::from(inner.y) * s).round() as i32)),
                        size: wry::dpi::Size::Physical(wry::dpi::PhysicalSize::new((f32::from(p.body.size.width) * s).round() as u32, (f32::from(p.body.size.height) * s).round() as u32)),
                    }
                } else {
                    wry::Rect {
                        position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(f32::from(inner.x) as f64, f32::from(inner.y) as f64)),
                        size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(f32::from(p.body.size.width) as f64, f32::from(p.body.size.height) as f64)),
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

    fn release_keys(&self, _window: &Window) {
        self.clip.release_keys();
    }

    fn set_background(&self, rgb: (u8, u8, u8)) {
        if cfg!(windows) {
            let _ = self.web.set_background_color((rgb.0, rgb.1, rgb.2, 255));
        }
    }
}

impl Drop for NativeSurface {
    fn drop(&mut self) {
        self.clip.hide();
    }
}

/// Decode an image a platform gave (TIFF, PNG) into BGRA rows.
pub fn decode(bytes: &[u8]) -> Result<Shot, String> {
    let picture = image::load_from_memory(bytes).map_err(|e| e.to_string())?.into_rgba8();
    let (width, height) = picture.dimensions();
    let mut bgra = picture.into_raw();
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Ok(Shot { width, height, bgra })
}

/// A raw handle (the container's) as wry wants a parent.
struct Parent(raw_window_handle::RawWindowHandle);

impl raw_window_handle::HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: the handle is the container's, which outlives the borrow (it's built with it).
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.0) })
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{Parent, Placement};
    use objc2::{MainThreadMarker, MainThreadOnly as _};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::msg_send;
    use objc2_app_kit::NSView;
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use std::ptr::NonNull;

    /// kCALayerMinXMinYCorner | kCALayerMaxXMinYCorner: the bottom corners of a layer in a view
    /// that isn't flipped.
    const BOTTOM_CORNERS: usize = 1 | 2;
    /// The card's corner radius, less its 1 px edge.
    const RADIUS: f64 = 11.;

    /// An `NSView` over the window's GPUI view that clips the web view in it to its frame.
    pub struct Clip {
        view: Retained<NSView>,
        host: Retained<NSView>,
    }

    impl Clip {
        pub fn new(handle: raw_window_handle::RawWindowHandle) -> Option<Self> {
            let raw_window_handle::RawWindowHandle::AppKit(h) = handle else { return None };
            let mtm = MainThreadMarker::new()?;
            // SAFETY: GPUI's view, alive as long as its window (which outlives every card in it).
            let host: Retained<NSView> = unsafe { Retained::retain(h.ns_view.as_ptr().cast())? };
            let view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.)));
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
            Parent(raw_window_handle::RawWindowHandle::AppKit(raw_window_handle::AppKitWindowHandle::new(ptr)))
        }

        /// Frame the visible part (window px, from the top left) and show it.
        pub fn place(&self, p: &Placement, _scale: f32) {
            let host = self.host.frame();
            let (x, w, h) = (f32::from(p.clip.origin.x) as f64, f32::from(p.clip.size.width) as f64, f32::from(p.clip.size.height) as f64);
            let top = f32::from(p.clip.origin.y) as f64;
            let y = if self.host.isFlipped() { top } else { host.size.height - top - h };
            self.view.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)));
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
                self.view.setHidden(true);
            }
        }

        /// Make GPUI's view the window's first responder again.
        pub fn release_keys(&self) {
            if let Some(window) = self.host.window() {
                window.makeFirstResponder(Some(&self.host));
            }
        }
    }

    impl Drop for Clip {
        fn drop(&mut self) {
            self.view.removeFromSuperview();
        }
    }

    /// The page as WebKit draws it: a TIFF.
    pub fn snapshot(web: &wry::WebView) -> Option<async_channel::Receiver<Result<Vec<u8>, String>>> {
        use objc2_app_kit::NSImage;
        use objc2_foundation::NSError;
        use wry::WebViewExtMacOS as _;
        let (tx, rx) = async_channel::bounded(1);
        let done = block2::RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
            // SAFETY: WebKit hands a valid image or nil.
            let tiff = unsafe { image.as_ref() }.and_then(|i| i.TIFFRepresentation()).map(|d| d.to_vec());
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
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, GetParent, HWND_TOP, IsWindowVisible, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS};

    /// A child window over GPUI's that clips the web view in it (Windows clips a child window's
    /// children to it).
    pub struct Clip {
        hwnd: HWND,
    }

    impl Clip {
        pub fn new(handle: raw_window_handle::RawWindowHandle) -> Option<Self> {
            let raw_window_handle::RawWindowHandle::Win32(h) = handle else { return None };
            let parent = h.hwnd.get() as HWND;
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: a plain child window of GPUI's, on its thread; destroyed in `drop`.
            let hwnd = unsafe {
                CreateWindowExW(0, class.as_ptr(), std::ptr::null(), WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS, 0, 0, 0, 0, parent, std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null())
            };
            (!hwnd.is_null()).then_some(Self { hwnd })
        }

        pub fn handle(&self) -> Parent {
            let hwnd = NonZeroIsize::new(self.hwnd as isize).expect("a window");
            Parent(raw_window_handle::RawWindowHandle::Win32(raw_window_handle::Win32WindowHandle::new(hwnd)))
        }

        /// Frame the visible part (window px from the top left, in GPUI's logical px) and show it.
        pub fn place(&self, p: &Placement, scale: f32) {
            let at = |v: gpui_kit::Pixels| (f32::from(v) * scale).round() as i32;
            let (x, y) = (at(p.clip.origin.x), at(p.clip.origin.y));
            let (w, h) = (at(p.clip.origin.x + p.clip.size.width) - x, at(p.clip.origin.y + p.clip.size.height) - y);
            // SAFETY: our own child window.
            unsafe { SetWindowPos(self.hwnd, HWND_TOP, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW) };
        }

        pub fn hide(&self) {
            // SAFETY: our own child window.
            unsafe {
                if IsWindowVisible(self.hwnd) != 0 {
                    ShowWindow(self.hwnd, SW_HIDE);
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
            // SAFETY: our own child window; the web view in it went first.
            unsafe { DestroyWindow(self.hwnd) };
        }
    }

    /// The page as WebView2 draws it (CapturePreview): a PNG.
    pub fn snapshot(web: &wry::WebView) -> Option<async_channel::Receiver<Result<Vec<u8>, String>>> {
        Some(crate::panels::browser::capture_png(web))
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
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

    pub fn snapshot(_: &wry::WebView) -> Option<async_channel::Receiver<Result<Vec<u8>, String>>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::respond;

    #[test]
    fn the_document_is_served_at_its_root_with_the_csp_and_nothing_else_is() {
        let page = respond("<p>hi</p>", "/");
        assert_eq!(page.status(), 200);
        assert_eq!(page.headers()["content-security-policy"], trek_core::artifact::CSP);
        assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
        assert_eq!(page.headers()["cache-control"], "no-store");
        assert_eq!(&page.body()[..], b"<p>hi</p>");
        for other in ["/favicon.ico", "/../../etc/passwd", "/x.js"] {
            let r = respond("<p>hi</p>", other);
            assert_eq!(r.status(), 404, "{other}");
            assert!(r.body().is_empty());
        }
    }

    #[test]
    fn pictures_are_handed_on_as_bgra() {
        let mut png = Vec::new();
        let picture = image::RgbaImage::from_raw(2, 1, vec![10, 20, 30, 255, 1, 2, 3, 128]).unwrap();
        picture.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let shot = super::decode(&png).unwrap();
        assert_eq!((shot.width, shot.height), (2, 1));
        assert_eq!(shot.bgra, vec![30, 20, 10, 255, 3, 2, 1, 128]);
    }
}
