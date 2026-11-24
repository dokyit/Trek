//! This computer's own banners: posting one, what the system answered, and whether the system
//! lets Trek's banners through at all. Settings › Notifications says the last in words and has a
//! test for it; the log has a line for every banner asked for and what came of it
//! (`system::log_alert`), so a report of "no banner" can be read from the log.
//!
//! On a Mac Trek posts to the notification center itself (`trek_banner`) rather than through
//! GPUI, whose call tells nobody how it went: the center's answer to the request is what the log
//! wants, and its settings for Trek (allowed, banner style) are what Settings shows. A click on
//! the banner still comes back through GPUI's delegate (`root::init`), which Trek leaves in place.

use gpui_kit::{App, SystemNotification};

/// The tag of the test banner from Settings › Notifications.
pub const TEST_TAG: &str = "trek-test-banner";

/// Whether the system lets Trek's banners through, as far as it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Os {
    /// Banners are allowed and nothing known holds them back.
    Allowed,
    /// The system hasn't asked the user about Trek yet: the first banner asks.
    NotAsked,
    /// Notifications are turned off for Trek.
    Denied,
    /// Allowed, but delivered quietly: they go to the notification center with no banner (the
    /// banner style is None, or macOS was only ever asked provisionally).
    Quiet,
    /// A Focus is on (its name, when it can be read): the system holds banners back until it ends.
    Focus(Option<String>),
    /// A build run outside an app bundle: the Mac's notification center takes none from it.
    NeedsApp,
    /// Not heard yet.
    Unknown,
}

impl Os {
    /// Something the user can change in the system's settings keeps banners from showing.
    pub fn blocked(&self) -> bool {
        matches!(self, Os::Denied | Os::Quiet | Os::Focus(_))
    }

    /// The log's word for it.
    pub fn word(&self) -> &'static str {
        match self {
            Os::Allowed => "allowed",
            Os::NotAsked => "not asked yet",
            Os::Denied => "denied",
            Os::Quiet => "delivered quietly",
            Os::Focus(_) => "a Focus is on",
            Os::NeedsApp => "no app bundle",
            Os::Unknown => "unknown",
        }
    }

    /// What Settings › Notifications says of it, under the test. `system` is the system's name.
    pub fn words(&self, system: &str) -> String {
        match self {
            Os::Allowed => format!("{system} allows Trek's banners."),
            Os::NotAsked => {
                format!("{system} hasn't asked you about Trek's notifications yet. The test asks.")
            }
            Os::Denied => format!("Denied: {system} has Trek's notifications turned off."),
            Os::Quiet => format!(
                "Delivered quietly: {system} puts Trek's notifications in Notification Center without a banner. Choose a banner style for Trek to see them."
            ),
            Os::Focus(Some(name)) => format!(
                "Focus is on ({name}): {system} holds banners back until it ends, unless the Focus allows Trek."
            ),
            Os::Focus(None) => format!(
                "Focus is on: {system} holds banners back until it ends, unless the Focus allows Trek."
            ),
            Os::NeedsApp => {
                "This build runs outside the Trek app, which macOS takes no banners from.".into()
            }
            Os::Unknown => String::new(),
        }
    }

    /// The system settings page that changes it.
    fn pane(&self) -> &'static str {
        match self {
            Os::Focus(_) if cfg!(target_os = "macos") => {
                "x-apple.systempreferences:com.apple.Focus-Settings.extension"
            }
            _ => crate::permissions::Permission::Notifications.pane(),
        }
    }
}

/// Where Trek's banners stand with the system now. Cheap enough for a view to ask as it draws: a
/// Mac answers from what its notification center last said, and asks it again.
pub fn os() -> Os {
    #[cfg(feature = "shots")]
    if let Some(os) = demo_state() {
        return os;
    }
    imp::os()
}

/// What the system says of Trek's banners in a capture run that asked for a stand-in
/// (`banners-demo`): the isolated app has no bundle, so its own answer is always that, and a
/// screenshot of the row as a Mac with banners off draws it needs one.
#[cfg(feature = "shots")]
static DEMO: std::sync::Mutex<Option<Os>> = std::sync::Mutex::new(None);

