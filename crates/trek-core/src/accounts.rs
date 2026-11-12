//! Several logins for one agent: people who hit 5-hour and weekly limits often own two plans.
//!
//! An account is a label and a config folder of its own under Trek's data folder, which the agent
//! is pointed at with its own variable (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`). The default account
//! isn't one of these: it's the user's normal login, with no variable set, and Trek never writes
//! to it. An account's folder holds what the agent keeps per login (its login files, its
//! sessions), and links to the user's own setup (settings, skills, global notes), so the second
//! account behaves as the first does.
//!
//! An agent's sessions live in its config folder, so a session belongs to one account: a thread
//! that moves to another account keeps its transcript in Trek and starts a new session there from
//! a recap (`rewind::recap`).

use crate::paths;
use crate::types::AgentId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Longest a label runs; the composer's pill and the limit bar's button show it.
const LABEL_MAX: usize = 32;

/// What of the user's Claude Code setup an account shares: never its login (`.credentials.json`,
/// `.claude.json`) or its sessions (`projects`).
const CLAUDE_SHARED: &[&str] = &[
    "CLAUDE.md",
    "settings.json",
    "skills",
    "agents",
    "commands",
    "plugins",
    "hooks",
    "output-styles",
];

/// The same for Codex: never `auth.json`, `sessions` or its state database.
const CODEX_SHARED: &[&str] = &[
    "AGENTS.md",
    "config.toml",
    "hooks.json",
    "skills",
    "rules",
    "prompts",
    "agents",
    "plugins",
];

/// A login of an agent's beyond the user's normal one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Stable, and the name of its folder.
    pub id: String,
    /// The agent it signs in to (`AgentId::key`).
    pub agent: String,
    pub label: String,
}

/// Where an agent finds an account: the variable it reads, and the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountHome {
    pub var: &'static str,
    pub dir: PathBuf,
}

/// The variable that points `agent` at a config folder, for the agents that have one.
pub fn config_var(agent: &AgentId) -> Option<&'static str> {
    match agent {
        AgentId::ClaudeCode => Some("CLAUDE_CONFIG_DIR"),
        AgentId::Codex => Some("CODEX_HOME"),
        _ => None,
    }
}

/// Whether `agent` can have accounts. The mock has them too, with no folder to point it at, so
/// the isolated profile and tests can reach everything accounts do.
pub fn supported(agent: &AgentId) -> bool {
    config_var(agent).is_some() || matches!(agent, AgentId::Direct(p) if crate::catalog::is_mock(p))
}

/// The folder accounts live in.
pub fn root() -> PathBuf {
    paths::data_dir().join("accounts")
}

/// Every account folder there is, whatever settings say: for finding a session by its id.
pub fn folders() -> Vec<PathBuf> {
    folders_in(&root())
}

fn folders_in(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// A label typed for an account: trimmed and kept short. `None`: nothing was typed.
pub fn clean_label(text: &str) -> Option<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then(|| text.chars().take(LABEL_MAX).collect())
}

impl Account {
    /// A new account of `agent`, named after how many it already has ("Account 2": the default
    /// account is the first). A name in use is passed over.
    pub fn new(agent: &AgentId, existing: &[Account]) -> Account {
        let key = agent.key();
        let of_agent = existing.iter().filter(|a| a.agent == key).count();
        let label = (of_agent + 2..)
            .map(|n| format!("Account {n}"))
            .find(|l| !existing.iter().any(|a| a.agent == key && a.label == *l))
            .unwrap_or_else(|| "Account".to_string());
        Account {
            id: uuid::Uuid::now_v7().simple().to_string(),
            agent: key,
            label,
        }
    }

    pub fn agent(&self) -> AgentId {
        AgentId::from_key(&self.agent)
    }

    /// Its config folder. Settings can be edited by hand: an id that would lead out of the
    /// accounts folder leads nowhere.
    pub fn dir(&self) -> Option<PathBuf> {
        let plain = !self.id.is_empty()
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        plain.then(|| root().join(&self.id))
    }

    /// Where the agent finds this account, once its folder is ready.
    pub fn home(&self) -> Option<AccountHome> {
        let var = config_var(&self.agent())?;
        Some(AccountHome {
            var,
            dir: self.dir()?,
        })
    }

