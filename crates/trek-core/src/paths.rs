use std::path::PathBuf;
use std::sync::OnceLock;

static ISOLATED: OnceLock<PathBuf> = OnceLock::new();

/// Keep this process away from the user's data: everything Trek stores goes under `dir`, and API
/// keys are neither read from nor written to the Keychain. For tests; the first call wins.
pub fn isolate(dir: PathBuf) {
    let _ = ISOLATED.set(dir);
}

/// Whether [`isolate`] is in effect.
pub fn isolated() -> bool {
    ISOLATED.get().is_some()
}

/// `~/Library/Application Support/Trek` on macOS (platform equivalent elsewhere).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = ISOLATED.get() {
        let _ = std::fs::create_dir_all(dir);
        return dir.clone();
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
    fn isolation_moves_the_data_dir() {
        let dir = std::env::temp_dir().join(format!("trek-paths-test-{}", std::process::id()));
        super::isolate(dir.clone());
        assert!(super::isolated());
        assert_eq!(super::data_dir(), dir);
        assert!(super::settings_file().starts_with(&dir));
        assert_eq!(crate::settings::secrets::api_key("anthropic"), None, "no Keychain while isolated");
        assert!(crate::settings::secrets::set_api_key("anthropic", "x").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
