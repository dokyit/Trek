//! Agent Client Protocol (v1) agents: newline-delimited JSON-RPC 2.0 over the agent's stdio.
//! Covers OpenCode, Droid and every ACP agent in the catalog. Trek acts as the ACP client:
//! it answers permission prompts and `fs/*` requests; the agent keeps its own login.

use crate::{AgentEvent, Command, CommandKind, Decision, GroupChild, SessionConfig, SlashCommand, StderrTail, Step, clip, plan_row};
use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use trek_core::catalog::{ACP_AGENTS, ModelInfo};
use trek_core::{AgentId, Effort, HandHolding, TokenUsage, UsageCost, detect};

/// What `acp_probe` learns about an installed ACP agent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AcpInfo {
    pub models: Vec<ModelInfo>,
    /// `(id, name)` of the agent's advertised login methods.
    pub auth_methods: Vec<(String, String)>,
    /// The agent refused to open a session until the user signs in.
    pub needs_auth: bool,
}

/// Binary and arguments that start `agent` as an ACP server.
fn launch_spec(agent: &AgentId) -> Result<(PathBuf, Vec<String>, String)> {
    // Tests talk to a stand-in agent (fixtures/fake-acp.pl).
    #[cfg(test)]
    if *agent == AgentId::Acp(tests::FAKE.into()) {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/fake-acp.pl");
        return Ok((PathBuf::from("/usr/bin/perl"), vec![script.into()], "Fake".into()));
    }
    let (binary, args, name, hint): (&str, Vec<&str>, String, &str) = match agent {
        AgentId::OpenCode => (detect::OPENCODE, vec!["acp"], "OpenCode".into(), trek_core::catalog::agent_setup("opencode").map_or("curl -fsSL https://opencode.ai/install | bash", |s| s.install)),
        AgentId::Droid => ("droid", vec!["exec", "--output-format", "acp"], "Droid".into(), trek_core::catalog::agent_setup("droid").map_or("curl -fsSL https://app.factory.ai/cli | sh", |s| s.install)),
        AgentId::Acp(id) => match ACP_AGENTS.iter().find(|a| a.id == id) {
            Some(a) => (a.binary, a.args.to_vec(), a.name.into(), a.install_hint),
            // One the user added: its own command, as a path or a name on PATH.
            None => {
                let a = trek_core::catalog::added_agent(id).ok_or_else(|| anyhow!("Unknown ACP agent: {id}"))?;
                let path = a.resolve().ok_or_else(|| anyhow!("{} can't start: {}", a.name, a.missing()))?;
                return Ok((path, a.args, a.name));
            }
        },
        other => bail!("{} doesn't speak ACP", other.display_name()),
    };
    let found = if *agent == AgentId::OpenCode { detect::opencode() } else { detect::which(binary) };
    let path = found.with_context(|| format!("{name} isn't installed ({hint})"))?;
    Ok((path, args.into_iter().map(String::from).collect(), name))
}

/// Extra environment for the agent. OpenCode is told to ask before edits and commands (see
/// `opencode`); Trek then answers the prompts the thread's level covers (see `auto_allow`).
fn launch_env(agent: &AgentId, cwd: &Path) -> Vec<(String, String)> {
    match agent {
        AgentId::OpenCode => crate::opencode::launch_env(cwd),
        // An added agent's own variables, secrets from the Keychain.
        AgentId::Acp(id) => trek_core::catalog::added_agent(id).map(|a| a.launch_env()).unwrap_or_default(),
        _ => vec![],
    }
}

struct Agent {
    child: GroupChild,
    rpc: Rpc,
    lines: crate::ProtocolLines<BufReader<ChildStdout>>,
    stderr: StderrTail,
    name: String,
    bin: PathBuf,
    /// What to tell the user about where OpenCode keeps this session (see `opencode::db_env`).
    db_notice: Option<String>,
    _own_db: Option<crate::opencode::OwnInUse>,
    /// Not the install Trek normally starts (OpenCode 2 beside 1.x, for a session only 2 has).
    stand_in: bool,
}