    /// Make its folder, and link the user's own setup into it (again, for what the user has made
    /// since). Call before the agent is started on it. What can't be linked is left out: the
    /// account still works, on the agent's defaults.
    pub fn open(&self) -> Option<AccountHome> {
        let home = self.home()?;
        let agent = self.agent();
        if let Err(e) = std::fs::create_dir_all(&home.dir) {
            tracing::warn!("account folder {}: {e}", home.dir.display());
            return None;
        }
        if let Some(source) = default_home(&agent) {
            link_shared(&source, &home.dir, shared(&agent));
        }
        Some(home)
    }

    /// The agent's own sign-in, for this account, as a line for Trek's terminal
    /// (`panels::terminal::Job::Setup`).
    pub fn login_command(&self) -> Option<String> {
        let home = self.home()?;
        let login = crate::catalog::agent_setup(&self.agent)?.login;
        Some(with_var(
            home.var,
            &home.dir.display().to_string(),
            login,
            cfg!(windows),
        ))
    }
}

/// Delete an account's folder (`Account::dir`): the links in it, never what they lead to.
pub fn remove_folder(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// The MCP servers the user gave Claude Code on their normal login, for a session in `cwd`: those
/// for every folder and that folder's own. They're kept in the file its login is (`.claude.json`),
/// so an account can't link to it: the session is handed them instead. `None`: there are none.
pub fn claude_user_mcp_servers(cwd: &Path) -> Option<serde_json::Value> {
    let home = default_home(&AgentId::ClaudeCode)?;
    // Beside the config folder for the default one, in it for one the user set.
    let file = if home == paths::agents_home().join(".claude") {
        paths::agents_home().join(".claude.json")
    } else {
        home.join(".claude.json")
    };
    mcp_servers_in(&std::fs::read_to_string(file).ok()?, cwd)
}

fn mcp_servers_in(text: &str, cwd: &Path) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let folder = cwd.display().to_string();
    let mut servers = serde_json::Map::new();
    for found in [
        &v["mcpServers"],
        &v["projects"][folder.as_str()]["mcpServers"],
    ] {
        if let Some(found) = found.as_object() {
            servers.extend(found.clone());
        }
    }
    (!servers.is_empty()).then_some(serde_json::Value::Object(servers))
}

/// `agent`'s own config folder: the one its default account uses.
fn default_home(agent: &AgentId) -> Option<PathBuf> {
    let home = paths::agents_home();
    match agent {
        AgentId::ClaudeCode => {
            let env = (!paths::isolated())
                .then(|| std::env::var_os("CLAUDE_CONFIG_DIR"))
                .flatten()
                .map(PathBuf::from)
                .filter(|d| d.is_absolute());
            Some(env.unwrap_or_else(|| home.join(".claude")))
        }
        AgentId::Codex => Some(crate::import::codex::codex_home_for(&home)),
        _ => None,
    }
}

fn shared(agent: &AgentId) -> &'static [&'static str] {
    match agent {
        AgentId::ClaudeCode => CLAUDE_SHARED,
        AgentId::Codex => CODEX_SHARED,
        _ => &[],
    }
}

/// Link each of `names` that `source` has into `dir`, unless `dir` has one already: the agent may
/// have replaced a link with a file of its own, and that stays. On Windows a file linked hard is
/// linked again when its original was replaced whole (`hard::follow`).
fn link_shared(source: &Path, dir: &Path, names: &[&str]) {
    #[cfg(windows)]
    hard::follow(source, dir, names);
    for name in names {
        let (from, to) = (source.join(name), dir.join(name));
        if !from.exists() || std::fs::symlink_metadata(&to).is_ok() {
            continue;
        }
        if let Err(e) = link(&from, &to) {
            tracing::warn!("account link {}: {e}", to.display());
        }
    }
}

#[cfg(unix)]
fn link(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(from, to)
}

