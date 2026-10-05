//! Notifications on your phone, through ntfy: when a thread needs you, finishes or fails (what
//! the Mac would alert for), Trek publishes a short note to your ntfy topic, and the ntfy app on
//! the phone shows it. Tapping it opens the thread in Trek on iPhone (`trek://open?thread=…`).
//!
//! ntfy rather than Apple's push service: sending through Apple needs a paid developer account
//! and a server holding its key. ntfy is free, open source, and can be your own server. Anyone
//! who knows the topic can read what's sent to it, so the topic is a long random name and the
//! notes say only what the Mac's own banner says: the thread's title and what it needs.

use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::Context;
use std::time::Duration;
use trek_core::settings::PushWhen;

/// No keyboard or mouse for this long counts as away from the Mac.
const AWAY_AFTER: Duration = Duration::from_secs(120);

/// A new topic: `trek-` and 32 random URL-safe characters.
pub fn new_topic() -> String {
    format!("trek-{}", &trek_remote::pairing::generate_token()[..32])
}

/// What one note says and where tapping it goes.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Note {
    pub topic: String,
    pub title: String,
    pub message: String,
    /// Opens the thread in Trek on iPhone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub click: Option<String>,
    pub tags: Vec<String>,
    /// 1–5; needs-you notes come in at 4 (high), the rest at 3.
    pub priority: u8,
}

/// The note for an alert the Mac raised (`message` as the Mac's banner words it).
pub fn note_for(topic: &str, message: &str, thread: Option<&str>, project: Option<&str>) -> Note {
    let needs = message.starts_with("Needs") || message.contains("waiting") || message.contains("Waiting");
    let failed = message.starts_with("Failed");
    let tags = if needs {
        vec!["raised_hand".into()]
    } else if failed {
        vec!["x".into()]
    } else {
        vec!["white_check_mark".into()]
    };
    Note {
        topic: topic.to_string(),
        title: match project {
            Some(p) => format!("Trek · {p}"),
            None => "Trek".into(),
        },
        message: message.to_string(),
        click: thread.map(|t| format!("trek://open?thread={t}")),
        tags,
        priority: if needs { 4 } else { 3 },
    }
}

/// Seconds since the last keyboard or mouse input anywhere on the Mac.
#[cfg(target_os = "macos")]
fn idle_seconds() -> f64 {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    }
    // kCGEventSourceStateCombinedSessionState, kCGAnyInputEventType.
    // SAFETY: a plain CoreGraphics query with no pointers.
    unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) }
}

#[cfg(not(target_os = "macos"))]
fn idle_seconds() -> f64 {
    f64::MAX
}

#[cfg(target_os = "macos")]
fn screen_locked() -> bool {
    use std::ffi::{c_char, c_void};
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGSessionCopyCurrentDictionary() -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(allocator: *const c_void, text: *const c_char, encoding: u32) -> *const c_void;
        fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
        fn CFBooleanGetValue(value: *const c_void) -> u8;
        fn CFRelease(value: *const c_void);
    }
    unsafe {
        let dictionary = CGSessionCopyCurrentDictionary();
        if dictionary.is_null() {
            return false;
        }
        let key = CFStringCreateWithCString(std::ptr::null(), c"CGSSessionScreenIsLocked".as_ptr(), 0x0800_0100);
        let value = if key.is_null() { std::ptr::null() } else { CFDictionaryGetValue(dictionary, key) };
        let locked = !value.is_null() && CFBooleanGetValue(value) != 0;
        if !key.is_null() {
            CFRelease(key);
        }
        CFRelease(dictionary);
        locked
    }
}

#[cfg(not(target_os = "macos"))]
fn screen_locked() -> bool {
    false
}

fn away(idle: f64, locked: bool) -> bool {
    locked || idle >= AWAY_AFTER.as_secs_f64()
}

/// Publish `note` to `server` in the background; a failure is logged (there's no one to tell
/// right then: the user is away).
pub fn publish(server: &str, note: Note) -> tokio::task::JoinHandle<Result<(), String>> {
    let url = server.trim_end_matches('/').to_string();
    trek_core::runtime().spawn(async move {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().map_err(|e| e.to_string())?;
        let response = client.post(&url).json(&note).send().await.map_err(|e| e.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("{} said {}", url, response.status()))
        }
    })
}

impl Workspace {
    /// An alert the Mac raised: to the phone too, when push is on (and you're away, if that's the
    /// setting).
    pub fn push_alert(&mut self, message: &str, thread: &str) {
        let m = &self.settings.mobile;
        if !m.push || m.push_topic.is_empty() || cfg!(test) {
            return;
        }
        if m.push_when == PushWhen::Away && !away(idle_seconds(), screen_locked()) {
            return;
        }
        let project = self.thread(thread).and_then(|t| t.project_id.as_deref()).and_then(|p| self.project(p)).map(|p| p.name.clone());
        let note = note_for(&m.push_topic, message, Some(thread), project.as_deref());
        let job = publish(&m.push_server, note);
        trek_core::runtime().spawn(async move {
            if let Ok(Err(e)) = job.await {
                tracing::warn!("phone notification: {e}");
            }
        });
    }

    /// Turn push on (making a topic the first time) or off.
    pub fn set_push(&mut self, on: bool, cx: &mut Context<Self>) {
        self.settings.mobile.push = on;
        if on && self.settings.mobile.push_topic.is_empty() {
            self.settings.mobile.push_topic = new_topic();
        }
        self.save_settings(cx);
    }

    /// A test note, now, whatever the "when" setting; says in a toast how it went.
    pub fn test_push(&mut self, cx: &mut Context<Self>) {
        let m = self.settings.mobile.clone();
        if m.push_topic.is_empty() {
            return;
        }
        let mut note = note_for(&m.push_topic, "Notifications from this Mac will come here.", None, None);
        note.title = "Trek is connected".into();
        note.tags = vec!["tada".into()];
        let job = publish(&m.push_server, note);
        cx.spawn(async move |this, cx| {
            let result = job.await.map_err(|e| e.to_string()).and_then(|r| r);
            let message = match result {
                Ok(()) => "Sent. It should be on your phone in a moment.".to_string(),
                Err(e) => format!("Couldn't send it: {e}"),
            };
            let _ = this.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message, undo: None }));
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_say_what_the_banner_says_and_open_the_thread() {
        let n = note_for("trek-abc", "Needs your approval: Run ./migrate.sh", Some("t1"), Some("api"));
        assert_eq!(n.title, "Trek · api");
        assert_eq!(n.message, "Needs your approval: Run ./migrate.sh");
        assert_eq!(n.click.as_deref(), Some("trek://open?thread=t1"));
        assert_eq!(n.priority, 4, "needs you comes first");
        let done = note_for("trek-abc", "Finished: Add a verbose flag", Some("t2"), None);
        assert_eq!((done.title.as_str(), done.priority), ("Trek", 3));
        let json = serde_json::to_value(&done).unwrap();
        assert_eq!(json["topic"], "trek-abc");
        assert_eq!(json["tags"][0], "white_check_mark");
    }

    #[test]
    fn a_locked_screen_counts_as_away() {
        assert!(!away(1., false));
        assert!(away(1., true));
        assert!(away(AWAY_AFTER.as_secs_f64(), false));
    }

    #[test]
    fn topics_are_long_random_names_ntfy_takes() {
        let (a, b) = (new_topic(), new_topic());
        assert_ne!(a, b);
        assert_eq!(a.len(), 37);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{a}");
    }
}
