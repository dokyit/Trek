//! A thread as text to share: Markdown (what Copy as Markdown, a gist and Send to carry), plain
//! text, a self-contained HTML page, and the items as JSON. What goes in is chosen by `Options`
//! (tool calls, thinking) and `Redact` (the home folder as `~`, paths outside the project as
//! `…/name`, and anything that looks like a key or token taken out), applied to the items before
//! any of them is rendered, so every format says the same.

use std::sync::LazyLock;
use trek_core::store::{Item, ToolStatus};

/// What a shared copy carries besides the messages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Tool calls with their input and output (diffs among them).
    pub tools: bool,
    /// The agent's thinking.
    pub thinking: bool,
}

/// What "Hide file paths and secrets" knows about where the thread ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redact {
    /// The user's home folder, shown as `~`.
    pub home: Option<String>,
    /// The project's folder: paths in it stay (with `~` for the home folder); others become
    /// `…/name`.
    pub project: Option<String>,
}

impl Redact {
    /// This user's home folder and the thread's project.
    pub fn here(project: Option<&std::path::Path>) -> Redact {
        Redact {
            home: Some(trek_core::paths::home().display().to_string()),
            project: project.map(|p| p.display().to_string()),
        }
    }
}

/// A tool's output is cut to this many characters in a shared copy.
const TOOL_OUTPUT_MAX: usize = 12_000;

/// `items` with what `options` leaves out dropped, and every text redacted when `redact` is given.
pub fn prepare(items: Vec<Item>, options: Options, redact: Option<&Redact>) -> Vec<Item> {
    items
        .into_iter()
        .filter(|item| match item {
            Item::Tool { .. } => options.tools,
            Item::Reasoning { .. } => options.thinking,
            _ => true,
        })
        .map(|item| match redact {
            Some(r) => redact_item(item, r),
            None => item,
        })
        .collect()
}

/// Every text an item has, redacted.
fn redact_item(item: Item, r: &Redact) -> Item {
    let red = |s: String| redact_text(&s, r);
    match item {
        Item::User {
            text,
            images,
            at,
            resume,
            aside,
        } => Item::User {
            text: red(text),
            images: images.into_iter().map(red).collect(),
            at,
            resume,
            aside,
        },
        Item::Assistant { text } => Item::Assistant { text: red(text) },
        Item::Reasoning { text } => Item::Reasoning { text: red(text) },
        Item::Tool {
            id,
            title,
            detail,
            output,
            status,
        } => Item::Tool {
            id,
            title: red(title),
            detail: red(detail),
            output: red(output),
            status,
        },
        Item::Notice { text } => Item::Notice { text: red(text) },
        Item::Error { text } => Item::Error { text: red(text) },
        Item::Limit {
            text,
            resets_at,
            scope,
        } => Item::Limit {
            text: red(text),
            resets_at,
            scope,
        },
        other => other,
    }
}

// ───────────────────────────── redaction ─────────────────────────────

static PRIVATE_KEY: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(-----END [A-Z0-9 ]*PRIVATE KEY-----|$)",
    )
    .unwrap()
});

/// Tokens recognisable by their shape: Anthropic, OpenAI, GitHub, Slack, AWS, Google, GitLab,
/// npm keys, and JSON web tokens.
static TOKEN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(concat!(
        r"\b(?:sk-ant-[A-Za-z0-9_\-]{10,}",
        r"|sk-(?:proj-|live-|test-)?[A-Za-z0-9_\-]{20,}",
        r"|gh[pousr]_[A-Za-z0-9]{20,}",
        r"|github_pat_[A-Za-z0-9_]{20,}",
        r"|xox[abposr]-[A-Za-z0-9\-]{10,}",
        r"|AKIA[0-9A-Z]{16}",
        r"|AIza[0-9A-Za-z_\-]{35}",
        r"|glpat-[A-Za-z0-9_\-]{20,}",
        r"|npm_[A-Za-z0-9]{36}",
        r"|eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,})",
    ))
    .unwrap()
});

static BEARER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(bearer|token|basic)(\s+)[A-Za-z0-9._~+/\-]{16,}=*").unwrap()
});

