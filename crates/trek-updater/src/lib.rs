//! The updater's state: what it's doing, what it found, when it checks next, and which results
//! still count. `Workspace` runs the checks and downloads (trek_core::update) and reports back.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use trek_core::changelog::{self, Release};
use trek_core::settings::Channel;
use trek_core::update::{AvailableUpdate, Blocker, Found};

/// A quarter of an hour between background checks, on every channel. Stable releases come out
/// several times on some days, and an hour's wait still read as "it never told me" (the release
/// was out, Trek hadn't looked yet). A check is a small manifest per channel.
const CHECK_EVERY_MS: i64 = 15 * 60 * 1000;
/// Trek coming to the front looks again when the last check is at least this old.
const ON_ACTIVATE_AFTER_MS: i64 = 5 * 60 * 1000;
/// After a failure that may pass (offline, server trouble), try again within the hour.
const RETRY_AFTER_MS: i64 = 60 * 60 * 1000;
/// A newer version is out without this platform's build: it's added to the release minutes after
/// the other's, so look again soon.
const COMING_RETRY_MS: i64 = 15 * 60 * 1000;
/// A release that didn't verify isn't downloaded again for a day (a newer one is, by Check now).
const BAD_RELEASE_WAIT_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Default, PartialEq)]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// A newer version is out without this platform's build yet (it's added to the release some
    /// minutes after the other's): nothing to download, check again soon.
    Coming {
        version: String,
    },
    Available {
        version: String,
    },
    Downloading {
        version: String,
        progress: f32,
    },
    /// Verified and unpacked at `staged`: installs on "Restart to update" or when Trek quits.
    Ready {
        version: String,
        staged: PathBuf,
    },
    /// Ready, waiting for agent work to be over: running or paused turns, plans waiting for an
    /// answer (`Workspace::work_in_flight`).
    RestartPending {
        version: String,
        staged: PathBuf,
    },
    /// A full sentence for the user ("Couldn't download Trek 0.2.1: …").
    Failed(String),
}

/// The one thing the updater offers to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateAction {
    Check,
    Download,
    Restart,
    /// A restart waiting on agents: go now, stopping them.
    RestartNow,
    /// Windows: Trek is where it can't update itself (a folder ransomware protection guards, say):
    /// copy itself to the user's programs folder and start from there (`trek_core::update::relocate`).
    Move,
    /// A move waiting on agents: go now, stopping them.
    MoveNow,
    /// After a move: delete the copy Trek left behind.
    RemoveOldCopy,
    /// After a move: leave the old copy where it is, and stop offering to remove it.
    DismissOldCopy,
}

impl UpdateAction {
    pub fn label(self) -> &'static str {
        match self {
            UpdateAction::Check => "Check now",
            UpdateAction::Download => "Download",
            UpdateAction::Restart => "Restart to update",
            UpdateAction::RestartNow => "Restart now",
            UpdateAction::Move => "Move Trek to its own folder",
            UpdateAction::MoveNow => "Move now",
            UpdateAction::RemoveOldCopy => "Remove old copy",
            UpdateAction::DismissOldCopy => "Dismiss",
        }
    }

    /// The step that gets Trek going again (an update, or out of a place it can't update in), not
    /// one that only looks.
    pub fn is_primary(self) -> bool {
        matches!(self, UpdateAction::Restart | UpdateAction::Move)
    }
}

/// What the Updates page and the sidebar updater show for the current state.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateView {
    pub line: String,
    pub action: Option<UpdateAction>,
    /// A second step, quieter, beside the first (the Updates page shows it; the sidebar's updater
    /// keeps to one button).
    pub secondary: Option<UpdateAction>,
    /// Checking or downloading: the action button spins.
    pub busy: bool,
    pub progress: Option<f32>,
}

/// The copy of Trek left in the folder it moved out of, while the Updates page offers to remove it
/// (`trek_core::update::relocate::at_launch`).
#[derive(Debug, Clone, PartialEq)]
pub struct OldCopy {
    pub from: PathBuf,
    /// Where Trek runs now.
    pub to: PathBuf,
    /// What the move did to the user's shortcuts, which this launch knows only when the move was
    /// its own.
    pub shortcuts: trek_shortcuts::Report,
    /// Windows turned the removal away: the rest is the user's.
    pub refused: bool,
}

/// What Trek says about a move: where it runs now, and what became of the user's shortcuts.
/// (`said` has no full stop; this adds it.)
pub fn with_shortcuts(said: &str, shortcuts: &trek_shortcuts::Report) -> String {
    let mut out = said.to_string();
    if let Some(changed) = shortcuts.changed_clause() {
        out = format!("{out}, and {changed}");
    }
    out.push('.');
    if let Some(left) = shortcuts.left_sentence() {
        out = format!("{out} {left}");
    }
    out
}

/// The Updates page's offer to remove the old copy: its own line and steps, beside the page's usual.
pub fn describe_old_copy(old: &OldCopy) -> UpdateView {
    let runs = format!("Trek now runs from {}", old.to.display());
    if old.refused {
        // The one thing left to the user, and only when Windows refuses.
        let line = format!(
            "{} Windows wouldn't let Trek remove the copy in {}; delete it yourself when convenient.",
            with_shortcuts(&runs, &old.shortcuts),
            old.from.display()
        );
        return UpdateView {
            line,
            action: Some(UpdateAction::DismissOldCopy),
            secondary: None,
            busy: false,
            progress: None,
        };
    }
    let line = with_shortcuts(
        &format!(
            "{runs}. The copy in {} isn't needed any more",
            old.from.display()
        ),
        &old.shortcuts,
    );
    UpdateView {
        line,
        action: Some(UpdateAction::RemoveOldCopy),
        secondary: Some(UpdateAction::DismissOldCopy),
        busy: false,
        progress: None,
    }
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
    /// Every published release, newest first (`trek_core::changelog`); empty until the first
    /// fetch.
    pub changelog: Vec<Release>,
    /// When the background check runs next, in wall-clock ms. Not `Instant`: that stops while
    /// the computer sleeps, and "every hour" would become every few hours on a laptop. `None`:
    /// due now (never checked, or the channel just changed).
    next_check: Option<i64>,
    /// The channel the last check asked.
    channel: Option<Channel>,
    /// When the last check started, in wall-clock ms (`check_on_activate`).
    last_check: Option<i64>,
    /// A check runs while an update is staged (`Ready`), which stays on screen meanwhile.
    rechecking: bool,
    /// A staged update a newer release took the place of, for the caller to delete
    /// (`take_replaced`).
    replaced: Option<PathBuf>,
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
}

