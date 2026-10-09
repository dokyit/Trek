//! OpenCode sessions: `~/.local/share/opencode/opencode.db`. OpenCode 1.x keeps them in
//! `session`, `message` and `part`; OpenCode 2 in `session_v2` and `session_message` (one row per
//! message, its parts inside). 2.0 copies 1.x's sessions into its own tables once, under the same
//! ids, and leaves the old tables as they were, so a database may have both, and a session may be
//! in both: the copy that changed last is the one read.
//!
//! 1.x won't open a database 2.0 created ("Database is not empty and has no session table": it
//! only migrates one that has its `session` table), so Trek starts 1.x on `opencode-1x.db` beside
//! it then (`db_for_1x`). Both files are read.

use super::{Evidence, ImportedThread, Transcript, classify, clip, source_title, title_from, user_text};
use crate::store::{Item, ToolStatus};
use super::UsageEntry;
use crate::types::{Effort, ThreadSource, TokenUsage};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// OpenCode's data folder (`$XDG_DATA_HOME/opencode`, as OpenCode finds it).
fn data_dir() -> PathBuf {
    let xdg = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute());
    xdg.unwrap_or_else(|| crate::paths::home().join(".local/share")).join("opencode")
}

/// The database OpenCode uses unless told otherwise.
const MAIN_DB: &str = "opencode.db";
/// The one Trek gives OpenCode 1.x when 2.0 has made `opencode.db` (see the top of this file).
const DB_1X: &str = "opencode-1x.db";

fn open(path: &Path) -> Option<Connection> {
    // Opening read-only fails rather than creating a missing file.
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()
}

/// OpenCode's databases that exist: the main one, then 1.x's own beside it.
fn dbs() -> Vec<Connection> {
    dbs_in(&data_dir())
}

fn dbs_in(dir: &Path) -> Vec<Connection> {
    [MAIN_DB, DB_1X].iter().map(|f| dir.join(f)).filter(|p| p.is_file()).filter_map(|p| open(&p)).collect()
}

/// The database session `id` is in.
fn db_of(id: &str) -> Option<Connection> {
    dbs().into_iter().find(|c| tables_of(c, id).is_some())
}

/// Where OpenCode 1.x should keep its sessions, when that isn't `opencode.db`: 2.0 made that
/// one (it has tables, none of them 1.x's `session`), or it's gone and 1.x already has its own.
/// `None`: 1.x opens `opencode.db` as usual (it's 1.x's, or there's none yet).
pub fn db_for_1x() -> Option<PathBuf> {
    db_for_1x_in(&data_dir())
}

fn db_for_1x_in(dir: &Path) -> Option<PathBuf> {
    let (main, own) = (dir.join(MAIN_DB), dir.join(DB_1X));
    let names: Vec<String> = match main.is_file().then(|| open(&main)).flatten() {
        Some(conn) => conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
            .and_then(|mut st| st.query_map([], |r| r.get(0))?.collect())
            // Unreadable: leave it to 1.x.
            .ok()?,
        None => vec![],
    };
    if names.iter().any(|n| n == "session") {
        None
    } else if !names.is_empty() || own.is_file() {
        Some(own)
    } else {
        None
    }
}

/// Which of OpenCode's tables a session is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tables {
    /// OpenCode 1.x: `session`, `message`, `part`.
    V1,
    /// OpenCode 2: `session_v2`, `session_message`.
    V2,
}

impl Tables {
    fn session(self) -> &'static str {
        match self {
            Tables::V1 => "session",
            Tables::V2 => "session_v2",
        }
    }
}

/// The tables this database has: 1.x's, 2's, or both.
fn tables(conn: &Connection) -> Vec<Tables> {
    let has = |name: &str| conn.query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", [name], |_| Ok(())).is_ok();
    let mut out = vec![];
    if has("session") && has("message") && has("part") {
        out.push(Tables::V1);
    }
    if has("session_v2") && has("session_message") {
        out.push(Tables::V2);
    }
    out
}

/// Where session `id` is read from: the tables whose copy changed last (1.x's on a tie: a copy
/// 2.0 made and never continued is the same conversation).
fn tables_of(conn: &Connection, id: &str) -> Option<Tables> {
    tables(conn)
        .into_iter()
        .filter_map(|t| {
            let sql = format!("SELECT time_updated FROM {} WHERE id = ?1", t.session());
            conn.query_row(&sql, [id], |r| r.get::<_, i64>(0)).ok().map(|at| (t, at))
        })
        .fold(None, |best: Option<(Tables, i64)>, (t, at)| match best {
            Some((_, b)) if b >= at => best,
            _ => Some((t, at)),
        })
        .map(|(t, _)| t)
}

/// Sessions updated since `min_updated`, and the ones in `held` (already in Trek) whatever their age.
pub fn scan(min_updated: i64, held: &HashSet<String>) -> Vec<ImportedThread> {
    scan_all(&dbs(), min_updated, held)
}

/// Every database's sessions; one in both (a copied file) is read from where it changed last.
fn scan_all(dbs: &[Connection], min_updated: i64, held: &HashSet<String>) -> Vec<ImportedThread> {
    let mut found: HashMap<String, ImportedThread> = HashMap::new();
    for conn in dbs {
        for t in scan_conn(conn, min_updated, held) {
            if found.get(&t.native_id).is_none_or(|had| t.updated_at > had.updated_at) {
                found.insert(t.native_id.clone(), t);
            }
        }
    }
    let mut out: Vec<ImportedThread> = found.into_values().collect();
    out.sort_by(|a, b| a.native_id.cmp(&b.native_id));
    out
}

struct Row {
    id: String,
    tables: Tables,
    dir: Option<String>,
    title: String,
    model: Option<String>,
    created: i64,
    updated: i64,
    additions: i64,
    deletions: i64,
    child: bool,
    /// Started by `opencode run`, the one-shot command line.
    cli_run: bool,
    /// User messages, counted up to 2.
    prompts: usize,
    replied: bool,
}

