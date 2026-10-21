//! One Trek driving another (`docs/MULTI-MACHINE.md`): machine A, paired with machine B as a phone
//! is, asks what B is (`machine_info`), clones a repo there (`clone_project`, with
//! `clone_progress` pushes while git runs) and copies the files git ignores into it (`put_files`).
//! A Mac that serves these lists [`super::feature::MACHINES`] in `welcome`/`paired` `features`.

use serde::{Deserialize, Serialize};

/// The reply to `machine_info`: what A needs to name B and to default a clone's folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineInfo {
    /// The machine's name, as its Bonjour record and the pairing link give it ("Studio").
    pub name: String,
    /// `macos`, `windows`, `linux` (Rust's `std::env::consts::OS`).
    pub platform: String,
    /// Trek's version there ("0.4.7").
    pub trek_version: String,
    /// The user's home folder there (`/Users/tobias`): a clone goes to `<home>/<repo>` by default.
    pub home: String,
}

/// `clone_project`: `git clone <url> <folder>` on B, then the folder registered as a project
/// named `name`. Answered with `ack {project_id}` once done; meanwhile git's progress comes as
/// `clone_progress {folder, line}` to the machine that asked. A folder that exists and isn't
/// empty is refused; a failed clone is `error {code: "clone_failed", message}`, the message being
/// what git said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneProjectRequest {
    /// `https://…`, `http://…`, `ssh://…`, `git://…` or `user@host:path`. Nothing else: no
    /// local paths, `file://` or `ext::` (which runs a command), nothing git could read as an
    /// option.
    pub url: String,
    /// Where on B: absolute, or under the home folder (`~/trek`).
    pub folder: String,
    /// The project's name in B's sidebar.
    pub name: String,
}

impl CloneProjectRequest {
    /// Whether the server passes it to the host: a URL git only fetches from, a folder it can
    /// only take as a folder, a name.
    pub fn check(&self) -> Result<(), String> {
        if !clone_url_ok(&self.url) {
            return Err("Clone from an https://, ssh://, git:// or user@host:path URL".into());
        }
        if !clone_folder_ok(&self.folder) {
            return Err("The folder must be absolute or under ~/, without ..".into());
        }
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 200 || name.chars().any(char::is_control) {
            return Err("The project needs a name".into());
        }
        Ok(())
    }
}

fn clone_url_ok(url: &str) -> bool {
    if url.is_empty() || url.len() > 2048 || url.starts_with('-') || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    if let Some((scheme, rest)) = url.split_once("://") {
        return matches!(scheme.to_ascii_lowercase().as_str(), "https" | "http" | "ssh" | "git") && !rest.is_empty() && !rest.starts_with('-');
    }
    // scp-like: `[user@]host:path`, with no `/` before the colon (else git reads a local path)
    // and not `transport::address` (git's remote helpers, `ext::` among them).
    match url.split_once(':') {
        // A one-letter "host" is a Windows drive (`C:/repo`), a local path again.
        Some((host, path)) => host.len() > 1 && !host.contains('/') && !path.is_empty() && !path.starts_with(':'),
        None => false,
    }
}

fn clone_folder_ok(folder: &str) -> bool {
    if folder.is_empty() || folder.len() > 4096 || folder.chars().any(char::is_control) {
        return false;
    }
    let bytes = folder.as_bytes();
    let rest = if let Some(rest) = folder.strip_prefix("~/") {
        rest
    } else if folder.starts_with('/') {
        folder
    } else if bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\') {
        // A Windows machine's `C:\Users\…`.
        &folder[3..]
    } else {
        return false;
    };
    let mut parts = rest.split(['/', '\\']).filter(|p| !p.is_empty()).peekable();
    parts.peek().is_some() && parts.all(|p| p != "..")
}

/// The most `put_files` carries in one message, decoded (the 8 MiB frame limit caps the base64
/// text below that anyway: A sends several messages for more).
pub const MAX_PUT_FILES: usize = 8 << 20;

/// `put_files`: files to write under a project's folder on B (the ignored files of a checkout,
/// an `.env`), answered with `ack`. Paths are relative to the project folder, `/`-separated, with
/// no `..`, no root or drive and nothing inside `.git`; the server refuses the whole message
/// (`bad_request`) when one isn't. The host writes them under the folder only, never through a
/// symbolic link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutFilesRequest {
    pub project_id: String,
    pub files: Vec<PutFile>,
}

/// One file of a `put_files`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutFile {
    pub path: String,
    /// Its bytes, base64.
    pub bytes_b64: String,
    /// Unix permission bits as a number (`0o600` is 384); missing is `0o644`. Bits beyond
    /// `0o777` (setuid, setgid, sticky) are dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
}

impl PutFile {
    /// `bytes` at `path`, for the wire.
    pub fn new(path: impl Into<String>, bytes: &[u8], mode: Option<u32>) -> Self {
        Self { path: path.into(), bytes_b64: super::base64(bytes), mode }
    }
}

