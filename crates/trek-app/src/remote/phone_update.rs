//! Trek on the iPhone, kept current from here. The app is signed with a free Apple ID, so it can't
//! update itself: this Mac builds it and installs it (`ios/scripts/phone-update.sh`, every half
//! hour from its LaunchAgent). A phone asks for it now (`update_phone`), and so does Settings ›
//! Phone & Machines ("Update phone now"): the copy of the script built into Trek runs with
//! `--asked`, and the steps it prints ("@building 0.4.28") go to every phone as `phone_update` and
//! to Settings. Phones learn the newest Trek this Mac knows of from the snapshot (`release`).

use crate::workspace::Workspace;
use gpui_kit::Context;
use std::path::PathBuf;
use trek_remote as tr;

/// The phone updater, as this version of Trek has it.
const SCRIPT: &str = include_str!("../../../../ios/scripts/phone-update.sh");
/// The LaunchAgent's own folder (`install-phone-update.sh`): its copy of the script, what it
/// installed and how the last attempt went.
const STATE: &str = "Library/Application Support/dev.trek.phone-update";
const AGENT: &str = "Library/LaunchAgents/dev.trek.phone-update.plist";

/// An update of the iPhone app run from here, and the last attempt's outcome.
#[derive(Default)]
pub struct PhoneUpdates {
    /// A run is going (the step it's at is `now`, once it said one).
    pub running: bool,
    /// The step the last run reached.
    pub now: Option<tr::PhoneUpdate>,
}

/// How the last attempt went, as the phone updater recorded it (`last` in its folder): by the
/// LaunchAgent or from here.
#[derive(Debug, Clone, PartialEq)]
pub struct Attempt {
    /// Unix ms.
    pub at: i64,
    pub ok: bool,
    pub line: String,
}

fn state_dir() -> PathBuf {
    trek_core::paths::home().join(STATE)
}

/// A line the script printed for Trek: "@building 0.4.28", "@failed <why>".
pub fn parse_step(line: &str) -> Option<tr::PhoneUpdate> {
    let (state, rest) = line
        .strip_prefix('@')?
        .split_once(' ')
        .unwrap_or((line.trim_start_matches('@'), ""));
    let rest = rest.trim();
    let state = match state {
        "building" => tr::PhoneUpdateState::Building,
        "installing" => tr::PhoneUpdateState::Installing,
        "installed" => tr::PhoneUpdateState::Installed,
        "failed" => tr::PhoneUpdateState::Failed,
        _ => return None,
    };
    let failed = state == tr::PhoneUpdateState::Failed;
    let text = (!rest.is_empty()).then(|| rest.to_string());
    Some(tr::PhoneUpdate {
        state,
        version: if failed { None } else { text.clone() },
        message: if failed { text } else { None },
    })
}

/// `key=value` lines, as the script writes `last` and `installed`.
fn read_keys(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse_attempt(text: &str) -> Option<Attempt> {
    Some(Attempt {
        at: read_keys(text, "at")?.parse::<i64>().ok()? * 1000,
        ok: read_keys(text, "result")? == "ok",
        line: read_keys(text, "line")?,
    })
}

/// The last attempt, read again only when the file changed (Settings asks on every frame).
pub fn last_attempt() -> Option<Attempt> {
    static CACHE: std::sync::Mutex<Option<(std::time::SystemTime, Option<Attempt>)>> =
        std::sync::Mutex::new(None);
    let path = state_dir().join("last");
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, attempt)) = cache.as_ref().filter(|(at, _)| *at == modified) {
        return attempt.clone();
    }
    let attempt = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| parse_attempt(&t));
    *cache = Some((modified, attempt.clone()));
    attempt
}

/// The version the updater last installed on the iPhone, for when no phone has said its own.
pub fn installed_version() -> Option<String> {
    read_keys(
        &std::fs::read_to_string(state_dir().join("installed")).ok()?,
        "version",
    )
}

/// The newest Trek this Mac knows of, for phones to offer: a stable release its updater found and
/// hasn't installed yet (the iPhone needn't wait for Trek here to quit), else its own version, with
/// the first line of the release's notes when Trek has read them. Pre-releases aren't offered: the
/// iPhone app is built at a release's tag.
pub fn release(updater: &crate::updater::Updater) -> tr::TrekRelease {
    use crate::updater::UpdateStatus;
    let current = trek_core::update::current_version();
    let waiting = matches!(
        updater.status,
        UpdateStatus::Available { .. }
            | UpdateStatus::Downloading { .. }
            | UpdateStatus::Ready { .. }
            | UpdateStatus::RestartPending { .. }
    );
    match updater
        .offer
        .as_ref()
        .filter(|o| waiting && o.version > current && o.version.pre.is_empty())
    {
        Some(o) => tr::TrekRelease {
            version: o.version.to_string(),
            note: tr::TrekRelease::first_note(&o.notes),
        },
        None => tr::TrekRelease {
            version: current.to_string(),
            note: updater
                .installed_release()
                .and_then(|r| tr::TrekRelease::first_note(&r.notes)),
        },
    }
}

