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

/// What the store's update service offers: a newer version, where its package is, and (when it
/// says) the package's SHA-256.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub version: String,
    pub codebase: String,
    pub sha256: Option<String>,
}

/// The offer in the service's answer (`<updatecheck codebase="…" version="…" hash_sha256="…"
/// status="ok"/>`); `None` for `noupdate`, or a package anywhere but Google's update hosts.
pub fn parse_answer(xml: &str) -> Option<Offer> {
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
    let codebase = attr("codebase").filter(|c| update_host(c))?;
    Some(Offer {
        version: attr("version")?,
        codebase,
        sha256: attr("hash_sha256").filter(|h| h.len() == 64),
    })
}

/// Whether `url` is on one of the hosts Google serves extension packages from.
pub fn update_host(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest
        .split(['/', '?', '#', ':'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        host.as_str(),
        "clients2.google.com" | "clients2.googleusercontent.com" | "update.googleapis.com"
    ) || host.ends_with(".gvt1.com")
}

/// The manifest's parts an update may change by itself: everything else waits for the user.
const FREE_TO_CHANGE: &[&str] = &[
    "version",
    "version_name",
    "description",
    "icons",
    "name",
    "short_name",
    "update_url",
];

/// What differs between two manifests beyond what an update may change by itself (both read as
/// Trek runs them, after its own changes): the keys, for the user to see.
pub fn manifest_changes(old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
    let as_trek_runs = |m: &serde_json::Value| -> serde_json::Value {
        let mut m = super::compat::plan(m).manifest.unwrap_or_else(|| m.clone());
        if let Some(o) = m.as_object_mut() {
            for k in FREE_TO_CHANGE {
                o.remove(*k);
            }
        }
        m
    };
    let (old, new) = (as_trek_runs(old), as_trek_runs(new));
    let empty = serde_json::Map::new();
    let (o, n) = (
        old.as_object().unwrap_or(&empty),
        new.as_object().unwrap_or(&empty),
    );
    let mut keys: Vec<String> = o
        .keys()
        .chain(n.keys())
        .filter(|k| o.get(*k) != n.get(*k))
        .cloned()
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// What an update waiting for the user changes, in words.
fn changes_in_words(keys: &[String], old: &Manifest, new: &Manifest) -> Vec<String> {
    let mut words = asks_more(old, new);
    let rest: Vec<&str> = keys
        .iter()
        .map(String::as_str)
        .filter(|k| {
            !matches!(
                *k,
                "permissions" | "host_permissions" | "optional_permissions"
            )
        })
        .collect();
    if !rest.is_empty() {
        words.push(format!("its manifest changes: {}", rest.join(", ")));
    }
    if words.is_empty() {
        words.push(format!("its manifest changes: {}", keys.join(", ")));
    }
    words
}

/// The file that marks an extension Trek installed from the Chrome Web Store (the only ones it
/// updates: one from a file or folder is the user's to update, whatever its id).
pub const STORE_MARK: &str = ".trek-store";

pub fn from_store(folder: &Path) -> bool {
    folder.join(STORE_MARK).is_file()
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

/// What a package may be at most.
pub const MAX_PACKAGE: usize = 64 << 20;

/// `response`'s body, at most `max` bytes, refused as it arrives past that.
pub async fn read_capped(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err("it is too large".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > max {
            return Err("it is too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
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
    // What an earlier check left (one that stopped half way).
    if let Ok(entries) = std::fs::read_dir(&incoming) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().ends_with("-update") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
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
        let Some(offer) = answer.as_deref().and_then(parse_answer) else {
            continue;
        };
        let new_version = offer.version.clone();
        if !newer(&new_version, &version) {
            continue;
        }
        let bytes = async {
            let r = client
                .get(&offer.codebase)
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(|e| e.to_string())?;
            // Redirected, it must still end on Google's update hosts.
            if !update_host(r.url().as_str()) {
                return Err(format!(
                    "it was sent from {}",
                    r.url().host_str().unwrap_or("?")
                ));
            }
            read_capped(r, MAX_PACKAGE).await
        }
        .await;
        let bytes = match bytes {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    "browser extensions: {id}: update {new_version} didn't download: {e}"
                );
                continue;
            }
        };
        // The package as the store signed it, for this extension (as Chrome checks it).
        if let Some(want) = &offer.sha256 {
            use sha2::Digest as _;
            let got: String = sha2::Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            if !got.eq_ignore_ascii_case(want) {
                tracing::warn!(
                    "browser extensions: {id}: update {new_version}: its hash isn't the one the store gave"
                );
                continue;
            }
        }
        let zip = match super::crx::verified(&bytes, &id, true) {
            Ok(zip) => zip,
            Err(e) => {
                tracing::warn!("browser extensions: {id}: update {new_version} refused: {e}");
                continue;
            }
        };
        let dest = incoming.join(format!("{id}-update"));
        if let Err(e) = super::unpack_crx(zip, &dest) {
            let _ = std::fs::remove_dir_all(&dest);
            tracing::warn!("browser extensions: {id}: update {new_version}: {e}");
            continue;
        }
        let _ = std::fs::write(dest.join(STORE_MARK), "");
        let read = |f: &Path| -> Option<(Manifest, serde_json::Value)> {
            let text = std::fs::read_to_string(f.join("manifest.json")).ok()?;
            let json = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
            Some((manifest::read(f).ok()?, json))
        };
        let (Some((old, old_json)), Some((new, new_json))) = (read(&folder), read(&dest)) else {
            let _ = std::fs::remove_dir_all(&dest);
            continue;
        };
        let changed = manifest_changes(&old_json, &new_json);
        if changed.is_empty() {
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
                asks: changes_in_words(&changed, &old, &new),
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
            Some(Offer {
                version: "2026.1124.1811".into(),
                codebase:
                    "https://clients2.googleusercontent.com/crx/blobs/abc/EXT_2026_1124.crx?a=1&b=2"
                        .into(),
                sha256: None,
            })
        );
        // Only Google's update hosts.
        let elsewhere = r#"<app><updatecheck codebase="https://evil.example/x.crx" status="ok" version="2"/></app>"#;
        assert_eq!(parse_answer(elsewhere), None);
        assert!(update_host("https://edgedl.me.gvt1.com/edgedl/x.crx"));
        assert!(!update_host("https://gvt1.com.evil.example/x.crx"));
        assert!(!update_host("https://clients2.google.com.evil.example/"));
        let hashed = r#"<app><updatecheck codebase="https://clients2.google.com/x.crx" hash_sha256="0000000000000000000000000000000000000000000000000000000000000000" status="ok" version="2"/></app>"#;
        assert!(parse_answer(hashed).unwrap().sha256.is_some());
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
    fn any_manifest_change_but_the_version_waits() {
        use serde_json::json;
        let old = json!({
            "manifest_version": 3, "name": "X", "version": "1.0",
            "background": { "service_worker": "bg.js" },
            "permissions": ["storage"],
            "content_scripts": [{ "matches": ["<all_urls>"], "js": ["c.js"] }],
        });
        let mut same = old.clone();
        same["version"] = json!("1.1");
        same["description"] = json!("new words");
        assert!(manifest_changes(&old, &same).is_empty());
        for (key, value) in [
            (
                "content_scripts",
                json!([{ "matches": ["<all_urls>"], "js": ["c.js"], "world": "MAIN", "all_frames": true }]),
            ),
            (
                "externally_connectable",
                json!({ "matches": ["https://*.example.com/*"] }),
            ),
            (
                "web_accessible_resources",
                json!([{ "resources": ["x.html"], "matches": ["<all_urls>"] }]),
            ),
            (
                "content_security_policy",
                json!({ "extension_pages": "script-src 'self' 'unsafe-eval'" }),
            ),
            (
                "declarative_net_request",
                json!({ "rule_resources": [{ "id": "r", "enabled": true, "path": "r.json" }] }),
            ),
        ] {
            let mut changed = same.clone();
            changed[key] = value;
            assert_eq!(
                manifest_changes(&old, &changed),
                vec![key.to_string()],
                "{key}"
            );
        }
        // As installed (Trek's shim and channel in it), the same.
        let installed = super::super::compat::plan(&old).manifest.unwrap();
        assert!(manifest_changes(&installed, &same).is_empty());
    }

    #[test]
    fn only_store_installs_are_updated() {
        let dir = std::env::temp_dir().join(format!("trek-store-mark-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!from_store(&dir));
        std::fs::write(dir.join(STORE_MARK), "").unwrap();
        assert!(from_store(&dir));
        let _ = std::fs::remove_dir_all(&dir);
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
