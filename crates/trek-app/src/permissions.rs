//! The Mac's privacy permissions Trek uses, for onboarding's permissions step: whether each is
//! granted, the quickest way to grant it (the system's own prompt where macOS has one, else the
//! exact pane of System Settings), and Trek's app as a file to drag into a pane's list.
//!
//! What uses each:
//! - Accessibility: computer use clicks and types for an agent (`trek-mcp`'s desktop tools).
//! - Screen & System Audio Recording: computer use's screenshots of other apps, the picture of
//!   background computer use, and snapshots of a window.
//! - Microphone and Speech Recognition: dictation in the composer (`trek-dictate`); Speech also
//!   transcribes videos for `watch_video`.
//! - Notifications: banners when a thread needs you or finishes.
//! - Full Disk Access (optional): agents run as Trek, so a project in Documents, Desktop or
//!   iCloud Drive otherwise brings one macOS prompt per folder.
//! - Local Network: the phone server and other machines on the same Wi-Fi. macOS has no way to
//!   read it, so it's asked when Trek first reaches the network.
//!
//! Windows asks for none of these (or nothing Trek can read), so there the step isn't shown.

use gpui_kit::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    Accessibility,
    ScreenRecording,
    Microphone,
    SpeechRecognition,
    Notifications,
    FullDiskAccess,
    LocalNetwork,
}

/// Where a permission stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Granted,
    /// Not granted, and macOS would still show its prompt (or can't say whether it would).
    NotYet,
    /// Turned down: only System Settings can change it now.
    Denied,
    /// macOS doesn't say (Local Network).
    Unknown,
    /// This build can't ask (a bare `cargo run` without the usage descriptions in a bundle).
    NeedsApp,
}

/// How the row's button grants a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// macOS shows its own prompt.
    Prompt,
    /// The pane in System Settings, where Trek is turned on in a list (or dragged into it).
    Pane,
}

impl Permission {
    pub const ALL: [Permission; 7] = [
        Permission::Accessibility,
        Permission::ScreenRecording,
        Permission::Microphone,
        Permission::SpeechRecognition,
        Permission::Notifications,
        Permission::FullDiskAccess,
        Permission::LocalNetwork,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Permission::Accessibility => "accessibility",
            Permission::ScreenRecording => "screen-recording",
            Permission::Microphone => "microphone",
            Permission::SpeechRecognition => "speech",
            Permission::Notifications => "notifications",
            Permission::FullDiskAccess => "full-disk",
            Permission::LocalNetwork => "local-network",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Permission::Accessibility => "Accessibility",
            Permission::ScreenRecording => "Screen & System Audio Recording",
            Permission::Microphone => "Microphone",
            Permission::SpeechRecognition => "Speech Recognition",
            Permission::Notifications => "Notifications",
            Permission::FullDiskAccess => "Full Disk Access",
            Permission::LocalNetwork => "Local Network",
        }
    }

    /// Why Trek needs it, in a line.
    pub fn why(self) -> &'static str {
        match self {
            Permission::Accessibility => "Computer use: agents click and type in other apps.",
            Permission::ScreenRecording => {
                "Computer use and snapshots: agents see the windows they work in."
            }
            Permission::Microphone => "Dictate prompts instead of typing them.",
            Permission::SpeechRecognition => {
                "Turns what you dictate, and videos agents watch, into text."
            }
            Permission::Notifications => "A banner when a thread needs you or finishes.",
            Permission::FullDiskAccess => {
                "Agents work in Documents, Desktop or iCloud Drive without a prompt per folder."
            }
            Permission::LocalNetwork => {
                "Your iPhone and other machines reach Trek on this Wi-Fi. Asked when you pair one."
            }
        }
    }

    /// Not needed for anything Trek does by default.
    pub fn optional(self) -> bool {
        matches!(self, Permission::FullDiskAccess | Permission::LocalNetwork)
    }

    /// The System Settings pane it's granted in.
    pub fn pane(self) -> &'static str {
        match self {
            Permission::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            Permission::ScreenRecording => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
            Permission::Microphone => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            }
            Permission::SpeechRecognition => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition"
            }
            Permission::Notifications => {
                "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=dev.trek.Trek"
            }
            Permission::FullDiskAccess => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"
            }
            Permission::LocalNetwork => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork"
            }
        }
    }

    /// The pane lists apps with a + (and takes an app dragged in): Trek's icon is offered to drag.
    pub fn takes_a_drop(self) -> bool {
        matches!(
            self,
            Permission::Accessibility | Permission::ScreenRecording | Permission::FullDiskAccess
        )
    }

    /// How the button grants it, given where it stands and whether the prompt was already shown
    /// this session (Accessibility and Screen Recording don't say whether they were asked: their
    /// prompt shows once, so a second press opens the pane).
    pub fn grant(self, status: Status, asked: bool) -> Grant {
        let has_prompt = matches!(
            self,
            Permission::Accessibility
                | Permission::ScreenRecording
                | Permission::Microphone
                | Permission::SpeechRecognition
                | Permission::Notifications
        );
        if has_prompt && status == Status::NotYet && !asked {
            Grant::Prompt
        } else {
            Grant::Pane
        }
    }
}

