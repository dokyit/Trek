//! Codex threads: index in `~/.codex/state_5.sqlite`, transcripts in rollout JSONL files.

use super::{ImportedThread, clip, is_injected, title_from};
use crate::store::{Item, ToolStatus};
use crate::types::{Effort, ThreadSource};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

fn db() -> Option<Connection> {
    let home = crate::paths::home().join(".codex");
    // Newest state db wins (state_5 today; tolerate future bumps).
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&home)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            n.starts_with("state_") && n.ends_with(".sqlite")
        })
        .collect();
    candidates.sort();
    let path = candidates.pop()?;
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()
}

pub fn scan(min_updated: i64) -> Vec<ImportedThread> {
    let Some(conn) = db() else { return vec![] };
    let sql = "SELECT id, cwd, title, first_user_message, model, reasoning_effort, git_branch,
                      COALESCE(created_at_ms, created_at * 1000), COALESCE(updated_at_ms, updated_at * 1000)
               FROM threads
               WHERE archived = 0 AND tokens_used > 0 AND COALESCE(agent_role, '') = '' AND COALESCE(updated_at_ms, updated_at * 1000) >= ?1";
    let Ok(mut st) = conn.prepare(sql) else { return vec![] };
    let rows = st.query_map([min_updated], |r| {
        let title: Option<String> = r.get(2)?;
        let first: Option<String> = r.get(3)?;
        Ok(ImportedThread {
            source: ThreadSource::Codex,
            native_id: r.get(0)?,
            cwd: r.get::<_, Option<String>>(1)?.map(PathBuf::from),
            title: title
                .filter(|t| !t.trim().is_empty())
                .or(first.map(|f| title_from(&f)))
                .unwrap_or_else(|| "Codex thread".into()),
            model: r.get(4)?,
            effort: r.get::<_, Option<String>>(5)?.and_then(|e| Effort::parse(&e)),
            branch: r.get(6)?,
            created_at: r.get(7)?,
            updated_at: r.get(8)?,
            additions: 0,
            deletions: 0,
        })
    });
    match rows {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(_) => vec![],
    }
}

pub(super) fn rollout_path(id: &str) -> Option<PathBuf> {
    let conn = db()?;
    conn.query_row("SELECT rollout_path FROM threads WHERE id = ?1", [id], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .map(PathBuf::from)
}

/// Joined text blocks, skipping injected context blocks (environment, instructions).
fn message_text(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["text"].as_str())
        .filter(|t| !is_injected(t))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn load(id: &str) -> anyhow::Result<Vec<Item>> {
    let path = rollout_path(id).ok_or_else(|| anyhow::anyhow!("rollout not found"))?;
    let reader = BufReader::new(std::fs::File::open(path)?);
    let mut items = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["type"] != "response_item" {
            continue;
        }
        let p = &v["payload"];
        match p["type"].as_str() {
            Some("message") => {
                let text = message_text(&p["content"]);
                match p["role"].as_str() {
                    Some("user") if !is_injected(&text) => items.push(Item::User { text, images: vec![], at: None }),
                    Some("assistant") if !text.trim().is_empty() => items.push(Item::Assistant { text }),
                    _ => {}
                }
            }
            Some("reasoning") => {
                let text = p["summary"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.trim().is_empty() {
                    items.push(Item::Reasoning { text });
                }
            }
            Some("function_call") | Some("custom_tool_call") => {
                let name = p["name"].as_str().unwrap_or("tool").to_string();
                let input = p["arguments"].as_str().or(p["input"].as_str()).unwrap_or_default();
                let detail = serde_json::from_str::<Value>(input)
                    .ok()
                    .and_then(|a| a["cmd"].as_str().or(a["command"].as_str()).map(String::from))
                    .unwrap_or_else(|| clip(input, 300));
                let title = match name.as_str() {
                    "exec_command" | "shell" => "Ran command".to_string(),
                    "apply_patch" => "Edited files".to_string(),
                    _ => name,
                };
                items.push(Item::Tool {
                    id: p["call_id"].as_str().unwrap_or_default().to_string(),
                    title,
                    detail,
                    output: String::new(),
                    status: ToolStatus::Done,
                });
            }
            Some("function_call_output") | Some("custom_tool_call_output") => {
                let id = p["call_id"].as_str().unwrap_or_default();
                let out = match &p["output"] {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if let Some(Item::Tool { output, .. }) =
                    items.iter_mut().rev().find(|i| matches!(i, Item::Tool { id: tid, .. } if tid == id))
                {
                    *output = clip(&out, 4000);
                }
            }
            _ => {}
        }
    }
    Ok(items)
}
