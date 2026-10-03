//! OpenCode sessions: `~/.local/share/opencode/opencode.db` (session, message, part).

use super::{Evidence, ImportedThread, Transcript, classify, clip, source_title, title_from, user_text};
use crate::store::{Item, ToolStatus};
use crate::types::ThreadSource;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

fn db() -> Option<Connection> {
    let path = crate::paths::home().join(".local/share/opencode/opencode.db");
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()
}

/// Sessions updated since `min_updated`, and the ones in `held` (already in Trek) whatever their age.
pub fn scan(min_updated: i64, held: &HashSet<String>) -> Vec<ImportedThread> {
    db().map(|conn| scan_conn(&conn, min_updated, held)).unwrap_or_default()
}

struct Row {
    id: String,
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
    // Columns arrived over OpenCode versions; read the ones this database has.
    let columns: HashSet<String> = conn
        .prepare("SELECT name FROM pragma_table_info('session')")
        .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0))?.collect())
        .unwrap_or_default();
    let col = |name: &str| if columns.contains(name) { format!("s.{name}") } else { "NULL".to_string() };
    let sql = format!(
        "SELECT s.id, s.directory, s.title, {model}, s.time_created, s.time_updated,
                COALESCE(s.summary_additions, 0), COALESCE(s.summary_deletions, 0), s.parent_id IS NOT NULL,
                (SELECT COUNT(*) FROM (SELECT 1 FROM message m WHERE m.session_id = s.id AND json_extract(m.data, '$.role') = 'user' LIMIT 2)),
                EXISTS (SELECT 1 FROM message m WHERE m.session_id = s.id AND json_extract(m.data, '$.role') = 'assistant'),
                {permission}
         FROM session s WHERE s.time_archived IS NULL AND (s.time_updated >= ?1 OR s.id IN (SELECT value FROM json_each(?2)))",
        model = col("model"),
        permission = col("permission"),
    );
    let Ok(mut st) = conn.prepare(&sql) else { return vec![] };
    let held = serde_json::to_string(held).unwrap_or_else(|_| "[]".into());
    let rows = st.query_map(rusqlite::params![min_updated, held], |r| {
        Ok(Row {
            id: r.get(0)?,
            dir: r.get(1)?,
            title: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            model: r.get(3)?,
            created: r.get(4)?,
            updated: r.get(5)?,
            additions: r.get(6)?,
            deletions: r.get(7)?,
            child: r.get(8)?,
            prompts: r.get::<_, i64>(9)? as usize,
            replied: r.get(10)?,
            cli_run: r.get::<_, Option<String>>(11)?.as_deref().is_some_and(is_cli_run),
        })
    });
    let rows: Vec<Row> = match rows {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(_) => return vec![],
    };
    rows.into_iter().map(|row| thread_from(conn, row)).collect()
}

fn thread_from(conn: &Connection, row: Row) -> ImportedThread {
    let cwd = row.dir.as_deref().map(PathBuf::from);
    let own_title = source_title(&row.title);
    // The first message names untitled sessions and tells title generators apart.
    let first = (!row.child && row.prompts > 0 && (row.prompts == 1 || own_title.is_none())).then(|| first_prompt(conn, &row.id)).flatten();
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
    ImportedThread {
        source: ThreadSource::OpenCode,
        native_id: row.id,
        legacy_title: Some(if row.title.is_empty() { "OpenCode session".into() } else { row.title }),
        title,
        cwd,
        // Stored as JSON ({"providerID","modelID"}) in newer versions.
        model: row.model.map(|m| serde_json::from_str::<Value>(&m).ok().and_then(|v| v["modelID"].as_str().map(String::from)).unwrap_or(m)),
        effort: None,
        branch: None,
        created_at: row.created,
        updated_at: row.updated,
        additions: row.additions,
        deletions: row.deletions,
        skip,
    }
}

/// `opencode run` creates its session with the interactive tools (questions, plan mode) denied,
/// as nobody is there to answer them. OpenCode's own apps don't.
fn is_cli_run(permission: &str) -> bool {
    let Ok(Value::Array(rules)) = serde_json::from_str::<Value>(permission) else { return false };
    rules.iter().any(|r| r["permission"] == "question" && r["action"] == "deny")
}

/// Every session OpenCode has; `None` when its database can't be read.
pub(crate) fn session_ids() -> Option<HashSet<String>> {
    let conn = db()?;
    let mut st = conn.prepare("SELECT id FROM session").ok()?;
    st.query_map([], |r| r.get::<_, String>(0)).ok()?.collect::<rusqlite::Result<_>>().ok()
}

/// Text of the session's first user message (what the user typed, not attached file contents).
fn first_prompt(conn: &Connection, id: &str) -> Option<String> {
    let mut st = conn
        .prepare(
            "SELECT p.data FROM message m JOIN part p ON p.message_id = m.id
             WHERE m.session_id = ?1 AND json_extract(m.data, '$.role') = 'user'
             ORDER BY m.time_created, m.id, p.id LIMIT 20",
        )
        .ok()?;
    let parts = st.query_map([id], |r| r.get::<_, String>(0)).ok()?;
    parts.filter_map(Result::ok).filter_map(|d| serde_json::from_str::<Value>(&d).ok()).find_map(|p| {
        (p["type"] == "text" && p["synthetic"] != true).then(|| p["text"].as_str().map(String::from)).flatten().filter(|t| !t.trim().is_empty())
    })
}

pub fn load(id: &str) -> anyhow::Result<Vec<Item>> {
    let conn = db().ok_or_else(|| anyhow::anyhow!("opencode db not found"))?;
    load_conn(&conn, id)
}

/// A message being read: its role and times, and (for the user) the text typed so far.
struct Message {
    id: String,
    data: Value,
    typed: Vec<String>,
}

fn load_conn(conn: &Connection, id: &str) -> anyhow::Result<Vec<Item>> {
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
            Some("reasoning") => {
                let text = p["text"].as_str().unwrap_or_default();
                if !text.trim().is_empty() {
                    t.push(Item::Reasoning { text: text.into() });
                }
            }
            Some("tool") => {
                let state = &p["state"];
                let input = &state["input"];
                let detail = input["command"]
                    .as_str()
                    .or(input["filePath"].as_str())
                    .or(input["pattern"].as_str())
                    .map(String::from)
                    .unwrap_or_else(|| clip(&input.to_string(), 200));
                t.push(Item::Tool {
                    id: p["callID"].as_str().unwrap_or_default().into(),
                    title: state["title"].as_str().or(p["tool"].as_str()).unwrap_or("tool").into(),
                    detail,
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
    let completed = time["completed"].as_i64();
    if m.data["error"]["name"] == "MessageAbortedError" {
        t.interrupt();
    } else if completed.is_some() && m.data["finish"] != "tool-calls" && m.data["error"].is_null() {
        t.complete(completed);
    } else {
        t.touch(completed);
    }
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
        session(&conn, "d-empty", "/Users/tobias", "New session - 2026-10-02T19:55:56.192Z", None);
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
        assert_eq!(items[0], Item::User { text: "connect copilot".into(), images: vec![], at: Some(1_000) });
        assert!(matches!(&items[1], Item::Tool { detail, .. } if detail == "ls"));
        assert_eq!(items[2], Item::Assistant { text: "Connected.".into() });
        assert_eq!(items[3], Item::TurnEnd { at: 9_400, took_secs: 8 });
        assert_eq!(items[4], Item::User { text: "now refresh".into(), images: vec![], at: Some(20_000) });
        assert_eq!(items.len(), 6, "the stopped reply has no footer: {items:?}");
    }
}
