//! Built-in updater: check the release feed, download in the background, verify (SHA-256 +
//! minisign), unpack and inspect the new bundle, then swap it in place of the running one — on
//! "Restart to update" or when Trek quits, never during an agent turn.
//!
//! Manifest (`{channel}.json`, an asset on the channel's GitHub release):
//! ```json
//! { "version": "0.2.0", "notes": "…", "pub_date": "2026-10-01T00:00:00Z",
//!   "platforms": { "darwin-aarch64": { "url": "…/Trek-0.2.0-darwin-aarch64.app.tar.gz",
//!                                      "sha256": "…", "signature": "<minisign .minisig file>" } } }
//! ```
//! The signature's trusted comment is `Trek <version> <platform>`; it must name the version the
//! manifest offers, so an older signed archive can't be served as a newer one.
//!
//! Channels on GitHub Releases (see docs/RELEASING.md): stable is the latest non-prerelease
//! release (`releases/latest/download/stable.json`); beta and nightly are prereleases under the
//! fixed tags `beta` and `nightly`, whose assets are replaced on every publish
//! (`releases/download/beta/beta.json`). Beta also sees stable releases, nightly sees both.
//!
//! Windows: the artifact (`windows-x86_64`) is a flat zip of `TREK_FILES`, and Trek is installed
//! as the folder holding them. Trek can't replace the folder it runs from, so `install` only gets
//! the staged copy ready and `trek-update.exe` (crates/trek-update) swaps it in once Trek has quit.

use crate::settings::{Channel, Updates};
use anyhow::{Context as _, Result, anyhow, bail};
use futures::StreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Where official releases are published.
pub const OFFICIAL_FEED: &str = "https://github.com/dokyit/Trek/releases";

/// The release signing key (minisign) every build from this repository trusts. Forks that publish
/// their own releases build with `TREK_UPDATE_PUBKEY=<base64 key line>`.
const EMBEDDED_PUBLIC_KEY: &str = include_str!("../../../assets/update/minisign.pub");

pub fn public_key() -> &'static str {
    match option_env!("TREK_UPDATE_PUBKEY").map(str::trim) {
        Some(k) if !k.is_empty() => k,
        _ => key_line(EMBEDDED_PUBLIC_KEY),
    }
}

/// The base64 line of a `minisign.pub` file (skipping its comment), or the input if it's bare.
fn key_line(file: &str) -> &str {
    file.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("untrusted comment:")).unwrap_or("")
}

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: String,
    pub platforms: HashMap<String, Artifact>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Artifact {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub signature: String,
}

#[derive(Debug, Clone)]
pub struct AvailableUpdate {
    pub version: semver::Version,
    pub notes: String,
    pub pub_date: String,
    pub artifact: Artifact,
}

pub fn platform_key() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

pub fn current_version() -> semver::Version {
    semver::Version::parse(crate::VERSION).expect("valid crate version")
}

/// Why this copy of Trek can't replace itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocker {
    /// Not a release: `cargo run`, or a bundle built locally with `script/bundle.sh`. Rebuilding
    /// is the update; a published release would replace the newer code it was built from.
    DevBuild,
    /// macOS runs a quarantined app from a read-only copy until it's moved out of Downloads.
    Translocated,
    /// Trek can't write where it's installed (a standard account with Trek in /Applications, a
    /// copy an administrator installed): an update could be downloaded but never installed.
    ReadOnlyLocation,
    /// Windows: Trek is in Program Files (or WindowsApps), which only an administrator changes.
    ProtectedFolder,
    /// Windows: Trek's folder holds more than Trek (it was unzipped straight into Downloads, say).
    /// An update replaces the whole folder, so it never happens there.
    SharedFolder,
}

impl Blocker {
    pub fn message(self) -> &'static str {
        match self {
            Blocker::DevBuild => "This is a development build. It updates when you rebuild it.",
            Blocker::Translocated => "macOS is running Trek from a read-only copy. Move Trek to Applications to get updates.",
            Blocker::ReadOnlyLocation => "Trek can't write to the folder it's in, so it can't update itself. Move it to a folder you own to get updates.",
            Blocker::ProtectedFolder => "Trek is in Program Files, which only an administrator can change, so it can't update itself there. Move the Trek folder to one you own to get updates.",
            Blocker::SharedFolder => "Trek's folder has other files in it, and an update replaces the whole folder, so Trek can't update itself there. Put Trek's three files in a folder of their own to get updates.",
        }
    }
}

/// The `Info.plist` key `script/release.sh` stamps release bundles with (`script/bundle.sh` on its
/// own leaves it out).
const RELEASE_KEY: &str = "TrekRelease";

/// Lets a bundle built locally update itself anyway (testing the updater).
pub const LOCAL_UPDATES_ENV: &str = "TREK_UPDATE_LOCAL_BUNDLE";

pub fn blocker() -> Option<Blocker> {
    if cfg!(windows) {
        // Settled once: the checks touch the disk, and the Updates page asks on every frame.
        static WINDOWS: std::sync::OnceLock<Option<Blocker>> = std::sync::OnceLock::new();
        return *WINDOWS.get_or_init(|| {
            let Some(root) = install_root() else { return Some(Blocker::DevBuild) };
            let release = std::env::var(LOCAL_UPDATES_ENV).is_ok_and(|v| v == "1") || !is_cargo_output(&root);
            windows_blocker_for(&root, release, &protected_folders())
        });
    }
    static RELEASE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let Some(bundle) = running_bundle() else { return Some(Blocker::DevBuild) };
    let release = *RELEASE.get_or_init(|| std::env::var(LOCAL_UPDATES_ENV).is_ok_and(|v| v == "1") || is_release(&bundle));
    blocker_for(&bundle, release)
}

fn blocker_for(bundle: &Path, release: bool) -> Option<Blocker> {
    if !release {
        Some(Blocker::DevBuild)
    } else if bundle.to_string_lossy().contains("/AppTranslocation/") {
        Some(Blocker::Translocated)
    } else if !writable(bundle) || !bundle.parent().is_some_and(writable) {
        // Installing renames the bundle out of its folder and a new one in.
        Some(Blocker::ReadOnlyLocation)
    } else {
        None
    }
}

/// Whether `app` is a published release (`RELEASE_KEY`).
fn is_release(app: &Path) -> bool {
    plist_value(app, RELEASE_KEY).is_ok_and(|v| v == "true")
}

#[cfg(unix)]
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: a valid NUL-terminated path that outlives the call.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// A folder's read-only attribute doesn't stop files being created in it and there is no
/// `access(W_OK)` that honours ACLs, so try: create and remove a file in a folder, open a file
/// for writing (without truncating it).
#[cfg(windows)]
fn writable(path: &Path) -> bool {
    if path.is_dir() {
        let probe = path.join(format!(".trek-write-test-{}", std::process::id()));
        let ok = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe).is_ok();
        let _ = std::fs::remove_file(&probe);
        ok
    } else {
        std::fs::OpenOptions::new().write(true).open(path).is_ok()
    }
}

/// The `.app` bundle we're running from, if any.
pub fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

// ---------- the Windows install folder ----------

/// Everything a Windows install holds: the release zip is flat, these and nothing else (assets
/// are embedded). `trek-update`'s `TREK_FILES` is the same list. A file added to the release
/// must be added here, in a release before the one that ships it: older versions refuse an update
/// with a file they don't know, and a folder holding one.
pub const TREK_FILES: &[&str] = &["trek.exe", "trek-mcp.exe", "trek-update.exe"];
const TREK_EXE: &str = "trek.exe";
const HELPER_EXE: &str = "trek-update.exe";

/// The folder Trek runs from on Windows: the one holding `trek.exe`, which an update replaces.
fn install_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.to_str().and_then(|s| s.strip_prefix(r"\\?\")).map(PathBuf::from).unwrap_or(exe);
    exe.parent().map(Path::to_path_buf)
}

/// A build cargo left where it built it (`target\debug`, its `deps`): a development build.
fn is_cargo_output(root: &Path) -> bool {
    root.ancestors().take(2).any(|d| d.join(".fingerprint").is_dir())
}

/// Program Files, wherever this Windows keeps it.
fn protected_folders() -> Vec<PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"].into_iter().filter_map(std::env::var_os).map(PathBuf::from).filter(|p| p.is_absolute()).collect()
}

fn windows_blocker_for(root: &Path, release: bool, protected: &[PathBuf]) -> Option<Blocker> {
    let windows_apps = root.components().any(|c| c.as_os_str().eq_ignore_ascii_case("WindowsApps"));
    if !release {
        Some(Blocker::DevBuild)
    } else if windows_apps || protected.iter().any(|p| inside(root, p)) {
        Some(Blocker::ProtectedFolder)
    } else if !own_folder(root) {
        Some(Blocker::SharedFolder)
    } else if !writable(root) || !root.parent().is_some_and(writable) {
        // Installing renames the folder out of its parent and the update in.
        Some(Blocker::ReadOnlyLocation)
    } else {
        None
    }
}

/// `path` is `folder` or inside it, as Windows compares paths: whatever the case, with or
/// without `\\?\`.
fn inside(path: &Path, folder: &Path) -> bool {
    let parts = |p: &Path| -> Vec<String> {
        let s = p.to_string_lossy().to_lowercase();
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        Path::new(s).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect()
    };
    let (path, folder) = (parts(path), parts(folder));
    !folder.is_empty() && path.starts_with(&folder)
}

