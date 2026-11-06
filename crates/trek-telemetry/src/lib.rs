//! Telemetry the user said yes to, crash reports and feedback, sent to PostHog. Asked once
//! (Settings › Privacy changes it); with a no, nothing but feedback the user sends ever leaves.
//! With a yes: an anonymous install id, a few events (`event`), and the panics of earlier runs
//! with paths and secrets taken out (`scrub`), batched and sent in the background, tried again
//! a few times when offline (`queue`). Without a PostHog key in the build (`config`), in a test or
//! the isolated verify profile, or with `DO_NOT_TRACK` set, nothing is sent. docs/PRIVACY.md lists
//! every event and what it carries.

pub mod config;
pub mod event;
pub mod feedback;
mod global;
pub mod queue;
pub mod scrub;
pub mod state;

pub use config::{Blocked, Config, Key};
pub use event::{Event, Feature, Platform};
pub use feedback::{Diagnostics, Feedback, Kind, NotSent};
pub use global::{
    Answer, Start, answered, changes, consent, count, feedback_destination, platform,
    reset_install_id, send_feedback, set_consent, should_ask, start, status,
};
pub use queue::Outcome;
pub use scrub::Scrubber;
pub use state::{Consent, Sent};

use chrono::{DateTime, Local, Utc};
use queue::{Queue, Queued};
use serde_json::Value;
use state::{Files, State};
use std::path::{Path, PathBuf};

/// Names counted on one day at most (a runaway caller can't grow the file).
const MAX_NAMES_A_DAY: usize = 64;

/// A request to PostHog: the events in it, and where it goes.
#[derive(Debug, Clone)]
pub struct Batch {
    pub url: String,
    pub body: Value,
    /// The queued events it carries (none for feedback, which isn't queued).
    pub items: Vec<Queued>,
}

/// What Settings › Privacy shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub consent: Consent,
    pub blocked: Option<Blocked>,
    pub key: Option<Key>,
    pub install_id: Option<String>,
    /// Events waiting to be sent.
    pub waiting: usize,
    pub last_sent: Option<Sent>,
    /// Why the last send failed, while it's still failing.
    pub last_error: Option<String>,
    /// Where a key for trying a build out goes.
    pub override_file: PathBuf,
}

/// Telemetry for one data folder.
pub struct Telemetry {
    data_dir: PathBuf,
    files: Files,
    pub config: Config,
    platform: Platform,
    state: State,
    queue: Queue,
    last_sent: Option<Sent>,
    last_error: Option<String>,
    /// Counts changed since they were last saved.
    dirty: bool,
}

impl Telemetry {
    pub fn open(data_dir: &Path, config: Config, platform: Platform) -> Telemetry {
        let files = Files::new(data_dir);
        let state = files.load_state();
        let queue = files.load_queue();
        let last_sent = files.load_sent();
        Telemetry {
            data_dir: data_dir.to_path_buf(),
            files,
            config,
            platform,
            state,
            queue,
            last_sent,
            last_error: None,
            dirty: false,
        }
    }

    pub fn consent(&self) -> Consent {
        self.state.consent
    }

    /// The card asking should show: not answered yet, and nothing rules telemetry out already
    /// (`DO_NOT_TRACK`, a test or the verify profile). A build without a key asks too.
    pub fn should_ask(&self) -> bool {
        self.state.consent == Consent::Unasked && self.config.blocked.is_none()
    }

    /// The user said yes and nothing blocks it: things are counted and queued.
    pub fn on(&self) -> bool {
        self.state.consent == Consent::Yes && self.config.blocked.is_none()
    }

    /// `on`, and there's somewhere to send to.
    pub fn sending(&self) -> bool {
        self.on() && self.config.key.is_some()
    }

    pub fn set_consent(&mut self, yes: bool, now: DateTime<Utc>) {
        self.state.consent = if yes { Consent::Yes } else { Consent::No };
        self.state.answered_at = Some(now.timestamp());
        if yes {
            self.install_id();
        } else {
            // A no stops what was waiting too.
            self.queue.clear();
            self.state.counts.clear();
            self.files.save_queue(&self.queue);
        }
        self.files.save_state(&self.state);
    }

