//! The helper's swap and rollback on a temporary folder tree: as functions everywhere, and as the
//! real `trek-update` process on Windows. `trek.exe` is the `fixture` binary (`exit <code>`,
//! `sleep <secs>`); which version a folder holds is written in its `trek-mcp.exe`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use trek_update::{Plan, parse, restore, run, swap};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trek-update-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A Trek folder: a `trek.exe` that runs, `trek-mcp.exe` saying which version this is, and the
/// helper.
fn trek_folder(dir: &Path, version: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::copy(trek_test_fixtures::bin("fixture"), dir.join("trek.exe")).unwrap();
    std::fs::write(dir.join("trek-mcp.exe"), version).unwrap();
    std::fs::write(dir.join("trek-update.exe"), "helper").unwrap();
}

fn version(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("trek-mcp.exe")).unwrap_or_default()
}

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

/// The install, the staged update, and Trek's updates folder.
fn tree(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let dir = scratch(name);
    let (install, staged, updates) = (dir.join("Apps").join("Trek"), dir.join("updates").join("download-1-0.noindex").join("stage"), dir.join("updates"));
    trek_folder(&install, "0.4.0");
    trek_folder(&staged, "0.4.1");
    (dir, install, staged, updates)
}

fn plan(install: &Path, staged: &Path, updates: &Path, relaunch: Option<&[&str]>) -> Plan {
    Plan {
        pid: u32::MAX,
        install: install.into(),
        staged: Some(staged.into()),
        from: "0.4.0".into(),
        to: "0.4.1".into(),
        updates: Some(updates.into()),
        relaunch: relaunch.map(args),
    }
}

#[test]
fn arguments_are_read_and_checked() {
    let p = parse(args(&["--pid", "42", "--install", "C:/T", "--staged", "C:/S", "--from", "0.4.0", "--to", "0.4.1", "--updates", "C:/U", "--relaunch", "--x", "y"])).unwrap();
    assert_eq!(p.pid, 42);
    assert_eq!((p.install, p.staged), (PathBuf::from("C:/T"), Some(PathBuf::from("C:/S"))));
    assert_eq!((p.from.as_str(), p.to.as_str()), ("0.4.0", "0.4.1"));
    assert_eq!(p.relaunch, Some(args(&["--x", "y"])), "everything after --relaunch is Trek's");
    let quit = parse(args(&["--pid", "42", "--install", "C:/T", "--from", "0.4.0"])).unwrap();
    assert_eq!((quit.staged, quit.relaunch), (None, None));
    assert_eq!(parse(args(&["--pid", "42", "--install", "C:/T", "--from", "0.4.0", "--relaunch"])).unwrap().relaunch, Some(vec![]));
    assert!(parse(args(&["--pid", "x", "--install", "C:/T", "--from", "0.4.0"])).is_err());
    assert!(parse(args(&["--install", "C:/T", "--from", "0.4.0"])).is_err(), "no pid");
    assert!(parse(args(&["--pid", "1", "--from", "0.4.0"])).is_err(), "no install");
    assert!(parse(args(&["--pid", "1", "--install", "C:/T", "--from"])).is_err(), "a flag without its value");
    assert!(parse(args(&["--pid", "1", "--install", "C:/T", "--from", "1", "--delete", "C:/"])).is_err());
}

