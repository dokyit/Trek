//! Claude Code sessions: `~/.claude/projects/<cwd-slug>/<session-id>.jsonl`.

use super::{
    Evidence, ImportedThread, Transcript, classify, clip, file_mtime_ms, is_injected, is_interruption, is_temp_dir,
    is_title_request, legacy_is_injected, legacy_title_from, ms_from_rfc3339, source_title, title_from, unwrap_pasted, user_text,
};
use crate::store::{Item, ResumePoint, ToolStatus};
use super::UsageEntry;
use crate::types::{Effort, ThreadSource, TokenUsage};
use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    crate::paths::home().join(".claude/projects")
}

/// Sessions updated since `min_updated`, and the ones in `held` (already in Trek) whatever their age.
pub fn scan(min_updated: i64, held: &HashSet<String>) -> Vec<ImportedThread> {
    scan_root(&root(), min_updated, held)
}

fn scan_root(root: &Path, min_updated: i64, held: &HashSet<String>) -> Vec<ImportedThread> {
    let Ok(dirs) = std::fs::read_dir(root) else { return vec![] };
    let mut out = Vec::new();
    for dir in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(dir.path()) else { continue };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().is_some_and(|e| e == "jsonl") {
                let updated = file_mtime_ms(&path);
                if updated < min_updated && !path.file_stem().is_some_and(|s| held.contains(s.to_string_lossy().as_ref())) {
                    continue;
                }
                if let Some(t) = index_file(&path, updated, min_updated, held) {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// Every session on disk, however old; `None` when the history can't be read.
pub(crate) fn session_ids() -> Option<HashSet<String>> {
    session_ids_in(&root())
}

fn session_ids_in(root: &Path) -> Option<HashSet<String>> {
    let dirs = std::fs::read_dir(root).ok()?;
    Some(
        dirs.flatten()
            .filter_map(|d| std::fs::read_dir(d.path()).ok())
            .flat_map(|files| files.flatten())
            .map(|f| f.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .collect(),
    )
}

/// Text blocks of a user message that the user wrote (string or array of blocks).
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

/// The first prompt as earlier Trek versions picked it (for recognising their titles).
fn legacy_content_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            let text: Vec<&str> =
                blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).filter(|t| !legacy_is_injected(t)).collect();
            (!text.is_empty()).then(|| text.join("\n"))
        }
        _ => None,
    }
}

/// All text blocks of a user line, injected ones included.
fn raw_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            let text: Vec<&str> = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
            (!text.is_empty()).then(|| text.join("\n"))
        }
        _ => None,
    }
}

/// A user line's typed text: the user's words, without skill bodies and other injected context.
fn typed_text(v: &Value) -> Option<String> {
    if v["isMeta"] == true {
        return None;
    }
    content_text(&v["message"]["content"]).and_then(|t| user_text(&t).map(|_| t))
}

/// `/name args` from a slash-command wrapper.
fn command_text(text: &str) -> Option<String> {
    let between = |open: &str, close: &str| -> Option<String> {
        let start = text.find(open)? + open.len();
        let end = text[start..].find(close)? + start;
        Some(text[start..end].trim().to_string())
    };
    let name = between("<command-name>", "</command-name>")?;
    let args = between("<command-args>", "</command-args>").unwrap_or_default();
    Some(if args.is_empty() { name } else { format!("{name} {args}") })
}

