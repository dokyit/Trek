//! Linux: the AppImage updates itself; the other ways Trek is installed there don't.
//!
//! The artifact (`linux-x86_64`, `linux-aarch64`) is the release's AppImage itself, signed like
//! the other platforms' archives. Linux lets a running program's file be replaced (the old one
//! lives on until its last user lets go, and the AppImage's runtime keeps its own mount of it),
//! so `install` puts the new AppImage in the running one's place at once, keeping the old as the
//! single backup, and the relaunch starts it once this Trek has exited; if it can't start, the
//! old one goes back.
//!
//! A `.deb` install (`packaged-by` beside the binary, written by script/bundle-linux.sh) belongs to
//! apt, which updates it with the rest of the system: Trek never writes there. The tarball, or
//! a build cargo left in its target folder, doesn't update itself either; each says so.

use super::{
    Blocker, Installed, LOCAL_UPDATES_ENV, PREVIOUS_DIR, RELAUNCH_KEEPS, SKIP_MARKER,
    UPDATED_MARKER, download_dir_of, is_cargo_output, writable,
};
use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};

/// The file beside the binary that says a package manager installed it (its name inside: `deb`).
pub const PACKAGED_MARKER: &str = "packaged-by";

/// The file a release AppImage carries (`usr/share/trek/release`, script/release-linux.sh): a
/// locally built AppImage (`bundle-linux.sh` alone) leaves it out, as `bundle.sh` leaves the Mac's
/// `TrekRelease` key out.
const RELEASE_MARKER: &str = "../share/trek/release";

/// How this Linux Trek was installed: what decides whether it updates itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// The AppImage at this path (its runtime's `APPIMAGE`), which an update replaces whole.
    AppImage(PathBuf),
    /// A package manager's, which updates it (the `.deb`).
    Packaged,
    /// The folder of anything else: the tarball, or a build cargo left in its target folder.
    Plain(PathBuf),
}

/// How the running Trek was installed.
pub fn install_kind() -> Option<Install> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    Some(kind_of(
        std::env::var_os("APPIMAGE").map(PathBuf::from).as_deref(),
        &dir,
    ))
}

/// `install_kind` for a Trek running from `dir`, its AppImage's runtime saying `appimage`.
fn kind_of(appimage: Option<&Path>, dir: &Path) -> Install {
    match appimage {
        Some(a) if a.is_absolute() && a.is_file() => Install::AppImage(a.to_path_buf()),
        _ if dir.join(PACKAGED_MARKER).is_file() => Install::Packaged,
        _ => Install::Plain(dir.to_path_buf()),
    }
}

/// Why this Linux Trek can't replace itself, if it can't.
pub fn blocker() -> Option<Blocker> {
    let local = std::env::var(LOCAL_UPDATES_ENV).is_ok_and(|v| v == "1");
    let release = || {
        std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join(RELEASE_MARKER)))
            .is_some_and(|m| m.is_file())
    };
    blocker_for(install_kind(), local, release)
}

fn blocker_for(
    kind: Option<Install>,
    local: bool,
    release: impl FnOnce() -> bool,
) -> Option<Blocker> {
    match kind {
        None => Some(Blocker::DevBuild),
        Some(Install::Packaged) => Some(Blocker::Packaged),
        Some(Install::Plain(dir)) if is_cargo_output(&dir) && !local => Some(Blocker::DevBuild),
        Some(Install::Plain(_)) => Some(Blocker::NotAppImage),
        Some(Install::AppImage(_)) if !local && !release() => Some(Blocker::DevBuild),
        // Installing renames a new file in beside it, over it.
        Some(Install::AppImage(path))
            if !path.parent().is_some_and(writable) || !writable(&path) =>
        {
            Some(Blocker::ReadOnlyLocation)
        }
        Some(Install::AppImage(_)) => None,
    }
}

/// The AppImage's magic: an ELF file whose padding says `AI` and type 2.
fn is_appimage(path: &Path) -> Result<bool> {
    use std::io::Read as _;
    let mut head = [0u8; 11];
    std::fs::File::open(path)?.read_exact(&mut head)?;
    Ok(head[..4] == *b"\x7fELF" && head[8..11] == *b"AI\x02")
}