impl Agent {
    /// Start `agent`; `resume`: the session it will be asked to reopen.
    async fn spawn(agent: &AgentId, cwd: &Path, extra: &[String], resume: Option<&str>) -> Result<Agent> {
        let (mut bin, mut args, name) = launch_spec(agent)?;
        args.extend(extra.iter().cloned());
        let mut stand_in = false;
        if *agent == AgentId::OpenCode
            && let Some(id) = resume
            && let Some(other) = crate::opencode::bin_for(&bin, id).await
        {
            bin = other;
            stand_in = true;
        }
        let mut env = launch_env(agent, cwd);
        let db = match agent {
            AgentId::OpenCode => crate::opencode::db_env(&bin, ProbeCache::stamp(&bin)).await,
            _ => Default::default(),
        };
        env.extend(db.env);
        // An agent the user added may be a `.cmd` given arguments they typed.
        if let Some(problem) = detect::batch_args_problem(&bin, &args) {
            bail!("{name} can't start: {problem}");
        }
        let mut command = tokio::process::Command::new(&bin);
        command
            .args(&args)
            .envs(env)
            .current_dir(cwd)
            .env("PATH", detect::login_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = crate::spawn_group(&mut command).with_context(|| format!("failed to start {}", bin.display()))?;
        let stderr = StderrTail::capture(child.stderr.take().unwrap(), "acp");
        let rpc = Rpc { stdin: child.stdin.take().unwrap(), next_id: 0 };
        let lines = crate::ProtocolLines::new(BufReader::new(child.stdout.take().unwrap()));
        Ok(Agent { child, rpc, lines, stderr, name, bin, db_notice: db.notice, _own_db: db.own, stand_in })
    }

    /// End the agent: its stdin first, the cue to exit and on Windows the only gentle one (see
    /// `GroupChild::terminate`), then the group.
    async fn stop(self) {
        let Agent { mut child, rpc, .. } = self;
        drop(rpc);
        child.terminate().await;
    }

    fn exited(&self) -> anyhow::Error {
        match crate::opencode::exit_reason(&self.stderr.lines()) {
            Some(why) => anyhow!(why),
            None => self.stderr.exited(&self.name),
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
/// session's folder unless the user granted full access, and there are none in a read-only one.
struct FsPolicy {
    cwd: PathBuf,
    full_access: bool,
    read_only: bool,
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
                if self.read_only {
                    return Err((-32003, format!("{} can't be changed: this session is read-only", path.display())));
                }
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

pub(crate) fn within(path: &Path, root: &Path) -> bool {
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

/// The `optionId` that best matches `decision`. Only an option of the same sense will do: with
/// none, the prompt is cancelled (which refuses it), so a "no" never picks an allow option.
fn pick_option(options: &[Value], decision: Decision) -> Option<String> {
    let kinds: &[&str] = match decision {
        Decision::Allow => &["allow_once", "allow_always"],
        Decision::AllowForSession => &["allow_always", "allow_once"],
        Decision::Deny => &["reject_once", "reject_always"],
    };
    kinds.iter().find_map(|k| options.iter().find(|o| o["kind"] == *k)).and_then(|o| o["optionId"].as_str()).map(String::from)
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

/// The row's detail: the command, else the file, else the most telling input. A command's
/// location is just its working directory, so it doesn't count.
fn tool_detail(tc: &Value, kind: &str) -> String {
    let input = &tc["rawInput"];
    if let Some(steps) = todo_steps(input) {
        return plan_row(&steps).0;
    }
    let command = match &input["command"] {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => Some(a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")),
        _ => input["cmd"].as_str().map(String::from),
    };
    let location = tc["locations"].as_array().and_then(|l| l.first()).and_then(|l| l["path"].as_str()).map(String::from);
    command
        .or(location.filter(|_| kind != "execute"))
        .or_else(|| {
            ["path", "file_path", "filePath", "filepath", "url", "query", "pattern", "description"]
                .iter()
                .find_map(|k| input[*k].as_str().map(String::from))
        })
        .filter(|s| !s.is_empty())
        .map(|s| clip(&s, 400))
        .unwrap_or_default()
}

/// A to-do tool's list (`rawInput.todos`, as OpenCode's todowrite sends it).
fn todo_steps(input: &Value) -> Option<Vec<(String, Step)>> {
    let todos = input["todos"].as_array().filter(|t| !t.is_empty())?;
    Some(todos.iter().map(|t| (t["content"].as_str().unwrap_or_default().to_string(), step(t["status"].as_str()))).collect())
}

fn step(status: Option<&str>) -> Step {
    match status {
        Some("completed") => Step::Done,
        Some("in_progress") => Step::Active,
        _ => Step::Pending,
    }
}

/// Rows read the same for every agent ("Run command", "Read", "Edit"); the agent's own title
/// is kept for tools without a standard kind.
fn row_title(tool: &Tool) -> String {
    match tool.kind.as_str() {
        _ if tool.todos.is_some() => "Update plan".to_string(),
        _ if tool.agent => "Subagent".to_string(),
        k @ ("read" | "edit" | "delete" | "move" | "search" | "execute" | "fetch") => kind_title(k).to_string(),
        k if tool.title.is_empty() => kind_title(k).to_string(),
        _ => tool.title.clone(),
    }
}

/// Lines the call's diffs add and remove (`content` entries of type `diff`), if it has any.
fn diff_lines(tc: &Value) -> Option<(u32, u32)> {
    let diffs: Vec<&Value> = tc["content"].as_array().into_iter().flatten().filter(|c| c["type"] == "diff").collect();
    (!diffs.is_empty()).then(|| {
        diffs.iter().fold((0, 0), |(a, r), d| {
            let (a2, r2) = crate::line_changes(d["oldText"].as_str().unwrap_or_default(), d["newText"].as_str().unwrap_or_default());
            (a + a2, r + r2)
        })
    })
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
    /// A to-do tool's list, shown as a checklist once it's done.
    todos: Option<Vec<(String, Step)>>,
    /// A sub-agent of the agent's own (OpenCode's `task`, `subagent` in OpenCode 2): it gets a
    /// sub-agent row, and its call's end is the sub-agent's.
    agent: bool,
    /// The tool's own name, where the agent says it (OpenCode 2: `shell`, `subagent`, …).
    name: String,
    started: bool,
}

/// A call that starts a sub-agent of the agent's own: OpenCode's `task` names the kind of agent
/// it sends (`subagent_type`) along with what to do; OpenCode 2's `subagent` tool names it `agent`.
fn is_sub_agent(name: &str, input: &Value) -> bool {
    input["prompt"].is_string() && (input["subagent_type"].is_string() || (name == "subagent" && input["agent"].is_string()))
}

/// The kind of agent a sub-agent call sends.
fn sub_agent_kind(input: &Value) -> Option<&str> {
    input["subagent_type"].as_str().or(input["agent"].as_str())
}

/// Turns `session/update` notifications into [`AgentEvent`]s for one session.
#[derive(Default)]
struct Turn {
    text: String,
    tools: HashMap<String, Tool>,
    plan_updates: u32,
    /// The session's running cost (`usage_update`) as last reported.
    cost: Option<f64>,
    /// The running cost when the last turn ended: what the next one adds is the difference.
    billed: Option<f64>,
    /// The session was loaded, and its running cost may carry its earlier turns: until a report
    /// before a turn says where it stands, a turn's share of it can't be told.
    resumed: bool,
    /// Tokens the turn used, by model (`None`: the session's own), and what they cost when the
    /// agent said: from the prompt responses' `usage` (agents that report it), or the agent's
    /// own history where that says more.
    tokens: Vec<(Option<String>, TokenUsage, Option<UsageCost>)>,
    /// The turn showed the user something: words (or thoughts), a tool call, a plan, or a
    /// request for approval.
    produced: bool,
    /// The user interrupted the turn: an agent may end it with `end_turn` and nothing said.
    interrupted: bool,
}

/// Whether a turn that ended without an error ended with nothing to show: no words, tool call,
/// plan, approval request or tokens, and not because the user cut it short. Some agents answer
/// a prompt this way when their model's provider can't (OpenCode, signed out).
fn ended_empty(error: Option<&str>, produced: bool, interrupted: bool) -> bool {
    error.is_none() && !produced && !interrupted
}

/// What the user is told when `ended_empty`: in OpenCode's case where to look, the others get
/// the general advice.
fn empty_turn_error(agent: &AgentId, name: &str, model: Option<&str>) -> String {
    match (agent, model) {
        (AgentId::OpenCode, Some(model)) => format!("{name} ended the turn without a reply. Check that its provider is signed in: `opencode run -m {model} hi` in a terminal should answer."),
        (AgentId::OpenCode, None) => format!("{name} ended the turn without a reply. Check that its provider is signed in: `opencode run hi` in a terminal should answer."),
        _ => format!("{name} ended the turn without a reply. Check that {name} is signed in and can answer when run in a terminal."),
    }
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
                    self.produced = true;
                    self.text.push_str(t);
                    out.push(AgentEvent::TextDelta(t.into()));
                }
            }
            Some("agent_thought_chunk") => {
                if let Some(t) = u["content"]["text"].as_str().filter(|t| !t.is_empty()) {
                    self.produced = true;
                    out.push(AgentEvent::ReasoningDelta(t.into()));
                }
            }
            Some(kind @ ("tool_call" | "tool_call_update")) => {
                self.produced = true;
                let id = u["toolCallId"].as_str().unwrap_or_default().to_string();
                let tool = self.tools.entry(id.clone()).or_default();
                // The row shows the title the call had when it started.
                if !tool.started
                    && let Some(t) = u["title"].as_str().filter(|t| !t.is_empty())
                {
                    tool.title = t.into();
                }
                if let Some(k) = u["kind"].as_str() {
                    tool.kind = k.into();
                }
                if let Some(n) = u["name"].as_str() {
                    tool.name = n.into();
                }
                if let Some(steps) = todo_steps(&u["rawInput"]) {
                    tool.todos = Some(steps);
                }
                tool.agent |= is_sub_agent(&tool.name, &u["rawInput"]);
                let detail = tool_detail(u, &tool.kind);
                if !detail.is_empty() {
                    tool.detail = detail;
                }
                let status = u["status"].as_str().unwrap_or(if kind == "tool_call" { "pending" } else { "" });
                let done = matches!(status, "completed" | "failed");
                // Hold a bare pending call until its input arrives, so the row has something to show.
                let start = !tool.started && (done || status == "in_progress" || !tool.detail.is_empty());
                if start {
                    tool.started = true;
                    let (title, detail, agent) = (row_title(tool), tool.detail.clone(), tool.agent);
                    self.flush_text(&mut out);
                    out.push(AgentEvent::ToolStarted { id: id.clone(), title, detail: detail.clone() });
                    if agent {
                        // ACP carries nothing of what it does meanwhile: what kind of agent it is.
                        let activity = sub_agent_kind(&u["rawInput"]).map(|k| format!("The {k} agent is at work"));
                        out.push(AgentEvent::Task { id: id.clone(), description: Some(detail), activity, tool_uses: None, done: None });
                    }
                }
                if done {
                    if self.tools.get(&id).is_some_and(|t| t.agent) {
                        out.push(AgentEvent::Task { id: id.clone(), description: None, activity: None, tool_uses: None, done: Some(status == "completed") });
                    }
                    if let Some((added, removed)) = diff_lines(u).filter(|_| status == "completed") {
                        out.push(AgentEvent::ToolLines { id: id.clone(), added, removed });
                    }
                    let output = match self.tools.remove(&id).and_then(|t| t.todos) {
                        Some(steps) if status == "completed" => plan_row(&steps).1,
                        _ => tool_output(u),
                    };
                    out.push(AgentEvent::ToolFinished { id, output, ok: status == "completed" });
                }
            }
            // The agent's plan (ACP `plan` entries), as a checklist row.
            Some("plan") => {
                let steps: Vec<(String, Step)> = u["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|e| (e["content"].as_str().unwrap_or_default().to_string(), step(e["status"].as_str())))
                    .collect();
                if !steps.is_empty() {
                    self.produced = true;
                    self.plan_updates += 1;
                    let id = format!("plan-{}", self.plan_updates);
                    let (detail, output) = plan_row(&steps);
                    self.flush_text(&mut out);
                    out.push(AgentEvent::ToolStarted { id: id.clone(), title: "Update plan".into(), detail });
                    out.push(AgentEvent::ToolFinished { id, output, ok: true });
                }
            }
            Some("usage_update") => {
                if u["cost"]["currency"] == "USD"
                    && let Some(c) = u["cost"]["amount"].as_f64()
                {
                    self.cost = Some(c);
                }
                // A cancelled turn reports 0 used; the window still holds the conversation.
                if let (Some(used), Some(window)) = (u["used"].as_u64().filter(|u| *u > 0), u["size"].as_u64()) {
                    out.push(AgentEvent::Context { used, window });
                }
            }
            Some("available_commands_update") => out.push(AgentEvent::Commands(
                u["availableCommands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| {
                        Some(SlashCommand {
                            name: c["name"].as_str()?.trim_start_matches('/').to_string(),
                            description: c["description"].as_str().unwrap_or_default().to_string(),
                            kind: CommandKind::Command,
                        })
                    })
                    .collect(),
            )),
            _ => {}
        }
        out
    }

    /// A turn is starting: a running cost reported before it is where it starts from.
    fn begin(&mut self) {
        if self.billed.is_none() {
            self.billed = self.cost;
        }
    }

    /// What the turn added to the session's running cost, when that can be told.
    fn turn_cost(&mut self) -> Option<f64> {
        let now = self.cost?;
        let added = match self.billed {
            // Counted again from zero (a new session behind the same id): all of it is new.
            Some(before) if now < before => now,
            Some(before) => now - before,
            None if self.resumed => {
                self.billed = Some(now);
                return None;
            }
            None => now,
        };
        self.billed = Some(now);
        Some(added)
    }

    fn finish(&mut self, stop: std::result::Result<&str, String>) -> Vec<AgentEvent> {
        self.finish_with(stop, None)
    }

    /// `finish`; `if_empty` is the error a turn that ended with nothing to show fails with.
    fn finish_with(&mut self, stop: std::result::Result<&str, String>, if_empty: Option<String>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        self.flush_text(&mut out);
        self.tools.clear();
        let failed = stop.is_err();
        let added = self.turn_cost();
        let mut tokens = std::mem::take(&mut self.tokens);
        let produced = std::mem::take(&mut self.produced) || tokens.iter().any(|(_, t, c)| !t.is_empty() || c.is_some());
        let interrupted = std::mem::take(&mut self.interrupted);
        // The session's cost goes with the turn's tokens when they're one lot priced by nobody
        // else; a cost with no tokens is reported on its own.
        if let Some(usd) = added.filter(|c| *c > 0.0) {
            match tokens.as_mut_slice() {
                [] => tokens.push((None, TokenUsage::default(), Some(UsageCost::reported(usd)))),
                [(_, _, cost @ None)] => *cost = Some(UsageCost::reported(usd)),
                _ => {}
            }
        }
        for (model, tokens, cost) in tokens {
            if !tokens.is_empty() || cost.is_some() {
                out.push(AgentEvent::Usage { model, tokens, cost });
            }
        }
        let error = match stop {
            Ok("cancelled") => Some("Interrupted".to_string()),
            Ok("refusal") => Some("The agent refused to continue.".to_string()),
            Ok("max_tokens") => Some("The agent hit its output limit.".to_string()),
            Ok("max_turn_requests") => Some("The agent hit its request limit for this turn.".to_string()),
            Ok(_) => None,
            Err(msg) => Some(msg),
        };
        // A cost added with no tokens counts as something done too (a priced turn isn't empty).
        let error = if ended_empty(error.as_deref(), produced || added.is_some_and(|c| c > 0.0), interrupted) { if_empty } else { error };
        // ACP has no word for a usage limit: agents pass on their provider's error as it reads.
        if let Some(limit) = error.as_deref().filter(|_| failed).and_then(|e| crate::Limit::from_text(e, trek_core::store::now_ms())) {
            out.push(limit.event());
        }
        out.push(AgentEvent::TurnComplete { error });
        out
    }
}

/// Count `used` toward `model`'s tokens in `tokens`, with what it cost if the agent said.
fn add_usage(tokens: &mut Vec<(Option<String>, TokenUsage, Option<UsageCost>)>, model: Option<String>, used: &TokenUsage, cost: Option<UsageCost>) {
    match tokens.iter_mut().find(|(m, _, _)| *m == model) {
        Some((_, t, c)) => {
            t.add(used);
            *c = match (*c, cost) {
                (Some(a), Some(b)) => Some(UsageCost::reported(a.usd + b.usd)),
                (a, b) => a.or(b),
            };
        }
        None => tokens.push((model, *used, cost)),
    }
}

/// A prompt response's `usage` (ACP's session usage: totals for the turn). Input is fresh
/// input: agents that count cache reads and writes inside it (Devin, as OpenAI does) have the
/// parts add up to `totalTokens` without them, and they're taken out. Thought tokens are
/// output, unless the agent already counted them there.
fn prompt_usage(u: &Value) -> Option<TokenUsage> {
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    let (input, output, read, written, thoughts) = (n("inputTokens"), n("outputTokens"), n("cachedReadTokens"), n("cachedWriteTokens"), n("thoughtTokens"));
    let total = u["totalTokens"].as_u64();
    let cache_inside = read + written > 0 && read + written <= input && (total == Some(input + output) || total == Some(input + output + thoughts));
    let mut t = TokenUsage { input: if cache_inside { input - read - written } else { input }, output, cache_read: read, cache_write: written };
    let counted = if cache_inside { input + output } else { input + output + read + written };
    if total != Some(counted) {
        t.output += thoughts;
    }
    (!t.is_empty()).then_some(t)
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

/// How an agent enters and leaves plan mode.
#[derive(Debug, Clone, PartialEq)]
enum PlanSwitch {
    /// `session/set_mode` (ACP `modes`): the plan mode id and the one to go back to.
    Mode { plan: String, off: String },
    /// `session/set_config_option` on a mode select (OpenCode's build/plan).
    Config { id: String, plan: String, off: String },
}

/// Session controls advertised in a `session/new` or `session/load` result.
#[derive(Debug, Clone)]
struct Controls {
    models: Vec<ModelInfo>,
    current_model: Option<String>,
    switch: ModelSwitch,
    /// Config option id and values for reasoning effort.
    effort: Option<(String, Vec<String>)>,
    /// The effort value the session is on.
    effort_current: Option<String>,
    plan_mode: Option<PlanSwitch>,
    /// The session is in plan mode right now.
    planning: bool,
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

/// The effort an agent's level stands for. "default" (the model's own, as OpenCode 2 offers it
/// beside the levels it names) is Trek's default, medium, unless the agent names a medium too.
fn effort_level(value: &str, values: &[String]) -> Option<Effort> {
    Effort::parse(value).or_else(|| (value == "default" && !values.iter().any(|v| Effort::parse(v) == Some(Effort::Medium))).then_some(Effort::Medium))
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
    let effort_option = by_category("thought_level").or_else(|| by_category("reasoning_effort"));
    let effort = effort_option.and_then(|o| Some((o["id"].as_str()?.to_string(), select_options(o).into_iter().map(|(v, _)| v).collect::<Vec<_>>())));
    let effort_current = effort_option.and_then(|o| o["currentValue"].as_str()).map(String::from);
    let shared_efforts = sorted_efforts(effort.iter().flat_map(|(_, v)| v.iter().filter_map(|s| effort_level(s, v))));

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

    let is_plan = |id: &str, name: Option<&str>| {
        let id = id.to_lowercase();
        id == "plan" || id.ends_with("#plan") || name.is_some_and(|n| n.eq_ignore_ascii_case("plan"))
    };
    let modes: Vec<&Value> = result["modes"]["availableModes"].as_array().into_iter().flatten().collect();
    let (plan_mode, planning) = if let Some(plan) = modes.iter().find(|m| is_plan(m["id"].as_str().unwrap_or_default(), m["name"].as_str())) {
        let plan = plan["id"].as_str().unwrap_or_default().to_string();
        let current = result["modes"]["currentModeId"].as_str().unwrap_or_default();
        let off = Some(current)
            .filter(|c| !c.is_empty() && *c != plan)
            .or_else(|| modes.iter().filter_map(|m| m["id"].as_str()).find(|id| *id != plan))
            .unwrap_or_default()
            .to_string();
        (Some(PlanSwitch::Mode { plan: plan.clone(), off }), current == plan)
    } else if let Some(o) = by_category("mode") {
        let values = select_options(o);
        match values.iter().find(|(v, n)| is_plan(v, Some(n))) {
            Some((plan, _)) => {
                let off = values.iter().map(|(v, _)| v.clone()).find(|v| v != plan).unwrap_or_default();
                let id = o["id"].as_str().unwrap_or("mode").to_string();
                (Some(PlanSwitch::Config { id, plan: plan.clone(), off }), o["currentValue"].as_str() == Some(plan.as_str()))
            }
            None => (None, false),
        }
    } else {
        (None, false)
    };

    Controls { models, current_model, switch, effort, effort_current, plan_mode, planning }
}

impl Controls {
    /// The session's options as a `session/set_config_option` response or a `config_option_update`
    /// has them now. Effort levels can depend on the model (OpenCode 2 offers each model's own), so
    /// a model switch brings new ones.
    fn refresh(&mut self, options: &Value) {
        if !options["configOptions"].is_array() {
            return;
        }
        let now = controls(options);
        self.effort = now.effort;
        self.effort_current = now.effort_current;
        if now.current_model.is_some() {
            self.current_model = now.current_model;
        }
        // Models listed as a config option share its levels; now they're the current model's.
        if matches!(self.switch, ModelSwitch::Config(_)) {
            let efforts = self.efforts();
            if let Some(m) = self.models.iter_mut().find(|m| Some(&m.id) == self.current_model.as_ref()) {
                m.efforts = efforts;
            }
        }
    }

    /// The levels the effort option offers now.
    fn efforts(&self) -> Vec<Effort> {
        sorted_efforts(self.effort.iter().flat_map(|(_, v)| v.iter().filter_map(|s| effort_level(s, v))))
    }

    /// The session's model, the levels it offers and the one it's on, for the UI. Only for models
    /// listed as a config option: the effort option's levels are the current model's then.
    fn efforts_event(&self) -> Option<AgentEvent> {
        if !matches!(self.switch, ModelSwitch::Config(_)) {
            return None;
        }
        let model = self.current_model.clone()?;
        let values = self.effort.as_ref().map(|(_, v)| v.as_slice()).unwrap_or_default();
        let effort = self.effort_current.as_deref().and_then(|c| effort_level(c, values));
        Some(AgentEvent::Efforts { model, efforts: self.efforts(), effort })
    }
}

/// Request params that turn plan mode on or off.
fn plan_request(c: &Controls, session_id: &str, on: bool) -> Option<(&'static str, Value)> {
    match c.plan_mode.as_ref()? {
        PlanSwitch::Mode { plan, off } => {
            Some(("session/set_mode", json!({ "sessionId": session_id, "modeId": if on { plan } else { off } })))
        }
        PlanSwitch::Config { id, plan, off } => {
            Some(("session/set_config_option", json!({ "sessionId": session_id, "configId": id, "value": if on { plan } else { off } })))
        }
    }
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

/// Request params that set reasoning effort: the model's own level for it where it has one, else
/// the model's "default" where it offers that, else its nearest level.
fn effort_request(c: &Controls, session_id: &str, effort: Effort) -> Option<(&'static str, Value)> {
    let (id, values) = c.effort.as_ref()?;
    let levels: Vec<(Effort, &String)> = values.iter().filter_map(|v| effort_level(v, values).map(|e| (e, v))).collect();
    let value = match levels.iter().find(|(e, _)| *e == effort) {
        Some((_, v)) => *v,
        None => match values.iter().find(|v| *v == "default") {
            Some(v) => v,
            None => {
                let want = effort.clamp_to(&sorted_efforts(levels.iter().map(|(e, _)| *e)));
                levels.iter().find(|(e, _)| *e == want)?.1
            }
        },
    };
    Some(("session/set_config_option", json!({ "sessionId": session_id, "configId": id, "value": value })))
}

/// `effort_request`, unless the session is on that level already.
fn effort_change(c: &Controls, session_id: &str, effort: Effort) -> Option<(&'static str, Value)> {
    effort_request(c, session_id, effort).filter(|(_, p)| p["value"].as_str() != c.effort_current.as_deref())
}

/// The session's efforts, when they aren't what the UI was last told.
fn efforts_news(ctl: &Controls, told: &mut Option<AgentEvent>) -> Option<AgentEvent> {
    let ev = ctl.efforts_event()?;
    (told.as_ref() != Some(&ev)).then(|| {
        *told = Some(ev.clone());
        ev
    })
}

/// A `config_option_update` for this session: its options as they are now.
fn config_update<'a>(v: &'a Value, session_id: &str) -> Option<&'a Value> {
    let p = &v["params"];
    (v["method"] == "session/update" && p["sessionId"] == session_id && p["update"]["sessionUpdate"] == "config_option_update").then(|| &p["update"])
}

/// Run an ACP session for `config.agent` until shutdown or the agent exits.
pub async fn run(
    config: SessionConfig,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let mut agent = Agent::spawn(&config.agent, &config.cwd, &launch_flags(&config), config.resume.as_deref()).await?;
    let mut hand_holding = config.hand_holding;
    let mut fs = FsPolicy { cwd: config.cwd.clone(), full_access: hand_holding == HandHolding::FullAccess, read_only: config.read_only };
    let mut backlog = Vec::new();
    let init = agent.handshake(&fs, &mut backlog).await?;
    let cwd = config.cwd.display().to_string();
    let (mcp, skipped) = acp_mcp_servers(&config.mcp_servers, &init);
    if !skipped.is_empty() {
        tracing::info!("{}: no HTTP MCP support, leaving out {}", agent.name, skipped.join(", "));
    }
    let skipped_notice = skipped_mcp_notice(&config.agent, &agent.name, &skipped);
    let setup = Duration::from_secs(120);

    let mut opened = None;
    // The saved conversation can't be reopened: the session starts over, and the user is told.
    let mut lost = false;
    if let Some(id) = &config.resume {
        if init["agentCapabilities"]["loadSession"] == true {
            let params = json!({ "sessionId": id, "cwd": cwd, "mcpServers": mcp });
            match agent.call("session/load", params, &fs, &mut backlog, setup).await? {
                Ok(r) => {
                    drop_replay(&mut backlog);
                    opened = Some((id.clone(), r));
                }
                Err(e) => {
                    tracing::warn!("session/load failed, starting fresh: {}", rpc_message(&e));
                    lost = true;
                }
            }
        } else {
            lost = true;
        }
    }
    let (session_id, result) = match opened {
        Some(s) => s,
        None => match agent.call("session/new", json!({ "cwd": cwd, "mcpServers": mcp }), &fs, &mut backlog, setup).await? {
            Ok(r) => (r["sessionId"].as_str().context("session/new returned no sessionId")?.to_string(), r),
            Err(e) if is_auth_error(&e) => {
                ProbeCache::signed_out(&ProbeCache::dir(), &config.agent);
                bail!(auth_hint(&agent.name, &init))
            }
            Err(e) => bail!("{}: {}", agent.name, rpc_message(&e)),
        },
    };

    let mut ctl = controls(&result);
    let mut model = ctl.current_model.clone();
    let mut switched = None;
    if let Some(want) = config.model.as_ref().filter(|m| model.as_ref() != Some(*m))
        && let Some((method, params)) = model_request(&ctl, &session_id, want)
    {
        if let Ok(Ok(r)) = agent.call(method, params, &fs, &mut backlog, setup).await {
            model = Some(want.clone());
            switched = Some(r);
        }
    }
    // The effort levels are the model's, so they're read after it's chosen: what the agent said
    // before its answer, then the answer.
    for u in backlog.iter().filter_map(|v| config_update(v, &session_id)) {
        ctl.refresh(u);
    }
    if let Some(r) = switched {
        ctl.refresh(&r);
    }
    if config.effort != Effort::Off
        && let Some((method, params)) = effort_change(&ctl, &session_id, config.effort)
        && let Ok(r) = agent.call(method, params, &fs, &mut backlog, setup).await?
    {
        ctl.refresh(&r);
    }
    // What this session reports is what the next launch's probe would learn (of the install it
    // probes).
    if !agent.stand_in {
        ProbeCache::session_opened(&ProbeCache::dir(), &config.agent, &agent.bin, &ctl.models, auth_methods(&init));
    }
    // Into plan mode, or out of it when a resumed session was left there. A read-only session
    // works in plan mode too: the agent's own tools edit files without asking Trek.
    let plan = config.plan || config.read_only;
    let mut planning = ctl.planning;
    if plan != ctl.planning
        && let Some((method, params)) = plan_request(&ctl, &session_id, plan)
        && agent.call(method, params, &fs, &mut backlog, setup).await?.is_ok()
    {
        planning = plan;
    }
    // Plan mode was asked for and isn't on: prompts are refused rather than run with edits allowed.
    let no_plan = (plan && !planning).then(|| plan_refusal(&agent.name, ctl.plan_mode.is_some(), config.read_only));
    events.send(AgentEvent::Started { native_id: session_id.clone(), model: model.clone() }).await?;
    let mut told_efforts = None;
    if let Some(ev) = efforts_news(&ctl, &mut told_efforts) {
        events.send(ev).await?;
    }
    if let Some(notice) = skipped_notice {
        events.send(AgentEvent::Notice(notice)).await?;
    }
    if let Some(notice) = agent.db_notice.take() {
        events.send(AgentEvent::Notice(notice)).await?;
    }
    if lost {
        events.send(crate::lost_session(&agent.name)).await?;
    }

    let turn = Turn { resumed: config.resume.is_some() && !lost, ..Default::default() };
    let mut s = Live { session_id, turn, perms: HashMap::new(), prompts: vec![], planning, agent: config.agent.clone(), name: agent.name.clone(), model, turn_began: None };
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
                    Command::Prompt { .. } if let Some(why) = &no_plan => {
                        let _ = events.send(AgentEvent::TurnComplete { error: Some(why.clone()) }).await;
                    }
                    Command::Prompt { text, images } => {
                        let mut prompt = Vec::new();
                        for path in &images {
                            match crate::load_image(path) {
                                Ok((mime, data)) => prompt.push(json!({ "type": "image", "mimeType": mime, "data": data })),
                                Err(e) => { let _ = events.send(AgentEvent::Notice(format!("Image left out: {e:#}"))).await; }
                            }
                        }
                        prompt.push(json!({ "type": "text", "text": text }));
                        let params = json!({ "sessionId": s.session_id, "prompt": prompt });
                        // Sent mid-turn, it's another prompt open at once: the turn ends with the last.
                        if s.turn_began.is_none() {
                            s.turn.begin();
                        }
                        s.turn_began.get_or_insert_with(trek_core::store::now_ms);
                        s.prompts.push(agent.rpc.request("session/prompt", params).await?);
                    }
                    Command::Interrupt => {
                        s.turn.interrupted |= s.turn_began.is_some();
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
                        // Answered before the effort is set: the new model's levels come with it.
                        let wait = Duration::from_secs(30);
                        let mut backlog = vec![];
                        let mut refused = None;
                        // The last answer that had the options: newer than any update sent before it.
                        let mut latest = None;
                        if let Some((method, params)) = model_request(&ctl, &s.session_id, &model) {
                            match agent.call(method, params, &fs, &mut backlog, wait).await? {
                                Ok(r) => {
                                    ctl.refresh(&r);
                                    latest = Some(r).filter(|r| r["configOptions"].is_array());
                                    s.model = Some(model.clone());
                                }
                                Err(e) => refused = Some(AgentEvent::Error(format!("{} didn't switch to {model}: {}", agent.name, rpc_message(&e)))),
                            }
                        }
                        if let Some((method, params)) = effort_change(&ctl, &s.session_id, effort)
                            && let Ok(r) = agent.call(method, params, &fs, &mut backlog, wait).await?
                        {
                            ctl.refresh(&r);
                            latest = Some(r).filter(|r| r["configOptions"].is_array()).or(latest);
                        }
                        if let Some(ev) = refused
                            && events.send(ev).await.is_err()
                        {
                            break;
                        }
                        // What came in meanwhile.
                        let mut gone = false;
                        for v in backlog {
                            if let Some(u) = config_update(&v, &s.session_id) {
                                ctl.refresh(u);
                            } else if !s.handle(&v, &mut agent.rpc, &events, &fs, hand_holding).await? {
                                gone = true;
                                break;
                            }
                        }
                        if gone {
                            break;
                        }
                        if let Some(r) = latest {
                            ctl.refresh(&r);
                        }
                        // Told even when nothing changed: the thread may be on a level the model hasn't.
                        told_efforts = None;
                        if let Some(ev) = efforts_news(&ctl, &mut told_efforts) {
                            if !agent.stand_in {
                                ProbeCache::session_opened(&ProbeCache::dir(), &config.agent, &agent.bin, &ctl.models, auth_methods(&init));
                            }
                            if events.send(ev).await.is_err() {
                                break;
                            }
                        }
                    }
                    Command::Respond { request_id, decision } => {
                        if let Some((rpc_id, options)) = s.perms.remove(&request_id) {
                            agent.rpc.reply(rpc_id, Ok(permission_outcome(pick_option(&options, decision)))).await?;
                        }
                    }
                    // ACP has no background work to read or stop.
                    Command::Answer { .. } | Command::SetModes { .. } | Command::ReadTask { .. } | Command::StopTask { .. } => {}
                    Command::Shutdown => break,
                }
            }
            line = agent.lines.next_line() => {
                let Some(line) = line? else {
                    if !s.prompts.is_empty() {
                        bail!(agent.exited());
                    }
                    break;
                };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if let Some(u) = config_update(&v, &s.session_id) {
                    ctl.refresh(u);
                    if let Some(ev) = efforts_news(&ctl, &mut told_efforts)
                        && events.send(ev).await.is_err()
                    {
                        break;
                    }
                    continue;
                }
                if !s.handle(&v, &mut agent.rpc, &events, &fs, hand_holding).await? {
                    break;
                }
            }
        }
    }
    agent.stop().await;
    Ok(())
}

/// Why a prompt isn't sent when plan mode is on but the agent couldn't enter it.
fn plan_refusal(agent: &str, has_plan_mode: bool, read_only: bool) -> String {
    let why = if has_plan_mode { "didn't switch to plan mode" } else { "has no plan mode" };
    if read_only {
        format!("{agent} {why}, so it could change files, and this task is read-only. Ask another agent for advice.")
    } else {
        format!("{agent} {why}, so it could change files. Turn plan mode off to send this.")
    }
}

/// `session/load` replays the conversation as updates; the transcript already has it. Session
/// state (commands, usage) still counts.
fn drop_replay(backlog: &mut Vec<Value>) {
    backlog.retain(|v| {
        v["method"] != "session/update" || matches!(v["params"]["update"]["sessionUpdate"].as_str(), Some("available_commands_update" | "usage_update"))
    });
}

/// State of an open session between turns.
struct Live {
    session_id: String,
    turn: Turn,
    /// Permission prompts awaiting the user: our request id → (JSON-RPC id, options).
    perms: HashMap<String, (Value, Vec<Value>)>,
    /// `session/prompt` requests not answered yet. A message sent while a turn runs is a prompt
    /// of its own; agents merge it into the turn, queue it, or turn it down.
    prompts: Vec<i64>,
    /// In plan mode: edits are never approved on the user's behalf.
    planning: bool,
    agent: AgentId,
    /// The agent's name for the user, and the model the session is on (what a turn that ends
    /// with nothing says to check).
    name: String,
    model: Option<String>,
    /// When the running turn's first prompt went out (unix ms).
    turn_began: Option<i64>,
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
                if let Some(i) = v["id"].as_i64().and_then(|id| self.prompts.iter().position(|p| *p == id)) {
                    self.prompts.remove(i);
                    let stop = match v.get("error") {
                        Some(e) => Err(rpc_message(e)),
                        None => Ok(v["result"]["stopReason"].as_str().unwrap_or("end_turn")),
                    };
                    // Each prompt answered reports its own; messages that joined the turn add theirs.
                    if let Some(used) = prompt_usage(&v["result"]["usage"]) {
                        add_usage(&mut self.turn.tokens, None, &used, None);
                    }
                    if self.prompts.is_empty() {
                        self.perms.clear();
                        if let Some(began) = self.turn_began.take()
                            && self.agent == AgentId::OpenCode
                        {
                            // OpenCode's `usage` is the turn's last step (one model call) alone; its
                            // own history has every step, sub-agents' included, and what each cost
                            // (nothing, for models it has no price for).
                            let id = self.session_id.clone();
                            let steps = tokio::task::spawn_blocking(move || trek_core::import::opencode::usage_priced(&id, began, i64::MAX)).await.unwrap_or_default();
                            if !steps.is_empty() {
                                self.turn.tokens.clear();
                                for (_, model, tokens, cost) in steps {
                                    add_usage(&mut self.turn.tokens, model, &tokens, cost.filter(|c| *c > 0.0).map(UsageCost::reported));
                                }
                            }
                        }
                        self.turn.finish_with(stop, Some(empty_turn_error(&self.agent, &self.name, self.model.as_deref())))
                    } else {
                        // Others are still open: the turn goes on. A message the agent turned down
                        // didn't reach it, so the user is told.
                        match stop {
                            Err(e) => vec![AgentEvent::Notice(format!("A message sent during the turn wasn't taken: {e}"))],
                            Ok(_) => vec![],
                        }
                    }
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
        self.turn.produced = true;
        match self.turn.permission(p, hand_holding, self.planning) {
            Ask::Answer(option) => {
                rpc.reply(rpc_id, Ok(permission_outcome(Some(option)))).await?;
                Ok(vec![])
            }
            Ask::User { title, detail, options } => {
                let request_id = format!("acp-{rpc_id}");
                self.perms.insert(request_id.clone(), (rpc_id, options));
                Ok(vec![AgentEvent::PermissionRequest { request_id, title, detail, prompt: None }])
            }
        }
    }
}

/// What to do with a `session/request_permission`.
#[derive(Debug, PartialEq)]
enum Ask {
    /// The thread's access level covers it: pick this option.
    Answer(String),
    User { title: String, detail: String, options: Vec<Value> },
}

impl Turn {
    /// `planning`: the session is in plan mode, so an edit the agent asks to make (its own rules
    /// would normally forbid it) goes to the user whatever the access level.
    fn permission(&self, p: &Value, hand_holding: HandHolding, planning: bool) -> Ask {
        let tc = &p["toolCall"];
        let known = tc["toolCallId"].as_str().and_then(|id| self.tools.get(id));
        let kind = tc["kind"].as_str().or(known.map(|t| t.kind.as_str())).unwrap_or("other").to_string();
        let options = p["options"].as_array().cloned().unwrap_or_default();
        let edit = matches!(kind.as_str(), "edit" | "delete" | "move");
        if auto_allow(hand_holding, &kind)
            && !(planning && edit)
            && let Some(option) = pick_option(&options, Decision::Allow)
        {
            return Ask::Answer(option);
        }
        let title = tc["title"].as_str().filter(|t| !t.is_empty()).map(String::from).or(known.map(|t| t.title.clone())).unwrap_or_default();
        let title = row_title(&Tool { title, kind: kind.clone(), ..Default::default() });
        let detail = Some(tool_detail(tc, &kind)).filter(|d| !d.is_empty()).or(known.map(|t| t.detail.clone())).unwrap_or_default();
        Ask::User { title, detail, options }
    }
}

/// Models and login state for an installed ACP agent. ACP only reports them for an open
/// session, and agents keep every session they open in their history, so this answers from what
/// the agent reported last time (its last probe, or the last session Trek opened with it). Only
/// a new install, a new version, or a missing sign-in opens a throwaway session, which is
/// deleted again where the agent allows it. `id` is a catalog id, or `opencode` / `droid`.
pub async fn acp_probe(id: &str) -> Result<AcpInfo> {
    let agent_id = match id {
        "opencode" => AgentId::OpenCode,
        "droid" => AgentId::Droid,
        other => AgentId::Acp(other.into()),
    };
    let (bin, _, _) = launch_spec(&agent_id)?;
    let cache = ProbeCache::dir();
    if let Some(info) = ProbeCache::load(&cache, &agent_id, &bin).filter(|i| !i.needs_auth) {
        return Ok(info);
    }
    let home = trek_core::paths::home();
    // The session the probe opened, as soon as the agent says: it's deleted however the probe ends.
    let mut opened: Option<String> = None;
    let probe = async {
        let mut agent = Agent::spawn(&agent_id, &home, &[], None).await?;
        let fs = FsPolicy { cwd: home.clone(), full_access: false, read_only: false };
        let mut backlog = Vec::new();
        let init = agent.handshake(&fs, &mut backlog).await?;
        let mut info = AcpInfo { auth_methods: auth_methods(&init), ..Default::default() };
        let params = json!({ "cwd": home.display().to_string(), "mcpServers": [] });
        match agent.call("session/new", params, &fs, &mut backlog, Duration::from_secs(30)).await? {
            Ok(r) => {
                info.models = controls(&r).models;
                opened = r["sessionId"].as_str().map(String::from);
                // Deleted through ACP where the agent offers it (OpenCode 2, whose CLI would start
                // its background server to do it).
                if let Some(session) = opened.clone().filter(|_| deletes_sessions(&init))
                    && let Ok(Ok(_)) = agent.call("session/delete", json!({ "sessionId": session }), &fs, &mut backlog, Duration::from_secs(10)).await
                {
                    opened = None;
                }
            }
            Err(e) if is_auth_error(&e) => info.needs_auth = true,
            Err(e) => bail!("{}: {}", agent.name, rpc_message(&e)),
        }
        agent.stop().await;
        if info.models.is_empty() && agent_id == AgentId::Acp("github-copilot".into()) {
            info.models = copilot_models().await;
        }
        Ok(info)
    };
    // Room for each step's own limit (initialize 30s, session/new 30s), so a slow cold start
    // fails at a step rather than partway through one.
    let probed = tokio::time::timeout(Duration::from_secs(90), probe).await;
    if let Some(session) = opened {
        discard_session(&agent_id, &session, &home).await;
    }
    let info = probed.map_err(|_| anyhow!("{id} didn't respond in 90s"))??;
    ProbeCache::store(&cache, &agent_id, &bin, &info);
    Ok(info)
}

/// The agent deletes sessions over ACP (`session/delete`, not yet part of ACP itself).
fn deletes_sessions(init: &Value) -> bool {
    init["agentCapabilities"]["sessionCapabilities"]["delete"].is_object()
}

/// Delete a session Trek opened only to look at it, through the agent's own CLI. Agents
/// without a way to do that keep it.
async fn discard_session(agent: &AgentId, session: &str, cwd: &Path) {
    let (bin, args): (Option<PathBuf>, [&str; 2]) = match agent {
        AgentId::OpenCode => (detect::opencode(), ["session", "delete"]),
        _ => return,
    };
    let Some(bin) = bin else { return };
    // Held until it's done: the session is in the file this names.
    let db = crate::opencode::db_env(&bin, ProbeCache::stamp(&bin)).await;
    let mut command = tokio::process::Command::new(bin);
    command.args(args).arg(session).current_dir(cwd).envs(db.env).env("PATH", detect::login_path());
    match crate::output_group(&mut command, None, Duration::from_secs(20)).await {
        Ok(out) if out.status.success() => {}
        Ok(out) => tracing::warn!("couldn't delete probe session {session}: {}", String::from_utf8_lossy(&out.stderr).trim()),
        Err(e) => tracing::warn!("couldn't delete probe session {session}: {e}"),
    }
}

/// Which install of an agent `bin` is (see `ProbeCache`).
pub(crate) fn stamp(bin: &Path) -> String {
    ProbeCache::stamp(bin)
}

/// The last thing each ACP agent reported, one file per agent under Trek's data folder. It's
/// tied to the agent's binary: a reinstall or new version is probed afresh.
#[derive(Debug, Serialize, Deserialize)]
struct ProbeCache {
    binary: String,
    info: AcpInfo,
}

impl ProbeCache {
    fn dir() -> PathBuf {
        trek_core::paths::data_dir().join("acp-agents")
    }

