//! SQLite persistence for projects, threads and transcript items.

use crate::import::ImportedThread;
use crate::types::{AgentId, Effort, HandHolding, RunState, ThreadSource};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Thread {
    pub id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub agent: AgentId,
    pub model: Option<String>,
    pub effort: Effort,
    pub hand_holding: HandHolding,
    pub source: ThreadSource,
    /// The agent's own session/thread id, used to resume.
    pub native_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
    pub settled_at: Option<i64>,
    pub pinned_at: Option<i64>,
    pub snoozed_until: Option<i64>,
    pub archived_at: Option<i64>,
    pub run_state: RunState,
    pub additions: i64,
    pub deletions: i64,
    /// Set for side chats: the thread they were opened from. Hidden from the sidebar.
    pub side_of: Option<String>,
}

/// Sidebar section, computed from thread state (T3 Code's inbox model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Pinned,
    Inbox,
    Working,
    Snoozed,
    Settled,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Pinned => "Pinned",
            Section::Inbox => "Inbox",
            Section::Working => "Working",
            Section::Snoozed => "Snoozed",
            Section::Settled => "Settled",
        }
    }
}

impl Thread {
    pub fn is_unseen(&self) -> bool {
        self.updated_at > self.last_seen_at
    }

    pub fn needs_you(&self) -> bool {
        self.run_state == RunState::NeedsYou || self.run_state == RunState::Failed
    }

    pub fn section(&self, now: i64) -> Option<Section> {
        if self.archived_at.is_some() || self.side_of.is_some() {
            return None;
        }
        if self.pinned_at.is_some() {
            return Some(Section::Pinned);
        }
        // Snoozed threads raise their hand when they need you.
        if self.snoozed_until.is_some_and(|t| t > now) && !self.needs_you() {
            return Some(Section::Snoozed);
        }
        if self.needs_you() {
            return Some(Section::Inbox);
        }
        if self.settled_at.is_some() {
            return Some(Section::Settled);
        }
        if self.run_state == RunState::Working {
            return Some(Section::Working);
        }
        Some(Section::Inbox)
    }

    /// Whether auto-settle applies now.
    pub fn should_auto_settle(&self, now: i64, after_days: u32) -> bool {
        after_days > 0
            && self.settled_at.is_none()
            && self.pinned_at.is_none()
            && self.run_state == RunState::Idle
            && !self.is_unseen()
            && now - self.updated_at > after_days as i64 * 86_400_000
    }

    /// Urgency for sorting inside Inbox: needs-you first, then unseen, then recency.
    pub fn inbox_rank(&self) -> (u8, i64) {
        let class = match (self.run_state, self.is_unseen()) {
            (RunState::NeedsYou, _) => 0,
            (RunState::Failed, _) => 1,
            (_, true) => 2,
            _ => 3,
        };
        (class, -self.updated_at)
    }
}

/// A transcript entry. Stored as JSON so new kinds don't need migrations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Item {
    User { text: String },
    Assistant { text: String },
    Reasoning { text: String },
    Tool { id: String, title: String, detail: String, output: String, status: ToolStatus },
    Notice { text: String },
    Error { text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Done,
    Failed,
    Denied,
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS threads (
  id TEXT PRIMARY KEY, project_id TEXT, title TEXT NOT NULL, agent TEXT NOT NULL,
  model TEXT, effort TEXT NOT NULL, hand_holding TEXT NOT NULL, source TEXT NOT NULL,
  native_id TEXT, cwd TEXT, branch TEXT,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, last_seen_at INTEGER NOT NULL,
  settled_at INTEGER, pinned_at INTEGER, snoozed_until INTEGER, archived_at INTEGER,
  run_state TEXT NOT NULL DEFAULT 'idle', additions INTEGER NOT NULL DEFAULT 0, deletions INTEGER NOT NULL DEFAULT 0,
  UNIQUE(source, native_id)
);
CREATE INDEX IF NOT EXISTS threads_project ON threads(project_id, updated_at);
CREATE TABLE IF NOT EXISTS items (
  thread_id TEXT NOT NULL, seq INTEGER NOT NULL, data TEXT NOT NULL, created_at INTEGER NOT NULL,
  PRIMARY KEY (thread_id, seq)
);
"#;

fn enum_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

fn enum_from<T: for<'de> Deserialize<'de> + Default>(s: &str) -> T {
    serde_json::from_value(serde_json::Value::String(s.into())).unwrap_or_default()
}

/// Project root for a working directory: the enclosing git repo, else the folder itself.
pub fn project_root(cwd: &Path) -> PathBuf {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return d.to_path_buf();
        }
        dir = d.parent();
    }
    cwd.to_path_buf()
}

