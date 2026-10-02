//! Built-in updater: check a JSON manifest, download in the background, verify (SHA-256 +
//! minisign), then swap the app bundle in place and relaunch — only when agents are idle.
//!
//! Manifest (`{channel}.json` on GitHub Releases):
//! ```json
//! { "version": "0.2.0", "notes": "…", "pub_date": "2026-10-01T00:00:00Z",
//!   "platforms": { "darwin-aarch64": { "url": "…/Trek-0.2.0-arm64.app.tar.gz",
//!                                      "sha256": "…", "signature": "<minisign signature>" } } }
//! ```

use anyhow::{Context as _, Result, bail};
use futures::StreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Minisign public key for release artifacts. Builds without one (local builds) never check for
/// or install updates: an unsigned update from the network must not replace the app.
pub const PUBLIC_KEY: &str = match option_env!("TREK_UPDATE_PUBKEY") {
    Some(k) => k,
    None => "",
};

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: String,
    pub platforms: HashMap<String, Artifact>,
}

#[derive(Debug, Clone, Deserialize)]
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
    pub artifact: Artifact,
}

pub fn platform_key() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// Whether this build can update itself (it was compiled with the release signing key).
pub fn can_update() -> bool {
    !PUBLIC_KEY.is_empty()
}

pub fn current_version() -> semver::Version {
    semver::Version::parse(crate::VERSION).expect("valid crate version")
}

pub fn feed_url(settings: &crate::settings::Updates) -> String {
    let channel = match settings.channel {
        crate::settings::Channel::Stable => "stable",
        crate::settings::Channel::Beta => "beta",
        crate::settings::Channel::Nightly => "nightly",
    };
    settings.feed_url.replace("{channel}", channel)
}

/// Returns `Some` when the feed has a newer build for this platform.
pub async fn check(feed: &str) -> Result<Option<AvailableUpdate>> {
    let client = reqwest::Client::builder()
        .user_agent(format!("Trek/{}", crate::VERSION))
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let manifest: Manifest = client.get(feed).send().await?.error_for_status()?.json().await?;
    newer_than_current(manifest)
}

fn newer_than_current(manifest: Manifest) -> Result<Option<AvailableUpdate>> {
    let version = semver::Version::parse(manifest.version.trim_start_matches('v'))?;
    if version <= current_version() {
        return Ok(None);
    }
    let Some(artifact) = manifest.platforms.get(&platform_key()).cloned() else {
        return Ok(None);
    };
    Ok(Some(AvailableUpdate { version, notes: manifest.notes, artifact }))
}

/// Download to the updates folder, reporting progress in 0.0..=1.0. Verifies before returning.
pub async fn download(update: &AvailableUpdate, mut progress: impl FnMut(f32)) -> Result<PathBuf> {
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;

    let dest = crate::paths::updates_dir().join(format!("Trek-{}.tar.gz", update.version));
    let resp = reqwest::get(&update.artifact.url).await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(&dest).await?;
    let mut hasher = sha2::Sha256::new();
    let mut got = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        got += chunk.len() as u64;
        if total > 0 {
            progress(got as f32 / total as f32);
        }
    }
    file.flush().await?;
    let digest = hex::encode(hasher.finalize());
    if !digest.eq_ignore_ascii_case(update.artifact.sha256.trim()) {
        let _ = tokio::fs::remove_file(&dest).await;
        bail!("checksum mismatch");
    }
    verify_signature(&dest, &update.artifact.signature)?;
    progress(1.0);
    Ok(dest)
}

fn verify_signature(path: &Path, signature: &str) -> Result<()> {
    if PUBLIC_KEY.is_empty() {
        bail!("this build has no update signing key; refusing an unsigned update");
    }
    let pk = minisign_verify::PublicKey::from_base64(PUBLIC_KEY).context("bad public key")?;
    let sig = minisign_verify::Signature::decode(signature).context("bad signature")?;
    let data = std::fs::read(path)?;
    pk.verify(&data, &sig, false).context("signature verification failed")?;
    Ok(())
}

/// The `.app` bundle we're running from, if any.
pub fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// Replace the running bundle with the downloaded one and relaunch. The caller quits right after.
pub fn install_and_relaunch(archive: &Path) -> Result<()> {
    let bundle = running_bundle().context("not running from an app bundle (dev build)")?;
    let staging = crate::paths::updates_dir().join("staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let status = std::process::Command::new("/usr/bin/tar").arg("-xzf").arg(archive).arg("-C").arg(&staging).status()?;
    if !status.success() {
        bail!("could not unpack update");
    }
    let new_app = std::fs::read_dir(&staging)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .context("update archive has no .app")?;
    let backup = bundle.with_extension("app.old");
    let _ = std::fs::remove_dir_all(&backup);
    std::fs::rename(&bundle, &backup).context("could not move current app aside")?;
    if let Err(e) = std::fs::rename(&new_app, &bundle) {
        std::fs::rename(&backup, &bundle)?; // roll back
        return Err(e).context("could not install update");
    }
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_file(archive);
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("sleep 1; /usr/bin/open -n '{}'", bundle.display()))
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_newer_versions_for_this_platform_count() {
        let mk = |v: &str| Manifest {
            version: v.into(),
            notes: String::new(),
            pub_date: String::new(),
            platforms: HashMap::from([(
                platform_key(),
                Artifact { url: "u".into(), sha256: "s".into(), signature: String::new() },
            )]),
        };
        assert!(newer_than_current(mk("0.0.1")).unwrap().is_none());
        assert!(newer_than_current(mk("v99.0.0")).unwrap().is_some());
        let mut other = mk("99.0.0");
        other.platforms = HashMap::new();
        assert!(newer_than_current(other).unwrap().is_none());
    }
}
