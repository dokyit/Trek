//! `trek-update.exe`: once Trek has quit, swaps the folder Trek is installed in for the update
//! Trek staged, and puts the old one back if anything fails. Trek starts it from the staged
//! folder (never from the install it replaces) and quits:
//!
//! ```text
//! trek-update --pid <Trek's pid> --install <folder holding trek.exe> --from <running version>
//!             [--staged <new folder>] [--to <new version>] [--updates <Trek's updates folder>]
//!             [--relaunch <arguments for trek.exe…>]
//! ```
//!
//! 1. Waits up to a minute for Trek to exit. Still running: gives up and changes nothing.
//! 2. Renames the install to `<install>.old-<from>`, moves the staged folder into its place (a
//!    rename on one volume, a copy across volumes) and checks `trek.exe` is there.
//! 3. With `--relaunch`, starts the new `trek.exe`. If it won't start, or exits with an error in
//!    its first seconds, the old version goes back, the new one is recorded in
//!    `<updates>/skip-version` (Trek won't offer it again) and the old one is started.
//!
//! Any failure puts the old version back, and is written to `update.log` in Trek's log folder and,
//! as a sentence for Trek to show, to `<updates>/install-failed`. Nothing is deleted here: Trek
//! deletes `<install>.old-*` on its next start, and its download folder with what's left of the
//! staged copy. Without `--staged` there's nothing to swap and only the restart happens.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Everything a Trek install folder holds (the release zip, flat). The folder is replaced as a
/// whole, so one holding anything else (Trek unzipped straight into Downloads) is never touched.
/// trek-core's `update::TREK_FILES` is the same list.
pub const TREK_FILES: &[&str] = &["trek.exe", "trek-mcp.exe", "trek-update.exe"];

/// Trek's check that it can write to its folder creates and removes one of these.
const PROBE_PREFIX: &str = ".trek-write-test-";

/// How long Trek gets to quit.
const WAIT_FOR_TREK: Duration = Duration::from_secs(60);
/// How long a new Trek is watched for exiting with an error.
const WATCH_START: Duration = Duration::from_secs(5);
/// An antivirus scan or the search indexer can hold a file for a moment: renames are tried
/// again, over about two seconds.
const RENAME_TRIES: u32 = 10;
const RENAME_PAUSE: Duration = Duration::from_millis(200);

#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub pid: u32,
    pub install: PathBuf,
    pub staged: Option<PathBuf>,
    pub from: String,
    pub to: String,
    pub updates: Option<PathBuf>,
    /// Start Trek afterwards, with these arguments.
    pub relaunch: Option<Vec<OsString>>,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Plan, String> {
    let mut plan = Plan::default();
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        if flag == "--relaunch" {
            plan.relaunch = Some(args.by_ref().collect());
            break;
        }
        let value = args.next().ok_or_else(|| format!("{} needs a value", flag.to_string_lossy()))?;
        match flag.to_str() {
            Some("--pid") => plan.pid = value.to_str().and_then(|v| v.parse().ok()).ok_or_else(|| "--pid takes a process id".to_string())?,
            Some("--install") => plan.install = value.into(),
            Some("--staged") => plan.staged = Some(value.into()),
            Some("--from") => plan.from = version_tag(&value),
            Some("--to") => plan.to = version_tag(&value),
            Some("--updates") => plan.updates = Some(value.into()),
            _ => return Err(format!("unknown argument {}", flag.to_string_lossy())),
        }
    }
    if plan.pid == 0 || plan.install.as_os_str().is_empty() || plan.from.is_empty() {
        return Err("usage: trek-update --pid <pid> --install <folder> --from <version> [--staged <folder>] [--to <version>] [--updates <folder>] [--relaunch <args…>]".into());
    }
    Ok(plan)
}

/// A version as it goes into a folder name: semver's characters only.
fn version_tag(v: &std::ffi::OsStr) -> String {
    v.to_string_lossy().chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')).collect()
}