/// A file of a `put_files`, checked and decoded: what the host writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileUpload {
    /// Relative to the project folder, `/`-separated, inside it (see [`safe_relative_path`]).
    pub path: String,
    pub bytes: Vec<u8>,
    /// Permission bits, `0o777` at most.
    pub mode: u32,
}

impl PutFilesRequest {
    /// The files, checked (paths inside the project, each once, [`MAX_PUT_FILES`] in all) and
    /// decoded.
    pub fn decode(&self) -> Result<Vec<FileUpload>, String> {
        use base64::Engine as _;
        if self.project_id.trim().is_empty() {
            return Err("Which project?".into());
        }
        let mut total = 0usize;
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::with_capacity(self.files.len());
        for file in &self.files {
            if !safe_relative_path(&file.path) {
                return Err(format!("{:?} isn't a path inside the project", file.path));
            }
            if !seen.insert(file.path.as_str()) {
                return Err(format!("{:?} comes twice", file.path));
            }
            let bytes = base64::engine::general_purpose::STANDARD.decode(file.bytes_b64.trim()).map_err(|e| format!("{:?}: not base64: {e}", file.path))?;
            total += bytes.len();
            if total > MAX_PUT_FILES {
                return Err(format!("More than {} MiB in one message", MAX_PUT_FILES >> 20));
            }
            out.push(FileUpload { path: file.path.clone(), bytes, mode: file.mode.unwrap_or(0o644) & 0o777 });
        }
        Ok(out)
    }
}

/// A path `put_files` may write: relative, `/`- (or `\`-) separated parts with none empty, `.`,
/// `..` or `.git`; no root, no drive (`C:`), no NUL.
pub fn safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
        return false;
    }
    let bytes = path.as_bytes();
    if matches!(bytes[0], b'/' | b'\\') || std::path::Path::new(path).is_absolute() || (bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()) {
        return false;
    }
    path.split(['/', '\\']).all(|part| !part.is_empty() && part != "." && part != ".." && !part.eq_ignore_ascii_case(".git"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_project() {
        for ok in [".env", "config/local.toml", "a/b/c.txt", "weird name.txt", "..env", "x..y/z"] {
            assert!(safe_relative_path(ok), "{ok}");
        }
        for bad in ["", "../x", "a/../../x", "..", "/etc/passwd", "\\x", "C:\\x", "c:x", "a//b", "./a", "a/./b", "a\\..\\b", ".git/config", "sub/.GIT/hooks/x", "a\0b", "a/"] {
            assert!(!safe_relative_path(bad), "{bad:?}");
        }
    }

    #[test]
    fn put_files_decode_checks_every_file() {
        let req = |files: Vec<PutFile>| PutFilesRequest { project_id: "p".into(), files };
        let files = req(vec![PutFile::new(".env", b"KEY=1\n", None), PutFile::new("bin/run", b"#!/bin/sh\n", Some(0o4755))]).decode().unwrap();
        assert_eq!(files[0], FileUpload { path: ".env".into(), bytes: b"KEY=1\n".to_vec(), mode: 0o644 });
        assert_eq!(files[1].mode, 0o755, "setuid is dropped");
        assert!(req(vec![PutFile::new("../x", b"", None)]).decode().is_err());
        assert!(req(vec![PutFile::new("a", b"", None), PutFile::new("a", b"", None)]).decode().is_err());
        assert!(req(vec![PutFile { path: "a".into(), bytes_b64: "not base64!".into(), mode: None }]).decode().is_err());
        let big = vec![0u8; MAX_PUT_FILES / 2 + 1];
        assert!(req(vec![PutFile::new("a", &big, None), PutFile::new("b", &big, None)]).decode().unwrap_err().contains("MiB"));
    }

    #[test]
    fn clone_requests_take_only_fetchable_urls_and_plain_folders() {
        let req = |url: &str, folder: &str| CloneProjectRequest { url: url.into(), folder: folder.into(), name: "trek".into() };
        for url in ["https://github.com/dokyit/trek.git", "ssh://git@github.com/dokyit/trek", "git@github.com:dokyit/trek.git", "git://host/x"] {
            assert_eq!(req(url, "~/trek").check(), Ok(()), "{url}");
        }
        for url in ["", "-uhttps://x", "--upload-pack=touch /tmp/x", "ext::sh -c touch% /tmp/pwned", "file:///etc", "/tmp/repo", "./repo", "fd::17", "https://", "https://x y", "C:/repo"] {
            assert!(req(url, "~/trek").check().is_err(), "{url}");
        }
        for folder in ["/Users/me/trek", "~/code/trek", "C:\\Users\\me\\trek"] {
            assert_eq!(req("https://h/r", folder).check(), Ok(()), "{folder}");
        }
        for folder in ["", "trek", "~", "~/", "/", "~/../etc", "/tmp/../etc", "-x"] {
            assert!(req("https://h/r", folder).check().is_err(), "{folder}");
        }
        assert!(CloneProjectRequest { name: " ".into(), ..req("https://h/r", "~/r") }.check().is_err());
    }
}
