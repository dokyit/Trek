//! The updater's state: what it's doing, what it found, when it checks next, and which results
//! still count. `Workspace` runs the checks and downloads (trek_core::update) and reports back.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use trek_core::settings::Channel;
use trek_core::update::{AvailableUpdate, Blocker};

/// A day between background checks.
const CHECK_EVERY_MS: i64 = 24 * 60 * 60 * 1000;
/// After a failure that may pass (offline, server trouble), try again within the hour.
const RETRY_AFTER_MS: i64 = 60 * 60 * 1000;

#[derive(Debug, Clone, Default, PartialEq)]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available { version: String },
    Downloading { version: String, progress: f32 },
    /// Verified and unpacked at `staged`: installs on "Restart to update" or when Trek quits.
    Ready { version: String, staged: PathBuf },
    /// Ready, waiting for agent work to be over: running or paused turns, plans waiting for an
    /// answer (`Workspace::work_in_flight`).
    RestartPending { version: String, staged: PathBuf },
    /// A full sentence for the user ("Couldn't download Trek 0.2.1: …").
    Failed(String),
}

/// The one thing the updater offers to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateAction {
    Check,
    Download,
    Restart,
}

impl UpdateAction {
    pub fn label(self) -> &'static str {
        match self {
            UpdateAction::Check => "Check now",
            UpdateAction::Download => "Download",
            UpdateAction::Restart => "Restart to update",
        }
    }
}

/// What the Updates page and the sidebar updater show for the current state.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateView {
    pub line: String,
    pub action: Option<UpdateAction>,
    /// Checking or downloading: the action button spins.
    pub busy: bool,
    pub progress: Option<f32>,
}

/// Identifies a check or download by the state it started in; one that finishes after the
/// update was dropped (the channel changed) no longer counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket(u64);

/// A download to start: what, its ticket, and the flag that stops it.
pub struct DownloadJob {
    pub update: AvailableUpdate,
    pub ticket: Ticket,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct Updater {
    pub status: UpdateStatus,
    /// The newest release the last check found (version, notes, download).
    pub offer: Option<AvailableUpdate>,
    /// When the background check runs next, in wall-clock ms. Not `Instant`: that stops while
    /// the Mac sleeps, and "once a day" would become once every few days on a laptop.
    next_check: Option<i64>,
    /// The channel the last check asked.
    channel: Option<Channel>,
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
}

impl Updater {
    /// Start a check. `None` while one runs or an update is downloading or waiting to install.
    pub fn begin_check(&mut self, channel: Channel, now: i64) -> Option<Ticket> {
        if matches!(self.status, UpdateStatus::Checking | UpdateStatus::Downloading { .. } | UpdateStatus::Ready { .. } | UpdateStatus::RestartPending { .. }) {
            return None;
        }
        self.status = UpdateStatus::Checking;
        self.next_check = Some(now + CHECK_EVERY_MS);
        self.channel = Some(channel);
        Some(Ticket(self.generation))
    }

    /// A check finished. True when what it found should start downloading (`auto_download`).
    /// A background check that fails stays quiet; one the user asked for says why.
    pub fn finish_check(&mut self, ticket: Ticket, result: Result<Option<AvailableUpdate>, String>, user_initiated: bool, auto_download: bool, now: i64) -> bool {
        if ticket.0 != self.generation {
            return false;
        }
        match result {
            Ok(Some(u)) => {
                self.status = UpdateStatus::Available { version: u.version.to_string() };
                self.offer = Some(u);
                auto_download
            }
            Ok(None) => {
                self.offer = None;
                self.status = UpdateStatus::UpToDate;
                false
            }
            Err(e) => {
                tracing::warn!("update check failed: {e}");
                self.next_check = Some(now + RETRY_AFTER_MS);
                self.status = match &self.offer {
                    _ if user_initiated => UpdateStatus::Failed(format!("Couldn't check for updates: {e}")),
                    // What an earlier check found is still on offer.
                    Some(o) => UpdateStatus::Available { version: o.version.to_string() },
                    None => UpdateStatus::Idle,
                };
                false
            }
        }
    }

    /// The background check is due (housekeeping asks every minute). An offer not downloaded yet
    /// (automatic downloads off) is checked again too: channels replace their archives, so an old
    /// offer's download can disappear, and a newer build may be out.
    pub fn check_due(&self, auto_check: bool, now: i64) -> bool {
        auto_check
            && self.next_check.is_none_or(|t| now >= t)
            && matches!(self.status, UpdateStatus::Idle | UpdateStatus::UpToDate | UpdateStatus::Available { .. } | UpdateStatus::Failed(_))
    }

