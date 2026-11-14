//! What the agent's own sub-agents (Claude Code's Task, Codex's agents) did, as their agent
//! reported it: kept with their thread, so their steps outlive the session and Trek's restart.
//! Each sub-agent is keyed by the tool call that started it; its steps are numbered for good (`n`
//! counts from its first, as `SubLog` in the app numbers them), so one that grows or changes is
//! written a row at a time, and the earliest let go are deleted as the app lets them go.
//!
//! Tables of their own: an older Trek's database gains them empty, and an older Trek never reads
//! them.

use super::{Item, Store, lock};
use anyhow::Result;
use rusqlite::params;
use std::collections::HashMap;

pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS sub_logs (
  thread_id TEXT NOT NULL, call_id TEXT NOT NULL, prompt TEXT, PRIMARY KEY (thread_id, call_id)
);
CREATE TABLE IF NOT EXISTS sub_log_items (
  thread_id TEXT NOT NULL, call_id TEXT NOT NULL, n INTEGER NOT NULL, data TEXT NOT NULL,
  PRIMARY KEY (thread_id, call_id, n)
);";

/// One sub-agent's steps as stored: its prompt, and its steps oldest first with their numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoredSubLog {
    pub prompt: Option<String>,
    pub items: Vec<(usize, Item)>,
}

/// What changed in one sub-agent's steps since they were last written.
#[derive(Debug, Clone, Copy)]
pub struct SubLogChange<'a> {
    /// Its prompt, when that changed.
    pub prompt: Option<&'a str>,
    /// Steps new or changed, by number.
    pub items: &'a [(usize, &'a Item)],
    /// Steps numbered below this are let go.
    pub dropped: usize,
}

impl Store {
    /// Write what changed in the steps of `thread`'s sub-agent started by call `call`.
    pub fn save_sub_log(&self, thread: &str, call: &str, change: SubLogChange<'_>) -> Result<()> {
        let mut conn = lock(&self.conn);
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO sub_logs (thread_id, call_id, prompt) VALUES (?1, ?2, ?3)
             ON CONFLICT(thread_id, call_id) DO UPDATE SET prompt = COALESCE(excluded.prompt, prompt)",
            params![thread, call, change.prompt],
        )?;
        {
            let mut put = tx.prepare_cached(
                "INSERT OR REPLACE INTO sub_log_items (thread_id, call_id, n, data) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (n, item) in change.items {
                if *n < change.dropped {
                    continue;
                }
                put.execute(params![
                    thread,
                    call,
                    *n as i64,
                    serde_json::to_string(item)?
                ])?;
            }
        }
        if change.dropped > 0 {
            tx.execute(
                "DELETE FROM sub_log_items WHERE thread_id = ?1 AND call_id = ?2 AND n < ?3",
                params![thread, call, change.dropped as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The steps of each of `thread`'s sub-agents kept, by the call that started it. Steps a
    /// build can't read (written by a newer one) are left out.
    pub fn sub_logs(&self, thread: &str) -> Result<HashMap<String, StoredSubLog>> {
        self.with(|c| {
            let mut out: HashMap<String, StoredSubLog> = HashMap::new();
            let mut st =
                c.prepare_cached("SELECT call_id, prompt FROM sub_logs WHERE thread_id = ?1")?;
            let heads = st.query_map([thread], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })?;
            for head in heads {
                let (call, prompt) = head?;
                out.entry(call).or_default().prompt = prompt;
            }
            let mut st = c.prepare_cached(
                "SELECT call_id, n, data FROM sub_log_items WHERE thread_id = ?1 ORDER BY call_id, n",
            )?;
            let rows = st.query_map([thread], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (call, n, data) = row?;
                if let Ok(item) = serde_json::from_str::<Item>(&data) {
                    out.entry(call).or_default().items.push((n as usize, item));
                }
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ToolStatus;
    use crate::types::{AgentId, Effort, HandHolding};

    fn said(text: &str) -> Item {
        Item::Assistant { text: text.into() }
    }

    #[test]
    fn a_sub_agent_s_steps_are_written_as_they_change_and_let_go_as_they_drop() {
        let s = Store::in_memory().unwrap();
        let t = s
            .create_thread(
                None,
                AgentId::ClaudeCode,
                None,
                Effort::Medium,
                HandHolding::Auto,
            )
            .unwrap();
        let call = Item::Tool {
            id: "k1".into(),
            title: "Read".into(),
            detail: "src/lib.rs".into(),
            output: String::new(),
            status: ToolStatus::Running,
        };
        let (a, b) = (said("Looking"), said("Found it"));
        s.save_sub_log(
            &t.id,
            "toolu_a",
            SubLogChange {
                prompt: Some("Map the routes"),
                items: &[(0, &a), (1, &call)],
                dropped: 0,
            },
        )
        .unwrap();
        // Its call's output arrives, and it says more: only those are written.
        let back = Item::Tool {
            id: "k1".into(),
            title: "Read".into(),
            detail: "src/lib.rs".into(),
            output: "fn main() {}".into(),
            status: ToolStatus::Done,
        };
        s.save_sub_log(
            &t.id,
            "toolu_a",
            SubLogChange {
                prompt: None,
                items: &[(1, &back), (2, &b)],
                dropped: 0,
            },
        )
        .unwrap();
        let logs = s.sub_logs(&t.id).unwrap();
        let log = &logs["toolu_a"];
        assert_eq!(log.prompt.as_deref(), Some("Map the routes"), "kept");
        assert_eq!(
            log.items,
            vec![(0, a.clone()), (1, back.clone()), (2, b.clone())]
        );
        // The earliest let go go from the store too.
        s.save_sub_log(
            &t.id,
            "toolu_a",
            SubLogChange {
                prompt: None,
                items: &[],
                dropped: 2,
            },
        )
        .unwrap();
        assert_eq!(s.sub_logs(&t.id).unwrap()["toolu_a"].items, vec![(2, b)]);
        // Unreadable steps (a newer Trek's) are left out.
        s.with(|c| {
            c.execute(
                "INSERT INTO sub_log_items (thread_id, call_id, n, data) VALUES (?1, 'toolu_a', 3, '{\"kind\":\"from_the_future\"}')",
                [&t.id],
            )
        })
        .unwrap();
        assert_eq!(s.sub_logs(&t.id).unwrap()["toolu_a"].items.len(), 1);
        // They go with their thread.
        s.delete_thread(&t.id).unwrap();
        assert!(s.sub_logs(&t.id).unwrap().is_empty());
        let left: i64 = s
            .with(|c| c.query_row("SELECT COUNT(*) FROM sub_log_items", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(left, 0);
    }
}
