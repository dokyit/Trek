//! Agent Client Protocol (v1) agents: newline-delimited JSON-RPC 2.0 over the agent's stdio.
//! Covers OpenCode, Droid and every ACP agent in the catalog. Trek acts as the ACP client:
//! it answers permission prompts and `fs/*` requests; the agent keeps its own login.

use crate::{AgentEvent, Command, Decision, SessionConfig, clip};
use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use trek_core::catalog::{ACP_AGENTS, ModelInfo};
use trek_core::{AgentId, Effort, HandHolding, detect};

/// What `acp_probe` learns about an installed ACP agent.
#[derive(Debug, Clone, Default)]
pub struct AcpInfo {
    pub models: Vec<ModelInfo>,
    /// `(id, name)` of the agent's advertised login methods.
    pub auth_methods: Vec<(String, String)>,
    /// The agent refused to open a session until the user signs in.
    pub needs_auth: bool,
}

/// Binary and arguments that start `agent` as an ACP server.
fn launch_spec(agent: &AgentId) -> Result<(PathBuf, Vec<String>, String)> {
    let (binary, args, name, hint): (&str, Vec<&str>, String, &str) = match agent {
        AgentId::OpenCode => ("opencode", vec!["acp"], "OpenCode".into(), "curl -fsSL https://opencode.ai/install | bash"),
        AgentId::Droid => ("droid", vec!["exec", "--output-format", "acp"], "Droid".into(), "curl -fsSL https://app.factory.ai/cli | sh"),
        AgentId::Acp(id) => {
            let a = ACP_AGENTS.iter().find(|a| a.id == id).ok_or_else(|| anyhow!("Unknown ACP agent: {id}"))?;
            (a.binary, a.args.to_vec(), a.name.into(), a.install_hint)
        }
        other => bail!("{} doesn't speak ACP", other.display_name()),
    };
    let path = detect::which(binary).with_context(|| format!("{name} isn't installed ({hint})"))?;
    Ok((path, args.into_iter().map(String::from).collect(), name))
}

struct Agent {
    child: Child,
    rpc: Rpc,
    lines: Lines<BufReader<ChildStdout>>,
    stderr: Arc<Mutex<Vec<String>>>,
    name: String,
}

impl Agent {
    fn spawn(agent: &AgentId, cwd: &Path) -> Result<Agent> {
        let (bin, args, name) = launch_spec(agent)?;
        let mut child = tokio::process::Command::new(&bin)
            .args(&args)
            .current_dir(cwd)
            .env("PATH", detect::login_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start {}", bin.display()))?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let tail = stderr.clone();
        let err = child.stderr.take().unwrap();
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                tracing::debug!("acp stderr: {l}");
                let mut t = tail.lock().unwrap();
                t.push(l);
                if t.len() > 20 {
                    t.remove(0);
                }
            }
        });
        let rpc = Rpc { stdin: child.stdin.take().unwrap(), next_id: 0 };
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Ok(Agent { child, rpc, lines, stderr, name })
    }

    fn exited(&self) -> anyhow::Error {
        let tail = self.stderr.lock().unwrap();
        match tail.iter().rev().find(|l| !l.trim().is_empty()) {
            Some(l) => anyhow!("{} exited: {}", self.name, l.trim()),
            None => anyhow!("{} exited", self.name),
        }
    }

    /// Send a request and read until its response, answering `fs/*` requests inline and
    /// queueing everything else in `backlog`. The inner result is the JSON-RPC error object.
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        fs: &FsPolicy,
        backlog: &mut Vec<Value>,
        timeout: Duration,
    ) -> Result<std::result::Result<Value, Value>> {
        let id = self.rpc.request(method, params).await?;
        let wait = async {
            while let Some(line) = self.lines.next_line().await? {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if v.get("method").is_none() && v["id"].as_i64() == Some(id) {
                    return Ok(match v.get("error") {
                        Some(e) => Err(e.clone()),
                        None => Ok(v["result"].clone()),
                    });
                }
                if let (Some(m), Some(rid)) = (v["method"].as_str(), v.get("id"))
                    && m.starts_with("fs/")
                {
                    let reply = fs.handle(m, &v["params"]).await;
                    self.rpc.reply(rid.clone(), reply).await?;
                    continue;
                }
                backlog.push(v);
            }
            Err(self.exited())
        };
        match tokio::time::timeout(timeout, wait).await {
            Ok(r) => r,
            Err(_) => bail!("{} didn't answer {method} in {}s", self.name, timeout.as_secs()),
        }
    }

    async fn handshake(&mut self, fs: &FsPolicy, backlog: &mut Vec<Value>) -> Result<Value> {
        let params = json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true }, "terminal": false },
            "clientInfo": { "name": "trek", "title": "Trek", "version": trek_core::VERSION },
        });
        match self.call("initialize", params, fs, backlog, Duration::from_secs(30)).await? {
            Ok(r) => Ok(r),
            Err(e) => bail!("{}: {}", self.name, rpc_message(&e)),
        }
    }
}