/// Additive migrations; each is a no-op once applied.
fn migrate(conn: &Connection) {
    let _ = conn.execute("ALTER TABLE threads ADD COLUMN side_of TEXT", []);
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn);
        Ok(Store { conn: Arc::new(Mutex::new(conn)) })
    }

    pub fn open_default() -> Result<Store> {
        Store::open(&crate::paths::database_file())
    }

    pub fn in_memory() -> Result<Store> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn);
        Ok(Store { conn: Arc::new(Mutex::new(conn)) })
    }

    fn with<R>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<R>) -> Result<R> {
        let conn = self.conn.lock().expect("store lock");
        Ok(f(&conn)?)
    }

    // ---- projects ----

    pub fn ensure_project(&self, path: &Path) -> Result<Project> {
        let root = project_root(path);
        let path_str = root.display().to_string();
        let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path_str.clone());
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "INSERT INTO projects(id, path, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(path) DO NOTHING",
                params![uuid::Uuid::now_v7().to_string(), path_str, name, now],
            )?;
            c.query_row("SELECT id, path, name, updated_at FROM projects WHERE path = ?1", [&path_str], |r| {
                Ok(Project { id: r.get(0)?, path: PathBuf::from(r.get::<_, String>(1)?), name: r.get(2)?, updated_at: r.get(3)? })
            })
        })
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT p.id, p.path, p.name, MAX(p.updated_at, COALESCE(MAX(t.updated_at), 0)) AS u
                 FROM projects p LEFT JOIN threads t ON t.project_id = p.id AND t.archived_at IS NULL
                 GROUP BY p.id ORDER BY u DESC",
            )?;
            let rows = st.query_map([], |r| {
                Ok(Project { id: r.get(0)?, path: PathBuf::from(r.get::<_, String>(1)?), name: r.get(2)?, updated_at: r.get(3)? })
            })?;
            rows.collect()
        })
    }

    // ---- threads ----

    const THREAD_COLS: &'static str = "id, project_id, title, agent, model, effort, hand_holding, source, native_id, cwd, branch, created_at, updated_at, last_seen_at, settled_at, pinned_at, snoozed_until, archived_at, run_state, additions, deletions, side_of";

    fn row_to_thread(r: &rusqlite::Row) -> rusqlite::Result<Thread> {
        Ok(Thread {
            id: r.get(0)?,
            project_id: r.get(1)?,
            title: r.get(2)?,
            agent: AgentId::from_key(&r.get::<_, String>(3)?),
            model: r.get(4)?,
            effort: Effort::parse(&r.get::<_, String>(5)?).unwrap_or_default(),
            hand_holding: enum_from(&r.get::<_, String>(6)?),
            source: ThreadSource::from_key(&r.get::<_, String>(7)?),
            native_id: r.get(8)?,
            cwd: r.get::<_, Option<String>>(9)?.map(PathBuf::from),
            branch: r.get(10)?,
            created_at: r.get(11)?,
            updated_at: r.get(12)?,
            last_seen_at: r.get(13)?,
            settled_at: r.get(14)?,
            pinned_at: r.get(15)?,
            snoozed_until: r.get(16)?,
            archived_at: r.get(17)?,
            run_state: enum_from(&r.get::<_, String>(18)?),
            additions: r.get(19)?,
            deletions: r.get(20)?,
            side_of: r.get(21)?,
        })
    }

    pub fn threads(&self) -> Result<Vec<Thread>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM threads WHERE archived_at IS NULL ORDER BY updated_at DESC",
                Self::THREAD_COLS
            ))?;
            let rows = st.query_map([], Self::row_to_thread)?;
            rows.collect()
        })
    }

    pub fn thread(&self, id: &str) -> Result<Option<Thread>> {
        self.with(|c| {
            c.query_row(&format!("SELECT {} FROM threads WHERE id = ?1", Self::THREAD_COLS), [id], Self::row_to_thread)
                .optional()
        })
    }

    pub fn create_thread(
        &self,
        cwd: Option<&Path>,
        agent: AgentId,
        model: Option<String>,
        effort: Effort,
        hand_holding: HandHolding,
    ) -> Result<Thread> {
        let project = cwd.map(|p| self.ensure_project(p)).transpose()?;
        let now = now_ms();
        let t = Thread {
            id: uuid::Uuid::now_v7().to_string(),
            project_id: project.map(|p| p.id),
            title: "New thread".into(),
            agent,
            model,
            effort,
            hand_holding,
            source: ThreadSource::Trek,
            native_id: None,
            cwd: cwd.map(Path::to_path_buf),
            branch: None,
            created_at: now,
            updated_at: now,
            last_seen_at: now,
            settled_at: None,
            pinned_at: None,
            snoozed_until: None,
            archived_at: None,
            run_state: RunState::Idle,
            additions: 0,
            deletions: 0,
            side_of: None,
        };
        self.save_thread(&t)?;
        Ok(t)
    }

    pub fn save_thread(&self, t: &Thread) -> Result<()> {
        self.with(|c| {
            c.execute(
                &format!(
                    "INSERT OR REPLACE INTO threads ({}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
                    Self::THREAD_COLS
                ),
                params![
                    t.id,
                    t.project_id,
                    t.title,
                    t.agent.key(),
                    t.model,
                    t.effort.as_str(),
                    enum_str(&t.hand_holding),
                    t.source.key(),
                    t.native_id,
                    t.cwd.as_ref().map(|p| p.display().to_string()),
                    t.branch,
                    t.created_at,
                    t.updated_at,
                    t.last_seen_at,
                    t.settled_at,
                    t.pinned_at,
                    t.snoozed_until,
                    t.archived_at,
                    enum_str(&t.run_state),
                    t.additions,
                    t.deletions,
                    t.side_of
                ],
            )?;
            Ok(())
        })
    }

    pub fn update_thread(&self, id: &str, f: impl FnOnce(&mut Thread)) -> Result<Option<Thread>> {
        let Some(mut t) = self.thread(id)? else { return Ok(None) };
        f(&mut t);
        self.save_thread(&t)?;
        Ok(Some(t))
    }

    /// Insert or refresh an imported thread. Imported history arrives settled so it
    /// doesn't flood the Inbox. Returns true if it was new.
    pub fn upsert_imported(&self, imp: &ImportedThread) -> Result<bool> {
        let existing: Option<(String, i64)> = self.with(|c| {
            c.query_row(
                "SELECT id, updated_at FROM threads WHERE source = ?1 AND native_id = ?2",
                params![imp.source.key(), imp.native_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })?;
        if let Some((id, updated)) = existing {
            if updated < imp.updated_at {
                // Activity that happened outside Trek isn't "unread" here.
                self.update_thread(&id, |t| {
                    let seen = t.last_seen_at >= t.updated_at;
                    t.updated_at = imp.updated_at;
                    t.title = imp.title.clone();
                    if seen {
                        t.last_seen_at = imp.updated_at;
                    }
                })?;
            }
            return Ok(false);
        }
        let project = imp.cwd.as_deref().filter(|p| p.exists()).map(|p| self.ensure_project(p)).transpose()?;
        let agent = imp.source.agent().unwrap_or(AgentId::ClaudeCode);
        let t = Thread {
            id: uuid::Uuid::now_v7().to_string(),
            project_id: project.map(|p| p.id),
            title: imp.title.clone(),
            agent,
            model: imp.model.clone(),
            effort: imp.effort.unwrap_or_default(),
            hand_holding: HandHolding::default(),
            source: imp.source,
            native_id: Some(imp.native_id.clone()),
            cwd: imp.cwd.clone(),
            branch: imp.branch.clone(),
            created_at: imp.created_at,
            updated_at: imp.updated_at,
            last_seen_at: imp.updated_at,
            settled_at: Some(imp.updated_at),
            pinned_at: None,
            snoozed_until: None,
            archived_at: None,
            run_state: RunState::Idle,
            additions: imp.additions,
            deletions: imp.deletions,
            side_of: None,
        };
        self.save_thread(&t)?;
        Ok(true)
    }

    // ---- items ----

    pub fn items(&self, thread_id: &str) -> Result<Vec<Item>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT data FROM items WHERE thread_id = ?1 ORDER BY seq")?;
            let rows = st.query_map([thread_id], |r| r.get::<_, String>(0))?;
            Ok(rows.filter_map(|r| r.ok()).filter_map(|s| serde_json::from_str(&s).ok()).collect())
        })
    }

    /// Replace the full transcript (used after a turn completes or after an import load).
    pub fn set_items(&self, thread_id: &str, items: &[Item]) -> Result<()> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM items WHERE thread_id = ?1", [thread_id])?;
        let now = now_ms();
        for (i, item) in items.iter().enumerate() {
            tx.execute(
                "INSERT INTO items(thread_id, seq, data, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![thread_id, i as i64, serde_json::to_string(item).unwrap_or_default(), now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread() -> Thread {
        let s = Store::in_memory().unwrap();
        s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap()
    }

    #[test]
    fn sections_follow_inbox_rules() {
        let now = now_ms();
        let mut t = thread();
        assert_eq!(t.section(now), Some(Section::Inbox));
        t.run_state = RunState::Working;
        assert_eq!(t.section(now), Some(Section::Working));
        t.run_state = RunState::Idle;
        t.settled_at = Some(now);
        assert_eq!(t.section(now), Some(Section::Settled));
        t.snoozed_until = Some(now + 60_000);
        assert_eq!(t.section(now), Some(Section::Snoozed));
        // Snoozed threads raise their hand.
        t.run_state = RunState::NeedsYou;
        assert_eq!(t.section(now), Some(Section::Inbox));
        t.pinned_at = Some(now);
        assert_eq!(t.section(now), Some(Section::Pinned));
    }

    #[test]
    fn thread_round_trips_through_sqlite() {
        let s = Store::in_memory().unwrap();
        let t = s.create_thread(None, AgentId::Codex, Some("gpt-6-astra".into()), Effort::Max, HandHolding::FullAccess).unwrap();
        let back = s.thread(&t.id).unwrap().unwrap();
        assert_eq!(back, t);
        s.set_items(&t.id, &[Item::User { text: "hi".into() }, Item::Assistant { text: "hello".into() }]).unwrap();
        assert_eq!(s.items(&t.id).unwrap().len(), 2);
    }

    #[test]
    fn auto_settle_waits_for_idle_and_seen() {
        let mut t = thread();
        let later = t.updated_at + 4 * 86_400_000;
        assert!(t.should_auto_settle(later, 3));
        t.updated_at += 1; // unseen
        assert!(!t.should_auto_settle(later, 3));
    }
}