/// A folder of Trek's own: `trek.exe`, and nothing that isn't one of Trek's files.
fn own_folder(dir: &Path) -> bool {
    dir.join(TREK_EXE).is_file() && only_trek_files(dir)
}

/// Every entry is one of `TREK_FILES` (or the moment's probe `writable` makes); none is a folder.
fn only_trek_files(dir: &Path) -> bool {
    let Ok(mut entries) = std::fs::read_dir(dir) else { return false };
    entries.all(|e| {
        e.is_ok_and(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            e.file_type().is_ok_and(|t| t.is_file()) && (TREK_FILES.contains(&name.as_str()) || name.starts_with(".trek-write-test-"))
        })
    })
}

/// Delete what updates left next to the install, now that this version runs: `<install>.old-*`,
/// the version an update replaced or one that failed to start. Only folders of Trek's files.
fn clean_old_installs(root: &Path) {
    let (Some(parent), Some(name)) = (root.parent(), root.file_name()) else { return };
    let prefix = format!("{}.old-", name.to_string_lossy().to_lowercase());
    for entry in std::fs::read_dir(parent).into_iter().flatten().flatten() {
        let leftover = entry.file_name().to_string_lossy().to_lowercase().starts_with(&prefix);
        // `file_type` doesn't follow links: a junction is never a folder here.
        if !leftover || !entry.file_type().is_ok_and(|t| t.is_dir()) || !only_trek_files(&entry.path()) {
            continue;
        }
        match std::fs::remove_dir_all(entry.path()) {
            Ok(()) => tracing::info!("removed {}, left by an update", entry.path().display()),
            Err(e) => tracing::warn!("couldn't remove {}, left by an update: {e}", entry.path().display()),
        }
    }
}

fn channel_key(c: Channel) -> &'static str {
    match c {
        Channel::Stable => "stable",
        Channel::Beta => "beta",
        Channel::Nightly => "nightly",
    }
}

/// Manifest URLs for the chosen channel, newest channel first. `feed_url` is a GitHub releases
/// URL (`https://github.com/<owner>/<repo>/releases`), a template with `{channel}`, or one
/// manifest URL ending in `.json` used for every channel.
pub fn manifest_urls(updates: &Updates) -> Vec<String> {
    let feed = updates.feed_url.trim().trim_end_matches('/');
    let feed = if feed.is_empty() { OFFICIAL_FEED } else { feed };
    if feed.ends_with(".json") && !feed.contains("{channel}") {
        return vec![feed.to_string()];
    }
    let channels: &[Channel] = match updates.channel {
        Channel::Stable => &[Channel::Stable],
        Channel::Beta => &[Channel::Beta, Channel::Stable],
        Channel::Nightly => &[Channel::Nightly, Channel::Beta, Channel::Stable],
    };
    channels
        .iter()
        .map(|&c| {
            let key = channel_key(c);
            if feed.contains("{channel}") {
                feed.replace("{channel}", key)
            } else if c == Channel::Stable {
                format!("{feed}/latest/download/stable.json")
            } else {
                format!("{feed}/download/{key}/{key}.json")
            }
        })
        .collect()
}

pub(crate) fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(format!("Trek/{}", crate::VERSION))
        .connect_timeout(std::time::Duration::from_secs(15))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()?)
}

/// Couldn't reach or read from the update server: worth retrying soon, unlike a release that
/// doesn't verify.
#[derive(Debug)]
struct NetError(String);

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NetError {}

/// A network failure in a sentence, without reqwest's URL-and-cause chain.
fn net_error(e: reqwest::Error) -> anyhow::Error {
    let host = e.url().and_then(|u| u.host_str()).unwrap_or("the update server").to_string();
    let message = if e.is_connect() {
        format!("couldn't connect to {host}")
    } else if e.is_timeout() {
        format!("{host} took too long to answer")
    } else if let Some(status) = e.status() {
        format!("{host} answered {status}")
    } else {
        e.without_url().to_string()
    };
    anyhow::Error::new(NetError(message))
}

/// Whether an update failed for a reason that may pass (offline, timeout, server error) rather
/// than because the release itself is bad.
pub fn is_transient(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<NetError>())
}

async fn fetch_manifest(client: &reqwest::Client, url: &str) -> Result<Manifest> {
    let resp = client.get(url).timeout(std::time::Duration::from_secs(20)).send().await.map_err(net_error)?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("no release has been published on this channel yet");
    }
    let bytes = resp.error_for_status().map_err(net_error)?.bytes().await.map_err(net_error)?;
    serde_json::from_slice(&bytes).with_context(|| format!("unreadable update manifest at {url}"))
}

/// Returns `Some` when any of the feeds has a build for this platform newer than this one (the
/// newest of them). Feeds that fail are skipped unless all of them do.
pub async fn check(urls: &[String]) -> Result<Option<AvailableUpdate>> {
    let client = http()?;
    let results = futures::future::join_all(urls.iter().map(|u| fetch_manifest(&client, u))).await;
    newest(results, &current_version(), skipped_version(&crate::paths::updates_dir()).as_ref())
}

/// The version a rollback turned away from (`SKIP_MARKER`), if any.
fn skipped_version(updates: &Path) -> Option<semver::Version> {
    semver::Version::parse(std::fs::read_to_string(updates.join(SKIP_MARKER)).ok()?.trim()).ok()
}

