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

    /// The headline: the extension's name, then its title (a banner names who posted it).
    pub fn headline(&self) -> String {
        match (self.from.trim(), self.title.trim()) {
            ("", t) => t.to_string(),
            (f, "") => f.to_string(),
            (f, t) => format!("{f}: {t}"),
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

/// `sendNativeMessage`: start `host`, send `message`, wait for one answer, end it.
pub fn one_message(host: &Host, id: &str, message: &Value) -> Result<Value, String> {
    let mut run = start(host, id)?;
    let sent = encode(message).and_then(|b| run.input.write_all(&b).map_err(|e| e.to_string()));
    let answer = sent.and_then(|()| read_message(&mut run.output));
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
            one_message(&host, "aaaa", &json!({"ping": true})).unwrap(),
            json!({"ping": true})
        );
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
