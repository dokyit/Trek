//! Notes: things to jot down next to the agents' work. Each note is a markdown file in the data
//! folder's `notes/`, titled by its first line, so they're plain files anywhere else too. Bold,
//! italics, lists, checklists and headings are markdown; colour and highlights are inline
//! `<span style="color: …">` and `<mark>`, which Trek's markdown renders.
//!
//! The editing helpers below work on the markdown text and a selection in it (byte offsets), and
//! hand back the new text and selection: wrapping a selection in marks (or taking them off again),
//! turning lines into a list, a checklist, a heading or a quote (or back), and carrying a list on
//! when Return is pressed in one.

use std::ops::Range;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: String,
    pub path: PathBuf,
    pub body: String,
    /// Last changed, in milliseconds since the epoch.
    pub modified: i64,
}

impl Note {
    /// The first line with words in it, without its markdown, else "Untitled".
    pub fn title(&self) -> String {
        title_of(&self.body)
    }

    /// The text after the title, flattened onto one line, for the list.
    pub fn preview(&self) -> String {
        let mut lines = self.body.lines().map(plain).filter(|l| !l.is_empty());
        lines.next();
        let joined: Vec<String> = lines.take(3).collect();
        joined.join(" ")
    }
}

pub fn notes_dir() -> PathBuf {
    crate::paths::data_dir().join("notes")
}

/// Every note in `dir`, the most recently changed first.
pub fn list_in(dir: &Path) -> Vec<Note> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut notes: Vec<Note> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .filter_map(|e| {
            let path = e.path();
            let body = std::fs::read_to_string(&path).ok()?;
            let modified = e.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
            Some(Note { id: path.file_stem()?.to_string_lossy().to_string(), path, body, modified })
        })
        .collect();
    notes.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.id.cmp(&a.id)));
    notes
}

/// A new, empty note in `dir`.
pub fn create_in(dir: &Path) -> std::io::Result<Note> {
    std::fs::create_dir_all(dir)?;
    let id = uuid::Uuid::now_v7().simple().to_string();
    let path = dir.join(format!("{id}.md"));
    std::fs::write(&path, "")?;
    Ok(Note { id, path, body: String::new(), modified: crate::store::now_ms() })
}

/// Move `note` out of the list into `Deleted/` beside it, where it can still be got back.
pub fn delete(note: &Note) -> std::io::Result<PathBuf> {
    let bin = note.path.parent().unwrap_or(Path::new(".")).join("Deleted");
    std::fs::create_dir_all(&bin)?;
    let to = bin.join(note.path.file_name().unwrap_or_default());
    std::fs::rename(&note.path, &to)?;
    Ok(to)
}

/// Save `note` whole or not at all: written beside it, then moved over it.
pub fn save(note: &Note) -> std::io::Result<()> {
    let tmp = note.path.with_extension("md.tmp");
    std::fs::write(&tmp, &note.body)?;
    std::fs::rename(&tmp, &note.path)
}

fn title_of(body: &str) -> String {
    body.lines().map(plain).find(|l| !l.is_empty()).map(|l| l.chars().take(80).collect()).unwrap_or_else(|| "Untitled".into())
}

/// A line of markdown as text: no heading, list, quote or checkbox marker, no emphasis marks or
/// inline tags.
fn plain(line: &str) -> String {
    let mut l = line.trim();
    l = l.trim_start_matches('#').trim_start();
    l = l.strip_prefix("> ").unwrap_or(l);
    for marker in ["- [ ] ", "- [x] ", "- [X] ", "- ", "* ", "+ "] {
        if let Some(rest) = l.strip_prefix(marker) {
            l = rest;
            break;
        }
    }
    if let Some((num, rest)) = l.split_once(". ")
        && !num.is_empty()
        && num.chars().all(|c| c.is_ascii_digit())
    {
        l = rest;
    }
    let mut out = String::with_capacity(l.len());
    let mut in_tag = false;
    for c in l.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            '*' | '_' | '~' | '`' | '=' if !in_tag => {}
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// An edit to a note: its new text and what's selected after.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub text: String,
    pub selection: Range<usize>,
}

