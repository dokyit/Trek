//! Updating the installed Trek while it's closed (macOS): `trek-mcp mac-update`, run by the phone
//! updater's LaunchAgent (`ios/scripts/phone-update.sh`) every half hour. Trek updates itself only
//! while it runs (it downloads, and installs on quit or "Restart to update"), so a Mac where Trek
//! stayed closed kept an old version all day while a new one was out, and the iPhone, built at the
//! Mac's version, with it.
//!
//! The same path as Trek's own updater, for the app at a given place rather than the one running:
//! the channel and feed from Trek's settings (`check_against`), the download checked against the
//! release key this build ships (`download`), the bundle checked and staged (`stage_for`), then
//! swapped in with the old one kept as the single backup (`install_over`). Never while Trek is
//! open: its own updater has that release in hand, and swapping the bundle under a running Trek
//! would leave it on files of another version.

use super::{
    AvailableUpdate, Found, LOCAL_UPDATES_ENV, blocker_for, bundle_version, check_against,
    download, install_over, is_release, manifest_urls, plist_value, stage_for,
};
use anyhow::{Context as _, Result};
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// What to do about the installed app, by what a check found and whether Trek is open.
#[derive(Debug, Clone)]
pub enum Plan {
    /// Trek is closed and a newer release is out for this Mac: install it.
    Install(AvailableUpdate),
    /// A newer release is out, but Trek is open: its own updater installs it (on quit, or when the
    /// user restarts it).
    LeaveToTrek(semver::Version),
    /// The release is out without this Mac's build yet.
    Coming(semver::Version),
    UpToDate,
}

pub fn plan(found: Found, trek_running: bool) -> Plan {
    match found {
        Found::Update(u) if trek_running => Plan::LeaveToTrek(u.version),
        Found::Update(u) => Plan::Install(u),
        Found::Coming(v) => Plan::Coming(v),
        Found::Nothing => Plan::UpToDate,
    }
}

/// Whether Trek from `app` is running: its own executable, not the `trek-mcp` or helpers an agent
/// may have started from the bundle (they run on while it's swapped, from the files they opened).
pub fn trek_running(app: &Path) -> bool {
    let exe = plist_value(app, "CFBundleExecutable").unwrap_or_else(|_| "trek".into());
    let exe = app.join("Contents/MacOS").join(exe);
    let Ok(out) = std::process::Command::new("/bin/ps")
        .args(["-axo", "comm="])
        .output()
    else {
        // Can't tell: take it as running, which only waits.
        return true;
    };
    running_in(&String::from_utf8_lossy(&out.stdout), &exe)
}

/// `ps -o comm` lists each process by its executable's full path.
fn running_in(ps: &str, exe: &Path) -> bool {
    ps.lines().any(|l| Path::new(l.trim()) == exe)
}

