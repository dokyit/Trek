//! Notes: things to jot down next to the agents' work. Each note is a markdown file in the data
//! folder's `notes/`, titled by its first line, so they're plain files anywhere else too. Bold,
//! italics, lists, checklists and headings are markdown; colour and highlights are inline
//! `<span style="color: …">` and `<mark>`, which Trek's markdown renders. Pictures are files in
//! `notes/attachments/`, named by their content and linked as `![alt](attachments/<name>)`
//! (`add_image`), so the folder stays whole wherever it goes.
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
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut notes: Vec<Note> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .filter_map(|e| {
            let path = e.path();
            let body = std::fs::read_to_string(&path).ok()?;
            let modified = e
                .metadata()
                .ok()?
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_millis() as i64;
            Some(Note {
                id: path.file_stem()?.to_string_lossy().to_string(),
                path,
                body,
                modified,
            })
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
    Ok(Note {
        id,
        path,
        body: String::new(),
        modified: crate::store::now_ms(),
    })
}

/// Move `note` out of the list into `Deleted/` beside it, where it can still be got back. Its
/// images go with it into `Deleted/attachments/`, so it still shows them there: moved when no
/// other note shows them, copied when one does. Only plain files in the notes' own
/// `attachments/` are ever touched.
pub fn delete(note: &Note) -> std::io::Result<PathBuf> {
    let dir = note.path.parent().unwrap_or(Path::new("."));
    let bin = dir.join("Deleted");
    std::fs::create_dir_all(&bin)?;
    let to = bin.join(note.path.file_name().unwrap_or_default());
    std::fs::rename(&note.path, &to)?;
    let mine = attachment_names(&note.body);
    if !mine.is_empty() {
        let others: Vec<String> = list_in(dir)
            .into_iter()
            .filter(|n| n.id != note.id)
            .flat_map(|n| attachment_names(&n.body))
            .collect();
        let (from, into) = (attachments_dir(dir), attachments_dir(&bin));
        for name in mine {
            let src = from.join(&name);
            // A plain file, not a link to somewhere else.
            if !std::fs::symlink_metadata(&src).is_ok_and(|m| m.is_file()) {
                continue;
            }
            let dst = into.join(&name);
            let shared = others.contains(&name);
            let moved = std::fs::create_dir_all(&into).and_then(|_| match (shared, dst.exists()) {
                // Named by its content: one already there is the same picture.
                (true, true) => Ok(()),
                (true, false) => std::fs::copy(&src, &dst).map(|_| ()),
                (false, true) => std::fs::remove_file(&src),
                (false, false) => std::fs::rename(&src, &dst),
            });
            if let Err(e) = moved {
                tracing::warn!("note image {name}: {e}");
            }
        }
    }
    Ok(to)
}

/// Save `note` whole or not at all: written beside it, then moved over it.
pub fn save(note: &Note) -> std::io::Result<()> {
    let tmp = note.path.with_extension("md.tmp");
    std::fs::write(&tmp, &note.body)?;
    std::fs::rename(&tmp, &note.path)
}

/// The folder beside the notes their images are kept in: a note links them as
/// `attachments/<name>`, so the notes folder can go anywhere with its pictures.
pub const ATTACHMENTS: &str = "attachments";

/// The images folder of the notes in `notes_dir`.
pub fn attachments_dir(notes_dir: &Path) -> PathBuf {
    notes_dir.join(ATTACHMENTS)
}

/// The kinds of image a note keeps as they are.
pub const IMAGE_KINDS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg"];

/// Keep an image's `bytes` (a `kind` from `IMAGE_KINDS`) with the notes in `notes_dir`, named by
/// its content so the same picture twice is one file. Returns its link from a note
/// (`attachments/<hash>.<kind>`).
pub fn add_image(notes_dir: &Path, bytes: &[u8], kind: &str) -> std::io::Result<String> {
    let kind = kind.to_ascii_lowercase();
    if !IMAGE_KINDS.contains(&kind.as_str()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("a .{kind} file isn't an image a note can show"),
        ));
    }
    if bytes.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the image is empty",
        ));
    }
    let dir = attachments_dir(notes_dir);
    std::fs::create_dir_all(&dir)?;
    let hash = crate::registry::sha256_hex(bytes);
    let name = format!("{}.{kind}", &hash[..32]);
    let path = dir.join(&name);
    if !path.is_file() {
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(format!("{ATTACHMENTS}/{name}"))
}

