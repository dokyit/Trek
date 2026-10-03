//! What changed between releases: the notes of every published release on the update channel,
//! read from the GitHub Releases API of the feed's repository and kept on disk for offline use.
//! The updater's manifest only carries the newest release's notes; an update that skips versions
//! shows all of theirs, and after an update Trek shows what the new version brought.

use crate::settings::{Channel, Updates};
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One published release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub version: semver::Version,
    /// Markdown, as written for the release.
    pub notes: String,
    /// RFC 3339, empty if unknown.
    pub date: String,
    /// The release's page on GitHub.
    pub url: String,
    pub prerelease: bool,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

const CACHE: &str = "changelog.json";

/// `owner/repo` of a GitHub releases feed (`https://github.com/<owner>/<repo>/releases`); `None`
/// for other feeds, which have no release list to read.
pub fn github_repo(updates: &Updates) -> Option<String> {
    let feed = updates.feed_url.trim().trim_end_matches('/');
    let feed = if feed.is_empty() { crate::update::OFFICIAL_FEED } else { feed };
    let rest = feed.strip_prefix("https://github.com/")?.strip_suffix("/releases")?;
    let mut parts = rest.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && !owner.is_empty() && !repo.is_empty()).then(|| format!("{owner}/{repo}"))
}

/// Every published release, newest first (`for_channel` picks a channel's): fetched, or the copy
/// saved by the last fetch when GitHub can't be reached.
pub async fn fetch(updates: &Updates) -> Result<Vec<Release>> {
    let cache = crate::paths::updates_dir().join(CACHE);
    match fetch_live(updates).await {
        Ok(all) => {
            if let Ok(json) = serde_json::to_vec(&all) {
                let _ = std::fs::write(&cache, json);
            }
            Ok(all)
        }
        Err(e) => cached(&cache).ok_or(e),
    }
}

fn cached(path: &Path) -> Option<Vec<Release>> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

async fn fetch_live(updates: &Updates) -> Result<Vec<Release>> {
    let Some(repo) = github_repo(updates) else { bail!("this update feed has no release list") };
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=50");
    let resp = crate::update::http()?
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .with_context(|| format!("couldn't reach {url}"))?;
    let resp = resp.error_for_status().with_context(|| format!("couldn't read {url}"))?;
    let releases: Vec<ApiRelease> = resp.json().await.context("unreadable release list")?;
    Ok(parse(releases))
}

fn parse(releases: Vec<ApiRelease>) -> Vec<Release> {
    let mut out: Vec<Release> = releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            // Stable and beta tags are `v<version>`; the nightly prerelease lives under the fixed
            // tag `nightly`, its title naming the version ("Trek 0.3.0-nightly.20261003 (nightly)").
            let version = version_in(&r.tag_name).or_else(|| r.name.as_deref().and_then(version_in))?;
            Some(Release {
                version,
                notes: r.body.unwrap_or_default().replace("\r\n", "\n").trim().to_string(),
                date: r.published_at.unwrap_or_default(),
                url: r.html_url,
                prerelease: r.prerelease,
            })
        })
        .collect();
    out.sort_by(|a, b| b.version.cmp(&a.version));
    out.dedup_by(|a, b| a.version == b.version);
    out
}

/// The first word of `text` that reads as a version, `v` prefix allowed.
fn version_in(text: &str) -> Option<semver::Version> {
    text.split(|c: char| c.is_whitespace() || c == '(' || c == ')').find_map(|w| semver::Version::parse(w.trim_start_matches('v')).ok())
}

/// Whether `channel` offers `release`: stable gets releases, beta also betas, nightly everything.
pub fn on_channel(release: &Release, channel: Channel) -> bool {
    let pre = release.version.pre.as_str();
    match channel {
        Channel::Stable => pre.is_empty(),
        Channel::Beta => pre.is_empty() || pre.starts_with("beta"),
        Channel::Nightly => true,
    }
}

/// The channel's releases after `from` up to and including `to`, newest first: what an update
/// from `from` to `to` brings.
pub fn between<'a>(releases: &'a [Release], channel: Channel, from: &semver::Version, to: &semver::Version) -> Vec<&'a Release> {
    releases.iter().filter(|r| r.version > *from && r.version <= *to && on_channel(r, channel)).collect()
}

