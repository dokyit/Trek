//! What the AI side bar sends along with a message: the file open in the editor (attached
//! by itself, until removed), lines picked with ⌘L / ⌘⇧L, files from the Explorer, a problem
//! from Problems and a terminal's output ("Add to Chat"). They go at the end of the prompt in a
//! `<trek-context>` block, as `@path` mentions (the agent reads the file if it wants it) and as
//! quoted lines with their path and range; the transcript shows the message without the block,
//! the chips named under it. Ask mode adds a line of its own the same way (`with_ask`), and so
//! does a ⌘K inline edit (`trek_core::inline_edit`).

use std::path::{Path, PathBuf};

const OPEN: &str = "<trek-context>";
const CLOSE: &str = "</trek-context>";
const LEAD: &str = "Attached from the editor:";

const ASK_OPEN: &str = "<trek-ask>";
const ASK_CLOSE: &str = "</trek-ask>";
/// What Ask mode tells the agent with each message. Agents that can be held to reading by Trek
/// are too (see `Workspace::set_chat_mode_in`); this says it to every agent.
const ASK: &str = "Ask mode: reply in words only. Read whatever helps, but change no files and run nothing that changes anything.";

/// One chip over the AI input.
#[derive(Debug, Clone, PartialEq)]
pub enum ContextChip {
    /// A file, mentioned as `@path`.
    File { path: PathBuf },
    /// Lines of a file (1-based, inclusive), quoted.
    Selection { path: PathBuf, lines: (u32, u32), text: String },
    /// What a language server said about `line` of a file, with the lines around it (`lines`,
    /// `text`).
    Problem { path: PathBuf, line: u32, severity: String, message: String, lines: (u32, u32), text: String },
    /// A terminal's latest output.
    Terminal { text: String },
}

impl ContextChip {
    pub fn path(&self) -> Option<&Path> {
        match self {
            ContextChip::File { path } | ContextChip::Selection { path, .. } | ContextChip::Problem { path, .. } => Some(path),
            ContextChip::Terminal { .. } => None,
        }
    }

    /// What the chip says: `main.rs`, `util.rs:3–8`, `⚠ util.rs:4`, `Terminal`.
    pub fn label(&self) -> String {
        let name = self.path().map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string())).unwrap_or_default();
        match self {
            ContextChip::File { .. } => name,
            ContextChip::Selection { lines: (a, b), .. } if a == b => format!("{name}:{a}"),
            ContextChip::Selection { lines: (a, b), .. } => format!("{name}:{a}–{b}"),
            ContextChip::Problem { line, .. } => format!("⚠ {name}:{line}"),
            ContextChip::Terminal { .. } => "Terminal".into(),
        }
    }

    /// What its tooltip says.
    pub fn tip(&self) -> String {
        match self {
            ContextChip::Problem { message, .. } => message.clone(),
            ContextChip::Terminal { text } => text.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"),
            _ => self.path().map(|p| p.display().to_string()).unwrap_or_default(),
        }
    }

    /// The chip as the prompt carries it, paths relative to `root` when inside it.
    fn serialize(&self, root: Option<&Path>) -> String {
        let rel = self.path().map(|p| relative(p, root)).unwrap_or_default();
        let lang = self.path().and_then(Path::extension).map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        match self {
            ContextChip::File { .. } => format!("@{rel}"),
            ContextChip::Selection { lines: (a, b), text, .. } => format!("{rel}:{a}-{b}\n{}", fenced(text, &lang)),
            ContextChip::Problem { line, severity, message, lines: (a, b), text, .. } => {
                format!("{PROBLEM}{rel}:{line} ({severity}): {}\n{rel}:{a}-{b}\n{}", message.lines().next().unwrap_or_default(), fenced(text, &lang))
            }
            ContextChip::Terminal { text } => format!("{TERMINAL}\n{}", fenced(text, "")),
        }
    }
}

const PROBLEM: &str = "Problem at ";
const TERMINAL: &str = "Terminal output:";

/// `text` in a fence longer than any run of backticks in it.
fn fenced(text: &str, lang: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{lang}\n{}\n{fence}", text.trim_end_matches('\n'))
}

fn relative(path: &Path, root: Option<&Path>) -> String {
    root.and_then(|r| path.strip_prefix(r).ok()).filter(|p| !p.as_os_str().is_empty()).unwrap_or(path).display().to_string()
}

/// `text` with `chips` after it (none: as it is).
pub fn with_context(text: &str, chips: &[ContextChip], root: Option<&Path>) -> String {
    if chips.is_empty() {
        return text.to_string();
    }
    let items: Vec<String> = chips.iter().map(|c| c.serialize(root)).collect();
    format!("{}\n\n{OPEN}\n{LEAD}\n{}\n{CLOSE}", text.trim_end(), items.join("\n"))
}

/// `text`, in Ask mode.
pub fn with_ask(text: &str) -> String {
    format!("{}\n\n{ASK_OPEN}{ASK}{ASK_CLOSE}", text.trim_end())
}

/// A message as the user wrote it, and what the side bar added to it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sent<'a> {
    pub said: &'a str,
    /// Sent in Ask mode.
    pub ask: bool,
    /// The chips it carried, by label (`main.rs`, `util.rs:3–8`).
    pub context: Vec<String>,
}

/// Split what the side bar added off a message (`with_context`, then an inline edit's block, then
/// `with_ask`).
pub fn split(text: &str) -> Sent<'_> {
    let (rest, ask) = match text.rfind(&format!("\n\n{ASK_OPEN}")) {
        Some(at) if text[at..].trim_end().ends_with(ASK_CLOSE) => (text[..at].trim_end(), true),
        _ => (text, false),
    };
    let (rest, inline) = trek_core::inline_edit::split(rest);
    let mut sent = split_context(rest);
    sent.ask = ask;
    if inline {
        sent.context.insert(0, crate::keys::localize("⌘K edit").into_owned());
    }
    sent
}