/// Keep a copy of the image file `src` with the notes (see `add_image`).
pub fn add_image_file(notes_dir: &Path, src: &Path) -> std::io::Result<String> {
    let kind = src.extension().and_then(|e| e.to_str()).unwrap_or_default();
    add_image(notes_dir, &std::fs::read(src)?, kind)
}

/// The kinds of video a note plays in place (the engine's own player: what it can't play opens
/// in Quick Look from the file's chip).
pub const VIDEO_KINDS: &[&str] = &["mp4", "m4v", "mov", "webm"];

/// Whether `link` (or a file name) is a video a note plays.
pub fn is_video(link: &str) -> bool {
    link.rsplit_once('.')
        .is_some_and(|(_, kind)| VIDEO_KINDS.contains(&kind.to_ascii_lowercase().as_str()))
}

/// The longest extension an attached file keeps in its name.
const EXTENSION_MAX: usize = 12;

/// Keep a copy of any file `src` (a video, a PDF, a photo) with the notes, named by its content
/// like an image, so the same file twice is one. Read once, in pieces, so a long video isn't
/// held in memory. Returns its link from a note (`attachments/<hash>.<ext>`).
pub fn add_file(notes_dir: &Path, src: &Path) -> std::io::Result<String> {
    use sha2::Digest as _;
    use std::io::{Read as _, Write as _};
    let mut from = std::fs::File::open(src)?;
    if !from.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "it isn't a file",
        ));
    }
    let dir = attachments_dir(notes_dir);
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!(
        ".{}-{}.tmp",
        std::process::id(),
        crate::store::now_ms()
    ));
    let kept = (|| {
        let (mut hasher, mut to, mut buf, mut len) = (
            sha2::Sha256::new(),
            std::fs::File::create(&tmp)?,
            vec![0u8; 1 << 20],
            0u64,
        );
        loop {
            let n = from.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            to.write_all(&buf[..n])?;
            len += n as u64;
        }
        if len == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the file is empty",
            ));
        }
        Ok(hex::encode(hasher.finalize()))
    })();
    let hash = match kept {
        Ok(hash) => hash,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    let kind: String = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let kind = if !kind.is_empty()
        && kind.len() <= EXTENSION_MAX
        && kind.chars().all(|c| c.is_ascii_alphanumeric())
    {
        format!(".{kind}")
    } else {
        String::new()
    };
    let name = format!("{}{kind}", &hash[..32]);
    let path = dir.join(&name);
    if path.is_file() {
        let _ = std::fs::remove_file(&tmp);
    } else {
        std::fs::rename(&tmp, &path)?;
    }
    Ok(format!("{ATTACHMENTS}/{name}"))
}

/// `text` as it can stand in a markdown link's brackets: no brackets or backslashes, no
/// control characters.
fn bracketed(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '[' | ']' | '\\'))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// `link` as a markdown destination: in `<>` when it has spaces or brackets.
fn destination(link: &str) -> String {
    if link.contains([' ', '(', ')', '<', '>']) {
        format!("<{link}>")
    } else {
        link.to_string()
    }
}

/// The markdown that shows the image at `link`, described by `alt`.
pub fn image_markdown(alt: &str, link: &str) -> String {
    format!("![{}]({})", bracketed(alt), destination(link))
}

/// The markdown that links the file at `link` (not an image), named `name`.
pub fn file_markdown(name: &str, link: &str) -> String {
    format!("[{}]({})", bracketed(name), destination(link))
}

/// Put `block` (whole lines, an image or several) in place of the selection, on lines of its
/// own: the text before and after keeps its lines (an empty line it lands on is taken up), and
/// the cursor goes to the start of the line after it.
pub fn insert_lines(text: &str, sel: Range<usize>, block: &str) -> Edit {
    let sel = clamp(text, sel);
    let (before, after) = (&text[..sel.start], &text[sel.end..]);
    let lead = if before.is_empty() || before.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let tail = if after.starts_with('\n') { "" } else { "\n" };
    let new = format!("{before}{lead}{block}{tail}{after}");
    let cursor = before.len() + lead.len() + block.len() + 1;
    Edit {
        text: new,
        selection: cursor..cursor,
    }
}