/// A release asked for (`update_phone`'s `version`) that's worth building: newer than this Trek,
/// and a release (it has a tag to build at).
fn wanted(version: Option<String>) -> Option<String> {
    let v = semver::Version::parse(version?.trim().trim_start_matches('v')).ok()?;
    (v > trek_core::update::current_version() && v.pre.is_empty()).then(|| v.to_string())
}

/// The repository the LaunchAgent builds from (`TREK_REPO` in its plist), if it's installed.
fn agent_repo() -> Option<String> {
    let out = std::process::Command::new("/usr/bin/plutil")
        .args([
            "-extract",
            "EnvironmentVariables.TREK_REPO",
            "raw",
            "-o",
            "-",
        ])
        .arg(trek_core::paths::home().join(AGENT))
        .output()
        .ok()?;
    let repo = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !repo.is_empty()).then_some(repo)
}

/// The LaunchAgent runs its own copy of the script, from when it was installed: a published Trek
/// keeps it at its own version, so a new step (this Mac's own update while Trek is closed) reaches
/// it with the update. A development build leaves it be.
pub fn refresh_agent_copy() {
    if trek_core::update::blocker() == Some(trek_core::update::Blocker::DevBuild) {
        return;
    }
    let copy = state_dir().join("phone-update.sh");
    match std::fs::read_to_string(&copy) {
        Ok(text) if text != SCRIPT => {
            if let Err(e) = std::fs::write(&copy, SCRIPT) {
                tracing::warn!("phone updater: couldn't refresh {}: {e}", copy.display());
            } else {
                tracing::info!("phone updater: its copy is now this Trek's");
            }
        }
        _ => {}
    }
}

/// Run the script for someone who asked, sending each line it prints to `lines`. Blocking.
fn run(target: Option<String>, lines: async_channel::Sender<String>) -> std::io::Result<()> {
    use std::io::BufRead as _;
    let dir = trek_core::paths::data_dir().join("mobile");
    std::fs::create_dir_all(&dir)?;
    let script = dir.join("phone-update.sh");
    std::fs::write(&script, SCRIPT)?;
    let mut cmd = trek_core::procs::command("/bin/zsh");
    cmd.arg(&script)
        .arg("--asked")
        .env("TREK_PHONE_UPDATE_EVENTS", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    if let Some(repo) = agent_repo() {
        cmd.env("TREK_REPO", repo);
    }
    if let Some(target) = target {
        cmd.env("TREK_PHONE_TARGET", target);
    }
    let mut child = cmd.spawn()?;
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            let _ = lines.send_blocking(line);
        }
    }
    child.wait()?;
    Ok(())
}

