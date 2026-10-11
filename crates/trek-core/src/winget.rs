//! The newest version of a WinGet package that `winget upgrade` can install: what the local
//! `winget` says its source has (`winget show --versions`), not what winget-pkgs has. WinGet's
//! own source index follows winget-pkgs by hours, and an update offered before it has it only
//! fails; winget-pkgs is asked over GitHub's API, with a request budget shared by everything
//! behind the same address (`feeds`), only when `winget` can't answer.
//!
//! `winget show --exact --id <id> --versions` prints the package's name, then a table: a header
//! (translated into the user's language), a row of dashes, and a version a row, the newest first.
//! Nothing but the row of dashes is relied on, so the language doesn't matter. It reads only: no
//! source agreement is left to answer (`--accept-source-agreements`, `--disable-interactivity`),
//! and nothing is installed.

use crate::agent_update::{Latest, parse_version};
use std::time::Duration;

/// A first run may have WinGet bring its source up to date first (a few seconds); the whole
/// check waits for the slowest agent, so a `winget` that hangs isn't waited for long.
const TIMEOUT: Duration = Duration::from_secs(30);

/// One `winget` at a time: its commands queue on the source database anyway, and each of Trek's
/// WinGet installs asking at once would only have them wait on one another.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A version's name, as WinGet names it (a row of its table, a folder of winget-pkgs):
/// "2.1.292", Copilot's "v1.0.95". A name of anything else isn't one.
pub(crate) fn version_row(row: &str) -> Option<String> {
    let version = parse_version(row)?;
    (version == row.trim_start_matches(['v', 'V'])).then_some(version)
}

/// The newest version in `output`, the text of `winget show --versions`; `None` when it isn't a
/// table of versions (no such package, a failure, a language of another shape).
pub fn parse_versions(output: &str) -> Option<Latest> {
    // A row ends at `\r\n` or `\n`; a lone `\r` is a progress spinner redrawing, one row too.
    let output = output.replace("\r\n", "\n").replace('\r', "\n");
    let mut rows = output.lines().map(str::trim);
    rows.find(|row| row.len() >= 3 && row.bytes().all(|b| b == b'-'))?;
    let versions: Vec<String> = rows.take_while(|row| !row.is_empty()).filter_map(version_row).collect();
    versions.into_iter().reduce(|a, b| if crate::agent_update::is_newer(&b, &a) { b } else { a }).map(|version| Latest { version, auto_updates: false })
}

/// What `program` (`winget`) says is `id`'s newest version, `None` when it can't say: not
/// installed, a package its source doesn't have, offline, too slow.
pub async fn latest(program: &str, id: &str) -> Option<Latest> {
    let _one = ONE_AT_A_TIME.lock().await;
    let run = crate::procs::async_command(program)
        .args(["show", "--exact", "--id", id, "--versions", "--source", "winget", "--disable-interactivity", "--accept-source-agreements"])
        .env("PATH", crate::detect::login_path())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(TIMEOUT, run).await.ok()?.ok()?;
    if !out.status.success() {
        tracing::info!("winget couldn't say {id}'s versions: {}", out.status);
        return None;
    }
    parse_versions(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real output of `winget show --exact --id Anthropic.ClaudeCode --versions --source winget
    // --disable-interactivity --accept-source-agreements` (winget v1.29.380, Windows 11, English,
    // 2026-10-11), standard output with its Windows line ends; the list shortened.
    const CLAUDE: &str = "Found Claude Code [Anthropic.ClaudeCode]\r\nVersion\r\n-------\r\n2.1.292\r\n2.1.291\r\n2.1.289\r\n2.1.288\r\n2.1.287\r\n2.1.286\r\n2.1.285\r\n2.1.284\r\n2.1.283\r\n";
    // The same for `GitHub.Copilot`: its versions are named with a "v".
    const COPILOT: &str = "Found Copilot CLI [GitHub.Copilot]\r\nVersion\r\n--------\r\nv1.0.95\r\nv1.0.94\r\nv1.0.93\r\nv1.0.92\r\nv1.0.91\r\n";
    // What it prints for an id its source doesn't have (exit code 0x8A150014).
    const NONE: &str = "No package found matching input criteria.\r\n";

    fn version(output: &str) -> Option<String> {
        parse_versions(output).map(|l| l.version)
    }

    #[test]
    fn the_newest_version_is_read_from_winget_shows_table() {
        assert_eq!(version(CLAUDE).as_deref(), Some("2.1.292"));
        assert_eq!(version(COPILOT).as_deref(), Some("1.0.95"), "a `v` is not part of the version");
        assert_eq!(version(NONE), None);
        assert_eq!(version(""), None);
        // Newest by version, not by position: a list in any order, a pre-release among them.
        assert_eq!(version("Found X [A.B]\nVersion\n-------\n1.9.0\n1.10.0\n1.2.0\n").as_deref(), Some("1.10.0"));
        assert_eq!(version("Found X [A.B]\nVersion\n-------\n2.0.0-beta.1\n1.9.0\n").as_deref(), Some("2.0.0-beta.1"));
        assert_eq!(version("Found X [A.B]\nVersion\n-------\n2.0.0-beta.1\n2.0.0\n").as_deref(), Some("2.0.0"), "a release after its pre-release");
        // Unix line ends, and a header in another language, change nothing.
        assert_eq!(version("Encontrado Claude Code [Anthropic.ClaudeCode]\nVersión\n-------\n2.1.292\n2.1.291\n").as_deref(), Some("2.1.292"));
        assert_eq!(version("Gefunden X [A.B]\nVersion\n----------\n3.0.1\n").as_deref(), Some("3.0.1"));
    }

    #[test]
    fn only_the_table_under_the_dashes_is_read() {
        // A version-like word above the table (a package's name, a notice) isn't a version.
        assert_eq!(version("Found Tool 9.9.9 [A.B]\nVersion\n-------\n1.2.3\n").as_deref(), Some("1.2.3"));
        assert_eq!(version("Notice 9.9.9\n-------\n"), None);
        // Nothing after the table is: a blank line ends it.
        assert_eq!(version("Found X [A.B]\nVersion\n-------\n1.2.3\n\nSee 9.9.9\n").as_deref(), Some("1.2.3"));
        // Rows that aren't a version's name are left out (a build, a note, a spinner's frame).
        assert_eq!(version("Found X [A.B]\nVersion\n-------\n  \\  \n1.0.0 beta\nlatest\n1.2.3\n").as_deref(), Some("1.2.3"));
        assert_eq!(version("Found X [A.B]\r\nVersion\r\n-------\r\n   -   \r   \\   \r\n1.2.3\r\n").as_deref(), Some("1.2.3"));
        // No dashes, no table; a table with no versions is none.
        assert_eq!(version("Found X [A.B]\nVersion\n1.2.3\n"), None);
        assert_eq!(version("Found X [A.B]\nVersion\n-------\n"), None);
        assert_eq!(version("--\n1.2.3\n"), None, "two dashes are no row");
    }

    /// The real `winget`, read-only: `cargo test -p trek-core live_winget -- --ignored --nocapture`.
    #[test]
    #[ignore = "asks the winget installed on this computer"]
    fn live_winget_says_the_newest_version() {
        for id in ["Anthropic.ClaudeCode", "GitHub.Copilot", "Nope.Nothing"] {
            let started = std::time::Instant::now();
            let said = crate::runtime().block_on(latest("winget", id));
            println!("{id}: {:?} in {:?}", said.map(|l| l.version), started.elapsed());
        }
    }
}
