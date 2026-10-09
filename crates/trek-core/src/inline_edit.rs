//! ⌘K in the editor: an edit of picked lines, asked of the agent in the AI side bar's chat. The
//! request is an ordinary message (what the user typed, the lines attached as context) with a
//! block saying which lines to change; the agent edits the file in place, and the change shows
//! as a pending hunk to keep or undo, like any other agent edit. The block is Trek's: views take
//! it off again (`split`), and the mock agent reads it (`parse`).

const OPEN: &str = "<trek-inline-edit";
const CLOSE: &str = "</trek-inline-edit>";

/// The block asking for an edit of `lines` (1-based, inclusive) of `path` (as the agent should
/// read it: relative to its folder).
pub fn block(path: &str, lines: (u32, u32)) -> String {
    let (a, b) = lines;
    let which = if a == b { format!("line {a}") } else { format!("lines {a}-{b}") };
    format!(
        "{OPEN} path=\"{path}\" lines=\"{a}-{b}\">Inline edit: change {which} of {path} as asked above, editing the file in place. Leave the rest of the file as it is. The lines as they are now are attached. Make the edit directly rather than showing it, then reply with one short sentence.{CLOSE}"
    )
}

/// `text` with the block after it.
pub fn with_block(text: &str, path: &str, lines: (u32, u32)) -> String {
    format!("{}\n\n{}", text.trim_end(), block(path, lines))
}

/// The file and lines a message's block asks to edit.
pub fn parse(text: &str) -> Option<(String, (u32, u32))> {
    let at = text.rfind(OPEN)?;
    let head = &text[at + OPEN.len()..];
    let head = &head[..head.find('>')?];
    let attr = |name: &str| -> Option<&str> {
        let key = format!("{name}=\"");
        let from = head.find(&key)? + key.len();
        Some(&head[from..from + head[from..].find('"')?])
    };
    let (a, b) = attr("lines")?.split_once('-')?;
    Some((attr("path")?.to_string(), (a.parse().ok()?, b.parse().ok()?)))
}

/// A message without its block, and whether it had one.
pub fn split(text: &str) -> (&str, bool) {
    match text.rfind(&format!("\n\n{OPEN}")) {
        Some(at) if text[at..].trim_end().ends_with(CLOSE) => (&text[..at], true),
        _ => (text, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_names_the_lines_and_comes_off_again() {
        let text = with_block("make it async\n\n<trek-context>…</trek-context>", "src/a.rs", (3, 8));
        assert_eq!(parse(&text), Some(("src/a.rs".to_string(), (3, 8))));
        assert_eq!(split(&text), ("make it async\n\n<trek-context>…</trek-context>", true));
        assert!(block("a.rs", (4, 4)).contains("change line 4 of a.rs"));
        assert_eq!(split("plain"), ("plain", false));
        assert_eq!(parse("plain"), None);
    }
}