fn index_file(path: &Path, updated: i64, min_updated: i64, held: &HashSet<String>) -> Option<ImportedThread> {
    let file = std::fs::File::open(path).ok()?;
    let session_id = path.file_stem()?.to_string_lossy().to_string();
    let mut cwd: Option<PathBuf> = None;
    let mut branch = None;
    let mut created = None;
    let mut model = None;
    let mut effort = None;
    let mut entrypoint: Option<String> = None;
    let mut first_prompt: Option<String> = None;
    let mut first_message: Option<String> = None;
    let mut legacy_first: Option<String> = None;
    let mut command: Option<String> = None;
    // A command waiting to see whether the agent answers it (a skill, `/goal`) or not (`/model`).
    let mut asked: Option<String> = None;
    let mut prompts = 0usize;
    let mut replied = false;
    let mut main_line = false;
    let mut side_chain = false;
    let mut fork_of: Option<Option<String>> = None;
    let (mut ai_title, mut custom_title, mut summary) = (None, None, None);

    // Head: the first real prompt and its context. Bounded so huge files stay cheap.
    let mut read = 0usize;
    let mut whole = false;
    let mut lines = super::jsonl_lines(BufReader::new(file));
    for _ in 0..400 {
        let Some(line) = lines.next() else {
            whole = true;
            break;
        };
        read += line.len();
        if read > 4 << 20 {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if created.is_none() {
            created = v["timestamp"].as_str().and_then(ms_from_rfc3339);
        }
        match v["type"].as_str() {
            Some("ai-title") => ai_title = v["aiTitle"].as_str().map(String::from),
            Some("custom-title") => custom_title = v["customTitle"].as_str().map(String::from),
            Some("summary") => summary = v["summary"].as_str().map(String::from),
            _ => {}
        }
        if v["isSidechain"] == true {
            side_chain = true;
            continue;
        }
        if cwd.is_none() {
            cwd = v["cwd"].as_str().map(PathBuf::from);
        }
        if branch.is_none() {
            branch = v["gitBranch"].as_str().filter(|b| !b.is_empty()).map(String::from);
        }
        if entrypoint.is_none() {
            entrypoint = v["entrypoint"].as_str().map(String::from);
        }
        match v["type"].as_str() {
            Some("user") => {
                main_line = true;
                fork_of.get_or_insert_with(|| v["forkedFrom"]["sessionId"].as_str().map(String::from));
                let content = &v["message"]["content"];
                if legacy_first.is_none() {
                    legacy_first = legacy_content_text(content).filter(|t| !legacy_is_injected(t));
                }
                if v["isMeta"] != true && first_message.is_none() {
                    first_message = raw_text(content);
                }
                if let Some(t) = typed_text(&v) {
                    prompts += 1;
                    first_prompt.get_or_insert(t);
                    asked = None;
                } else if let Some(c) = raw_text(content).as_deref().and_then(command_text) {
                    command.get_or_insert_with(|| c.clone());
                    asked = Some(c);
                }
            }
            Some("attachment") => {
                if let Some(text) = queued_prompt(&v) {
                    prompts += 1;
                    first_prompt.get_or_insert(text);
                }
            }
            Some("assistant") => {
                main_line = true;
                fork_of.get_or_insert_with(|| v["forkedFrom"]["sessionId"].as_str().map(String::from));
                replied = true;
                // The agent answered the command: it was a request (`/goal …`, a skill).
                if let Some(c) = asked.take() {
                    prompts += 1;
                    first_prompt.get_or_insert(c);
                }
                if model.is_none() {
                    model = v["message"]["model"].as_str().map(String::from);
                    effort = v["effort"].as_str().and_then(Effort::parse);
                }
            }
            _ => {}
        }
        // Stop once there's enough to title and classify it. Whether there was only one prompt
        // matters only for title requests and hand-started sessions in temp folders. The
        // prompt earlier versions titled it by may come later (they skipped pasted text).
        if first_prompt.is_some()
            && model.is_some()
            && legacy_first.is_some()
            && (prompts > 1
                || !(first_message.as_deref().is_some_and(is_title_request)
                    || (cwd.as_deref().is_some_and(is_temp_dir) && !is_scripted(entrypoint.as_deref()))))
        {
            break;
        }
    }

    // Tail: titles are rewritten as the session goes on, so the newest of each kind wins.
    let tail = tail_lines(path, 256 * 1024);
    // Claude Code keeps appending bookkeeping (titles, cost, mode) long after the last message,
    // so the file's modified time can be days late: the last message says when it was active.
    let updated = last_message_at(&tail).unwrap_or(updated);
    if updated < min_updated && !held.contains(&session_id) {
        return None;
    }
    let newest = |kind: &str, key: &str| {
        let needle = format!("\"type\":\"{kind}\"");
        tail.iter().rev().filter(|l| l.contains(&needle)).find_map(|l| title_line(l, kind, key))
    };
    custom_title = newest("custom-title", "customTitle").or(custom_title);
    ai_title = newest("ai-title", "aiTitle").or(ai_title);
    summary = newest("summary", "summary").or(summary);

    // A fork is a copy of another session until something new is said in it; its own
    // messages come after the copied ones.
    let untouched_fork = fork_of.flatten().is_some_and(|original_id| {
        let last_copied = tail.iter().rev().find_map(|l| {
            let v: Value = serde_json::from_str(l).ok()?;
            (v["isSidechain"] != true && matches!(v["type"].as_str(), Some("user") | Some("assistant"))).then(|| !v["forkedFrom"].is_null())
        });
        let original = path.with_file_name(format!("{original_id}.jsonl"));
        last_copied == Some(true) && original.is_file() && (file_mtime_ms(&original) >= min_updated || held.contains(&original_id))
    });
    let skip = classify(&Evidence {
        cwd: cwd.as_deref(),
        scripted: is_scripted(entrypoint.as_deref()),
        prompts: whole.then_some(prompts),
        first_message: first_message.as_deref(),
        replied,
        // Read whole or not: a session's own messages come first, so a head with nothing but a
        // side chain is a sub-agent's transcript (older Claude Code kept them as `agent-*.jsonl`
        // next to the sessions, and long ones run past the head).
        subagent: side_chain && !main_line,
        untouched_fork,
        trek: false,
        cli_run: false,
    });
    let title = [custom_title.as_deref(), ai_title.as_deref(), summary.as_deref()]
        .into_iter()
        .flatten()
        .find_map(source_title)
        .or_else(|| first_prompt.as_deref().or(command.as_deref()).map(title_from))
        .unwrap_or_else(|| "Claude Code session".into());
    let legacy_title = custom_title.clone().or_else(|| legacy_first.as_deref().map(legacy_title_from));

    Some(ImportedThread {
        source: ThreadSource::ClaudeCode,
        native_id: session_id,
        title,
        cwd,
        branch,
        model,
        effort,
        created_at: created.unwrap_or(updated),
        updated_at: updated,
        additions: 0,
        deletions: 0,
        skip,
        legacy_title,
    })
}

/// When the newest message of the main conversation in `tail` was written.
fn last_message_at(tail: &[String]) -> Option<i64> {
    tail.iter().rev().filter(|l| l.contains("\"timestamp\"")).find_map(|l| {
        let v: Value = serde_json::from_str(l).ok()?;
        let message = matches!(v["type"].as_str(), Some("user" | "assistant")) && v["isSidechain"] != true;
        message.then(|| v["timestamp"].as_str().and_then(ms_from_rfc3339)).flatten()
    })
}

/// What the user typed while the agent was working, from the `queued_command` attachment Claude
/// Code records when the running turn takes it in. Such a message gets no user line of its own.
/// Background tasks reporting back queue the same way; they aren't the user's.
fn queued_prompt(v: &Value) -> Option<String> {
    let a = &v["attachment"];
    if v["isSidechain"] == true || a["type"] != "queued_command" || a["origin"]["kind"] != "human" {
        return None;
    }
    let text = content_text(&a["prompt"])?;
    (!text.trim_start().starts_with("<task-notification>") && user_text(&text).is_some()).then(|| unwrap_pasted(&text))
}

/// Print mode and the SDKs record `sdk-*`; the terminal UI and IDE extensions don't.
fn is_scripted(entrypoint: Option<&str>) -> bool {
    entrypoint.is_some_and(|e| e.starts_with("sdk"))
}

/// Values of `<tag>…</tag>` in a raw line.
fn tagged<'a>(line: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(i) = rest.find(&open) {
        rest = &rest[i + open.len()..];
        let Some(end) = rest.find(&close) else { break };
        out.push(&rest[..end]);
        rest = &rest[end..];
    }
    out
}