/// An image a note shows: `![alt](link)`, and where that is in the note.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageRef {
    pub alt: String,
    pub link: String,
    pub range: Range<usize>,
}

/// The images in a note's markdown, in order (`![alt](link)` and `![alt](<link> "title")`).
pub fn images(body: &str) -> Vec<ImageRef> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = body[from..].find("![") {
        let start = from + i;
        from = start + 2;
        let Some((alt, link, end)) = image_at(body, start) else {
            continue;
        };
        out.push(ImageRef {
            alt,
            link,
            range: start..end,
        });
        from = end;
    }
    out
}

/// A file a note links from its `attachments/` (`[name](attachments/…)`, not an image), and
/// where that is in the note.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRef {
    pub name: String,
    pub link: String,
    pub range: Range<usize>,
}

/// The files a note links from its `attachments/` that aren't images, in order.
pub fn files(body: &str) -> Vec<FileRef> {
    let prefix = format!("{ATTACHMENTS}/");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = body[from..].find('[') {
        let open = from + i;
        from = open + 1;
        if body[..open].ends_with('!') {
            continue;
        }
        let Some((name, link, end)) = link_at(body, open) else {
            continue;
        };
        let shown = link.strip_prefix("./").unwrap_or(&link);
        let kind = shown
            .rsplit_once('.')
            .map(|(_, k)| k.to_ascii_lowercase())
            .unwrap_or_default();
        if shown.starts_with(&prefix) && !IMAGE_KINDS.contains(&kind.as_str()) {
            out.push(FileRef {
                name,
                link,
                range: open..end,
            });
            from = end;
        }
    }
    out
}

/// The image whose markdown starts at `start` (on its `!`): its alt text, link and where it ends.
fn image_at(body: &str, start: usize) -> Option<(String, String, usize)> {
    link_at(body, start + 1)
}

/// The link whose markdown starts at `open` (on its `[`): its text, destination and where it ends.
fn link_at(body: &str, open: usize) -> Option<(String, String, usize)> {
    let rest = &body[open + 1..];
    let close = rest.find(']')?;
    let alt = &rest[..close];
    if alt.contains('\n') {
        return None;
    }
    let after = rest[close + 1..].strip_prefix('(')?;
    let line = &after[..after.find('\n').unwrap_or(after.len())];
    let (link, used) = if let Some(inner) = line.strip_prefix('<') {
        let end = inner.find('>')?;
        (inner[..end].to_string(), end + 2)
    } else {
        let link: String = line
            .trim_start()
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != ')')
            .collect();
        (
            link.clone(),
            line.len() - line.trim_start().len() + link.len(),
        )
    };
    let paren = line[used..].find(')')?;
    let end = open + 1 + close + 2 + used + paren + 1;
    Some((alt.to_string(), link, end))
}

/// The names of the files in the notes' `attachments/` that `body` mentions anywhere (an
/// image, a link, an `<img>`): plain names only, never a path out of the folder.
pub fn attachment_names(body: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let prefix = format!("{ATTACHMENTS}/");
    let mut from = 0;
    while let Some(i) = body[from..].find(&prefix) {
        let at = from + i + prefix.len();
        from = at;
        // The link starts there (or at `./` before it): not `old-attachments/`, nor some other
        // folder's `…/attachments/`.
        let before = body[..at - prefix.len()]
            .strip_suffix("./")
            .unwrap_or(&body[..at - prefix.len()]);
        let begins = before
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace() || matches!(c, '(' | '<' | '"' | '\'' | '='));
        let name: String = body[at..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            .collect();
        if begins && !name.is_empty() && !name.starts_with('.') && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn title_of(body: &str) -> String {
    body.lines()
        .map(plain)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(80).collect())
        .unwrap_or_else(|| "Untitled".into())
}

/// `![alt](link)` as its alt text and `[name](attachments/…)` as its name, for a line read as
/// text.
fn images_as_alt(line: &str) -> std::borrow::Cow<'_, str> {
    let mut found: Vec<(Range<usize>, String)> =
        images(line).into_iter().map(|i| (i.range, i.alt)).collect();
    found.extend(files(line).into_iter().map(|f| (f.range, f.name)));
    if found.is_empty() {
        return line.into();
    }
    found.sort_by_key(|(range, _)| range.start);
    let mut out = String::with_capacity(line.len());
    let mut at = 0;
    for (range, text) in found {
        if range.start < at {
            continue;
        }
        out.push_str(&line[at..range.start]);
        out.push_str(&text);
        at = range.end;
    }
    out.push_str(&line[at..]);
    out.into()
}