/// The downloaded AppImage (verified already: checksum, and a signature that names its version)
/// made runnable, once it's seen to be one. On failure the download folder goes.
pub fn stage(archive: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    let result = (|| {
        if !is_appimage(archive).context("couldn't read the update")? {
            bail!("the update isn't an AppImage");
        }
        std::fs::set_permissions(archive, std::fs::Permissions::from_mode(0o755))?;
        Ok(archive.to_path_buf())
    })();
    if result.is_err()
        && let Some(dir) = download_dir_of(archive)
    {
        let _ = std::fs::remove_dir_all(dir);
    }
    result
}

/// Put the staged AppImage in the running one's place, the old one kept as the single backup
/// (`updates/previous.noindex`). Nothing has changed if it fails.
pub fn install(staged: &Path, updates: &Path) -> Result<Installed> {
    let Some(Install::AppImage(target)) = install_kind() else {
        bail!("only the AppImage updates itself");
    };
    let installed = install_over(staged, &target, updates, crate::VERSION);
    if let Some(dir) = download_dir_of(staged) {
        let _ = std::fs::remove_dir_all(dir);
    }
    installed
}

fn install_over(staged: &Path, target: &Path, updates: &Path, from: &str) -> Result<Installed> {
    if !staged.is_file() {
        bail!("the downloaded update is gone; check again");
    }
    let dir = target.parent().context("the AppImage has no folder")?;
    let name = target.file_name().context("the AppImage has no name")?;
    // The backup first, by copy: the running one stays where it is until the rename below.
    let backup_dir = updates.join(PREVIOUS_DIR);
    let backup = backup_dir.join(name);
    let backup = (std::fs::create_dir_all(&backup_dir).is_ok()
        && std::fs::copy(target, &backup).is_ok())
    .then_some(backup);
    if backup.is_none() {
        tracing::warn!("couldn't keep a backup of the previous version");
    }
    // Beside it, so the swap is one rename on one file system.
    let incoming = dir.join(format!(".{}.update", name.to_string_lossy()));
    let result = (|| {
        std::fs::copy(staged, &incoming).with_context(|| {
            format!(
                "Trek can't write to {}; move the AppImage to a folder you own",
                dir.display()
            )
        })?;
        std::fs::rename(&incoming, target).context("couldn't put the update in place")
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&incoming);
        return Err(e);
    }
    let _ = std::fs::write(updates.join(UPDATED_MARKER), from);
    Ok(Installed {
        bundle: target.to_path_buf(),
        backup,
        staged: None,
    })
}

/// Waits for the old process to exit, then starts the AppImage. If it ends with an error within
/// its first seconds (it couldn't start), the backup goes back in its place, the new version is
/// recorded as one to skip, and the backup is started instead. One that starts is left alone,
/// however it ends later. Arguments: pid, AppImage, backup (empty: none), skip marker, version.
const RELAUNCH_SCRIPT: &str = r#"
    pid=$1 app=$2 backup=$3 skip=$4 version=$5
    while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
    "$app" </dev/null >/dev/null 2>&1 &
    child=$!
    i=0
    while [ $i -lt 50 ] && kill -0 "$child" 2>/dev/null; do sleep 0.1; i=$((i+1)); done
    kill -0 "$child" 2>/dev/null && exit 0
    wait "$child" && exit 0
    if [ -n "$backup" ] && [ -f "$backup" ]; then
        cp "$backup" "$app.rollback" && chmod 755 "$app.rollback" && mv "$app.rollback" "$app"
        printf '%s\n' "$version" > "$skip"
        "$app" </dev/null >/dev/null 2>&1 &
    fi
"#;

/// The variables an AppImage's runtime sets for the process it starts: the new AppImage's runtime
/// sets its own.
const APPIMAGE_VARS: &[&str] = &["APPIMAGE", "APPDIR", "ARGV0", "OWD"];

