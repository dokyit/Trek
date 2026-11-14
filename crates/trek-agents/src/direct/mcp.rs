//! MCP servers for the Direct harness: Trek's own (browser, computer, phone, simulator, video,
//! orchestration) and the user's. Their tools reach the model through two small tools rather than
//! dozens of schemas in every request: `mcp_list` says what a server has, `mcp_call` runs one. A
//! model that calls a server's tool by its own name is understood too (`find`). Servers start when
//! first needed and end with the session.

use crate::mcp_check::{MAX_PAGES, initialize, post, program_for};
use crate::{GroupChild, McpServer, McpTransport, ProtocolLines};
use serde_json::{Value, json};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};
use tokio::io::{AsyncWriteExt as _, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::Mutex;

/// How long a server gets to start and say what it has: `npx` may be fetching the package.
const START_LIMIT: Duration = Duration::from_secs(30);
/// How long before a server that failed to start is tried again.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(120);
/// How long a tool call may take unless the server's config says.
const CALL_LIMIT: Duration = Duration::from_secs(300);

/// One tool a server has.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub schema: Value,
    /// The server says calling it changes nothing.
    pub read_only: bool,
}

/// An image a tool returned: its type and base64 data.
pub type ToolImage = (String, String);

pub struct McpHub {
    servers: Vec<Entry>,
    /// Images from calls since the session last asked (`take_images`).
    images: StdMutex<Vec<ToolImage>>,
}

struct Entry {
    config: McpServer,
    conn: Mutex<Option<Conn>>,
    tools: Mutex<Option<Vec<McpTool>>>,
    /// When it last failed to start, and why: not tried again for a while, or a server that takes
    /// its whole limit to fail would slow every look at the tools.
    failed: Mutex<Option<(Instant, String)>>,
}

enum Conn {
    Stdio {
        // Dropping it ends the server and all it started.
        _child: GroupChild,
        stdin: ChildStdin,
        lines: ProtocolLines<BufReader<ChildStdout>>,
        next_id: u64,
        /// The server said its tools changed (`notifications/tools/list_changed`).
        tools_changed: bool,
    },
    Http {
        client: reqwest::Client,
        url: String,
        headers: Vec<(String, String)>,
        session: Option<String>,
        next_id: u64,
    },
}