struct Rpc {
    stdin: ChildStdin,
    next_id: i64,
}

impl Rpc {
    async fn send(&mut self, mut v: Value) -> Result<()> {
        v["jsonrpc"] = json!("2.0");
        let mut s = serde_json::to_string(&v)?;
        s.push('\n');
        self.stdin.write_all(s.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<i64> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "id": id, "method": method, "params": params })).await?;
        Ok(id)
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({ "method": method, "params": params })).await
    }

    async fn reply(&mut self, id: Value, reply: std::result::Result<Value, (i64, String)>) -> Result<()> {
        match reply {
            Ok(result) => self.send(json!({ "id": id, "result": result })).await,
            Err((code, message)) => self.send(json!({ "id": id, "error": { "code": code, "message": message } })).await,
        }
    }
}

fn rpc_message(e: &Value) -> String {
    let msg = e["message"].as_str().unwrap_or("request failed");
    match e["data"].as_str().or(e["data"]["message"].as_str()) {
        Some(d) if !d.is_empty() && d != msg => format!("{msg}: {d}"),
        _ => msg.to_string(),
    }
}

fn is_auth_error(e: &Value) -> bool {
    e["code"].as_i64() == Some(-32000) || rpc_message(e).to_lowercase().contains("auth")
}

fn auth_methods(init: &Value) -> Vec<(String, String)> {
    init["authMethods"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            Some((id.clone(), m["name"].as_str().map(String::from).unwrap_or(id)))
        })
        .collect()
}

fn auth_hint(name: &str, init: &Value) -> String {
    let how = init["authMethods"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|m| m["description"].as_str().or(m["name"].as_str()))
        .unwrap_or("sign in from its CLI");
    format!("{name} needs you to sign in first: {how}")
}

/// Answers `fs/read_text_file` and `fs/write_text_file`. Writes stay inside the
/// session's folder unless the user granted full access.
struct FsPolicy {
    cwd: PathBuf,
    full_access: bool,
}

impl FsPolicy {
    async fn handle(&self, method: &str, p: &Value) -> std::result::Result<Value, (i64, String)> {
        if !matches!(method, "fs/read_text_file" | "fs/write_text_file") {
            return Err((-32601, format!("{method} isn't supported by Trek")));
        }
        let path = PathBuf::from(p["path"].as_str().unwrap_or_default());
        if !path.is_absolute() {
            return Err((-32602, "path must be absolute".into()));
        }
        match method {
            "fs/read_text_file" => {
                let text = tokio::fs::read_to_string(&path).await.map_err(|e| (-32002, format!("{}: {e}", path.display())))?;
                let line = p["line"].as_u64().unwrap_or(1).max(1) as usize;
                let content = match p["limit"].as_u64() {
                    None if line == 1 => text,
                    limit => {
                        let lines = text.split_inclusive('\n').skip(line - 1);
                        match limit {
                            Some(n) => lines.take(n as usize).collect(),
                            None => lines.collect(),
                        }
                    }
                };
                Ok(json!({ "content": content }))
            }
            "fs/write_text_file" => {
                if !self.full_access && !within(&path, &self.cwd) {
                    return Err((-32003, format!("{} is outside the project folder", path.display())));
                }
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| (-32603, e.to_string()))?;
                }
                let content = p["content"].as_str().unwrap_or_default();
                tokio::fs::write(&path, content).await.map_err(|e| (-32603, format!("{}: {e}", path.display())))?;
                Ok(Value::Null)
            }
            _ => unreachable!(),
        }
    }
}

/// `path` with `.`/`..` removed and its longest existing ancestor resolved through symlinks.
fn resolve(path: &Path) -> PathBuf {
    let mut clean = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    let mut base = clean.clone();
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = base.canonicalize() {
            return rest.iter().rev().fold(real, |acc: PathBuf, part| acc.join(part));
        }
        match (base.file_name().map(|n| n.to_os_string()), base.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                base = parent.to_path_buf();
            }
            _ => return clean,
        }
    }
}

fn within(path: &Path, root: &Path) -> bool {
    resolve(path).starts_with(resolve(root))
}

/// Whether Trek answers a permission prompt itself for this tool kind.
fn auto_allow(h: HandHolding, kind: &str) -> bool {
    match h {
        HandHolding::FullAccess => true,
        HandHolding::AutoAcceptEdits => kind == "edit",
        HandHolding::Auto => matches!(kind, "read" | "search" | "think" | "fetch" | "edit"),
        HandHolding::Supervised => false,
    }
}

