//! A plugin's pages and panels: HTML, CSS and JavaScript from the plugin's folder, run in the
//! system's web view with the interactive-artifact sandbox (`crate::artifact`). This module is
//! the plain part (trek-app owns the view): the origin a plugin is served from, the policy it's
//! served under, which file a request gets, the script Trek runs first in the page (`window.trek`),
//! and the check on every message the page sends.
//!
//! The fence: a plugin's pages are served under a scheme and origin of their own, with only the
//! plugin's own files (nothing above its folder, nothing through a link out of it); the policy
//! lets scripts, styles, images and fonts load from that origin alone and lets nothing reach the
//! network, unless the user allowed `network:<host>` (then `fetch` may reach `https://<host>`);
//! the frame may not leave its origin; and the page's one channel to Trek is `window.trek`,
//! whose messages carry a per-view secret only Trek's own script knows.

use super::{Manifest, Permission};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// The scheme a plugin's pages are served under. WebView2 can't load a custom scheme, so on
/// Windows wry serves `trek-plugin://<id>/` as `http://trek-plugin.<id>/` (`origin`).
pub const SCHEME: &str = "trek-plugin";
/// Where Trek's own fonts are served to a page, so `--trek-font` is the app's type.
pub const FONTS_PATH: &str = "/_trek/fonts/";
/// A message from the page: longer is refused unread (storage is the largest thing it sends).
pub const MAX_MESSAGE_BYTES: usize = super::STORAGE_LIMIT + 4096;
/// The longest text `trek.send` or `trek.toast` takes.
pub const MAX_TEXT_BYTES: usize = 8 * 1024;
/// The biggest file a page is served: a page's own files are small.
pub const MAX_FILE_BYTES: u64 = 32 << 20;

/// The origin plugin `id`'s pages are served from, as the view addresses it.
pub fn origin(id: &str, windows: bool) -> String {
    if windows {
        format!("http://{SCHEME}.{id}")
    } else {
        format!("{SCHEME}://{id}")
    }
}

/// The address of `entry` (a file in the plugin's folder) under `origin`.
pub fn url(origin: &str, entry: &str) -> String {
    format!("{origin}/{}", entry.trim_start_matches('/'))
}

/// The Content-Security-Policy a plugin's pages are served with: everything from the plugin's
/// own origin (inline scripts and styles too: it's the plugin's own page), `data:` and `blob:`
/// for what scripts build in memory, and no network at all but the hosts the user allowed.
pub fn csp(m: &Manifest) -> String {
    let hosts: Vec<String> = m
        .permissions
        .iter()
        .filter_map(|p| match p {
            Permission::Network(h) => Some(format!("https://{h}")),
            _ => None,
        })
        .collect();
    let connect = if hosts.is_empty() {
        "'self'".to_string()
    } else {
        format!("'self' {}", hosts.join(" "))
    };
    format!(
        "default-src 'none'; script-src 'self' 'unsafe-inline' blob:; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; media-src 'self' data: blob:; connect-src {connect}; worker-src 'self' blob:; child-src blob:; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'"
    )
}

/// `%XX` decoded; `None` for a sequence that isn't UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The file in plugin folder `dir` a request for `path` (a URL's path) gets: inside the folder
/// for real (no `..`, no link leading out of it), not hidden, and never the manifest's
/// neighbours Trek keeps. `None` gets a 404.
pub fn resolve(dir: &Path, path: &str) -> Option<PathBuf> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let rel = percent_decode(path.trim_start_matches('/'))?;
    if rel.is_empty()
        || rel.contains('\\')
        || rel.contains(':')
        || rel
            .split('/')
            .any(|p| p.is_empty() || p == ".." || p.starts_with('.'))
    {
        return None;
    }
    let file = dir.join(&rel);
    // A link in the folder may point anywhere: the file has to be inside it once followed.
    let (real, root) = (
        std::fs::canonicalize(&file).ok()?,
        std::fs::canonicalize(dir).ok()?,
    );
    (real.starts_with(&root) && real.is_file()).then_some(real)
}

