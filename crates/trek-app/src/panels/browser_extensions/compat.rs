//! Chrome APIs WebKit's extensions lack, made from what WebKit has or answered by Trek, for the
//! extensions Trek unpacked (macOS). WebKit runs an extension's background as it is, and a
//! background that touches an API WebKit doesn't have stops there (Grammarly at
//! `chrome.storage.managed`, Bitwarden and 1Password at `chrome.notifications`, Privacy Badger at
//! `chrome.webRequest.OnBeforeSendHeadersOptions`). So before WebKit reads one, Trek puts a
//! script of its own (`trek-shim.js`, [`SHIM`]) in front of its background (an MV3 service
//! worker is started by a wrapper beside it, `trek-background.js`, that imports the shim by its
//! version, then the extension's own worker; a `scripts` list starts with it) and of each of its
//! pages, adds `nativeMessaging` to its permissions for Trek's own channel (`bridge`), and, when
//! it has DevTools panels, writes Trek's page to show them in ([`DEVTOOLS_FILE`]).
//!
//! What the shim adds is listed in `trek-shim.js` itself: constants and events made from what
//! WebKit has, fixes to what WebKit hands extensions (an iframe's request typed `main_frame`, a
//! sender without an `origin`), and the APIs Trek answers over its channel (`notifications`,
//! `offscreen`, `identity.launchWebAuthFlow`, `idle`, `downloads`, `sidePanel`) or answers
//! honestly as not changeable here (`privacy`). A folder of the user's own (an unpacked
//! extension they develop) is never changed.

use serde_json::Value;
use std::path::Path;

/// The shim's file, beside the background it runs before.
pub const SHIM_FILE: &str = "trek-shim.js";
/// The MV3 wrapper that starts the shim and then the extension's own service worker.
pub const WRAPPER_FILE: &str = "trek-background.js";

/// The shim (see the module's notes, and the file itself). Plain script: it runs as a classic
/// worker's first `importScripts`, an ES module's first import, an MV2 background's first script,
/// or an extension page's first `<script>`.
pub const SHIM: &str = include_str!("trek-shim.js");

/// The permission the shim needs for Trek's own channel (`runtime.sendNativeMessage` to
/// `dev.trek.browser`, see `bridge`), added to an extension that didn't ask for it.
pub const BRIDGE_PERMISSION: &str = "nativeMessaging";
/// The manifest key that lists what Trek added (`BRIDGE_PERMISSION`), so the Extensions page
/// still says only what the extension itself asked for.
pub const ADDED_KEY: &str = "trek_added_permissions";
/// What an extension page gets as its first script (`page_tag`, with the shim's version).
const PAGE_TAG_START: &str = "<script src=\"/trek-shim.js";

/// Trek's page for an extension's DevTools panels (in its folder, so it is one of its pages).
pub const DEVTOOLS_FILE: &str = "trek-devtools.html";

/// What starts the DevTools page inside Trek's: the inspected tab is the one in front.
const DEVTOOLS_JS: &str = r#"// Added by Trek: runs the extension's DevTools page for the tab in front (see trek-shim.js).
chrome.tabs.query({ active: true, lastFocusedWindow: true }).then(([tab]) => {
  const page = document.body.dataset.page;
  document.title = `${chrome.runtime.getManifest().name}: ${tab ? tab.title || tab.url : 'no tab'}`;
  if (!tab) { document.getElementById('trek-devtools-panels').textContent = 'Open a page in the Browser first.'; return; }
  const frame = document.createElement('iframe');
  frame.hidden = true;
  frame.src = `/${page.replace(/^\//, '')}${page.includes('?') ? '&' : '?'}trek_tab=${tab.id}`;
  document.body.appendChild(frame);
});
"#;

