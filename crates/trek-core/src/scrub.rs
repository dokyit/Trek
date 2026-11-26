//! What's taken out of a log line, a panic message or a backtrace before it is persisted as a crash report or leaves: secrets
//! agents were given (`crate::session_secrets`), anything shaped like a key or token, email
//! addresses, web addresses, and paths. The home folder is `~`; a project's folder and what's in it, and any
//! other path that could name the user's files, is `[path]`. Trek's own folders and the system's
//! stay readable, as they name nothing of the user's.

use std::sync::LazyLock;

/// A text with what mustn't leave taken out (`scrub`).
#[derive(Clone)]
pub struct Scrubber {
    /// The home folder, shown as `~`.
    home: Option<String>,
    /// Trek's data and log folders (which may have spaces in them: `Application Support`), and
    /// what each is shown as.
    trek_folders: Vec<(String, &'static str)>,
    /// The user's projects' folders, each with whatever path follows it: never shown, not even
    /// a file's name.
    projects: Vec<regex::Regex>,
    /// Values agents were given this run, `(name, value)`.
    secrets: Vec<(String, String)>,
    /// What takes out keys and tokens by their shape (`redact_secrets`).
    shapes: fn(&str) -> String,
}

impl Scrubber {
    pub fn new(
        home: Option<String>,
        trek_folders: Vec<(String, &'static str)>,
        projects: &[String],
        secrets: Vec<(String, String)>,
        shapes: fn(&str) -> String,
    ) -> Scrubber {
        let mut folders: Vec<&str> = projects
            .iter()
            .map(|p| p.trim_end_matches(['/', '\\']))
            .filter(|p| p.len() > 1)
            .collect();
        // Longest first, so a folder inside another is matched whole.
        folders.sort_by_key(|p| std::cmp::Reverse(p.len()));
        let projects = folders
            .into_iter()
            .filter_map(|p| {
                let p = regex::escape(p);
                // Require a root boundary, preserving it when it isn't a path separator.
                // Spaces can be part of any filename; ambiguous trailing prose is removed too.
                regex::Regex::new(&format!(
                    r#"{p}(?:[/\\][^\r\n\t"'`,;:()\[\]<>|]*|(?P<boundary>[\s"'`]|$))"#,
                ))
                .ok()
            })
            .collect();
        let mut trek_folders: Vec<_> = trek_folders
            .into_iter()
            .filter(|(f, _)| f.len() > 1)
            .collect();
        trek_folders.sort_by_key(|(f, _)| std::cmp::Reverse(f.len()));
        Scrubber {
            home: home.filter(|h| h.len() > 1),
            trek_folders,
            projects,
            secrets,
            shapes,
        }
    }

    pub fn scrub(&self, text: &str) -> String {
        let mut text = crate::session_secrets::scrub(text, &self.secrets).into_owned();
        text = redact_private_keys(&text);
        text = (self.shapes)(&text);
        // Web addresses before emails, so a login in one (`https://me:pw@host`) goes with it.
        text = FILE_URL.replace_all(&text, "[url]").into_owned();
        text = URL
            .replace_all(&text, |c: &regex::Captures| web_address(&c[1], &c[2]))
            .into_owned();
        text = EMAIL.replace_all(&text, "[email]").into_owned();
        for project in &self.projects {
            text = project
                .replace_all(&text, |c: &regex::Captures| {
                    let boundary = c.name("boundary").map_or("", |m| m.as_str());
                    let path = &c[0][..c[0].len() - boundary.len()];
                    let (_, tail) = separate_path_tail(path);
                    // The permissive path matcher includes a separator after a path with spaces.
                    // Keep that separator visible instead of running `[path]` into the next
                    // already-scrubbed value (such as `[url]`).
                    let path = path.trim_end_matches(char::is_whitespace);
                    let gap = &c[0][path.len()..c[0].len() - boundary.len()];
                    format!("[path]{}{}{}", self.scrub(tail.trim_end()), gap, boundary)
                })
                .into_owned();
        }
        for (folder, name) in &self.trek_folders {
            text = text.replace(folder.as_str(), name);
        }
        // A crate's sources, on whichever machine built it, are the crate's: `gpui-0.2/src/x.rs`.
        text = REGISTRY.replace_all(&text, "").into_owned();
        let home = self.home.as_deref();
        text = UNIX_PATH
            .replace_all(&text, |c: &regex::Captures| {
                let path = &c[2];
                if SYSTEM.iter().any(|s| path.starts_with(s)) {
                    // A system path may be followed by a private one. Only keep its first
                    // token, then run the rest through the scrubber again.
                    if let Some(at) = path.find(char::is_whitespace) {
                        return format!("{}{}{}", &c[1], &path[..at], self.scrub(&path[at..]));
                    }
                }
                let (path, tail) = separate_path_tail(path);
                let trimmed = path.trim_end_matches(char::is_whitespace);
                let gap = &path[trimmed.len()..];
                format!(
                    "{}{}{}{}",
                    &c[1],
                    unix_path(trimmed, home),
                    self.scrub(tail),
                    gap
                )
            })
            .into_owned();
        text = WINDOWS_PATH
            .replace_all(&text, |c: &regex::Captures| {
                let path = &c[2];
                if path.to_ascii_lowercase().starts_with(r"c:\windows\") {
                    if let Some(at) = path.find(char::is_whitespace) {
                        return format!("{}{}{}", &c[1], &path[..at], self.scrub(&path[at..]));
                    }
                }
                let (path, tail) = separate_path_tail(path);
                let trimmed = path.trim_end_matches(char::is_whitespace);
                let gap = &path[trimmed.len()..];
                format!(
                    "{}{}{}{}",
                    &c[1],
                    windows_path(trimmed, home),
                    self.scrub(tail),
                    gap
                )
            })
            .into_owned();
        text = HOME_PATH
            .replace_all(&text, |c: &regex::Captures| {
                format!("{}~{}[path]", &c[1], &c[2])
            })
            .into_owned();
        // The home folder where no path was seen (`HOME=/Users/me`).
        match home {
            Some(h) => text.replace(h, "~"),
            None => text,
        }
    }

    /// Scrub with multiline context, then keep the caller's line-oriented representation.
    pub fn lines(&self, lines: &[String]) -> Vec<String> {
        let joined = lines
            .iter()
            .map(|l| l.replace('\n', " "))
            .collect::<Vec<_>>()
            .join("\n");
        self.scrub(&joined).lines().map(str::to_owned).collect()
    }
}

static PRIVATE_KEY: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|$)|\A(?:[A-Za-z0-9+/=]{16,}\r?\n)+-----END [A-Z0-9 ]*PRIVATE KEY-----",
    )
    .unwrap()
});

