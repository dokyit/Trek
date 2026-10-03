//! What the agent is doing right now. While a turn runs, its current group of tool calls (the
//! ones since the agent last said something) shows live above the working bar: a summary line
//! ("Ran 2 commands · Exploring the project") and a row per call ("Read src/auth.rs"). This
//! module reads tool calls into those words; `working_bar` draws them, and the transcript folds
//! finished groups into summary rows.

use crate::workspace::Workspace;
use gpui_kit::component::{Icon, IconName};
use std::path::Path;
use trek_core::RunState;
use trek_core::store::Item;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToolKind {
    Command,
    Edit,
    Read,
    Search,
    WebSearch,
    Fetch,
    Other,
    Agent,
    Thought,
}

/// A tool call's kind, from the row title agents report ("Run command", "Read", "Edit").
pub fn tool_kind(title: &str) -> ToolKind {
    match title {
        "Subagent" => ToolKind::Agent,
        "Fetch" => ToolKind::Fetch,
        t if t.starts_with("Search the web") || t.starts_with("Web") => ToolKind::WebSearch,
        t if t.starts_with("Run") || t.starts_with("Ran") => ToolKind::Command,
        t if t.starts_with("Edit") || t.starts_with("Wr") => ToolKind::Edit,
        t if t.starts_with("Read") => ToolKind::Read,
        t if t.contains("Search") || t.starts_with("List") => ToolKind::Search,
        _ => ToolKind::Other,
    }
}

pub fn kind_icon(kind: ToolKind) -> Icon {
    match kind {
        ToolKind::Command => Icon::new(IconName::SquareTerminal),
        ToolKind::Edit => Icon::new(crate::assets::Lucide::Pencil),
        ToolKind::Read => Icon::new(IconName::FileText),
        ToolKind::Search => Icon::new(IconName::Search),
        ToolKind::WebSearch | ToolKind::Fetch => Icon::new(IconName::Globe),
        ToolKind::Other => Icon::new(crate::assets::Lucide::Wrench),
        ToolKind::Agent => Icon::new(crate::assets::Lucide::Users),
        ToolKind::Thought => Icon::new(crate::assets::Lucide::Sparkle),
    }
}

/// A group's icon, for the kind of its latest call: looking around (reads and finds) gets the
/// magnifier, as the phrase says "Exploring".
pub fn group_icon(kind: ToolKind) -> Icon {
    match kind {
        ToolKind::Read | ToolKind::Search => Icon::new(IconName::Search),
        k => kind_icon(k),
    }
}

/// "Ran 3 commands, read 2 files, and edited 1 file"
pub fn summarize(kinds: &[ToolKind]) -> String {
    let count = |k: ToolKind| kinds.iter().filter(|x| **x == k).count();
    let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    let mut parts = vec![];
    let c = count(ToolKind::Command);
    if c > 0 {
        parts.push(format!("ran {}", plural(c, "command", "commands")));
    }
    let e = count(ToolKind::Edit);
    if e > 0 {
        parts.push(format!("edited {}", plural(e, "file", "files")));
    }
    let r = count(ToolKind::Read);
    if r > 0 {
        parts.push(format!("read {}", plural(r, "file", "files")));
    }
    let s = count(ToolKind::Search);
    if s > 0 {
        parts.push(format!("ran {}", plural(s, "search", "searches")));
    }
    let w = count(ToolKind::WebSearch);
    if w > 0 {
        parts.push(if w == 1 { "searched the web".to_string() } else { format!("searched the web {w} times") });
    }
    let f = count(ToolKind::Fetch);
    if f > 0 {
        parts.push(format!("fetched {}", plural(f, "page", "pages")));
    }
    let o = count(ToolKind::Other);
    if o > 0 {
        parts.push(format!("used {}", plural(o, "tool", "tools")));
    }
    let a = count(ToolKind::Agent);
    if a > 0 {
        parts.push(format!("started {}", plural(a, "agent", "agents")));
    }
    let text = match parts.len() {
        0 => "thought it through".to_string(),
        1 => parts.remove(0),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => {
            let last = parts.pop().unwrap_or_default();
            format!("{}, and {last}", parts.join(", "))
        }
    };
    capitalize(&text)
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|f| f.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// One tool call as a live row: a muted verb ("Read", "Find"), then what it acted on, verbatim
/// and in monospace. Commands are their own words, with no verb.
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    pub verb: String,
    pub text: String,
    /// The file it read or changed, for the file-type badge.
    pub file: Option<String>,
}