    /// A new install id: nothing sent from now on can be tied to what was sent before. What was
    /// waiting under the old one goes unsent.
    pub fn reset_install_id(&mut self) {
        self.state.install_id = Some(uuid::Uuid::new_v4().to_string());
        self.queue.clear();
        self.files.save_queue(&self.queue);
        self.files.save_state(&self.state);
    }

    fn install_id(&mut self) -> String {
        self.state
            .install_id
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone()
    }

    /// Queue `event` if it may go. Whether it was.
    fn enqueue(&mut self, event: &Event, at: DateTime<Utc>) -> bool {
        if !self.sending() {
            return false;
        }
        let id = self.install_id();
        let envelope = event::envelope(event.name(), event.properties(&self.platform), &id, at);
        self.queue.push(envelope, at.timestamp());
        true
    }

    /// What a launch has to say (off the UI thread: it reads the crash reports): an update since
    /// the last run, the install the first time, the panics of earlier runs, and the day's open.
    /// `set_up_before`: Trek was set up in this data folder before this launch.
    pub fn launched(&mut self, set_up_before: bool, scrubber: &Scrubber, now: DateTime<Local>) {
        let at = now.to_utc();
        let version = self.platform.app_version.clone();
        if let Some(from) = self.state.last_version.clone().filter(|v| *v != version) {
            self.enqueue(
                &Event::Updated {
                    from,
                    to: version.clone(),
                },
                at,
            );
        }
        self.state.last_version = Some(version);
        if !self.state.install_sent && self.enqueue(&Event::Install { set_up_before }, at) {
            self.state.install_sent = true;
        }
        // Unanswered, they wait for the answer; answered, they're sent or not wanted.
        if self.state.consent != Consent::Unasked {
            for mut crash in trek_logging::take_crash_reports(&self.data_dir) {
                crash.message = scrubber.scrub(&crash.message);
                crash.location = scrubber.scrub(&crash.location);
                crash.backtrace = scrubber.scrub(&crash.backtrace);
                self.enqueue(&Event::Crash(crash), at);
            }
        }
        self.daily(now);
        self.save();
    }

    /// Once a day while Trek runs: the day's open, and what was counted on days gone by.
    pub fn daily(&mut self, now: DateTime<Local>) {
        let at = now.to_utc();
        let today = now.format("%Y-%m-%d").to_string();
        if self.state.opened_day.as_deref() != Some(today.as_str())
            && self.enqueue(&Event::Opened, at)
        {
            self.state.opened_day = Some(today.clone());
        }
        let past: Vec<String> = self
            .state
            .counts
            .keys()
            .filter(|day| **day < today)
            .cloned()
            .collect();
        for day in past {
            let counts = self.state.counts.remove(&day).unwrap_or_default();
            for (key, count) in counts {
                let Some((feature, name)) = key
                    .split_once(':')
                    .and_then(|(kind, name)| Some((Feature::from_key(kind)?, name)))
                else {
                    continue;
                };
                self.enqueue(
                    &Event::Used {
                        feature,
                        name: name.to_string(),
                        count,
                        day: day.clone(),
                    },
                    at,
                );
            }
            self.dirty = true;
        }
    }

    /// One more use of `name` today. Only with a yes and a key; `name` is Trek's own word for
    /// it, never anything the user typed.
    pub fn count(&mut self, feature: Feature, name: &str, now: DateTime<Local>) {
        if !self.sending() || !plain_name(name) {
            return;
        }
        let day = self
            .state
            .counts
            .entry(now.format("%Y-%m-%d").to_string())
            .or_default();
        let key = format!("{}:{name}", feature.key());
        if day.len() >= MAX_NAMES_A_DAY && !day.contains_key(&key) {
            return;
        }
        let n = day.entry(key).or_default();
        *n = n.saturating_add(1);
        self.dirty = true;
    }

