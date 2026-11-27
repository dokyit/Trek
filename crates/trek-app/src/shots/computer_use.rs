//! Computer use, live (`computer open|elements|click|type|press|key|shot|close`): another app
//! opened for the check, driven and read through Trek's own computer tools as an agent drives
//! and reads it (`trek-mcp computer`: `list_windows` finds its window, `elements` reads its
//! outline with refs, and `click`, `type_text`, `press`, `key` and `screenshot` act on it). One
//! `trek-mcp` session serves the whole check, so the refs `elements` hands out are the ones the
//! other verbs name. On CI's machines alone (`live_ci` in the ledger): on someone's own computer,
//! Notepad opens as a tab of theirs.

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::Mutex;
use std::time::Duration;

/// The app opened for the check.
static OPENED: Mutex<Option<Child>> = Mutex::new(None);
/// The `trek-mcp computer` session the verbs share: its refs are per session.
static SESSION: Mutex<Option<Session>> = Mutex::new(None);
/// The window `elements` last found, for verbs that name an element rather than a ref.
static WINDOW: Mutex<Option<String>> = Mutex::new(None);

/// The longest the computer tools have to answer, all told.
const ANSWER_WITHIN: Duration = Duration::from_secs(45);

/// Open `app` (a program: `notepad.exe`, or a path on Linux) for the check, kept to close after.
pub fn open(app: &str) -> Result<()> {
    let child = trek_core::procs::command(app)
        .spawn()
        .with_context(|| format!("computer open {app}"))?;
    *OPENED.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
    Ok(())
}

/// Close the app `open` opened, and the `trek-mcp` session with it.
pub fn close() {
    if let Some(mut child) = OPENED.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    drop(SESSION.lock().unwrap_or_else(|e| e.into_inner()).take());
    *WINDOW.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Where Trek's own MCP server is: `TREK_MCP_BIN` (CI builds it beside the capture build), else
/// beside Trek, as a release has it.
fn trek_mcp() -> Result<PathBuf> {
    std::env::var_os("TREK_MCP_BIN")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(crate::workspace::trek_mcp_binary)
        .context("computer: no trek-mcp (TREK_MCP_BIN, or beside Trek)")
}

/// A `trek-mcp computer` over stdio, one request at a time.
struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
    /// Held for the session's life: dropped with it, which ends the watchdog before it can fire.
    _watchdog: std::sync::mpsc::Sender<()>,
}

impl Session {
    /// Start `trek-mcp computer` and say hello, as an MCP client does.
    fn connect() -> Result<Self> {
        let mut child = trek_core::procs::command(trek_mcp()?)
            .arg("computer")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("computer: trek-mcp computer")?;
        // A server that stops answering is ended, so the check fails instead of waiting for ever
        // (and only then: done, the watchdog goes without touching what may by then be another
        // process).
        let pid = child.id();
        let (done, ended) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                ended.recv_timeout(ANSWER_WITHIN)
            {
                #[cfg(windows)]
                let _ = trek_core::procs::command("taskkill")
                    .args(["/F", "/PID", &pid.to_string()])
                    .output();
                #[cfg(not(windows))]
                let _ = trek_core::procs::command("kill")
                    .args(["-9", &pid.to_string()])
                    .output();
            }
        });
        // The watchdog's `done` goes when the session does; held here it would kill a later
        // process only if this one never ended, which is what it's for.
        let stdin = child.stdin.take().context("trek-mcp's stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("trek-mcp's stdout")?);
        let mut session = Session {
            child,
            stdin,
            stdout,
            next: 0,
            _watchdog: done,
        };
        session.call_raw(
            "initialize",
            json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "trek-live", "version": "1"}}),
        )?;
        Ok(session)
    }

    /// One JSON-RPC call, answered with its `result`.
    fn call_raw(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next += 1;
        let id = self.next;
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )?;
        self.stdin.flush()?;
        let mut line = String::new();
        loop {
            line.clear();
            if self.stdout.read_line(&mut line)? == 0 {
                bail!("trek-mcp stopped answering ({method})");
            }
            let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if msg["id"] != json!(id) {
                continue;
            }
            if let Some(e) = msg.get("error") {
                bail!("{method}: {e}");
            }
            return Ok(msg["result"].clone());
        }
    }

    /// A tool call's text, or its error.
    fn call(&mut self, tool: &str, args: Value) -> Result<String> {
        let result = self.call_raw("tools/call", json!({"name": tool, "arguments": args}))?;
        let text = result["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if result["isError"].as_bool() == Some(true) {
            bail!("{tool}: {text}");
        }
        Ok(text)
    }

    /// A tool call's whole result, for what isn't text (a screenshot's image).
    fn call_result(&mut self, tool: &str, args: Value) -> Result<Value> {
        self.call_raw("tools/call", json!({"name": tool, "arguments": args}))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The session, started on first use; `None`-checked out of the lock only for the call.
fn with_session<R>(f: impl FnOnce(&mut Session) -> Result<R>) -> Result<R> {
    let mut guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(Session::connect()?);
    }
    f(guard.as_mut().expect("session just made"))
}

