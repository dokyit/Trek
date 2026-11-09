//! The system's Photos picker (`PHPickerViewController`, PhotosUI, macOS 13+) as a sheet on a
//! Trek window. It runs in its own process and needs no Photos permission or Info.plist key.
//! There's no objc2 crate for PhotosUI, so its classes are reached by name.
//!
//! The picker is the content of a sheet window (`windowWithContentViewController:`), begun on the
//! window the composer is in. Add and Cancel (and Escape, which the picker takes as Cancel) both
//! come back as `picker:didFinishPicking:`, which ends the sheet; the picker, its delegate and the
//! sheet are let go of a turn of the run loop later, once the picker's call has returned.
//!
//! Each result's item provider writes its file to a temporary path that's deleted once the
//! handler returns, on a queue of its own: the handler moves it into Trek's snapshots folder
//! there (`keep`), off the main thread, and the last one in sends `Picked::Done`.

use super::{Failed, Pick, Picked};
use block2::RcBlock;
use objc2::rc::{Allocated, Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, NSObject, NSObjectProtocol};
use objc2::{
    ClassType as _, DefinedClass as _, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    sel,
};
use objc2_app_kit::{NSView, NSWindow};
use objc2_foundation::{NSArray, NSError, NSSize, NSString, NSURL};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[link(name = "PhotosUI", kind = "framework")]
unsafe extern "C" {}

/// `PHPickerConfigurationAssetRepresentationModeCurrent`: nothing is transcoded up front. Photos
/// still offers a JPEG beside a HEIC (seen on macOS 27), which `load_type` takes; a HEIC alone is
/// converted by `ready`.
const CURRENT: isize = 1;
/// `PHPickerConfigurationSelectionOrdered`: results come in the order they were picked.
const ORDERED: isize = 1;
/// How many can be picked in one go (0 would be no limit).
const LIMIT: isize = 20;

/// What `open` did.
pub enum Opened {
    /// The sheet is up: what's picked comes here.
    Showing(async_channel::Receiver<Picked>),
    /// One was up already; it's brought forward instead.
    AlreadyShowing,
    /// No picker here (before macOS 13, or no native window): use the open panel.
    Unavailable,
}

/// The sheet up now, kept alive while it's up: the picker holds its delegate weakly.
struct Showing {
    parent: Weak<NSWindow>,
    sheet: Retained<NSWindow>,
    _picker: Retained<AnyObject>,
    delegate: Retained<Delegate>,
}

thread_local! {
    static SHOWING: RefCell<Option<Showing>> = const { RefCell::new(None) };
}

struct Ivars {
    /// Taken when the picker finishes: the handlers' copies send on it, and it closes (so the
    /// composer stops waiting) once they're done, or at once when nothing was picked.
    tx: RefCell<Option<async_channel::Sender<Picked>>>,
    dir: PathBuf,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars are dropped with the object.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TrekPhotosPickerDelegate"]
    #[ivars = Ivars]
    struct Delegate;

    impl Delegate {
        /// `PHPickerViewControllerDelegate`: Add (with what's picked), Cancel or Escape (none).
        #[unsafe(method(picker:didFinishPicking:))]
        fn did_finish(&self, _picker: &AnyObject, results: &NSArray<AnyObject>) {
            let tx = self.ivars().tx.borrow_mut().take();
            end_sheet(self);
            let Some(tx) = tx else { return };
            let results = results.to_vec();
            if results.is_empty() {
                return;
            }
            let _ = tx.try_send(Picked::Copying(results.len()));
            copy_out(&results, tx, self.ivars().dir.clone());
        }

        /// A turn after the picker finished: let go of it, its sheet and this delegate.
        #[unsafe(method(trekLetGo))]
        fn let_go(&self) {
            SHOWING.with(|s| {
                let mut s = s.borrow_mut();
                if s.as_ref().is_some_and(|s| std::ptr::eq(&*s.delegate, self)) {
                    s.take();
                }
            });
        }
    }

    unsafe impl NSObjectProtocol for Delegate {}
);

