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

async fn fetch_manifest(client: &reqwest::Client, url: &str) -> Result<Manifest> {
    let resp = client.get(url).timeout(std::time::Duration::from_secs(20)).send().await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("no release has been published on this channel yet");
    }
    let bytes = resp.error_for_status()?.bytes().await?;
    serde_json::from_slice(&bytes).with_context(|| format!("unreadable update manifest at {url}"))
}

/// Returns `Some` when any of the feeds has a build for this platform newer than this one (the
/// newest of them). Feeds that fail are skipped unless all of them do.
pub async fn check(urls: &[String]) -> Result<Option<AvailableUpdate>> {
    let client = http()?;
    let results = futures::future::join_all(urls.iter().map(|u| fetch_manifest(&client, u))).await;
    let current = current_version();
    let mut best: Option<AvailableUpdate> = None;
    let mut first_error = None;
    let mut any_ok = false;
    for result in results {
        match result.and_then(|m| newer_than(m, &current)) {
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

/// Download into the updates folder, reporting progress in 0.0..=1.0. Returns the archive only
/// once its SHA-256 and minisign signature check out; a bad download is deleted.
pub async fn download(update: &AvailableUpdate, mut progress: impl FnMut(f32)) -> Result<PathBuf> {
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;

    // Refuse before downloading anything if the release isn't signed for this version.
    let key = minisign_verify::PublicKey::from_base64(public_key()).context("this build's update key is invalid")?;
    let signature = signature_for(&update.artifact.signature, &update.version)?;
    let mut verifier = key.verify_stream(&signature).context("the update is signed with another key")?;

    let dest = crate::paths::updates_dir().join(format!("Trek-{}.tar.gz", update.version));
    let partial = dest.with_extension("gz.part");
    let resp = http()?.get(&update.artifact.url).send().await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(&partial).await?;
    let mut hasher = sha2::Sha256::new();
    let mut got = 0u64;
    let mut stream = resp.bytes_stream();
    let result: Result<()> = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
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
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(e);
    }
    tokio::fs::rename(&partial, &dest).await?;
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

fn staged_dir() -> PathBuf {
    // `.noindex`: Spotlight skips it, so the waiting copy doesn't show up as a second Trek.
    crate::paths::updates_dir().join("staged.noindex")
}

fn previous_dir() -> PathBuf {
    crate::paths::updates_dir().join("previous.noindex")
}

/// Records the version an update replaced, so the new version can say it was just updated.
fn updated_marker() -> PathBuf {
    crate::paths::updates_dir().join("updated-from")
}

/// Unpack a verified archive and inspect the bundle inside: the same app, exactly `expected`,
/// newer than this one, intact code signature, no quarantine. Returns the staged `.app`; the
/// archive is removed. Blocking.
pub fn stage(archive: &Path, expected: &semver::Version) -> Result<PathBuf> {
    let bundle = running_bundle().context("not running from an app bundle")?;
    let dir = staged_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let result = (|| {
        run("/usr/bin/tar", [OsStr::new("-xzf"), archive.as_os_str(), OsStr::new("-C"), dir.as_os_str()]).context("couldn't unpack the update")?;
        let app = std::fs::read_dir(&dir)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .context("the update archive has no app in it")?;
        check_bundle(&app, &bundle_id(&bundle)?, expected, &current_version())?;
        // Downloaded by Trek itself, so normally unquarantined; an archive fetched by a browser
        // would carry the flag into every file.
        let _ = std::process::Command::new("/usr/bin/xattr").arg("-dr").arg("com.apple.quarantine").arg(&app).output();
        run("/usr/bin/codesign", [OsStr::new("--verify"), OsStr::new("--strict"), app.as_os_str()]).context("the update's code signature is broken")?;
        Ok(app)
    })();
    let _ = std::fs::remove_file(archive);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
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

/// Swap the staged bundle in for the running one and keep the old one as the single backup.
/// Returns the bundle path. If anything fails before the swap, nothing has changed.
pub fn install(staged: &Path) -> Result<PathBuf> {
    let bundle = running_bundle().context("not running from an app bundle (dev build)")?;
    if !staged.exists() {
        bail!("the downloaded update is gone; check again");
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
        swap(&incoming, &bundle)?;
        // `incoming` now holds the previous version.
        let backup = previous_dir().join(name);
        let _ = std::fs::remove_dir_all(&backup);
        if std::fs::create_dir_all(previous_dir()).is_err() || move_dir(&incoming, &backup).is_err() {
            tracing::warn!("couldn't keep a backup of the previous version");
        }
        Ok(bundle.clone())
    })();
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(staged_dir());
    if result.is_ok() {
        let _ = std::fs::write(updated_marker(), crate::VERSION);
    }
    result
}

/// Rename, or copy then delete across volumes (`ditto` keeps signatures and attributes).
fn move_dir(from: &Path, to: &Path) -> Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    run("/usr/bin/ditto", [from.as_os_str(), to.as_os_str()]).context("couldn't move the update into place")?;
    let _ = std::fs::remove_dir_all(from);
    Ok(())
}

/// Exchange two directories. One atomic `renamex_np(RENAME_SWAP)` on APFS; on volumes without
/// it, two renames with the first undone if the second fails.
fn swap(incoming: &Path, bundle: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let a = std::ffi::CString::new(incoming.as_os_str().as_bytes())?;
        let b = std::ffi::CString::new(bundle.as_os_str().as_bytes())?;
        // SAFETY: both are valid NUL-terminated paths that outlive the call.
        if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } == 0 {
            return Ok(());
        }
    }
    let aside = incoming.with_extension("swap");
    std::fs::rename(bundle, &aside).context("couldn't move the current app aside")?;
    if let Err(e) = std::fs::rename(incoming, bundle) {
        std::fs::rename(&aside, bundle).context("couldn't restore the current app")?;
        return Err(e).context("couldn't install the update");
    }
    std::fs::rename(&aside, incoming)?;
    Ok(())
}