/// The row for a tool call titled `title` with `detail`, paths shown relative to `cwd`.
pub fn op(title: &str, detail: &str, cwd: Option<&Path>) -> Op {
    let detail = detail.trim();
    let path = |d: &str| relative(d, cwd);
    let (verb, text, file) = match tool_kind(title) {
        ToolKind::Command => (String::new(), first_line(detail), None),
        ToolKind::Read | ToolKind::Edit => {
            // Codex lists every file a change touched: "a.rs, b.rs".
            let shown: Vec<String> = detail.split(", ").filter(|p| !p.is_empty()).map(path).collect();
            let verb = if title.starts_with("Wr") { "Write" } else if title.starts_with("Edit") { "Edit" } else { "Read" };
            (verb.to_string(), shown.join(", "), shown.first().cloned())
        }
        ToolKind::Search => ("Find".to_string(), first_line(detail), None),
        ToolKind::WebSearch => ("Search".to_string(), first_line(detail), None),
        ToolKind::Fetch => ("Fetch".to_string(), first_line(detail), None),
        ToolKind::Agent => ("Agent".to_string(), first_line(detail), None),
        ToolKind::Other | ToolKind::Thought => match title {
            "Update plan" | "Plan" => ("Plan".to_string(), first_line(detail), None),
            "Delete" | "Move" => (title.to_string(), path(detail), Some(path(detail)).filter(|p| !p.is_empty())),
            _ => (tool_name(title), first_line(detail), None),
        },
    };
    Op { verb, text, file }
}

/// An MCP tool's name as people say it: `mcp__github__get_issue` → "github get_issue".
fn tool_name(title: &str) -> String {
    match title.strip_prefix("mcp__") {
        Some(rest) => rest.replace("__", " "),
        None => title.to_string(),
    }
}

/// The first line, with "…" when there's more.
fn first_line(s: &str) -> String {
    let mut lines = s.lines().filter(|l| !l.trim().is_empty());
    let first = lines.next().unwrap_or_default().trim_end().to_string();
    if lines.next().is_some() { format!("{first} …") } else { first }
}

/// `path` relative to `cwd` when it's inside it; home-relative (`~/…`) when it's in the home folder.
pub fn relative(path: &str, cwd: Option<&Path>) -> String {
    let p = Path::new(path);
    // Agents may report `/tmp` as the folder it links to, `/private/tmp`.
    let inside = |c: &Path| p.strip_prefix(c).ok().or_else(|| p.strip_prefix(Path::new("/private").join(c.strip_prefix("/").ok()?)).ok()).map(Path::to_path_buf);
    if let Some(rest) = cwd.and_then(inside).filter(|r| !r.as_os_str().is_empty()) {
        return rest.display().to_string();
    }
    if p.is_absolute() {
        return trek_core::paths::tildify(p);
    }
    path.to_string()
}

fn file_name(path: &str) -> &str {
    let path = path.split(", ").next().unwrap_or(path).trim_end_matches('/');
    path.rsplit('/').next().unwrap_or(path)
}