fn scan_conn(conn: &Connection, min_updated: i64, held: &HashSet<String>) -> Vec<ImportedThread> {
    let held = serde_json::to_string(held).unwrap_or_else(|_| "[]".into());
    let mut found: HashMap<String, Row> = HashMap::new();
    for t in tables(conn) {
        for row in scan_rows(conn, t, min_updated, &held) {
            match found.get_mut(&row.id) {
                // Both have it: the copy that changed last, still marked as a run if 1.x's was
                // (2.0 clears what 1.x recorded of its permissions when it copies a session).
                Some(had) if row.updated > had.updated => {
                    let cli_run = had.cli_run;
                    *had = row;
                    had.cli_run |= cli_run;
                }
                Some(had) => had.cli_run |= row.cli_run,
                None => {
                    found.insert(row.id.clone(), row);
                }
            }
        }
    }
    let mut rows: Vec<Row> = found.into_values().collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows.into_iter().map(|row| thread_from(conn, row)).collect()
}

fn scan_rows(conn: &Connection, tables: Tables, min_updated: i64, held: &str) -> Vec<Row> {
    // Columns arrived over OpenCode versions; read the ones this database has.
    let columns: HashSet<String> = conn
        .prepare(&format!("SELECT name FROM pragma_table_info('{}')", tables.session()))
        .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0))?.collect())
        .unwrap_or_default();
    let col = |name: &str| if columns.contains(name) { format!("s.{name}") } else { "NULL".to_string() };
    let (user, assistant) = match tables {
        Tables::V1 => (
            "SELECT 1 FROM message m WHERE m.session_id = s.id AND json_extract(m.data, '$.role') = 'user' LIMIT 2",
            "SELECT 1 FROM message m WHERE m.session_id = s.id AND json_extract(m.data, '$.role') = 'assistant'",
        ),
        Tables::V2 => (
            "SELECT 1 FROM session_message m WHERE m.session_id = s.id AND m.type = 'user' LIMIT 2",
            "SELECT 1 FROM session_message m WHERE m.session_id = s.id AND m.type = 'assistant'",
        ),
    };
    // OpenCode 2's run leaves the session's agent unset; its other clients name one.
    let run_mark = tables == Tables::V2 && columns.contains("agent") && columns.contains("version");
    let sql = format!(
        "SELECT s.id, s.directory, s.title, {model}, s.time_created, s.time_updated,
                COALESCE(s.summary_additions, 0), COALESCE(s.summary_deletions, 0), s.parent_id IS NOT NULL,
                (SELECT COUNT(*) FROM ({user})), EXISTS ({assistant}), {permission}, {agent} IS NULL, {version}
         FROM {session} s WHERE s.time_archived IS NULL AND (s.time_updated >= ?1 OR s.id IN (SELECT value FROM json_each(?2)))",
        model = col("model"),
        permission = col("permission"),
        agent = col("agent"),
        version = col("version"),
        session = tables.session(),
    );
    let Ok(mut st) = conn.prepare(&sql) else { return vec![] };
    let rows = st.query_map(rusqlite::params![min_updated, held], |r| {
        Ok(Row {
            id: r.get(0)?,
            tables,
            dir: r.get(1)?,
            // OpenCode 2 leaves a session it hasn't named untitled.
            title: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            model: r.get(3)?,
            created: r.get(4)?,
            updated: r.get(5)?,
            additions: r.get(6)?,
            deletions: r.get(7)?,
            child: r.get(8)?,
            prompts: r.get::<_, i64>(9)? as usize,
            replied: r.get(10)?,
            cli_run: r.get::<_, Option<String>>(11)?.as_deref().is_some_and(is_cli_run)
                || (run_mark && r.get(12)? && !r.get::<_, bool>(8)? && r.get::<_, Option<String>>(13)?.as_deref().is_some_and(made_by_2)),
        })
    });
    match rows {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(_) => vec![],
    }
}

fn thread_from(conn: &Connection, row: Row) -> ImportedThread {
    let cwd = row.dir.as_deref().map(PathBuf::from);
    let own_title = source_title(&row.title);
    // The first message names untitled sessions and tells title generators apart.
    let first = (!row.child && row.prompts > 0 && (row.prompts == 1 || own_title.is_none())).then(|| first_prompt(conn, row.tables, &row.id)).flatten();
    let skip = classify(&Evidence {
        cwd: cwd.as_deref(),
        scripted: row.cli_run,
        prompts: Some(row.prompts),
        first_message: first.as_deref(),
        replied: row.replied,
        subagent: row.child,
        untouched_fork: false,
        trek: false,
        cli_run: row.cli_run,
    });
    let title = own_title
        .or_else(|| first.as_deref().and_then(user_text).map(|t| title_from(&t)))
        .unwrap_or_else(|| "OpenCode session".into());
    let (model, effort) = row.model.as_deref().map(session_model).unwrap_or_default();
    ImportedThread {
        source: ThreadSource::OpenCode,
        native_id: row.id,
        legacy_title: Some(if row.title.is_empty() { "OpenCode session".into() } else { row.title }),
        title,
        cwd,
        model,
        effort,
        branch: None,
        created_at: row.created,
        updated_at: row.updated,
        additions: row.additions,
        deletions: row.deletions,
        skip,
    }
}

/// `<provider>/<model>` from a `{"id" | "modelID", "providerID"}` object.
fn model_name(v: &Value) -> Option<String> {
    v["modelID"].as_str().or(v["id"].as_str()).filter(|m| !m.is_empty()).map(|m| match v["providerID"].as_str().filter(|p| !p.is_empty()) {
        Some(provider) => format!("{provider}/{m}"),
        None => m.to_string(),
    })
}

/// The session's model as OpenCode's ACP agent names it (`<provider>/<model>`, what continuing the
/// thread asks for), and its variant as an effort. The column holds JSON: `{"id","providerID",
/// "variant"}` now, `{"providerID","modelID"}` in older versions; plain text is taken as it is.
fn session_model(raw: &str) -> (Option<String>, Option<Effort>) {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return (Some(raw.trim().to_string()).filter(|m| !m.is_empty()), None);
    };
    (model_name(&v), v["variant"].as_str().and_then(Effort::parse))
}

