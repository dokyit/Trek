//! Notifications on your phone: when a thread needs you, finishes or fails (what the computer
//! would alert for), Trek tells the phone, by two routes.
//!
//! - **The web remote** (`site/remote`), added to the phone's home screen, asks its browser for
//!   notifications, and the computer sends each one itself, through the browser's own push
//!   service (Apple's, Google's, Mozilla's): standard Web Push, encrypted to the phone's key and
//!   signed with a key pair this computer made for itself (`trek_remote::webpush`). No account,
//!   no app to install, no server of Trek's. Tapping one opens the thread in the web remote.
//! - **ntfy**, optional, for anyone who uses its app: Trek publishes a short note to a topic of
//!   theirs, and tapping it opens the thread in Trek on iPhone (`trek://open?thread=…`). Anyone
//!   who knows the topic can read what's sent to it, so the topic is a long random name.
//!
//! Apple's push service for the iPhone app itself needs a paid developer account and a server
//! holding its key, so it isn't one of them. Either way a note says only the thread's title and
//! what kind of news it is, and with names off not even that; what a thread asks for stays off
//! the phone's notes: a command can hold a secret (`for_the_phone`).

use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::Context;
use std::sync::OnceLock;
use std::time::Duration;
use trek_core::settings::PushWhen;
use trek_remote::webpush;

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

/// The note's words for an alert the Mac raised: as the banner says it, unless the banner says what
/// a thread is asking for (`attention::Why::alert`). Then only that it needs the user.
fn for_the_phone(message: &str, title: &str) -> String {
    if message.starts_with("Waiting: ") {
        format!("Needs your approval: {title}")
    } else {
        message.to_string()
    }
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
pub fn note_for(
    topic: &str,
    message: &str,
    thread: Option<&str>,
    project: Option<&str>,
    names: bool,
    host: &str,
) -> Note {
    // By how the banner starts (or ends), never by words that may be the thread's own title:
    // "Finished: Fix waiting spinner" is a finish. A thread an agent started says "Thread
    // failed · …" when it fails.
    let needs = message.starts_with("Needs") || message.ends_with("waiting on your answer");
    let failed = message.starts_with("Failed") || message.starts_with("Thread failed");
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
        message: if names {
            message.to_string()
        } else {
            unnamed(message, needs, failed).to_string()
        },
        click: thread.map(|t| format!("trek://open?thread={t}&host={host}")),
        tags,
        priority: if needs { 4 } else { 3 },
    }
}

/// The note that hands thread `id` to the phone ("Continue on iPhone"): tapping it opens the
/// thread in Trek on iPhone. `names` off: the thread's title stays out of it.
pub fn phone_handoff_note(
    topic: &str,
    title: &str,
    id: &str,
    names: bool,
    host: &trek_remote::HostInfo,
) -> Note {
    Note {
        topic: topic.to_string(),
        title: format!("Continue on iPhone · {}", host.name),
        message: if names && !title.trim().is_empty() {
            title.trim().to_string()
        } else {
            "A thread is ready to pick up".into()
        },
        click: Some(format!("trek://open?thread={id}&host={}", host.id)),
        tags: vec!["iphone".into()],
        priority: 4,
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
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            text: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
        fn CFBooleanGetValue(value: *const c_void) -> u8;
        fn CFRelease(value: *const c_void);
    }
    unsafe {
        let dictionary = CGSessionCopyCurrentDictionary();
        if dictionary.is_null() {
            return false;
        }
        let key = CFStringCreateWithCString(
            std::ptr::null(),
            c"CGSSessionScreenIsLocked".as_ptr(),
            0x0800_0100,
        );
        let value = if key.is_null() {
            std::ptr::null()
        } else {
            CFDictionaryGetValue(dictionary, key)
        };
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
pub fn publish(
    server: &str,
    note: Note,
    tries: usize,
) -> tokio::task::JoinHandle<Result<(), String>> {
    let url = server.trim_end_matches('/').to_string();
    trek_core::runtime().spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        let mut waits = RETRIES.iter().take(tries.saturating_sub(1));
        loop {
            let (status, error) = match client.post(&url).json(&note).send().await {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => (
                    Some(response.status()),
                    format!("{} said {}", url, response.status()),
                ),
                Err(e) => (None, e.to_string()),
            };
            match waits.next().filter(|_| worth_retrying(status)) {
                Some(wait) => tokio::time::sleep(*wait).await,
                None => return Err(error),
            }
        }
    })
}