/// What the group is about, in a few words: the agent's latest reasoning headline when it has
/// one, unless the call under way says something more specific ("Editing auth.rs", "Running
/// tests"); otherwise read from that call ("Exploring the project").
pub fn phrase(title: &str, detail: &str, headline: Option<&str>) -> String {
    let specific = match tool_kind(title) {
        ToolKind::Edit => Some(format!("{} {}", if title.starts_with("Wr") { "Writing" } else { "Editing" }, file_name(detail.trim()))),
        ToolKind::Command => Some(command_phrase(detail)).filter(|p| !matches!(*p, EXPLORING | RUNNING)).map(str::to_string),
        ToolKind::WebSearch => Some("Searching the web".to_string()),
        ToolKind::Fetch => Some(match host(detail) {
            Some(h) => format!("Reading {h}"),
            None => "Reading the web".to_string(),
        }),
        ToolKind::Agent => Some("Sending agents out".to_string()),
        ToolKind::Read | ToolKind::Search | ToolKind::Other | ToolKind::Thought => None,
    };
    if let Some(p) = specific.or_else(|| headline.map(str::to_string)) {
        return p;
    }
    match (tool_kind(title), title) {
        (ToolKind::Command, _) => command_phrase(detail).to_string(),
        (ToolKind::Other, "Update plan" | "Plan") => "Planning".to_string(),
        (ToolKind::Other, "Think") => "Thinking".to_string(),
        (ToolKind::Other, "Question") => "Asking you".to_string(),
        (ToolKind::Other, t) if !t.is_empty() && t != "Tool" => format!("Using {}", tool_name(t)),
        _ => EXPLORING.to_string(),
    }
}

const EXPLORING: &str = "Exploring the project";
const RUNNING: &str = "Running commands";