    /// Start downloading what the last check found. `None` if nothing is on offer or a download
    /// already ran.
    pub fn begin_download(&mut self) -> Option<DownloadJob> {
        let update = self.offer.clone()?;
        if matches!(self.status, UpdateStatus::Downloading { .. } | UpdateStatus::Ready { .. } | UpdateStatus::RestartPending { .. }) {
            return None;
        }
        self.status = UpdateStatus::Downloading { version: update.version.to_string(), progress: 0.0 };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        Some(DownloadJob { update, ticket: Ticket(self.generation), cancel })
    }

    /// True if the progress belongs to the download on screen.
    pub fn download_progress(&mut self, ticket: Ticket, p: f32) -> bool {
        match &mut self.status {
            UpdateStatus::Downloading { progress, .. } if ticket.0 == self.generation => {
                *progress = p;
                true
            }
            _ => false,
        }
    }

    /// A download finished: staged, or the error and whether it may pass. Returns a staged copy
    /// nobody wants any more (the download was dropped while it ran), for the caller to delete.
    pub fn finish_download(&mut self, ticket: Ticket, version: String, result: Result<PathBuf, (String, bool)>, now: i64) -> Option<PathBuf> {
        if ticket.0 != self.generation {
            return result.ok();
        }
        self.cancel = None;
        match result {
            Ok(staged) => {
                tracing::info!("update {version} verified and staged");
                self.status = UpdateStatus::Ready { version, staged };
            }
            Err((e, transient)) => {
                tracing::warn!("update {version} rejected: {e}");
                if transient {
                    self.next_check = Some(now + RETRY_AFTER_MS);
                }
                self.status = UpdateStatus::Failed(format!("Couldn't update to Trek {version}: {e}"));
            }
        }
        None
    }

    /// The channel setting differs from the one the last check asked.
    pub fn channel_changed(&self, channel: Channel) -> bool {
        self.channel.is_some_and(|c| c != channel)
    }

    /// Forget the update in flight or waiting: stop its download, ignore late results of
    /// earlier checks and downloads. Returns the staged copy to delete, if there was one.
    pub fn drop_update(&mut self) -> Option<PathBuf> {
        self.generation += 1;
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.offer = None;
        match std::mem::take(&mut self.status) {
            UpdateStatus::Ready { staged, .. } | UpdateStatus::RestartPending { staged, .. } => Some(staged),
            _ => None,
        }
    }

