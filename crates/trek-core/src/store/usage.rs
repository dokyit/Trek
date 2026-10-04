//! What threads did when, for Basecamp's recap: token usage per turn as agents report it, turns
//! that failed or were stopped, and the prompts and turn ends stored in transcripts. Imported history that hasn't been continued here
//! isn't in the store; `basecamp` reads it from the agents' own files.

use super::{Store, Thread};
use crate::pricing::Spend;
use crate::types::{AgentId, TokenUsage, UsageCost};
use anyhow::Result;
use rusqlite::params;
use std::collections::{HashMap, HashSet};

/// One row per model a turn used (sub-agents on another model add rows of their own), with
/// what it cost at API prices when that was known (`cost_reported`: the agent's own figure).
pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS token_usage (
  thread_id TEXT NOT NULL, at INTEGER NOT NULL, agent TEXT NOT NULL, model TEXT,
  input INTEGER NOT NULL, output INTEGER NOT NULL, cache_read INTEGER NOT NULL, cache_write INTEGER NOT NULL,
  cost_usd REAL, cost_reported INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS token_usage_at ON token_usage(at);
CREATE INDEX IF NOT EXISTS token_usage_thread ON token_usage(thread_id);
CREATE TABLE IF NOT EXISTS turn_stops (
  thread_id TEXT NOT NULL, at INTEGER NOT NULL, took_secs INTEGER NOT NULL, failed INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS turn_stops_at ON turn_stops(at);";

/// Columns `token_usage` gained after it was first made.
const COLUMNS_ADDED: &[(&str, &str)] = &[("cost_usd", "REAL"), ("cost_reported", "INTEGER NOT NULL DEFAULT 0")];

pub(super) fn migrate(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    let have: HashSet<String> = conn.prepare("SELECT name FROM pragma_table_info('token_usage')")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    for (name, decl) in COLUMNS_ADDED {
        if !have.contains(*name) {
            conn.execute(&format!("ALTER TABLE token_usage ADD COLUMN {name} {decl}"), [])?;
        }
    }
    Ok(())
}

/// Tokens one model used in one turn of a thread.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    pub thread_id: String,
    /// When the agent reported it (unix ms): the end of the turn.
    pub at: i64,
    pub agent: AgentId,
    pub model: Option<String>,
    pub tokens: TokenUsage,
    /// What it cost at API prices, as recorded (`None`: no price was known then).
    pub cost: Option<UsageCost>,
}

impl UsageRow {
    /// Spend of `rows`, priced from the table where no cost was recorded.
    pub fn spend<'a>(rows: impl IntoIterator<Item = &'a UsageRow>) -> Spend {
        let mut s = Spend::default();
        for r in rows {
            s.add(&r.agent, r.model.as_deref(), &r.tokens, r.cost, r.at);
        }
        s
    }
}

const ROW_COLS: &str = "thread_id, at, agent, model, input, output, cache_read, cache_write, cost_usd, cost_reported";

fn row(r: &rusqlite::Row) -> rusqlite::Result<UsageRow> {
    Ok(UsageRow {
        thread_id: r.get(0)?,
        at: r.get(1)?,
        agent: AgentId::from_key(&r.get::<_, String>(2)?),
        model: r.get(3)?,
        tokens: TokenUsage {
            input: r.get::<_, i64>(4)?.max(0) as u64,
            output: r.get::<_, i64>(5)?.max(0) as u64,
            cache_read: r.get::<_, i64>(6)?.max(0) as u64,
            cache_write: r.get::<_, i64>(7)?.max(0) as u64,
        },
        cost: r.get::<_, Option<f64>>(8)?.map(|usd| UsageCost { usd, reported: r.get::<_, bool>(9).unwrap_or(false) }),
    })
}

/// A transcript entry that tells when work happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// The user sent a message (asides, like `/model` or a typed answer, aren't prompts).
    Prompt { at: i64 },
    /// A turn finished cleanly after `took_secs`.
    TurnEnd { at: i64, took_secs: u32 },
    /// A turn failed, or was stopped, after `took_secs`. Transcripts don't keep these as turn
    /// ends; Trek records them apart.
    TurnStopped { at: i64, took_secs: u32, failed: bool },
}

impl Activity {
    pub fn at(&self) -> i64 {
        match self {
            Activity::Prompt { at } | Activity::TurnEnd { at, .. } | Activity::TurnStopped { at, .. } => *at,
        }
    }
}