/// Stand `os` in for the system's answer (`None`: the real one again). With one in, nothing is
/// posted either: a banner is answered as that system would answer it.
#[cfg(feature = "shots")]
pub fn demo(os: Option<Os>) {
    if let Ok(mut demo) = DEMO.lock() {
        *demo = os;
    }
}

#[cfg(feature = "shots")]
fn demo_state() -> Option<Os> {
    DEMO.lock().ok()?.clone()
}

/// Open the system's settings where `os` is changed: Trek's notifications, or Focus.
pub fn open_settings(os: &Os, cx: &App) {
    #[cfg(any(test, feature = "test-support"))]
    {
        let _ = cx;
        imp::OPENED.with(|o| o.borrow_mut().push(os.pane().to_string()));
    }
    #[cfg(not(any(test, feature = "test-support")))]
    cx.open_url(os.pane());
}

/// Post `banner` about `thread` (its id; empty for one about no thread) and write what the system
/// said to the log. `link`: what a click opens where the system starts Trek for it (Windows,
/// `root::banner_link`).
pub fn post(banner: SystemNotification, link: Option<String>, thread: &str, cx: &mut App) {
    #[cfg(feature = "shots")]
    if let Some(os) = demo_state() {
        let answer = match os {
            Os::Denied => Err("Notifications are not allowed for this application".to_string()),
            _ => Ok(()),
        };
        return answered(thread, &banner.tag, os.word(), answer);
    }
    imp::post(banner, link, thread, cx)
}

/// What the system said to the last test banner: `None` until it has answered.
pub fn test_answer() -> Option<Result<(), String>> {
    TEST_ANSWER.lock().ok()?.clone()
}

/// A test is about to go: its answer isn't in yet.
pub fn test_begins() {
    if let Ok(mut answer) = TEST_ANSWER.lock() {
        *answer = None;
    }
}

static TEST_ANSWER: std::sync::Mutex<Option<Result<(), String>>> = std::sync::Mutex::new(None);

/// The system's name in a sentence about its settings.
pub fn system_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(windows) {
        "Windows"
    } else {
        "The desktop"
    }
}

/// The system answered a post: the log says so, and a test hears it.
fn answered(thread: &str, tag: &str, os: &str, answer: Result<(), String>) {
    let who = if thread.is_empty() {
        "-"
    } else {
        crate::system::short_id(thread)
    };
    let system = system_name();
    match &answer {
        Ok(()) => crate::system::log_alert(format!(
            "alert thread={who} banner: {system} took it ({os})"
        )),
        Err(e) => {
            let line = format!("alert thread={who} banner: {system} refused it ({os}): {e}");
            tracing::warn!("{line}");
            #[cfg(any(test, feature = "test-support"))]
            crate::system::ALERT_LOG.with(|l| l.borrow_mut().push(line));
        }
    }
    if tag == TEST_TAG
        && let Ok(mut test) = TEST_ANSWER.lock()
    {
        *test = Some(answer);
    }
}

/// A banner Trek couldn't hand the system at all: no fault of the system's, so no warning.
#[cfg_attr(
    not(all(target_os = "macos", not(any(test, feature = "test-support")))),
    allow(dead_code)
)]
fn not_posted(thread: &str, tag: &str, why: &str) {
    let who = if thread.is_empty() {
        "-"
    } else {
        crate::system::short_id(thread)
    };
    crate::system::log_alert(format!("alert thread={who} banner: not posted: {why}"));
    if tag == TEST_TAG
        && let Ok(mut test) = TEST_ANSWER.lock()
    {
        *test = Some(Err(why.to_string()));
    }
}

#[cfg(all(target_os = "macos", not(any(test, feature = "test-support"))))]
mod imp {
    use super::{Os, answered};
    use gpui_kit::{App, SystemNotification};
    use trek_banner::Heard;

    pub fn os() -> Os {
        if !trek_banner::bundled() {
            return Os::NeedsApp;
        }
        match trek_banner::heard() {
            Some(Heard::Allowed) => match trek_banner::focus() {
                Some(name) => Os::Focus(name),
                None => Os::Allowed,
            },
            Some(Heard::NotAsked) => Os::NotAsked,
            Some(Heard::Denied) => Os::Denied,
            Some(Heard::Quiet) => Os::Quiet,
            None => Os::Unknown,
        }
    }

