//! One file's unified diff, cut into hunks: what a review's editor shows line by line, and what
//! Keep and Undo take one hunk at a time (`checkpoint::Repo::keep_hunk`, `undo_hunk`). A hunk
//! goes back to git as a patch of its own: the file's header, then just that hunk.

/// A file's diff: its header (`diff --git`, mode lines, `---`, `+++`) and its hunks in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub header: String,
    pub hunks: Vec<Hunk>,
}

/// One `@@` hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Where it starts in the old and the new file (1-based, as the `@@` line says; 0 for an
    /// empty side), and how many lines each side has.
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// The `@@` line and the lines under it, each ending in a newline.
    pub body: String,
}

/// What a hunk does to the new file's rows (0-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// Rows `start..end` are new or changed.
    Added { start: u32, end: u32 },
    /// Lines went away just above row `at` (`at` may be one past the last row).
    Deleted { at: u32, lines: u32 },
}

impl FileDiff {
    /// Parse `git diff` output for one file; empty (no hunks) when it's empty or binary.
    pub fn parse(patch: &str) -> FileDiff {
        let mut out = FileDiff::default();
        let mut current: Option<Hunk> = None;
        for line in patch.split_inclusive('\n') {
            if line.starts_with("@@ ") {
                out.hunks.extend(current.take());
                let Some((os, oc, ns, nc)) = parse_range(line) else { continue };
                current = Some(Hunk { old_start: os, old_count: oc, new_start: ns, new_count: nc, body: ensure_newline(line) });
            } else if let Some(h) = current.as_mut() {
                if line.starts_with([' ', '+', '-', '\\']) || line == "\n" {
                    h.body.push_str(&ensure_newline(line));
                } else {
                    // Another file's header: this parser takes one file.
                    out.hunks.extend(current.take());
                    break;
                }
            } else {
                out.header.push_str(&ensure_newline(line));
            }
        }
        out.hunks.extend(current);
        out
    }

    /// The patch of hunk `ix` alone, to hand to `git apply`.
    pub fn patch(&self, ix: usize) -> Option<String> {
        self.hunks.get(ix).map(|h| format!("{}{}", self.header, h.body))
    }

    /// Every hunk's marks, in row order.
    pub fn marks(&self) -> Vec<Mark> {
        self.hunks.iter().flat_map(Hunk::marks).collect()
    }
}

impl Hunk {
    /// Rows of the new file this hunk added or changed, and where it took lines away. A run of
    /// lines taken away with others put in their place reads as changed.
    pub fn marks(&self) -> Vec<Mark> {
        let mut out = vec![];
        // The new file's row of the line being read (0-based). A pure deletion's `+start` names
        // the line before the gap.
        let mut row = if self.new_count == 0 { self.new_start } else { self.new_start.saturating_sub(1) };
        // The run of changed lines under way: where its additions start, and lines taken away.
        let mut added: Option<u32> = None;
        let mut removed = 0u32;
        let end_run = |out: &mut Vec<Mark>, added: &mut Option<u32>, removed: &mut u32, row: u32| {
            match added.take() {
                Some(start) => out.push(Mark::Added { start, end: row }),
                None if *removed > 0 => out.push(Mark::Deleted { at: row, lines: *removed }),
                None => {}
            }
            *removed = 0;
        };
        for line in self.body.lines().skip(1) {
            match line.as_bytes().first() {
                Some(b'+') => {
                    added.get_or_insert(row);
                    row += 1;
                }
                Some(b'-') => removed += 1,
                Some(b'\\') => {}
                _ => {
                    end_run(&mut out, &mut added, &mut removed, row);
                    row += 1;
                }
            }
        }
        end_run(&mut out, &mut added, &mut removed, row);
        out
    }

    /// The first row of the new file the hunk touches (0-based), where its bar sits.
    pub fn first_row(&self) -> u32 {
        match self.marks().first() {
            Some(Mark::Added { start, .. }) => *start,
            Some(Mark::Deleted { at, .. }) => *at,
            None => self.new_start.saturating_sub(1),
        }
    }