/// `opencode run` creates its session with the interactive tools (questions, plan mode) denied,
/// as nobody is there to answer them. OpenCode's own apps don't. (OpenCode 2's run records no
/// such rules; it's the one client that starts a session without naming its agent, unless told
/// one with `--agent`.)
fn is_cli_run(permission: &str) -> bool {
    let Ok(Value::Array(rules)) = serde_json::from_str::<Value>(permission) else { return false };
    rules.iter().any(|r| r["permission"] == "question" && r["action"] == "deny")
}

/// A session OpenCode 2 started: its version is 2's (`2.0.26`, or a preview build's
/// `0.0.0-beta-…`), not that of the 1.x it was copied from.
fn made_by_2(version: &str) -> bool {
    version.starts_with("0.0.0-") || version.split('.').next().and_then(|m| m.parse::<u32>().ok()).is_some_and(|m| m >= 2)
}

/// Every session OpenCode has; `None` when its database can't be read.
pub(crate) fn session_ids() -> Option<HashSet<String>> {
    let dir = data_dir();
    let mut ids = HashSet::new();
    let mut any = false;
    for path in [MAIN_DB, DB_1X].iter().map(|f| dir.join(f)).filter(|p| p.is_file()) {
        // One that's there and can't be read says nothing about which sessions are gone.
        let conn = open(&path)?;
        for t in tables(&conn) {
            let mut st = conn.prepare(&format!("SELECT id FROM {}", t.session())).ok()?;
            let found: HashSet<String> = st.query_map([], |r| r.get::<_, String>(0)).ok()?.collect::<rusqlite::Result<_>>().ok()?;
            ids.extend(found);
        }
        any = true;
    }
    any.then_some(ids)
}

/// Text of the session's first user message (what the user typed, not attached file contents).
fn first_prompt(conn: &Connection, tables: Tables, id: &str) -> Option<String> {
    let sql = match tables {
        Tables::V1 => {
            "SELECT p.data FROM message m JOIN part p ON p.message_id = m.id
             WHERE m.session_id = ?1 AND json_extract(m.data, '$.role') = 'user'
             ORDER BY m.time_created, m.id, p.id LIMIT 20"
        }
        // A user message is its text; what OpenCode adds itself is a message of another type.
        Tables::V2 => "SELECT json_object('type', 'text', 'text', json_extract(data, '$.text')) FROM session_message WHERE session_id = ?1 AND type = 'user' ORDER BY seq LIMIT 20",
    };
    let mut st = conn.prepare(sql).ok()?;
    let parts = st.query_map([id], |r| r.get::<_, String>(0)).ok()?;
    parts.filter_map(Result::ok).filter_map(|d| serde_json::from_str::<Value>(&d).ok()).find_map(|p| {
        (p["type"] == "text" && p["synthetic"] != true).then(|| p["text"].as_str().map(String::from)).flatten().filter(|t| !t.trim().is_empty())
    })
}

/// Tokens session `id` used between `from` and `to` (unix ms), per assistant message: one per
/// step of a turn (each round of tool calls is a message of its own), its sub-agents' sessions
/// included (they aren't imported as threads of their own).
pub fn usage(id: &str, from: i64, to: i64) -> Vec<UsageEntry> {
    usage_priced(id, from, to).into_iter().map(|(at, model, tokens, _)| (at, model, tokens)).collect()
}

/// `usage`, with what OpenCode says each step cost in dollars (`0` for models it has no price
/// for, or that a subscription covers).
pub fn usage_priced(id: &str, from: i64, to: i64) -> Vec<(i64, Option<String>, TokenUsage, Option<f64>)> {
    db_of(id).map(|c| usage_conn(&c, id, from, to)).unwrap_or_default()
}

fn usage_conn(conn: &Connection, id: &str, from: i64, to: i64) -> Vec<(i64, Option<String>, TokenUsage, Option<f64>)> {
    let sql = match tables_of(conn, id) {
        Some(Tables::V1) => "SELECT data FROM message WHERE session_id = ?1 OR session_id IN (SELECT id FROM session WHERE parent_id = ?1) ORDER BY time_created, id",
        Some(Tables::V2) => {
            "SELECT json_set(data, '$.role', type) FROM session_message
             WHERE type = 'assistant' AND (session_id = ?1 OR session_id IN (SELECT id FROM session_v2 WHERE parent_id = ?1)) ORDER BY time_created, id"
        }
        None => return vec![],
    };
    let Ok(mut st) = conn.prepare(sql) else { return vec![] };
    let Ok(rows) = st.query_map([id], |r| r.get::<_, String>(0)) else { return vec![] };
    rows.filter_map(Result::ok)
        .filter_map(|data| {
            let m: Value = serde_json::from_str(&data).ok()?;
            if m["role"] != "assistant" {
                return None;
            }
            let at = m["time"]["completed"].as_i64().or(m["time"]["created"].as_i64()).filter(|at| (from..to).contains(at))?;
            let t = &m["tokens"];
            let n = |v: &Value| v.as_u64().unwrap_or(0);
            // Reasoning is output the model wrote, billed as output.
            let tokens = TokenUsage {
                input: n(&t["input"]),
                output: n(&t["output"]) + n(&t["reasoning"]),
                cache_read: n(&t["cache"]["read"]),
                cache_write: n(&t["cache"]["write"]),
            };
            // Named as the session's model is: `<provider>/<model>` (1.x's message has the two
            // at its top level, 2's under `model`).
            let model = if m["model"].is_object() { model_name(&m["model"]) } else { model_name(&m) };
            (!tokens.is_empty()).then_some((at, model, tokens, m["cost"].as_f64()))
        })
        .collect()
}

pub fn load(id: &str) -> anyhow::Result<Vec<Item>> {
    let conn = db_of(id).or_else(|| dbs().into_iter().next()).ok_or_else(|| anyhow::anyhow!("opencode db not found"))?;
    load_conn(&conn, id)
}

fn load_conn(conn: &Connection, id: &str) -> anyhow::Result<Vec<Item>> {
    match tables_of(conn, id) {
        Some(Tables::V2) => load_v2(conn, id),
        _ => load_v1(conn, id),
    }
}

/// A message being read: its role and times, and (for the user) the text typed so far.
struct Message {
    id: String,
    data: Value,
    typed: Vec<String>,
}

