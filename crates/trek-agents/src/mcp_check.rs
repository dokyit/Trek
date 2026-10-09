//! Checking that an MCP server works, the way an agent would start talking to it: `initialize`,
//! `notifications/initialized`, then `tools/list`. A stdio server is started for the check and
//! ended after it; a remote one is sent the same over streamable HTTP.

use crate::{McpServer, McpTransport, ProtocolLines, StderrTail};
use serde_json::{Value, json};
use std::time::Duration;

/// How long a command gets: `npx`/`uvx` may be fetching the package on a first run.
pub const STDIO_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a remote server gets for the whole exchange.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

const PROTOCOL_VERSION: &str = "2025-06-18";
/// The most pages of tools read (a server that keeps handing out cursors isn't followed forever).
const MAX_PAGES: usize = 20;

/// The names of the tools `server` offers, or why it couldn't be asked, in words for Settings.
pub async fn list_tools(server: &McpServer) -> Result<Vec<String>, String> {
    match &server.transport {
        McpTransport::Stdio { command, args, env } => stdio(command, args, env, STDIO_TIMEOUT).await,
        McpTransport::Http { url, headers } => http(url, headers).await,
    }
}

fn initialize() -> Value {
    json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "clientInfo": { "name": "Trek", "version": env!("CARGO_PKG_VERSION") } })
}

/// A JSON-RPC error's message, as the server put it.
fn said(error: &Value) -> String {
    let message = error["message"].as_str().unwrap_or("an error").trim();
    format!("It said: {message}")
}

/// Tool names out of one `tools/list` result, and the cursor of the next page.
fn page(result: &Value) -> (Vec<String>, Option<String>) {
    let names = result["tools"].as_array().map(|t| t.iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    (names, result["nextCursor"].as_str().filter(|c| !c.is_empty()).map(str::to_string))
}

async fn stdio(command: &str, args: &[String], env: &[(String, String)], limit: Duration) -> Result<Vec<String>, String> {
    use std::process::Stdio;
    let mut cmd = tokio::process::Command::new(command);
    // The login shell's PATH (where npx and uvx are), unless the server sets its own.
    cmd.args(args).env("PATH", trek_core::detect::login_path()).envs(env.iter().map(|(k, v)| (k, v)));
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = crate::spawn_group(&mut cmd).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("{command} isn't installed, or isn't on your PATH."),
        std::io::ErrorKind::PermissionDenied => format!("{command} can't be run (permission denied)."),
        _ => format!("{command} couldn't start: {e}"),
    })?;
    let tail = StderrTail::capture(child.stderr.take().expect("piped"), "mcp-check");
    let (stdin, stdout) = (child.stdin.take().expect("piped"), child.stdout.take().expect("piped"));
    let talk = tokio::time::timeout(limit, stdio_talk(stdin, stdout)).await;
    // Whatever the outcome, the server and anything it started are ended.
    let exit = child.try_wait().ok().flatten();
    child.terminate().await;
    // Its last words may still be on their way through the pipe.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let hint = useful_line(&tail.lines());
    match talk {
        Ok(Ok(tools)) => Ok(tools),
        Ok(Err(Talk::Said(message))) => Err(message),
        Ok(Err(Talk::Ended)) => Err(match (hint, exit.and_then(|s| s.code())) {
            (Some(line), _) => format!("It stopped: {line}"),
            (None, Some(code)) => format!("It stopped before answering (exit code {code})."),
            (None, None) => "It stopped before answering.".into(),
        }),
        Err(_) => Err(match hint {
            Some(line) => format!("No answer within {} seconds. Last it said: {line}", limit.as_secs()),
            None => format!("No answer within {} seconds.", limit.as_secs()),
        }),
    }
}

enum Talk {
    /// Its output ended (it exited).
    Ended,
    /// It answered with an error.
    Said(String),
}

