//! Codex threads: index in `~/.codex/state_5.sqlite`, transcripts in rollout JSONL files.

use super::{
    Evidence, ImportedThread, Transcript, classify, clip, codex_attachments, codex_request, copies_message, is_injected, is_temp_dir,
    is_title_request, legacy_title_from, ms_from_rfc3339, source_title, strip_block, title_from, user_text,
};
use crate::store::{Item, ToolStatus};
use crate::types::{Effort, ThreadSource};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

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

/// Threads updated since `min_updated`, and the ones in `held` (already in Trek) whatever their age.
pub fn scan(min_updated: i64, held: &HashSet<String>) -> Vec<ImportedThread> {
    db().map(|conn| scan_conn(&conn, min_updated, held)).unwrap_or_default()
}

struct Row {
    id: String,
    cwd: Option<String>,
    title: Option<String>,
    first: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    branch: Option<String>,
    created: i64,
    updated: i64,
    source: String,
    originator: String,
    thread_source: String,
    name: Option<String>,
    agent_role: String,
    rollout: String,
}

fn scan_conn(conn: &Connection, min_updated: i64, held: &HashSet<String>) -> Vec<ImportedThread> {
    // Columns arrived over Codex versions; read the ones this database has.
    let columns: HashSet<String> = conn
        .prepare("SELECT name FROM pragma_table_info('threads')")
        .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0))?.collect())
        .unwrap_or_default();
    let col = |name: &str, fallback: &str| if columns.contains(name) { name.to_string() } else { fallback.to_string() };
    let ms = |name: &str| {
        if columns.contains(&format!("{name}_ms")) { format!("COALESCE({name}_ms, {name} * 1000)") } else { format!("{name} * 1000") }
    };
    let sql = format!(
        "SELECT id, cwd, title, {first}, {model}, {effort}, git_branch, {created}, {updated}, source, {originator}, {thread_source}, {name}, {role}, rollout_path
         FROM threads WHERE archived = 0 AND tokens_used > 0 AND ({updated} >= ?1 OR id IN (SELECT value FROM json_each(?2)))",
        first = col("first_user_message", "NULL"),
        model = col("model", "NULL"),
        effort = col("reasoning_effort", "NULL"),
        created = ms("created_at"),
        updated = ms("updated_at"),
        originator = col("originator", "NULL"),
        thread_source = col("thread_source", "NULL"),
        name = col("name", "NULL"),
        role = col("agent_role", "NULL"),
    );
    let Ok(mut st) = conn.prepare(&sql) else { return vec![] };
    let held = serde_json::to_string(held).unwrap_or_else(|_| "[]".into());
    let rows = st.query_map(rusqlite::params![min_updated, held], |r| {
        Ok(Row {
            id: r.get(0)?,
            cwd: r.get(1)?,
            title: r.get(2)?,
            first: r.get(3)?,
            model: r.get(4)?,
            effort: r.get(5)?,
            branch: r.get(6)?,
            created: r.get(7)?,
            updated: r.get(8)?,
            source: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
            originator: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
            thread_source: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
            name: r.get(12)?,
            agent_role: r.get::<_, Option<String>>(13)?.unwrap_or_default(),
            rollout: r.get(14)?,
        })
    });
    match rows {
        Ok(rows) => rows.filter_map(Result::ok).map(|row| thread_from(&row)).collect(),
        Err(_) => vec![],
    }
}

/// Clients that are Codex's own interactive UIs (terminal, desktop app, IDE extension).
fn is_interactive(source: &str, originator: &str) -> bool {
    source == "cli" || ["Codex Desktop", "codex_vscode", "codex-tui", "codex_cli_rs"].contains(&originator)
}

