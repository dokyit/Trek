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

pub fn home() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/"))
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

#[cfg(test)]
mod tests {
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