    /// Save the counts, if they changed.
    pub fn save_if_changed(&mut self) {
        if std::mem::take(&mut self.dirty) {
            self.save();
        }
    }

    fn save(&mut self) {
        self.dirty = false;
        self.files.save_state(&self.state);
        self.files.save_queue(&self.queue);
    }

    /// The next batch to send, if anything may go and is due.
    pub fn take_batch(&mut self, now: i64) -> Option<Batch> {
        if !self.sending() {
            return None;
        }
        let key = self.config.key.clone()?;
        let items = self.queue.take_due(now);
        if items.is_empty() {
            return None;
        }
        let events = items.iter().map(|q| q.event.clone()).collect();
        Some(Batch {
            url: format!("{}/batch/", key.host),
            body: event::batch_body(&key.key, events),
            items,
        })
    }

    /// When the next queued event is due, if one is waiting and may go.
    pub fn next_due(&self) -> Option<i64> {
        self.sending().then(|| self.queue.next_due()).flatten()
    }

    /// What became of `batch`.
    pub fn settle(&mut self, batch: Batch, outcome: &Outcome, now: i64) {
        match outcome {
            Outcome::Sent => {
                self.record_sent(&batch, now);
                self.last_error = None;
            }
            Outcome::Retry(why) => self.last_error = Some(why.clone()),
            Outcome::Rejected(why) => {
                tracing::warn!("telemetry: {why}; {} events dropped", batch.items.len());
                self.last_error = Some(why.clone());
            }
        }
        self.queue.settle(batch.items, outcome, now);
        // A no while it was out: what came back stays here.
        if !self.on() {
            self.queue.clear();
        }
        self.files.save_queue(&self.queue);
    }

    /// Keep `batch` as what went out last.
    pub fn record_sent(&mut self, batch: &Batch, now: i64) {
        let events = batch.body["batch"].as_array().map_or(0, Vec::len);
        let sent = Sent {
            at: now,
            url: batch.url.clone(),
            events,
            body: serde_json::to_string_pretty(&batch.body).unwrap_or_default(),
        };
        self.files.save_sent(&sent);
        self.last_sent = Some(sent);
    }

    /// The request that sends `feedback`, if it may go. Sending it is the user's consent, so a
    /// no to telemetry doesn't keep it in; but then it carries a one-off id, not the install's.
    pub fn feedback_request(
        &mut self,
        feedback: &Feedback,
        at: DateTime<Utc>,
    ) -> Result<Batch, NotSent> {
        if self.config.blocked == Some(Blocked::Isolated) {
            return Err(NotSent::TestProfile);
        }
        let key = self.config.key.clone().ok_or(NotSent::NoKey)?;
        let id = if self.on() {
            self.install_id()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        let envelope = event::envelope("feedback", feedback.properties(), &id, at);
        Ok(Batch {
            url: format!("{}/batch/", key.host),
            body: event::batch_body(&key.key, vec![envelope]),
            items: vec![],
        })
    }

    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    /// Asked for once the launch is under way (it may start a process).
    pub fn set_os_version(&mut self, version: Option<String>) {
        self.platform.os_version = version;
    }

    pub fn status(&self) -> Status {
        Status {
            consent: self.state.consent,
            blocked: self.config.blocked,
            key: self.config.key.clone(),
            install_id: self.state.install_id.clone(),
            waiting: self.queue.len(),
            last_sent: self.last_sent.clone(),
            last_error: self.last_error.clone(),
            override_file: config::override_file(&self.data_dir),
        }
    }
}

/// Trek's own words for what's counted: lowercase, digits, `-`, `_`, `.` and `:`, and short.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 48
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.:".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    pub(crate) fn scratch(name: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "trek-telemetry-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub(crate) fn platform(version: &str) -> Platform {
        Platform {
            os: "macOS",
            os_version: Some("26.1".into()),
            arch: "aarch64",
            app_version: version.into(),
            channel: "stable".into(),
        }
    }

