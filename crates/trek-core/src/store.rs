//! SQLite persistence for projects, threads and transcript items.
//!
//! Transcripts are append-only: every item is a row with a stable id (uuid v7) and a `seq` that
//! orders it within its thread. Saving a live transcript writes only the rows that are new,
//! changed or gone (`save_transcript`), so long threads stay cheap to save mid-turn.

mod search;
mod usage;

pub use search::{INDEX_MAX_BYTES, ImportedToIndex, SearchHit, fts_query};
pub use usage::{Activity, UsageRow};

use crate::import::ImportedThread;
use crate::transcript::Transcript;
use crate::types::{AgentId, Effort, HandHolding, RunState, ThreadSource};
use crate::worktree::Worktree;
use anyhow::Result;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
    /// The title the last import gave it; a different current title is one the user typed.
    pub imported_title: Option<String>,
    /// Why an import archived it: the `import::Skip` rule it matched (by key), or
    /// `import::DELETED`. Brought back if a later import finds it as a conversation again.
    pub import_hidden: Option<String>,
    /// The user chose to keep it although a rule matches it ("Show in sidebar" in Settings).
    pub import_kept: bool,
    /// The thread runs in a worktree of its own (then `cwd` is the worktree's folder).
    pub worktree: Option<Worktree>,
    /// The latest point in the agent's session (`native_id`) that a later message can be traced
    /// back to (see `ResumePoint::after`).
    pub native_at: Option<String>,
    /// How the next session picks the conversation up after a rewind or a fork; cleared once
    /// that session has started.
    pub reopen: Option<crate::rewind::Reopen>,
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
    /// The conversation goes on in a session Trek started: the thread is Trek's own from now on.
    /// The branch an import recorded is only where the agent once ran; Trek ties a thread to a
    /// branch once it commits there (and settles it when that's merged), so it goes.
    pub fn become_trek_thread(&mut self) {
        if self.source != ThreadSource::Trek {
            self.source = ThreadSource::Trek;
            self.branch = None;
        }
    }

    pub fn is_unseen(&self) -> bool {
        self.updated_at > self.last_seen_at
    }

    /// Waiting on the user: a question or approval, or a failure they haven't settled (settling
    /// it is acknowledging it; it stays Failed, so moving it back to the inbox brings it back).
    pub fn needs_you(&self) -> bool {
        self.run_state == RunState::NeedsYou || (self.run_state == RunState::Failed && self.settled_at.is_none())
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

    /// Whether auto-settle applies now: the wait runs from the thread's last activity, the last
    /// time the user looked at it (or moved it back to the inbox), or the end of its snooze. A
    /// snoozed thread is left alone until it wakes, then gets the full wait again.
    pub fn should_auto_settle(&self, now: i64, after_days: u32) -> bool {
        let last_seen_in_inbox = self.updated_at.max(self.last_seen_at).max(self.snoozed_until.unwrap_or(0));
        after_days > 0
            && !self.never_settle
            && self.settled_at.is_none()
            && self.pinned_at.is_none()
            && self.run_state == RunState::Idle
            && !self.is_unseen()
            && now - last_seen_in_inbox > after_days as i64 * 86_400_000
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
        /// When it was sent (unix ms); absent when the source didn't record it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<i64>,
        /// Where the agent's own session stood just before it, so a rewind can take the agent
        /// back there; absent when that isn't known (the message started a session, say).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume: Option<ResumePoint>,
        /// Not a prompt: a command Trek answered itself (`/model`), or a typed answer to the
        /// agent's question. It neither starts a turn nor is somewhere to rewind to.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        aside: bool,
    },
    Assistant { text: String },
    /// Marks the end of a response: when it finished and how long the turn took.
    TurnEnd { at: i64, took_secs: u32 },
    Reasoning { text: String },
    Tool { id: String, title: String, detail: String, output: String, status: ToolStatus },
    Notice { text: String },
    Error { text: String },
}

/// A point in an agent's own session: resuming `session` there brings back the conversation up to
/// it and nothing after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumePoint {
    /// The agent's session id (a thread's `native_id`).
    pub session: String,
    /// The agent's id for the last thing in the session before the point: a message (Claude
    /// Code) or a turn (Codex). Absent when the session had nothing before it yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// The notice ending a turn that Trek quit (or crashed) in the middle of.
pub const INTERRUPTED_BY_QUIT: &str = "Interrupted: Trek closed before this turn finished";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Done,
    Failed,
    Denied,
}

/// A file checkpoint: the working tree of `repo` as message `item_id` was sent (see `checkpoint`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub item_id: String,
    pub repo: PathBuf,
    pub sha: String,
    pub created_at: i64,
}

impl Checkpoint {
    fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Checkpoint> {
        Ok(Checkpoint { item_id: r.get(0)?, repo: PathBuf::from(r.get::<_, String>(1)?), sha: r.get(2)?, created_at: r.get(3)? })
    }
}

/// What an import did to one session's thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Upserted {
    pub added: bool,
    pub retitled: bool,
    pub hidden: bool,
    pub restored: bool,
    /// A rule keeps it out of the sidebar: never added, or archived by an import.
    pub left_out: bool,
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    /// Held while an imported transcript is (re)indexed: that runs over many transactions, and
    /// two passes over the same thread mustn't interleave.
    indexer: Arc<Mutex<()>>,
    /// A second, read-only connection for searches. WAL lets it read while `conn` writes, so a
    /// search ranking a big history never holds up a save on the main thread. `None` in memory
    /// (a second connection would open another, empty, database): searches share `conn`.
    reader: Option<Arc<Mutex<Connection>>>,
}