/// Start the installed bundle once this process has exited. If it can't be opened, the backup
/// goes back in its place and that is opened instead. The caller quits right after.
pub fn relaunch(bundle: &Path, foreground: bool) -> Result<()> {
    let backup = bundle.file_name().map(|n| previous_dir().join(n)).unwrap_or_default();
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
    let script = r#"
        pid=$1 bundle=$2 backup=$3; shift 3
        while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
        /usr/bin/open "$@" "$bundle" && exit 0
        if [ -d "$backup" ]; then
            /bin/mv "$bundle" "$bundle.failed" && /bin/mv "$backup" "$bundle" && /bin/rm -rf "$bundle.failed"
            /usr/bin/open "$@" "$bundle"
        fi
    "#;
    use std::os::unix::process::CommandExt as _;
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("trek-relaunch")
        .arg(std::process::id().to_string())
        .arg(bundle)
        .arg(backup)
        .args(open_args)
        .process_group(0)
        .spawn()
        .context("couldn't schedule the relaunch")?;
    Ok(())
}

fn relaunch_env(foreground: bool) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> =
        std::env::vars().filter(|(k, _)| (k.starts_with("TREK_") && k != "TREK_BACKGROUND") || k == "RUST_LOG").collect();
    if !foreground {
        env.push(("TREK_BACKGROUND".into(), "1".into()));
    }
    env
}

/// Clear what an interrupted update left behind (a partial download, an unpacked copy) and
/// report the version this launch replaced, once, if it just updated.
pub fn after_launch() -> Option<String> {
    let dir = crate::paths::updates_dir();
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".part") || name.ends_with(".tar.gz") {
            let _ = std::fs::remove_file(&path);
        }
    }
    let _ = std::fs::remove_dir_all(staged_dir());
    let marker = updated_marker();
    let from = std::fs::read_to_string(&marker).ok()?;
    let _ = std::fs::remove_file(&marker);
    let from = from.trim().to_string();
    (from != crate::VERSION).then_some(from)
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

    #[test]
    fn swap_exchanges_bundles() {
        let dir = scratch("swap");
        let (new, old) = (dir.join("incoming.app"), dir.join("Trek.app"));
        std::fs::create_dir_all(new.join("Contents")).unwrap();
        std::fs::create_dir_all(old.join("Contents")).unwrap();
        std::fs::write(new.join("Contents/v"), "new").unwrap();
        std::fs::write(old.join("Contents/v"), "old").unwrap();
        swap(&new, &old).unwrap();
        assert_eq!(std::fs::read_to_string(old.join("Contents/v")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(new.join("Contents/v")).unwrap(), "old");
        // Nothing to swap with: an error, and the bundle is untouched.
        assert!(swap(&dir.join("missing.app"), &old).is_err());
        assert_eq!(std::fs::read_to_string(old.join("Contents/v")).unwrap(), "new");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bundle_check_refuses_other_apps_versions_and_downgrades() {
        let dir = scratch("bundle");
        let app = dir.join("Trek.app");
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        let write = |id: &str, version: &str| {
            let plist = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{id}</string><key>CFBundleVersion</key><string>{version}</string></dict></plist>"#
            );
            std::fs::write(app.join("Contents/Info.plist"), plist).unwrap();
        };
        let v = |s: &str| semver::Version::parse(s).unwrap();
        write("dev.trek.Trek", "0.2.1");
        check_bundle(&app, "dev.trek.Trek", &v("0.2.1"), &v("0.2.0")).unwrap();
        assert!(check_bundle(&app, "dev.trek.Trek", &v("0.2.2"), &v("0.2.0")).is_err(), "not the advertised version");
        assert!(check_bundle(&app, "dev.trek.Trek", &v("0.2.1"), &v("0.2.1")).is_err(), "not newer");
        assert!(check_bundle(&app, "com.example.Other", &v("0.2.1"), &v("0.2.0")).is_err(), "another app");
        write("dev.trek.Trek", "0.3.0-beta.2");
        check_bundle(&app, "dev.trek.Trek", &v("0.3.0-beta.2"), &v("0.2.1")).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn relaunch_keeps_trek_environment() {
        let env = relaunch_env(false);
        assert!(env.contains(&("TREK_BACKGROUND".into(), "1".into())));
        assert!(env.iter().all(|(k, _)| k.starts_with("TREK_") || k == "RUST_LOG"));
        assert!(!relaunch_env(true).iter().any(|(k, _)| k == "TREK_BACKGROUND"));
    }
}