/// The window `window` names (its app's or title's words), found in `list_windows` for up to
/// twenty seconds, as the app may still be opening; its id kept for `elements`.
fn window_id(session: &mut Session, window: &str) -> Result<u64> {
    let started = std::time::Instant::now();
    loop {
        let listed = session.call("list_windows", json!({}))?;
        let found = listed.lines().skip(1).find_map(|row| {
            let w: Value = serde_json::from_str(row).ok()?;
            let named = |k: &str| w[k].as_str().is_some_and(|s| s.contains(window));
            (named("app") || named("title")).then(|| w["window_id"].as_u64())?
        });
        if let Some(id) = found {
            return Ok(id);
        }
        if started.elapsed() > Duration::from_secs(20) {
            bail!("computer: no window of {window} in list_windows:\n{listed}");
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The outline of `window`'s elements, as `elements` gives an agent (with the menu bar): the
/// window is found first, as the app may still be opening.
pub fn elements(window: &str) -> Result<String> {
    with_session(|session| {
        let id = window_id(session, window)?;
        *WINDOW.lock().unwrap_or_else(|e| e.into_inner()) = Some(window.to_string());
        session.call("elements", json!({"window_id": id, "menu_bar": true}))
    })
}

/// The ref an outline line gives `target`: `e12` as written, `"a name"` the element whose name
/// holds it, `field` the first text field. Read from `outline`, the window `elements` last read.
fn ref_in(outline: &str, target: &str) -> Result<String> {
    let target = target.trim();
    if target
        .strip_prefix('e')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    {
        return Ok(target.to_string());
    }
    let want = target.trim_matches('"').to_lowercase();
    let needle = if want == "field" {
        None
    } else {
        Some(want.as_str())
    };
    for line in outline.lines() {
        let is_field = line.contains("text field") || line.contains("search field");
        let named = needle.is_some_and(|n| line.to_lowercase().contains(n));
        if (needle.is_none() && is_field) || named {
            if let Some(rest) = line.split("[ref=").nth(1)
                && let Some(r) = rest.split(']').next()
            {
                return Ok(r.to_string());
            }
        }
    }
    bail!("computer: no element {target:?} with a ref in the outline:\n{outline}")
}

/// `target` as a ref: `e12` itself, else resolved through the window `elements` last read.
fn resolve(target: &str) -> Result<String> {
    if target
        .trim()
        .strip_prefix('e')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    {
        return Ok(target.trim().to_string());
    }
    let window = WINDOW
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .context("computer: name an element only after `computer elements <window>` has read it")?;
    let outline = elements(&window)?;
    ref_in(&outline, target)
}

/// `computer click <target>`: the element `target` names (`e12`, `"a name"`, `field`), clicked
/// by its ref — the pointer moves to its centre and clicks, as a person's would; or
/// `computer click <x> <y>` in screenshot pixels.
pub fn click(arg: &str) -> Result<()> {
    let arg = arg.trim();
    let mut parts = arg.split_whitespace();
    let at = match (parts.next(), parts.next(), parts.next()) {
        (Some(x), Some(y), None) => match (x.parse::<f64>(), y.parse::<f64>()) {
            (Ok(x), Ok(y)) => Some((x, y)),
            _ => None,
        },
        _ => None,
    };
    if let Some((x, y)) = at {
        return with_session(|session| {
            session.call("click", json!({"x": x, "y": y}))?;
            Ok(())
        });
    }
    let r = resolve(arg)?;
    with_session(|session| {
        session.call("click", json!({"ref": r}))?;
        Ok(())
    })
}

/// `computer type <text>` into the focused element, or `computer type <text> into <target>`
/// into the element `target` names (focused first, as a click in it would).
pub fn type_text(arg: &str) -> Result<()> {
    let (text, target) = match arg.rsplit_once(" into ") {
        Some((text, target)) => (text.trim(), Some(target.trim())),
        None => (arg.trim(), None),
    };
    if text.is_empty() {
        bail!("computer type needs text");
    }
    let mut args = json!({"text": text});
    if let Some(target) = target {
        args["ref"] = json!(resolve(target)?);
    }
    with_session(|session| {
        session.call("type_text", args)?;
        Ok(())
    })
}

/// `computer press <target> [action]`: the element's own action by ref (no pointer or key).
pub fn press(arg: &str) -> Result<()> {
    let (target, action) = match arg.split_once(' ') {
        Some((t, a)) => (t.trim(), Some(a.trim())),
        None => (arg.trim(), None),
    };
    let r = resolve(target)?;
    let mut args = json!({"ref": r});
    if let Some(action) = action {
        args["action"] = json!(action);
    }
    with_session(|session| {
        session.call("press", args)?;
        Ok(())
    })
}

/// `computer key <combo>`: a key or shortcut (`return`, `ctrl+a`) as the keyboard sends it.
pub fn key(combo: &str) -> Result<()> {
    with_session(|session| {
        session.call("key", json!({"combo": combo.trim()}))?;
        Ok(())
    })
}

/// `computer shot <name>`: the screen as `screenshot` gives it, saved as `<name>.png` in the
/// shot folder — the check's proof a capture came back, and an artifact to look at.
pub fn shot(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.contains(['/', '\\']) {
        bail!("computer shot needs a plain file name");
    }
    let result = with_session(|session| session.call_result("screenshot", json!({})))?;
    let image = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["type"] == "image")
        .and_then(|c| c["data"].as_str())
        .context("computer shot: the screenshot had no image")?;
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(image)
        .context("computer shot: the screenshot's image isn't base64")?;
    anyhow::ensure!(
        png.starts_with(b"\x89PNG\r\n\x1a\n") && png.len() > 256,
        "computer shot: the screenshot is {} bytes and not a real PNG",
        png.len()
    );
    let dir = std::env::var_os("TREK_SHOT_DIR")
        .map(PathBuf::from)
        .context("computer shot: no TREK_SHOT_DIR")?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.png", name.trim()));
    std::fs::write(&path, &png)?;
    tracing::info!("computer shot: {}", path.display());
    Ok(())
}
