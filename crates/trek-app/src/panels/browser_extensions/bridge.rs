//! What Trek answers for an extension, and the programs it may talk to (macOS).
//!
//! **Trek's channel.** WebKit gives an extension's `runtime.sendNativeMessage` and
//! `runtime.connectNative` to the app (`WKWebExtensionControllerDelegate`), naming the program
//! the extension asked for. `trek-shim.js` (see `compat`) builds the Chrome APIs WebKit lacks on
//! messages to one name of Trek's own, [`APP`]: `chrome.notifications` (Trek's banners),
//! `chrome.offscreen` (a hidden page of the extension's), `chrome.identity.launchWebAuthFlow` (a
//! sign-in window), `chrome.idle`, `chrome.downloads` (to Downloads) and `chrome.sidePanel` (its page in
//! a window). Trek answers those itself: no program is started for them. A message is
//! `{method, args}`; the answer is `{value}` or `{error}`. Events Trek sends back (a banner
//! clicked) go down the one port each extension context opens to [`APP`] (`{event, args}`).
//!
//! **Native messaging.** Any other name is a native messaging host, as Chrome has them: a
//! program on the Mac that a password manager's desktop app installs, described by a manifest
//! (`<name>.json`: `path`, `type: "stdio"`, `allowed_origins`) in Chrome's or Chromium's
//! `NativeMessagingHosts` folders ([`host_dirs`]). Trek reads the manifest, checks the host lets
//! this extension in (`chrome-extension://<id>/`, Chrome's id, which is Trek's id for it), and
//! asks the user once per extension and host before starting it, because it is a program outside
//! Trek ("<extension> wants to start <program>"). Then it runs it as Chrome does: the
//! extension's origin as its argument, messages as UTF-8 JSON after a 4-byte length in the Mac's
//! byte order, both ways, over its standard input and output; `sendNativeMessage` starts it for
//! one answer, `connectNative` for as long as the port is open.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Trek's own name for the channel (`trek-shim.js` sends to it).
pub const APP: &str = "dev.trek.browser";

/// What a host's native message may be at most, from it (Chrome's limit) and to it.
pub const MAX_FROM_HOST: usize = 1024 * 1024;
pub const MAX_TO_HOST: usize = 64 * 1024 * 1024;

/// A call from the shim: `{method, args}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Call {
    pub method: String,
    #[serde(default)]
    pub args: Value,
}

pub fn ok(value: Value) -> Value {
    json!({ "value": value })
}

pub fn err(text: impl Into<String>) -> Value {
    json!({ "error": text.into() })
}

/// An event for an extension's pages: `{event, args}`.
pub fn event(name: &str, args: Vec<Value>) -> Value {
    json!({ "event": name, "args": args })
}

/// A notification an extension made (`chrome.notifications.create`), as Trek shows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Notice {
    /// The extension's id and its own id for the notification.
    pub ext: String,
    pub id: String,
    /// The extension's name, which the banner starts with.
    pub from: String,
    pub title: String,
    pub message: String,
    pub buttons: Vec<String>,
}

/// The tag Trek's banner carries, which a click on it brings back.
pub const TAG_PREFIX: &str = "trek-ext-notification:";

impl Notice {
    /// From the shim's `{id, options}` (`create`), or `options` laid over `self` (`update`).
    pub fn with_options(mut self, options: &Value) -> Self {
        let text = |key: &str| options[key].as_str().map(|s| s.trim().to_string());
        if let Some(t) = text("title") {
            self.title = t;
        }
        let mut message = text("message").unwrap_or_else(|| self.message.clone());
        if let Some(c) = text("contextMessage").filter(|c| !c.is_empty()) {
            message = format!("{message}\n{c}").trim().to_string();
        }
        if let Some(items) = options["items"].as_array().filter(|i| !i.is_empty()) {
            for i in items {
                let line = format!(
                    "{}: {}",
                    i["title"].as_str().unwrap_or(""),
                    i["message"].as_str().unwrap_or("")
                );
                message = format!("{message}\n{line}").trim().to_string();
            }
        }
        self.message = message;
        if let Some(buttons) = options["buttons"].as_array() {
            self.buttons = buttons
                .iter()
                .filter_map(|b| b["title"].as_str().map(str::to_string))
                .take(2)
                .collect();
        }
        self
    }

