//! What each finished turn changed (`trek_core::changes::TurnChanges`), for the card under its
//! answer (`changes_card`) and the phone. Counted from the git checkpoints Trek takes as turns
//! start, so changes made through shell commands count as much as the agent's edits: a turn runs
//! from its own checkpoint to the next turn's, or to the files as they are now for the latest.
//! In a folder outside git (or a turn no checkpoints bracket) it's counted from the agent's edit
//! tools instead.
//!
//! The git work runs off the main thread, once per turn: results are kept by (thread, turn) and
//! worked out again only when they may have moved: the latest turn's when another finishes or
//! files are put back, all of a thread's on a rewind.

use super::{Workspace, WorkspaceEvent, in_repo, worktree_missing};
use crate::activity::{ToolKind, tool_kind};
use gpui_kit::Context;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use trek_core::changes::{Counted, End, FileChange, FileStatus, TurnChanges, turn_bounds};
use trek_core::checkpoint::Repo;
use trek_core::rewind::ends_turn;
use trek_core::store::{Item, ToolStatus};

/// The two snapshots a turn's changes were counted between, for its diff in the Git tool.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnRange {
    /// The repository's top folder.
    pub repo: PathBuf,
    /// The checkpoint taken as the turn started.
    pub from: String,
    /// The next turn's checkpoint, or the tree the files made when they were counted.
    pub to: String,
}

/// One turn's entry: what its card shows, and whether it's being worked out.
struct Slot {
    shown: Option<Arc<TurnChanges>>,
    range: Option<TurnRange>,
    state: State,
    /// What a waiting slot waited on when it was last asked (`waits_on`): nothing of that
    /// changed, the answer hasn't either.
    waited: Option<WaitsOn>,
}

/// What decides whether a turn's count still has to wait: the turn running after it, the git
/// work under way, the messages that followed and the checkpoints taken or failed.
type WaitsOn = (bool, bool, usize, usize, usize, usize);

#[derive(Clone, Copy, PartialEq)]
enum State {
    /// `now`: counted against the files as they are, not a later checkpoint.
    Ready { now: bool },
    /// Being worked out (`run` tells this one from an older one that was dropped).
    Working { run: u64, now: bool },
    /// The checkpoint that ends it is still being taken, or a turn after it runs that no
    /// message started: worked out once that's over (asked again each time until then).
    Waiting,
    /// May have moved: worked out again when next asked for, the old count showing meanwhile.
    Stale,
}

impl State {
    /// Whether what it shows may move as the files or the transcript do.
    fn moves(self) -> bool {
        matches!(self, State::Ready { now: true } | State::Working { now: true, .. } | State::Waiting)
    }
}

/// Turns' changes by (thread, its `TurnEnd` item id).
#[derive(Default)]
pub struct Cache {
    slots: HashMap<(String, String), Slot>,
    run: u64,
}

/// How a turn's changes are to be counted.
enum Plan {
    Git { repo: PathBuf, from: String, to: Option<String> },
    Wait,
    Tools,
}

impl Workspace {
    /// What the turn ending at `turn_end_item_index` (its `TurnEnd`) of `thread_id` changed, as
    /// last worked out: `None` when it changed nothing, or before it has been worked out (see
    /// `load_turn_changes`). While it's being worked out again the previous count stays.
    ///
    /// For the phone bridge: call `load_turn_changes` first, then read this once
    /// `WorkspaceEvent::TurnChanges` names the turn.
    pub fn turn_changes(&self, thread_id: &str, turn_end_item_index: usize) -> Option<TurnChanges> {
        let end = self.live.get(thread_id)?.items.id_at(turn_end_item_index)?;
        self.changes_cache.slots.get(&(thread_id.to_string(), end.to_string()))?.shown.as_deref().cloned()
    }

