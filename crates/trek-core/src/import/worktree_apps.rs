//! Coming from another worktree app: what Conductor, T3 Code and Orca left on disk, shown first
//! and then brought into Trek as projects and threads. Conductor is the one people are leaving
//! (it joined Meta in November 2026; its local app winds down in early 2027).
//!
//! Each worktree stays exactly where it is and keeps its branch; the other app goes on working as
//! before. Everything here only reads: folders, each worktree's `.git` file, and Conductor's
//! database. Agent sessions come from the importers Trek already has, which read `~/.claude`,
//! `~/.codex` and OpenCode's folder, where these apps' agents write them like any other.
//!
//! What Conductor keeps (read from the 0.93 app and its FAQ, since it isn't documented):
//! - `~/Library/Application Support/com.conductor.app/conductor.db`, SQLite. `repos` (`name`,
//!   `root_path`, `default_branch`, `storage_version`), `workspaces` (`directory_name`,
//!   `workspace_path`, `branch`, `state`, `pr_title`, `intended_target_branch`,
//!   `hosting_server_url`), `sessions` (`agent_type`, `claude_session_id`), `settings`
//!   (`conductor_root_path`).
//! - Worktrees at `<root, or ~/conductor>/workspaces/<repo>/<directory>`; repos of older versions
//!   keep them at `<repo>/<directory>` (`storage_version` 1) or `<repo>/.conductor/<directory>` (2).
//! - `cloud-workspaces` and `remote-workspace-sync` beside `workspaces`: its own clones, not the
//!   user's.
//!
//! The database only adds to what the folders say (names, the base branch, what's archived), so a
//! newer Conductor whose schema moved still comes across, by its folders.

use super::{ImportedThread, claude, codex, import_found, opencode};
use crate::settings::Import;
use crate::store::{AdoptedWorktree, Store};
use crate::types::{AgentId, Effort, HandHolding};
use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// How deep under an app's folder its worktrees are: `<repo>/<name>`.
const DEPTH: usize = 2;
/// Entries looked at per folder before giving up on the rest: a folder someone filled with other
/// things mustn't hold this up.
const ENTRIES_MAX: usize = 5000;

/// The apps worktrees are brought in from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WorktreeApp {
    Conductor,
    T3Code,
    Orca,
}

impl WorktreeApp {
    pub const ALL: [WorktreeApp; 3] = [
        WorktreeApp::Conductor,
        WorktreeApp::T3Code,
        WorktreeApp::Orca,
    ];

    pub fn name(self) -> &'static str {
        match self {
            WorktreeApp::Conductor => "Conductor",
            WorktreeApp::T3Code => "T3 Code",
            WorktreeApp::Orca => "Orca",
        }
    }

    /// How the store records whose a worktree is.
    pub fn key(self) -> &'static str {
        match self {
            WorktreeApp::Conductor => "conductor",
            WorktreeApp::T3Code => "t3-code",
            WorktreeApp::Orca => "orca",
        }
    }

    pub fn from_key(key: &str) -> Option<WorktreeApp> {
        Self::ALL.into_iter().find(|a| a.key() == key)
    }

    /// Where it keeps worktrees unless told otherwise.
    fn default_folder(self, home: &Path) -> PathBuf {
        match self {
            WorktreeApp::Conductor => home.join("conductor/workspaces"),
            WorktreeApp::T3Code => home.join(".t3/worktrees"),
            WorktreeApp::Orca => home.join("orca/workspaces"),
        }
    }
}

/// Where the agents' histories are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histories {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub opencode: PathBuf,
}

impl Histories {
    /// The user's own.
    pub fn real() -> Self {
        Histories {
            claude: claude::root(),
            codex: codex::codex_home(),
            opencode: opencode::data_dir(),
        }
    }

    /// Those of a made-up home folder, whatever the environment says.
    pub fn under(home: &Path) -> Self {
        Histories {
            claude: home.join(".claude/projects"),
            codex: home.join(".codex"),
            opencode: home.join(".local/share/opencode"),
        }
    }

    /// Every session in `folders`, however old: "How far back" is for finding threads in general,
    /// and these are worktrees the user is asking for. An importer turned off stays off.
    fn sessions(&self, folders: &[PathBuf], import: &Import) -> Vec<ImportedThread> {
        let mut out = vec![];
        if import.claude_code {
            out.extend(claude::scan_folders(&self.claude, folders));
        }
        if import.codex {
            out.extend(codex::scan_home(&self.codex, 0, &HashSet::new()));
        }
        if import.opencode {
            out.extend(opencode::scan_dir(&self.opencode, 0, &HashSet::new()));
        }
        out
    }
}

/// A worktree that would come across.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundWorktree {
    pub path: PathBuf,
    pub branch: String,
    /// The branch it merges back into.
    pub base: String,
    /// What the app calls it: a thread's title when no session comes with it.
    pub name: String,
    /// Agent sessions the importers found in it, newest first.
    pub sessions: Vec<ImportedThread>,
    /// Threads already in Trek that work in it, and move under its project.
    pub moving: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FoundRepo {
    /// The main checkout: the project.
    pub path: PathBuf,
    pub name: String,
    pub worktrees: Vec<FoundWorktree>,
}