/// Symbolic links need Developer Mode or an elevated process on Windows. Without them a folder
/// gets a junction, and a file a hard link (`hard`), which need neither; a copy (another drive)
/// follows the original no more.
#[cfg(windows)]
fn link(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        if std::os::windows::fs::symlink_dir(from, to).is_ok() {
            return Ok(());
        }
        let status = crate::procs::command("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(to)
            .arg(from)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        return if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other("mklink /J failed"))
        };
    }
    if std::os::windows::fs::symlink_file(from, to).is_ok() {
        return Ok(());
    }
    if hard::link(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map(|_| ())
}

/// A shared file as a hard link (Windows, without Developer Mode): one file under two names, so an
/// edit made in place on either side is the other's too. A save that replaces the original whole
/// (written beside it, then renamed over it) leaves the account's name on the old file: each
/// link made is recorded with the file's id (`LINKED` in the account's folder), and the next open
/// links the new original, unless the account's name has a new file of the agent's own by then,
/// which stays.
#[cfg(windows)]
mod hard {
    use std::collections::BTreeMap;
    use std::path::Path;

    /// The record, beside what it records: each name linked, with its file's id then.
    const LINKED: &str = ".trek-linked.json";

    /// Link `to` to `from`, and record it.
    pub fn link(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::hard_link(from, to)?;
        if let (Some(dir), Some(name), Some(id)) = (to.parent(), to.file_name(), id(to)) {
            let mut linked = read(dir);
            linked.insert(name.to_string_lossy().into_owned(), id);
            write(dir, &linked);
        }
        Ok(())
    }

    /// Link again each of `names` in `dir` still on the file it was linked to, when `source`'s
    /// is another file now.
    pub fn follow(source: &Path, dir: &Path, names: &[&str]) {
        let mut linked = read(dir);
        let mut changed = false;
        for name in names {
            let Some(was) = linked.get(*name).cloned() else {
                continue;
            };
            let (from, to) = (source.join(name), dir.join(name));
            let (Some(now), Some(ours)) = (id(&from), id(&to)) else {
                continue;
            };
            if ours != was {
                // The agent's own file in its place: it stays, and is no longer followed.
                linked.remove(*name);
                changed = true;
                continue;
            }
            if now == was {
                continue;
            }
            // In use by the account's agent: left for the next open.
            if std::fs::remove_file(&to).is_ok() && std::fs::hard_link(&from, &to).is_ok() {
                linked.insert((*name).to_string(), now);
                changed = true;
            }
        }
        if changed {
            write(dir, &linked);
        }
    }

    fn read(dir: &Path) -> BTreeMap<String, String> {
        std::fs::read_to_string(dir.join(LINKED))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn write(dir: &Path, linked: &BTreeMap<String, String>) {
        if let Ok(text) = serde_json::to_string(linked) {
            let _ = std::fs::write(dir.join(LINKED), text);
        }
    }

    /// The file's identity on its volume (the volume's serial number and the file's index, in
    /// hex): the same for every name a hard link gives it, another for a file put in its place.
    pub fn id(path: &Path) -> Option<String> {
        use std::os::windows::fs::OpenOptionsExt as _;
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            GetFileInformationByHandle,
        };
        // Its attributes alone, sharing everything: an agent holding it open is no matter.
        let file = std::fs::OpenOptions::new()
            .access_mode(0x80) // FILE_READ_ATTRIBUTES
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(path)
            .ok()?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: an open handle, and a structure the call fills.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
        (ok != 0).then(|| {
            format!(
                "{:08x}-{:08x}{:08x}",
                info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
            )
        })
    }
}