    pub(crate) fn keyed() -> Config {
        Config {
            key: config::key_from(Some("phc_0123456789abcdefABCDEF"), None, None),
            blocked: None,
        }
    }

    fn scrubber() -> Scrubber {
        Scrubber::new(
            Some("/Users/me".into()),
            vec![],
            &["/Users/me/code/app".into()],
            vec![],
            |s| s.to_string(),
        )
    }

    fn day(d: u32, h: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, d, h, 0, 0).unwrap()
    }

    fn names(t: &Telemetry) -> Vec<String> {
        t.queue
            .items
            .iter()
            .map(|q| q.event["event"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn nothing_is_asked_or_sent_before_a_yes() {
        let dir = scratch("unasked");
        let mut t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        assert_eq!(t.consent(), Consent::Unasked);
        assert!(
            t.should_ask(),
            "an existing user without an answer is asked once"
        );
        t.launched(true, &scrubber(), day(5, 9));
        t.count(Feature::Agent, "codex", day(5, 9));
        assert!(t.queue.is_empty());
        assert!(t.take_batch(0).is_none());
        assert_eq!(t.state.install_id, None, "no id is made before a yes");
        // A no: still nothing, and it's not asked again.
        t.set_consent(false, Utc::now());
        assert!(!t.should_ask());
        t.daily(day(6, 9));
        assert!(t.take_batch(0).is_none());
        // The answer is kept.
        let t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        assert_eq!(t.consent(), Consent::No);
    }

    #[test]
    fn without_a_key_its_asked_but_nothing_is_sent() {
        let dir = scratch("keyless");
        let mut t = Telemetry::open(&dir, Config::default(), platform("0.4.41"));
        assert!(t.should_ask());
        t.set_consent(true, Utc::now());
        t.launched(false, &scrubber(), day(5, 9));
        t.count(Feature::Tool, "browser", day(5, 9));
        assert!(t.queue.is_empty());
        assert!(t.take_batch(0).is_none());
        assert!(
            !t.state.install_sent,
            "the install is told once a key is there"
        );
    }

    #[test]
    fn a_blocked_process_is_never_asked_and_never_sends() {
        for blocked in [Blocked::DoNotTrack, Blocked::Isolated] {
            let dir = scratch("blocked");
            let config = Config {
                blocked: Some(blocked),
                ..keyed()
            };
            let mut t = Telemetry::open(&dir, config, platform("0.4.41"));
            assert!(!t.should_ask(), "{blocked:?}");
            t.set_consent(true, Utc::now());
            t.launched(false, &scrubber(), day(5, 9));
            assert!(t.take_batch(0).is_none(), "{blocked:?}");
        }
    }

    #[test]
    fn a_yes_sends_the_install_once_and_the_open_once_a_day() {
        let dir = scratch("yes");
        let mut t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        t.set_consent(true, Utc::now());
        t.launched(true, &scrubber(), day(5, 9));
        assert_eq!(names(&t), ["install", "app_opened"]);
        let install = &t.queue.items[0].event;
        assert_eq!(install["properties"]["set_up_before"], true);
        let id = t.state.install_id.clone().unwrap();
        assert_eq!(install["distinct_id"], id.as_str());
        // A second launch that day says nothing; the next day it opens again.
        let mut t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        t.queue.clear();
        t.launched(true, &scrubber(), day(5, 18));
        assert!(t.queue.is_empty());
        t.daily(day(6, 8));
        assert_eq!(names(&t), ["app_opened"]);
    }

    #[test]
    fn an_update_is_told_with_its_versions() {
        let dir = scratch("updated");
        let mut t = Telemetry::open(&dir, keyed(), platform("0.4.40"));
        t.set_consent(true, Utc::now());
        t.launched(true, &scrubber(), day(5, 9));
        let mut t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        t.queue.clear();
        t.launched(true, &scrubber(), day(5, 10));
        assert_eq!(names(&t), ["app_updated"]);
        let p = &t.queue.items[0].event["properties"];
        assert_eq!(
            (&p["from_version"], &p["to_version"]),
            (&"0.4.40".into(), &"0.4.41".into())
        );
    }

    #[test]
    fn counts_go_out_the_next_day_one_event_each() {
        let dir = scratch("counts");
        let mut t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        t.set_consent(true, Utc::now());
        t.launched(false, &scrubber(), day(5, 9));
        t.queue.clear();
        for name in ["codex", "codex", "direct:anthropic"] {
            t.count(Feature::Agent, name, day(5, 10));
        }
        t.count(Feature::Tool, "browser", day(5, 11));
        // Nothing the user typed: a name that isn't Trek's own word isn't counted.
        t.count(Feature::Tool, "My secret project", day(5, 11));
        t.daily(day(5, 23));
        assert!(t.queue.is_empty(), "today's counts wait for the day to end");
        t.save_if_changed();
        // They survive a restart.
        let mut t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        t.daily(day(6, 8));
        let used: Vec<(String, String, u64)> = t
            .queue
            .items
            .iter()
            .filter(|q| q.event["event"] == "feature_used")
            .map(|q| {
                let p = &q.event["properties"];
                (
                    p["kind"].as_str().unwrap().to_string(),
                    p["name"].as_str().unwrap().to_string(),
                    p["count"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            used,
            [
                ("agent".into(), "codex".into(), 2),
                ("agent".into(), "direct:anthropic".into(), 1),
                ("tool".into(), "browser".into(), 1),
            ]
        );
        assert!(t.state.counts.is_empty());
    }

    #[test]
    fn a_no_drops_whats_waiting() {
        let dir = scratch("no");
        let mut t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        t.set_consent(true, Utc::now());
        t.launched(false, &scrubber(), day(5, 9));
        t.count(Feature::Agent, "codex", day(5, 9));
        assert!(!t.queue.is_empty());
        let batch = t.take_batch(day(5, 9).timestamp()).unwrap();
        t.set_consent(false, Utc::now());
        // What was out when the user said no doesn't come back to be sent.
        t.settle(batch, &Outcome::Retry("offline".into()), 0);
        assert!(t.queue.is_empty() && t.state.counts.is_empty());
        assert!(
            Telemetry::open(&dir, keyed(), platform("0.4.41"))
                .queue
                .is_empty()
        );
    }

    #[test]
    fn a_sent_batch_is_kept_to_show_and_a_failed_one_waits() {
        let dir = scratch("sent");
        let mut t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        t.set_consent(true, Utc::now());
        t.launched(false, &scrubber(), day(5, 9));
        let now = day(5, 9).timestamp();
        let batch = t.take_batch(now).unwrap();
        assert_eq!(batch.url, "https://us.i.posthog.com/batch/");
        assert_eq!(batch.body["api_key"], "phc_0123456789abcdefABCDEF");
        t.settle(batch, &Outcome::Retry("offline".into()), now);
        assert_eq!(t.status().last_error.as_deref(), Some("offline"));
        assert_eq!(t.status().waiting, 2);
        assert!(t.take_batch(now).is_none(), "not again straight away");
        let later = t.next_due().unwrap();
        let batch = t.take_batch(later).unwrap();
        t.settle(batch, &Outcome::Sent, later);
        let status = t.status();
        assert_eq!((status.waiting, status.last_error), (0, None));
        let sent = status.last_sent.unwrap();
        assert_eq!(sent.events, 2);
        assert!(sent.body.contains("\"install\"") && sent.body.contains('\n'));
        // And after a restart.
        let t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        assert_eq!(t.status().last_sent.unwrap().at, later);
    }

    #[test]
    fn crashes_wait_for_an_answer_and_go_scrubbed() {
        let dir = scratch("crash");
        let crash = |message: &str| {
            let reports = dir.join("crash-reports");
            std::fs::create_dir_all(&reports).unwrap();
            let report = trek_logging::CrashReport {
                at: 7,
                version: "0.4.40".into(),
                thread: "main".into(),
                location: "crates/x/src/lib.rs:3".into(),
                message: message.into(),
                backtrace: "   0: x\n      at /Users/me/code/app/src/main.rs:4\n".into(),
            };
            std::fs::write(
                reports.join("panic-7-0.json"),
                serde_json::to_string(&report).unwrap(),
            )
            .unwrap();
        };
        crash("couldn't open /Users/me/code/app/secret.txt");
        let mut t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        t.launched(false, &scrubber(), day(5, 9));
        assert_eq!(
            trek_logging::crash_reports(&dir).len(),
            1,
            "kept until the user answers"
        );
        t.set_consent(true, Utc::now());
        t.launched(false, &scrubber(), day(5, 10));
        assert!(trek_logging::crash_reports(&dir).is_empty());
        let report = t
            .queue
            .items
            .iter()
            .find(|q| q.event["event"] == "$exception")
            .unwrap();
        let p = &report.event["properties"];
        assert_eq!(p["$exception_list"][0]["value"], "couldn't open [path]");
        assert!(!p["backtrace"].as_str().unwrap().contains("/Users/me"));
        // A no: the next crash is thrown away, not kept for later.
        t.set_consent(false, Utc::now());
        crash("again");
        t.launched(false, &scrubber(), day(5, 11));
        assert!(trek_logging::crash_reports(&dir).is_empty());
        assert!(t.queue.is_empty());
    }

    #[test]
    fn a_reset_id_is_a_new_one_and_drops_whats_waiting() {
        let dir = scratch("reset");
        let mut t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        t.set_consent(true, Utc::now());
        t.launched(false, &scrubber(), day(5, 9));
        let before = t.state.install_id.clone().unwrap();
        t.reset_install_id();
        let after = t.state.install_id.clone().unwrap();
        assert_ne!(before, after);
        assert!(t.queue.is_empty());
        // The install isn't told again: it's the same install.
        t.daily(day(6, 9));
        assert_eq!(names(&t), ["app_opened"]);
        assert_eq!(t.queue.items[0].event["distinct_id"], after.as_str());
    }

    #[test]
    fn feedback_goes_with_a_no_but_not_without_a_key_or_from_a_test_profile() {
        let dir = scratch("feedback");
        let feedback = Feedback {
            kind: Kind::Bug,
            message: "The sidebar froze".into(),
            email: None,
            diagnostics: None,
        };
        let mut t = Telemetry::open(&dir, keyed(), platform("0.4.41"));
        t.set_consent(false, Utc::now());
        let request = t.feedback_request(&feedback, Utc::now()).unwrap();
        let event = &request.body["batch"][0];
        assert_eq!(event["event"], "feedback");
        assert_eq!(event["properties"]["message"], "The sidebar froze");
        assert_eq!(
            t.state.install_id, None,
            "a one-off id: no install id is made for it"
        );
        // With a yes, it's the install's.
        t.set_consent(true, Utc::now());
        let request = t.feedback_request(&feedback, Utc::now()).unwrap();
        assert_eq!(
            request.body["batch"][0]["distinct_id"],
            t.state.install_id.clone().unwrap().as_str()
        );
        let mut keyless = Telemetry::open(&scratch("fb2"), Config::default(), platform("0.4.41"));
        assert_eq!(
            keyless.feedback_request(&feedback, Utc::now()).unwrap_err(),
            NotSent::NoKey
        );
        let isolated = Config {
            blocked: Some(Blocked::Isolated),
            ..keyed()
        };
        let mut test = Telemetry::open(&scratch("fb3"), isolated, platform("0.4.41"));
        assert_eq!(
            test.feedback_request(&feedback, Utc::now()).unwrap_err(),
            NotSent::TestProfile
        );
        let dnt = Config {
            blocked: Some(Blocked::DoNotTrack),
            ..keyed()
        };
        let mut dnt = Telemetry::open(&scratch("fb4"), dnt, platform("0.4.41"));
        assert!(dnt.feedback_request(&feedback, Utc::now()).is_ok());
    }
}