/// Where `p` stands now. Cheap enough to ask every second while the step is open (Notifications
/// answers from what was last heard, and asks again).
pub fn status(p: Permission) -> Status {
    #[cfg(feature = "shots")]
    if let Some(s) = DEMO.lock().ok().and_then(|d| d.as_ref()?.get(&p).copied()) {
        return s;
    }
    imp::status(p)
}

/// Statuses a set-up Mac would show, for screenshots of the isolated app (`onboarding demo`).
#[cfg(feature = "shots")]
static DEMO: std::sync::Mutex<Option<std::collections::HashMap<Permission, Status>>> =
    std::sync::Mutex::new(None);

#[cfg(feature = "shots")]
pub fn demo() {
    let set = [
        (Permission::Accessibility, Status::Granted),
        (Permission::ScreenRecording, Status::NotYet),
        (Permission::Microphone, Status::Granted),
        (Permission::SpeechRecognition, Status::NotYet),
        (Permission::Notifications, Status::Granted),
        (Permission::FullDiskAccess, Status::NotYet),
        (Permission::LocalNetwork, Status::Unknown),
    ];
    if let Ok(mut d) = DEMO.lock() {
        *d = Some(set.into_iter().collect());
    }
}

/// Show macOS's prompt for `p` (see `Permission::grant`).
pub fn prompt(p: Permission) {
    imp::prompt(p)
}

/// Open `p`'s pane in System Settings.
pub fn open_pane(p: Permission, cx: &App) {
    #[cfg(any(test, feature = "test-support"))]
    imp::OPENED.with(|o| o.borrow_mut().push(p));
    #[cfg(not(any(test, feature = "test-support")))]
    cx.open_url(p.pane());
    #[cfg(any(test, feature = "test-support"))]
    let _ = cx;
}

/// Trek's app on disk: the bundle (`Trek.app`), or the executable of a build that isn't one.
pub fn app_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| p.to_path_buf());
    Some(bundle.unwrap_or(exe))
}

/// Start dragging Trek's app (as a file, with its icon) from `window`, as the pointer moves with
/// the button down. Panes with a list of apps take it like a drop from Finder. Only from inside a
/// mouse event: macOS starts the drag from the event it's handling.
#[cfg(all(target_os = "macos", not(any(test, feature = "test-support"))))]
pub fn drag_app(window: &Window) {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{
        AnyThread as _, DefinedClass as _, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    };
    use objc2_app_kit::{
        NSApplication, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession,
        NSDraggingSource, NSView, NSWorkspace,
    };
    use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString, NSURL};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and the class adds no ivars to drop.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "TrekAppDragSource"]
        struct Source;

        unsafe impl NSObjectProtocol for Source {}

        unsafe impl NSDraggingSource for Source {
            #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
            fn mask(
                &self,
                _session: &NSDraggingSession,
                _context: NSDraggingContext,
            ) -> NSDragOperation {
                NSDragOperation::Copy | NSDragOperation::Link | NSDragOperation::Generic
            }
        }
    );
    let _ = Source::ivars;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(path) = app_path() else { return };
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else {
        return;
    };
    // SAFETY: `ns_view` is the live NSView GPUI made for this window, used on the main thread.
    let view: &NSView = unsafe { &*appkit.ns_view.as_ptr().cast::<NSView>() };
    let path = NSString::from_str(&path.to_string_lossy());
    let url = NSURL::fileURLWithPath(&path);
    let item = NSDraggingItem::initWithPasteboardWriter(
        NSDraggingItem::alloc(),
        ProtocolObject::from_ref(&*url),
    );
    let icon = NSWorkspace::sharedWorkspace().iconForFile(&path);
    let at: NSPoint = view.convertPoint_fromView(event.locationInWindow(), None);
    let frame = NSRect::new(NSPoint::new(at.x - 32., at.y - 32.), NSSize::new(64., 64.));
    // SAFETY: the image is what the drag shows; AppKit retains it.
    unsafe {
        item.setDraggingFrame_contents(frame, Some(&*Retained::cast_unchecked::<AnyObject>(icon)))
    };
    let source: Retained<Source> = unsafe { msg_send![Source::alloc(mtm), init] };
    view.beginDraggingSessionWithItems_event_source(
        &NSArray::from_retained_slice(&[item]),
        &event,
        ProtocolObject::from_ref(&*source),
    );
}