impl Conn {
    async fn start(config: &McpServer) -> Result<Conn, String> {
        match &config.transport {
            McpTransport::Stdio { command, args, env } => {
                use std::process::Stdio;
                let program = program_for(command, env);
                if let Some(problem) = trek_core::detect::batch_args_problem(&program, args) {
                    return Err(problem);
                }
                let mut cmd = trek_core::procs::async_command(&program);
                cmd.args(args)
                    .env("PATH", trek_core::detect::login_path())
                    .envs(env.iter().map(|(k, v)| (k, v)))
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null());
                let mut child = crate::spawn_group(&mut cmd)
                    .map_err(|e| format!("{command} couldn't start: {e}"))?;
                let stdin = child.stdin.take().expect("piped");
                let stdout = child.stdout.take().expect("piped");
                let mut conn = Conn::Stdio {
                    _child: child,
                    stdin,
                    lines: ProtocolLines::new(BufReader::new(stdout)),
                    next_id: 1,
                    tools_changed: false,
                };
                conn.rpc("initialize", initialize()).await?;
                conn.notify("notifications/initialized").await?;
                Ok(conn)
            }
            McpTransport::Http { url, headers } => {
                let client = reqwest::Client::builder()
                    .timeout(
                        config
                            .tool_timeout_secs
                            .map_or(CALL_LIMIT, Duration::from_secs),
                    )
                    .build()
                    .map_err(|e| e.to_string())?;
                let (answer, session) = post(
                    &client,
                    url,
                    headers,
                    None,
                    false,
                    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize() }),
                )
                .await?;
                answer.ok_or("It didn't answer initialize.")?;
                let mut conn = Conn::Http {
                    client,
                    url: url.clone(),
                    headers: headers.clone(),
                    session,
                    next_id: 2,
                };
                conn.notify("notifications/initialized").await?;
                Ok(conn)
            }
        }
    }

    async fn notify(&mut self, method: &str) -> Result<(), String> {
        let message = json!({ "jsonrpc": "2.0", "method": method });
        match self {
            Conn::Stdio { stdin, .. } => write_line(stdin, &message).await,
            Conn::Http {
                client,
                url,
                headers,
                session,
                ..
            } => post(client, url, headers, session.as_deref(), true, message)
                .await
                .map(|_| ()),
        }
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match self {
            Conn::Stdio {
                stdin,
                lines,
                next_id,
                tools_changed,
                ..
            } => {
                let id = *next_id;
                *next_id += 1;
                write_line(
                    stdin,
                    &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
                )
                .await?;
                loop {
                    let Some(line) = lines
                        .next_line()
                        .await
                        .map_err(|e| format!("The server stopped: {e}"))?
                    else {
                        return Err("The server stopped.".to_string());
                    };
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if message["method"] == "notifications/tools/list_changed" {
                        *tools_changed = true;
                        continue;
                    }
                    // A reply to a call a Stop abandoned has an older id, and is passed over.
                    if message["id"] == json!(id) && message.get("method").is_none() {
                        if let Some(error) = message.get("error") {
                            let said = error["message"].as_str().unwrap_or("an error");
                            return Err(format!("The server said: {said}"));
                        }
                        return Ok(message["result"].clone());
                    }
                    // A request of its own is answered so it doesn't wait on Trek.
                    if let (Some(asked), Some(their)) =
                        (message["method"].as_str(), message.get("id"))
                    {
                        let answer = match asked {
                            "ping" => json!({ "jsonrpc": "2.0", "id": their, "result": {} }),
                            "roots/list" => {
                                json!({ "jsonrpc": "2.0", "id": their, "result": { "roots": [] } })
                            }
                            _ => {
                                json!({ "jsonrpc": "2.0", "id": their, "error": { "code": -32601, "message": "Not supported" } })
                            }
                        };
                        write_line(stdin, &answer).await?;
                    }
                }
            }
            Conn::Http {
                client,
                url,
                headers,
                session,
                next_id,
            } => {
                let id = *next_id;
                *next_id += 1;
                let (answer, _) = post(
                    client,
                    url,
                    headers,
                    session.as_deref(),
                    true,
                    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
                )
                .await?;
                answer.ok_or_else(|| format!("It didn't answer {method}."))
            }
        }
    }

    /// Whether the server said its tools changed since this was last asked.
    fn take_tools_changed(&mut self) -> bool {
        match self {
            Conn::Stdio { tools_changed, .. } => std::mem::take(tools_changed),
            Conn::Http { .. } => false,
        }
    }

    async fn list_tools(&mut self) -> Result<Vec<McpTool>, String> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let page = self.rpc("tools/list", params).await?;
            for tool in page["tools"].as_array().into_iter().flatten() {
                let Some(name) = tool["name"].as_str() else {
                    continue;
                };
                tools.push(McpTool {
                    name: name.to_string(),
                    description: tool["description"].as_str().unwrap_or_default().to_string(),
                    schema: tool["inputSchema"].clone(),
                    read_only: tool["annotations"]["readOnlyHint"].as_bool() == Some(true),
                });
            }
            cursor = page["nextCursor"]
                .as_str()
                .filter(|c| !c.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }
}

async fn write_line(stdin: &mut ChildStdin, message: &Value) -> Result<(), String> {
    let mut line = message.to_string();
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|_| "The server stopped.".to_string())?;
    stdin
        .flush()
        .await
        .map_err(|_| "The server stopped.".to_string())
}