/// `API_KEY=…`, `"password": "…"`, `client_secret: …`: the name stays, the value goes.
static ASSIGNED: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?i)\b([A-Za-z0-9_\-]*(?:api[_\-]?key|secret|token|passw(?:or)?d|access[_\-]?key|private[_\-]?key)[A-Za-z0-9_\-]*)("?\s*[:=]\s*)(["']?)([^\s"'`,;]{8,})"#,
    )
    .unwrap()
});

/// `https://user:password@host`.
static URL_CREDENTIALS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\b([a-z][a-z0-9+.\-]*://)[^\s/:@]+:[^\s/@]+@").unwrap());

/// An absolute path of two or more parts, where a path can start: `/Users/me/x.rs`, and on
/// Windows `C:\Users\me\x.rs`. URLs (`https://a/b`) aren't, as their `//` starts no part.
static UNIX_PATH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(^|[\s(\[<"'`=:,*])(/[^\s/()\[\]<>"'`,;:*|]+(?:/[^\s/()\[\]<>"'`,;:*|]*)+)"#,
    )
    .unwrap()
});

static WINDOWS_PATH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(^|[\s(\[<"'`=,*])([A-Za-z]:\\[^\s\\()\[\]<>"'`,;*|]+(?:\\[^\s\\()\[\]<>"'`,;*|]*)+)"#,
    )
    .unwrap()
});

const REDACTED: &str = "[redacted]";

/// `text` with keys and tokens taken out and paths shortened (see `Redact`).
pub fn redact_text(text: &str, r: &Redact) -> String {
    let text = redact_secrets(text);
    redact_paths(&text, r)
}

/// `text` without what looks like a private key, an API key or token, or a password.
pub fn redact_secrets(text: &str) -> String {
    let text = PRIVATE_KEY.replace_all(text, "[redacted private key]");
    let text = URL_CREDENTIALS.replace_all(&text, "${1}[redacted]@");
    let text = TOKEN.replace_all(&text, REDACTED);
    let text = BEARER.replace_all(&text, "${1}${2}[redacted]");
    let text = ASSIGNED.replace_all(&text, |c: &regex::Captures| {
        let value = &c[4];
        // A word ("token: the next one") isn't a secret: a value with a digit, or a long one, is.
        let secret = value != REDACTED
            && (value.chars().any(|ch| ch.is_ascii_digit()) || value.chars().count() >= 16);
        if secret {
            format!("{}{}{}{REDACTED}", &c[1], &c[2], &c[3])
        } else {
            c[0].to_string()
        }
    });
    text.into_owned()
}

/// `text` with the home folder as `~` and absolute paths outside the project as `…/name`.
pub fn redact_paths(text: &str, r: &Redact) -> String {
    let home = r.home.as_deref().filter(|h| h.len() > 1);
    let project = r.project.as_deref().filter(|p| p.len() > 1);
    let shorten = |path: &str, sep: char| -> String {
        // A sentence's full stop or colon after a path isn't part of it.
        let trimmed = path.trim_end_matches(['.', ':', '!', '?']);
        let tail = &path[trimmed.len()..];
        let inside = |root: &str| {
            trimmed == root
                || trimmed
                    .strip_prefix(root)
                    .is_some_and(|rest| rest.starts_with(sep))
        };
        let shown = if project.is_some_and(inside) {
            match home.filter(|h| inside(h)) {
                Some(h) => format!("~{}", &trimmed[h.len()..]),
                None => trimmed.to_string(),
            }
        } else if home.is_some_and(|h| trimmed.trim_end_matches(sep) == h) {
            format!("~{}", &trimmed[home.unwrap_or_default().len()..])
        } else {
            let name = trimmed
                .trim_end_matches(sep)
                .rsplit(sep)
                .next()
                .unwrap_or_default();
            format!("…{sep}{name}")
        };
        shown + tail
    };
    let text = UNIX_PATH.replace_all(text, |c: &regex::Captures| {
        format!("{}{}", &c[1], shorten(&c[2], '/'))
    });
    let text = WINDOWS_PATH.replace_all(&text, |c: &regex::Captures| {
        format!("{}{}", &c[1], shorten(&c[2], '\\'))
    });
    // The home folder where no path was seen (`file:///Users/me/…`, `HOME=/Users/me`).
    match home {
        Some(h) => text.replace(h, "~"),
        None => text.into_owned(),
    }
}

// ───────────────────────────── Markdown ─────────────────────────────

/// A code fence longer than any run of backticks in `text`, so it can't be closed early.
fn fence(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!(
            "{}\n… ({} more characters)",
            &text[..at],
            text[at..].chars().count()
        ),
        None => text.to_string(),
    }
}

fn status_note(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Failed => " (failed)",
        ToolStatus::Denied => " (denied)",
        ToolStatus::Running => " (running)",
        ToolStatus::Done => "",
    }
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// The conversation as Markdown: your messages with the images you attached, the agent's
/// answers, turns that failed, and whatever else `items` still has (tool calls and thinking, as
/// collapsed `<details>`; see `prepare`).
pub fn markdown(title: &str, items: &[Item]) -> String {
    let mut out = format!("# {title}\n");
    for item in items {
        match item {
            Item::User { text, images, .. } => {
                out.push_str("\n## You\n");
                // As written: what Trek adds for the agent (a consult, a restatement) isn't the user's words.
                let text = trek_core::restate::as_written(text);
                if !text.trim().is_empty() {
                    out.push_str(&format!("\n{}\n", text.trim()));
                }
                for path in images {
                    // Angle brackets keep paths with spaces in one link.
                    out.push_str(&format!("\n![{}](<{path}>)\n", file_name(path)));
                }
            }
            Item::Assistant { text } if !text.trim().is_empty() => {
                out.push_str(&format!("\n{}\n", text.trim()))
            }
            Item::Reasoning { text } if !text.trim().is_empty() => out.push_str(&format!(
                "\n<details>\n<summary>Thinking</summary>\n\n{}\n\n</details>\n",
                text.trim()
            )),
            Item::Tool {
                title,
                detail,
                output,
                status,
                ..
            } => {
                out.push_str(&format!(
                    "\n<details>\n<summary>{}{}</summary>\n",
                    escape_html(title.trim()),
                    status_note(*status)
                ));
                for part in [detail.trim(), output.trim()] {
                    if part.is_empty() {
                        continue;
                    }
                    let part = clip(part, TOOL_OUTPUT_MAX);
                    let f = fence(&part);
                    out.push_str(&format!("\n{f}\n{part}\n{f}\n"));
                }
                out.push_str("\n</details>\n");
            }
            Item::Error { text } => out.push_str(&format!(
                "\n> **Error:** {}\n",
                text.trim().replace('\n', "\n> ")
            )),
            Item::Limit { text, .. } => out.push_str(&format!(
                "\n> **Usage limit:** {}\n",
                text.trim().replace('\n', "\n> ")
            )),
            _ => {}
        }
    }
    out
}

/// The conversation as plain text: who said what, in order (`agent` names the agent's turns).
pub fn plain_text(title: &str, agent: &str, items: &[Item]) -> String {
    let mut out = format!("{title}\n");
    for item in items {
        let (who, text) = match item {
            Item::User { text, .. } => ("You", trek_core::restate::as_written(text).to_string()),
            Item::Assistant { text } => (agent, text.clone()),
            Item::Error { text } => ("Error", text.clone()),
            Item::Limit { text, .. } => ("Usage limit", text.clone()),
            _ => continue,
        };
        if text.trim().is_empty() {
            continue;
        }
        out.push_str(&format!("\n{who}:\n{}\n", text.trim()));
    }
    out
}

// ───────────────────────────── HTML ─────────────────────────────

pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Markdown to HTML with GitHub's extensions (tables, task lists, strikethrough). Raw HTML in
/// the text is shown as text, never run.
fn md_html(text: &str) -> String {
    markdown::to_html_with_options(text, &markdown::Options::gfm())
        .unwrap_or_else(|_| format!("<pre>{}</pre>", escape_html(text)))
}

const PAGE_STYLE: &str = r#"
:root { color-scheme: light dark; --bg: #ffffff; --fg: #1d1d1f; --muted: #6e6e73; --card: #f5f5f7; --line: rgba(0,0,0,.08); --code: #f3f3f5; --accent: #0a6cff; --red: #c4321f; }
@media (prefers-color-scheme: dark) { :root { --bg: #161618; --fg: #ececf0; --muted: #9a9aa2; --card: #222226; --line: rgba(255,255,255,.1); --code: #1d1d21; --accent: #5c9dff; --red: #ff7a66; } }
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 15px/1.6 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif; }
main { max-width: 760px; margin: 0 auto; padding: 40px 20px 80px; }
header h1 { font-size: 24px; line-height: 1.3; margin: 0 0 4px; }
header p { margin: 0 0 28px; color: var(--muted); font-size: 13px; }
.who { font-size: 12px; font-weight: 600; color: var(--muted); text-transform: uppercase; letter-spacing: .04em; margin: 22px 0 6px; }
.you .text { background: var(--card); border: 1px solid var(--line); border-radius: 14px; padding: 10px 14px; }
.text > :first-child { margin-top: 0; } .text > :last-child { margin-bottom: 0; }
a { color: var(--accent); }
pre { background: var(--code); border: 1px solid var(--line); border-radius: 10px; padding: 10px 12px; overflow-x: auto; font-size: 13px; line-height: 1.5; }
code { font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
:not(pre) > code { background: var(--code); border-radius: 5px; padding: 1px 5px; }
table { border-collapse: collapse; } th, td { border: 1px solid var(--line); padding: 4px 8px; }
blockquote { margin: 0; padding-left: 12px; border-left: 3px solid var(--line); color: var(--muted); }
details { border: 1px solid var(--line); border-radius: 10px; margin: 8px 0; padding: 6px 12px; font-size: 13px; }
details summary { cursor: pointer; color: var(--muted); }
details[open] summary { margin-bottom: 6px; }
.error { color: var(--red); }
img { max-width: 100%; border-radius: 8px; }
footer { margin-top: 40px; color: var(--muted); font-size: 12px; }
"#;

/// The conversation as one self-contained page: the messages, code blocks, tool calls and
/// thinking as collapsed details; light or dark as the reader's system is.
pub fn html(title: &str, agent: &str, subtitle: &str, items: &[Item]) -> String {
    let mut body = String::new();
    let mut last_who: Option<&str> = None;
    let mut who = |body: &mut String, name: &'static str, label: &str| {
        if last_who != Some(name) {
            body.push_str(&format!(
                "<div class=\"who\">{}</div>\n",
                escape_html(label)
            ));
            last_who = Some(name);
        }
    };
    for item in items {
        match item {
            Item::User { text, images, .. } => {
                who(&mut body, "you", "You");
                let text = trek_core::restate::as_written(text);
                body.push_str("<section class=\"you\"><div class=\"text\">");
                body.push_str(&md_html(text.trim()));
                for path in images {
                    body.push_str(&format!(
                        "<p>Image: <code>{}</code></p>",
                        escape_html(&file_name(path))
                    ));
                }
                body.push_str("</div></section>\n");
            }
            Item::Assistant { text } if !text.trim().is_empty() => {
                who(&mut body, "agent", agent);
                body.push_str(&format!(
                    "<section class=\"agent\"><div class=\"text\">{}</div></section>\n",
                    md_html(text.trim())
                ));
            }
            Item::Reasoning { text } if !text.trim().is_empty() => {
                who(&mut body, "agent", agent);
                body.push_str(&format!(
                    "<details class=\"thinking\"><summary>Thinking</summary><div class=\"text\">{}</div></details>\n",
                    md_html(text.trim())
                ));
            }
            Item::Tool {
                title,
                detail,
                output,
                status,
                ..
            } => {
                who(&mut body, "agent", agent);
                body.push_str(&format!(
                    "<details class=\"tool\"><summary>{}{}</summary>",
                    escape_html(title.trim()),
                    status_note(*status)
                ));
                for part in [detail.trim(), output.trim()] {
                    if !part.is_empty() {
                        body.push_str(&format!(
                            "<pre><code>{}</code></pre>",
                            escape_html(&clip(part, TOOL_OUTPUT_MAX))
                        ));
                    }
                }
                body.push_str("</details>\n");
            }
            Item::Error { text } => body.push_str(&format!(
                "<p class=\"error\"><strong>Error:</strong> {}</p>\n",
                escape_html(text.trim())
            )),
            Item::Limit { text, .. } => body.push_str(&format!(
                "<blockquote><strong>Usage limit:</strong> {}</blockquote>\n",
                escape_html(text.trim())
            )),
            _ => {}
        }
    }
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{t}</title>\n<style>{PAGE_STYLE}</style>\n</head>\n<body>\n<main>\n<header><h1>{t}</h1><p>{s}</p></header>\n{body}<footer>Shared from Trek</footer>\n</main>\n</body>\n</html>\n",
        t = escape_html(title),
        s = escape_html(subtitle),
    )
}

// ───────────────────────────── JSON ─────────────────────────────

/// The thread's items as Trek stores them, with what the thread is.
pub fn json(t: &trek_core::store::Thread, items: &[Item]) -> String {
    let value = serde_json::json!({
        "trek_transcript": 1,
        "id": t.id,
        "title": t.title,
        "agent": t.agent.key(),
        "model": t.model,
        "created_at": t.created_at,
        "updated_at": t.updated_at,
        "items": items,
    });
    serde_json::to_string_pretty(&value).unwrap_or_default() + "\n"
}

/// A file name for the thread: its title in lower case, words joined by `-`.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.chars().count() >= 60 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "trek-thread".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Item {
        Item::User {
            text: text.into(),
            images: vec![],
            at: None,
            resume: None,
            aside: false,
        }
    }

    fn answer(text: &str) -> Item {
        Item::Assistant { text: text.into() }
    }

    fn tool(title: &str, detail: &str, output: &str) -> Item {
        Item::Tool {
            id: "t1".into(),
            title: title.into(),
            detail: detail.into(),
            output: output.into(),
            status: ToolStatus::Done,
        }
    }

    fn here() -> Redact {
        Redact {
            home: Some("/Users/me".into()),
            project: Some("/Users/me/code/app".into()),
        }
    }

    fn sample() -> Vec<Item> {
        vec![
            user("Fix the bug"),
            Item::Reasoning {
                text: "Look at main.rs".into(),
            },
            tool(
                "Read main.rs",
                "/Users/me/code/app/src/main.rs",
                "fn main() {}",
            ),
            answer("Done."),
        ]
    }

    #[test]
    fn options_choose_what_goes_in() {
        let plain = markdown("T", &prepare(sample(), Options::default(), None));
        assert_eq!(plain, "# T\n\n## You\n\nFix the bug\n\nDone.\n");

        let tools = markdown(
            "T",
            &prepare(
                sample(),
                Options {
                    tools: true,
                    thinking: false,
                },
                None,
            ),
        );
        assert!(tools.contains("<summary>Read main.rs</summary>"));
        assert!(tools.contains("```\nfn main() {}\n```"));
        assert!(!tools.contains("Thinking"));

        let thinking = markdown(
            "T",
            &prepare(
                sample(),
                Options {
                    tools: false,
                    thinking: true,
                },
                None,
            ),
        );
        assert!(thinking.contains("<summary>Thinking</summary>\n\nLook at main.rs"));
        assert!(!thinking.contains("Read main.rs</summary>"));
    }

    #[test]
    fn a_fence_outlasts_the_backticks_inside() {
        let md = markdown("T", &[tool("Write", "", "```rust\nx\n```")]);
        assert!(md.contains("````\n```rust\nx\n```\n````"), "{md}");
    }

    #[test]
    fn home_becomes_tilde_and_outside_paths_their_name() {
        let r = here();
        assert_eq!(
            redact_paths("Edited /Users/me/code/app/src/main.rs.", &r),
            "Edited ~/code/app/src/main.rs."
        );
        assert_eq!(
            redact_paths("Read /Users/me/.ssh/config and /etc/hosts/x", &r),
            "Read …/config and …/x"
        );
        assert_eq!(redact_paths("cd /Users/me", &r), "cd ~");
        assert_eq!(
            redact_paths("HOME=/Users/me/ and file:///Users/me/a", &r),
            "HOME=~/ and file://~/a"
        );
        // URLs, single names and relative paths stay.
        assert_eq!(
            redact_paths("See https://github.com/a/b and src/x.rs or /help", &r),
            "See https://github.com/a/b and src/x.rs or /help"
        );
        // The project's path itself (not a prefix of another folder's).
        assert_eq!(
            redact_paths("/Users/me/code/application/x", &r),
            "…/x",
            "a folder whose name starts like the project's is outside it"
        );
    }

    #[test]
    fn windows_paths_too() {
        let r = Redact {
            home: Some(r"C:\Users\me".into()),
            project: Some(r"C:\Users\me\code\app".into()),
        };
        assert_eq!(
            redact_paths(
                r"Opened C:\Users\me\code\app\src\main.rs and D:\secret\keys.txt",
                &r
            ),
            r"Opened ~\code\app\src\main.rs and …\keys.txt"
        );
    }

    #[test]
    fn secrets_are_taken_out() {
        let text = "key sk-ant-api03-abcdefghijklmnop and ghp_0123456789abcdefghijABCDEFGHIJ0123 \
                    AKIAABCDEFGHIJKLMNOP; OPENAI_API_KEY=sk-proj-1234567890abcdefghij; \
                    \"password\": \"hunter2hunter2\"; Authorization: Bearer abcdefghijklmnop1234; \
                    https://bob:pa55word@example.com/x";
        let out = redact_secrets(text);
        for gone in [
            "sk-ant-api03",
            "ghp_0123",
            "AKIAABCD",
            "sk-proj-1234",
            "hunter2",
            "abcdefghijklmnop1234",
            "pa55word",
        ] {
            assert!(!out.contains(gone), "{gone} is still in {out}");
        }
        assert!(out.contains("OPENAI_API_KEY=[redacted]"), "{out}");
        assert!(out.contains("\"password\": \"[redacted]\""), "{out}");
        assert!(out.contains("Bearer [redacted]"), "{out}");
        assert!(out.contains("https://[redacted]@example.com/x"), "{out}");
        // Words aren't secrets.
        assert_eq!(
            redact_secrets("the token: provided by you"),
            "the token: provided by you"
        );
        let pem =
            "a\n-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\nb";
        assert_eq!(redact_secrets(pem), "a\n[redacted private key]\nb");
    }

    #[test]
    fn redaction_reaches_every_item_and_format() {
        let items = prepare(
            vec![
                user("my key is ghp_0123456789abcdefghijABCDEFGHIJ0123 in /Users/me/.env"),
                tool("Read /Users/me/.env", "", "TOKEN=abc123def456"),
            ],
            Options {
                tools: true,
                thinking: false,
            },
            Some(&here()),
        );
        let md = markdown("T", &items);
        assert!(!md.contains("ghp_") && !md.contains("/Users/me") && !md.contains("abc123"));
        assert!(md.contains("…/.env"), "{md}");
        let page = html("T", "Claude", "", &items);
        assert!(!page.contains("ghp_") && !page.contains("/Users/me"));
    }

    #[test]
    fn plain_text_and_html_and_slug() {
        let items = prepare(sample(), Options::default(), None);
        assert_eq!(
            plain_text("T", "Claude", &items),
            "T\n\nYou:\nFix the bug\n\nClaude:\nDone.\n"
        );
        let page = html(
            "A <b> title",
            "Claude",
            "Shared",
            &[
                user("Hi <script>alert(1)</script>"),
                answer("```rs\nlet x = 1;\n```"),
            ],
        );
        assert!(page.contains("<title>A &lt;b&gt; title</title>"));
        assert!(
            !page.contains("<script>"),
            "raw HTML in a message isn't run"
        );
        assert!(page.contains("<pre><code class=\"language-rs\">let x = 1;"));
        assert!(page.contains("prefers-color-scheme: dark"));
        assert_eq!(slug("Fix: the bug — in main.rs!"), "fix-the-bug-in-main-rs");
        assert_eq!(slug("…"), "trek-thread");
    }
}
