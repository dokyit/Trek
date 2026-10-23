//! Moving Trek out of a folder it can't update itself in (Windows: one ransomware protection
//! guards, Program Files, a folder holding more than Trek's files), in one click.
//!
//! Reading from such a folder is never refused, only writing into it, so Trek copies its own files
//! out to a folder the user owns (`%LOCALAPPDATA%\Programs\Trek`, which nothing guards), checks
//! each copy, and has the copy's `trek-update.exe` start it once this Trek has quit. Nothing here
//! touches the folder Trek leaves, until the user asks for it to go (`remove_old_copy`).
//!
//! 1. `copy_out`: choose the folder (an own folder of Trek's with nothing running from it is
//!    replaced; anything else gets a free name beside it), copy `TREK_FILES` there under temporary
//!    names, compare each copy's size and SHA-256 with the original, rename them into place.
//!    Any failure takes back what it made and says why; the folder Trek runs from is only read.
//! 2. `start_copy`: leave `relocated-from` in the updates folder and start the copy's helper, which
//!    waits for this process to exit (as for an update's restart) and starts the copy. The caller
//!    quits.
//! 3. `at_launch` (`take_relocation`): the copy, at its launch, finds the note and says where it
//!    runs now, and keeps a note of the old copy until it is dealt with; `remove_old_copy` is the
//!    button on the toast and on the Updates page.

use super::{
    HELPER_EXE, INSTALL_FAILED, TREK_EXE, TREK_FILES, current_version, exe_version, helper_command,
    inside, install_root, is_link, own_folder, spawn_detached,
};
use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};

/// Where to move Trek instead of the programs folder (testing; never the user's own).
pub const MOVE_TO_ENV: &str = "TREK_RELOCATE_TO";
/// Test hook: the names of more `TREK_*` variables (comma separated) the moved Trek keeps, which
/// a restart drops (`RELAUNCH_KEEPS`): a capture run's `TREK_SHOT_DIR` has to follow it.
pub const MOVE_KEEP_ENV: &str = "TREK_RELOCATE_KEEP";

/// What the Trek that moved leaves for the one that runs from the copy: two lines, the folder it
/// left and the folder it went to.
const MOVED_MARKER: &str = "relocated-from";
/// The old copy, while it is still offered for removal: the folder it left and the one it went to,
/// as the marker has them, kept after the marker is read so that a Trek restarted before the old
/// copy was dealt with still offers it. Removed with the old copy, or when the user dismisses it.
const OLD_COPY_NOTE: &str = "old-copy";
/// A file mid-copy is called this until its copy is checked.
const PART: &str = ".relocating";
/// How many folders (`Trek`, `Trek-2`, …) are tried before giving up.
const NAMES_TRIED: u32 = 99;

/// The folder Trek moves to: `TREK_RELOCATE_TO`, else `Programs\Trek` in the user's local
/// application data, where Windows keeps the programs a user installs for themselves.
pub fn destination() -> Option<PathBuf> {
    destination_in(std::env::var_os(MOVE_TO_ENV), local_app_data())
}

fn destination_in(over: Option<std::ffi::OsString>, local: Option<PathBuf>) -> Option<PathBuf> {
    over.map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| local.map(|l| l.join("Programs").join("Trek")))
}

fn local_app_data() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| directories::BaseDirs::new().map(|d| d.data_local_dir().to_path_buf()))
}

/// `destination` as the Updates page says it: no disk, so it can ask on every frame.
pub fn destination_hint() -> String {
    destination()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| r"C:\Users\<you>\AppData\Local\Programs\Trek".into())
}

/// What the Updates page says under why Trek can't update itself.
pub fn moves_line() -> String {
    format!(
        "Trek will move itself to {} and start from there.",
        destination_hint()
    )
}

/// The folder Trek runs from.
pub fn running_from() -> Option<PathBuf> {
    install_root()
}

/// Trek's files, copied and checked in their new folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Copy this Trek's files to the folder it moves to and check them. Reads the folder it runs from
/// and writes only the new one; on any failure the new one is as it was.
pub fn copy_out() -> Result<Prepared> {
    if !cfg!(windows) {
        bail!("moving Trek is for Windows");
    }
    let from = install_root().context("Trek can't tell which folder it runs from")?;
    let wanted = destination().context("Trek can't tell where your programs folder is")?;
    // Trek that already runs from the folder it would move to (and can't update there: it holds
    // more than Trek's files, or the folder is read-only) moves to a free name beside it.
    let beside = same_folder(&wanted, &from);
    let prepared = copy_out_with(&from, &wanted, beside, &in_use_or_newer)?;
    tracing::info!(
        "relocation: copied Trek's files from {} to {}",
        prepared.from.display(),
        prepared.to.display()
    );
    Ok(prepared)
}

/// `copy_out` from `from` to `wanted`, or the free name beside it. `keep` says an own folder of
/// Trek's is not to be replaced.
#[cfg(test)]
fn copy_out_from(from: &Path, wanted: &Path, keep: &dyn Fn(&Path) -> bool) -> Result<Prepared> {
    copy_out_with(from, wanted, false, keep)
}