/// A line of markdown as text: no heading, list, quote or checkbox marker, no emphasis marks or
/// inline tags; an image is its alt text.
fn plain(line: &str) -> String {
    let line = images_as_alt(line);
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
    let chars: Vec<char> = l.chars().collect();
    let mut out = String::with_capacity(l.len());
    let mut in_tag = false;
    for (i, &c) in chars.iter().enumerate() {
        let before = i.checked_sub(1).map(|j| chars[j]);
        let after = chars.get(i + 1).copied();
        let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        // Whether `c` at `i` is half of a doubled mark with text on its outer side.
        let pair = |i: usize, c: char| {
            let start = if chars.get(i + 1) == Some(&c) {
                i
            } else if i > 0 && chars[i - 1] == c {
                i - 1
            } else {
                return false;
            };
            let outer = |at: Option<usize>| {
                at.and_then(|j| chars.get(j))
                    .is_some_and(|c| !c.is_whitespace())
            };
            outer(start.checked_sub(1)) || outer(Some(start + 2))
        };
        if in_tag {
            in_tag = c != '>';
            continue;
        }
        match c {
            // A tag opens with a letter, `/` or `!` and closes on this line: `x < 5` and `I <3`
            // are text.
            '<' if after.is_some_and(|a| a.is_alphabetic() || matches!(a, '/' | '!'))
                && chars[i..].contains(&'>') =>
            {
                in_tag = true;
            }
            '*' | '`' => {}
            // `_` inside a word (`snake_case`) is part of it; `~~strike~~` and `==mark==` are
            // pairs, and a lone `~` or `=` is text (`a = b`, `~5 min`).
            '_' if !(word(before) && word(after)) => {}
            '~' | '=' if pair(i, c) => {}
            c => out.push(c),
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
            return Edit {
                text: new,
                selection: start - 1..end - 1,
            };
        }
        let new = format!("{}{c}{inner}{c}{}", &text[..start], &text[end..]);
        return Edit {
            text: new,
            selection: start + 1..end + 1,
        };
    }
    // Selected with its marks: unwrap.
    if inner.len() >= open.len() + close.len() && inner.starts_with(open) && inner.ends_with(close)
    {
        let body = &inner[open.len()..inner.len() - close.len()];
        let new = format!("{}{}{}", &text[..sel.start], body, &text[sel.end..]);
        return Edit {
            text: new,
            selection: sel.start..sel.start + body.len(),
        };
    }
    // Marks just outside the selection: unwrap.
    if sel.start >= open.len()
        && text[..sel.start].ends_with(open)
        && text[sel.end..].starts_with(close)
        && !sel.is_empty()
    {
        let start = sel.start - open.len();
        let new = format!(
            "{}{}{}",
            &text[..start],
            inner,
            &text[sel.end + close.len()..]
        );
        return Edit {
            text: new,
            selection: start..start + inner.len(),
        };
    }
    let new = format!(
        "{}{}{}{}{}",
        &text[..sel.start],
        open,
        inner,
        close,
        &text[sel.end..]
    );
    let start = sel.start + open.len();
    Edit {
        text: new,
        selection: start..start + inner.len(),
    }
}