    /// The banner's tag: `trek-ext-notification:<extension id>:<its id>`.
    pub fn tag(&self) -> String {
        format!("{TAG_PREFIX}{}:{}", self.ext, self.id)
    }

    /// The headline: the extension's name, then its title (a banner names who posted it, and is
    /// never only Trek's: an extension with no name is named by its id).
    pub fn headline(&self) -> String {
        let from = match self.from.trim() {
            "" => format!("Extension {}", self.ext.chars().take(8).collect::<String>()),
            f => f.to_string(),
        };
        match self.title.trim() {
            "" => from,
            t => format!("{from}: {t}"),
        }
    }
}

/// The extension and notification a banner's tag names.
pub fn parse_tag(tag: &str) -> Option<(&str, &str)> {
    tag.strip_prefix(TAG_PREFIX)?.split_once(':')
}

/// A button's id on Trek's banner, and back.
pub fn button_id(index: usize) -> String {
    format!("button-{index}")
}

pub fn button_index(id: &str) -> Option<usize> {
    id.strip_prefix("button-")?.parse().ok()
}

/// The permission an extension must have asked for itself (its manifest, or granted when it
/// asked later) before Trek answers `method` for it, as Chrome gates these APIs.
pub fn required_permission(method: &str) -> Option<&'static str> {
    let api = method.split('.').next().unwrap_or("");
    match api {
        "notifications" => Some("notifications"),
        "downloads" => Some("downloads"),
        "identity" => Some("identity"),
        "sidePanel" => Some("sidePanel"),
        "offscreen" => Some("offscreen"),
        "idle" => Some("idle"),
        _ => None,
    }
}

/// The permissions extension's `manifest` asked for itself: its `permissions` without what Trek
/// added for its channel (`compat::ADDED_KEY`), and those granted later (`granted`).
pub fn own_permissions(manifest: &Value, granted: &[String]) -> Vec<String> {
    let list = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let added = list(&manifest[super::compat::ADDED_KEY]);
    let mut own: Vec<String> = list(&manifest["permissions"])
        .into_iter()
        .filter(|p| !added.contains(p))
        .collect();
    own.extend(granted.iter().cloned());
    own
}

/// How many banners an extension may post: `BURST` in a short while, `PER_MINUTE` in a minute.
pub struct RateLimit {
    times: std::collections::VecDeque<std::time::Instant>,
}

impl RateLimit {
    pub const BURST: usize = 3;
    pub const BURST_WINDOW: std::time::Duration = std::time::Duration::from_secs(10);
    pub const PER_MINUTE: usize = 10;

    pub fn new() -> Self {
        Self {
            times: Default::default(),
        }
    }

    /// Whether one more is allowed at `now` (and if so, it's counted).
    pub fn allow(&mut self, now: std::time::Instant) -> bool {
        let minute = std::time::Duration::from_secs(60);
        while self
            .times
            .front()
            .is_some_and(|t| now.duration_since(*t) > minute)
        {
            self.times.pop_front();
        }
        let recent = self
            .times
            .iter()
            .filter(|t| now.duration_since(**t) <= Self::BURST_WINDOW)
            .count();
        if recent >= Self::BURST || self.times.len() >= Self::PER_MINUTE {
            return false;
        }
        self.times.push_back(now);
        true
    }
}

impl Default for RateLimit {
    fn default() -> Self {
        Self::new()
    }
}

/// How many notifications one extension, and all of them, may keep at once.
pub const MAX_NOTICES_EACH: usize = 20;
pub const MAX_NOTICES: usize = 200;

/// What macOS records on a file from the internet (`com.apple.quarantine`), as browsers set it:
/// flags (downloaded, not yet opened), the time, the app that saved it, an id.
pub fn quarantine_value(now_secs: u64, agent: &str, id: &str) -> String {
    format!("0083;{now_secs:x};{agent};{id}")
}

/// The line a sign-in window shows above the page: the origin it's on, as Trek sees it (the
/// page can't draw it). `https://accounts.google.com`; a non-default port too.
pub fn origin_line(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.chars().take(80).collect();
    };
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Never the credentials part (`user:pass@`), which a page could use to look like a host.
    let host_port = host_port.rsplit('@').next().unwrap_or("");
    format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        host_port.to_ascii_lowercase()
    )
}