/// The `optionId` that best matches `decision`.
fn pick_option(options: &[Value], decision: Decision) -> Option<String> {
    let kinds: &[&str] = match decision {
        Decision::Allow => &["allow_once", "allow_always"],
        Decision::AllowForSession => &["allow_always", "allow_once"],
        Decision::Deny => &["reject_once", "reject_always"],
    };
    kinds
        .iter()
        .find_map(|k| options.iter().find(|o| o["kind"] == *k))
        .or_else(|| options.first())
        .and_then(|o| o["optionId"].as_str())
        .map(String::from)
}

fn permission_outcome(option: Option<String>) -> Value {
    match option {
        Some(id) => json!({ "outcome": { "outcome": "selected", "optionId": id } }),
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

fn kind_title(kind: &str) -> &'static str {
    match kind {
        "read" => "Read",
        "edit" => "Edit",
        "delete" => "Delete",
        "move" => "Move",
        "search" => "Search",
        "execute" => "Run command",
        "think" => "Think",
        "fetch" => "Fetch",
        _ => "Tool",
    }
}

fn tool_detail(tc: &Value) -> String {
    let input = &tc["rawInput"];
    let command = match &input["command"] {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => Some(a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")),
        _ => input["cmd"].as_str().map(String::from),
    };
    let location = tc["locations"].as_array().and_then(|l| l.first()).and_then(|l| l["path"].as_str()).map(String::from);
    command
        .or(location)
        .or_else(|| {
            ["path", "file_path", "filePath", "filepath", "url", "query", "pattern", "description"]
                .iter()
                .find_map(|k| input[*k].as_str().map(String::from))
        })
        .filter(|s| !s.is_empty())
        .map(|s| clip(&s, 400))
        .unwrap_or_default()
}

fn tool_output(tc: &Value) -> String {
    let mut parts = Vec::new();
    for c in tc["content"].as_array().into_iter().flatten() {
        match c["type"].as_str() {
            Some("content") => {
                if let Some(t) = c["content"]["text"].as_str() {
                    parts.push(t.to_string());
                }
            }
            Some("diff") => parts.push(format!("Edited {}", c["path"].as_str().unwrap_or_default())),
            _ => {}
        }
    }
    if parts.is_empty() {
        match &tc["rawOutput"] {
            Value::String(s) => parts.push(s.clone()),
            Value::Null => {}
            v => match v["output"].as_str().or(v["stdout"].as_str()) {
                Some(s) => parts.push(s.to_string()),
                None => parts.push(v.to_string()),
            },
        }
    }
    clip(&parts.join("\n"), 8000)
}

#[derive(Default)]
struct Tool {
    title: String,
    kind: String,
    detail: String,
    started: bool,
}

/// Turns `session/update` notifications into [`AgentEvent`]s for one session.
#[derive(Default)]
struct Turn {
    text: String,
    tools: HashMap<String, Tool>,
}

impl Turn {
    fn flush_text(&mut self, out: &mut Vec<AgentEvent>) {
        if !self.text.is_empty() {
            out.push(AgentEvent::TextDone(std::mem::take(&mut self.text)));
        }
    }

    fn update(&mut self, u: &Value) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        match u["sessionUpdate"].as_str() {
            Some("agent_message_chunk") => {
                if let Some(t) = u["content"]["text"].as_str().filter(|t| !t.is_empty()) {
                    self.text.push_str(t);
                    out.push(AgentEvent::TextDelta(t.into()));
                }
            }
            Some("agent_thought_chunk") => {
                if let Some(t) = u["content"]["text"].as_str().filter(|t| !t.is_empty()) {
                    out.push(AgentEvent::ReasoningDelta(t.into()));
                }
            }
            Some(kind @ ("tool_call" | "tool_call_update")) => {
                let id = u["toolCallId"].as_str().unwrap_or_default().to_string();
                let tool = self.tools.entry(id.clone()).or_default();
                if let Some(t) = u["title"].as_str().filter(|t| !t.is_empty()) {
                    tool.title = t.into();
                }
                if let Some(k) = u["kind"].as_str() {
                    tool.kind = k.into();
                }
                let detail = tool_detail(u);
                if !detail.is_empty() {
                    tool.detail = detail;
                }
                let status = u["status"].as_str().unwrap_or(if kind == "tool_call" { "pending" } else { "" });
                let done = matches!(status, "completed" | "failed");
                // Hold a bare pending call until its input arrives, so the row has something to show.
                let start = !tool.started && (done || status == "in_progress" || !tool.detail.is_empty());
                if start {
                    tool.started = true;
                    let title = if tool.title.is_empty() { kind_title(&tool.kind).to_string() } else { tool.title.clone() };
                    let detail = tool.detail.clone();
                    self.flush_text(&mut out);
                    out.push(AgentEvent::ToolStarted { id: id.clone(), title, detail });
                }
                if done {
                    self.tools.remove(&id);
                    out.push(AgentEvent::ToolFinished { id, output: tool_output(u), ok: status == "completed" });
                }
            }
            _ => {}
        }
        out
    }

    fn finish(&mut self, stop: std::result::Result<&str, String>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        self.flush_text(&mut out);
        self.tools.clear();
        let error = match stop {
            Ok("cancelled") => Some("Interrupted".to_string()),
            Ok("refusal") => Some("The agent refused to continue.".to_string()),
            Ok("max_tokens") => Some("The agent hit its output limit.".to_string()),
            Ok("max_turn_requests") => Some("The agent hit its request limit for this turn.".to_string()),
            Ok(_) => None,
            Err(msg) => Some(msg),
        };
        out.push(AgentEvent::TurnComplete { cost_usd: None, error });
        out
    }
}

/// How this agent switches models, if at all.
#[derive(Debug, Clone, PartialEq)]
enum ModelSwitch {
    None,
    /// `session/set_model` (ACP `models`).
    SetModel,
    /// `session/set_config_option` on the given select option.
    Config(String),
}

/// Session controls advertised in a `session/new` or `session/load` result.
#[derive(Debug, Clone)]
struct Controls {
    models: Vec<ModelInfo>,
    current_model: Option<String>,
    switch: ModelSwitch,
    /// Config option id and values for reasoning effort.
    effort: Option<(String, Vec<String>)>,
    plan_mode: Option<String>,
}

fn select_options(opt: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for o in opt["options"].as_array().into_iter().flatten() {
        // Grouped selects nest their options.
        if o["options"].is_array() {
            out.extend(select_options(o));
        } else if let Some(v) = o["value"].as_str() {
            out.push((v.to_string(), o["name"].as_str().unwrap_or(v).to_string()));
        }
    }
    out
}

fn sorted_efforts(values: impl Iterator<Item = Effort>) -> Vec<Effort> {
    let mut e: Vec<Effort> = values.collect();
    e.sort();
    e.dedup();
    e
}

fn controls(result: &Value) -> Controls {
    let config: Vec<&Value> = result["configOptions"].as_array().into_iter().flatten().collect();
    let by_category = |c: &str| config.iter().find(|o| o["category"] == c || o["id"] == c).copied();
    let effort = by_category("thought_level")
        .or_else(|| by_category("reasoning_effort"))
        .and_then(|o| Some((o["id"].as_str()?.to_string(), select_options(o).into_iter().map(|(v, _)| v).collect::<Vec<_>>())));
    let shared_efforts = sorted_efforts(effort.iter().flat_map(|(_, v)| v.iter().filter_map(|s| Effort::parse(s))));

    let (models, current_model, switch) = if let Some(list) = result["models"]["availableModels"].as_array() {
        let models = list
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                let id = m["modelId"].as_str()?.to_string();
                let own = sorted_efforts(
                    m["_meta"]["reasoningEfforts"].as_array().into_iter().flatten().filter_map(|e| e["value"].as_str().and_then(Effort::parse)),
                );
                Some(ModelInfo {
                    name: m["name"].as_str().map(String::from).unwrap_or_else(|| id.clone()),
                    id,
                    efforts: if own.is_empty() { shared_efforts.clone() } else { own },
                    tier: i.min(255) as u8,
                    fast: None,
                })
            })
            .collect();
        (models, result["models"]["currentModelId"].as_str().map(String::from), ModelSwitch::SetModel)
    } else if let Some(o) = by_category("model") {
        let models = select_options(o)
            .into_iter()
            .enumerate()
            .map(|(i, (id, name))| ModelInfo { id, name, efforts: shared_efforts.clone(), tier: i.min(255) as u8, fast: None })
            .collect();
        let switch = o["id"].as_str().map(|id| ModelSwitch::Config(id.into())).unwrap_or(ModelSwitch::None);
        (models, o["currentValue"].as_str().map(String::from), switch)
    } else {
        (vec![], None, ModelSwitch::None)
    };

    let plan_mode = result["modes"]["availableModes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|m| {
            let id = m["id"].as_str().unwrap_or_default().to_lowercase();
            id == "plan" || id.ends_with("#plan") || m["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("plan"))
        })
        .and_then(|m| m["id"].as_str().map(String::from));

    Controls { models, current_model, switch, effort, plan_mode }
}