    pub fn post(banner: SystemNotification, _link: Option<String>, thread: &str, _cx: &mut App) {
        let tag = banner.tag.to_string();
        if !trek_banner::bundled() {
            return super::not_posted(thread, &tag, "this build runs outside an app bundle");
        }
        let thread = thread.to_string();
        trek_banner::post(
            &banner.tag,
            &banner.title,
            &banner.body,
            // On a thread of the center's: the log takes lines from any.
            std::sync::Arc::new(move |answer: Result<(), String>, said: &str| {
                answered(&thread, &tag, said, answer)
            }),
        );
    }
}

/// Windows and Linux: GPUI posts (a toast under Trek's app id; the desktop's notification
/// service), and tells nobody how it went, so the log says the banner was handed over and what
/// the system's switches say.
#[cfg(all(not(target_os = "macos"), not(any(test, feature = "test-support"))))]
mod imp {
    use super::{Os, answered};
    use crate::permissions::{Permission, Status};
    use gpui_kit::{App, SystemNotification};

    pub fn os() -> Os {
        match crate::permissions::status(Permission::Notifications) {
            Status::Denied => Os::Denied,
            _ => Os::Allowed,
        }
    }

    pub fn post(banner: SystemNotification, link: Option<String>, thread: &str, cx: &mut App) {
        let tag = banner.tag.to_string();
        match link {
            Some(link) => cx.show_system_notification_with_launch(banner, link),
            None => cx.show_system_notification(banner),
        }
        let os = os();
        // Handed over, which is all GPUI says; a system with Trek's banners off shows none.
        let answer = match os {
            Os::Denied => Err("its notifications for Trek are turned off".to_string()),
            _ => Ok(()),
        };
        answered(thread, &tag, os.word(), answer);
    }
}

/// Tests: banners go to GPUI's test platform, which keeps them; the system's side is what a test
/// says it is (`fake`).
#[cfg(any(test, feature = "test-support"))]
mod imp {
    use super::{Os, answered};
    use gpui_kit::{App, SystemNotification};
    use std::cell::RefCell;

    thread_local! {
        pub static OS: RefCell<Os> = const { RefCell::new(Os::Allowed) };
        pub static REFUSES: RefCell<Option<String>> = const { RefCell::new(None) };
        pub static OPENED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub fn os() -> Os {
        OS.with(|os| os.borrow().clone())
    }

    pub fn post(banner: SystemNotification, link: Option<String>, thread: &str, cx: &mut App) {
        let tag = banner.tag.to_string();
        let answer = match REFUSES.with(|r| r.borrow().clone()) {
            Some(why) => Err(why),
            None => {
                match link {
                    Some(link) => cx.show_system_notification_with_launch(banner, link),
                    None => cx.show_system_notification(banner),
                }
                Ok(())
            }
        };
        answered(thread, &tag, os().word(), answer);
    }
}

/// What tests say of the system's side, and what they see opened.
#[cfg(any(test, feature = "test-support"))]
pub mod fake {
    use super::{Os, imp};

    /// Where Trek's banners stand with the system.
    pub fn set(os: Os) {
        imp::OS.with(|o| *o.borrow_mut() = os);
    }

    /// The system refuses the next banners, for this reason (`None`: takes them again).
    pub fn refuse(why: Option<&str>) {
        imp::REFUSES.with(|r| *r.borrow_mut() = why.map(String::from));
    }

    /// The system settings pages opened.
    pub fn opened() -> Vec<String> {
        imp::OPENED.with(|o| o.borrow().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::Os;

    #[test]
    fn what_settings_says_names_the_system_and_what_to_do() {
        assert!(!Os::Allowed.blocked() && !Os::NotAsked.blocked() && !Os::NeedsApp.blocked());
        for os in [Os::Denied, Os::Quiet, Os::Focus(None)] {
            assert!(os.blocked(), "{os:?}");
            assert!(os.words("macOS").contains("macOS"), "{os:?}");
        }
        assert!(Os::Denied.words("macOS").starts_with("Denied"));
        assert!(Os::Quiet.words("macOS").starts_with("Delivered quietly"));
        assert!(
            Os::Focus(Some("Work".into()))
                .words("macOS")
                .starts_with("Focus is on (Work)")
        );
        assert_eq!(Os::Unknown.words("macOS"), "");
    }
}