/// Run before selecting/clipping diagnostics so PEM bodies cannot lose their header.
/// A partial block (including one whose BEGIN fell outside a log read) is removed too.
pub fn redact_private_keys(text: &str) -> String {
    PRIVATE_KEY
        .replace_all(text, "[redacted private key]")
        .into_owned()
}

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
        r"|eyJ[A-Za-z0-9_\-]*\.[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]*)",
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

const REDACTED: &str = "[redacted]";

/// `text` without what looks like a private key, an API key or token, or a password.
pub fn redact_secrets(text: &str) -> String {
    let text = redact_private_keys(text);
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

/// Where the system's own files are: they name nothing of the user's.
const SYSTEM: &[&str] = &[
    "/usr/",
    "/bin/",
    "/sbin/",
    "/opt/homebrew/",
    "/System/",
    "/Library/",
    "/Applications/",
    "/rustc/",
    "/dev/",
];

/// Hosts Trek itself, or the providers its agents talk to, contact: a web address on one of them
/// keeps its scheme and host (so "the update check to github.com failed" stays readable) and
/// loses everything after it. Any other web address is somewhere the user went, which is theirs.
const KNOWN_HOSTS: &[&str] = &[
    "github.com",
    "api.github.com",
    "raw.githubusercontent.com",
    "objects.githubusercontent.com",
    "registry.npmjs.org",
    "formulae.brew.sh",
    "cdn.agentclientprotocol.com",
    "downloads.claude.ai",
    "nodejs.org",
    "huggingface.co",
    "us.i.posthog.com",
    "ntfy.sh",
    "trekdesktop.com",
    "api.anthropic.com",
    "api.openai.com",
    "generativelanguage.googleapis.com",
    "openrouter.ai",
    "api.deepseek.com",
    "api.x.ai",
    "api.mistral.ai",
    "api.groq.com",
    // This computer's own servers (a model server, a dev server): no name of anyone's.
    "localhost",
    "127.0.0.1",
];

/// A web address cut down to `[url]`, or to `https://host` when the host is a known one. What
/// ends a sentence after it (`.`, `,`, `:`) stays after it.
fn web_address(scheme: &str, rest: &str) -> String {
    let mut body = rest.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    // Keep a log's closing wrapper, but consume balanced parentheses/brackets in the URL.
    while let Some(close) = body.chars().last().filter(|c| matches!(c, ')' | ']')) {
        let open = if close == ')' { '(' } else { '[' };
        if body.matches(close).count() <= body.matches(open).count() {
            break;
        }
        body = &body[..body.len() - 1];
    }
    let tail = &rest[body.len()..];
    let authority = body.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let plain = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
    // `user:pass@host` is never kept, even on a known host.
    if plain && !authority.contains('@') && KNOWN_HOSTS.contains(&host.as_str()) {
        format!("{}://{authority}{tail}", scheme.to_ascii_lowercase())
    } else {
        format!("[url]{tail}")
    }
}

fn unix_path(path: &str, home: Option<&str>) -> String {
    if SYSTEM.iter().any(|s| path.starts_with(s)) {
        return path.to_string();
    }
    match home {
        Some(h) if path == h => "~".into(),
        Some(h) if path.starts_with(&format!("{h}/")) => "~/[path]".into(),
        _ => "[path]".into(),
    }
}

fn windows_path(path: &str, home: Option<&str>) -> String {
    let lower = path.to_ascii_lowercase();
    if lower.starts_with(r"c:\windows\") {
        return path.to_string();
    }
    match home.map(str::to_ascii_lowercase) {
        Some(h) if lower == h => "~".into(),
        Some(h) if lower.starts_with(&format!("{h}\\")) => r"~\[path]".into(),
        _ => "[path]".into(),
    }
}

/// The permissive path patterns allow spaces inside a filename. Split only the unambiguous
/// prose or a second absolute path that a diagnostic appends after the first one, so neither is
/// swallowed by the first replacement.
fn separate_path_tail(path: &str) -> (&str, &str) {
    for separator in [" and /", " or /"] {
        if let Some(at) = path.find(separator) {
            return (&path[..at], &path[at..]);
        }
    }
    for suffix in [
        " failed",
        " not found",
        " locked",
        " denied",
        " refused",
        " crashed",
    ] {
        if let Some(at) = path.find(suffix) {
            return (&path[..at], &path[at..]);
        }
    }
    (path, "")
}

/// A web address: a scheme, `://`, and up to a space, a quote or a bracket (a log line's own
/// `({url})`). It takes in `file://` addresses too, which hold paths.
static URL: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"([A-Za-z][A-Za-z0-9+.\-]*)://([^\s"'`<>]+)"#).unwrap());

/// A file URL, which can contain unescaped spaces.
static FILE_URL: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(?i)file://[^\r\n\t"'`<>\[\]()]+"#).unwrap());

/// A crash's message without the text a panic quotes: Rust's own messages quote what they were
/// working on between backticks (``byte index 9 is not a char boundary; … of `<the text>` ``, up to
/// 256 characters of it), which may be what the user wrote or a file of theirs. What a report
/// needs is where and why, so a quoted span with a space in it or longer than a name is written
/// `[text]`; a name (`Option::unwrap()`, `None`) stays.
pub fn without_quoted_text(message: &str) -> String {
    QUOTED
        .replace_all(message, |c: &regex::Captures| {
            let inner = &c[1];
            if inner.chars().count() > 48 || inner.chars().any(char::is_whitespace) {
                "`[text]`".to_string()
            } else {
                c[0].to_string()
            }
        })
        .into_owned()
}

static QUOTED: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"`([^`]*)`").unwrap());