/// Request params that switch to `model`, if the agent supports it.
fn model_request(c: &Controls, session_id: &str, model: &str) -> Option<(&'static str, Value)> {
    match &c.switch {
        ModelSwitch::SetModel => Some(("session/set_model", json!({ "sessionId": session_id, "modelId": model }))),
        ModelSwitch::Config(id) => {
            Some(("session/set_config_option", json!({ "sessionId": session_id, "configId": id, "value": model })))
        }
        ModelSwitch::None => None,
    }
}

/// Request params that set reasoning effort to the agent's nearest level.
fn effort_request(c: &Controls, session_id: &str, effort: Effort) -> Option<(&'static str, Value)> {
    let (id, values) = c.effort.as_ref()?;
    let levels: Vec<(Effort, &String)> = values.iter().filter_map(|v| Effort::parse(v).map(|e| (e, v))).collect();
    let want = effort.clamp_to(&sorted_efforts(levels.iter().map(|(e, _)| *e)));
    let (_, value) = levels.iter().find(|(e, _)| *e == want)?;
    Some(("session/set_config_option", json!({ "sessionId": session_id, "configId": id, "value": value })))
}

/// Run an ACP session for `config.agent` until shutdown or the agent exits.
pub async fn run(
    config: SessionConfig,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let mut agent = Agent::spawn(&config.agent, &config.cwd)?;
    let mut hand_holding = config.hand_holding;
    let mut fs = FsPolicy { cwd: config.cwd.clone(), full_access: hand_holding == HandHolding::FullAccess };
    let mut backlog = Vec::new();
    let init = agent.handshake(&fs, &mut backlog).await?;
    let cwd = config.cwd.display().to_string();
    let mcp = mcp_servers_json(&config.mcp_servers);
    let setup = Duration::from_secs(120);

    let mut opened = None;
    if let Some(id) = &config.resume
        && init["agentCapabilities"]["loadSession"] == true
    {
        let params = json!({ "sessionId": id, "cwd": cwd, "mcpServers": mcp });
        match agent.call("session/load", params, &fs, &mut backlog, setup).await? {
            Ok(r) => {
                // The agent replays history as updates; the transcript already has it.
                backlog.retain(|v| v["method"] != "session/update");
                opened = Some((id.clone(), r));
            }
            Err(e) => tracing::warn!("session/load failed, starting fresh: {}", rpc_message(&e)),
        }
    }
    let (session_id, result) = match opened {
        Some(s) => s,
        None => match agent.call("session/new", json!({ "cwd": cwd, "mcpServers": mcp }), &fs, &mut backlog, setup).await? {
            Ok(r) => (r["sessionId"].as_str().context("session/new returned no sessionId")?.to_string(), r),
            Err(e) if is_auth_error(&e) => bail!(auth_hint(&agent.name, &init)),
            Err(e) => bail!("{}: {}", agent.name, rpc_message(&e)),
        },
    };

    let ctl = controls(&result);
    let mut model = ctl.current_model.clone();
    if let Some(want) = config.model.as_ref().filter(|m| model.as_ref() != Some(*m))
        && let Some((method, params)) = model_request(&ctl, &session_id, want)
    {
        if let Ok(Ok(_)) = agent.call(method, params, &fs, &mut backlog, setup).await {
            model = Some(want.clone());
        }
    }
    if config.effort != Effort::Off
        && let Some((method, params)) = effort_request(&ctl, &session_id, config.effort)
    {
        let _ = agent.call(method, params, &fs, &mut backlog, setup).await?;
    }
    if config.plan
        && let Some(mode) = &ctl.plan_mode
    {
        let params = json!({ "sessionId": session_id, "modeId": mode });
        let _ = agent.call("session/set_mode", params, &fs, &mut backlog, setup).await?;
    }
    events.send(AgentEvent::Started { native_id: session_id.clone(), model }).await?;

    let mut s = Live { session_id, turn: Turn::default(), perms: HashMap::new(), prompt: None };
    for v in std::mem::take(&mut backlog) {
        if !s.handle(&v, &mut agent.rpc, &events, &fs, hand_holding).await? {
            return Ok(());
        }
    }

    loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Ok(cmd) = cmd else { break };
                match cmd {
                    Command::Prompt { text, images } => {
                        let mut prompt = Vec::new();
                        for path in &images {
                            match crate::load_image(path) {
                                Ok((mime, data)) => prompt.push(json!({ "type": "image", "mimeType": mime, "data": data })),
                                Err(e) => { let _ = events.send(AgentEvent::Error(format!("{e:#}"))).await; }
                            }
                        }
                        prompt.push(json!({ "type": "text", "text": text }));
                        let params = json!({ "sessionId": s.session_id, "prompt": prompt });
                        s.prompt = Some(agent.rpc.request("session/prompt", params).await?);
                    }
                    Command::Interrupt => {
                        agent.rpc.notify("session/cancel", json!({ "sessionId": s.session_id })).await?;
                        for (_, (rpc_id, _)) in s.perms.drain() {
                            agent.rpc.reply(rpc_id, Ok(permission_outcome(None))).await?;
                        }
                    }
                    Command::SetHandHolding(h) => {
                        hand_holding = h;
                        fs.full_access = h == HandHolding::FullAccess;
                    }
                    Command::SetModel { model, effort } => {
                        if let Some((method, params)) = model_request(&ctl, &s.session_id, &model) {
                            agent.rpc.request(method, params).await?;
                        }
                        if let Some((method, params)) = effort_request(&ctl, &s.session_id, effort) {
                            agent.rpc.request(method, params).await?;
                        }
                    }
                    Command::Respond { request_id, decision } => {
                        if let Some((rpc_id, options)) = s.perms.remove(&request_id) {
                            agent.rpc.reply(rpc_id, Ok(permission_outcome(pick_option(&options, decision)))).await?;
                        }
                    }
                    Command::Shutdown => break,
                }
            }
            line = agent.lines.next_line() => {
                let Some(line) = line? else {
                    if s.prompt.is_some() {
                        bail!(agent.exited());
                    }
                    break;
                };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if !s.handle(&v, &mut agent.rpc, &events, &fs, hand_holding).await? {
                    break;
                }
            }
        }
    }
    let _ = agent.child.start_kill();
    Ok(())
}