    /// The update on offer, while it's on offer and has release notes.
    pub fn notes(&self) -> Option<&AvailableUpdate> {
        let offered = matches!(
            self.status,
            UpdateStatus::Available { .. } | UpdateStatus::Downloading { .. } | UpdateStatus::Ready { .. } | UpdateStatus::RestartPending { .. }
        );
        self.offer.as_ref().filter(|o| offered && !o.notes.is_empty())
    }
}

/// The updater's status line and next action for a state.
pub fn describe_update(status: &UpdateStatus, auto_check: bool, blocker: Option<Blocker>) -> UpdateView {
    let view = |line: String, action: Option<UpdateAction>| UpdateView { line, action, busy: false, progress: None };
    if let Some(blocker) = blocker {
        return view(blocker.message().into(), None);
    }
    match status {
        UpdateStatus::Idle if auto_check => view("Trek checks for updates once a day.".into(), Some(UpdateAction::Check)),
        UpdateStatus::Idle => view("Automatic checks are off.".into(), Some(UpdateAction::Check)),
        UpdateStatus::Checking => UpdateView { busy: true, ..view("Checking for updates…".into(), Some(UpdateAction::Check)) },
        UpdateStatus::UpToDate => view(format!("Trek {} is up to date.", trek_core::VERSION), Some(UpdateAction::Check)),
        UpdateStatus::Available { version } => view(format!("Trek {version} is available."), Some(UpdateAction::Download)),
        UpdateStatus::Downloading { version, progress } => {
            UpdateView { busy: true, progress: Some(*progress), ..view(format!("Downloading Trek {version} · {:.0}%", progress * 100.), None) }
        }
        UpdateStatus::Ready { version, .. } => view(format!("Trek {version} is ready. Restart now, or it installs when you quit."), Some(UpdateAction::Restart)),
        UpdateStatus::RestartPending { version, .. } => view(format!("Trek restarts into {version} when your agents finish."), None),
        UpdateStatus::Failed(e) => view(e.clone(), Some(UpdateAction::Check)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trek_core::update::Artifact;

    const HOUR: i64 = 60 * 60 * 1000;

    fn release(v: &str) -> AvailableUpdate {
        AvailableUpdate {
            version: v.parse().unwrap(),
            notes: "- faster".into(),
            pub_date: String::new(),
            artifact: Artifact { url: "u".into(), sha256: "s".into(), signature: "sig".into() },
        }
    }

    /// Checked at `now` on `channel` and found `v`, downloaded and staged.
    fn staged(u: &mut Updater, channel: Channel, v: &str, now: i64) -> PathBuf {
        let t = u.begin_check(channel, now).unwrap();
        assert!(u.finish_check(t, Ok(Some(release(v))), false, true, now));
        let job = u.begin_download().unwrap();
        let path = PathBuf::from(format!("/tmp/download-1-{v}.noindex/Trek.app"));
        assert_eq!(u.finish_download(job.ticket, v.into(), Ok(path.clone()), now), None);
        path
    }

    #[test]
    fn updater_offers_one_next_step_per_state() {
        let action = |s: UpdateStatus| describe_update(&s, true, None).action;
        let staged = PathBuf::from("/tmp/Trek.app");
        assert_eq!(action(UpdateStatus::Idle), Some(UpdateAction::Check));
        assert_eq!(action(UpdateStatus::Available { version: "0.2.1".into() }), Some(UpdateAction::Download));
        assert_eq!(action(UpdateStatus::Downloading { version: "0.2.1".into(), progress: 0.5 }), None);
        assert_eq!(action(UpdateStatus::Ready { version: "0.2.1".into(), staged: staged.clone() }), Some(UpdateAction::Restart));
        assert_eq!(action(UpdateStatus::RestartPending { version: "0.2.1".into(), staged }), None);
        assert_eq!(action(UpdateStatus::Failed("Couldn't check for updates: offline".into())), Some(UpdateAction::Check));
        let downloading = describe_update(&UpdateStatus::Downloading { version: "0.2.1".into(), progress: 0.42 }, true, None);
        assert_eq!((downloading.line.as_str(), downloading.busy, downloading.progress), ("Downloading Trek 0.2.1 · 42%", true, Some(0.42)));
        assert_eq!(describe_update(&UpdateStatus::Idle, false, None).line, "Automatic checks are off.");
    }

    #[test]
    fn builds_that_cant_replace_themselves_offer_nothing() {
        let dev = describe_update(&UpdateStatus::Idle, true, Some(Blocker::DevBuild));
        assert_eq!((dev.action, dev.line.as_str()), (None, Blocker::DevBuild.message()));
        assert_eq!(describe_update(&UpdateStatus::UpToDate, true, Some(Blocker::Translocated)).action, None);
    }

    #[test]
    fn checks_run_daily_by_the_clock_and_retry_within_the_hour() {
        let mut u = Updater::default();
        assert!(u.check_due(true, 0), "never checked");
        assert!(!u.check_due(false, 0), "automatic checks off");
        let t = u.begin_check(Channel::Stable, 0).unwrap();
        assert!(u.begin_check(Channel::Stable, 0).is_none(), "one at a time");
        assert!(!u.finish_check(t, Ok(None), false, true, 0));
        assert_eq!(u.status, UpdateStatus::UpToDate);
        assert!(!u.check_due(true, 23 * HOUR));
        assert!(u.check_due(true, 24 * HOUR));

        // A background check that fails stays quiet and retries in an hour.
        let t = u.begin_check(Channel::Stable, 24 * HOUR).unwrap();
        u.finish_check(t, Err("offline".into()), false, true, 24 * HOUR);
        assert_eq!(u.status, UpdateStatus::Idle);
        assert!(u.check_due(true, 25 * HOUR));
        let t = u.begin_check(Channel::Stable, 25 * HOUR).unwrap();
        u.finish_check(t, Err("offline".into()), true, true, 25 * HOUR);
        assert!(matches!(&u.status, UpdateStatus::Failed(e) if e.contains("offline")), "the user asked: say why");
    }

    #[test]
    fn offers_not_downloaded_are_checked_again() {
        let mut u = Updater::default();
        let t = u.begin_check(Channel::Beta, 0).unwrap();
        assert!(!u.finish_check(t, Ok(Some(release("0.3.0-beta.1"))), false, false, 0), "automatic downloads off");
        assert!(!u.check_due(true, HOUR));
        assert!(u.check_due(true, 24 * HOUR));
        // A newer build replaces the offer.
        let t = u.begin_check(Channel::Beta, 24 * HOUR).unwrap();
        u.finish_check(t, Ok(Some(release("0.3.0-beta.2"))), false, false, 24 * HOUR);
        assert_eq!(u.status, UpdateStatus::Available { version: "0.3.0-beta.2".into() });
        // Offline: the offer stays up.
        let t = u.begin_check(Channel::Beta, 48 * HOUR).unwrap();
        u.finish_check(t, Err("offline".into()), false, false, 48 * HOUR);
        assert_eq!(u.status, UpdateStatus::Available { version: "0.3.0-beta.2".into() });
        assert_eq!(u.begin_download().unwrap().update.version.to_string(), "0.3.0-beta.2");
    }

    #[test]
    fn a_download_that_fails_offline_is_retried_and_a_bad_release_waits_a_day() {
        let mut u = Updater::default();
        let t = u.begin_check(Channel::Stable, 0).unwrap();
        u.finish_check(t, Ok(Some(release("0.2.1"))), false, false, 0);
        let job = u.begin_download().unwrap();
        assert!(u.begin_download().is_none(), "already downloading");
        assert!(u.download_progress(job.ticket, 0.5));
        u.finish_download(job.ticket, "0.2.1".into(), Err(("couldn't connect to github.com".into(), true)), 0);
        assert!(matches!(&u.status, UpdateStatus::Failed(e) if e.starts_with("Couldn't update to Trek 0.2.1")));
        assert!(u.check_due(true, HOUR));

        let t = u.begin_check(Channel::Stable, HOUR).unwrap();
        u.finish_check(t, Ok(Some(release("0.2.1"))), false, false, HOUR);
        let job = u.begin_download().unwrap();
        u.finish_download(job.ticket, "0.2.1".into(), Err(("the update's signature doesn't match".into(), false)), HOUR);
        assert!(!u.check_due(true, 2 * HOUR));
        assert!(u.check_due(true, 25 * HOUR));
    }

    #[test]
    fn a_staged_update_is_dropped_when_the_channel_changes() {
        let mut u = Updater::default();
        let path = staged(&mut u, Channel::Nightly, "0.3.0-nightly.20261002", 0);
        assert!(!u.channel_changed(Channel::Nightly));
        assert!(u.channel_changed(Channel::Stable));
        assert_eq!(u.drop_update(), Some(path), "the staged nightly is deleted, never installed on quit");
        assert_eq!((u.status.clone(), u.offer.is_none()), (UpdateStatus::Idle, true));
        assert!(u.begin_check(Channel::Stable, 0).is_some());
    }

    #[test]
    fn work_from_the_old_channel_doesnt_count_after_a_switch() {
        // A check still running when the channel changes.
        let mut u = Updater::default();
        let old = u.begin_check(Channel::Beta, 0).unwrap();
        u.drop_update();
        let new = u.begin_check(Channel::Stable, 0).unwrap();
        assert!(!u.finish_check(old, Ok(Some(release("0.3.0-beta.1"))), false, true, 0), "the beta isn't downloaded");
        assert_eq!(u.status, UpdateStatus::Checking);
        u.finish_check(new, Ok(None), false, true, 0);
        assert_eq!(u.status, UpdateStatus::UpToDate);

        // A download still running: stopped, and its staged copy handed back for deletion.
        let t = u.begin_check(Channel::Beta, 0).unwrap();
        u.finish_check(t, Ok(Some(release("0.3.0-beta.1"))), false, true, 0);
        let job = u.begin_download().unwrap();
        assert_eq!(u.drop_update(), None);
        assert!(job.cancel.load(Ordering::Relaxed));
        assert!(!u.download_progress(job.ticket, 0.9));
        let late = PathBuf::from("/tmp/download-1-0.noindex/Trek.app");
        assert_eq!(u.finish_download(job.ticket, "0.3.0-beta.1".into(), Ok(late.clone()), 0), Some(late));
        assert_eq!(u.status, UpdateStatus::Idle);
    }

    #[test]
    fn release_notes_show_while_an_update_is_on_offer() {
        let mut u = Updater::default();
        staged(&mut u, Channel::Stable, "0.2.1", 0);
        assert_eq!(u.notes().map(|n| n.notes.as_str()), Some("- faster"));
        u.status = UpdateStatus::UpToDate;
        assert!(u.notes().is_none());
    }
}