/// Whether `url` is one of extension `base`'s own pages (`webkit-extension://<id>/…`).
pub fn is_own_page(url: &str, base: &str) -> bool {
    !base.is_empty()
        && base.ends_with('/')
        && url.starts_with(base)
        && !url[base.len()..].contains("..")
}

/// What one extension may write to Trek's log in a run, at most.
pub const MAX_LOG_LINES: usize = 200;

/// A native messaging host's manifest.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Host {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub path: PathBuf,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// The manifest it came from.
    #[serde(skip)]
    pub manifest: PathBuf,
}

/// Where hosts' manifests are, as Chrome looks for them: per user first, then for the Mac (a
/// test profile only in its own data folder, so a test never starts a program of the user's).
pub fn host_dirs() -> Vec<PathBuf> {
    if trek_core::paths::isolated() {
        return vec![trek_core::paths::data_dir().join("NativeMessagingHosts")];
    }
    let home = trek_core::paths::home();
    if cfg!(target_os = "macos") {
        let user = home.join("Library/Application Support");
        vec![
            user.join("Google/Chrome/NativeMessagingHosts"),
            user.join("Chromium/NativeMessagingHosts"),
            PathBuf::from("/Library/Google/Chrome/NativeMessagingHosts"),
            PathBuf::from("/Library/Application Support/Chromium/NativeMessagingHosts"),
        ]
    } else {
        vec![
            home.join(".config/google-chrome/NativeMessagingHosts"),
            home.join(".config/chromium/NativeMessagingHosts"),
            PathBuf::from("/etc/opt/chrome/native-messaging-hosts"),
            PathBuf::from("/etc/chromium/native-messaging-hosts"),
        ]
    }
}

/// Whether `name` is a host name Chrome takes: dot-separated lowercase letters, digits and `_`
/// (and so no path in it).
pub fn valid_host_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

/// The origin Chrome gives extension `id`, which a host's `allowed_origins` names.
pub fn origin(id: &str) -> String {
    format!("chrome-extension://{id}/")
}

/// Host `name` from the first of `dirs` that has it, for extension `id`: the reason it can't be
/// used when it can't.
pub fn find_host(name: &str, id: &str, dirs: &[PathBuf]) -> Result<Host, String> {
    if !valid_host_name(name) {
        return Err(format!("“{name}” isn't a native messaging host's name."));
    }
    for dir in dirs {
        let path = dir.join(format!("{name}.json"));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut host: Host = serde_json::from_str(&text).map_err(|e| {
            format!(
                "{}: the host's manifest doesn't parse ({e}).",
                path.display()
            )
        })?;
        host.manifest = path;
        if host.name != name {
            return Err(format!(
                "{}: the host's manifest names another host ({}).",
                host.manifest.display(),
                host.name
            ));
        }
        if host.kind != "stdio" {
            return Err(format!("{name}: only “stdio” hosts can be started."));
        }
        if !host.path.is_absolute() {
            host.path = dir.join(&host.path);
        }
        if !host.allowed_origins.iter().any(|o| o == &origin(id)) {
            return Err(format!("{name} doesn't let this extension in."));
        }
        if !host.path.is_file() {
            return Err(format!(
                "{name}'s program isn't there ({}).",
                host.path.display()
            ));
        }
        return Ok(host);
    }
    Err(format!(
        "No program on {} answers to “{name}” (its app isn't installed, or doesn't offer it to Chrome).",
        crate::words::words().this_computer
    ))
}

/// What the user is asked before host `host` is first started for an extension.
pub fn ask_line(host: &Host) -> String {
    let program = host
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| host.name.clone());
    let what = if host.description.trim().is_empty() {
        program.clone()
    } else {
        format!("{} ({program})", host.description.trim())
    };
    format!(
        "Start {what}, a program on {} outside Trek, and exchange messages with it ({})",
        crate::words::words().this_computer,
        host.path.display()
    )
}

/// What the user's yes to `host` is kept as: its name and the program it resolved to, so a
/// manifest that later names another program asks again.
pub fn host_grant(host: &Host) -> String {
    let program = host
        .path
        .canonicalize()
        .unwrap_or_else(|_| host.path.clone());
    format!("{} {}", host.name, program.display())
}

