//! Worktrees another app made and keeps (`import::worktree_apps`), which threads here work in.
//! Each stays where it is and stays that app's: the row says whose it is, so Trek never removes
//! one, and sessions found in it later go to its repository's project rather than a project of
//! their own.

use super::{Store, Thread, lock, now_ms, project_in};
use crate::worktree::Worktree;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS adopted_worktrees (
  path TEXT PRIMARY KEY, real TEXT NOT NULL, app TEXT NOT NULL, project TEXT NOT NULL,
  branch TEXT NOT NULL, base TEXT NOT NULL, at INTEGER NOT NULL
);";

/// A worktree another app keeps, as Trek brought it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptedWorktree {
    /// The folder, as git and the app write it.
    pub path: PathBuf,
    /// Which app's it is (`WorktreeApp::key`).
    pub app: String,
    /// The repository's main checkout: the thread's project.
    pub project: PathBuf,
    pub branch: String,
    pub base: String,
}

impl AdoptedWorktree {
    pub fn worktree(&self) -> Worktree {
        Worktree {
            path: self.path.clone(),
            branch: self.branch.clone(),
            base: self.base.clone(),
        }
    }
}

/// A folder with its links resolved: agents record `cwd` as they found it, and `/tmp`, a renamed
/// workspace's link and the like are other spellings of the same folder.
fn real_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn read_all(c: &Connection) -> rusqlite::Result<Vec<(AdoptedWorktree, PathBuf)>> {
    let mut st =
        c.prepare("SELECT path, real, app, project, branch, base FROM adopted_worktrees")?;
    st.query_map([], |r| {
        Ok((
            AdoptedWorktree {
                path: PathBuf::from(r.get::<_, String>(0)?),
                app: r.get(2)?,
                project: PathBuf::from(r.get::<_, String>(3)?),
                branch: r.get(4)?,
                base: r.get(5)?,
            },
            PathBuf::from(r.get::<_, String>(1)?),
        ))
    })?
    .collect()
}

/// The adopted worktree `cwd` is in, if any.
pub(super) fn holding(c: &Connection, cwd: &Path) -> rusqlite::Result<Option<AdoptedWorktree>> {
    let all = read_all(c)?;
    if all.is_empty() {
        return Ok(None);
    }
    let real = real_path(cwd);
    Ok(all.into_iter().find_map(|(w, w_real)| {
        (cwd.starts_with(&w.path) || cwd.starts_with(&w_real) || real.starts_with(&w_real))
            .then_some(w)
    }))
}

impl Store {
    pub fn adopted_worktrees(&self) -> Result<Vec<AdoptedWorktree>> {
        self.with(|c| Ok(read_all(c)?.into_iter().map(|(w, _)| w).collect()))
    }

    /// How many threads already in Trek work in `path` and aren't filed under any worktree: the
    /// ones [`adopt_worktree`](Store::adopt_worktree) would move.
    pub fn threads_to_adopt(&self, path: &Path) -> Result<usize> {
        self.with(|c| Ok(unfiled_in(c, path)?.len()))
    }

    /// Record `w` as adopted, and file the threads already working in it under its project (an
    /// earlier import made them a project per folder), leaving no empty project behind. Nothing on
    /// disk changes. Says how many threads moved.
    pub fn adopt_worktree(&self, w: &AdoptedWorktree) -> Result<usize> {
        let mut conn = lock(&self.conn);
        let tx = conn.transaction()?;
        let project = project_in(&tx, &w.project)?;
        tx.execute(
            "INSERT INTO adopted_worktrees (path, real, app, project, branch, base, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path) DO UPDATE SET real = excluded.real, app = excluded.app,
               project = excluded.project, branch = excluded.branch, base = excluded.base",
            params![
                w.path.display().to_string(),
                real_path(&w.path).display().to_string(),
                w.app,
                project.path.display().to_string(),
                w.branch,
                w.base,
                now_ms(),
            ],
        )?;
        let unfiled = unfiled_in(&tx, &w.path)?;
        let moved = unfiled.len();
        let mut emptied: Vec<String> = vec![];
        for mut t in unfiled {
            if let Some(old) = t.project_id.take().filter(|old| *old != project.id) {
                emptied.push(old);
            }
            t.project_id = Some(project.id.clone());
            t.worktree = Some(w.worktree());
            t.branch = Some(w.branch.clone());
            Self::save_thread_in(&tx, &t)?;
        }
        // The folder's own project, once nothing's in it: the sidebar would list a project per
        // workspace otherwise. One the user has other threads in stays.
        let own = w.path.display().to_string();
        for old in emptied {
            tx.execute(
                "DELETE FROM projects WHERE id = ?1 AND path = ?2
                 AND NOT EXISTS (SELECT 1 FROM threads WHERE project_id = ?1)",
                params![old, own],
            )?;
        }
        tx.commit()?;
        Ok(moved)
    }