/// Put `open` and `close` around the selection, or take them off when it's already wrapped (in
/// or just outside the selection). With nothing selected, the cursor lands between them.
pub fn wrap(text: &str, sel: Range<usize>, open: &str, close: &str) -> Edit {
    let sel = clamp(text, sel);
    let inner = &text[sel.clone()];
    // One-character marks (`*` italics, `` ` `` code) share their character with longer ones
    // (`**` bold): count the run on each side instead. An odd run means this mark is there.
    if let (Some(c), true) = (open.chars().next(), open.len() == 1 && open == close) {
        let (mut start, mut end) = (sel.start, sel.end);
        while start < end && text[start..].starts_with(c) {
            start += 1;
        }
        while end > start && text[..end].ends_with(c) {
            end -= 1;
        }
        let before = text[..start].chars().rev().take_while(|x| *x == c).count();
        let after = text[end..].chars().take_while(|x| *x == c).count();
        let inner = &text[start..end];
        if before.min(after) % 2 == 1 {
            let new = format!("{}{}{}", &text[..start - 1], inner, &text[end + 1..]);
            return Edit { text: new, selection: start - 1..end - 1 };
        }
        let new = format!("{}{c}{inner}{c}{}", &text[..start], &text[end..]);
        return Edit { text: new, selection: start + 1..end + 1 };
    }
    // Selected with its marks: unwrap.
    if inner.len() >= open.len() + close.len() && inner.starts_with(open) && inner.ends_with(close) {
        let body = &inner[open.len()..inner.len() - close.len()];
        let new = format!("{}{}{}", &text[..sel.start], body, &text[sel.end..]);
        return Edit { text: new, selection: sel.start..sel.start + body.len() };
    }
    // Marks just outside the selection: unwrap.
    if sel.start >= open.len() && text[..sel.start].ends_with(open) && text[sel.end..].starts_with(close) && !sel.is_empty() {
        let start = sel.start - open.len();
        let new = format!("{}{}{}", &text[..start], inner, &text[sel.end + close.len()..]);
        return Edit { text: new, selection: start..start + inner.len() };
    }
    let new = format!("{}{}{}{}{}", &text[..sel.start], open, inner, close, &text[sel.end..]);
    let start = sel.start + open.len();
    Edit { text: new, selection: start..start + inner.len() }
}

/// Colour the selection's text, or highlight it (`background`), in `css` (`#rrggbb`).
pub fn color(text: &str, sel: Range<usize>, css: &str, background: bool) -> Edit {
    if background {
        wrap(text, sel, &format!("<mark style=\"background: {css}\">"), "</mark>")
    } else {
        wrap(text, sel, &format!("<span style=\"color: {css}\">"), "</span>")
    }
}

/// How the lines of a block start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Block {
    Bullets,
    Numbers,
    Checklist,
    Heading(u8),
    Quote,
}

/// The marker a line of `block` starts with (`n`: its number in a numbered list).
fn marker(block: Block, n: usize) -> String {
    match block {
        Block::Bullets => "- ".into(),
        Block::Numbers => format!("{n}. "),
        Block::Checklist => "- [ ] ".into(),
        Block::Heading(level) => format!("{} ", "#".repeat(level.clamp(1, 6) as usize)),
        Block::Quote => "> ".into(),
    }
}

/// How long the block marker `line` starts with is, if it's one of `block`'s.
fn marker_len(line: &str, block: Block) -> Option<usize> {
    let indent = line.len() - line.trim_start().len();
    let l = &line[indent..];
    let len = match block {
        Block::Bullets => ["- ", "* ", "+ "].iter().find(|m| l.starts_with(**m) && !l.starts_with("- [")).map(|m| m.len()),
        Block::Numbers => {
            let digits = l.chars().take_while(char::is_ascii_digit).count();
            (digits > 0 && l[digits..].starts_with(". ")).then_some(digits + 2)
        }
        Block::Checklist => ["- [ ] ", "- [x] ", "- [X] "].iter().find(|m| l.starts_with(**m)).map(|m| m.len()),
        Block::Heading(level) => {
            let hashes = l.chars().take_while(|c| *c == '#').count();
            (hashes == level as usize && l[hashes..].starts_with(' ')).then_some(hashes + 1)
        }
        Block::Quote => l.starts_with("> ").then_some(2),
    }?;
    Some(indent + len)
}

/// Any block marker `line` starts with (another list, a heading…), to swap for a new one.
fn any_marker_len(line: &str) -> usize {
    [Block::Checklist, Block::Bullets, Block::Numbers, Block::Quote]
        .into_iter()
        .chain((1..=6).map(Block::Heading))
        .find_map(|b| marker_len(line, b))
        .unwrap_or(0)
}

