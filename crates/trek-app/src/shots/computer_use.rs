//! Computer use by element, live (`computer open|elements|close`): another app opened for the
//! check, read through Trek's own computer tools as an agent reads it (`trek-mcp computer`: its
//! window found in `list_windows`, then its outline with refs from `elements`), and closed again.
//! On CI's machines alone (`live_ci` in the ledger): on someone's own computer, Notepad opens as a
//! tab of theirs.

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

/// The app opened for the check.
static OPENED: Mutex<Option<Child>> = Mutex::new(None);

/// The longest the computer tools have to answer, all told.
const ANSWER_WITHIN: Duration = Duration::from_secs(45);

/// Open `app` (a program: `notepad.exe`) for the check, kept to close after.
pub fn open(app: &str) -> Result<()> {
    let child = Command::new(app)
        .spawn()
        .with_context(|| format!("computer open {app}"))?;
    *OPENED.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
    Ok(())
}

/// Close the app `open` opened.
pub fn close() {
    if let Some(mut child) = OPENED.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = child.kill();
        let _ = child.wait();
    }
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

/// The outline of the window whose app or title holds `window`, as `elements` gives an agent
/// (with the menu bar): looked for in `list_windows` for up to `ANSWER_WITHIN`, as the app may
/// still be opening.
pub fn elements(window: &str) -> Result<String> {
    let mut server = Command::new(trek_mcp()?)
        .arg("computer")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("computer elements: trek-mcp computer")?;
    // A server that stops answering is ended, so the check fails instead of waiting for ever (and
    // only then: done, the watchdog goes without touching what may by then be another process).
    let pid = server.id();
    let (done, ended) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = ended.recv_timeout(ANSWER_WITHIN) {
            #[cfg(windows)]
            let _ = Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output();
            #[cfg(not(windows))]
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
        }
    });
    let result = talk(&mut server, window);
    drop(done);
    let _ = server.kill();
    let _ = server.wait();
    result
}

fn talk(server: &mut Child, window: &str) -> Result<String> {
    let mut stdin = server.stdin.take().context("trek-mcp's stdin")?;
    let mut stdout = BufReader::new(server.stdout.take().context("trek-mcp's stdout")?);
    let mut next = 0u64;
    let mut ask = |method: &str, params: Value| -> Result<Value> {
        next += 1;
        let id = next;
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )?;
        stdin.flush()?;
        let mut line = String::new();
        loop {
            line.clear();
            if stdout.read_line(&mut line)? == 0 {
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
    };
    ask(
        "initialize",
        json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "trek-live", "version": "1"}}),
    )?;
    let call = |ask: &mut dyn FnMut(&str, Value) -> Result<Value>, tool: &str, args: Value| {
        let result = ask("tools/call", json!({"name": tool, "arguments": args}))?;
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
    };
    let started = std::time::Instant::now();
    let id = loop {
        let listed = call(&mut ask, "list_windows", json!({}))?;
        let found = listed.lines().skip(1).find_map(|row| {
            let w: Value = serde_json::from_str(row).ok()?;
            let named = |k: &str| w[k].as_str().is_some_and(|s| s.contains(window));
            (named("app") || named("title")).then(|| w["window_id"].as_u64())?
        });
        if let Some(id) = found {
            break id;
        }
        if started.elapsed() > Duration::from_secs(20) {
            bail!("computer elements: no window of {window} in list_windows:\n{listed}");
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    call(
        &mut ask,
        "elements",
        json!({"window_id": id, "menu_bar": true}),
    )
}