impl Updater {
    /// Start a check. `None` while one runs, an update is downloading, or a restart into one is
    /// pending. A staged update (`Ready`) doesn't stop it: Trek can stay open for days with one
    /// waiting, and a newer release takes its place.
    pub fn begin_check(&mut self, channel: Channel, now: i64) -> Option<Ticket> {
        match self.status {
            UpdateStatus::Checking
            | UpdateStatus::Downloading { .. }
            | UpdateStatus::RestartPending { .. } => return None,
            UpdateStatus::Ready { .. } if self.rechecking => return None,
            UpdateStatus::Ready { .. } => self.rechecking = true,
            _ => self.status = UpdateStatus::Checking,
        }
        self.next_check = Some(now + CHECK_EVERY_MS);
        self.last_check = Some(now);
        self.channel = Some(channel);
        Some(Ticket(self.generation))
    }

    /// A check finished. True when what it found should start downloading (`auto_download`).
    /// A background check that fails stays quiet; one the user asked for says why.
    pub fn finish_check(
        &mut self,
        ticket: Ticket,
        result: Result<Found, String>,
        user_initiated: bool,
        auto_download: bool,
        now: i64,
    ) -> bool {
        if ticket.0 != self.generation {
            return false;
        }
        if std::mem::take(&mut self.rechecking) {
            return self.finish_recheck(result, auto_download, now);
        }
        match result {
            Ok(Found::Update(u)) => {
                self.status = UpdateStatus::Available {
                    version: u.version.to_string(),
                };
                self.offer = Some(u);
                auto_download
            }
            Ok(Found::Nothing) => {
                self.offer = None;
                self.status = UpdateStatus::UpToDate;
                false
            }
            Ok(Found::Coming(version)) => {
                self.offer = None;
                self.status = UpdateStatus::Coming {
                    version: version.to_string(),
                };
                // This platform's build follows within minutes.
                self.next_check = Some(now + COMING_RETRY_MS);
                false
            }
            Err(e) => {
                tracing::warn!("update check failed: {e}");
                self.next_check = Some(now + RETRY_AFTER_MS);
                self.status = match &self.offer {
                    _ if user_initiated => {
                        UpdateStatus::Failed(format!("Couldn't check for updates: {e}"))
                    }
                    // What an earlier check found is still on offer.
                    Some(o) => UpdateStatus::Available {
                        version: o.version.to_string(),
                    },
                    None => UpdateStatus::Idle,
                };
                false
            }
        }
    }

    /// A check made while an update was staged finished: a newer release takes its place (true when
    /// it should start downloading), anything else leaves it ready. Quiet either way, failures too:
    /// the update on screen is still the one to restart into. One that arrives after the restart
    /// was asked for (`RestartPending`) changes nothing.
    fn finish_recheck(
        &mut self,
        result: Result<Found, String>,
        auto_download: bool,
        now: i64,
    ) -> bool {
        let UpdateStatus::Ready { staged, .. } = &self.status else {
            return false;
        };
        match result {
            Ok(Found::Update(u)) if self.offer.as_ref().is_none_or(|o| u.version > o.version) => {
                tracing::info!("update {} replaces the staged one", u.version);
                self.replaced = Some(staged.clone());
                self.status = UpdateStatus::Available {
                    version: u.version.to_string(),
                };
                self.offer = Some(u);
                auto_download
            }
            Ok(Found::Coming(_)) => {
                self.next_check = Some(now + COMING_RETRY_MS);
                false
            }
            Err(e) => {
                tracing::warn!("update check failed: {e}");
                self.next_check = Some(now + RETRY_AFTER_MS);
                false
            }
            Ok(_) => false,
        }
    }

    /// The staged update a newer release replaced (`finish_check`), once, for the caller to delete.
    pub fn take_replaced(&mut self) -> Option<PathBuf> {
        self.replaced.take()
    }

    /// The background check is due (housekeeping asks every minute). An offer not downloaded yet
    /// (automatic downloads off) is checked again too: channels replace their archives, so an old
    /// offer's download can disappear, and a newer build may be out. So is one staged and waiting
    /// for a restart, which a newer release replaces.
    /// Trek came to the front: the next background check is due now if the last one is a few
    /// minutes old, so a release that came out while Trek sat behind other windows shows at once.
    pub fn check_on_activate(&mut self, now: i64) {
        if self
            .last_check
            .is_some_and(|t| now - t >= ON_ACTIVATE_AFTER_MS)
        {
            self.next_check = Some(now);
        }
    }

    pub fn check_due(&self, auto_check: bool, now: i64) -> bool {
        auto_check
            && self.next_check.is_none_or(|t| now >= t)
            && match self.status {
                UpdateStatus::Idle
                | UpdateStatus::UpToDate
                | UpdateStatus::Coming { .. }
                | UpdateStatus::Available { .. }
                | UpdateStatus::Failed(_) => true,
                UpdateStatus::Ready { .. } => !self.rechecking,
                UpdateStatus::Checking
                | UpdateStatus::Downloading { .. }
                | UpdateStatus::RestartPending { .. } => false,
            }
    }

    /// Start downloading what the last check found. `None` if nothing is on offer or a download
    /// already ran.
    pub fn begin_download(&mut self) -> Option<DownloadJob> {
        let update = self.offer.clone()?;
        if matches!(
            self.status,
            UpdateStatus::Downloading { .. }
                | UpdateStatus::Ready { .. }
                | UpdateStatus::RestartPending { .. }
        ) {
            return None;
        }
        self.status = UpdateStatus::Downloading {
            version: update.version.to_string(),
            progress: 0.0,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        Some(DownloadJob {
            update,
            ticket: Ticket(self.generation),
            cancel,
        })
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
    pub fn finish_download(
        &mut self,
        ticket: Ticket,
        version: String,
        result: Result<PathBuf, (String, bool)>,
        now: i64,
    ) -> Option<PathBuf> {
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
                self.next_check = Some(
                    now + if transient {
                        RETRY_AFTER_MS
                    } else {
                        BAD_RELEASE_WAIT_MS
                    },
                );
                self.status =
                    UpdateStatus::Failed(format!("Couldn't update to Trek {version}: {e}"));
            }
        }
        None
    }

    /// The channel setting differs from the one the last check asked.
    pub fn channel_changed(&self, channel: Channel) -> bool {
        self.channel.is_some_and(|c| c != channel)
    }

