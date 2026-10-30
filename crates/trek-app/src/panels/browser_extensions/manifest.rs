//! What an unpacked extension's `manifest.json` says about it, for the Extensions page and the
//! permission prompt before it's installed: its name (localized, as `__MSG_name__` names are),
//! version, icon, options page, and what it asks to reach. And its id, as Chromium makes one for
//! a folder: from the manifest's `key` when it has one (the store's id), else from where it is.

use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub manifest_version: u32,
    /// The biggest icon up to 128 px (the toolbar's action icon first), in the folder.
    pub icon: Option<PathBuf>,
    pub has_popup: bool,
    pub has_options: bool,
    /// API permissions (`storage`, `tabs`…), as asked for.
    pub permissions: Vec<String>,
    /// Sites it reaches: host permissions and its content scripts' matches.
    pub hosts: Vec<String>,
    /// The manifest's `key` made into its id (an extension from a store keeps its id unpacked).
    pub key_id: Option<String>,
}

/// The extension in `folder`, from its manifest (and its default locale's messages).
pub fn read(folder: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(folder.join("manifest.json")).map_err(|_| {
        "There's no manifest.json in it: it isn't an unpacked extension.".to_string()
    })?;
    let json: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("Its manifest.json doesn't parse ({e})."))?;
    let messages = locale_messages(folder, json["default_locale"].as_str());
    let text = |v: &Value| localize(v.as_str().unwrap_or(""), &messages);
    let strings = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let manifest_version = json["manifest_version"].as_u64().unwrap_or(2) as u32;
    // MV2 mixes sites into `permissions`; MV3 keeps them apart.
    let (mut permissions, mut hosts): (Vec<String>, Vec<String>) = strings(&json["permissions"])
        .into_iter()
        .partition(|p| !is_host(p));
    hosts.extend(strings(&json["host_permissions"]));
    for script in json["content_scripts"].as_array().into_iter().flatten() {
        hosts.extend(strings(&script["matches"]));
    }
    permissions.dedup();
    let mut seen = std::collections::HashSet::new();
    hosts.retain(|h| seen.insert(h.clone()));
    let action = if json["action"].is_object() {
        &json["action"]
    } else if json["browser_action"].is_object() {
        &json["browser_action"]
    } else {
        &json["page_action"]
    };
    let name = Some(text(&json["name"]))
        .filter(|n| !n.is_empty())
        .or_else(|| Some(text(&json["short_name"])).filter(|n| !n.is_empty()))
        .unwrap_or_else(|| folder_name(folder));
    Ok(Manifest {
        name,
        version: json["version"].as_str().unwrap_or("").to_string(),
        description: text(&json["description"]),
        manifest_version,
        icon: icon(folder, &action["default_icon"]).or_else(|| icon(folder, &json["icons"])),
        has_popup: action["default_popup"]
            .as_str()
            .is_some_and(|p| !p.is_empty()),
        has_options: json["options_page"].is_string() || json["options_ui"]["page"].is_string(),
        permissions,
        hosts,
        key_id: json["key"].as_str().and_then(key_id),
    })
}

fn folder_name(folder: &Path) -> String {
    folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A site pattern (`<all_urls>`, `*://*/*`, `https://example.com/*`) rather than an API name.
fn is_host(p: &str) -> bool {
    p == "<all_urls>" || p.contains("://")
}

/// The default locale's `messages.json`: message name (lower case) to text.
fn locale_messages(
    folder: &Path,
    locale: Option<&str>,
) -> std::collections::HashMap<String, String> {
    let Some(locale) = locale else {
        return Default::default();
    };
    let path = folder.join("_locales").join(locale).join("messages.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Default::default();
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}'))
    else {
        return Default::default();
    };
    map.into_iter()
        .filter_map(|(k, v)| Some((k.to_lowercase(), v["message"].as_str()?.to_string())))
        .collect()
}

/// `__MSG_name__` in its locale's words (names are case-insensitive); other text as it is.
fn localize(s: &str, messages: &std::collections::HashMap<String, String>) -> String {
    s.strip_prefix("__MSG_")
        .and_then(|rest| rest.strip_suffix("__"))
        .and_then(|key| messages.get(&key.to_lowercase()))
        .cloned()
        .unwrap_or_else(|| s.to_string())
}

/// The biggest icon up to 128 px in an icon entry (a path, or sizes to paths), when it's there.
fn icon(folder: &Path, entry: &Value) -> Option<PathBuf> {
    let path = match entry {
        Value::String(path) => path.clone(),
        Value::Object(sizes) => {
            let mut sizes: Vec<(u32, &str)> = sizes
                .iter()
                .filter_map(|(size, path)| Some((size.parse().ok()?, path.as_str()?)))
                .collect();
            sizes.sort();
            let fit = sizes.iter().rev().find(|(size, _)| *size <= 128);
            fit.or(sizes.first())?.1.to_string()
        }
        _ => return None,
    };
    let path = folder.join(path.trim_start_matches('/'));
    path.is_file().then_some(path)
}

/// Chromium's alphabet for ids: each hex digit of a hash's first 16 bytes as a letter a–p.
fn id_of(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    hash[..16]
        .iter()
        .flat_map(|b| [b >> 4, b & 0xf])
        .map(|n| (b'a' + n) as char)
        .collect()
}

/// The id a manifest's `key` (its public key, base64) gives the extension.
pub fn key_id(key: &str) -> Option<String> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(key.split_whitespace().collect::<String>())
        .ok()?;
    Some(id_of(&der))
}