    /// The last row it touches (0-based, inclusive).
    pub fn last_row(&self) -> u32 {
        match self.marks().last() {
            Some(Mark::Added { end, .. }) => end.saturating_sub(1),
            Some(Mark::Deleted { at, .. }) => *at,
            None => self.first_row(),
        }
    }

    /// Lines added and removed.
    pub fn counts(&self) -> (u32, u32) {
        self.body.lines().skip(1).fold((0, 0), |(a, r), l| match l.as_bytes().first() {
            Some(b'+') => (a + 1, r),
            Some(b'-') => (a, r + 1),
            _ => (a, r),
        })
    }

    /// The lines it took away, as they were.
    pub fn removed_text(&self) -> String {
        self.body.lines().skip(1).filter_map(|l| l.strip_prefix('-')).map(|l| format!("{l}\n")).collect()
    }
}

fn ensure_newline(line: &str) -> String {
    if line.ends_with('\n') { line.to_string() } else { format!("{line}\n") }
}

/// `@@ -a[,b] +c[,d] @@` → (a, b, c, d); a missing count is 1.
fn parse_range(line: &str) -> Option<(u32, u32, u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(' ')?;
    let new = rest.strip_prefix('+')?.split_once(' ')?.0;
    let side = |s: &str| -> Option<(u32, u32)> {
        let mut it = s.split(',');
        let start = it.next()?.parse().ok()?;
        let count = it.next().map_or(Some(1), |c| c.parse().ok())?;
        Some((start, count))
    };
    let (os, oc) = side(old)?;
    let (ns, nc) = side(new)?;
    Some((os, oc, ns, nc))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/f.txt b/f.txt\nindex 1111111..2222222 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,4 +1,4 @@\n-one\n+ONE\n two\n three\n four\n@@ -8,3 +8,5 @@ fn x\n eight\n nine\n+nine and a half\n+nine and three quarters\n ten\n@@ -12,3 +14,2 @@\n twelve\n-thirteen\n fourteen\n";

    #[test]
    fn a_diff_splits_into_hunks_that_patch_on_their_own() {
        let d = FileDiff::parse(PATCH);
        assert_eq!(d.header, "diff --git a/f.txt b/f.txt\nindex 1111111..2222222 100644\n--- a/f.txt\n+++ b/f.txt\n");
        assert_eq!(d.hunks.len(), 3);
        assert_eq!((d.hunks[1].old_start, d.hunks[1].old_count, d.hunks[1].new_start, d.hunks[1].new_count), (8, 3, 8, 5));
        let p = d.patch(1).unwrap();
        assert!(p.starts_with("diff --git a/f.txt b/f.txt\n") && p.ends_with("+nine and three quarters\n ten\n"), "{p}");
        assert!(!p.contains("thirteen"), "one hunk only: {p}");
        assert_eq!(d.hunks[1].counts(), (2, 0));
        assert_eq!(d.hunks[2].removed_text(), "thirteen\n");
    }

    #[test]
    fn hunks_mark_the_rows_they_add_change_and_take_away() {
        let d = FileDiff::parse(PATCH);
        assert_eq!(d.hunks[0].marks(), [Mark::Added { start: 0, end: 1 }], "a changed line");
        assert_eq!(d.hunks[1].marks(), [Mark::Added { start: 9, end: 11 }]);
        assert_eq!(d.hunks[2].marks(), [Mark::Deleted { at: 14, lines: 1 }], "the gap sits above `fourteen`");
        assert_eq!((d.hunks[1].first_row(), d.hunks[1].last_row()), (9, 10));
        // A zero-context deletion at the end of the file, and a new file.
        let gone = FileDiff::parse("--- a/x\n+++ b/x\n@@ -3,2 +2,0 @@\n-c\n-d\n");
        assert_eq!(gone.marks(), [Mark::Deleted { at: 2, lines: 2 }]);
        let new = FileDiff::parse("--- /dev/null\n+++ b/x\n@@ -0,0 +1,2 @@\n+a\n+b\n");
        assert_eq!(new.marks(), [Mark::Added { start: 0, end: 2 }]);
        assert!(FileDiff::parse("").hunks.is_empty());
    }
}