/// Put the Photos picker up as a sheet on `window` (the composer's), keeping picks in `dir`.
pub fn open(window: &gpui_kit::Window, dir: PathBuf) -> Opened {
    let Some(mtm) = MainThreadMarker::new() else {
        return Opened::Unavailable;
    };
    // One at a time: a second ask brings the sheet up already showing forward.
    let up = SHOWING.with(|s| {
        let s = s.borrow();
        let showing = s.as_ref()?;
        let alive = showing.sheet.isVisible() && showing.parent.load().is_some();
        alive.then(|| showing.sheet.clone())
    });
    if let Some(sheet) = up {
        sheet.makeKeyAndOrderFront(None);
        return Opened::AlreadyShowing;
    }
    // One whose window closed under it never finished: it goes now.
    SHOWING.with(|s| s.borrow_mut().take());

    let Some(parent) = ns_window(window) else {
        return Opened::Unavailable;
    };
    let (Some(config_class), Some(filter_class), Some(picker_class)) = (
        AnyClass::get(c"PHPickerConfiguration"),
        AnyClass::get(c"PHPickerFilter"),
        AnyClass::get(c"PHPickerViewController"),
    ) else {
        tracing::warn!("photos: PhotosUI's picker isn't here");
        return Opened::Unavailable;
    };
    let (tx, rx) = async_channel::unbounded();

    // SAFETY: PhotosUI's documented API (PHPicker.h), on the main thread; every selector and
    // argument type is as declared there.
    let picker: Retained<AnyObject> = unsafe {
        let config: Retained<AnyObject> = msg_send![config_class, new];
        let _: () = msg_send![&*config, setSelectionLimit: LIMIT];
        let _: () = msg_send![&*config, setSelection: ORDERED];
        let _: () = msg_send![&*config, setPreferredAssetRepresentationMode: CURRENT];
        let images: Retained<AnyObject> = msg_send![filter_class, imagesFilter];
        let videos: Retained<AnyObject> = msg_send![filter_class, videosFilter];
        let both = NSArray::from_retained_slice(&[images, videos]);
        let filter: Retained<AnyObject> =
            msg_send![filter_class, anyFilterMatchingSubfilters: &*both];
        let _: () = msg_send![&*config, setFilter: &*filter];
        let picker: Allocated<AnyObject> = msg_send![picker_class, alloc];
        msg_send![picker, initWithConfiguration: &*config]
    };
    let delegate: Retained<Delegate> = {
        let this = mtm.alloc::<Delegate>().set_ivars(Ivars {
            tx: RefCell::new(Some(tx)),
            dir,
        });
        unsafe { msg_send![super(this), init] }
    };
    conform(delegate.class());
    // SAFETY: the picker's `delegate` is a weak `id<PHPickerViewControllerDelegate>`; the
    // delegate stays alive in `SHOWING` until the picker is let go of.
    unsafe {
        let _: () = msg_send![&*picker, setDelegate: &*delegate];
    }

    // SAFETY: `windowWithContentViewController:` takes any NSViewController; the window is kept
    // in `SHOWING` (so it mustn't also release itself on close).
    let sheet: Retained<NSWindow> =
        unsafe { msg_send![NSWindow::class(), windowWithContentViewController: &*picker] };
    unsafe { sheet.setReleasedWhenClosed(false) };
    let frame = parent.frame().size;
    let size = NSSize::new(
        (frame.width - 80.).clamp(480., 960.),
        (frame.height - 100.).clamp(400., 680.),
    );
    sheet.setContentSize(size);
    sheet.setContentMinSize(NSSize::new(480., 400.));
    // SAFETY: AppKit's `beginSheet:completionHandler:`; no handler is needed, the delegate hears
    // how it ended.
    unsafe {
        let none: Option<&block2::Block<dyn Fn(isize)>> = None;
        let _: () = msg_send![&*parent, beginSheet: &*sheet, completionHandler: none];
    }
    SHOWING.with(|s| {
        *s.borrow_mut() = Some(Showing {
            parent: Weak::from_retained(&parent),
            sheet,
            _picker: picker,
            delegate,
        })
    });
    Opened::Showing(rx)
}

/// The NSWindow behind a GPUI window.
fn ns_window(window: &gpui_kit::Window) -> Option<Retained<NSWindow>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    // SAFETY: `ns_view` is the live NSView GPUI made for this window, used on the main thread.
    let view: &NSView = unsafe { &*appkit.ns_view.as_ptr().cast::<NSView>() };
    view.window()
}

/// Say the delegate class is a `PHPickerViewControllerDelegate`, for a picker that checks.
fn conform(class: &AnyClass) {
    let Some(protocol) = AnyProtocol::get(c"PHPickerViewControllerDelegate") else {
        return;
    };
    if !class.conforms_to(protocol) {
        // SAFETY: the class has the protocol's one required method (`picker:didFinishPicking:`).
        unsafe {
            objc2::ffi::class_addProtocol((class as *const AnyClass).cast_mut(), protocol);
        }
    }
}

/// End the sheet `delegate`'s picker is in, and let it all go a turn later (not inside the
/// picker's own call into its delegate).
fn end_sheet(delegate: &Delegate) {
    let windows = SHOWING.with(|s| {
        let s = s.borrow();
        let s = s
            .as_ref()
            .filter(|s| std::ptr::eq(&*s.delegate, delegate))?;
        Some((s.parent.load(), s.sheet.clone()))
    });
    if let Some((parent, sheet)) = windows {
        match parent {
            Some(parent) => parent.endSheet(&sheet),
            None => sheet.orderOut(None),
        }
    }
    // SAFETY: `performSelector:withObject:afterDelay:` keeps the delegate until it's called.
    unsafe {
        let none: Option<&AnyObject> = None;
        let _: () = msg_send![
            delegate,
            performSelector: sel!(trekLetGo),
            withObject: none,
            afterDelay: 0.0f64
        ];
    }
}