fn devtools_host(page: &str) -> String {
    let page = page.replace(['"', '<', '>', '&'], "");
    format!(
        "<!doctype html>\n<html><head>{}<meta charset=\"utf-8\"><title>DevTools</title>\n<style>\n\
         :root {{ color-scheme: light dark; font: 12px -apple-system, sans-serif; }}\n\
         body {{ margin: 0; height: 100vh; display: flex; flex-direction: column; }}\n\
         #trek-devtools-tabs {{ display: flex; gap: 2px; padding: 4px 6px; border-bottom: 1px solid rgba(128,128,128,.3); }}\n\
         #trek-devtools-tabs button {{ font: inherit; border: 0; background: none; padding: 4px 10px; border-radius: 5px; color: inherit; }}\n\
         #trek-devtools-tabs button[aria-selected=true] {{ background: rgba(128,128,128,.22); }}\n\
         #trek-devtools-panels {{ flex: 1; position: relative; }}\n\
         #trek-devtools-panels iframe {{ position: absolute; inset: 0; width: 100%; height: 100%; border: 0; }}\n\
         </style></head>\n<body data-page=\"{page}\"><div id=\"trek-devtools-tabs\"></div>\
         <div id=\"trek-devtools-panels\"></div><script src=\"/trek-devtools.js\"></script></body></html>\n",
        page_tag()
    )
}

/// The shim's version: a hash of it. Its address carries it (`trek-shim.js?v=…`) because WebKit
/// keeps a background's imported scripts from its first run, fetching them again only when the
/// background's own script changes: the wrapper (or page) naming a new version is that change.
pub fn version() -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for b in SHIM.bytes() {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

/// The shim's file name with its version, as a script names it.
fn shim_ref() -> String {
    format!("{SHIM_FILE}?v={}", version())
}

fn page_tag() -> String {
    format!("<script src=\"/{}\"></script>", shim_ref())
}

/// What [`patch`] does to a manifest: the files to write beside it (path in the folder, text),
/// and the manifest as it should be, when it changes.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub files: Vec<(String, String)>,
    pub manifest: Option<Value>,
}

/// The shim's place for `manifest`'s background (see the module's notes), with the shim at the
/// folder's root for its pages, and Trek's channel among its permissions.
pub fn plan(manifest: &Value) -> Plan {
    let mut plan = background_plan(manifest);
    if !plan.files.iter().any(|(f, _)| f == SHIM_FILE) {
        plan.files.push((SHIM_FILE.to_string(), SHIM.to_string()));
    }
    // An extension with DevTools panels gets Trek's page to show them in (`trek-shim.js`'s
    // `chrome.devtools`).
    if let Some(page) = manifest["devtools_page"].as_str() {
        plan.files
            .push((DEVTOOLS_FILE.to_string(), devtools_host(page)));
        plan.files
            .push(("trek-devtools.js".to_string(), DEVTOOLS_JS.to_string()));
    }
    let base = plan.manifest.as_ref().unwrap_or(manifest);
    let has = |list: &Value| {
        list.as_array()
            .is_some_and(|a| a.iter().any(|p| p.as_str() == Some(BRIDGE_PERMISSION)))
    };
    if !has(&base["permissions"]) {
        let mut patched = base.clone();
        match patched["permissions"].as_array_mut() {
            Some(list) => list.push(Value::String(BRIDGE_PERMISSION.into())),
            None => patched["permissions"] = Value::Array(vec![BRIDGE_PERMISSION.into()]),
        }
        // Asked for later (`optional_permissions`), it was the extension's own wish: not added.
        if !has(&base["optional_permissions"]) {
            match patched[ADDED_KEY].as_array_mut() {
                Some(list) => list.push(Value::String(BRIDGE_PERMISSION.into())),
                None => patched[ADDED_KEY] = Value::Array(vec![BRIDGE_PERMISSION.into()]),
            }
        }
        plan.manifest = Some(patched);
    }
    plan
}

