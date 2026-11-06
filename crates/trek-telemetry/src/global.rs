//! The app's one `Telemetry`, and what runs it: a loop on Trek's tokio runtime (never the UI
//! thread) that sends what's due in batches, says the day's open once a day, and saves the
//! counts now and then. Everything here does nothing until `start`, which only the app calls.

use crate::event::{Feature, Platform};
use crate::feedback::{Feedback, NotSent};
use crate::queue::Outcome;
use crate::scrub::Scrubber;
use crate::state::Consent;
use crate::{Batch, Config, Status, Telemetry};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

/// The loop wakes at least this often.
const TICK_SECS: i64 = 600;
/// A request gives up after this.
const TIMEOUT: Duration = Duration::from_secs(20);

/// What the app knows at launch.
pub struct Start {
    pub data_dir: PathBuf,
    /// The update channel: `stable`, `beta`, `nightly`.
    pub channel: String,
    /// Trek was set up in this data folder before this launch (an existing user's first count).
    pub set_up_before: bool,
    /// The OS version (`26.1`), asked for off the UI thread: it may start a process.
    pub os_version: fn() -> Option<String>,
    /// Takes what could name the user out of crash reports.
    pub scrubber: Scrubber,
}

struct Global {
    telemetry: Mutex<Telemetry>,
    wake: tokio::sync::Notify,
    changed: (async_channel::Sender<()>, async_channel::Receiver<()>),
}

static GLOBAL: OnceLock<Global> = OnceLock::new();

fn with<R>(f: impl FnOnce(&mut Telemetry) -> R) -> Option<R> {
    let global = GLOBAL.get()?;
    let mut telemetry = global
        .telemetry
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    Some(f(&mut telemetry))
}

/// Tell whoever listens (Settings › Privacy) that something changed. One pending note is
/// enough.
fn changed() {
    if let Some(global) = GLOBAL.get() {
        let _ = global.changed.0.try_send(());
    }
}

fn wake() {
    if let Some(global) = GLOBAL.get() {
        global.wake.notify_one();
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Start telemetry for this run: read what's kept (small files, so the first frame already
/// knows whether to ask), then, on the runtime, what the launch has to say and the loop.
pub fn start(start: Start) {
    let config = Config::detect(&start.data_dir);
    let platform = Platform::here(None, &start.channel);
    let telemetry = Telemetry::open(&start.data_dir, config, platform);
    let global = Global {
        telemetry: Mutex::new(telemetry),
        wake: tokio::sync::Notify::new(),
        changed: async_channel::bounded(1),
    };
    if GLOBAL.set(global).is_err() {
        return;
    }
    let Start {
        set_up_before,
        os_version,
        scrubber,
        ..
    } = start;
    trek_core::runtime().spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            let version = os_version();
            with(|t| {
                t.set_os_version(version);
                t.launched(set_up_before, &scrubber, chrono::Local::now());
            });
        })
        .await;
        changed();
        run().await;
    });
}

async fn run() {
    let Some(global) = GLOBAL.get() else {
        return;
    };
    let client = match reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(format!("Trek/{}", trek_core::VERSION))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!("telemetry: no HTTP client ({e}); nothing is sent this run");
            return;
        }
    };
    loop {
        with(|t| {
            t.daily(chrono::Local::now());
            t.save_if_changed();
        });
        while let Some(batch) = with(|t| t.take_batch(now())).flatten() {
            let outcome = post(&client, &batch).await;
            with(|t| t.settle(batch, &outcome, now()));
            changed();
            if outcome != Outcome::Sent {
                break;
            }
        }
        let wait = with(|t| t.next_due())
            .flatten()
            .map_or(TICK_SECS, |due| (due - now()).clamp(1, TICK_SECS));
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(wait as u64)) => {}
            _ = global.wake.notified() => {}
        }
    }
}

async fn post(client: &reqwest::Client, batch: &Batch) -> Outcome {
    match client.post(&batch.url).json(&batch.body).send().await {
        Ok(response) => Outcome::of_status(response.status().as_u16()),
        Err(e) if e.is_timeout() => Outcome::Retry("it timed out".into()),
        Err(e) if e.is_connect() => Outcome::Retry("no connection".into()),
        Err(e) => Outcome::Retry(e.without_url().to_string()),
    }
}

/// The user's answer, once started.
pub fn consent() -> Option<Consent> {
    with(|t| t.consent())
}

/// Whether the card asking should show this launch.
pub fn should_ask() -> bool {
    with(|t| t.should_ask()).unwrap_or(false)
}

pub fn set_consent(yes: bool) {
    with(|t| t.set_consent(yes, chrono::Utc::now()));
    // A yes may have something to say straight away (today's open).
    wake();
    changed();
}

pub fn reset_install_id() {
    with(|t| t.reset_install_id());
    changed();
}

/// One more use of `name` (Trek's own word for an agent or a tool) today.
pub fn count(feature: Feature, name: &str) {
    with(|t| t.count(feature, name, chrono::Local::now()));
}

pub fn status() -> Option<Status> {
    with(|t| t.status())
}

/// The platform events describe, for a feedback form's diagnostics.
pub fn platform() -> Option<Platform> {
    with(|t| t.platform().clone())
}

/// Where feedback goes: PostHog's host, or why it can't go.
pub fn feedback_destination() -> Result<String, NotSent> {
    // Not started (a test that didn't): what this process's config would say.
    with(|t| destination(&t.config))
        .unwrap_or_else(|| destination(&Config::detect(&trek_core::paths::data_dir())))
}

fn destination(config: &Config) -> Result<String, NotSent> {
    if config.blocked == Some(crate::Blocked::Isolated) {
        return Err(NotSent::TestProfile);
    }
    config
        .key
        .as_ref()
        .map(|k| k.host_name().to_string())
        .ok_or(NotSent::NoKey)
}

/// A note each time something the Privacy page shows changed (sent, failed, answered).
pub fn changes() -> Option<async_channel::Receiver<()>> {
    GLOBAL.get().map(|g| g.changed.1.clone())
}

/// How a feedback send went, when it has.
pub type Answer = futures::channel::oneshot::Receiver<Result<(), NotSent>>;

/// An answer given already: what a stand-in for PostHog hands back.
pub fn answered(result: Result<(), NotSent>) -> Answer {
    let (tx, rx) = futures::channel::oneshot::channel();
    let _ = tx.send(result);
    rx
}

/// Send `feedback` now, on the runtime; the answer comes back on the channel.
pub fn send_feedback(feedback: Feedback) -> Answer {
    let (tx, rx) = futures::channel::oneshot::channel();
    let request =
        with(|t| t.feedback_request(&feedback, chrono::Utc::now())).unwrap_or(Err(NotSent::NoKey));
    let batch = match request {
        Ok(batch) => batch,
        Err(why) => {
            let _ = tx.send(Err(why));
            return rx;
        }
    };
    trek_core::runtime().spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(format!("Trek/{}", trek_core::VERSION))
            .build();
        let outcome = match client {
            Ok(client) => post(&client, &batch).await,
            Err(e) => Outcome::Retry(e.to_string()),
        };
        let result = match outcome {
            Outcome::Sent => {
                with(|t| t.record_sent(&batch, now()));
                changed();
                Ok(())
            }
            Outcome::Retry(why) | Outcome::Rejected(why) => Err(NotSent::Failed(why)),
        };
        let _ = tx.send(result);
    });
    rx
}