/// What won't come across, by why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeftOut {
    pub archived: usize,
    /// Not on a branch (detached HEAD): nothing to keep it on.
    pub detached: usize,
    /// Its repository's main checkout is gone, or can't be told.
    pub no_repo: usize,
    /// In Conductor's cloud, or one of its own clones.
    pub cloud: usize,
    /// Sessions of agents Trek has no importer for (Cursor, ACP agents).
    pub other_agents: usize,
}

impl LeftOut {
    /// In words, for under the list: empty when nothing's left out.
    pub fn words(&self) -> Vec<String> {
        let mut out = vec![];
        let n = |count: usize, one: &str, many: &str| {
            format!("{count} {}", if count == 1 { one } else { many })
        };
        if self.archived > 0 {
            out.push(n(
                self.archived,
                "archived workspace",
                "archived workspaces",
            ));
        }
        if self.cloud > 0 {
            out.push(n(self.cloud, "cloud workspace", "cloud workspaces"));
        }
        if self.no_repo > 0 {
            out.push(n(
                self.no_repo,
                "worktree whose repository is gone",
                "worktrees whose repository is gone",
            ));
        }
        if self.detached > 0 {
            out.push(n(
                self.detached,
                "worktree not on a branch",
                "worktrees not on a branch",
            ));
        }
        if self.other_agents > 0 {
            out.push(n(
                self.other_agents,
                "session from an agent Trek can't import",
                "sessions from agents Trek can't import",
            ));
        }
        out
    }
}

/// What one app would bring over.
#[derive(Debug, Clone, PartialEq)]
pub struct AppPlan {
    pub app: WorktreeApp,
    /// The folder it keeps them in.
    pub folder: PathBuf,
    pub repos: Vec<FoundRepo>,
    pub left_out: LeftOut,
}

impl AppPlan {
    pub fn worktrees(&self) -> usize {
        self.repos.iter().map(|r| r.worktrees.len()).sum()
    }

    pub fn sessions(&self) -> usize {
        self.all_worktrees().map(|w| w.sessions.len()).sum()
    }

    pub fn moving(&self) -> usize {
        self.all_worktrees().map(|w| w.moving).sum()
    }

    fn all_worktrees(&self) -> impl Iterator<Item = &FoundWorktree> {
        self.repos.iter().flat_map(|r| r.worktrees.iter())
    }

    /// "2 projects, 3 worktrees, 4 agent sessions".
    pub fn counts(&self) -> String {
        let n = |count: usize, one: &str, many: &str| {
            format!("{count} {}", if count == 1 { one } else { many })
        };
        format!(
            "{}, {}, {}",
            n(self.repos.len(), "project", "projects"),
            n(self.worktrees(), "worktree", "worktrees"),
            n(self.sessions(), "agent session", "agent sessions"),
        )
    }
}

/// What bringing them over did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Adopted {
    pub projects: Vec<PathBuf>,
    pub worktrees: usize,
    /// Threads that are new: one per session, and one per worktree no session came with.
    pub new_threads: usize,
    /// Threads already here that moved under their project.
    pub moved: usize,
}

/// A linked worktree's folder in `.git/worktrees`, from its `.git` file.
fn linked_git_dir(dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dir.join(".git")).ok()?;
    let gitdir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
    Some(if gitdir.is_absolute() {
        gitdir
    } else {
        dir.join(gitdir)
    })
}

/// The branch a git folder's `HEAD` is on; `None` when it's detached or can't be read.
fn head_branch(git_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
        .filter(|b| !b.is_empty())
}

/// Folders under `root`, at most `depth` down, that are linked worktrees: they have a `.git`
/// file. A `.git` folder is a clone, which stays out, and so does what's inside it. Hidden folders
/// are an app's own business (`.orca-preparing`).
fn find_worktrees(root: &Path, depth: usize, entries_max: usize) -> Vec<PathBuf> {
    fn walk(dir: &Path, left: usize, seen: &mut usize, max: usize, out: &mut Vec<PathBuf>) {
        if *seen >= max {
            return;
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let entries: Vec<_> = read.flatten().collect();
        *seen += entries.len();
        let git = entries.iter().find(|e| e.file_name() == ".git");
        match git.and_then(|e| e.file_type().ok()) {
            Some(kind) if kind.is_file() => {
                out.push(dir.to_path_buf());
                return;
            }
            Some(_) => return,
            None => {}
        }
        if left == 0 {
            return;
        }
        for e in entries {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            if e.file_type().is_ok_and(|k| k.is_dir()) {
                walk(&e.path(), left - 1, seen, max, out);
            }
        }
    }
    let mut out = vec![];
    let mut seen = 0;
    walk(root, depth, &mut seen, entries_max, &mut out);
    if seen >= entries_max {
        tracing::warn!("stopped looking in {} at the limit", root.display());
    }
    out
}

/// What Conductor's database says about a workspace.
#[derive(Debug, Clone, Default, PartialEq)]
struct ConductorRow {
    name: Option<String>,
    base: Option<String>,
}

/// What Conductor's database adds to its folders.
#[derive(Debug, Default)]
struct ConductorDb {
    /// `conductor_root_path`: where its folders are, when not `~/conductor`.
    root: Option<PathBuf>,
    rows: HashMap<PathBuf, ConductorRow>,
    archived: HashSet<PathBuf>,
    cloud: usize,
    other_agents: usize,
}

/// `path` as a read-only connection that leaves its folder as it was. Plain read-only still makes
/// `-shm` and `-wal` files beside a WAL database nothing has open; `immutable` makes none, and is
/// right as long as there's no write-ahead log to read, while an app with the database open has
/// them there already, so read-only adds nothing.
fn open_untouched(path: &Path) -> Option<Connection> {
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    let in_use = std::fs::metadata(&wal).is_ok_and(|m| m.len() > 0);
    // Characters a URI would read as something else.
    let mut name = String::new();
    for c in path.to_string_lossy().chars() {
        match c {
            '%' => name.push_str("%25"),
            '?' => name.push_str("%3F"),
            '#' => name.push_str("%23"),
            '\\' => name.push('/'),
            c => name.push(c),
        }
    }
    let lead = if name.starts_with('/') { "" } else { "/" };
    let uri = format!(
        "file:{lead}{name}?{}",
        if in_use { "mode=ro" } else { "immutable=1" }
    );
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(2));
    Some(conn)
}