fn relaunch_command(
    pid: u32,
    installed: &Installed,
    skip: &Path,
    version: &str,
    foreground: bool,
    vars: impl Iterator<Item = (String, String)>,
) -> std::process::Command {
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(RELAUNCH_SCRIPT)
        .arg("trek-relaunch")
        .arg(pid.to_string())
        .arg(&installed.bundle)
        .arg(installed.backup.as_deref().unwrap_or(Path::new("")))
        .arg(skip)
        .arg(version);
    // The new Trek keeps this one's setup, never its one-time launch flags.
    for (k, _) in vars.filter(|(k, _)| {
        (k.starts_with("TREK_") && !RELAUNCH_KEEPS.contains(&k.as_str()))
            || APPIMAGE_VARS.contains(&k.as_str())
    }) {
        cmd.env_remove(k);
    }
    if !foreground {
        cmd.env("TREK_BACKGROUND", "1");
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

/// Start the installed AppImage once this process has exited (see `RELAUNCH_SCRIPT`). The caller
/// quits right after.
pub fn relaunch(installed: &Installed, foreground: bool, updates: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt as _;
    let mut cmd = relaunch_command(
        std::process::id(),
        installed,
        &updates.join(SKIP_MARKER),
        crate::VERSION,
        foreground,
        std::env::vars(),
    );
    // Its own process group: what ends Trek (a terminal closing) doesn't end it.
    cmd.process_group(0);
    cmd.spawn().context("couldn't schedule the relaunch")?;
    Ok(())
}

/// Start this Trek again once it has exited: the AppImage, else the program itself.
pub fn restart(foreground: bool, updates: &Path) -> Result<()> {
    let bundle = match install_kind() {
        Some(Install::AppImage(path)) => path,
        _ => std::env::current_exe().context("Trek can't tell where it runs from")?,
    };
    relaunch(
        &Installed {
            bundle,
            backup: None,
            staged: None,
        },
        foreground,
        updates,
    )
}

/// The desktop entry's id, as the packages name it (script/bundle-linux.sh).
const APP_ID: &str = "dev.trek.Trek";

/// The release AppImage, made known to the desktop as the `.deb` makes Trek known to it: a desktop
/// entry under `~/.local/share/applications` that starts this AppImage (the app menus list it, and
/// a `trek://` link opens it), its icon, and `trek://` handed to it (`xdg-mime`). As on Windows,
/// where Trek registers `trek://` itself at launch. Written again only when it changed (the
/// AppImage moved, or a new version's icon), on a thread of its own. Never for a Trek isolated
/// from the user's data (tests, capture runs), nor for anything but the AppImage.
pub fn integrate_desktop() {
    if crate::paths::isolated() {
        return;
    }
    let Some(Install::AppImage(image)) = install_kind() else {
        return;
    };
    let Some(data) = directories::BaseDirs::new().map(|b| b.data_dir().to_path_buf()) else {
        return;
    };
    let appdir = std::env::var_os("APPDIR").map(PathBuf::from);
    std::thread::Builder::new()
        .name("trek-desktop-entry".into())
        .spawn(move || {
            if let Err(e) = integrate_into(&image, appdir.as_deref(), &data) {
                tracing::warn!("desktop entry: {e:#}");
            }
        })
        .ok();
}

/// The desktop entry for the AppImage at `image`.
fn desktop_entry(image: &Path) -> String {
    // The Exec key's quoting (the Desktop Entry spec): a path in double quotes, with `"`, `` ` ``,
    // `$` and `\` escaped.
    let quoted = image
        .to_string_lossy()
        .chars()
        .fold(String::from("\""), |mut s, c| {
            if matches!(c, '"' | '`' | '$' | '\\') {
                s.push('\\');
            }
            s.push(c);
            s
        })
        + "\"";
    format!(
        "[Desktop Entry]\nType=Application\nName=Trek\nGenericName=Coding agents\nComment=Every agent, one trail.\nExec={quoted} %U\nTryExec={}\nIcon={APP_ID}\nTerminal=false\nCategories=Development;IDE;\nKeywords=AI;agent;Claude;Codex;coding;\nMimeType=x-scheme-handler/trek;\nStartupWMClass={APP_ID}\nStartupNotify=true\n",
        image.display()
    )
}

fn integrate_into(image: &Path, appdir: Option<&Path>, data: &Path) -> Result<()> {
    let entry = data.join("applications").join(format!("{APP_ID}.desktop"));
    let wanted = desktop_entry(image);
    if std::fs::read_to_string(&entry).is_ok_and(|e| e == wanted) {
        return Ok(());
    }
    std::fs::create_dir_all(entry.parent().context("no applications folder")?)?;
    // The icons the AppImage carries, at every size it has them.
    if let Some(hicolor) = appdir.map(|a| a.join("usr/share/icons/hicolor")) {
        for size in std::fs::read_dir(&hicolor).into_iter().flatten().flatten() {
            for ext in ["png", "svg"] {
                let from = size.path().join("apps").join(format!("{APP_ID}.{ext}"));
                if from.is_file() {
                    let to = data
                        .join("icons/hicolor")
                        .join(size.file_name())
                        .join("apps")
                        .join(format!("{APP_ID}.{ext}"));
                    if let Some(dir) = to.parent() {
                        std::fs::create_dir_all(dir)?;
                    }
                    std::fs::copy(&from, &to)?;
                }
            }
        }
    }
    let tmp = entry.with_extension("desktop.tmp");
    std::fs::write(&tmp, &wanted)?;
    std::fs::rename(&tmp, &entry)?;
    // Both are xdg-utils' and desktop-file-utils' (most desktops have them); without them the
    // entry still lists Trek, and a desktop that reads MimeType itself still opens links with it.
    let quiet = |program: &str, args: &[&std::ffi::OsStr]| {
        let _ = crate::procs::command(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    };
    quiet(
        "update-desktop-database",
        &[entry.parent().unwrap_or(data).as_os_str()],
    );
    quiet(
        "xdg-mime",
        &[
            std::ffi::OsStr::new("default"),
            std::ffi::OsStr::new(&format!("{APP_ID}.desktop")),
            std::ffi::OsStr::new("x-scheme-handler/trek"),
        ],
    );
    tracing::info!(
        "desktop entry: {} starts {}",
        entry.display(),
        image.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_entry_starts_this_appimage_and_takes_trek_links() {
        let d = dir("desktop");
        let image = d.join("My Apps/Trek $1.AppImage");
        let appdir = d.join("mount");
        let icon = appdir.join("usr/share/icons/hicolor/256x256/apps");
        std::fs::create_dir_all(&icon).unwrap();
        std::fs::write(icon.join(format!("{APP_ID}.png")), b"png").unwrap();
        let data = d.join("share");
        integrate_into(&image, Some(&appdir), &data).unwrap();
        let entry =
            std::fs::read_to_string(data.join(format!("applications/{APP_ID}.desktop"))).unwrap();
        assert!(
            entry.contains(&format!(
                "Exec=\"{}/My Apps/Trek \\$1.AppImage\" %U\n",
                d.display()
            )),
            "{entry}"
        );
        assert!(entry.contains("MimeType=x-scheme-handler/trek;\n"));
        assert_eq!(
            std::fs::read(data.join(format!("icons/hicolor/256x256/apps/{APP_ID}.png"))).unwrap(),
            b"png"
        );
        // Written again only when it would change.
        let before = std::fs::metadata(data.join(format!("applications/{APP_ID}.desktop")))
            .unwrap()
            .modified()
            .unwrap();
        integrate_into(&image, None, &data).unwrap();
        let after = std::fs::metadata(data.join(format!("applications/{APP_ID}.desktop")))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after);
        let _ = std::fs::remove_dir_all(&d);
    }

    fn dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("trek-linux-update-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A file with an AppImage's magic and `body` after it.
    fn appimage(path: &Path, body: &str) {
        let mut bytes = b"\x7fELF\x02\x01\x01\x00AI\x02\x00\x00\x00\x00\x00".to_vec();
        bytes.extend_from_slice(body.as_bytes());
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn how_trek_was_installed_decides_whether_it_updates_itself() {
        let d = dir("kind");
        let image = d.join("Trek.AppImage");
        appimage(&image, "old");
        let bin = d.join("usr-lib-trek");
        std::fs::create_dir_all(&bin).unwrap();
        // The AppImage's runtime names it: that's the install, wherever the binary is mounted.
        assert_eq!(
            kind_of(Some(&image), &bin),
            Install::AppImage(image.clone())
        );
        // A relative or missing path isn't the runtime's.
        assert_eq!(
            kind_of(Some(Path::new("Trek.AppImage")), &bin),
            Install::Plain(bin.clone())
        );
        std::fs::write(bin.join(PACKAGED_MARKER), "deb\n").unwrap();
        assert_eq!(kind_of(None, &bin), Install::Packaged);

        // Only a release AppImage in a folder Trek can write replaces itself.
        let yes = || true;
        assert_eq!(
            blocker_for(Some(Install::AppImage(image.clone())), false, yes),
            None
        );
        assert_eq!(
            blocker_for(Some(Install::AppImage(image.clone())), false, || false),
            Some(Blocker::DevBuild)
        );
        assert_eq!(
            blocker_for(Some(Install::AppImage(image.clone())), true, || false),
            None,
            "TREK_UPDATE_LOCAL_BUNDLE=1 lets a local one try"
        );
        assert_eq!(
            blocker_for(Some(Install::AppImage(d.join("gone.AppImage"))), false, yes),
            Some(Blocker::ReadOnlyLocation)
        );
        assert_eq!(
            blocker_for(Some(Install::Packaged), true, yes),
            Some(Blocker::Packaged)
        );
        assert_eq!(
            blocker_for(Some(Install::Plain(d.clone())), false, yes),
            Some(Blocker::NotAppImage)
        );
        let cargo = d.join("target/release");
        std::fs::create_dir_all(cargo.join(".fingerprint")).unwrap();
        assert_eq!(
            blocker_for(Some(Install::Plain(cargo)), false, yes),
            Some(Blocker::DevBuild)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_update_takes_the_appimage_s_place_and_keeps_the_old_one() {
        let d = dir("install");
        let image = d.join("Trek.AppImage");
        appimage(&image, "0.1");
        let updates = d.join("updates");
        let download = updates.join("download-1-0.noindex");
        std::fs::create_dir_all(&download).unwrap();
        let archive = download.join("Trek-0.2.0.AppImage");
        appimage(&archive, "0.2");
        let staged = stage(&archive).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&staged).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let installed = install_over(&staged, &image, &updates, "0.1.0").unwrap();
        assert!(std::fs::read(&image).unwrap().ends_with(b"0.2"));
        let backup = installed.backup.unwrap();
        assert!(std::fs::read(&backup).unwrap().ends_with(b"0.1"));
        assert_eq!(
            std::fs::read_to_string(updates.join(UPDATED_MARKER)).unwrap(),
            "0.1.0"
        );
        assert!(
            !d.join(".Trek.AppImage.update").exists(),
            "nothing left beside it"
        );
        // Something that isn't an AppImage is refused, and its download goes.
        let bad = download.join("Trek-0.3.0.AppImage");
        std::fs::write(&bad, b"#!/bin/sh\necho hi\n").unwrap();
        assert!(stage(&bad).is_err());
        assert!(!download.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_relaunch_keeps_trek_s_setup_but_not_its_launch_flags_or_the_old_runtime_s() {
        let installed = Installed {
            bundle: PathBuf::from("/home/u/Apps/Trek.AppImage"),
            backup: None,
            staged: None,
        };
        let vars = [
            ("TREK_DATA_DIR", "/d"),
            ("TREK_MOCK_PROMPT", "x"),
            ("APPIMAGE", "/home/u/Apps/Trek.AppImage"),
            ("APPDIR", "/tmp/.mount_x"),
            ("HOME", "/home/u"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()));
        let cmd = relaunch_command(
            7,
            &installed,
            Path::new("/u/skip"),
            "0.4.63",
            false,
            vars.into_iter(),
        );
        let envs: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for gone in ["TREK_MOCK_PROMPT", "APPIMAGE", "APPDIR"] {
            assert!(envs.contains(&(gone.to_string(), None)), "{gone}: {envs:?}");
        }
        assert!(
            !envs
                .iter()
                .any(|(k, _)| k == "TREK_DATA_DIR" || k == "HOME")
        );
        assert!(envs.contains(&("TREK_BACKGROUND".into(), Some("1".into()))));
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args[2..],
            [
                "trek-relaunch",
                "7",
                "/home/u/Apps/Trek.AppImage",
                "",
                "/u/skip",
                "0.4.63"
            ]
        );
    }
}
