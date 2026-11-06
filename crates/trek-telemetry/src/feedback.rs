//! Feedback and bug reports (Help › Send Feedback…): what the user wrote, an email if they'd
//! like a reply, and, unless they turn it off, diagnostics: the version, OS and architecture,
//! the recent errors and the end of Trek's log, with paths and secrets taken out. It goes to
//! PostHog as a `feedback` event (sending it is the consent), or as text the user copies, or as
//! a GitHub issue they file themselves.

use crate::event::{self, Platform};
use crate::scrub::Scrubber;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Lines from the end of Trek's log a report carries.
pub const LOG_LINES: usize = 200;
/// Recent warnings and errors a report carries.
pub const ERROR_LINES: usize = 20;
/// A log line is cut to this many characters.
const LINE_MAX: usize = 400;
/// A pre-filled GitHub issue link is kept under this (GitHub turns longer ones away).
const ISSUE_URL_MAX: usize = 7_500;
/// Where "Report a bug on GitHub" files it.
pub const ISSUES: &str = "https://github.com/dokyit/Trek/issues/new";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bug,
    Idea,
    Other,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Bug => "bug",
            Kind::Idea => "idea",
            Kind::Other => "other",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Bug => "Bug",
            Kind::Idea => "Idea",
            Kind::Other => "Other",
        }
    }
}

/// What "Include diagnostics" adds.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostics {
    pub platform: Platform,
    /// Warnings and errors from the last two days' logs, newest last.
    pub recent_errors: Vec<String>,
    /// The end of the newest log.
    pub log_tail: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Feedback {
    pub kind: Kind,
    pub message: String,
    /// Only when the user gave one, for a reply.
    pub email: Option<String>,
    pub diagnostics: Option<Diagnostics>,
}

/// Why feedback didn't go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotSent {
    /// This build has no PostHog key.
    NoKey,
    /// A test or the isolated verify profile, where nothing leaves.
    TestProfile,
    /// The network or PostHog.
    Failed(String),
}

impl NotSent {
    pub fn message(&self) -> String {
        match self {
            NotSent::NoKey => {
                "This build of Trek has nowhere to send reports yet. Copy the report and email it, or report it on GitHub.".into()
            }
            NotSent::TestProfile => {
                "This is a test profile, so nothing is sent. Copy the report instead.".into()
            }
            NotSent::Failed(why) => {
                format!("Couldn't send it ({why}). Try again, or copy the report.")
            }
        }
    }
}

/// Whether `email` could be an address: one `@`, a dot after it, no spaces.
pub fn plausible_email(email: &str) -> bool {
    let email = email.trim();
    match email.split_once('@') {
        Some((name, domain)) => {
            !name.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && !email.contains(char::is_whitespace)
        }
        None => false,
    }
}

impl Feedback {
    /// The `feedback` event's properties: the machine only with diagnostics.
    pub fn properties(&self) -> Map<String, Value> {
        let mut p = event::base(self.diagnostics.as_ref().map(|d| &d.platform));
        p.insert("kind".into(), json!(self.kind.key()));
        p.insert("message".into(), json!(self.message.trim()));
        if let Some(email) = self
            .email
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            p.insert("email".into(), json!(email));
        }
        p.insert("diagnostics".into(), json!(self.diagnostics.is_some()));
        if let Some(d) = &self.diagnostics {
            p.insert("recent_errors".into(), json!(d.recent_errors));
            p.insert("log_tail".into(), json!(d.log_tail.join("\n")));
        }
        p
    }

    /// The report as text: what the form shows before sending, and what Copy report copies.
    pub fn report(&self) -> String {
        let mut out = format!(
            "Trek feedback: {}\n\n{}\n",
            self.kind.label(),
            self.message.trim()
        );
        if let Some(email) = self
            .email
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            out.push_str(&format!("\nReply to: {email}\n"));
        }
        match &self.diagnostics {
            None => out.push_str("\nNo diagnostics.\n"),
            Some(d) => {
                out.push_str(&format!(
                    "\nTrek {} ({})\n{}\n",
                    d.platform.app_version,
                    d.platform.channel,
                    d.platform.describe()
                ));
                out.push_str("\nRecent errors\n");
                if d.recent_errors.is_empty() {
                    out.push_str("None\n");
                }
                for line in &d.recent_errors {
                    out.push_str(line);
                    out.push('\n');
                }
                out.push_str(&format!("\nLog (last {} lines)\n", d.log_tail.len()));
                for line in &d.log_tail {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        out
    }

    /// A new GitHub issue, filled in: the title, what the user wrote and the diagnostics, never
    /// their email (an issue is public). The log is cut from its start to fit a link.
    pub fn github_issue_url(&self) -> String {
        let message = self.message.trim();
        let first = message.lines().next().unwrap_or_default();
        let mut title: String = first.chars().take(80).collect();
        if title.len() < first.len() {
            title.push('…');
        }
        let label = match self.kind {
            Kind::Bug => "bug",
            Kind::Idea => "enhancement",
            Kind::Other => "",
        };
        let mut errors = self
            .diagnostics
            .as_ref()
            .map(|d| d.recent_errors.clone())
            .unwrap_or_default();
        let mut log = self
            .diagnostics
            .as_ref()
            .map(|d| d.log_tail.clone())
            .unwrap_or_default();
        loop {
            let body = self.issue_body(message, &errors, &log);
            let mut params = vec![("title", title.as_str()), ("body", body.as_str())];
            if !label.is_empty() {
                params.push(("labels", label));
            }
            let url = reqwest::Url::parse_with_params(ISSUES, &params)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| ISSUES.to_string());
            if url.len() <= ISSUE_URL_MAX || (log.is_empty() && errors.is_empty()) {
                return url;
            }
            // The log's oldest lines go first, then the oldest errors.
            if !log.is_empty() {
                let drop = (log.len() / 4).max(1);
                log.drain(..drop);
            } else {
                errors.remove(0);
            }
        }
    }

    fn issue_body(&self, message: &str, errors: &[String], log: &[String]) -> String {
        let mut body = format!("{message}\n");
        let Some(d) = &self.diagnostics else {
            return body;
        };
        body.push_str(&format!(
            "\n**Trek** {} ({}) · {}\n",
            d.platform.app_version,
            d.platform.channel,
            d.platform.describe()
        ));
        if !errors.is_empty() {
            body.push_str(&format!(
                "\n<details><summary>Recent errors</summary>\n\n```\n{}\n```\n</details>\n",
                errors.join("\n")
            ));
        }
        if !log.is_empty() {
            body.push_str(&format!(
                "\n<details><summary>Log (last {} lines)</summary>\n\n```\n{}\n```\n</details>\n",
                log.len(),
                log.join("\n")
            ));
        }
        body
    }
}