impl McpHub {
    pub fn new(servers: Vec<McpServer>) -> Self {
        McpHub {
            servers: servers
                .into_iter()
                .map(|config| Entry {
                    config,
                    conn: Mutex::new(None),
                    tools: Mutex::new(None),
                    failed: Mutex::new(None),
                })
                .collect(),
            images: StdMutex::new(Vec::new()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    pub fn names(&self) -> Vec<&str> {
        self.servers
            .iter()
            .map(|e| e.config.name.as_str())
            .collect()
    }

    fn entry(&self, name: &str) -> Option<&Entry> {
        self.servers.iter().find(|e| e.config.name == name)
    }

    /// The tools `entry` has, starting it if it isn't running.
    async fn tools_of(&self, entry: &Entry) -> Result<Vec<McpTool>, String> {
        let mut known = entry.tools.lock().await;
        if let Some(tools) = &*known {
            return Ok(tools.clone());
        }
        if let Some((when, why)) = &*entry.failed.lock().await {
            if when.elapsed() < RETRY_AFTER_FAILURE {
                return Err(why.clone());
            }
        }
        let mut conn = entry.conn.lock().await;
        let work = async {
            if conn.is_none() {
                *conn = Some(Conn::start(&entry.config).await?);
            }
            conn.as_mut().expect("just started").list_tools().await
        };
        let listed = tokio::time::timeout(START_LIMIT, work)
            .await
            .map_err(|_| format!("It didn't answer within {} seconds.", START_LIMIT.as_secs()))
            .and_then(|r| r);
        match listed {
            Ok(tools) => {
                *known = Some(tools.clone());
                Ok(tools)
            }
            Err(e) => {
                *conn = None;
                *entry.failed.lock().await = Some((Instant::now(), e.clone()));
                Err(e)
            }
        }
    }

    /// What `mcp_list` answers: a server's tools with their parameters, or the servers and the
    /// names of what each has.
    pub async fn listing(&self, server: Option<&str>) -> (String, bool) {
        if let Some(name) = server.filter(|s| !s.trim().is_empty()) {
            let Some(entry) = self.entry(name) else {
                return (
                    format!(
                        "Error: there's no server called {name}. There are: {}.",
                        self.names().join(", ")
                    ),
                    false,
                );
            };
            return match self.tools_of(entry).await {
                Ok(tools) if tools.is_empty() => (format!("{name} has no tools."), true),
                Ok(tools) => {
                    let mut out = String::new();
                    for tool in tools {
                        out.push_str(&format!(
                            "{}{}\n  arguments: {}\n",
                            tool.name,
                            if tool.description.is_empty() {
                                String::new()
                            } else {
                                format!(": {}", tool.description)
                            },
                            compact_schema(&tool.schema),
                        ));
                    }
                    (out, true)
                }
                Err(e) => (format!("Error: {name} isn't available. {e}"), false),
            };
        }
        let mut out = String::new();
        for entry in &self.servers {
            match self.tools_of(entry).await {
                Ok(tools) => {
                    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
                    out.push_str(&format!("{}: {}\n", entry.config.name, names.join(", ")));
                }
                Err(e) => out.push_str(&format!("{}: not available. {e}\n", entry.config.name)),
            }
        }
        out.push_str("Call mcp_list with a server's name for what its tools take.\n");
        (out, true)
    }

    /// Which server has a tool called `tool`, in order: for a model that calls one by its name.
    pub async fn find(&self, tool: &str) -> Option<String> {
        for entry in &self.servers {
            if let Ok(tools) = self.tools_of(entry).await {
                if tools.iter().any(|t| t.name == tool) {
                    return Some(entry.config.name.clone());
                }
            }
        }
        None
    }

    /// `tool` on `server`, or what to tell the model instead.
    pub async fn tool(&self, server: &str, tool: &str) -> Result<McpTool, String> {
        let entry = self.entry(server).ok_or_else(|| {
            format!(
                "Error: there's no server called {server}. There are: {}.",
                self.names().join(", ")
            )
        })?;
        let tools = self
            .tools_of(entry)
            .await
            .map_err(|e| format!("Error: {server} isn't available. {e}"))?;
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        tools
            .iter()
            .find(|t| t.name == tool)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "Error: {server} has no tool called {tool}. It has: {}.",
                    names.join(", ")
                )
            })
    }