/// Do what `plan` says; the process's exit code. `log` gets a line for each step.
pub fn run(plan: &Plan, log: &mut dyn FnMut(&str)) -> i32 {
    log(&format!("Trek {} in {} (pid {}) → {}", plan.from, plan.install.display(), plan.pid, if plan.to.is_empty() { "a restart" } else { &plan.to }));
    if !wait_for_exit(plan.pid, WAIT_FOR_TREK) {
        log("Trek is still running after a minute; nothing was changed");
        return 1;
    }
    let Some(staged) = &plan.staged else {
        // Nothing to swap (the install is already up to date): only the restart.
        return match &plan.relaunch {
            Some(args) => start(&plan.install, args, log),
            None => 0,
        };
    };
    let old = match swap(&plan.install, staged, &plan.from, &plan.to) {
        Ok(old) => old,
        Err(why) => {
            fail(plan, &why, log);
            if let Some(args) = &plan.relaunch {
                start(&plan.install, args, log);
            }
            return 1;
        }
    };
    log(&format!("installed; the previous version is in {}", old.display()));
    let Some(args) = &plan.relaunch else { return 0 };
    let Err(e) = start_and_watch(&plan.install, args) else {
        log("the new version started");
        return 0;
    };
    let why = format!("Trek {} didn't start ({e})", plan.to);
    match restore(&plan.install, &old, &plan.to) {
        Ok(()) => {
            if let (Some(updates), false) = (&plan.updates, plan.to.is_empty()) {
                let _ = std::fs::write(updates.join("skip-version"), format!("{}\n", plan.to));
            }
            fail(plan, &format!("{why}; the previous version is back"), log);
            start(&plan.install, args, log);
        }
        Err(back) => fail(plan, &format!("{why}, and {back}"), log),
    }
    1
}

/// Log a failure and leave Trek a sentence to show on its next start.
fn fail(plan: &Plan, why: &str, log: &mut dyn FnMut(&str)) {
    log(why);
    if let Some(updates) = &plan.updates {
        let to = if plan.to.is_empty() { "the new version".to_string() } else { format!("Trek {}", plan.to) };
        let _ = std::fs::write(updates.join("install-failed"), format!("{to} didn't install: {why}.\n"));
    }
}

/// Swap `staged` in for `install`; returns where the replaced version is now. When it fails,
/// the install is as it was (or the error says where the previous version is).
pub fn swap(install: &Path, staged: &Path, from: &str, to: &str) -> Result<PathBuf, String> {
    if !staged.join("trek.exe").is_file() {
        return Err(format!("{} has no trek.exe; nothing was changed", staged.display()));
    }
    own_folder(install)?;
    let old = free_name(install, &format!("old-{from}"));
    rename_patiently(install, &old).map_err(|e| format!("couldn't move {} aside ({e}); nothing was changed", install.display()))?;
    let moved = match rename_patiently(staged, install) {
        Err(e) if cross_volume(&e) => copy_dir(staged, install),
        other => other,
    };
    let problem = match moved {
        Ok(()) if install.join("trek.exe").is_file() => return Ok(old),
        Ok(()) => format!("{} had no trek.exe after the move", install.display()),
        Err(e) => format!("couldn't move the update into {} ({e})", install.display()),
    };
    match restore(install, &old, to) {
        Ok(()) => Err(format!("{problem}; the previous version is back")),
        Err(back) => Err(format!("{problem}, and {back}")),
    }
}

/// Put the previous version back in `install`'s place. Whatever is there now (an update that
/// failed, part of a copy) moves to `<install>.old-failed-<to>` for Trek to delete.
pub fn restore(install: &Path, old: &Path, to: &str) -> Result<(), String> {
    if install.exists() {
        let failed = free_name(install, &format!("old-failed-{to}"));
        rename_patiently(install, &failed).map_err(|e| format!("couldn't move the failed update aside ({e}); the previous version is in {}", old.display()))?;
    }
    rename_patiently(old, install).map_err(|e| format!("couldn't put the previous version back ({e}); it is in {}", old.display()))
}

/// The install is a folder of Trek's own: `trek.exe` and nothing but Trek's files.
pub fn own_folder(install: &Path) -> Result<(), String> {
    if !install.join("trek.exe").is_file() {
        return Err(format!("{} has no trek.exe; nothing was changed", install.display()));
    }
    let entries = std::fs::read_dir(install).map_err(|e| format!("couldn't read {} ({e}); nothing was changed", install.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let file = entry.file_type().is_ok_and(|t| t.is_file());
        if !(file && (TREK_FILES.contains(&name.as_str()) || name.starts_with(PROBE_PREFIX))) {
            return Err(format!("{} holds more than Trek ({name}), so it isn't replaced; nothing was changed", install.display()));
        }
    }
    Ok(())
}

/// `<install>.<tag>` next to the install, or `<install>.<tag>-2`, … if that's taken.
fn free_name(install: &Path, tag: &str) -> PathBuf {
    let name = install.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Trek".into());
    let tag = tag.trim_end_matches('-');
    let at = |suffix: String| install.with_file_name(format!("{name}.{tag}{suffix}"));
    (1..1000).map(|n| at(if n == 1 { String::new() } else { format!("-{n}") })).find(|p| !p.exists()).unwrap_or_else(|| at(format!("-{}", std::process::id())))
}

/// Rename a folder, trying again for a while if something holds a file in it. Never onto
/// something that exists (a rename on Unix would replace an empty folder).
pub fn rename_patiently(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} exists", to.display())));
    }
    let mut tries = 1;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if tries < RENAME_TRIES && from.exists() && !cross_volume(&e) => {
                tries += 1;
                std::thread::sleep(RENAME_PAUSE);
            }
            result => return result,
        }
    }
}