    /// The channel setting changed to `channel`: what the old channel found goes unless `channel`
    /// offers it too (a stable release is on every channel; a nightly staged before the user left
    /// Nightly must not install on quit), and `channel` is asked at once, or as soon as a download
    /// in flight is done. Returns the staged copy to delete, if one went.
    pub fn switch_channel(&mut self, channel: Channel) -> Option<PathBuf> {
        self.channel = Some(channel);
        self.next_check = None;
        let kept = !matches!(self.status, UpdateStatus::Checking)
            && self
                .offer
                .as_ref()
                .is_some_and(|o| changelog::offers(channel, &o.version));
        if !kept {
            return self.drop_update();
        }
        // A check of the old channel's, running while the update was staged, no longer counts. A
        // download in flight does (no check runs meanwhile).
        if !matches!(self.status, UpdateStatus::Downloading { .. }) {
            self.generation += 1;
            self.rechecking = false;
        }
        None
    }

    /// Forget the update in flight or waiting: stop its download, ignore late results of
    /// earlier checks and downloads. Returns the staged copy to delete, if there was one.
    pub fn drop_update(&mut self) -> Option<PathBuf> {
        self.generation += 1;
        self.rechecking = false;
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.offer = None;
        match std::mem::take(&mut self.status) {
            UpdateStatus::Ready { staged, .. } | UpdateStatus::RestartPending { staged, .. } => {
                Some(staged)
            }
            _ => None,
        }
    }

    /// The update the last check found, while it's on offer (not yet installed or dropped).
    fn offered(&self) -> Option<&AvailableUpdate> {
        let offered = matches!(
            self.status,
            UpdateStatus::Available { .. }
                | UpdateStatus::Downloading { .. }
                | UpdateStatus::Ready { .. }
                | UpdateStatus::RestartPending { .. }
        );
        self.offer.as_ref().filter(|_| offered)
    }

    /// What the update on offer brings: the notes of every release since this one (an update can
    /// skip several), or the offer's own notes until the release list has been read.
    pub fn pending_changes(&self, channel: Channel, repo: Option<&str>) -> Option<Changes> {
        let offer = self.offered()?;
        let current = trek_core::update::current_version();
        let mut releases: Vec<Release> =
            changelog::between(&self.changelog, channel, &current, &offer.version)
                .into_iter()
                .cloned()
                .collect();
        if releases.first().is_none_or(|r| r.version != offer.version) && !offer.notes.is_empty() {
            releases.insert(
                0,
                Release {
                    version: offer.version.clone(),
                    notes: offer.notes.clone(),
                    date: offer.pub_date.clone(),
                    url: String::new(),
                    prerelease: !offer.version.pre.is_empty(),
                },
            );
        }
        if releases.is_empty() {
            return None;
        }
        // Nightlies have no tag of their own to compare against.
        let tagged = !offer.version.pre.starts_with("nightly") && current.pre.is_empty();
        let compare = repo
            .filter(|_| tagged)
            .map(|r| changelog::compare_url(r, &current, &offer.version));
        Some(Changes { releases, compare })
    }

    /// This version's release, once the release list has been read.
    pub fn installed_release(&self) -> Option<&Release> {
        let current = trek_core::update::current_version();
        self.changelog.iter().find(|r| r.version == current)
    }
}

/// Release notes to show for an update: newest release first.
#[derive(Debug, Clone, PartialEq)]
pub struct Changes {
    pub releases: Vec<Release>,
    /// The code changes between the two versions on GitHub, when both are tagged there.
    pub compare: Option<String>,
}

/// What the Updates page says of a version on offer on `channel`: nothing when it's the channel's
/// own; a stable release on Beta (or a beta on Nightly) says that's what it is, since the channel
/// has nothing newer of its own.
fn from_steadier_channel(version: &str, channel: Channel) -> Option<String> {
    let from = changelog::channel_of(&semver::Version::parse(version).ok()?);
    let kind = match from {
        _ if from == channel => return None,
        Channel::Stable => "a stable release",
        Channel::Beta => "a beta",
        Channel::Nightly => return None,
    };
    let own = match channel {
        Channel::Stable => return None,
        Channel::Beta => "beta",
        Channel::Nightly => "nightly",
    };
    Some(format!(" It's {kind}: there's no newer {own}."))
}

/// What "up to date" covers on `channel`: the channels it gets releases from.
fn up_to_date_on(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "",
        Channel::Beta => " There's no newer beta or stable release.",
        Channel::Nightly => " There's no newer nightly, beta or stable release.",
    }
}