impl Store {
    /// Keep what one model used in a turn of `thread_id`, and what it cost if that's known. A
    /// report with neither tokens nor a cost isn't kept.
    pub fn record_usage(&self, thread_id: &str, at: i64, agent: &AgentId, model: Option<&str>, tokens: &TokenUsage, cost: Option<UsageCost>) -> Result<()> {
        if tokens.is_empty() && cost.is_none_or(|c| c.usd <= 0.0) {
            return Ok(());
        }
        self.with(|c| {
            c.prepare_cached(
                "INSERT INTO token_usage (thread_id, at, agent, model, input, output, cache_read, cache_write, cost_usd, cost_reported) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?
            .execute(params![
                thread_id,
                at,
                agent.key(),
                model,
                tokens.input as i64,
                tokens.output as i64,
                tokens.cache_read as i64,
                tokens.cache_write as i64,
                cost.map(|c| c.usd),
                cost.is_some_and(|c| c.reported)
            ])?;
            Ok(())
        })
    }

    /// Everything `threads` reported, oldest first.
    pub fn usage_of(&self, threads: &[String]) -> Result<Vec<UsageRow>> {
        self.reading(|c| {
            let mut st = c.prepare_cached(&format!("SELECT {ROW_COLS} FROM token_usage WHERE thread_id = ?1 ORDER BY at"))?;
            let mut out = Vec::new();
            for id in threads {
                for r in st.query_map([id], row)? {
                    out.push(r?);
                }
            }
            out.sort_by_key(|r| r.at);
            Ok(out)
        })
    }

    /// The sub-agents `id` started, theirs, and so on down.
    pub fn descendants(&self, id: &str) -> Result<Vec<String>> {
        self.reading(|c| {
            let mut st = c.prepare_cached(
                "WITH RECURSIVE sub(id) AS (
                   SELECT id FROM threads WHERE parent_id = ?1
                   UNION SELECT t.id FROM threads t JOIN sub ON t.parent_id = sub.id
                 ) SELECT id FROM sub",
            )?;
            let rows = st.query_map([id], |r| r.get(0))?;
            rows.collect()
        })
    }

    /// A turn of `thread_id` that failed (or was stopped, `failed` false) at `at`.
    pub fn record_stop(&self, thread_id: &str, at: i64, took_secs: u32, failed: bool) -> Result<()> {
        self.with(|c| {
            c.prepare_cached("INSERT INTO turn_stops (thread_id, at, took_secs, failed) VALUES (?1, ?2, ?3, ?4)")?.execute(params![thread_id, at, took_secs, failed])?;
            Ok(())
        })
    }

    /// When Trek first recorded usage for each of `threads` (those it has recorded any for).
    pub fn first_usage(&self, threads: &[String]) -> Result<HashMap<String, i64>> {
        self.reading(|c| {
            let mut st = c.prepare_cached("SELECT MIN(at) FROM token_usage WHERE thread_id = ?1")?;
            let mut out = HashMap::new();
            for id in threads {
                if let Some(at) = st.query_row([id], |r| r.get::<_, Option<i64>>(0))? {
                    out.insert(id.clone(), at);
                }
            }
            Ok(out)
        })
    }

    /// Usage reported in `[from, to)`, oldest first.
    pub fn usage_between(&self, from: i64, to: i64) -> Result<Vec<UsageRow>> {
        self.reading(|c| {
            let mut st = c.prepare(&format!("SELECT {ROW_COLS} FROM token_usage WHERE at >= ?1 AND at < ?2 ORDER BY at"))?;
            let rows = st.query_map(params![from, to], row)?;
            rows.collect()
        })
    }