    /// The newest thread in the worktree at `path` joins the inbox: a workspace in another app is
    /// work in progress, unlike the history an import finds. It counts as read, and as seen just
    /// now, so it has its full time before it settles again.
    pub fn surface_worktree(&self, path: &Path) -> Result<()> {
        let id: Option<String> = self.with(|c| {
            c.query_row(
                "SELECT id FROM threads WHERE worktree_path = ?1 AND archived_at IS NULL
                 AND parent_id IS NULL AND side_of IS NULL ORDER BY updated_at DESC LIMIT 1",
                [path.display().to_string()],
                |r| r.get(0),
            )
            .optional()
        })?;
        if let Some(id) = id {
            self.update_thread(&id, |t| {
                t.settled_at = None;
                t.last_seen_at = now_ms().max(t.updated_at);
            })?;
        }
        Ok(())
    }

    /// Threads filed under `path`, archived ones too.
    pub fn threads_in_worktree(&self, path: &Path) -> Result<usize> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM threads WHERE worktree_path = ?1",
                [path.display().to_string()],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as usize)
        })
    }
}

/// Threads whose folder is in `path` that have no worktree of their own on record. Only threads
/// beside it are looked at closely: resolving every thread's folder for every worktree, with the
/// store locked, would hold up everything else that reads it. A link to a worktree (Conductor's
/// renamed workspaces) sits next to it.
fn unfiled_in(c: &Connection, path: &Path) -> rusqlite::Result<Vec<Thread>> {
    let real = real_path(path);
    let beside = |p: &Path| p.parent().unwrap_or(p).display().to_string();
    let mut st = c.prepare(&format!(
        "SELECT {} FROM threads WHERE worktree_path IS NULL AND cwd IS NOT NULL
         AND (substr(cwd, 1, length(?1)) = ?1 OR substr(cwd, 1, length(?2)) = ?2)",
        Store::THREAD_COLS
    ))?;
    let rows = st.query_map(params![beside(path), beside(&real)], Store::row_to_thread)?;
    let mut out = vec![];
    for t in rows {
        let t = t?;
        let inside = t.cwd.as_deref().is_some_and(|cwd| {
            cwd.starts_with(path) || cwd.starts_with(&real) || real_path(cwd).starts_with(&real)
        });
        if inside {
            out.push(t);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::ImportedThread;
    use crate::types::{AgentId, Effort, HandHolding, ThreadSource};

    fn adopted(path: &str, project: &str) -> AdoptedWorktree {
        AdoptedWorktree {
            path: path.into(),
            app: "conductor".into(),
            project: project.into(),
            branch: "feat/login".into(),
            base: "main".into(),
        }
    }

    fn session(id: &str, cwd: &Path) -> ImportedThread {
        ImportedThread {
            source: ThreadSource::ClaudeCode,
            native_id: id.into(),
            title: "Fix the login".into(),
            cwd: Some(cwd.to_path_buf()),
            branch: None,
            model: None,
            effort: None,
            created_at: 1,
            updated_at: 2,
            additions: 0,
            deletions: 0,
            skip: None,
            legacy_title: None,
        }
    }

    /// A folder for these tests that exists, spelled as the system gives it back.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trek-adopted-{name}-{}",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        real_path(&dir)
    }

    #[test]
    fn a_session_found_in_an_adopted_worktree_joins_its_project() {
        let s = Store::in_memory().unwrap();
        let root = scratch("join");
        let (repo, wt) = (root.join("acme"), root.join("kyoto"));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(wt.join("sub")).unwrap();
        s.adopt_worktree(&adopted(wt.to_str().unwrap(), repo.to_str().unwrap()))
            .unwrap();
        // One in a folder inside it counts as in it too.
        s.upsert_imported(&[session("a", &wt), session("b", &wt.join("sub"))])
            .unwrap();
        let projects = s.projects().unwrap();
        assert_eq!(projects.len(), 1, "{projects:?}");
        assert_eq!(projects[0].path, repo);
        let threads = s.threads().unwrap();
        assert_eq!(threads.len(), 2);
        for t in threads {
            assert_eq!(t.project_id.as_ref(), Some(&projects[0].id));
            let got = t.worktree.expect("filed under its worktree");
            assert_eq!(got.path, wt);
            assert_eq!(got.branch, "feat/login");
        }
    }

    #[test]
    fn threads_an_earlier_import_made_a_project_of_move_and_its_project_goes() {
        let s = Store::in_memory().unwrap();
        let root = scratch("move");
        let (repo, wt) = (root.join("acme"), root.join("kyoto"));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // Git is what makes it a project root as an import saw it.
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: somewhere").unwrap();
        s.upsert_imported(&[session("a", &wt)]).unwrap();
        assert_eq!(s.projects().unwrap()[0].path, wt);
        assert_eq!(s.threads_to_adopt(&wt).unwrap(), 1);

        let moved = s
            .adopt_worktree(&adopted(wt.to_str().unwrap(), repo.to_str().unwrap()))
            .unwrap();
        assert_eq!(moved, 1);
        let projects = s.projects().unwrap();
        assert_eq!(projects.len(), 1, "the folder's own project goes");
        assert_eq!(projects[0].path, repo);
        let t = s.threads().unwrap().remove(0);
        assert_eq!(t.project_id.as_ref(), Some(&projects[0].id));
        assert_eq!(t.worktree.unwrap().path, wt);
        assert_eq!(s.threads_to_adopt(&wt).unwrap(), 0);
        assert_eq!(s.threads_in_worktree(&wt).unwrap(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_worktree_counts_as_the_worktree() {
        let s = Store::in_memory().unwrap();
        let root = scratch("link");
        let (repo, wt, link) = (root.join("acme"), root.join("kyoto"), root.join("renamed"));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: somewhere").unwrap();
        std::os::unix::fs::symlink(&wt, &link).unwrap();
        // Imported through the link before: moves.
        s.upsert_imported(&[session("old", &link)]).unwrap();
        assert_eq!(s.threads_to_adopt(&wt).unwrap(), 1);
        s.adopt_worktree(&adopted(wt.to_str().unwrap(), repo.to_str().unwrap()))
            .unwrap();
        // Found through it later: joins.
        s.upsert_imported(&[session("new", &link)]).unwrap();
        for t in s.threads().unwrap() {
            assert_eq!(t.worktree.expect("in the worktree").path, wt);
        }
    }

    #[test]
    fn a_folder_merely_named_like_a_worktree_isnt_in_it() {
        let s = Store::in_memory().unwrap();
        let root = scratch("prefix");
        let (repo, wt, other) = (root.join("acme"), root.join("kyoto"), root.join("kyoto-2"));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let mut t = s
            .create_thread(
                Some(&other),
                AgentId::ClaudeCode,
                None,
                Effort::Medium,
                HandHolding::Auto,
            )
            .unwrap();
        t.title = "Elsewhere".into();
        s.save_thread(&t).unwrap();
        assert_eq!(
            s.adopt_worktree(&adopted(wt.to_str().unwrap(), repo.to_str().unwrap()))
                .unwrap(),
            0
        );
        assert_eq!(s.threads().unwrap()[0].worktree, None);
    }
}