    /// `turn_changes`, having the count worked out (off the main thread) if it isn't yet or may
    /// have moved; `WorkspaceEvent::TurnChanges` says when it's in.
    pub fn load_turn_changes(&mut self, id: &str, end: usize, cx: &mut Context<Self>) -> Option<Arc<TurnChanges>> {
        let live = self.live.get(id)?;
        if !matches!(live.items.get(end), Some(Item::TurnEnd { .. })) {
            return None;
        }
        let key = (id.to_string(), live.items.id_at(end)?.to_string());
        let waits_on: WaitsOn = (live.turn_started.is_some(), live.git_busy, live.git_jobs.len(), live.items.len(), live.checkpointed.len(), live.checkpoint_failed.len());
        let shown = match self.changes_cache.slots.get(&key) {
            // Waiting is asked again each time what it waits on has moved (this runs as often as
            // its row is drawn, and asking reads the disk and the database).
            Some(Slot { shown, state: State::Waiting, waited, .. }) if *waited == Some(waits_on) => return shown.clone(),
            Some(Slot { shown, state: State::Stale | State::Waiting, .. }) => shown.clone(),
            Some(slot) => return slot.shown.clone(),
            None => None,
        };
        match self.plan(id, end) {
            Plan::Wait => {
                self.changes_cache.slots.insert(key, Slot { shown: shown.clone(), range: None, state: State::Waiting, waited: Some(waits_on) });
                shown
            }
            Plan::Tools => {
                let changes = self.from_tools(id, end).map(Arc::new);
                self.changes_cache.slots.insert(key, Slot { shown: changes.clone(), range: None, state: State::Ready { now: false }, waited: None });
                changes
            }
            Plan::Git { repo, from, to } => {
                self.changes_cache.run += 1;
                let run = self.changes_cache.run;
                let now = to.is_none();
                let range = self.changes_cache.slots.get(&key).and_then(|s| s.range.clone());
                self.changes_cache.slots.insert(key.clone(), Slot { shown: shown.clone(), range, state: State::Working { run, now }, waited: None });
                let task = cx.spawn(async move |this, cx| {
                    let counted = cx
                        .background_executor()
                        .spawn(async move {
                            let r = Repo::find(&repo).ok_or_else(|| anyhow::anyhow!("{} isn't a git repository any more", repo.display()))?;
                            let to = match to {
                                Some(sha) => sha,
                                None => r.tree_now()?,
                            };
                            let files = r.diff_stat(&from, &to)?;
                            Ok::<_, anyhow::Error>((files, TurnRange { repo: r.top, from, to }))
                        })
                        .await;
                    let _ = this.update(cx, |ws, cx| {
                        if !ws.changes_cache.slots.get(&key).is_some_and(|s| s.state == State::Working { run, now }) {
                            return;
                        }
                        let (shown, range) = match counted {
                            Ok((files, range)) => {
                                let changes = (!files.is_empty()).then(|| Arc::new(TurnChanges { files, root: range.repo.clone(), counted: Counted::Checkpoints }));
                                (changes, Some(range))
                            }
                            Err(e) => {
                                tracing::warn!("changes of a turn in {}: {e:#}", key.0);
                                (ws.live.get(&key.0).and_then(|l| l.items.position(&key.1)).and_then(|end| ws.from_tools(&key.0, end)).map(Arc::new), None)
                            }
                        };
                        if let Some(slot) = ws.changes_cache.slots.get_mut(&key) {
                            *slot = Slot { shown, range, state: State::Ready { now }, waited: None };
                        }
                        ws.remote_turn_changes_moved(&key.0);
                        cx.emit(WorkspaceEvent::TurnChanges { id: key.0.clone(), end: key.1.clone() });
                    });
                });
                self.keep(task);
                shown
            }
        }
    }

    /// The snapshots the turn ending at item `end` (by id) of `id` was counted between, and
    /// what it changed: what the Git tool shows for it. `None` unless it was counted from git.
    pub fn turn_range(&self, id: &str, end: &str) -> Option<(TurnRange, Arc<TurnChanges>)> {
        let slot = self.changes_cache.slots.get(&(id.to_string(), end.to_string()))?;
        Some((slot.range.clone()?, slot.shown.clone()?))
    }

    /// Whether the count of the turn ending at `end` of `id` waits on something that may finish
    /// without a word (a checkpoint being taken, a turn the agent took by itself): asked again
    /// until it doesn't.
    pub fn turn_changes_waiting(&self, id: &str, end: usize) -> bool {
        let key = self.live.get(id).and_then(|l| l.items.id_at(end)).map(|e| (id.to_string(), e.to_string()));
        key.and_then(|k| self.changes_cache.slots.get(&k)).is_some_and(|s| matches!(s.state, State::Waiting | State::Stale))
    }