const SCHEMA: &str = r#"
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

/// Project root for a working directory: the enclosing git repo (the main checkout, for one of
/// Trek's own worktrees), else the folder itself.
pub fn project_root(cwd: &Path) -> PathBuf {
    project_root_in(cwd, &crate::worktree::worktrees_dir())
}

/// [`project_root`], with Trek's worktrees under `trek_worktrees`. Only those belong to their main
/// checkout: a worktree the user made themselves (by hand, or with another tool) is a project of
/// its own, as it's opened.
fn project_root_in(cwd: &Path, trek_worktrees: &Path) -> PathBuf {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if d.join(".git").exists() {
            let main = d.starts_with(trek_worktrees).then(|| crate::worktree::main_checkout(d)).flatten();
            return main.unwrap_or_else(|| d.to_path_buf());
        }
        dir = d.parent();
    }
    cwd.to_path_buf()
}

/// Columns added to `threads` since the first release, in order.
const THREAD_COLUMNS_ADDED: &[(&str, &str)] = &[
    ("side_of", "TEXT"),
    ("never_settle", "INTEGER NOT NULL DEFAULT 0"),
    ("imported_title", "TEXT"),
    ("import_hidden", "TEXT"),
    ("import_kept", "INTEGER NOT NULL DEFAULT 0"),
    ("worktree_path", "TEXT"),
    ("worktree_branch", "TEXT"),
    ("worktree_base", "TEXT"),
    ("native_at", "TEXT"),
    ("reopen", "TEXT"),
];