/// One message to a host: its length in the Mac's byte order, then the JSON.
pub fn encode(message: &Value) -> Result<Vec<u8>, String> {
    let body = serde_json::to_vec(message).map_err(|e| e.to_string())?;
    if body.len() > MAX_TO_HOST {
        return Err("The message is too long for a native host.".into());
    }
    let mut out = (body.len() as u32).to_ne_bytes().to_vec();
    out.extend(body);
    Ok(out)
}

/// The next message from a host (`None`: it closed its output).
pub fn read_message(from: &mut impl Read) -> Result<Option<Value>, String> {
    let mut len = [0u8; 4];
    match from.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.to_string()),
    }
    let len = u32::from_ne_bytes(len) as usize;
    if len > MAX_FROM_HOST {
        return Err(format!("The host sent a message too long ({len} bytes)."));
    }
    let mut body = vec![0u8; len];
    from.read_exact(&mut body).map_err(|e| e.to_string())?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| format!("The host sent something that isn't JSON ({e})."))
}

/// A started host: what to write to it, and its output.
pub struct Running {
    pub child: std::process::Child,
    pub input: std::process::ChildStdin,
    pub output: std::process::ChildStdout,
}

/// Start `host` for extension `id`, as Chrome does (its folder as the working directory, the
/// origin as its argument).
pub fn start(host: &Host, id: &str) -> Result<Running, String> {
    let mut command = trek_core::procs::command(&host.path);
    command
        .arg(origin(id))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    if let Some(dir) = host.path.parent() {
        command.current_dir(dir);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("{} didn't start: {e}", host.name))?;
    let input = child.stdin.take().ok_or("No input to the host.")?;
    let output = child.stdout.take().ok_or("No output from the host.")?;
    Ok(Running {
        child,
        input,
        output,
    })
}

/// How long `sendNativeMessage` waits for its host's one answer.
pub const ONE_MESSAGE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// `sendNativeMessage`: start `host`, send `message`, wait (at most `wait`) for one answer, end
/// it: a host that doesn't answer in time is stopped.
pub fn one_message(
    host: &Host,
    id: &str,
    message: &Value,
    wait: std::time::Duration,
) -> Result<Value, String> {
    let mut run = start(host, id)?;
    let sent = encode(message).and_then(|b| run.input.write_all(&b).map_err(|e| e.to_string()));
    let answer = sent.and_then(|()| {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut output = run.output;
        std::thread::spawn(move || {
            let _ = tx.send(read_message(&mut output));
        });
        rx.recv_timeout(wait)
            .map_err(|_| format!("{} didn't answer in {} s.", host.name, wait.as_secs()))?
    });
    let _ = run.child.kill();
    let _ = run.child.wait();
    answer?.ok_or_else(|| format!("{} ended without answering.", host.name))
}

/// Where an extension's download goes: Downloads (a test profile's own folder), under its name
/// alone (never a path), numbered when taken.
pub fn download_path(dir: &Path, name: &str) -> PathBuf {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('.');
    let base = if base.is_empty() { "download" } else { base };
    let (stem, ext) = match base.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (base.to_string(), String::new()),
    };
    let mut path = dir.join(base);
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    path
}

pub fn downloads_dir() -> PathBuf {
    if trek_core::paths::isolated() {
        trek_core::paths::data_dir().join("downloads")
    } else {
        trek_core::paths::home().join("Downloads")
    }
}

/// The idle state `chrome.idle` reports: locked, idle past `threshold` seconds, or active.
pub fn idle_state(idle_seconds: f64, locked: bool, threshold: f64) -> &'static str {
    if locked {
        "locked"
    } else if idle_seconds >= threshold {
        "idle"
    } else {
        "active"
    }
}

