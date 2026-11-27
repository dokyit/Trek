//! Updates for the extensions Trek installed from the Chrome Web Store (macOS), as Chrome keeps
//! them current: once a day Trek asks the store's update service about each one (the same request
//! Chrome makes: its id and version), downloads a newer version, and puts it in place of the old
//! one, keeping what the extension stored. An update that asks for more than the version the user
//! installed (a new permission or site) isn't installed by itself: the Extensions page says so,
//! and installing it again from the store asks.
//!
//! Without this an extension stayed at the version first installed: uBlock Origin Lite's filter
//! lists, which arrive only with its updates, grew stale while sites changed their ads.

use super::manifest::{self, Manifest};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often the store is asked.
pub const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// What the last check found (`updates.json` beside the installed list).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// When the store was last asked (seconds since 1970).
    pub checked: u64,
    /// Updates that ask for more than was installed, by id: the version, and what more it asks.
    pub waiting: Vec<Waiting>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Waiting {
    pub id: String,
    pub version: String,
    pub asks: Vec<String>,
}

fn file(dir: &Path) -> PathBuf {
    dir.join("updates.json")
}

pub fn load(dir: &Path) -> State {
    std::fs::read_to_string(file(dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(dir: &Path, state: &State) {
    if let Ok(text) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(file(dir), text);
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether it's time to ask the store again.
pub fn due(state: &State, now: u64) -> bool {
    now.saturating_sub(state.checked) >= EVERY.as_secs()
}

/// The store's update service, asked about extension `id` at `version` (Chrome's request).
pub fn check_url(id: &str, version: &str) -> String {
    format!(
        "https://clients2.google.com/service/update2/crx?response=updatecheck&acceptformat=crx3\
         &prodversion={}.0.0.0&x=id%3D{id}%26v%3D{version}%26uc",
        super::CHROME_MAJOR
    )
}

/// The newer version and where to get it, from the service's answer (`<updatecheck
/// codebase="…" version="…" status="ok"/>`); `None` for `noupdate` or anything else.
pub fn parse_answer(xml: &str) -> Option<(String, String)> {
    let at = xml.find("<updatecheck")?;
    let tag = &xml[at..at + xml[at..].find('>')?];
    let attr = |name: &str| -> Option<String> {
        let start = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        let end = start + tag[start..].find('"')?;
        Some(tag[start..end].replace("&amp;", "&"))
    };
    if attr("status").as_deref() != Some("ok") {
        return None;
    }
    let codebase = attr("codebase").filter(|c| c.starts_with("https://"))?;
    Some((attr("version")?, codebase))
}

/// Whether version `a` is newer than `b` (dotted numbers, as Chrome compares them).
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> { v.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// What `new` asks for that `old` didn't (permissions and sites, in words): an update that asks
/// for any of it waits for the user.
pub fn asks_more(old: &Manifest, new: &Manifest) -> Vec<String> {
    let more = Manifest {
        permissions: new
            .permissions
            .iter()
            .filter(|p| !old.permissions.contains(p))
            .cloned()
            .collect(),
        hosts: new
            .hosts
            .iter()
            .filter(|h| !old.hosts.contains(h) && !old.hosts.iter().any(|o| o == "<all_urls>"))
            .cloned()
            .collect(),
        ..Default::default()
    };
    if more.permissions.is_empty() && more.hosts.is_empty() {
        return vec![];
    }
    let words = manifest::describe(&more);
    if words.is_empty() {
        more.permissions.into_iter().chain(more.hosts).collect()
    } else {
        words
    }
}

/// An update found and unpacked, ready to go in place of the installed extension.
#[derive(Debug, Clone)]
pub struct Ready {
    pub id: String,
    pub version: String,
    pub folder: PathBuf,
}

/// Ask the store about each of `installed` (id, version, folder), and unpack each newer version
/// into `incoming`; those that ask for more come back as waiting.
pub async fn check(
    installed: Vec<(String, String, PathBuf)>,
    incoming: PathBuf,
) -> (Vec<Ready>, Vec<Waiting>) {
    let mut ready = vec![];
    let mut waiting = vec![];
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
    else {
        return (ready, waiting);
    };
    for (id, version, folder) in installed {
        let answer = async {
            client
                .get(check_url(&id, &version))
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .ok()?
                .text()
                .await
                .ok()
        }
        .await;
        let Some((new_version, codebase)) = answer.as_deref().and_then(parse_answer) else {
            continue;
        };
        if !newer(&new_version, &version) {
            continue;
        }
        let bytes = async {
            let r = client
                .get(&codebase)
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .ok()?;
            r.bytes().await.ok()
        }
        .await;
        let Some(bytes) = bytes.filter(|b| b.len() < 64 << 20) else {
            tracing::warn!("browser extensions: {id}: update {new_version} didn't download");
            continue;
        };
        let dest = incoming.join(format!("{id}-update"));
        if let Err(e) = super::unpack_crx(&bytes, &dest) {
            tracing::warn!("browser extensions: {id}: update {new_version}: {e}");
            continue;
        }
        let (Ok(old), Ok(new)) = (manifest::read(&folder), manifest::read(&dest)) else {
            let _ = std::fs::remove_dir_all(&dest);
            continue;
        };
        let more = asks_more(&old, &new);
        if more.is_empty() {
            ready.push(Ready {
                id,
                version: new_version,
                folder: dest,
            });
        } else {
            let _ = std::fs::remove_dir_all(&dest);
            waiting.push(Waiting {
                id,
                version: new_version,
                asks: more,
            });
        }
    }
    (ready, waiting)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_s_answer_is_read() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><gupdate xmlns="http://www.google.com/update2/response" protocol="2.0" server="prod"><daystart elapsed_days="7270" elapsed_seconds="1"/><app appid="ddkjiahejlhfcafbddmgiahcphecmpfh" cohort="1::" cohortname="" status="ok"><updatecheck codebase="https://clients2.googleusercontent.com/crx/blobs/abc/EXT_2026_1124.crx?a=1&amp;b=2" fp="1.x" hash_sha256="00" size="100" status="ok" version="2026.1124.1811"/></app></gupdate>"#;
        assert_eq!(
            parse_answer(xml),
            Some((
                "2026.1124.1811".into(),
                "https://clients2.googleusercontent.com/crx/blobs/abc/EXT_2026_1124.crx?a=1&b=2"
                    .into()
            ))
        );
        let none = r#"<gupdate><app appid="x" status="ok"><updatecheck status="noupdate"/></app></gupdate>"#;
        assert_eq!(parse_answer(none), None);
        // Only from the web's secure scheme.
        let plain =
            r#"<app><updatecheck codebase="http://x/y.crx" status="ok" version="2"/></app>"#;
        assert_eq!(parse_answer(plain), None);
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("2026.1124.1811", "2026.1116.1650"));
        assert!(newer("1.10", "1.9"));
        assert!(newer("1.0.1", "1.0"));
        assert!(!newer("1.0", "1.0.0"));
        assert!(!newer("1.9", "1.10"));
    }

    #[test]
    fn an_update_that_asks_for_more_waits() {
        let old = Manifest {
            permissions: vec!["storage".into()],
            hosts: vec!["https://example.com/*".into()],
            ..Default::default()
        };
        let same = old.clone();
        assert!(asks_more(&old, &same).is_empty());
        let more = Manifest {
            permissions: vec!["storage".into(), "history".into()],
            ..old.clone()
        };
        assert!(!asks_more(&old, &more).is_empty());
        let everywhere = Manifest {
            hosts: vec!["<all_urls>".into()],
            ..Default::default()
        };
        // Had every site already: a new site isn't more.
        let site = Manifest {
            hosts: vec!["https://new.example/*".into()],
            ..Default::default()
        };
        assert!(asks_more(&everywhere, &site).is_empty());
        assert!(!asks_more(&old, &everywhere).is_empty());
    }

    #[test]
    fn a_check_is_due_once_a_day() {
        let state = State {
            checked: 1_000_000,
            ..Default::default()
        };
        assert!(!due(&state, 1_000_000 + 3600));
        assert!(due(&state, 1_000_000 + 24 * 3600));
        assert!(due(&State::default(), 1_000_000));
    }
}