/// Make the selected lines a `block`, or plain lines again when they all already are one.
pub fn toggle_block(text: &str, sel: Range<usize>, block: Block) -> Edit {
    let sel = clamp(text, sel);
    let start = text[..sel.start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    // A selection ending at the start of a line doesn't take that line in.
    let end_at = if sel.end > sel.start && text[..sel.end].ends_with('\n') { sel.end - 1 } else { sel.end };
    let end = text[end_at..].find('\n').map(|i| end_at + i).unwrap_or(text.len());
    let lines: Vec<&str> = text[start..end].split('\n').collect();
    let all = lines.iter().filter(|l| !l.trim().is_empty()).all(|l| marker_len(l, block).is_some());
    let mut n = 0;
    let changed: Vec<String> = lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() && lines.len() > 1 {
                return l.to_string();
            }
            if all {
                let cut = marker_len(l, block).unwrap_or(0);
                let indent = l.len() - l.trim_start().len();
                format!("{}{}", &l[..indent.min(cut)], &l[cut..])
            } else {
                n += 1;
                let cut = any_marker_len(l);
                let indent = (l.len() - l.trim_start().len()).min(cut);
                format!("{}{}{}", &l[..indent], marker(block, n), &l[cut..])
            }
        })
        .collect();
    let block_text = changed.join("\n");
    let new = format!("{}{}{}", &text[..start], block_text, &text[end..]);
    // One line and nothing selected: the cursor keeps its place in the words.
    let selection = if lines.len() == 1 && sel.is_empty() {
        let shift = block_text.len() as isize - (end - start) as isize;
        let at = (sel.start as isize + shift).clamp(start as isize, (start + block_text.len()) as isize) as usize;
        at..at
    } else {
        start..start + block_text.len()
    };
    Edit { text: new, selection }
}

/// Return pressed at `at`: a new line that carries the list on (the next number, an unticked
/// box), or, on an empty item, ends the list.
pub fn newline(text: &str, sel: Range<usize>) -> Edit {
    let sel = clamp(text, sel);
    let at = sel.start;
    let line_start = text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line = &text[line_start..at];
    let indent = &line[..line.len() - line.trim_start().len()];
    let carried = [Block::Checklist, Block::Bullets, Block::Numbers, Block::Quote].into_iter().find_map(|b| marker_len(line, b).map(|len| (b, len)));
    let insert = match carried {
        // An empty item: Return ends the list, leaving a blank line.
        Some((_, len)) if line[len..].trim().is_empty() => {
            let new = format!("{}{}", &text[..line_start], &text[sel.end..]);
            return Edit { text: new, selection: line_start..line_start };
        }
        Some((Block::Numbers, _)) => {
            let n: usize = line.trim_start().chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0);
            format!("\n{indent}{}", marker(Block::Numbers, n + 1))
        }
        Some((Block::Checklist, _)) => format!("\n{indent}- [ ] "),
        // Bullets and quotes: the same marker again.
        Some((_, len)) => format!("\n{indent}{}", &line[indent.len()..len]),
        None => "\n".into(),
    };
    let new = format!("{}{}{}", &text[..at], insert, &text[sel.end..]);
    let cursor = at + insert.len();
    Edit { text: new, selection: cursor..cursor }
}

/// Tick or untick the checklist item on line `line` (0-based).
pub fn toggle_check(text: &str, line: usize) -> Option<String> {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let l = lines.get_mut(line)?;
    let indent = l.len() - l.trim_start().len();
    let rest = l[indent..].to_string();
    let swapped = if let Some(r) = rest.strip_prefix("- [ ] ") {
        format!("- [x] {r}")
    } else {
        let r = rest.strip_prefix("- [x] ").or_else(|| rest.strip_prefix("- [X] "))?;
        format!("- [ ] {r}")
    };
    *l = format!("{}{}", &l[..indent], swapped);
    Some(lines.join("\n"))
}