/// This computer's Web Push key pair, kept in the data folder (`mobile/push-key.json`, the user's
/// alone) and made the first time it's needed. `None` when it can be neither read nor written:
/// the web remote then isn't offered notifications.
pub fn vapid_key() -> Option<&'static webpush::VapidKey> {
    static KEY: OnceLock<Option<webpush::VapidKey>> = OnceLock::new();
    KEY.get_or_init(|| {
        let path = trek_core::paths::data_dir()
            .join("mobile")
            .join("push-key.json");
        webpush::VapidKey::load_or_create(&path)
            .inspect_err(|e| tracing::warn!("phone notifications: no push key: {e}"))
            .ok()
    })
    .as_ref()
}

/// The web remote's note for an ntfy-shaped one: the same words, and a tag per thread so a newer
/// note about it replaces the older one on the phone. Urgent when it needs the user.
pub fn web_note(note: &Note, thread: Option<&str>, host: &str) -> (webpush::PushNote, bool) {
    (
        webpush::PushNote {
            title: note.title.clone(),
            body: note.message.clone(),
            thread: thread.map(str::to_string),
            host: host.to_string(),
            tag: match thread {
                Some(t) => format!("{host}:{t}"),
                None => host.to_string(),
            },
        },
        note.priority >= 4,
    )
}

/// Send `note` to one phone's browser through its push service, trying again after a failure that
/// may pass (`tries` in all). A subscription the service says is gone or not ours (403, 404, 410)
/// is forgotten.
pub fn send_web_push(
    remote: trek_remote::RemoteHandle,
    device_id: String,
    sub: webpush::PushSubscription,
    note: webpush::PushNote,
    urgent: bool,
    tries: usize,
) -> tokio::task::JoinHandle<Result<(), String>> {
    trek_core::runtime().spawn(async move {
        let key = vapid_key().ok_or("there's no push key")?;
        let body = webpush::encrypt(&sub, &note.to_bytes()).map_err(|e| e.to_string())?;
        // The service's name is looked up here, once, and every address it has must be public;
        // the POST goes to the address checked (no second lookup that could answer otherwise),
        // and isn't sent on elsewhere.
        let (host, port) = sub.host_port().ok_or("that isn't a push endpoint")?;
        let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|e| format!("{host} couldn't be looked up: {e}"))?
            .collect();
        let addr = webpush::public_target(&addrs).map_err(str::to_string)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&host, addr)
            .build()
            .map_err(|e| e.to_string())?;
        let service = sub.audience();
        let mut waits = RETRIES.iter().take(tries.saturating_sub(1));
        loop {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let auth = key.authorization(&sub, now).map_err(|e| e.to_string())?;
            let mut request = client.post(&sub.endpoint).body(body.clone());
            for (name, value) in webpush::headers(auth, urgent) {
                request = request.header(name, value);
            }
            let (outcome, error) = match request.send().await {
                Ok(r) => (
                    webpush::Outcome::of(r.status().as_u16()),
                    format!("{service} said {}", r.status()),
                ),
                Err(e) => (webpush::Outcome::Retry, e.to_string()),
            };
            match outcome {
                webpush::Outcome::Sent => return Ok(()),
                webpush::Outcome::Gone => {
                    remote.drop_push(&device_id, &sub.endpoint);
                    return Err(format!("{error}: the phone no longer takes notifications"));
                }
                webpush::Outcome::Refused => return Err(error),
                webpush::Outcome::Retry => match waits.next() {
                    Some(wait) => tokio::time::sleep(*wait).await,
                    None => return Err(error),
                },
            }
        }
    })
}