    /// Whether the turn ending at `end` of `id` has been worked out and isn't being again.
    #[cfg(test)]
    pub fn turn_changes_settled(&self, id: &str, end: usize) -> bool {
        let key = self.live.get(id).and_then(|l| l.items.id_at(end)).map(|e| (id.to_string(), e.to_string()));
        key.and_then(|k| self.changes_cache.slots.get(&k)).is_some_and(|s| matches!(s.state, State::Ready { .. }))
    }

    /// How the changes of the turn ending at `end` of `id` can be counted.
    fn plan(&self, id: &str, end: usize) -> Plan {
        let (Some(live), Some(thread)) = (self.live.get(id), self.thread(id)) else { return Plan::Tools };
        if worktree_missing(thread) || !in_repo(thread.cwd.as_deref()) {
            return Plan::Tools;
        }
        let Some((start, until)) = turn_bounds(&live.items, end) else {
            // A turn the agent took by itself is running after it: once that's over, there's
            // no telling them apart.
            return if live.turn_started.is_some() && !live.items[end + 1..].iter().any(|i| matches!(i, Item::User { aside: false, .. })) { Plan::Wait } else { Plan::Tools };
        };
        let checkpoint = |pos: usize| {
            let item = live.items.id_at(pos)?;
            live.checkpointed.contains(item).then(|| self.store.checkpoint(id, item).ok().flatten()).flatten()
        };
        let Some(from) = checkpoint(start) else { return Plan::Tools };
        let to = match until {
            // A turn the agent took by itself is changing the files now.
            End::Now if live.turn_started.is_some() => return Plan::Wait,
            End::Now => None,
            End::Message(pos) => match checkpoint(pos) {
                Some(c) if c.repo == from.repo => Some(c.sha),
                Some(_) => return Plan::Tools,
                None => {
                    let failed = live.items.id_at(pos).is_some_and(|i| live.checkpoint_failed.contains_key(i));
                    if !failed && (live.git_busy || !live.git_jobs.is_empty()) {
                        return Plan::Wait;
                    }
                    return Plan::Tools;
                }
            },
        };
        Plan::Git { repo: from.repo, from: from.sha, to }
    }

    /// The turns of `id` whose changes may have moved are worked out again when next asked
    /// for: the latest turn's (counted against the files as they are) and any waiting, or with
    /// `all` every one (a rewind; turns it took away are dropped). Views showing them hear of it.
    pub(super) fn forget_turn_changes(&mut self, id: &str, all: bool, cx: &mut Context<Self>) {
        let live = self.live.get(id);
        let mut moved = vec![];
        self.changes_cache.slots.retain(|(thread, end), slot| {
            if thread != id || !(all || slot.state.moves()) {
                return true;
            }
            moved.push(end.clone());
            slot.state = State::Stale;
            !all || live.is_some_and(|l| l.items.position(end).is_some())
        });
        if !moved.is_empty() {
            self.remote_turn_changes_moved(id);
        }
        for end in moved {
            cx.emit(WorkspaceEvent::TurnChanges { id: id.to_string(), end });
        }
    }

    /// What the turn ending at `end` of `id` changed through the agent's edit tools, with the
    /// lines each call reported; `None` when it changed nothing that way.
    fn from_tools(&self, id: &str, end: usize) -> Option<TurnChanges> {
        let live = self.live.get(id)?;
        let cwd = self.thread(id).and_then(|t| t.cwd.clone());
        let files = from_tools(&live.items, end, &live.lines, cwd.as_deref());
        (!files.is_empty()).then(|| TurnChanges { files, root: cwd.unwrap_or_default(), counted: Counted::EditTools })
    }
}

/// The items of the turn ending at `end`: everything after the previous turn's end.
fn turn_items(items: &[Item], end: usize) -> &[Item] {
    let end = end.min(items.len());
    let from = items[..end].iter().rposition(ends_turn).map_or(0, |b| b + 1);
    &items[from..end]
}

/// `path` relative to `cwd` when it's inside it (agents may report `/tmp` as `/private/tmp`).
fn inside(path: &str, cwd: Option<&Path>) -> String {
    let p = Path::new(path);
    let rest = cwd.and_then(|c| p.strip_prefix(c).ok().or_else(|| p.strip_prefix(Path::new("/private").join(c.strip_prefix("/").ok()?)).ok()));
    match rest.filter(|r| !r.as_os_str().is_empty()) {
        Some(r) => r.display().to_string(),
        None => path.to_string(),
    }
}

