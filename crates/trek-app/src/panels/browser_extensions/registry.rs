//! Which extensions are installed (macOS), in `extensions.json` beside them: WebKit keeps an
//! extension's storage but not the list of what to load, nor what the user let it do. WebView2
//! keeps its own list (Windows), so there this file isn't written.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub id: String,
    /// A folder of the user's own, run where it is (an unpacked extension being worked on); else
    /// the extension is in `<extensions dir>/<id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
    pub enabled: bool,
    /// Permissions and sites allowed when it asked later (optional ones), beyond those it was
    /// installed with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_sites: Vec<String>,
}

impl Installed {
    pub fn folder_in(&self, dir: &Path) -> PathBuf {
        self.folder.clone().unwrap_or_else(|| dir.join(&self.id))
    }
}

fn file(dir: &Path) -> PathBuf {
    dir.join("extensions.json")
}

/// What's installed in `dir` (nothing when the list is missing or unreadable).
pub fn load(dir: &Path) -> Vec<Installed> {
    std::fs::read_to_string(file(dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(dir: &Path, all: &[Installed]) {
    let _ = std::fs::create_dir_all(dir);
    let text = serde_json::to_string_pretty(all).unwrap_or_default();
    let tmp = file(dir).with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, file(dir));
    }
}

/// Change the list in `dir` with `f`, and save it.
pub fn update<R>(dir: &Path, f: impl FnOnce(&mut Vec<Installed>) -> R) -> R {
    let mut all = load(dir);
    let out = f(&mut all);
    save(dir, &all);
    out
}

/// Put `entry` in the list, in place of one with its id (keeping what that one was allowed).
pub fn put(all: &mut Vec<Installed>, mut entry: Installed) {
    if let Some(old) = all.iter_mut().find(|e| e.id == entry.id) {
        if entry.allowed.is_empty() {
            entry.allowed = std::mem::take(&mut old.allowed);
        }
        if entry.allowed_sites.is_empty() {
            entry.allowed_sites = std::mem::take(&mut old.allowed_sites);
        }
        *old = entry;
    } else {
        all.push(entry);
    }
}

/// Move the unpacked extension at `from` to `to`, in place of what was there.
pub fn replace_dir(from: &Path, to: &Path) -> Result<(), String> {
    if from == to {
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(to);
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::rename(from, to).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_keeps_what_was_allowed_across_an_update() {
        let dir = std::env::temp_dir().join(format!("trek-ext-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load(&dir).is_empty());
        update(&dir, |all| {
            put(
                all,
                Installed {
                    id: "a".into(),
                    enabled: true,
                    allowed: vec!["tabs".into()],
                    ..Default::default()
                },
            );
            put(
                all,
                Installed {
                    id: "b".into(),
                    folder: Some("/src/b".into()),
                    ..Default::default()
                },
            );
        });
        // Installed again (an update): what it was allowed stays.
        update(&dir, |all| {
            put(
                all,
                Installed {
                    id: "a".into(),
                    enabled: true,
                    ..Default::default()
                },
            )
        });
        let all = load(&dir);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].allowed, ["tabs"]);
        assert_eq!(all[0].folder_in(&dir), dir.join("a"));
        assert_eq!(all[1].folder_in(&dir), PathBuf::from("/src/b"));
        assert!(!all[1].enabled);

        let (from, to) = (dir.join("in"), dir.join("out"));
        std::fs::create_dir_all(from.join("x")).unwrap();
        std::fs::create_dir_all(to.join("old")).unwrap();
        replace_dir(&from, &to).unwrap();
        assert!(to.join("x").is_dir() && !to.join("old").exists() && !from.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