async fn stdio_talk(mut stdin: tokio::process::ChildStdin, stdout: tokio::process::ChildStdout) -> Result<Vec<String>, Talk> {
    use tokio::io::AsyncWriteExt as _;
    let mut lines = ProtocolLines::new(tokio::io::BufReader::new(stdout));
    let send = async |stdin: &mut tokio::process::ChildStdin, msg: Value| {
        let mut line = msg.to_string();
        line.push('\n');
        stdin.write_all(line.as_bytes()).await.map_err(|_| Talk::Ended)?;
        stdin.flush().await.map_err(|_| Talk::Ended)
    };
    // The reply to request `id`. What else it says on the way is passed over (logs, progress),
    // and a request of its own is answered so it doesn't wait on Trek.
    async fn reply(lines: &mut ProtocolLines<tokio::io::BufReader<tokio::process::ChildStdout>>, stdin: &mut tokio::process::ChildStdin, id: u64) -> Result<Value, Talk> {
        use tokio::io::AsyncWriteExt as _;
        loop {
            let line = lines.next_line().await.map_err(|_| Talk::Ended)?.ok_or(Talk::Ended)?;
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            if msg["id"] == json!(id) && msg.get("method").is_none() {
                if let Some(error) = msg.get("error") {
                    return Err(Talk::Said(said(error)));
                }
                return Ok(msg["result"].clone());
            }
            if let (Some(method), Some(their)) = (msg["method"].as_str(), msg.get("id")) {
                let answer = match method {
                    "ping" => json!({ "jsonrpc": "2.0", "id": their, "result": {} }),
                    "roots/list" => json!({ "jsonrpc": "2.0", "id": their, "result": { "roots": [] } }),
                    _ => json!({ "jsonrpc": "2.0", "id": their, "error": { "code": -32601, "message": "Not supported while checking" } }),
                };
                let _ = stdin.write_all(format!("{answer}\n").as_bytes()).await;
            }
        }
    }
    send(&mut stdin, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize() })).await?;
    reply(&mut lines, &mut stdin, 1).await?;
    send(&mut stdin, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await?;
    let mut tools = vec![];
    let mut cursor: Option<String> = None;
    for id in 2..2 + MAX_PAGES as u64 {
        let params = match &cursor {
            Some(c) => json!({ "cursor": c }),
            None => json!({}),
        };
        send(&mut stdin, json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list", "params": params })).await?;
        let (names, next) = page(&reply(&mut lines, &mut stdin, id).await?);
        tools.extend(names);
        cursor = next;
        if cursor.is_none() {
            break;
        }
    }
    Ok(tools)
}

/// The stderr line that best says what went wrong: one that says "error", or else one that
/// reads like a problem, or else the last thing it said. npm's chatter is passed over.
fn useful_line(lines: &[String]) -> Option<String> {
    let lines: Vec<&str> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with("npm warn") && !l.starts_with("npm notice")).collect();
    let has = |l: &str, words: &[&str]| words.iter().any(|w| l.to_ascii_lowercase().contains(w));
    let line = lines
        .iter()
        .find(|l| has(l, &["error"]))
        .or_else(|| lines.iter().find(|l| has(l, &["not found", "required", "missing", "invalid", "denied", "cannot", "can't", "unable", "failed", "enoent", "e404"])))
        .or(lines.last())?;
    Some(crate::clip(line, 200))
}

async fn http(url: &str, headers: &[(String, String)]) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder().timeout(HTTP_TIMEOUT).build().map_err(|e| e.to_string())?;
    let talk = async {
        let (init, session) = post(&client, url, headers, None, false, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize() })).await?;
        init.ok_or("It didn't answer initialize.")?;
        post(&client, url, headers, session.as_deref(), true, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await?;
        let mut tools = vec![];
        let mut cursor: Option<String> = None;
        for id in 2..2 + MAX_PAGES as u64 {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let (result, _) = post(&client, url, headers, session.as_deref(), true, json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list", "params": params })).await?;
            let (names, next) = page(&result.ok_or("It didn't answer tools/list.")?);
            tools.extend(names);
            cursor = next;
            if cursor.is_none() {
                break;
            }
        }
        // Done with the session: let the server drop it.
        if let Some(sid) = &session {
            let mut req = client.delete(url).header("Mcp-Session-Id", sid);
            for (k, v) in headers {
                req = req.header(k, v);
            }
            let _ = req.send().await;
        }
        Ok::<_, String>(tools)
    };
    tokio::time::timeout(HTTP_TIMEOUT, talk).await.unwrap_or_else(|_| Err(format!("No answer within {} seconds.", HTTP_TIMEOUT.as_secs())))
}