/// The rename failed because the two places are on different volumes.
fn cross_volume(e: &std::io::Error) -> bool {
    // ERROR_NOT_SAME_DEVICE on Windows, EXDEV on Unix.
    e.raw_os_error() == Some(if cfg!(windows) { 17 } else { 18 })
}

/// Copy a folder of files and folders (the staged update has no links).
pub fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &dest)?;
        } else {
            return Err(std::io::Error::other(format!("{} is a link", entry.path().display())));
        }
    }
    Ok(())
}

fn command(install: &Path, args: &[OsString]) -> Command {
    let mut cmd = Command::new(install.join("trek.exe"));
    // Not the install itself: a process working in a folder keeps it from being renamed, and
    // the next update renames it.
    cmd.args(args).current_dir(install.parent().unwrap_or(install));
    cmd
}

/// Start Trek and leave it be; the exit code to end with.
fn start(install: &Path, args: &[OsString], log: &mut dyn FnMut(&str)) -> i32 {
    match command(install, args).spawn() {
        Ok(_) => 0,
        Err(e) => {
            log(&format!("couldn't start {} ({e})", install.join("trek.exe").display()));
            1
        }
    }
}

/// Start the new Trek and watch it for a few seconds: one that won't start or exits with an
/// error straight away is a failed update. (One that exits cleanly handed over to a Trek that
/// was already running.)
fn start_and_watch(install: &Path, args: &[OsString]) -> Result<(), String> {
    let mut child = command(install, args).spawn().map_err(|e| format!("it couldn't be started: {e}"))?;
    let until = Instant::now() + WATCH_START;
    while Instant::now() < until {
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => return Err(format!("it exited with {status}")),
            Ok(Some(_)) => return Ok(()),
            _ => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    Ok(())
}

/// Wait for process `pid` to exit; false if it's still running after `timeout`. A process that
/// can't be opened has exited (or the id is someone else's now).
#[cfg(windows)]
pub fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: plain calls; the handle is checked before use and closed after.
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return true;
        }
        let waited = WaitForSingleObject(h, timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32);
        CloseHandle(h);
        waited == WAIT_OBJECT_0
    }
}

/// The helper only runs on Windows; elsewhere (its tests) every process counts as gone.
#[cfg(not(windows))]
pub fn wait_for_exit(_pid: u32, _timeout: Duration) -> bool {
    true
}

/// `update.log` in Trek's log folder: `logs` in the data folder `TREK_DATA_DIR` moved, else
/// `%LOCALAPPDATA%\Trek\logs` (where Trek's own logs are).
pub fn log_file() -> PathBuf {
    let dir = match std::env::var_os("TREK_DATA_DIR").filter(|d| !d.is_empty()) {
        Some(data) => PathBuf::from(data).join("logs"),
        None => std::env::var_os("LOCALAPPDATA").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(std::env::temp_dir).join("Trek").join("logs"),
    };
    dir.join("update.log")
}

/// Append a line to the log, stamped with the time (UTC).
pub fn append_log(file: &Path, line: &str) {
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{}  {line}", utc_now());
    }
}

/// `YYYY-MM-DD HH:MM:SS` in UTC, without a date library.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", rest / 3600, rest / 60 % 60, rest % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_tags_keep_semver_characters_only() {
        assert_eq!(super::version_tag("0.4.1-nightly.20261002+x".as_ref()), "0.4.1-nightly.20261002+x");
        assert_eq!(super::version_tag(r"..\..\evil".as_ref()), "....evil");
    }

    #[test]
    fn log_lines_are_stamped_in_utc() {
        let stamp = super::utc_now();
        assert_eq!(stamp.len(), 19, "{stamp}");
        assert!(stamp.starts_with("20"), "{stamp}");
    }
}