fn load_v1(conn: &Connection, id: &str) -> anyhow::Result<Vec<Item>> {
    // Messages once, parts in order; joining them would repeat a message's data on every part.
    let mut messages: HashMap<String, String> = conn
        .prepare("SELECT id, data FROM message WHERE session_id = ?1")?
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut st = conn.prepare(
        "SELECT m.id, p.data FROM part p JOIN message m ON m.id = p.message_id
         WHERE p.session_id = ?1 ORDER BY m.time_created, m.id, p.time_created, p.id",
    )?;
    let rows = st.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut t = Transcript::default();
    let mut current: Option<Message> = None;
    for (message_id, data) in rows.filter_map(Result::ok) {
        if current.as_ref().is_none_or(|m| m.id != message_id) {
            if let Some(done) = current.take() {
                finish_message(&mut t, done);
            }
            let data: Value = messages.remove(&message_id).and_then(|m| serde_json::from_str(&m).ok()).unwrap_or_default();
            if data["role"] == "assistant" {
                t.activity(data["time"]["created"].as_i64());
            }
            current = Some(Message { id: message_id, data, typed: vec![] });
        }
        let Some(m) = current.as_mut() else { continue };
        let Ok(p) = serde_json::from_str::<Value>(&data) else { continue };
        match p["type"].as_str() {
            Some("text") => {
                let text = p["text"].as_str().unwrap_or_default().to_string();
                if text.trim().is_empty() || p["synthetic"] == true {
                    continue;
                }
                if m.data["role"] == "user" {
                    m.typed.push(text);
                } else {
                    t.push(Item::Assistant { text });
                }
            }
            Some("reasoning") => reasoning(&mut t, &p),
            Some("tool") => {
                let state = &p["state"];
                t.push(Item::Tool {
                    id: p["callID"].as_str().unwrap_or_default().into(),
                    title: state["title"].as_str().or(p["tool"].as_str()).unwrap_or("tool").into(),
                    detail: tool_detail(&state["input"]),
                    output: clip(state["output"].as_str().unwrap_or_default(), 4000),
                    status: if state["status"] == "error" { ToolStatus::Failed } else { ToolStatus::Done },
                });
            }
            _ => {}
        }
    }
    if let Some(done) = current {
        finish_message(&mut t, done);
    }
    Ok(t.finish())
}

fn reasoning(t: &mut Transcript, p: &Value) {
    let text = p["text"].as_str().unwrap_or_default();
    if !text.trim().is_empty() {
        t.push(Item::Reasoning { text: text.into() });
    }
}

/// What a tool call's row says it did: its command, file or pattern.
fn tool_detail(input: &Value) -> String {
    input["command"]
        .as_str()
        .or(input["filePath"].as_str())
        .or(input["pattern"].as_str())
        .or(input["description"].as_str())
        .map(String::from)
        .unwrap_or_else(|| clip(&input.to_string(), 200))
}

/// A user message becomes one entry; an assistant message that stopped (rather than handing
/// over to tool calls) finishes the turn.
fn finish_message(t: &mut Transcript, m: Message) {
    let time = &m.data["time"];
    if m.data["role"] == "user" {
        if !m.typed.is_empty() {
            t.user(m.typed.join("\n\n"), time["created"].as_i64());
        }
        return;
    }
    finish_reply(t, &m.data, m.data["error"]["name"] == "MessageAbortedError");
}

/// An assistant message's end: stopped by the user, the turn's end, or a step of it.
fn finish_reply(t: &mut Transcript, data: &Value, aborted: bool) {
    let completed = data["time"]["completed"].as_i64();
    if aborted {
        t.interrupt();
    } else if completed.is_some() && data["finish"] != "tool-calls" && data["error"].is_null() {
        t.complete(completed);
    } else {
        t.touch(completed);
    }
}