/// The shim in front of `manifest`'s background (see the module's notes); nothing for an
/// extension without a script background (a background page, or none).
fn background_plan(manifest: &Value) -> Plan {
    // A manifest with both (Firefox's `scripts` beside Chrome's worker, as Privacy Badger has):
    // WebKit runs the scripts, so the shim goes in front of those.
    let background = &manifest["background"];
    if background["service_worker"].is_string() && background["scripts"].is_array() {
        let mut only = manifest.clone();
        if let Some(b) = only["background"].as_object_mut() {
            b.remove("service_worker");
        }
        let plan = background_plan(&only);
        return Plan {
            files: plan.files,
            manifest: plan.manifest.map(|mut m| {
                m["background"]["service_worker"] = background["service_worker"].clone();
                m
            }),
        };
    }
    if let Some(worker) = background["service_worker"].as_str() {
        let lead = if worker.starts_with('/') { "/" } else { "" };
        let path = worker.trim_start_matches('/');
        let (dir, name) = match path.rsplit_once('/') {
            Some((dir, name)) => (format!("{dir}/"), name),
            None => (String::new(), path),
        };
        let mut files = vec![(format!("{dir}{SHIM_FILE}"), SHIM.to_string())];
        // Already started by Trek's wrapper: only the shim is brought up to date.
        if name == WRAPPER_FILE {
            return Plan {
                files,
                manifest: None,
            };
        }
        let module = background["type"].as_str() == Some("module");
        // Beside the worker, so what it imports (or `importScripts`) resolves as it did.
        let wrapper = if module {
            format!("import './{}';\nimport './{name}';\n", shim_ref())
        } else {
            format!(
                "importScripts('{}', '{}');\n",
                shim_ref(),
                name.replace('\'', "\\'")
            )
        };
        files.push((format!("{dir}{WRAPPER_FILE}"), wrapper));
        let mut patched = manifest.clone();
        patched["background"]["service_worker"] =
            Value::String(format!("{lead}{dir}{WRAPPER_FILE}"));
        return Plan {
            files,
            manifest: Some(patched),
        };
    }
    if let Some(scripts) = background["scripts"].as_array() {
        let files = vec![(SHIM_FILE.to_string(), SHIM.to_string())];
        let first = scripts.first().and_then(Value::as_str);
        if first.is_some_and(|s| s.trim_start_matches('/') == SHIM_FILE) {
            return Plan {
                files,
                manifest: None,
            };
        }
        let mut patched = manifest.clone();
        let mut list = vec![Value::String(SHIM_FILE.to_string())];
        list.extend(scripts.iter().cloned());
        patched["background"]["scripts"] = Value::Array(list);
        return Plan {
            files,
            manifest: Some(patched),
        };
    }
    Plan::default()
}

/// Put the shim in front of the background of the unpacked extension in `folder` (one Trek
/// unpacked: never call it on a folder of the user's). Run again, it brings the shim up to date.
pub fn patch(folder: &Path) -> Result<(), String> {
    let path = folder.join("manifest.json");
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let manifest: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("Its manifest.json doesn't parse ({e})."))?;
    let plan = plan(&manifest);
    for (file, text) in &plan.files {
        let to = folder.join(file);
        if std::fs::read_to_string(&to).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&to, text).map_err(|e| format!("{file}: {e}"))?;
        }
    }
    if let Some(manifest) = plan.manifest {
        let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
    }
    // A wrapper written by an earlier Trek names an older shim: it names this one now.
    if let Some(worker) = manifest["background"]["service_worker"].as_str() {
        let worker = worker.trim_start_matches('/');
        let dir = worker.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let wrapper = folder.join(dir).join(WRAPPER_FILE);
        if let Ok(text) = std::fs::read_to_string(&wrapper) {
            let fresh = with_shim_version(&text);
            if fresh != text {
                std::fs::write(&wrapper, fresh).map_err(|e| format!("{WRAPPER_FILE}: {e}"))?;
            }
        }
    }
    patch_pages(folder, folder, 0);
    Ok(())
}

/// `text` (a wrapper or a page) naming this version of the shim wherever it names one.
pub fn with_shim_version(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(at) = rest.find(SHIM_FILE) {
        let end = at + SHIM_FILE.len();
        out.push_str(&rest[..at]);
        out.push_str(&shim_ref());
        rest = &rest[end..];
        // Drop an older version's `?v=…`.
        if let Some(after) = rest.strip_prefix("?v=") {
            let n = after.chars().take_while(|c| c.is_ascii_hexdigit()).count();
            rest = &after[n..];
        }
    }
    out.push_str(rest);
    out
}

