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

/// No keyboard or mouse for this long counts as away from the computer.
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

/// What a note says with names left out (`Mobile::push_names` off): which kind of news it is,
/// and nothing of what the thread or its project is called or asked for.
fn unnamed(message: &str, needs: bool, failed: bool) -> &'static str {
    if message.starts_with("Needs your approval") {
        "A thread needs your approval"
    } else if needs {
        "A thread needs you"
    } else if failed {
        "A thread failed"
    } else if message.starts_with("Paused") || message.starts_with("Usage limit") {
        "A thread is paused at its usage limit"
    } else if message.starts_with("Resumed") {
        "A thread resumed"
    } else {
        "A thread finished"
    }
}

/// The note for an alert the Mac raised (`message` as the Mac's banner words it). `names`: say
/// it as the banner does, with the project in the title; else only what kind of news it is.
pub fn note_for(topic: &str, message: &str, thread: Option<&str>, project: Option<&str>, names: bool) -> Note {
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
        title: match project.filter(|_| names) {
            Some(p) => format!("Trek · {p}"),
            None => "Trek".into(),
        },
        message: if names { message.to_string() } else { unnamed(message, needs, failed).to_string() },
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

/// Seconds since the last keyboard or mouse input in this session. When Windows won't say, away:
/// a note the user didn't need beats one they never got.
#[cfg(windows)]
fn idle_seconds() -> f64 {
    crate::winsys::idle_seconds().unwrap_or(f64::MAX)
}

#[cfg(not(any(target_os = "macos", windows)))]
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

/// Whether the screen is locked (or the session switched away from, on Windows).
#[cfg(windows)]
fn screen_locked() -> bool {
    crate::winsys::session_locked()
}

#[cfg(not(any(target_os = "macos", windows)))]
fn screen_locked() -> bool {
    false
}

fn away(idle: f64, locked: bool) -> bool {
    locked || idle >= AWAY_AFTER.as_secs_f64()
}

/// Whether a note goes out now: always, or when the user is away. The system is asked (idle time,
/// lock state) only when it matters, as that is a call per alert.
fn due(when: PushWhen, idle: impl FnOnce() -> f64, locked: impl FnOnce() -> bool) -> bool {
    match when {
        PushWhen::Away => away(idle(), locked()),
        PushWhen::Always => true,
    }
}

/// How long after a try that failed the next one goes: a network that blinked, or a server
/// busy for a moment, shouldn't cost the one note about a thread that needs you.
const RETRIES: [Duration; 2] = [Duration::from_secs(4), Duration::from_secs(20)];

/// Whether a failed try is worth another: the server couldn't be reached, or said it was busy.
/// What it refused (a bad topic, a note it won't take) it would refuse again.
fn worth_retrying(status: Option<reqwest::StatusCode>) -> bool {
    status.is_none_or(|s| s.is_server_error() || s == reqwest::StatusCode::TOO_MANY_REQUESTS)
}

/// Publish `note` to `server` in the background, trying again after a failure that may pass
/// (`tries` times in all). The last failure is the result (logged by the caller: there's no one
/// to tell right then, the user is away).
pub fn publish(server: &str, note: Note, tries: usize) -> tokio::task::JoinHandle<Result<(), String>> {
    let url = server.trim_end_matches('/').to_string();
    trek_core::runtime().spawn(async move {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().map_err(|e| e.to_string())?;
        let mut waits = RETRIES.iter().take(tries.saturating_sub(1));
        loop {
            let (status, error) = match client.post(&url).json(&note).send().await {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => (Some(response.status()), format!("{} said {}", url, response.status())),
                Err(e) => (None, e.to_string()),
            };
            match waits.next().filter(|_| worth_retrying(status)) {
                Some(wait) => tokio::time::sleep(*wait).await,
                None => return Err(error),
            }
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
        if !due(m.push_when, idle_seconds, screen_locked) {
            return;
        }
        let project = self.thread(thread).and_then(|t| t.project_id.as_deref()).and_then(|p| self.project(p)).map(|p| p.name.clone());
        let note = note_for(&m.push_topic, message, Some(thread), project.as_deref(), m.push_names);
        let job = publish(&m.push_server, note, 1 + RETRIES.len());
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
        let mut note = note_for(&m.push_topic, &format!("Notifications from {} will come here.", crate::words::words().this_computer), None, None, true);
        note.title = "Trek is connected".into();
        note.tags = vec!["tada".into()];
        // Once: the user is waiting to hear how it went.
        let job = publish(&m.push_server, note, 1);
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
        let n = note_for("trek-abc", "Needs your approval: Run ./migrate.sh", Some("t1"), Some("api"), true);
        assert_eq!(n.title, "Trek · api");
        assert_eq!(n.message, "Needs your approval: Run ./migrate.sh");
        assert_eq!(n.click.as_deref(), Some("trek://open?thread=t1"));
        assert_eq!(n.priority, 4, "needs you comes first");
        let done = note_for("trek-abc", "Finished: Add a verbose flag", Some("t2"), None, true);
        assert_eq!((done.title.as_str(), done.priority), ("Trek", 3));
        let json = serde_json::to_value(&done).unwrap();
        assert_eq!(json["topic"], "trek-abc");
        assert_eq!(json["tags"][0], "white_check_mark");
    }

    #[test]
    fn without_names_a_note_says_only_what_kind_of_news_it_is() {
        let unnamed = |message: &str| note_for("trek-abc", message, Some("t1"), Some("api"), false);
        let n = unnamed("Needs your approval: Run ./migrate.sh --apply");
        // Neither the project nor what was asked leaves the Mac; the note still opens the thread.
        assert_eq!((n.title.as_str(), n.message.as_str(), n.priority), ("Trek", "A thread needs your approval", 4));
        assert_eq!(n.click.as_deref(), Some("trek://open?thread=t1"));
        assert_eq!(unnamed("Finished: Rotate the signing key").message, "A thread finished");
        assert_eq!(unnamed("Failed: Rotate the signing key").message, "A thread failed");
        assert_eq!(unnamed("Paused until 2:10 AM: Rotate the signing key").message, "A thread is paused at its usage limit");
        assert_eq!(unnamed("Resumed: Rotate the signing key").message, "A thread resumed");
        assert_eq!(unnamed("Rotate the signing key is waiting on your answer").message, "A thread needs you");
        let json = serde_json::to_string(&unnamed("Failed: Rotate the signing key")).unwrap();
        assert!(!json.contains("Rotate") && !json.contains("api"), "{json}");
    }

    #[test]
    fn only_failures_that_may_pass_are_tried_again() {
        use reqwest::StatusCode;
        assert!(worth_retrying(None), "the server wasn't reached");
        assert!(worth_retrying(Some(StatusCode::BAD_GATEWAY)) && worth_retrying(Some(StatusCode::TOO_MANY_REQUESTS)));
        assert!(!worth_retrying(Some(StatusCode::FORBIDDEN)) && !worth_retrying(Some(StatusCode::BAD_REQUEST)));
    }

    #[test]
    fn a_locked_screen_counts_as_away() {
        assert!(!away(1., false));
        assert!(away(1., true));
        assert!(away(AWAY_AFTER.as_secs_f64(), false));
        assert!(!away(AWAY_AFTER.as_secs_f64() - 0.5, false), "two minutes, not a moment less");
    }

    #[test]
    fn a_note_goes_out_when_the_user_is_away_or_always() {
        let (idle, locked) = (|| 5.0, || false);
        assert!(!due(PushWhen::Away, idle, locked), "at the keyboard");
        assert!(due(PushWhen::Away, || 600.0, locked), "idle for ten minutes");
        assert!(due(PushWhen::Away, idle, || true), "locked, though the last input was a moment ago");
        assert!(due(PushWhen::Always, idle, locked));
        // `Always` doesn't ask the system anything: the queries cost a call per alert.
        assert!(due(PushWhen::Always, || unreachable!("idle time"), || unreachable!("lock state")));
    }

    #[test]
    fn topics_are_long_random_names_ntfy_takes() {
        let (a, b) = (new_topic(), new_topic());
        assert_ne!(a, b);
        assert_eq!(a.len(), 37);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{a}");
    }
}