/// `copy_out_from`; with `beside`, `wanted` being the folder Trek runs from is no refusal: the copy
/// goes to the free name beside it (`wanted-2`, …) and never into `wanted` itself.
fn copy_out_with(
    from: &Path,
    wanted: &Path,
    beside: bool,
    keep: &dyn Fn(&Path) -> bool,
) -> Result<Prepared> {
    // `trek-update.exe` is what starts the copy once this Trek has quit.
    for needed in [TREK_EXE, HELPER_EXE] {
        if !from.join(needed).is_file() {
            bail!("{needed} isn't in {}", from.display());
        }
    }
    let running_there = same_folder(wanted, from);
    if running_there && !beside {
        bail!("Trek runs from {} already", from.display());
    }
    if !running_there && (inside(wanted, from) || inside(from, wanted)) {
        bail!(
            "{} and {} are one inside the other",
            wanted.display(),
            from.display()
        );
    }
    let to = choose_destination(wanted, if running_there { 2 } else { 1 }, keep)?;
    let fresh = !to.exists();
    std::fs::create_dir_all(&to).with_context(|| format!("couldn't make {}", to.display()))?;
    if let Err(e) = copy_files(from, &to) {
        clear_parts(&to);
        // A folder this made holds nothing but what was copied into it.
        if fresh {
            let _ = std::fs::remove_dir_all(&to);
        }
        return Err(e);
    }
    Ok(Prepared {
        from: from.to_path_buf(),
        to,
    })
}

/// `wanted`, or the first of `wanted-2`, `wanted-3`, … that can take the copy: not there yet, or
/// empty, or an own folder of Trek's that `keep` lets go. `first` is the number to start at: 2
/// leaves `wanted` out.
fn choose_destination(wanted: &Path, first: u32, keep: &dyn Fn(&Path) -> bool) -> Result<PathBuf> {
    let name = wanted
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Trek".into());
    for n in first..=NAMES_TRIED {
        let candidate = if n == 1 {
            wanted.to_path_buf()
        } else {
            wanted.with_file_name(format!("{name}-{n}"))
        };
        if usable(&candidate, keep) {
            return Ok(candidate);
        }
    }
    bail!("every folder from {name} to {name}-{NAMES_TRIED} next to it is taken")
}

fn usable(dir: &Path, keep: &dyn Fn(&Path) -> bool) -> bool {
    match std::fs::symlink_metadata(dir) {
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        // A link isn't a folder here: `file_type` doesn't follow it.
        Ok(m) if m.file_type().is_dir() => {
            is_empty(dir) || (dir.join(TREK_EXE).is_file() && holds_only_trek(dir) && !keep(dir))
        }
        Ok(_) => false,
    }
}

fn is_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none())
}

/// Only Trek's files, the moment's probe, and what an interrupted copy left.
fn holds_only_trek(dir: &Path) -> bool {
    let Ok(mut entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.all(|e| {
        e.is_ok_and(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let ours = TREK_FILES.contains(&name.as_str())
                || name.starts_with(".trek-write-test-")
                || name
                    .strip_suffix(PART)
                    .is_some_and(|f| TREK_FILES.contains(&f));
            e.file_type().is_ok_and(|t| t.is_file()) && ours
        })
    })
}

/// A Trek is running from `dir` (Windows keeps a running program's file from being opened for
/// writing), or what's there is a newer Trek than this one, which a copy of this one must not
/// replace.
fn in_use_or_newer(dir: &Path) -> bool {
    let running = TREK_FILES
        .iter()
        .map(|f| dir.join(f))
        .filter(|p| p.exists())
        .any(|p| std::fs::OpenOptions::new().write(true).open(p).is_err());
    running || exe_version(&dir.join(TREK_EXE)).is_ok_and(|v| v > current_version())
}

fn copy_files(from: &Path, to: &Path) -> Result<()> {
    let names: Vec<&str> = TREK_FILES
        .iter()
        .copied()
        .filter(|f| from.join(f).is_file())
        .collect();
    for name in &names {
        let (src, part) = (from.join(name), to.join(format!("{name}{PART}")));
        std::fs::copy(&src, &part)
            .with_context(|| format!("couldn't copy {name} to {}", to.display()))?;
        verify_copy(&src, &part)
            .with_context(|| format!("the copy of {name} in {} isn't right", to.display()))?;
    }
    for name in &names {
        std::fs::rename(to.join(format!("{name}{PART}")), to.join(name))
            .with_context(|| format!("couldn't put {name} in place in {}", to.display()))?;
    }
    Ok(())
}

/// The copy is as long as the original and hashes the same.
fn verify_copy(original: &Path, copy: &Path) -> Result<()> {
    let (a, b) = (
        std::fs::metadata(original)?.len(),
        std::fs::metadata(copy)?.len(),
    );
    if a != b {
        bail!("it has {b} of {a} bytes");
    }
    if sha256(original)? != sha256(copy)? {
        bail!("it isn't the same as the original");
    }
    Ok(())
}

fn sha256(path: &Path) -> std::io::Result<String> {
    use sha2::Digest as _;
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let (mut hasher, mut buf) = (sha2::Sha256::new(), vec![0u8; 1 << 20]);
    loop {
        match file.read(&mut buf)? {
            0 => return Ok(hex::encode(hasher.finalize())),
            n => hasher.update(&buf[..n]),
        }
    }
}

/// Take away what an interrupted copy left in `dir`.
fn clear_parts(dir: &Path) {
    for name in TREK_FILES {
        let _ = std::fs::remove_file(dir.join(format!("{name}{PART}")));
    }
}

/// Start the copy, and leave the note it reads at launch. The copy's helper waits for this
/// process to exit, so the caller quits right after (with `foreground`: the copy opens in front,
/// as this one was).
pub fn start_copy(prepared: &Prepared, foreground: bool) -> Result<()> {
    start_copy_in(
        prepared,
        foreground,
        &crate::paths::updates_dir(),
        &std::env::vars().collect::<Vec<_>>(),
        &|cmd| spawn_detached(cmd),
    )
}