/// Colour the selection's text, or highlight it (`background`), in `css` (`#rrggbb`).
pub fn color(text: &str, sel: Range<usize>, css: &str, background: bool) -> Edit {
    if background {
        wrap(
            text,
            sel,
            &format!("<mark style=\"background: {css}\">"),
            "</mark>",
        )
    } else {
        wrap(
            text,
            sel,
            &format!("<span style=\"color: {css}\">"),
            "</span>",
        )
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
        Block::Bullets => ["- ", "* ", "+ "]
            .iter()
            // A checkbox is a checklist's, but a link (`- [docs](…)`) is still a bullet.
            .find(|m| {
                l.starts_with(**m)
                    && !["- [ ] ", "- [x] ", "- [X] "]
                        .iter()
                        .any(|check| l.starts_with(check))
            })
            .map(|m| m.len()),
        Block::Numbers => {
            let digits = l.chars().take_while(char::is_ascii_digit).count();
            (digits > 0 && l[digits..].starts_with(". ")).then_some(digits + 2)
        }
        Block::Checklist => ["- [ ] ", "- [x] ", "- [X] "]
            .iter()
            .find(|m| l.starts_with(**m))
            .map(|m| m.len()),
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
    [
        Block::Checklist,
        Block::Bullets,
        Block::Numbers,
        Block::Quote,
    ]
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
    let end_at = if sel.end > sel.start && text[..sel.end].ends_with('\n') {
        sel.end - 1
    } else {
        sel.end
    };
    let end = text[end_at..]
        .find('\n')
        .map(|i| end_at + i)
        .unwrap_or(text.len());
    let lines: Vec<&str> = text[start..end].split('\n').collect();
    // Blank lines alone aren't a block already: they get the marker.
    let all = lines.iter().any(|l| !l.trim().is_empty())
        && lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .all(|l| marker_len(l, block).is_some());
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
            } else if l.trim().is_empty() {
                n += 1;
                format!("{l}{}", marker(block, n))
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
        let at = (sel.start as isize + shift)
            .clamp(start as isize, (start + block_text.len()) as isize) as usize;
        at..at
    } else {
        start..start + block_text.len()
    };
    Edit {
        text: new,
        selection,
    }
}