fn split_context(rest: &str) -> Sent<'_> {
    let ask = false;
    let Some(at) = rest.rfind(&format!("\n\n{OPEN}\n")).filter(|_| rest.trim_end().ends_with(CLOSE)) else {
        return Sent { said: rest, ask, context: vec![] };
    };
    let block = &rest[at + OPEN.len() + 3..rest.trim_end().len() - CLOSE.len()];
    // Item heads: `@path`, or `path:a-b` before a fence (the quoted lines are skipped).
    let mut context = vec![];
    let mut fence: Option<String> = None;
    // The line after a problem's head names its quoted lines: not a chip of its own.
    let mut skip_range = false;
    for line in block.lines() {
        if let Some(f) = &fence {
            if line.trim_end() == f {
                fence = None;
            }
            continue;
        }
        if line.starts_with("``") {
            fence = Some(line.chars().take_while(|c| *c == '`').collect());
            continue;
        }
        if let Some(p) = line.strip_prefix(PROBLEM) {
            // `path:line (severity): message`; the quoted lines after it aren't a chip of their own.
            let head = p.split(" (").next().unwrap_or(p);
            context.push(format!("⚠ {}", name(head)));
            skip_range = true;
        } else if line == TERMINAL {
            context.push("Terminal".into());
        } else if let Some(p) = line.strip_prefix('@') {
            context.push(name(p).to_string());
        } else if let Some((p, range)) = line.rsplit_once(':').filter(|(_, r)| r.split_once('-').is_some_and(|(a, b)| a.parse::<u32>().is_ok() && b.parse::<u32>().is_ok())) {
            if std::mem::take(&mut skip_range) {
                continue;
            }
            let (a, b) = range.split_once('-').unwrap_or_default();
            context.push(if a == b { format!("{}:{a}", name(p)) } else { format!("{}:{a}–{b}", name(p)) });
        }
    }
    Sent { said: rest[..at].trim_end(), ask, context }
}

fn name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_go_after_the_message_and_come_off_again() {
        let root = Path::new("/p");
        let chips = vec![
            ContextChip::File { path: "/p/src/main.rs".into() },
            ContextChip::Selection { path: "/p/src/util.rs".into(), lines: (3, 8), text: "fn a() {}\n```\nb\n".into() },
            ContextChip::Selection { path: "/elsewhere/x.py".into(), lines: (2, 2), text: "x = 1".into() },
        ];
        let text = with_context("Make greet polite", &chips, Some(root));
        assert!(text.starts_with("Make greet polite\n\n<trek-context>\n"), "{text}");
        assert!(text.contains("\n@src/main.rs\n"), "{text}");
        assert!(text.contains("\nsrc/util.rs:3-8\n````rs\nfn a() {}\n```\nb\n````\n"), "a fence longer than the lines' own: {text}");
        assert!(text.contains("\n/elsewhere/x.py:2-2\n```py\nx = 1\n```\n"), "{text}");
        let sent = split(&text);
        assert_eq!(sent, Sent { said: "Make greet polite", ask: false, context: vec!["main.rs".into(), "util.rs:3–8".into(), "x.py:2".into()] });
        // Ask mode wraps the lot.
        let asked = with_ask(&text);
        assert_eq!(split(&asked), Sent { ask: true, ..sent });
        assert_eq!(split("plain"), Sent { said: "plain", ask: false, context: vec![] });
        assert_eq!(with_context("plain", &[], None), "plain");
    }

    #[test]
    fn problems_terminal_output_and_inline_edits_come_off_as_chips() {
        let root = Path::new("/p");
        let chips = vec![
            ContextChip::Problem { path: "/p/src/a.rs".into(), line: 4, severity: "error".into(), message: "mismatched types\nexpected u32".into(), lines: (2, 6), text: "let x: u32 = \"no\";".into() },
            ContextChip::Terminal { text: "$ cargo test\ntest result: FAILED".into() },
        ];
        let text = with_context("Fix this problem.", &chips, Some(root));
        assert!(text.contains("\nProblem at src/a.rs:4 (error): mismatched types\nsrc/a.rs:2-6\n```rs\nlet x: u32 = \"no\";\n```\n"), "{text}");
        assert!(text.contains("\nTerminal output:\n```\n$ cargo test\ntest result: FAILED\n```\n"), "{text}");
        assert_eq!(split(&text).context, ["⚠ a.rs:4", "Terminal"]);
        let inline = trek_core::inline_edit::with_block(&with_context("make it loud", &[ContextChip::Selection { path: "/p/a.rs".into(), lines: (2, 3), text: "x".into() }], Some(root)), "a.rs", (2, 3));
        assert_eq!(split(&inline), Sent { said: "make it loud", ask: false, context: vec![crate::keys::localize("⌘K edit").into_owned(), "a.rs:2–3".into()] });
        assert_eq!(chips[0].label(), "⚠ a.rs:4");
        assert_eq!(chips[1].label(), "Terminal");
    }

    #[test]
    fn chips_are_labelled_by_file_and_lines() {
        assert_eq!(ContextChip::File { path: "/p/a.rs".into() }.label(), "a.rs");
        assert_eq!(ContextChip::Selection { path: "/p/a.rs".into(), lines: (3, 8), text: String::new() }.label(), "a.rs:3–8");
        assert_eq!(ContextChip::Selection { path: "/p/a.rs".into(), lines: (4, 4), text: String::new() }.label(), "a.rs:4");
    }
}