/// The updater's status line and next action for a state, on the `channel` the user picked.
pub fn describe_update(
    status: &UpdateStatus,
    auto_check: bool,
    blocker: Option<Blocker>,
    channel: Channel,
) -> UpdateView {
    let view = |line: String, action: Option<UpdateAction>| UpdateView {
        line,
        action,
        secondary: None,
        busy: false,
        progress: None,
    };
    if let Some(blocker) = blocker {
        // Windows: Trek can move itself out of where it is, in one click.
        if trek_words::is_windows() && blocker.movable() {
            // The cause only: the message's advice to move the folder by hand would contradict
            // the button. One Windows may lift (it refused a write; the user may have told it to
            // let Trek be) can be asked again beside the move (`blocker_now`).
            let line = format!(
                "{} {}",
                blocker.cause(),
                trek_core::update::relocate::moves_line()
            );
            return UpdateView {
                secondary: blocker.may_pass().then_some(UpdateAction::Check),
                ..view(line, Some(UpdateAction::Move))
            };
        }
        // One Windows may lift (it refused a write) can be asked again; where Trek is can't.
        return view(
            blocker.message(),
            blocker.may_pass().then_some(UpdateAction::Check),
        );
    }
    match status {
        UpdateStatus::Idle if auto_check => view(
            "Trek checks for updates every 15 minutes.".into(),
            Some(UpdateAction::Check),
        ),
        UpdateStatus::Idle => view(
            "Automatic checks are off.".into(),
            Some(UpdateAction::Check),
        ),
        UpdateStatus::Checking => UpdateView {
            busy: true,
            ..view("Checking for updates…".into(), Some(UpdateAction::Check))
        },
        UpdateStatus::UpToDate => view(
            format!(
                "Trek {} is up to date.{}",
                trek_core::VERSION,
                up_to_date_on(channel)
            ),
            Some(UpdateAction::Check),
        ),
        UpdateStatus::Coming { version } => view(
            format!(
                "Trek {version} is out; the {} build is on its way. Check again in a while.",
                trek_words::words().build_name
            ),
            Some(UpdateAction::Check),
        ),
        UpdateStatus::Available { version } => view(
            format!(
                "Trek {version} is available.{}",
                from_steadier_channel(version, channel).unwrap_or_default()
            ),
            Some(UpdateAction::Download),
        ),
        UpdateStatus::Downloading { version, progress } => UpdateView {
            busy: true,
            progress: Some(*progress),
            ..view(
                format!("Downloading Trek {version} · {:.0}%", progress * 100.),
                None,
            )
        },
        UpdateStatus::Ready { version, .. } => view(
            format!("Trek {version} is ready. Restart now, or it installs when you quit."),
            Some(UpdateAction::Restart),
        ),
        UpdateStatus::RestartPending { version, .. } => view(
            format!(
                "Trek restarts into {version} when your agents finish their turns. Restart now stops them."
            ),
            Some(UpdateAction::RestartNow),
        ),
        UpdateStatus::Failed(e) => view(e.clone(), Some(UpdateAction::Check)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trek_core::update::Artifact;
    use trek_shortcuts::Report;

    const HOUR: i64 = 60 * 60 * 1000;
    const QUARTER: i64 = HOUR / 4;
    const EVERY: i64 = CHECK_EVERY_MS;

    fn release(v: &str) -> AvailableUpdate {
        AvailableUpdate {
            version: v.parse().unwrap(),
            notes: "- faster".into(),
            pub_date: String::new(),
            artifact: Artifact {
                url: "u".into(),
                sha256: "s".into(),
                signature: "sig".into(),
            },
        }
    }

    /// Checked at `now` on `channel` and found `v`, downloaded and staged.
    fn staged(u: &mut Updater, channel: Channel, v: &str, now: i64) -> PathBuf {
        let t = u.begin_check(channel, now).unwrap();
        assert!(u.finish_check(t, Ok(Found::Update(release(v))), false, true, now));
        let job = u.begin_download().unwrap();
        let path = PathBuf::from(format!("/tmp/download-1-{v}.noindex/Trek.app"));
        assert_eq!(
            u.finish_download(job.ticket, v.into(), Ok(path.clone()), now),
            None
        );
        path
    }

    #[test]
    fn updater_offers_one_next_step_per_state() {
        let action = |s: UpdateStatus| describe_update(&s, true, None, Channel::Stable).action;
        let staged = PathBuf::from("/tmp/Trek.app");
        assert_eq!(action(UpdateStatus::Idle), Some(UpdateAction::Check));
        assert_eq!(
            action(UpdateStatus::Available {
                version: "0.2.1".into()
            }),
            Some(UpdateAction::Download)
        );
        assert_eq!(
            action(UpdateStatus::Downloading {
                version: "0.2.1".into(),
                progress: 0.5
            }),
            None
        );
        assert_eq!(
            action(UpdateStatus::Ready {
                version: "0.2.1".into(),
                staged: staged.clone()
            }),
            Some(UpdateAction::Restart)
        );
        assert_eq!(
            action(UpdateStatus::RestartPending {
                version: "0.2.1".into(),
                staged
            }),
            Some(UpdateAction::RestartNow),
            "waiting never leaves you stuck"
        );
        assert_eq!(
            action(UpdateStatus::Failed(
                "Couldn't check for updates: offline".into()
            )),
            Some(UpdateAction::Check)
        );
        let downloading = describe_update(
            &UpdateStatus::Downloading {
                version: "0.2.1".into(),
                progress: 0.42,
            },
            true,
            None,
            Channel::Stable,
        );
        assert_eq!(
            (
                downloading.line.as_str(),
                downloading.busy,
                downloading.progress
            ),
            ("Downloading Trek 0.2.1 · 42%", true, Some(0.42))
        );
        assert_eq!(
            describe_update(&UpdateStatus::Idle, false, None, Channel::Stable).line,
            "Automatic checks are off."
        );
    }

    #[test]
    fn builds_that_cant_replace_themselves_offer_nothing() {
        // (Windows offers a move out of a shared folder: `trek_can_move_itself_out_of_where_it_cant_update`.)
        let _platform = trek_words::pretend(false);
        let dev = describe_update(
            &UpdateStatus::Idle,
            true,
            Some(Blocker::DevBuild),
            Channel::Stable,
        );
        assert_eq!((dev.action, dev.line), (None, Blocker::DevBuild.message()));
        assert_eq!(
            describe_update(
                &UpdateStatus::UpToDate,
                true,
                Some(Blocker::Translocated),
                Channel::Stable
            )
            .action,
            None
        );
        assert_eq!(
            describe_update(
                &UpdateStatus::UpToDate,
                true,
                Some(Blocker::SharedFolder),
                Channel::Stable
            )
            .action,
            None
        );
    }

    #[test]
    fn a_blocker_windows_may_lift_can_be_checked_again() {
        // Ransomware protection judges each program as it runs, and may be turned off: "Check now"
        // asks again (`blocker_now`). (Windows offers the move instead: `trek_can_move_itself_out_of_where_it_cant_update`.)
        let _platform = trek_words::pretend(false);
        let guarded = Blocker::GuardedFolder(trek_core::update::KnownFolder::Documents);
        let v = describe_update(
            &UpdateStatus::UpToDate,
            true,
            Some(guarded),
            Channel::Stable,
        );
        assert_eq!(
            (v.line, v.action),
            (guarded.message(), Some(UpdateAction::Check))
        );
        assert!(v.action.is_some_and(|a| a.label() == "Check now"));
        assert_eq!(
            describe_update(
                &UpdateStatus::Idle,
                true,
                Some(Blocker::ReadOnlyLocation),
                Channel::Stable
            )
            .action,
            Some(UpdateAction::Check)
        );
    }

    #[test]
    fn trek_can_move_itself_out_of_where_it_cant_update() {
        use trek_core::update::KnownFolder;
        let _platform = trek_words::pretend(true);
        let moves = trek_core::update::relocate::moves_line();
        assert!(
            moves.starts_with("Trek will move itself to ")
                && moves.ends_with(" and start from there."),
            "{moves}"
        );
        assert!(
            moves.contains("Programs") && moves.contains("Trek"),
            "{moves}"
        );
        for blocker in [
            Blocker::GuardedFolder(KnownFolder::Documents),
            Blocker::ReadOnlyLocation,
            Blocker::SharedFolder,
            Blocker::ProtectedFolder,
        ] {
            for status in [
                UpdateStatus::Idle,
                UpdateStatus::UpToDate,
                UpdateStatus::Failed("Couldn't check for updates: offline".into()),
            ] {
                let v = describe_update(&status, true, Some(blocker), Channel::Stable);
                assert_eq!(
                    v.action,
                    Some(UpdateAction::Move),
                    "{blocker:?} in {status:?}"
                );
                assert_eq!(
                    v.line,
                    format!("{} {moves}", blocker.cause()),
                    "why Trek can't update itself, then where it goes"
                );
                assert!(
                    !v.line.contains("to get updates"),
                    "no advice to move the folder by hand next to the button: {}",
                    v.line
                );
                assert!(!v.busy && v.progress.is_none());
            }
        }
        // Check now beside the move when Windows may lift what's in the way; the move alone when
        // it can't (where Trek is can't change, and neither can who shares the folder).
        for (blocker, secondary) in [
            (
                Blocker::GuardedFolder(KnownFolder::Documents),
                Some(UpdateAction::Check),
            ),
            (Blocker::ReadOnlyLocation, Some(UpdateAction::Check)),
            (Blocker::SharedFolder, None),
            (Blocker::ProtectedFolder, None),
        ] {
            for status in [
                UpdateStatus::Idle,
                UpdateStatus::UpToDate,
                UpdateStatus::Failed("Couldn't check for updates: offline".into()),
            ] {
                assert_eq!(
                    describe_update(&status, true, Some(blocker), Channel::Stable).secondary,
                    secondary,
                    "{blocker:?} in {status:?}"
                );
            }
        }
        // And nothing else has a second button.
        for status in [
            UpdateStatus::Idle,
            UpdateStatus::UpToDate,
            UpdateStatus::Available {
                version: "0.2.1".into(),
            },
            UpdateStatus::Checking,
        ] {
            assert_eq!(
                describe_update(&status, true, None, Channel::Stable).secondary,
                None
            );
        }
        assert_eq!(UpdateAction::Move.label(), "Move Trek to its own folder");
        assert!(
            UpdateAction::Move.is_primary()
                && UpdateAction::Restart.is_primary()
                && !UpdateAction::MoveNow.is_primary()
                && !UpdateAction::Check.is_primary()
        );
        // Not for what moving doesn't help: a build run from a checkout, a link to the folder.
        for blocker in [
            Blocker::DevBuild,
            Blocker::LinkedFolder,
            Blocker::Translocated,
        ] {
            let v = describe_update(&UpdateStatus::Idle, true, Some(blocker), Channel::Stable);
            assert_eq!((v.action, v.line), (None, blocker.message()), "{blocker:?}");
        }
        // And not on a Mac, whatever the blocker: its own words and actions are unchanged.
        drop(_platform);
        let _mac = trek_words::pretend(false);
        let v = describe_update(
            &UpdateStatus::Idle,
            true,
            Some(Blocker::ReadOnlyLocation),
            Channel::Stable,
        );
        assert_eq!(
            (v.action, v.secondary, v.line),
            (
                Some(UpdateAction::Check),
                None,
                Blocker::ReadOnlyLocation.message()
            )
        );
        assert_eq!(
            describe_update(
                &UpdateStatus::Idle,
                true,
                Some(Blocker::SharedFolder),
                Channel::Stable
            )
            .action,
            None
        );
    }

    fn old_copy(shortcuts: Report, refused: bool) -> OldCopy {
        let dir = std::env::temp_dir();
        OldCopy {
            from: dir.join("Documents").join("Trek"),
            to: dir.join("Programs").join("Trek"),
            shortcuts,
            refused,
        }
    }

    fn changed(n: usize) -> Report {
        Report {
            changed: (0..n)
                .map(|i| PathBuf::from(format!("Trek {i}.lnk")))
                .collect(),
            left: vec![],
        }
    }

    #[test]
    fn after_a_move_the_old_copy_is_offered_for_removal_or_dismissal() {
        let old = old_copy(Report::default(), false);
        let v = describe_old_copy(&old);
        assert_eq!(
            v.line,
            format!(
                "Trek now runs from {}. The copy in {} isn't needed any more.",
                old.to.display(),
                old.from.display()
            )
        );
        assert_eq!(
            (v.action, v.secondary, v.busy, v.progress),
            (
                Some(UpdateAction::RemoveOldCopy),
                Some(UpdateAction::DismissOldCopy),
                false,
                None
            )
        );
        assert_eq!(
            (
                UpdateAction::RemoveOldCopy.label(),
                UpdateAction::DismissOldCopy.label()
            ),
            ("Remove old copy", "Dismiss")
        );
        assert!(
            !UpdateAction::RemoveOldCopy.is_primary() && !UpdateAction::DismissOldCopy.is_primary(),
            "tidying up isn't the page's main step"
        );

        // What became of the user's shortcuts is in the same sentence, in the singular too.
        let one = describe_old_copy(&old_copy(changed(1), false));
        assert!(
            one.line
                .ends_with("isn't needed any more, and 1 shortcut now points at the new copy."),
            "{}",
            one.line
        );
        let two = describe_old_copy(&old_copy(changed(2), false));
        assert!(
            two.line
                .ends_with("isn't needed any more, and 2 shortcuts now point at the new copy."),
            "{}",
            two.line
        );
    }

    #[test]
    fn an_old_copy_windows_refused_to_remove_is_the_users_to_delete() {
        let old = old_copy(Report::default(), true);
        let v = describe_old_copy(&old);
        assert_eq!(
            v.line,
            format!(
                "Trek now runs from {}. Windows wouldn't let Trek remove the copy in {}; delete it yourself when convenient.",
                old.to.display(),
                old.from.display()
            )
        );
        assert_eq!(
            (v.action, v.secondary),
            (Some(UpdateAction::DismissOldCopy), None),
            "nothing left to try: Dismiss clears the note"
        );
        let with = describe_old_copy(&old_copy(changed(2), true));
        assert!(with.line.starts_with(&format!("Trek now runs from {}, and 2 shortcuts now point at the new copy. Windows wouldn't let Trek remove", old.to.display())), "{}", with.line);
    }

    #[test]
    fn shortcuts_that_couldnt_be_changed_are_said_after_the_move() {
        let left = trek_shortcuts::Left {
            lnk: PathBuf::from("Trek.lnk"),
            place: "on your Desktop",
            refused: true,
            why: "Access is denied.".into(),
        };
        let report = Report {
            changed: vec![PathBuf::from("Trek.lnk")],
            left: vec![left],
        };
        assert_eq!(
            with_shortcuts("Trek now runs from here", &report),
            "Trek now runs from here, and 1 shortcut now points at the new copy. Windows wouldn't let Trek change the shortcut \"Trek\" on your Desktop, so it still starts the old copy."
        );
        assert_eq!(
            with_shortcuts("Trek now runs from here", &Report::default()),
            "Trek now runs from here."
        );
    }

    #[test]
    fn a_release_without_this_platforms_build_yet_says_so_and_is_checked_again_soon() {
        let mut u = Updater::default();
        let t = u.begin_check(Channel::Beta, 0).unwrap();
        assert!(
            !u.finish_check(
                t,
                Ok(Found::Coming("0.4.1-beta.2".parse().unwrap())),
                true,
                true,
                0
            ),
            "nothing to download"
        );
        assert_eq!(
            (u.status.clone(), u.offer.is_none()),
            (
                UpdateStatus::Coming {
                    version: "0.4.1-beta.2".into()
                },
                true
            )
        );
        assert!(!u.check_due(true, QUARTER - 1));
        assert!(
            u.check_due(true, QUARTER),
            "the build follows within minutes"
        );
        assert!(
            u.pending_changes(Channel::Beta, None).is_none(),
            "no notes for what can't be installed yet"
        );
        for (windows, line) in [
            (
                true,
                "Trek 0.4.1-beta.2 is out; the Windows build is on its way. Check again in a while.",
            ),
            (
                false,
                "Trek 0.4.1-beta.2 is out; the Mac build is on its way. Check again in a while.",
            ),
        ] {
            let _platform = trek_words::pretend(windows);
            let v = describe_update(&u.status, true, None, Channel::Stable);
            assert_eq!(
                (v.line.as_str(), v.action, v.busy),
                (line, Some(UpdateAction::Check), false)
            );
        }
        // And once it's there, it's offered as usual.
        let t = u.begin_check(Channel::Beta, HOUR).unwrap();
        assert!(u.finish_check(
            t,
            Ok(Found::Update(release("0.4.1-beta.2"))),
            false,
            true,
            HOUR
        ));
        assert_eq!(
            u.status,
            UpdateStatus::Available {
                version: "0.4.1-beta.2".into()
            }
        );
    }

    #[test]
    fn every_channel_is_looked_at_every_quarter_hour() {
        // Stable releases come out several times on some days: a daily check left Trek behind
        // until someone checked by hand.
        for channel in [Channel::Stable, Channel::Beta, Channel::Nightly] {
            let mut u = Updater::default();
            let t = u.begin_check(channel, 0).unwrap();
            assert!(!u.finish_check(t, Ok(Found::Nothing), false, true, 0));
            assert!(!u.check_due(true, EVERY - 1), "{channel:?}");
            assert!(u.check_due(true, EVERY), "{channel:?}");
        }
    }

    #[test]
    fn checks_run_hourly_by_the_clock_and_retry_within_the_hour() {
        let mut u = Updater::default();
        assert!(u.check_due(true, 0), "never checked");
        assert!(!u.check_due(false, 0), "automatic checks off");
        let t = u.begin_check(Channel::Stable, 0).unwrap();
        assert!(u.begin_check(Channel::Stable, 0).is_none(), "one at a time");
        assert!(!u.check_due(true, 2 * HOUR), "not while one runs");
        assert!(!u.finish_check(t, Ok(Found::Nothing), false, true, 0));
        assert_eq!(u.status, UpdateStatus::UpToDate);
        assert!(!u.check_due(true, EVERY - 1));
        assert!(u.check_due(true, EVERY));

        // A background check that fails stays quiet and retries in an hour.
        let t = u.begin_check(Channel::Stable, HOUR).unwrap();
        u.finish_check(t, Err("offline".into()), false, true, HOUR);
        assert_eq!(u.status, UpdateStatus::Idle);
        assert!(u.check_due(true, 2 * HOUR));
        let t = u.begin_check(Channel::Stable, 2 * HOUR).unwrap();
        u.finish_check(t, Err("offline".into()), true, true, 2 * HOUR);
        assert!(
            matches!(&u.status, UpdateStatus::Failed(e) if e.contains("offline")),
            "the user asked: say why"
        );
    }

    #[test]
    fn coming_to_the_front_looks_again_once_the_last_check_is_minutes_old() {
        let mut u = Updater::default();
        let t = u.begin_check(Channel::Stable, 0).unwrap();
        assert!(!u.finish_check(t, Ok(Found::Nothing), false, true, 0));
        // Just checked: coming to the front changes nothing.
        u.check_on_activate(ON_ACTIVATE_AFTER_MS - 1);
        assert!(!u.check_due(true, ON_ACTIVATE_AFTER_MS - 1));
        // Minutes later: due now, not at the quarter hour.
        u.check_on_activate(ON_ACTIVATE_AFTER_MS);
        assert!(u.check_due(true, ON_ACTIVATE_AFTER_MS));
        // Automatic checks off: still nothing.
        assert!(!u.check_due(false, ON_ACTIVATE_AFTER_MS));
    }

    #[test]
    fn offers_not_downloaded_are_checked_again() {
        let mut u = Updater::default();
        let t = u.begin_check(Channel::Beta, 0).unwrap();
        assert!(
            !u.finish_check(
                t,
                Ok(Found::Update(release("0.3.0-beta.1"))),
                false,
                false,
                0
            ),
            "automatic downloads off"
        );
        assert!(!u.check_due(true, EVERY - 1));
        assert!(u.check_due(true, EVERY));
        // A newer build replaces the offer.
        let t = u.begin_check(Channel::Beta, HOUR).unwrap();
        u.finish_check(
            t,
            Ok(Found::Update(release("0.3.0-beta.2"))),
            false,
            false,
            HOUR,
        );
        assert_eq!(
            u.status,
            UpdateStatus::Available {
                version: "0.3.0-beta.2".into()
            }
        );
        // Offline: the offer stays up.
        let t = u.begin_check(Channel::Beta, 48 * HOUR).unwrap();
        u.finish_check(t, Err("offline".into()), false, false, 48 * HOUR);
        assert_eq!(
            u.status,
            UpdateStatus::Available {
                version: "0.3.0-beta.2".into()
            }
        );
        assert_eq!(
            u.begin_download().unwrap().update.version.to_string(),
            "0.3.0-beta.2"
        );
    }

    #[test]
    fn a_download_that_fails_offline_is_retried_and_a_bad_release_waits_a_day() {
        let mut u = Updater::default();
        let t = u.begin_check(Channel::Stable, 0).unwrap();
        u.finish_check(t, Ok(Found::Update(release("0.2.1"))), false, false, 0);
        let job = u.begin_download().unwrap();
        assert!(u.begin_download().is_none(), "already downloading");
        assert!(u.download_progress(job.ticket, 0.5));
        u.finish_download(
            job.ticket,
            "0.2.1".into(),
            Err(("couldn't connect to github.com".into(), true)),
            0,
        );
        assert!(
            matches!(&u.status, UpdateStatus::Failed(e) if e.starts_with("Couldn't update to Trek 0.2.1"))
        );
        assert!(u.check_due(true, HOUR));

        let t = u.begin_check(Channel::Stable, HOUR).unwrap();
        u.finish_check(t, Ok(Found::Update(release("0.2.1"))), false, false, HOUR);
        let job = u.begin_download().unwrap();
        u.finish_download(
            job.ticket,
            "0.2.1".into(),
            Err(("the update's signature doesn't match".into(), false)),
            HOUR,
        );
        assert!(!u.check_due(true, 2 * HOUR));
        assert!(u.check_due(true, 25 * HOUR));
    }

    #[test]
    fn a_staged_update_is_dropped_when_the_channel_changes() {
        let mut u = Updater::default();
        let path = staged(&mut u, Channel::Nightly, "0.3.0-nightly.20261002", 0);
        assert!(!u.channel_changed(Channel::Nightly));
        assert!(u.channel_changed(Channel::Stable));
        assert_eq!(
            u.drop_update(),
            Some(path),
            "the staged nightly is deleted, never installed on quit"
        );
        assert_eq!(
            (u.status.clone(), u.offer.is_none()),
            (UpdateStatus::Idle, true)
        );
        assert!(u.begin_check(Channel::Stable, 0).is_some());
    }

    #[test]
    fn a_staged_update_is_checked_past_and_a_newer_release_replaces_it() {
        let mut u = Updater::default();
        let path = staged(&mut u, Channel::Stable, "0.2.1", 0);
        assert!(!u.check_due(true, EVERY - 1));
        assert!(
            u.check_due(true, HOUR),
            "Trek can stay open for days with an update waiting"
        );
        let ready = u.status.clone();

        // Nothing newer, the same release, one on its way, a failure: the staged one stays, and
        // the check never shows (it's still the update to restart into).
        for (result, user, next) in [
            (Ok(Found::Nothing), false, EVERY),
            (Ok(Found::Update(release("0.2.1"))), false, EVERY),
            (Ok(Found::Coming("0.2.2".parse().unwrap())), false, QUARTER),
            (Err("offline".to_string()), true, HOUR),
        ] {
            let t = u.begin_check(Channel::Stable, HOUR).unwrap();
            assert_eq!(u.status, ready, "no spinner over Restart to update");
            assert!(
                u.begin_check(Channel::Stable, HOUR).is_none(),
                "one at a time"
            );
            assert!(!u.check_due(true, 10 * HOUR), "not while one runs");
            assert!(!u.finish_check(t, result, user, true, HOUR));
            assert_eq!(u.status, ready);
            assert_eq!(u.take_replaced(), None);
            assert!(!u.check_due(true, HOUR + next - 1));
            assert!(u.check_due(true, HOUR + next));
        }

        // A newer one: offered (and downloaded) in its place; the staged copy is handed back once.
        let t = u.begin_check(Channel::Stable, 2 * HOUR).unwrap();
        assert!(u.finish_check(
            t,
            Ok(Found::Update(release("0.2.2"))),
            false,
            true,
            2 * HOUR
        ));
        assert_eq!(
            u.status,
            UpdateStatus::Available {
                version: "0.2.2".into()
            }
        );
        assert_eq!(u.take_replaced(), Some(path));
        assert_eq!(u.take_replaced(), None);
        assert_eq!(
            u.begin_download().unwrap().update.version.to_string(),
            "0.2.2"
        );
    }

    #[test]
    fn a_restart_asked_for_isnt_overtaken_by_a_check() {
        let mut u = Updater::default();
        let path = staged(&mut u, Channel::Stable, "0.2.1", 0);
        // The check started before Restart was clicked with agents still working.
        let t = u.begin_check(Channel::Stable, HOUR).unwrap();
        u.status = UpdateStatus::RestartPending {
            version: "0.2.1".into(),
            staged: path.clone(),
        };
        assert!(!u.finish_check(t, Ok(Found::Update(release("0.2.2"))), false, true, HOUR));
        assert_eq!(
            (u.status.clone(), u.take_replaced()),
            (
                UpdateStatus::RestartPending {
                    version: "0.2.1".into(),
                    staged: path
                },
                None
            )
        );
        assert!(!u.check_due(true, 10 * HOUR));
        assert!(u.begin_check(Channel::Stable, 10 * HOUR).is_none());
    }

    #[test]
    fn switching_channels_keeps_what_the_new_channel_offers_too() {
        // A stable release is on every channel: switching from Stable keeps it staged, and the
        // new channel is asked at once for something newer of its own.
        let mut u = Updater::default();
        let path = staged(&mut u, Channel::Stable, "0.4.16", 0);
        assert_eq!(u.switch_channel(Channel::Beta), None);
        assert!(matches!(u.status, UpdateStatus::Ready { .. }));
        assert!(!u.channel_changed(Channel::Beta));
        assert!(u.check_due(true, 1), "asked at once");
        let t = u.begin_check(Channel::Beta, 1).unwrap();
        assert!(u.finish_check(
            t,
            Ok(Found::Update(release("0.4.17-beta.1"))),
            false,
            true,
            1
        ));
        assert_eq!(u.take_replaced(), Some(path));

        // A beta or a nightly isn't on Stable: dropped, never installed on quit.
        for (from, v, to) in [
            (Channel::Beta, "0.4.17-beta.1", Channel::Stable),
            (Channel::Nightly, "0.4.17-nightly.20261027", Channel::Beta),
            (Channel::Nightly, "0.4.17-nightly.20261027", Channel::Stable),
        ] {
            let mut u = Updater::default();
            let path = staged(&mut u, from, v, 0);
            assert_eq!(u.switch_channel(to), Some(path), "{v} on {to:?}");
            assert_eq!(
                (u.status.clone(), u.offer.is_none()),
                (UpdateStatus::Idle, true)
            );
        }
        // A beta stays on the way to Nightly.
        let mut u = Updater::default();
        staged(&mut u, Channel::Beta, "0.4.17-beta.1", 0);
        assert_eq!(u.switch_channel(Channel::Nightly), None);
        assert!(matches!(u.status, UpdateStatus::Ready { .. }));
    }

    #[test]
    fn a_check_of_the_old_channel_doesnt_count_after_a_switch_that_kept_the_update() {
        let mut u = Updater::default();
        staged(&mut u, Channel::Stable, "0.4.16", 0);
        assert_eq!(u.switch_channel(Channel::Nightly), None);
        let old = u.begin_check(Channel::Nightly, HOUR).unwrap();
        // Back to Stable while Nightly is being asked: the stable release stays, the nightly the
        // check finds isn't offered, and Stable is asked afresh.
        assert_eq!(u.switch_channel(Channel::Stable), None);
        assert!(!u.finish_check(
            old,
            Ok(Found::Update(release("0.4.17-nightly.20261027"))),
            false,
            true,
            HOUR
        ));
        assert!(matches!(&u.status, UpdateStatus::Ready { version, .. } if version == "0.4.16"));
        assert_eq!(u.take_replaced(), None);
        assert!(u.check_due(true, HOUR));
        assert!(u.begin_check(Channel::Stable, HOUR).is_some());

        // A download the new channel wants too carries on.
        let mut u = Updater::default();
        let t = u.begin_check(Channel::Stable, 0).unwrap();
        assert!(u.finish_check(t, Ok(Found::Update(release("0.4.16"))), false, true, 0));
        let job = u.begin_download().unwrap();
        assert_eq!(u.switch_channel(Channel::Beta), None);
        assert!(!job.cancel.load(Ordering::Relaxed));
        assert!(u.download_progress(job.ticket, 0.5));
        let path = PathBuf::from("/tmp/download-1-0.4.16.noindex/Trek.app");
        assert_eq!(
            u.finish_download(job.ticket, "0.4.16".into(), Ok(path), 0),
            None
        );
        assert!(matches!(u.status, UpdateStatus::Ready { .. }));
        assert!(u.check_due(true, 0), "then Beta is asked");
    }

    #[test]
    fn the_updates_page_says_when_the_channel_has_nothing_newer_of_its_own() {
        let line =
            |status: UpdateStatus, channel| describe_update(&status, true, None, channel).line;
        let available = |v: &str| UpdateStatus::Available { version: v.into() };
        assert_eq!(
            line(available("0.4.16"), Channel::Stable),
            "Trek 0.4.16 is available."
        );
        assert_eq!(
            line(available("0.4.16"), Channel::Beta),
            "Trek 0.4.16 is available. It's a stable release: there's no newer beta."
        );
        assert_eq!(
            line(available("0.4.16"), Channel::Nightly),
            "Trek 0.4.16 is available. It's a stable release: there's no newer nightly."
        );
        assert_eq!(
            line(available("0.4.17-beta.1"), Channel::Nightly),
            "Trek 0.4.17-beta.1 is available. It's a beta: there's no newer nightly."
        );
        for (v, channel) in [
            ("0.4.17-beta.1", Channel::Beta),
            ("0.4.17-nightly.20261027", Channel::Nightly),
        ] {
            assert_eq!(
                line(available(v), channel),
                format!("Trek {v} is available."),
                "the channel's own"
            );
        }
        let current = trek_core::VERSION;
        assert_eq!(
            line(UpdateStatus::UpToDate, Channel::Stable),
            format!("Trek {current} is up to date.")
        );
        assert_eq!(
            line(UpdateStatus::UpToDate, Channel::Beta),
            format!("Trek {current} is up to date. There's no newer beta or stable release.")
        );
        assert_eq!(
            line(UpdateStatus::UpToDate, Channel::Nightly),
            format!(
                "Trek {current} is up to date. There's no newer nightly, beta or stable release."
            )
        );
        assert_eq!(
            line(UpdateStatus::Idle, Channel::Stable),
            "Trek checks for updates every 15 minutes."
        );
    }

    #[test]
    fn work_from_the_old_channel_doesnt_count_after_a_switch() {
        // A check still running when the channel changes.
        let mut u = Updater::default();
        let old = u.begin_check(Channel::Beta, 0).unwrap();
        u.drop_update();
        let new = u.begin_check(Channel::Stable, 0).unwrap();
        assert!(
            !u.finish_check(
                old,
                Ok(Found::Update(release("0.3.0-beta.1"))),
                false,
                true,
                0
            ),
            "the beta isn't downloaded"
        );
        assert_eq!(u.status, UpdateStatus::Checking);
        u.finish_check(new, Ok(Found::Nothing), false, true, 0);
        assert_eq!(u.status, UpdateStatus::UpToDate);

        // A download still running: stopped, and its staged copy handed back for deletion.
        let t = u.begin_check(Channel::Beta, 0).unwrap();
        u.finish_check(
            t,
            Ok(Found::Update(release("0.3.0-beta.1"))),
            false,
            true,
            0,
        );
        let job = u.begin_download().unwrap();
        assert_eq!(u.drop_update(), None);
        assert!(job.cancel.load(Ordering::Relaxed));
        assert!(!u.download_progress(job.ticket, 0.9));
        let late = PathBuf::from("/tmp/download-1-0.noindex/Trek.app");
        assert_eq!(
            u.finish_download(job.ticket, "0.3.0-beta.1".into(), Ok(late.clone()), 0),
            Some(late)
        );
        assert_eq!(u.status, UpdateStatus::Idle);
    }

    #[test]
    fn an_update_shows_every_release_it_brings() {
        let current = trek_core::update::current_version();
        let next = |minor: u64| semver::Version::new(current.major, current.minor + minor, 0);
        let published = |v: semver::Version| Release {
            notes: format!("- in {v}"),
            version: v,
            date: String::new(),
            url: String::new(),
            prerelease: false,
        };
        let mut u = Updater::default();
        assert!(
            u.pending_changes(Channel::Stable, Some("dokyit/Trek"))
                .is_none(),
            "nothing on offer"
        );
        staged(&mut u, Channel::Stable, &next(2).to_string(), 0);
        // Before the release list is read: the offer's own notes.
        let changes = u
            .pending_changes(Channel::Stable, Some("dokyit/Trek"))
            .unwrap();
        assert_eq!(
            changes
                .releases
                .iter()
                .map(|r| r.notes.as_str())
                .collect::<Vec<_>>(),
            ["- faster"]
        );
        // After: every release since this one, newest first; none from before or beyond.
        u.changelog = vec![
            published(next(3)),
            published(next(2)),
            published(next(1)),
            published(current.clone()),
        ];
        let changes = u
            .pending_changes(Channel::Stable, Some("dokyit/Trek"))
            .unwrap();
        assert_eq!(
            changes
                .releases
                .iter()
                .map(|r| r.version.clone())
                .collect::<Vec<_>>(),
            [next(2), next(1)]
        );
        assert_eq!(
            changes.compare.as_deref(),
            Some(
                format!(
                    "https://github.com/dokyit/Trek/compare/v{current}...v{}",
                    next(2)
                )
                .as_str()
            )
            .filter(|_| current.pre.is_empty())
        );
        assert_eq!(
            u.installed_release().map(|r| r.version.clone()),
            Some(current)
        );
        u.status = UpdateStatus::UpToDate;
        assert!(
            u.pending_changes(Channel::Stable, None).is_none(),
            "installed or dropped"
        );
    }

    #[test]
    fn release_notes_show_while_an_update_is_on_offer() {
        let mut u = Updater::default();
        staged(&mut u, Channel::Stable, "0.2.1", 0);
        let notes = |u: &Updater| {
            u.pending_changes(Channel::Stable, None)
                .map(|c| c.releases[0].notes.clone())
        };
        assert_eq!(notes(&u).as_deref(), Some("- faster"));
        u.status = UpdateStatus::UpToDate;
        assert!(notes(&u).is_none());
    }
}
