//! Shortcuts (`.lnk` files) a user made to a Trek that has since moved (`trek_core::update::relocate`):
//! the copy at the new folder points the ones that start the old `trek.exe` at itself, so the next
//! click on the Start menu, the Desktop or the taskbar doesn't start the old copy in the folder
//! Trek couldn't update itself in.
//!
//! Only a shortcut whose target is exactly the old `trek.exe` is touched, and only to change where
//! it points; none is made. Reading a shortcut and writing one are the two things that need Windows
//! (`Links`, which tests replace), so what is matched and what is changed is tested anywhere.
// What looks through the folders is used only where there are shortcuts to look at.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};
use trek_core::update::relocate::ShortcutFolder;

/// What a shortcut starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub target: PathBuf,
    /// The folder the program starts in (empty: none was chosen).
    pub dir: PathBuf,
    /// The file the icon is from (empty: the target's own), and which icon in it.
    pub icon: PathBuf,
    pub icon_index: i32,
}

/// Reading and rewriting shortcut files.
pub trait Links {
    fn read(&self, lnk: &Path) -> std::io::Result<Link>;
    fn write(&self, lnk: &Path, link: &Link) -> std::io::Result<()>;
}

/// A shortcut that still starts the old copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Left {
    pub lnk: PathBuf,
    pub place: &'static str,
    /// Windows turned the change away (the Desktop is a folder ransomware protection guards).
    pub refused: bool,
    pub why: String,
}

/// What `retarget_in` did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub changed: Vec<PathBuf>,
    pub left: Vec<Left>,
}

impl Report {
    /// "2 shortcuts now point at the new copy", when any do.
    pub fn changed_clause(&self) -> Option<String> {
        match self.changed.len() {
            0 => None,
            1 => Some("1 shortcut now points at the new copy".into()),
            n => Some(format!("{n} shortcuts now point at the new copy")),
        }
    }

    /// What couldn't be changed, in words, when anything couldn't: the user's to fix by hand.
    pub fn left_sentence(&self) -> Option<String> {
        let named: Vec<String> = self.left.iter().map(|l| format!("\"{}\" {}", l.lnk.file_stem().unwrap_or_default().to_string_lossy(), l.place)).collect();
        let list = match named.as_slice() {
            [] => return None,
            [one] => one.clone(),
            [a, b] => format!("{a} and {b}"),
            [a, b, rest @ ..] => format!("{a}, {b} and {} more", rest.len()),
        };
        let many = named.len() > 1;
        let who = if self.left.iter().all(|l| l.refused) { "Windows wouldn't let Trek change" } else { "Trek couldn't change" };
        Some(format!("{who} the shortcut{} {list}, so {} still start{} the old copy.", if many { "s" } else { "" }, if many { "they" } else { "it" }, if many { "" } else { "s" }))
    }
}

/// Point every shortcut under `folders` that starts `old_exe` at `new_exe`.
pub fn retarget_in(folders: &[ShortcutFolder], old_exe: &Path, new_exe: &Path, links: &dyn Links) -> Report {
    let mut report = Report::default();
    for folder in folders {
        for lnk in shortcut_files(folder) {
            // A file that isn't a shortcut Windows can read starts nothing of ours.
            let Ok(link) = links.read(&lnk) else { continue };
            let Some(moved) = moved_link(&link, old_exe, new_exe) else { continue };
            match links.write(&lnk, &moved) {
                Ok(()) => {
                    tracing::info!("relocation: the shortcut {} starts {} now", lnk.display(), new_exe.display());
                    report.changed.push(lnk);
                }
                Err(e) => {
                    tracing::warn!("relocation: couldn't change the shortcut {}: {e}", lnk.display());
                    report.left.push(Left { lnk, place: folder.place, refused: e.kind() == std::io::ErrorKind::PermissionDenied, why: e.to_string() });
                }
            }
        }
    }
    report
}