/// State of an open session between turns.
struct Live {
    session_id: String,
    turn: Turn,
    /// Permission prompts awaiting the user: our request id → (JSON-RPC id, options).
    perms: HashMap<String, (Value, Vec<Value>)>,
    prompt: Option<i64>,
}

impl Live {
    /// Handle one message from the agent. Returns false once the UI has gone away.
    async fn handle(
        &mut self,
        v: &Value,
        rpc: &mut Rpc,
        events: &async_channel::Sender<AgentEvent>,
        fs: &FsPolicy,
        hand_holding: HandHolding,
    ) -> Result<bool> {
        let out = match (v["method"].as_str(), v.get("id")) {
            (Some(method), Some(rpc_id)) => self.request(method, rpc_id.clone(), &v["params"], rpc, fs, hand_holding).await?,
            (Some("session/update"), None) if v["params"]["sessionId"] == self.session_id.as_str() => {
                self.turn.update(&v["params"]["update"])
            }
            (Some(_), None) => vec![],
            (None, _) => {
                if v["id"].as_i64().is_some() && v["id"].as_i64() == self.prompt {
                    self.prompt = None;
                    let stop = match v.get("error") {
                        Some(e) => Err(rpc_message(e)),
                        None => Ok(v["result"]["stopReason"].as_str().unwrap_or("end_turn")),
                    };
                    self.perms.clear();
                    self.turn.finish(stop)
                } else {
                    if let Some(e) = v.get("error") {
                        tracing::debug!("acp request failed: {}", rpc_message(e));
                    }
                    vec![]
                }
            }
        };
        for ev in out {
            if events.send(ev).await.is_err() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn request(
        &mut self,
        method: &str,
        rpc_id: Value,
        p: &Value,
        rpc: &mut Rpc,
        fs: &FsPolicy,
        hand_holding: HandHolding,
    ) -> Result<Vec<AgentEvent>> {
        if method != "session/request_permission" {
            let reply = fs.handle(method, p).await;
            rpc.reply(rpc_id, reply).await?;
            return Ok(vec![]);
        }
        let tc = &p["toolCall"];
        let known = tc["toolCallId"].as_str().and_then(|id| self.turn.tools.get(id));
        let kind = tc["kind"].as_str().or(known.map(|t| t.kind.as_str())).unwrap_or("other").to_string();
        let options = p["options"].as_array().cloned().unwrap_or_default();
        if auto_allow(hand_holding, &kind)
            && let Some(option) = pick_option(&options, Decision::Allow)
        {
            rpc.reply(rpc_id, Ok(permission_outcome(Some(option)))).await?;
            return Ok(vec![]);
        }
        let title = tc["title"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(String::from)
            .or(known.map(|t| t.title.clone()).filter(|t| !t.is_empty()))
            .unwrap_or_else(|| kind_title(&kind).into());
        let detail = Some(tool_detail(tc)).filter(|d| !d.is_empty()).or(known.map(|t| t.detail.clone())).unwrap_or_default();
        let request_id = format!("acp-{rpc_id}");
        self.perms.insert(request_id.clone(), (rpc_id, options));
        Ok(vec![AgentEvent::PermissionRequest { request_id, title, detail }])
    }
}

/// Start an ACP agent in the home folder, open a throwaway session and report its models and
/// login state. `id` is a catalog id, or `opencode` / `droid`.
pub async fn acp_probe(id: &str) -> Result<AcpInfo> {
    let agent_id = match id {
        "opencode" => AgentId::OpenCode,
        "droid" => AgentId::Droid,
        other => AgentId::Acp(other.into()),
    };
    let home = trek_core::paths::home();
    let probe = async {
        let mut agent = Agent::spawn(&agent_id, &home)?;
        let fs = FsPolicy { cwd: home.clone(), full_access: false };
        let mut backlog = Vec::new();
        let init = agent.handshake(&fs, &mut backlog).await?;
        let mut info = AcpInfo { auth_methods: auth_methods(&init), ..Default::default() };
        let params = json!({ "cwd": home.display().to_string(), "mcpServers": [] });
        match agent.call("session/new", params, &fs, &mut backlog, Duration::from_secs(20)).await? {
            Ok(r) => info.models = controls(&r).models,
            Err(e) if is_auth_error(&e) => info.needs_auth = true,
            Err(e) => bail!("{}: {}", agent.name, rpc_message(&e)),
        }
        let _ = agent.child.start_kill();
        Ok(info)
    };
    tokio::time::timeout(Duration::from_secs(20), probe).await.map_err(|_| anyhow!("{id} didn't respond in 20s"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_message_and_thought_chunks() {
        let mut t = Turn::default();
        let a = t.update(&(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"po"}})));
        let b = t.update(&(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ng"}})));
        let c = t.update(&(json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"hmm"}})));
        assert_eq!(a, vec![AgentEvent::TextDelta("po".into())]);
        assert_eq!(b, vec![AgentEvent::TextDelta("ng".into())]);
        assert_eq!(c, vec![AgentEvent::ReasoningDelta("hmm".into())]);
        assert!(t.update(&json!({"sessionUpdate":"plan","entries":[]})).is_empty());
        assert_eq!(
            t.finish(Ok("end_turn")),
            vec![AgentEvent::TextDone("pong".into()), AgentEvent::TurnComplete { cost_usd: None, error: None }]
        );
    }

    #[test]
    fn tool_calls_flush_text_and_finish() {
        let mut t = Turn::default();
        t.update(&json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Looking."}}));
        let ev = t.update(&json!({
            "sessionUpdate":"tool_call","toolCallId":"t1","title":"Read file","kind":"read","status":"pending",
            "locations":[{"path":"/p/src/main.rs"}]
        }));
        assert_eq!(
            ev,
            vec![
                AgentEvent::TextDone("Looking.".into()),
                AgentEvent::ToolStarted { id: "t1".into(), title: "Read file".into(), detail: "/p/src/main.rs".into() },
            ]
        );
        let ev = t.update(&json!({
            "sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed",
            "content":[{"type":"content","content":{"type":"text","text":"fn main() {}"}}]
        }));
        assert_eq!(ev, vec![AgentEvent::ToolFinished { id: "t1".into(), output: "fn main() {}".into(), ok: true }]);
    }

    #[test]
    fn bare_pending_tool_waits_for_input() {
        let mut t = Turn::default();
        assert!(t.update(&json!({"sessionUpdate":"tool_call","toolCallId":"b","title":"bash","kind":"execute","status":"pending","rawInput":{}})).is_empty());
        let ev = t.update(&json!({"sessionUpdate":"tool_call_update","toolCallId":"b","status":"in_progress","rawInput":{"command":"ls -la"}}));
        assert_eq!(ev, vec![AgentEvent::ToolStarted { id: "b".into(), title: "bash".into(), detail: "ls -la".into() }]);
        let ev = t.update(&json!({"sessionUpdate":"tool_call_update","toolCallId":"b","status":"failed","rawOutput":{"output":"boom"}}));
        assert_eq!(ev, vec![AgentEvent::ToolFinished { id: "b".into(), output: "boom".into(), ok: false }]);
    }

    #[test]
    fn stop_reasons_map_to_errors() {
        let err = |stop| match Turn::default().finish(stop).pop() {
            Some(AgentEvent::TurnComplete { error, .. }) => error,
            _ => panic!(),
        };
        assert_eq!(err(Ok("end_turn")), None);
        assert_eq!(err(Ok("cancelled")).as_deref(), Some("Interrupted"));
        assert!(err(Ok("refusal")).is_some());
        assert_eq!(err(Err("rate limited".into())).as_deref(), Some("rate limited"));
    }

    #[test]
    fn picks_permission_option_by_kind() {
        let options = vec![
            json!({"optionId":"once","name":"Allow","kind":"allow_once"}),
            json!({"optionId":"always","name":"Always","kind":"allow_always"}),
            json!({"optionId":"no","name":"Reject","kind":"reject_once"}),
        ];
        assert_eq!(pick_option(&options, Decision::Allow).as_deref(), Some("once"));
        assert_eq!(pick_option(&options, Decision::AllowForSession).as_deref(), Some("always"));
        assert_eq!(pick_option(&options, Decision::Deny).as_deref(), Some("no"));
        let only_once = vec![json!({"optionId":"ok","kind":"allow_once"}), json!({"optionId":"never","kind":"reject_always"})];
        assert_eq!(pick_option(&only_once, Decision::AllowForSession).as_deref(), Some("ok"));
        assert_eq!(pick_option(&only_once, Decision::Deny).as_deref(), Some("never"));
        assert_eq!(pick_option(&[], Decision::Allow), None);
        assert_eq!(permission_outcome(None), json!({"outcome":{"outcome":"cancelled"}}));
    }

    #[test]
    fn hand_holding_gates_auto_allow() {
        assert!(auto_allow(HandHolding::FullAccess, "execute"));
        assert!(auto_allow(HandHolding::AutoAcceptEdits, "edit"));
        assert!(!auto_allow(HandHolding::AutoAcceptEdits, "execute"));
        assert!(auto_allow(HandHolding::Auto, "read"));
        assert!(!auto_allow(HandHolding::Auto, "delete"));
        assert!(!auto_allow(HandHolding::Supervised, "read"));
    }

    #[test]
    fn reads_models_from_either_shape() {
        let acp_models = json!({
            "sessionId":"s",
            "models":{"currentModelId":"b","availableModels":[
                {"modelId":"a","name":"A","_meta":{"reasoningEfforts":[{"value":"high"},{"value":"low"}]}},
                {"modelId":"b","name":"B"}
            ]},
            "modes":{"availableModes":[{"id":"build","name":"Build"},{"id":"plan","name":"Plan"}]}
        });
        let c = controls(&acp_models);
        assert_eq!(c.switch, ModelSwitch::SetModel);
        assert_eq!(c.current_model.as_deref(), Some("b"));
        assert_eq!(c.models[0].efforts, vec![Effort::Low, Effort::High]);
        assert_eq!(c.plan_mode.as_deref(), Some("plan"));

        let config = json!({"sessionId":"s","configOptions":[
            {"id":"model","category":"model","type":"select","currentValue":"x/1","options":[
                {"value":"x/1","name":"X One"},{"group":"y","name":"Y","options":[{"value":"y/2","name":"Y Two"}]}
            ]},
            {"id":"thought_level","category":"thought_level","type":"select","currentValue":"max","options":[
                {"value":"medium","name":"Medium"},{"value":"max","name":"Max"}
            ]}
        ]});
        let c = controls(&config);
        assert_eq!(c.switch, ModelSwitch::Config("model".into()));
        assert_eq!(c.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["x/1", "y/2"]);
        let (method, params) = model_request(&c, "s", "y/2").unwrap();
        assert_eq!(method, "session/set_config_option");
        assert_eq!(params["value"], "y/2");
        let (_, params) = effort_request(&c, "s", Effort::High).unwrap();
        assert!(params["value"] == "medium" || params["value"] == "max");
        assert_eq!(effort_request(&c, "s", Effort::Low).unwrap().1["value"], "medium");
    }

    #[test]
    fn writes_stay_inside_cwd() {
        let root = std::env::temp_dir().join("trek-acp-within");
        std::fs::create_dir_all(&root).unwrap();
        assert!(within(&root.join("a/b.txt"), &root));
        assert!(!within(&root.join("../escape.txt"), &root));
        assert!(!within(Path::new("/etc/hosts"), &root));
    }

    #[test]
    fn auth_errors_are_recognised() {
        assert!(is_auth_error(&json!({"code":-32000,"message":"Authentication required"})));
        assert!(!is_auth_error(&json!({"code":-32603,"message":"Internal error"})));
    }
}

/// ACP stdio MCP server descriptors.
fn mcp_servers_json(servers: &[crate::McpServer]) -> Value {
    Value::Array(
        servers
            .iter()
            .map(|m| {
                json!({
                    "name": m.name,
                    "command": m.command,
                    "args": m.args,
                    "env": m.env.iter().map(|(k, v)| json!({ "name": k, "value": v })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}
