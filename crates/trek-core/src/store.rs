//! SQLite persistence for projects, threads and transcript items.

use crate::import::ImportedThread;
use crate::types::{AgentId, Effort, HandHolding, RunState, ThreadSource};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
    /// `owner/repo` from the origin remote, when there is one.
    pub remote: Option<String>,
    pub is_repo: bool,
}

/// `owner/repo` parsed from `.git/config`'s origin URL (https or ssh).
pub fn git_remote(path: &Path) -> Option<String> {
    let cfg = std::fs::read_to_string(path.join(".git/config")).ok()?;
    let mut in_origin = false;
    for line in cfg.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_origin = l == "[remote \"origin\"]";
        } else if in_origin {
            if let Some(url) = l.strip_prefix("url = ") {
                let tail = url.trim_end_matches(".git").replace(':', "/");
                let parts: Vec<&str> = tail.rsplit('/').take(2).collect();
                if parts.len() == 2 {
                    return Some(format!("{}/{}", parts[1], parts[0]));
                }
            }
        }
    }
    None
}

impl Project {
    fn from_row(id: String, path: String, name: String, updated_at: i64) -> Project {
        let path = PathBuf::from(path);
        Project { remote: git_remote(&path), is_repo: path.join(".git").exists(), id, path, name, updated_at }
    }

    /// Folders that look like real work (a repo, or one the user opened), not scratch or home.
    pub fn is_workspace(&self, user_added: &[String]) -> bool {
        if user_added.iter().any(|p| std::path::Path::new(p) == self.path) {
            return true;
        }
        let home = crate::paths::home();
        let junk = [home.clone(), home.join("Downloads"), home.join("Desktop"), home.join("Documents")];
        if junk.contains(&self.path) || self.path.starts_with("/tmp") || self.path.starts_with("/private") {
            return false;
        }
        if self.path.starts_with(home.join("Documents/Codex")) {
            return false;
        }
        self.is_repo
    }
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
    /// Exempt from auto-settle (set from the thread's context menu).
    pub never_settle: bool,
    /// The title the last import gave it; a different current title is one the user typed.
    pub imported_title: Option<String>,
    /// Archived by an import: not a conversation (see `import::Skip`), or its session was
    /// deleted. Brought back if a later import finds it as a conversation again.
    pub hidden_by_import: bool,
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
            && !self.never_settle
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
    User {
        text: String,
        /// Attached image paths (screenshots, snapshots).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<String>,
        /// When it was sent (unix ms); absent on imported history.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<i64>,
    },
    Assistant { text: String },
    /// Marks the end of a response: when it finished and how long the turn took.
    TurnEnd { at: i64, took_secs: u32 },
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

/// What an import did to one session's thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Upserted {
    pub added: bool,
    pub retitled: bool,
    pub hidden: bool,
    pub restored: bool,
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
    let _ = conn.execute("ALTER TABLE threads ADD COLUMN never_settle INTEGER NOT NULL DEFAULT 0", []);
    let _ = conn.execute("ALTER TABLE threads ADD COLUMN imported_title TEXT", []);
    let _ = conn.execute("ALTER TABLE threads ADD COLUMN hidden_by_import INTEGER NOT NULL DEFAULT 0", []);
}