/// The diagnostics, read now from the log folder (off the UI thread: it reads files).
pub fn diagnostics(platform: Platform, log_dir: &Path, scrubber: &Scrubber) -> Diagnostics {
    let logs = newest_logs(log_dir);
    Diagnostics {
        platform,
        recent_errors: scrubber.lines(&recent_errors(&logs, ERROR_LINES)),
        log_tail: scrubber.lines(&log_tail(&logs, LOG_LINES)),
    }
}

/// `trek-<date>.log` files, newest first.
fn newest_logs(log_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return vec![];
    };
    let mut logs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("trek-") && n.ends_with(".log"))
        })
        .collect();
    logs.sort();
    logs.reverse();
    logs
}

fn lines_of(path: &Path) -> Vec<String> {
    // The end of the file is what's wanted: a day's log can be long.
    const TAIL_BYTES: u64 = 512 * 1024;
    use std::io::{Read as _, Seek as _};
    let Ok(mut file) = std::fs::File::open(path) else {
        return vec![];
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(TAIL_BYTES);
    let _ = file.seek(std::io::SeekFrom::Start(from));
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text.lines().map(clip).collect();
    // A cut-off first line is half a line.
    if from > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    lines
}

fn clip(line: &str) -> String {
    match line.char_indices().nth(LINE_MAX) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    }
}

/// The last `n` lines over the newest logs (yesterday's too, if today's is short).
fn log_tail(logs: &[PathBuf], n: usize) -> Vec<String> {
    let mut tail: Vec<String> = Vec::new();
    for log in logs.iter().take(2) {
        let mut lines = lines_of(log);
        let room = n - tail.len();
        if lines.len() > room {
            lines.drain(..lines.len() - room);
        }
        lines.append(&mut tail);
        tail = lines;
        if tail.len() >= n {
            break;
        }
    }
    tail
}