/// OpenCode 2's messages, in order: each is a row, an assistant reply's text, reasoning and tool
/// calls inside it (`content`). What OpenCode adds itself (`synthetic` notes, `system`
/// instructions, compaction summaries, model switches, idle marks) isn't part of the conversation.
fn load_v2(conn: &Connection, id: &str) -> anyhow::Result<Vec<Item>> {
    let mut st = conn.prepare("SELECT type, data FROM session_message WHERE session_id = ?1 ORDER BY seq")?;
    let rows = st.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut t = Transcript::default();
    for (kind, data) in rows.filter_map(Result::ok) {
        let Ok(m) = serde_json::from_str::<Value>(&data) else { continue };
        let created = m["time"]["created"].as_i64();
        match kind.as_str() {
            "user" => {
                let text = m["text"].as_str().unwrap_or_default();
                if !text.trim().is_empty() {
                    t.user(text.to_string(), created);
                }
            }
            "assistant" => {
                t.activity(created);
                for p in m["content"].as_array().into_iter().flatten() {
                    match p["type"].as_str() {
                        Some("text") => {
                            let text = p["text"].as_str().unwrap_or_default();
                            if !text.trim().is_empty() {
                                t.push(Item::Assistant { text: text.into() });
                            }
                        }
                        Some("reasoning") => reasoning(&mut t, p),
                        Some("tool") => {
                            let state = &p["state"];
                            let failed = state["status"] == "error";
                            let output = if failed {
                                state["error"]["message"].as_str().or(state["error"].as_str()).unwrap_or_default().to_string()
                            } else {
                                let text: Vec<&str> = state["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect();
                                text.join("\n")
                            };
                            t.push(Item::Tool {
                                id: p["id"].as_str().unwrap_or_default().into(),
                                title: p["name"].as_str().unwrap_or("tool").into(),
                                detail: tool_detail(&state["input"]),
                                output: clip(&output, 4000),
                                status: if failed { ToolStatus::Failed } else { ToolStatus::Done },
                            });
                        }
                        _ => {}
                    }
                }
                finish_reply(&mut t, &m, m["error"]["type"] == "aborted");
            }
            _ => {}
        }
    }
    Ok(t.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::Skip;
    use serde_json::json;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL, title TEXT NOT NULL, model TEXT, permission TEXT,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER, summary_additions INTEGER, summary_deletions INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);",
        )
        .unwrap();
        conn
    }

    fn session(conn: &Connection, id: &str, dir: &str, title: &str, parent: Option<&str>) {
        conn.execute(
            "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, 1, 2)",
            rusqlite::params![id, parent, dir, title],
        )
        .unwrap();
    }

    fn message(conn: &Connection, session: &str, id: &str, data: Value, parts: &[Value]) {
        let at = data["time"]["created"].as_i64().unwrap();
        conn.execute("INSERT INTO message VALUES (?1, ?2, ?3, ?4)", rusqlite::params![id, session, at, data.to_string()]).unwrap();
        for (i, p) in parts.iter().enumerate() {
            conn.execute("INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?5)", rusqlite::params![format!("{id}-{i}"), id, session, at, p.to_string()]).unwrap();
        }
    }

    fn ask(conn: &Connection, session: &str, id: &str, text: &str, at: i64) {
        message(conn, session, id, json!({ "role": "user", "time": { "created": at } }), &[json!({ "type": "text", "text": text })]);
    }

    fn answer(conn: &Connection, session: &str, id: &str, text: &str, at: i64, done: i64, finish: &str) {
        let data = json!({ "role": "assistant", "time": { "created": at, "completed": done }, "finish": finish });
        message(conn, session, id, data, &[json!({ "type": "text", "text": text })]);
    }

    /// What `opencode run` records.
    const RUN: &str = r#"[{"permission":"question","pattern":"*","action":"deny"},{"permission":"plan_enter","pattern":"*","action":"deny"},{"permission":"plan_exit","pattern":"*","action":"deny"}]"#;

    fn set_permission(conn: &Connection, id: &str, permission: &str) {
        conn.execute("UPDATE session SET permission = ?2 WHERE id = ?1", rusqlite::params![id, permission]).unwrap();
    }

    fn scan(conn: &Connection) -> Vec<(String, String, Option<Skip>)> {
        let mut v: Vec<_> = scan_conn(conn, 0, &HashSet::new()).into_iter().map(|t| (t.native_id, t.title, t.skip)).collect();
        v.sort();
        v
    }

    #[test]
    fn titles_and_helper_sessions() {
        let conn = db();
        session(&conn, "a-titled", "/Users/me/app", "Fix GitHub connector", None);
        ask(&conn, "a-titled", "m1", "my github connector keeps failing", 10);
        answer(&conn, "a-titled", "m2", "Checking", 11, 12, "stop");
        // A script probing `opencode run`, never answered.
        session(&conn, "b-placeholder", "/Users/me/app", "New session - 2026-07-10T03:44:55.268Z", None);
        ask(&conn, "b-placeholder", "m3", "\"Reply with exactly: OK\"", 10);
        set_permission(&conn, "b-placeholder", RUN);
        // The same probe, answered and titled.
        session(&conn, "b-probe", "/Users/me/app", "Reply with OK", None);
        ask(&conn, "b-probe", "m30", "\"Reply with exactly: OK\"", 10);
        answer(&conn, "b-probe", "m31", "OK", 11, 12, "stop");
        set_permission(&conn, "b-probe", RUN);
        // Asked once in OpenCode's own app: a conversation, answered or not.
        session(&conn, "b-question", "/Users/me/app", "New session - 2026-07-10T03:50:00.000Z", None);
        ask(&conn, "b-question", "m32", "why is the build slow?", 10);
        // Other apps set their own permissions; only the run command's mark counts.
        session(&conn, "b-app", "/Users/me/app", "Build speed", None);
        ask(&conn, "b-app", "m33", "why is the build slow?", 10);
        answer(&conn, "b-app", "m34", "Caching", 11, 12, "stop");
        set_permission(&conn, "b-app", r#"[{"permission":"*","pattern":"*","action":"allow"}]"#);
        session(&conn, "c-t3", "/Users/me/app", "T3 Code 8e72d07c-b653-46c1-ade2-27747b6b23a3", None);
        ask(&conn, "c-t3", "m4", "Summarize the project", 10);
        ask(&conn, "c-t3", "m5", "and the open issues", 20);
        session(&conn, "d-empty", "/Users/someone", "New session - 2026-10-02T19:55:56.192Z", None);
        session(&conn, "e-child", "/Users/me/app", "Explore codebase (@explore subagent)", Some("a-titled"));
        ask(&conn, "e-child", "m6", "explore", 10);
        session(&conn, "f-namer", "/Users/me/app", "New session - 2026-09-09T18:27:20.681Z", None);
        ask(&conn, "f-namer", "m7", "Generate a title that will help the user recognize this coding session weeks later.\nReturn JSON", 10);
        answer(&conn, "f-namer", "m8", "{\"title\":\"x\"}", 11, 12, "stop");
        session(&conn, "g-tmp", "/private/tmp/trek-e2e", "Pong response request", None);
        ask(&conn, "g-tmp", "m9", "Reply with just the word: pong", 10);
        assert_eq!(
            scan(&conn),
            vec![
                ("a-titled".into(), "Fix GitHub connector".into(), None),
                ("b-app".into(), "Build speed".into(), None),
                ("b-placeholder".into(), "Reply with exactly: OK".into(), Some(Skip::OneShotRun)),
                ("b-probe".into(), "Reply with OK".into(), Some(Skip::OneShotRun)),
                ("b-question".into(), "why is the build slow?".into(), None),
                ("c-t3".into(), "Summarize the project".into(), None),
                ("d-empty".into(), "OpenCode session".into(), Some(Skip::NoUserMessage)),
                ("e-child".into(), "Explore codebase (@explore subagent)".into(), Some(Skip::Subagent)),
                ("f-namer".into(), "Generate a title that will help the user recognize this…".into(), Some(Skip::TitleGenerator)),
                ("g-tmp".into(), "Pong response request".into(), Some(Skip::TempDir)),
            ]
        );
    }

    #[test]
    fn models_are_named_as_the_acp_agent_names_them() {
        let conn = db();
        let models = [
            ("current", r#"{"id":"gpt-5.4-mini","providerID":"github-copilot","variant":"medium"}"#),
            ("no-variant", r#"{"id":"deepseek-v4-flash-free","providerID":"opencode"}"#),
            ("slashed", r#"{"id":"z-ai/glm-5.2","providerID":"openrouter","variant":"default"}"#),
            ("older", r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-5"}"#),
            ("broken", r#"{"id":"","providerID":"claude-sonnet-4.5","variant":"default"}"#),
            ("plain", "gpt-4.1"),
        ];
        for (id, model) in models {
            session(&conn, id, "/Users/me/app", "Fix GitHub connector", None);
            conn.execute("UPDATE session SET model = ?2 WHERE id = ?1", rusqlite::params![id, model]).unwrap();
            ask(&conn, id, &format!("{id}-m"), "my github connector keeps failing", 10);
        }
        let mut found: Vec<_> = scan_conn(&conn, 0, &HashSet::new()).into_iter().map(|t| (t.native_id, t.model, t.effort)).collect();
        found.sort();
        let row = |id: &str, model: Option<&str>, effort| (id.to_string(), model.map(String::from), effort);
        assert_eq!(
            found,
            vec![
                row("broken", None, None),
                row("current", Some("github-copilot/gpt-5.4-mini"), Some(Effort::Medium)),
                row("no-variant", Some("opencode/deepseek-v4-flash-free"), None),
                row("older", Some("anthropic/claude-sonnet-4-5"), None),
                row("plain", Some("gpt-4.1"), None),
                row("slashed", Some("openrouter/z-ai/glm-5.2"), None),
            ]
        );
    }

    #[test]
    fn older_databases_and_old_sessions_still_scan() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL, title TEXT NOT NULL,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER, summary_additions INTEGER, summary_deletions INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);",
        )
        .unwrap();
        session(&conn, "old", "/Users/me/app", "Fix GitHub connector", None);
        ask(&conn, "old", "m1", "my github connector keeps failing", 10);
        assert_eq!(scan(&conn), vec![("old".into(), "Fix GitHub connector".into(), None)]);
        // Outside the window unless Trek already has it.
        assert!(scan_conn(&conn, i64::MAX, &HashSet::new()).is_empty());
        assert_eq!(scan_conn(&conn, i64::MAX, &HashSet::from(["old".to_string()])).len(), 1);
    }

    #[test]
    fn transcripts_carry_times_and_turn_footers() {
        let conn = db();
        session(&conn, "s", "/Users/me/app", "Copilot", None);
        ask(&conn, "s", "m1", "connect copilot", 1_000);
        let step = json!({ "role": "assistant", "time": { "created": 2_000, "completed": 3_000 }, "finish": "tool-calls" });
        let tool = json!({ "type": "tool", "callID": "c", "tool": "bash", "state": { "status": "completed", "title": "List", "input": { "command": "ls" }, "output": "a" } });
        message(&conn, "s", "m2", step, &[tool]);
        answer(&conn, "s", "m3", "Connected.", 3_500, 9_400, "stop");
        ask(&conn, "s", "m4", "now refresh", 20_000);
        let aborted = json!({ "role": "assistant", "time": { "created": 20_500 }, "error": { "name": "MessageAbortedError" } });
        message(&conn, "s", "m5", aborted, &[json!({ "type": "text", "text": "Refreshing" })]);
        let items = load_conn(&conn, "s").unwrap();
        assert_eq!(items[0], Item::User { text: "connect copilot".into(), images: vec![], at: Some(1_000), resume: None, aside: false });
        assert!(matches!(&items[1], Item::Tool { detail, .. } if detail == "ls"));
        assert_eq!(items[2], Item::Assistant { text: "Connected.".into() });
        assert_eq!(items[3], Item::TurnEnd { at: 9_400, took_secs: 8 });
        assert_eq!(items[4], Item::User { text: "now refresh".into(), images: vec![], at: Some(20_000), resume: None, aside: false });
        assert_eq!(items.len(), 6, "the stopped reply has no footer: {items:?}");
    }

    /// Recorded with OpenCode 1.18.35 and then 2.0.26 on one database: a 1.x session (which 2.0
    /// copied into its own tables on first start), and two `opencode2 run`s: one with a shell
    /// call, one whose model refused it. And a session a 2.0 preview build opened over ACP.
    fn recorded() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Its projects aren't part of it.
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute_batch(include_str!("../../fixtures/opencode-v1-and-v2.sql")).unwrap();
        conn
    }

    const MIGRATED: &str = "ses_edfc986b6ffe4mZ9ccSLf9DwLP";
    const V2: &str = "ses_edfcbb35fffervIIJpuPw7hX2w";
    const REFUSED: &str = "ses_edfcbd056ffeGuaDEh9Roa4ew5";
    const ACP: &str = "ses_edee57470ffeJ8L7m282BCBa0v";

    #[test]
    fn opencode_2_sessions_import_beside_1x_ones() {
        let conn = recorded();
        assert_eq!(tables(&conn), vec![Tables::V1, Tables::V2]);
        let mut found: Vec<_> = scan_conn(&conn, 0, &HashSet::new()).into_iter().map(|t| (t.native_id, t.title, t.model, t.skip)).collect();
        found.sort();
        let row = |id: &str, title: &str, model: Option<&str>, skip| (id.to_string(), title.to_string(), model.map(String::from), skip);
        assert_eq!(
            found,
            vec![
                // Opened over ACP, untitled: named by what was asked.
                row(ACP, "Reply with the single word: pong", Some("opencode/big-pickle"), None),
                // Once, though both versions' tables have it.
                row(MIGRATED, "Counting directory entries with ls", Some("opencode/mimo-v2.6-flash-free"), None),
                // `opencode2 run`s: no agent named.
                row(V2, "Running ls and counting entries", Some("opencode/mimo-v2.6-flash-free"), Some(Skip::OneShotRun)),
                row(REFUSED, "Use your bash tool to run ls, then reply with just the…", None, Some(Skip::OneShotRun)),
            ]
        );
        // Outside the window unless Trek already has it.
        assert!(scan_conn(&conn, i64::MAX, &HashSet::new()).is_empty());
        assert_eq!(scan_conn(&conn, i64::MAX, &HashSet::from([V2.to_string()])).len(), 1);
    }

    #[test]
    fn an_opencode_2_transcript_reads_like_a_1x_one() {
        let conn = recorded();
        let items = load_conn(&conn, V2).unwrap();
        assert!(matches!(&items[0], Item::User { text, at: Some(1791541660925), .. } if text.starts_with("Use your bash tool")), "{items:?}");
        assert!(matches!(&items[1], Item::Reasoning { .. }));
        assert_eq!(
            items[2],
            Item::Tool { id: "call_63e33f94ef1f4abd8bff6720".into(), title: "shell".into(), detail: "ls".into(), output: "notes.txt\n".into(), status: ToolStatus::Done }
        );
        assert!(matches!(&items[3], Item::Reasoning { .. }));
        assert_eq!(items[4], Item::Assistant { text: "1".into() });
        assert_eq!(items[5], Item::TurnEnd { at: 1791541675204, took_secs: 14 });
        assert_eq!(items.len(), 6, "{items:?}");
        // The refused one: the question, and no reply to close.
        let items = load_conn(&conn, REFUSED).unwrap();
        assert!(matches!(&items[..], [Item::User { .. }]), "{items:?}");
        // The same conversation from either version's tables.
        let v1 = load_conn(&conn, MIGRATED).unwrap();
        assert_eq!(v1, load_v2(&conn, MIGRATED).unwrap().into_iter().map(|i| match i {
            // 1.x's tool rows have OpenCode's own title for the call.
            Item::Tool { title, .. } if title == "bash" => v1.iter().find(|i| matches!(i, Item::Tool { .. })).unwrap().clone(),
            other => other,
        }).collect::<Vec<_>>());
    }

    #[test]
    fn a_session_continued_in_opencode_2_is_read_from_its_tables() {
        let conn = recorded();
        assert_eq!(tables_of(&conn, MIGRATED), Some(Tables::V1), "2.0's untouched copy");
        conn.execute("UPDATE session_v2 SET time_updated = time_updated + 60000 WHERE id = ?1", [MIGRATED]).unwrap();
        let next = json!({ "time": { "created": 1791541871322i64 }, "text": "and hidden ones?" });
        conn.execute(
            "INSERT INTO session_message VALUES ('msg_next', ?1, 'user', 99, 1791541871322, 1791541871322, ?2)",
            rusqlite::params![MIGRATED, next.to_string()],
        )
        .unwrap();
        assert_eq!(tables_of(&conn, MIGRATED), Some(Tables::V2));
        assert!(matches!(load_conn(&conn, MIGRATED).unwrap().last(), Some(Item::User { text, .. }) if text == "and hidden ones?"));
        let found = scan_conn(&conn, 0, &HashSet::new());
        assert_eq!(found.iter().filter(|t| t.native_id == MIGRATED).count(), 1);
        // Only 1.x's tables, or only 2's.
        let v2_only = recorded();
        v2_only.execute_batch("DROP TABLE part; DROP TABLE message; DROP TABLE session;").unwrap();
        assert_eq!(tables(&v2_only), vec![Tables::V2]);
        assert_eq!(scan_conn(&v2_only, 0, &HashSet::new()).len(), 4);
        assert_eq!(tables(&db()), vec![Tables::V1]);
    }

    #[test]
    fn opencode_2_usage_counts_every_step() {
        let conn = recorded();
        let steps = usage_conn(&conn, V2, 0, i64::MAX);
        let model = Some("opencode/mimo-v2.6-flash-free".to_string());
        assert_eq!(
            steps,
            vec![
                (1791541668636, model.clone(), TokenUsage { input: 230, output: 19 + 230, cache_read: 4672, cache_write: 0 }, Some(0.0)),
                (1791541675204, model, TokenUsage { input: 297, output: 3 + 74, cache_read: 4864, cache_write: 0 }, Some(0.0)),
            ]
        );
        assert_eq!(usage_conn(&conn, V2, 1791541668637, i64::MAX).len(), 1, "a turn's own steps");
        assert!(usage_conn(&conn, "ses_unknown", 0, i64::MAX).is_empty());
    }

    #[test]
    fn opencode_2_tool_errors_and_stops() {
        let conn = recorded();
        let user = json!({ "time": { "created": 1_000 }, "text": "fix it" });
        let failed = json!({ "type": "tool", "id": "c1", "name": "edit", "state": { "status": "error", "input": { "filePath": "/Users/me/app/a.rs" },
            "error": { "type": "tool.execution", "message": "Could not find oldString in the file." } } });
        let stopped = json!({ "time": { "created": 2_000 }, "content": [failed, { "type": "text", "text": "Trying" }], "error": { "type": "aborted" } });
        conn.execute("INSERT INTO session_v2 (id, project_id, slug, directory, version, time_created, time_updated) VALUES ('s', 'p', 'x', '/Users/me/app', '2.0.26', 1, 2)", []).unwrap();
        for (seq, (kind, data)) in [("user", user), ("assistant", stopped)].into_iter().enumerate() {
            conn.execute("INSERT INTO session_message VALUES (?1, 's', ?2, ?3, 1, 1, ?4)", rusqlite::params![format!("m{seq}"), kind, seq as i64, data.to_string()]).unwrap();
        }
        let items = load_conn(&conn, "s").unwrap();
        assert_eq!(
            items[1],
            Item::Tool { id: "c1".into(), title: "edit".into(), detail: "/Users/me/app/a.rs".into(), output: "Could not find oldString in the file.".into(), status: ToolStatus::Failed }
        );
        assert_eq!(items.len(), 3, "a stopped reply has no footer: {items:?}");
    }

    #[test]
    fn usage_comes_from_assistant_messages() {
        let conn = db();
        session(&conn, "s", "/x", "t", None);
        ask(&conn, "s", "m1", "hi", 1_000);
        // As OpenCode stores it (a real message's fields).
        let data = json!({ "role": "assistant", "modelID": "gpt-4.1", "providerID": "github-copilot", "time": { "created": 1_100, "completed": 2_000 },
            "tokens": { "total": 12620, "input": 12618, "output": 2, "reasoning": 3, "cache": { "write": 0, "read": 40 } }, "cost": 0, "finish": "stop" });
        message(&conn, "s", "m2", data, &[json!({ "type": "text", "text": "hello" })]);
        let got = usage_conn(&conn, "s", 0, 10_000);
        assert_eq!(got, vec![(2_000, Some("github-copilot/gpt-4.1".into()), TokenUsage { input: 12618, output: 5, cache_read: 40, cache_write: 0 }, Some(0.0))]);
        assert!(usage_conn(&conn, "s", 2_001, 10_000).is_empty());
        // A turn with tools: a message per step (finish "tool-calls", then "stop"), and a
        // sub-agent's session. Every step counts, not just the last.
        ask(&conn, "s", "m3", "fix it", 3_000);
        let step = |id: &str, at: i64, input: u64, finish: &str| {
            json!({ "role": "assistant", "id": id, "modelID": "big-pickle", "time": { "created": at, "completed": at + 10 },
                "tokens": { "input": input, "output": 20, "reasoning": 0, "cache": { "write": 0, "read": 0 } }, "cost": input as f64 / 1e6, "finish": finish })
        };
        message(&conn, "s", "m4", step("m4", 3_100, 547_873, "tool-calls"), &[]);
        message(&conn, "s", "m5", step("m5", 3_200, 841, "tool-calls"), &[]);
        session(&conn, "kid", "/x", "Explore (@explore subagent)", Some("s"));
        message(&conn, "kid", "k1", step("k1", 3_250, 100, "stop"), &[]);
        message(&conn, "s", "m6", step("m6", 3_300, 365, "stop"), &[]);
        let turn = usage_conn(&conn, "s", 3_000, i64::MAX);
        assert_eq!(turn.len(), 4);
        assert_eq!(turn.iter().map(|(_, _, t, _)| t.input).sum::<u64>(), 547_873 + 841 + 100 + 365);
        // Each step's own cost, as OpenCode priced it.
        assert_eq!(turn[0].3, Some(0.547873));
    }

    #[test]
    fn an_opencode_2_run_is_told_apart_only_when_its_own() {
        let conn = recorded();
        let skip = |conn: &Connection, id: &str| scan_conn(conn, 0, &HashSet::new()).into_iter().find(|t| t.native_id == id).unwrap().skip;
        // Continued with a second message: a conversation after all.
        let next = json!({ "time": { "created": 1791541700000i64 }, "text": "and the hidden ones?" });
        conn.execute("INSERT INTO session_message VALUES ('msg_more', ?1, 'user', 30, 1791541700000, 1791541700000, ?2)", rusqlite::params![V2, next.to_string()]).unwrap();
        assert_eq!(skip(&conn, V2), None);
        // Started with `--agent`, it names one like any other client.
        conn.execute("UPDATE session_v2 SET agent = 'build' WHERE id = ?1", [REFUSED]).unwrap();
        assert_eq!(skip(&conn, REFUSED), None);
        // A 1.x session 2.0 copied keeps 1.x's version, whatever agent it had: 1.x's own marks decide.
        conn.execute("UPDATE session_v2 SET agent = NULL, time_updated = time_updated + 1 WHERE id = ?1", [MIGRATED]).unwrap();
        assert_eq!(skip(&conn, MIGRATED), None);
        assert!(made_by_2("2.0.26") && made_by_2("0.0.0-beta-19296") && made_by_2("10.1.0"));
        assert!(!made_by_2("1.18.35") && !made_by_2("0.15.31") && !made_by_2(""));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek-opencode-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn opencode_1x_gets_a_database_of_its_own_beside_2s() {
        let dir = temp_dir("db-1x");
        let own = dir.join(DB_1X);
        // Nothing yet, or 1.x's own (2.0 may have added its tables to it since): 1.x opens it.
        assert_eq!(db_for_1x_in(&dir), None);
        Connection::open(dir.join(MAIN_DB)).unwrap().execute_batch("CREATE TABLE session (id TEXT); CREATE TABLE session_v2 (id TEXT);").unwrap();
        assert_eq!(db_for_1x_in(&dir), None);
        // Made by 2.0: 1.x would refuse it.
        std::fs::remove_file(dir.join(MAIN_DB)).unwrap();
        Connection::open(dir.join(MAIN_DB)).unwrap().execute_batch("CREATE TABLE migration (id TEXT); CREATE TABLE session_v2 (id TEXT);").unwrap();
        assert_eq!(db_for_1x_in(&dir), Some(own.clone()));
        // And once 1.x has its own, it stays there while 2.0's is away.
        std::fs::rename(dir.join(MAIN_DB), dir.join("opencode.db.bak")).unwrap();
        assert_eq!(db_for_1x_in(&dir), None);
        Connection::open(&own).unwrap().execute_batch("CREATE TABLE session (id TEXT);").unwrap();
        assert_eq!(db_for_1x_in(&dir), Some(own));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn both_databases_are_read() {
        let dir = temp_dir("both");
        // 2.0's, as recorded, and 1.x's own beside it.
        let main = Connection::open(dir.join(MAIN_DB)).unwrap();
        main.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        main.execute_batch(include_str!("../../fixtures/opencode-v1-and-v2.sql")).unwrap();
        main.execute_batch("DROP TABLE part; DROP TABLE message; DROP TABLE session;").unwrap();
        drop(main);
        let own = Connection::open(dir.join(DB_1X)).unwrap();
        own.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL, title TEXT NOT NULL, model TEXT, permission TEXT,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER, summary_additions INTEGER, summary_deletions INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);",
        )
        .unwrap();
        session(&own, "ses_1x", "/Users/me/app", "Fix GitHub connector", None);
        ask(&own, "ses_1x", "m1", "my github connector keeps failing", 10);
        drop(own);
        let dbs = dbs_in(&dir);
        assert_eq!(dbs.len(), 2);
        let ids: Vec<String> = scan_all(&dbs, 0, &HashSet::new()).into_iter().map(|t| t.native_id).collect();
        let mut want = ["ses_1x", MIGRATED, ACP, V2, REFUSED].map(String::from).to_vec();
        want.sort();
        assert_eq!(ids, want);
        // Each session is read from the file that has it.
        let of = |id: &str| dbs.iter().position(|c| tables_of(c, id).is_some());
        assert_eq!((of("ses_1x"), of(V2), of("ses_none")), (Some(1), Some(0), None));
        assert!(matches!(&load_conn(&dbs[1], "ses_1x").unwrap()[..], [Item::User { .. }]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
