use std::path::PathBuf;

/// `~/Library/Application Support/Trek` on macOS (platform equivalent elsewhere).
pub fn data_dir() -> PathBuf {
    let dir = directories::ProjectDirs::from("dev", "trek", "Trek")
        .map(|p| p.data_dir().to_path_buf())
        .unwrap_or_else(|| home().join(".trek"));
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