/// The last `n` warnings and errors in the last two days' logs, panics among them.
fn recent_errors(logs: &[PathBuf], n: usize) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for log in logs.iter().take(2).rev() {
        found.extend(
            lines_of(log)
                .into_iter()
                .filter(|l| l.contains(" ERROR ") || l.contains(" WARN ")),
        );
    }
    if found.len() > n {
        found.drain(..found.len() - n);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform() -> Platform {
        Platform {
            os: "macOS",
            os_version: Some("26.1".into()),
            arch: "aarch64",
            app_version: "0.4.41".into(),
            channel: "stable".into(),
        }
    }

    fn scrubber() -> Scrubber {
        Scrubber::new(
            Some("/Users/me".into()),
            vec![],
            &["/Users/me/code/app".into()],
            vec![("NPM_TOKEN".into(), "npm-secret-1".into())],
            |s| s.to_string(),
        )
    }

    fn logs() -> PathBuf {
        let dir = crate::tests::scratch("logs");
        let mut today = String::new();
        for i in 0..250 {
            today.push_str(&format!(
                "2026-11-05T10:00:{i:02}Z  INFO trek_app: line {i}\n"
            ));
        }
        today.push_str("2026-11-05T10:05:00Z ERROR trek_app::workspace: open /Users/me/code/app/a.rs failed for me@example.com with npm-secret-1\n");
        std::fs::write(dir.join("trek-2026-11-05.log"), today).unwrap();
        std::fs::write(
            dir.join("trek-2026-11-04.log"),
            "2026-11-04T08:00:00Z  WARN trek_core: yesterday's warning\n2026-11-04T08:00:01Z  INFO trek_core: fine\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn diagnostics_are_the_end_of_the_log_and_its_errors_scrubbed() {
        let dir = logs();
        let d = diagnostics(platform(), &dir, &scrubber());
        assert_eq!(d.log_tail.len(), LOG_LINES);
        assert!(d.log_tail[0].ends_with("line 51"), "{}", d.log_tail[0]);
        let last = d.log_tail.last().unwrap();
        assert_eq!(
            last,
            "2026-11-05T10:05:00Z ERROR trek_app::workspace: open [path] failed for [email] with [NPM_TOKEN hidden]"
        );
        assert_eq!(d.recent_errors.len(), 2);
        assert!(d.recent_errors[0].contains("yesterday's warning"));
        assert_eq!(&d.recent_errors[1], last);
        // A short log reaches back into yesterday's.
        let short = log_tail(&newest_logs(&dir)[1..], LOG_LINES);
        assert_eq!(short.len(), 2);
    }

    fn bug(diagnostics: Option<Diagnostics>) -> Feedback {
        Feedback {
            kind: Kind::Bug,
            message: "  The sidebar froze after I opened a thread\nThen it came back.  ".into(),
            email: Some("me@example.com".into()),
            diagnostics,
        }
    }

    #[test]
    fn the_feedback_event_carries_the_machine_only_with_diagnostics() {
        let without = bug(None).properties();
        assert_eq!(without["kind"], "bug");
        assert_eq!(
            without["message"],
            "The sidebar froze after I opened a thread\nThen it came back."
        );
        assert_eq!(without["email"], "me@example.com");
        assert_eq!(without["diagnostics"], false);
        assert!(!without.contains_key("$os") && !without.contains_key("log_tail"));
        let d = diagnostics(platform(), &logs(), &scrubber());
        let with = bug(Some(d)).properties();
        assert_eq!(with["$os"], "macOS");
        assert_eq!(with["$app_version"], "0.4.41");
        assert_eq!(
            with["log_tail"].as_str().unwrap().lines().count(),
            LOG_LINES
        );
        // An empty email is no email.
        let mut f = bug(None);
        f.email = Some("  ".into());
        assert!(!f.properties().contains_key("email"));
    }

    #[test]
    fn the_report_says_everything_that_would_be_sent() {
        let d = diagnostics(platform(), &logs(), &scrubber());
        let report = bug(Some(d)).report();
        assert!(report.starts_with("Trek feedback: Bug\n\nThe sidebar froze"));
        assert!(report.contains("Reply to: me@example.com"));
        assert!(report.contains("Trek 0.4.41 (stable)\nmacOS 26.1, aarch64"));
        assert!(report.contains("Log (last 200 lines)"));
        assert!(!report.contains("/Users/me"));
        assert!(bug(None).report().ends_with("No diagnostics.\n"));
    }

    #[test]
    fn a_github_issue_is_filled_in_fits_a_link_and_leaves_the_email_out() {
        let d = diagnostics(platform(), &logs(), &scrubber());
        let url = bug(Some(d)).github_issue_url();
        assert!(
            url.starts_with("https://github.com/dokyit/Trek/issues/new?title=The+sidebar+froze")
        );
        assert!(url.len() <= ISSUE_URL_MAX, "{}", url.len());
        assert!(url.contains("labels=bug"));
        let parsed = reqwest::Url::parse(&url).unwrap();
        let body: String = parsed
            .query_pairs()
            .find(|(k, _)| k == "body")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        assert!(body.contains("**Trek** 0.4.41 (stable) · macOS 26.1, aarch64"));
        assert!(body.contains("Recent errors"));
        assert!(!body.contains("me@example.com"), "an issue is public");
        // The newest log lines are the ones kept.
        assert!(body.contains("line 249"));
        let idea = Feedback {
            kind: Kind::Idea,
            ..bug(None)
        };
        assert!(idea.github_issue_url().contains("labels=enhancement"));
    }

    #[test]
    fn an_email_has_to_look_like_one() {
        for good in ["me@example.com", " a.b+c@mail.co.uk "] {
            assert!(plausible_email(good), "{good}");
        }
        for bad in [
            "",
            "me",
            "me@",
            "@example.com",
            "me@example",
            "me@@x.com",
            "m e@x.com",
            "me@x.",
        ] {
            assert!(!plausible_email(bad), "{bad}");
        }
    }
}