fn thread_from(row: &Row) -> ImportedThread {
    let cwd = row.cwd.as_deref().map(PathBuf::from);
    let first = row.first.as_deref().filter(|f| !f.trim().is_empty());
    let interactive = is_interactive(&row.source, &row.originator);
    // `codex exec` from a script or another agent; the Codex SDK drives exec for app conversations.
    let cli_run = row.source == "exec" && !row.originator.starts_with("codex_sdk");
    // Counting prompts means reading the rollout: only when it decides a rule.
    let count_matters = cli_run || first.is_some_and(is_title_request) || (cwd.as_deref().is_some_and(is_temp_dir) && !interactive);
    let prompts = count_matters.then(|| count_prompts(Path::new(&row.rollout))).flatten();
    let skip = classify(&Evidence {
        cwd: cwd.as_deref(),
        scripted: !interactive,
        prompts,
        first_message: first,
        // It used tokens: the model answered.
        replied: true,
        subagent: !row.agent_role.is_empty() || row.thread_source == "subagent" || row.source.contains("subagent"),
        untouched_fork: false,
        trek: row.originator == "trek",
        cli_run,
    });
    // The thread's name (set by the user or by Codex), else its stored title, else the first
    // message. Until Codex names a thread, both hold the start of the first message, cut mid-word.
    let title = [row.name.as_deref(), row.title.as_deref()]
        .into_iter()
        .flatten()
        .filter(|t| !first.is_some_and(|f| copies_message(t, f)))
        .find_map(source_title)
        .or_else(|| first.and_then(user_text).map(|t| title_from(&t)))
        .unwrap_or_else(|| "Codex thread".into());
    let legacy_title = row
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| row.first.as_deref().map(legacy_title_from))
        .unwrap_or_else(|| "Codex thread".into());
    ImportedThread {
        source: ThreadSource::Codex,
        native_id: row.id.clone(),
        title,
        cwd,
        branch: row.branch.clone(),
        model: row.model.clone(),
        effort: row.effort.as_deref().and_then(Effort::parse),
        created_at: row.created,
        updated_at: row.updated,
        additions: 0,
        deletions: 0,
        skip,
        legacy_title: Some(legacy_title),
    }
}