/// What a shell command is doing, in a few words. The command that matters is the last one in a
/// chain that isn't `cd` (`cd app && npm test` runs tests).
pub fn command_phrase(command: &str) -> &'static str {
    let line = command.lines().find(|l| !l.trim().is_empty()).unwrap_or_default();
    let segments: Vec<&str> = line.split(['&', ';', '|']).map(str::trim).filter(|s| !s.is_empty()).collect();
    let words = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(|w| w.trim_matches(|c| c == '"' || c == '\'' || c == '(' || c == ')').to_string())
            // Environment assignments and wrappers in front of the program.
            .skip_while(|w| w.contains('=') || matches!(w.as_str(), "sudo" | "time" | "env" | "exec" | "command" | "nice"))
            .collect()
    };
    let main = segments.iter().rev().map(|s| words(s)).find(|w| w.first().is_some_and(|p| p != "cd")).unwrap_or_default();
    let Some(program) = main.first().map(|p| p.rsplit('/').next().unwrap_or(p).to_string()) else { return EXPLORING };
    let has = |w: &str| main.iter().skip(1).any(|x| x == w);
    let sub = main.get(1).map(String::as_str).unwrap_or_default();
    // `npx vitest`, `uv run pytest`, `bun x jest`: the program they run.
    let runner = matches!(program.as_str(), "npx" | "bunx" | "pnpx") || (matches!(program.as_str(), "uv" | "poetry" | "pipenv" | "bun") && matches!(sub, "run" | "x"));
    let program = if runner { main.iter().skip(1).find(|w| !matches!(w.as_str(), "run" | "x") && !w.starts_with('-')).cloned().unwrap_or(program) } else { program };
    let script = |names: &[&str]| matches!(program.as_str(), "npm" | "pnpm" | "yarn" | "bun") && main.iter().skip(1).any(|w| names.iter().any(|n| w == n || w.starts_with(&format!("{n}:"))));
    match program.as_str() {
        "pytest" | "jest" | "vitest" | "rspec" | "phpunit" | "ctest" | "mocha" | "ava" | "nextest" => "Running tests",
        _ if has("pytest") => "Running tests",
        _ if script(&["test", "tests"]) || has("test") && !matches!(program.as_str(), "git" | "ls" | "cat" | "rg" | "grep" | "find" | "fd") => "Running tests",
        _ if program == "xcodebuild" && has("test") => "Running tests",
        "cargo" if matches!(sub, "build" | "b") => "Building",
        "cargo" if matches!(sub, "check" | "clippy" | "c") => "Checking the code",
        "cargo" if sub == "fmt" => "Formatting",
        "cargo" if matches!(sub, "add" | "install" | "update") => "Installing dependencies",
        "cargo" if matches!(sub, "run" | "r") => "Running the app",
        "tsc" | "eslint" | "mypy" | "ruff" | "pyright" | "golangci-lint" | "swiftlint" | "biome" => "Checking the code",
        "prettier" | "black" | "gofmt" | "rustfmt" | "swiftformat" => "Formatting",
        "make" | "xcodebuild" | "gradle" | "gradlew" | "ninja" | "cmake" | "vite" | "webpack" | "next" => "Building",
        "swift" | "go" if sub == "build" => "Building",
        "swift" | "go" if sub == "run" => "Running the app",
        _ if script(&["build"]) => "Building",
        _ if script(&["typecheck", "lint", "check"]) => "Checking the code",
        _ if script(&["dev", "start", "preview"]) => "Running the app",
        "npm" | "pnpm" | "yarn" | "bun" if matches!(sub, "install" | "i" | "add" | "ci") || sub.is_empty() && program == "yarn" => "Installing dependencies",
        "pip" | "pip3" | "brew" | "gem" | "bundle" | "pod" if matches!(sub, "install" | "add") => "Installing dependencies",
        "uv" if matches!(sub, "add" | "sync" | "pip") => "Installing dependencies",
        "git" => match sub {
            "commit" => "Committing",
            "push" => "Pushing",
            "pull" | "fetch" => "Pulling",
            "checkout" | "switch" | "branch" | "merge" | "rebase" | "stash" | "reset" => "Working with git",
            _ => "Reviewing changes",
        },
        "gh" => "Checking GitHub",
        "curl" | "wget" | "http" => "Calling the network",
        "docker" | "docker-compose" | "kubectl" => "Working with containers",
        "ls" | "cat" | "head" | "tail" | "sed" | "awk" | "rg" | "grep" | "find" | "fd" | "wc" | "tree" | "pwd" | "file" | "stat" | "du" | "which"
        | "echo" | "less" | "nl" | "sort" | "uniq" | "diff" | "jq" | "bat" | "eza" | "realpath" | "readlink" | "cd" => EXPLORING,
        "python" | "python3" | "node" | "deno" | "ruby" | "bash" | "sh" | "zsh" | "swift" | "go" => "Running a script",
        _ => RUNNING,
    }
}