    fn path(dir: &Path, agent: &AgentId) -> PathBuf {
        let name: String = agent.key().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
        dir.join(format!("{name}.json"))
    }

    /// Which binary this is: its real path, size and modification time.
    fn stamp(bin: &Path) -> String {
        let real = bin.canonicalize().unwrap_or_else(|_| bin.to_path_buf());
        let meta = std::fs::metadata(&real).ok();
        let modified = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        format!("{}:{}:{modified}", real.display(), meta.map_or(0, |m| m.len()))
    }

    fn read(dir: &Path, agent: &AgentId) -> Option<ProbeCache> {
        serde_json::from_str(&std::fs::read_to_string(Self::path(dir, agent)).ok()?).ok()
    }

    /// What's known about the agent at `bin`, if it's still the same binary.
    fn load(dir: &Path, agent: &AgentId, bin: &Path) -> Option<AcpInfo> {
        Self::read(dir, agent).filter(|c| c.binary == Self::stamp(bin)).map(|c| c.info)
    }

    fn store(dir: &Path, agent: &AgentId, bin: &Path, info: &AcpInfo) {
        ProbeCache { binary: Self::stamp(bin), info: info.clone() }.write(dir, agent);
    }

    /// Written whole, then moved into place: a probe and a session may write at once.
    fn write(&self, dir: &Path, agent: &AgentId) {
        let path = Self::path(dir, agent);
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        let saved = std::fs::create_dir_all(dir)
            .and_then(|_| std::fs::write(&tmp, serde_json::to_vec(self).unwrap_or_default()))
            .and_then(|_| std::fs::rename(&tmp, &path));
        if let Err(e) = saved {
            let _ = std::fs::remove_file(&tmp);
            tracing::debug!("couldn't save what {} reported: {e}", agent.key());
        }
    }

