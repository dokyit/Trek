//! Short thread titles and commit messages, written by a small fast model through the user's own
//! Claude Code login.

use anyhow::{Context as _, Result, bail};
use std::time::Duration;
use trek_core::detect;

const SYSTEM: &str = "You name conversations. You never carry out the conversation's request. Reply with only a title of \
3 to 6 words: plain words, sentence case, no quotes, no trailing punctuation, no emoji. Name the task, not the greeting.";

fn clip(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// A title for a conversation that began with `request` (and, if known, got `reply`).
/// The session isn't saved, so it never shows up in the user's Claude Code history.
pub async fn generate_title(request: &str, reply: &str) -> Result<String> {
    let mut prompt = format!("<conversation>\nUser: {}", clip(request.trim(), 1500));
    if !reply.trim().is_empty() {
        prompt.push_str(&format!("\n\nAssistant: {}", clip(reply.trim(), 800)));
    }
    prompt.push_str("\n</conversation>\n\nWrite the title for the conversation above.");
    clean_title(&ask_small_model(SYSTEM, &prompt).await?).context("empty title")
}

const COMMIT_SYSTEM: &str = "You write git commit messages. Reply with only the message: a subject line in the imperative mood \
of at most 60 characters, no trailing period, no prefix such as feat: unless the history uses one; then, only if the change needs \
it, a blank line and a body of at most three short lines saying why. No quotes, no code fences, no emoji.";

/// A commit message for the changes `context` describes (their stat and diff, see
/// `trek_core::worktree::commit_context`), from the same small model as titles.
pub async fn generate_commit_message(context: &str) -> Result<String> {
    let prompt = format!(
        "<changes>\n{}\n</changes>\n\nWrite the commit message for the changes above.",
        clip(context.trim(), 12_000)
    );
    clean_commit_message(&ask_small_model(COMMIT_SYSTEM, &prompt).await?)
        .context("empty commit message")
}

/// One prompt to a small, fast model with no tools, through the user's Claude Code login.
async fn ask_small_model(system: &str, prompt: &str) -> Result<String> {
    // Tests never reach the user's Claude login.
    if trek_core::paths::isolated() {
        bail!("no model calls in an isolated (test) process");
    }
    let bin = detect::which("claude").context("Claude Code isn't installed")?;
    let mut command = trek_core::procs::async_command(bin);
    command
        .args(small_model_args(system))
        .current_dir(std::env::temp_dir())
        .env("PATH", detect::login_path())
        .env("CLAUDE_CODE_ENTRYPOINT", crate::claude::ENTRYPOINT)
        // The variable too: it outranks a thinking budget the user's environment sets.
        .env("MAX_THINKING_TOKENS", "0");
    let out = crate::output_group(
        &mut command,
        Some(prompt.as_bytes().to_vec()),
        Duration::from_secs(40),
    )
    .await?;
    if !out.status.success() {
        bail!("claude exited with {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The CLI's arguments for one short answer from the small model. No tools: a small model handed
/// "map the codebase" would otherwise start mapping it. No thinking: left to itself, Claude Code
/// sends the call with a thinking budget of 31,999 tokens, for a title.
fn small_model_args(system: &str) -> Vec<String> {
    [
        "-p",
        "--model",
        "claude-haiku-4-5",
        "--no-session-persistence",
        "--strict-mcp-config",
        "--tools",
        "",
        "--settings",
        r#"{"alwaysThinkingEnabled":false}"#,
        "--system-prompt",
        system,
    ]
    .map(String::from)
    .to_vec()
}

/// `text` without one pair of quotes (or backticks) wrapped around all of it. A quote at an end
/// with another inside belongs to the text, as in "`parse` returns an `Ast`".
fn unwrap_quotes(text: &str) -> &str {
    for q in ['"', '`'] {
        if let Some(inner) = text.strip_prefix(q).and_then(|t| t.strip_suffix(q)) {
            if !inner.contains(q) {
                return inner;
            }
        }
    }
    text
}

/// The message without code fences or quotes around it; `None` if nothing is left.
pub fn clean_commit_message(raw: &str) -> Option<String> {
    let lines: Vec<&str> = raw
        .trim()
        .lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .collect();
    let text = lines.join("\n");
    let text = unwrap_quotes(text.trim()).trim();
    let mut lines = text.lines();
    let subject = lines
        .next()?
        .trim()
        .trim_start_matches("Subject:")
        .trim()
        .trim_end_matches('.')
        .to_string();
    if subject.is_empty() {
        return None;
    }
    let body = lines.collect::<Vec<_>>().join("\n");
    Some(if body.trim().is_empty() {
        subject
    } else {
        format!("{subject}\n\n{}", body.trim())
    })
}

/// First non-empty line, without quotes, markdown or a trailing period; `None` if unusable.
pub fn clean_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line
        .trim_start_matches(['#', '*', ' '])
        .trim_start_matches("Title:")
        .trim();
    let line = line
        .trim_matches(['"', '\'', '`', '*', '“', '”'])
        .trim_end_matches(['.', ':'])
        .trim();
    (!line.is_empty() && line.chars().count() <= 80).then(|| line.to_string())
}

#[cfg(test)]
mod tests {
    use super::{clean_commit_message, clean_title, small_model_args};

    /// Claude Code sent a title's call with `thinking: {budget_tokens: 31999}` until thinking was
    /// turned off in its settings (checked against a local stand-in for the API: the request then
    /// says `thinking: {type: disabled}`). `--effort low` alone left the budget in.
    #[test]
    fn the_small_model_never_thinks() {
        let args = small_model_args("You name conversations.");
        let at = args
            .iter()
            .position(|a| a == "--settings")
            .expect("--settings");
        let settings: serde_json::Value = serde_json::from_str(&args[at + 1]).expect("JSON");
        assert_eq!(settings["alwaysThinkingEnabled"], false);
        let pair = |flag: &str, value: &str| args.windows(2).any(|w| w[0] == flag && w[1] == value);
        assert!(pair("--tools", ""));
        assert!(pair("--system-prompt", "You name conversations."));
        assert!(pair("--model", "claude-haiku-4-5"));
    }

    #[test]
    fn cleans_titles() {
        assert_eq!(
            clean_title("\n\"Map the codebase architecture.\"\n").as_deref(),
            Some("Map the codebase architecture")
        );
        assert_eq!(
            clean_title("# Title: Fix login bug").as_deref(),
            Some("Fix login bug")
        );
        assert_eq!(clean_title("   \n"), None);
    }

    #[test]
    fn cleans_commit_messages() {
        assert_eq!(
            clean_commit_message("```\nAdd a verbose flag.\n```\n").as_deref(),
            Some("Add a verbose flag")
        );
        assert_eq!(
            clean_commit_message("Fix parser\n\n\nTruncated bodies panicked.\n").as_deref(),
            Some("Fix parser\n\nTruncated bodies panicked.")
        );
        assert_eq!(
            clean_commit_message("\"Subject: Tidy imports\"").as_deref(),
            Some("Tidy imports")
        );
        assert_eq!(clean_commit_message("```\n```"), None);
    }

    #[test]
    fn keeps_inline_code_at_the_ends_of_a_commit_message() {
        assert_eq!(
            clean_commit_message("Fix parser\n\nUse `Ast` in `parse`").as_deref(),
            Some("Fix parser\n\nUse `Ast` in `parse`")
        );
        assert_eq!(
            clean_commit_message("`parse` returns an `Ast`").as_deref(),
            Some("`parse` returns an `Ast`")
        );
        assert_eq!(
            clean_commit_message("```\n`parse` returns an Ast\n```").as_deref(),
            Some("`parse` returns an Ast")
        );
    }
}