    /// Threads that may have done something since `from`: active since then, and not archived
    /// by an import (a helper session, or one whose history is gone). Threads the user archived
    /// still count; the work happened.
    pub fn threads_since(&self, from: i64) -> Result<Vec<Thread>> {
        self.reading(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM threads WHERE updated_at >= ?1 AND import_hidden IS NULL AND (archived_at IS NULL OR source = 'trek') ORDER BY updated_at DESC",
                Self::THREAD_COLS
            ))?;
            let rows = st.query_map([from], Self::row_to_thread)?;
            rows.collect()
        })
    }

    /// Of `threads`, the ones with a stored transcript (Trek's own, or imported ones continued here).
    pub fn with_transcripts(&self, threads: &[String]) -> Result<HashSet<String>> {
        self.reading(|c| {
            let mut st = c.prepare_cached("SELECT EXISTS (SELECT 1 FROM items WHERE thread_id = ?1)")?;
            let mut out = HashSet::new();
            for id in threads {
                if st.query_row([id], |r| r.get::<_, bool>(0))? {
                    out.insert(id.clone());
                }
            }
            Ok(out)
        })
    }

    /// Prompts, turn ends and stopped turns with a time in `[from, to)`, by thread: transcript
    /// entries in transcript order, then the stops. Only threads active since `from` are looked
    /// through. The "Continue" a resume at a limit's reset sends on its own isn't the user's, nor
    /// is a sub-agent's brief or the message Trek wakes an agent with when its sub-agents report.
    pub fn activity_between(&self, from: i64, to: i64) -> Result<Vec<(String, Activity)>> {
        self.reading(|c| {
            let mut st = c.prepare(
                "SELECT i.thread_id, json_extract(i.data, '$.kind'), json_extract(i.data, '$.at'), json_extract(i.data, '$.took_secs')
                 FROM items i JOIN threads t ON t.id = i.thread_id
                 WHERE t.updated_at >= ?1
                   AND json_extract(i.data, '$.kind') IN ('user', 'turn_end')
                   AND COALESCE(json_extract(i.data, '$.aside'), 0) = 0
                   AND NOT (json_extract(i.data, '$.kind') = 'user' AND json_extract(i.data, '$.text') IS ?3)
                   AND NOT (json_extract(i.data, '$.kind') = 'user' AND (t.parent_id IS NOT NULL OR substr(json_extract(i.data, '$.text'), 1, ?4) = ?5))
                   AND json_extract(i.data, '$.at') >= ?1 AND json_extract(i.data, '$.at') < ?2
                 ORDER BY i.thread_id, i.seq",
            )?;
            let rows = st.query_map(params![from, to, crate::limit::CONTINUE, crate::orchestrate::WAKE_PREFIX.chars().count() as i64, crate::orchestrate::WAKE_PREFIX], |r| {
                let kind: String = r.get(1)?;
                let at: i64 = r.get(2)?;
                let activity = match kind.as_str() {
                    "user" => Activity::Prompt { at },
                    _ => Activity::TurnEnd { at, took_secs: r.get::<_, Option<i64>>(3)?.unwrap_or(0).clamp(0, u32::MAX as i64) as u32 },
                };
                Ok((r.get::<_, String>(0)?, activity))
            })?;
            let mut out: Vec<(String, Activity)> = rows.collect::<rusqlite::Result<_>>()?;
            let mut st = c.prepare("SELECT thread_id, at, took_secs, failed FROM turn_stops WHERE at >= ?1 AND at < ?2 ORDER BY at")?;
            let stops = st.query_map(params![from, to], |r| {
                let took_secs = r.get::<_, i64>(2)?.clamp(0, u32::MAX as i64) as u32;
                Ok((r.get::<_, String>(0)?, Activity::TurnStopped { at: r.get(1)?, took_secs, failed: r.get(3)? }))
            })?;
            for stop in stops {
                out.push(stop?);
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Item;
    use crate::types::{Effort, HandHolding};

    fn user(text: &str, at: i64, aside: bool) -> Item {
        Item::User { text: text.into(), images: vec![], at: Some(at), resume: None, aside }
    }

    #[test]
    fn usage_is_kept_per_turn_and_read_back_by_time() {
        let s = Store::in_memory().unwrap();
        let t = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
        let tokens = TokenUsage { input: 10, output: 40, cache_read: 22_000, cache_write: 1_000 };
        s.record_usage(&t.id, 1_000, &AgentId::ClaudeCode, Some("claude-opus-5-5"), &tokens, Some(UsageCost::reported(0.0161))).unwrap();
        s.record_usage(&t.id, 2_000, &AgentId::ClaudeCode, None, &TokenUsage { output: 5, ..Default::default() }, None).unwrap();
        // Nothing used, nothing kept.
        s.record_usage(&t.id, 3_000, &AgentId::ClaudeCode, None, &TokenUsage::default(), None).unwrap();
        let rows = s.usage_between(0, 10_000).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], UsageRow { thread_id: t.id.clone(), at: 1_000, agent: AgentId::ClaudeCode, model: Some("claude-opus-5-5".into()), tokens, cost: Some(UsageCost::reported(0.0161)) });
        assert_eq!(rows[1].cost, None);
        assert_eq!(s.usage_between(1_500, 10_000).unwrap().len(), 1);
        assert_eq!(s.first_usage(&[t.id.clone(), "other".into()]).unwrap(), HashMap::from([(t.id.clone(), 1_000)]));
        // A deleted thread's usage goes with it.
        s.delete_thread(&t.id).unwrap();
        assert!(s.usage_between(0, 10_000).unwrap().is_empty());
    }

    #[test]
    fn cost_is_kept_with_the_tokens_and_survives_reopening() {
        let path = std::env::temp_dir().join(format!("trek-usage-cost-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (parent, child, grandchild);
        {
            let s = Store::open(&path).unwrap();
            parent = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap().id;
            let mut c = s.create_thread(None, AgentId::Codex, None, Effort::Low, HandHolding::Auto).unwrap();
            c.parent_id = Some(parent.clone());
            s.save_thread(&c).unwrap();
            child = c.id;
            let mut g = s.create_thread(None, AgentId::Codex, None, Effort::Low, HandHolding::Auto).unwrap();
            g.parent_id = Some(child.clone());
            s.save_thread(&g).unwrap();
            grandchild = g.id;
            let t = TokenUsage { input: 1_000, output: 100, cache_read: 0, cache_write: 0 };
            s.record_usage(&parent, 1, &AgentId::ClaudeCode, Some("claude-sonnet-5-5"), &t, Some(UsageCost::reported(0.25))).unwrap();
            s.record_usage(&child, 2, &AgentId::Codex, Some("gpt-5.6-luna"), &t, Some(UsageCost::priced(0.00032))).unwrap();
            s.record_usage(&grandchild, 3, &AgentId::Codex, Some("unknown-model"), &t, None).unwrap();
            // A cost with no tokens (an agent that reports only what it spent) is kept.
            s.record_usage(&parent, 4, &AgentId::OpenCode, Some("anthropic/claude-sonnet-5-5"), &TokenUsage::default(), Some(UsageCost::reported(0.1))).unwrap();
        }
        let s = Store::open(&path).unwrap();
        let own = UsageRow::spend(&s.usage_of(std::slice::from_ref(&parent)).unwrap());
        assert!((own.usd() - 0.35).abs() < 1e-9);
        let subs = s.descendants(&parent).unwrap();
        assert_eq!(subs.len(), 2);
        let theirs = UsageRow::spend(&s.usage_of(&subs).unwrap());
        assert!((theirs.usd() - 0.00032).abs() < 1e-12);
        assert_eq!(theirs.unpriced(), 1_100);
        drop(s);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_older_usage_table_gains_the_cost_columns() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE token_usage (thread_id TEXT NOT NULL, at INTEGER NOT NULL, agent TEXT NOT NULL, model TEXT,
               input INTEGER NOT NULL, output INTEGER NOT NULL, cache_read INTEGER NOT NULL, cache_write INTEGER NOT NULL);
             INSERT INTO token_usage VALUES ('t', 1, 'codex', 'gpt-5.6-luna', 10, 5, 0, 0);",
        )
        .unwrap();
        migrate(&c).unwrap();
        migrate(&c).unwrap();
        let got = c.query_row(&format!("SELECT {ROW_COLS} FROM token_usage"), [], row).unwrap();
        assert_eq!(got.cost, None);
        assert_eq!(got.tokens.output, 5);
    }

    #[test]
    fn activity_is_prompts_and_turn_ends_in_the_window() {
        let s = Store::in_memory().unwrap();
        let t = s.create_thread(None, AgentId::Codex, None, Effort::High, HandHolding::Auto).unwrap();
        let items = vec![
            user("yesterday", 500, false),
            Item::TurnEnd { at: 900, took_secs: 400 },
            user("go", 1_000, false),
            Item::Assistant { text: "ok".into() },
            Item::TurnEnd { at: 1_200, took_secs: 12 },
            user("/model", 1_300, true),
            // Sent by a resume at a limit's reset, not by the user.
            user(crate::limit::CONTINUE, 1_400, false),
            Item::User { text: "no time".into(), images: vec![], at: None, resume: None, aside: false },
            user("later", 5_000, false),
        ];
        let mut tr = crate::transcript::Transcript::unsaved(items);
        s.save_transcript(&t.id, &mut tr).unwrap();
        let got: Vec<Activity> = s.activity_between(1_000, 5_000).unwrap().into_iter().map(|(_, a)| a).collect();
        assert_eq!(got, vec![Activity::Prompt { at: 1_000 }, Activity::TurnEnd { at: 1_200, took_secs: 12 }]);
        assert_eq!(s.with_transcripts(&[t.id.clone(), "other".into()]).unwrap(), HashSet::from([t.id.clone()]));
        assert_eq!(s.threads_since(0).unwrap().len(), 1);
        // Turns that failed or were stopped are kept apart, and come after the transcript's.
        s.record_stop(&t.id, 1_500, 30, true).unwrap();
        s.record_stop(&t.id, 6_000, 5, false).unwrap();
        let got: Vec<Activity> = s.activity_between(1_000, 5_000).unwrap().into_iter().map(|(_, a)| a).collect();
        assert_eq!(got.last(), Some(&Activity::TurnStopped { at: 1_500, took_secs: 30, failed: true }));
        assert_eq!(got.len(), 3);
        assert!(s.threads_since(t.updated_at + 1).unwrap().is_empty());
    }
}