/// The newest build above `current` across the feeds' manifests, `skipped` aside; the first
/// error only if every feed failed.
fn newest(results: Vec<Result<Manifest>>, current: &semver::Version, skipped: Option<&semver::Version>) -> Result<Option<AvailableUpdate>> {
    let mut best: Option<AvailableUpdate> = None;
    let mut first_error = None;
    let mut any_ok = false;
    for result in results {
        match result.and_then(|m| newer_than(m, current)) {
            Ok(found) => {
                let found = found.filter(|u| skipped != Some(&u.version));
                any_ok = true;
                if let Some(u) = found.filter(|u| best.as_ref().is_none_or(|b| u.version > b.version)) {
                    best = Some(u);
                }
            }
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    match (any_ok, first_error) {
        (false, Some(e)) => Err(e),
        _ => Ok(best),
    }
}

/// The manifest's build for this platform, if it's newer than `current`. Never a downgrade.
fn newer_than(manifest: Manifest, current: &semver::Version) -> Result<Option<AvailableUpdate>> {
    let version = semver::Version::parse(manifest.version.trim().trim_start_matches('v'))?;
    if version <= *current {
        return Ok(None);
    }
    let Some(artifact) = manifest.platforms.get(&platform_key()).cloned() else {
        return Ok(None);
    };
    Ok(Some(AvailableUpdate { version, notes: manifest.notes.trim().to_string(), pub_date: manifest.pub_date, artifact }))
}

/// Download into a new download folder, reporting progress in 0.0..=1.0. Returns the archive only
/// once its SHA-256 and minisign signature check out; a bad or cancelled download is deleted.
pub async fn download(update: &AvailableUpdate, cancel: &AtomicBool, mut progress: impl FnMut(f32)) -> Result<PathBuf> {
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;

    // Refuse before downloading anything if the release isn't signed for this version.
    let key = minisign_verify::PublicKey::from_base64(public_key()).context("this build's update key is invalid")?;
    let signature = signature_for(&update.artifact.signature, &update.version)?;
    let mut verifier = key.verify_stream(&signature).context("the update isn't signed with this build's release key")?;

    let dir = new_download_dir(&crate::paths::updates_dir())?;
    let dest = dir.join(archive_name(&update.version));
    let result: Result<()> = async {
        let resp = http()?.get(&update.artifact.url).send().await.and_then(|r| r.error_for_status()).map_err(net_error)?;
        let total = resp.content_length().unwrap_or(0);
        let mut file = tokio::fs::File::create(&dest).await?;
        let mut hasher = sha2::Sha256::new();
        let mut got = 0u64;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            let chunk = chunk.map_err(net_error)?;
            hasher.update(&chunk);
            verifier.update(&chunk);
            file.write_all(&chunk).await?;
            got += chunk.len() as u64;
            if total > 0 {
                progress(got as f32 / total as f32);
            }
        }
        file.flush().await?;
        let digest = hex::encode(hasher.finalize());
        if !digest.eq_ignore_ascii_case(update.artifact.sha256.trim()) {
            bail!("the download is damaged (checksum mismatch)");
        }
        verifier.finalize().map_err(|_| anyhow!("the update's signature doesn't match; it was not installed"))?;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return Err(e);
    }
    progress(1.0);
    Ok(dest)
}

/// What the download is saved as: the macOS app is a `.tar.gz`, the Windows folder a `.zip`.
fn archive_name(version: &semver::Version) -> String {
    if cfg!(windows) { format!("Trek-{version}.zip") } else { format!("Trek-{version}.tar.gz") }
}

/// Decode a release signature and check its trusted comment names `version`.
fn signature_for(text: &str, version: &semver::Version) -> Result<minisign_verify::Signature> {
    if text.trim().is_empty() {
        bail!("the update isn't signed; it was not downloaded");
    }
    let sig = minisign_verify::Signature::decode(text.trim()).map_err(|_| anyhow!("the update's signature is unreadable"))?;
    let v = version.to_string();
    if !sig.trusted_comment().split_whitespace().any(|w| w.trim_start_matches('v') == v) {
        bail!("the update's signature is for another version");
    }
    Ok(sig)
}

/// Check a whole file against a minisign signature (used where the bytes are already on disk).
pub fn verify_file(path: &Path, signature: &str, public_key: &str) -> Result<()> {
    let key = minisign_verify::PublicKey::from_base64(key_line(public_key)).context("bad public key")?;
    let sig = minisign_verify::Signature::decode(signature.trim()).context("bad signature")?;
    key.verify(&std::fs::read(path)?, &sig, false).map_err(|_| anyhow!("signature verification failed"))
}

// ---------- staging and install ----------

/// A new folder for one download, `updates/download-<pid>-<n>.noindex/`: the archive, then the
/// unpacked app waiting to be installed. It's named after the process that owns it, so another
/// Trek sharing the data folder (a second copy, a dev build) leaves it alone. `.noindex` keeps
/// Spotlight from listing the waiting copy as a second Trek.
fn new_download_dir(updates: &Path) -> Result<PathBuf> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = updates.join(format!("download-{}-{}.noindex", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The process that owns a download folder, if `name` is one.
fn download_owner(name: &str) -> Option<u32> {
    name.strip_prefix("download-")?.strip_suffix(".noindex")?.split('-').next()?.parse().ok()
}

/// The download folder a staged app sits in.
fn download_dir_of(staged: &Path) -> Option<&Path> {
    staged.parent().filter(|d| d.file_name().and_then(OsStr::to_str).and_then(download_owner).is_some())
}

/// Delete a staged update that won't be installed (the channel changed under it). Blocking.
pub fn discard(staged: &Path) {
    if let Some(dir) = download_dir_of(staged) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Unpack a verified archive next to itself and inspect the bundle inside: the same app, exactly
/// `expected`, newer than this one, intact code signature, no quarantine. Returns the staged
/// `.app`; the archive is removed, and on failure the whole download folder. Blocking.
pub fn stage(archive: &Path, expected: &semver::Version, cancel: &AtomicBool) -> Result<PathBuf> {
    if cfg!(windows) {
        return stage_zip(archive, expected, &current_version(), cancel, exe_version);
    }
    let dir = archive.parent().context("the download has no folder")?;
    let result = (|| {
        let bundle = running_bundle().context("not running from an app bundle")?;
        run("/usr/bin/tar", [OsStr::new("-xzf"), archive.as_os_str(), OsStr::new("-C"), dir.as_os_str()]).context("couldn't unpack the update")?;
        let app = std::fs::read_dir(dir)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .context("the update archive has no app in it")?;
        check_bundle(&app, &bundle_id(&bundle)?, expected, &current_version())?;
        // Downloaded by Trek itself, so normally unquarantined; an archive fetched by a browser
        // would carry the flag into every file.
        let _ = std::process::Command::new("/usr/bin/xattr").arg("-dr").arg("com.apple.quarantine").arg(&app).output();
        run("/usr/bin/codesign", [OsStr::new("--verify"), OsStr::new("--strict"), app.as_os_str()]).context("the update's code signature is broken")?;
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        Ok(app)
    })();
    let _ = std::fs::remove_file(archive);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(dir);
    }
    result
}

fn run<'a>(program: &str, args: impl IntoIterator<Item = &'a OsStr>) -> Result<()> {
    let out = std::process::Command::new(program).args(args).output()?;
    if !out.status.success() {
        bail!("{program} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

fn plist_value(app: &Path, key: &str) -> Result<String> {
    let out = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(app.join("Contents/Info.plist"))
        .output()?;
    if !out.status.success() {
        bail!("{} has no {key}", app.display());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn bundle_id(app: &Path) -> Result<String> {
    plist_value(app, "CFBundleIdentifier")
}

/// The bundle's full version: `CFBundleVersion` carries the semver (pre-release included).
fn bundle_version(app: &Path) -> Result<semver::Version> {
    let v = plist_value(app, "CFBundleVersion")?;
    semver::Version::parse(&v).with_context(|| format!("the update has an unreadable version ({v})"))
}

fn check_bundle(app: &Path, id: &str, expected: &semver::Version, current: &semver::Version) -> Result<()> {
    let found = bundle_id(app)?;
    if found != id {
        bail!("the update is a different app ({found})");
    }
    let version = bundle_version(app)?;
    if version != *expected {
        bail!("the update contains version {version}, not {expected}");
    }
    if version <= *current {
        bail!("the update ({version}) isn't newer than this version ({current})");
    }
    Ok(())
}

/// The most a Windows release may unpack to.
const MAX_UNPACKED: u64 = 1 << 30;

/// Windows: unpack a verified zip into `stage` next to it and check it: Trek's files and nothing
/// else, and a `trek.exe` of exactly `expected`, newer than `current`. Returns the staged folder;
/// the archive is removed, and on failure the whole download folder.
fn stage_zip(archive: &Path, expected: &semver::Version, current: &semver::Version, cancel: &AtomicBool, version_of: impl Fn(&Path) -> Result<semver::Version>) -> Result<PathBuf> {
    let dir = archive.parent().context("the download has no folder")?;
    let staged = dir.join("stage");
    let result = (|| {
        unpack_release_zip(archive, &staged, MAX_UNPACKED)?;
        check_version(&version_of(&staged.join(TREK_EXE))?, expected, current)?;
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        Ok(staged.clone())
    })();
    let _ = std::fs::remove_file(archive);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(dir);
    }
    result
}

/// Unpack a Windows release: each of `TREK_FILES` once, at the top, as a plain file, and nothing
/// else (no folders, no links, no other names), `limit` bytes at most. The names are the fixed
/// list, so nothing can land outside `into`.
fn unpack_release_zip(archive: &Path, into: &Path, limit: u64) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|_| anyhow!("the update isn't a zip archive"))?;
    if zip.len() > TREK_FILES.len() {
        bail!("the update has more in it than Trek ({} entries)", zip.len());
    }
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir(into)?;
    let mut left = limit;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| anyhow!("couldn't unpack the update: {e}"))?;
        let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
        if !TREK_FILES.contains(&name.as_str()) || !entry.is_file() || entry.is_symlink() {
            bail!("the update has something in it that isn't Trek ({name})");
        }
        if entry.size() > left {
            bail!("the update is larger than Trek could be");
        }
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(into.join(&name)).with_context(|| format!("couldn't unpack {name} from the update"))?;
        // The size the archive states can lie: stop past what's left either way.
        let copied = std::io::copy(&mut std::io::Read::take(&mut entry, left + 1), &mut out)?;
        if copied > left {
            bail!("the update is larger than Trek could be");
        }
        left -= copied;
    }
    if let Some(missing) = TREK_FILES.iter().find(|f| !into.join(f).is_file()) {
        bail!("the update has no {missing} in it");
    }
    Ok(())
}

fn check_version(found: &semver::Version, expected: &semver::Version, current: &semver::Version) -> Result<()> {
    if found != expected {
        bail!("the update contains version {found}, not {expected}");
    }
    if found <= current {
        bail!("the update ({found}) isn't newer than this version ({current})");
    }
    Ok(())
}

/// The full version of a `trek.exe`: the `ProductVersion` its version resource carries
/// (trek-app's build.rs writes the semver there, pre-release included).
#[cfg(windows)]
fn exe_version(exe: &Path) -> Result<semver::Version> {
    let v = exe_product_version(exe).with_context(|| format!("{} has no version", exe.display()))?;
    semver::Version::parse(v.trim()).with_context(|| format!("the update has an unreadable version ({v})"))
}

#[cfg(not(windows))]
fn exe_version(exe: &Path) -> Result<semver::Version> {
    bail!("{} has no version Trek can read here", exe.display())
}

/// The `ProductVersion` string of a Windows executable, in its first language.
#[cfg(windows)]
fn exe_product_version(exe: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let wide = |s: &OsStr| s.encode_wide().chain([0]).collect::<Vec<u16>>();
    let path = wide(exe.as_os_str());
    // SAFETY: a NUL-terminated path; a buffer of at least the size asked for, DWORD-aligned like
    // the structures in it (so the UTF-16 strings in it are aligned); VerQueryValueW points into
    // that buffer, which outlives every read, for the length it reports.
    unsafe {
        let size = GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut data = vec![0u32; size.div_ceil(4) as usize];
        if GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let query = |sub: &str| {
            let sub = wide(OsStr::new(sub));
            let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
            let found = VerQueryValueW(data.as_ptr().cast(), sub.as_ptr(), &mut ptr, &mut len) != 0 && !ptr.is_null() && len > 0;
            found.then_some((ptr as *const u8, len))
        };
        // Language and code page, as two little-endian u16s.
        let (t, n) = query(r"\VarFileInfo\Translation")?;
        if n < 4 {
            return None;
        }
        let t = std::slice::from_raw_parts(t, 4);
        let (lang, codepage) = (u16::from_le_bytes([t[0], t[1]]), u16::from_le_bytes([t[2], t[3]]));
        let (s, n) = query(&format!(r"\StringFileInfo\{lang:04x}{codepage:04x}\ProductVersion"))?;
        // A string's length is in characters, its NUL included.
        let chars = std::slice::from_raw_parts(s.cast::<u16>(), n as usize);
        Some(String::from_utf16_lossy(chars).trim_end_matches('\0').to_string())
    }
}

/// What `install` left: the app to open, and the version it replaced, if it replaced one and
/// kept it. The relaunch puts that backup back only if the new app won't open.
#[derive(Debug, PartialEq)]
pub struct Installed {
    pub bundle: PathBuf,
    pub backup: Option<PathBuf>,
    /// Windows: the staged folder `trek-update` swaps in for `bundle` (the install folder) once
    /// Trek has quit. `None` when the install is already at least that version.
    pub staged: Option<PathBuf>,
}

/// Swap the staged bundle in for the running one and keep the old one as the single backup.
/// If anything fails before the swap, nothing has changed.
/// On Windows nothing moves yet: see `prepare_swap`.
pub fn install(staged: &Path) -> Result<Installed> {
    if cfg!(windows) {
        let root = install_root().context("Trek can't tell which folder it runs from")?;
        return prepare_swap(staged, &root, &crate::paths::updates_dir(), exe_version);
    }
    let bundle = running_bundle().context("not running from an app bundle (dev build)")?;
    let backup = install_into(staged, &bundle, &crate::paths::updates_dir())?;
    Ok(Installed { bundle, backup, staged: None })
}

/// Windows: Trek can't replace the folder it runs from, so the swap waits for `trek-update`,
/// started as Trek quits (`relaunch`, `swap_on_exit`). Here: the staged copy is still there, and
/// newer than the install (another copy of Trek may have updated it, or the user replaced it by
/// hand: then the download goes and nothing is swapped). The version that's running is noted for
/// "Trek updated" next time; a version that didn't install finds its own and says nothing.
fn prepare_swap(staged: &Path, root: &Path, updates: &Path, version_of: impl Fn(&Path) -> Result<semver::Version>) -> Result<Installed> {
    if !staged.join(TREK_EXE).is_file() {
        bail!("the downloaded update is gone; check again");
    }
    if let (Ok(installed), Ok(incoming)) = (version_of(&root.join(TREK_EXE)), version_of(&staged.join(TREK_EXE))) {
        if installed >= incoming {
            tracing::info!("{} is already Trek {installed}; not installing {incoming}", root.display());
            if let Some(dir) = download_dir_of(staged) {
                let _ = std::fs::remove_dir_all(dir);
            }
            return Ok(Installed { bundle: root.to_path_buf(), backup: None, staged: None });
        }
    }
    let _ = std::fs::write(updates.join(UPDATED_MARKER), crate::VERSION);
    Ok(Installed { bundle: root.to_path_buf(), backup: None, staged: Some(staged.to_path_buf()) })
}

/// Quitting with an update ready: on Windows `install` only got it ready, and `trek-update` swaps
/// it in once Trek has exited, without starting Trek again. Nothing to do on macOS, where
/// `install` has swapped it already.
pub fn swap_on_exit(installed: &Installed) -> Result<()> {
    if cfg!(windows) { hand_over(installed, None) } else { Ok(()) }
}

/// Windows: start `trek-update` to swap the staged folder in once this process has exited and,
/// with `relaunch` (in front or not), start Trek again.
fn hand_over(installed: &Installed, relaunch: Option<bool>) -> Result<()> {
    let root = &installed.bundle;
    let helper = match &installed.staged {
        // The staged copy's own helper: the install is renamed while it runs.
        Some(staged) => staged.join(HELPER_EXE),
        // Already up to date: only the restart, which the install's helper can do.
        None if relaunch.is_some() => root.join(HELPER_EXE),
        None => return Ok(()),
    };
    let to = installed.staged.as_deref().and_then(|s| exe_version(&s.join(TREK_EXE)).ok()).map(|v| v.to_string()).unwrap_or_default();
    let vars = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)));
    let mut cmd = helper_command(&helper, std::process::id(), root, installed.staged.as_deref(), &to, &crate::paths::updates_dir(), relaunch, vars);
    spawn_detached(&mut cmd).with_context(|| format!("couldn't start {}", helper.display()))
}