#[test]
fn the_update_takes_the_installs_place_and_the_old_one_moves_aside() {
    let (dir, install, staged, _) = tree("swap");
    let old = swap(&install, &staged, "0.4.0", "0.4.1").unwrap();
    assert_eq!(old, install.with_file_name("Trek.old-0.4.0"));
    assert_eq!((version(&install), version(&old)), ("0.4.1".into(), "0.4.0".into()));
    assert!(!staged.exists());
    // A leftover from an earlier update keeps its name; this one takes the next.
    trek_folder(&staged, "0.4.2");
    assert_eq!(swap(&install, &staged, "0.4.0", "0.4.2").unwrap(), install.with_file_name("Trek.old-0.4.0-2"));
    assert_eq!(version(&install), "0.4.2");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_folder_holding_more_than_trek_is_never_swapped() {
    let (dir, install, staged, _) = tree("shared");
    // Trek unzipped straight into Downloads: replacing "its folder" would take everything else.
    std::fs::write(install.join("holiday.jpg"), "x").unwrap();
    let e = swap(&install, &staged, "0.4.0", "0.4.1").unwrap_err();
    assert!(e.contains("holds more than Trek") && e.contains("nothing was changed"), "{e}");
    std::fs::remove_file(install.join("holiday.jpg")).unwrap();
    std::fs::create_dir(install.join("projects")).unwrap();
    assert!(swap(&install, &staged, "0.4.0", "0.4.1").is_err(), "a folder in it too");
    std::fs::remove_dir(install.join("projects")).unwrap();
    // Trek's own write check leaves nothing for long, and doesn't count.
    std::fs::write(install.join(".trek-write-test-7"), "").unwrap();
    // No trek.exe in the update: refused before anything moves.
    std::fs::remove_file(staged.join("trek.exe")).unwrap();
    assert!(swap(&install, &staged, "0.4.0", "0.4.1").unwrap_err().contains("no trek.exe"));
    assert_eq!(version(&install), "0.4.0");
    assert_eq!(std::fs::read_dir(install.parent().unwrap()).unwrap().count(), 1, "nothing next to the install");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn restore_puts_the_previous_version_back_and_keeps_the_failed_one_aside() {
    let (dir, install, staged, _) = tree("restore");
    let old = swap(&install, &staged, "0.4.0", "0.4.1").unwrap();
    restore(&install, &old, "0.4.1").unwrap();
    assert_eq!(version(&install), "0.4.0");
    assert!(!old.exists());
    assert_eq!(version(&install.with_file_name("Trek.old-failed-0.4.1")), "0.4.1", "named so Trek deletes it");
    // Nothing to put back: it says where things are rather than guessing.
    let e = restore(&install, &dir.join("gone"), "0.4.1").unwrap_err();
    assert!(e.contains("gone"), "{e}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn copying_a_folder_copies_files_and_folders() {
    let dir = scratch("copy");
    std::fs::create_dir_all(dir.join("a/sub")).unwrap();
    std::fs::write(dir.join("a/trek.exe"), "t").unwrap();
    std::fs::write(dir.join("a/sub/x"), "x").unwrap();
    trek_update::copy_dir(&dir.join("a"), &dir.join("b")).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("b/sub/x")).unwrap(), "x");
    assert!(trek_update::copy_dir(&dir.join("a"), &dir.join("b")).is_err(), "never into an existing folder");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn run_installs_starts_the_new_version_and_rolls_back_when_it_wont_start() {
    let (dir, install, staged, updates) = tree("run");
    let mut lines = vec![];
    // Quitting: installed, nothing started.
    assert_eq!(run(&plan(&install, &staged, &updates, None), &mut |l| lines.push(l.to_string())), 0);
    assert_eq!(version(&install), "0.4.1");
    assert!(lines.last().unwrap().contains("installed"), "{lines:?}");

    // Restarting into a version that exits with an error at once: the previous one comes back,
    // the new one is skipped from now on, and Trek is told why.
    trek_folder(&staged, "0.4.2");
    let mut p = plan(&install, &staged, &updates, Some(&["exit", "3"]));
    p.from = "0.4.1".into();
    p.to = "0.4.2".into();
    assert_eq!(run(&p, &mut |l| lines.push(l.to_string())), 1);
    assert_eq!(version(&install), "0.4.1");
    assert_eq!(std::fs::read_to_string(updates.join("skip-version")).unwrap().trim(), "0.4.2");
    let note = std::fs::read_to_string(updates.join("install-failed")).unwrap();
    assert!(note.starts_with("Trek 0.4.2 didn't install: Trek 0.4.2 didn't start"), "{note}");

    // One that starts: done.
    std::fs::remove_file(updates.join("skip-version")).unwrap();
    trek_folder(&staged, "0.4.2");
    assert_eq!(run(&plan(&install, &staged, &updates, Some(&["exit", "0"])), &mut |l| lines.push(l.to_string())), 0);
    assert_eq!(version(&install), "0.4.2");
    assert!(!updates.join("skip-version").exists());
    assert_eq!(lines.last().unwrap(), "the new version started");

    // A swap that can't happen (the folder isn't Trek's alone) starts the version that's there.
    trek_folder(&staged, "0.4.3");
    std::fs::write(install.join("notes.txt"), "mine").unwrap();
    assert_eq!(run(&plan(&install, &staged, &updates, Some(&["exit", "0"])), &mut |l| lines.push(l.to_string())), 1);
    assert_eq!((version(&install), std::fs::read_to_string(install.join("notes.txt")).unwrap()), ("0.4.2".into(), "mine".into()));
    assert!(staged.exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// What the brief calls "simulate a failed swap by locking a file": Windows won't rename a folder
/// while a file in it is open without delete sharing (an editor, an antivirus scan that lasts).
#[cfg(windows)]
#[test]
fn a_locked_file_fails_the_swap_and_leaves_or_puts_everything_back() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let (dir, install, staged, _) = tree("locked");
    let lock = |path: &Path| std::fs::OpenOptions::new().read(true).share_mode(1 /* FILE_SHARE_READ */).open(path).unwrap();

    // In the install: it can't move aside, so nothing changes.
    let held = lock(&install.join("trek-mcp.exe"));
    let e = swap(&install, &staged, "0.4.0", "0.4.1").unwrap_err();
    assert!(e.contains("couldn't move") && e.contains("nothing was changed"), "{e}");
    drop(held);
    assert_eq!((version(&install), version(&staged)), ("0.4.0".into(), "0.4.1".into()));

    // In the update: it can't be brought next to the install, which hasn't been touched.
    let held = lock(&staged.join("trek-mcp.exe"));
    let e = swap(&install, &staged, "0.4.0", "0.4.1").unwrap_err();
    assert!(e.contains("couldn't get the update next to") && e.contains("nothing was changed"), "{e}");
    drop(held);
    assert_eq!(version(&install), "0.4.0");
    assert_eq!(std::fs::read_dir(install.parent().unwrap()).unwrap().count(), 1, "nothing next to the install");
    let _ = std::fs::remove_dir_all(dir);
}

/// A copy across volumes can fail half way (a file held open, a full disk); the install, which
/// the copy never touches, is as it was.
#[cfg(windows)]
#[test]
fn a_copy_that_fails_leaves_the_install_alone() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let (dir, install, staged, _) = tree("copy-fails");
    let other_volume = |_: &Path, _: &Path| Err(std::io::Error::from_raw_os_error(17));
    // Exclusive: it can't be read, so it can't be copied.
    let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(staged.join("trek-update.exe")).unwrap();
    let e = trek_update::swap_with(&install, &staged, "0.4.0", "0.4.1", &other_volume).unwrap_err();
    assert!(e.contains("couldn't get the update next to") && e.contains("nothing was changed"), "{e}");
    drop(held);
    assert_eq!((version(&install), version(&staged)), ("0.4.0".into(), "0.4.1".into()));
    // What the failed copy left is named for Trek to delete, and holds only Trek's files.
    let names: Vec<String> = std::fs::read_dir(install.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().all(|n| n == "Trek" || n.starts_with("Trek.old-incoming-")), "{names:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// What `swap` does when the update is on another volume: copied, not moved, and only then does
/// the install move aside.
#[test]
fn an_update_on_another_volume_is_copied_next_to_the_install_before_anything_moves() {
    let (dir, install, staged, _) = tree("cross-volume");
    let other_volume = |_: &Path, _: &Path| Err(std::io::Error::from_raw_os_error(if cfg!(windows) { 17 } else { 18 }));
    let old = trek_update::swap_with(&install, &staged, "0.4.0", "0.4.1", &other_volume).unwrap();
    assert_eq!((version(&install), version(&old)), ("0.4.1".into(), "0.4.0".into()));
    assert_eq!(version(&staged), "0.4.1", "a copy leaves the staged folder for Trek to delete");
    assert_eq!(std::fs::read_dir(install.parent().unwrap()).unwrap().count(), 2, "the install and the old one: no copy left over");
    assert_eq!(std::fs::read(install.join("trek.exe")).unwrap(), std::fs::read(trek_test_fixtures::bin("fixture")).unwrap());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_full_paths_to_trek_folders_one_not_inside_the_other_are_swapped() {
    let (dir, install, staged, _) = tree("paths");
    let unchanged = |what: &str| {
        assert_eq!((version(&install), version(&staged)), ("0.4.0".into(), "0.4.1".into()), "{what}");
        assert_eq!(std::fs::read_dir(install.parent().unwrap()).unwrap().count(), 1, "{what}: nothing next to the install");
    };
    assert!(swap(Path::new("Trek"), &staged, "0.4.0", "0.4.1").unwrap_err().contains("full paths"));
    assert!(swap(&install, Path::new("stage"), "0.4.0", "0.4.1").unwrap_err().contains("full paths"));
    unchanged("relative");
    // The install is the update, or one holds the other.
    assert!(swap(&install, &install, "0.4.0", "0.4.1").unwrap_err().contains("one inside the other"));
    let inside = install.join("..").join("Trek").join("trek-mcp.exe");
    assert!(swap(&install, &inside, "0.4.0", "0.4.1").is_err());
    let upper = PathBuf::from(install.to_string_lossy().to_uppercase());
    if cfg!(windows) {
        assert!(swap(&install, &upper, "0.4.0", "0.4.1").unwrap_err().contains("one inside the other"), "however it's spelled");
    }
    // An update folder that holds anything else (say, a Downloads that has a trek.exe in it).
    std::fs::write(staged.join("holiday.jpg"), "x").unwrap();
    let e = swap(&install, &staged, "0.4.0", "0.4.1").unwrap_err();
    assert!(e.contains("isn't Trek's own folder") && e.contains("holds more than Trek"), "{e}");
    std::fs::remove_file(staged.join("holiday.jpg")).unwrap();
    unchanged("stray file in the update");
    // A drive root is never a Trek folder.
    assert!(swap(Path::new(r"C:\"), &staged, "0.4.0", "0.4.1").is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(windows)]
#[test]
fn a_install_that_is_a_link_is_never_swapped() {
    let (dir, install, staged, _) = tree("link");
    let link = dir.join("Apps").join("Link");
    let made = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(&install).output().unwrap();
    assert!(made.status.success());
    let e = swap(&link, &staged, "0.4.0", "0.4.1").unwrap_err();
    assert!(e.contains("is a link") && e.contains("nothing was changed"), "{e}");
    assert_eq!((version(&install), version(&staged)), ("0.4.0".into(), "0.4.1".into()));
    // Through the link works for what's inside it: its folder is a real one.
    assert_eq!(version(&link), "0.4.0");
    let _ = std::fs::remove_dir_all(dir);
}

/// If the previous version has gone (something deleted it between the swap and the rollback), the
/// install, the new version even if it's a broken one, is left in its place: moving it aside for a
/// folder that isn't there would leave no Trek at all.
#[test]
fn restore_with_nothing_to_put_back_leaves_the_install_in_place() {
    let (dir, install, staged, _) = tree("restore-gone");
    let old = swap(&install, &staged, "0.4.0", "0.4.1").unwrap();
    std::fs::remove_dir_all(&old).unwrap();
    let e = restore(&install, &old, "0.4.1").unwrap_err();
    assert!(e.contains("isn't in") && e.contains("left as it is"), "{e}");
    assert_eq!(version(&install), "0.4.1");
    assert_eq!(std::fs::read_dir(install.parent().unwrap()).unwrap().count(), 1, "nothing was moved aside");
    // An old folder with no trek.exe in it (a part) isn't a version to put back either.
    std::fs::create_dir(&old).unwrap();
    std::fs::write(old.join("trek-mcp.exe"), "part").unwrap();
    assert!(restore(&install, &old, "0.4.1").is_err());
    assert_eq!(version(&install), "0.4.1");
    let _ = std::fs::remove_dir_all(dir);
}

/// The rollback's second rename failing must not leave the install's place empty.
#[cfg(windows)]
#[test]
fn restore_that_cant_finish_puts_the_failed_update_back_rather_than_nothing() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let (dir, install, staged, _) = tree("restore-locked");
    let old = swap(&install, &staged, "0.4.0", "0.4.1").unwrap();
    let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(old.join("trek-mcp.exe")).unwrap();
    let e = restore(&install, &old, "0.4.1").unwrap_err();
    assert!(e.contains("couldn't put the previous version back") && e.contains("Trek.old-0.4.0"), "{e}");
    drop(held);
    assert_eq!((version(&install), version(&old)), ("0.4.1".into(), "0.4.0".into()), "a Trek in its place, the other where the error says");
    let _ = std::fs::remove_dir_all(dir);
}

/// A Trek that doesn't quit in time: nothing is changed, Trek isn't started again, and the next
/// start says why.
#[cfg(windows)]
#[test]
fn a_trek_that_wont_quit_leaves_everything_as_it_is() {
    let (dir, install, staged, updates) = tree("wont-quit");
    let mut trek = std::process::Command::new(trek_test_fixtures::bin("fixture")).args(["sleep", "30"]).spawn().unwrap();
    let mut p = plan(&install, &staged, &updates, Some(&["exit", "0"]));
    p.pid = trek.id();
    let mut lines = vec![];
    let started = std::time::Instant::now();
    let code = trek_update::run_waiting(&p, std::time::Duration::from_millis(700), &mut |l| lines.push(l.to_string()));
    let _ = trek.kill();
    let _ = trek.wait();
    assert_eq!(code, 1);
    assert!(started.elapsed() >= std::time::Duration::from_millis(600) && started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert_eq!((version(&install), version(&staged)), ("0.4.0".into(), "0.4.1".into()));
    assert_eq!(std::fs::read_dir(install.parent().unwrap()).unwrap().count(), 1);
    assert!(!lines.iter().any(|l| l.starts_with("started")), "{lines:?}");
    let note = std::fs::read_to_string(updates.join("install-failed")).unwrap();
    assert!(note.contains("still running") && note.contains("nothing was changed"), "{note}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The real helper, as Trek starts it: it waits for the old Trek to exit, swaps, starts the new
/// one and logs what it did.
#[cfg(windows)]
#[test]
fn the_helper_waits_for_trek_then_swaps_and_relaunches() {
    let (dir, install, staged, updates) = tree("process");
    let data = dir.join("data");
    let mut old_trek = std::process::Command::new(install.join("trek.exe")).args(["sleep", "1.5"]).spawn().unwrap();
    let started = std::time::Instant::now();
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_trek-update"))
        .args(["--pid", &old_trek.id().to_string(), "--from", "0.4.0", "--to", "0.4.1"])
        .arg("--install")
        .arg(&install)
        .arg("--staged")
        .arg(&staged)
        .arg("--updates")
        .arg(&updates)
        .args(["--relaunch", "exit", "0"])
        .env("TREK_DATA_DIR", &data)
        .status()
        .unwrap();
    assert!(status.success(), "{status}");
    assert!(old_trek.try_wait().unwrap().is_some(), "the old Trek had exited");
    assert!(started.elapsed() >= std::time::Duration::from_millis(1200), "it waited for it: {:?}", started.elapsed());
    assert_eq!((version(&install), version(&install.with_file_name("Trek.old-0.4.0"))), ("0.4.1".into(), "0.4.0".into()));
    let log = std::fs::read_to_string(data.join("logs").join("update.log")).unwrap();
    assert!(log.contains("installed; the previous version is in") && log.contains("the new version started"), "{log}");
    assert!(!updates.join("install-failed").exists());

    // Bad arguments: logged, nothing touched.
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_trek-update")).args(["--pid", "nope"]).env("TREK_DATA_DIR", &data).status().unwrap();
    assert_eq!(status.code(), Some(2));
    assert!(std::fs::read_to_string(data.join("logs").join("update.log")).unwrap().contains("--pid takes a process id"));
    let _ = std::fs::remove_dir_all(dir);
}