/// Check for a newer Trek than the one at `app` and, with `apply` and Trek closed, install it.
/// Returns what happened, in a line for the log ("Installed Trek 0.4.27 (was 0.4.26)"); an error
/// is a line too.
pub async fn update(app: &Path, apply: bool) -> Result<String> {
    let installed =
        bundle_version(app).with_context(|| format!("there's no Trek at {}", app.display()))?;
    let release = std::env::var(LOCAL_UPDATES_ENV).is_ok_and(|v| v == "1") || is_release(app);
    if let Some(blocker) = blocker_for(app, release) {
        return Ok(format!(
            "Trek {installed} can't update itself: {}",
            blocker.message()
        ));
    }
    let settings = crate::settings::Settings::load().updates;
    if !settings.auto_check {
        return Ok(format!(
            "Trek {installed}: automatic updates are off in its settings"
        ));
    }
    let found = check_against(&manifest_urls(&settings), settings.channel, &installed)
        .await
        .context("couldn't check for updates")?;
    let update = match plan(found, trek_running(app)) {
        Plan::UpToDate => return Ok(format!("Trek {installed} is up to date")),
        Plan::Coming(v) => {
            return Ok(format!(
                "Trek {v} is out; this Mac's build is on its way (Trek {installed} here)"
            ));
        }
        Plan::LeaveToTrek(v) => {
            return Ok(format!(
                "Trek {v} is out and Trek {installed} is open: it installs the update itself"
            ));
        }
        Plan::Install(u) if !apply => {
            return Ok(format!(
                "would install Trek {} (Trek {installed} is closed)",
                u.version
            ));
        }
        Plan::Install(u) => u,
    };
    let version = update.version.clone();
    let cancel = AtomicBool::new(false);
    let archive = download(&update, &cancel, |_| {})
        .await
        .with_context(|| format!("couldn't download Trek {version}"))?;
    // Blocking (tar, codesign), on the one task a command-line run has.
    let staged = stage_for(
        &archive,
        &version,
        Some(app.to_path_buf()),
        &installed,
        &cancel,
    )
    .with_context(|| format!("Trek {version} didn't check out"))?;
    // Opened while it downloaded: its own updater takes it from here.
    if trek_running(app) {
        super::discard(&staged);
        return Ok(format!(
            "Trek {installed} opened while Trek {version} downloaded: it installs the update itself"
        ));
    }
    install_over(
        &staged,
        app,
        &crate::paths::updates_dir(),
        &installed.to_string(),
    )
    .with_context(|| format!("couldn't install Trek {version}"))?;
    Ok(format!("Installed Trek {version} (was {installed})"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::Artifact;

    fn release(v: &str) -> AvailableUpdate {
        AvailableUpdate {
            version: v.parse().unwrap(),
            notes: String::new(),
            pub_date: String::new(),
            artifact: Artifact {
                url: "u".into(),
                sha256: "s".into(),
                signature: String::new(),
            },
        }
    }

    #[test]
    fn a_newer_release_installs_only_while_trek_is_closed() {
        let closed = plan(Found::Update(release("0.4.27")), false);
        assert!(
            matches!(&closed, Plan::Install(u) if u.version.to_string() == "0.4.27"),
            "{closed:?}"
        );
        let open = plan(Found::Update(release("0.4.27")), true);
        assert!(
            matches!(&open, Plan::LeaveToTrek(v) if v.to_string() == "0.4.27"),
            "Trek's own updater has it: {open:?}"
        );
        assert!(matches!(plan(Found::Nothing, false), Plan::UpToDate));
        assert!(matches!(
            plan(Found::Coming("0.4.27".parse().unwrap()), false),
            Plan::Coming(_)
        ));
    }

    #[test]
    fn trek_is_running_when_its_own_executable_is() {
        let exe = Path::new("/Applications/Trek.app/Contents/MacOS/trek");
        let ps = "/sbin/launchd\n/Applications/Trek.app/Contents/MacOS/trek-mcp\n";
        assert!(!running_in(ps, exe), "an agent's trek-mcp isn't Trek");
        assert!(running_in(
            &format!("{ps}/Applications/Trek.app/Contents/MacOS/trek\n"),
            exe
        ));
        assert!(!running_in(
            "/Users/me/Trek/target/debug/trek\n/Applications/Other.app/Contents/MacOS/trek\n",
            exe
        ));
    }

    /// The install step against a bundle in a temporary folder, standing in for /Applications.
    #[test]
    fn a_closed_trek_is_swapped_and_its_next_launch_knows_from_what() {
        let dir = std::env::temp_dir().join(format!("trek-closed-{}", uuid::Uuid::new_v4()));
        let (bundle, updates) = (dir.join("Applications/Trek.app"), dir.join("updates"));
        let write_app = |app: &Path, version: &str| {
            std::fs::create_dir_all(app.join("Contents")).unwrap();
            std::fs::write(
                app.join("Contents/Info.plist"),
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>dev.trek.Trek</string><key>CFBundleVersion</key><string>{version}</string></dict></plist>"#
                ),
            )
            .unwrap();
        };
        write_app(&bundle, "0.4.26");
        let staged = super::super::new_download_dir(&updates)
            .unwrap()
            .join("Trek.app");
        write_app(&staged, "0.4.27");
        let installed = bundle_version(&bundle).unwrap().to_string();
        install_over(&staged, &bundle, &updates, &installed).unwrap();
        assert_eq!(bundle_version(&bundle).unwrap().to_string(), "0.4.27");
        assert_eq!(
            bundle_version(&updates.join("previous.noindex/Trek.app"))
                .unwrap()
                .to_string(),
            "0.4.26",
            "the one backup"
        );
        // The installed version, not this program's: `trek-mcp` may be another build.
        assert_eq!(
            std::fs::read_to_string(updates.join("updated-from")).unwrap(),
            "0.4.26"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real process, from an executable in a bundle of its own.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_running_trek_is_found_by_its_bundle() {
        let dir = std::env::temp_dir().join(format!("trek-closed-{}", uuid::Uuid::new_v4()));
        let app = dir.join("Trek.app");
        let macos = app.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleExecutable</key><string>trek</string></dict></plist>"#,
        )
        .unwrap();
        std::fs::copy("/bin/sleep", macos.join("trek")).unwrap();
        assert!(!trek_running(&app));
        let mut child = std::process::Command::new(macos.join("trek"))
            .arg("30")
            .spawn()
            .unwrap();
        let seen = (0..50).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            trek_running(&app)
        });
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(seen, "Trek running from {} was missed", app.display());
    }
}