/// The `trek-update` command line (see crates/trek-update). Like the macOS relaunch, the Trek it
/// starts keeps this one's setup but not its one-time launch flags (`RELAUNCH_KEEPS`), and opens
/// behind other windows unless this one was in front.
#[allow(clippy::too_many_arguments)]
fn helper_command(
    helper: &Path,
    pid: u32,
    root: &Path,
    staged: Option<&Path>,
    to: &str,
    updates: &Path,
    relaunch: Option<bool>,
    vars: impl Iterator<Item = (String, String)>,
) -> std::process::Command {
    let mut cmd = std::process::Command::new(helper);
    cmd.arg("--pid").arg(pid.to_string()).arg("--install").arg(root).arg("--from").arg(crate::VERSION).arg("--updates").arg(updates);
    if let Some(staged) = staged {
        cmd.arg("--staged").arg(staged);
    }
    if !to.is_empty() {
        cmd.arg("--to").arg(to);
    }
    // The helper, and the Trek it starts, inherit the rest.
    for (k, _) in vars.filter(|(k, _)| k.starts_with("TREK_") && !RELAUNCH_KEEPS.contains(&k.as_str())) {
        cmd.env_remove(k);
    }
    if let Some(foreground) = relaunch {
        cmd.arg("--relaunch");
        if !foreground {
            cmd.env("TREK_BACKGROUND", "1");
        }
    }
    // Not the install: a process working in a folder keeps it from being renamed.
    if let Some(dir) = root.parent() {
        cmd.current_dir(dir);
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd
}

/// Start the helper on its own: no console, and out of any job Trek runs in (a terminal's, an
/// editor's), which could end it as Trek exits.
#[cfg(windows)]
fn spawn_detached(cmd: &mut std::process::Command) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt as _;
    use windows_sys::Win32::System::Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
    if cmd.spawn().is_ok() {
        return Ok(());
    }
    // A job that can't be left.
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    cmd.spawn().map(drop)
}

#[cfg(not(windows))]
fn spawn_detached(cmd: &mut std::process::Command) -> std::io::Result<()> {
    cmd.spawn().map(drop)
}

/// Returns the backup of the replaced version, if there is one.
fn install_into(staged: &Path, bundle: &Path, updates: &Path) -> Result<Option<PathBuf>> {
    let result = replace_bundle(staged, bundle, updates);
    if let Some(dir) = download_dir_of(staged) {
        let _ = std::fs::remove_dir_all(dir);
    }
    match result? {
        Replaced::No => Ok(None),
        Replaced::Yes { backup } => {
            let _ = std::fs::write(updates.join(UPDATED_MARKER), crate::VERSION);
            Ok(backup)
        }
    }
}

enum Replaced {
    /// The app on disk is already at least the staged version: another copy of Trek updated
    /// it, or the user installed a newer one by hand. Installing would downgrade it.
    No,
    Yes { backup: Option<PathBuf> },
}

fn replace_bundle(staged: &Path, bundle: &Path, updates: &Path) -> Result<Replaced> {
    if !staged.exists() {
        bail!("the downloaded update is gone; check again");
    }
    if let (Ok(installed), Ok(incoming)) = (bundle_version(bundle), bundle_version(staged)) {
        if installed >= incoming {
            tracing::info!("{} is already Trek {installed}; not installing {incoming}", bundle.display());
            return Ok(Replaced::No);
        }
    }
    let parent = bundle.parent().context("app has no folder")?;
    let name = bundle.file_name().context("app has no name")?;
    // Next to the bundle, so the swap is a rename on one volume.
    let work = parent.join(format!(".{}.update", name.to_string_lossy()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir(&work).with_context(|| format!("Trek can't write to {}; move it to a folder you own", parent.display()))?;
    let result = (|| {
        let incoming = work.join(name);
        move_dir(staged, &incoming)?;
        let replaced = swap(&incoming, bundle)?;
        let backup_dir = updates.join(PREVIOUS_DIR);
        let backup = backup_dir.join(name);
        let _ = std::fs::remove_dir_all(&backup);
        if std::fs::create_dir_all(&backup_dir).is_err() || move_dir(&replaced, &backup).is_err() {
            tracing::warn!("couldn't keep a backup of the previous version");
            return Ok(Replaced::Yes { backup: None });
        }
        Ok(Replaced::Yes { backup: Some(backup) })
    })();
    // A swap that couldn't put the current app back leaves it in `work`: the only copy then.
    if bundle.exists() {
        let _ = std::fs::remove_dir_all(&work);
    }
    result
}

/// Where the single backup of the version an update replaced is kept.
const PREVIOUS_DIR: &str = "previous.noindex";

/// Records the version an update replaced, so the new version can say it was just updated.
const UPDATED_MARKER: &str = "updated-from";

/// A version that was rolled back from: the relaunch helper writes it when the new app won't
/// open, and so can someone rolling back by hand (docs/RELEASING.md). Checks don't offer that
/// version again; a newer one is offered as usual.
pub const SKIP_MARKER: &str = "skip-version";

/// Rename, or copy then delete across volumes (`ditto` keeps signatures and attributes).
fn move_dir(from: &Path, to: &Path) -> Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    run("/usr/bin/ditto", [from.as_os_str(), to.as_os_str()]).context("couldn't move the update into place")?;
    let _ = std::fs::remove_dir_all(from);
    Ok(())
}

/// Exchange two directories; returns where the replaced bundle is now. One atomic
/// `renamex_np(RENAME_SWAP)` on APFS, two renames elsewhere.
fn swap(incoming: &Path, bundle: &Path) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let a = std::ffi::CString::new(incoming.as_os_str().as_bytes())?;
        let b = std::ffi::CString::new(bundle.as_os_str().as_bytes())?;
        // SAFETY: both are valid NUL-terminated paths that outlive the call.
        if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } == 0 {
            return Ok(incoming.to_path_buf());
        }
    }
    swap_by_rename(incoming, bundle)
}