/// The id of the extension in `folder`: its manifest key's, else one made from the folder's path
/// (as Chromium does for an unpacked extension), so the same folder keeps its id.
pub fn folder_id(folder: &Path, manifest: &Manifest) -> String {
    manifest.key_id.clone().unwrap_or_else(|| {
        let path = folder
            .canonicalize()
            .unwrap_or_else(|_| folder.to_path_buf());
        id_of(path.to_string_lossy().as_bytes())
    })
}

/// The permissions in words, the way the prompt before installing lists them: the sites first,
/// then what the APIs let it do (ones that say nothing to a person are left out).
pub fn describe(manifest: &Manifest) -> Vec<String> {
    let mut out = Vec::new();
    let all_sites = manifest
        .hosts
        .iter()
        .any(|h| h == "<all_urls>" || h.starts_with("*://*/") || h.contains("://*/*"));
    if all_sites {
        out.push("Read and change everything on every website".to_string());
    } else if !manifest.hosts.is_empty() {
        let mut sites: Vec<String> = manifest
            .hosts
            .iter()
            .filter_map(|h| h.split("://").nth(1)?.split('/').next().map(str::to_string))
            .collect();
        sites.dedup();
        let shown = sites.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        let more = sites.len().saturating_sub(3);
        out.push(if more > 0 {
            format!("Read and change your data on {shown} and {more} more")
        } else {
            format!("Read and change your data on {shown}")
        });
    }
    for p in &manifest.permissions {
        let said = match p.as_str() {
            "tabs" => "See your open tabs and their addresses",
            "history" => "Read and change your browsing history",
            "bookmarks" => "Read and change your bookmarks",
            "cookies" => "Read and change cookies",
            "clipboardRead" => "Read what you copy",
            "clipboardWrite" => "Change what's on the clipboard",
            "downloads" => "Manage your downloads",
            "nativeMessaging" => "Talk to other apps you have installed",
            "webNavigation" => "See where your pages go",
            "webRequest" | "webRequestBlocking" => "See the requests pages make",
            "declarativeNetRequest" | "declarativeNetRequestWithHostAccess" => {
                "Block content on any page"
            }
            "scripting" | "userScripts" => "Run its scripts in pages",
            "notifications" => "Show notifications",
            "privacy" => "Change privacy settings",
            "management" => "Manage your other extensions",
            "identity" => "Know who you are signed in as",
            "geolocation" => "Know your location",
            _ => continue,
        };
        if !out.iter().any(|o| o == said) {
            out.push(said.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(files: &[(&str, &str)]) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "trek-manifest-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, body) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        dir
    }

    #[test]
    fn a_manifest_says_what_the_extension_is_and_reaches() {
        let dir = folder(&[
            (
                "manifest.json",
                r#"{
                    "manifest_version": 3,
                    "name": "__MSG_extName__",
                    "version": "1.2",
                    "default_locale": "en",
                    "icons": {"16": "i16.png", "48": "i48.png", "256": "i256.png"},
                    "action": {"default_popup": "popup.html"},
                    "options_ui": {"page": "options.html"},
                    "permissions": ["storage", "tabs"],
                    "host_permissions": ["https://example.com/*"],
                    "content_scripts": [{"matches": ["https://example.com/*", "https://a.org/*"], "js": ["c.js"]}]
                }"#,
            ),
            (
                "_locales/en/messages.json",
                r#"{"extname": {"message": "Example Tool"}}"#,
            ),
            ("i16.png", "."),
            ("i48.png", "."),
            ("i256.png", "."),
        ]);
        let m = read(&dir).unwrap();
        assert_eq!(m.name, "Example Tool");
        assert_eq!(m.version, "1.2");
        assert_eq!(m.manifest_version, 3);
        assert_eq!(m.icon, Some(dir.join("i48.png")));
        assert!(m.has_popup && m.has_options);
        assert_eq!(m.permissions, ["storage", "tabs"]);
        assert_eq!(m.hosts, ["https://example.com/*", "https://a.org/*"]);
        assert_eq!(
            describe(&m),
            [
                "Read and change your data on example.com, a.org",
                "See your open tabs and their addresses"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_v2_sites_in_permissions_are_sites() {
        let dir = folder(&[(
            "manifest.json",
            r#"{"manifest_version": 2, "name": "Old", "permissions": ["<all_urls>", "storage", "webRequest"]}"#,
        )]);
        let m = read(&dir).unwrap();
        assert_eq!(m.permissions, ["storage", "webRequest"]);
        assert_eq!(m.hosts, ["<all_urls>"]);
        assert_eq!(
            describe(&m),
            [
                "Read and change everything on every website",
                "See the requests pages make"
            ]
        );
        assert!(read(&dir.join("nowhere")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ids_are_chromiums() {
        // SHA-256 of the key's bytes, its first half in Chromium's letters (checked against
        // Python's hashlib).
        assert_eq!(
            key_id("AAAA").as_deref(),
            Some("hajoiamiieihkcebbobooenpljpcckig")
        );
        assert_eq!(
            key_id("AA\nAA").as_deref(),
            Some("hajoiamiieihkcebbobooenpljpcckig")
        );
        assert_eq!(key_id("not base64!"), None);
        let dir = folder(&[("manifest.json", r#"{"name": "x"}"#)]);
        let m = read(&dir).unwrap();
        assert_eq!(folder_id(&dir, &m), folder_id(&dir, &m));
        assert_ne!(folder_id(&dir, &m), folder_id(&dir.join(".."), &m));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
