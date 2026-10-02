//! OpenCode sessions: `~/.local/share/opencode/opencode.db` (session, message, part).

use super::{ImportedThread, clip};
use crate::store::{Item, ToolStatus};
use crate::types::ThreadSource;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::PathBuf;

fn db() -> Option<Connection> {
    let path = crate::paths::home().join(".local/share/opencode/opencode.db");
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()
}

pub fn scan(min_updated: i64) -> Vec<ImportedThread> {
    let Some(conn) = db() else { return vec![] };
    let sql = "SELECT id, directory, title, model, time_created, time_updated,
                      COALESCE(summary_additions, 0), COALESCE(summary_deletions, 0)
               FROM session WHERE parent_id IS NULL AND time_archived IS NULL AND time_updated >= ?1";
    let Ok(mut st) = conn.prepare(sql) else { return vec![] };
    let rows = st.query_map([min_updated], |r| {
        let model: Option<String> = r.get(3)?;
        Ok(ImportedThread {
            source: ThreadSource::OpenCode,
            native_id: r.get(0)?,
            cwd: r.get::<_, Option<String>>(1)?.map(PathBuf::from),
            title: r.get::<_, Option<String>>(2)?.unwrap_or_else(|| "OpenCode session".into()),
            // Stored as JSON ({"providerID","modelID"}) in newer versions.
            model: model.map(|m| {
                serde_json::from_str::<Value>(&m)
                    .ok()
                    .and_then(|v| v["modelID"].as_str().map(String::from))
                    .unwrap_or(m)
            }),
            effort: None,
            branch: None,
            created_at: r.get(4)?,
            updated_at: r.get(5)?,
            additions: r.get(6)?,
            deletions: r.get(7)?,
        })
    });
    match rows {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(_) => vec![],
    }
}

pub fn load(id: &str) -> anyhow::Result<Vec<Item>> {
    let conn = db().ok_or_else(|| anyhow::anyhow!("opencode db not found"))?;
    let mut st = conn.prepare(
        "SELECT json_extract(m.data, '$.role'), p.data FROM part p JOIN message m ON m.id = p.message_id
         WHERE p.session_id = ?1 ORDER BY m.time_created, p.time_created, p.id",
    )?;
    let rows = st.query_map([id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)))?;
    let mut items = Vec::new();
    for (role, data) in rows.filter_map(Result::ok) {
        let Ok(p) = serde_json::from_str::<Value>(&data) else { continue };
        match p["type"].as_str() {
            Some("text") => {
                let text = p["text"].as_str().unwrap_or_default().to_string();
                if text.trim().is_empty() || p["synthetic"] == true {
                    continue;
                }
                if role.as_deref() == Some("user") {
                    items.push(Item::User { text });
                } else {
                    items.push(Item::Assistant { text });
                }
            }
            Some("reasoning") => {
                let text = p["text"].as_str().unwrap_or_default();
                if !text.trim().is_empty() {
                    items.push(Item::Reasoning { text: text.into() });
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
                items.push(Item::Tool {
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
    Ok(items)
}
