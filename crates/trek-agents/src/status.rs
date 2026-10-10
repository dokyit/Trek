//! Account, plan, usage limits, slash commands and models for the vendor CLIs, read without
//! sending a prompt (free). Claude: `initialize` + `get_usage` + `get_context_usage` control
//! requests on an idle stream-json session. Codex: `account/read`, `account/rateLimits/read`,
//! `skills/list` (once `plugin/reconcile` answers) and `model/list` on `codex app-server`.
//! Devin: `devin auth status`, and the quota its terminal UI shows for `/usage`.

use crate::codex::{Rpc, RpcLines, await_response, fetch_models, start_app_server};
use anyhow::{Context as _, Result};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use trek_core::catalog::ModelInfo;
use trek_core::{Effort, detect};

const TIMEOUT: Duration = Duration::from_secs(30);
/// How long Codex's skills wait for it to learn the account's plugins.
const PLUGINS_TIMEOUT: Duration = Duration::from_secs(10);

/// A granted, redeemable rate-limit reset ("Use reset" in Codex's Usage).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResetCredit {
    pub id: String,
    /// e.g. "Full reset (Weekly + 5 hr)".
    pub title: String,
    pub description: Option<String>,
    /// Unix ms when the credit lapses.
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UsageLimit {
    /// "5-hour limit", "Weekly limit", "Weekly · Fable", ...
    pub label: String,
    /// 0–100.
    pub percent: f32,
    /// Unix milliseconds.
    pub resets_at: Option<i64>,
    /// Window length: "5h", "7d", or e.g. "24h" / "90m".
    pub window: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    Command,
    Skill,
    Agent,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub kind: CommandKind,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentStatus {
    /// Email (or a description of the auth method).
    pub account: Option<String>,
    /// "Claude Max", "ChatGPT Plus", ...
    pub plan: Option<String>,
    pub limits: Vec<UsageLimit>,
    pub commands: Vec<SlashCommand>,
    pub models: Vec<ModelInfo>,
    pub logged_in: bool,
    /// How the login pays for tokens: a plan, or per token.
    pub billing: Option<crate::Billing>,
    /// Something the plan reports besides its limits (Devin's on-demand balance), as it said it.
    pub note: Option<String>,
    /// Granted but unspent rate-limit resets (Codex "Usage limit resets").
    pub resets: Vec<ResetCredit>,
    /// Partial failures (some data may still be present).
    pub error: Option<String>,
}

impl AgentStatus {
    fn add_error(&mut self, e: impl std::fmt::Display) {
        let e = e.to_string();
        self.error = Some(match self.error.take() {
            Some(prev) => format!("{prev}; {e}"),
            None => e,
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Claude

/// Status of the user's Claude Code login. Sends no prompt.
pub async fn claude_status(cwd: &Path) -> Result<AgentStatus> {
    claude_read(cwd, true).await
}

/// Claude Code's account, commands and models in `cwd`, without its plan's usage (no
/// `get_usage`): for when the Usage card doesn't show it. Sends no prompt.
pub async fn claude_commands(cwd: &Path) -> Result<AgentStatus> {
    claude_read(cwd, false).await
}

async fn claude_read(cwd: &Path, with_usage: bool) -> Result<AgentStatus> {
    let bin = detect::which("claude").context("Claude Code isn't installed (npm i -g @anthropic-ai/claude-code)")?;
    // In a group of its own: the MCP servers it starts go with it.
    let mut command = tokio::process::Command::new(bin);
    command
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-prompt-tool",
            "stdio",
        ])
        .current_dir(cwd)
        .env("PATH", detect::login_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = crate::spawn_group(&mut command).context("failed to start claude")?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut stderr = child.stderr.take().unwrap();
    let stderr_task = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    });

    let wanted: Vec<(&str, &str)> = [("i1", "initialize"), ("u1", "get_usage"), ("c1", "get_context_usage")].into_iter().filter(|(id, _)| with_usage || *id != "u1").collect();
    for &(id, subtype) in &wanted {
        let msg = json!({ "type": "control_request", "request_id": id, "request": { "subtype": subtype } });
        let mut s = serde_json::to_string(&msg)?;
        s.push('\n');
        stdin.write_all(s.as_bytes()).await?;
    }
    stdin.flush().await?;

    let mut responses: HashMap<String, Value> = HashMap::new();
    let mut status = AgentStatus::default();
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    while responses.len() < wanted.len() {
        match tokio::time::timeout_at(deadline, stdout.next_line()).await {
            Err(_) => {
                status.add_error("timed out waiting for Claude Code");
                break;
            }
            Ok(Err(e)) => {
                status.add_error(e);
                break;
            }
            Ok(Ok(None)) => {
                let _ = child.start_kill();
                let err = tokio::time::timeout(Duration::from_secs(2), stderr_task).await.ok().and_then(|r| r.ok()).unwrap_or_default();
                let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("Claude Code exited").trim().to_string();
                status.add_error(last);
                break;
            }
            Ok(Ok(Some(line))) => {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if v["type"] != "control_response" {
                    continue;
                }
                let r = &v["response"];
                let id = r["request_id"].as_str().unwrap_or_default().to_string();
                if r["subtype"] == "success" {
                    responses.insert(id, r["response"].clone());
                } else {
                    let name = wanted.iter().find(|(w, _)| *w == id).map(|(_, s)| *s).unwrap_or("request");
                    status.add_error(format!("{name}: {}", r["error"].as_str().unwrap_or("failed")));
                    responses.insert(id, Value::Null);
                }
            }
        }
    }
    // A probe: Claude has nothing to save, and it has been answered (or given up on).
    child.kill_now().await;

    let null = Value::Null;
    let init = responses.get("i1").unwrap_or(&null);
    let usage = responses.get("u1").unwrap_or(&null);
    let context = responses.get("c1").unwrap_or(&null);
    apply_claude_init(&mut status, init, context);
    status.billing = crate::claude::account_billing(&init["account"]);
    if status.plan.is_none() {
        status.plan = usage["subscription_type"].as_str().filter(|s| !s.is_empty()).map(|s| format!("Claude {}", capitalize(s)));
    }
    status.limits = claude_limits(usage);
    Ok(status)
}

/// Account, commands and models from the `initialize` response. `context` (the
/// `get_context_usage` response) lists which commands are skills.
fn apply_claude_init(status: &mut AgentStatus, init: &Value, context: &Value) {
    let account = &init["account"];
    status.account = account["email"].as_str().filter(|s| !s.is_empty()).map(String::from);
    status.plan = account["subscriptionType"].as_str().filter(|s| !s.is_empty()).map(String::from);
    status.logged_in = status.account.is_some() || status.plan.is_some() || account["apiKeySource"].is_string();
    if status.account.is_none() {
        if let Some(src) = account["apiKeySource"].as_str() {
            status.account = Some(format!("API key ({src})"));
        }
    }

    let skills: Option<HashSet<&str>> = context["skills"]["skillFrontmatter"]
        .as_array()
        .map(|a| a.iter().filter_map(|s| s["name"].as_str()).collect());
    for c in init["commands"].as_array().into_iter().flatten() {
        let Some(name) = c["name"].as_str() else { continue };
        if name.starts_with("__") {
            continue;
        }
        let is_skill = match &skills {
            Some(set) => set.contains(name),
            // No skill list: plugin skills carry a namespaced alias; user skills aren't builtin.
            None => {
                c["builtin"] != true
                    || c["aliases"].as_array().into_iter().flatten().any(|a| a.as_str().is_some_and(|a| a.contains(':')))
            }
        };
        status.commands.push(SlashCommand {
            name: name.to_string(),
            description: c["description"].as_str().unwrap_or_default().to_string(),
            kind: if is_skill { CommandKind::Skill } else { CommandKind::Command },
        });
    }
    for a in init["agents"].as_array().into_iter().flatten() {
        let Some(name) = a["name"].as_str() else { continue };
        status.commands.push(SlashCommand {
            name: name.to_string(),
            description: a["description"].as_str().unwrap_or_default().to_string(),
            kind: CommandKind::Agent,
        });
    }
    status.models = claude_models(&init["models"]);
}

fn claude_models(models: &Value) -> Vec<ModelInfo> {
    let list = models.as_array().map(Vec::as_slice).unwrap_or_default();
    // "default" is an alias; list it last so the real model's name wins the dedupe.
    let ordered = list.iter().filter(|m| m["value"] != "default").chain(list.iter().filter(|m| m["value"] == "default"));
    let mut out: Vec<ModelInfo> = Vec::new();
    for m in ordered {
        let Some(id) = m["resolvedModel"].as_str().or(m["value"].as_str()) else { continue };
        if out.iter().any(|o| o.id == id) {
            continue;
        }
        let efforts: Vec<Effort> = m["supportedEffortLevels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e.as_str().and_then(Effort::parse))
            .collect();
        out.push(ModelInfo {
            id: id.to_string(),
            name: m["displayName"].as_str().unwrap_or(id).to_string(),
            efforts,
            tier: claude_tier(id),
            fast: (m["supportsFastMode"] == true).then(|| "settings".to_string()),
        });
    }
    out
}

fn claude_tier(id: &str) -> u8 {
    if id.contains("haiku") {
        0
    } else if id.contains("sonnet") {
        1
    } else if id.contains("fable") {
        3
    } else {
        2
    }
}

/// `get_usage` → `rate_limits.limits[]`.
fn claude_limits(usage: &Value) -> Vec<UsageLimit> {
    let mut out = Vec::new();
    for l in usage["rate_limits"]["limits"].as_array().into_iter().flatten() {
        let kind = l["kind"].as_str().unwrap_or_default();
        let (label, window) = match kind {
            "session" => ("5-hour limit".to_string(), "5h"),
            "weekly_all" => ("Weekly limit".to_string(), "7d"),
            "weekly_scoped" => {
                let scope = &l["scope"];
                let name = scope["model"]["display_name"]
                    .as_str()
                    .or(scope["surface"]["display_name"].as_str())
                    .or(scope["surface"].as_str());
                (name.map(|n| format!("Weekly · {n}")).unwrap_or_else(|| "Weekly (scoped)".into()), "7d")
            }
            other => {
                let group = l["group"].as_str().unwrap_or(other);
                (format!("{} limit", capitalize(&group.replace('_', " "))), group)
            }
        };
        out.push(UsageLimit {
            label,
            percent: l["percent"].as_f64().unwrap_or(0.0) as f32,
            resets_at: l["resets_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp_millis()),
            window: window.to_string(),
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Codex

/// Status of the user's Codex login. Sends no prompt.
pub async fn codex_status(cwd: &Path) -> Result<AgentStatus> {
    codex_read(cwd, true).await
}

/// Codex's account, skills and models in `cwd`, without its plan's usage (no
/// `account/rateLimits/read`): for when the Usage card doesn't show it. Sends no prompt.
pub async fn codex_commands(cwd: &Path) -> Result<AgentStatus> {
    codex_read(cwd, false).await
}

async fn codex_read(cwd: &Path, with_usage: bool) -> Result<AgentStatus> {
    let mut backlog = Vec::new();
    let (mut child, mut rpc, mut lines, _) =
        tokio::time::timeout(TIMEOUT, start_app_server(cwd, &[], &mut backlog)).await.context("codex app-server timed out")??;
    let mut status = AgentStatus::default();
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    // Codex learns which of its plugins the account installed (over the network) after it
    // starts, and lists their skills only then, without saying so: `plugin/reconcile` answers
    // once it knows. Asked first, it runs while the rest is read.
    let reconcile = rpc.request("plugin/reconcile", json!({ "reason": "trek" })).await;

    match call(&mut rpc, &mut lines, &mut backlog, deadline, "account/read", json!({})).await {
        Ok(r) => {
            apply_codex_account(&mut status, &r);
            status.billing = crate::codex::account_billing(&r);
        }
        Err(e) => status.add_error(format!("account: {e:#}")),
    }
    if with_usage && status.logged_in {
        match call(&mut rpc, &mut lines, &mut backlog, deadline, "account/rateLimits/read", Value::Null).await {
            Ok(r) => {
                status.limits = codex_limits(&r);
                status.resets = codex_reset_credits(&r);
            }
            // API-key accounts have no plan limits.
            Err(e) if status.plan.is_some() => status.add_error(format!("rate limits: {e:#}")),
            Err(_) => {}
        }
    }
    // Skills are listed regardless: without the account's plugins if it fails or takes long.
    if let Ok(id) = reconcile {
        let _ = response(&mut lines, &mut backlog, id, deadline.min(tokio::time::Instant::now() + PLUGINS_TIMEOUT)).await;
    }
    let cwd_s = cwd.display().to_string();
    match call(&mut rpc, &mut lines, &mut backlog, deadline, "skills/list", json!({ "cwds": [cwd_s] })).await {
        Ok(r) => status.commands = codex_skills(&r),
        Err(e) => status.add_error(format!("skills: {e:#}")),
    }
    match tokio::time::timeout_at(deadline, fetch_models(&mut rpc, &mut lines, &mut backlog)).await {
        Ok(Ok(m)) => status.models = m,
        Ok(Err(e)) => status.add_error(format!("models: {e:#}")),
        Err(_) => status.add_error("models: timed out"),
    }
    // A probe that opened no thread: Codex has nothing to save.
    child.kill_now().await;
    Ok(status)
}

/// Spend one granted reset credit; Usage refreshes right after via the caller.
pub async fn codex_consume_reset(cwd: &Path, credit_id: &str) -> Result<()> {
    let mut backlog = Vec::new();
    let (mut child, mut rpc, mut lines, _) =
        tokio::time::timeout(TIMEOUT, start_app_server(cwd, &[], &mut backlog)).await.context("codex app-server timed out")??;
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    call(
        &mut rpc,
        &mut lines,
        &mut backlog,
        deadline,
        "account/rateLimitResetCredit/consume",
        json!({ "creditId": credit_id, "idempotencyKey": trek_core::transcript::new_id() }),
    )
    .await?;
    // Codex has answered: the credit is spent, and there's nothing else to save.
    child.kill_now().await;
    Ok(())
}

fn codex_reset_credits(r: &Value) -> Vec<ResetCredit> {
    r["rateLimitResetCredits"]["credits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["status"].as_str() == Some("available"))
        .filter_map(|c| {
            let id = c["id"].as_str()?;
            Some(ResetCredit {
                id: id.to_string(),
                title: c["title"].as_str().unwrap_or("Usage limit reset").to_string(),
                description: c["description"].as_str().map(String::from),
                expires_at: c["expiresAt"].as_i64().map(|t| t * 1000),
            })
        })
        .collect()
}

async fn call(
    rpc: &mut Rpc,
    lines: &mut RpcLines,
    backlog: &mut Vec<Value>,
    deadline: tokio::time::Instant,
    method: &str,
    params: Value,
) -> Result<Value> {
    let id = if params.is_null() {
        rpc.next_id += 1;
        let id = rpc.next_id;
        rpc.send(&json!({ "id": id, "method": method })).await?;
        id
    } else {
        rpc.request(method, params).await?
    };
    tokio::time::timeout_at(deadline, await_response(lines, id, backlog)).await.context("timed out")?
}

/// The answer to request `id`, sent earlier: put aside while other answers were awaited, or
/// still to come.
async fn response<R: tokio::io::AsyncBufRead + Unpin>(lines: &mut crate::ProtocolLines<R>, backlog: &mut Vec<Value>, id: i64, deadline: tokio::time::Instant) -> Result<Value> {
    if let Some(i) = backlog.iter().position(|v| v["id"].as_i64() == Some(id) && v.get("method").is_none()) {
        let v = backlog.remove(i);
        if let Some(err) = v.get("error") {
            anyhow::bail!("codex: {}", err["message"].as_str().unwrap_or("request failed"));
        }
        return Ok(v["result"].clone());
    }
    tokio::time::timeout_at(deadline, await_response(lines, id, backlog)).await.context("timed out")?
}

fn apply_codex_account(status: &mut AgentStatus, r: &Value) {
    let a = &r["account"];
    match a["type"].as_str() {
        Some("chatgpt") => {
            status.logged_in = true;
            status.account = a["email"].as_str().map(String::from);
            status.plan = a["planType"].as_str().map(codex_plan_name);
        }
        Some("apiKey") => {
            status.logged_in = true;
            status.account = Some("API key".into());
        }
        Some("amazonBedrock") => {
            status.logged_in = true;
            status.account = Some("Amazon Bedrock".into());
        }
        Some(other) => {
            status.logged_in = true;
            status.account = Some(capitalize(other));
        }
        None => status.logged_in = false,
    }
}

pub(crate) fn codex_plan_name(plan: &str) -> String {
    match plan {
        "plus" => "ChatGPT Plus".into(),
        "pro" => "ChatGPT Pro".into(),
        "prolite" => "ChatGPT Pro Lite".into(),
        "promax" => "ChatGPT Pro Max".into(),
        "free" => "ChatGPT Free".into(),
        "go" => "ChatGPT Go".into(),
        "unknown" => "ChatGPT".into(),
        other => format!("ChatGPT {}", capitalize(&other.replace('_', " "))),
    }
}

/// `(label prefix, window)` for a window length in minutes.
fn codex_window(mins: Option<i64>) -> (String, String) {
    match mins {
        Some(300) => ("5-hour".into(), "5h".into()),
        Some(10080) => ("Weekly".into(), "7d".into()),
        Some(m) if m % 60 == 0 => (format!("{}h", m / 60), format!("{}h", m / 60)),
        Some(m) => (format!("{m}m"), format!("{m}m")),
        None => ("Usage".into(), String::new()),
    }
}

fn codex_limit(w: &Value, label: impl FnOnce(&str) -> String) -> Option<UsageLimit> {
    if !w.is_object() {
        return None;
    }
    let (prefix, window) = codex_window(w["windowDurationMins"].as_i64());
    Some(UsageLimit {
        label: label(&prefix),
        percent: w["usedPercent"].as_f64().unwrap_or(0.0) as f32,
        resets_at: w["resetsAt"].as_i64().map(|s| s * 1000),
        window,
    })
}

/// `account/rateLimits/read` → main `rateLimits.primary/secondary`, plus named extra limits.
fn codex_limits(r: &Value) -> Vec<UsageLimit> {
    let mut out = Vec::new();
    let main = &r["rateLimits"];
    for key in ["primary", "secondary"] {
        out.extend(codex_limit(&main[key], |p| format!("{p} limit")));
    }
    if let Some(by_id) = r["rateLimitsByLimitId"].as_object() {
        let main_id = main["limitId"].as_str().unwrap_or("codex");
        for (id, snap) in by_id {
            if id == main_id || id == "codex" {
                continue;
            }
            let Some(name) = snap["limitName"].as_str() else { continue };
            for key in ["primary", "secondary"] {
                out.extend(codex_limit(&snap[key], |p| format!("{p} · {name}")));
            }
        }
    }
    out
}

/// `skills/list` → enabled skills (deduped across cwds).
fn codex_skills(r: &Value) -> Vec<SlashCommand> {
    let mut out: Vec<SlashCommand> = Vec::new();
    for entry in r["data"].as_array().into_iter().flatten() {
        for s in entry["skills"].as_array().into_iter().flatten() {
            if s["enabled"] == false {
                continue;
            }
            let Some(name) = s["name"].as_str() else { continue };
            if out.iter().any(|c| c.name == name) {
                continue;
            }
            out.push(SlashCommand {
                name: name.to_string(),
                description: s["description"].as_str().unwrap_or_default().to_string(),
                kind: CommandKind::Skill,
            });
        }
    }
    out
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// Devin

/// Status of the user's Devin login: account and plan from `devin auth status`, and the plan's
/// daily and weekly quota from the `/usage` panel of Devin's terminal UI. Devin reports its
/// quota nowhere else (not over ACP, not with `--print`), so the UI runs in a pseudo-terminal
/// of its own, offscreen, in an empty folder; typing `/usage` there sends no prompt and starts
/// no session. Credentials stay with Devin: nothing of `auth status` but the email and the
/// plan's name is kept, and none of it is logged.
pub async fn devin_status() -> Result<AgentStatus> {
    #[cfg(windows)]
    let missing = "Devin isn't installed";
    #[cfg(not(windows))]
    let missing = "Devin isn't installed (curl -fsSL https://cli.devin.ai/install.sh | bash)";
    let bin = detect::which("devin").context(missing)?;
    let out = crate::output_group(tokio::process::Command::new(&bin).args(["auth", "status"]).env("PATH", detect::login_path()), None, TIMEOUT)
        .await
        .context("devin auth status failed")?;
    let mut status = devin_account(&String::from_utf8_lossy(&out.stdout));
    if !status.logged_in {
        return Ok(status);
    }
    let dir = std::env::temp_dir().join("trek-devin-usage");
    std::fs::create_dir_all(&dir)?;
    match tokio::task::spawn_blocking(move || devin_usage_screen(&bin, &dir)).await? {
        Ok(screen) => {
            let now = chrono::Local::now();
            status.limits = devin_limits(&screen, now.fixed_offset());
            status.note = devin_note(&screen);
            if status.limits.is_empty() {
                status.add_error(if screen.contains("Failed to fetch quota") { "Devin couldn't fetch its quota" } else { "Devin didn't report its quota" });
            }
        }
        Err(e) => status.add_error(format!("{e:#}")),
    }
    Ok(status)
}

/// What `devin auth status` says: signed in or not, the email, and the plan ("Tier: Devin Pro").
fn devin_account(text: &str) -> AgentStatus {
    let field = |name: &str| {
        text.lines().find_map(|l| l.trim().strip_prefix(name).map(|v| v.trim().to_string())).filter(|v| !v.is_empty())
    };
    let logged_in = text.lines().next().is_some_and(|l| l.trim_start().starts_with("Logged in"));
    let plan = field("Tier:").or_else(|| field("Plan:").map(|p| format!("Devin {p}")));
    let plan = plan.filter(|_| logged_in);
    // Usage comes out of the plan's quota (on-demand credits only past it).
    let billing = plan.clone().map(|p| crate::Billing::Plan(Some(p)));
    AgentStatus { logged_in, account: field("Email:").filter(|_| logged_in), plan, billing, ..Default::default() }
}

/// Run Devin's terminal UI in `dir`, ask it for `/usage`, and return the screen once the quota
/// is on it (or the UI gave up fetching it).
fn devin_usage_screen(bin: &Path, dir: &Path) -> Result<String> {
    let mut cmd = portable_pty::CommandBuilder::new(bin);
    // The folder is Trek's own and empty; nothing to trust, and the prompt would wait forever.
    cmd.args(["--respect-workspace-trust", "false"]);
    cmd.cwd(dir);
    cmd.env("PATH", detect::login_path());
    cmd.env("TERM", "xterm-256color");
    let mut pty = Pty::start(cmd, 40, 140)?;
    let result = (|| {
        let ready = pty.pump(Duration::from_secs(20), &|s| s.contains('❭') || s.contains("Trust "));
        if ready.contains("Trust ") && !ready.contains('❭') {
            anyhow::bail!("Devin asked to trust a folder before showing its quota");
        }
        if !ready.contains('❭') {
            anyhow::bail!("Devin's terminal UI didn't start");
        }
        pty.send(b"/usage")?;
        pty.pump(Duration::from_millis(600), &|_| false);
        // Close the command list it opened, then run the command as typed.
        pty.send(b"\x1b")?;
        pty.pump(Duration::from_millis(300), &|_| false);
        pty.send(b"\r")?;
        // "Fetching quota…" stays above the bars once they're in.
        let fetched = |s: &str| s.contains("% used") || s.contains("Failed to fetch quota");
        let screen = pty.pump(Duration::from_secs(20), &fetched);
        // The panel draws in one go; a moment more for anything after the bars.
        Ok(if screen.contains("% used") { pty.pump(Duration::from_millis(300), &|_| false) } else { screen })
    })();
    // Quit as a user would (twice Ctrl-C), so Devin lets go of the session it opened.
    let pids = pty.end(b"\x03\x03");
    drop_session_locks(&devin_session_locks(), &pids);
    result
}

/// A program running in a pseudo-terminal of its own (ConPTY on Windows), its screen kept by a
/// terminal emulator, for reading what a terminal UI shows and typing to it. No window is
/// involved, and nothing reaches one: the program's console is this.
struct Pty {
    parser: vt100::Parser,
    chunks: std::sync::mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn std::io::Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Pty {
    fn start(cmd: portable_pty::CommandBuilder, rows: u16, cols: u16) -> Result<Pty> {
        use std::io::Read;
        let pair = portable_pty::native_pty_system().openpty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (tx, chunks) = std::sync::mpsc::channel::<Vec<u8>>();
        // Reads until the terminal closes, even once nobody listens: on Windows, closing it waits
        // for its output to be read.
        std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = tx.send(buf[..n].to_vec());
            }
        });
        // Written down, so that a Trek that crashes with it running can end it at its next launch.
        #[cfg(windows)]
        if let Some(id) = child.process_id() {
            trek_core::procs::register(id as i32);
        }
        Ok(Pty { parser: vt100::Parser::new(rows, cols, 0), chunks, writer, child, master: pair.master })
    }

    fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Feed the screen until `done` says so, or `limit` runs out; the screen's text then. Terminal
    /// queries (cursor position, device attributes) are answered as a terminal would.
    fn pump(&mut self, limit: Duration, done: &dyn Fn(&str) -> bool) -> String {
        let end = std::time::Instant::now() + limit;
        loop {
            let screen = self.parser.screen().contents();
            if done(&screen) || std::time::Instant::now() >= end {
                return screen;
            }
            match self.chunks.recv_timeout(Duration::from_millis(100)) {
                Ok(chunk) => self.feed(&chunk),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // The program exited: the screen is all there will be. (On Windows the terminal
                    // stays open, and quiet, after it.)
                    if matches!(self.child.try_wait(), Ok(Some(_))) {
                        while let Ok(chunk) = self.chunks.try_recv() {
                            self.feed(&chunk);
                        }
                        return self.parser.screen().contents();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return screen,
            }
        }
    }

    fn feed(&mut self, chunk: &[u8]) {
        use std::io::Write;
        self.parser.process(chunk);
        if chunk.windows(4).any(|w| w == b"\x1b[6n") {
            let (r, c) = self.parser.screen().cursor_position();
            // In one write: a terminal takes an escape sequence split in two for a key and the rest.
            let _ = self.writer.write_all(format!("\x1b[{};{}R", r + 1, c + 1).as_bytes());
        }
        if chunk.windows(3).any(|w| w == b"\x1b[c") || chunk.windows(4).any(|w| w == b"\x1b[0c") {
            let _ = self.writer.write_all(b"\x1b[?62;c");
        }
        let _ = self.writer.flush();
    }

    /// Stop the program and everything it started: type each key of `quit` to it, give it a few
    /// seconds to go, then end it. The ids of the processes it ran as (it may start its UI in one
    /// of its own), for the caller to tidy up after.
    fn end(mut self, quit: &[u8]) -> Vec<u32> {
        let id = self.child.process_id();
        let pids: Vec<u32> = id.map(descendants).unwrap_or_default();
        for key in quit {
            let _ = self.send(&[*key]);
            std::thread::sleep(Duration::from_millis(250));
        }
        let end = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < end && matches!(self.child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        // Ending a process on Windows leaves what it started running: found by parent ids.
        #[cfg(windows)]
        if let Some(id) = id {
            trek_core::procs::end_tree(id as i32);
            trek_core::procs::unregister(id as i32);
        }
        let _ = self.child.wait();
        drop(self.writer);
        drop(self.master);
        pids
    }
}

/// `pid` and the processes under it, as they are now.
fn descendants(pid: u32) -> Vec<u32> {
    let mut out = vec![pid];
    let mut i = 0;
    while i < out.len() && out.len() < 64 {
        let kids = trek_core::procs::children(out[i]);
        for kid in kids {
            if !out.contains(&kid) {
                out.push(kid);
            }
        }
        i += 1;
    }
    out
}

/// Devin leaves a lock behind for each terminal UI it ran (`session_locks/<name>.lock`, holding
/// the process id), even quit as a user would. The ones of the processes Trek just ran go too.
fn drop_session_locks(locks: &Path, pids: &[u32]) {
    let Ok(entries) = std::fs::read_dir(locks) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let ours = || std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u32>().ok()).is_some_and(|pid| pids.contains(&pid));
        if path.extension().is_some_and(|e| e == "lock") && ours() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Where Devin keeps its session locks: under the user's data folder, `$XDG_DATA_HOME` or
/// `~/.local/share` on a Mac; `%APPDATA%` (Roaming) on Windows, where the others are fallbacks.
fn devin_session_locks() -> PathBuf {
    let home = trek_core::paths::home();
    let absolute = |name: &str| std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute());
    #[cfg(windows)]
    let data = vec![absolute("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming")), home.join(".local").join("share")];
    #[cfg(not(windows))]
    let data = vec![absolute("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"))];
    first_existing(data.into_iter().map(|d| d.join("devin").join("cli").join("session_locks")).collect())
}

/// The first of `candidates` that is a folder, else the first.
fn first_existing(candidates: Vec<PathBuf>) -> PathBuf {
    candidates.iter().find(|c| c.is_dir()).or(candidates.first()).cloned().unwrap_or_default()
}

/// The quota bars of Devin's `/usage` panel, as its screen shows them:
/// `Daily   ■■■■  12% used  · resets in 5h 10m` and
/// `Weekly  ■■■■  3% used  · resets Oct 4, 4:00 AM (UTC-4)`. Without the panel, the header's
/// `Pro · 88% remaining (resets in 5h 10m)` stands for the plan's quota.
fn devin_limits(screen: &str, now: chrono::DateTime<chrono::FixedOffset>) -> Vec<UsageLimit> {
    let mut out = Vec::new();
    for line in screen.lines().map(str::trim) {
        let Some(at) = line.find("% used") else { continue };
        let Some(percent) = number_before(&line[..at]) else { continue };
        let name = line.split_whitespace().next().unwrap_or_default();
        let window = match name {
            "Daily" => "24h",
            "Weekly" => "7d",
            "Monthly" => "30d",
            _ => "",
        };
        let resets_at = line.split_once("resets ").and_then(|(_, r)| devin_reset(r, now));
        out.push(UsageLimit { label: format!("{name} limit"), percent, resets_at, window: window.into() });
    }
    if out.is_empty()
        && let Some(line) = screen.lines().map(str::trim).find(|l| l.contains("% remaining"))
        && let Some(left) = number_before(&line[..line.find("% remaining").unwrap_or(0)])
    {
        let resets_at = line.split_once("resets ").and_then(|(_, r)| devin_reset(r.trim_end_matches(')'), now));
        out.push(UsageLimit { label: "Quota".into(), percent: (100.0 - left).clamp(0.0, 100.0), resets_at, window: String::new() });
    }
    out
}

/// The number that ends `text` ("■■■ 12" → 12).
fn number_before(text: &str) -> Option<f32> {
    let digits: String = text.trim_end().chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
    digits.parse().ok()
}

/// When a Devin quota resets: `in 5h 10m` (from `now`), or `Oct 4, 4:00 AM (UTC-4)`. Unix ms.
fn devin_reset(text: &str, now: chrono::DateTime<chrono::FixedOffset>) -> Option<i64> {
    use chrono::{Datelike, NaiveDateTime, TimeZone};
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("in ") {
        let mut secs = 0i64;
        for part in rest.split_whitespace() {
            let (n, unit) = part.split_at(part.find(|c: char| !c.is_ascii_digit())?);
            let n: i64 = n.parse().ok()?;
            let unit = match unit {
                "d" => 86_400,
                "h" => 3_600,
                "m" => 60,
                "s" => 1,
                _ => return None,
            };
            secs = secs.checked_add(n.checked_mul(unit)?)?;
        }
        return now.timestamp_millis().checked_add(secs.checked_mul(1000)?);
    }
    let (when, zone) = match text.split_once(" (") {
        Some((w, z)) => (w.trim(), Some(z.trim_end_matches(')').trim())),
        None => (text, None),
    };
    let offset = match zone {
        Some(z) => {
            let z = z.strip_prefix("UTC").or_else(|| z.strip_prefix("GMT"))?;
            if z.is_empty() {
                0
            } else {
                let sign = if z.starts_with('-') { -1 } else { 1 };
                let z = z.get(1..)?;
                let (h, m) = z.split_once(':').unwrap_or((z, "0"));
                sign * h.parse::<i32>().ok()?.checked_mul(3_600)?.checked_add(m.parse::<i32>().ok()?.checked_mul(60)?)?
            }
        }
        None => now.offset().local_minus_utc(),
    };
    let tz = chrono::FixedOffset::east_opt(offset)?;
    let at = |year: i32| NaiveDateTime::parse_from_str(&format!("{year} {when}"), "%Y %b %d, %I:%M %p").ok().and_then(|t| tz.from_local_datetime(&t).single());
    let this_year = at(now.year())?;
    // Late December showing an early-January reset.
    let t = if this_year.timestamp() < now.timestamp() - 86_400 { at(now.year() + 1)? } else { this_year };
    Some(t.timestamp_millis())
}

/// A balance line on the `/usage` panel (on-demand credits past the quota), as Devin wrote it.
fn devin_note(screen: &str) -> Option<String> {
    screen.lines().map(str::trim).find(|l| l.to_lowercase().contains("balance")).map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from a real `get_usage` response (Claude Code 2.1.287).
    fn claude_usage() -> Value {
        json!({"subscription_type":"max","rate_limits_available":true,"rate_limits":{
            "five_hour":{"utilization":31,"resets_at":"2026-10-02T07:59:59.568705+00:00"},
            "limits":[
                {"kind":"session","group":"session","percent":31,"resets_at":"2026-10-02T07:59:59.568705+00:00","severity":"normal","is_active":true,"scope":null},
                {"kind":"weekly_all","group":"weekly","percent":13,"resets_at":"2026-10-08T14:59:59.568726+00:00","severity":"normal","is_active":false,"scope":null},
                {"kind":"weekly_scoped","group":"weekly","percent":0,"resets_at":"2026-10-08T15:00:00+00:00","severity":"normal","is_active":false,
                 "scope":{"model":{"display_name":"Fable","id":null},"surface":null}}
            ]}})
    }

    #[test]
    fn claude_limits_are_labeled() {
        let l = claude_limits(&claude_usage());
        assert_eq!(l.len(), 3);
        assert_eq!((l[0].label.as_str(), l[0].percent, l[0].window.as_str()), ("5-hour limit", 31.0, "5h"));
        assert_eq!(l[0].resets_at, Some(1790927999568));
        assert_eq!((l[1].label.as_str(), l[1].percent, l[1].window.as_str()), ("Weekly limit", 13.0, "7d"));
        assert_eq!((l[2].label.as_str(), l[2].percent), ("Weekly · Fable", 0.0));
        assert_eq!(l[2].resets_at, Some(1791471600000));
        assert!(claude_limits(&json!({"rate_limits_available":false,"rate_limits":null})).is_empty());
    }

    #[test]
    fn claude_init_parses_account_commands_models() {
        let init = json!({
            "account":{"email":"me@example.com","organization":"Org","subscriptionType":"Claude Max","apiProvider":"firstParty"},
            "commands":[
                {"name":"ego-browser","description":"Browser skill","argumentHint":""},
                {"name":"dataviz","description":"Charts","argumentHint":"","builtin":true},
                {"name":"clear","description":"Start a new session","argumentHint":"[name]","aliases":["reset","new"],"builtin":true},
                {"name":"__remote-workflow","description":"internal","builtin":true},
                {"name":"pdf","description":"PDFs","argumentHint":"","aliases":["anthropic-skills:pdf"]}
            ],
            "agents":[{"name":"Explore","description":"Read-only search","model":"haiku"}],
            "models":[
                {"value":"default","resolvedModel":"claude-opus-5-5","displayName":"Default (recommended)","supportedEffortLevels":["low","medium","high","xhigh","max"],"supportsFastMode":true},
                {"value":"opus","resolvedModel":"claude-opus-5-5","displayName":"Opus 5.5","supportedEffortLevels":["low","medium","high","xhigh","max"],"supportsFastMode":true},
                {"value":"fable","resolvedModel":"claude-fable-5-1","displayName":"Fable 5.1","supportedEffortLevels":["low","high"]},
                {"value":"haiku","resolvedModel":"claude-haiku-4-5-20251001","displayName":"Haiku 4.5"}
            ]
        });
        let context = json!({"skills":{"skillFrontmatter":[{"name":"ego-browser"},{"name":"dataviz"},{"name":"pdf"}]}});
        let mut s = AgentStatus::default();
        apply_claude_init(&mut s, &init, &context);
        assert!(s.logged_in);
        assert_eq!(s.account.as_deref(), Some("me@example.com"));
        assert_eq!(s.plan.as_deref(), Some("Claude Max"));
        let kinds: Vec<(&str, CommandKind)> = s.commands.iter().map(|c| (c.name.as_str(), c.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("ego-browser", CommandKind::Skill),
                ("dataviz", CommandKind::Skill),
                ("clear", CommandKind::Command),
                ("pdf", CommandKind::Skill),
                ("Explore", CommandKind::Agent),
            ]
        );
        let ids: Vec<&str> = s.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["claude-opus-5-5", "claude-fable-5-1", "claude-haiku-4-5-20251001"]);
        assert_eq!(s.models[0].name, "Opus 5.5");
        assert_eq!(s.models[0].fast.as_deref(), Some("settings"));
        assert_eq!(s.models[1].efforts, vec![Effort::Low, Effort::High]);
        assert_eq!(s.models[1].fast, None);
        assert_eq!((s.models[0].tier, s.models[1].tier, s.models[2].tier), (2, 3, 0));

        // Without the context response, fall back to the builtin/alias heuristic.
        let mut s = AgentStatus::default();
        apply_claude_init(&mut s, &init, &Value::Null);
        let skills: Vec<&str> = s.commands.iter().filter(|c| c.kind == CommandKind::Skill).map(|c| c.name.as_str()).collect();
        assert_eq!(skills, vec!["ego-browser", "pdf"]);
    }

    #[test]
    fn claude_logged_out_has_no_account() {
        let mut s = AgentStatus::default();
        apply_claude_init(&mut s, &json!({"account":{"apiProvider":"firstParty"},"commands":[],"models":[]}), &Value::Null);
        assert!(!s.logged_in);
        assert_eq!(s.account, None);
    }

    // Trimmed from a real `account/rateLimits/read` response (codex-cli 0.160.0).
    fn codex_rate_limits() -> Value {
        json!({
            "rateLimits":{"limitId":"codex","limitName":null,
                "primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":1790929839},
                "secondary":{"usedPercent":16,"windowDurationMins":10080,"resetsAt":1791065371},
                "planType":"plus"},
            "rateLimitsByLimitId":{
                "codex":{"limitId":"codex","limitName":null,
                    "primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":1790929839},
                    "secondary":{"usedPercent":16,"windowDurationMins":10080,"resetsAt":1791065371}},
                "base_model_inference":{"limitId":"base_model_inference","limitName":"gpt-reserve","normalModelSlug":"gpt-5.6-luna",
                    "primary":{"usedPercent":7,"windowDurationMins":10080,"resetsAt":1791526579},"secondary":null},
                "unnamed":{"limitId":"unnamed","limitName":null,"primary":{"usedPercent":50,"windowDurationMins":300,"resetsAt":1}}
            }
        })
    }

    #[test]
    fn codex_reset_credits_are_read() {
        let r = json!({"rateLimitResetCredits":{"availableCount":2,"credits":[
            {"id":"RateLimitResetCredit_a","resetType":"codexRateLimits","status":"available","grantedAt":1790109912,"expiresAt":1792701912,"title":"Full reset (Weekly + 5 hr)","description":"free reset"},
            {"id":"RateLimitResetCredit_b","status":"used","title":"Full reset (Weekly + 5 hr)","expiresAt":1792701912},
            {"status":"available","title":"no id is dropped"}
        ]}});
        let c = codex_reset_credits(&r);
        assert_eq!(c.len(), 1, "only the available, identified credit survives");
        assert_eq!((c[0].id.as_str(), c[0].title.as_str()), ("RateLimitResetCredit_a", "Full reset (Weekly + 5 hr)"));
        assert_eq!(c[0].expires_at, Some(1792701912000));
        assert_eq!(c[0].description.as_deref(), Some("free reset"));
        assert!(codex_reset_credits(&json!({})).is_empty(), "other providers carry no reset credits");
    }

    #[test]
    fn codex_limits_are_labeled() {
        let l = codex_limits(&codex_rate_limits());
        let got: Vec<(&str, f32, Option<i64>, &str)> =
            l.iter().map(|l| (l.label.as_str(), l.percent, l.resets_at, l.window.as_str())).collect();
        assert_eq!(
            got,
            vec![
                ("5-hour limit", 0.0, Some(1790929839000), "5h"),
                ("Weekly limit", 16.0, Some(1791065371000), "7d"),
                ("Weekly · gpt-reserve", 7.0, Some(1791526579000), "7d"),
            ]
        );
        let odd = codex_limits(&json!({"rateLimits":{"primary":{"usedPercent":3,"windowDurationMins":1440,"resetsAt":null}}}));
        assert_eq!((odd[0].label.as_str(), odd[0].window.as_str(), odd[0].resets_at), ("24h limit", "24h", None));
    }

    #[test]
    fn recorded_windows_tell_whether_a_limit_still_holds() {
        use crate::limits::{LimitScope, limited_until};
        let now = 1_790_900_000_000;
        // Claude: the session window used up, and Fable's week.
        let mut usage = claude_usage();
        usage["rate_limits"]["limits"][0]["percent"] = json!(100);
        usage["rate_limits"]["limits"][2]["percent"] = json!(100);
        let claude = claude_limits(&usage);
        assert_eq!(limited_until(&claude, &LimitScope::Session, Some("claude-sonnet-5-5"), now), Some(1_790_927_999_568));
        assert_eq!(limited_until(&claude, &LimitScope::Session, Some("claude-fable-5-1"), now), Some(1_791_471_600_000));
        // After the session reset, only Fable's week holds anything back.
        assert_eq!(limited_until(&claude, &LimitScope::Session, Some("claude-sonnet-5-5"), 1_790_928_000_000), None);
        // Codex: a model's quota used up holds back that limit, not the account's.
        let mut r = codex_rate_limits();
        r["rateLimitsByLimitId"]["base_model_inference"]["primary"]["usedPercent"] = json!(100);
        let codex = codex_limits(&r);
        assert_eq!(limited_until(&codex, &LimitScope::Session, Some("gpt-5.6-sol"), now), None);
        assert_eq!(limited_until(&codex, &LimitScope::Model("gpt-reserve".into()), Some("gpt-5.6-luna"), now), Some(1_791_526_579_000));
        assert_eq!(limited_until(&codex_limits(&codex_rate_limits()), &LimitScope::Session, None, now), None);
    }

    #[test]
    fn an_answer_put_aside_is_found_later() {
        trek_core::runtime().block_on(async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            // `plugin/reconcile` (id 1) answered while `account/read` (id 2) was awaited.
            let mut backlog = vec![json!({"id":1,"result":{"changedPlugins":[]}}), json!({"method":"account/updated","params":{}})];
            let mut lines = crate::ProtocolLines::new(BufReader::new(&b""[..]));
            assert_eq!(response(&mut lines, &mut backlog, 1, deadline).await.unwrap(), json!({"changedPlugins":[]}));
            assert_eq!(backlog.len(), 1, "taken out of the backlog; the rest stays");
            // Not answered yet: read on, setting aside what comes first.
            let text = b"{\"method\":\"skills/changed\"}\n{\"id\":3,\"error\":{\"message\":\"unknown method\"}}\n";
            let mut lines = crate::ProtocolLines::new(BufReader::new(&text[..]));
            let err = response(&mut lines, &mut backlog, 3, deadline).await.unwrap_err();
            assert!(err.to_string().contains("unknown method"), "{err}");
            assert_eq!(backlog.len(), 2);
        });
    }

    #[test]
    fn codex_account_and_skills() {
        let mut s = AgentStatus::default();
        apply_codex_account(&mut s, &json!({"account":{"type":"chatgpt","email":"me@example.com","planType":"plus"},"requiresOpenaiAuth":true}));
        assert!(s.logged_in);
        assert_eq!((s.account.as_deref(), s.plan.as_deref()), (Some("me@example.com"), Some("ChatGPT Plus")));
        let mut s = AgentStatus::default();
        apply_codex_account(&mut s, &json!({"account":{"type":"chatgpt","email":null,"planType":"pro"}}));
        assert_eq!(s.plan.as_deref(), Some("ChatGPT Pro"));
        let mut s = AgentStatus::default();
        apply_codex_account(&mut s, &json!({"account":{"type":"chatgpt","email":"x","planType":"team"}}));
        assert_eq!(s.plan.as_deref(), Some("ChatGPT Team"));
        let mut s = AgentStatus::default();
        apply_codex_account(&mut s, &json!({"account":null,"requiresOpenaiAuth":true}));
        assert!(!s.logged_in);

        let skills = codex_skills(&json!({"data":[{"cwd":"/tmp","skills":[
            {"name":"browser:control-in-app-browser","description":"Control the browser","path":"/x/SKILL.md","scope":"user","enabled":true},
            {"name":"off","description":"Disabled","path":"/y/SKILL.md","scope":"user","enabled":false}
        ]}]}));
        assert_eq!(
            skills,
            vec![SlashCommand { name: "browser:control-in-app-browser".into(), description: "Control the browser".into(), kind: CommandKind::Skill }]
        );
    }

    // Devin CLI 3000.11.3, as `devin auth status` printed it (identifiers removed).
    const DEVIN_AUTH: &str = "Logged in (via Devin).\n\nCredentials:\n  File:              /Users/me/.local/share/devin/credentials.toml\n  API server:        https://server.codeium.com\n\nUser:\n  Name:              Toby\n  Email:             toby@example.com\n  User ID:           user-0000\n\nAccount:\n  Tier:              Devin Pro\n  Plan:              Pro\n  Enterprise:        no\n";

    // The screen after `/usage` (Devin CLI 3000.11.3), as the terminal emulator reads it.
    const DEVIN_USAGE: &str = " ⠀⣴⣾⣶⡄⠀⠀⠀⠀\n ⠀⠛⠿⠟⠻⣶⣾⣶⡄  Devin CLI\n Pro · 100% remaining (resets in 5h 10m)\n❭ /usage\n Daily   ■■■■■■■■■■■■■■■■■■■■  0% used  · resets in 5h 10m\n Weekly  ■■■■■■■■■■■■■■■■■■■■  0% used  · resets Oct 4, 4:00 AM (UTC-4)\n No quota consumed yet in this session.\n──────\n❭ Ask Devin to build features, fix bugs, or work on your code\nSWE-2 Max";

    fn at(s: &str) -> chrono::DateTime<chrono::FixedOffset> {
        chrono::DateTime::parse_from_rfc3339(s).unwrap()
    }

    #[test]
    fn devin_account_and_plan() {
        let st = devin_account(DEVIN_AUTH);
        assert!(st.logged_in);
        assert_eq!((st.account.as_deref(), st.plan.as_deref()), (Some("toby@example.com"), Some("Devin Pro")));
        assert_eq!(st.billing, Some(crate::Billing::Plan(Some("Devin Pro".into()))));
        let out = devin_account("Not logged in. Run `devin auth login` to sign in.\n");
        assert!(!out.logged_in && out.plan.is_none());
    }

    #[test]
    fn devin_quota_from_the_usage_panel() {
        let now = at("2026-10-03T22:50:00-04:00");
        let l = devin_limits(DEVIN_USAGE, now);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].label.as_str(), l[0].percent, l[0].window.as_str()), ("Daily limit", 0.0, "24h"));
        assert_eq!(l[0].resets_at, Some(now.timestamp_millis() + (5 * 3_600 + 10 * 60) * 1000));
        assert_eq!((l[1].label.as_str(), l[1].window.as_str()), ("Weekly limit", "7d"));
        assert_eq!(l[1].resets_at, Some(at("2026-10-04T04:00:00-04:00").timestamp_millis()));
        let used = DEVIN_USAGE.replace("Daily   ■■■■■■■■■■■■■■■■■■■■  0% used", "Daily   ■■■■■■■■■■■■■■■■■■■■  37% used");
        assert_eq!(devin_limits(&used, now)[0].percent, 37.0);
        assert_eq!(devin_note(DEVIN_USAGE), None);
        assert_eq!(devin_note(" Extra usage   $12.40 balance\n").as_deref(), Some("Extra usage $12.40 balance"));
    }

    #[test]
    fn devin_quota_from_the_header_alone() {
        let now = at("2026-10-03T22:50:00-04:00");
        let l = devin_limits(" Pro · 88% remaining (resets in 2h 5m)\n❭ Ask Devin", now);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].percent, l[0].resets_at), (12.0, Some(now.timestamp_millis() + (2 * 3_600 + 5 * 60) * 1000)));
        assert!(devin_limits("❭ Ask Devin", now).is_empty());
    }

    #[test]
    fn only_the_locks_of_trek_s_own_devin_go() {
        let dir = std::env::temp_dir().join(format!("trek-devin-locks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("open-badge.lock"), "4242\n").unwrap();
        std::fs::write(dir.join("ionized-growth.lock"), "424").unwrap();
        std::fs::write(dir.join("notes.txt"), "4242").unwrap();
        std::fs::write(dir.join("mutual-bakery.lock"), "4243\n").unwrap();
        drop_session_locks(&dir, &[4242, 4243]);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["ionized-growth.lock", "notes.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_session_locks_are_where_devin_made_them() {
        let dir = std::env::temp_dir().join(format!("trek-devin-where-{}", std::process::id()));
        let (first, second) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&second).unwrap();
        assert_eq!(first_existing(vec![first.clone(), second.clone()]), second, "the first that exists");
        assert_eq!(first_existing(vec![first.clone(), dir.join("c")]), first, "none do: the first");
        assert_eq!(first_existing(vec![]), PathBuf::new());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(devin_session_locks().ends_with(Path::new("devin").join("cli").join("session_locks")));
    }

    /// The pseudo-terminal Devin's UI runs in, with `cmd.exe` as the program: it shows what it
    /// prints, takes what is typed, and ends with everything it started.
    #[cfg(windows)]
    #[test]
    fn a_program_runs_in_a_conpty_and_ends_with_all_it_started() {
        let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
        cmd.args(["/d", "/k", "prompt $G$S"]);
        let mut pty = Pty::start(cmd, 40, 140).unwrap();
        let shown = |word: &'static str| move |s: &str| s.lines().any(|l| l.trim() == word);
        pty.pump(Duration::from_secs(20), &|s| s.contains("> "));
        // The echoed command line holds the word too; its output is a line of its own.
        pty.send(b"echo hello\r").unwrap();
        let screen = pty.pump(Duration::from_secs(20), &shown("hello"));
        assert!(shown("hello")(&screen), "{screen}");
        // A child that keeps running: cmd waits for it.
        pty.send(b"ping -n 60 127.0.0.1 >nul\r").unwrap();
        let id = pty.child.process_id().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while descendants(id).len() < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        let pids = pty.end(b"");
        assert_eq!(pids.len(), 2, "cmd and its ping: {pids:?}");
        let running = |pid: u32| String::from_utf8_lossy(&std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap().stdout).contains(&pid.to_string());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while pids.iter().any(|p| running(*p)) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!pids.iter().any(|p| running(*p)), "everything under cmd was ended: {pids:?}");
        assert!(!trek_core::procs::live().contains(&(id as i32)), "and untracked");
    }

    /// A program that exits is noticed, not waited out.
    #[cfg(windows)]
    #[test]
    fn a_program_that_exits_leaves_its_last_screen() {
        let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
        cmd.args(["/d", "/c", "echo goodbye"]);
        let mut pty = Pty::start(cmd, 40, 140).unwrap();
        let started = std::time::Instant::now();
        let screen = pty.pump(Duration::from_secs(20), &|_| false);
        assert!(screen.contains("goodbye"), "{screen}");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        pty.end(b"");
    }

    #[test]
    fn devin_resets_read_every_way_it_writes_them() {
        let now = at("2026-12-31T20:00:00+00:00");
        assert_eq!(devin_reset("in 2d 3h", now), Some(now.timestamp_millis() + (2 * 86_400 + 3 * 3_600) * 1000));
        assert_eq!(devin_reset("Jan 2, 9:30 AM (UTC+5:30)", now), Some(at("2027-01-02T09:30:00+05:30").timestamp_millis()), "next year");
        assert_eq!(devin_reset("Dec 31, 11:00 PM (UTC)", now), Some(at("2026-12-31T23:00:00+00:00").timestamp_millis()));
        assert_eq!(devin_reset("soon", now), None);
        // Absurd numbers and a zone with a multi-byte sign: no time, no panic.
        assert_eq!(devin_reset("in 9999999999999999d", now), None);
        assert_eq!(devin_reset("in 9223372036854775807s", now), None);
        assert_eq!(devin_reset("Jan 2, 9:30 AM (UTC\u{2212}5)", now), None);
        assert_eq!(devin_reset("Jan 2, 9:30 AM (UTC+99999999)", now), None);
    }

    /// Against the installed Devin CLI (signed in): its plan and quota. Sends no prompt.
    /// `cargo test -p trek-agents devin_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn devin_live() {
        let st = trek_core::runtime().block_on(devin_status()).unwrap();
        println!("plan {:?}, limits {:?}, note {:?}, error {:?}", st.plan, st.limits, st.note, st.error);
        assert!(st.logged_in && !st.limits.is_empty());
    }

    /// Against the installed Claude Code and Codex (signed in): how each login is billed.
    /// Sends no prompt. `cargo test -p trek-agents billing_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn billing_live() {
        let dir = std::env::temp_dir();
        let claude = trek_core::runtime().block_on(claude_status(&dir)).unwrap();
        let codex = trek_core::runtime().block_on(codex_status(&dir)).unwrap();
        println!("claude {:?} {:?}; codex {:?} {:?}", claude.plan, claude.billing, codex.plan, codex.billing);
        assert!(claude.billing.is_some() && codex.billing.is_some());
    }
}
