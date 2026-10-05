//! What a finished turn changed: the card under its answer, and what the phone is told. Counted
//! from the git checkpoints Trek takes as turns start (`checkpoint`), so changes made through
//! shell commands count as much as the agent's edits; outside git, from the agent's edit tools.
//! Also the recap Trek asks agents to start such an answer with.

use crate::rewind::{ends_turn, turn_start};
use crate::store::Item;
use std::path::PathBuf;

/// What Trek asks agents (Settings › General › Ask agents for a recap): answers that end work
/// start with what's done, what's left and what turned up, so the outcome isn't buried.
pub const RECAP: &str = "When you finish a turn in which you changed files or ran work, start your final message with a short recap in three bold-labelled parts: **Done** (what's finished), **Still to do** (what's missing or left; \"Nothing\" if nothing is), and **Found** (anything you discovered that the user should know: bugs, risks, surprises; leave this part out if there's nothing). Keep each part to one to three short lines, in your own words, then give any detail. Answers that only explain or discuss need no recap.";

/// What a turn changed, file by file (sorted by path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnChanges {
    pub files: Vec<FileChange>,
    /// The folder the paths are relative to: the repository's top folder (counted from git), or
    /// the thread's folder (from the agent's tools).
    pub root: PathBuf,
    pub counted: Counted,
}

/// Where a turn's changes were counted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counted {
    /// The git checkpoints around the turn: every change, those commands made too.
    Checkpoints,
    /// The agent's edit and write tools (the folder isn't a git repository, or no checkpoints
    /// bracket the turn): changes made by commands aren't in it.
    EditTools,
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Relative to `TurnChanges::root` (where it is now, for a rename); absolute when an agent's
    /// tool changed a file outside it.
    pub path: String,
    pub status: FileStatus,
    pub added: u32,
    pub removed: u32,
    /// A binary file: it has no lines to count (`added` and `removed` are 0).
    pub binary: bool,
    /// `added` and `removed` were counted. False for a binary file, and for an edit whose tool
    /// didn't say (counted from the agent's tools).
    pub lines_known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    /// Changed, or (counted from the agent's tools) edited without saying whether it's new.
    Modified,
    Deleted,
    Renamed { from: String },
}

impl TurnChanges {
    /// Lines added and removed, over every file.
    pub fn totals(&self) -> (u32, u32) {
        self.files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed))
    }
}

/// `git diff-tree -r -z -M --raw --numstat` output, as files sorted by path. The raw records give
/// each file's status (and modes: a nested repository is mode 160000, and is left out, as are
/// files inside one); the numstat records its lines ("-" for a binary file). Both name a
/// rename's old path and its new one.
pub fn parse_diff_stat(out: &str) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = vec![];
    let mut nested: Vec<String> = vec![];
    let mut fields = out.split('\0');
    while let Some(field) = fields.next() {
        if let Some(meta) = field.strip_prefix(':') {
            // "<old mode> <new mode> <old sha> <new sha> <status>"
            let meta: Vec<&str> = meta.split(' ').collect();
            let letter = meta.get(4).and_then(|s| s.chars().next()).unwrap_or('M');
            let from = matches!(letter, 'R' | 'C').then(|| fields.next().unwrap_or_default().to_string());
            let path = fields.next().unwrap_or_default().to_string();
            if meta.iter().take(2).any(|m| *m == "160000") {
                nested.push(path);
                continue;
            }
            let status = match (letter, from) {
                ('R', Some(from)) => FileStatus::Renamed { from },
                // A copy is a new file; where it was copied from is no matter here.
                ('A' | 'C', _) => FileStatus::Added,
                ('D', _) => FileStatus::Deleted,
                _ => FileStatus::Modified,
            };
            files.push(FileChange { path, status, added: 0, removed: 0, binary: false, lines_known: false });
            continue;
        }
        // "<added>\t<removed>\t<path>", or with an empty path and the old and new paths after.
        let mut parts = field.splitn(3, '\t');
        let (Some(added), Some(removed), Some(rest)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let path = if rest.is_empty() {
            let _old = fields.next();
            fields.next().unwrap_or_default().to_string()
        } else {
            rest.to_string()
        };
        let Some(f) = files.iter_mut().find(|f| f.path == path) else { continue };
        match (added.parse::<u32>(), removed.parse::<u32>()) {
            (Ok(a), Ok(r)) => (f.added, f.removed, f.lines_known) = (a, r, true),
            _ => f.binary = true,
        }
    }
    files.retain(|f| !nested.iter().any(|n| f.path == *n || f.path.starts_with(&format!("{n}/"))));
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// Where the files stood as a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// In the checkpoint of the message at this position: the one that started the next turn.
    Message(usize),
    /// As they are now: no turn has followed.
    Now,
}