/// What's left to copy out of a pick's results, and what's in so far, in the order picked.
struct Copies {
    done: Vec<Option<Result<Pick, Failed>>>,
    left: usize,
    tx: async_channel::Sender<Picked>,
}

impl Copies {
    /// Pick `i` is in: the last one sends them all.
    fn set(&mut self, i: usize, result: Result<Pick, Failed>) {
        if self.done[i].replace(result).is_none() {
            self.left -= 1;
        }
        if self.left == 0 {
            let done: Vec<_> = self.done.drain(..).flatten().collect();
            if let Err(async_channel::TrySendError::Closed(Picked::Done(done))) =
                self.tx.try_send(Picked::Done(done))
            {
                // The composer is gone: nothing will mention these.
                for pick in done.into_iter().flatten() {
                    let _ = std::fs::remove_file(pick.path);
                }
            }
        }
    }
}

/// Load each result's file from its item provider and keep it in `dir`.
fn copy_out(results: &[Retained<AnyObject>], tx: async_channel::Sender<Picked>, dir: PathBuf) {
    let copies = Arc::new(Mutex::new(Copies {
        done: (0..results.len()).map(|_| None).collect(),
        left: results.len(),
        tx,
    }));
    let stamp = crate::attachments::timestamp();
    for (i, result) in results.iter().enumerate() {
        // SAFETY: `PHPickerResult.itemProvider` and `NSItemProvider`'s documented API.
        let (provider, name, registered, image, movie) = unsafe {
            let provider: Retained<AnyObject> = msg_send![&**result, itemProvider];
            let name: Option<Retained<NSString>> = msg_send![&*provider, suggestedName];
            let registered: Retained<NSArray<NSString>> =
                msg_send![&*provider, registeredTypeIdentifiers];
            let image: bool = msg_send![
                &*provider,
                hasItemConformingToTypeIdentifier: &*NSString::from_str("public.image")
            ];
            let movie: bool = msg_send![
                &*provider,
                hasItemConformingToTypeIdentifier: &*NSString::from_str("public.movie")
            ];
            (
                provider,
                name.map(|n| n.to_string()),
                registered,
                image,
                movie,
            )
        };
        let registered: Vec<String> = registered.iter().map(|t| t.to_string()).collect();
        let label = name.clone().unwrap_or_else(|| {
            if movie && !image {
                "a video"
            } else {
                "a photo"
            }
            .into()
        });
        let Some((ty, photo)) = super::load_type(&registered, image, movie) else {
            let why = format!("Photos offered it as {}", registered.join(", "));
            copies
                .lock()
                .unwrap()
                .set(i, Err(Failed { name: label, why }));
            continue;
        };
        tracing::info!(
            "photos: pick {i} as {ty} (offered {})",
            registered.join(", ")
        );
        let (copies, dir, stamp) = (copies.clone(), dir.clone(), stamp.clone());
        let done = RcBlock::new(move |url: *mut NSURL, error: *mut NSError| {
            // SAFETY: the provider hands a file URL or an error, alive for this call.
            let (url, error) = unsafe { (url.as_ref(), error.as_ref()) };
            let result = match url.and_then(|u| u.path()) {
                Some(path) => {
                    let from = PathBuf::from(path.to_string());
                    let ext = from
                        .extension()
                        .map(|e| e.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let file = super::file_name(name.as_deref(), photo, &ext, &stamp);
                    super::keep(&from, &dir, &file)
                        .map(|path| Pick {
                            path,
                            photo,
                            name: label.clone(),
                        })
                        .map_err(|e| Failed {
                            name: label.clone(),
                            why: format!("{e:#}"),
                        })
                }
                None => Err(Failed {
                    name: label.clone(),
                    why: error
                        .map(|e| e.localizedDescription().to_string())
                        .unwrap_or_else(|| "Photos didn't hand it over".into()),
                }),
            };
            if let Err(f) = &result {
                tracing::warn!("photos: {} couldn't be had: {}", f.name, f.why);
            }
            copies.lock().unwrap().set(i, result);
        });
        // SAFETY: `loadFileRepresentationForTypeIdentifier:completionHandler:` calls the block
        // once, on a queue of its own; the block holds only what's safe there.
        unsafe {
            let _: Option<Retained<AnyObject>> = msg_send![
                &*provider,
                loadFileRepresentationForTypeIdentifier: &*NSString::from_str(&ty),
                completionHandler: &*done
            ];
        }
    }
}
