//! Chrome APIs WebKit's extensions lack, made from what WebKit has, for the extensions Trek
//! unpacked (macOS). WebKit runs an extension's background as it is, and a background that
//! touches an API WebKit doesn't have stops there: Grammarly at `chrome.storage.managed`, React
//! Developer Tools at `chrome.scripting.ExecutionWorld`, Vimium at
//! `chrome.webNavigation.onHistoryStateUpdated`. So before WebKit reads one, Trek puts a small
//! script of its own (`trek-shim.js`, [`SHIM`]) in front of its background: an MV3 service worker
//! is started by a wrapper beside it (`trek-background.js`) that runs the shim, then the
//! extension's own worker; an MV2 background's script list starts with it.
//!
//! The shim adds only what is true here and adds nothing WebKit already has:
//! - `storage.managed`: an empty store. Managed storage is what an administrator's policy sets
//!   for an extension; nothing sets one in Trek, so it is empty, as in a Chrome without policies.
//! - `scripting.ExecutionWorld`: the enum's two values. WebKit takes `world: "MAIN"` and
//!   `"ISOLATED"` (uBlock Origin Lite's scriptlets run in the main world); only the constant
//!   object is missing.
//! - `webNavigation.onHistoryStateUpdated` / `onReferenceFragmentUpdated`: a tab's address that
//!   changes while it isn't loading (`history.pushState`, a `#fragment`), from `tabs.onUpdated`,
//!   for the main frame (WebKit tells extensions of no frame's in-page navigation).
//!
//! What it can't make stays missing (`chrome.notifications`, native messaging): those
//! extensions still say why they don't run. A folder of the user's own (an unpacked extension
//! they develop) is never changed.

use serde_json::Value;
use std::path::Path;

/// The shim's file, beside the background it runs before.
pub const SHIM_FILE: &str = "trek-shim.js";
/// The MV3 wrapper that starts the shim and then the extension's own service worker.
pub const WRAPPER_FILE: &str = "trek-background.js";

/// The shim (see the module's notes). Plain script: it runs as a classic worker's first
/// `importScripts`, an ES module's first import, or an MV2 background's first script.
pub const SHIM: &str = r#"// Added by Trek: Chrome APIs WebKit lacks, made from what WebKit has.
// (crates/trek-app/src/panels/browser_extensions/compat.rs)
(() => {
  const api = globalThis.chrome || globalThis.browser;
  if (!api) return;
  const event = () => {
    const listeners = new Set();
    return {
      addListener: (f) => { listeners.add(f); },
      removeListener: (f) => { listeners.delete(f); },
      hasListener: (f) => listeners.has(f),
      hasListeners: () => listeners.size > 0,
      dispatch: (details) => {
        for (const f of listeners) {
          try { f(details); } catch (e) { console.error(e); }
        }
      },
    };
  };
  const define = (object, key, value) => {
    try {
      Object.defineProperty(object, key, { value, configurable: true, enumerable: true });
    } catch {}
  };
  // No administrator's policy sets anything here: an empty managed store.
  if (api.storage && !api.storage.managed) {
    const answer = (value, callback) => {
      if (typeof callback === 'function') { Promise.resolve().then(() => callback(value)); return; }
      return Promise.resolve(value);
    };
    define(api.storage, 'managed', {
      get: (keys, callback) => answer({}, typeof keys === 'function' ? keys : callback),
      getBytesInUse: (keys, callback) => answer(0, typeof keys === 'function' ? keys : callback),
      getKeys: (callback) => answer([], callback),
      onChanged: event(),
    });
  }
  // WebKit takes these worlds; only the constants are missing.
  if (api.scripting && !api.scripting.ExecutionWorld) {
    define(api.scripting, 'ExecutionWorld', Object.freeze({ ISOLATED: 'ISOLATED', MAIN: 'MAIN' }));
  }
  // In-page navigations of a tab's main frame: its address changing while it isn't loading.
  const nav = api.webNavigation;
  if (nav && api.tabs && api.tabs.onUpdated && (!nav.onHistoryStateUpdated || !nav.onReferenceFragmentUpdated)) {
    const history = nav.onHistoryStateUpdated ? null : event();
    const fragment = nav.onReferenceFragmentUpdated ? null : event();
    if (history) define(nav, 'onHistoryStateUpdated', history);
    if (fragment) define(nav, 'onReferenceFragmentUpdated', fragment);
    const seen = new Map();
    const strip = (url) => String(url || '').split('#')[0];
    api.tabs.onUpdated.addListener((tabId, change, tab) => {
      const url = change.url || (tab && tab.url) || '';
      if (change.status === 'loading') { seen.set(tabId, { url, loading: true }); return; }
      if (change.status === 'complete') { seen.set(tabId, { url, loading: false }); return; }
      if (!change.url) return;
      const before = seen.get(tabId);
      const loading = tab && tab.status === 'loading';
      seen.set(tabId, { url, loading: !!loading });
      if (loading || !before || before.loading || before.url === url) return;
      const details = {
        tabId, url, frameId: 0, parentFrameId: -1, processId: -1,
        timeStamp: Date.now(), transitionType: 'link', transitionQualifiers: [],
      };
      const target = strip(before.url) === strip(url) ? fragment : history;
      if (target) target.dispatch(details);
    });
    if (api.tabs.onRemoved) api.tabs.onRemoved.addListener((tabId) => seen.delete(tabId));
  }
})();
"#;