/// Put the shim in front of every page in `dir` (its popup, options, offscreen documents, side
/// panel): an extension's pages call the same APIs its background does.
fn patch_pages(root: &Path, dir: &Path, depth: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if depth < 8 && !name.starts_with('_') && name != "node_modules" {
                patch_pages(root, &path, depth + 1);
            }
            continue;
        }
        if !(name.ends_with(".html") || name.ends_with(".htm")) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(patched) = page_with_shim(&text) {
            let _ = std::fs::write(&path, patched);
        }
    }
    let _ = root;
}

/// `html` with the shim as its first script, or `None` when it already has it.
pub fn page_with_shim(html: &str) -> Option<String> {
    if html.contains(PAGE_TAG_START) {
        let fresh = with_shim_version(html);
        return (fresh != html).then_some(fresh);
    }
    let lower = html.to_ascii_lowercase();
    // After `<head …>`, else `<html …>`, else a doctype, else at the very start.
    let after = |tag: &str| -> Option<usize> {
        let at = lower.find(tag)?;
        let next = lower[at + tag.len()..].chars().next()?;
        if next != '>' && !next.is_whitespace() {
            return None;
        }
        Some(at + lower[at..].find('>')? + 1)
    };
    let at = after("<head")
        .or_else(|| after("<html"))
        .or_else(|| after("<!doctype"))
        .unwrap_or(0);
    Some(format!("{}{}{}", &html[..at], page_tag(), &html[at..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_module_worker_is_started_by_a_wrapper_beside_it() {
        let m = json!({
            "manifest_version": 3,
            "background": { "service_worker": "/js/background.js", "type": "module" },
            "key": "abc",
        });
        let plan = plan(&m);
        assert_eq!(
            plan.files,
            vec![
                ("js/trek-shim.js".to_string(), SHIM.to_string()),
                (
                    "js/trek-background.js".to_string(),
                    format!("import './{}';\nimport './background.js';\n", shim_ref())
                ),
                // At the root too, for its pages.
                ("trek-shim.js".to_string(), SHIM.to_string()),
            ]
        );
        let patched = plan.manifest.unwrap();
        assert_eq!(
            patched["background"]["service_worker"],
            "/js/trek-background.js"
        );
        assert_eq!(patched["background"]["type"], "module");
        // The rest of the manifest (its key: its id) is as it was.
        assert_eq!(patched["key"], "abc");
    }

    #[test]
    fn a_classic_worker_imports_the_shim_first() {
        let m = json!({ "background": { "service_worker": "background.js" } });
        let plan = plan(&m);
        assert_eq!(
            plan.files[1],
            (
                "trek-background.js".to_string(),
                format!("importScripts('{}', 'background.js');\n", shim_ref())
            )
        );
        // One shim at the root serves the worker and the pages.
        assert_eq!(plan.files.len(), 2);
        assert_eq!(
            plan.manifest.unwrap()["background"]["service_worker"],
            "trek-background.js"
        );
    }

    #[test]
    fn trek_s_channel_is_added_and_marked_as_trek_s() {
        let m = json!({ "permissions": ["storage"] });
        let patched = plan(&m).manifest.unwrap();
        assert_eq!(
            patched["permissions"],
            json!(["storage", "nativeMessaging"])
        );
        assert_eq!(patched[ADDED_KEY], json!(["nativeMessaging"]));
        // One that asks for it itself keeps it as its own.
        let m = json!({ "permissions": ["nativeMessaging"] });
        assert_eq!(plan(&m).manifest, None);
        let m = json!({ "optional_permissions": ["nativeMessaging"] });
        let patched = plan(&m).manifest.unwrap();
        assert_eq!(patched["permissions"], json!(["nativeMessaging"]));
        assert!(patched.get(ADDED_KEY).is_none());
    }

    #[test]
    fn patched_again_only_the_shim_is_written() {
        let m = json!({ "background": { "service_worker": "build/trek-background.js" }, "permissions": ["nativeMessaging"] });
        let again = plan(&m);
        assert_eq!(again.manifest, None);
        assert_eq!(
            again.files,
            vec![
                ("build/trek-shim.js".to_string(), SHIM.to_string()),
                ("trek-shim.js".to_string(), SHIM.to_string())
            ]
        );
        let m = json!({ "background": { "scripts": ["trek-shim.js", "bg.js"] }, "permissions": ["nativeMessaging"] });
        assert_eq!(plan(&m).manifest, None);
    }

    #[test]
    fn with_scripts_and_a_worker_the_scripts_get_the_shim() {
        let m = json!({ "background": { "scripts": ["js/background.js"], "service_worker": "js/background.js", "type": "module" } });
        let patched = plan(&m).manifest.unwrap();
        assert_eq!(
            patched["background"],
            json!({ "scripts": ["trek-shim.js", "js/background.js"], "service_worker": "js/background.js", "type": "module" })
        );
    }

    #[test]
    fn an_mv2_background_starts_with_the_shim() {
        let m = json!({ "background": { "scripts": ["lib.js", "bg.js"], "persistent": false } });
        let patched = plan(&m).manifest.unwrap();
        assert_eq!(
            patched["background"]["scripts"],
            json!(["trek-shim.js", "lib.js", "bg.js"])
        );
        assert_eq!(patched["background"]["persistent"], false);
    }

    #[test]
    fn a_background_page_or_none_gets_only_the_page_shim() {
        let p = plan(&json!({ "background": { "page": "bg.html" } }));
        assert_eq!(
            p.files,
            vec![("trek-shim.js".to_string(), SHIM.to_string())]
        );
        assert_eq!(
            p.manifest.unwrap()["background"],
            json!({ "page": "bg.html" })
        );
    }

    #[test]
    fn a_page_gets_the_shim_as_its_first_script_once() {
        let page =
            "<!DOCTYPE html>\n<html lang=en><head>\n<script src=popup.js></script></head></html>";
        let once = page_with_shim(page).unwrap();
        assert_eq!(
            once,
            format!(
                "<!DOCTYPE html>\n<html lang=en><head>{}\n<script src=popup.js></script></head></html>",
                page_tag()
            )
        );
        assert_eq!(page_with_shim(&once), None);
        // A page an earlier Trek patched names this shim now.
        let older = once.replace(&shim_ref(), "trek-shim.js?v=0badf00d");
        assert_eq!(page_with_shim(&older), Some(once.clone()));
        assert_eq!(
            with_shim_version("importScripts('trek-shim.js', 'sw.js');"),
            format!("importScripts('{}', 'sw.js');", shim_ref())
        );
        // `<header>` isn't `<head>`; no head: after `<html>`; nothing: at the start.
        assert_eq!(
            page_with_shim("<html><body><header>x</header></body></html>").unwrap(),
            format!("<html>{}<body><header>x</header></body></html>", page_tag())
        );
        assert_eq!(
            page_with_shim("<div id=root></div>").unwrap(),
            format!("{}<div id=root></div>", page_tag())
        );
    }

    #[test]
    fn patching_a_folder_twice_changes_it_once() {
        let dir = std::env::temp_dir().join(format!("trek-compat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("js")).unwrap();
        std::fs::create_dir_all(dir.join("popup")).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"background":{"service_worker":"js/bg.js","type":"module"}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("popup/popup.html"), "<html><head></head></html>").unwrap();
        patch(&dir).unwrap();
        let once = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
        let page = std::fs::read_to_string(dir.join("popup/popup.html")).unwrap();
        patch(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("manifest.json")).unwrap(),
            once
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("popup/popup.html")).unwrap(),
            page
        );
        assert!(page.contains(&page_tag()));
        assert!(once.contains("js/trek-background.js"));
        assert!(once.contains("nativeMessaging"));
        assert_eq!(
            std::fs::read_to_string(dir.join("js/trek-background.js")).unwrap(),
            format!("import './{}';\nimport './bg.js';\n", shim_ref())
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("js/trek-shim.js")).unwrap(),
            SHIM
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("trek-shim.js")).unwrap(),
            SHIM
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
