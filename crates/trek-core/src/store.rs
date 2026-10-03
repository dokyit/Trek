//! SQLite persistence for projects, threads and transcript items.
//!
//! Transcripts are append-only: every item is a row with a stable id (uuid v7) and a `seq` that
//! orders it within its thread. Saving a live transcript writes only the rows that are new,
//! changed or gone (`save_transcript`), so long threads stay cheap to save mid-turn.

mod search;

pub use search::{INDEX_MAX_BYTES, ImportedToIndex, SearchHit, fts_query};

use crate::import::ImportedThread;
use crate::transcript::Transcript;
use crate::types::{AgentId, Effort, HandHolding, RunState, ThreadSource};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

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

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    /// Held while an imported transcript is (re)indexed: that runs over many transactions, and
    /// two passes over the same thread mustn't interleave.
    indexer: Arc<Mutex<()>>,
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
"#;

/// Transcript rows. `pk` is the full-text index's rowid; `id` is the item's stable identity.
/// Trek passes uuid v7 ids; the default (a random v4 uuid) is for older Trek builds sharing the
/// database, which insert rows without one. SQLite only takes an expression default in CREATE
/// TABLE (not ADD COLUMN), hence the table rebuilds in `migrate_items`.
const ITEMS_TABLE: &str = "CREATE TABLE IF NOT EXISTS items (
  pk INTEGER PRIMARY KEY,
  id TEXT NOT NULL UNIQUE DEFAULT (lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' || substr(hex(randomblob(2)), 2) || '-'
    || substr('89ab', 1 + (random() & 3), 1) || substr(hex(randomblob(2)), 2) || '-' || hex(randomblob(6)))),
  thread_id TEXT NOT NULL, seq INTEGER NOT NULL, data TEXT NOT NULL, created_at INTEGER NOT NULL,
  UNIQUE (thread_id, seq)
)";

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

/// Migrations; each is a no-op once applied.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let _ = conn.execute("ALTER TABLE threads ADD COLUMN side_of TEXT", []);
    let _ = conn.execute("ALTER TABLE threads ADD COLUMN never_settle INTEGER NOT NULL DEFAULT 0", []);
    migrate_items(conn)?;
    search::ensure_schema(conn)
}