fn columns(conn: &Connection, table: &str) -> HashSet<String> {
    conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0))?.collect())
        .unwrap_or_default()
}

/// A branch as its name, not as a remote's (`origin/main` is where Conductor forks from).
fn plain_branch(name: &str) -> &str {
    name.strip_prefix("origin/").unwrap_or(name)
}

/// What a worktree's changes compare with: `wanted` as a local branch, or as `origin`'s when only
/// that exists (Conductor fetches a base rather than updating the local one, and some repos never
/// had it locally). `None` when git knows neither: comparing with it would only fail.
fn known_base(repo: &Path, wanted: &str) -> Option<String> {
    let has = |reference: String| {
        crate::worktree::git(repo, &["show-ref", "--verify", "--quiet", &reference]).is_ok()
    };
    let name = plain_branch(wanted);
    if has(format!("refs/heads/{name}")) {
        Some(name.to_string())
    } else if has(format!("refs/remotes/origin/{name}")) {
        Some(format!("origin/{name}"))
    } else {
        None
    }
}

/// The folder a workspace is in, as Conductor works it out.
fn conductor_path(
    root_path: &str,
    repo_name: &str,
    storage_version: i64,
    directory: &str,
    workspaces: &Path,
) -> PathBuf {
    let root = Path::new(root_path);
    match storage_version {
        1 if directory == "." => root.join(".main"),
        1 => root.join(directory),
        2 if directory == "." => root.to_path_buf(),
        2 => root.join(".conductor").join(directory),
        _ => workspaces.join(repo_name).join(directory),
    }
}

/// Read what the database says, or `None` when it can't be read as expected: its folders still
/// say what there is.
fn read_conductor(path: &Path, home: &Path) -> Option<ConductorDb> {
    let conn = open_untouched(path)?;
    let mut db = ConductorDb {
        root: conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'conductor_root_path'",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
        ..Default::default()
    };
    let workspaces = db
        .root
        .clone()
        .unwrap_or_else(|| home.join("conductor"))
        .join("workspaces");
    let (w, r) = (columns(&conn, "workspaces"), columns(&conn, "repos"));
    let opt = |have: &HashSet<String>, table: &str, name: &str| {
        if have.contains(name) {
            format!("{table}.{name}")
        } else {
            "NULL".to_string()
        }
    };
    let sql = format!(
        "SELECT w.directory_name, w.state, r.root_path, r.name, {storage}, {path}, {name}, {pr_title},
                {target}, {parent}, {default}, {hosting}
         FROM workspaces w JOIN repos r ON w.repository_id = r.id",
        storage = match r.contains("storage_version") {
            true => "COALESCE(r.storage_version, 1)",
            false => "1",
        },
        path = opt(&w, "w", "workspace_path"),
        name = opt(&w, "w", "workspace_name"),
        pr_title = opt(&w, "w", "pr_title"),
        target = opt(&w, "w", "intended_target_branch"),
        parent = opt(&w, "w", "initialization_parent_branch"),
        default = opt(&r, "r", "default_branch"),
        hosting = opt(&w, "w", "hosting_server_url"),
    );
    let mut st = conn.prepare(&sql).ok()?;
    let rows = st
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                [
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ],
                row.get::<_, Option<String>>(11)?,
            ))
        })
        .ok()?
        .flatten()
        .collect::<Vec<_>>();
    let filled = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    for (directory, state, root_path, repo_name, version, wpath, name, pr_title, bases, hosting) in
        rows
    {
        // `/` is how it writes "this computer".
        if filled(hosting).is_some_and(|h| h != "/") {
            db.cloud += 1;
            continue;
        }
        let directory = filled(directory);
        let path = filled(wpath).map(PathBuf::from).or_else(|| {
            Some(conductor_path(
                &filled(root_path)?,
                &filled(repo_name)?,
                version.unwrap_or(1),
                directory.as_deref()?,
                &workspaces,
            ))
        });
        let Some(path) = path else { continue };
        if matches!(state.as_deref(), Some("archived" | "archiving")) {
            db.archived.insert(real(&path));
            continue;
        }
        db.rows.insert(
            real(&path),
            ConductorRow {
                name: filled(pr_title).or(filled(name)).or(directory),
                base: bases.into_iter().find_map(filled),
            },
        );
    }
    // Sessions Trek has no importer for: what it can bring doesn't include them. Empty ones never
    // started an agent, so they have no session id.
    if columns(&conn, "sessions").contains("agent_type") {
        db.other_agents = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE agent_type IN ('cursor', 'acp')
                 AND claude_session_id IS NOT NULL AND COALESCE(is_hidden, 0) = 0",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0) as usize;
    }
    Some(db)
}