/// `https://docs.rs/serde/latest` → "docs.rs".
fn host(url: &str) -> Option<&str> {
    let rest = url.trim().split_once("://").map_or(url.trim(), |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next()?.trim_start_matches("www.");
    (host.contains('.') && !host.contains(' ')).then_some(host)
}

/// The latest headline in a thought: a line in bold (`**Exploring the codebase**`, as Codex
/// writes its reasoning summaries) or a markdown heading.
pub fn headline(reasoning: &str) -> Option<String> {
    reasoning
        .lines()
        .rev()
        .map(str::trim)
        .find_map(|l| {
            let bold = l.strip_prefix("**").and_then(|l| l.strip_suffix("**")).filter(|l| !l.contains("**"));
            let heading = l.strip_prefix('#').map(|h| h.trim_start_matches('#')).filter(|h| h.starts_with(' '));
            bold.or(heading)
        })
        .map(|h| h.trim().trim_end_matches([':', '.']).trim().to_string())
        .filter(|h| !h.is_empty() && h.chars().count() <= 60)
}

/// The tool calls since the agent last said something: the transcript's tail of tool calls and
/// thoughts, when it holds at least one call.
#[derive(Debug, Clone, PartialEq)]
pub struct Trail {
    /// The first item of the tail with a row (a call, or a thought with text).
    pub start: usize,
    /// The calls, oldest first.
    pub tools: Vec<usize>,
    /// The latest reasoning headline among its thoughts (`headline`).
    pub headline: Option<String>,
}

pub fn trail(items: &[Item]) -> Option<Trail> {
    let mut start = None;
    let mut tools = vec![];
    let mut headline_found = None;
    for (ix, item) in items.iter().enumerate().rev() {
        match item {
            Item::Tool { .. } => {
                tools.push(ix);
                start = Some(ix);
            }
            Item::Reasoning { text } => {
                if headline_found.is_none() {
                    headline_found = headline(text);
                }
                if !text.trim().is_empty() {
                    start = Some(ix);
                }
            }
            _ => break,
        }
    }
    if tools.is_empty() {
        return None;
    }
    tools.reverse();
    Some(Trail { start: start?, tools, headline: headline_found })
}

/// The group `id` shows live: its tail of tool calls while a turn runs and nothing waits on the
/// user (an approval card takes the bar's place, and the transcript shows the group then).
pub fn live(ws: &Workspace, id: &str) -> Option<Trail> {
    let live = ws.live.get(id)?;
    if ws.thread(id)?.run_state != RunState::Working || !live.permissions.is_empty() {
        return None;
    }
    trail(&live.items)
}

/// Where the transcript of `id` stops for now: at its live group (shown in the bar instead), or
/// at a group still folding away in the bar (until it has, so what follows doesn't show above it).
pub fn transcript_end(ws: &Workspace, id: &str) -> Option<usize> {
    let l = ws.live.get(id)?;
    let folding = l.fold.as_ref().filter(|f| f.until > std::time::Instant::now()).and_then(|f| l.items.position(&f.first));
    folding.or_else(|| live(ws, id).map(|t| t.start))
}

/// A group that stopped being live a moment ago and is folding into its summary row.
#[derive(Debug, Clone, PartialEq)]
pub struct Fold {
    /// The group's first item.
    pub first: String,
    pub until: std::time::Instant,
}

/// How long a finished group takes to fold into its summary row.
pub const FOLD: std::time::Duration = std::time::Duration::from_millis(240);

/// Lines added and removed, as "+12 −3" (either side left out at zero).
pub fn lines_label(added: u32, removed: u32) -> (Option<String>, Option<String>) {
    ((added > 0).then(|| format!("+{added}")), (removed > 0).then(|| format!("−{removed}")))
}

#[cfg(test)]
mod tests {
    use super::{ToolKind, Trail, command_phrase, headline, lines_label, op, phrase, summarize, tool_kind, trail};
    use std::path::Path;
    use trek_core::store::{Item, ToolStatus};

    #[test]
    fn kinds_follow_every_agents_titles() {
        let k = |t: &str| tool_kind(t);
        assert_eq!(k("Run command"), ToolKind::Command);
        assert_eq!(k("Ran command"), ToolKind::Command);
        assert_eq!(k("Read"), ToolKind::Read);
        assert_eq!(k("Edit"), ToolKind::Edit);
        assert_eq!(k("Write"), ToolKind::Edit);
        assert_eq!(k("Search"), ToolKind::Search);
        assert_eq!(k("List files"), ToolKind::Search);
        assert_eq!(k("Search the web"), ToolKind::WebSearch);
        assert_eq!(k("Fetch"), ToolKind::Fetch);
        assert_eq!(k("Subagent"), ToolKind::Agent);
        assert_eq!(k("Update plan"), ToolKind::Other);
        assert_eq!(k("mcp__github__get_issue"), ToolKind::Other);
    }

    #[test]
    fn summaries_count_each_kind() {
        use ToolKind::*;
        assert_eq!(summarize(&[Command, Command]), "Ran 2 commands");
        assert_eq!(summarize(&[Command, Read, Search]), "Ran 1 command, read 1 file, and ran 1 search");
        assert_eq!(summarize(&[Edit, Command]), "Ran 1 command and edited 1 file");
        assert_eq!(summarize(&[WebSearch, Fetch, Fetch]), "Searched the web and fetched 2 pages");
        assert_eq!(summarize(&[WebSearch, WebSearch]), "Searched the web 2 times");
        assert_eq!(summarize(&[Agent, Agent]), "Started 2 agents");
        assert_eq!(summarize(&[]), "Thought it through");
    }

    #[test]
    fn rows_read_like_what_the_agent_did() {
        let cwd = Path::new("/Users/me/code/app");
        let row = |t: &str, d: &str| {
            let o = op(t, d, Some(cwd));
            (o.verb, o.text, o.file)
        };
        assert_eq!(row("Run command", "cd /Users/me/code/app && ls"), (String::new(), "cd /Users/me/code/app && ls".into(), None), "commands verbatim");
        assert_eq!(row("Run command", "cat <<EOF\nhello\nEOF"), (String::new(), "cat <<EOF …".into(), None));
        assert_eq!(row("Read", "/Users/me/code/app/src/auth.rs"), ("Read".into(), "src/auth.rs".into(), Some("src/auth.rs".into())));
        assert_eq!(row("Edit", "src/a.rs, src/b.rs"), ("Edit".into(), "src/a.rs, src/b.rs".into(), Some("src/a.rs".into())));
        assert_eq!(row("Write", "/Users/me/code/app/NOTES.md").0, "Write");
        assert_eq!(row("Search", "generateTitle|titleGen").0, "Find");
        assert_eq!(row("List files", "src/**/*.tsx"), ("Find".into(), "src/**/*.tsx".into(), None));
        assert_eq!(row("Fetch", "https://docs.rs/serde").1, "https://docs.rs/serde");
        assert_eq!(row("Search the web", "gpui shimmer").0, "Search");
        assert_eq!(row("Update plan", "Write the tests").0, "Plan");
        assert_eq!(row("mcp__github__get_issue", "{\"number\":4}").0, "github get_issue");
        assert_eq!(row("Read", "").1, "");
        assert_eq!(op("Edit", "/private/tmp/e2e/notes.txt", Some(Path::new("/tmp/e2e"))).text, "notes.txt", "the same folder by its other name");
        // Outside the project: from home.
        let home = trek_core::paths::home();
        let elsewhere = home.join("notes/todo.md");
        assert_eq!(op("Read", &elsewhere.display().to_string(), Some(cwd)).text, "~/notes/todo.md");
    }

    #[test]
    fn phrases_say_what_is_going_on() {
        assert_eq!(phrase("Read", "src/main.rs", None), "Exploring the project");
        assert_eq!(phrase("Search", "fn main", None), "Exploring the project");
        assert_eq!(phrase("Run command", "cd app && sed -n 1,20p src/x.rs", None), "Exploring the project");
        assert_eq!(phrase("Edit", "/p/src/auth.rs", None), "Editing auth.rs");
        assert_eq!(phrase("Write", "docs/NOTES.md", None), "Writing NOTES.md");
        assert_eq!(phrase("Run command", "cargo test --workspace", None), "Running tests");
        assert_eq!(phrase("Search the web", "rust gpui", None), "Searching the web");
        assert_eq!(phrase("Fetch", "https://www.docs.rs/serde/latest", None), "Reading docs.rs");
        assert_eq!(phrase("Subagent", "Map the routes", None), "Sending agents out");
        assert_eq!(phrase("Update plan", "", None), "Planning");
        // A headline names exploring better than the tool does; a specific call still wins.
        assert_eq!(phrase("Read", "src/main.rs", Some("Tracing the title flow")), "Tracing the title flow");
        assert_eq!(phrase("Edit", "src/main.rs", Some("Tracing the title flow")), "Editing main.rs");
        assert_eq!(phrase("Run command", "./deploy.sh", Some("Shipping it")), "Shipping it", "a command that says little");
        assert_eq!(phrase("Run command", "./deploy.sh", None), "Running commands");
    }

    #[test]
    fn commands_read_as_phrases() {
        for (cmd, want) in [
            ("cargo test -p trek-core", "Running tests"),
            ("cd web && npm test", "Running tests"),
            ("npm run test:unit", "Running tests"),
            ("npx vitest run src", "Running tests"),
            ("uv run pytest -q", "Running tests"),
            ("go test ./...", "Running tests"),
            ("swift test", "Running tests"),
            ("RUST_LOG=debug cargo build --release", "Building"),
            ("pnpm run build", "Building"),
            ("make", "Building"),
            ("cargo clippy --all-targets", "Checking the code"),
            ("npm run typecheck", "Checking the code"),
            ("npx tsc --noEmit", "Checking the code"),
            ("cargo fmt", "Formatting"),
            ("npm install", "Installing dependencies"),
            ("pip install requests", "Installing dependencies"),
            ("git status --short", "Reviewing changes"),
            ("git commit -m wip", "Committing"),
            ("ls -la", "Exploring the project"),
            ("rg -n \"test\" src", "Exploring the project"),
            ("grep -rn test src | head", "Exploring the project"),
            ("cat src/tests.rs", "Exploring the project"),
            ("python3 scripts/gen.py", "Running a script"),
            ("python -m pytest tests", "Running tests"),
            ("curl -s https://example.com", "Calling the network"),
            ("./scripts/deploy.sh", "Running commands"),
            ("", "Exploring the project"),
        ] {
            assert_eq!(command_phrase(cmd), want, "{cmd}");
        }
    }

    #[test]
    fn headlines_come_from_bold_lines_and_headings() {
        assert_eq!(headline("**Exploring the codebase**\n\nI'll look at main.rs."), Some("Exploring the codebase".into()));
        assert_eq!(headline("**First**\n\ntext\n\n**Then the tests**\n\nmore"), Some("Then the tests".into()), "the latest one");
        assert_eq!(headline("## Planning the change:\n"), Some("Planning the change".into()));
        assert_eq!(headline("Just thinking about **this** and that."), None);
        assert_eq!(headline("#hashtag"), None);
        assert_eq!(headline(""), None);
    }

    fn tool(id: &str) -> Item {
        Item::Tool { id: id.into(), title: "Read".into(), detail: "a.rs".into(), output: String::new(), status: ToolStatus::Running }
    }

    #[test]
    fn the_trail_is_the_tail_of_calls_and_thoughts() {
        let said = |t: &str| Item::Assistant { text: t.into() };
        let thought = |t: &str| Item::Reasoning { text: t.into() };
        let user = Item::User { text: "go".into(), images: vec![], at: None, resume: None, aside: false };
        assert_eq!(trail(&[user.clone(), said("hi")]), None);
        assert_eq!(trail(&[user.clone(), thought("**Looking**")]), None, "thoughts alone aren't a group");
        let items = [user.clone(), said("Let me look."), thought("**Exploring the code**"), tool("a"), thought(""), tool("b")];
        assert_eq!(trail(&items), Some(Trail { start: 2, tools: vec![3, 5], headline: Some("Exploring the code".into()) }));
        // An empty thought in front has no row: the group starts at the call.
        let items = [said("x"), thought(" "), tool("a")];
        assert_eq!(trail(&items).map(|t| t.start), Some(2));
        // Text after the calls ends the group.
        assert_eq!(trail(&[tool("a"), said("done")]), None);
    }

    #[test]
    fn line_counts_label_each_side() {
        assert_eq!(lines_label(12, 3), (Some("+12".into()), Some("−3".into())));
        assert_eq!(lines_label(4, 0), (Some("+4".into()), None));
        assert_eq!(lines_label(0, 0), (None, None));
    }
}