/// Give every transcript row a stable id. Early databases keyed rows by (thread, seq) alone and
/// rewrote whole transcripts on every save; their rows move into the new table as they are,
/// keeping their order and timestamps. Tables from before `id` had a default are rebuilt with
/// their ids and keys unchanged.
fn migrate_items(conn: &Connection) -> rusqlite::Result<()> {
    let exists = conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'items'")?.exists([])?;
    if !exists {
        return conn.execute_batch(ITEMS_TABLE);
    }
    let id_default: Option<Option<String>> = conn.query_row("SELECT dflt_value FROM pragma_table_info('items') WHERE name = 'id'", [], |r| r.get(0)).optional()?;
    if matches!(id_default, Some(Some(_))) {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("ALTER TABLE items RENAME TO items_old")?;
    tx.execute_batch(ITEMS_TABLE)?;
    if id_default.is_some() {
        tx.execute_batch("INSERT INTO items (pk, id, thread_id, seq, data, created_at) SELECT pk, id, thread_id, seq, data, created_at FROM items_old")?;
    } else {
        let mut read = tx.prepare("SELECT thread_id, seq, data, created_at FROM items_old ORDER BY thread_id, seq")?;
        let mut write = tx.prepare("INSERT INTO items (id, thread_id, seq, data, created_at) VALUES (?1, ?2, ?3, ?4, ?5)")?;
        let mut rows = read.query([])?;
        while let Some(r) = rows.next()? {
            let (thread, seq, data, at): (String, i64, String, i64) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            write.execute(params![crate::transcript::new_id(), thread, seq, data, at])?;
        }
    }
    // The search triggers moved to the old table with the rename and go with it: have
    // `search::ensure_schema` set the index up again.
    tx.execute_batch("DROP TABLE items_old")?;
    if tx.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'search_state'")?.exists([])? {
        tx.execute_batch("DELETE FROM search_state WHERE key = 'version'")?;
    }
    tx.commit()
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        Store::with_connection(Connection::open(path)?)
    }

    pub fn open_default() -> Result<Store> {
        Store::open(&crate::paths::database_file())
    }

    pub fn in_memory() -> Result<Store> {
        Store::with_connection(Connection::open_in_memory()?)
    }

    fn with_connection(conn: Connection) -> Result<Store> {
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Store { conn: Arc::new(Mutex::new(conn)), indexer: Arc::default() })
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
                Ok(Project::from_row(r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
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
            let rows = st.query_map([], |r| Ok(Project::from_row(r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            rows.collect()
        })
    }

    // ---- threads ----

    const THREAD_COLS: &'static str = "id, project_id, title, agent, model, effort, hand_holding, source, native_id, cwd, branch, created_at, updated_at, last_seen_at, settled_at, pinned_at, snoozed_until, archived_at, run_state, additions, deletions, side_of, never_settle";

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
        };
        self.save_thread(&t)?;
        Ok(t)
    }

    pub fn save_thread(&self, t: &Thread) -> Result<()> {
        // An upsert, not INSERT OR REPLACE: the row (and its rowid, which the title index uses)
        // stays put. Another thread holding the same agent session is replaced, as before.
        static SQL: LazyLock<String> = LazyLock::new(|| {
            let cols: Vec<&str> = Store::THREAD_COLS.split(", ").collect();
            let marks: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
            let set: Vec<String> = cols[1..].iter().map(|c| format!("{c} = excluded.{c}")).collect();
            format!("INSERT INTO threads ({}) VALUES ({}) ON CONFLICT(id) DO UPDATE SET {}", cols.join(", "), marks.join(", "), set.join(", "))
        });
        self.with(|c| {
            if let Some(native) = &t.native_id {
                c.execute("DELETE FROM threads WHERE source = ?1 AND native_id = ?2 AND id <> ?3", params![t.source.key(), native, t.id])?;
            }
            c.execute(
                &SQL,
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
                    t.never_settle as i64
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
            never_settle: false,
        };
        self.save_thread(&t)?;
        Ok(true)
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
        Ok(self.items_with_ids(thread_id)?.into_iter().map(|(_, item)| item).collect())
    }

    /// The transcript with each item's stable id, in order.
    pub fn items_with_ids(&self, thread_id: &str) -> Result<Vec<(String, Item)>> {
        self.with(|c| {
            let mut st = c.prepare_cached("SELECT id, data FROM items WHERE thread_id = ?1 ORDER BY seq")?;
            let rows = st.query_map([thread_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            Ok(rows.filter_map(|r| r.ok()).filter_map(|(id, data)| Some((id, serde_json::from_str(&data).ok()?))).collect())
        })
    }

    /// Add items after the last one, under the given ids.
    pub fn append_items<'a>(&self, thread_id: &str, items: impl IntoIterator<Item = (&'a str, &'a Item)>) -> Result<()> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        append_rows(&tx, thread_id, items)?;
        tx.commit()?;
        Ok(())
    }

    /// Rewrite one item. False if there's no row with that id.
    pub fn update_item(&self, id: &str, item: &Item) -> Result<bool> {
        self.with(|c| Ok(update_row(c, id, item)? > 0))
    }

    /// Delete items by id; returns how many rows went.
    pub fn delete_items(&self, ids: &[String]) -> Result<usize> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        let n = delete_rows(&tx, ids)?;
        tx.commit()?;
        Ok(n)
    }

    /// Keep the first `len` items of a thread; returns how many rows went.
    pub fn truncate_items(&self, thread_id: &str, len: usize) -> Result<usize> {
        self.with(|c| {
            let cut: Option<i64> = c
                .query_row("SELECT seq FROM items WHERE thread_id = ?1 ORDER BY seq LIMIT 1 OFFSET ?2", params![thread_id, len as i64], |r| r.get(0))
                .optional()?;
            match cut {
                Some(seq) => c.execute("DELETE FROM items WHERE thread_id = ?1 AND seq >= ?2", params![thread_id, seq]),
                None => Ok(0),
            }
        })
    }

    /// Delete everything after the item with this id (it stays); returns how many rows went.
    /// The basis for editing a message, retrying a turn or forking from a point.
    pub fn truncate_after(&self, thread_id: &str, id: &str) -> Result<usize> {
        let removed = self.with(|c| {
            let seq: Option<i64> = c.query_row("SELECT seq FROM items WHERE thread_id = ?1 AND id = ?2", params![thread_id, id], |r| r.get(0)).optional()?;
            seq.map(|seq| c.execute("DELETE FROM items WHERE thread_id = ?1 AND seq > ?2", params![thread_id, seq])).transpose()
        })?;
        removed.ok_or_else(|| anyhow::anyhow!("no item {id} in thread {thread_id}"))
    }

    /// Delete a thread's whole transcript.
    pub fn clear_items(&self, thread_id: &str) -> Result<usize> {
        self.with(|c| c.execute("DELETE FROM items WHERE thread_id = ?1", [thread_id]))
    }

    /// Write what changed in a live transcript since its last save, in one transaction, and mark
    /// it saved. Nothing is written when nothing changed. Returns true when the new rows were too
    /// much text to index on the spot (an imported transcript's first save): they're searchable
    /// once `backfill_search` has run.
    pub fn save_transcript(&self, thread_id: &str, transcript: &mut Transcript) -> Result<bool> {
        let changes = transcript.changes();
        if changes.is_empty() {
            return Ok(false);
        }
        let defer = search::defer_indexing(changes.appended.iter().map(|(_, item)| *item));
        {
            let mut conn = self.conn.lock().expect("store lock");
            let tx = conn.transaction()?;
            delete_rows(&tx, changes.removed)?;
            for (id, item) in &changes.changed {
                update_row(&tx, id, item)?;
            }
            if defer {
                search::append_unindexed(&tx, thread_id, changes.appended)?;
            } else {
                append_rows(&tx, thread_id, changes.appended)?;
            }
            tx.commit()?;
        }
        transcript.mark_saved();
        Ok(defer)
    }
}

/// Insert rows after a thread's last one. Returns the first and last new `pk`.
fn append_rows<'a>(c: &Connection, thread_id: &str, items: impl IntoIterator<Item = (&'a str, &'a Item)>) -> rusqlite::Result<Option<(i64, i64)>> {
    let mut seq: i64 = c.query_row("SELECT COALESCE(MAX(seq), -1) + 1 FROM items WHERE thread_id = ?1", [thread_id], |r| r.get(0))?;
    let mut st = c.prepare_cached("INSERT INTO items (id, thread_id, seq, data, created_at) VALUES (?1, ?2, ?3, ?4, ?5)")?;
    let now = now_ms();
    let mut pks = None;
    for (id, item) in items {
        st.execute(params![id, thread_id, seq, serde_json::to_string(item).unwrap_or_default(), now])?;
        let pk = c.last_insert_rowid();
        pks = Some((pks.map_or(pk, |(first, _)| first), pk));
        seq += 1;
    }
    Ok(pks)
}