/// What one app would bring over: `None` when its folder isn't there, or nothing new is in it.
fn plan_app(
    app: WorktreeApp,
    home: &Path,
    conductor_db: &Path,
    known: &dyn Fn(&Path) -> bool,
) -> Option<AppPlan> {
    let db = (app == WorktreeApp::Conductor && conductor_db.is_file())
        .then(|| read_conductor(conductor_db, home))
        .flatten();
    let conductor_root = db.as_ref().and_then(|d| d.root.clone());
    let folder = match &conductor_root {
        Some(root) => root.join("workspaces"),
        None => app.default_folder(home),
    };
    let on_disk = find_worktrees(&folder, DEPTH, ENTRIES_MAX);
    let mut candidates: Vec<PathBuf> = on_disk;
    if let Some(db) = &db {
        candidates.extend(db.rows.keys().cloned());
    }
    candidates.sort();
    candidates.dedup();
    // Its own clones, in the folders beside the worktrees.
    let own: Vec<PathBuf> = match app {
        WorktreeApp::Conductor => {
            let base = conductor_root.unwrap_or_else(|| home.join("conductor"));
            vec![
                base.join("cloud-workspaces"),
                base.join("remote-workspace-sync"),
            ]
        }
        _ => vec![],
    };
    let mut left_out = LeftOut::default();
    let mut repos: BTreeMap<PathBuf, Vec<FoundWorktree>> = BTreeMap::new();
    let mut seen = HashSet::new();
    for path in candidates {
        // A folder the database still lists after it went, or Conductor's root checkout.
        let Some(git_dir) = linked_git_dir(&path) else {
            continue;
        };
        // The same folder, as the database and the walk each spell it.
        let key = real(&path);
        if !seen.insert(key.clone()) || known(&path) {
            continue;
        }
        if db.as_ref().is_some_and(|d| d.archived.contains(&key)) {
            left_out.archived += 1;
            continue;
        }
        let Some(repo) = crate::worktree::main_checkout(&path) else {
            left_out.no_repo += 1;
            continue;
        };
        if own.iter().any(|o| repo.starts_with(o)) {
            left_out.cloud += 1;
            continue;
        }
        let Some(branch) = head_branch(&git_dir) else {
            left_out.detached += 1;
            continue;
        };
        let row = db.as_ref().and_then(|d| d.rows.get(&key));
        let base = row
            .and_then(|r| r.base.as_deref())
            .filter(|b| plain_branch(b) != branch)
            .and_then(|b| known_base(&repo, b))
            .or_else(|| head_branch(&repo.join(".git")).filter(|b| *b != branch))
            .unwrap_or_else(|| "main".into());
        let name = row
            .and_then(|r| r.name.clone())
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| branch.clone());
        repos.entry(repo).or_default().push(FoundWorktree {
            path,
            branch,
            base,
            name,
            sessions: vec![],
            moving: 0,
        });
    }
    if let Some(db) = &db {
        left_out.cloud += db.cloud;
        left_out.other_agents += db.other_agents;
        left_out.archived += db.archived.iter().filter(|p| !p.exists()).count();
    }
    if repos.is_empty() {
        return None;
    }
    Some(AppPlan {
        app,
        folder,
        repos: repos
            .into_iter()
            .map(|(path, worktrees)| FoundRepo {
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string()),
                path,
                worktrees,
            })
            .collect(),
        left_out,
    })
}

/// A folder with its links resolved, for comparing what agents recorded with what git says.
fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Conductor's database, by the home folder it's under.
pub fn conductor_db(home: &Path) -> PathBuf {
    home.join("Library/Application Support/com.conductor.app/conductor.db")
}

/// What each app would bring over, with no app listed that has nothing new. Reads only; run it off
/// the main thread (it walks folders and reads session heads).
pub fn scan(store: &Store, home: &Path, histories: &Histories, import: &Import) -> Vec<AppPlan> {
    let adopted: HashSet<PathBuf> = store
        .adopted_worktrees()
        .unwrap_or_default()
        .into_iter()
        .map(|w| w.path)
        .collect();
    let known = |path: &Path| {
        adopted.contains(path) || store.threads_in_worktree(path).is_ok_and(|n| n > 0)
    };
    let db = conductor_db(home);
    let mut plans: Vec<AppPlan> = WorktreeApp::ALL
        .into_iter()
        .filter_map(|app| plan_app(app, home, &db, &known))
        .collect();
    if plans.is_empty() {
        return plans;
    }
    // The sessions in them, by the worktree they ran in. Trek's own threads are shown as they are.
    let folders: Vec<PathBuf> = plans
        .iter()
        .flat_map(|p| p.all_worktrees())
        .map(|w| w.path.clone())
        .collect();
    let trek = store.trek_native_ids().unwrap_or_default();
    let by_real: Vec<(PathBuf, PathBuf)> = folders.iter().map(|f| (real(f), f.clone())).collect();
    let mut found: HashMap<PathBuf, Vec<ImportedThread>> = HashMap::new();
    for t in histories.sessions(&folders, import) {
        if t.skip.is_some() || trek.contains(&t.native_id) {
            continue;
        }
        let Some(cwd) = t.cwd.as_deref() else {
            continue;
        };
        let cwd_real = real(cwd);
        let holder = by_real
            .iter()
            .find(|(r, f)| cwd.starts_with(f) || cwd_real.starts_with(r))
            .map(|(_, f)| f.clone());
        if let Some(holder) = holder {
            found.entry(holder).or_default().push(t);
        }
    }
    for w in plans
        .iter_mut()
        .flat_map(|p| p.repos.iter_mut())
        .flat_map(|r| r.worktrees.iter_mut())
    {
        let mut sessions = found.remove(&w.path).unwrap_or_default();
        sessions.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        w.sessions = sessions;
        w.moving = store.threads_to_adopt(&w.path).unwrap_or(0);
    }
    plans
}