/// Which checkpoints bound the turn ending at `end` (its `TurnEnd`): the one taken as its first
/// message was sent, and where the files stood as it ended. `None` when checkpoints can't tell:
/// the turn didn't start from a message (the agent took it by itself), or another such turn
/// came between it and the next message, so the next checkpoint holds that one's changes too.
pub fn turn_bounds(items: &[Item], end: usize) -> Option<(usize, End)> {
    let start = turn_start(items, end)?;
    for (ix, item) in items.iter().enumerate().skip(end + 1) {
        match item {
            Item::User { aside: false, .. } => return Some((start, End::Message(ix))),
            i if ends_turn(i) => return None,
            _ => {}
        }
    }
    Some((start, End::Now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ToolStatus;

    fn user(t: &str) -> Item {
        Item::User { text: t.into(), images: vec![], at: None, resume: None, aside: false }
    }
    fn aside(t: &str) -> Item {
        Item::User { text: t.into(), images: vec![], at: None, resume: None, aside: true }
    }
    fn said(t: &str) -> Item {
        Item::Assistant { text: t.into() }
    }
    fn end() -> Item {
        Item::TurnEnd { at: 1, took_secs: 1 }
    }
    fn tool() -> Item {
        Item::Tool { id: "t".into(), title: "Run command".into(), detail: "make".into(), output: String::new(), status: ToolStatus::Done }
    }

    #[test]
    fn a_turn_runs_from_its_first_message_to_the_next_turns() {
        // 0 user, 1 steer, 2 tool, 3 said, 4 end, 5 aside, 6 user, 7 said, 8 end
        let items = [user("go"), user("also this"), tool(), said("done"), end(), aside("/model"), user("next"), said("ok"), end()];
        assert_eq!(turn_bounds(&items, 4), Some((0, End::Message(6))), "a steer is part of the turn; an aside isn't a turn");
        assert_eq!(turn_bounds(&items, 8), Some((6, End::Now)), "the latest turn ends with the files as they are");
    }

    #[test]
    fn turns_checkpoints_cant_bound_have_none() {
        // The agent took a turn by itself (woken by work in the background) after the first.
        let items = [user("go"), said("done"), end(), said("the watcher failed"), end(), user("next"), said("ok"), end()];
        assert_eq!(turn_bounds(&items, 2), None, "the next checkpoint holds the turn between too");
        assert_eq!(turn_bounds(&items, 4), None, "no message started it, so no checkpoint did");
        assert_eq!(turn_bounds(&items, 7), Some((5, End::Now)));
        // An interrupted turn after it ends turns too.
        let items = [user("go"), said("done"), end(), Item::Notice { text: "Interrupted".into() }];
        assert_eq!(turn_bounds(&items, 2), None);
    }

    #[test]
    fn diff_stat_output_is_read_as_files() {
        let raw = |meta: &str, paths: &[&str]| format!(":{meta}\0{}\0", paths.join("\0"));
        let out = [
            raw("100644 100644 aaa bbb M", &["src/a.rs"]),
            raw("000000 100644 000 ccc A", &["new\tname.md"]),
            raw("100644 000000 ddd 000 D", &["gone.txt"]),
            raw("100644 100644 eee eee R087", &["old/x.rs", "new/x.rs"]),
            raw("100644 100644 fff ggg M", &["logo.png"]),
            raw("000000 160000 000 hhh A", &["vendor/lib"]),
            "12\t3\tsrc/a.rs\0".into(),
            "4\t0\tnew\tname.md\0".into(),
            "0\t7\tgone.txt\0".into(),
            "2\t1\t\0old/x.rs\0new/x.rs\0".into(),
            "-\t-\tlogo.png\0".into(),
            "1\t0\tvendor/lib\0".into(),
        ]
        .concat();
        let file = |path: &str, status: FileStatus, added, removed| FileChange { path: path.into(), status, added, removed, binary: false, lines_known: true };
        assert_eq!(
            parse_diff_stat(&out),
            [
                file("gone.txt", FileStatus::Deleted, 0, 7),
                FileChange { binary: true, lines_known: false, ..file("logo.png", FileStatus::Modified, 0, 0) },
                file("new\tname.md", FileStatus::Added, 4, 0),
                file("new/x.rs", FileStatus::Renamed { from: "old/x.rs".into() }, 2, 1),
                file("src/a.rs", FileStatus::Modified, 12, 3),
            ],
            "sorted by path; a nested repository is left out"
        );
        assert!(parse_diff_stat("").is_empty());
        let changes = TurnChanges { files: parse_diff_stat(&out), root: "/p".into(), counted: Counted::Checkpoints };
        assert_eq!(changes.totals(), (18, 11));
    }
}