/// Which phones' browsers get a note: each device that asked for notifications (`web_push`).
fn web_targets(remote: Option<&crate::remote::Remote>) -> Vec<(String, webpush::PushSubscription)> {
    remote.map_or_else(Vec::new, |r| r.handle.push_targets())
}

impl Workspace {
    /// Phones' browsers that asked for notifications, by device id.
    pub fn web_push_devices(&self) -> Vec<String> {
        web_targets(self.remote.as_ref())
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// Send `note` by every route there is: each phone's browser that asked, and ntfy when it has
    /// a topic. One job per route, each with its result.
    fn note_jobs(
        &self,
        note: Note,
        thread: Option<&str>,
        tries: usize,
    ) -> Vec<tokio::task::JoinHandle<Result<(), String>>> {
        let m = &self.settings.mobile;
        let host = crate::remote::host_info().id;
        let mut jobs = Vec::new();
        if let Some(remote) = self.remote.as_ref() {
            for (device_id, sub) in remote.handle.push_targets() {
                let (web, urgent) = web_note(&note, thread, &host);
                jobs.push(send_web_push(
                    remote.handle.clone(),
                    device_id,
                    sub,
                    web,
                    urgent,
                    tries,
                ));
            }
        }
        if !m.push_topic.is_empty() {
            jobs.push(publish(&m.push_server, note, tries));
        }
        jobs
    }

    /// `jobs` sent, and a toast saying how it went: `sent` when any route took it, else the
    /// first failure after `failed`.
    fn toast_when_sent(
        jobs: Vec<tokio::task::JoinHandle<Result<(), String>>>,
        sent: &'static str,
        failed: &'static str,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let mut error = None;
            let mut any = false;
            for job in jobs {
                match job.await.map_err(|e| e.to_string()).and_then(|r| r) {
                    Ok(()) => any = true,
                    Err(e) => {
                        error.get_or_insert(e);
                    }
                }
            }
            let message = match (any, error) {
                (true, _) => sent.to_string(),
                (false, Some(e)) => format!("{failed}: {e}"),
                (false, None) => format!("{failed}: no phone takes notifications yet"),
            };
            let _ = this.update(cx, |_, cx| {
                cx.emit(WorkspaceEvent::Toast {
                    message,
                    undo: None,
                })
            });
        })
        .detach();
    }

    /// Whether any route reaches a phone: ntfy has a topic, or a phone's browser asked.
    fn has_phone_route(&self) -> bool {
        !self.settings.mobile.push_topic.is_empty() || !self.web_push_devices().is_empty()
    }

    /// An alert the computer raised: to the phone too, when phone notifications are on (and
    /// you're away, if that's the setting).
    pub fn push_alert(&mut self, message: &str, thread: &str) {
        let m = &self.settings.mobile;
        if !m.push || cfg!(any(test, feature = "test-support")) || !self.has_phone_route() {
            return;
        }
        if !due(m.push_when, idle_seconds, screen_locked) {
            return;
        }
        let project = self
            .thread(thread)
            .and_then(|t| t.project_id.as_deref())
            .and_then(|p| self.project(p))
            .map(|p| p.name.clone());
        let title = self
            .thread(thread)
            .map(|t| t.title.as_str())
            .unwrap_or_default();
        let note = note_for(
            &m.push_topic,
            &for_the_phone(message, title),
            Some(thread),
            project.as_deref(),
            m.push_names,
            &crate::remote::host_info().id,
        );
        for job in self.note_jobs(note, Some(thread), 1 + RETRIES.len()) {
            trek_core::runtime().spawn(async move {
                if let Ok(Err(e)) = job.await {
                    tracing::warn!("phone notification: {e}");
                }
            });
        }
    }

    /// Whether a thread can be sent to the phone: phone notifications are on, and reach one.
    pub fn can_send_to_phone(&self) -> bool {
        self.settings.mobile.push && self.has_phone_route()
    }

    /// The paired phone's name, as it gave it when it paired (the last one seen), if any phone
    /// has paired.
    pub fn phone_name(&self) -> Option<String> {
        let remote = self.remote.as_ref()?;
        remote
            .devices
            .iter()
            .filter(|d| d.is_phone())
            .max_by_key(|d| d.last_seen_at.unwrap_or(d.paired_at))
            .map(|d| d.name.clone())
    }

    /// "Continue on your phone": a notification on the phone that opens thread `id` there when
    /// tapped (Trek has no other way to put a thread in front of the phone). Without phone
    /// notifications, says how to turn them on and opens Settings › Phone & Machines.
    pub fn send_thread_to_phone(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.can_send_to_phone() {
            cx.emit(WorkspaceEvent::Toast {
                message: "Turn on phone notifications first: the thread goes to your phone as one, which opens it when tapped.".into(),
                undo: None,
            });
            self.navigate(
                crate::workspace::Route::Settings(crate::workspace::SettingsPage::Mobile),
                cx,
            );
            return;
        }
        let m = self.settings.mobile.clone();
        let title = self.thread(id).map(|t| t.title.clone()).unwrap_or_default();
        let note = phone_handoff_note(
            &m.push_topic,
            &title,
            id,
            m.push_names,
            &crate::remote::host_info(),
        );
        let jobs = self.note_jobs(note, Some(id), 1);
        Self::toast_when_sent(
            jobs,
            "Sent to your phone. Tap the notification to open the thread there.",
            "Couldn't send it to your phone",
            cx,
        );
    }

    /// Turn phone notifications on or off. They go to each phone whose web remote asked for them,
    /// and through ntfy once that's set up (`set_ntfy`).
    pub fn set_push(&mut self, on: bool, cx: &mut Context<Self>) {
        self.settings.mobile.push = on;
        self.save_settings(cx);
    }

    /// Send notifications through ntfy too (a new topic), or stop (the topic is forgotten).
    pub fn set_ntfy(&mut self, on: bool, cx: &mut Context<Self>) {
        self.settings.mobile.push_topic = if on { new_topic() } else { String::new() };
        self.save_settings(cx);
    }

    /// A test note, now, whatever the "when" setting, by every route; says in a toast how it went.
    pub fn test_push(&mut self, cx: &mut Context<Self>) {
        let m = self.settings.mobile.clone();
        let mut note = note_for(
            &m.push_topic,
            &format!(
                "Notifications from {} will come here.",
                crate::words::words().this_computer
            ),
            None,
            None,
            true,
            &crate::remote::host_info().id,
        );
        note.title = "Trek is connected".into();
        // Plain: the status emoji (done, needs you) carry meaning; this one would only decorate.
        note.tags = vec![];
        // Once: the user is waiting to hear how it went.
        let jobs = self.note_jobs(note, None, 1);
        Self::toast_when_sent(
            jobs,
            "Sent. It should be on your phone in a moment.",
            "Couldn't send it",
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_thread_asks_for_stays_off_the_topic() {
        let banner = crate::workspace::attention::Why::asking(
            "Run command",
            "curl -H 'Authorization: Bearer sk-secret' example.com",
            None,
        )
        .alert("Fix the login bug");
        assert!(banner.contains("sk-secret"), "the Mac's own banner has it");
        let said = for_the_phone(&banner, "Fix the login bug");
        assert_eq!(said, "Needs your approval: Fix the login bug");
        // Still called what it is, named or not.
        let n = note_for("trek-abc", &said, Some("t1"), None, true, "m1");
        assert_eq!((n.message.as_str(), n.priority), (said.as_str(), 4));
        let n = note_for("trek-abc", &said, Some("t1"), None, false, "m1");
        assert_eq!(n.message, "A thread needs your approval");
        // Other news goes as the banner says it.
        assert_eq!(
            for_the_phone("Finished: Fix the login bug", "Fix the login bug"),
            "Finished: Fix the login bug"
        );
    }

    /// The web remote's banner says what ntfy's does: needs-you notes urgent, the rest not, one
    /// per thread (a newer one replaces it), and with names off nothing of the thread's.
    #[test]
    fn the_web_remote_hears_what_ntfy_hears() {
        let n = note_for(
            "",
            "Needs your approval: Fix the bug",
            Some("t1"),
            Some("api"),
            true,
            "m1",
        );
        let (w, urgent) = web_note(&n, Some("t1"), "m1");
        assert_eq!(
            (w.title.as_str(), w.body.as_str(), urgent),
            ("Trek · api", "Needs your approval: Fix the bug", true)
        );
        assert_eq!((w.tag.as_str(), w.thread.as_deref()), ("m1:t1", Some("t1")));
        let done = note_for(
            "",
            "Finished: Fix the bug",
            Some("t1"),
            Some("api"),
            false,
            "m1",
        );
        let (w, urgent) = web_note(&done, Some("t1"), "m1");
        assert_eq!(
            (w.title.as_str(), w.body.as_str(), urgent),
            ("Trek", "A thread finished", false)
        );
        let sent = String::from_utf8(w.to_bytes()).unwrap();
        assert!(!sent.contains("Fix") && !sent.contains("api"), "{sent}");
        let (test, _) = web_note(&done, None, "m1");
        assert_eq!(
            test.tag, "m1",
            "a note about no thread has the computer's tag"
        );
    }

    #[test]
    fn notes_say_what_the_banner_says_and_open_the_thread() {
        let n = note_for(
            "trek-abc",
            "Needs your approval: Run ./migrate.sh",
            Some("t1"),
            Some("api"),
            true,
            "m1",
        );
        assert_eq!(n.title, "Trek · api");
        assert_eq!(n.message, "Needs your approval: Run ./migrate.sh");
        assert_eq!(n.click.as_deref(), Some("trek://open?thread=t1&host=m1"));
        assert_eq!(n.priority, 4, "needs you comes first");
        let done = note_for(
            "trek-abc",
            "Finished: Add a verbose flag",
            Some("t2"),
            None,
            true,
            "m1",
        );
        assert_eq!((done.title.as_str(), done.priority), ("Trek", 3));
        let json = serde_json::to_value(&done).unwrap();
        assert_eq!(json["topic"], "trek-abc");
        assert_eq!(json["tags"][0], "white_check_mark");
    }

    #[test]
    fn without_names_a_note_says_only_what_kind_of_news_it_is() {
        let unnamed =
            |message: &str| note_for("trek-abc", message, Some("t1"), Some("api"), false, "m1");
        let n = unnamed("Needs your approval: Run ./migrate.sh --apply");
        // Neither the project nor what was asked leaves the Mac; the note still opens the thread.
        assert_eq!(
            (n.title.as_str(), n.message.as_str(), n.priority),
            ("Trek", "A thread needs your approval", 4)
        );
        assert_eq!(n.click.as_deref(), Some("trek://open?thread=t1&host=m1"));
        assert_eq!(
            unnamed("Finished: Rotate the signing key").message,
            "A thread finished"
        );
        assert_eq!(
            unnamed("Failed: Rotate the signing key").message,
            "A thread failed"
        );
        assert_eq!(
            unnamed("Paused until 2:10 AM: Rotate the signing key").message,
            "A thread is paused at its usage limit"
        );
        assert_eq!(
            unnamed("Resumed: Rotate the signing key").message,
            "A thread resumed"
        );
        assert_eq!(
            unnamed("Rotate the signing key is waiting on your answer").message,
            "A thread needs you"
        );
        // A thread an agent started fails in other words; a title that says "waiting" is still
        // the title, not what the thread needs.
        let agent_started = unnamed("Thread failed · Migrate the DB");
        assert_eq!(
            (
                agent_started.message.as_str(),
                agent_started.tags[0].as_str(),
                agent_started.priority
            ),
            ("A thread failed", "x", 3)
        );
        assert_eq!(
            unnamed("Thread completed · Migrate the DB").message,
            "A thread finished"
        );
        for (message, said) in [
            ("Finished: Fix waiting spinner", "A thread finished"),
            ("Failed: Waiting room layout", "A thread failed"),
            (
                "Paused until 2:10 AM: Fix waiting spinner",
                "A thread is paused at its usage limit",
            ),
        ] {
            let n = unnamed(message);
            assert_eq!((n.message.as_str(), n.priority), (said, 3), "{message}");
            assert_ne!(n.tags[0], "raised_hand", "{message}");
        }
        let named = note_for(
            "trek-abc",
            "Thread failed · Migrate the DB",
            Some("t1"),
            None,
            true,
            "m1",
        );
        assert_eq!((named.tags[0].as_str(), named.priority), ("x", 3));
        let json = serde_json::to_string(&unnamed("Failed: Rotate the signing key")).unwrap();
        assert!(!json.contains("Rotate") && !json.contains("api"), "{json}");
    }

    #[test]
    fn only_failures_that_may_pass_are_tried_again() {
        use reqwest::StatusCode;
        assert!(worth_retrying(None), "the server wasn't reached");
        assert!(
            worth_retrying(Some(StatusCode::BAD_GATEWAY))
                && worth_retrying(Some(StatusCode::TOO_MANY_REQUESTS))
        );
        assert!(
            !worth_retrying(Some(StatusCode::FORBIDDEN))
                && !worth_retrying(Some(StatusCode::BAD_REQUEST))
        );
    }

    #[test]
    fn a_locked_screen_counts_as_away() {
        assert!(!away(1., false));
        assert!(away(1., true));
        assert!(away(AWAY_AFTER.as_secs_f64(), false));
        assert!(
            !away(AWAY_AFTER.as_secs_f64() - 0.5, false),
            "two minutes, not a moment less"
        );
    }

    #[test]
    fn a_note_goes_out_when_the_user_is_away_or_always() {
        let (idle, locked) = (|| 5.0, || false);
        assert!(!due(PushWhen::Away, idle, locked), "at the keyboard");
        assert!(
            due(PushWhen::Away, || 600.0, locked),
            "idle for ten minutes"
        );
        assert!(
            due(PushWhen::Away, idle, || true),
            "locked, though the last input was a moment ago"
        );
        assert!(due(PushWhen::Always, idle, locked));
        // `Always` doesn't ask the system anything: the queries cost a call per alert.
        assert!(due(
            PushWhen::Always,
            || unreachable!("idle time"),
            || unreachable!("lock state")
        ));
    }

    #[test]
    fn topics_are_long_random_names_ntfy_takes() {
        let (a, b) = (new_topic(), new_topic());
        assert_ne!(a, b);
        assert_eq!(a.len(), 37);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{a}"
        );
    }

    #[test]
    fn handing_a_thread_to_the_phone_opens_it_there() {
        let host = trek_remote::HostInfo {
            id: "m1".into(),
            name: "Studio".into(),
            version: "0".into(),
        };
        let n = phone_handoff_note("trek-abc", "Fix the bug", "t1", true, &host);
        assert_eq!(n.title, "Continue on iPhone · Studio");
        assert_eq!(n.message, "Fix the bug");
        assert_eq!(n.click.as_deref(), Some("trek://open?thread=t1&host=m1"));
        let unnamed = phone_handoff_note("trek-abc", "Fix the bug", "t1", false, &host);
        assert!(
            !unnamed.message.contains("Fix"),
            "names off keeps the title out"
        );
    }
}