/// `line` with `var` set to `value`, for a POSIX shell or PowerShell (what Trek's own commands run
/// in on Windows). The value stays one word whatever is in it: a data folder may have spaces.
fn with_var(var: &str, value: &str, line: &str, powershell: bool) -> String {
    if powershell {
        format!("$env:{var}='{}'; {line}", value.replace('\'', "''"))
    } else {
        format!("{var}='{}' {line}", value.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: &str, agent: AgentId, label: &str) -> Account {
        Account {
            id: id.into(),
            agent: agent.key(),
            label: label.into(),
        }
    }

    #[test]
    fn only_agents_with_a_config_folder_get_a_variable() {
        assert_eq!(config_var(&AgentId::ClaudeCode), Some("CLAUDE_CONFIG_DIR"));
        assert_eq!(config_var(&AgentId::Codex), Some("CODEX_HOME"));
        assert_eq!(config_var(&AgentId::OpenCode), None);
        assert!(!supported(&AgentId::OpenCode));
        assert!(supported(&AgentId::Codex));
    }

    #[test]
    fn a_new_account_is_named_after_the_ones_before_it() {
        let first = Account::new(&AgentId::ClaudeCode, &[]);
        assert_eq!(first.label, "Account 2");
        assert_eq!(first.agent, "claude-code");
        // Another agent's accounts don't count, and a label in use is passed over.
        let others = vec![
            account("a", AgentId::Codex, "Account 2"),
            account("b", AgentId::ClaudeCode, "Account 3"),
        ];
        assert_eq!(
            Account::new(&AgentId::ClaudeCode, &others).label,
            "Account 4"
        );
        assert_ne!(first.id, Account::new(&AgentId::ClaudeCode, &[]).id);
    }

    #[test]
    fn labels_are_trimmed_and_kept_short() {
        assert_eq!(clean_label("  Work   plan "), Some("Work plan".to_string()));
        assert_eq!(clean_label("   "), None);
        assert_eq!(clean_label(&"x".repeat(80)).unwrap().chars().count(), 32);
    }

    #[test]
    fn an_id_that_leads_out_of_the_accounts_folder_has_no_folder() {
        for bad in ["..", "../..", "a/b", "a\\b", ""] {
            let a = account(bad, AgentId::Codex, "Bad");
            assert_eq!(a.dir(), None, "{bad}");
            assert_eq!(a.home(), None, "{bad}");
        }
        let good = account("0193abc", AgentId::Codex, "Good");
        assert_eq!(good.dir().unwrap(), root().join("0193abc"));
    }

    #[test]
    fn the_sign_in_runs_the_agents_own_command_with_the_folder_quoted() {
        assert_eq!(
            with_var("CODEX_HOME", "/a b/it's", "codex login", false),
            r"CODEX_HOME='/a b/it'\''s' codex login"
        );
        assert_eq!(
            with_var("CODEX_HOME", r"C:\a b\it's", "codex login", true),
            r"$env:CODEX_HOME='C:\a b\it''s'; codex login"
        );
        let a = account("abc", AgentId::ClaudeCode, "Work");
        let line = a.login_command().unwrap();
        assert!(line.contains("CLAUDE_CONFIG_DIR"), "{line}");
        assert!(line.ends_with("claude auth login"), "{line}");
        // The mock signs in to nothing.
        let mock = account("abc", AgentId::Direct("mock".into()), "Mock");
        assert_eq!(mock.login_command(), None);
    }

    /// The account shares the user's setup, not their login or their sessions, and leaves alone
    /// what it has of its own.
    #[test]
    fn an_account_links_the_users_setup_but_never_their_login() {
        let tmp = crate::import::Scratch::new();
        let source = tmp.0.join("source");
        let dir = tmp.0.join("account");
        std::fs::create_dir_all(source.join("skills")).unwrap();
        std::fs::create_dir_all(source.join("projects")).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        for file in ["settings.json", ".credentials.json", ".claude.json"] {
            std::fs::write(source.join(file), "real").unwrap();
        }
        std::fs::write(source.join("CLAUDE.md"), "notes").unwrap();
        std::fs::write(dir.join("CLAUDE.md"), "its own").unwrap();
        link_shared(&source, &dir, CLAUDE_SHARED);
        assert_eq!(
            std::fs::read_to_string(dir.join("settings.json")).unwrap(),
            "real"
        );
        assert!(dir.join("skills").is_dir());
        assert_eq!(
            std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
            "its own"
        );
        for private in [".credentials.json", ".claude.json", "projects"] {
            assert!(!dir.join(private).exists(), "{private}");
        }
        // What the user made since comes along at the next open.
        std::fs::create_dir_all(source.join("agents")).unwrap();
        link_shared(&source, &dir, CLAUDE_SHARED);
        assert!(dir.join("agents").is_dir());
        // Nothing in the lists can be a login or a session store.
        for private in [
            ".credentials.json",
            ".claude.json",
            "auth.json",
            "projects",
            "sessions",
        ] {
            assert!(!CLAUDE_SHARED.contains(&private), "{private}");
            assert!(!CODEX_SHARED.contains(&private), "{private}");
        }
    }

    /// Without Developer Mode a shared file is a hard link: an edit made in place on either side
    /// is the other's, and a save that replaces the original whole is followed at the next open.
    /// A file the agent puts in its place stays its own.
    #[cfg(windows)]
    #[test]
    fn a_hard_linked_file_follows_its_original_when_it_is_replaced_whole() {
        let tmp = crate::import::Scratch::new();
        let (source, dir) = (tmp.0.join("source"), tmp.0.join("account"));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let (from, to) = (source.join("settings.json"), dir.join("settings.json"));
        let read = || std::fs::read_to_string(&to).unwrap();
        // A save that writes beside the file and renames over it.
        let replace = |path: &Path, text: &str| {
            let beside = path.with_extension("json.tmp");
            std::fs::write(&beside, text).unwrap();
            std::fs::rename(&beside, path).unwrap();
        };
        std::fs::write(&from, "one").unwrap();
        // As `link` does where symbolic links aren't allowed.
        hard::link(&from, &to).unwrap();
        assert_eq!(hard::id(&from), hard::id(&to));
        std::fs::write(&from, "two").unwrap();
        assert_eq!(read(), "two", "an edit in place is the account's too");
        replace(&from, "three");
        assert_eq!(read(), "two", "the old file, until the next open");
        link_shared(&source, &dir, CLAUDE_SHARED);
        assert_eq!(read(), "three");
        assert_eq!(hard::id(&from), hard::id(&to));
        // The agent's own file in its place stays, and isn't followed any more.
        replace(&to, "its own");
        replace(&from, "four");
        link_shared(&source, &dir, CLAUDE_SHARED);
        assert_eq!(read(), "its own");
        replace(&from, "five");
        link_shared(&source, &dir, CLAUDE_SHARED);
        assert_eq!(read(), "its own");
    }

    /// Deleting an account's folder takes the links, not what they lead to.
    #[test]
    fn removing_an_account_leaves_what_its_links_lead_to() {
        let tmp = crate::import::Scratch::new();
        let source = tmp.0.join("source");
        let dir = tmp.0.join("account");
        std::fs::create_dir_all(source.join("skills")).unwrap();
        std::fs::write(source.join("skills").join("kept"), "x").unwrap();
        std::fs::write(source.join("settings.json"), "x").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        link_shared(&source, &dir, CLAUDE_SHARED);
        remove_folder(&dir).unwrap();
        assert!(!dir.exists());
        // Already gone: nothing to do.
        remove_folder(&dir).unwrap();
        assert!(source.join("skills").join("kept").exists());
        assert!(source.join("settings.json").exists());
    }

    #[test]
    fn the_users_mcp_servers_are_read_for_the_folder_and_for_every_folder() {
        let text = r#"{
            "oauthAccount": {"emailAddress": "me@example.com"},
            "mcpServers": {"all": {"command": "a"}, "both": {"command": "global"}},
            "projects": {
                "/work": {"mcpServers": {"here": {"command": "b"}, "both": {"command": "local"}}},
                "/other": {"mcpServers": {"there": {"command": "c"}}}
            }
        }"#;
        let servers = mcp_servers_in(text, Path::new("/work")).unwrap();
        let mut names: Vec<&String> = servers.as_object().unwrap().keys().collect();
        names.sort();
        assert_eq!(names, ["all", "both", "here"]);
        assert_eq!(servers["both"]["command"], "local", "the folder's own wins");
        // Nothing of the login comes along.
        assert!(!servers.to_string().contains("me@example.com"));
        assert_eq!(mcp_servers_in("{}", Path::new("/work")), None);
        assert_eq!(mcp_servers_in("not json", Path::new("/work")), None);
    }

    #[test]
    fn folders_lists_each_account_folder() {
        let tmp = crate::import::Scratch::new();
        assert!(folders_in(&tmp.0.join("none")).is_empty());
        std::fs::create_dir_all(tmp.0.join("a")).unwrap();
        std::fs::write(tmp.0.join("stray"), "x").unwrap();
        assert_eq!(folders_in(&tmp.0), vec![tmp.0.join("a")]);
    }
}
