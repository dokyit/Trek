//! Claude Code sessions: `~/.claude/projects/<cwd-slug>/<session-id>.jsonl`.

use super::{ImportedThread, clip, file_mtime_ms, is_injected, ms_from_rfc3339, title_from};
use crate::store::{Item, ToolStatus};
use crate::types::{Effort, ThreadSource};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    crate::paths::home().join(".claude/projects")
}

pub fn scan(min_updated: i64) -> Vec<ImportedThread> {
    let Ok(dirs) = std::fs::read_dir(root()) else { return vec![] };
    let mut out = Vec::new();
    for dir in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(dir.path()) else { continue };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().is_some_and(|e| e == "jsonl") {
                let updated = file_mtime_ms(&path);
                if updated < min_updated {
                    continue;
                }
                if let Some(t) = index_file(&path, updated) {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// Text of a user message's content (string or array of blocks).
fn content_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            let text: Vec<&str> = blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .filter(|t| !is_injected(t))
                .collect();
            (!text.is_empty()).then(|| text.join("\n"))
        }
        _ => None,
    }
}

fn index_file(path: &Path, updated: i64) -> Option<ImportedThread> {
    let file = std::fs::File::open(path).ok()?;
    let session_id = path.file_stem()?.to_string_lossy().to_string();
    let mut cwd = None;
    let mut branch = None;
    let mut first_prompt = None;
    let mut created = None;
    let mut model = None;
    let mut effort = None;

    // Head: the first real prompt and its context. Bounded so huge files stay cheap.
    let mut read = 0usize;
    for line in BufReader::new(file).lines().map_while(Result::ok).take(400) {
        read += line.len();
        if read > 4 << 20 {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["isSidechain"] == true {
            continue;
        }
        if created.is_none() {
            created = v["timestamp"].as_str().and_then(ms_from_rfc3339);
        }
        if cwd.is_none() {
            cwd = v["cwd"].as_str().map(PathBuf::from);
            // Machine-generated sessions (e.g. other apps' title generators) run in temp dirs.
            if cwd.as_ref().is_some_and(|c| c.starts_with("/private/var/folders") || c.starts_with("/tmp") || c.starts_with("/var/folders")) {
                return None;
            }
        }
        if branch.is_none() {
            branch = v["gitBranch"].as_str().filter(|b| !b.is_empty()).map(String::from);
        }
        match v["type"].as_str() {
            Some("user") if first_prompt.is_none() => {
                if let Some(t) = content_text(&v["message"]["content"]).filter(|t| !is_injected(t)) {
                    first_prompt = Some(t);
                }
            }
            Some("assistant") if model.is_none() => {
                model = v["message"]["model"].as_str().map(String::from);
                effort = v["effort"].as_str().and_then(Effort::parse);
            }
            _ => {}
        }
        if first_prompt.is_some() && model.is_some() {
            break;
        }
    }
    let first_prompt = first_prompt?;

    // Tail: a renamed session stores `custom-title` near the end.
    let custom_title = tail_lines(path, 256 * 1024).into_iter().rev().find_map(|l| {
        let v: Value = serde_json::from_str(&l).ok()?;
        (v["type"] == "custom-title").then(|| v["customTitle"].as_str().map(String::from)).flatten()
    });

    Some(ImportedThread {
        source: ThreadSource::ClaudeCode,
        native_id: session_id,
        title: custom_title.unwrap_or_else(|| title_from(&first_prompt)),
        cwd,
        branch,
        model,
        effort,
        created_at: created.unwrap_or(updated),
        updated_at: updated,
        additions: 0,
        deletions: 0,
    })
}

fn tail_lines(path: &Path, bytes: u64) -> Vec<String> {
    let Ok(mut f) = std::fs::File::open(path) else { return vec![] };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(bytes);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return vec![];
    }
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // partial line
    }
    lines
}

pub(super) fn find_session(id: &str) -> Option<PathBuf> {
    let name = format!("{id}.jsonl");
    std::fs::read_dir(root()).ok()?.flatten().map(|d| d.path().join(&name)).find(|p| p.is_file())
}

fn tool_title(name: &str, input: &Value) -> (String, String) {
    let s = |k: &str| input[k].as_str().unwrap_or_default().to_string();
    match name {
        "Bash" => ("Ran command".into(), s("command")),
        "Read" => ("Read".into(), s("file_path")),
        "Edit" | "MultiEdit" => ("Edited".into(), s("file_path")),
        "Write" => ("Wrote".into(), s("file_path")),
        "Grep" => ("Searched".into(), s("pattern")),
        "Glob" => ("Listed files".into(), s("pattern")),
        "WebFetch" => ("Fetched".into(), s("url")),
        "WebSearch" => ("Searched the web".into(), s("query")),
        "Agent" | "Task" => ("Ran subagent".into(), s("description")),
        "TodoWrite" => ("Updated plan".into(), String::new()),
        other => (other.to_string(), clip(&input.to_string(), 200)),
    }
}

/// Full transcript, streamed line by line.
pub fn load(session_id: &str) -> anyhow::Result<Vec<Item>> {
    let path = find_session(session_id).ok_or_else(|| anyhow::anyhow!("session file not found"))?;
    let reader = BufReader::new(std::fs::File::open(path)?);
    let mut items: Vec<Item> = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["isSidechain"] == true {
            continue;
        }
        let content = &v["message"]["content"];
        match v["type"].as_str() {
            Some("user") => {
                if let Some(t) = content_text(content).filter(|t| !is_injected(t)) {
                    items.push(Item::User { text: t, images: vec![], at: None });
                }
                for block in content.as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    let id = block["tool_use_id"].as_str().unwrap_or_default();
                    let output = match &block["content"] {
                        Value::String(s) => s.clone(),
                        other => content_text(other).unwrap_or_default(),
                    };
                    let failed = block["is_error"] == true;
                    if let Some(Item::Tool { output: o, status, .. }) =
                        items.iter_mut().rev().find(|i| matches!(i, Item::Tool { id: tid, .. } if tid == id))
                    {
                        *o = clip(&output, 4000);
                        *status = if failed { ToolStatus::Failed } else { ToolStatus::Done };
                    }
                }
            }
            Some("assistant") => {
                for block in content.as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            let text = block["text"].as_str().unwrap_or_default().to_string();
                            // Streaming writes one line per block; merge consecutive text.
                            if let Some(Item::Assistant { text: prev }) = items.last_mut() {
                                prev.push_str("\n\n");
                                prev.push_str(&text);
                            } else if !text.trim().is_empty() {
                                items.push(Item::Assistant { text });
                            }
                        }
                        Some("thinking") => {
                            let text = block["thinking"].as_str().unwrap_or_default();
                            // Hidden reasoning is stored as an empty block.
                            if !text.trim().is_empty() {
                                items.push(Item::Reasoning { text: text.to_string() });
                            }
                        }
                        Some("tool_use") => {
                            let name = block["name"].as_str().unwrap_or("tool");
                            let (title, detail) = tool_title(name, &block["input"]);
                            items.push(Item::Tool {
                                id: block["id"].as_str().unwrap_or_default().to_string(),
                                title,
                                detail,
                                output: String::new(),
                                status: ToolStatus::Done,
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(items)
}