#[cfg(not(all(target_os = "macos", not(any(test, feature = "test-support")))))]
pub fn drag_app(_: &Window) {
    #[cfg(any(test, feature = "test-support"))]
    imp::DRAGS.with(|d| d.set(d.get() + 1));
}

#[cfg(all(target_os = "macos", not(any(test, feature = "test-support"))))]
mod imp {
    use super::{Permission, Status};
    use std::sync::atomic::{AtomicU8, Ordering};

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    /// Run from a bundle with an identifier: the notification center aborts the process
    /// otherwise, and TCC ends one that asks for what its Info.plist doesn't declare.
    fn bundled() -> bool {
        objc2_foundation::NSBundle::mainBundle()
            .bundleIdentifier()
            .is_some()
    }

    fn from_access(a: trek_dictate::Access) -> Status {
        match a {
            trek_dictate::Access::Granted => Status::Granted,
            trek_dictate::Access::NotAsked => Status::NotYet,
            trek_dictate::Access::Denied => Status::Denied,
            trek_dictate::Access::Unavailable => Status::NeedsApp,
        }
    }

    /// What the notification center last said: 0 not heard yet, else a `Status` as 1 + index.
    static NOTIFICATIONS: AtomicU8 = AtomicU8::new(0);

    fn notifications() -> Status {
        if !bundled() {
            return Status::NeedsApp;
        }
        use block2::RcBlock;
        use objc2_user_notifications::{
            UNAuthorizationStatus, UNNotificationSettings, UNUserNotificationCenter,
        };
        let block = RcBlock::new(|settings: std::ptr::NonNull<UNNotificationSettings>| {
            // SAFETY: the center hands a valid settings object for the call's length.
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            let s = match status {
                UNAuthorizationStatus::Authorized
                | UNAuthorizationStatus::Provisional
                | UNAuthorizationStatus::Ephemeral => 1,
                UNAuthorizationStatus::Denied => 3,
                _ => 2,
            };
            NOTIFICATIONS.store(s, Ordering::Relaxed);
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .getNotificationSettingsWithCompletionHandler(&block);
        match NOTIFICATIONS.load(Ordering::Relaxed) {
            1 => Status::Granted,
            3 => Status::Denied,
            _ => Status::NotYet,
        }
    }

    /// Full Disk Access lets Trek read the user's own TCC database; without it, opening it is
    /// refused (and never prompts).
    fn full_disk() -> Status {
        let db = trek_core::paths::home().join("Library/Application Support/com.apple.TCC/TCC.db");
        match std::fs::File::open(db) {
            Ok(_) => Status::Granted,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Status::Unknown,
            Err(_) => Status::NotYet,
        }
    }

    pub fn status(p: Permission) -> Status {
        match p {
            Permission::Accessibility => {
                if trek_integrations::accessibility_allowed() {
                    Status::Granted
                } else {
                    Status::NotYet
                }
            }
            Permission::ScreenRecording => {
                if trek_integrations::screen_recording_allowed() {
                    Status::Granted
                } else {
                    Status::NotYet
                }
            }
            Permission::Microphone => from_access(trek_dictate::mic_access()),
            Permission::SpeechRecognition => from_access(trek_dictate::speech_access()),
            Permission::Notifications => notifications(),
            Permission::FullDiskAccess => full_disk(),
            Permission::LocalNetwork => Status::Unknown,
        }
    }

    pub fn prompt(p: Permission) {
        match p {
            Permission::Accessibility => {
                use objc2_foundation::{NSDictionary, NSNumber, NSString};
                let key = NSString::from_str("AXTrustedCheckOptionPrompt");
                let yes = NSNumber::new_bool(true);
                let options = NSDictionary::from_slices(&[&*key], &[&*yes]);
                // SAFETY: an NSDictionary is a CFDictionary (toll-free bridged), alive for the call.
                unsafe {
                    AXIsProcessTrustedWithOptions(objc2::rc::Retained::as_ptr(&options).cast())
                };
            }
            // SAFETY: a plain call; it shows the prompt the first time and adds Trek to the list.
            Permission::ScreenRecording => {
                unsafe { CGRequestScreenCaptureAccess() };
            }
            Permission::Microphone => trek_dictate::ask_mic(|_| {}),
            Permission::SpeechRecognition => trek_dictate::ask_speech(|_| {}),
            Permission::Notifications => {
                if !bundled() {
                    return;
                }
                use block2::RcBlock;
                use objc2::runtime::Bool;
                use objc2_foundation::NSError;
                use objc2_user_notifications::{UNAuthorizationOptions, UNUserNotificationCenter};
                let done = RcBlock::new(|granted: Bool, _: *mut NSError| {
                    NOTIFICATIONS.store(if granted.as_bool() { 1 } else { 3 }, Ordering::Relaxed);
                });
                UNUserNotificationCenter::currentNotificationCenter()
                    .requestAuthorizationWithOptions_completionHandler(
                        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                        &done,
                    );
            }
            Permission::FullDiskAccess | Permission::LocalNetwork => {}
        }
    }
}

/// Windows and Linux: nothing to ask (the step isn't shown).
#[cfg(all(not(target_os = "macos"), not(any(test, feature = "test-support"))))]
mod imp {
    use super::{Permission, Status};

    pub fn status(_: Permission) -> Status {
        Status::Granted
    }

    pub fn prompt(_: Permission) {}
}

/// Tests say what each permission is (`set`), and see what was asked for.
#[cfg(any(test, feature = "test-support"))]
mod imp {
    use super::{Permission, Status};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    thread_local! {
        pub static STATUS: RefCell<HashMap<Permission, Status>> = RefCell::default();
        pub static PROMPTED: RefCell<Vec<Permission>> = const { RefCell::new(Vec::new()) };
        pub static OPENED: RefCell<Vec<Permission>> = const { RefCell::new(Vec::new()) };
        pub static DRAGS: Cell<usize> = const { Cell::new(0) };
    }

    pub fn status(p: Permission) -> Status {
        STATUS.with(|s| s.borrow().get(&p).copied().unwrap_or(Status::NotYet))
    }

    pub fn prompt(p: Permission) {
        PROMPTED.with(|v| v.borrow_mut().push(p));
    }
}

/// What tests see of the permissions: set one's status, and what was prompted for or opened.
#[cfg(any(test, feature = "test-support"))]
pub mod fake {
    use super::{Permission, Status, imp};

    pub fn set(p: Permission, s: Status) {
        imp::STATUS.with(|m| m.borrow_mut().insert(p, s));
    }

    pub fn prompted() -> Vec<Permission> {
        imp::PROMPTED.with(|v| v.borrow().clone())
    }

    pub fn opened() -> Vec<Permission> {
        imp::OPENED.with(|v| v.borrow().clone())
    }

    pub fn drags() -> usize {
        imp::DRAGS.with(|d| d.get())
    }
}

#[cfg(test)]
mod tests {
    use super::{Grant, Permission, Status};

    #[test]
    fn the_button_prompts_once_where_macos_can_then_opens_the_pane() {
        use Permission::*;
        for p in [
            Accessibility,
            ScreenRecording,
            Microphone,
            SpeechRecognition,
            Notifications,
        ] {
            assert_eq!(p.grant(Status::NotYet, false), Grant::Prompt, "{p:?}");
            assert_eq!(
                p.grant(Status::NotYet, true),
                Grant::Pane,
                "{p:?} asked already"
            );
            assert_eq!(
                p.grant(Status::Denied, false),
                Grant::Pane,
                "{p:?} turned down"
            );
        }
        // No prompt exists for these: always the pane.
        for p in [FullDiskAccess, LocalNetwork] {
            assert_eq!(p.grant(Status::NotYet, false), Grant::Pane, "{p:?}");
        }
        // Only list panes take Trek dragged in.
        assert!(Accessibility.takes_a_drop() && ScreenRecording.takes_a_drop());
        assert!(FullDiskAccess.takes_a_drop());
        assert!(!Microphone.takes_a_drop() && !Notifications.takes_a_drop());
        // Every pane is a System Settings link, and every key is distinct.
        let mut keys: Vec<_> = Permission::ALL.iter().map(|p| p.key()).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), Permission::ALL.len());
        assert!(
            Permission::ALL
                .iter()
                .all(|p| p.pane().starts_with("x-apple.systempreferences:"))
        );
    }

    #[test]
    fn the_app_to_drag_is_the_bundle_when_there_is_one() {
        let path = super::app_path().expect("an executable");
        assert!(path.exists());
    }
}