/// What [`patch`] does to a manifest: the files to write beside it (path in the folder, text),
/// and the manifest as it should be, when it changes.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub files: Vec<(String, String)>,
    pub manifest: Option<Value>,
}

/// The shim's place for `manifest`'s background (see the module's notes); nothing for an
/// extension without a script background (a background page, or none).
pub fn plan(manifest: &Value) -> Plan {
    let background = &manifest["background"];
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
            format!("import './{SHIM_FILE}';\nimport './{name}';\n")
        } else {
            format!(
                "importScripts('{SHIM_FILE}', '{}');\n",
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
    Ok(())
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
                    "import './trek-shim.js';\nimport './background.js';\n".to_string()
                ),
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
                "importScripts('trek-shim.js', 'background.js');\n".to_string()
            )
        );
        assert_eq!(
            plan.manifest.unwrap()["background"]["service_worker"],
            "trek-background.js"
        );
    }

    #[test]
    fn patched_again_only_the_shim_is_written() {
        let m = json!({ "background": { "service_worker": "build/trek-background.js" } });
        let again = plan(&m);
        assert_eq!(again.manifest, None);
        assert_eq!(
            again.files,
            vec![("build/trek-shim.js".to_string(), SHIM.to_string())]
        );
        let m = json!({ "background": { "scripts": ["trek-shim.js", "bg.js"] } });
        assert_eq!(plan(&m).manifest, None);
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
    fn a_background_page_or_none_is_left_alone() {
        assert_eq!(
            plan(&json!({ "background": { "page": "bg.html" } })),
            Plan::default()
        );
        assert_eq!(plan(&json!({ "name": "x" })), Plan::default());
    }

    #[test]
    fn patching_a_folder_twice_changes_it_once() {
        let dir = std::env::temp_dir().join(format!("trek-compat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("js")).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"background":{"service_worker":"js/bg.js","type":"module"}}"#,
        )
        .unwrap();
        patch(&dir).unwrap();
        let once = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
        patch(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("manifest.json")).unwrap(),
            once
        );
        assert!(once.contains("js/trek-background.js"));
        assert_eq!(
            std::fs::read_to_string(dir.join("js/trek-background.js")).unwrap(),
            "import './trek-shim.js';\nimport './bg.js';\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("js/trek-shim.js")).unwrap(),
            SHIM
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