/// GitHub's comparison of two tagged versions, for "all the changes" beyond the notes.
pub fn compare_url(repo: &str, from: &semver::Version, to: &semver::Version) -> String {
    format!("https://github.com/{repo}/compare/v{from}...v{to}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(tag: &str, name: &str, pre: bool) -> ApiRelease {
        ApiRelease {
            tag_name: tag.into(),
            name: Some(name.into()),
            body: Some(format!("notes for {name}\r\n")),
            published_at: Some("2026-10-03T14:25:06Z".into()),
            html_url: format!("https://github.com/dokyit/Trek/releases/tag/{tag}"),
            prerelease: pre,
            draft: false,
        }
    }

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    #[test]
    fn releases_read_their_version_from_the_tag_or_the_nightly_title() {
        let mut draft = api("v9.0.0", "Trek 9.0.0", false);
        draft.draft = true;
        let all = parse(vec![
            api("v0.2.0", "Trek 0.2.0", false),
            api("nightly", "Trek 0.3.0-nightly.20261004 (nightly)", true),
            api("v0.2.1", "Trek 0.2.1", false),
            api("v0.3.0-beta.1", "Trek 0.3.0-beta.1 (beta)", true),
            api("misc", "Not a version", false),
            draft,
        ]);
        let versions: Vec<String> = all.iter().map(|r| r.version.to_string()).collect();
        assert_eq!(versions, ["0.3.0-nightly.20261004", "0.3.0-beta.1", "0.2.1", "0.2.0"]);
        assert_eq!(all[2].notes, "notes for Trek 0.2.1", "trimmed, with Unix line ends");
    }

    #[test]
    fn each_channel_sees_its_own_releases_and_the_steadier_ones() {
        let all = parse(vec![api("v0.2.0", "Trek 0.2.0", false), api("v0.3.0-beta.1", "b", true), api("nightly", "Trek 0.3.0-nightly.1", true)]);
        let names = |c| all.iter().filter(|r| on_channel(r, c)).map(|r| r.version.to_string()).collect::<Vec<_>>();
        assert_eq!(names(Channel::Stable), ["0.2.0"]);
        assert_eq!(names(Channel::Beta), ["0.3.0-beta.1", "0.2.0"]);
        assert_eq!(names(Channel::Nightly), ["0.3.0-nightly.1", "0.3.0-beta.1", "0.2.0"]);
    }

    #[test]
    fn an_update_brings_every_release_since_the_installed_one() {
        let all = parse(vec![api("v0.2.0", "a", false), api("v0.2.1", "b", false), api("v0.3.0-beta.1", "c", true), api("v0.3.0", "d", false), api("v0.3.1", "e", false)]);
        let brought = |c| between(&all, c, &v("0.2.0"), &v("0.3.0")).iter().map(|r| r.version.to_string()).collect::<Vec<_>>();
        assert_eq!(brought(Channel::Stable), ["0.3.0", "0.2.1"], "no betas on stable");
        assert_eq!(brought(Channel::Beta), ["0.3.0", "0.3.0-beta.1", "0.2.1"]);
        assert!(between(&all, Channel::Stable, &v("0.3.1"), &v("0.3.1")).is_empty());
    }

    #[test]
    fn only_github_release_feeds_have_a_release_list() {
        let mut u = Updates::default();
        assert_eq!(github_repo(&u).as_deref(), Some("dokyit/Trek"));
        u.feed_url = "https://github.com/someone/fork/releases/".into();
        assert_eq!(github_repo(&u).as_deref(), Some("someone/fork"));
        u.feed_url = "http://127.0.0.1:8765/{channel}.json".into();
        assert_eq!(github_repo(&u), None);
        assert_eq!(compare_url("dokyit/Trek", &v("0.2.0"), &v("0.2.1")), "https://github.com/dokyit/Trek/compare/v0.2.0...v0.2.1");
    }

    #[test]
    fn the_saved_list_survives_a_round_trip() {
        let all = parse(vec![api("v0.2.0", "Trek 0.2.0", false)]);
        let dir = std::env::temp_dir().join(format!("trek-changelog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CACHE);
        std::fs::write(&path, serde_json::to_vec(&all).unwrap()).unwrap();
        assert_eq!(cached(&path), Some(all));
        let _ = std::fs::remove_dir_all(dir);
    }
}