    /// Runs a tool: its output as text, and whether it worked. Images it returned wait in
    /// `take_images`.
    pub async fn call(&self, server: &str, tool: &str, arguments: Value) -> (String, bool) {
        let Some(entry) = self.entry(server) else {
            return (format!("Error: there's no server called {server}."), false);
        };
        let limit = entry
            .config
            .tool_timeout_secs
            .map_or(CALL_LIMIT, Duration::from_secs);
        // Calls to one server go one at a time: its line protocol is matched up by order of need.
        let mut conn = entry.conn.lock().await;
        if conn.is_none() {
            match tokio::time::timeout(START_LIMIT, Conn::start(&entry.config)).await {
                Ok(Ok(started)) => *conn = Some(started),
                Ok(Err(e)) => return (format!("Error: {server} isn't available. {e}"), false),
                Err(_) => {
                    return (
                        format!(
                            "Error: {server} didn't start within {} seconds.",
                            START_LIMIT.as_secs()
                        ),
                        false,
                    );
                }
            }
        }
        let params = json!({ "name": tool, "arguments": arguments });
        let answer = tokio::time::timeout(
            limit,
            conn.as_mut()
                .expect("just started")
                .rpc("tools/call", params),
        )
        .await;
        match answer {
            Ok(Ok(result)) => {
                // Tools it added (the iPhone's, once one is connected) are listed afresh.
                if conn.as_mut().is_some_and(Conn::take_tools_changed) {
                    *entry.tools.lock().await = None;
                }
                let (text, ok, images) = render(&result);
                self.images.lock().expect("images").extend(images);
                (text, ok)
            }
            Ok(Err(e)) => {
                // A stopped server starts again on the next call, and so does an http session
                // the server has dropped, which would otherwise fail every call from here on.
                let http = matches!(conn.as_ref(), Some(Conn::Http { .. }));
                if http || e == "The server stopped." || e.starts_with("The server stopped:") {
                    *conn = None;
                }
                (format!("Error: {e}"), false)
            }
            Err(_) => (
                format!(
                    "Error: {tool} didn't finish within {} seconds.",
                    limit.as_secs()
                ),
                false,
            ),
        }
    }

    /// Images tool calls returned since this was last asked.
    pub fn take_images(&self) -> Vec<ToolImage> {
        std::mem::take(&mut *self.images.lock().expect("images"))
    }
}

/// A tool's parameter schema, small enough to read: names, types, what's required.
fn compact_schema(schema: &Value) -> String {
    let Some(properties) = schema["properties"].as_object() else {
        return "none".to_string();
    };
    if properties.is_empty() {
        return "none".to_string();
    }
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut parts = Vec::new();
    for (name, spec) in properties {
        let kind = spec["type"].as_str().unwrap_or("any");
        let mut part = format!(
            "{name} ({kind}{})",
            if required.contains(&name.as_str()) {
                ", required"
            } else {
                ""
            }
        );
        if let Some(about) = spec["description"].as_str().filter(|d| !d.is_empty()) {
            let short = about.lines().next().unwrap_or(about);
            part.push_str(&format!(": {short}"));
        }
        parts.push(part);
    }
    parts.join("; ")
}