/// The value of a title line (`custom-title`, `ai-title`, `summary`).
fn title_line(line: &str, kind: &str, key: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    (v["type"] == kind).then(|| v[key].as_str().map(String::from)).flatten()
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

/// Session `id`'s file, in whichever project folder Claude Code keeps it.
pub fn find_session(id: &str) -> Option<PathBuf> {
    let name = format!("{id}.jsonl");
    std::fs::read_dir(root()).ok()?.flatten().map(|d| d.path().join(&name)).find(|p| p.is_file())
}

/// Whether session `id` is on disk with a message `uuid` in it: somewhere
/// `--resume-session-at` can cut it.
pub fn has_message(id: &str, uuid: &str) -> bool {
    let Some(file) = find_session(id).and_then(|p| std::fs::File::open(p).ok()) else { return false };
    let needle = format!("\"uuid\":\"{uuid}\"");
    super::jsonl_lines(BufReader::new(file)).any(|l| l.contains(&needle))
}

/// The id of a line of the conversation that `--resume-session-at` can cut it after: a user or
/// assistant message of the main conversation (not a sub-agent's).
fn message_uuid(v: &Value) -> Option<&str> {
    (matches!(v["type"].as_str(), Some("user" | "assistant")) && v["isSidechain"] != true).then(|| v["uuid"].as_str()).flatten()
}

/// The last message in session `id` so far: where a message sent now goes after.
pub fn last_message(id: &str) -> Option<String> {
    last_message_in(&find_session(id)?)
}

fn last_message_in(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut last = None;
    for line in super::jsonl_lines(BufReader::new(file)).filter(|l| l.contains("\"uuid\"")) {
        if let Some(uuid) = serde_json::from_str::<Value>(&line).ok().as_ref().and_then(message_uuid) {
            last = Some(uuid.to_string());
        }
    }
    last
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

/// Tokens session `id` used between `from` and `to` (unix ms), per API response: when, which
/// model, how many. Read from the start of the file: run it off the main thread.
pub fn usage(id: &str, from: i64, to: i64) -> Vec<UsageEntry> {
    find_session(id).map(|p| usage_in(&p, from, to)).unwrap_or_default()
}

/// Claude Code writes a line per content block of a response, each repeating the response's
/// usage (the last one has the final output count): each response counts once, as its last
/// line has it.
fn usage_in(path: &Path, from: i64, to: i64) -> Vec<UsageEntry> {
    let Ok(file) = std::fs::File::open(path) else { return vec![] };
    let mut out: Vec<UsageEntry> = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for line in super::jsonl_lines(BufReader::new(file)) {
        // Most lines aren't responses; skip them without parsing.
        if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["type"] != "assistant" {
            continue;
        }
        let Some(at) = v["timestamp"].as_str().and_then(ms_from_rfc3339).filter(|at| (from..to).contains(at)) else { continue };
        let m = &v["message"];
        let u = &m["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let tokens = TokenUsage { input: n("input_tokens"), output: n("output_tokens"), cache_read: n("cache_read_input_tokens"), cache_write: n("cache_creation_input_tokens") };
        // API errors are written as "<synthetic>" responses that used nothing.
        let model = m["model"].as_str().filter(|m| !m.starts_with('<')).map(String::from);
        if tokens.is_empty() {
            continue;
        }
        let entry = (at, model, tokens);
        match m["id"].as_str().map(String::from) {
            Some(id) => match seen.get(&id) {
                Some(&i) => out[i] = entry,
                None => {
                    seen.insert(id, out.len());
                    out.push(entry);
                }
            },
            None => out.push(entry),
        }
    }
    out
}

/// Full transcript, streamed line by line.
pub fn load(session_id: &str) -> anyhow::Result<Vec<Item>> {
    let path = find_session(session_id).ok_or_else(|| anyhow::anyhow!("session file not found"))?;
    load_file(&path)
}

fn load_file(path: &Path) -> anyhow::Result<Vec<Item>> {
    let reader = BufReader::new(std::fs::File::open(path)?);
    let session = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut t = Transcript::default();
    // A slash command, shown once the agent answers it (`/goal …`, a skill); local ones like
    // `/model` get no answer and aren't shown. With the message before it, to resume at.
    let mut asked: Option<(String, Option<i64>, Option<String>)> = None;
    // The last message of the conversation so far: where `--resume-session-at` would cut it so
    // the next message and everything after it are gone.
    let mut last_message: Option<String> = None;
    let point = |after: Option<String>| Some(ResumePoint { session: session.clone(), after });
    // A message typed mid-turn and shown already, in case it's also written as a user line.
    let mut queued: Option<String> = None;
    for line in super::jsonl_lines(reader) {
        // Background tasks report back as a user line when the agent is idle, or queued into
        // the running turn; either way the task is done.
        if line.contains("<task-notification>") {
            for id in tagged(&line, "tool-use-id") {
                t.reported(id);
            }
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["isSidechain"] == true {
            continue;
        }
        let at = v["timestamp"].as_str().and_then(ms_from_rfc3339);
        let content = &v["message"]["content"];
        let before = last_message.clone();
        if let Some(uuid) = message_uuid(&v) {
            last_message = Some(uuid.to_string());
        }
        match v["type"].as_str() {
            Some("user") => {
                for block in content.as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    let id = block["tool_use_id"].as_str().unwrap_or_default();
                    if v["toolUseResult"]["isAsync"] == true {
                        t.launched(id);
                    }
                    let output = match &block["content"] {
                        Value::String(s) => s.clone(),
                        other => content_text(other).unwrap_or_default(),
                    };
                    let failed = block["is_error"] == true;
                    if let Some(Item::Tool { output: o, status, .. }) =
                        t.items.iter_mut().rev().find(|i| matches!(i, Item::Tool { id: tid, .. } if tid == id))
                    {
                        *o = clip(&output, 4000);
                        *status = if failed { ToolStatus::Failed } else { ToolStatus::Done };
                    }
                    t.touch(at);
                }
                // Skill bodies and image notes the CLI adds on the user's behalf.
                if v["isMeta"] == true {
                    continue;
                }
                let raw = raw_text(content).unwrap_or_default();
                match content_text(content).filter(|t| !is_injected(t)) {
                    Some(text) if queued.as_deref() == Some(unwrap_pasted(&text).as_str()) => queued = None,
                    Some(text) => {
                        asked = None;
                        t.user(unwrap_pasted(&text), at);
                        t.resume_from(point(before));
                    }
                    None if is_interruption(&raw) => {
                        asked = None;
                        t.interrupt();
                    }
                    // Idle, the agent picks up a background task's report on its own.
                    None if raw.trim_start().starts_with("<task-notification>") => {
                        asked = None;
                        t.wake(at);
                    }
                    None => {
                        if let Some(c) = command_text(&raw) {
                            asked = Some((c, at, before));
                        }
                    }
                }
            }
            // Typed while the agent was working: part of the running turn, which goes on.
            Some("attachment") => {
                if let Some(text) = queued_prompt(&v) {
                    t.steer(text.clone(), vec![], at);
                    queued = Some(text);
                }
            }
            Some("assistant") => {
                queued = None;
                if let Some((command, at, before)) = asked.take() {
                    t.user(command, at);
                    t.resume_from(point(before));
                }
                for block in content.as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            let text = block["text"].as_str().unwrap_or_default();
                            // Streaming writes one line per block; merge consecutive text.
                            match t.items.last_mut() {
                                Some(Item::Assistant { text: prev }) if !text.trim().is_empty() => {
                                    prev.push_str("\n\n");
                                    prev.push_str(text);
                                }
                                Some(Item::Assistant { .. }) => {}
                                _ if !text.trim().is_empty() => t.push(Item::Assistant { text: text.to_string() }),
                                _ => {}
                            }
                        }
                        Some("thinking") => {
                            let text = block["thinking"].as_str().unwrap_or_default();
                            // Hidden reasoning is stored as an empty block.
                            if !text.trim().is_empty() {
                                t.push(Item::Reasoning { text: text.to_string() });
                            }
                        }
                        Some("tool_use") => {
                            let name = block["name"].as_str().unwrap_or("tool");
                            let (title, detail) = tool_title(name, &block["input"]);
                            t.push(Item::Tool {
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
                t.activity(at);
                if matches!(v["message"]["stop_reason"].as_str(), Some("end_turn") | Some("stop_sequence")) {
                    t.complete(at);
                }
            }
            Some("system") if v["subtype"] == "turn_duration" => t.complete(at),
            _ => {}
        }
    }
    Ok(t.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::{Scratch, Skip};
    use serde_json::json;

    const REPO: &str = "/Users/me/code/app";

    /// A session file in Claude Code's format: conversation lines get the usual envelope.
    fn session(dir: &Scratch, id: &str, cwd: &str, entrypoint: &str, lines: &[Value]) -> PathBuf {
        let body: Vec<String> = lines
            .iter()
            .map(|l| {
                let mut l = l.clone();
                if matches!(l["type"].as_str(), Some("user") | Some("assistant") | Some("system")) {
                    for (k, v) in [("cwd", json!(cwd)), ("entrypoint", json!(entrypoint)), ("isSidechain", json!(false)), ("sessionId", json!(id))] {
                        if l.get(k).is_none() {
                            l[k] = v;
                        }
                    }
                }
                l.to_string()
            })
            .collect();
        dir.write(&format!("-Users-me-code-app/{id}.jsonl"), &(body.join("\n") + "\n"))
    }

    fn user(content: impl Into<Value>, at: &str) -> Value {
        json!({ "type": "user", "message": { "role": "user", "content": content.into() }, "timestamp": at })
    }

    fn meta(text: &str, at: &str) -> Value {
        let mut v = user(json!([{ "type": "text", "text": text }]), at);
        v["isMeta"] = json!(true);
        v
    }

    fn assistant(blocks: Value, stop: &str, at: &str) -> Value {
        json!({ "type": "assistant", "message": { "role": "assistant", "model": "claude-x", "content": blocks, "stop_reason": stop }, "timestamp": at })
    }

    fn reply(text: &str, at: &str) -> Value {
        assistant(json!([{ "type": "text", "text": text }]), "end_turn", at)
    }

    fn index(path: &Path) -> ImportedThread {
        index_file(path, 1, 0, &HashSet::new()).expect("indexed")
    }

    #[test]
    fn the_sessions_own_title_wins() {
        let dir = Scratch::new();
        let titled = session(&dir, "a", REPO, "cli", &[
            user("fix the login bug", "2026-10-01T10:00:00Z"),
            json!({ "type": "ai-title", "aiTitle": "Login bug" }),
            reply("fixed", "2026-10-01T10:01:00Z"),
            json!({ "type": "ai-title", "aiTitle": "Login redirect loop fix" }),
        ]);
        assert_eq!(index(&titled).title, "Login redirect loop fix");
        let renamed = session(&dir, "b", REPO, "cli", &[
            user("fix the login bug", "2026-10-01T10:00:00Z"),
            json!({ "type": "ai-title", "aiTitle": "Login bug" }),
            json!({ "type": "custom-title", "customTitle": "Auth week" }),
        ]);
        assert_eq!(index(&renamed).title, "Auth week");
        let summarised = session(&dir, "c", REPO, "cli", &[json!({ "type": "summary", "summary": "Fix flaky CI" }), user("ci is red again", "2026-10-01T10:00:00Z")]);
        assert_eq!(index(&summarised).title, "Fix flaky CI");
    }

    #[test]
    fn titles_skip_injected_context() {
        let dir = Scratch::new();
        let path = session(&dir, "a", REPO, "cli", &[
            meta("<local-command-caveat>Caveat: The messages below were generated by the user while running local commands.</local-command-caveat>", "2026-10-01T10:00:00Z"),
            user("<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args></command-args>", "2026-10-01T10:00:01Z"),
            user("<local-command-stdout>Set model to Opus</local-command-stdout>", "2026-10-01T10:00:02Z"),
            meta("Base directory for this skill: /tmp/skills/review\n\n# Review", "2026-10-01T10:00:03Z"),
            user(json!([{ "type": "text", "text": "[Image #1] the stadium is bugging out, the camera clips through the stand" }, { "type": "image" }]), "2026-10-01T10:00:04Z"),
            reply("Looking", "2026-10-01T10:00:05Z"),
        ]);
        let t = index(&path);
        assert_eq!(t.title, "the stadium is bugging out, the camera clips through the…");
        assert_eq!(t.skip, None);
        // What earlier versions called it, to recognise their titles when retitling.
        assert_eq!(t.legacy_title.as_deref(), Some("Base directory for this skill: /tmp/skills/review"));
        // A session that is a command and the agent's answer is titled by the command.
        let command = session(&dir, "b", REPO, "cli", &[
            user("<command-message>review</command-message>\n<command-name>/review</command-name>\n<command-args>42</command-args>", "2026-10-01T10:00:00Z"),
            reply("Reviewing PR 42", "2026-10-01T10:00:05Z"),
        ]);
        assert_eq!(index(&command).title, "/review 42");
        // A goal is the request itself, cut like any other.
        let instruction = "make the whole app faster, audit every screen for jank, and redesign the settings so they read like the rest of the app";
        let goal = session(&dir, "c", REPO, "cli", &[
            json!({ "type": "user", "message": { "role": "user", "content": format!("<command-name>/goal</command-name>\n<command-message>goal</command-message>\n<command-args>{instruction}</command-args>") }, "timestamp": "2026-10-01T10:00:00Z" }),
            user(format!("<local-command-stdout>Goal set: {instruction}</local-command-stdout>"), "2026-10-01T10:00:00Z"),
            meta("A session-scoped Stop hook is now active", "2026-10-01T10:00:00Z"),
            reply("On it", "2026-10-01T10:00:09Z"),
            user("and the dock icon", "2026-10-01T10:05:00Z"),
        ]);
        let t = index(&goal);
        assert_eq!(t.title, "make the whole app faster, audit every screen for jank, and…");
        assert_eq!(t.skip, None);
        // A local command before the first message doesn't name the session.
        let model = session(&dir, "d", REPO, "cli", &[
            user("<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>opus</command-args>", "2026-10-01T10:00:00Z"),
            user("<local-command-stdout>Set model to Opus</local-command-stdout>", "2026-10-01T10:00:01Z"),
            user("fix the login bug", "2026-10-01T10:00:02Z"),
            reply("Fixed", "2026-10-01T10:00:09Z"),
        ]);
        assert_eq!(index(&model).title, "fix the login bug");
    }

    #[test]
    fn titles_earlier_versions_gave_are_found_further_in() {
        let dir = Scratch::new();
        // They skipped pasted text, so a session that opens with a paste was named by a later message.
        let path = session(&dir, "a", REPO, "cli", &[
            user("<pasted_content id=\"1\">\nPort FM Season Hub to Mac\n</pasted_content>", "2026-10-01T10:00:00Z"),
            reply("Looking", "2026-10-01T10:00:05Z"),
            user("start with the memory reader", "2026-10-01T10:01:00Z"),
        ]);
        let t = index(&path);
        assert_eq!(t.title, "Port FM Season Hub to Mac");
        assert_eq!(t.legacy_title.as_deref(), Some("start with the memory reader"));
    }

    #[test]
    fn sessions_already_in_trek_are_scanned_however_old() {
        let dir = Scratch::new();
        session(&dir, "old", REPO, "cli", &[user("fix the build", "2026-01-01T10:00:00Z"), reply("done", "2026-01-01T10:00:05Z")]);
        let future = i64::MAX;
        assert!(scan_root(&dir.0, future, &HashSet::new()).is_empty());
        let found = scan_root(&dir.0, future, &HashSet::from(["old".to_string()]));
        assert_eq!(found.iter().map(|t| t.native_id.as_str()).collect::<Vec<_>>(), ["old"]);
    }

    #[test]
    fn temp_folder_runs_by_programs_are_skipped() {
        let dir = Scratch::new();
        let prompts = [user("say ok", "2026-10-01T10:00:00Z"), reply("ok", "2026-10-01T10:00:01Z"), user("again", "2026-10-01T10:00:02Z")];
        let scripted = session(&dir, "a", "/private/tmp/claude-501/scratchpad", "sdk-cli", &prompts);
        assert_eq!(index(&scripted).skip, Some(Skip::TempDir));
        // Someone working by hand in /tmp keeps their conversation.
        let by_hand = session(&dir, "b", "/private/tmp/experiment", "cli", &prompts);
        assert_eq!(index(&by_hand).skip, None);
        let one_prompt = session(&dir, "c", "/private/tmp/experiment", "cli", &prompts[..2]);
        assert_eq!(index(&one_prompt).skip, Some(Skip::TempDir));
    }

    #[test]
    fn title_generators_are_skipped() {
        let dir = Scratch::new();
        let prompt = "Generate a title that will help the user recognize this T3 Code thread weeks later.\nReturn JSON with keys title and needsRefinement.\n\nUser message:\nfix the login bug";
        let path = session(&dir, "a", REPO, "sdk-cli", &[user(prompt, "2026-10-01T10:00:00Z"), reply("{\"title\":\"Login bug\"}", "2026-10-01T10:00:01Z")]);
        assert_eq!(index(&path).skip, Some(Skip::TitleGenerator));
    }

    #[test]
    fn sessions_without_a_message_are_skipped() {
        let dir = Scratch::new();
        let commands = session(&dir, "a", REPO, "cli", &[
            user("<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>", "2026-10-01T10:00:00Z"),
            user("<local-command-stdout></local-command-stdout>", "2026-10-01T10:00:01Z"),
        ]);
        assert_eq!(index(&commands).skip, Some(Skip::NoUserMessage));
        let empty = session(&dir, "b", REPO, "cli", &[json!({ "type": "permission-mode", "permissionMode": "default" })]);
        assert_eq!(index(&empty).skip, Some(Skip::NoUserMessage));
        let mut side = user("Warmup", "2026-10-01T10:00:00Z");
        side["isSidechain"] = json!(true);
        let side_chain = session(&dir, "c", REPO, "cli", &[side]);
        assert_eq!(index(&side_chain).skip, Some(Skip::Subagent));
    }

    #[test]
    fn long_sub_agent_transcripts_are_sub_agents() {
        let dir = Scratch::new();
        // Older Claude Code kept them next to the sessions, and they run past the head.
        let lines: Vec<Value> = (0..460)
            .map(|i| {
                let mut l = if i % 2 == 0 { user("look into it", "2026-10-01T10:00:00Z") } else { reply("found it", "2026-10-01T10:00:01Z") };
                l["isSidechain"] = json!(true);
                l
            })
            .collect();
        let path = session(&dir, "agent-1234abcd", REPO, "cli", &lines);
        assert_eq!(index(&path).skip, Some(Skip::Subagent));
    }

    #[test]
    fn threads_are_dated_by_their_last_message() {
        let dir = Scratch::new();
        // Bookkeeping lines keep coming after the last message.
        let path = session(&dir, "a", REPO, "cli", &[
            user("fix the build", "2026-09-28T02:10:00Z"),
            reply("done", "2026-09-28T02:14:00Z"),
            json!({ "type": "ai-title", "aiTitle": "Build fix" }),
            json!({ "type": "last-prompt", "lastPrompt": "fix the build" }),
        ]);
        assert_eq!(index_file(&path, i64::MAX, 0, &HashSet::new()).unwrap().updated_at, ms_from_rfc3339("2026-09-28T02:14:00Z").unwrap());
        // "How far back" goes by it too.
        let cutoff = ms_from_rfc3339("2026-09-30T00:00:00Z").unwrap();
        assert!(index_file(&path, i64::MAX, cutoff, &HashSet::new()).is_none());
        assert!(index_file(&path, i64::MAX, cutoff, &HashSet::from(["a".to_string()])).is_some());
    }

    #[test]
    fn untouched_forks_are_skipped() {
        let dir = Scratch::new();
        let original = [user("plan the migration", "2026-10-01T10:00:00Z"), reply("Here's a plan", "2026-10-01T10:00:09Z")];
        session(&dir, "orig", REPO, "cli", &original);
        let copied = |from: &str| -> Vec<Value> {
            original
                .iter()
                .map(|l| {
                    let mut l = l.clone();
                    l["forkedFrom"] = json!({ "sessionId": from, "messageUuid": "m" });
                    l
                })
                .collect()
        };
        let mut lines = copied("orig");
        lines.push(json!({ "type": "custom-title", "customTitle": "plan the migration (fork)" }));
        let fork = session(&dir, "fork", REPO, "cli", &lines);
        assert_eq!(index(&fork).skip, Some(Skip::UntouchedFork));
        // Continued after forking: its own conversation now.
        lines.push(user("now do it", "2026-10-02T09:00:00Z"));
        let continued = session(&dir, "continued", REPO, "cli", &lines);
        let t = index(&continued);
        assert_eq!((t.skip, t.title.as_str()), (None, "plan the migration (fork)"));
        // The only copy left once the original is gone.
        let orphan = session(&dir, "orphan", REPO, "cli", &copied("deleted"));
        assert_eq!(index(&orphan).skip, None);
    }

    #[test]
    fn every_session_on_disk_is_known() {
        let dir = Scratch::new();
        session(&dir, "kept", REPO, "cli", &[user("hi", "2026-10-01T10:00:00Z")]);
        dir.write("-Users-me-code-app/kept/subagents/agent-1.jsonl", "");
        assert_eq!(session_ids_in(&dir.0), Some(HashSet::from(["kept".to_string()])));
        assert_eq!(session_ids_in(&dir.0.join("missing")), None);
    }

    #[test]
    fn transcripts_carry_times_and_turn_footers() {
        let dir = Scratch::new();
        let path = session(&dir, "a", REPO, "cli", &[
            user("fix the build", "2026-10-01T10:00:00Z"),
            assistant(json!([{ "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "cargo build" } }]), "tool_use", "2026-10-01T10:00:05Z"),
            user(json!([{ "type": "tool_result", "tool_use_id": "t1", "content": "error[E0425]" }]), "2026-10-01T10:00:20Z"),
            assistant(json!([{ "type": "text", "text": "Fixed the missing import." }]), "end_turn", "2026-10-01T10:00:40Z"),
            json!({ "type": "system", "subtype": "turn_duration", "durationMs": 42000, "timestamp": "2026-10-01T10:00:42Z" }),
            meta("Base directory for this skill: /tmp/skills/x", "2026-10-01T10:01:00Z"),
            user("<task-notification><task-id>1</task-id></task-notification>", "2026-10-01T10:02:00Z"),
            user("and the tests?", "2026-10-01T10:05:00Z"),
            reply("Running them", "2026-10-01T10:05:03Z"),
            user(json!([{ "type": "text", "text": "[Request interrupted by user]" }]), "2026-10-01T10:05:04Z"),
            user("never mind", "2026-10-01T10:06:00Z"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, mut l)| {
            l["uuid"] = json!(format!("l{i}"));
            l
        })
        .collect::<Vec<_>>());
        let items = load_file(&path).unwrap();
        let at = |s: &str| ms_from_rfc3339(s);
        let resume = |after: Option<&str>| Some(ResumePoint { session: "a".into(), after: after.map(String::from) });
        assert_eq!(items[0], Item::User { text: "fix the build".into(), images: vec![], at: at("2026-10-01T10:00:00Z"), resume: resume(None), aside: false });
        assert!(matches!(&items[1], Item::Tool { output, .. } if output == "error[E0425]"));
        assert_eq!(items[2], Item::Assistant { text: "Fixed the missing import.".into() });
        assert_eq!(items[3], Item::TurnEnd { at: at("2026-10-01T10:00:42Z").unwrap(), took_secs: 42 });
        // Skill bodies and task notifications aren't messages; the interrupted reply has no footer.
        assert_eq!(kinds(&items[4..]), ["user and", "assistant", "user never"]);
        // Each message can be cut off at the last line before it.
        assert!(matches!(&items[4], Item::User { resume: r, .. } if *r == resume(Some("l6"))));
        assert!(matches!(&items[6], Item::User { resume: r, .. } if *r == resume(Some("l9"))));
        // A message sent now goes after the last one.
        assert_eq!(last_message_in(&path).as_deref(), Some("l10"));
    }

    #[test]
    fn a_line_that_isnt_utf8_keeps_the_lines_after_it() {
        let dir = Scratch::new();
        let path = session(&dir, "a", REPO, "cli", &[
            user("first", "2026-10-01T10:00:00Z"),
            reply("one", "2026-10-01T10:00:01Z"),
            user("second", "2026-10-01T10:01:00Z"),
            reply("two", "2026-10-01T10:01:01Z"),
        ]);
        let text = std::fs::read_to_string(&path).unwrap();
        let (head, tail) = text.split_at(text.find('\n').unwrap() + 1);
        let mut bytes = head.as_bytes().to_vec();
        bytes.extend_from_slice(b"{\"type\":\"progress\",\"text\":\"\xff\xfe\"}\r\n");
        bytes.extend_from_slice(tail.as_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(kinds(&load_file(&path).unwrap()), ["user first", "assistant", "end 1s", "user second", "assistant", "end 1s"]);
    }

    /// Each item's kind, with a user message's first word and a footer's duration.
    fn kinds(items: &[Item]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                Item::User { text, .. } => format!("user {}", text.split(' ').next().unwrap_or_default()),
                Item::Assistant { .. } => "assistant".into(),
                Item::TurnEnd { took_secs, .. } => format!("end {took_secs}s"),
                _ => "other".into(),
            })
            .collect()
    }

    fn turn_done(at: &str, ms: u64) -> Value {
        json!({ "type": "system", "subtype": "turn_duration", "durationMs": ms, "timestamp": at })
    }

    #[test]
    fn commands_the_agent_answered_are_messages() {
        let dir = Scratch::new();
        let path = session(&dir, "a", REPO, "cli", &[
            user("what is the xcode toolchain", "2026-10-01T03:27:55Z"),
            reply("It's the compiler", "2026-10-01T03:50:27Z"),
            turn_done("2026-10-01T03:50:27Z", 1_352_000),
            user("<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args></command-args>", "2026-10-01T04:00:00Z"),
            user("<local-command-stdout>Set model to Opus</local-command-stdout>", "2026-10-01T04:00:00Z"),
            user("<command-name>/goal</command-name>\n<command-message>goal</command-message>\n<command-args>redesign the app</command-args>", "2026-10-01T04:18:22Z"),
            user("<local-command-stdout>Goal set: redesign the app</local-command-stdout>", "2026-10-01T04:18:22Z"),
            meta("A session-scoped Stop hook is now active", "2026-10-01T04:18:22Z"),
            reply("Redesigned everything", "2026-10-01T06:45:07Z"),
            turn_done("2026-10-01T06:45:07Z", 8_805_000),
        ]);
        let items = load_file(&path).unwrap();
        assert_eq!(kinds(&items), ["user what", "assistant", "end 1352s", "user /goal", "assistant", "end 8805s"]);
        assert!(matches!(&items[3], Item::User { text, at, .. } if text == "/goal redesign the app" && *at == ms_from_rfc3339("2026-10-01T04:18:22Z")));
    }

    #[test]
    fn messages_typed_mid_turn_are_shown() {
        let dir = Scratch::new();
        let queued = |prompt: &str, kind: &str, at: &str| {
            json!({ "type": "attachment", "isSidechain": false, "timestamp": at,
                    "attachment": { "type": "queued_command", "prompt": prompt, "origin": { "kind": kind }, "humanTurn": kind == "human" } })
        };
        let path = session(&dir, "a", REPO, "cli", &[
            user("set up the Xcode project", "2026-09-24T18:40:00Z"),
            assistant(json!([{ "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "xcodebuild" } }]), "tool_use", "2026-09-24T18:51:57Z"),
            json!({ "type": "queue-operation", "operation": "enqueue", "content": "I opened Xcode", "timestamp": "2026-09-24T18:51:58Z" }),
            user(json!([{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }]), "2026-09-24T18:52:19Z"),
            queued("I opened Xcode and did all the things", "human", "2026-09-24T18:51:58Z"),
            queued("<task-notification>\n<task-id>b1</task-id>\n</task-notification>", "task-notification", "2026-09-24T18:52:20Z"),
            queued("<pasted_content id=\"f8dc\">\ncrash report\n</pasted_content>", "human", "2026-09-24T18:53:00Z"),
            reply("Built it", "2026-09-24T19:00:00Z"),
            turn_done("2026-09-24T19:00:00Z", 1_200_000),
            // Taken in as the next message instead: written as a user line too, shown once.
            queued("now run it", "human", "2026-09-24T19:01:00Z"),
            user("now run it", "2026-09-24T19:01:00Z"),
            reply("Running", "2026-09-24T19:01:05Z"),
            turn_done("2026-09-24T19:01:05Z", 5_000),
        ]);
        let items = load_file(&path).unwrap();
        assert_eq!(kinds(&items), ["user set", "other", "user I", "user crash", "assistant", "end 1200s", "user now", "assistant", "end 5s"]);
        assert!(matches!(&items[2], Item::User { aside: true, at, .. } if *at == ms_from_rfc3339("2026-09-24T18:51:58Z")));
        assert!(matches!(&items[3], Item::User { text, .. } if text == "crash report"));
        assert!(matches!(&items[6], Item::User { aside: false, .. }));
        assert_eq!(index(&path).skip, None);
    }

    #[test]
    fn background_reports_start_turns_of_their_own() {
        let dir = Scratch::new();
        let notification = |task: &str, tool: &str, at: &str| {
            user(format!("<task-notification>\n<task-id>{task}</task-id>\n<tool-use-id>{tool}</tool-use-id>\n<status>completed</status>\n</task-notification>"), at)
        };
        let launch = |id: &str, name: &str, at: &str| assistant(json!([{ "type": "tool_use", "id": id, "name": name, "input": { "run_in_background": true } }]), "tool_use", at);
        let mut launched = user(json!([{ "type": "tool_result", "tool_use_id": "agent-1", "content": "Async agent launched successfully." }]), "2026-10-01T10:00:02Z");
        launched["toolUseResult"] = json!({ "isAsync": true, "status": "async_launched" });
        let path = session(&dir, "a", REPO, "cli", &[
            user("record the match", "2026-10-01T10:00:00Z"),
            launch("bash-1", "Bash", "2026-10-01T10:00:01Z"),
            user(json!([{ "type": "tool_result", "tool_use_id": "bash-1", "content": "Command running in background" }]), "2026-10-01T10:00:01Z"),
            reply("Recording", "2026-10-01T10:00:05Z"),
            turn_done("2026-10-01T10:00:05Z", 5_000),
            // Hours later: the command reports back and the agent answers on its own.
            notification("b1", "bash-1", "2026-10-01T16:15:22Z"),
            reply("The recording finished", "2026-10-01T16:35:05Z"),
            turn_done("2026-10-01T16:35:05Z", 1_183_000),
            user("research it", "2026-10-01T17:00:00Z"),
            launch("agent-1", "Agent", "2026-10-01T17:00:01Z"),
            launched,
            reply("An agent is on it", "2026-10-01T17:00:03Z"),
            turn_done("2026-10-01T17:00:03Z", 3_000),
            // A sub-agent's report continues the reply that launched it, as live.
            notification("a1", "agent-1", "2026-10-01T17:10:00Z"),
            reply("Here's what it found", "2026-10-01T17:10:30Z"),
            turn_done("2026-10-01T17:10:30Z", 30_000),
        ]);
        let items = load_file(&path).unwrap();
        let ends: Vec<u32> = items.iter().filter_map(|i| if let Item::TurnEnd { took_secs, .. } = i { Some(*took_secs) } else { None }).collect();
        assert_eq!(ends, [5, 1183, 630]);
    }

    #[test]
    fn usage_counts_each_response_once() {
        let dir = Scratch::new();
        // As Claude Code writes it: a line per content block, each with the response's usage
        // (the shape of a real session's lines, trimmed).
        let response = |id: &str, model: &str, out: u64, at: &str| {
            json!({ "type": "assistant", "timestamp": at, "message": { "id": id, "role": "assistant", "model": model, "content": [{ "type": "text", "text": "…" }],
                "usage": { "input_tokens": 10, "cache_creation_input_tokens": 8324, "cache_read_input_tokens": 13803, "output_tokens": out,
                    "output_tokens_details": { "thinking_tokens": 35 }, "service_tier": "standard" } } })
        };
        let path = session(&dir, "s", REPO, "cli", &[
            user("hi", "2026-10-03T15:15:29.000Z"),
            response("msg_1", "claude-haiku-4-5-20251001", 12, "2026-10-03T15:15:30.032Z"),
            response("msg_1", "claude-haiku-4-5-20251001", 41, "2026-10-03T15:15:30.037Z"),
            response("msg_2", "claude-opus-5-5", 7, "2026-10-03T15:16:00.000Z"),
            // An API error: nothing used.
            json!({ "type": "assistant", "timestamp": "2026-10-03T15:17:00.000Z", "message": { "id": "x", "model": "<synthetic>", "content": [], "usage": { "input_tokens": 0, "output_tokens": 0 } } }),
            // Yesterday.
            response("msg_0", "claude-opus-5-5", 99, "2026-10-02T10:00:00.000Z"),
        ]);
        let from = ms_from_rfc3339("2026-10-03T00:00:00Z").unwrap();
        let got = usage_in(&path, from, from + 86_400_000);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], (ms_from_rfc3339("2026-10-03T15:15:30.037Z").unwrap(), Some("claude-haiku-4-5-20251001".into()), TokenUsage { input: 10, output: 41, cache_read: 13803, cache_write: 8324 }));
        assert_eq!(got[1].1.as_deref(), Some("claude-opus-5-5"));
    }
}