/// The current app moves aside next to `incoming`, then `incoming` takes its place. If that
/// fails the current app goes back; if even that fails, the error says where it is.
fn swap_by_rename(incoming: &Path, bundle: &Path) -> Result<PathBuf> {
    let aside = incoming.with_extension("previous");
    std::fs::rename(bundle, &aside).context("couldn't move the current app aside")?;
    if let Err(e) = std::fs::rename(incoming, bundle) {
        if let Err(back) = std::fs::rename(&aside, bundle) {
            bail!("couldn't install the update ({e}) or put the current app back ({back}); it is at {}", aside.display());
        }
        return Err(e).context("couldn't install the update");
    }
    Ok(aside)
}

/// Waits for the old process to exit, then opens the new bundle; if that fails, puts the backup
/// back, records the new version as one to skip (so the version it went back to doesn't install
/// it again) and opens the backup instead. Arguments: pid, bundle, backup (empty: none), skip
/// marker, the new version, opener, then the opener's options.
const RELAUNCH_SCRIPT: &str = r#"
    pid=$1 bundle=$2 backup=$3 skip=$4 version=$5 opener=$6; shift 6
    while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
    "$opener" "$@" "$bundle" && exit 0
    if [ -n "$backup" ] && [ -d "$backup" ]; then
        /bin/mv "$bundle" "$bundle.failed" && /bin/mv "$backup" "$bundle" && /bin/rm -rf "$bundle.failed"
        printf '%s\n' "$version" > "$skip"
        "$opener" "$@" "$bundle"
    fi
"#;

fn relaunch_command(pid: u32, bundle: &Path, backup: Option<&Path>, skip: &Path, version: &str, opener: &str, open_args: &[String]) -> std::process::Command {
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c").arg(RELAUNCH_SCRIPT).arg("trek-relaunch").arg(pid.to_string()).arg(bundle);
    cmd.arg(backup.map(Path::as_os_str).unwrap_or_default()).arg(skip).arg(version).arg(opener).args(open_args);
    cmd
}

/// Start the installed bundle once this process has exited. If it can't be opened, the version
/// it replaced goes back in its place and that is opened instead; a bundle this update didn't
/// replace is never rolled back. The caller quits right after.
pub fn relaunch(installed: &Installed, foreground: bool) -> Result<()> {
    if cfg!(windows) {
        // Windows swaps the folder only now, once this process has exited (`hand_over`).
        return hand_over(installed, Some(foreground));
    }
    let mut open_args: Vec<String> = vec!["-n".into()];
    if !foreground {
        open_args.push("-g".into());
    }
    // Carry Trek's own environment (TREK_DATA_DIR, …) over to the new process; `open` starts
    // apps with launchd's environment otherwise.
    for (k, v) in relaunch_env(foreground) {
        open_args.push("--env".into());
        open_args.push(format!("{k}={v}"));
    }
    let version = bundle_version(&installed.bundle).map(|v| v.to_string()).unwrap_or_default();
    let skip = crate::paths::updates_dir().join(SKIP_MARKER);
    let mut helper = relaunch_command(std::process::id(), &installed.bundle, installed.backup.as_deref(), &skip, &version, "/usr/bin/open", &open_args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        helper.process_group(0);
    }
    helper.spawn().context("couldn't schedule the relaunch")?;
    Ok(())
}

/// What the relaunched Trek runs with: where its data lives and how it's set up, never the
/// one-time launch flags (`TREK_MOCK_PROMPT`, `TREK_OPEN_*`, `TREK_ONBOARDING`, measurement aids),
/// which would fire again after every update.
/// (`TREK_UPDATE_PUBKEY` isn't one: it's read when Trek is built, never at run time.)
const RELAUNCH_KEEPS: &[&str] = &["TREK_DATA_DIR", "TREK_UPDATE_AUTO_RESTART", LOCAL_UPDATES_ENV, "TREK_MOCK_AGENT", "TREK_AXE_PATH", "RUST_LOG"];

fn relaunch_env(foreground: bool) -> Vec<(String, String)> {
    relaunch_env_from(std::env::vars(), foreground)
}

fn relaunch_env_from(vars: impl Iterator<Item = (String, String)>, foreground: bool) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vars.filter(|(k, _)| RELAUNCH_KEEPS.contains(&k.as_str())).collect();
    if !foreground {
        env.push(("TREK_BACKGROUND".into(), "1".into()));
    }
    env
}

/// Clear what interrupted updates left behind (download folders of processes that are gone) and
/// report the version this launch replaced, once, if it just updated. Only for a bundled Trek:
/// a dev build sharing the data folder must not take another copy's update or its message.
/// On Windows, also the folders an update left next to the install (`clean_old_installs`).
pub fn after_launch() -> Option<String> {
    if cfg!(windows)
        && let Some(root) = install_root()
    {
        clean_old_installs(&root);
    }
    after_launch_in(&crate::paths::updates_dir())
}

/// What `trek-update` writes when it couldn't install an update (Windows): one sentence.
const INSTALL_FAILED: &str = "install-failed";

/// Why the last update didn't install, once, if `trek-update` couldn't install it.
pub fn take_install_failure() -> Option<String> {
    take_install_failure_in(&crate::paths::updates_dir())
}