/// Migrations; each is a no-op once applied. Run inside one transaction (`with_connection`), so
/// a failure leaves the database as it was rather than half migrated.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let have: HashSet<String> = conn.prepare("SELECT name FROM pragma_table_info('threads')")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    for (name, decl) in THREAD_COLUMNS_ADDED {
        if !have.contains(*name) {
            conn.execute(&format!("ALTER TABLE threads ADD COLUMN {name} {decl}"), [])?;
        }
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS checkpoints (
           thread_id TEXT NOT NULL, item_id TEXT NOT NULL, repo TEXT NOT NULL, sha TEXT NOT NULL, created_at INTEGER NOT NULL,
           PRIMARY KEY (thread_id, item_id)
         );
         CREATE TABLE IF NOT EXISTS retired_sessions (native_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL);",
    )?;
    conn.execute_batch(usage::SCHEMA)?;
    migrate_items(conn)?;
    search::ensure_schema(conn)
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
    conn.execute_batch("ALTER TABLE items RENAME TO items_old")?;
    conn.execute_batch(ITEMS_TABLE)?;
    if id_default.is_some() {
        conn.execute_batch("INSERT INTO items (pk, id, thread_id, seq, data, created_at) SELECT pk, id, thread_id, seq, data, created_at FROM items_old")?;
    } else {
        let mut read = conn.prepare("SELECT thread_id, seq, data, created_at FROM items_old ORDER BY thread_id, seq")?;
        let mut write = conn.prepare("INSERT INTO items (id, thread_id, seq, data, created_at) VALUES (?1, ?2, ?3, ?4, ?5)")?;
        let mut rows = read.query([])?;
        while let Some(r) = rows.next()? {
            let (thread, seq, data, at): (String, i64, String, i64) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            write.execute(params![crate::transcript::new_id(), thread, seq, data, at])?;
        }
    }
    // The search triggers moved to the old table with the rename and go with it: have
    // `search::ensure_schema` set the index up again.
    conn.execute_batch("DROP TABLE items_old")?;
    if conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'search_state'")?.exists([])? {
        conn.execute_batch("DELETE FROM search_state WHERE key = 'version'")?;
    }
    Ok(())
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let mut store = Store::with_connection(Connection::open(path)?)?;
        let reader = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI)?;
        store.reader = Some(Arc::new(Mutex::new(reader)));
        Ok(store)
    }

    pub fn open_default() -> Result<Store> {
        Store::open(&crate::paths::database_file())
    }

    pub fn in_memory() -> Result<Store> {
        Store::with_connection(Connection::open_in_memory()?)
    }

    fn with_connection(mut conn: Connection) -> Result<Store> {
        // Not in a transaction: SQLite can't change the journal mode inside one. Only when it
        // needs changing, as that waits for other connections' locks.
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            conn.execute_batch("PRAGMA journal_mode = WAL")?;
        }
        // IMMEDIATE takes the write lock up front: with another Trek migrating the same database,
        // this waits for it (or fails whole) instead of applying half the steps.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(SCHEMA)?;
        migrate(&tx)?;
        tx.commit()?;
        Ok(Store { conn: Arc::new(Mutex::new(conn)), indexer: Arc::default(), reader: None })
    }

    fn with<R>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<R>) -> Result<R> {
        let conn = self.conn.lock().expect("store lock");
        Ok(f(&conn)?)
    }

    /// `with`, for reads that may take a while: on the read-only connection where there is one.
    fn reading<R>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<R>) -> Result<R> {
        let Some(reader) = &self.reader else { return self.with(f) };
        let conn = reader.lock().expect("store reader lock");
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

    const THREAD_COLS: &'static str = "id, project_id, title, agent, model, effort, hand_holding, source, native_id, cwd, branch, created_at, updated_at, last_seen_at, settled_at, pinned_at, snoozed_until, archived_at, run_state, additions, deletions, side_of, never_settle, imported_title, import_hidden, import_kept, worktree_path, worktree_branch, worktree_base, native_at, reopen";

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
            import_hidden: r.get(24)?,
            import_kept: r.get::<_, i64>(25)? != 0,
            worktree: match (r.get::<_, Option<String>>(26)?, r.get::<_, Option<String>>(27)?, r.get::<_, Option<String>>(28)?) {
                (Some(path), Some(branch), Some(base)) => Some(Worktree { path: PathBuf::from(path), branch, base }),
                _ => None,
            },
            native_at: r.get(29)?,
            reopen: r.get::<_, Option<String>>(30)?.and_then(|j| serde_json::from_str(&j).ok()),
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
            import_hidden: None,
            import_kept: false,
            worktree: None,
            native_at: None,
            reopen: None,
        };
        self.save_thread(&t)?;
        Ok(t)
    }

    pub fn save_thread(&self, t: &Thread) -> Result<()> {
        self.with(|c| Self::save_thread_in(c, t))
    }

    fn save_thread_in(c: &Connection, t: &Thread) -> rusqlite::Result<()> {
        // An upsert, not INSERT OR REPLACE: the row stays put instead of being deleted and inserted
        // again on every save. Another thread holding the same agent session is replaced, as before.
        static SQL: LazyLock<String> = LazyLock::new(|| {
            let cols: Vec<&str> = Store::THREAD_COLS.split(", ").collect();
            let marks: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
            let set: Vec<String> = cols[1..].iter().map(|c| format!("{c} = excluded.{c}")).collect();
            format!("INSERT INTO threads ({}) VALUES ({}) ON CONFLICT(id) DO UPDATE SET {}", cols.join(", "), marks.join(", "), set.join(", "))
        });
        if let Some(native) = &t.native_id {
            let replaced = "SELECT id FROM threads WHERE source = ?1 AND native_id = ?2 AND id <> ?3";
            let args = params![t.source.key(), native, t.id];
            c.execute(&format!("DELETE FROM items WHERE thread_id IN ({replaced})"), args)?;
            c.execute(&format!("DELETE FROM checkpoints WHERE thread_id IN ({replaced})"), args)?;
            c.execute(&format!("DELETE FROM token_usage WHERE thread_id IN ({replaced})"), args)?;
            c.execute(&format!("DELETE FROM turn_stops WHERE thread_id IN ({replaced})"), args)?;
            c.execute("DELETE FROM threads WHERE source = ?1 AND native_id = ?2 AND id <> ?3", args)?;
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
                t.never_settle as i64,
                t.imported_title,
                t.import_hidden,
                t.import_kept as i64,
                t.worktree.as_ref().map(|w| w.path.display().to_string()),
                t.worktree.as_ref().map(|w| w.branch.clone()),
                t.worktree.as_ref().map(|w| w.base.clone()),
                t.native_at,
                t.reopen.as_ref().and_then(|r| serde_json::to_string(r).ok())
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

    /// Close the turns an earlier run left open (it quit or crashed mid-turn, or while a card was
    /// waiting): no session is behind them any more. Each thread goes back to idle; in its stored
    /// transcript, tool calls still running are marked failed, empty thoughts go, and a notice
    /// says the turn was cut short. Returns the threads closed. For launch, before any session
    /// starts.
    pub fn close_interrupted_turns(&self) -> Result<Vec<String>> {
        let open: Vec<String> = self.with(|c| {
            let mut st = c.prepare("SELECT id FROM threads WHERE run_state IN (?1, ?2)")?;
            let rows = st.query_map(params![enum_str(&RunState::Working), enum_str(&RunState::NeedsYou)], |r| r.get(0))?;
            rows.collect()
        })?;
        for id in &open {
            let rows = self.items_with_ids(id)?;
            // With nothing stored, the history is still the agent's own (an imported thread):
            // a lone notice would hide it.
            if !rows.is_empty() {
                let mut t = Transcript::stored(rows);
                t.retain(|i| !matches!(i, Item::Reasoning { text } if text.trim().is_empty()));
                for ix in 0..t.len() {
                    if matches!(t[ix], Item::Tool { status: ToolStatus::Running, .. }) {
                        if let Some(Item::Tool { status, .. }) = t.get_mut(ix) {
                            *status = ToolStatus::Failed;
                        }
                    }
                }
                t.push(Item::Notice { text: INTERRUPTED_BY_QUIT.into() });
                self.save_transcript(id, &mut t)?;
            }
            self.update_thread(id, |t| t.run_state = RunState::Idle)?;
        }
        Ok(open)
    }

    /// Agent session ids of Trek's own threads, archived ones included, and the sessions threads
    /// left behind when a rewind or a fork moved them to another (`retire_session`).
    pub fn trek_native_ids(&self) -> Result<HashSet<String>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT native_id FROM threads WHERE source = 'trek' AND native_id IS NOT NULL UNION SELECT native_id FROM retired_sessions",
            )?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect()
        })
    }

    /// `thread` moved on from the agent session `native_id` (a rewind continues in a copy of it,
    /// say). The session stays in the agent's history; imports leave it out, as they do the
    /// sessions threads use.
    pub fn retire_session(&self, native_id: &str, thread_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute("INSERT OR REPLACE INTO retired_sessions (native_id, thread_id) VALUES (?1, ?2)", params![native_id, thread_id])?;
            Ok(())
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
            out.left_out = imp.skip.is_some();
            return Ok(out);
        };
        let before = t.clone();
        match imp.skip {
            // Threads the user kept (pinned, continued in Trek, or shown anyway) stay where they are.
            Some(rule) if t.archived_at.is_none() && t.pinned_at.is_none() && !t.import_kept && !Self::has_items_in(c, &t.id)? => {
                t.archived_at = Some(now_ms());
                t.import_hidden = Some(rule.key().to_string());
                out.hidden = true;
            }
            // Archived earlier for another reason (its session was missing): record this one.
            Some(rule) if t.import_hidden.is_some() => t.import_hidden = Some(rule.key().to_string()),
            None if t.import_hidden.is_some() => {
                t.archived_at = None;
                t.import_hidden = None;
                out.restored = true;
            }
            _ => {}
        }
        out.left_out = imp.skip.is_some() && t.archived_at.is_some() && t.import_hidden.is_some();
        // Automatic titles follow the source; a title the user typed in Trek stays. With no
        // record of what an earlier import called it, the current title is kept too.
        let previous = t.imported_title.as_deref().or(imp.legacy_title.as_deref());
        if previous.is_some_and(|p| p == t.title) && t.title != imp.title {
            t.title = imp.title.clone();
            out.retitled = true;
        }
        t.imported_title = Some(imp.title.clone());
        // Earlier imports kept OpenCode's model as its raw JSON, which no agent understands.
        if t.model.as_deref().is_some_and(|m| m.starts_with('{')) {
            t.model = imp.model.clone();
        }
        if t.updated_at < imp.updated_at {
            // Activity that happened outside Trek isn't "unread" here.
            if t.last_seen_at >= t.updated_at {
                t.last_seen_at = imp.updated_at;
            }
            t.updated_at = imp.updated_at;
        } else if t.updated_at > imp.updated_at && !Self::has_items_in(c, &t.id)? {
            // Until it's continued here, only imports set its time, and earlier ones went by
            // the session file's modified time, which the agent bumps long after the last message.
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
            import_hidden: None,
            import_kept: false,
            worktree: None,
            native_at: None,
            reopen: None,
        })
    }

    /// Native ids of `source`'s imported threads the sidebar shows or an import archived: the
    /// ones every import revisits, however old.
    pub fn imported_native_ids(&self, source: ThreadSource) -> Result<HashSet<String>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT native_id FROM threads WHERE source = ?1 AND native_id IS NOT NULL AND (archived_at IS NULL OR import_hidden IS NOT NULL)",
            )?;
            let rows = st.query_map([source.key()], |r| r.get::<_, String>(0))?;
            rows.collect()
        })
    }

    /// Show a session the import left out after all: its thread is added, or brought back if an
    /// import archived it, and later imports leave it in the sidebar.
    pub fn keep_imported(&self, imp: &ImportedThread) -> Result<Thread> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        let existing = tx
            .query_row(
                &format!("SELECT {} FROM threads WHERE source = ?1 AND native_id = ?2", Self::THREAD_COLS),
                params![imp.source.key(), imp.native_id],
                Self::row_to_thread,
            )
            .optional()?;
        let mut t = match existing {
            Some(t) => t,
            None => Self::new_imported(&tx, imp)?,
        };
        if t.import_hidden.take().is_some() {
            t.archived_at = None;
        }
        t.import_kept = true;
        Self::save_thread_in(&tx, &t)?;
        tx.commit()?;
        Ok(t)
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
            hidden += tx.execute("UPDATE threads SET archived_at = ?2, import_hidden = ?3 WHERE id = ?1", params![id, now, crate::import::DELETED])?;
        }
        tx.commit()?;
        Ok(hidden)
    }

    fn has_items_in(c: &Connection, thread_id: &str) -> rusqlite::Result<bool> {
        c.query_row("SELECT EXISTS (SELECT 1 FROM items WHERE thread_id = ?1)", [thread_id], |r| r.get(0))
    }

    /// Remove a thread, its side chats and their transcripts from Trek's database. The agent
    /// sessions they ran stay in the agents' own history; they're retired, so no import brings
    /// the conversation back.
    pub fn delete_thread(&self, id: &str) -> Result<()> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO retired_sessions (native_id, thread_id)
             SELECT native_id, id FROM threads WHERE (id = ?1 OR side_of = ?1) AND source = 'trek' AND native_id IS NOT NULL",
            [id],
        )?;
        let gone = "SELECT id FROM threads WHERE id = ?1 OR side_of = ?1";
        tx.execute(&format!("DELETE FROM items WHERE thread_id IN ({gone})"), [id])?;
        tx.execute(&format!("DELETE FROM checkpoints WHERE thread_id IN ({gone})"), [id])?;
        tx.execute(&format!("DELETE FROM token_usage WHERE thread_id IN ({gone})"), [id])?;
        tx.execute(&format!("DELETE FROM turn_stops WHERE thread_id IN ({gone})"), [id])?;
        tx.execute("DELETE FROM threads WHERE id = ?1 OR side_of = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    /// Side chats left without their thread: ones started from a draft (`side_of = "draft"`) in
    /// an earlier run, which nothing can reopen, and any whose thread is gone. Removed like
    /// `delete_thread` removes them; returns them, for their file checkpoints. For launch, before
    /// any side chat opens.
    pub fn drop_orphan_side_chats(&self) -> Result<Vec<(Thread, Vec<Checkpoint>)>> {
        let orphans: Vec<Thread> = self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM threads WHERE side_of IS NOT NULL AND side_of NOT IN (SELECT id FROM threads)",
                Self::THREAD_COLS
            ))?;
            let rows = st.query_map([], Self::row_to_thread)?;
            rows.collect()
        })?;
        let mut out = Vec::with_capacity(orphans.len());
        for t in orphans {
            let checkpoints = self.checkpoints(&t.id)?;
            self.delete_thread(&t.id)?;
            out.push((t, checkpoints));
        }
        Ok(out)
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

    // ---- checkpoints ----

    /// Record the file checkpoint taken as message `item_id` of `thread_id` was sent.
    pub fn add_checkpoint(&self, thread_id: &str, item_id: &str, repo: &Path, sha: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO checkpoints (thread_id, item_id, repo, sha, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![thread_id, item_id, repo.display().to_string(), sha, now_ms()],
            )?;
            Ok(())
        })
    }

    /// The checkpoint taken as message `item_id` was sent, if there is one.
    pub fn checkpoint(&self, thread_id: &str, item_id: &str) -> Result<Option<Checkpoint>> {
        self.with(|c| {
            c.query_row(
                "SELECT item_id, repo, sha, created_at FROM checkpoints WHERE thread_id = ?1 AND item_id = ?2",
                params![thread_id, item_id],
                Checkpoint::from_row,
            )
            .optional()
        })
    }

    /// A thread's checkpoints, oldest first.
    pub fn checkpoints(&self, thread_id: &str) -> Result<Vec<Checkpoint>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT item_id, repo, sha, created_at FROM checkpoints WHERE thread_id = ?1 ORDER BY created_at, item_id")?;
            let rows = st.query_map([thread_id], Checkpoint::from_row)?;
            rows.collect()
        })
    }

    /// Checkpoints of threads archived or settled before `before` (pinned ones aside), and of
    /// threads that are gone, by thread.
    pub fn stale_checkpoints(&self, before: i64) -> Result<Vec<(String, Vec<Checkpoint>)>> {
        let threads: Vec<String> = self.with(|c| {
            let mut st = c.prepare(
                "SELECT DISTINCT k.thread_id FROM checkpoints k LEFT JOIN threads t ON t.id = k.thread_id
                 WHERE t.id IS NULL OR t.archived_at < ?1 OR (t.settled_at < ?1 AND t.pinned_at IS NULL AND t.archived_at IS NULL)",
            )?;
            let rows = st.query_map([before], |r| r.get(0))?;
            rows.collect()
        })?;
        threads.into_iter().map(|t| Ok((t.clone(), self.checkpoints(&t)?))).collect()
    }

    /// Forget checkpoints by message.
    pub fn delete_checkpoints(&self, thread_id: &str, item_ids: &[String]) -> Result<usize> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        let mut n = 0;
        for id in item_ids {
            n += tx.execute("DELETE FROM checkpoints WHERE thread_id = ?1 AND item_id = ?2", params![thread_id, id])?;
        }
        tx.commit()?;
        Ok(n)
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
        let hi = Item::User { text: "hi".into(), images: vec![], at: None, resume: None, aside: false };
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
        tr.push(Item::User { text: "go".into(), images: vec![], at: Some(1), resume: None, aside: false });
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
            tr.push(Item::User { text: "harbour lights".into(), images: vec![], at: None, resume: None, aside: false });
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
    fn checkpoints_resume_points_and_retired_sessions_are_kept() {
        let s = Store::in_memory().unwrap();
        let mut t = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
        t.native_id = Some("s2".into());
        t.native_at = Some("m9".into());
        t.reopen = Some(crate::rewind::Reopen::Native { session: "s1".into(), at: Some("m3".into()), fork: true });
        s.save_thread(&t).unwrap();
        assert_eq!(s.thread(&t.id).unwrap().unwrap(), t);

        // A message's resume point is stored with it.
        let sent = Item::User { text: "go".into(), images: vec![], at: Some(1), resume: Some(ResumePoint { session: "s2".into(), after: Some("m9".into()) }), aside: false };
        s.append_items(&t.id, [("u1", &sent)]).unwrap();
        assert_eq!(s.items(&t.id).unwrap(), [sent]);

        let repo = Path::new("/tmp/repo");
        s.add_checkpoint(&t.id, "u1", repo, "aaa").unwrap();
        s.add_checkpoint(&t.id, "u2", repo, "bbb").unwrap();
        assert_eq!(s.checkpoint(&t.id, "u2").unwrap().map(|c| (c.repo, c.sha)), Some((repo.to_path_buf(), "bbb".to_string())));
        assert_eq!(s.checkpoints(&t.id).unwrap().iter().map(|c| c.item_id.as_str()).collect::<Vec<_>>(), ["u1", "u2"]);
        assert_eq!(s.delete_checkpoints(&t.id, &["u1".into(), "nope".into()]).unwrap(), 1);
        assert!(s.checkpoint(&t.id, "u1").unwrap().is_none());

        // Sessions a thread moved on from count as Trek's, so imports leave them out.
        s.retire_session("s1", &t.id).unwrap();
        assert_eq!(s.trek_native_ids().unwrap(), HashSet::from(["s1".to_string(), "s2".to_string()]));
        s.delete_thread(&t.id).unwrap();
        assert!(s.checkpoints(&t.id).unwrap().is_empty());
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
    fn worktrees_are_kept_on_threads_and_older_databases_gain_them() {
        let dir = std::env::temp_dir().join(format!("trek-migrate-wt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trek.sqlite");
        {
            // A database from before worktrees, with a thread in it.
            let c = Connection::open(&path).unwrap();
            c.execute_batch(SCHEMA).unwrap();
            c.execute(
                "INSERT INTO threads (id, title, agent, effort, hand_holding, source, created_at, updated_at, last_seen_at) VALUES ('old', 'Old', 'claude-code', 'high', 'auto', 'trek', 1, 1, 1)",
                [],
            )
            .unwrap();
        }
        let s = Store::open(&path).unwrap();
        let old = s.thread("old").unwrap().unwrap();
        assert_eq!((old.title.as_str(), old.worktree.as_ref()), ("Old", None));
        let wt = Worktree { path: dir.join("worktrees/repo/fix-it"), branch: "trek/fix-it".into(), base: "main".into() };
        s.update_thread("old", |t| {
            t.cwd = Some(wt.path.clone());
            t.worktree = Some(wt.clone());
        })
        .unwrap();
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.thread("old").unwrap().unwrap().worktree, Some(wt));
        // Leaving the worktree clears all three columns.
        s.update_thread("old", |t| t.worktree = None).unwrap();
        assert_eq!(s.thread("old").unwrap().unwrap().worktree, None);
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrations_apply_whole_or_not_at_all() {
        let dir = std::env::temp_dir().join(format!("trek-migrate-lock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trek.sqlite");
        let columns = |c: &Connection| -> HashSet<String> { c.prepare("SELECT name FROM pragma_table_info('threads')").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect() };
        // An older database that got some of today's columns.
        let c = Connection::open(&path).unwrap();
        c.execute_batch("PRAGMA journal_mode = WAL").unwrap();
        c.execute_batch(SCHEMA).unwrap();
        c.execute_batch("ALTER TABLE threads ADD COLUMN side_of TEXT").unwrap();
        let before = columns(&c);
        // Another process holds the write lock past the busy timeout: opening fails, and none
        // of the steps are applied.
        c.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(Store::open(&path).is_err());
        assert_eq!(columns(&c), before);
        c.execute_batch("COMMIT").unwrap();
        // Once it's free, the rest are added around the column that's there already.
        let s = Store::open(&path).unwrap();
        let after = columns(&c);
        assert!(THREAD_COLUMNS_ADDED.iter().all(|(name, _)| after.contains(*name)));
        assert!(s.threads().unwrap().is_empty());
        drop((s, c));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn treks_own_worktrees_belong_to_their_main_checkout() {
        let dir = std::env::temp_dir().join(format!("trek-root-{}", uuid::Uuid::new_v4()));
        let trek = dir.join("worktrees");
        let main = dir.join("repo");
        let linked = |name: &str, at: &Path| {
            std::fs::create_dir_all(main.join(".git/worktrees").join(name)).unwrap();
            std::fs::create_dir_all(at.join("src")).unwrap();
            std::fs::write(main.join(".git/worktrees").join(name).join("commondir"), "../..\n").unwrap();
            std::fs::write(at.join(".git"), format!("gitdir: {}\n", main.join(".git/worktrees").join(name).display())).unwrap();
        };
        let (ours, theirs) = (trek.join("repo/fix-it"), dir.join("elsewhere/linked"));
        linked("fix-it", &ours);
        linked("linked", &theirs);
        assert_eq!(project_root_in(&ours.join("src"), &trek), main);
        assert_eq!(project_root_in(&main, &trek), main);
        // One the user made themselves opens as its own project.
        assert_eq!(project_root_in(&theirs.join("src"), &trek), theirs);
        // A submodule's `.git` file has no `commondir`: it's a project of its own, even in there.
        std::fs::create_dir_all(main.join(".git/modules/sub")).unwrap();
        std::fs::create_dir_all(ours.join("sub")).unwrap();
        std::fs::write(ours.join("sub/.git"), format!("gitdir: {}\n", main.join(".git/modules/sub").display())).unwrap();
        assert_eq!(project_root_in(&ours.join("sub"), &trek), ours.join("sub"));
        let _ = std::fs::remove_dir_all(dir);
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
    fn an_imported_thread_trek_takes_over_drops_the_imported_branch() {
        let s = Store::in_memory().unwrap();
        s.upsert_imported(&[ImportedThread { branch: Some("feature".into()), ..imported("a", "fix the login bug") }]).unwrap();
        let mut t = by_native(&s, "a");
        assert_eq!(t.branch.as_deref(), Some("feature"));
        t.become_trek_thread();
        assert_eq!((t.source, t.branch), (ThreadSource::Trek, None));
        // A thread that was Trek's all along keeps the branch it committed to.
        t.branch = Some("feature".into());
        t.become_trek_thread();
        assert_eq!(t.branch.as_deref(), Some("feature"));
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
        // Nothing on record about its old title: what it's called now may be the user's.
        s.update_thread(&by_native(&s, "auto").id, |t| {
            t.title = "My own name".into();
            t.imported_title = None;
        })
        .unwrap();
        assert!(!s.upsert_imported(&[imported("auto", "New auto title")]).unwrap()[0].retitled);
        assert_eq!(by_native(&s, "auto").title, "My own name");
    }

    #[test]
    fn helper_sessions_are_never_added_and_old_ones_are_archived() {
        let s = Store::in_memory().unwrap();
        let skip = |id: &str| ImportedThread { skip: Some(crate::import::Skip::TempDir), ..imported(id, "say ok") };
        assert_eq!(s.upsert_imported(&[skip("new")]).unwrap(), [Upserted { left_out: true, ..Default::default() }]);
        assert!(s.threads().unwrap().is_empty());
        s.upsert_imported(&[imported("old", "say ok"), imported("pinned", "say ok"), imported("used", "say ok")]).unwrap();
        s.update_thread(&by_native(&s, "pinned").id, |t| t.pinned_at = Some(5)).unwrap();
        s.append_items(&by_native(&s, "used").id, [("u1", &Item::User { text: "keep going".into(), images: vec![], at: None, resume: None, aside: false })]).unwrap();
        let out = s.upsert_imported(&[skip("old"), skip("pinned"), skip("used")]).unwrap();
        // Threads the user pinned or continued in Trek stay.
        assert_eq!(out.iter().map(|o| (o.hidden, o.left_out)).collect::<Vec<_>>(), [(true, true), (false, false), (false, false)]);
        let old = by_native(&s, "old");
        assert!(old.archived_at.is_some() && old.import_hidden.as_deref() == Some("temp-dir"));
        // Still left out on the next import; a thread the user archived isn't the import's doing.
        s.update_thread(&by_native(&s, "used").id, |t| t.archived_at = Some(6)).unwrap();
        let out = s.upsert_imported(&[skip("old"), skip("used")]).unwrap();
        assert_eq!(out.iter().map(|o| o.left_out).collect::<Vec<_>>(), [true, false]);
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
        s.append_items(&by_native(&s, "continued").id, [("c1", &Item::Assistant { text: "kept in Trek".into() })]).unwrap();
        let known = HashSet::from(["here".to_string()]);
        assert_eq!(s.hide_missing(ThreadSource::ClaudeCode, &known).unwrap(), 1);
        assert_eq!(by_native(&s, "deleted").import_hidden.as_deref(), Some(crate::import::DELETED));
        assert!(by_native(&s, "continued").archived_at.is_none());
        // Other sources' threads aren't judged by this source's history.
        assert_eq!(s.hide_missing(ThreadSource::Codex, &HashSet::new()).unwrap(), 0);
        // Back on disk (restored from a backup): back in the sidebar.
        assert!(s.upsert_imported(&[imported("deleted", "t")]).unwrap()[0].restored);
    }

    #[test]
    fn sessions_shown_anyway_stay() {
        let s = Store::in_memory().unwrap();
        let skip = |id: &str| ImportedThread { skip: Some(crate::import::Skip::OneShotRun), ..imported(id, "Reply with OK") };
        // Archived by an import: brought back.
        s.upsert_imported(&[imported("a", "Reply with OK")]).unwrap();
        s.upsert_imported(&[skip("a")]).unwrap();
        let kept = s.keep_imported(&skip("a")).unwrap();
        assert_eq!(kept.id, by_native(&s, "a").id);
        // Never added: added now.
        s.upsert_imported(&[skip("b")]).unwrap();
        s.keep_imported(&skip("b")).unwrap();
        for id in ["a", "b"] {
            let t = by_native(&s, id);
            assert!(t.archived_at.is_none() && t.import_kept && t.import_hidden.is_none(), "{id}");
        }
        // Later imports leave them be.
        assert_eq!(s.upsert_imported(&[skip("a"), skip("b")]).unwrap(), [Upserted::default(), Upserted::default()]);
        assert_eq!(s.threads().unwrap().len(), 2);
    }

    #[test]
    fn imports_revisit_threads_they_hold_however_old() {
        let s = Store::in_memory().unwrap();
        s.upsert_imported(&[imported("shown", "t"), imported("helper", "t"), imported("archived", "t")]).unwrap();
        s.upsert_imported(&[ImportedThread { skip: Some(crate::import::Skip::TempDir), ..imported("helper", "t") }]).unwrap();
        s.update_thread(&by_native(&s, "archived").id, |t| t.archived_at = Some(3)).unwrap();
        let held = s.imported_native_ids(ThreadSource::ClaudeCode).unwrap();
        // Shown ones and ones the import archived (they may need bringing back); not ones the user archived.
        assert_eq!(held, HashSet::from(["shown".to_string(), "helper".to_string()]));
        assert!(s.imported_native_ids(ThreadSource::Codex).unwrap().is_empty());
    }

    #[test]
    fn imported_activity_moves_threads_without_making_them_unread() {
        let s = Store::in_memory().unwrap();
        s.upsert_imported(&[imported("a", "t")]).unwrap();
        s.upsert_imported(&[ImportedThread { updated_at: 50, ..imported("a", "t") }]).unwrap();
        let t = by_native(&s, "a");
        assert_eq!((t.updated_at, t.last_seen_at), (50, 50));
        // A later import that knows better (the last message, not the file's modified time)
        // moves it back down, until it's continued here.
        s.upsert_imported(&[ImportedThread { updated_at: 30, ..imported("a", "t") }]).unwrap();
        assert_eq!(by_native(&s, "a").updated_at, 30);
        s.append_items(&t.id, [("u1", &Item::Assistant { text: "here".into() })]).unwrap();
        s.upsert_imported(&[ImportedThread { updated_at: 20, ..imported("a", "t") }]).unwrap();
        assert_eq!(by_native(&s, "a").updated_at, 30);
    }

    #[test]
    fn auto_settle_waits_for_idle_and_seen() {
        let mut t = thread();
        let later = t.updated_at + 4 * 86_400_000;
        assert!(t.should_auto_settle(later, 3));
        t.updated_at += 1; // unseen
        assert!(!t.should_auto_settle(later, 3));
    }

    #[test]
    fn auto_settle_skips_never_settle_pinned_busy_and_off() {
        const DAY: i64 = 86_400_000;
        let base = thread();
        let later = base.updated_at + 4 * DAY;
        assert!(!base.should_auto_settle(later, 0), "0 days is Never");
        assert!(!base.should_auto_settle(base.updated_at + 3 * DAY, 3), "not yet");
        assert!(!Thread { never_settle: true, ..base.clone() }.should_auto_settle(later, 3));
        assert!(!Thread { pinned_at: Some(1), ..base.clone() }.should_auto_settle(later, 3));
        assert!(!Thread { settled_at: Some(1), ..base.clone() }.should_auto_settle(later, 3));
        for state in [RunState::Working, RunState::NeedsYou, RunState::Failed] {
            assert!(!Thread { run_state: state, ..base.clone() }.should_auto_settle(later, 3), "{state:?}");
        }
    }

    #[test]
    fn auto_settle_waits_from_the_last_look() {
        const DAY: i64 = 86_400_000;
        let base = thread();
        // Last active 10 days ago, looked at (or moved back to the inbox) a day ago.
        let looked = Thread { updated_at: base.updated_at - 10 * DAY, last_seen_at: base.updated_at - DAY, ..base };
        assert!(!looked.should_auto_settle(looked.last_seen_at + 2 * DAY, 3));
        assert!(looked.should_auto_settle(looked.last_seen_at + 3 * DAY + 1, 3));
    }

    #[test]
    fn a_settled_failure_no_longer_needs_the_user_but_is_still_a_failure() {
        let now = now_ms();
        let failed = Thread { run_state: RunState::Failed, ..thread() };
        assert!(failed.needs_you());
        let settled = Thread { settled_at: Some(now), ..failed.clone() };
        assert!(!settled.needs_you());
        assert_eq!(settled.section(now), Some(Section::Settled));
        // A settled thread that asks again is back in the inbox.
        assert_eq!(Thread { run_state: RunState::NeedsYou, ..settled }.section(now), Some(Section::Inbox));
    }

    #[test]
    fn snoozed_threads_wake_into_the_inbox_and_get_the_full_wait_again() {
        const DAY: i64 = 86_400_000;
        let base = thread();
        let wake = base.updated_at + 7 * DAY;
        let snoozed = Thread { snoozed_until: Some(wake), ..base.clone() };
        // Asleep: in Snoozed, and never settled meanwhile, however old its last activity.
        assert_eq!(snoozed.section(wake - 1), Some(Section::Snoozed));
        assert!(!snoozed.should_auto_settle(wake - 1, 3));
        // Awake: back in the inbox, with the whole wait ahead of it.
        assert_eq!(snoozed.section(wake), Some(Section::Inbox));
        assert!(!snoozed.should_auto_settle(wake + 2 * DAY, 3));
        assert!(snoozed.should_auto_settle(wake + 3 * DAY + 1, 3));
        // One that needs the user raises its hand while snoozed.
        assert_eq!(Thread { run_state: RunState::NeedsYou, ..snoozed.clone() }.section(wake - 1), Some(Section::Inbox));
        // A settled thread snoozed by an earlier build stays settled when it wakes.
        assert_eq!(Thread { settled_at: Some(1), ..snoozed }.section(wake), Some(Section::Settled));
    }

    #[test]
    fn imports_repair_models_kept_as_raw_json() {
        let s = Store::in_memory().unwrap();
        let with_model = |id: &str, model: &str| ImportedThread { model: Some(model.into()), ..imported(id, "t") };
        s.upsert_imported(&[with_model("a", r#"{"id":"deepseek-v4-flash-free","providerID":"opencode"}"#), with_model("b", "gpt-4.1")]).unwrap();
        s.update_thread(&by_native(&s, "b").id, |t| t.model = Some("picked-in-trek".into())).unwrap();
        s.upsert_imported(&[with_model("a", "opencode/deepseek-v4-flash-free"), with_model("b", "github-copilot/gpt-4.1")]).unwrap();
        assert_eq!(by_native(&s, "a").model.as_deref(), Some("opencode/deepseek-v4-flash-free"));
        // A model that isn't raw JSON is the thread's own (possibly picked in Trek): it stays.
        assert_eq!(by_native(&s, "b").model.as_deref(), Some("picked-in-trek"));
    }

    #[test]
    fn turns_left_open_by_a_quit_are_closed_at_launch() {
        let s = Store::in_memory().unwrap();
        let open = |state: RunState| {
            let mut t = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
            t.run_state = state;
            s.save_thread(&t).unwrap();
            t.id
        };
        let (working, asking, failed, idle, imported) = (open(RunState::Working), open(RunState::NeedsYou), open(RunState::Failed), open(RunState::Idle), open(RunState::Working));
        let user = Item::User { text: "run the migrations".into(), images: vec![], at: Some(1), resume: None, aside: false };
        let empty_thought = Item::Reasoning { text: " ".into() };
        let partial = said("The schema change needs");
        let tool = |status| Item::Tool { id: "t1".into(), title: "Bash".into(), detail: "make migrate".into(), output: String::new(), status };
        let running = tool(ToolStatus::Running);
        s.append_items(&working, [("u", &user), ("r", &empty_thought), ("p", &partial), ("t", &running)]).unwrap();
        s.append_items(&asking, [("u2", &user)]).unwrap();

        let mut closed = s.close_interrupted_turns().unwrap();
        closed.sort();
        let mut expected = vec![working.clone(), asking.clone(), imported.clone()];
        expected.sort();
        assert_eq!(closed, expected);
        for id in [&working, &asking, &imported] {
            assert_eq!(s.thread(id).unwrap().unwrap().run_state, RunState::Idle);
        }
        assert_eq!(s.thread(&failed).unwrap().unwrap().run_state, RunState::Failed, "a failure still needs the user");
        let notice = Item::Notice { text: INTERRUPTED_BY_QUIT.into() };
        assert_eq!(s.items(&working).unwrap(), vec![user.clone(), partial, tool(ToolStatus::Failed), notice.clone()]);
        assert_eq!(s.items(&asking).unwrap(), vec![user, notice]);
        // No stored transcript (history still in the agent's files): nothing is added.
        assert!(s.items(&imported).unwrap().is_empty());
        assert!(s.items(&idle).unwrap().is_empty());
        assert!(s.close_interrupted_turns().unwrap().is_empty(), "closed once");
    }
}
