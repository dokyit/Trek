//! Account, plan, usage limits, slash commands and models for the vendor CLIs, read without
//! sending a prompt (free). Claude: `initialize` + `get_usage` + `get_context_usage` control
//! requests on an idle stream-json session. Codex: `account/read`, `account/rateLimits/read`,
//! `skills/list` and `model/list` on `codex app-server`.

use crate::codex::{Rpc, RpcLines, await_response, fetch_models, start_app_server};
use anyhow::{Context as _, Result};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use trek_core::catalog::ModelInfo;
use trek_core::{Effort, detect};

const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
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
    let bin = detect::which("claude").context("Claude Code isn't installed (npm i -g @anthropic-ai/claude-code)")?;
    let mut child = tokio::process::Command::new(bin)
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
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to start claude")?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut stderr = child.stderr.take().unwrap();
    let stderr_task = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    });

    let wanted = [("i1", "initialize"), ("u1", "get_usage"), ("c1", "get_context_usage")];
    for (id, subtype) in wanted {
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
    let _ = child.start_kill();

    let null = Value::Null;
    let init = responses.get("i1").unwrap_or(&null);
    let usage = responses.get("u1").unwrap_or(&null);
    let context = responses.get("c1").unwrap_or(&null);
    apply_claude_init(&mut status, init, context);
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
    let mut backlog = Vec::new();
    let (mut child, mut rpc, mut lines, _) =
        tokio::time::timeout(TIMEOUT, start_app_server(cwd, &[], &mut backlog)).await.context("codex app-server timed out")??;
    let mut status = AgentStatus::default();
    let deadline = tokio::time::Instant::now() + TIMEOUT;

    match call(&mut rpc, &mut lines, &mut backlog, deadline, "account/read", json!({})).await {
        Ok(r) => apply_codex_account(&mut status, &r),
        Err(e) => status.add_error(format!("account: {e:#}")),
    }
    if status.logged_in {
        match call(&mut rpc, &mut lines, &mut backlog, deadline, "account/rateLimits/read", Value::Null).await {
            Ok(r) => status.limits = codex_limits(&r),
            // API-key accounts have no plan limits.
            Err(e) if status.plan.is_some() => status.add_error(format!("rate limits: {e:#}")),
            Err(_) => {}
        }
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
    let _ = child.start_kill();
    Ok(status)
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
}