/// Prompts the user typed in a rollout, up to 2 (enough to tell a one-shot run).
fn count_prompts(path: &Path) -> Option<usize> {
    let reader = BufReader::new(std::fs::File::open(path).ok()?);
    let mut n = 0;
    for line in reader.lines().map_while(Result::ok) {
        if !line.contains("\"role\":\"user\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let p = &v["payload"];
        if v["type"] == "response_item" && p["type"] == "message" && p["role"] == "user" && user_text(&message_text(&p["content"])).is_some() {
            n += 1;
            if n > 1 {
                break;
            }
        }
    }
    Some(n)
}

/// Every thread Codex has, archived ones included; `None` when its index can't be read.
pub(crate) fn session_ids() -> Option<HashSet<String>> {
    let conn = db()?;
    let mut st = conn.prepare("SELECT id FROM threads").ok()?;
    st.query_map([], |r| r.get::<_, String>(0)).ok()?.collect::<rusqlite::Result<_>>().ok()
}

fn rollout_path(id: &str) -> Option<PathBuf> {
    let conn = db()?;
    conn.query_row("SELECT rollout_path FROM threads WHERE id = ?1", [id], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .map(PathBuf::from)
}

/// Joined text blocks, skipping injected context blocks (environment, instructions) and the
/// in-app browser state the desktop app attaches.
fn message_text(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["text"].as_str())
        .map(|t| {
            if !t.contains("<in-app-browser-context") {
                return t.to_string();
            }
            let t = strip_block(t, "in-app-browser-context");
            let t = t.trim();
            t.strip_prefix("## My request:").unwrap_or(t).trim().to_string()
        })
        .filter(|t| !is_injected(t))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A user message as it was sent: the request without the list of files the desktop app puts
/// above it, and the attached images that are still on disk. Nothing typed: the files' names.
fn sent_message(text: &str) -> (String, Vec<String>) {
    let files = codex_attachments(text);
    if files.is_empty() {
        return (text.to_string(), vec![]);
    }
    let image = |path: &str| {
        let ext = Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        ["png", "jpg", "jpeg", "gif", "webp", "heic"].contains(&ext.as_str())
    };
    let images: Vec<String> = files.iter().map(|(_, p)| *p).filter(|p| image(p) && Path::new(p).is_file()).map(String::from).collect();
    let request = codex_request(text);
    let request = if request.trim().is_empty() && images.is_empty() {
        files.iter().map(|(name, _)| name.trim_matches('"')).collect::<Vec<_>>().join("\n")
    } else {
        request
    };
    (request, images)
}

/// The user's answers to questions the agent asked (`request_user_input`), one per line.
fn question_reply(text: &str) -> Option<String> {
    let body = text.strip_prefix("<send_user_message_question_reply>")?;
    let body = body.split("</send_user_message_question_reply>").next()?.trim();
    let answers: Vec<Value> = serde_json::from_str(body).ok()?;
    let lines: Vec<&str> = answers.iter().filter_map(|a| a["answer"].as_str()).filter(|a| !a.trim().is_empty()).collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

pub fn load(id: &str) -> anyhow::Result<Vec<Item>> {
    let path = rollout_path(id).ok_or_else(|| anyhow::anyhow!("rollout not found"))?;
    load_rollout(&path)
}

fn load_rollout(path: &Path) -> anyhow::Result<Vec<Item>> {
    let reader = BufReader::new(std::fs::File::open(path)?);
    let mut t = Transcript::default();
    // A goal the user set from the goal bar rather than in a message, until its turn starts.
    let mut goal: Option<(String, Option<i64>)> = None;
    // The goal shown for the current turn: typed as "/goal …", the message itself is recorded
    // after the turn it started is under way.
    let mut goal_shown: Option<String> = None;
    for line in reader.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let at = v["timestamp"].as_str().and_then(ms_from_rfc3339);
        let p = &v["payload"];
        if v["type"] == "event_msg" {
            match p["type"].as_str() {
                // Every turn starts with this; ones without a message (a goal continuing) too.
                Some("task_started") => {
                    goal_shown = None;
                    t.wake(at);
                }
                Some("task_complete") => t.complete(at),
                Some("turn_aborted") => t.interrupt(),
                Some("thread_goal_updated") => {
                    let g = &p["goal"];
                    // Progress and pauses update the same goal; a new one starts unused.
                    if g["tokensUsed"].as_i64() == Some(0) && g["createdAt"] == g["updatedAt"] {
                        goal = g["objective"].as_str().filter(|o| !o.trim().is_empty()).map(|o| (format!("/goal {}", o.trim()), at));
                    }
                }
                _ => {}
            }
            continue;
        }
        if v["type"] != "response_item" {
            continue;
        }
        match p["type"].as_str() {
            Some("message") => {
                let text = message_text(&p["content"]);
                match p["role"].as_str() {
                    Some("user") => {
                        let first = p["content"][0]["text"].as_str().unwrap_or_default().trim_start();
                        if let Some(answers) = question_reply(first) {
                            t.answer(answers, at);
                        } else if first.starts_with("<codex_internal_context source=\"goal\"") {
                            // The turn a new goal started: show the goal as the user set it.
                            if let Some((objective, set_at)) = goal.take() {
                                goal_shown = Some(objective.clone());
                                t.user(objective, set_at.or(at));
                            }
                        } else if !text.trim().is_empty() {
                            goal = None;
                            if goal_shown.take().is_some_and(|g| g == text.trim()) {
                                continue;
                            }
                            let (text, images) = sent_message(&text);
                            t.user_with(text, images, at);
                        }
                    }
                    Some("assistant") => {
                        if !text.trim().is_empty() {
                            t.push(Item::Assistant { text });
                        }
                        t.activity(at);
                    }
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
                    t.push(Item::Reasoning { text });
                }
                t.activity(at);
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
                t.push(Item::Tool {
                    id: p["call_id"].as_str().unwrap_or_default().to_string(),
                    title,
                    detail,
                    output: String::new(),
                    status: ToolStatus::Done,
                });
                t.activity(at);
            }
            Some("function_call_output") | Some("custom_tool_call_output") => {
                let id = p["call_id"].as_str().unwrap_or_default();
                let out = match &p["output"] {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if let Some(Item::Tool { output, .. }) =
                    t.items.iter_mut().rev().find(|i| matches!(i, Item::Tool { id: tid, .. } if tid == id))
                {
                    *output = clip(&out, 4000);
                }
                t.touch(at);
            }
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

    const SCHEMA: &str = "CREATE TABLE threads (
        id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
        source TEXT NOT NULL, cwd TEXT NOT NULL, title TEXT NOT NULL, tokens_used INTEGER NOT NULL DEFAULT 0,
        archived INTEGER NOT NULL DEFAULT 0, git_branch TEXT, first_user_message TEXT NOT NULL DEFAULT '', agent_role TEXT,
        model TEXT, reasoning_effort TEXT, created_at_ms INTEGER, updated_at_ms INTEGER, thread_source TEXT, name TEXT, originator TEXT)";

    fn line(kind: &str, payload: Value, at: &str) -> String {
        json!({ "timestamp": at, "type": kind, "payload": payload }).to_string()
    }

    fn said(role: &str, text: &str, at: &str) -> String {
        let block = if role == "user" { "input_text" } else { "output_text" };
        line("response_item", json!({ "type": "message", "role": role, "content": [{ "type": block, "text": text }] }), at)
    }

    struct Fixture {
        dir: Scratch,
        conn: Connection,
    }

    impl Fixture {
        fn new() -> Fixture {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            Fixture { dir: Scratch::new(), conn }
        }

        /// A thread row and its rollout with the given user prompts (each answered).
        fn thread(&self, id: &str, cols: &[(&str, &str)], prompts: &[&str]) {
            let mut lines = vec![said("user", "<environment_context>\n  <cwd>/x</cwd>\n</environment_context>", "2026-09-06T22:15:24Z")];
            for p in prompts {
                lines.push(said("user", p, "2026-09-06T22:15:25Z"));
                lines.push(said("assistant", "done", "2026-09-06T22:15:30Z"));
                lines.push(line("event_msg", json!({ "type": "task_complete" }), "2026-09-06T22:15:31Z"));
            }
            let rollout = self.dir.write(&format!("rollout-{id}.jsonl"), &lines.join("\n"));
            let mut names = vec!["id", "rollout_path", "created_at", "updated_at", "source", "cwd", "title", "tokens_used", "first_user_message"];
            let rollout = rollout.display().to_string();
            let first = prompts.first().copied().unwrap_or_default();
            let mut values = vec![id, rollout.as_str(), "1", "2", "vscode", "/Users/me/app", first, "1000", first];
            for (k, v) in cols {
                match names.iter().position(|n| n == k) {
                    Some(i) => values[i] = v,
                    None => {
                        names.push(k);
                        values.push(v);
                    }
                }
            }
            let marks = vec!["?"; values.len()].join(",");
            self.conn
                .execute(&format!("INSERT INTO threads ({}) VALUES ({marks})", names.join(",")), rusqlite::params_from_iter(values))
                .unwrap();
        }

        fn scan(&self) -> Vec<ImportedThread> {
            let mut v = scan_conn(&self.conn, 0, &HashSet::new());
            v.sort_by(|a, b| a.native_id.cmp(&b.native_id));
            v
        }

        fn get(&self, id: &str) -> ImportedThread {
            self.scan().into_iter().find(|t| t.native_id == id).expect(id)
        }
    }

    #[test]
    fn titles_use_the_thread_name_then_the_request() {
        let f = Fixture::new();
        f.thread("named", &[("name", "Optimize WiFi coverage")], &["/goal make my WiFi coverage the best it can possibly be"]);
        f.thread("copied", &[], &["# Files mentioned by the user:\n\n## shot.png: /var/folders/x/shot.png\n\n## My request:\nwhy does it crash on launch?"]);
        f.thread("titled", &[("title", "Review workflow skills")], &["Look through my usage from all time"]);
        f.thread("goal", &[], &["/goal make my WiFi coverage the best it can possibly be"]);
        // Until Codex names a thread, its name is the first message cut mid-word.
        let request = "I've gotten Players accept contract to work(playing role and contract), I also have both reports";
        f.thread("cut", &[("name", "I've gotten Players accept contract to work(playing role an…")], &[request]);
        let attached = "# Files mentioned by the user:\n\n## shot.png: /var/folders/x/shot.png\n\n## My request:\nI really don't like two things, how codex is called and the icon";
        f.thread("cut-attached", &[("name", "I really don't like two things, how codex is called and t…")], &[attached]);
        assert_eq!(f.get("named").title, "Optimize WiFi coverage");
        assert_eq!(f.get("cut").title, "I've gotten Players accept contract to work(playing role…");
        assert_eq!(f.get("cut-attached").title, "I really don't like two things, how codex is called and the…");
        assert_eq!(f.get("copied").title, "why does it crash on launch?");
        assert_eq!(f.get("titled").title, "Review workflow skills");
        assert_eq!(f.get("goal").title, "make my WiFi coverage the best it can possibly be");
        // Earlier versions used the stored title as it was.
        assert_eq!(f.get("copied").legacy_title.as_deref().map(|t| t.lines().next().unwrap()), Some("# Files mentioned by the user:"));
    }

    #[test]
    fn helper_threads_are_recognised() {
        let f = Fixture::new();
        f.thread("conversation", &[], &["fix the build", "and the tests"]);
        f.thread("subagent", &[("source", r#"{"subagent":{"thread_spawn":{"parent_thread_id":"p"}}}"#), ("thread_source", "subagent")], &["map the code"]);
        f.thread("role", &[("agent_role", "code_mapper")], &["map the code"]);
        f.thread("trek", &[("originator", "trek")], &["Reply with just the word: pong"]);
        f.thread("exec", &[("source", "exec"), ("originator", "Codex Desktop")], &["Return exactly OK without tools."]);
        // Codex 0.155+ sends the instruction file as a message of its own; it isn't a prompt.
        let instructions = "# AGENTS.md instructions\n\n<INSTRUCTIONS>\nBe brief.\n</INSTRUCTIONS>";
        f.thread("exec-new", &[("source", "exec"), ("first_user_message", "Return exactly OVERRIDE_OK without tools.")], &[instructions, "Return exactly OVERRIDE_OK without tools."]);
        f.thread("exec-resumed", &[("source", "exec")], &["audit the config", "now fix it"]);
        f.thread("sdk", &[("source", "exec"), ("originator", "codex_sdk_ts")], &["build the settings page"]);
        f.thread("tmp", &[("cwd", "/private/tmp/fm26-workflow-ab/new"), ("originator", "t3code_desktop")], &["diagnose", "and fix"]);
        f.thread("tmp-tui", &[("cwd", "/tmp/scratch"), ("source", "cli")], &["try this", "and that"]);
        let skips: Vec<(String, Option<Skip>)> = f.scan().into_iter().map(|t| (t.native_id, t.skip)).collect();
        assert_eq!(
            skips,
            vec![
                ("conversation".into(), None),
                ("exec".into(), Some(Skip::OneShotRun)),
                ("exec-new".into(), Some(Skip::OneShotRun)),
                ("exec-resumed".into(), None),
                ("role".into(), Some(Skip::Subagent)),
                ("sdk".into(), None),
                ("subagent".into(), Some(Skip::Subagent)),
                ("tmp".into(), Some(Skip::TempDir)),
                ("tmp-tui".into(), None),
                ("trek".into(), Some(Skip::Trek)),
            ]
        );
    }

    #[test]
    fn older_state_databases_still_scan() {
        let f = Fixture::new();
        f.conn
            .execute_batch(
                "DROP TABLE threads; CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL, source TEXT NOT NULL, cwd TEXT NOT NULL, title TEXT NOT NULL, tokens_used INTEGER NOT NULL DEFAULT 0,
                 archived INTEGER NOT NULL DEFAULT 0, git_branch TEXT, first_user_message TEXT NOT NULL DEFAULT '')",
            )
            .unwrap();
        f.thread("old", &[], &["fix the build"]);
        let t = f.get("old");
        assert_eq!((t.title.as_str(), t.created_at, t.skip), ("fix the build", 1000, None));
    }

    #[test]
    fn threads_already_in_trek_are_scanned_however_old() {
        let f = Fixture::new();
        f.thread("old", &[], &["fix the build"]);
        assert!(scan_conn(&f.conn, i64::MAX, &HashSet::new()).is_empty());
        assert_eq!(scan_conn(&f.conn, i64::MAX, &HashSet::from(["old".to_string()])).len(), 1);
    }

    #[test]
    fn goals_answers_and_browser_requests_are_messages() {
        let dir = Scratch::new();
        let event = |payload: Value, at: &str| line("event_msg", payload, at);
        let started = |at: &str| event(json!({ "type": "task_started" }), at);
        let complete = |at: &str| event(json!({ "type": "task_complete" }), at);
        let goal = |objective: &str, tokens: i64, at: &str| {
            event(json!({ "type": "thread_goal_updated", "goal": { "objective": objective, "status": "active", "tokensUsed": tokens, "createdAt": 100, "updatedAt": if tokens == 0 { 100 } else { 200 } } }), at)
        };
        let nudge = |at: &str| said("user", "<codex_internal_context source=\"goal\">\nContinue working toward the active thread goal.\n</codex_internal_context>", at);
        let lines = [
            said("user", "# AGENTS.md instructions\n\n<INSTRUCTIONS>be brief</INSTRUCTIONS>", "2026-08-20T08:00:00Z"),
            started("2026-08-20T08:02:00Z"),
            said("user", "fix the staff editor", "2026-08-20T08:02:00Z"),
            said("user", "<send_user_message_question_reply>\n[{\"question\":\"Which build?\",\"answer\":\"The release one\"}]\n</send_user_message_question_reply>", "2026-08-20T08:03:00Z"),
            said("assistant", "Fixed.", "2026-08-20T09:05:00Z"),
            complete("2026-08-20T09:05:01Z"),
            // A goal set from the goal bar, no message: its own turn.
            goal("now get staff history working", 0, "2026-08-20T09:20:56Z"),
            started("2026-08-20T09:20:56Z"),
            nudge("2026-08-20T09:20:56Z"),
            said("assistant", "History works.", "2026-08-20T10:18:49Z"),
            complete("2026-08-20T10:18:49Z"),
            // Codex keeps going on the goal hours later: a turn of its own, timed from then.
            goal("now get staff history working", 900, "2026-08-21T00:37:32Z"),
            started("2026-08-21T00:37:32Z"),
            nudge("2026-08-21T00:37:32Z"),
            said("assistant", "Goal complete.", "2026-08-21T00:37:34Z"),
            complete("2026-08-21T00:37:34Z"),
            started("2026-08-21T01:00:00Z"),
            said("user", "\n<in-app-browser-context source=\"ambient-ui-state\">\n- Current URL: https://kits.dev/\n</in-app-browser-context>\n\n## My request:\nadd these kits", "2026-08-21T01:00:00Z"),
            said("assistant", "Added.", "2026-08-21T01:01:00Z"),
            complete("2026-08-21T01:01:00Z"),
        ];
        let path = dir.write("rollout.jsonl", &lines.join("\n"));
        let items = load_rollout(&path).unwrap();
        let shown: Vec<String> = items
            .iter()
            .map(|i| match i {
                Item::User { text, .. } => format!("user: {text}"),
                Item::Assistant { text } => format!("agent: {text}"),
                Item::TurnEnd { took_secs, .. } => format!("took {took_secs}s"),
                _ => "other".into(),
            })
            .collect();
        assert_eq!(
            shown,
            [
                "user: fix the staff editor",
                "user: The release one",
                "agent: Fixed.",
                "took 3781s",
                "user: /goal now get staff history working",
                "agent: History works.",
                "took 3473s",
                "agent: Goal complete.",
                "took 2s",
                "user: add these kits",
                "agent: Added.",
                "took 60s",
            ]
        );
        assert!(matches!(&items[4], Item::User { at, .. } if *at == ms_from_rfc3339("2026-08-20T09:20:56Z")));
    }

    #[test]
    fn typed_goals_and_attachments_show_as_sent() {
        let dir = Scratch::new();
        let shot = dir.write("shot.png", "png");
        let shot = shot.display().to_string();
        let event = |payload: Value, at: &str| line("event_msg", payload, at);
        let lines = [
            // "/goal …" typed in the composer: the goal starts its turn, and the message is recorded
            // once the turn is under way.
            event(json!({ "type": "thread_goal_updated", "goal": { "objective": "make my WiFi the best it can be", "tokensUsed": 0, "createdAt": 1, "updatedAt": 1 } }), "2026-08-17T02:01:56Z"),
            event(json!({ "type": "task_started" }), "2026-08-17T02:01:56Z"),
            said("user", "<codex_internal_context source=\"goal\">\nContinue working toward the active thread goal.\n</codex_internal_context>", "2026-08-17T02:01:56Z"),
            said("assistant", "I'll inventory the access points first.", "2026-08-17T02:02:00Z"),
            said("user", "/goal make my WiFi the best it can be\n", "2026-08-17T02:02:00Z"),
            said("assistant", "Scanning.", "2026-08-17T02:06:00Z"),
            event(json!({ "type": "task_complete" }), "2026-08-17T02:06:00Z"),
            // Screenshots and files listed above the request; one screenshot is gone from disk.
            said("user", &format!("\n# Files mentioned by the user:\n\n## shot.png: {shot}\n\n## gone.png: /var/folders/x/T/gone.png\n\n## Editor.app: /Users/me/Editor.app\n\n## My request:\nthe header is misaligned\n"), "2026-08-17T03:00:00Z"),
            said("assistant", "Fixed.", "2026-08-17T03:01:00Z"),
            event(json!({ "type": "task_complete" }), "2026-08-17T03:01:00Z"),
            // Nothing typed, nothing left to show: the files' names.
            said("user", "# Files pasted by the user:\n\n## \"--- Translated Report ---\": /Users/me/.codex/attachments/a/pasted-text.txt\n\n## My request:\n", "2026-08-17T04:00:00Z"),
            said("assistant", "That's a crash in the loader.", "2026-08-17T04:00:30Z"),
            event(json!({ "type": "task_complete" }), "2026-08-17T04:00:30Z"),
        ];
        let path = dir.write("rollout.jsonl", &lines.join("\n"));
        let users: Vec<(String, Vec<String>)> = load_rollout(&path)
            .unwrap()
            .into_iter()
            .filter_map(|i| if let Item::User { text, images, .. } = i { Some((text, images)) } else { None })
            .collect();
        assert_eq!(
            users,
            [
                ("/goal make my WiFi the best it can be".to_string(), vec![]),
                ("the header is misaligned".to_string(), vec![shot]),
                ("--- Translated Report ---".to_string(), vec![]),
            ]
        );
    }

    #[test]
    fn transcripts_carry_times_and_turn_footers() {
        let dir = Scratch::new();
        let lines = [
            said("user", "# AGENTS.md instructions for /repo\n\n<INSTRUCTIONS>be brief</INSTRUCTIONS>", "2026-09-06T22:15:24.000Z"),
            said("user", "<environment_context><cwd>/repo</cwd></environment_context>", "2026-09-06T22:15:24.000Z"),
            said("user", "reset eduroam", "2026-09-06T22:15:24.832Z"),
            line("response_item", json!({ "type": "function_call", "name": "exec_command", "call_id": "c1", "arguments": "{\"cmd\":\"ls\"}" }), "2026-09-06T22:15:30Z"),
            line("response_item", json!({ "type": "function_call_output", "call_id": "c1", "output": "a b" }), "2026-09-06T22:15:31Z"),
            said("assistant", "Done: forget the network.", "2026-09-06T22:16:29.090Z"),
            line("event_msg", json!({ "type": "task_complete" }), "2026-09-06T22:16:29.201Z"),
            said("user", "do it for me", "2026-09-06T22:17:08Z"),
            said("assistant", "Working on it", "2026-09-06T22:17:11Z"),
            line("event_msg", json!({ "type": "turn_aborted" }), "2026-09-06T22:17:20Z"),
            said("user", "<turn_aborted>The user interrupted the previous turn.</turn_aborted>", "2026-09-06T22:17:30Z"),
        ];
        let path = dir.write("rollout.jsonl", &lines.join("\n"));
        let items = load_rollout(&path).unwrap();
        assert_eq!(items[0], Item::User { text: "reset eduroam".into(), images: vec![], at: ms_from_rfc3339("2026-09-06T22:15:24.832Z") });
        assert!(matches!(&items[1], Item::Tool { title, detail, output, .. } if title == "Ran command" && detail == "ls" && output == "a b"));
        assert_eq!(items[3], Item::TurnEnd { at: ms_from_rfc3339("2026-09-06T22:16:29.201Z").unwrap(), took_secs: 64 });
        assert!(matches!(&items[4], Item::User { text, .. } if text == "do it for me"));
        assert_eq!(items.len(), 6, "the aborted turn has no footer: {items:?}");
    }
}
