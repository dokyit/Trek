use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::OnceLock;

static ISOLATED: OnceLock<PathBuf> = OnceLock::new();

/// Where everything goes once this process has deleted its profile (`leave`).
static LEFT: OnceLock<PathBuf> = OnceLock::new();

thread_local! {
    /// A data folder of the calling thread's own, inside an isolated process (`isolate_thread`).
    static THREAD_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Keep this process away from the user's data: everything Trek stores goes under `dir`, and API
/// keys and tokens are kept in memory (`keychain`), never in the Keychain. For tests; the first
/// call wins.
pub fn isolate(dir: PathBuf) {
    let _ = ISOLATED.set(dir);
}

/// In an isolated process, give the calling thread a data folder of its own: tests that run side
/// by side, each saving its settings, then don't overwrite one another's. Does nothing in a
/// process that isn't isolated.
pub fn isolate_thread(dir: PathBuf) {
    if isolated() {
        THREAD_DIR.with(|d| *d.borrow_mut() = Some(dir));
    }
}

/// The folder `isolate` set up. trek-core's own tests are isolated from the start, whichever
/// of them runs first: none of them may reach the user's folder.
fn isolated_dir() -> Option<&'static PathBuf> {
    if let Some(left) = LEFT.get() {
        return Some(left);
    }
    #[cfg(test)]
    ISOLATED.get_or_init(|| std::env::temp_dir().join("trek-core-tests"));
    ISOLATED.get()
}

/// Whether [`isolate`] is in effect.
pub fn isolated() -> bool {
    isolated_dir().is_some()
}

/// Leave the profile this process had, for good: "Delete all Trek data" deleted it, and Trek
/// quits next. Whatever is still saved on the way out (a thread, the settings, the window's place)
/// goes to `dir`, a throwaway folder, and secrets stay in memory, as in an isolated process: the
/// deleted folders aren't made again. The first call wins.
pub fn leave(dir: PathBuf) {
    let _ = LEFT.set(dir);
}

/// Whether the data folder is somewhere other than the user's own: a test, the verify app, or
/// `TREK_DATA_DIR`. Such a profile shares the system's places (the Keychain, the platform's cache
/// folder) with the user's own Trek, so they aren't its to empty.
pub fn moved() -> bool {
    isolated_dir().is_some() || std::env::var_os("TREK_DATA_DIR").is_some_and(|d| !d.is_empty())
}

/// The platform's cache folder for Trek (`~/Library/Caches/dev.trek.Trek` on a Mac,
/// `%LOCALAPPDATA%\trek\Trek\cache`, `~/.cache/trek`), or `cache` in the data folder when that's
/// been moved.
pub fn cache_dir() -> PathBuf {
    (!moved())
        .then(|| {
            directories::ProjectDirs::from("dev", "trek", "Trek")
                .map(|p| p.cache_dir().to_path_buf())
        })
        .flatten()
        .unwrap_or_else(|| data_dir().join("cache"))
}