/// The type a file is served as, by its extension.
pub fn mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("wasm") => "application/wasm",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Trek's fonts as the page may name them: `@font-face` rules over the files Trek serves at
/// `FONTS_PATH`, so `--trek-font` and `--trek-mono` are set in the app's own type.
pub fn fonts_css() -> String {
    let mut css = String::new();
    for (family, file) in [
        ("Inter", "Inter"),
        ("JetBrains Mono NL", "JetBrainsMonoNL"),
        ("Spectral", "Spectral"),
    ] {
        for (weight, name) in [
            (400, "Regular"),
            (500, "Medium"),
            (600, "SemiBold"),
            (700, "Bold"),
        ] {
            css.push_str(&format!(
                "@font-face{{font-family:\"{family}\";font-weight:{weight};src:url(\"{FONTS_PATH}{file}-{name}.ttf\")}}"
            ));
        }
        css.push_str(&format!(
            "@font-face{{font-family:\"{family}\";font-style:italic;font-weight:400;src:url(\"{FONTS_PATH}{file}-Italic.ttf\")}}"
        ));
    }
    css
}

/// The script Trek runs first in a plugin's page (before any of its own), with `nonce`, the
/// view's secret. It takes hold of the native message channel before the page can touch it and
/// gives the page `window.trek`:
///
/// - `trek.storage.get()` and `trek.storage.set(object)`: the plugin's own storage, kept by Trek;
/// - `trek.data(name)`: what the plugin's permissions allow (`threads`, `finished`, `usage`,
///   `commits`, `notes`, `project`);
/// - `trek.open(threadId)`, `trek.newThread()`, `trek.send(threadId, text)`, `trek.toast(text)`;
/// - `trek.theme` (`{dark, id}`) and `trek.on('theme', fn)`: the theme, when it changes (the
///   `--trek-*` CSS variables change by themselves).
///
/// Every call returns a promise Trek settles; one refused (no permission, too long) rejects with
/// the reason. The page's own scripts can't speak for it: a message without the secret is
/// dropped (`parse_request`).
pub fn bootstrap(nonce: &str, theme: &Value) -> String {
    BOOTSTRAP
        .replace("__NONCE__", nonce)
        .replace("__THEME__", &theme.to_string())
}

const BOOTSTRAP: &str = r#"(() => {
  if (window.__trekPage) return;
  Object.defineProperty(window, '__trekPage', { value: true });
  const N = '__NONCE__';
  const wk = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.ipc;
  const wv = window.chrome && window.chrome.webview;
  const raw = wk ? wk.postMessage.bind(wk) : wv ? wv.postMessage.bind(wv) : null;
  const str = JSON.stringify.bind(JSON);
  const make = Object.create, own = Object.keys;
  let next = 1;
  const waiting = new Map();
  const call = (t, extra) => new Promise((resolve, reject) => {
    if (!raw) { reject(new Error('Trek is not listening')); return; }
    const id = next++;
    const m = make(null);
    m.n = N; m.id = id; m.t = t;
    if (extra) { const ks = own(extra); for (let i = 0; i < ks.length; i++) m[ks[i]] = extra[ks[i]]; }
    waiting.set(id, [resolve, reject]);
    try { raw(str(m)); } catch (e) { waiting.delete(id); reject(e); }
  });
  const listeners = { theme: [] };
  let theme = __THEME__;
  // Trek answers and tells through this one function, by the secret: nothing else can call it
  // to settle a promise.
  Object.defineProperty(window, '__trekTell', { value: (n, kind, a, b) => {
    if (n !== N) return;
    if (kind === 'reply') {
      const w = waiting.get(a.id); if (!w) return; waiting.delete(a.id);
      if (a.error !== undefined) w[1](new Error(a.error)); else w[0](a.value);
    } else if (kind === 'theme') {
      theme = a;
      for (const f of listeners.theme.slice()) { try { f(a); } catch (_) {} }
    }
  }});
  const api = {
    storage: Object.freeze({
      get: () => call('storage.get'),
      set: (value) => call('storage.set', { value }),
    }),
    data: (name) => call('data', { name: String(name) }),
    open: (thread) => call('open', { thread: String(thread) }),
    newThread: () => call('new-thread'),
    send: (thread, text) => call('send', { thread: String(thread), text: String(text) }),
    toast: (text) => call('toast', { text: String(text) }),
    on: (event, f) => { if (listeners[event] && typeof f === 'function') listeners[event].push(f); },
  };
  Object.defineProperty(api, 'theme', { get: () => theme, enumerable: true });
  try { Object.defineProperty(window, 'trek', { value: Object.freeze(api), enumerable: true }); } catch (_) {}
  // Esc the page didn't use hands the keys back to Trek.
  window.addEventListener('keydown', (e) => {
    if (e.isTrusted && e.key === 'Escape' && !e.defaultPrevented) call('esc').catch(() => {});
  });
})();"#;

/// What a page asked of Trek.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    StorageGet,
    StorageSet(Map<String, Value>),
    /// Read-only data by name, behind the plugin's permissions.
    Data(String),
    Open(String),
    NewThread,
    Send {
        thread: String,
        text: String,
    },
    Toast(String),
    /// Esc the page didn't use.
    Escape,
}