fn project_in(c: &Connection, path: &Path) -> rusqlite::Result<Project> {
    let root = project_root(path);
    let path_str = root.display().to_string();
    let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path_str.clone());
    c.execute(
        "INSERT INTO projects(id, path, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(path) DO NOTHING",
        params![uuid::Uuid::now_v7().to_string(), path_str, name, now_ms()],
    )?;
    c.query_row("SELECT id, path, name, updated_at FROM projects WHERE path = ?1", [&path_str], |r| {
        Ok(Project::from_row(r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })
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
        self.with(|c| project_in(c, path))
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT p.id, p.path, p.name, MAX(p.updated_at, COALESCE(MAX(t.updated_at), 0)) AS u
                 FROM projects p LEFT JOIN threads t ON t.project_id = p.id AND t.archived_at IS NULL
                 GROUP BY p.id ORDER BY u DESC",
            )?;
            let rows = st.query_map([], |r| Ok(Project::from_row(r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            rows.collect()
        })
    }

    // ---- threads ----

    const THREAD_COLS: &'static str = "id, project_id, title, agent, model, effort, hand_holding, source, native_id, cwd, branch, created_at, updated_at, last_seen_at, settled_at, pinned_at, snoozed_until, archived_at, run_state, additions, deletions, side_of, never_settle, imported_title, hidden_by_import";

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
            never_settle: r.get::<_, i64>(22)? != 0,
            imported_title: r.get(23)?,
            hidden_by_import: r.get::<_, i64>(24)? != 0,
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
            never_settle: false,
            imported_title: None,
            hidden_by_import: false,
        };
        self.save_thread(&t)?;
        Ok(t)
    }

    pub fn save_thread(&self, t: &Thread) -> Result<()> {
        self.with(|c| Self::save_thread_in(c, t))
    }

    fn save_thread_in(c: &Connection, t: &Thread) -> rusqlite::Result<()> {
        c.execute(
            &format!(
                "INSERT OR REPLACE INTO threads ({}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)",
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
                t.side_of,
                t.never_settle as i64,
                t.imported_title,
                t.hidden_by_import as i64
            ],
        )?;
        Ok(())
    }

    pub fn update_thread(&self, id: &str, f: impl FnOnce(&mut Thread)) -> Result<Option<Thread>> {
        let Some(mut t) = self.thread(id)? else { return Ok(None) };
        f(&mut t);
        self.save_thread(&t)?;
        Ok(Some(t))
    }

    /// Agent session ids of Trek's own threads, archived ones included.
    pub fn trek_native_ids(&self) -> Result<HashSet<String>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT native_id FROM threads WHERE source = 'trek' AND native_id IS NOT NULL")?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect()
        })
    }

    /// Insert or refresh imported threads, in one transaction. Imported history arrives settled
    /// so it doesn't flood the Inbox. Sessions that aren't conversations are never added, and
    /// threads imported before a rule matched them are archived.
    pub fn upsert_imported(&self, found: &[ImportedThread]) -> Result<Vec<Upserted>> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        let out = found.iter().map(|imp| Self::upsert_one(&tx, imp)).collect::<rusqlite::Result<Vec<_>>>()?;
        tx.commit()?;
        Ok(out)
    }

    fn upsert_one(c: &Connection, imp: &ImportedThread) -> rusqlite::Result<Upserted> {
        let mut out = Upserted::default();
        let existing = c
            .query_row(
                &format!("SELECT {} FROM threads WHERE source = ?1 AND native_id = ?2", Self::THREAD_COLS),
                params![imp.source.key(), imp.native_id],
                Self::row_to_thread,
            )
            .optional()?;
        let Some(mut t) = existing else {
            if imp.skip.is_none() {
                Self::save_thread_in(c, &Self::new_imported(c, imp)?)?;
                out.added = true;
            }
            return Ok(out);
        };
        let before = t.clone();
        match imp.skip {
            // Threads the user kept (pinned, or continued in Trek) stay where they are.
            Some(_) if t.archived_at.is_none() && t.pinned_at.is_none() && !Self::has_items_in(c, &t.id)? => {
                t.archived_at = Some(now_ms());
                t.hidden_by_import = true;
                out.hidden = true;
            }
            None if t.hidden_by_import => {
                t.archived_at = None;
                t.hidden_by_import = false;
                out.restored = true;
            }
            _ => {}
        }
        // Automatic titles follow the source; a title the user typed in Trek stays.
        let previous = t.imported_title.as_deref().or(imp.legacy_title.as_deref());
        if previous.is_none_or(|p| p == t.title) && t.title != imp.title {
            t.title = imp.title.clone();
            out.retitled = true;
        }
        t.imported_title = Some(imp.title.clone());
        if t.updated_at < imp.updated_at {
            // Activity that happened outside Trek isn't "unread" here.
            if t.last_seen_at >= t.updated_at {
                t.last_seen_at = imp.updated_at;
            }
            t.updated_at = imp.updated_at;
        }
        if t != before {
            Self::save_thread_in(c, &t)?;
        }
        Ok(out)
    }

    fn new_imported(c: &Connection, imp: &ImportedThread) -> rusqlite::Result<Thread> {
        let project = imp.cwd.as_deref().filter(|p| p.exists()).map(|p| project_in(c, p)).transpose()?;
        Ok(Thread {
            id: uuid::Uuid::now_v7().to_string(),
            project_id: project.map(|p| p.id),
            title: imp.title.clone(),
            agent: imp.source.agent().unwrap_or(AgentId::ClaudeCode),
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
            never_settle: false,
            imported_title: Some(imp.title.clone()),
            hidden_by_import: false,
        })
    }

    /// Archive imported threads of `source` whose session is no longer in the agent's history
    /// (`known`), unless the user kept them: pinned, or continued here so Trek has the transcript.
    pub fn hide_missing(&self, source: ThreadSource, known: &HashSet<String>) -> Result<usize> {
        if source == ThreadSource::Trek {
            return Ok(0);
        }
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        let candidates: Vec<(String, String)> = {
            let mut st = tx.prepare(
                "SELECT id, native_id FROM threads
                 WHERE source = ?1 AND native_id IS NOT NULL AND archived_at IS NULL AND pinned_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM items WHERE items.thread_id = threads.id)",
            )?;
            st.query_map([source.key()], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
        };
        let now = now_ms();
        let mut hidden = 0;
        for (id, _) in candidates.iter().filter(|(_, native)| !known.contains(native)) {
            hidden += tx.execute("UPDATE threads SET archived_at = ?2, hidden_by_import = 1 WHERE id = ?1", params![id, now])?;
        }
        tx.commit()?;
        Ok(hidden)
    }

    fn has_items_in(c: &Connection, thread_id: &str) -> rusqlite::Result<bool> {
        c.query_row("SELECT EXISTS (SELECT 1 FROM items WHERE thread_id = ?1)", [thread_id], |r| r.get(0))
    }

    /// Remove a thread and its transcript from Trek's database.
    pub fn delete_thread(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM items WHERE thread_id = ?1", [id])?;
            c.execute("DELETE FROM threads WHERE id = ?1 OR side_of = ?1", [id])?;
            Ok(())
        })
    }

    pub fn rename_project(&self, id: &str, name: &str) -> Result<()> {
        self.with(|c| {
            c.execute("UPDATE projects SET name = ?2 WHERE id = ?1", params![id, name])?;
            Ok(())
        })
    }

    /// Archive every thread of a project (used when the project is removed from Trek).
    pub fn archive_project_threads(&self, project_id: &str) -> Result<usize> {
        self.with(|c| c.execute("UPDATE threads SET archived_at = ?2 WHERE project_id = ?1 AND archived_at IS NULL", params![project_id, now_ms()]))
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
        s.set_items(&t.id, &[Item::User { text: "hi".into(), images: vec![], at: None }, Item::Assistant { text: "hello".into() }]).unwrap();
        assert_eq!(s.items(&t.id).unwrap().len(), 2);
    }

    fn imported(id: &str, title: &str) -> ImportedThread {
        ImportedThread {
            source: ThreadSource::ClaudeCode,
            native_id: id.into(),
            title: title.into(),
            cwd: None,
            branch: None,
            model: None,
            effort: None,
            created_at: 1,
            updated_at: 10,
            additions: 0,
            deletions: 0,
            skip: None,
            legacy_title: None,
        }
    }

    fn by_native(s: &Store, id: &str) -> Thread {
        s.with(|c| c.query_row(&format!("SELECT {} FROM threads WHERE native_id = ?1", Store::THREAD_COLS), [id], Store::row_to_thread)).unwrap()
    }

    #[test]
    fn imports_retitle_unless_the_user_renamed() {
        let s = Store::in_memory().unwrap();
        let out = s.upsert_imported(&[imported("a", "fix the login bug"), imported("b", "add dark mode")]).unwrap();
        assert!(out.iter().all(|o| o.added));
        s.update_thread(&by_native(&s, "b").id, |t| t.title = "Theme work".into()).unwrap();
        let out = s.upsert_imported(&[imported("a", "Login redirect loop fix"), imported("b", "Dark mode toggle")]).unwrap();
        assert_eq!(out.iter().map(|o| o.retitled).collect::<Vec<_>>(), [true, false]);
        assert_eq!(by_native(&s, "a").title, "Login redirect loop fix");
        assert_eq!(by_native(&s, "b").title, "Theme work");
        // Nothing changed: nothing written.
        assert_eq!(s.upsert_imported(&[imported("a", "Login redirect loop fix")]).unwrap(), [Upserted::default()]);
    }

    #[test]
    fn threads_from_older_imports_are_retitled_by_their_old_title() {
        let s = Store::in_memory().unwrap();
        s.upsert_imported(&[imported("auto", "x"), imported("mine", "x")]).unwrap();
        // As earlier versions left them: no imported title on record.
        for (id, title) in [("auto", "Base directory for this skill: /tmp/skills/review"), ("mine", "Stadium work")] {
            s.update_thread(&by_native(&s, id).id, |t| {
                t.title = title.into();
                t.imported_title = None;
            })
            .unwrap();
        }
        let legacy = |id: &str| ImportedThread { legacy_title: Some("Base directory for this skill: /tmp/skills/review".into()), ..imported(id, "Stadium camera fix") };
        s.upsert_imported(&[legacy("auto"), legacy("mine")]).unwrap();
        assert_eq!(by_native(&s, "auto").title, "Stadium camera fix");
        assert_eq!(by_native(&s, "mine").title, "Stadium work");
        assert_eq!(by_native(&s, "mine").imported_title.as_deref(), Some("Stadium camera fix"));
    }

    #[test]
    fn helper_sessions_are_never_added_and_old_ones_are_archived() {
        let s = Store::in_memory().unwrap();
        let skip = |id: &str| ImportedThread { skip: Some(crate::import::Skip::TempDir), ..imported(id, "say ok") };
        assert_eq!(s.upsert_imported(&[skip("new")]).unwrap(), [Upserted::default()]);
        assert!(s.threads().unwrap().is_empty());
        s.upsert_imported(&[imported("old", "say ok"), imported("pinned", "say ok"), imported("used", "say ok")]).unwrap();
        s.update_thread(&by_native(&s, "pinned").id, |t| t.pinned_at = Some(5)).unwrap();
        s.set_items(&by_native(&s, "used").id, &[Item::User { text: "keep going".into(), images: vec![], at: None }]).unwrap();
        let out = s.upsert_imported(&[skip("old"), skip("pinned"), skip("used")]).unwrap();
        // Threads the user pinned or continued in Trek stay.
        assert_eq!(out.iter().map(|o| o.hidden).collect::<Vec<_>>(), [true, false, false]);
        let old = by_native(&s, "old");
        assert!(old.archived_at.is_some() && old.hidden_by_import);
        // A fork continued since: back in the sidebar. A thread the user archived stays archived.
        s.update_thread(&by_native(&s, "pinned").id, |t| t.archived_at = Some(7)).unwrap();
        let out = s.upsert_imported(&[imported("old", "say ok"), imported("pinned", "say ok")]).unwrap();
        assert_eq!(out.iter().map(|o| o.restored).collect::<Vec<_>>(), [true, false]);
        assert!(by_native(&s, "old").archived_at.is_none());
        assert!(by_native(&s, "pinned").archived_at.is_some());
    }

    #[test]
    fn threads_whose_session_was_deleted_are_archived() {
        let s = Store::in_memory().unwrap();
        s.upsert_imported(&[imported("here", "t"), imported("deleted", "t"), imported("continued", "t")]).unwrap();
        s.set_items(&by_native(&s, "continued").id, &[Item::Assistant { text: "kept in Trek".into() }]).unwrap();
        let known = HashSet::from(["here".to_string()]);
        assert_eq!(s.hide_missing(ThreadSource::ClaudeCode, &known).unwrap(), 1);
        assert!(by_native(&s, "deleted").hidden_by_import);
        assert!(by_native(&s, "continued").archived_at.is_none());
        // Other sources' threads aren't judged by this source's history.
        assert_eq!(s.hide_missing(ThreadSource::Codex, &HashSet::new()).unwrap(), 0);
        // Back on disk (restored from a backup): back in the sidebar.
        assert!(s.upsert_imported(&[imported("deleted", "t")]).unwrap()[0].restored);
    }

    #[test]
    fn imported_activity_moves_threads_without_making_them_unread() {
        let s = Store::in_memory().unwrap();
        s.upsert_imported(&[imported("a", "t")]).unwrap();
        s.upsert_imported(&[ImportedThread { updated_at: 50, ..imported("a", "t") }]).unwrap();
        let t = by_native(&s, "a");
        assert_eq!((t.updated_at, t.last_seen_at), (50, 50));
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