/// The `.lnk` files in `folder` and in the folders inside it, as many levels down as it says. Links to
/// folders (junctions, symbolic links) aren't followed.
fn shortcut_files(folder: &ShortcutFolder) -> Vec<PathBuf> {
    let (mut found, mut pending) = (vec![], vec![(folder.dir.clone(), 0u32)]);
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() && depth < folder.depth {
                pending.push((path, depth + 1));
            } else if kind.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) && !is_in_the_cloud(&entry) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// A shortcut OneDrive keeps online only (Files On-Demand): reading it would download it, which
/// isn't Trek's to start for a file it has no reason to think is its own.
fn is_in_the_cloud(entry: &std::fs::DirEntry) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        entry.metadata().is_ok_and(|m| is_online_only(m.file_attributes()))
    }
    #[cfg(not(windows))]
    {
        let _ = entry;
        false
    }
}

/// `FILE_ATTRIBUTE_OFFLINE`, `…_RECALL_ON_OPEN` or `…_RECALL_ON_DATA_ACCESS`: the data isn't on the disk.
fn is_online_only(attributes: u32) -> bool {
    const OFFLINE: u32 = 0x0000_1000;
    const RECALL_ON_OPEN: u32 = 0x0004_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    attributes & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
}

/// `link` as it should be once `old_exe` is at `new_exe`; `None` if it starts something else. The
/// folder it starts in and its icon follow only when they were the old copy's: ones the user chose
/// stay.
fn moved_link(link: &Link, old_exe: &Path, new_exe: &Path) -> Option<Link> {
    if !same_path(&link.target, old_exe) {
        return None;
    }
    let (old_dir, new_dir) = (old_exe.parent()?, new_exe.parent()?);
    let dir = if link.dir.as_os_str().is_empty() || same_path(&link.dir, old_dir) { new_dir.to_path_buf() } else { link.dir.clone() };
    let icon = if same_path(&link.icon, old_exe) { new_exe.to_path_buf() } else { link.icon.clone() };
    Some(Link { target: new_exe.to_path_buf(), dir, icon, icon_index: link.icon_index })
}