fn start_copy_in(
    prepared: &Prepared,
    foreground: bool,
    updates: &Path,
    vars: &[(String, String)],
    spawn: &dyn Fn(&mut std::process::Command) -> std::io::Result<()>,
) -> Result<()> {
    let helper = prepared.to.join(HELPER_EXE);
    let marker = updates.join(MOVED_MARKER);
    std::fs::write(
        &marker,
        format!("{}\n{}\n", prepared.from.display(), prepared.to.display()),
    )
    .context("couldn't leave a note for the copy")?;
    let mut cmd = mover_command(prepared, std::process::id(), updates, foreground, vars);
    if let Err(e) = spawn(&mut cmd) {
        let _ = std::fs::remove_file(&marker);
        return Err(e).with_context(|| format!("couldn't start {}", helper.display()));
    }
    // What the last update said went wrong is about the folder Trek has left.
    let _ = std::fs::remove_file(updates.join(INSTALL_FAILED));
    tracing::info!(
        "relocation: {} starts {} once this Trek has quit",
        helper.display(),
        prepared.to.join(TREK_EXE).display()
    );
    Ok(())
}

/// The copy's `trek-update` with no update to swap in: it waits for `pid` to exit and starts the
/// copy (`--relaunch`), which keeps this one's setup like an update's restart does.
fn mover_command(
    prepared: &Prepared,
    pid: u32,
    updates: &Path,
    foreground: bool,
    vars: &[(String, String)],
) -> std::process::Command {
    let mut cmd = helper_command(
        &prepared.to.join(HELPER_EXE),
        pid,
        &prepared.to,
        None,
        "",
        updates,
        Some(foreground),
        vars.iter().cloned(),
    );
    let keep = vars
        .iter()
        .find(|(k, _)| k == MOVE_KEEP_ENV)
        .map(|(_, v)| v.as_str())
        .unwrap_or_default();
    for name in keep
        .split(',')
        .map(str::trim)
        .filter(|n| n.starts_with("TREK_"))
    {
        if let Some((k, v)) = vars.iter().find(|(k, _)| k == name) {
            cmd.env(k, v);
        }
    }
    cmd
}

/// Where Trek was, as the Trek that runs from the copy finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Once, at launch: the note a Trek that moved left, if it is this one that runs from the folder
/// it went to. (Another Trek sharing the data folder, a build run from a checkout, leaves it for
/// the one it names.)
pub fn take_relocation() -> Option<Moved> {
    take_relocation_in(&crate::paths::updates_dir(), &install_root()?)
}

fn take_relocation_in(updates: &Path, here: &Path) -> Option<Moved> {
    let marker = updates.join(MOVED_MARKER);
    let text = std::fs::read_to_string(&marker).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let (Some(from), Some(to)) = (lines.next(), lines.next()) else {
        let _ = std::fs::remove_file(&marker);
        return None;
    };
    let (from, to) = (PathBuf::from(from), PathBuf::from(to));
    if !same_folder(&to, here) {
        return None;
    }
    let _ = std::fs::remove_file(&marker);
    Some(Moved { from, to }).filter(|m| !same_folder(&m.from, here))
}

fn same_folder(a: &Path, b: &Path) -> bool {
    inside(a, b) && inside(b, a)
}

/// What `at_launch` found: where Trek moved from, and whether this launch is the first since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocated {
    pub moved: Moved,
    /// The note the move left was read now (not an earlier launch's): the time to point the user's
    /// shortcuts at this copy.
    pub fresh: bool,
}

/// At launch: the move that brought Trek here, and the old copy that still stands. The note of the
/// move (`take_relocation`) is read once, so the old copy is kept in a note of its own until it is
/// dealt with (removed, or dismissed on the Updates page) and offered again at each launch.
pub fn at_launch() -> Option<Relocated> {
    at_launch_in(&crate::paths::updates_dir(), &install_root()?)
}

fn at_launch_in(updates: &Path, here: &Path) -> Option<Relocated> {
    if let Some(moved) = take_relocation_in(updates, here) {
        let _ = std::fs::write(
            updates.join(OLD_COPY_NOTE),
            format!("{}\n{}\n", moved.from.display(), moved.to.display()),
        );
        return Some(Relocated { moved, fresh: true });
    }
    old_copy_in(updates, here).map(|moved| Relocated {
        moved,
        fresh: false,
    })
}

/// The old copy an earlier launch of this Trek left a note about, if it still holds Trek's files.
/// A note about another Trek's move (a build run from a checkout, sharing the data folder) is left
/// for it; one that has nothing to offer any more goes.
fn old_copy_in(updates: &Path, here: &Path) -> Option<Moved> {
    let note = updates.join(OLD_COPY_NOTE);
    let text = std::fs::read_to_string(&note).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let moved = lines.next().zip(lines.next()).map(|(from, to)| Moved {
        from: PathBuf::from(from),
        to: PathBuf::from(to),
    });
    let Some(moved) = moved else {
        let _ = std::fs::remove_file(&note);
        return None;
    };
    if !same_folder(&moved.to, here) {
        return None;
    }
    if inside(here, &moved.from) || inside(&moved.from, here) || !old_copy_remains(&moved.from) {
        let _ = std::fs::remove_file(&note);
        return None;
    }
    Some(moved)
}

/// `old` still holds a copy of Trek: a folder with `trek.exe` in it, not a link to one.
pub fn old_copy_remains(old: &Path) -> bool {
    !is_link(old) && old.join(TREK_EXE).is_file()
}

/// The old copy is dealt with (or the user doesn't want to hear about it): no more offers at launch.
pub fn forget_old_copy() {
    let _ = std::fs::remove_file(crate::paths::updates_dir().join(OLD_COPY_NOTE));
}

/// `trek.exe` in a folder of Trek's, as a shortcut's target says it.
pub fn exe_in(folder: &Path) -> PathBuf {
    folder.join(TREK_EXE)
}

/// A folder where a user's shortcuts to Trek may be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutFolder {
    /// Where it is, as a sentence says it ("on your Desktop").
    pub place: &'static str,
    pub dir: PathBuf,
    /// How many levels of folders inside it hold shortcuts too.
    pub depth: u32,
}