/// Bring what `plan` shows into Trek: its repositories as projects, a thread per session found,
/// and a thread to carry on in for a worktree without any. Nothing on disk changes, and those
/// worktrees are marked as the other app's, so Trek never removes one.
pub fn adopt(
    store: &Store,
    plan: &AppPlan,
    agent: AgentId,
    effort: Effort,
    hand_holding: HandHolding,
) -> Result<Adopted> {
    let mut out = Adopted::default();
    let mut sessions = vec![];
    for repo in &plan.repos {
        out.projects.push(repo.path.clone());
        for w in &repo.worktrees {
            out.moved += store.adopt_worktree(&AdoptedWorktree {
                path: w.path.clone(),
                app: plan.app.key().to_string(),
                project: repo.path.clone(),
                branch: w.branch.clone(),
                base: w.base.clone(),
            })?;
            out.worktrees += 1;
            sessions.extend(w.sessions.iter().cloned());
        }
    }
    out.new_threads += import_found(store, sessions).new_threads;
    for repo in &plan.repos {
        for w in &repo.worktrees {
            if store.threads_in_worktree(&w.path)? > 0 {
                continue;
            }
            let mut t = store.create_thread(None, agent.clone(), None, effort, hand_holding)?;
            let project = store.ensure_project(&repo.path)?;
            t.project_id = Some(project.id);
            t.title = w.name.clone();
            t.cwd = Some(w.path.clone());
            t.branch = Some(w.branch.clone());
            t.worktree = Some(crate::worktree::Worktree {
                path: w.path.clone(),
                branch: w.branch.clone(),
                base: w.base.clone(),
            });
            store.save_thread(&t)?;
            out.new_threads += 1;
        }
    }
    for w in plan.all_worktrees() {
        store.surface_worktree(&w.path)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::git;
    use std::collections::BTreeSet;

    /// A folder of its own for each test: a made-up home with repositories and worktrees in it,
    /// never the user's.
    struct Home(PathBuf);

    impl Home {
        fn new(name: &str) -> Home {
            let dir = std::env::temp_dir().join(format!(
                "trek-worktree-apps-{name}-{}",
                uuid::Uuid::now_v7().simple()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Home(real(&dir))
        }

        /// A repository under `code/`, with a commit, on `main`.
        fn repo(&self, name: &str) -> PathBuf {
            let repo = self.0.join("code").join(name);
            std::fs::create_dir_all(&repo).unwrap();
            for args in [
                &["init", "-q", "-b", "main"][..],
                &["config", "user.email", "t@example.com"],
                &["config", "user.name", "T"],
                &["commit", "-q", "--allow-empty", "-m", "first"],
            ] {
                git(&repo, args).unwrap();
            }
            repo
        }

        fn worktree(&self, repo: &Path, at: &str, branch: &str) -> PathBuf {
            let path = self.0.join(at);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            git(
                repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    branch,
                    path.to_str().unwrap(),
                ],
            )
            .unwrap();
            real(&path)
        }

        /// A Claude Code session that ran in `cwd`, where Claude Code would keep it.
        fn claude_session(&self, id: &str, cwd: &Path, title: &str) {
            let folder = self
                .0
                .join(".claude/projects")
                .join(claude::folder_name(cwd));
            std::fs::create_dir_all(&folder).unwrap();
            let line = |role: &str, text: &str| {
                serde_json::json!({
                    "type": role,
                    "cwd": cwd,
                    "sessionId": id,
                    "uuid": uuid::Uuid::now_v7().to_string(),
                    "timestamp": "2026-10-01T03:27:55Z",
                    "entrypoint": "cli",
                    "message": { "role": role, "content": text },
                })
                .to_string()
            };
            // Two prompts: this home is in a temp folder, where a single one reads as a scratch run.
            std::fs::write(
                folder.join(format!("{id}.jsonl")),
                format!(
                    "{}\n{}\n{}\n{}\n",
                    line("user", title),
                    line("assistant", "Done"),
                    line("user", "And the tests"),
                    line("assistant", "Done"),
                ),
            )
            .unwrap();
        }

        /// Conductor's database as version 0.93 has it, the columns read here and a few others.
        fn conductor(&self, sql: &str) -> PathBuf {
            let db = conductor_db(&self.0);
            std::fs::create_dir_all(db.parent().unwrap()).unwrap();
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE repos (id TEXT, remote_url TEXT, name TEXT, default_branch TEXT DEFAULT 'main', root_path TEXT, storage_version INTEGER DEFAULT 1, hidden INTEGER DEFAULT 0);
                 CREATE TABLE workspaces (local_id TEXT, repository_id TEXT, directory_name TEXT, branch TEXT, state TEXT DEFAULT 'active', initialization_parent_branch TEXT, intended_target_branch TEXT, pr_title TEXT, workspace_path TEXT, workspace_name TEXT, hosting_server_url TEXT, secondary_directory_name TEXT);
                 CREATE TABLE sessions (id TEXT, workspace_id TEXT, agent_type TEXT, claude_session_id TEXT, title TEXT DEFAULT 'Untitled', is_hidden INTEGER DEFAULT 0);
                 CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);",
            )
            .unwrap();
            conn.execute_batch(sql).unwrap();
            drop(conn);
            db
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn all_on() -> Import {
        Import {
            claude_code: true,
            codex: true,
            opencode: true,
            max_age_days: 90,
        }
    }

    fn plans(store: &Store, home: &Home) -> Vec<AppPlan> {
        scan(store, &home.0, &Histories::under(&home.0), &all_on())
    }

    /// Every file under `dir` with its contents, to show nothing changed.
    fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        walkdir::WalkDir::new(dir)
            .into_iter()
            .flatten()
            .filter(|e| e.file_type().is_file())
            .map(|e| (e.path().to_path_buf(), std::fs::read(e.path()).unwrap()))
            .collect()
    }

    #[test]
    fn nothing_is_offered_when_the_apps_folders_arent_there() {
        let home = Home::new("empty");
        let store = Store::in_memory().unwrap();
        assert!(plans(&store, &home).is_empty());
        // Nor when the folder has no worktree in it: a clone, say, as Orca's can have.
        let repo = home.0.join("orca/workspaces/acme/app");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(home.0.join(".t3/worktrees")).unwrap();
        assert!(plans(&store, &home).is_empty());
    }

    #[test]
    fn conductors_folders_alone_show_what_comes_across() {
        let home = Home::new("folders");
        let repo = home.repo("acme");
        let wt = home.worktree(&repo, "conductor/workspaces/acme/kyoto", "tobias/kyoto");
        home.claude_session("11111111-1111-1111-1111-111111111111", &wt, "Fix the login");
        let store = Store::in_memory().unwrap();

        let found = plans(&store, &home);
        assert_eq!(found.len(), 1);
        let plan = &found[0];
        assert_eq!(plan.app, WorktreeApp::Conductor);
        assert_eq!(plan.counts(), "1 project, 1 worktree, 1 agent session");
        assert_eq!(plan.repos[0].path, repo);
        assert_eq!(plan.repos[0].name, "acme");
        let w = &plan.repos[0].worktrees[0];
        assert_eq!(w.path, wt);
        assert_eq!(w.branch, "tobias/kyoto");
        assert_eq!(w.base, "main");
        assert_eq!(w.name, "kyoto");
        assert_eq!(w.sessions[0].title, "Fix the login");
        assert_eq!(plan.left_out, LeftOut::default());
    }

    #[test]
    fn conductors_database_names_workspaces_and_says_whats_left_out() {
        let home = Home::new("db");
        let repo = home.repo("acme");
        let kyoto = home.worktree(&repo, "conductor/workspaces/acme/kyoto", "feat/login");
        let osaka = home.worktree(&repo, "conductor/workspaces/acme/osaka", "fix/old");
        // A repo from before Conductor moved its worktrees out of the repository.
        let old_style = home.worktree(&repo, "code/acme/.conductor/nara", "fix/nav");
        git(&repo, &["branch", "develop"]).unwrap();
        let db = home.conductor(&format!(
            "INSERT INTO repos (id, name, root_path, default_branch, storage_version) VALUES
               ('r', 'acme', '{root}', 'main', 3), ('o', 'acme', '{root}', 'main', 2);
             INSERT INTO workspaces (repository_id, directory_name, branch, state, pr_title, intended_target_branch, hosting_server_url) VALUES
               ('r', 'kyoto', 'feat/login', 'ready', 'Refresh the login token', 'develop', '/'),
               ('r', 'osaka', 'fix/old', 'archived', NULL, NULL, NULL),
               ('r', 'cloudy', 'feat/cloud', 'ready', NULL, NULL, 'https://roundhouse.example'),
               ('o', 'nara', 'fix/nav', 'ready', NULL, 'origin/main', NULL),
               ('r', 'gone', 'fix/gone', 'archived', NULL, NULL, NULL);
             INSERT INTO sessions (id, workspace_id, agent_type, claude_session_id) VALUES
               ('1', 'k', 'cursor', 'c1'), ('2', 'k', 'claude', 'x'), ('3', 'k', 'cursor', NULL);",
            root = repo.display()
        ));
        let before = snapshot(db.parent().unwrap());
        let store = Store::in_memory().unwrap();
        let found = plans(&store, &home);
        // The database's folder is exactly as it was: no `-wal`, no `-shm`.
        assert_eq!(snapshot(db.parent().unwrap()), before);

        let plan = &found[0];
        let by_path = |p: &Path| {
            plan.repos[0]
                .worktrees
                .iter()
                .find(|w| w.path == p)
                .unwrap_or_else(|| panic!("{p:?} in {plan:#?}"))
        };
        let k = by_path(&kyoto);
        assert_eq!(k.name, "Refresh the login token");
        assert_eq!(k.base, "develop", "the branch it's set to go into");
        // Found through the old layout's path, which no folder walk would reach.
        assert_eq!(by_path(&old_style).base, "main");
        assert_eq!(by_path(&old_style).name, "nara");
        assert!(plan.repos[0].worktrees.iter().all(|w| w.path != osaka));
        assert_eq!(plan.left_out.archived, 2);
        assert_eq!(plan.left_out.cloud, 1);
        assert_eq!(plan.left_out.other_agents, 1);
        assert_eq!(
            plan.left_out.words(),
            [
                "2 archived workspaces",
                "1 cloud workspace",
                "1 session from an agent Trek can't import"
            ]
        );
    }

    #[test]
    fn a_base_git_doesnt_know_gives_way_to_what_it_does() {
        let home = Home::new("base");
        let repo = home.repo("acme");
        // Known only as the remote's.
        git(
            &repo,
            &["update-ref", "refs/remotes/origin/release", "HEAD"],
        )
        .unwrap();
        let (a, b) = (
            home.worktree(&repo, "conductor/workspaces/acme/a", "feat/a"),
            home.worktree(&repo, "conductor/workspaces/acme/b", "feat/b"),
        );
        home.conductor(&format!(
            "INSERT INTO repos (id, name, root_path, storage_version) VALUES ('r', 'acme', '{}', 3);
             INSERT INTO workspaces (repository_id, directory_name, branch, state, intended_target_branch) VALUES
               ('r', 'a', 'feat/a', 'ready', 'origin/release'),
               ('r', 'b', 'feat/b', 'ready', 'never-heard-of-it');",
            repo.display()
        ));
        let store = Store::in_memory().unwrap();
        let found = plans(&store, &home);
        let base_of = |p: &Path| {
            let w = found[0].repos[0].worktrees.iter().find(|w| w.path == p);
            w.unwrap().base.clone()
        };
        assert_eq!(base_of(&a), "origin/release");
        assert_eq!(base_of(&b), "main", "what the repository's folder is on");
    }

    #[test]
    fn a_database_it_cant_read_leaves_the_folders_to_say() {
        let home = Home::new("baddb");
        let repo = home.repo("acme");
        home.worktree(&repo, "conductor/workspaces/acme/kyoto", "feat/login");
        let db = conductor_db(&home.0);
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        std::fs::write(&db, b"not a database").unwrap();
        let store = Store::in_memory().unwrap();
        assert_eq!(plans(&store, &home)[0].worktrees(), 1);
    }

    #[test]
    fn conductors_moved_root_is_followed() {
        let home = Home::new("root");
        let repo = home.repo("acme");
        let moved = home.0.join("elsewhere");
        let wt = home.worktree(&repo, "elsewhere/workspaces/acme/kyoto", "feat/x");
        home.conductor(&format!(
            "INSERT INTO settings (key, value) VALUES ('conductor_root_path', '{}');",
            moved.display()
        ));
        let store = Store::in_memory().unwrap();
        let found = plans(&store, &home);
        assert_eq!(found[0].folder, moved.join("workspaces"));
        assert_eq!(found[0].repos[0].worktrees[0].path, wt);
    }

    #[test]
    fn t3_codes_and_orcas_worktrees_come_the_same_way() {
        let home = Home::new("others");
        let repo = home.repo("acme");
        home.worktree(&repo, ".t3/worktrees/acme/a1", "t3/a1");
        home.worktree(&repo, "orca/workspaces/acme/b1", "orca/b1");
        let store = Store::in_memory().unwrap();
        let found = plans(&store, &home);
        let apps: Vec<_> = found.iter().map(|p| p.app).collect();
        assert_eq!(apps, [WorktreeApp::T3Code, WorktreeApp::Orca]);
    }

    #[test]
    fn what_cant_come_across_is_said_rather_than_guessed() {
        let home = Home::new("left");
        let repo = home.repo("acme");
        let detached = home.worktree(&repo, "conductor/workspaces/acme/loose", "feat/loose");
        git(&detached, &["checkout", "-q", "--detach"]).unwrap();
        let gone = home.repo("gone");
        home.worktree(&gone, "conductor/workspaces/gone/orphan", "feat/orphan");
        std::fs::remove_dir_all(&gone).unwrap();
        let cloud = home.0.join("conductor/cloud-workspaces/acme");
        std::fs::create_dir_all(&cloud).unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "t@example.com"],
            &["config", "user.name", "T"],
            &["commit", "-q", "--allow-empty", "-m", "first"],
        ] {
            git(&cloud, args).unwrap();
        }
        home.worktree(&cloud, "conductor/workspaces/acme/synced", "feat/synced");
        let keep = home.worktree(&repo, "conductor/workspaces/acme/keep", "feat/keep");
        let store = Store::in_memory().unwrap();
        let found = plans(&store, &home);
        let plan = &found[0];
        assert_eq!(plan.worktrees(), 1);
        assert_eq!(plan.repos[0].worktrees[0].path, keep);
        assert_eq!(plan.left_out.detached, 1);
        assert_eq!(plan.left_out.no_repo, 1);
        assert_eq!(plan.left_out.cloud, 1);
    }

    #[test]
    fn bringing_them_over_makes_projects_and_threads_and_touches_nothing() {
        let home = Home::new("adopt");
        let repo = home.repo("acme");
        let kyoto = home.worktree(&repo, "conductor/workspaces/acme/kyoto", "feat/login");
        let osaka = home.worktree(&repo, "conductor/workspaces/acme/osaka", "fix/nav");
        home.claude_session(
            "11111111-1111-1111-1111-111111111111",
            &kyoto,
            "Fix the login",
        );
        home.claude_session("22222222-2222-2222-2222-222222222222", &kyoto, "Add a test");
        let db = home.conductor(&format!(
            "INSERT INTO repos (id, name, root_path, storage_version) VALUES ('r', 'acme', '{}', 3);
             INSERT INTO workspaces (repository_id, directory_name, branch, state, workspace_name) VALUES
               ('r', 'osaka', 'fix/nav', 'ready', 'Tidy the nav');",
            repo.display()
        ));
        let before: BTreeSet<_> = snapshot(&home.0).into_iter().collect();
        let branch_before = git(&kyoto, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        let store = Store::in_memory().unwrap();

        let found = plans(&store, &home);
        assert_eq!(
            found[0].counts(),
            "1 project, 2 worktrees, 2 agent sessions"
        );
        // Scanning changed nothing in the store either.
        assert!(store.threads().unwrap().is_empty());
        assert!(store.projects().unwrap().is_empty());

        let done = adopt(
            &store,
            &found[0],
            AgentId::ClaudeCode,
            Effort::Medium,
            HandHolding::Auto,
        )
        .unwrap();
        assert_eq!(done.projects, std::slice::from_ref(&repo));
        assert_eq!(done.worktrees, 2);
        assert_eq!(done.new_threads, 3);

        let projects = store.projects().unwrap();
        assert_eq!(
            projects.len(),
            1,
            "one project per repository: {projects:?}"
        );
        assert_eq!(projects[0].path, repo);
        let threads = store.threads().unwrap();
        assert_eq!(threads.len(), 3);
        for t in &threads {
            assert_eq!(t.project_id.as_ref(), Some(&projects[0].id));
            let wt = t.worktree.as_ref().expect("in its worktree");
            assert_eq!(t.cwd.as_ref(), Some(&wt.path), "stays where it is");
        }
        let in_kyoto: Vec<_> = threads
            .iter()
            .filter(|t| t.worktree.as_ref().unwrap().path == kyoto)
            .collect();
        assert_eq!(in_kyoto.len(), 2);
        assert!(in_kyoto.iter().all(|t| t.native_id.is_some()));
        assert_eq!(in_kyoto[0].worktree.as_ref().unwrap().branch, "feat/login");
        // Each workspace shows, by its newest session; the older one's filed as an import's is.
        // None of it is unread.
        assert_eq!(
            in_kyoto.iter().filter(|t| t.settled_at.is_none()).count(),
            1
        );
        assert!(threads.iter().all(|t| !t.is_unseen()));
        // The one with no session gets a thread to carry on in, by the name Conductor gave it.
        let carry_on = threads
            .iter()
            .find(|t| t.worktree.as_ref().unwrap().path == osaka)
            .unwrap();
        assert_eq!(carry_on.title, "Tidy the nav");
        assert_eq!(carry_on.native_id, None);

        // Not one file in the home folder changed, Conductor's database and git's own included,
        // and the worktree is on the branch it was.
        let after: BTreeSet<_> = snapshot(&home.0).into_iter().collect();
        assert_eq!(after.len(), before.len());
        assert!(after == before, "files changed under {}", home.0.display());
        assert_eq!(
            git(&kyoto, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
            branch_before
        );
        assert!(db.is_file());

        // Nothing's offered again, and a second look doesn't add threads.
        assert!(plans(&store, &home).is_empty());
        assert_eq!(store.threads().unwrap().len(), 3);
    }

    #[test]
    fn an_importer_turned_off_stays_off() {
        let home = Home::new("off");
        let repo = home.repo("acme");
        let wt = home.worktree(&repo, "conductor/workspaces/acme/kyoto", "feat/x");
        home.claude_session("11111111-1111-1111-1111-111111111111", &wt, "Fix the login");
        let store = Store::in_memory().unwrap();
        let off = Import {
            claude_code: false,
            ..all_on()
        };
        let found = scan(&store, &home.0, &Histories::under(&home.0), &off);
        assert_eq!(found[0].counts(), "1 project, 1 worktree, 0 agent sessions");
    }

    #[test]
    fn threads_already_here_are_counted_and_move_under_the_repository() {
        let home = Home::new("moving");
        let repo = home.repo("acme");
        let wt = home.worktree(&repo, "conductor/workspaces/acme/kyoto", "feat/x");
        home.claude_session("11111111-1111-1111-1111-111111111111", &wt, "Fix the login");
        let store = Store::in_memory().unwrap();
        // An import before this one: the worktree became a project of its own.
        let sessions = Histories::under(&home.0).sessions(std::slice::from_ref(&wt), &all_on());
        store.upsert_imported(&sessions).unwrap();
        assert_eq!(store.projects().unwrap()[0].path, wt);

        let found = plans(&store, &home);
        assert_eq!(found[0].moving(), 1);
        let done = adopt(
            &store,
            &found[0],
            AgentId::ClaudeCode,
            Effort::Medium,
            HandHolding::Auto,
        )
        .unwrap();
        assert_eq!((done.moved, done.new_threads), (1, 0));
        let projects = store.projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].path, repo);
        assert_eq!(store.threads().unwrap().len(), 1);
    }
}