/// Paths as Windows compares them: without the `\\?\` prefix, either slash, any case, and a
/// path that exists in its long form: the shell hands a shortcut's target back as the long
/// name, while what Trek was started from may be the 8.3 one (`C:\Users\RUNNER~1\…`, as on
/// GitHub's runners), and those name one file.
fn same_path(a: &Path, b: &Path) -> bool {
    let plain = |p: &Path| {
        let long = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let s = long.to_string_lossy();
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        s.replace('/', r"\").trim_end_matches('\\').to_lowercase()
    };
    if a.as_os_str().is_empty() || b.as_os_str().is_empty() {
        return false;
    }
    plain(a) == plain(b)
}

/// Point the user's own shortcuts (Start menu, Desktop, taskbar pins) that start `old_exe` at
/// `new_exe`. Off Windows there are none. Waits a bounded time: a Desktop on a share that has gone
/// away mustn't keep Trek from saying where it runs.
pub fn retarget_user_shortcuts(old_exe: &Path, new_exe: &Path) -> Report {
    #[cfg(windows)]
    {
        let (old, new) = (old_exe.to_path_buf(), new_exe.to_path_buf());
        let (tx, rx) = std::sync::mpsc::channel();
        // COM is for the thread that starts it: one of its own, in the apartment shortcuts want.
        let started = std::thread::Builder::new().name("shortcuts".into()).spawn(move || {
            let report = com::with_com(|| retarget_in(&trek_core::update::relocate::shortcut_folders(), &old, &new, &com::ShellLinks));
            let _ = tx.send(report);
        });
        match started.map(|_| rx.recv_timeout(std::time::Duration::from_secs(20))) {
            Ok(Ok(report)) => return report,
            Ok(Err(e)) => tracing::warn!("relocation: the shortcuts weren't looked through in time: {e}"),
            Err(e) => tracing::warn!("relocation: couldn't look through the shortcuts: {e}"),
        }
    }
    let _ = (old_exe, new_exe);
    Report::default()
}

#[cfg(windows)]
mod com {
    use super::{Link, Links};
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
    use std::path::{Path, PathBuf};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, STGM, STGM_READ, STGM_READWRITE};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HRESULT, HSTRING, Interface as _};

    /// Longer than any path Windows lets a shortcut hold.
    const PATH_CHARS: usize = 32_768;

    /// `work` with COM started on this thread, as an apartment (what the shell's own objects want).
    pub fn with_com<T>(work: impl FnOnce() -> T) -> T {
        // SAFETY: balanced below; a thread already in another mode (`RPC_E_CHANGED_MODE`) still
        // has COM, and isn't ours to uninitialise.
        let started = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        let out = work();
        if started {
            // SAFETY: pairs the successful `CoInitializeEx` above.
            unsafe { CoUninitialize() };
        }
        out
    }

    pub struct ShellLinks;

    fn open(lnk: &Path, mode: STGM) -> windows::core::Result<(IShellLinkW, IPersistFile)> {
        // SAFETY: COM calls on a thread `with_com` started; the objects are released on drop.
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None::<&windows::core::IUnknown>, CLSCTX_INPROC_SERVER)?;
            let file: IPersistFile = link.cast()?;
            file.Load(&HSTRING::from(lnk.as_os_str()), mode)?;
            Ok((link, file))
        }
    }

    fn text(buf: &[u16]) -> PathBuf {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        PathBuf::from(OsString::from_wide(&buf[..len]))
    }

    /// Windows's own failures (`0x8007xxxx`) and the storage ones a shortcut file gives
    /// (`0x8003xxxx`: access denied is 5 in both) as the io errors they are.
    fn io_error(e: windows::core::Error) -> std::io::Error {
        let HRESULT(code) = e.code();
        let code = code as u32;
        match code >> 16 {
            0x8007 | 0x8003 => std::io::Error::from_raw_os_error((code & 0xFFFF) as i32),
            _ => std::io::Error::other(e.to_string()),
        }
    }

    impl Links for ShellLinks {
        fn read(&self, lnk: &Path) -> std::io::Result<Link> {
            let (link, _file) = open(lnk, STGM_READ).map_err(io_error)?;
            let (mut target, mut dir, mut icon) = (vec![0u16; PATH_CHARS], vec![0u16; PATH_CHARS], vec![0u16; PATH_CHARS]);
            let mut icon_index = 0;
            // SAFETY: buffers as long as they say; no find data asked for.
            unsafe {
                link.GetPath(&mut target, std::ptr::null_mut(), 0).map_err(io_error)?;
                link.GetWorkingDirectory(&mut dir).map_err(io_error)?;
                link.GetIconLocation(&mut icon, &mut icon_index).map_err(io_error)?;
            }
            Ok(Link { target: text(&target), dir: text(&dir), icon: text(&icon), icon_index })
        }

        fn write(&self, lnk: &Path, to: &Link) -> std::io::Result<()> {
            let (link, file) = open(lnk, STGM_READWRITE).map_err(io_error)?;
            let wide = |p: &Path| HSTRING::from_wide(&p.as_os_str().encode_wide().collect::<Vec<u16>>());
            // SAFETY: strings that outlive the calls. Everything else the shortcut holds (its
            // arguments, hotkey, window state, properties) stays as it was loaded.
            unsafe {
                link.SetPath(&wide(&to.target)).map_err(io_error)?;
                link.SetWorkingDirectory(&wide(&to.dir)).map_err(io_error)?;
                link.SetIconLocation(&wide(&to.icon), to.icon_index).map_err(io_error)?;
                file.Save(&HSTRING::from(lnk.as_os_str()), true).map_err(io_error)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn scratch(name: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("trek-shortcuts-test-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn folder(dir: &Path, depth: u32) -> ShortcutFolder {
        ShortcutFolder { place: "on your Desktop", dir: dir.to_path_buf(), depth }
    }

    /// Shortcuts as a map: what each starts, and which Windows would turn away.
    #[derive(Default)]
    struct Fake {
        links: RefCell<HashMap<PathBuf, Link>>,
        refuses: Vec<PathBuf>,
    }

    impl Fake {
        fn make(&self, lnk: &Path, target: &Path) {
            std::fs::write(lnk, "lnk").unwrap();
            let dir = target.parent().unwrap().to_path_buf();
            self.links.borrow_mut().insert(lnk.to_path_buf(), Link { target: target.to_path_buf(), dir, icon: PathBuf::new(), icon_index: 0 });
        }
        fn target(&self, lnk: &Path) -> PathBuf {
            self.links.borrow()[lnk].target.clone()
        }
    }

    impl Links for Fake {
        fn read(&self, lnk: &Path) -> std::io::Result<Link> {
            self.links.borrow().get(lnk).cloned().ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidData))
        }
        fn write(&self, lnk: &Path, link: &Link) -> std::io::Result<()> {
            if self.refuses.iter().any(|r| r == lnk) {
                // What `io_error` makes of Windows' access-denied; raw code 5 is EIO on Unix.
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            self.links.borrow_mut().insert(lnk.to_path_buf(), link.clone());
            Ok(())
        }
    }

    #[test]
    fn a_shortcut_to_the_old_exe_points_at_the_new_one_and_no_other_is_touched() {
        let dir = scratch("retarget");
        let (old, new) = (dir.join("Documents").join("Trek").join("trek.exe"), dir.join("Programs").join("Trek").join("trek.exe"));
        let other = dir.join("Elsewhere").join("trek.exe");
        let (desktop, sub) = (dir.join("Desktop"), dir.join("Desktop").join("Agents"));
        std::fs::create_dir_all(&sub).unwrap();
        let fake = Fake::default();
        fake.make(&desktop.join("Trek.lnk"), &old);
        // In a folder inside, and with the extension as Windows has it in any case.
        fake.make(&sub.join("Trek again.LNK"), &old);
        // Another Trek's, and another program's: left as they are.
        fake.make(&desktop.join("Trek (other).lnk"), &other);
        fake.make(&desktop.join("Notes.lnk"), &dir.join("Notes").join("notes.exe"));
        // Not a shortcut at all, and a file that isn't named like one.
        std::fs::write(desktop.join("Trek.txt"), "x").unwrap();
        fake.make(&desktop.join("Trek.url"), &old);

        let report = retarget_in(&[folder(&desktop, 1)], &old, &new, &fake);
        assert_eq!(report.changed, [sub.join("Trek again.LNK"), desktop.join("Trek.lnk")], "in path order");
        assert!(report.left.is_empty());
        assert_eq!(fake.target(&desktop.join("Trek.lnk")), new);
        assert_eq!(fake.target(&sub.join("Trek again.LNK")), new);
        assert_eq!(fake.target(&desktop.join("Trek (other).lnk")), other);
        assert_eq!(fake.target(&desktop.join("Notes.lnk")), dir.join("Notes").join("notes.exe"));
        assert_eq!(fake.target(&desktop.join("Trek.url")), old, "only .lnk files");
        // The folder it starts in follows the exe.
        assert_eq!(fake.links.borrow()[&desktop.join("Trek.lnk")].dir, new.parent().unwrap());
        assert_eq!(report.changed_clause().as_deref(), Some("2 shortcuts now point at the new copy"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn folders_are_searched_only_as_deep_as_they_say() {
        let dir = scratch("depth");
        let (old, new) = (dir.join("a").join("trek.exe"), dir.join("b").join("trek.exe"));
        let deep = dir.join("Programs").join("Vendor").join("Trek");
        std::fs::create_dir_all(&deep).unwrap();
        let fake = Fake::default();
        let (top, middle, bottom) = (dir.join("Programs").join("Trek.lnk"), dir.join("Programs").join("Vendor").join("Trek.lnk"), deep.join("Trek.lnk"));
        for lnk in [&top, &middle, &bottom] {
            fake.make(lnk, &old);
        }
        let report = retarget_in(&[folder(&dir.join("Programs"), 1)], &old, &new, &fake);
        assert_eq!(report.changed.len(), 2, "the folder and the one inside it: {:?}", report.changed);
        assert_eq!(fake.target(&bottom), old, "one level too deep");
        let report = retarget_in(&[folder(&dir.join("Programs"), 2)], &old, &new, &fake);
        assert_eq!(report.changed, [bottom.clone()]);
        assert_eq!(fake.target(&bottom), new);
        // A folder that isn't there (no taskbar pins yet) is nothing to say.
        assert_eq!(retarget_in(&[folder(&dir.join("Nowhere"), 2)], &old, &new, &fake), Report::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn targets_are_compared_as_windows_does() {
        let (old, new) = (Path::new(r"C:\Users\Me\Documents\Trek\trek.exe"), Path::new(r"C:\Users\Me\AppData\Local\Programs\Trek\trek.exe"));
        let link = |target: &str| Link { target: PathBuf::from(target), dir: PathBuf::new(), icon: PathBuf::new(), icon_index: 0 };
        for same in [r"c:\users\me\documents\trek\TREK.EXE", r"\\?\C:\Users\Me\Documents\Trek\trek.exe", "C:/Users/Me/Documents/Trek/trek.exe"] {
            assert_eq!(moved_link(&link(same), old, new).map(|l| l.target), Some(new.to_path_buf()), "{same}");
        }
        for other in [r"C:\Users\Me\Documents\Trek\trek-mcp.exe", r"C:\Users\Me\Documents\Trek 2\trek.exe", r"C:\Users\Me\Documents\Trek\trek.exe.bak", r"C:\Users\Me\Documents\Trek", ""] {
            assert_eq!(moved_link(&link(other), old, new), None, "{other:?}");
        }
        // Nothing to compare when the old exe is no path at all.
        assert_eq!(moved_link(&link(""), Path::new(""), new), None);
    }

    #[test]
    fn a_shortcut_kept_online_only_is_not_downloaded_to_be_looked_at() {
        assert!(is_online_only(0x0040_0020), "a OneDrive placeholder (recall on data access, archive)");
        assert!(is_online_only(0x0004_0000) && is_online_only(0x1000));
        assert!(!is_online_only(0x20), "an ordinary file (archive)");
        assert!(!is_online_only(0x0000_0001 | 0x0000_0020 | 0x0020_0000), "read-only, pinned and kept on the disk: it's there");
        // A file that is on the disk is looked at (the walk asks the real attributes).
        let dir = scratch("online");
        std::fs::write(dir.join("Trek.lnk"), "x").unwrap();
        let entry = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap();
        assert!(!is_in_the_cloud(&entry));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_start_folder_and_icon_follow_only_when_they_were_the_old_copys() {
        // Paths of the machine the test runs on: what is a folder's path is the platform's to say.
        let root = std::env::temp_dir();
        let (old, new) = (root.join("Docs").join("Trek").join("trek.exe"), root.join("Programs").join("Trek").join("trek.exe"));
        let (old_dir, new_dir) = (old.parent().unwrap().to_path_buf(), new.parent().unwrap().to_path_buf());
        let link = |dir: &Path, icon: &Path| Link { target: old.clone(), dir: dir.to_path_buf(), icon: icon.to_path_buf(), icon_index: 3 };
        let shouting = PathBuf::from(old.to_string_lossy().to_uppercase());
        let moved = moved_link(&link(&old_dir, &shouting), &old, &new).unwrap();
        assert_eq!((moved.dir, moved.icon, moved.icon_index), (new_dir.clone(), new.clone(), 3));
        let moved = moved_link(&link(Path::new(""), Path::new("")), &old, &new).unwrap();
        assert_eq!((moved.dir, moved.icon), (new_dir, PathBuf::new()), "no icon of its own: the target's");
        // A folder or icon the user chose is theirs.
        let (projects, icon) = (root.join("Projects"), root.join("Icons").join("trek.ico"));
        let moved = moved_link(&link(&projects, &icon), &old, &new).unwrap();
        assert_eq!((moved.dir, moved.icon), (projects, icon));
    }

    #[test]
    fn a_change_windows_refuses_is_reported_in_words_and_the_rest_go_on() {
        let dir = scratch("refused");
        let (old, new) = (dir.join("a").join("trek.exe"), dir.join("b").join("trek.exe"));
        let (desktop, start) = (dir.join("Desktop"), dir.join("Start"));
        std::fs::create_dir_all(&desktop).unwrap();
        std::fs::create_dir_all(&start).unwrap();
        let fake = Fake { refuses: vec![desktop.join("Trek.lnk")], ..Fake::default() };
        fake.make(&desktop.join("Trek.lnk"), &old);
        fake.make(&start.join("Trek.lnk"), &old);

        let report = retarget_in(&[folder(&desktop, 1), ShortcutFolder { place: "in your Start menu", dir: start.clone(), depth: 0 }], &old, &new, &fake);
        assert_eq!(report.changed, [start.join("Trek.lnk")]);
        assert_eq!(report.left.len(), 1);
        assert!(report.left[0].refused && report.left[0].place == "on your Desktop", "{:?}", report.left);
        assert_eq!(fake.target(&desktop.join("Trek.lnk")), old, "as it was");
        assert_eq!(report.left_sentence().as_deref(), Some("Windows wouldn't let Trek change the shortcut \"Trek\" on your Desktop, so it still starts the old copy."));
        assert_eq!(report.changed_clause().as_deref(), Some("1 shortcut now points at the new copy"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn what_could_not_be_changed_is_listed_in_a_sentence() {
        let left = |name: &str, place: &'static str, refused: bool| Left { lnk: PathBuf::from(name), place, refused, why: String::new() };
        assert_eq!(Report::default().left_sentence(), None);
        assert_eq!(Report::default().changed_clause(), None);
        let two = Report { changed: vec![], left: vec![left("Trek.lnk", "on your Desktop", true), left("Trek beta.lnk", "pinned to your taskbar", true)] };
        assert_eq!(
            two.left_sentence().as_deref(),
            Some("Windows wouldn't let Trek change the shortcuts \"Trek\" on your Desktop and \"Trek beta\" pinned to your taskbar, so they still start the old copy.")
        );
        let four = Report { changed: vec![], left: (0..4).map(|i| left(&format!("T{i}.lnk"), "on your Desktop", i != 2)).collect() };
        assert_eq!(
            four.left_sentence().as_deref(),
            Some("Trek couldn't change the shortcuts \"T0\" on your Desktop, \"T1\" on your Desktop and 2 more, so they still start the old copy."),
            "one that failed another way changes who says it"
        );
    }

    /// Real `.lnk` files, written and read by the shell's own API, in folders of the test's own.
    #[cfg(windows)]
    mod shell {
        use super::*;
        use crate::shortcuts::com::{ShellLinks, with_com};

        /// A shortcut file made the way Explorer makes one.
        fn make(lnk: &Path, target: &Path, dir: &Path, icon: &Path) {
            use std::os::windows::ffi::OsStrExt as _;
            use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
            use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
            use windows::core::{HSTRING, Interface as _};
            let wide = |p: &Path| HSTRING::from_wide(&p.as_os_str().encode_wide().collect::<Vec<u16>>());
            // SAFETY: COM on a thread `with_com` started, strings that outlive the calls.
            unsafe {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None::<&windows::core::IUnknown>, CLSCTX_INPROC_SERVER).unwrap();
                link.SetPath(&wide(target)).unwrap();
                link.SetWorkingDirectory(&wide(dir)).unwrap();
                link.SetArguments(&HSTRING::from("--on-purpose")).unwrap();
                link.SetIconLocation(&wide(icon), 1).unwrap();
                link.cast::<IPersistFile>().unwrap().Save(&wide(lnk), true).unwrap();
            }
        }

        fn arguments(lnk: &Path) -> String {
            use std::os::windows::ffi::OsStrExt as _;
            use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
            use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
            use windows::core::{HSTRING, Interface as _};
            let mut buf = vec![0u16; 1024];
            // SAFETY: as in `make`.
            unsafe {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None::<&windows::core::IUnknown>, CLSCTX_INPROC_SERVER).unwrap();
                link.cast::<IPersistFile>().unwrap().Load(&HSTRING::from_wide(&lnk.as_os_str().encode_wide().collect::<Vec<u16>>()), STGM_READ).unwrap();
                link.GetArguments(&mut buf).unwrap();
            }
            String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap()])
        }

        fn same(a: &Path, b: &Path) -> bool {
            same_path(a, b)
        }

        /// Trek's old and new folders, with an exe in each, and `lnk` shortcuts' folders beside them.
        fn places(name: &str) -> (PathBuf, PathBuf, PathBuf) {
            let dir = scratch(name);
            let (old, new) = (dir.join("Documents").join("Trek"), dir.join("Programs").join("Trek"));
            std::fs::create_dir_all(&old).unwrap();
            std::fs::create_dir_all(&new).unwrap();
            std::fs::write(old.join("trek.exe"), "old").unwrap();
            std::fs::write(new.join("trek.exe"), "new").unwrap();
            (dir, old.join("trek.exe"), new.join("trek.exe"))
        }

        #[test]
        fn real_shortcuts_to_the_old_exe_are_pointed_at_the_new_one() {
            let (dir, old, new) = places("com-retarget");
            let (start, sub) = (dir.join("Start Menu"), dir.join("Start Menu").join("Trek"));
            std::fs::create_dir_all(&sub).unwrap();
            let other = dir.join("Elsewhere").join("tool.exe");
            std::fs::create_dir_all(other.parent().unwrap()).unwrap();
            std::fs::write(&other, "tool").unwrap();
            let folders = [ShortcutFolder { place: "in your Start menu", dir: start.clone(), depth: 2 }];
            let old_dir = old.parent().unwrap();
            let report = with_com(|| {
                make(&start.join("Trek.lnk"), &old, old_dir, &old);
                make(&sub.join("Trek (sub).lnk"), &old, old_dir, &old);
                // Another program's, with an icon of its own and a folder of the user's: untouched.
                make(&start.join("Tool.lnk"), &other, &dir, &other);
                // Starts the old Trek in a folder the user chose, with the icon from elsewhere.
                make(&start.join("Trek in my project.lnk"), &old, &dir, &other);
                retarget_in(&folders, &old, &new, &ShellLinks)
            });
            assert!(report.left.is_empty(), "{:?}", report.left);
            assert_eq!(report.changed.len(), 3, "{:?}", report.changed);

            with_com(|| {
                let links = ShellLinks;
                let new_dir = new.parent().unwrap();
                for name in [start.join("Trek.lnk"), sub.join("Trek (sub).lnk")] {
                    let l = links.read(&name).unwrap();
                    assert!(same(&l.target, &new) && same(&l.dir, new_dir) && same(&l.icon, &new), "{name:?}: {l:?}");
                    assert_eq!(l.icon_index, 1, "which icon in the file");
                    assert_eq!(arguments(&name), "--on-purpose", "what else it held is kept");
                }
                let mine = links.read(&start.join("Trek in my project.lnk")).unwrap();
                assert!(same(&mine.target, &new) && same(&mine.dir, &dir) && same(&mine.icon, &other), "{mine:?}");
                let tool = links.read(&start.join("Tool.lnk")).unwrap();
                assert!(same(&tool.target, &other) && same(&tool.dir, &dir) && same(&tool.icon, &other), "{tool:?}");
            });
            // Said once: a second run finds nothing that starts the old copy.
            assert_eq!(with_com(|| retarget_in(&folders, &old, &new, &ShellLinks)), Report::default());
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn a_shortcut_windows_wont_let_trek_write_is_left_and_said() {
            let (dir, old, new) = places("com-refused");
            let desktop = dir.join("Desktop");
            std::fs::create_dir_all(&desktop).unwrap();
            let folders = [ShortcutFolder { place: "on your Desktop", dir: desktop.clone(), depth: 1 }];
            let (guarded, free) = (desktop.join("Guarded.lnk"), desktop.join("Free.lnk"));
            // What ransomware protection does to a write is a refusal; a read-only file is one too.
            let report = with_com(|| {
                make(&guarded, &old, old.parent().unwrap(), &old);
                make(&free, &old, old.parent().unwrap(), &old);
                let mut permissions = std::fs::metadata(&guarded).unwrap().permissions();
                permissions.set_readonly(true);
                std::fs::set_permissions(&guarded, permissions).unwrap();
                retarget_in(&folders, &old, &new, &ShellLinks)
            });
            assert_eq!(report.changed, [free.clone()]);
            assert_eq!(report.left.len(), 1, "{:?}", report.left);
            assert!(report.left[0].refused && report.left[0].lnk == guarded, "{:?}", report.left[0]);
            with_com(|| {
                assert!(same(&ShellLinks.read(&guarded).unwrap().target, &old), "left as it was");
                assert!(same(&ShellLinks.read(&free).unwrap().target, &new));
            });
            // (A read-only file goes with its folder on Windows.)
            let _ = std::fs::remove_dir_all(dir);
        }

        /// The shell gives a shortcut's target back as the long name; Trek may know its old exe
        /// by the 8.3 one (GitHub's runners live in `C:\Users\RUNNER~1\…`). Same file, matched.
        #[test]
        fn a_shortcut_matches_the_old_exe_through_its_short_name() {
            use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
            use windows::Win32::Storage::FileSystem::GetShortPathNameW;
            let (dir, old, new) = places("com-short");
            let wide: Vec<u16> = old.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
            let mut buf = vec![0u16; 1024];
            // SAFETY: a NUL-terminated path and a buffer of the length given.
            let len = unsafe { GetShortPathNameW(windows::core::PCWSTR(wide.as_ptr()), Some(&mut buf)) } as usize;
            assert!(len > 0 && len < buf.len(), "a short name for {old:?}");
            let short = PathBuf::from(std::ffi::OsString::from_wide(&buf[..len]));
            // (A volume with 8.3 names off gives the long name back: the test then only shows
            // nothing breaks.)
            let desktop = dir.join("Desktop");
            std::fs::create_dir_all(&desktop).unwrap();
            let folders = [ShortcutFolder { place: "on your Desktop", dir: desktop.clone(), depth: 0 }];
            let report = with_com(|| {
                make(&desktop.join("Trek.lnk"), &old, old.parent().unwrap(), &old);
                retarget_in(&folders, &short, &new, &ShellLinks)
            });
            assert_eq!(report.changed, [desktop.join("Trek.lnk")], "{:?}", report.left);
            with_com(|| assert!(same(&ShellLinks.read(&desktop.join("Trek.lnk")).unwrap().target, &new)));
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn a_file_that_is_not_a_shortcut_is_skipped_without_a_word() {
            let (dir, old, new) = places("com-garbage");
            let desktop = dir.join("Desktop");
            std::fs::create_dir_all(&desktop).unwrap();
            std::fs::write(desktop.join("Broken.lnk"), "this is not a shortcut").unwrap();
            let folders = [ShortcutFolder { place: "on your Desktop", dir: desktop.clone(), depth: 0 }];
            assert_eq!(with_com(|| retarget_in(&folders, &old, &new, &ShellLinks)), Report::default());
            assert_eq!(std::fs::read_to_string(desktop.join("Broken.lnk")).unwrap(), "this is not a shortcut");
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