/// The files the turn ending at `end` changed through the agent's edit tools, with the lines
/// each call reported (`lines`, by call), sorted by path. A call naming several files (Codex)
/// reports lines for all of them together, which can't be split between them; nor are a file's
/// lines known if any of its calls didn't say.
fn from_tools(items: &[Item], end: usize, lines: &HashMap<String, (u32, u32)>, cwd: Option<&Path>) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = vec![];
    for item in turn_items(items, end) {
        let Item::Tool { id, title, detail, status, .. } = item else { continue };
        let deleted = title == "Delete";
        if matches!(status, ToolStatus::Failed | ToolStatus::Denied) || (tool_kind(title) != ToolKind::Edit && !deleted) {
            continue;
        }
        let paths: Vec<&str> = detail.split(", ").map(str::trim).filter(|p| !p.is_empty()).collect();
        let counted = if paths.len() == 1 { lines.get(id).copied() } else { None };
        for p in paths {
            let path = inside(p, cwd);
            let i = match files.iter().position(|f| f.path == path) {
                Some(i) => i,
                None => {
                    files.push(FileChange { path, status: FileStatus::Modified, added: 0, removed: 0, binary: false, lines_known: true });
                    files.len() - 1
                }
            };
            let f = &mut files[i];
            match counted {
                Some((a, r)) => (f.added, f.removed) = (f.added + a, f.removed + r),
                None if !deleted => f.lines_known = false,
                None => {}
            }
            if deleted {
                f.status = FileStatus::Deleted;
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, title: &str, detail: &str, status: ToolStatus) -> Item {
        Item::Tool { id: id.into(), title: title.into(), detail: detail.into(), output: String::new(), status }
    }

    #[test]
    fn without_git_the_agents_edits_are_counted() {
        let items = vec![
            Item::User { text: "earlier".into(), images: vec![], at: None, resume: None, aside: false },
            tool("old", "Edit", "/p/early.rs", ToolStatus::Done),
            Item::TurnEnd { at: 1, took_secs: 1 },
            Item::User { text: "go".into(), images: vec![], at: None, resume: None, aside: false },
            tool("e1", "Edit", "/p/src/a.rs", ToolStatus::Done),
            tool("r1", "Read", "/p/src/b.rs", ToolStatus::Done),
            tool("e2", "Edit", "/p/src/a.rs", ToolStatus::Done),
            tool("w1", "Write", "/p/notes.md", ToolStatus::Done),
            tool("x1", "Edit", "/p/denied.rs", ToolStatus::Denied),
            tool("c1", "Run command", "rm old.txt", ToolStatus::Done),
            tool("m1", "Edit", "/p/x.rs, /p/y.rs", ToolStatus::Done),
            tool("d1", "Delete", "/p/gone.rs", ToolStatus::Done),
            tool("o1", "Edit", "/elsewhere/z.rs", ToolStatus::Done),
            Item::Assistant { text: "done".into() },
            Item::TurnEnd { at: 2, took_secs: 252 },
        ];
        let lines = HashMap::from([
            ("e1".to_string(), (10, 2)),
            ("e2".to_string(), (3, 1)),
            ("w1".to_string(), (40, 0)),
            ("m1".to_string(), (5, 5)),
            ("o1".to_string(), (1, 0)),
            ("old".to_string(), (99, 0)),
        ]);
        let files = from_tools(&items, 14, &lines, Some(Path::new("/p")));
        let got: Vec<(&str, &FileStatus, u32, u32, bool)> = files.iter().map(|f| (f.path.as_str(), &f.status, f.added, f.removed, f.lines_known)).collect();
        assert_eq!(
            got,
            [
                ("/elsewhere/z.rs", &FileStatus::Modified, 1, 0, true),
                ("gone.rs", &FileStatus::Deleted, 0, 0, true),
                ("notes.md", &FileStatus::Modified, 40, 0, true),
                ("src/a.rs", &FileStatus::Modified, 13, 3, true),
                ("x.rs", &FileStatus::Modified, 0, 0, false),
                ("y.rs", &FileStatus::Modified, 0, 0, false),
            ],
            "the turn's own calls, by file; a denied call changed nothing; one outside the folder keeps its path"
        );
    }
}