static EMAIL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"[\p{L}\p{M}\p{N}._%+\-]+@[\p{L}\p{M}\p{N}\-]+(?:\.[\p{L}\p{M}\p{N}\-]+)*")
        .unwrap()
});

/// A crate in cargo's registry: everything up to the crate's own folder goes.
static REGISTRY: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?:[A-Za-z]:)?[^\s:()\[\]<>]*[/\\]\.cargo[/\\]registry[/\\]src[/\\][^/\\\s]+[/\\]",
    )
    .unwrap()
});

/// An absolute path of two or more parts, where a path can start (as `share::render` has it).
static UNIX_PATH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(^|[\s(\[<"'`=:,*])(/[^\r\n\t/()\[\]<>"'`,;:*|]+(?:/[^\r\n\t/()\[\]<>"'`,;:*|]*)+)"#,
    )
    .unwrap()
});

static WINDOWS_PATH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(^|[\s(\[<"'`=,*])([A-Za-z]:\\[^\r\n\t\\()\[\]<>"'`,;*|]+(?:\\[^\r\n\t\\()\[\]<>"'`,;*|]*)+)"#,
    )
    .unwrap()
});

/// A path already written from the home folder (`~/code/app`).
static HOME_PATH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(^|[\s(\[<"'`=:,*])~([/\\])[^\r\n\t()\[\]<>"'`,;*|]+"#).unwrap()
});

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the app's shape matcher: takes out `sk-…` words.
    fn shapes(text: &str) -> String {
        text.split(' ')
            .map(|w| {
                if w.starts_with("sk-") {
                    "[redacted]"
                } else {
                    w
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn scrubber() -> Scrubber {
        Scrubber::new(
            Some("/Users/me".into()),
            vec![
                ("/Users/me/Library/Logs/Trek".into(), "[Trek logs]"),
                (
                    "/Users/me/Library/Application Support/dev.trek.Trek".into(),
                    "[Trek data]",
                ),
            ],
            &[
                "/Users/me/code/acme-secret".into(),
                "/Volumes/work/My Client/".into(),
            ],
            vec![("GITHUB_TOKEN".into(), "hunter2-value".into())],
            shapes,
        )
    }

    #[test]
    fn paths_say_where_without_naming_the_users_files() {
        let s = scrubber();
        // A project's folder never shows, nor what's in it.
        assert_eq!(
            s.scrub("open /Users/me/code/acme-secret/src/plan.rs: not found"),
            "open [path]: not found"
        );
        // Spaces and all; and no file's name anywhere else either.
        assert_eq!(
            s.scrub(
                "read /Volumes/work/My Client/notes.md, /Users/me/Desktop/taxes.pdf and /tmp/x/y"
            ),
            "read [path], ~/[path] and [path]"
        );
        assert_eq!(s.scrub("cwd ~/code/app/src"), "cwd ~/[path]");
        assert_eq!(s.scrub("HOME=/Users/me"), "HOME=~");
        // Trek's own folders, spaces and all, and the system's, stay readable.
        assert_eq!(
            s.scrub(
                "database: /Users/me/Library/Application Support/dev.trek.Trek/trek.sqlite locked"
            ),
            "database: [Trek data]/trek.sqlite locked"
        );
        assert_eq!(
            s.scrub("ran /opt/homebrew/bin/codex and /usr/bin/git"),
            "ran /opt/homebrew/bin/codex and /usr/bin/git"
        );
        // A crate's sources are named from the crate, on whichever machine built them.
        assert_eq!(
            s.scrub("at /Users/runner/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.7/src/window.rs:12"),
            "at gpui-pre-0.3.7/src/window.rs:12"
        );
        // Trek's own source paths are relative, and say nothing of the user.
        assert_eq!(
            s.scrub("panicked at crates/trek-app/src/root.rs:120:5"),
            "panicked at crates/trek-app/src/root.rs:120:5"
        );
    }

    #[test]
    fn web_addresses_keep_at_most_a_known_host() {
        let s = scrubber();
        // Where the user went is theirs: the whole address goes, path, query and all.
        assert_eq!(
            s.scrub("browser: https://bank.example/accounts/4417?tab=cards failed: NSURLErrorDomain -1003"),
            "browser: [url] failed: NSURLErrorDomain -1003"
        );
        assert_eq!(
            s.scrub("the page's process ended (https://intranet.acme-secret.com/wiki/Plan#top)"),
            "the page's process ended ([url])"
        );
        // A sentence going on after one is kept.
        assert_eq!(s.scrub("see https://example.org/a/b."), "see [url].");
        assert_eq!(
            s.scrub("went to http://192.168.1.20:8080/admin, then"),
            "went to [url], then"
        );
        assert_eq!(
            s.scrub("at /Users/me/private/file.rs https://private.example/path"),
            "at ~/[path] [url]"
        );
        // A file address holds a path, and goes whole.
        assert_eq!(s.scrub("open file:///Users/me/taxes.pdf"), "open [url]");
        // Trek's own hosts and the providers' APIs keep the host, never the path or query.
        assert_eq!(
            s.scrub("update check: https://github.com/dokyit/Trek/releases/latest/download/stable.json: 503"),
            "update check: https://github.com: 503"
        );
        assert_eq!(
            s.scrub("POST https://api.anthropic.com/v1/messages?beta=1 -> 401"),
            "POST https://api.anthropic.com -> 401"
        );
        assert_eq!(
            s.scrub("model server http://localhost:11434/api/tags refused"),
            "model server http://localhost:11434 refused"
        );
        // A look-alike host is not a known one, and neither is one with a login in front.
        assert_eq!(s.scrub("https://github.com.evil.example/x"), "[url]");
        assert_eq!(s.scrub("https://me:pw@github.com/x"), "[url]");
        assert_eq!(s.scrub("ws://localhost:3000/socket"), "[url]");
    }

    #[test]
    fn windows_paths_too() {
        let s = Scrubber::new(
            Some(r"C:\Users\me".into()),
            vec![],
            &[r"D:\work\client".into()],
            vec![],
            shapes,
        );
        assert_eq!(
            s.scrub(r"open C:\Users\me\code\app\x.rs failed"),
            r"open ~\[path] failed"
        );
        assert_eq!(
            s.scrub(r"open D:\work\client\x.rs failed"),
            r"open [path] failed"
        );
        assert_eq!(s.scrub(r"open E:\other\x.rs failed"), r"open [path] failed");
        assert_eq!(
            s.scrub(r"C:\Windows\System32\cmd.exe"),
            r"C:\Windows\System32\cmd.exe"
        );
    }

    /// A panic that quotes the user's text says where and why, not what the text was.
    #[test]
    fn a_crash_says_why_without_the_text_it_quoted() {
        // What `&s[..n]` says when `n` falls inside a character.
        let message = "byte index 9 is not a char boundary; it is inside 'é' (bytes 8..10) of `Dear Sam, the merger closes Friday`";
        assert_eq!(
            without_quoted_text(message),
            "byte index 9 is not a char boundary; it is inside 'é' (bytes 8..10) of `[text]`"
        );
        let long = format!("index out of bounds of `{}`[...]", "x".repeat(200));
        assert_eq!(
            without_quoted_text(&long),
            "index out of bounds of `[text]`[...]"
        );
        let named = "called `Option::unwrap()` on a `None` value";
        assert_eq!(without_quoted_text(named), named);
    }

    #[test]
    fn secrets_and_emails_never_leave() {
        let s = scrubber();
        let line = s.scrub(
            "token hunter2-value from me@example.com, key sk-abc123 and ann.lee+x@mail.co.uk",
        );
        assert_eq!(
            line,
            "token [GITHUB_TOKEN hidden] from [email], key [redacted] and [email]"
        );
        // Lines stay lines.
        assert_eq!(
            s.lines(&["a\nb hunter2-value".into()]),
            ["a b [GITHUB_TOKEN hidden]"]
        );
    }

    #[test]
    fn paths_with_spaces_and_project_prefixes_are_removed_completely() {
        let s = Scrubber::new(
            Some("/Users/me".into()),
            vec![],
            &[
                "/Users/me/code/app".into(),
                "/Volumes/work/My Client".into(),
            ],
            vec![],
            str::to_owned,
        );
        for (input, expected) in [
            (
                "open /Users/me/Library/Application Support/Slack/Cookies",
                "open ~/[path]",
            ),
            ("file:///Users/me/My File.pdf", "[url]"),
            (r"C:\Users\me\My Documents\x", "[path]"),
            ("/Users/me/code/app2/secret.txt", "~/[path]"),
            ("/Volumes/work/My Client/my docs/x.pdf", "[path]"),
            ("'/Users/me/code/app'", "'[path]'"),
            ("/Users/me/code/app end", "[path] end"),
            ("/Users/me/code/app", "[path]"),
            (
                "/usr/bin/git /Users/me/My File.pdf",
                "/usr/bin/git ~/[path]",
            ),
        ] {
            assert_eq!(s.scrub(input), expected, "{input}");
        }
        // This checks the project matcher itself, rather than letting the generic path
        // scrubber hide an incorrect prefix match.
        assert!(
            !s.projects
                .iter()
                .any(|p| p.is_match("/Users/me/code/app2/secret.txt"))
        );
    }

    #[test]
    fn urls_and_international_or_local_emails_are_removed_completely() {
        let s = scrubber();
        for url in [
            "http://[::1]:8080/private?token=secret",
            "https://example.com/a(b)c?private=yes",
            "https://example.com/a[b]c?private=yes",
        ] {
            assert_eq!(s.scrub(url), "[url]", "{url}");
        }
        assert_eq!(
            s.scrub("https://github.com/a(b)c?q=secret"),
            "https://github.com"
        );
        assert_eq!(
            s.scrub("élise@例え.テスト user@localhost"),
            "[email] [email]"
        );
    }

    #[test]
    fn multiline_private_keys_are_removed_before_lines_are_separated() {
        let s = scrubber();
        for kind in [
            "PRIVATE KEY",
            "RSA PRIVATE KEY",
            "EC PRIVATE KEY",
            "OPENSSH PRIVATE KEY",
        ] {
            let lines = vec![
                format!("WARN -----BEGIN {kind}-----"),
                "sensitiveBase64Body123456".into(),
                format!("-----END {kind}-----"),
                "after".into(),
            ];
            let output = s.lines(&lines).join("\n");
            assert!(!output.contains("sensitiveBase64"));
            assert!(output.contains("[redacted private key]") && output.ends_with("after"));
        }
    }
}