/// The user's own Start menu, Desktop and taskbar pins, where Windows says they are. (Not the
/// ones every user shares: those are the administrator's to change.)
pub fn shortcut_folders() -> Vec<ShortcutFolder> {
    // The same way the guarded folders are found (`SHGetKnownFolderPath`, through `directories`).
    let roaming = directories::BaseDirs::new().map(|d| d.data_dir().to_path_buf());
    let desktop = directories::UserDirs::new().and_then(|d| d.desktop_dir().map(Path::to_path_buf));
    shortcut_folders_in(roaming, desktop)
}

fn shortcut_folders_in(roaming: Option<PathBuf>, desktop: Option<PathBuf>) -> Vec<ShortcutFolder> {
    let mut folders = vec![];
    if let Some(roaming) = roaming.filter(|p| p.is_absolute()) {
        let microsoft = roaming.join("Microsoft");
        folders.push(ShortcutFolder {
            place: "in your Start menu",
            dir: microsoft
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
            depth: 2,
        });
        folders.push(ShortcutFolder {
            place: "pinned to your taskbar",
            dir: microsoft
                .join("Internet Explorer")
                .join("Quick Launch")
                .join("User Pinned")
                .join("TaskBar"),
            depth: 0,
        });
    }
    if let Some(desktop) = desktop.filter(|p| p.is_absolute()) {
        folders.push(ShortcutFolder {
            place: "on your Desktop",
            dir: desktop,
            depth: 1,
        });
    }
    folders
}

/// What became of the old copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    Removed,
    /// Not there any more.
    Gone,
    /// Not Trek's any more, or where Trek runs now: left alone, and why.
    Kept(String),
    /// Windows wouldn't delete it (its ransomware protection, a program holding a file).
    Refused,
}