impl Request {
    /// The permission a request needs, if any; `Data` is checked by its name (`data_permission`).
    pub fn needs(&self) -> Option<Permission> {
        match self {
            Request::Open(_) | Request::NewThread => Some(Permission::ThreadsOpen),
            Request::Send { .. } => Some(Permission::ThreadsSend),
            Request::Data(name) => data_permission(name),
            _ => None,
        }
    }
}

/// The data a page may ask for by name, and the permission each takes.
pub const DATA: [(&str, Permission); 6] = [
    ("threads", Permission::ThreadsRead),
    ("finished", Permission::ThreadsRead),
    ("usage", Permission::UsageRead),
    ("commits", Permission::GitRead),
    ("project", Permission::GitRead),
    ("notes", Permission::NotesRead),
];

/// The permission data `name` takes; `None` for a name Trek doesn't have.
pub fn data_permission(name: &str) -> Option<Permission> {
    DATA.iter()
        .find(|(n, _)| *n == name)
        .map(|(_, p)| p.clone())
}

/// `body`, a message from the page, if Trek's own script sent it with `nonce` and it asks for
/// something Trek knows, within bounds: its id (for the answer) and the request. Anything else
/// is `None`, unanswered.
pub fn parse_request(body: &str, nonce: &str) -> Option<(u64, Request)> {
    if body.len() > MAX_MESSAGE_BYTES {
        return None;
    }
    let v: Value = serde_json::from_str(body).ok()?;
    if v.get("n")?.as_str()? != nonce {
        return None;
    }
    let id = v.get("id")?.as_u64()?;
    let text = |key: &str, max: usize| -> Option<String> {
        let s = v.get(key)?.as_str()?;
        (s.len() <= max && !s.is_empty()).then(|| s.to_string())
    };
    let request = match v.get("t")?.as_str()? {
        "storage.get" => Request::StorageGet,
        "storage.set" => Request::StorageSet(v.get("value")?.as_object()?.clone()),
        "data" => Request::Data(text("name", 64)?),
        "open" => Request::Open(text("thread", 128)?),
        "new-thread" => Request::NewThread,
        "send" => Request::Send {
            thread: text("thread", 128)?,
            text: text("text", MAX_TEXT_BYTES)?,
        },
        "toast" => Request::Toast(text("text", 300)?),
        "esc" => Request::Escape,
        _ => return None,
    };
    Some((id, request))
}

/// A script that settles the page's promise `id` with `result`.
pub fn reply_script(nonce: &str, id: u64, result: &Result<Value, String>) -> String {
    let payload = match result {
        Ok(value) => json!({ "id": id, "value": value }),
        Err(error) => json!({ "id": id, "error": error }),
    };
    format!(
        "window.__trekTell&&window.__trekTell({},'reply',{payload});",
        Value::String(nonce.to_string())
    )
}

/// A script that tells the page the theme changed: its variables (`css`, `artifact::theme_css`
/// and the fonts) swapped in place, then `trek.on('theme')`'s listeners.
pub fn theme_script(nonce: &str, css: &str, theme: &Value) -> String {
    format!(
        "{}window.__trekTell&&window.__trekTell({},'theme',{theme});",
        crate::artifact::theme_update_script(css),
        Value::String(nonce.to_string())
    )
}

