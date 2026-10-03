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
    /// Not running from an app bundle (`cargo run`): rebuilding is the update.
    DevBuild,
    /// macOS runs a quarantined app from a read-only copy until it's moved out of Downloads.
    Translocated,
}

impl Blocker {
    pub fn message(self) -> &'static str {
        match self {
            Blocker::DevBuild => "This is a development build. It updates when you rebuild it.",
            Blocker::Translocated => "macOS is running Trek from a read-only copy. Move Trek to Applications to get updates.",
        }
    }
}

pub fn blocker() -> Option<Blocker> {
    match running_bundle() {
        None => Some(Blocker::DevBuild),
        Some(b) if b.to_string_lossy().contains("/AppTranslocation/") => Some(Blocker::Translocated),
        Some(_) => None,
    }
}

/// The `.app` bundle we're running from, if any.
pub fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
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

fn http() -> Result<reqwest::Client> {
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
    newest(results, &current_version())
}

/// The newest build above `current` across the feeds' manifests; the first error only if every
/// feed failed.
fn newest(results: Vec<Result<Manifest>>, current: &semver::Version) -> Result<Option<AvailableUpdate>> {
    let mut best: Option<AvailableUpdate> = None;
    let mut first_error = None;
    let mut any_ok = false;
    for result in results {
        match result.and_then(|m| newer_than(m, current)) {
            Ok(found) => {
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
    let dest = dir.join(format!("Trek-{}.tar.gz", update.version));
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

/// What `install` left: the app to open, and the version it replaced, if it replaced one and
/// kept it. The relaunch puts that backup back only if the new app won't open.
#[derive(Debug, PartialEq)]
pub struct Installed {
    pub bundle: PathBuf,
    pub backup: Option<PathBuf>,
}

/// Swap the staged bundle in for the running one and keep the old one as the single backup.
/// If anything fails before the swap, nothing has changed.
pub fn install(staged: &Path) -> Result<Installed> {
    let bundle = running_bundle().context("not running from an app bundle (dev build)")?;
    let backup = install_into(staged, &bundle, &crate::paths::updates_dir())?;
    Ok(Installed { bundle, backup })
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
/// back and opens it instead. Arguments: pid, bundle, backup (empty: none), opener, then the
/// opener's options.
const RELAUNCH_SCRIPT: &str = r#"
    pid=$1 bundle=$2 backup=$3 opener=$4; shift 4
    while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
    "$opener" "$@" "$bundle" && exit 0
    if [ -n "$backup" ] && [ -d "$backup" ]; then
        /bin/mv "$bundle" "$bundle.failed" && /bin/mv "$backup" "$bundle" && /bin/rm -rf "$bundle.failed"
        "$opener" "$@" "$bundle"
    fi
"#;

fn relaunch_command(pid: u32, bundle: &Path, backup: Option<&Path>, opener: &str, open_args: &[String]) -> std::process::Command {
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c").arg(RELAUNCH_SCRIPT).arg("trek-relaunch").arg(pid.to_string()).arg(bundle);
    cmd.arg(backup.map(Path::as_os_str).unwrap_or_default()).arg(opener).args(open_args);
    cmd
}

/// Start the installed bundle once this process has exited. If it can't be opened, the version
/// it replaced goes back in its place and that is opened instead; a bundle this update didn't
/// replace is never rolled back. The caller quits right after.
pub fn relaunch(installed: &Installed, foreground: bool) -> Result<()> {
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
    use std::os::unix::process::CommandExt as _;
    relaunch_command(std::process::id(), &installed.bundle, installed.backup.as_deref(), "/usr/bin/open", &open_args)
        .process_group(0)
        .spawn()
        .context("couldn't schedule the relaunch")?;
    Ok(())
}

/// What the relaunched Trek runs with: where its data lives and how it's set up, never the
/// one-time launch flags (`TREK_MOCK_PROMPT`, `TREK_OPEN_*`, `TREK_ONBOARDING`, measurement aids),
/// which would fire again after every update.
const RELAUNCH_KEEPS: &[&str] = &["TREK_DATA_DIR", "TREK_UPDATE_PUBKEY", "TREK_UPDATE_AUTO_RESTART", "TREK_MOCK_AGENT", "TREK_AXE_PATH", "RUST_LOG"];

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
pub fn after_launch() -> Option<String> {
    after_launch_in(&crate::paths::updates_dir())
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

#[cfg(not(target_os = "macos"))]
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
        let picked = newest(vec![Ok(manifest("0.3.0-beta.1")), Err(anyhow!("offline")), Ok(manifest("0.2.1"))], &current).unwrap();
        assert_eq!(picked.unwrap().version.to_string(), "0.3.0-beta.1");
        assert!(newest(vec![Ok(manifest("0.1.0")), Err(anyhow!("404"))], &current).unwrap().is_none(), "one feed answered: up to date");
        let all_failed = newest(vec![Err(anyhow!("no release yet")), Err(anyhow!("offline"))], &current);
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

    #[test]
    fn after_launch_clears_only_downloads_of_exited_processes() {
        let dir = scratch("after-launch");
        let mut exited = std::process::Command::new("/usr/bin/true").spawn().unwrap();
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

    #[test]
    fn relaunch_helper_restores_the_backup_when_the_new_app_wont_open() {
        let dir = scratch("relaunch");
        let (bundle, backup) = (dir.join("Trek.app"), dir.join("previous.noindex/Trek.app"));
        write_app(&bundle, "dev.trek.Trek", "new");
        write_app(&backup, "dev.trek.Trek", "old");
        let mut exited = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        exited.wait().unwrap();
        let helper = |backup: Option<&Path>, opener: &str| relaunch_command(exited.id(), &bundle, backup, opener, &["-n".into()]).status().unwrap();

        assert!(helper(Some(&backup), "/usr/bin/true").success());
        assert_eq!(contents(&bundle), "new", "opened: nothing moves");
        assert!(backup.exists());

        // Nothing was replaced (the app on disk was already newer): a backup from an earlier
        // update must not go back.
        helper(None, "/usr/bin/false");
        assert_eq!(contents(&bundle), "new");
        assert!(backup.exists());

        helper(Some(&backup), "/usr/bin/false");
        assert_eq!(contents(&bundle), "old", "the backup is back in place");
        assert!(!backup.exists());
        assert!(!dir.join("Trek.app.failed").exists());
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
        assert_eq!(keys(from(true)), ["TREK_DATA_DIR", "TREK_UPDATE_PUBKEY", "TREK_MOCK_AGENT", "RUST_LOG"]);
        // In the background it opens behind other apps again, whatever this launch was.
        let env = from(false);
        assert!(env.contains(&("TREK_DATA_DIR".into(), "/tmp/trek-data".into())));
        assert_eq!(env.last(), Some(&("TREK_BACKGROUND".into(), "1".into())));
        assert_eq!(env.iter().filter(|(k, _)| k == "TREK_BACKGROUND").count(), 1);
    }
}