/// Delete the copy of Trek in `old`, now that Trek runs from `here`. A folder of Trek's own goes
/// whole; one that holds more (Trek unzipped into Downloads) loses only Trek's files. Anything
/// that isn't Trek's, or is where Trek runs, stays.
pub fn remove_old_copy(old: &Path, here: &Path) -> Removal {
    if inside(here, old) || inside(old, here) {
        return Removal::Kept(format!(
            "{} is where Trek runs from now, so Trek left it alone.",
            old.display()
        ));
    }
    if std::fs::symlink_metadata(old).is_err() {
        return Removal::Gone;
    }
    if is_link(old) || !old.join(TREK_EXE).is_file() {
        return Removal::Kept(format!(
            "{} isn't a folder of Trek's any more, so Trek left it alone.",
            old.display()
        ));
    }
    let removed = if own_folder(old) {
        std::fs::remove_dir_all(old)
    } else {
        TREK_FILES
            .iter()
            .map(|f| old.join(f))
            .filter(|p| p.is_file())
            .try_for_each(std::fs::remove_file)
    };
    match removed {
        Ok(()) => {
            // What's left of a folder that held more than Trek stays as it is; an empty one goes.
            let _ = std::fs::remove_dir(old);
            tracing::info!("relocation: removed the old copy in {}", old.display());
            Removal::Removed
        }
        Err(e) => {
            tracing::warn!(
                "relocation: couldn't remove the old copy in {}: {e}",
                old.display()
            );
            Removal::Refused
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trek-relocate-test-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A Windows install: Trek's three files, each holding `tag` (so folders tell apart).
    fn trek_folder(dir: &Path, tag: &str) {
        std::fs::create_dir_all(dir).unwrap();
        for f in TREK_FILES {
            std::fs::write(dir.join(f), format!("{f} {tag}")).unwrap();
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn contents(dir: &Path, file: &str) -> String {
        std::fs::read_to_string(dir.join(file)).unwrap()
    }

    const NOT_IN_USE: &dyn Fn(&Path) -> bool = &|_| false;

    #[test]
    fn trek_moves_to_the_programs_folder_unless_told_otherwise() {
        let local = PathBuf::from(if cfg!(windows) {
            r"C:\Users\me\AppData\Local"
        } else {
            "/home/me/.local"
        });
        assert_eq!(
            destination_in(None, Some(local.clone())),
            Some(local.join("Programs").join("Trek"))
        );
        let elsewhere = std::env::temp_dir().join("trek-relocate-dest");
        assert_eq!(
            destination_in(
                Some(elsewhere.clone().into_os_string()),
                Some(local.clone())
            ),
            Some(elsewhere)
        );
        assert_eq!(
            destination_in(Some("relative".into()), Some(local.clone())),
            Some(local.join("Programs").join("Trek")),
            "a folder that isn't a full path is ignored"
        );
        assert_eq!(destination_in(None, None), None);
    }

    #[test]
    fn copying_out_puts_trek_s_files_in_a_new_folder_and_changes_nothing_where_they_were() {
        let dir = scratch("copy");
        let (from, wanted) = (
            dir.join("Documents").join("Trek"),
            dir.join("Programs").join("Trek"),
        );
        trek_folder(&from, "0.4.1");
        std::fs::write(from.join("holiday.jpg"), "x").unwrap();

        let prepared = copy_out_from(&from, &wanted, NOT_IN_USE).unwrap();
        assert_eq!(
            prepared,
            Prepared {
                from: from.clone(),
                to: wanted.clone()
            }
        );
        // Trek's three files, and nothing else: not the photo, no temporary names.
        assert_eq!(
            names(&wanted),
            ["trek-mcp.exe", "trek-update.exe", "trek.exe"]
        );
        for f in TREK_FILES {
            assert_eq!(contents(&wanted, f), contents(&from, f));
        }
        assert_eq!(names(&from).len(), 4, "the source is as it was");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_older_folder_without_the_mcp_server_is_copied_as_it_is() {
        let dir = scratch("older");
        let (from, wanted) = (dir.join("Trek"), dir.join("Programs").join("Trek"));
        trek_folder(&from, "old");
        std::fs::remove_file(from.join("trek-mcp.exe")).unwrap();
        copy_out_from(&from, &wanted, NOT_IN_USE).unwrap();
        assert_eq!(names(&wanted), ["trek-update.exe", "trek.exe"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_folder_without_trek_or_its_helper_is_not_moved() {
        let dir = scratch("missing");
        let (from, wanted) = (dir.join("Trek"), dir.join("Programs").join("Trek"));
        for gone in [TREK_EXE, HELPER_EXE] {
            trek_folder(&from, "x");
            std::fs::remove_file(from.join(gone)).unwrap();
            let e = copy_out_from(&from, &wanted, NOT_IN_USE).unwrap_err();
            assert!(e.to_string().starts_with(gone), "{e}");
            assert!(!wanted.exists(), "nothing was made");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_destination_is_replaced_when_it_is_trek_s_own_and_nothing_runs_from_it() {
        let dir = scratch("replace");
        let (from, wanted) = (dir.join("Trek"), dir.join("Programs").join("Trek"));
        trek_folder(&from, "new");
        trek_folder(&wanted, "old");
        let prepared = copy_out_from(&from, &wanted, NOT_IN_USE).unwrap();
        assert_eq!(prepared.to, wanted);
        assert_eq!(contents(&wanted, "trek.exe"), "trek.exe new");
        assert_eq!(
            names(&wanted),
            ["trek-mcp.exe", "trek-update.exe", "trek.exe"]
        );
        // An empty folder, too: nothing in it to lose.
        let empty = dir.join("Programs").join("Empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(copy_out_from(&from, &empty, NOT_IN_USE).unwrap().to, empty);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_destination_that_is_in_use_or_isnt_trek_s_gets_a_free_name_beside_it() {
        let dir = scratch("names");
        let (from, wanted) = (dir.join("Trek"), dir.join("Programs").join("Trek"));
        trek_folder(&from, "new");
        // Something runs from it (or it's newer): kept as it is.
        trek_folder(&wanted, "running");
        let busy = |d: &Path| d == wanted;
        let first = copy_out_from(&from, &wanted, &busy).unwrap();
        assert_eq!(first.to, dir.join("Programs").join("Trek-2"));
        assert_eq!(contents(&wanted, "trek.exe"), "trek.exe running");
        // And the next time: Trek-2 is Trek's own and free, so it is the one replaced.
        assert_eq!(copy_out_from(&from, &wanted, &busy).unwrap().to, first.to);
        // Trek-2 busy too, and a folder of someone else's: Trek-3, Trek-4.
        let busy_2 = |d: &Path| d == wanted || d == first.to;
        assert_eq!(
            copy_out_from(&from, &wanted, &busy_2).unwrap().to,
            dir.join("Programs").join("Trek-3")
        );
        std::fs::write(
            dir.join("Programs").join("Trek-3").join("notes.txt"),
            "mine",
        )
        .unwrap();
        assert_eq!(
            copy_out_from(&from, &wanted, &busy_2).unwrap().to,
            dir.join("Programs").join("Trek-4")
        );
        assert_eq!(
            contents(&dir.join("Programs").join("Trek-3"), "notes.txt"),
            "mine"
        );
        // A file by the name is no place either.
        let file = dir.join("Other");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(
            copy_out_from(&from, &file, NOT_IN_USE).unwrap().to,
            dir.join("Other-2")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn trek_that_cannot_update_in_the_destination_moves_to_a_name_beside_it() {
        let dir = scratch("beside");
        let from = dir.join("Programs").join("Trek");
        trek_folder(&from, "x");
        // The reason it can't update there: a file that isn't Trek's.
        std::fs::write(from.join("notes.txt"), "mine").unwrap();

        // Asked for the folder it runs from, with nothing said about it: refused, as before.
        assert!(copy_out_from(&from, &from, NOT_IN_USE).is_err());
        let moved = copy_out_with(&from, &from, true, NOT_IN_USE).unwrap();
        assert_eq!(moved.to, dir.join("Programs").join("Trek-2"));
        assert_eq!(
            names(&moved.to),
            ["trek-mcp.exe", "trek-update.exe", "trek.exe"]
        );
        assert_eq!(names(&from).len(), 4, "where Trek runs is only read");
        // Again: Trek-2 is Trek's own and nothing runs from it, so it is replaced.
        assert_eq!(
            copy_out_with(&from, &from, true, NOT_IN_USE).unwrap().to,
            moved.to
        );
        // Busy: the next free name.
        let busy = |d: &Path| d == moved.to;
        assert_eq!(
            copy_out_with(&from, &from, true, &busy).unwrap().to,
            dir.join("Programs").join("Trek-3")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn trek_does_not_move_into_itself_or_out_of_itself() {
        let dir = scratch("inside");
        let from = dir.join("Trek");
        trek_folder(&from, "x");
        let e = copy_out_from(&from, &from, NOT_IN_USE).unwrap_err();
        assert!(
            e.to_string().starts_with("Trek runs from") && e.to_string().ends_with("already"),
            "{e}"
        );
        for wanted in [from.join("sub"), dir.clone()] {
            let e = copy_out_from(&from, &wanted, NOT_IN_USE).unwrap_err();
            assert!(e.to_string().ends_with("one inside the other"), "{e}");
        }
        assert_eq!(names(&from).len(), 3, "nothing was made");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Until dropped, nothing can read (or delete) `file`: held without sharing on Windows, no
    /// permissions elsewhere.
    struct Unreadable {
        path: PathBuf,
        _held: Option<std::fs::File>,
    }

    impl Unreadable {
        fn new(path: &Path) -> Self {
            #[cfg(windows)]
            let held = {
                use std::os::windows::fs::OpenOptionsExt as _;
                Some(
                    std::fs::OpenOptions::new()
                        .read(true)
                        .share_mode(0)
                        .open(path)
                        .unwrap(),
                )
            };
            #[cfg(not(windows))]
            let held = {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
                None
            };
            Unreadable {
                path: path.to_path_buf(),
                _held: held,
            }
        }

        /// It really is out of reach (a test run as root reads anything).
        fn holds(&self) -> bool {
            std::fs::File::open(&self.path).is_err()
        }
    }

    #[cfg(not(windows))]
    impl Drop for Unreadable {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o644));
        }
    }

    #[test]
    fn a_copy_that_fails_leaves_the_source_alone_and_takes_back_what_it_made() {
        let dir = scratch("fail");
        let (from, wanted) = (dir.join("Trek"), dir.join("Programs").join("Trek"));
        trek_folder(&from, "new");
        // The last file can't be read: the others are copied by then.
        let guard = Unreadable::new(&from.join("trek-update.exe"));
        if !guard.holds() {
            return;
        }

        // A folder made for the copy goes again, with what it had copied.
        let e = copy_out_from(&from, &wanted, NOT_IN_USE).unwrap_err();
        assert!(
            e.to_string().contains("couldn't copy trek-update.exe"),
            "{e:#}"
        );
        assert!(!wanted.exists());

        // One that was there keeps what it held, and no half-made copies.
        trek_folder(&wanted, "old");
        assert!(copy_out_from(&from, &wanted, NOT_IN_USE).is_err());
        assert_eq!(
            names(&wanted),
            ["trek-mcp.exe", "trek-update.exe", "trek.exe"]
        );
        for f in TREK_FILES {
            assert_eq!(contents(&wanted, f), format!("{f} old"));
        }
        drop(guard);
        for f in TREK_FILES {
            assert_eq!(
                contents(&from, f),
                format!("{f} new"),
                "the source is as it was"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_copy_is_checked_against_the_original() {
        let dir = scratch("verify");
        let (a, b, c) = (dir.join("a"), dir.join("b"), dir.join("c"));
        std::fs::write(&a, "trek 0.4.1").unwrap();
        std::fs::write(&b, "trek 0.4.1").unwrap();
        std::fs::write(&c, "trek 0.4.2").unwrap();
        assert!(verify_copy(&a, &b).is_ok());
        let same_length = verify_copy(&a, &c).unwrap_err();
        assert!(
            same_length.to_string().contains("isn't the same"),
            "{same_length}"
        );
        std::fs::write(&c, "trek").unwrap();
        let shorter = verify_copy(&a, &c).unwrap_err();
        assert!(shorter.to_string().contains("4 of 10 bytes"), "{shorter}");
        assert_eq!(sha256(&a).unwrap(), sha256(&b).unwrap());
        // SHA-256 of "abc".
        std::fs::write(&c, "abc").unwrap();
        assert_eq!(
            sha256(&c).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn env_of(cmd: &std::process::Command, key: &str) -> Option<Option<String>> {
        cmd.get_envs()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    fn args_of(cmd: &std::process::Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_copy_is_started_by_its_own_helper_with_trek_s_setup_but_not_the_one_time_flags() {
        let dir = scratch("command");
        let prepared = Prepared {
            from: dir.join("Documents").join("Trek"),
            to: dir.join("Programs").join("Trek"),
        };
        let env = vars(&[
            ("TREK_DATA_DIR", "D:/data"),
            ("TREK_MOCK_AGENT", "1"),
            ("TREK_RELOCATE_TO", "D:/dest"),
            ("TREK_SHOT_DIR", "D:/shots"),
            ("TREK_MOCK_PROMPT", "tools"),
            ("PATH", "x"),
        ]);
        let cmd = mover_command(&prepared, 4242, &dir.join("updates"), false, &env);
        assert_eq!(
            cmd.get_program(),
            prepared.to.join(HELPER_EXE).as_os_str(),
            "the copy's helper, never the original's"
        );
        let args = args_of(&cmd);
        assert_eq!(args[..2], ["--pid", "4242"]);
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--install" && Path::new(&w[1]) == prepared.to),
            "{args:?}"
        );
        assert!(args.contains(&"--relaunch".to_string()), "{args:?}");
        assert!(
            !args.contains(&"--staged".to_string()),
            "nothing to swap: only the start"
        );
        assert_eq!(
            env_of(&cmd, "TREK_RELOCATE_TO"),
            Some(None),
            "the copy doesn't move again"
        );
        assert_eq!(env_of(&cmd, "TREK_SHOT_DIR"), Some(None));
        assert_eq!(env_of(&cmd, "TREK_MOCK_PROMPT"), Some(None));
        assert_eq!(
            env_of(&cmd, "TREK_DATA_DIR"),
            None,
            "inherited: the data folder is the same"
        );
        assert_eq!(
            env_of(&cmd, "TREK_BACKGROUND"),
            Some(Some("1".into())),
            "behind other windows, as this one was"
        );
        assert_eq!(
            mover_command(&prepared, 1, &dir, true, &env)
                .get_envs()
                .find(|(k, _)| *k == "TREK_BACKGROUND"),
            None
        );

        // An ordinary install's move: the copy starts with none of Trek's own settings, so it is
        // the user's Trek that registers `trek://` for itself at its launch (`register_trek_protocol`
        // leaves it to a process that has a data folder or a capture folder of its own).
        let plain = mover_command(
            &prepared,
            1,
            &dir,
            true,
            &vars(&[("PATH", "x"), ("USERPROFILE", "C:/Users/me")]),
        );
        assert!(
            plain
                .get_envs()
                .all(|(k, _)| !k.to_string_lossy().starts_with("TREK_")),
            "{:?}",
            plain.get_envs().collect::<Vec<_>>()
        );

        // A capture run asks for its variables to follow it.
        let kept = mover_command(
            &prepared,
            1,
            &dir,
            true,
            &vars(&[
                ("TREK_SHOT_DIR", "D:/shots"),
                ("TREK_RELOCATE_KEEP", "TREK_SHOT_DIR, PATH, TREK_NOT_SET"),
                ("PATH", "x"),
            ]),
        );
        assert_eq!(
            env_of(&kept, "TREK_SHOT_DIR"),
            Some(Some("D:/shots".into()))
        );
        assert_eq!(env_of(&kept, "PATH"), None, "only Trek's own variables");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn starting_the_copy_leaves_a_note_for_it_and_drops_the_last_update_s_complaint() {
        let dir = scratch("start");
        let (updates, prepared) = (
            dir.join("updates"),
            Prepared {
                from: dir.join("Documents").join("Trek"),
                to: dir.join("Programs").join("Trek"),
            },
        );
        std::fs::create_dir_all(&updates).unwrap();
        std::fs::write(
            updates.join(INSTALL_FAILED),
            "Trek 0.4.2 didn't install: ransomware protection.",
        )
        .unwrap();
        let started = std::cell::RefCell::new(vec![]);
        let spawn = |cmd: &mut std::process::Command| {
            started.borrow_mut().push(cmd.get_program().to_owned());
            Ok(())
        };
        start_copy_in(&prepared, true, &updates, &[], &spawn).unwrap();
        assert_eq!(
            *started.borrow(),
            [prepared.to.join(HELPER_EXE).into_os_string()]
        );
        assert!(!updates.join(INSTALL_FAILED).exists());

        // The copy reads it, and only the copy: a Trek elsewhere leaves it.
        assert_eq!(take_relocation_in(&updates, &dir.join("Elsewhere")), None);
        assert!(
            updates.join(MOVED_MARKER).exists(),
            "left for the Trek it names"
        );
        assert_eq!(
            take_relocation_in(&updates, &prepared.to),
            Some(Moved {
                from: prepared.from.clone(),
                to: prepared.to.clone()
            })
        );
        assert_eq!(
            take_relocation_in(&updates, &prepared.to),
            None,
            "said once"
        );

        // A helper that won't start: no note, so no copy is reported that never ran, and the old
        // complaint stays for whoever reads it.
        std::fs::write(updates.join(INSTALL_FAILED), "again").unwrap();
        let refuse = |_: &mut std::process::Command| {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        };
        let e = start_copy_in(&prepared, true, &updates, &[], &refuse).unwrap_err();
        assert!(e.to_string().starts_with("couldn't start"), "{e:#}");
        assert!(!updates.join(MOVED_MARKER).exists());
        assert!(updates.join(INSTALL_FAILED).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_note_is_read_in_windows_s_way_and_a_damaged_one_goes() {
        let dir = scratch("note");
        let (from, to) = (
            dir.join("Documents").join("Trek"),
            dir.join("Programs").join("Trek"),
        );
        let shouting = PathBuf::from(to.to_string_lossy().to_uppercase());
        std::fs::write(
            dir.join(MOVED_MARKER),
            format!("{}\r\n{}\r\n", from.display(), to.display()),
        )
        .unwrap();
        let moved = take_relocation_in(&dir, &shouting).unwrap();
        assert_eq!((moved.from, moved.to), (from.clone(), to.clone()));
        // Nothing to say when this Trek is the one that was left.
        std::fs::write(
            dir.join(MOVED_MARKER),
            format!("{}\n{}\n", to.display(), to.display()),
        )
        .unwrap();
        assert_eq!(take_relocation_in(&dir, &to), None);
        assert!(!dir.join(MOVED_MARKER).exists());
        std::fs::write(dir.join(MOVED_MARKER), "just one line\n").unwrap();
        assert_eq!(take_relocation_in(&dir, &to), None);
        assert!(!dir.join(MOVED_MARKER).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_old_copy_is_offered_again_at_the_next_launch_until_it_is_dealt_with() {
        let dir = scratch("old-copy");
        let (updates, here) = (dir.join("updates"), dir.join("Programs").join("Trek"));
        let from = dir.join("Documents").join("Trek");
        std::fs::create_dir_all(&updates).unwrap();
        trek_folder(&from, "old");
        trek_folder(&here, "new");
        let moved = Moved {
            from: from.clone(),
            to: here.clone(),
        };
        std::fs::write(
            updates.join(MOVED_MARKER),
            format!("{}\n{}\n", from.display(), here.display()),
        )
        .unwrap();

        // The launch that reads the move: fresh, so the shortcuts are pointed here.
        assert_eq!(
            at_launch_in(&updates, &here),
            Some(Relocated {
                moved: moved.clone(),
                fresh: true
            })
        );
        assert!(
            !updates.join(MOVED_MARKER).exists(),
            "the move is said once"
        );
        // The next launches: the old copy still stands, and is offered, but nothing is fresh.
        for _ in 0..2 {
            assert_eq!(
                at_launch_in(&updates, &here),
                Some(Relocated {
                    moved: moved.clone(),
                    fresh: false
                })
            );
        }
        // Another Trek sharing the data folder leaves the note for the one it names.
        assert_eq!(at_launch_in(&updates, &dir.join("Checkout")), None);
        assert!(updates.join(OLD_COPY_NOTE).exists());

        // Dismissed or removed (`forget_old_copy`): no more offers.
        std::fs::remove_file(updates.join(OLD_COPY_NOTE)).unwrap();
        assert_eq!(at_launch_in(&updates, &here), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_old_copy_with_nothing_left_to_offer_is_forgotten() {
        let dir = scratch("old-copy-gone");
        let (updates, here) = (dir.join("updates"), dir.join("Programs").join("Trek"));
        let from = dir.join("Documents").join("Trek");
        std::fs::create_dir_all(&updates).unwrap();
        trek_folder(&here, "new");
        let note = |from: &Path, to: &Path| {
            std::fs::write(
                updates.join(OLD_COPY_NOTE),
                format!("{}\n{}\n", from.display(), to.display()),
            )
            .unwrap()
        };

        // Removed by hand (or by the toast of an earlier run): not there.
        note(&from, &here);
        assert_eq!(at_launch_in(&updates, &here), None);
        assert!(!updates.join(OLD_COPY_NOTE).exists());
        // A folder with something else in it isn't Trek's any more.
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("notes.txt"), "mine").unwrap();
        note(&from, &here);
        assert_eq!(at_launch_in(&updates, &here), None);
        assert!(!updates.join(OLD_COPY_NOTE).exists());
        // Trek runs from the folder it once left (moved back, or the note is stale).
        trek_folder(&from, "old");
        note(&from, &here);
        assert_eq!(at_launch_in(&updates, &from), None, "not this Trek's note");
        assert!(
            updates.join(OLD_COPY_NOTE).exists(),
            "left for the Trek it names"
        );
        note(&from, &from);
        assert_eq!(at_launch_in(&updates, &from), None);
        assert!(
            !updates.join(OLD_COPY_NOTE).exists(),
            "the offer would be to remove where Trek runs"
        );
        // A damaged note goes.
        std::fs::write(updates.join(OLD_COPY_NOTE), "one line\n").unwrap();
        assert_eq!(at_launch_in(&updates, &here), None);
        assert!(!updates.join(OLD_COPY_NOTE).exists());
        assert!(old_copy_remains(&from) && !old_copy_remains(&dir.join("nowhere")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_users_own_shortcut_folders_are_the_start_menu_the_desktop_and_the_taskbar_pins() {
        let (roaming, desktop) = (
            std::env::temp_dir().join("Roaming"),
            std::env::temp_dir().join("OneDrive").join("Desktop"),
        );
        let folders = shortcut_folders_in(Some(roaming.clone()), Some(desktop.clone()));
        let found: Vec<_> = folders
            .iter()
            .map(|f| (f.place, f.dir.clone(), f.depth))
            .collect();
        let start = roaming
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs");
        let pinned = roaming
            .join("Microsoft")
            .join("Internet Explorer")
            .join("Quick Launch")
            .join("User Pinned")
            .join("TaskBar");
        assert_eq!(
            found,
            [
                ("in your Start menu", start, 2),
                ("pinned to your taskbar", pinned, 0),
                ("on your Desktop", desktop, 1)
            ]
        );
        // What Windows doesn't say, or says without a full path, isn't searched.
        assert!(shortcut_folders_in(None, Some(PathBuf::from("Desktop"))).is_empty());
        assert!(shortcut_folders_in(None, None).is_empty());
    }

    #[test]
    fn the_old_copy_goes_whole_when_it_is_trek_s_own_and_only_trek_s_files_when_it_is_not() {
        let dir = scratch("remove");
        let here = dir.join("Programs").join("Trek");
        trek_folder(&here, "new");
        let (own, shared) = (dir.join("Documents").join("Trek"), dir.join("Downloads"));
        trek_folder(&own, "old");
        assert_eq!(remove_old_copy(&own, &here), Removal::Removed);
        assert!(!own.exists());
        assert_eq!(remove_old_copy(&own, &here), Removal::Gone);

        trek_folder(&shared, "old");
        std::fs::write(shared.join("holiday.jpg"), "x").unwrap();
        assert_eq!(remove_old_copy(&shared, &here), Removal::Removed);
        assert_eq!(names(&shared), ["holiday.jpg"], "what isn't Trek's stays");
        // Trek's three files and nothing else: the folder goes with them.
        trek_folder(&shared, "old");
        std::fs::remove_file(shared.join("holiday.jpg")).unwrap();
        assert_eq!(remove_old_copy(&shared, &here), Removal::Removed);
        assert!(!shared.exists());
        assert_eq!(names(&here).len(), 3, "where Trek runs is never touched");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn what_is_not_trek_s_or_is_where_trek_runs_is_left_alone() {
        let dir = scratch("keep");
        let here = dir.join("Programs").join("Trek");
        trek_folder(&here, "new");
        for old in [here.clone(), here.join("sub"), dir.join("Programs")] {
            assert!(
                matches!(remove_old_copy(&old, &here), Removal::Kept(_)),
                "{old:?}"
            );
        }
        assert_eq!(names(&here).len(), 3);
        // A folder that has no trek.exe any more isn't one to delete from.
        let other = dir.join("Other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("notes.txt"), "mine").unwrap();
        let Removal::Kept(why) = remove_old_copy(&other, &here) else {
            panic!("kept")
        };
        assert!(why.contains("isn't a folder of Trek's"), "{why}");
        assert_eq!(names(&other), ["notes.txt"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[cfg(windows)]
    fn an_old_copy_windows_wont_delete_is_reported_as_refused() {
        let dir = scratch("refused");
        let (here, old) = (
            dir.join("Programs").join("Trek"),
            dir.join("Documents").join("Trek"),
        );
        trek_folder(&here, "new");
        trek_folder(&old, "old");
        // The program running from it, or Defender, holds a file: a delete is turned away.
        let held = Unreadable::new(&old.join("trek.exe"));
        assert_eq!(remove_old_copy(&old, &here), Removal::Refused);
        assert!(old.join("trek.exe").exists());
        drop(held);
        assert_eq!(remove_old_copy(&old, &here), Removal::Removed);
        let _ = std::fs::remove_dir_all(dir);
    }
}