/// Return pressed at `at`: a new line that carries the list on (the next number, an unticked
/// box), or, on an empty item, ends the list.
pub fn newline(text: &str, sel: Range<usize>) -> Edit {
    let sel = clamp(text, sel);
    let at = sel.start;
    let line_start = text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line = &text[line_start..at];
    let indent = &line[..line.len() - line.trim_start().len()];
    let carried = [
        Block::Checklist,
        Block::Bullets,
        Block::Numbers,
        Block::Quote,
    ]
    .into_iter()
    .find_map(|b| marker_len(line, b).map(|len| (b, len)));
    let insert = match carried {
        // An empty item: Return ends the list, leaving a blank line.
        Some((_, len)) if line[len..].trim().is_empty() => {
            let new = format!("{}{}", &text[..line_start], &text[sel.end..]);
            return Edit {
                text: new,
                selection: line_start..line_start,
            };
        }
        Some((Block::Numbers, _)) => {
            let n: usize = line
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0);
            format!("\n{indent}{}", marker(Block::Numbers, n + 1))
        }
        Some((Block::Checklist, _)) => format!("\n{indent}- [ ] "),
        // Bullets and quotes: the same marker again.
        Some((_, len)) => format!("\n{indent}{}", &line[indent.len()..len]),
        None => "\n".into(),
    };
    let new = format!("{}{}{}", &text[..at], insert, &text[sel.end..]);
    let cursor = at + insert.len();
    Edit {
        text: new,
        selection: cursor..cursor,
    }
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
        let r = rest
            .strip_prefix("- [x] ")
            .or_else(|| rest.strip_prefix("- [X] "))?;
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
        assert_eq!(
            wrap(&both.text, both.selection.clone(), "*", "*").text,
            "**bold**",
            "and takes them off again"
        );
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
    fn a_bullet_that_starts_with_a_link_is_still_a_bullet() {
        let text = "- [Trek docs](https://x)\n- plain";
        // Both read as bullets, so Bullets takes the markers off instead of doubling them.
        let off = toggle_block(text, 0..text.len(), Block::Bullets);
        assert_eq!(off.text, "[Trek docs](https://x)\nplain");
        // A single linked item, from plain text and back.
        let on = toggle_block("[Trek docs](https://x)", 0..0, Block::Bullets);
        assert_eq!(on.text, "- [Trek docs](https://x)");
        assert_eq!(
            toggle_block(&on.text, 0..0, Block::Bullets).text,
            "[Trek docs](https://x)"
        );
        // Return at the end of a linked item continues the list.
        let line = "- [Trek docs](https://x)";
        let e = newline(line, line.len()..line.len());
        assert_eq!(e.text, format!("{line}\n- "));
        // A checkbox is still a checklist item, not a bullet.
        assert_eq!(marker_len("- [ ] a", Block::Bullets), None);
        assert_eq!(marker_len("  - [X] a", Block::Bullets), None);
        assert_eq!(marker_len("- [x] a", Block::Checklist), Some(6));
        let e = newline("- [ ] a", 7..7);
        assert_eq!(e.text, "- [ ] a\n- [ ] ");
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
        assert_eq!(
            h2.text, "## Title\nbody",
            "one heading level swaps for another"
        );
    }

    #[test]
    fn a_blank_line_starts_a_block() {
        let e = toggle_block("", 0..0, Block::Bullets);
        assert_eq!(e.text, "- ");
        assert_eq!(e.selection, 2..2);
        assert_eq!(toggle_block("", 0..0, Block::Checklist).text, "- [ ] ");
        assert_eq!(toggle_block("", 0..0, Block::Heading(2)).text, "## ");
        assert_eq!(toggle_block("", 0..0, Block::Quote).text, "> ");
        // The blank line between two paragraphs, and turning it off again.
        let text = "one\n\ntwo";
        let on = toggle_block(text, 4..4, Block::Numbers);
        assert_eq!(on.text, "one\n1. \ntwo");
        assert_eq!(
            toggle_block(&on.text, on.selection.clone(), Block::Numbers).text,
            text
        );
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
        assert_eq!(
            toggle_check("- [ ] a\n- [x] b", 0).as_deref(),
            Some("- [x] a\n- [x] b")
        );
        assert_eq!(
            toggle_check("- [ ] a\n- [x] b", 1).as_deref(),
            Some("- [ ] a\n- [ ] b")
        );
        assert_eq!(toggle_check("plain", 0), None);
    }

    #[test]
    fn titles_keep_the_text_that_only_looks_like_markup() {
        let title = |body: &str| {
            Note {
                id: "x".into(),
                path: PathBuf::new(),
                body: body.into(),
                modified: 0,
            }
            .title()
        };
        assert_eq!(title("x < 5 means small"), "x < 5 means small");
        assert_eq!(title("I <3 this"), "I <3 this");
        assert_eq!(title("a <b and never closes"), "a <b and never closes");
        assert_eq!(title("snake_case_name"), "snake_case_name");
        assert_eq!(title("a = b, about ~5 min"), "a = b, about ~5 min");
        assert_eq!(title("a == b"), "a == b");
        // Real markup still goes.
        assert_eq!(
            title("_soft_ and *loud* and `code`"),
            "soft and loud and code"
        );
        assert_eq!(title("~~old~~ ==new== plan"), "old new plan");
        assert_eq!(
            title("## <b>Bold</b> and <mark style=\"x\">marked</mark> text"),
            "Bold and marked text"
        );
    }

    #[test]
    fn titles_and_previews_read_as_text() {
        let n = Note {
            id: "x".into(),
            path: PathBuf::new(),
            body: "\n# **Ideas** for <span style=\"color: red\">Trek</span>\n- tabs\n- [ ] notes"
                .into(),
            modified: 0,
        };
        assert_eq!(n.title(), "Ideas for Trek");
        assert_eq!(n.preview(), "tabs notes");
        let empty = Note {
            body: "  \n".into(),
            ..n
        };
        assert_eq!(empty.title(), "Untitled");
    }

    #[test]
    fn notes_are_files() {
        let dir =
            std::env::temp_dir().join(format!("trek-notes-{}", uuid::Uuid::now_v7().simple()));
        let mut a = create_in(&dir).unwrap();
        a.body = "first".into();
        save(&a).unwrap();
        let listed = list_in(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].body, "first");
        let kept = delete(&a).unwrap();
        assert!(list_in(&dir).is_empty(), "out of the list");
        assert_eq!(
            std::fs::read_to_string(kept).unwrap(),
            "first",
            "but not gone"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn temp_notes(what: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "trek-notes-{what}-{}",
            uuid::Uuid::now_v7().simple()
        ))
    }

    #[test]
    fn images_are_kept_with_the_notes_by_their_content() {
        let dir = temp_notes("images");
        let png = b"\x89PNG\r\n\x1a\nnot really";
        let link = add_image(&dir, png, "PNG").unwrap();
        assert!(
            link.starts_with("attachments/") && link.ends_with(".png"),
            "{link}"
        );
        assert_eq!(std::fs::read(dir.join(&link)).unwrap(), png);
        // The same picture again is the same file; another is another.
        assert_eq!(add_image(&dir, png, "png").unwrap(), link);
        assert_ne!(add_image(&dir, b"other", "png").unwrap(), link);
        // From a file, by its kind.
        let src = dir.join("shot.jpeg");
        std::fs::write(&src, b"jpeg bytes").unwrap();
        assert!(add_image_file(&dir, &src).unwrap().ends_with(".jpeg"));
        // What isn't an image a note shows (or is empty) isn't kept.
        assert!(add_image(&dir, b"x", "exe").is_err());
        assert!(add_image(&dir, b"", "png").is_err());
        let names: Vec<_> = std::fs::read_dir(attachments_dir(&dir))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            names.iter().all(|n| !n.ends_with(".tmp")),
            "nothing half-written left: {names:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn any_file_is_kept_with_the_notes_and_linked_by_name() {
        let dir = temp_notes("files");
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("Trip Clip.MOV");
        std::fs::write(&clip, b"movie bytes").unwrap();
        let link = add_file(&dir, &clip).unwrap();
        assert!(
            link.starts_with("attachments/") && link.ends_with(".mov"),
            "{link}"
        );
        assert_eq!(std::fs::read(dir.join(&link)).unwrap(), b"movie bytes");
        assert_eq!(
            add_file(&dir, &clip).unwrap(),
            link,
            "the same content is the same file"
        );
        // No extension: just the hash. A strange one isn't carried into the name.
        let bare = dir.join("LICENSE");
        std::fs::write(&bare, b"words").unwrap();
        assert!(!add_file(&dir, &bare).unwrap().contains('.'));
        // Empty files and folders are refused, leaving nothing half-written.
        let empty = dir.join("empty.pdf");
        std::fs::write(&empty, b"").unwrap();
        assert!(add_file(&dir, &empty).is_err() && add_file(&dir, &dir).is_err());
        let names: Vec<_> = std::fs::read_dir(attachments_dir(&dir))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
        // Linked by name, found again in the note, and its copy goes with the note's attachments.
        let md = file_markdown("Trip [Clip].MOV", &link);
        assert_eq!(md, format!("[Trip Clip.MOV]({link})"));
        let body = format!(
            "Notes\n{md}\n![pic](attachments/a.png)\n[web](https://example.com) [other](docs/x.pdf)\n"
        );
        let found = files(&body);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            (found[0].name.as_str(), found[0].link.as_str()),
            ("Trip Clip.MOV", link.as_str())
        );
        assert_eq!(&body[found[0].range.clone()], md);
        assert_eq!(attachment_names(&body).len(), 2);
        assert_eq!(images(&body).len(), 1);
        assert!(is_video(&link) && !is_video("attachments/a.pdf") && !is_video("mov"));
        assert_eq!(
            plain(&format!("see {md} and ![pic](attachments/a.png)")),
            "see Trip Clip.MOV and pic",
            "the list reads names, not links"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_image_goes_in_on_lines_of_its_own() {
        let md = image_markdown("Screen [shot]", "attachments/a.png");
        assert_eq!(md, "![Screen shot](attachments/a.png)");
        assert_eq!(
            image_markdown("x", "/Users/me/My Pics/a.png"),
            "![x](</Users/me/My Pics/a.png>)"
        );
        // Mid-line: it breaks the line, and the cursor lands after it.
        let e = insert_lines("before after", 7..7, &md);
        assert_eq!(e.text, format!("before \n{md}\nafter"));
        assert_eq!(&e.text[e.selection.start..], "after");
        // On an empty line, or at the end: no blank lines added around it.
        let e = insert_lines("a\n\nb", 2..2, "IMG");
        assert_eq!(e.text, "a\nIMG\nb");
        assert_eq!(e.selection, 6..6, "at the start of the next line");
        assert_eq!(insert_lines("", 0..0, "IMG").text, "IMG\n");
        // A selection is replaced.
        assert_eq!(insert_lines("a\nold\nb", 2..5, "IMG").text, "a\nIMG\nb");
    }

    #[test]
    fn a_notes_images_are_found_and_read_as_text() {
        let body = "# Trip\n![The view](attachments/abc.png) and ![](https://x.y/a.jpg \"t\")\n![odd](<My Pics/b.png>)\n![not\nan image](x)";
        let found = images(body);
        assert_eq!(
            found
                .iter()
                .map(|i| (i.alt.as_str(), i.link.as_str()))
                .collect::<Vec<_>>(),
            [
                ("The view", "attachments/abc.png"),
                ("", "https://x.y/a.jpg"),
                ("odd", "My Pics/b.png")
            ]
        );
        assert_eq!(
            &body[found[0].range.clone()],
            "![The view](attachments/abc.png)"
        );
        assert_eq!(
            &body[found[1].range.clone()],
            "![](https://x.y/a.jpg \"t\")"
        );
        // In the list, an image is its description.
        let n = Note {
            id: "x".into(),
            path: PathBuf::new(),
            body: "![Whiteboard](attachments/w.png)\nmore".into(),
            modified: 0,
        };
        assert_eq!(n.title(), "Whiteboard");
        let n = Note {
            body: "Plan\n![](attachments/w.png)\nsteps".into(),
            ..n
        };
        assert_eq!(n.preview(), "steps");
    }

    #[test]
    fn only_the_notes_own_attachments_are_its() {
        let body = "![a](attachments/one.png) [link](./attachments/two.jpg) <img src=\"attachments/three.gif\">\n\
                    ![x](old-attachments/no.png) ![y](/elsewhere/attachments/no2.png) ![z](attachments/../../../etc/passwd) ![again](attachments/one.png)";
        assert_eq!(attachment_names(body), ["one.png", "two.jpg", "three.gif"]);
    }

    #[test]
    fn deleting_a_note_takes_its_images_along_but_leaves_shared_ones() {
        let dir = temp_notes("delete");
        let only = add_image(&dir, b"only mine", "png").unwrap();
        let shared = add_image(&dir, b"shared", "png").unwrap();
        let mut a = create_in(&dir).unwrap();
        a.body = format!(
            "A\n{}\n{}",
            image_markdown("", &only),
            image_markdown("", &shared)
        );
        save(&a).unwrap();
        let mut b = create_in(&dir).unwrap();
        b.body = format!("B\n{}", image_markdown("", &shared));
        save(&b).unwrap();
        // Something outside the folder a note names can't be reached.
        let outside = dir.join("keep.png");
        std::fs::write(&outside, b"not an attachment").unwrap();
        let mut sneaky = a.clone();
        sneaky.body.push_str("\n![](attachments/../keep.png)");
        a = sneaky;
        save(&a).unwrap();

        delete(&a).unwrap();
        let bin = dir.join("Deleted");
        assert!(
            !dir.join(&only).exists(),
            "its own image left the list's folder"
        );
        assert_eq!(
            std::fs::read(bin.join(&only)).unwrap(),
            b"only mine",
            "and went with it to Deleted, still showing there"
        );
        assert!(
            dir.join(&shared).exists(),
            "the other note still shows the shared one"
        );
        assert!(
            bin.join(&shared).exists(),
            "and the deleted note keeps a copy"
        );
        assert!(outside.exists(), "nothing outside attachments/ is touched");
        // The other note goes too: now nothing else shows the shared one.
        delete(&b).unwrap();
        assert!(!dir.join(&shared).exists() && bin.join(&shared).exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