/// POST one JSON-RPC message; its reply (`None` for a notification) and the session id the
/// server gave. The reply comes as JSON or as an event stream with it in a `data:` line.
async fn post(client: &reqwest::Client, url: &str, headers: &[(String, String)], session: Option<&str>, initialized: bool, msg: Value) -> Result<(Option<Value>, Option<String>), String> {
    let mut req = client.post(url).header("Accept", "application/json, text/event-stream").header("Content-Type", "application/json");
    if initialized {
        req = req.header("MCP-Protocol-Version", PROTOCOL_VERSION);
    }
    if let Some(sid) = session {
        req = req.header("Mcp-Session-Id", sid);
    }
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let mut resp = req.body(msg.to_string()).send().await.map_err(|e| {
        if e.is_timeout() {
            format!("No answer within {} seconds.", HTTP_TIMEOUT.as_secs())
        } else if e.is_connect() {
            "Couldn't reach it. Is it running, and is the address right?".to_string()
        } else {
            format!("Couldn't reach it: {e}")
        }
    })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => "Needs sign-in or a token (401).".to_string(),
            403 => "Turned away (403). Check its token.".to_string(),
            404 => "Nothing at that address (404).".to_string(),
            405 => "That address doesn't take MCP requests (405).".to_string(),
            code => format!("The server answered {code} {}.", status.canonical_reason().unwrap_or_default()).replace(" .", "."),
        });
    }
    let sid = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(str::to_string);
    let Some(id) = msg.get("id").cloned() else { return Ok((None, sid)) };
    let sse = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("text/event-stream"));
    let answer = |v: Value| -> Option<Result<Value, String>> {
        (v["id"] == id && v.get("method").is_none()).then(|| match v.get("error") {
            Some(e) => Err(said(e)),
            None => Ok(v["result"].clone()),
        })
    };
    if !sse {
        let body = resp.bytes().await.map_err(|e| format!("Its answer was cut off: {e}"))?;
        let v: Value = serde_json::from_slice(&body).map_err(|_| "Its answer isn't MCP (not JSON).".to_string())?;
        return answer(v).unwrap_or(Err("It answered something else.".into())).map(|r| (Some(r), sid));
    }
    // An event stream: read it until our reply's event comes, then stop (it may stay open).
    let mut buf = String::new();
    let mut data = String::new();
    loop {
        let Some(chunk) = resp.chunk().await.map_err(|e| format!("Its answer was cut off: {e}"))? else {
            return Err("It closed the stream without answering.".into());
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buf.find('\n') {
            let line: String = buf.drain(..=end).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            if let Some(d) = line.strip_prefix("data:") {
                data.push_str(d.strip_prefix(' ').unwrap_or(d));
            } else if line.is_empty()
                && !data.is_empty()
                && let Some(found) = serde_json::from_str::<Value>(&std::mem::take(&mut data)).ok().and_then(answer)
            {
                return found.map(|r| (Some(r), sid));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    fn fake(mode: &str, env: Vec<(String, String)>) -> McpServer {
        McpServer::stdio("fake", trek_test_fixtures::bin("fake-mcp").display().to_string(), vec![mode.into()], env)
    }

    #[tokio::test]
    async fn a_stdio_server_lists_its_tools_with_its_environment() {
        // Past a log line on stdout, a ping of its own and a notification, over two pages.
        let tools = list_tools(&fake("ok", vec![("FAKE_TOKEN".into(), "abc".into())])).await.unwrap();
        assert_eq!(tools, ["echo", "add", "get_time", "env_ok"]);
        let tools = list_tools(&fake("ok", vec![])).await.unwrap();
        assert!(!tools.contains(&"env_ok".to_string()), "the tool needs the env var");
    }

    #[tokio::test]
    async fn a_stdio_server_that_fails_says_why() {
        let err = list_tools(&fake("crash", vec![])).await.unwrap_err();
        assert_eq!(err, "It stopped: Error: GITHUB_PERSONAL_ACCESS_TOKEN environment variable is required");
        assert_eq!(list_tools(&fake("refuse", vec![])).await.unwrap_err(), "It said: Unsupported protocol version");
        let err = list_tools(&McpServer::stdio("x", "trek-no-such-mcp-server", vec![], vec![])).await.unwrap_err();
        assert_eq!(err, "trek-no-such-mcp-server isn't installed, or isn't on your PATH.");
    }

    #[tokio::test]
    async fn a_silent_server_is_given_up_on_and_ended() {
        let McpTransport::Stdio { command, args, env } = fake("silent", vec![]).transport else { unreachable!() };
        let started = std::time::Instant::now();
        let err = stdio(&command, &args, &env, Duration::from_secs(1)).await.unwrap_err();
        assert_eq!(err, "No answer within 1 seconds.");
        assert!(started.elapsed() < Duration::from_secs(5), "and it's ended without waiting on it");
    }

    /// A real server from npm, fetched by npx: `cargo test -p trek-agents real_server -- --ignored`.
    #[tokio::test]
    #[ignore = "needs npx and the network"]
    async fn a_real_server_from_npm_lists_its_tools() {
        let tools = list_tools(&McpServer::stdio("everything", "npx", vec!["-y".into(), "@modelcontextprotocol/server-everything".into()], vec![])).await.unwrap();
        println!("{} tools: {}", tools.len(), tools.join(", "));
        assert!(tools.iter().any(|t| t == "echo"), "{tools:?}");
        // And remote ones: an open one, and one that wants a sign-in.
        let tools = list_tools(&McpServer::http("deepwiki", "https://mcp.deepwiki.com/mcp", vec![])).await.unwrap();
        println!("deepwiki: {} tools: {}", tools.len(), tools.join(", "));
        assert!(!tools.is_empty());
        println!("notion: {:?}", list_tools(&McpServer::http("notion", "https://mcp.notion.com/mcp", vec![])).await);
    }

    /// A one-request-per-connection HTTP server: `answer` gets the request's head and body and
    /// gives back the whole response.
    async fn serve(answer: impl Fn(&str, &Value) -> String + Send + Sync + 'static) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let mut raw = vec![];
                let mut buf = [0u8; 4096];
                let (head, body) = loop {
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break (String::new(), Value::Null);
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw).to_string();
                    if let Some(split) = text.find("\r\n\r\n") {
                        let head = text[..split].to_string();
                        let len = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if raw.len() >= split + 4 + len {
                            break (head, serde_json::from_slice(&raw[split + 4..split + 4 + len]).unwrap_or(Value::Null));
                        }
                    }
                };
                let _ = sock.write_all(answer(&head, &body).as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        (url, task)
    }

    fn response(status: &str, headers: &str, body: &str) -> String {
        format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    #[tokio::test]
    async fn a_remote_server_answers_over_json_or_an_event_stream() {
        let (url, _server) = serve(|head, body| {
            let head = head.to_ascii_lowercase();
            if head.starts_with("delete") {
                return response("200 OK", "", "");
            }
            assert!(head.contains("authorization: bearer t"), "the token goes along: {head}");
            assert!(head.contains("accept: application/json, text/event-stream"), "{head}");
            match body["method"].as_str() {
                // The reply as an event, with a session to carry on in.
                Some("initialize") => {
                    let event = format!("event: message\ndata: {}\n\n", json!({ "jsonrpc": "2.0", "id": body["id"], "result": { "protocolVersion": PROTOCOL_VERSION, "capabilities": {} } }));
                    response("200 OK", "Content-Type: text/event-stream\r\nMcp-Session-Id: s-1\r\n", &event)
                }
                Some("notifications/initialized") => response("202 Accepted", "", ""),
                Some("tools/list") => {
                    assert!(head.contains("mcp-session-id: s-1"), "the session is carried: {head}");
                    response("200 OK", "Content-Type: application/json\r\n", &json!({ "jsonrpc": "2.0", "id": body["id"], "result": { "tools": [{ "name": "search" }, { "name": "fetch" }] } }).to_string())
                }
                _ => response("400 Bad Request", "", ""),
            }
        })
        .await;
        let tools = list_tools(&McpServer::http("r", url, vec![("Authorization".into(), "Bearer t".into())])).await.unwrap();
        assert_eq!(tools, ["search", "fetch"]);
    }

    #[tokio::test]
    async fn a_remote_server_that_turns_trek_away_says_why() {
        let (url, _server) = serve(|_, _| response("401 Unauthorized", "WWW-Authenticate: Bearer\r\n", "")).await;
        assert_eq!(list_tools(&McpServer::http("r", url, vec![])).await.unwrap_err(), "Needs sign-in or a token (401).");
        // Nothing listening there.
        let free = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        let err = list_tools(&McpServer::http("r", format!("http://{free}/mcp"), vec![])).await.unwrap_err();
        assert!(err.starts_with("Couldn't reach it"), "{err}");
    }
}
