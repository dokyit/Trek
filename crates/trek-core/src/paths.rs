use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::OnceLock;

static ISOLATED: OnceLock<PathBuf> = OnceLock::new();

thread_local! {
    /// A data folder of the calling thread's own, inside an isolated process (`isolate_thread`).
    static THREAD_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Keep this process away from the user's data: everything Trek stores goes under `dir`, and API
/// keys are neither read from nor written to the Keychain. For tests; the first call wins.
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
    #[cfg(test)]
    ISOLATED.get_or_init(|| std::env::temp_dir().join("trek-core-tests"));
    ISOLATED.get()
}

/// Whether [`isolate`] is in effect.
pub fn isolated() -> bool {
    isolated_dir().is_some()
}

/// `~/Library/Application Support/Trek` on macOS (platform equivalent elsewhere).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = isolated_dir() {
        let dir = THREAD_DIR.with(|d| d.borrow().clone()).unwrap_or_else(|| dir.clone());
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }
    // TREK_DATA_DIR points Trek at another data folder (testing a build without touching real data).
    let dir = std::env::var_os("TREK_DATA_DIR").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| {
        directories::ProjectDirs::from("dev", "trek", "Trek").map(|p| p.data_dir().to_path_buf()).unwrap_or_else(|| home().join(".trek"))
    });
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The user's home folder. `HOME` wins when it's set, on every platform: the Unix resolver reads it
/// anyway, and it lets a test or a capture run point Trek at a throwaway home on Windows too, where
/// the profile folder would otherwise be used whatever the environment says.
pub fn home() -> PathBuf {
    // Only a real, absolute folder: a POSIX-style `/c/Users/x` from an MSYS shell is left to the
    // resolver, as is a value that names nothing.
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_absolute() && h.is_dir()) {
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
    if isolated() { data_dir().join("home") } else { home() }
}

/// Where threads without a project run: each in a folder of its own under here, so an agent has
/// somewhere to work that isn't the user's home or one of their projects. `~/Trek/Chats`: easy to
/// find, and no space in the path for an agent's shell commands to trip on. Under the data folder
/// when that's been moved (tests, `TREK_DATA_DIR`), so a trial run leaves nothing in the home folder.
pub fn chats_dir() -> PathBuf {
    let moved = isolated_dir().is_some() || std::env::var_os("TREK_DATA_DIR").is_some_and(|d| !d.is_empty());
    if moved { data_dir().join("chats") } else { home().join("Trek").join("Chats") }
}

/// A new folder for a thread without a project: a git repository of its own, so Trek's file
/// checkpoints (and rewinding a turn) work there as in a project.
pub fn new_chat_dir() -> std::io::Result<PathBuf> {
    let dir = chats_dir().join(chrono::Local::now().format("%Y-%m-%d-%H%M%S-%3f").to_string());
    std::fs::create_dir_all(&dir)?;
    let _ = std::process::Command::new(crate::git::git_program()).args(["init", "-q"]).current_dir(&dir).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
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

pub fn updates_dir() -> PathBuf {
    let dir = data_dir().join("updates");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Shorten a path for display: `/Users/me/Documents/x` → `~/Documents/x`.
pub fn tildify(path: &std::path::Path) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Move `path` to the Trash (the Recycle Bin on Windows), where the user can still get it back.
pub fn trash(path: &std::path::Path) -> anyhow::Result<()> {
    let path = path.to_path_buf();
    // The crate initialises COM on the calling thread, and panics if that thread already holds
    // it in another mode. A thread of its own always starts clean.
    #[cfg(windows)]
    let result = std::thread::spawn(move || move_to_trash(&path)).join().map_err(|_| anyhow::anyhow!("The Recycle Bin wouldn't open"))?;
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
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let items = loop {
                let items: Vec<_> = trash::os_limited::list()
                    .unwrap()
                    .into_iter()
                    .filter(|i| i.original_parent.file_name() == dir.file_name() && (i.name == "note.txt" || i.name == "folder"))
                    .collect();
                if items.len() == 2 {
                    break items;
                }
                assert!(std::time::Instant::now() < deadline, "both went into the Recycle Bin");
                std::thread::sleep(std::time::Duration::from_millis(100));
            };
            trash::os_limited::purge_all(items).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tests_never_reach_the_users_data() {
        // Isolated before any test asked, and staying where it was put.
        assert!(super::isolated());
        let dir = std::env::temp_dir().join("trek-core-tests");
        super::isolate(std::env::temp_dir().join("elsewhere"));
        assert_eq!(super::data_dir(), dir);
        assert!(super::settings_file().starts_with(&dir));
        assert_eq!(crate::settings::secrets::api_key("anthropic"), None, "no Keychain while isolated");
        assert!(crate::settings::secrets::set_api_key("anthropic", "x").is_err());
    }

    #[test]
    fn a_thread_can_have_a_folder_of_its_own() {
        let own = std::env::temp_dir().join("trek-core-tests").join(format!("thread-{}", std::process::id()));
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