fn take_install_failure_in(updates: &Path) -> Option<String> {
    let note = updates.join(INSTALL_FAILED);
    let text = std::fs::read_to_string(&note).ok()?;
    let _ = std::fs::remove_file(&note);
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

fn after_launch_in(updates: &Path) -> Option<String> {
    for entry in std::fs::read_dir(updates).into_iter().flatten().flatten() {
        if entry.file_name().to_str().and_then(download_owner).is_some_and(|pid| !process_alive(pid)) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let marker = updates.join(UPDATED_MARKER);
    let from = std::fs::read_to_string(&marker).ok()?;
    let _ = std::fs::remove_file(&marker);
    let from = from.trim().to_string();
    (from != crate::VERSION).then_some(from)
}

#[cfg(target_os = "macos")]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
    // SAFETY: signal 0 only asks whether the process exists. EPERM: it does, as another user's.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    alive || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: plain calls; the handle is checked before use and closed after. Access denied: it
    // exists, as another user's.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let read = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        !read || code == STILL_ACTIVE as u32
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn process_alive(_: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_KEY: &str = include_str!("../tests/fixtures/update/test.pub");
    const FIXTURE_SIG: &str = include_str!("../tests/fixtures/update/archive.bin.minisig");

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/update").join(name)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek-update-test-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest(v: &str) -> Manifest {
        Manifest {
            version: v.into(),
            notes: " - fixed things\n".into(),
            pub_date: String::new(),
            platforms: HashMap::from([(platform_key(), Artifact { url: "u".into(), sha256: "s".into(), signature: String::new() })]),
        }
    }

    #[test]
    fn only_newer_versions_for_this_platform_count() {
        let current = semver::Version::parse("0.2.0").unwrap();
        assert!(newer_than(manifest("0.1.9"), &current).unwrap().is_none());
        assert!(newer_than(manifest("0.2.0"), &current).unwrap().is_none(), "same version");
        assert!(newer_than(manifest("0.2.1-beta.1"), &current).unwrap().is_some());
        assert!(newer_than(manifest("0.2.0-beta.3"), &current).unwrap().is_none(), "a prerelease of this version is older");
        let found = newer_than(manifest("v0.3.0"), &current).unwrap().unwrap();
        assert_eq!((found.version.to_string(), found.notes.as_str()), ("0.3.0".into(), "- fixed things"));
        let mut other = manifest("9.0.0");
        other.platforms.clear();
        assert!(newer_than(other, &current).unwrap().is_none());
        assert!(newer_than(manifest("soon"), &current).is_err());
    }

    #[test]
    fn channel_urls_follow_github_release_paths() {
        let mut u = Updates::default();
        assert_eq!(manifest_urls(&u), ["https://github.com/dokyit/Trek/releases/latest/download/stable.json"]);
        u.channel = Channel::Beta;
        assert_eq!(
            manifest_urls(&u),
            ["https://github.com/dokyit/Trek/releases/download/beta/beta.json", "https://github.com/dokyit/Trek/releases/latest/download/stable.json"]
        );
        u.channel = Channel::Nightly;
        assert_eq!(manifest_urls(&u).len(), 3);
        assert_eq!(manifest_urls(&u)[0], "https://github.com/dokyit/Trek/releases/download/nightly/nightly.json");
        u.feed_url = "http://127.0.0.1:8765/{channel}.json".into();
        assert_eq!(manifest_urls(&u)[1], "http://127.0.0.1:8765/beta.json");
        u.feed_url = "https://example.com/trek.json".into();
        assert_eq!(manifest_urls(&u), ["https://example.com/trek.json"]);
        u.feed_url = "  ".into();
        u.channel = Channel::Stable;
        assert_eq!(manifest_urls(&u), ["https://github.com/dokyit/Trek/releases/latest/download/stable.json"]);
    }

    #[test]
    fn embedded_release_key_is_valid() {
        assert!(minisign_verify::PublicKey::from_base64(public_key()).is_ok());
        assert_eq!(key_line(FIXTURE_KEY), FIXTURE_KEY.lines().nth(1).unwrap());
    }

    #[test]
    fn signatures_reject_tampering_and_other_keys() {
        let data = fixture("archive.bin");
        verify_file(&data, FIXTURE_SIG, FIXTURE_KEY).unwrap();
        let dir = scratch("sig");
        let tampered = dir.join("archive.bin");
        let mut bytes = std::fs::read(&data).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&tampered, bytes).unwrap();
        assert!(verify_file(&tampered, FIXTURE_SIG, FIXTURE_KEY).is_err());
        assert!(verify_file(&data, FIXTURE_SIG, public_key()).is_err(), "release key must not accept the test key's signature");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn signature_must_name_the_offered_version() {
        let v = |s: &str| semver::Version::parse(s).unwrap();
        assert!(signature_for("", &v("1.0.0")).is_err());
        assert!(signature_for("garbage", &v("1.0.0")).is_err());
        // The fixture's trusted comment is "Trek test fixture": no version in it.
        assert!(signature_for(FIXTURE_SIG, &v("1.0.0")).err().unwrap().to_string().contains("another version"));
    }

    fn write_app(app: &Path, id: &str, version: &str) {
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{id}</string><key>CFBundleVersion</key><string>{version}</string></dict></plist>"#
        );
        std::fs::write(app.join("Contents/Info.plist"), plist).unwrap();
        std::fs::write(app.join("Contents/v"), version).unwrap();
    }

    fn contents(app: &Path) -> String {
        std::fs::read_to_string(app.join("Contents/v")).unwrap()
    }

    #[test]
    fn the_newest_feed_wins_and_failing_feeds_are_skipped() {
        let current = semver::Version::parse("0.2.0").unwrap();
        let picked = newest(vec![Ok(manifest("0.3.0-beta.1")), Err(anyhow!("offline")), Ok(manifest("0.2.1"))], &current, None).unwrap();
        assert_eq!(picked.unwrap().version.to_string(), "0.3.0-beta.1");
        // A version rolled back from isn't offered again; the next best is.
        let skipped = semver::Version::parse("0.3.0-beta.1").unwrap();
        let picked = newest(vec![Ok(manifest("0.3.0-beta.1")), Ok(manifest("0.2.1"))], &current, Some(&skipped)).unwrap();
        assert_eq!(picked.unwrap().version.to_string(), "0.2.1");
        assert!(newest(vec![Ok(manifest("0.1.0")), Err(anyhow!("404"))], &current, None).unwrap().is_none(), "one feed answered: up to date");
        let all_failed = newest(vec![Err(anyhow!("no release yet")), Err(anyhow!("offline"))], &current, None);
        assert_eq!(all_failed.err().unwrap().to_string(), "no release yet");
    }

    #[test]
    fn network_failures_are_transient_and_bad_releases_are_not() {
        let net = anyhow::Error::new(NetError("couldn't connect to github.com".into())).context("download");
        assert!(is_transient(&net));
        assert!(!is_transient(&anyhow!("the update's signature doesn't match; it was not installed")));
    }

    #[test]
    fn swap_exchanges_bundles() {
        let dir = scratch("swap");
        let (new, old) = (dir.join("incoming.app"), dir.join("Trek.app"));
        write_app(&new, "dev.trek.Trek", "new");
        write_app(&old, "dev.trek.Trek", "old");
        let replaced = swap(&new, &old).unwrap();
        assert_eq!((contents(&old), contents(&replaced)), ("new".into(), "old".into()));
        // Nothing to swap with: an error, and the bundle is untouched.
        assert!(swap(&dir.join("missing.app"), &old).is_err());
        assert_eq!(contents(&old), "new");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn swap_by_rename_puts_the_app_back_when_it_fails() {
        let dir = scratch("rename");
        let work = dir.join(".Trek.app.update");
        let (new, old) = (work.join("Trek.app"), dir.join("Trek.app"));
        write_app(&new, "dev.trek.Trek", "new");
        write_app(&old, "dev.trek.Trek", "old");
        let replaced = swap_by_rename(&new, &old).unwrap();
        assert_eq!((contents(&old), contents(&replaced)), ("new".into(), "old".into()));
        assert!(replaced.starts_with(&work));
        assert!(swap_by_rename(&work.join("missing.app"), &old).is_err());
        assert_eq!(contents(&old), "new", "restored after a failed second rename");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bundle_check_refuses_other_apps_versions_and_downgrades() {
        let dir = scratch("bundle");
        let app = dir.join("Trek.app");
        let v = |s: &str| semver::Version::parse(s).unwrap();
        write_app(&app, "dev.trek.Trek", "0.2.1");
        check_bundle(&app, "dev.trek.Trek", &v("0.2.1"), &v("0.2.0")).unwrap();
        assert!(check_bundle(&app, "dev.trek.Trek", &v("0.2.2"), &v("0.2.0")).is_err(), "not the advertised version");
        assert!(check_bundle(&app, "dev.trek.Trek", &v("0.2.1"), &v("0.2.1")).is_err(), "not newer");
        assert!(check_bundle(&app, "com.example.Other", &v("0.2.1"), &v("0.2.0")).is_err(), "another app");
        write_app(&app, "dev.trek.Trek", "0.3.0-beta.2");
        check_bundle(&app, "dev.trek.Trek", &v("0.3.0-beta.2"), &v("0.2.1")).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn install_swaps_keeps_one_backup_and_never_downgrades() {
        let dir = scratch("install");
        let (apps, updates) = (dir.join("Applications"), dir.join("updates"));
        let bundle = apps.join("Trek.app");
        write_app(&bundle, "dev.trek.Trek", "0.2.0");
        let stage_new = |version: &str| {
            let app = new_download_dir(&updates).unwrap().join("Trek.app");
            write_app(&app, "dev.trek.Trek", version);
            app
        };

        let staged = stage_new("0.2.1");
        assert_eq!(install_into(&staged, &bundle, &updates).unwrap(), Some(updates.join("previous.noindex/Trek.app")));
        assert_eq!(contents(&bundle), "0.2.1");
        assert_eq!(contents(&updates.join("previous.noindex/Trek.app")), "0.2.0");
        assert!(!download_dir_of(&staged).unwrap().exists(), "the download folder is gone");
        assert!(!apps.join(".Trek.app.update").exists());
        assert_eq!(std::fs::read_to_string(updates.join("updated-from")).unwrap(), crate::VERSION);

        // The next update replaces the backup: there's only ever one.
        install_into(&stage_new("0.2.2"), &bundle, &updates).unwrap();
        assert_eq!((contents(&bundle), contents(&updates.join("previous.noindex/Trek.app"))), ("0.2.2".into(), "0.2.1".into()));

        // Something newer is already installed (another copy updated it): left alone, staged copy dropped.
        std::fs::remove_file(updates.join("updated-from")).unwrap();
        let older = stage_new("0.2.1");
        assert_eq!(install_into(&older, &bundle, &updates).unwrap(), None, "no backup to roll back to");
        assert_eq!(contents(&bundle), "0.2.2");
        assert!(!older.exists());
        assert!(!updates.join("updated-from").exists(), "nothing was installed");

        let gone = updates.join("download-1-0.noindex/Trek.app");
        assert!(install_into(&gone, &bundle, &updates).err().unwrap().to_string().contains("gone"));
        assert_eq!(std::fs::read_dir(&apps).unwrap().count(), 1, "only Trek.app next to the bundle");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn after_launch_clears_only_downloads_of_exited_processes() {
        let dir = scratch("after-launch");
        let mut exited = std::process::Command::new(trek_test_fixtures::bin("fixture")).arg("exit").spawn().unwrap();
        exited.wait().unwrap();
        let mine = new_download_dir(&dir).unwrap();
        let theirs = dir.join(format!("download-{}-0.noindex", exited.id()));
        std::fs::create_dir_all(theirs.join("Trek.app")).unwrap();
        std::fs::create_dir_all(dir.join("previous.noindex/Trek.app")).unwrap();
        std::fs::write(dir.join("updated-from"), "0.0.1").unwrap();

        assert_eq!(after_launch_in(&dir).as_deref(), Some("0.0.1"));
        assert!(mine.exists(), "a running process's download stays");
        assert!(!theirs.exists());
        assert!(dir.join("previous.noindex/Trek.app").exists(), "the backup stays");
        assert_eq!(after_launch_in(&dir), None, "reported once");
        std::fs::write(dir.join("updated-from"), crate::VERSION).unwrap();
        assert_eq!(after_launch_in(&dir), None, "not an update");
        assert_eq!(download_owner("download-123-4.noindex"), Some(123));
        assert_eq!(download_owner("previous.noindex"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn relaunch_helper_restores_the_backup_when_the_new_app_wont_open() {
        let dir = scratch("relaunch");
        let (bundle, backup) = (dir.join("Trek.app"), dir.join("previous.noindex/Trek.app"));
        write_app(&bundle, "dev.trek.Trek", "new");
        write_app(&backup, "dev.trek.Trek", "old");
        let mut exited = std::process::Command::new(trek_test_fixtures::bin("fixture")).arg("exit").spawn().unwrap();
        exited.wait().unwrap();
        let skip = dir.join(SKIP_MARKER);
        // The "opener" is `fixture exit <code>`: the app opens or it doesn't.
        let fixture = trek_test_fixtures::bin("fixture");
        let fixture = fixture.to_str().unwrap();
        let helper = |backup: Option<&Path>, code: &str| {
            relaunch_command(exited.id(), &bundle, backup, &skip, "0.2.1", fixture, &["exit".into(), code.into()]).status().unwrap()
        };

        assert!(helper(Some(&backup), "0").success());
        assert_eq!(contents(&bundle), "new", "opened: nothing moves");
        assert!(backup.exists());
        assert!(!skip.exists());

        // Nothing was replaced (the app on disk was already newer): a backup from an earlier
        // update must not go back.
        helper(None, "1");
        assert_eq!(contents(&bundle), "new");
        assert!(backup.exists());

        helper(Some(&backup), "1");
        assert_eq!(contents(&bundle), "old", "the backup is back in place");
        assert!(!backup.exists());
        assert!(!dir.join("Trek.app.failed").exists());
        // The version it went back to won't install the one that failed again.
        assert_eq!(skipped_version(&dir).map(|v| v.to_string()).as_deref(), Some("0.2.1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn only_releases_somewhere_writable_update_themselves() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = scratch("blocker");
        let app = dir.join("apps/Trek.app");
        write_app(&app, "dev.trek.Trek", "0.2.0");
        // script/bundle.sh on its own: a local build, whatever its version says.
        assert!(!is_release(&app));
        assert_eq!(blocker_for(&app, false), Some(Blocker::DevBuild));
        let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), plist.replace("</dict>", "<key>TrekRelease</key><true/></dict>")).unwrap();
        assert!(is_release(&app));
        assert_eq!(blocker_for(&app, true), None);
        assert_eq!(blocker_for(Path::new("/private/var/folders/x/AppTranslocation/y/d/Trek.app"), true), Some(Blocker::Translocated));
        // A folder this user can't write to: nothing would ever install.
        std::fs::set_permissions(dir.join("apps"), std::fs::Permissions::from_mode(0o555)).unwrap();
        assert_eq!(blocker_for(&app, true), Some(Blocker::ReadOnlyLocation));
        std::fs::set_permissions(dir.join("apps"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn writable_means_files_can_be_created_there() {
        let dir = scratch("writable");
        assert!(writable(&dir));
        let file = dir.join("f");
        std::fs::write(&file, "x").unwrap();
        assert!(writable(&file));
        assert!(!writable(&dir.join("missing")));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "x", "probing leaves files alone");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "and leaves no probe behind");
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---------- Windows ----------

    #[cfg(windows)]
    #[test]
    fn windows_builds_ask_for_the_windows_artifact() {
        assert_eq!(platform_key(), format!("windows-{}", std::env::consts::ARCH));
        if cfg!(target_arch = "x86_64") {
            assert_eq!(platform_key(), "windows-x86_64");
        }
        // The test binary sits in cargo's output: a development build, which never updates.
        assert_eq!(blocker(), Some(Blocker::DevBuild));
        assert_eq!(archive_name(&semver::Version::new(0, 4, 1)), "Trek-0.4.1.zip");
    }

    /// A Windows install: Trek's three files in a folder of their own.
    fn trek_folder(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        for f in TREK_FILES {
            std::fs::write(dir.join(f), *f).unwrap();
        }
    }

    #[test]
    fn a_windows_install_updates_only_as_a_release_in_a_folder_of_its_own_the_user_owns() {
        let dir = scratch("win-blocker");
        let (root, program_files) = (dir.join("Programs").join("Trek"), dir.join("Program Files"));
        trek_folder(&root);
        let protected = [program_files.clone()];
        assert_eq!(windows_blocker_for(&root, true, &protected), None);
        assert_eq!(windows_blocker_for(&root, false, &protected), Some(Blocker::DevBuild));

        // Program Files, however it's spelled, and Windows's app store folder.
        let installed = program_files.join("Trek");
        trek_folder(&installed);
        assert_eq!(windows_blocker_for(&installed, true, &protected), Some(Blocker::ProtectedFolder));
        let shouting = PathBuf::from(installed.to_string_lossy().to_uppercase());
        assert_eq!(windows_blocker_for(&shouting, true, &protected), Some(Blocker::ProtectedFolder));
        let store = dir.join("WindowsApps").join("Trek_1.0_x64");
        trek_folder(&store);
        assert_eq!(windows_blocker_for(&store, true, &[]), Some(Blocker::ProtectedFolder));

        // Unzipped straight into Downloads: the folder isn't Trek's to replace.
        std::fs::write(root.join("holiday.jpg"), "x").unwrap();
        assert_eq!(windows_blocker_for(&root, true, &protected), Some(Blocker::SharedFolder));
        std::fs::remove_file(root.join("holiday.jpg")).unwrap();
        std::fs::create_dir(root.join("projects")).unwrap();
        assert_eq!(windows_blocker_for(&root, true, &protected), Some(Blocker::SharedFolder));
        std::fs::remove_dir(root.join("projects")).unwrap();
        std::fs::remove_file(root.join("trek-mcp.exe")).unwrap();
        assert_eq!(windows_blocker_for(&root, true, &protected), None, "an older release's folder, without a file, is still Trek's own");
        std::fs::remove_file(root.join("trek.exe")).unwrap();
        assert_eq!(windows_blocker_for(&root, true, &protected), Some(Blocker::SharedFolder), "no trek.exe: not an install");

        assert!(is_cargo_output(&{
            let target = dir.join("target").join("debug");
            std::fs::create_dir_all(target.join(".fingerprint")).unwrap();
            std::fs::create_dir_all(target.join("deps")).unwrap();
            target.join("deps")
        }));
        assert!(!is_cargo_output(&root));
        for b in [Blocker::ProtectedFolder, Blocker::SharedFolder] {
            assert!(b.message().ends_with("to get updates."), "{b:?} says what to do");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_compare_as_windows_compares_them() {
        let pf = Path::new(r"C:\Program Files");
        assert!(inside(Path::new(r"c:\program files\Trek"), pf));
        assert!(inside(Path::new(r"\\?\C:\Program Files\Trek"), pf));
        assert!(inside(pf, pf));
        assert!(!inside(Path::new(r"C:\Program Files (x86)\Trek"), pf), "a sibling isn't inside");
        assert!(!inside(Path::new(r"D:\Program Files\Trek"), pf));
        assert!(!inside(Path::new(r"C:\Users\me\Trek"), Path::new("")));
    }

    /// A zip with these entries: (name, contents); a name ending in `/` is a folder, `@target` a
    /// symlink to it.
    fn zip_of(path: &Path, entries: &[(&str, &str)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            if let Some(target) = body.strip_prefix('@') {
                z.add_symlink(*name, target, options).unwrap();
            } else if name.ends_with('/') {
                z.add_directory(*name, options).unwrap();
            } else {
                z.start_file(*name, options).unwrap();
                std::io::Write::write_all(&mut z, body.as_bytes()).unwrap();
            }
        }
        z.finish().unwrap();
    }

    const RELEASE: &[(&str, &str)] = &[("trek.exe", "0.4.1"), ("trek-mcp.exe", "mcp"), ("trek-update.exe", "helper")];

    #[test]
    fn a_windows_release_unpacks_only_as_trek_s_flat_folder() {
        let dir = scratch("zip");
        let (archive, into) = (dir.join("a.zip"), dir.join("stage"));
        let unpack = |entries: &[(&str, &str)]| {
            zip_of(&archive, entries);
            unpack_release_zip(&archive, &into, 1 << 20).map_err(|e| e.to_string())
        };
        unpack(RELEASE).unwrap();
        assert_eq!(std::fs::read_to_string(into.join("trek-update.exe")).unwrap(), "helper");
        assert_eq!(std::fs::read_dir(&into).unwrap().count(), 3);

        let refused = |entries: &[(&str, &str)], why: &str| {
            let e = unpack(entries).unwrap_err();
            assert!(e.contains(why), "{entries:?}: {e}");
        };
        refused(&[("trek.exe", "x"), ("trek-mcp.exe", "x")], "no trek-update.exe");
        refused(&[("Trek-0.4.1/trek.exe", "x"), ("Trek-0.4.1/trek-mcp.exe", "x"), ("Trek-0.4.1/trek-update.exe", "x")], "isn't Trek (Trek-0.4.1/trek.exe)");
        refused(&[("../trek.exe", "x")], "isn't Trek");
        refused(&[("C:/Windows/trek.exe", "x")], "isn't Trek");
        refused(&[("trek.exe/", "")], "isn't Trek (trek.exe/)");
        refused(&[("trek.exe", "@C:/Windows/System32/cmd.exe")], "isn't Trek (trek.exe)");
        refused(&[("trek.exe", "x"), ("trek-mcp.exe", "x"), ("trek-update.exe", "x"), ("readme.txt", "x")], "more in it than Trek");
        refused(&[("TREK.EXE", "x")], "isn't Trek");
        let big = "x".repeat(600 * 1024);
        refused(&[("trek.exe", &big), ("trek-mcp.exe", &big)], "larger than Trek could be");
        assert!(unpack(&[]).unwrap_err().contains("no trek.exe"));
        std::fs::write(&archive, "not a zip").unwrap();
        assert!(unpack_release_zip(&archive, &into, 1 << 20).unwrap_err().to_string().contains("isn't a zip"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn staging_a_windows_release_checks_its_version_and_cleans_up_after_itself() {
        let updates = scratch("stage");
        let v = |s: &str| semver::Version::parse(s).unwrap();
        // The fake trek.exe says its version as its contents.
        let version_of = |exe: &Path| -> Result<semver::Version> { Ok(v(&std::fs::read_to_string(exe)?)) };
        let nobody = AtomicBool::new(false);
        let download = |entries: &[(&str, &str)]| {
            let archive = new_download_dir(&updates).unwrap().join("Trek-0.4.1.zip");
            zip_of(&archive, entries);
            archive
        };

        let archive = download(RELEASE);
        let staged = stage_zip(&archive, &v("0.4.1"), &v("0.4.0"), &nobody, version_of).unwrap();
        assert_eq!(staged, archive.with_file_name("stage"));
        assert!(!archive.exists(), "the archive is gone once unpacked");
        assert_eq!(download_dir_of(&staged), archive.parent(), "discard finds its download folder");
        discard(&staged);
        assert!(!archive.parent().unwrap().exists());

        for (current, expected, why) in [("0.4.0", "0.4.2", "contains version 0.4.1, not 0.4.2"), ("0.4.1", "0.4.1", "isn't newer")] {
            let archive = download(RELEASE);
            let e = stage_zip(&archive, &v(expected), &v(current), &nobody, version_of).unwrap_err();
            assert!(e.to_string().contains(why), "{e}");
            assert!(!archive.parent().unwrap().exists(), "a refused update leaves nothing behind");
        }
        let archive = download(&[("trek.exe", "0.4.1"), ("trek-mcp.exe", "x"), ("trek-update.exe", "x"), ("evil.dll", "x")]);
        assert!(stage_zip(&archive, &v("0.4.1"), &v("0.4.0"), &nobody, version_of).is_err());
        assert!(!archive.parent().unwrap().exists());
        let cancelled = AtomicBool::new(true);
        let archive = download(RELEASE);
        assert!(stage_zip(&archive, &v("0.4.1"), &v("0.4.0"), &cancelled, version_of).is_err());
        assert_eq!(std::fs::read_dir(&updates).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(updates);
    }

    #[cfg(windows)]
    #[test]
    fn versions_are_read_from_an_executables_version_resource() {
        // Every Windows DLL carries one, in four numbers rather than semver.
        let kernel32 = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\kernel32.dll");
        let v = exe_product_version(&kernel32).unwrap();
        assert!(v.starts_with("10.0."), "{v}");
        assert!(exe_version(&kernel32).unwrap_err().to_string().contains("unreadable version"));
        assert!(exe_version(&fixture("archive.bin")).unwrap_err().to_string().contains("has no version"));
    }

    #[test]
    fn installing_on_windows_waits_for_the_helper_unless_the_install_is_newer_already() {
        let dir = scratch("prepare");
        let (root, updates) = (dir.join("Trek"), dir.join("updates"));
        let v = |s: &str| semver::Version::parse(s).unwrap();
        let version_of = |exe: &Path| -> Result<semver::Version> { Ok(v(&std::fs::read_to_string(exe)?)) };
        trek_folder(&root);
        std::fs::write(root.join("trek.exe"), "0.4.0").unwrap();
        let stage_new = |version: &str| {
            let staged = new_download_dir(&updates).unwrap().join("stage");
            trek_folder(&staged);
            std::fs::write(staged.join("trek.exe"), version).unwrap();
            staged
        };

        let staged = stage_new("0.4.1");
        let installed = prepare_swap(&staged, &root, &updates, version_of).unwrap();
        assert_eq!(installed, Installed { bundle: root.clone(), backup: None, staged: Some(staged.clone()) });
        assert_eq!(std::fs::read_to_string(&root.join("trek.exe")).unwrap(), "0.4.0", "nothing moves while Trek runs");
        assert_eq!(std::fs::read_to_string(updates.join(UPDATED_MARKER)).unwrap(), crate::VERSION);

        // Another copy of Trek got there first: the download goes, nothing is swapped.
        std::fs::remove_file(updates.join(UPDATED_MARKER)).unwrap();
        std::fs::write(root.join("trek.exe"), "0.4.2").unwrap();
        let older = stage_new("0.4.1");
        assert_eq!(prepare_swap(&older, &root, &updates, version_of).unwrap().staged, None);
        assert!(!download_dir_of(&older).unwrap().exists());
        assert!(!updates.join(UPDATED_MARKER).exists());
        assert!(prepare_swap(&older, &root, &updates, version_of).unwrap_err().to_string().contains("gone"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_helper_is_told_what_to_swap_and_how_to_relaunch() {
        let (helper, root, staged, updates) = (Path::new("S/trek-update.exe"), Path::new("Apps/Trek"), Path::new("S"), Path::new("U"));
        let vars = || {
            [("TREK_DATA_DIR", "D"), ("TREK_MOCK_PROMPT", "plan"), ("TREK_BACKGROUND", "1"), ("RUST_LOG", "info"), ("PATH", "P")].into_iter().map(|(k, v)| (k.to_string(), v.to_string()))
        };
        let args = |cmd: &std::process::Command| cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
        let env = |cmd: &std::process::Command| cmd.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect::<Vec<_>>();

        let cmd = helper_command(helper, 42, root, Some(staged), "0.4.1", updates, Some(false), vars());
        assert_eq!(cmd.get_program(), helper.as_os_str());
        assert_eq!(args(&cmd), ["--pid", "42", "--install", "Apps/Trek", "--from", crate::VERSION, "--updates", "U", "--staged", "S", "--to", "0.4.1", "--relaunch"]);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("Apps")), "never inside the install it renames");
        let env = env(&cmd);
        assert!(env.contains(&("TREK_MOCK_PROMPT".into(), None)), "one-time flags don't fire again: {env:?}");
        assert!(env.contains(&("TREK_BACKGROUND".into(), Some("1".into()))), "behind other windows, as this one was: {env:?}");
        assert!(!env.iter().any(|(k, _)| k == "TREK_DATA_DIR" || k == "PATH"), "setup is inherited as it is: {env:?}");

        // In front: no TREK_BACKGROUND, even though this launch had it.
        let cmd = helper_command(helper, 42, root, Some(staged), "0.4.1", updates, Some(true), vars());
        assert!(env_of(&cmd, "TREK_BACKGROUND").is_some_and(|v| v.is_none()));
        // Quitting: swapped, not started.
        let cmd = helper_command(helper, 42, root, Some(staged), "", updates, None, vars());
        assert_eq!(args(&cmd).last().map(String::as_str), Some("S"));
    }

    fn env_of(cmd: &std::process::Command, key: &str) -> Option<Option<String>> {
        cmd.get_envs().find(|(k, _)| *k == key).map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    #[test]
    fn what_updates_left_next_to_the_install_is_removed_and_nothing_else() {
        let dir = scratch("old");
        let root = dir.join("Trek");
        trek_folder(&root);
        for leftover in ["Trek.old-0.4.0", "trek.OLD-0.4.0-2", "Trek.old-failed-0.4.1"] {
            trek_folder(&dir.join(leftover));
        }
        std::fs::create_dir(dir.join("Trek.old-empty")).unwrap();
        // Someone's own folders, whatever they're called, stay.
        let mine = dir.join("Trek.old-photos");
        std::fs::create_dir(&mine).unwrap();
        std::fs::write(mine.join("cat.jpg"), "x").unwrap();
        trek_folder(&dir.join("Trek.older"));
        trek_folder(&dir.join("Other.old-1"));
        std::fs::write(dir.join("Trek.old-notes.txt"), "x").unwrap();

        clean_old_installs(&root);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["Other.old-1", "Trek", "Trek.old-notes.txt", "Trek.old-photos", "Trek.older"]);
        assert!(mine.join("cat.jpg").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_install_is_reported_once() {
        let dir = scratch("failed");
        assert_eq!(take_install_failure_in(&dir), None);
        std::fs::write(dir.join(INSTALL_FAILED), "Trek 0.4.1 didn't install: it's locked.\n").unwrap();
        assert_eq!(take_install_failure_in(&dir).as_deref(), Some("Trek 0.4.1 didn't install: it's locked."));
        assert_eq!(take_install_failure_in(&dir), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn relaunch_keeps_setup_but_not_launch_flags() {
        let vars = [
            ("TREK_DATA_DIR", "/tmp/trek-data"),
            ("TREK_UPDATE_PUBKEY", "RWQ…"),
            ("TREK_MOCK_AGENT", "1"),
            ("RUST_LOG", "warn,trek=info"),
            ("TREK_MOCK_PROMPT", "mock:long 60s"),
            ("TREK_OPEN_THREAD_WINDOW", "0199"),
            ("TREK_OPEN_SETTINGS", "updates"),
            ("TREK_OPEN_TOOL", "browser"),
            ("TREK_ONBOARDING", "1"),
            ("TREK_FORCE_ACTIVE", "1"),
            ("TREK_BACKGROUND", "1"),
            ("HOME", "/Users/me"),
        ];
        let from = |foreground| relaunch_env_from(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())), foreground);
        let keys = |env: Vec<(String, String)>| env.into_iter().map(|(k, _)| k).collect::<Vec<_>>();
        assert_eq!(keys(from(true)), ["TREK_DATA_DIR", "TREK_MOCK_AGENT", "RUST_LOG"]);
        // In the background it opens behind other apps again, whatever this launch was.
        let env = from(false);
        assert!(env.contains(&("TREK_DATA_DIR".into(), "/tmp/trek-data".into())));
        assert_eq!(env.last(), Some(&("TREK_BACKGROUND".into(), "1".into())));
        assert_eq!(env.iter().filter(|(k, _)| k == "TREK_BACKGROUND").count(), 1);
    }
}