fn update_row(c: &Connection, id: &str, item: &Item) -> rusqlite::Result<usize> {
    c.prepare_cached("UPDATE items SET data = ?2 WHERE id = ?1")?.execute(params![id, serde_json::to_string(item).unwrap_or_default()])
}

fn delete_rows(c: &Connection, ids: &[String]) -> rusqlite::Result<usize> {
    let mut st = c.prepare_cached("DELETE FROM items WHERE id = ?1")?;
    let mut n = 0;
    for id in ids {
        n += st.execute([id])?;
    }
    Ok(n)
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
        let hi = Item::User { text: "hi".into(), images: vec![], at: None };
        let hello = Item::Assistant { text: "hello".into() };
        s.append_items(&t.id, [("a", &hi), ("b", &hello)]).unwrap();
        assert_eq!(s.items(&t.id).unwrap(), vec![hi, hello]);
    }

    fn said(t: &str) -> Item {
        Item::Assistant { text: t.into() }
    }

    fn texts(s: &Store, thread: &str) -> Vec<String> {
        s.items(thread).unwrap().into_iter().map(|i| if let Item::Assistant { text } = i { text } else { String::new() }).collect()
    }

    #[test]
    fn items_append_update_delete_and_truncate_by_id() {
        let s = Store::in_memory().unwrap();
        let t = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
        let other = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
        s.append_items(&t.id, [("1", &said("one")), ("2", &said("two"))]).unwrap();
        s.append_items(&other.id, [("x", &said("elsewhere"))]).unwrap();
        s.append_items(&t.id, [("3", &said("three")), ("4", &said("four"))]).unwrap();
        assert_eq!(texts(&s, &t.id), ["one", "two", "three", "four"]);

        assert!(s.update_item("2", &said("TWO")).unwrap());
        assert!(!s.update_item("missing", &said("?")).unwrap());
        assert_eq!(s.delete_items(&["3".into(), "missing".into()]).unwrap(), 1);
        assert_eq!(texts(&s, &t.id), ["one", "TWO", "four"]);
        let ids: Vec<String> = s.items_with_ids(&t.id).unwrap().into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["1", "2", "4"]);

        // Appending after a delete continues the order.
        s.append_items(&t.id, [("5", &said("five"))]).unwrap();
        assert_eq!(s.truncate_after(&t.id, "2").unwrap(), 2);
        assert_eq!(texts(&s, &t.id), ["one", "TWO"]);
        assert!(s.truncate_after(&t.id, "x").is_err());
        assert_eq!(s.truncate_items(&t.id, 1).unwrap(), 1);
        assert_eq!(s.truncate_items(&t.id, 5).unwrap(), 0);
        assert_eq!(texts(&s, &t.id), ["one"]);
        assert_eq!(texts(&s, &other.id), ["elsewhere"]);
        assert_eq!(s.clear_items(&t.id).unwrap(), 1);
        assert!(s.items(&t.id).unwrap().is_empty());
    }

    #[test]
    fn saving_a_transcript_writes_only_what_changed() {
        let s = Store::in_memory().unwrap();
        let t = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
        let mut tr = Transcript::default();
        tr.push(Item::User { text: "go".into(), images: vec![], at: Some(1) });
        tr.push(Item::Reasoning { text: String::new() });
        let answer = tr.push(said("work"));
        s.save_transcript(&t.id, &mut tr).unwrap();
        assert!(!tr.is_dirty());
        let created: Vec<i64> = s.with(|c| c.prepare("SELECT created_at FROM items ORDER BY seq")?.query_map([], |r| r.get(0))?.collect()).unwrap();

        // Mid-turn: the streaming answer grows, a tool starts; the empty thought goes at turn end.
        if let Some(Item::Assistant { text }) = tr.get_mut(answer) {
            text.push_str("ing on it");
        }
        tr.push(Item::Tool { id: "t1".into(), title: "Ran command".into(), detail: "ls".into(), output: String::new(), status: ToolStatus::Running });
        tr.retain(|i| !matches!(i, Item::Reasoning { text } if text.is_empty()));
        if let Some(Item::Tool { status, .. }) = tr.rfind_mut(|i| matches!(i, Item::Tool { id, .. } if id == "t1")) {
            *status = ToolStatus::Done;
        }
        let c = tr.changes();
        assert_eq!((c.removed.len(), c.changed.len(), c.appended.len()), (1, 1, 1));
        s.save_transcript(&t.id, &mut tr).unwrap();

        let back = s.items_with_ids(&t.id).unwrap();
        assert_eq!(back.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(), tr.ids());
        assert_eq!(back.into_iter().map(|(_, i)| i).collect::<Vec<_>>(), tr.to_vec());
        // Rows that were there keep their created_at.
        let first: i64 = s.with(|c| c.query_row("SELECT created_at FROM items WHERE id = ?1", [&tr.ids()[0]], |r| r.get(0))).unwrap();
        assert_eq!(first, created[0]);
        // Saving with nothing changed writes nothing.
        let writes = |s: &Store| s.with(|c| Ok(c.total_changes())).unwrap();
        let before = writes(&s);
        s.save_transcript(&t.id, &mut tr).unwrap();
        assert_eq!(writes(&s), before);
    }

    #[test]
    fn old_transcripts_get_stable_ids_on_open() {
        let dir = std::env::temp_dir().join(format!("trek-migrate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trek.sqlite");
        {
            // The schema before stable ids: rows keyed by (thread, seq), rewritten on every save.
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE items (thread_id TEXT NOT NULL, seq INTEGER NOT NULL, data TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY (thread_id, seq));",
            )
            .unwrap();
            for (thread, seq, text) in [("a", 1, "a-second"), ("a", 0, "a-first"), ("b", 0, "b-only")] {
                let data = serde_json::to_string(&said(text)).unwrap();
                c.execute("INSERT INTO items VALUES (?1, ?2, ?3, ?4)", params![thread, seq, data, 42 + seq]).unwrap();
            }
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(texts(&s, "a"), ["a-first", "a-second"]);
        assert_eq!(texts(&s, "b"), ["b-only"]);
        let rows = s.items_with_ids("a").unwrap();
        assert!(rows.iter().all(|(id, _)| uuid::Uuid::parse_str(id).is_ok()));
        assert_ne!(rows[0].0, rows[1].0);
        let created: Vec<i64> = s.with(|c| c.prepare("SELECT created_at FROM items WHERE thread_id = 'a' ORDER BY seq")?.query_map([], |r| r.get(0))?.collect()).unwrap();
        assert_eq!(created, [42, 43]);
        // Ids survive reopening, and old rows become searchable once backfilled.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.items_with_ids("a").unwrap(), rows);
        while s.backfill_search(100).unwrap() {}
        let t = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
        s.with(|c| c.execute("UPDATE items SET thread_id = ?1 WHERE thread_id = 'a'", [&t.id])).unwrap();
        assert_eq!(s.search("second", 5).unwrap()[0].position, Some(1));
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ids_without_a_default_get_one_and_keep_their_rows() {
        let dir = std::env::temp_dir().join(format!("trek-migrate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trek.sqlite");
        let (thread, rows) = {
            let s = Store::open(&path).unwrap();
            let t = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
            let mut tr = Transcript::default();
            tr.push(Item::User { text: "harbour lights".into(), images: vec![], at: None });
            tr.push(said("the harbour is dark"));
            s.save_transcript(&t.id, &mut tr).unwrap();
            // Put the table back the way the first build with ids made it: no default for `id`.
            s.with(|c| {
                c.execute_batch(
                    "PRAGMA writable_schema = ON;
                     UPDATE sqlite_master SET sql = 'CREATE TABLE items (pk INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, thread_id TEXT NOT NULL, seq INTEGER NOT NULL, data TEXT NOT NULL, created_at INTEGER NOT NULL, UNIQUE (thread_id, seq))' WHERE type = 'table' AND name = 'items';
                     PRAGMA writable_schema = OFF;",
                )
            })
            .unwrap();
            let pks: Vec<(i64, String)> = s.with(|c| c.prepare("SELECT pk, id FROM items ORDER BY seq")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()).unwrap();
            (t.id, pks)
        };
        {
            let c = Connection::open(&path).unwrap();
            let insert = c.execute("INSERT INTO items (thread_id, seq, data, created_at) VALUES ('x', 0, '{}', 1)", []);
            assert!(insert.is_err(), "the reconstructed table has no default");
        }
        let s = Store::open(&path).unwrap();
        let pks: Vec<(i64, String)> = s.with(|c| c.prepare("SELECT pk, id FROM items ORDER BY seq")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()).unwrap();
        assert_eq!(pks, rows);
        // The index is set up again (triggers included) and refilled in the background.
        while s.backfill_search(100).unwrap() {}
        assert_eq!(s.search("harbour", 5).unwrap()[0].item_id.as_deref(), Some(rows[0].1.as_str()));
        let data = serde_json::to_string(&said("lighthouse keeper")).unwrap();
        s.with(|c| c.execute("INSERT INTO items (thread_id, seq, data, created_at) VALUES (?1, 2, ?2, 1)", params![thread, data])).unwrap();
        assert_eq!(s.search("lighthouse", 5).unwrap()[0].position, Some(2));
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_a_thread_keeps_its_row() {
        let s = Store::in_memory().unwrap();
        let t = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
        let rowid = |s: &Store| -> i64 { s.with(|c| c.query_row("SELECT rowid FROM threads WHERE id = ?1", [&t.id], |r| r.get(0))).unwrap() };
        let before = rowid(&s);
        s.update_thread(&t.id, |t| t.title = "Renamed".into()).unwrap();
        assert_eq!(rowid(&s), before);
        // Another thread claiming the same agent session replaces it, as INSERT OR REPLACE did.
        s.update_thread(&t.id, |t| t.native_id = Some("sess".into())).unwrap();
        let mut b = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
        b.native_id = Some("sess".into());
        s.save_thread(&b).unwrap();
        assert!(s.thread(&t.id).unwrap().is_none());
        assert_eq!(s.threads().unwrap().len(), 1);
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