impl Workspace {
    /// Build Trek for the iPhone and install it (a phone asked, or Settings): `Ok` once it has
    /// started, or when a run is going already. `version` is the release the phone was offered.
    pub fn update_phone(
        &mut self,
        version: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !cfg!(target_os = "macos") {
            let words = crate::words::words();
            return Err(format!(
                "Trek on {} can't build the iPhone app: that takes Xcode",
                words.this_computer
            ));
        }
        if self.phone_update.running {
            return Ok(());
        }
        let (tx, rx) = async_channel::unbounded::<String>();
        let target = wanted(version);
        std::thread::Builder::new()
            .name("trek-phone-update".into())
            .spawn(move || {
                if let Err(e) = run(target, tx.clone()) {
                    let _ = tx.send_blocking(format!("@failed Couldn't start the update: {e}"));
                }
            })
            .map_err(|e| format!("Couldn't start the update: {e}"))?;
        self.phone_update = PhoneUpdates {
            running: true,
            now: None,
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            while let Ok(line) = rx.recv().await {
                if let Some(step) = parse_step(&line) {
                    let _ = this.update(cx, |ws, cx| ws.phone_update_step(step, cx));
                }
            }
            let _ = this.update(cx, |ws, cx| {
                ws.phone_update.running = false;
                let ended = ws.phone_update.now.as_ref().is_some_and(|s| {
                    matches!(
                        s.state,
                        tr::PhoneUpdateState::Installed | tr::PhoneUpdateState::Failed
                    )
                });
                if !ended {
                    ws.phone_update_step(
                        tr::PhoneUpdate {
                            state: tr::PhoneUpdateState::Failed,
                            version: None,
                            message: Some(
                                "The update stopped before it finished; its log is ~/Library/Logs/trek-phone-update.log".into(),
                            ),
                        },
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
        Ok(())
    }

    fn phone_update_step(&mut self, step: tr::PhoneUpdate, cx: &mut Context<Self>) {
        tracing::info!(
            "phone update: {:?} {:?}",
            step.state,
            step.version.as_deref().or(step.message.as_deref())
        );
        if let Some(remote) = self.remote.as_ref() {
            remote.handle.push(tr::HostEvent::PhoneUpdate(step.clone()));
        }
        self.phone_update.now = Some(step);
        cx.notify();
    }
}

/// The iPhone's Trek: the version a paired iPhone last said, and its name.
pub fn phone_version(devices: &[tr::DeviceInfo]) -> Option<(String, Option<String>)> {
    devices
        .iter()
        .filter(|d| d.platform.as_deref() == Some("ios"))
        .filter_map(|d| {
            Some((
                d.app_version.clone()?,
                d.last_seen_at.unwrap_or(d.paired_at),
                &d.name,
            ))
        })
        .max_by_key(|(_, seen, _)| *seen)
        .map(|(v, _, name)| {
            (
                v.split(' ').next().unwrap_or(&v).to_string(),
                Some(name.clone()),
            )
        })
        .or_else(|| installed_version().map(|v| (v, None)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scripts_steps_become_phone_updates() {
        assert_eq!(
            parse_step("@building 0.4.28"),
            Some(tr::PhoneUpdate {
                state: tr::PhoneUpdateState::Building,
                version: Some("0.4.28".into()),
                message: None
            })
        );
        assert_eq!(
            parse_step("@installed 0.4.28").map(|s| s.state),
            Some(tr::PhoneUpdateState::Installed)
        );
        assert_eq!(
            parse_step(
                "@failed Your Mac couldn't reach the iPhone: plug it in or join the same Wi-Fi"
            ),
            Some(tr::PhoneUpdate {
                state: tr::PhoneUpdateState::Failed,
                version: None,
                message: Some(
                    "Your Mac couldn't reach the iPhone: plug it in or join the same Wi-Fi".into()
                ),
            })
        );
        assert_eq!(parse_step("2026-10-30 12:51:10 up to date"), None);
        assert_eq!(parse_step("@later 0.4.28"), None);
    }

    #[test]
    fn the_last_attempt_is_read_from_the_updaters_record() {
        assert_eq!(
            parse_attempt(
                "at=1793390000\nresult=ok\nline=Installed Trek 0.4.27 on the iPhone (Toby's iPhone)\n"
            ),
            Some(Attempt {
                at: 1_793_390_000_000,
                ok: true,
                line: "Installed Trek 0.4.27 on the iPhone (Toby's iPhone)".into()
            })
        );
        assert_eq!(
            parse_attempt("at=1\nresult=failed\nline=The iPhone didn't answer").map(|a| a.ok),
            Some(false)
        );
        assert_eq!(parse_attempt("result=ok\n"), None, "no time");
    }

    #[test]
    fn only_a_newer_release_is_built_for_the_phone() {
        assert_eq!(wanted(Some("99.0.0".into())), Some("99.0.0".into()));
        assert_eq!(wanted(Some("v99.0.1".into())), Some("99.0.1".into()));
        assert_eq!(wanted(Some("99.0.0-nightly.1".into())), None, "no tag");
        assert_eq!(wanted(Some("0.0.1".into())), None, "older: the Mac's own");
        assert_eq!(wanted(Some("soon".into())), None);
        assert_eq!(wanted(None), None);
    }

    #[test]
    fn the_phones_version_is_the_one_it_last_said() {
        let device =
            |name: &str, platform: &str, version: Option<&str>, seen: i64| tr::DeviceInfo {
                device_id: name.into(),
                name: name.into(),
                paired_at: 0,
                last_seen_at: Some(seen),
                app_version: version.map(str::to_string),
                platform: Some(platform.into()),
            };
        let devices = [
            device("Old iPhone", "ios", Some("0.4.20 (800)"), 1),
            device("Toby's iPhone", "ios", Some("0.4.26"), 5),
            device("Studio", "macos", Some("0.4.27"), 9),
        ];
        assert_eq!(
            phone_version(&devices),
            Some(("0.4.26".into(), Some("Toby's iPhone".into())))
        );
    }
}