/// A `tools/call` result as text, whether it worked, and the images in it.
fn render(result: &Value) -> (String, bool, Vec<ToolImage>) {
    let ok = result["isError"].as_bool() != Some(true);
    let mut text = String::new();
    let mut images = Vec::new();
    for part in result["content"].as_array().into_iter().flatten() {
        let piece = match part["type"].as_str() {
            Some("text") => part["text"].as_str().unwrap_or_default().to_string(),
            Some("image") => {
                if let (Some(kind), Some(data)) = (part["mimeType"].as_str(), part["data"].as_str())
                {
                    images.push((kind.to_string(), data.to_string()));
                }
                "[image]".to_string()
            }
            Some("resource") => part["resource"]["text"]
                .as_str()
                .map(str::to_string)
                .or_else(|| part["resource"]["uri"].as_str().map(str::to_string))
                .unwrap_or_default(),
            Some("resource_link") => part["uri"].as_str().unwrap_or_default().to_string(),
            _ => String::new(),
        };
        if piece.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&piece);
    }
    if text.is_empty() {
        text = match result.get("structuredContent") {
            Some(structured) if !structured.is_null() => structured.to_string(),
            _ => "(The tool returned nothing.)".to_string(),
        };
    }
    (text, ok, images)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake() -> McpHub {
        McpHub::new(vec![McpServer::stdio(
            "fake",
            trek_test_fixtures::bin("fake-mcp").display().to_string(),
            vec!["ok".into()],
            vec![],
        )])
    }

    #[tokio::test]
    async fn a_server_says_what_it_has_and_runs_it() {
        let hub = fake();
        let (all, ok) = hub.listing(None).await;
        assert!(ok && all.contains("fake: echo"), "{all}");
        let (one, ok) = hub.listing(Some("fake")).await;
        assert!(
            ok && one.contains("echo") && one.contains("arguments:"),
            "{one}"
        );
        assert_eq!(hub.find("echo").await.as_deref(), Some("fake"));
        assert_eq!(hub.find("nope").await, None);
        let (out, ok) = hub.call("fake", "echo", json!({ "text": "hello" })).await;
        assert!(ok, "{out}");
        assert_eq!(out, "hello");
        let (out, ok) = hub.call("fake", "fail", json!({})).await;
        assert!(!ok && out.contains("it failed"), "{out}");
        let (out, ok) = hub.call("fake", "picture", json!({})).await;
        assert!(ok && out.contains("[image]"), "{out}");
        assert_eq!(
            hub.take_images(),
            [("image/png".to_string(), "aGk=".to_string())]
        );
        assert!(hub.take_images().is_empty());
    }

    /// A server that adds tools as it goes (Trek's iPhone tools, once a phone is connected) says
    /// so, and they're listed afresh after the call that added them.
    #[tokio::test]
    async fn tools_a_server_adds_are_listed_after_it_says_so() {
        let hub = fake();
        assert!(hub.tool("fake", "grown").await.is_err());
        let (out, ok) = hub.call("fake", "grow", json!({})).await;
        assert!(ok && out == "grew", "{out}");
        assert!(hub.tool("fake", "grown").await.is_ok());
        let (out, ok) = hub.call("fake", "echo", json!({ "text": "still" })).await;
        assert!(ok && out == "still", "{out}");
    }

    #[tokio::test]
    async fn tools_are_told_apart_by_what_they_say_about_themselves() {
        let hub = fake();
        assert!(hub.tool("fake", "get_time").await.unwrap().read_only);
        assert!(!hub.tool("fake", "echo").await.unwrap().read_only);
        let missing = hub.tool("fake", "nope").await.unwrap_err();
        assert!(
            missing.contains("has no tool") && missing.contains("echo"),
            "{missing}"
        );
        let missing = hub.tool("other", "echo").await.unwrap_err();
        assert!(missing.contains("no server called other"), "{missing}");
    }

    #[tokio::test]
    async fn a_server_that_cant_start_says_so_and_leaves_the_rest() {
        let hub = McpHub::new(vec![
            McpServer::stdio(
                "broken",
                trek_test_fixtures::bin("fake-mcp").display().to_string(),
                vec!["crash".into()],
                vec![],
            ),
            McpServer::stdio(
                "fake",
                trek_test_fixtures::bin("fake-mcp").display().to_string(),
                vec!["ok".into()],
                vec![],
            ),
        ]);
        let (all, ok) = hub.listing(None).await;
        assert!(ok, "{all}");
        assert!(all.contains("broken: not available"), "{all}");
        assert!(all.contains("fake: echo"), "{all}");
        let (out, ok) = hub.call("broken", "echo", json!({})).await;
        assert!(!ok && out.contains("isn't available"), "{out}");
    }

    #[test]
    fn schemas_read_as_short_text() {
        let schema = json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "Where to go.\nMore words." },
                "wait": { "type": "integer" }
            },
            "required": ["url"]
        });
        assert_eq!(
            compact_schema(&schema),
            "url (string, required): Where to go.; wait (integer)"
        );
        assert_eq!(compact_schema(&json!({})), "none");
    }

    #[test]
    fn results_read_as_text() {
        let (text, ok, images) = render(&json!({
            "content": [
                { "type": "text", "text": "one" },
                { "type": "resource", "resource": { "uri": "file:///a", "text": "two" } },
                { "type": "image", "mimeType": "image/png", "data": "abc" }
            ]
        }));
        assert_eq!(text, "one\ntwo\n[image]");
        assert!(ok);
        assert_eq!(images.len(), 1);
        let (text, ok, _) = render(&json!({ "isError": true, "structuredContent": { "a": 1 } }));
        assert_eq!(text, "{\"a\":1}");
        assert!(!ok);
    }
}