/// `sel` inside `text` and on character boundaries.
fn clamp(text: &str, sel: Range<usize>) -> Range<usize> {
    let fix = |mut i: usize| {
        i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let (a, b) = (fix(sel.start), fix(sel.end));
    a.min(b)..a.max(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_marks_on_and_off() {
        let e = wrap("make this bold", 5..9, "**", "**");
        assert_eq!(e.text, "make **this** bold");
        assert_eq!(&e.text[e.selection.clone()], "this");
        // Again, on the same selection: off.
        let off = wrap(&e.text, e.selection.clone(), "**", "**");
        assert_eq!(off.text, "make this bold");
        assert_eq!(&off.text[off.selection.clone()], "this");
        // Selected with its marks: off too.
        let off = wrap("make **this** bold", 5..13, "**", "**");
        assert_eq!(off.text, "make this bold");
        // Italic on bold text adds italics; it doesn't eat one of bold's stars.
        let both = wrap("**bold**", 2..6, "*", "*");
        assert_eq!(both.text, "***bold***");
        assert_eq!(wrap(&both.text, both.selection.clone(), "*", "*").text, "**bold**", "and takes them off again");
        assert_eq!(wrap("**bold**", 0..8, "*", "*").text, "***bold***");
        // Nothing selected: the cursor between the marks.
        let empty = wrap("ab", 1..1, "*", "*");
        assert_eq!(empty.text, "a**b");
        assert_eq!(empty.selection, 2..2);
    }

    #[test]
    fn colour_and_highlight() {
        let e = color("a red word", 2..5, "#ef4444", false);
        assert_eq!(e.text, "a <span style=\"color: #ef4444\">red</span> word");
        assert_eq!(&e.text[e.selection.clone()], "red");
        let h = color("note", 0..4, "#facc1555", true);
        assert_eq!(h.text, "<mark style=\"background: #facc1555\">note</mark>");
    }

    #[test]
    fn lists_on_and_off() {
        let text = "milk\neggs\nbread";
        let e = toggle_block(text, 0..text.len(), Block::Bullets);
        assert_eq!(e.text, "- milk\n- eggs\n- bread");
        let off = toggle_block(&e.text, 0..e.text.len(), Block::Bullets);
        assert_eq!(off.text, text);
        let n = toggle_block(text, 0..text.len(), Block::Numbers);
        assert_eq!(n.text, "1. milk\n2. eggs\n3. bread");
        // A list of one kind becomes the other.
        let c = toggle_block(&n.text, 0..n.text.len(), Block::Checklist);
        assert_eq!(c.text, "- [ ] milk\n- [ ] eggs\n- [ ] bread");
        // A heading, on the cursor's line only.
        let h = toggle_block("Title\nbody", 2..2, Block::Heading(1));
        assert_eq!(h.text, "# Title\nbody");
        assert_eq!(h.selection, 4..4, "the cursor stays in the word");
        let h2 = toggle_block(&h.text, 3..3, Block::Heading(2));
        assert_eq!(h2.text, "## Title\nbody", "one heading level swaps for another");
    }

    #[test]
    fn return_carries_a_list_on() {
        let e = newline("- milk", 6..6);
        assert_eq!(e.text, "- milk\n- ");
        assert_eq!(e.selection, 9..9);
        assert_eq!(newline("1. one", 6..6).text, "1. one\n2. ");
        assert_eq!(newline("- [x] done", 10..10).text, "- [x] done\n- [ ] ");
        assert_eq!(newline("  - nested", 10..10).text, "  - nested\n  - ");
        assert_eq!(newline("> quoted", 8..8).text, "> quoted\n> ");
        // An empty item ends the list.
        let end = newline("- milk\n- ", 9..9);
        assert_eq!(end.text, "- milk\n");
        assert_eq!(end.selection, 7..7);
        assert_eq!(newline("plain", 5..5).text, "plain\n");
    }

    #[test]
    fn checking_items_off() {
        assert_eq!(toggle_check("- [ ] a\n- [x] b", 0).as_deref(), Some("- [x] a\n- [x] b"));
        assert_eq!(toggle_check("- [ ] a\n- [x] b", 1).as_deref(), Some("- [ ] a\n- [ ] b"));
        assert_eq!(toggle_check("plain", 0), None);
    }

    #[test]
    fn titles_and_previews_read_as_text() {
        let n = Note { id: "x".into(), path: PathBuf::new(), body: "\n# **Ideas** for <span style=\"color: red\">Trek</span>\n- tabs\n- [ ] notes".into(), modified: 0 };
        assert_eq!(n.title(), "Ideas for Trek");
        assert_eq!(n.preview(), "tabs notes");
        let empty = Note { body: "  \n".into(), ..n };
        assert_eq!(empty.title(), "Untitled");
    }

    #[test]
    fn notes_are_files() {
        let dir = std::env::temp_dir().join(format!("trek-notes-{}", uuid::Uuid::now_v7().simple()));
        let mut a = create_in(&dir).unwrap();
        a.body = "first".into();
        save(&a).unwrap();
        let listed = list_in(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].body, "first");
        let kept = delete(&a).unwrap();
        assert!(list_in(&dir).is_empty(), "out of the list");
        assert_eq!(std::fs::read_to_string(kept).unwrap(), "first", "but not gone");
        let _ = std::fs::remove_dir_all(dir);
    }
}