static REAL_AGENTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A capture run's opt-in (`trek-dev start --real-agents`): this isolated process still looks for
/// the agents installed on this computer, asks them about their models, and starts them, signed
/// in as the user. Everything else stays isolated: the data folder, the Keychain, imports,
/// notifications.
pub fn allow_real_agents() {
    REAL_AGENTS.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether this process may look for, ask and start the user's own agents: always outside an
/// isolated process, and in one only after [`allow_real_agents`].
pub fn real_agents() -> bool {
    reaches_real_agents(
        isolated(),
        REAL_AGENTS.load(std::sync::atomic::Ordering::Relaxed),
    )
}

fn reaches_real_agents(isolated: bool, allowed: bool) -> bool {
    !isolated || allowed
}

/// `~/Library/Application Support/Trek` on macOS (platform equivalent elsewhere).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = isolated_dir() {
        let dir = match LEFT.get() {
            Some(left) => left.clone(),
            None => THREAD_DIR
                .with(|d| d.borrow().clone())
                .unwrap_or_else(|| dir.clone()),
        };
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }
    // TREK_DATA_DIR points Trek at another data folder (testing a build without touching real data).
    let dir = std::env::var_os("TREK_DATA_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            directories::ProjectDirs::from("dev", "trek", "Trek")
                .map(|p| p.data_dir().to_path_buf())
                .unwrap_or_else(|| home().join(".trek"))
        });
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Where Trek keeps what it can download again and has no use on another computer (dictation's
/// speech model): `%LOCALAPPDATA%\trek\Trek\data` on Windows, which a roaming profile doesn't
/// copy from PC to PC as it does the data folder (`%APPDATA%`); the data folder itself on a Mac,
/// where the two are one. The data folder whenever that's been moved (tests, `TREK_DATA_DIR`),
/// so a trial run keeps it with the rest.
pub fn local_data_dir() -> PathBuf {
    let moved = isolated_dir().is_some()
        || std::env::var_os("TREK_DATA_DIR").is_some_and(|d| !d.is_empty());
    if moved {
        return data_dir();
    }
    directories::ProjectDirs::from("dev", "trek", "Trek")
        .map(|p| p.data_local_dir().to_path_buf())
        .unwrap_or_else(data_dir)
}

/// The user's home folder. `HOME` wins when it's set, on every platform: the Unix resolver reads it
/// anyway, and it lets a test or a capture run point Trek at a throwaway home on Windows too, where
/// the profile folder would otherwise be used whatever the environment says.
pub fn home() -> PathBuf {
    // Only a real, absolute folder: a POSIX-style `/c/Users/x` from an MSYS shell is left to the
    // resolver, as is a value that names nothing.
    if let Some(home) = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute() && h.is_dir())
    {
        return home;
    }
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The home folder whose agent setup (skills, MCP servers, plugins) Trek's settings read and
/// change: the user's, or `home` in the data folder in an isolated process, so a test or capture
/// run never sees or edits theirs.
pub fn agents_home() -> PathBuf {
    if isolated() {
        data_dir().join("home")
    } else {
        home()
    }
}

/// Where threads without a project run: each in a folder of its own under here, so an agent has
/// somewhere to work that isn't the user's home or one of their projects. `~/Trek/Chats`: easy to
/// find, and no space in the path for an agent's shell commands to trip on. Under the data folder
/// when that's been moved (tests, `TREK_DATA_DIR`), so a trial run leaves nothing in the home folder.
pub fn chats_dir() -> PathBuf {
    let moved = isolated_dir().is_some()
        || std::env::var_os("TREK_DATA_DIR").is_some_and(|d| !d.is_empty());
    if moved {
        data_dir().join("chats")
    } else {
        home().join("Trek").join("Chats")
    }
}

/// A new folder for a thread without a project: a git repository of its own, so Trek's file
/// checkpoints (and rewinding a turn) work there as in a project.
pub fn new_chat_dir() -> std::io::Result<PathBuf> {
    let dir = chats_dir().join(
        chrono::Local::now()
            .format("%Y-%m-%d-%H%M%S-%3f")
            .to_string(),
    );
    std::fs::create_dir_all(&dir)?;
    let _ = crate::procs::command(crate::git::git_program())
        .args(["init", "-q"])
        .current_dir(&dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Ok(dir)
}

/// `path` is (in) a folder for a thread without a project.
pub fn is_chat_dir(path: &std::path::Path) -> bool {
    path.starts_with(chats_dir())
}

pub fn settings_file() -> PathBuf {
    data_dir().join("settings.toml")
}

pub fn database_file() -> PathBuf {
    data_dir().join("trek.sqlite")
}

/// Where each thread's transcript is kept as a plain-text log (`session_log`), beside the
/// database.
pub fn transcripts_dir() -> PathBuf {
    data_dir().join("transcripts")
}

pub fn updates_dir() -> PathBuf {
    let dir = data_dir().join("updates");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Windows's long form of a path (`\\?\C:\x`) as the plain one (`C:\x`), which is how git, the
/// agents and the user write it (git can't take the long one). Any other path comes back as it is.
pub fn plain(path: &std::path::Path) -> PathBuf {
    use std::path::{Component, Prefix};
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Prefix(p) => match p.kind() {
                Prefix::VerbatimDisk(drive) => out.push(format!("{}:", drive as char)),
                _ => out.push(c.as_os_str()),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path` with its links resolved, in its plain form (`plain`); as it is when it can't be.
pub fn resolved(path: &std::path::Path) -> PathBuf {
    plain(&std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// Shorten a path for display: `/Users/me/Documents/x` → `~/Documents/x`, and on Windows
/// `C:\Users\me\Documents\x` → `~\Documents\x` (PowerShell's own `~`): the folders under `~` are
/// joined with the platform's separator, never the half-and-half `~/Documents\x` that `~/` plus
/// the native path would make, and never the Unix form on a PC.
pub fn tildify(path: &std::path::Path) -> String {
    tildify_with(path, std::path::MAIN_SEPARATOR_STR)
}

/// [`tildify`] with `/` on every platform (`~/Documents/x`), for a path put in a message to an
/// agent: its shell may be Git Bash, where `~\Documents` is no path, and `~/` is one in every
/// shell Windows has.
pub fn tildify_for_agents(path: &std::path::Path) -> String {
    tildify_with(path, "/")
}

fn tildify_with(path: &std::path::Path, sep: &str) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => {
            let parts: Vec<_> = rest
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect();
            format!("~{sep}{}", parts.join(sep))
        }
        Err(_) => path.display().to_string(),
    }
}

/// The other computer that looking at `path` would reach: on Windows a share's server
/// (`\\server\share\…`, `//server/…`, `\\?\UNC\server\…`), or any other `\\?\` or `\\.\` device
/// path but a drive's (`device`); elsewhere the automounter's `/net/<host>/…`, and on Linux a share
/// GVFS (the file manager) mounted, `/run/user/<uid>/gvfs/smb-share:server=<host>,…/…` (`host=`
/// for sftp, dav, ftp and the rest). `None` for a path on this one. Asking whether such a path exists connects to that computer, and on Windows offers
/// it the user's sign-in (NTLM), so text an agent or a web page wrote mustn't make Trek look.
pub fn other_machine(path: &std::path::Path) -> Option<String> {
    other_machine_on(&path.to_string_lossy(), WINDOWS)
}

/// Paths are read as Windows reads them (shares, device paths) or as Unix does (`/net`).
const WINDOWS: bool = cfg!(windows);

/// [`other_machine`] for a path written `raw`, as Windows reads it (`windows`) or as Unix does.
pub fn other_machine_on(raw: &str, windows: bool) -> Option<String> {
    let lower = |s: &str| s.to_ascii_lowercase();
    if !windows {
        if let Some(rest) = raw.strip_prefix("/net/") {
            let host = rest.split('/').next().unwrap_or_default();
            return (!host.is_empty()).then(|| lower(host));
        }
        return gvfs_host(raw).map(|h| lower(&h));
    }
    let sep = |c: char| c == '\\' || c == '/';
    let mut chars = raw.chars();
    if !(chars.next().is_some_and(sep) && chars.next().is_some_and(sep)) {
        return None;
    }
    let rest = &raw[2..];
    let mut parts = rest.split(sep);
    let first = parts.next().unwrap_or_default();
    if first == "?" || first == "." {
        let next = parts.next().unwrap_or_default();
        // `\\?\C:\…`, a drive's long form, is this computer's.
        let drive = next.len() == 2
            && next.as_bytes()[0].is_ascii_alphabetic()
            && next.as_bytes()[1] == b':';
        if drive {
            return None;
        }
        if next.eq_ignore_ascii_case("UNC") {
            let server = parts.next().unwrap_or_default();
            return Some(if server.is_empty() {
                "device".into()
            } else {
                lower(server)
            });
        }
        return Some("device".into());
    }
    Some(if first.is_empty() {
        "device".into()
    } else {
        lower(first)
    })
}

/// The server of a GVFS mount's path, `/run/user/<uid>/gvfs/<backend>:<key>=<value>,…/…`: its
/// `server=` (an SMB share) or `host=` (sftp, dav, ftp, nfs, afp…). A phone's mount (`mtp:`,
/// `gphoto2:`, `afc:`, whose `host=` is the device) or one with neither key (`trash:`) is this
/// computer's, as is anything that isn't under a user's runtime folder.
fn gvfs_host(raw: &str) -> Option<String> {
    let rest = raw.strip_prefix("/run/user/")?;
    let mut parts = rest.split('/');
    let uid = parts.next()?;
    if uid.is_empty() || !uid.bytes().all(|b| b.is_ascii_digit()) || parts.next()? != "gvfs" {
        return None;
    }
    let (backend, keys) = parts.next()?.split_once(':')?;
    if matches!(backend, "mtp" | "gphoto2" | "afc") {
        return None;
    }
    keys.split(',')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == "server" || *k == "host")
        .map(|(_, v)| v.to_string())
        .filter(|v| !v.is_empty())
}

/// Whether looking at `path` reaches another computer ([`other_machine`]) that `cwd`, the folder
/// the text naming it belongs to, isn't on already: a project kept on a share still finds its own
/// files there, while a path an answer names on any other server isn't looked at.
pub fn reaches_out(path: &std::path::Path, cwd: Option<&std::path::Path>) -> bool {
    reaches_out_on(
        &path.to_string_lossy(),
        cwd.map(|c| c.to_string_lossy()).as_deref(),
        WINDOWS,
    )
}

/// [`reaches_out`] for paths written `path` and `cwd`, as Windows reads them (`windows`) or as
/// Unix does.
fn reaches_out_on(path: &str, cwd: Option<&str>, windows: bool) -> bool {
    match other_machine_on(path, windows) {
        None => false,
        Some(machine) => cwd.and_then(|c| other_machine_on(c, windows)).as_ref() != Some(&machine),
    }
}

/// Move `path` to the Trash (the Recycle Bin on Windows), where the user can still get it back.
pub fn trash(path: &std::path::Path) -> anyhow::Result<()> {
    let path = path.to_path_buf();
    // The crate initialises COM on the calling thread, and panics if that thread already holds
    // it in another mode. A thread of its own always starts clean.
    #[cfg(windows)]
    let result = std::thread::spawn(move || move_to_trash(&path))
        .join()
        .map_err(|_| anyhow::anyhow!("The Recycle Bin wouldn't open"))?;
    #[cfg(not(windows))]
    let result = move_to_trash(&path);
    result
}

fn move_to_trash(path: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // NSFileManager, as the app has always done, not the Finder (an AppleScript round trip).
        use trash::macos::{DeleteMethod, TrashContextExtMacos as _};
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(path)?;
    }
    #[cfg(not(target_os = "macos"))]
    trash::delete(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_isolated_process_reaches_the_users_agents_only_when_allowed() {
        assert!(
            super::reaches_real_agents(false, false),
            "the user's own Trek"
        );
        assert!(
            !super::reaches_real_agents(true, false),
            "a test or a capture run"
        );
        assert!(
            super::reaches_real_agents(true, true),
            "trek-dev start --real-agents"
        );
        // These tests run isolated, and nothing here allowed it.
        assert!(!super::real_agents());
    }

    #[test]
    fn trashing_a_file_or_folder_takes_it_out_of_its_folder() {
        let dir = std::env::temp_dir().join(format!("trek-trash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("folder/inner")).unwrap();
        let file = dir.join("note.txt");
        std::fs::write(&file, "x").unwrap();
        std::fs::write(dir.join("folder/inner/a.txt"), "y").unwrap();
        super::trash(&file).unwrap();
        super::trash(&dir.join("folder")).unwrap();
        assert!(!file.exists() && !dir.join("folder").exists());
        assert!(super::trash(&dir.join("never-was")).is_err());
        // These two now sit in the user's Recycle Bin (their own temp files, nothing else); take
        // just them out again where the crate can list and purge.
        #[cfg(windows)]
        {
            // The bin's enumeration trails IFileOperation on change notifications: on a loaded
            // machine the second item hasn't shown yet when the first is already listed. Wait for
            // both rather than read once (CI run 38048787332 saw exactly one).
            //
            // The bin lists an item by its display name, which leaves the extension off where
            // Explorer's "Hide extensions for known file types" is on (the default of a fresh
            // profile, as on GitHub's runners; off on a developer's machine): "note", not
            // "note.txt". Matching by that name is why those runs found exactly one item even after
            // waiting 30 s. The folder is this process's own, so its parent alone picks the two out.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let items = loop {
                let all = trash::os_limited::list().unwrap();
                let total = all.len();
                let items: Vec<_> = all
                    .into_iter()
                    .filter(|i| i.original_parent.file_name() == dir.file_name())
                    .collect();
                if items.len() == 2 {
                    break items;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "both went into the Recycle Bin; found {:?} of {total} items there",
                    items
                        .iter()
                        .map(|i| (&i.name, &i.original_parent))
                        .collect::<Vec<_>>()
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
            };
            trash::os_limited::purge_all(items).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_home_relative_path_is_written_with_the_platforms_separator() {
        let home = super::home();
        let (sep, written) = if cfg!(windows) {
            ('\\', "~\\Documents\\Trek")
        } else {
            ('/', "~/Documents/Trek")
        };
        // Built piece by piece, so on Windows it's `C:\Users\…\Documents\Trek` with backslashes,
        // and what's shown has them too, not the Unix form (nor `~/Documents\Trek`).
        assert_eq!(
            super::tildify(&home.join("Documents").join("Trek")),
            written
        );
        assert_eq!(
            super::tildify(&home.join("a").join("b").join("c.toml")),
            format!("~{sep}a{sep}b{sep}c.toml")
        );
        assert_eq!(super::tildify(&home), "~");
        // What an agent is told is the same on both: `~/…` works in every shell.
        assert_eq!(
            super::tildify_for_agents(&home.join("Documents").join("Trek")),
            "~/Documents/Trek"
        );
        assert_eq!(super::tildify_for_agents(&home), "~");
        // A name with a space (OneDrive's folders have them) stays whole.
        assert_eq!(
            super::tildify(&home.join("OneDrive").join("FC 24 LE v24.1.1.0")),
            format!("~{sep}OneDrive{sep}FC 24 LE v24.1.1.0")
        );
        // And a path that merely shares a prefix with the home folder isn't taken for one under it.
        let mut sibling = home.clone().into_os_string();
        sibling.push("-other");
        let sibling = std::path::PathBuf::from(sibling);
        assert_eq!(super::tildify(&sibling), sibling.display().to_string());
    }

    #[test]
    fn tests_never_reach_the_users_data() {
        // Isolated before any test asked, and staying where it was put.
        assert!(super::isolated());
        let dir = std::env::temp_dir().join("trek-core-tests");
        super::isolate(std::env::temp_dir().join("elsewhere"));
        assert_eq!(super::data_dir(), dir);
        assert!(super::settings_file().starts_with(&dir));
        assert_eq!(
            crate::settings::secrets::api_key("anthropic"),
            None,
            "no Keychain (and no shell keys) while isolated"
        );
        // Saved keys stay in the process's own memory.
        let probe = format!("isolation-probe-{}", std::process::id());
        crate::settings::secrets::set_api_key(&probe, "x").unwrap();
        assert_eq!(
            crate::settings::secrets::stored_api_key(&probe).as_deref(),
            Some("x")
        );
        crate::settings::secrets::delete_api_key(&probe).unwrap();
        assert_eq!(crate::settings::secrets::stored_api_key(&probe), None);
        // What can be downloaded again stays with the rest of an isolated process's data, not
        // in the user's local folder.
        assert_eq!(super::local_data_dir(), dir);
    }

    /// A path that would make Trek connect to another computer just to look at it: a Windows
    /// share in every form Windows takes one, a device path, the automounter's `/net`. A drive's
    /// long form, a local path, and the Unix reading of backslashes are this computer's.
    #[test]
    fn paths_on_other_machines_are_told_apart() {
        use super::other_machine_on as other;
        for (raw, machine) in [
            (r"\\attacker.example\share\x.png", "attacker.example"),
            ("//Attacker/share/x", "attacker"),
            (r"\/attacker\share", "attacker"),
            (r"\\?\UNC\nas\code\a.rs", "nas"),
            (r"\\.\UNC\nas\code", "nas"),
            (r"\\.\pipe\trek", "device"),
            (r"\\?\GLOBALROOT\Device\x", "device"),
            (r"\\", "device"),
        ] {
            assert_eq!(other(raw, true).as_deref(), Some(machine), "{raw}");
        }
        for raw in [
            r"C:\Users\me\x.png",
            r"\\?\C:\Users\me",
            r"\Users\me",
            "src/main.rs",
        ] {
            assert_eq!(other(raw, true), None, "{raw}");
        }
        assert_eq!(
            other("/net/host.example/x", false).as_deref(),
            Some("host.example")
        );
        for raw in [
            r"\\attacker\share\x",
            "//attacker/share",
            "/network/x",
            "/net",
            "/Users/me/net/x",
        ] {
            assert_eq!(other(raw, false), None, "{raw}");
        }
        // A project on a share still sees its own files there, and no other server's.
        use super::reaches_out_on as out;
        let share = Some(r"\\NAS\code\proj");
        assert!(!out(r"\\nas\code\proj\a.rs", share, true));
        assert!(out(r"\\evil\s\a.rs", share, true));
        assert!(out(r"\\evil\s\a.rs", None, true));
        assert!(!out(r"C:\code\a.rs", None, true));
        // GVFS's mounts (Linux's file manager): a share's server, an sftp or WebDAV host; a
        // phone or the trash isn't another computer.
        for (raw, machine) in [
            (
                "/run/user/1000/gvfs/smb-share:server=NAS.local,share=code/a.png",
                "nas.local",
            ),
            (
                "/run/user/1000/gvfs/sftp:host=example.com,user=me/x",
                "example.com",
            ),
            (
                "/run/user/42/gvfs/dav:host=dav.example,ssl=true/x",
                "dav.example",
            ),
        ] {
            assert_eq!(other(raw, false).as_deref(), Some(machine), "{raw}");
        }
        for raw in [
            "/run/user/1000/gvfs/mtp:host=Google_Pixel_123/x",
            "/run/user/1000/gvfs/trash:/x",
            "/run/user/1000/gvfs",
            "/run/user/me/gvfs/smb-share:server=nas,share=c/x",
            "/home/me/run/user/1000/gvfs/sftp:host=h/x",
        ] {
            assert_eq!(other(raw, false), None, "{raw}");
        }
        let gvfs = Some("/run/user/1000/gvfs/smb-share:server=nas,share=code/proj");
        assert!(!out(
            "/run/user/1000/gvfs/smb-share:server=nas,share=code/proj/a.rs",
            gvfs,
            false
        ));
        assert!(out(
            "/run/user/1000/gvfs/sftp:host=evil.example/a",
            gvfs,
            false
        ));
        let net = Some("/net/nas/code");
        assert!(!out("/net/nas/code/a.rs", net, false));
        assert!(out("/net/evil/a", net, false));
        assert!(!out("/tmp/a", None, false));
    }

    #[test]
    fn a_thread_can_have_a_folder_of_its_own() {
        let own = std::env::temp_dir()
            .join("trek-core-tests")
            .join(format!("thread-{}", std::process::id()));
        std::thread::spawn({
            let own = own.clone();
            move || {
                super::isolate_thread(own.clone());
                assert_eq!(super::settings_file(), own.join("settings.toml"));
            }
        })
        .join()
        .unwrap();
        assert_ne!(super::data_dir(), own, "other threads keep the process's");
        let _ = std::fs::remove_dir_all(own);
    }
}
