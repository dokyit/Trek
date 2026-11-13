//! What "Add to another machine…" works out on this Mac before it asks the other one to clone:
//! the origin's URL, over SSH or HTTPS; the files git ignores that a fresh clone would lack (an
//! `.env`), leaving out folders ignored whole (`node_modules`, `target`), with their sizes and
//! which go by default; and those files read for `put_files`.

use std::path::Path;
use trek_remote as tr;

/// A file goes by default when it's under this...
pub const FILE_LIMIT: u64 = 1 << 20;
/// ...and the ones ticked so far stay under this together.
pub const TOTAL_LIMIT: u64 = 8 << 20;

/// How the other machine fetches the repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Ssh,
    Https,
}

/// An ignored file of the project, as the sheet lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredFile {
    /// Relative to the project folder, `/`-separated.
    pub path: String,
    pub size: u64,
    pub ticked: bool,
}

/// `git remote get-url origin` in `root`.
pub fn origin_url(root: &Path) -> Option<String> {
    let out = trek_core::procs::command("git")
        .arg("-C")
        .arg(root)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !url.is_empty()).then_some(url)
}

/// The scheme `url` uses.
pub fn scheme_of(url: &str) -> Scheme {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        Scheme::Https
    } else {
        Scheme::Ssh
    }
}

/// `url` (`https://host/owner/repo.git`, `ssh://git@host/owner/repo`, `git@host:owner/repo.git`)
/// over `scheme`: `https://host/owner/repo.git` or `git@host:owner/repo.git`. A URL already over
/// that scheme stays as written (credentials and ports with it); one that can't be read too.
pub fn rewrite_url(url: &str, scheme: Scheme) -> String {
    let url = url.trim();
    if scheme_of(url) == scheme && !url.to_ascii_lowercase().starts_with("git://") {
        return url.to_string();
    }
    let Some((host, path)) = host_and_path(url) else {
        return url.to_string();
    };
    match scheme {
        Scheme::Https => format!("https://{host}/{path}"),
        Scheme::Ssh => format!("git@{host}:{path}"),
    }
}

/// The host (no user, no port) and the path (no leading `/`) of a git URL.
fn host_and_path(url: &str) -> Option<(String, String)> {
    let (host, path) = match url.split_once("://") {
        Some((_, rest)) => {
            let (authority, path) = rest.split_once('/')?;
            (authority.to_string(), path.to_string())
        }
        // scp-like `[user@]host:path`
        None => {
            let (authority, path) = url.split_once(':')?;
            if authority.contains('/') {
                return None;
            }
            (authority.to_string(), path.to_string())
        }
    };
    let host = host.rsplit_once('@').map_or(host.as_str(), |(_, h)| h);
    let host = host.split(':').next().unwrap_or(host).to_string();
    let path = path.trim_start_matches('/').to_string();
    (!host.is_empty() && !path.is_empty()).then_some((host, path))
}

/// The repository's name in `url` (`trek` for `…/dokyit/trek.git`).
pub fn repo_name(url: &str) -> Option<String> {
    let last = url.trim().trim_end_matches('/').rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty()).then(|| name.to_string())
}

fn git_list(root: &Path, directory: bool) -> anyhow::Result<Vec<String>> {
    let mut cmd = trek_core::procs::command("git");
    cmd.arg("-C").arg(root).args([
        "ls-files",
        "--others",
        "--ignored",
        "--exclude-standard",
        "-z",
    ]);
    if directory {
        cmd.arg("--directory");
    }
    let out = cmd.output()?;
    anyhow::ensure!(
        out.status.success(),
        "git ls-files: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

/// The files git ignores in `root` that aren't inside a folder it ignores whole, with their sizes,
/// ticked when under `FILE_LIMIT` each and `TOTAL_LIMIT` together (in path order).
pub fn ignored_files(root: &Path) -> anyhow::Result<Vec<IgnoredFile>> {
    let all = git_list(root, false)?;
    let whole: Vec<String> = git_list(root, true)?
        .into_iter()
        .filter(|e| e.ends_with('/'))
        .collect();
    let mut files: Vec<IgnoredFile> = all
        .into_iter()
        .filter(|f| !whole.iter().any(|d| f.starts_with(d.as_str())))
        .filter(|f| tr::safe_relative_path(f))
        .filter_map(|path| {
            // Files only: a link could point anywhere.
            let meta = std::fs::symlink_metadata(root.join(&path))
                .ok()
                .filter(|m| m.is_file())?;
            Some(IgnoredFile {
                path,
                size: meta.len(),
                ticked: false,
            })
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut total = 0;
    for f in &mut files {
        if f.size < FILE_LIMIT && total + f.size <= TOTAL_LIMIT {
            f.ticked = true;
            total += f.size;
        }
    }
    Ok(files)
}

/// `files` (relative to `root`) read for `put_files`, each with its permission bits.
pub fn read_uploads(root: &Path, files: &[String]) -> anyhow::Result<Vec<tr::FileUpload>> {
    files
        .iter()
        .map(|path| {
            let full = root.join(path);
            let meta = std::fs::symlink_metadata(&full)?;
            anyhow::ensure!(meta.is_file(), "{path} isn't a file");
            let bytes = std::fs::read(&full)?;
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt as _;
                meta.permissions().mode() & 0o777
            };
            #[cfg(not(unix))]
            let mode = mode_from_contents(&bytes);
            Ok(tr::FileUpload {
                path: path.clone(),
                bytes,
                mode,
            })
        })
        .collect()
}

/// The permission bits a file from Windows, which has none to read, goes to another machine with:
/// executable for a script that names its interpreter on its first line (`#!`, as git treats one
/// on the other side), plain otherwise.
#[cfg(any(not(unix), test))]
fn mode_from_contents(bytes: &[u8]) -> u32 {
    if bytes.starts_with(b"#!") {
        0o755
    } else {
        0o644
    }
}

/// "12 KB", "1.4 MB".
pub fn size_label(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{} KB", bytes.div_ceil(1_000)),
        _ => format!("{:.1} MB", bytes as f64 / 1_000_000.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a PC sends: no bits to read, so a script that names its interpreter goes executable
    /// (it arrives runnable on a Mac), and anything else plain. Checked on both platforms.
    #[test]
    fn a_file_from_windows_goes_executable_only_when_it_is_a_script() {
        assert_eq!(mode_from_contents(b"#!/bin/sh\necho hi\n"), 0o755);
        assert_eq!(mode_from_contents(b"#!/usr/bin/env node\n"), 0o755);
        assert_eq!(mode_from_contents(b"KEY=1\n"), 0o644);
        assert_eq!(mode_from_contents(b""), 0o644);
    }
}