/// Whether `url` is extension `id`'s web sign-in redirect (`identity.getRedirectURL`), where the
/// sign-in window stops and hands the address back.
pub fn is_auth_redirect(url: &str, id: &str) -> bool {
    let prefix = format!("https://{id}.chromiumapp.org/");
    url.starts_with(&prefix) || url == prefix.trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_reads_chrome_s_options() {
        let n = Notice {
            ext: "abc".into(),
            id: "n1".into(),
            from: "Bitwarden".into(),
            ..Default::default()
        }
        .with_options(&json!({
            "type": "basic", "title": "Vault locked", "message": "Unlock to fill.",
            "contextMessage": "bitwarden.com",
            "buttons": [{"title": "Unlock"}, {"title": "Later"}, {"title": "Third"}],
        }));
        assert_eq!(n.headline(), "Bitwarden: Vault locked");
        assert_eq!(n.message, "Unlock to fill.\nbitwarden.com");
        // Chrome shows two buttons at most.
        assert_eq!(n.buttons, vec!["Unlock", "Later"]);
        assert_eq!(n.tag(), "trek-ext-notification:abc:n1");
        assert_eq!(parse_tag(&n.tag()), Some(("abc", "n1")));
        // An id with a colon in it comes back whole.
        assert_eq!(
            parse_tag("trek-ext-notification:abc:a:b"),
            Some(("abc", "a:b"))
        );
        assert_eq!(parse_tag("other"), None);
        // `update` keeps what it doesn't change.
        let u = n.clone().with_options(&json!({ "title": "Unlocked" }));
        assert_eq!(u.title, "Unlocked");
        assert_eq!(u.buttons, n.buttons);
        assert_eq!(button_index(&button_id(1)), Some(1));
    }

    #[test]
    fn trek_answers_only_what_the_extension_asked_for() {
        let m = json!({
            "permissions": ["storage", "notifications", "nativeMessaging"],
            "trek_added_permissions": ["nativeMessaging"],
        });
        let own = own_permissions(&m, &["downloads".to_string()]);
        assert_eq!(own, vec!["storage", "notifications", "downloads"]);
        assert_eq!(
            required_permission("notifications.create"),
            Some("notifications")
        );
        assert_eq!(required_permission("sidePanel.open"), Some("sidePanel"));
        assert_eq!(
            required_permission("identity.launchWebAuthFlow"),
            Some("identity")
        );
        assert_eq!(required_permission("downloads.save"), Some("downloads"));
        assert_eq!(required_permission("log.error"), None);
        // A banner always names an extension, never Trek alone.
        let n = Notice {
            ext: "abcdefghijklmnop".into(),
            title: "Trek".into(),
            ..Default::default()
        };
        assert_eq!(n.headline(), "Extension abcdefgh: Trek");
    }

    #[test]
    fn banners_are_rate_limited() {
        let mut limit = RateLimit::new();
        let t0 = std::time::Instant::now();
        let at = |s: u64| t0 + std::time::Duration::from_secs(s);
        assert!((0..RateLimit::BURST).all(|_| limit.allow(at(0))));
        assert!(!limit.allow(at(1)));
        // Past the short window, more come, up to the minute's.
        let mut posted = RateLimit::BURST;
        for s in (11..60).step_by(11) {
            for _ in 0..RateLimit::BURST {
                if limit.allow(at(s)) {
                    posted += 1;
                }
            }
        }
        assert_eq!(posted, RateLimit::PER_MINUTE);
        assert!(limit.allow(at(200)));
    }

    #[test]
    fn origins_and_own_pages() {
        assert_eq!(
            origin_line("https://Accounts.Google.com/o/oauth2?x=1"),
            "https://accounts.google.com"
        );
        assert_eq!(
            origin_line("https://evil.example@accounts.google.com/"),
            "https://accounts.google.com"
        );
        assert_eq!(
            origin_line("http://127.0.0.1:8080/a"),
            "http://127.0.0.1:8080"
        );
        let base = "webkit-extension://abc/";
        assert!(is_own_page("webkit-extension://abc/panel.html", base));
        assert!(!is_own_page("file:///etc/passwd", base));
        assert!(!is_own_page("https://example.com/", base));
        assert!(!is_own_page("webkit-extension://abcd/x.html", base));
        assert!(!is_own_page("webkit-extension://abc/../x", base));
        assert_eq!(quarantine_value(255, "Trek", "u1"), "0083;ff;Trek;u1");
    }

    #[test]
    fn a_host_is_found_only_when_it_lets_the_extension_in() {
        let dir = std::env::temp_dir().join(format!("trek-nm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("host.sh");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        let write = |name: &str, body: Value| {
            std::fs::write(dir.join(format!("{name}.json")), body.to_string()).unwrap()
        };
        write(
            "com.example.pw",
            json!({"name": "com.example.pw", "description": "Example", "path": program,
                   "type": "stdio", "allowed_origins": ["chrome-extension://aaaa/"]}),
        );
        write(
            "com.example.relative",
            json!({"name": "com.example.relative", "path": "host.sh", "type": "stdio",
                   "allowed_origins": ["chrome-extension://aaaa/"]}),
        );
        let dirs = vec![dir.join("missing"), dir.clone()];
        let host = find_host("com.example.pw", "aaaa", &dirs).unwrap();
        assert_eq!(host.path, program);
        assert!(ask_line(&host).starts_with("Start Example (host.sh), a program on "));
        assert_eq!(
            find_host("com.example.relative", "aaaa", &dirs)
                .unwrap()
                .path,
            program
        );
        assert!(
            find_host("com.example.pw", "bbbb", &dirs)
                .unwrap_err()
                .contains("doesn't let this extension in")
        );
        assert!(
            find_host("com.example.none", "aaaa", &dirs)
                .unwrap_err()
                .contains("No program")
        );
        // A name is never a path.
        assert!(find_host("../evil", "aaaa", &dirs).is_err());
        assert!(!valid_host_name("Com.Example"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn messages_are_framed_as_chrome_frames_them() {
        let bytes = encode(&json!({"a": 1})).unwrap();
        assert_eq!(&bytes[..4], &7u32.to_ne_bytes());
        let mut two = bytes.clone();
        two.extend(encode(&json!("x")).unwrap());
        let mut reader = std::io::Cursor::new(two);
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({"a": 1})));
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!("x")));
        assert_eq!(read_message(&mut reader).unwrap(), None);
        let mut huge = std::io::Cursor::new(((MAX_FROM_HOST + 1) as u32).to_ne_bytes().to_vec());
        assert!(read_message(&mut huge).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_host_answers_one_message() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("trek-nm-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Echoes the first message back: `cat` writes the frame it reads.
        let program = dir.join("echo.sh");
        std::fs::write(&program, "#!/bin/sh\nexec cat\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let host = Host {
            name: "com.example.echo".into(),
            description: String::new(),
            path: program,
            kind: "stdio".into(),
            allowed_origins: vec![origin("aaaa")],
            manifest: PathBuf::new(),
        };
        assert_eq!(
            one_message(&host, "aaaa", &json!({"ping": true}), ONE_MESSAGE_WAIT).unwrap(),
            json!({"ping": true})
        );
        // One that never answers is stopped after the wait.
        let silent = dir.join("silent.sh");
        std::fs::write(&silent, "#!/bin/sh\nexec sleep 60\n").unwrap();
        std::fs::set_permissions(&silent, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = std::time::Instant::now();
        let quiet = Host {
            path: silent,
            ..host.clone()
        };
        let e = one_message(
            &quiet,
            "aaaa",
            &json!(1),
            std::time::Duration::from_millis(300),
        )
        .unwrap_err();
        assert!(e.contains("didn't answer"), "{e}");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        // A grant names the program, so another program in the manifest asks again.
        assert!(host_grant(&host).starts_with("com.example.echo /"));
        assert_ne!(host_grant(&host), host_grant(&quiet));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn downloads_never_leave_their_folder() {
        let dir = std::env::temp_dir().join(format!("trek-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(download_path(&dir, "../../etc/passwd"), dir.join("passwd"));
        assert_eq!(download_path(&dir, ".hidden"), dir.join("hidden"));
        assert_eq!(download_path(&dir, ""), dir.join("download"));
        std::fs::write(dir.join("vault.json"), "").unwrap();
        assert_eq!(
            download_path(&dir, "vault.json"),
            dir.join("vault (1).json")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn idle_and_redirects() {
        assert_eq!(idle_state(5., false, 60.), "active");
        assert_eq!(idle_state(61., false, 60.), "idle");
        assert_eq!(idle_state(0., true, 60.), "locked");
        assert!(is_auth_redirect(
            "https://abc.chromiumapp.org/cb?code=1",
            "abc"
        ));
        assert!(!is_auth_redirect(
            "https://abc.chromiumapp.org.evil.com/",
            "abc"
        ));
        assert!(!is_auth_redirect("https://xyz.chromiumapp.org/", "abc"));
    }
}