/// Whether the frame may go to `url`: only within the plugin's own origin, or to what scripts
/// make in memory.
pub fn navigation_allowed(url: &str, origin: &str) -> bool {
    crate::artifact::navigation_allowed(url, origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trek-page-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest(permissions: &str) -> Manifest {
        let plugin = scratch("manifest").join("pp");
        std::fs::create_dir_all(&plugin).unwrap();
        let m = super::super::parse(
            &format!("name = \"P\"\npermissions = [{permissions}]\n"),
            &plugin,
            "pp",
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(plugin.parent().unwrap());
        m
    }

    #[test]
    fn a_page_reaches_no_network_unless_a_host_was_allowed() {
        let quiet = csp(&manifest(""));
        assert!(quiet.contains("connect-src 'self';"), "{quiet}");
        assert!(!quiet.contains("https:"), "{quiet}");
        assert!(quiet.contains("default-src 'none'") && quiet.contains("form-action 'none'"));
        let weather = csp(&manifest("\"network:api.open-meteo.com\""));
        assert!(
            weather.contains("connect-src 'self' https://api.open-meteo.com;"),
            "{weather}"
        );
        // Scripts still come from the plugin alone: the host is for fetch.
        assert!(weather.contains("script-src 'self' 'unsafe-inline' blob:;"));
    }

    #[test]
    fn only_the_plugins_own_files_are_served() {
        let root = scratch("serve");
        let dir = root.join("plug");
        std::fs::create_dir_all(dir.join("pages/log")).unwrap();
        std::fs::write(dir.join("pages/log/index.html"), "<h1>x</h1>").unwrap();
        std::fs::write(dir.join("pages/log/my file.js"), "1").unwrap();
        std::fs::write(dir.join(".secret"), "s").unwrap();
        std::fs::write(root.join("outside.txt"), "o").unwrap();
        let ok = |p: &str| resolve(&dir, p).is_some();
        assert!(ok("/pages/log/index.html"));
        assert!(ok("/pages/log/index.html?v=2#top"));
        assert!(ok("/pages/log/my%20file.js"));
        assert!(!ok("/"));
        assert!(!ok("/pages/log/"));
        assert!(!ok("/../outside.txt"));
        assert!(!ok("/pages/%2e%2e/%2e%2e/outside.txt"));
        assert!(!ok("/.secret"));
        assert!(!ok("/pages/log/gone.js"));
        assert!(!ok("/C:/Windows/win.ini"));
        // A link out of the folder isn't followed out.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("outside.txt"), dir.join("pages/log/out.txt"))
                .unwrap();
            assert!(!ok("/pages/log/out.txt"));
        }
        assert_eq!(mime(Path::new("a/b.JS")), "text/javascript; charset=utf-8");
        assert_eq!(mime(Path::new("x.bin")), "application/octet-stream");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_message_needs_the_secret_and_a_known_request() {
        let ok = |body: &str| parse_request(body, "s3cret");
        assert_eq!(
            ok(r#"{"n":"s3cret","id":7,"t":"data","name":"threads"}"#),
            Some((7, Request::Data("threads".into())))
        );
        assert_eq!(
            ok(r#"{"n":"s3cret","id":1,"t":"storage.set","value":{"a":1}}"#).map(|r| r.0),
            Some(1)
        );
        assert_eq!(
            ok(r#"{"n":"wrong","id":7,"t":"data","name":"threads"}"#),
            None
        );
        assert_eq!(ok(r#"{"id":7,"t":"data","name":"threads"}"#), None);
        assert_eq!(ok(r#"{"n":"s3cret","id":7,"t":"shell","cmd":"rm"}"#), None);
        assert_eq!(
            ok(r#"{"n":"s3cret","id":7,"t":"storage.set","value":[1]}"#),
            None
        );
        assert_eq!(ok("not json"), None);
        let long = format!(
            r#"{{"n":"s3cret","id":7,"t":"send","thread":"t","text":"{}"}}"#,
            "x".repeat(MAX_TEXT_BYTES + 1)
        );
        assert_eq!(ok(&long), None);
        // What each needs.
        assert_eq!(
            Request::Data("commits".into()).needs(),
            Some(Permission::GitRead)
        );
        assert_eq!(Request::Data("passwords".into()).needs(), None);
        assert_eq!(Request::NewThread.needs(), Some(Permission::ThreadsOpen));
        assert_eq!(
            Request::Send {
                thread: "t".into(),
                text: "hi".into()
            }
            .needs(),
            Some(Permission::ThreadsSend)
        );
        assert_eq!(Request::StorageGet.needs(), None);
    }

    #[test]
    fn answers_and_origins_are_well_formed() {
        assert_eq!(origin("summit-log", false), "trek-plugin://summit-log");
        assert_eq!(origin("summit-log", true), "http://trek-plugin.summit-log");
        assert_eq!(
            url("trek-plugin://p", "pages/p/index.html"),
            "trek-plugin://p/pages/p/index.html"
        );
        let s = reply_script("n\"x", 3, &Ok(json!({"a": "</script>"})));
        assert!(s.contains(r#""n\"x""#) && s.contains("'reply'"), "{s}");
        let e = reply_script("n", 4, &Err("no permission".into()));
        assert!(e.contains(r#""error":"no permission""#), "{e}");
        assert!(navigation_allowed(
            "trek-plugin://p/pages/a.html#x",
            "trek-plugin://p"
        ));
        assert!(!navigation_allowed(
            "https://example.com/",
            "trek-plugin://p"
        ));
        let b = bootstrap("abc", &json!({"dark": true, "id": "trek"}));
        assert!(b.contains("const N = 'abc'") && b.contains(r#""dark":true"#));
        assert!(fonts_css().contains("/_trek/fonts/Inter-Medium.ttf"));
    }
}
