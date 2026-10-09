//! Composer pickers: what `/`, `@` and `$` complete to, and the project file index behind `@`.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    /// `/command` (only at the start of the message).
    Slash,
    /// `@path/to/file` or `@agent-name`.
    Mention,
    /// `$skill`.
    Skill,
}

/// The token being typed right before the cursor, if it opens a picker.
#[derive(Debug, Clone, PartialEq)]
pub struct Trigger {
    pub kind: PickKind,
    /// Byte offset of the trigger character.
    pub start: usize,
    /// Text after the trigger character, up to the cursor.
    pub query: String,
}

pub fn trigger_at(text: &str, cursor: usize) -> Option<Trigger> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let before = &text[..cursor];
    let start = before.rfind(char::is_whitespace).map(|i| i + before[i..].chars().next().map_or(1, char::len_utf8)).unwrap_or(0);
    let token = &before[start..];
    let mut chars = token.chars();
    let kind = match chars.next()? {
        '/' if before[..start].trim().is_empty() => PickKind::Slash,
        '@' => PickKind::Mention,
        '$' => PickKind::Skill,
        _ => return None,
    };
    Some(Trigger { kind, start, query: chars.as_str().to_string() })
}

/// One row in a picker.
#[derive(Debug, Clone, PartialEq)]
pub struct PickItem {
    pub label: String,
    pub detail: String,
    /// Replaces the trigger token (a trailing space is added).
    pub insert: String,
    pub icon: PickIcon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickIcon {
    Command,
    Skill,
    Agent,
    File,
    Folder,
}

const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".build", "build", "dist", ".next", "DerivedData", "Pods", ".venv", "venv", "__pycache__", ".cache", ".turbo"];
const MAX_FILES: usize = 30_000;

/// A path under a folder as the app keeps them: written with `/` on every platform, as git does,
/// so the code that splits one into folder and name has one separator to look for.
pub fn rel_string(rel: &Path) -> String {
    let s = rel.to_string_lossy();
    if cfg!(windows) { s.replace('\\', "/") } else { s.into_owned() }
}

/// Relative paths of the project's files and folders (folders end with `/`).
pub fn index_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name == ".DS_Store" {
                continue;
            }
            let path = e.path();
            let Ok(rel) = path.strip_prefix(root) else { continue };
            let rel = rel_string(rel);
            match e.file_type() {
                Ok(t) if t.is_dir() => {
                    if SKIP_DIRS.contains(&name.as_str()) || (name.starts_with('.') && !matches!(name.as_str(), ".github" | ".claude" | ".codex" | ".vscode")) {
                        continue;
                    }
                    out.push(format!("{rel}/"));
                    stack.push(path);
                }
                Ok(_) => out.push(rel),
                Err(_) => {}
            }
            if out.len() >= MAX_FILES {
                return out;
            }
        }
    }
    out
}

/// Best matches for `query` among indexed paths: file-name prefix, then file-name substring,
/// then path substring, then subsequence. Shorter paths win ties.
pub fn match_files(files: &[String], query: &str, limit: usize) -> Vec<String> {
    let q = query.to_lowercase();
    let mut scored: Vec<(u8, usize, &String)> = files
        .iter()
        .filter_map(|f| {
            let lower = f.to_lowercase();
            let name = lower.trim_end_matches('/').rsplit('/').next().unwrap_or(&lower).to_string();
            let score = if q.is_empty() {
                // Top level first when nothing is typed yet.
                if lower.trim_end_matches('/').contains('/') { 4 } else { 0 }
            } else if name.starts_with(&q) {
                0
            } else if name.contains(&q) {
                1
            } else if lower.contains(&q) {
                2
            } else if is_subsequence(&q, &lower) {
                3
            } else {
                return None;
            };
            Some((score, f.len(), f))
        })
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, _, f)| f.clone()).collect()
}

fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

pub fn is_image(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(), Some("png" | "jpg" | "jpeg" | "gif" | "webp"))
}

/// Where snapshots are written before they're attached.
/// Delete snapshots older than `keep_days` (0 keeps everything).
pub fn prune_snapshots(keep_days: u32) {
    if keep_days == 0 {
        return;
    }
    let max_age = std::time::Duration::from_secs(keep_days as u64 * 86_400);
    let Ok(entries) = std::fs::read_dir(trek_core::paths::data_dir().join("snapshots")) else { return };
    for e in entries.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > max_age));
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// (count, total bytes) of saved snapshots.
pub fn snapshot_usage() -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir(trek_core::paths::data_dir().join("snapshots")) else { return (0, 0) };
    entries.flatten().filter_map(|e| e.metadata().ok().filter(|m| m.is_file())).fold((0, 0), |(n, b), m| (n + 1, b + m.len()))
}

pub fn snapshot_path() -> PathBuf {
    let dir = trek_core::paths::data_dir().join("snapshots");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("snapshot-{}.png", chrono::Local::now().format("%Y%m%d-%H%M%S-%3f")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers() {
        assert_eq!(trigger_at("/rev", 4).unwrap().kind, PickKind::Slash);
        assert!(trigger_at("fix /rev", 8).is_none(), "slash only at the start");
        let t = trigger_at("look at @src/ma", 15).unwrap();
        assert_eq!((t.kind, t.start, t.query.as_str()), (PickKind::Mention, 8, "src/ma"));
        assert_eq!(trigger_at("use $sk", 7).unwrap().kind, PickKind::Skill);
        assert!(trigger_at("email a@b", 5).is_none());
    }

    #[test]
    fn file_matching() {
        let files = vec!["src/main.rs".to_string(), "README.md".into(), "src/domain/remain.rs".into(), "src/".into()];
        assert_eq!(match_files(&files, "main", 2), vec!["src/main.rs".to_string(), "src/domain/remain.rs".to_string()]);
    }
}