    /// A session opened: its model list is the agent's current one. Agents that list no models
    /// over ACP (Copilot) keep the list found another way.
    fn session_opened(dir: &Path, agent: &AgentId, bin: &Path, models: &[ModelInfo], auth_methods: Vec<(String, String)>) {
        if !models.is_empty() {
            Self::store(dir, agent, bin, &AcpInfo { models: models.to_vec(), auth_methods, needs_auth: false });
        }
    }

    /// The agent turned a session down for want of a sign-in: the next launch probes it again.
    fn signed_out(dir: &Path, agent: &AgentId) {
        if let Some(mut c) = Self::read(dir, agent).filter(|c| !c.info.needs_auth) {
            c.info.needs_auth = true;
            c.write(dir, agent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ACP agent id that runs fixtures/fake-acp.pl (see `launch_spec`).
    pub(super) const FAKE: &str = "test-fake";

    #[test]
    fn a_rewound_session_starts_over_with_a_recap() {
        // A rewind or fork of an ACP thread (`Reopen::Recap`): no session to resume, a recap.
        let dir = std::env::temp_dir().join(format!("trek-acp-recap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = SessionConfig {
            agent: AgentId::Acp(FAKE.into()),
            cwd: dir.clone(),
            model: None,
            effort: Effort::Off,
            hand_holding: HandHolding::Auto,
            plan: false,
            read_only: false,
            resume: None,
            resume_at: None,
            fork: false,
            recap: Some("User: remember APPLE\n\nAssistant: OK".into()),
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
        };
        let h = crate::start(config);
        trek_core::runtime().block_on(async {
            let turn = async || loop {
                match tokio::time::timeout(Duration::from_secs(20), h.events.recv()).await.expect("agent stalled").expect("agent exited") {
                    AgentEvent::TurnComplete { error, .. } => return error,
                    AgentEvent::Error(e) => panic!("{e}"),
                    _ => {}
                }
            };
            for text in ["what did I ask?", "and now?"] {
                h.commands.send(Command::Prompt { text: text.into(), images: vec![] }).await.unwrap();
                assert_eq!(turn().await, None);
            }
            h.commands.send(Command::Shutdown).await.unwrap();
        });
        let log: Vec<Value> = std::fs::read_to_string(dir.join("acp-log.jsonl")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        let methods: Vec<&str> = log.iter().filter_map(|m| m["method"].as_str()).collect();
        assert_eq!(methods, ["initialize", "session/new", "session/prompt", "session/prompt"]);
        let prompts: Vec<&str> = log.iter().filter(|m| m["method"] == "session/prompt").map(|m| m["params"]["prompt"][0]["text"].as_str().unwrap()).collect();
        assert!(prompts[0].contains("<recap>\nUser: remember APPLE\n\nAssistant: OK\n</recap>") && prompts[0].ends_with("what did I ask?"), "{}", prompts[0]);
        assert_eq!(prompts[1], "and now?", "only the first message carries it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_added_agent_starts_with_its_own_command_and_environment() {
        use trek_core::registry::{AddedAgent, EnvVar};
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/fake-acp.pl");
        let perl = AddedAgent {
            id: "perl-agent".into(),
            name: "Perl agent".into(),
            command: "/usr/bin/perl".into(),
            args: vec![script.into()],
            env: vec![EnvVar { name: "PERL_AGENT_MODE".into(), value: "quiet".into(), secret: false }],
            ..Default::default()
        };
        let on_path = AddedAgent { id: "on-path".into(), name: "On PATH".into(), command: "sh".into(), ..Default::default() };
        let gone = AddedAgent { id: "gone".into(), name: "Gone".into(), command: "trek-no-such-agent".into(), ..Default::default() };
        trek_core::catalog::set_added_agents(&[perl, on_path, gone]);
        let agent = AgentId::Acp("perl-agent".into());
        let (bin, args, name) = launch_spec(&agent).unwrap();
        assert_eq!((bin, args, name.as_str()), (PathBuf::from("/usr/bin/perl"), vec![script.to_string()], "Perl agent"));
        assert_eq!(launch_env(&agent, Path::new("/")), [("PERL_AGENT_MODE".to_string(), "quiet".to_string())]);
        assert_eq!(agent.display_name(), "Perl agent");
        assert!(launch_spec(&AgentId::Acp("on-path".into())).unwrap().0.ends_with("sh"), "a name is looked up on PATH");
        let err = launch_spec(&AgentId::Acp("gone".into())).unwrap_err().to_string();
        assert!(err.contains("trek-no-such-agent isn't on your PATH"), "{err}");
        assert!(launch_spec(&AgentId::Acp("never-added".into())).is_err());

        // And it runs a turn like any ACP agent.
        let dir = std::env::temp_dir().join(format!("trek-acp-added-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = SessionConfig {
            agent,
            cwd: dir.clone(),
            model: None,
            effort: Effort::Off,
            hand_holding: HandHolding::Auto,
            plan: false,
            read_only: false,
            resume: None,
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
        };
        let h = crate::start(config);
        trek_core::runtime().block_on(async {
            h.commands.send(Command::Prompt { text: "hello".into(), images: vec![] }).await.unwrap();
            loop {
                match tokio::time::timeout(Duration::from_secs(20), h.events.recv()).await.expect("agent stalled").expect("agent exited") {
                    AgentEvent::TurnComplete { error, .. } => break assert_eq!(error, None),
                    AgentEvent::Error(e) => panic!("{e}"),
                    _ => {}
                }
            }
            h.commands.send(Command::Shutdown).await.unwrap();
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_message_turned_down_mid_turn_leaves_the_turn_running() {
        let dir = std::env::temp_dir().join(format!("trek-acp-steer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        trek_core::paths::isolate(dir.join("data"));
        let config = SessionConfig {
            agent: AgentId::Acp(FAKE.into()),
            cwd: dir.clone(),
            model: None,
            effort: Effort::Off,
            hand_holding: HandHolding::Auto,
            plan: false,
            read_only: false,
            resume: None,
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
        };
        let h = crate::start(config);
        let seen = trek_core::runtime().block_on(async {
            let mut seen = vec![];
            // The second message is turned down at once, while the first is still running.
            for text in ["hold", "refuse"] {
                h.commands.send(Command::Prompt { text: text.into(), images: vec![] }).await.unwrap();
            }
            loop {
                let ev = tokio::time::timeout(Duration::from_secs(20), h.events.recv()).await.expect("agent stalled").expect("agent exited");
                let done = matches!(ev, AgentEvent::TurnComplete { .. });
                seen.push(ev);
                if done {
                    break;
                }
            }
            h.commands.send(Command::Shutdown).await.unwrap();
            seen
        });
        let notices: Vec<&String> = seen.iter().filter_map(|e| if let AgentEvent::Notice(n) = e { Some(n) } else { None }).collect();
        assert!(matches!(&notices[..], [n] if n.contains("a prompt is already running")), "{seen:?}");
        // The held prompt is answered with nothing said, so the turn that ends with it says so.
        assert!(matches!(seen.last(), Some(AgentEvent::TurnComplete { error: Some(e) }) if e.contains("ended the turn without a reply")), "the turn ends with the first prompt: {seen:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn switching_models_rereads_their_efforts() {
        let dir = std::env::temp_dir().join(format!("trek-acp-efforts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        trek_core::paths::isolate(dir.join("data"));
        // Levels per model, as OpenCode 2 has them: "default" among some, none for others.
        let models = r#"{"models":{"a":["low","medium","high"],"b":["high","default"],"c":[]},"current":"c"}"#;
        std::fs::write(dir.join("fake-acp-config.json"), models).unwrap();
        let config = SessionConfig {
            agent: AgentId::Acp(FAKE.into()),
            cwd: dir.clone(),
            model: Some("a".into()),
            effort: Effort::High,
            hand_holding: HandHolding::Auto,
            plan: false,
            read_only: false,
            resume: None,
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
        };
        let h = crate::start(config);
        let efforts = |model: &str, efforts: &[Effort], effort: Option<Effort>| AgentEvent::Efforts { model: model.into(), efforts: efforts.to_vec(), effort };
        let told = trek_core::runtime().block_on(async {
            let next = async || loop {
                match tokio::time::timeout(Duration::from_secs(20), h.events.recv()).await.expect("agent stalled").expect("agent exited") {
                    ev @ AgentEvent::Efforts { .. } => return ev,
                    AgentEvent::Error(e) => panic!("{e}"),
                    _ => {}
                }
            };
            // Started on its model, at the effort asked for: the levels are that model's.
            let mut told = vec![next().await];
            for (model, effort) in [("b", Effort::High), ("b", Effort::Low), ("c", Effort::Low), ("a", Effort::Max)] {
                h.commands.send(Command::SetModel { model: model.into(), effort }).await.unwrap();
                told.push(next().await);
            }
            h.commands.send(Command::Shutdown).await.unwrap();
            told
        });
        assert_eq!(
            told,
            vec![
                efforts("a", &[Effort::Low, Effort::Medium, Effort::High], Some(Effort::High)),
                // Kept where the new model has it.
                efforts("b", &[Effort::Medium, Effort::High], Some(Effort::High)),
                // Not there: the model's default.
                efforts("b", &[Effort::Medium, Effort::High], Some(Effort::Medium)),
                // No levels at all.
                efforts("c", &[], None),
                // No default either: the nearest.
                efforts("a", &[Effort::Low, Effort::Medium, Effort::High], Some(Effort::High)),
            ]
        );
        let log: Vec<Value> = std::fs::read_to_string(dir.join("acp-log.jsonl")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        let sets: Vec<String> = log
            .iter()
            .filter(|m| m["method"] == "session/set_config_option")
            .map(|m| format!("{}={}", m["params"]["configId"].as_str().unwrap(), m["params"]["value"].as_str().unwrap()))
            .collect();
        // B starts on its default, so going back to it at Low needs no effort change.
        assert_eq!(sets, ["model=a", "effort=high", "model=b", "effort=high", "model=b", "model=c", "model=a", "effort=high"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Run one fake-agent session with `servers` (resuming `resume` if given), the agent taking
    /// remote MCP servers when `http`; returns what the agent was sent and the session's events.
    fn mcp_session(tag: &str, servers: Vec<crate::McpServer>, http: bool, resume: Option<&str>) -> (Vec<Value>, Vec<AgentEvent>) {
        let dir = std::env::temp_dir().join(format!("trek-acp-mcp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        trek_core::paths::isolate(dir.join("data"));
        if http {
            std::fs::write(dir.join("fake-acp-mcp.json"), r#"{"http":true}"#).unwrap();
        }
        let config = SessionConfig {
            agent: AgentId::Acp(FAKE.into()),
            cwd: dir.clone(),
            model: None,
            effort: Effort::Off,
            hand_holding: HandHolding::Auto,
            plan: false,
            read_only: false,
            resume: resume.map(String::from),
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: servers,
            instructions: None,
            read_dirs: vec![],
        };
        let h = crate::start(config);
        let seen = trek_core::runtime().block_on(async {
            let mut seen = vec![];
            h.commands.send(Command::Prompt { text: "hi".into(), images: vec![] }).await.unwrap();
            loop {
                let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_secs(20), h.events.recv()).await else { break };
                let done = matches!(ev, AgentEvent::TurnComplete { .. } | AgentEvent::Exited);
                seen.push(ev);
                if done {
                    break;
                }
            }
            let _ = h.commands.send(Command::Shutdown).await;
            seen
        });
        let log = std::fs::read_to_string(dir.join("acp-log.jsonl")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        let _ = std::fs::remove_dir_all(&dir);
        (log, seen)
    }

    fn sent_mcp<'a>(log: &'a [Value], method: &str) -> &'a Value {
        &log.iter().find(|m| m["method"] == method).unwrap_or_else(|| panic!("no {method}: {log:?}"))["params"]["mcpServers"]
    }

    fn trek_servers() -> Vec<crate::McpServer> {
        vec![
            crate::McpServer::stdio("trek-computer", "/Applications/Trek.app/Contents/MacOS/trek-mcp", vec!["computer".into()], vec![("TREK_SOCKET".into(), "/tmp/s".into())]),
            crate::McpServer::http("figma-desktop", "http://127.0.0.1:3845/mcp", vec![]),
            crate::McpServer::http("linear", "https://mcp.linear.app/mcp", vec![("Authorization".into(), "Bearer t".into())]),
        ]
    }

    #[test]
    fn mcp_servers_go_to_acp_agents_as_the_spec_array() {
        // An agent that takes only stdio servers (the default): the remote ones are left out,
        // and the user is told once.
        let (log, seen) = mcp_session("stdio", trek_servers(), false, None);
        assert!(!seen.iter().any(|e| matches!(e, AgentEvent::Error(_))), "the fake agent accepted the servers: {seen:?}");
        assert_eq!(
            *sent_mcp(&log, "session/new"),
            json!([{"name":"trek-computer","command":"/Applications/Trek.app/Contents/MacOS/trek-mcp","args":["computer"],"env":[{"name":"TREK_SOCKET","value":"/tmp/s"}]}])
        );
        let notices: Vec<&String> = seen.iter().filter_map(|e| if let AgentEvent::Notice(n) = e { Some(n) } else { None }).collect();
        assert!(matches!(&notices[..], [n] if n.contains("figma-desktop, linear")), "{seen:?}");
        let (_, again) = mcp_session("stdio-again", trek_servers(), false, None);
        assert!(!again.iter().any(|e| matches!(e, AgentEvent::Notice(_))), "told once, not every session: {again:?}");

        // One that said `mcpCapabilities.http`: every server, the remote ones typed.
        let (log, seen) = mcp_session("http", trek_servers(), true, None);
        assert!(!seen.iter().any(|e| matches!(e, AgentEvent::Error(_) | AgentEvent::Notice(_))), "{seen:?}");
        let sent = sent_mcp(&log, "session/new");
        assert_eq!(sent.as_array().map(Vec::len), Some(3));
        assert_eq!(sent[1], json!({"type":"http","name":"figma-desktop","url":"http://127.0.0.1:3845/mcp","headers":[]}));
        assert_eq!(sent[2], json!({"type":"http","name":"linear","url":"https://mcp.linear.app/mcp","headers":[{"name":"Authorization","value":"Bearer t"}]}));

        // A resumed session sends the same on session/load.
        let (log, seen) = mcp_session("load", trek_servers(), true, Some("fake-9"));
        assert!(!seen.iter().any(|e| matches!(e, AgentEvent::Error(_))), "{seen:?}");
        assert_eq!(sent_mcp(&log, "session/load").as_array().map(Vec::len), Some(3));
        assert!(!log.iter().any(|m| m["method"] == "session/new"), "it was loaded, not started over");
    }

    #[test]
    fn the_fake_agent_turns_down_servers_in_the_wrong_shape() {
        // What the test above relies on: the stand-in checks `mcpServers` as the spec has it.
        // Claude's `{name: {command…}}` map is refused, as is an HTTP server it didn't ask for.
        let dir = std::env::temp_dir().join(format!("trek-acp-shape-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ask = |servers: Value| -> Value {
            let input = [json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}), json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/","mcpServers":servers}})]
                .iter()
                .map(|m| format!("{m}\n"))
                .collect::<String>();
            let mut child = std::process::Command::new("/usr/bin/perl")
                .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/fake-acp.pl"))
                .current_dir(&dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            std::io::Write::write_all(&mut child.stdin.take().unwrap(), input.as_bytes()).unwrap();
            let out = child.wait_with_output().unwrap();
            serde_json::from_str(String::from_utf8_lossy(&out.stdout).lines().last().unwrap()).unwrap()
        };
        let servers = trek_servers();
        let refused = |r: &Value, why: &str| r["error"]["message"].as_str().is_some_and(|m| m.contains(why));
        assert!(refused(&ask(crate::mcp_servers_json_with(&servers[..1], crate::BatchHandOff::CmdWrapper, false, |_| None).0), "must be an array"));
        let (http, _) = acp_mcp_servers(&servers[1..2], &json!({"agentCapabilities":{"mcpCapabilities":{"http":true}}}));
        assert!(refused(&ask(http), "http isn't supported"));
        let (stdio, skipped) = acp_mcp_servers(&servers, &json!({}));
        assert_eq!(skipped, ["figma-desktop", "linear"], "HTTP is off unless the agent says");
        assert_eq!(ask(stdio)["result"]["sessionId"], "fake-1");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
            vec![AgentEvent::TextDone("pong".into()), AgentEvent::TurnComplete { error: None }]
        );
    }

    #[test]
    fn edits_report_the_lines_their_diffs_change() {
        let mut t = Turn::default();
        t.update(&json!({"sessionUpdate":"tool_call","toolCallId":"e1","title":"Edit","kind":"edit","status":"in_progress","locations":[{"path":"/p/a.rs"}]}));
        let ev = t.update(&json!({
            "sessionUpdate":"tool_call_update","toolCallId":"e1","status":"completed",
            "content":[{"type":"diff","path":"/p/a.rs","oldText":"a\nb\n","newText":"a\nB\nc\n"},{"type":"diff","path":"/p/n.rs","oldText":null,"newText":"x\n"}]
        }));
        assert_eq!(ev[0], AgentEvent::ToolLines { id: "e1".into(), added: 3, removed: 1 });
        assert!(matches!(&ev[1], AgentEvent::ToolFinished { ok: true, .. }));
    }

    #[test]
    fn an_opencode_task_is_a_sub_agent_of_its_own() {
        // Shapes from OpenCode's ACP adapter (`acp/tool.ts`): its `task` tool is of kind "think",
        // titled with the description, and its input names the kind of agent it sends.
        let mut t = Turn::default();
        let input = json!({"description":"Map the routes","prompt":"List every HTTP route.","subagent_type":"explore"});
        assert!(t.update(&json!({"sessionUpdate":"tool_call","toolCallId":"k1","title":"task","kind":"think","status":"pending","rawInput":{}})).is_empty());
        let ev = t.update(&json!({"sessionUpdate":"tool_call_update","toolCallId":"k1","status":"in_progress","kind":"think","title":"Map the routes","rawInput":input}));
        assert_eq!(
            ev,
            vec![
                AgentEvent::ToolStarted { id: "k1".into(), title: "Subagent".into(), detail: "Map the routes".into() },
                AgentEvent::Task { id: "k1".into(), description: Some("Map the routes".into()), activity: Some("The explore agent is at work".into()), tool_uses: None, done: None },
            ]
        );
        let ev = t.update(&json!({"sessionUpdate":"tool_call_update","toolCallId":"k1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"Three routes."}}]}));
        assert_eq!(
            ev,
            vec![
                AgentEvent::Task { id: "k1".into(), description: None, activity: None, tool_uses: None, done: Some(true) },
                AgentEvent::ToolFinished { id: "k1".into(), output: "Three routes.".into(), ok: true },
            ]
        );
        // Another "think" call is just that.
        let ev = t.update(&json!({"sessionUpdate":"tool_call","toolCallId":"k2","title":"sequential","kind":"think","status":"in_progress","rawInput":{"thought":"hm"}}));
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Task { .. })), "{ev:?}");
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
                AgentEvent::ToolStarted { id: "t1".into(), title: "Read".into(), detail: "/p/src/main.rs".into() },
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
        assert_eq!(ev, vec![AgentEvent::ToolStarted { id: "b".into(), title: "Run command".into(), detail: "ls -la".into() }]);
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
    fn a_turn_that_ends_with_nothing_is_decided_from_what_it_produced() {
        // (error, produced, interrupted)
        assert!(ended_empty(None, false, false));
        assert!(!ended_empty(None, true, false), "something was shown");
        assert!(!ended_empty(None, false, true), "the user cut it short");
        assert!(!ended_empty(Some("rate limited"), false, false), "an error already says why");
    }

    #[test]
    fn an_empty_turn_fails_in_the_agents_name() {
        let opencode = empty_turn_error(&AgentId::OpenCode, "OpenCode", Some("opencode/big-pickle"));
        assert_eq!(opencode, "OpenCode ended the turn without a reply. Check that its provider is signed in: `opencode run -m opencode/big-pickle hi` in a terminal should answer.");
        assert!(empty_turn_error(&AgentId::OpenCode, "OpenCode", None).contains("`opencode run hi`"));
        let other = empty_turn_error(&AgentId::Acp("goose".into()), "Goose", Some("x"));
        assert_eq!(other, "Goose ended the turn without a reply. Check that Goose is signed in and can answer when run in a terminal.");
    }

    #[test]
    fn a_turn_with_nothing_in_it_ends_with_the_error_and_one_with_something_does_not() {
        let said = Some("X ended the turn without a reply.".to_string());
        let ends = |t: &mut Turn, stop| match t.finish_with(stop, said.clone()).pop() {
            Some(AgentEvent::TurnComplete { error }) => error,
            other => panic!("{other:?}"),
        };
        // OpenCode 1.2.27 with a provider that can't answer: end_turn, no tokens, no words.
        let mut t = Turn::default();
        assert_eq!(ends(&mut t, Ok("end_turn")), said);
        // Whatever it shows, the turn is not empty.
        let shows = [
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}),
            json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"hm"}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"a","title":"ls","kind":"execute","status":"in_progress"}),
            json!({"sessionUpdate":"plan","entries":[{"content":"a","status":"pending"}]}),
        ];
        for update in shows {
            let mut t = Turn::default();
            t.update(&update);
            assert_eq!(ends(&mut t, Ok("end_turn")), None, "{update}");
        }
        // Empty message chunks, unknown updates and a plan with no steps are not output.
        let mut t = Turn::default();
        for update in [json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":""}}), json!({"sessionUpdate":"plan","entries":[]}), json!({"sessionUpdate":"available_commands_update","availableCommands":[]})] {
            t.update(&update);
        }
        assert_eq!(ends(&mut t, Ok("end_turn")), said);
        // Tokens used, an approval asked for, or the user's interrupt: none of them fail it.
        let mut t = Turn { tokens: vec![(None, TokenUsage { input: 5, ..Default::default() }, None)], ..Default::default() };
        assert_eq!(ends(&mut t, Ok("end_turn")), None);
        let mut t = Turn { produced: true, ..Default::default() };
        assert_eq!(ends(&mut t, Ok("end_turn")), None);
        let mut t = Turn { interrupted: true, ..Default::default() };
        assert_eq!(ends(&mut t, Ok("end_turn")), None);
        assert_eq!(ends(&mut Turn::default(), Ok("cancelled")).as_deref(), Some("Interrupted"));
        assert_eq!(ends(&mut Turn::default(), Err("boom".into())).as_deref(), Some("boom"));
        // The next turn starts clean.
        let mut t = Turn::default();
        t.update(&json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}));
        ends(&mut t, Ok("end_turn"));
        assert_eq!(ends(&mut t, Ok("end_turn")), said);
        // `finish` alone (no error to give) is unchanged.
        assert_eq!(Turn::default().finish(Ok("end_turn")), vec![AgentEvent::TurnComplete { error: None }]);
    }

    #[test]
    fn a_provider_limit_is_reported_before_the_turn_ends() {
        // As `rpc_message` reads an agent's JSON-RPC error that passes its provider's 429 on.
        let e = rpc_message(&json!({"code":-32603,"message":"Internal error","data":{"message":"Rate limit reached for requests. Please try again in 20s."}}));
        let ev = Turn::default().finish(Err(e.clone()));
        let [AgentEvent::LimitReached { message, resets_at: Some(at), scope }, AgentEvent::TurnComplete { error: Some(err), .. }] = &ev[..] else { panic!("{ev:?}") };
        assert_eq!((message, err, scope), (&e, &e, &crate::LimitScope::Other));
        assert!((*at - trek_core::store::now_ms() - 20_000).abs() < 5_000);
        assert_eq!(Turn::default().finish(Err("Internal error: model not found".into())).len(), 1);
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
        // Refusing is left to `cancelled`: a "no" never becomes a yes, nor a yes a no.
        let allow_only = vec![json!({"optionId":"yes","kind":"allow_once"}), json!({"optionId":"always","kind":"allow_always"})];
        assert_eq!(pick_option(&allow_only, Decision::Deny), None);
        let reject_only = vec![json!({"optionId":"no","kind":"reject_once"})];
        assert_eq!(pick_option(&reject_only, Decision::Allow), None);
        assert_eq!(pick_option(&reject_only, Decision::AllowForSession), None);
        let t = Turn::default();
        let ask = json!({"toolCall":{"toolCallId":"c1","kind":"edit","title":"Edit a.txt"},"options":reject_only});
        assert!(matches!(t.permission(&ask, HandHolding::FullAccess, false), Ask::User { .. }), "nothing to allow it with: the user decides");
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
        assert_eq!(c.plan_mode, Some(PlanSwitch::Mode { plan: "plan".into(), off: "build".into() }));
        assert!(!c.planning);

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

    /// Recorded ACP traffic (OpenCode 1.18.34, opencode/mimo-v2.6-flash-free), one message per line.
    fn fixture(text: &str) -> Vec<Value> {
        text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    fn updates(t: &mut Turn, lines: &[Value]) -> Vec<AgentEvent> {
        lines.iter().filter(|v| v["method"] == "session/update").flat_map(|v| t.update(&v["params"]["update"])).collect()
    }

    #[test]
    fn opencode_turn_maps_todos_reads_edits_usage_and_commands() {
        let lines = fixture(include_str!("../fixtures/opencode-turn.jsonl"));
        let mut t = Turn::default();
        let ev = updates(&mut t, &lines);
        assert_eq!(ev[0], AgentEvent::Commands(vec![
            SlashCommand { name: "init".into(), description: "guided AGENTS.md setup".into(), kind: CommandKind::Command },
            SlashCommand { name: "review".into(), description: "review changes [commit|branch|pr], defaults to uncommitted".into(), kind: CommandKind::Command },
        ]));
        let rows: Vec<(&str, &str)> =
            ev.iter().filter_map(|e| if let AgentEvent::ToolStarted { title, detail, .. } = e { Some((title.as_str(), detail.as_str())) } else { None }).collect();
        assert_eq!(
            rows,
            vec![
                ("Update plan", "Read notes"),
                ("Read", "/private/tmp/trek-agents-e2e/notes.txt"),
                ("Update plan", "Edit notes"),
                ("Edit", "/private/tmp/trek-agents-e2e/notes.txt"),
                ("Update plan", "All 2 steps done"),
            ]
        );
        assert!(ev.contains(&AgentEvent::ToolFinished { id: "call_a47ac35a1bd64360a32c5a34".into(), output: "→ Read notes\n○ Edit notes".into(), ok: true }));
        assert!(ev.contains(&AgentEvent::ToolFinished { id: "call_339ea6bf075f4a0286098c91".into(), output: "Edit applied successfully.\nEdited /private/tmp/trek-agents-e2e/notes.txt".into(), ok: true }));
        assert_eq!(ev.last(), Some(&AgentEvent::Context { used: 12761, window: 200000 }));
        assert_eq!(t.finish(Ok("end_turn")), vec![AgentEvent::TextDone("done".into()), AgentEvent::TurnComplete { error: None }]);

        // The edit prompt reads like the row it belongs to.
        let ask = lines.iter().find(|v| v["method"] == "session/request_permission").unwrap();
        let mut t = Turn::default();
        let upto = lines.iter().position(|v| v == ask).unwrap();
        updates(&mut t, &lines[..upto]);
        let Ask::User { title, detail, .. } = t.permission(&ask["params"], HandHolding::Supervised, false) else { panic!() };
        assert_eq!((title.as_str(), detail.as_str()), ("Edit", "/private/tmp/trek-agents-e2e/notes.txt"));
        assert_eq!(t.permission(&ask["params"], HandHolding::AutoAcceptEdits, false), Ask::Answer("once".into()));
        // In plan mode an edit is never approved on the user's behalf, whatever the level.
        for level in [HandHolding::AutoAcceptEdits, HandHolding::Auto, HandHolding::FullAccess] {
            assert!(matches!(t.permission(&ask["params"], level, true), Ask::User { .. }), "{level:?}");
        }
    }

    #[test]
    fn opencode_command_waits_for_its_command_and_asks() {
        let lines = fixture(include_str!("../fixtures/opencode-permission.jsonl"));
        let mut t = Turn::default();
        // The pending call only knows its working directory: no row yet.
        assert!(t.update(&lines[0]["params"]["update"]).is_empty());
        assert_eq!(
            t.update(&lines[1]["params"]["update"]),
            vec![AgentEvent::ToolStarted { id: "call_66c3b2d0ebfa47829d8d907f".into(), title: "Run command".into(), detail: "touch oc1.txt".into() }]
        );
        let ask = &lines[2]["params"];
        let Ask::User { title, detail, options } = t.permission(ask, HandHolding::Supervised, false) else { panic!() };
        assert_eq!((title.as_str(), detail.as_str()), ("Run command", "touch oc1.txt"));
        assert_eq!(pick_option(&options, Decision::Deny).as_deref(), Some("reject"));
        assert_eq!(pick_option(&options, Decision::AllowForSession).as_deref(), Some("always"));
        assert!(matches!(t.permission(ask, HandHolding::Auto, false), Ask::User { .. }), "Auto still asks before commands");
        assert_eq!(t.permission(ask, HandHolding::FullAccess, false), Ask::Answer("once".into()));
        // Plan mode keeps commands (OpenCode's plan agent may run them); only edits always ask.
        assert_eq!(t.permission(ask, HandHolding::FullAccess, true), Ask::Answer("once".into()));
        assert_eq!(
            t.update(&lines[3]["params"]["update"]),
            vec![AgentEvent::ToolFinished {
                id: "call_66c3b2d0ebfa47829d8d907f".into(),
                output: "The user rejected permission to use this specific tool call.".into(),
                ok: false
            }]
        );
    }

    #[test]
    fn opencode_2_offers_its_efforts_beside_models_and_plan() {
        // Recorded `session/new` (OpenCode 2.0.26): an effort select beside model and mode, with
        // "default" (the model's own) and the levels the model names.
        let c = controls(&serde_json::from_str(include_str!("../fixtures/opencode2-session-new.json")).unwrap());
        assert_eq!(c.switch, ModelSwitch::Config("model".into()));
        assert!(c.models.iter().any(|m| m.id == "opencode/mimo-v2.6-flash-free"));
        assert_eq!(c.effort, Some(("effort".into(), vec!["high".into(), "default".into()])));
        assert_eq!(c.models[0].efforts, vec![Effort::Medium, Effort::High], "default stands for medium");
        let value = |e| effort_request(&c, "s", e).unwrap().1["value"].clone();
        assert_eq!((value(Effort::Medium), value(Effort::High)), (json!("default"), json!("high")));
        // A level the model hasn't: its default.
        assert_eq!((value(Effort::Low), value(Effort::Max)), (json!("default"), json!("default")));
        // Already there: nothing to send.
        assert_eq!(effort_change(&c, "s", Effort::Medium), None);
        assert!(effort_change(&c, "s", Effort::High).is_some());
        assert_eq!(c.plan_mode, Some(PlanSwitch::Config { id: "mode".into(), plan: "plan".into(), off: "build".into() }));
        // A named medium is that, and "default" isn't a level then.
        assert_eq!(effort_level("default", &["medium".into(), "default".into()]), None);
    }

    #[test]
    fn opencode_2_turn_asks_before_its_shell() {
        // Recorded (OpenCode 2.0.26, with the ask rules Trek gives it): `bash` is `shell` now, and
        // still asks.
        let lines = fixture(include_str!("../fixtures/opencode2-turn.jsonl"));
        let ask = lines.iter().position(|v| v["method"] == "session/request_permission").unwrap();
        let mut t = Turn::default();
        let ev = updates(&mut t, &lines[..ask]);
        assert!(ev.contains(&AgentEvent::ToolStarted { id: "call_a63808222f7c4f9cb300824b".into(), title: "Run command".into(), detail: "ls".into() }), "{ev:?}");
        let Ask::User { title, detail, .. } = t.permission(&lines[ask]["params"], HandHolding::Supervised, false) else { panic!() };
        assert_eq!((title.as_str(), detail.as_str()), ("Run command", "ls"));
        assert_eq!(t.permission(&lines[ask]["params"], HandHolding::FullAccess, false), Ask::Answer("once".into()));
        let ev = updates(&mut t, &lines[ask + 1..]);
        assert!(ev.contains(&AgentEvent::ToolFinished { id: "call_a63808222f7c4f9cb300824b".into(), output: "notes.txt\n".into(), ok: true }), "{ev:?}");
        assert_eq!(t.finish(Ok("end_turn")), vec![AgentEvent::TextDone("1".into()), AgentEvent::TurnComplete { error: None }]);
        let used = prompt_usage(&lines.last().unwrap()["result"]["usage"]).unwrap();
        assert_eq!(used, TokenUsage { input: 135, output: 22 + 34, cache_read: 9728, cache_write: 0 });
    }

    #[test]
    fn an_opencode_2_subagent_is_a_sub_agent_of_its_own() {
        // OpenCode 2's tool is `subagent`, its input naming the `agent` it sends.
        let mut t = Turn::default();
        assert!(t.update(&json!({"sessionUpdate":"tool_call","toolCallId":"k1","name":"subagent","title":"subagent","kind":"other","status":"pending","rawInput":{}})).is_empty());
        let input = json!({"agent":"explore","description":"Map the routes","prompt":"List every HTTP route."});
        let ev = t.update(&json!({"sessionUpdate":"tool_call_update","toolCallId":"k1","status":"in_progress","title":"Map the routes","rawInput":input}));
        assert!(ev.contains(&AgentEvent::Task { id: "k1".into(), description: Some("Map the routes".into()), activity: Some("The explore agent is at work".into()), tool_uses: None, done: None }), "{ev:?}");
        // Another tool's `agent` and `prompt` don't make it one.
        let ev = t.update(&json!({"sessionUpdate":"tool_call","toolCallId":"k2","name":"skill","title":"skill","status":"in_progress","rawInput":{"agent":"x","prompt":"y"}}));
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Task { .. })), "{ev:?}");
    }

    #[test]
    fn opencode_2_deletes_sessions_over_acp() {
        let v2 = json!({"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"close":{},"delete":{},"fork":{},"list":{},"resume":{}}}});
        let v1 = json!({"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"close":{},"fork":{},"list":{},"resume":{}}}});
        assert!(deletes_sessions(&v2) && !deletes_sessions(&v1));
    }

    #[test]
    fn opencode_plan_mode_is_a_config_option() {
        let c = controls(&serde_json::from_str(include_str!("../fixtures/opencode-session-new.json")).unwrap());
        assert_eq!(c.switch, ModelSwitch::Config("model".into()));
        assert_eq!(c.current_model.as_deref(), Some("opencode/big-pickle"));
        assert_eq!(c.plan_mode, Some(PlanSwitch::Config { id: "mode".into(), plan: "plan".into(), off: "build".into() }));
        assert!(!c.planning);
        assert_eq!(plan_request(&c, "s", true), Some(("session/set_config_option", json!({"sessionId":"s","configId":"mode","value":"plan"}))));
        assert_eq!(plan_request(&c, "s", false).unwrap().1["value"], "build");
    }

    #[test]
    fn plan_entries_and_running_cost() {
        // ACP `plan` update shape (agent-client-protocol schema); OpenCode sends todos as a tool instead.
        let mut t = Turn::default();
        let ev = t.update(&json!({"sessionUpdate":"plan","entries":[
            {"content":"Read","priority":"high","status":"completed"},{"content":"Write","priority":"high","status":"in_progress"}]}));
        assert_eq!(
            ev,
            vec![
                AgentEvent::ToolStarted { id: "plan-1".into(), title: "Update plan".into(), detail: "Write".into() },
                AgentEvent::ToolFinished { id: "plan-1".into(), output: "✓ Read\n→ Write".into(), ok: true },
            ]
        );
        // `cost` is the session's running total: each turn is charged the difference.
        t.begin();
        t.update(&json!({"sessionUpdate":"usage_update","used":10,"size":100,"cost":{"amount":0.5,"currency":"USD"}}));
        assert!(t.update(&json!({"sessionUpdate":"usage_update","used":0,"size":100})).is_empty());
        t.update(&json!({"sessionUpdate":"usage_update","used":20,"size":100,"cost":{"amount":0.75,"currency":"USD"}}));
        t.tokens.push((None, TokenUsage { input: 10, ..Default::default() }, None));
        assert_eq!(
            t.finish(Ok("end_turn")),
            vec![AgentEvent::Usage { model: None, tokens: TokenUsage { input: 10, ..Default::default() }, cost: Some(UsageCost::reported(0.75)) }, AgentEvent::TurnComplete { error: None }]
        );
        // A turn without a new report adds nothing; one with a report and no tokens, its cost.
        t.begin();
        assert_eq!(t.finish(Ok("end_turn")), vec![AgentEvent::TurnComplete { error: None }]);
        t.begin();
        t.update(&json!({"sessionUpdate":"usage_update","used":20,"size":100,"cost":{"amount":1.0,"currency":"USD"}}));
        assert_eq!(t.finish(Ok("end_turn")), vec![AgentEvent::Usage { model: None, tokens: TokenUsage::default(), cost: Some(UsageCost::reported(0.25)) }, AgentEvent::TurnComplete { error: None }]);
        // A loaded session's first report carries its earlier turns: unless it came before the
        // turn, the turn's share can't be told, and its tokens are priced from the table instead.
        let mut loaded = Turn { resumed: true, ..Default::default() };
        loaded.begin();
        loaded.update(&json!({"sessionUpdate":"usage_update","used":20,"size":100,"cost":{"amount":3.0,"currency":"USD"}}));
        loaded.tokens.push((None, TokenUsage { input: 10, ..Default::default() }, None));
        assert!(matches!(&loaded.finish(Ok("end_turn"))[0], AgentEvent::Usage { cost: None, .. }));
        loaded.begin();
        loaded.update(&json!({"sessionUpdate":"usage_update","used":20,"size":100,"cost":{"amount":3.5,"currency":"USD"}}));
        assert!(matches!(&loaded.finish(Ok("end_turn"))[0], AgentEvent::Usage { cost: Some(c), .. } if *c == UsageCost::reported(0.5)));
        let mut told = Turn { resumed: true, ..Default::default() };
        told.update(&json!({"sessionUpdate":"usage_update","used":20,"size":100,"cost":{"amount":3.0,"currency":"USD"}}));
        told.begin();
        told.update(&json!({"sessionUpdate":"usage_update","used":20,"size":100,"cost":{"amount":3.2,"currency":"USD"}}));
        assert!(matches!(&told.finish(Ok("end_turn"))[0], AgentEvent::Usage { cost: Some(c), .. } if (c.usd - 0.2).abs() < 1e-12));
    }

    #[test]
    fn opencode_is_told_to_ask() {
        // What's added depends on the user's own OpenCode config (see `opencode`); it's always
        // inline agent config, never the global permission override.
        let cwd = std::env::temp_dir();
        for (k, v) in launch_env(&AgentId::OpenCode, &cwd) {
            assert_eq!(k, "OPENCODE_CONFIG_CONTENT");
            assert!(serde_json::from_str::<Value>(&v).unwrap()["agent"].is_object());
        }
        assert!(launch_env(&AgentId::Droid, &cwd).is_empty());
    }

    #[test]
    fn a_reopened_session_drops_its_replayed_history() {
        // Recorded `session/load` (OpenCode 1.18.34): the conversation comes back as updates
        // before the response. Commands and usage are session state, so they stay.
        let lines = fixture(include_str!("../fixtures/opencode-load.jsonl"));
        let commands = fixture(include_str!("../fixtures/opencode-turn.jsonl"))
            .into_iter()
            .find(|v| v["params"]["update"]["sessionUpdate"] == "available_commands_update")
            .unwrap();
        let usage = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"usage_update","used":12132,"size":200000}}});
        let mut backlog = lines[..3].to_vec();
        backlog.extend([commands.clone(), usage.clone()]);
        drop_replay(&mut backlog);
        assert_eq!(backlog, vec![commands, usage]);
        let c = controls(&lines[3]["result"]);
        assert_eq!(c.current_model.as_deref(), Some("opencode/mimo-v2.6-flash-free"));
        assert!(!c.planning);
    }

    #[test]
    fn a_resumed_session_left_in_plan_mode_is_taken_out_of_it() {
        // ACP `modes` (Copilot, Devin), reopened while in plan mode.
        let loaded = json!({"modes":{"currentModeId":"plan","availableModes":[{"id":"agent","name":"Agent"},{"id":"plan","name":"Plan"},{"id":"autopilot","name":"Autopilot"}]}});
        let c = controls(&loaded);
        assert!(c.planning);
        assert_eq!(c.plan_mode, Some(PlanSwitch::Mode { plan: "plan".into(), off: "agent".into() }));
        assert_eq!(plan_request(&c, "s", false), Some(("session/set_mode", json!({"sessionId":"s","modeId":"agent"}))));
        // Copilot's mode ids are URLs.
        let copilot = json!({"modes":{"currentModeId":"https://agentclientprotocol.com/protocol/session-modes#agent","availableModes":[
            {"id":"https://agentclientprotocol.com/protocol/session-modes#agent","name":"Agent"},{"id":"https://agentclientprotocol.com/protocol/session-modes#plan","name":"Plan"}]}});
        let c = controls(&copilot);
        assert!(!c.planning);
        assert_eq!(plan_request(&c, "s", true).unwrap().1["modeId"], "https://agentclientprotocol.com/protocol/session-modes#plan");
    }

    #[test]
    fn plan_mode_that_cant_be_had_is_refused() {
        assert_eq!(plan_refusal("Grok", false, false), "Grok has no plan mode, so it could change files. Turn plan mode off to send this.");
        assert!(plan_refusal("Devin", true, false).starts_with("Devin didn't switch to plan mode"));
        assert!(plan_refusal("Grok", false, true).ends_with("this task is read-only. Ask another agent for advice."));
        assert_eq!(controls(&json!({"sessionId":"s"})).plan_mode, None);
    }

    #[test]
    fn a_read_only_session_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("trek-acp-ro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        let write = json!({ "path": file.display().to_string(), "content": "x" });
        trek_core::runtime().block_on(async {
            let ro = FsPolicy { cwd: dir.clone(), full_access: true, read_only: true };
            let err = ro.handle("fs/write_text_file", &write).await.unwrap_err();
            assert!(err.1.contains("read-only"), "{err:?}");
            assert!(!file.exists());
            let rw = FsPolicy { cwd: dir.clone(), full_access: false, read_only: false };
            assert!(rw.handle("fs/write_text_file", &write).await.is_ok());
            assert_eq!(ro.handle("fs/read_text_file", &json!({ "path": file.display().to_string() })).await.unwrap()["content"], "x", "reading is fine");
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_results_are_kept_per_binary() {
        let dir = std::env::temp_dir().join(format!("trek-acp-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("agent-bin");
        std::fs::write(&bin, "v1").unwrap();
        let agent = AgentId::Acp("grok".into());
        assert_eq!(ProbeCache::load(&dir, &agent, &bin), None);
        let model = |id: &str| ModelInfo { id: id.into(), name: id.into(), efforts: vec![], tier: 0, fast: None };
        let info = AcpInfo { models: vec![model("a")], auth_methods: vec![("login".into(), "Log in".into())], needs_auth: false };
        ProbeCache::store(&dir, &agent, &bin, &info);
        assert!(dir.join("acp_grok.json").exists());
        assert_eq!(ProbeCache::load(&dir, &agent, &bin), Some(info.clone()));
        // A session's list replaces it; one with no models (Copilot) leaves it alone.
        ProbeCache::session_opened(&dir, &agent, &bin, &[model("b")], vec![]);
        assert_eq!(ProbeCache::load(&dir, &agent, &bin).unwrap().models, vec![model("b")]);
        ProbeCache::session_opened(&dir, &agent, &bin, &[], vec![]);
        assert_eq!(ProbeCache::load(&dir, &agent, &bin).unwrap().models, vec![model("b")]);
        // Turned away for want of a sign-in: still cached, but marked so the probe runs again.
        ProbeCache::signed_out(&dir, &agent);
        assert!(ProbeCache::load(&dir, &agent, &bin).unwrap().needs_auth);
        // Another binary (an upgrade) isn't the one that was probed.
        std::fs::write(&bin, "version 2").unwrap();
        assert_eq!(ProbeCache::load(&dir, &agent, &bin), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auth_errors_are_recognised() {
        assert!(is_auth_error(&json!({"code":-32000,"message":"Authentication required"})));
        assert!(!is_auth_error(&json!({"code":-32603,"message":"Internal error"})));
    }

    #[test]
    fn prompt_responses_report_the_turns_tokens() {
        // As OpenCode answers `session/prompt`: its message's tokens, reasoning apart.
        let u = json!({"totalTokens":12663,"inputTokens":12618,"outputTokens":2,"thoughtTokens":3,"cachedReadTokens":40,"cachedWriteTokens":0});
        assert_eq!(prompt_usage(&u), Some(TokenUsage { input: 12618, output: 5, cache_read: 40, cache_write: 0 }));
        // Thoughts already inside the output count: the parts add up without them.
        let inside = json!({"totalTokens":120,"inputTokens":100,"outputTokens":20,"thoughtTokens":5});
        assert_eq!(prompt_usage(&inside).map(|t| t.output), Some(20));
        assert_eq!(prompt_usage(&Value::Null), None);
        let mut t = Turn { tokens: vec![(None, prompt_usage(&u).unwrap(), None)], ..Default::default() };
        let ev = t.finish(Ok("end_turn"));
        assert!(matches!(&ev[..], [AgentEvent::Usage { model: None, tokens, cost: None }, AgentEvent::TurnComplete { .. }] if tokens.total() == 12663));
        // Devin (CLI 3000.11.3) counts its cache writes inside the input, as OpenAI does: here
        // on gpt-5-6-luna-medium, 10,837 of the 10,840 input tokens were written to the cache.
        let devin = json!({"totalTokens":10845,"inputTokens":10840,"outputTokens":5,"cachedWriteTokens":10837});
        assert_eq!(prompt_usage(&devin), Some(TokenUsage { input: 3, output: 5, cache_read: 0, cache_write: 10837 }));
        let swe = json!({"totalTokens":11882,"inputTokens":11842,"outputTokens":40});
        assert_eq!(prompt_usage(&swe), Some(TokenUsage { input: 11842, output: 40, ..Default::default() }));
    }
}

/// `mcpServers` for session/new and session/load: an array, unlike Claude's and Codex's maps.
/// Stdio servers are `{name, command, args, env: [{name, value}]}`; remote ones `{type: "http",
/// name, url, headers: [{name, value}]}`, sent only to an agent whose `initialize` said it takes
/// them (`agentCapabilities.mcpCapabilities.http`, false unless said). Returns the names of the
/// remote servers it left out.
fn acp_mcp_servers<'a>(servers: &'a [crate::McpServer], init: &Value) -> (Value, Vec<&'a str>) {
    let pairs = |p: &[(String, String)]| p.iter().map(|(k, v)| json!({ "name": k, "value": v })).collect::<Vec<_>>();
    let http = init["agentCapabilities"]["mcpCapabilities"]["http"] == true;
    let mut skipped = vec![];
    let mut out = vec![];
    for m in servers {
        match &m.transport {
            crate::McpTransport::Stdio { command, args, env } => out.push(json!({ "name": m.name, "command": command, "args": args, "env": pairs(env) })),
            crate::McpTransport::Http { url, headers } if http => out.push(json!({ "type": "http", "name": m.name, "url": url, "headers": pairs(headers) })),
            crate::McpTransport::Http { .. } => skipped.push(m.name.as_str()),
        }
    }
    (Value::Array(out), skipped)
}

/// What the user is told about remote MCP servers `agent` couldn't be given: once per agent and
/// server each time Trek runs, not at every session. `None` when there's nothing new to say.
fn skipped_mcp_notice(agent: &AgentId, name: &str, skipped: &[&str]) -> Option<String> {
    static TOLD: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::LazyLock::new(Default::default);
    let mut told = TOLD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let new: Vec<&str> = skipped.iter().copied().filter(|s| told.insert(format!("{}/{s}", agent.key()))).collect();
    if new.is_empty() {
        return None;
    }
    let (list, them) = if new.len() == 1 { (new[0].to_string(), "it") } else { (new.join(", "), "them") };
    Some(format!("{name} can't connect to remote MCP servers, so this session runs without {list}. Claude Code and Codex can use {them}."))
}

/// Agents that take their model on the command line rather than over ACP.
fn launch_flags(config: &SessionConfig) -> Vec<String> {
    let mut out = vec![];
    if config.agent == AgentId::Acp("github-copilot".into()) {
        if let Some(m) = &config.model {
            out.extend(["--model".to_string(), m.clone()]);
        }
        let effort = match config.effort {
            trek_core::Effort::Off => "none",
            trek_core::Effort::Minimal => "minimal",
            trek_core::Effort::Low => "low",
            trek_core::Effort::Medium => "medium",
            trek_core::Effort::High => "high",
            trek_core::Effort::XHigh => "xhigh",
            trek_core::Effort::Max => "max",
        };
        out.extend(["--reasoning-effort".to_string(), effort.to_string()]);
    }
    out
}

/// Copilot doesn't list models over ACP; its `help config` does.
async fn copilot_models() -> Vec<ModelInfo> {
    let Some(bin) = detect::which("copilot") else { return vec![] };
    let mut command = tokio::process::Command::new(bin);
    command.args(["help", "config"]).env("PATH", detect::login_path());
    let Ok(out) = crate::output_group(&mut command, None, Duration::from_secs(20)).await else {
        return vec![];
    };
    parse_copilot_models(&String::from_utf8_lossy(&out.stdout))
}

fn parse_copilot_models(help: &str) -> Vec<ModelInfo> {
    let mut in_model = false;
    let mut out = vec![ModelInfo { id: "auto".into(), name: "Auto".into(), efforts: vec![], tier: 0, fast: None }];
    for line in help.lines() {
        if line.trim_start().starts_with("`model`:") {
            in_model = true;
            continue;
        }
        if in_model {
            match line.trim().strip_prefix("- \"").and_then(|l| l.strip_suffix('"')) {
                Some(id) => out.push(ModelInfo { id: id.to_string(), name: pretty_model(id), efforts: vec![], tier: 1, fast: None }),
                None if line.trim().is_empty() || line.trim_start().starts_with('-') => {}
                None => break,
            }
        }
    }
    if out.len() == 1 { vec![] } else { out }
}

/// "claude-opus-4.8-fast" → "Claude Opus 4.8 Fast", "gpt-5.3-codex" → "GPT-5.3 Codex".
fn pretty_model(id: &str) -> String {
    let mut parts: Vec<String> = vec![];
    for p in id.split('-') {
        let w = match p {
            "gpt" => "GPT".to_string(),
            "mai" => "MAI".to_string(),
            p if p.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
                if parts.last().is_some_and(|l| l == "GPT") {
                    let last = parts.pop().unwrap();
                    format!("{last}-{p}")
                } else {
                    p.to_string()
                }
            }
            p => {
                let mut c = p.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        };
        parts.push(w);
    }
    parts.join(" ")
}

#[cfg(test)]
mod copilot_tests {
    use super::*;

    #[test]
    fn parses_copilot_model_list() {
        let help = "  `model`: AI model to use\n    - \"claude-opus-4.8-fast\"\n    - \"gpt-5.3-codex\"\n\n  `contextTier`: x";
        let m = parse_copilot_models(help);
        assert_eq!(m.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["Auto", "Claude Opus 4.8 Fast", "GPT-5.3 Codex"]);
    }
}
