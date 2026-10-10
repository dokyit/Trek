//! Finds agents and model servers already on this machine.

use crate::catalog::{ACP_AGENTS, DIRECT_PROVIDERS};
use crate::types::AgentId;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum Availability {
    Ready,
    NotInstalled,
    /// Installed but the vendor CLI reports no login.
    NeedsLogin,
    /// Local server not running.
    Offline,
}

#[derive(Debug, Clone)]
pub struct DetectedAgent {
    pub agent: AgentId,
    pub name: String,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub availability: Availability,
    /// For local servers: models found.
    pub models: Vec<String>,
    pub install_hint: Option<String>,
}

/// How long the login shell gets to say its PATH: a slow or stuck rc file mustn't hold up every
/// agent start (and the UI thread waiting on it).
#[cfg(unix)]
const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(4);

/// The PATH child processes get, as a string in the platform's own format (`:`-separated on Unix,
/// `;` on Windows). Apps launched from Finder or the Start menu get a minimal PATH, so this is
/// what the user's terminal would have (see `base_path`) plus the folders agent CLIs install into
/// (see `extra_dirs`), worked out once.
pub fn login_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| compose_path(base_path().as_deref(), &crate::paths::home()))
}

/// PATH as a login shell sees it: ask the user's shell once (giving up after a few seconds, then
/// `$PATH`).
#[cfg(unix)]
fn base_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    shell_output(std::process::Command::new(shell).args(["-ilc", "printf %s \"$PATH\""]), LOGIN_SHELL_TIMEOUT)
        .and_then(|o| String::from_utf8(o).ok())
        .map(|s| s.lines().last().unwrap_or_default().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("PATH").ok())
}

/// Windows has no login shell. A GUI app gets the PATH of whatever started it, which can be older
/// than the user's settings, so add what a fresh terminal would have: the machine PATH, then the
/// user's, from the registry. Trek's own PATH stays first. A failure just leaves those out.
#[cfg(windows)]
fn base_path() -> Option<String> {
    let process = std::env::var("PATH").ok();
    let registry = [registry_path(true), registry_path(false)];
    Some(process.into_iter().chain(registry.into_iter().flatten()).collect::<Vec<_>>().join(";"))
}

/// The `Path` value of the machine's or the user's environment, with its `%VAR%`s expanded.
#[cfg(windows)]
fn registry_path(machine: bool) -> Option<String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    let (hive, key) = if machine {
        (HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
    } else {
        (HKEY_CURRENT_USER, "Environment")
    };
    let raw: String = winreg::RegKey::predef(hive).open_subkey_with_flags(key, KEY_READ).ok()?.get_value("Path").ok()?;
    Some(expand_env_refs(&raw, |name| std::env::var(name).ok()))
}

/// `value` with each `%NAME%` that `lookup` knows replaced (what a `REG_EXPAND_SZ` value means);
/// the rest, a lone `%` included, is left as written.
#[cfg(any(windows, test))]
fn expand_env_refs(value: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(open) = rest.find('%') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let known = after.find('%').filter(|&len| len > 0).and_then(|len| Some((lookup(&after[..len])?, len)));
        match known {
            Some((found, len)) => {
                out.push_str(&found);
                rest = &after[len + 1..];
            }
            // Not a reference: keep this `%`; the next one may open a real reference.
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Folders agent CLIs install into that a minimal PATH lacks.
#[cfg(unix)]
fn extra_dirs(home: &Path) -> Vec<PathBuf> {
    let relative = [".local/bin", ".bun/bin", ".cargo/bin", ".npm-global/bin", ".opencode/bin", ".factory/bin"];
    let absolute = ["/opt/homebrew/bin", "/usr/local/bin"];
    relative.iter().map(|d| home.join(d)).chain(absolute.iter().map(PathBuf::from)).collect()
}

#[cfg(windows)]
fn extra_dirs(home: &Path) -> Vec<PathBuf> {
    windows_dirs(home, |name| std::env::var_os(name))
}

/// Windows' install folders, `var` reading the environment. Where a tool names its folder with a
/// variable of its own (`SCOOP`, `PNPM_HOME`, `VOLTA_HOME`, `NVM_SYMLINK`, ...), that comes first;
/// its default otherwise. A variable that isn't an absolute path is ignored.
///
/// Vendors' own installers come first, then the package managers whose shims are `.exe`s, then
/// Node's, whose are `.cmd`s: when a CLI is in more than one and neither is on the PATH, the one
/// that takes its arguments without cmd.exe is found (`claude.exe` from Claude's installer before
/// npm's `claude.cmd`; see `which`).
#[cfg(windows)]
fn windows_dirs(home: &Path, var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Vec<PathBuf> {
    let dir = |name: &str| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let local = dir("LOCALAPPDATA").unwrap_or_else(|| home.join(r"AppData\Local"));
    let roaming = dir("APPDATA").unwrap_or_else(|| home.join(r"AppData\Roaming"));
    let program_data = dir("ProgramData").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    let program_files = dir("ProgramFiles").unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let mut dirs: Vec<PathBuf> = [r".local\bin", r".bun\bin", r".cargo\bin", r".opencode\bin", r".factory\bin"].iter().map(|d| home.join(d)).collect();
    dirs.extend([
        // Scoop's shims, for the user and for the machine (`scoop install -g`).
        dir("SCOOP").unwrap_or_else(|| home.join("scoop")).join("shims"),
        dir("SCOOP_GLOBAL").unwrap_or_else(|| program_data.join("scoop")).join("shims"),
        // WinGet's links to the portable packages it installs, for the user and for the machine.
        local.join(r"Microsoft\WinGet\Links"),
        program_files.join(r"WinGet\Links"),
        dir("ChocolateyInstall").unwrap_or_else(|| program_data.join("chocolatey")).join("bin"),
        // npm's global prefix, where `claude.cmd` and `codex.cmd` land.
        roaming.join("npm"),
        dir("PNPM_HOME").unwrap_or_else(|| local.join("pnpm")),
        dir("VOLTA_HOME").unwrap_or_else(|| local.join("Volta")).join("bin"),
        // nvm for Windows: the active Node's folder, where its global packages' shims land.
        dir("NVM_SYMLINK").unwrap_or_else(|| PathBuf::from(r"C:\nvm4w\nodejs")),
    ]);
    dirs
}

/// `from` (a PATH string) followed by the `extra_dirs` of `home`, without empty or repeated
/// entries (Windows paths repeat in any letter case), as one PATH string.
fn compose_path(from: Option<&str>, home: &Path) -> String {
    let mut parts: Vec<PathBuf> = from.map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    parts.extend(extra_dirs(home));
    let mut seen = std::collections::HashSet::new();
    // An entry `join_paths` can't hold (a `"` on Windows) would lose the whole PATH.
    parts.retain(|p| !p.as_os_str().is_empty() && std::env::join_paths([p]).is_ok() && seen.insert(dedup_key(p)));
    std::env::join_paths(parts).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(unix)]
fn dedup_key(dir: &Path) -> std::ffi::OsString {
    dir.as_os_str().to_owned()
}

#[cfg(windows)]
fn dedup_key(dir: &Path) -> std::ffi::OsString {
    dir.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase().into()
}

/// `command`'s stdout, if it's done within `timeout`; otherwise it (and what it started) is killed.
#[cfg(unix)]
fn shell_output(command: &mut std::process::Command, timeout: Duration) -> Option<Vec<u8>> {
    use std::io::Read as _;
    std::os::unix::process::CommandExt::process_group(command, 0);
    let mut child = command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    // Done once stdout closes: the shell has exited, or handed it to nothing still running.
    let out = rx.recv_timeout(timeout).ok();
    if out.is_none() {
        tracing::warn!("the login shell didn't say its PATH within {timeout:?}; using Trek's own");
        #[cfg(target_os = "macos")]
        // SAFETY: a negative pid signals the process group the shell leads.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL)
        };
        let _ = child.kill();
    }
    let _ = child.wait();
    out
}

/// Where `binary` is installed, found on `login_path()`.
///
/// On Windows a name without an extension is looked for with each of `PATHEXT`'s (`claude` finds
/// `claude.cmd`, which npm installs next to an extensionless shell script Windows can't run), and
/// the result keeps the extension it was found under. In one folder the order is `PATHEXT`'s,
/// `.EXE` before `.CMD` by default, so a vendor's own `.exe` wins over a shim beside it; across
/// folders the first wins, as in a terminal (`extra_dirs` puts `.exe` folders first).
///
/// On Windows, `std::process::Command` runs a `.cmd` or `.bat` result through `cmd.exe` (see
/// `runs_through_cmd`): every argument still arrives exactly as given, quotes, `%VAR%`, `&` and
/// all, except that one with a line break can't be passed at all and the whole command line has
/// a limit (see `batch_args_problem`, and trek-agents' `tests_cmd_args` for the full table).
pub fn which(binary: &str) -> Option<PathBuf> {
    which_in(login_path(), binary)
}

/// `which`, on another PATH (`path`, in the platform's own format).
pub fn which_in(path: &str, binary: &str) -> Option<PathBuf> {
    find_in(path, binary, &std::env::var("PATHEXT").unwrap_or_default())
}

/// Whether Windows starts `program` through `cmd.exe`: a `.cmd` or `.bat` (npm's `claude.cmd` and
/// `codex.cmd`), which `std::process::Command` runs as `cmd.exe /c`. Never on macOS.
pub fn runs_through_cmd(program: &Path) -> bool {
    cfg!(windows) && program.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// cmd.exe's limit on the command line it's given, in UTF-16 units.
const CMD_LINE_MAX: usize = 8191;

/// Why `args` can't reach `program`, in words for the user; `None` when they can, as they always
/// can but through cmd.exe (see `runs_through_cmd`). Through cmd.exe, std escapes every argument
/// so it arrives exactly as given, but it refuses to start one with a line break in an argument
/// (cmd.exe would end the command there), and cmd.exe gives up on a command line over 8191
/// characters. This says which, before anything starts, without repeating the argument (it may be
/// a secret). The length is what the arguments need at least: a line just short of the limit can
/// still fail once std's quotes and the shim's own words are added, and then the program's output
/// is cmd.exe's "The command line is too long."
pub fn batch_args_problem(program: &Path, args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>) -> Option<String> {
    if !runs_through_cmd(program) {
        return None;
    }
    let name = program.file_name().unwrap_or(program.as_os_str()).to_string_lossy();
    let mut len = program.as_os_str().to_string_lossy().encode_utf16().count();
    for (n, arg) in args.into_iter().enumerate() {
        let arg = arg.as_ref().to_string_lossy();
        if arg.contains(['\r', '\n']) {
            return Some(format!(
                "{name} is a batch script, which Windows runs through cmd.exe, and cmd.exe can't be given an argument with a line break in it (argument {} has one).",
                n + 1
            ));
        }
        len += 1 + arg.encode_utf16().count();
    }
    (len > CMD_LINE_MAX).then(|| format!("{name} is a batch script, which Windows runs through cmd.exe, and its command line would be {len} characters, over cmd.exe's limit of {CMD_LINE_MAX}."))
}

/// The first file in `path`'s folders that is one of `candidate_names(binary, pathext)`.
fn find_in(path: &str, binary: &str, pathext: &str) -> Option<PathBuf> {
    let names = candidate_names(binary, pathext);
    std::env::split_paths(path).filter(|dir| !dir.as_os_str().is_empty()).find_map(|dir| names.iter().map(|n| dir.join(n)).find(|p| p.is_file()))
}

/// The file names to try for `binary` in one folder, best first. Unix has just the name.
#[cfg(unix)]
fn candidate_names(binary: &str, _pathext: &str) -> Vec<String> {
    vec![binary.to_string()]
}

/// A name already ending in one of `pathext`'s extensions (`.COM;.EXE;...`, the default if it has
/// none) is tried as given. Any other is tried with each extension (in lower case, as npm names its
/// shims), in order, and never bare: Windows can't run a file with no extension.
#[cfg(windows)]
fn candidate_names(binary: &str, pathext: &str) -> Vec<String> {
    let exts: Vec<&str> = pathext.split(';').map(str::trim).filter(|e| e.len() > 1 && e.starts_with('.')).collect();
    let exts = if exts.is_empty() { vec![".COM", ".EXE", ".BAT", ".CMD"] } else { exts };
    let lower = binary.to_ascii_lowercase();
    if exts.iter().any(|e| lower.ends_with(&e.to_ascii_lowercase())) {
        return vec![binary.to_string()];
    }
    exts.iter().map(|e| format!("{binary}{}", e.to_ascii_lowercase())).collect()
}

/// OpenCode's command.
pub const OPENCODE: &str = "opencode";

/// OpenCode: `opencode`, else `opencode2`, the name OpenCode 2's beta package installs it under
/// so it can sit beside 1.x (its later packages and installers install `opencode` as well).
pub fn opencode() -> Option<PathBuf> {
    which(OPENCODE).or_else(|| which("opencode2"))
}

pub async fn version_of(path: &Path) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::process::Command::new(path)
            .arg("--version")
            .env("PATH", login_path())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| !l.trim().is_empty())?.trim().to_string();
    Some(line)
}

async fn cli_agent(agent: AgentId, binary: &str, hint: &str) -> DetectedAgent {
    let name = agent.display_name();
    let found = if agent == AgentId::OpenCode { opencode() } else { which(binary) };
    match found {
        Some(path) => {
            let version = version_of(&path).await;
            DetectedAgent {
                agent,
                name,
                path: Some(path),
                version,
                availability: Availability::Ready,
                models: vec![],
                install_hint: None,
            }
        }
        None => DetectedAgent {
            agent,
            name,
            path: None,
            version: None,
            availability: Availability::NotInstalled,
            models: vec![],
            install_hint: Some(hint.into()),
        },
    }
}

async fn codex_logged_in(path: &Path) -> bool {
    let out = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::process::Command::new(path)
            .args(["login", "status"])
            .env("PATH", login_path())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    match out {
        Ok(Ok(o)) => o.status.success(),
        _ => true, // Unknown: don't block the user.
    }
}

async fn local_server(id: &str) -> DetectedAgent {
    let provider = crate::catalog::direct_provider(id).expect("known provider");
    let client = reqwest::Client::builder().timeout(Duration::from_millis(1500)).build().unwrap();
    let models: Option<Vec<String>> = if id == "ollama" {
        let url = provider.base_url.trim_end_matches("/v1").to_string() + "/api/tags";
        match client.get(url).send().await {
            Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok().map(|v| {
                v["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().map(String::from)).collect()
            }),
            _ => None,
        }
    } else {
        match client.get(format!("{}/models", provider.base_url)).send().await {
            Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok().map(|v| {
                v["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(String::from)).collect()
            }),
            _ => None,
        }
    };
    DetectedAgent {
        agent: AgentId::Direct(id.into()),
        name: provider.name.into(),
        path: None,
        version: None,
        availability: if models.is_some() { Availability::Ready } else { Availability::Offline },
        models: models.unwrap_or_default(),
        install_hint: None,
    }
}

/// Scan everything in parallel. Takes ~1–4 s.
pub async fn detect_all() -> Vec<DetectedAgent> {
    use futures::future::join_all;

    let (claude, codex, opencode, droid) = tokio::join!(
        cli_agent(AgentId::ClaudeCode, "claude", "npm i -g @anthropic-ai/claude-code"),
        cli_agent(AgentId::Codex, "codex", "npm i -g @openai/codex"),
        cli_agent(AgentId::OpenCode, OPENCODE, "curl -fsSL https://opencode.ai/install | bash"),
        cli_agent(AgentId::Droid, "droid", "curl -fsSL https://app.factory.ai/cli | sh"),
    );
    let mut codex = codex;
    if let (Availability::Ready, Some(path)) = (&codex.availability, &codex.path) {
        if !codex_logged_in(path).await {
            codex.availability = Availability::NeedsLogin;
        }
    }

    let acp = join_all(ACP_AGENTS.iter().map(|a| cli_agent(AgentId::Acp(a.id.into()), a.binary, a.install_hint))).await;
    let local = join_all(DIRECT_PROVIDERS.iter().filter(|p| p.local).map(|p| local_server(p.id))).await;

    let mut all = vec![claude, codex, opencode, droid];
    all.extend(acp);
    all.extend(crate::catalog::added_agents().iter().map(added_agent));
    all.extend(local);
    all
}

/// Whether an agent the user added can start. Never runs it: its program is only looked for
/// (`--version` means nothing to an arbitrary command, and `npx` would fetch the package).
pub fn added_agent(a: &crate::registry::AddedAgent) -> DetectedAgent {
    let path = a.resolve();
    DetectedAgent {
        agent: AgentId::Acp(a.id.clone()),
        name: a.name.clone(),
        availability: if path.is_some() { Availability::Ready } else { Availability::NotInstalled },
        install_hint: path.is_none().then(|| a.missing()),
        path,
        version: a.version.clone(),
        models: vec![],
    }
}

/// API providers with a key in the environment (keys saved in Trek are checked separately).
pub fn env_api_keys() -> Vec<&'static str> {
    DIRECT_PROVIDERS
        .iter()
        .filter(|p| p.env_key.is_some_and(|k| std::env::var(k).is_ok_and(|v| !v.is_empty())))
        .map(|p| p.id)
        .collect()
}

// Unix only: the one test runs `/bin/sh` as a login shell; Phase 2 rewrites PATH detection for Windows.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_slow_login_shell_is_given_up_on() {
        let fixture = || std::process::Command::new(trek_test_fixtures::bin("fixture"));
        let started = std::time::Instant::now();
        let out = shell_output(fixture().args(["sleep", "30"]), Duration::from_millis(200));
        assert_eq!(out, None);
        assert!(started.elapsed() < Duration::from_secs(5));
        let out = shell_output(fixture().args(["print", "/usr/bin"]), Duration::from_secs(5));
        assert_eq!(out.as_deref(), Some(&b"/usr/bin"[..]));
    }
}

// Platform-neutral: PATH composition and lookup on any OS, the pure parts of `login_path` and `which`.
#[cfg(test)]
mod tests_paths {
    use super::*;

    /// A folder of its own, removed when dropped (trek-core has no `tempfile`).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("trek-detect-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn join(dirs: &[&Path]) -> String {
        std::env::join_paths(dirs).unwrap().to_string_lossy().into_owned()
    }

    fn entries(path: &str) -> Vec<PathBuf> {
        std::env::split_paths(path).collect()
    }

    #[test]
    fn the_given_path_comes_first_then_the_extras_in_order() {
        let home = TempDir::new();
        let (a, b) = (home.path().join("a"), home.path().join("b"));
        let path = compose_path(Some(&join(&[&a, &b])), home.path());
        let got = entries(&path);
        assert_eq!(&got[..2], [a, b]);
        let extras = extra_dirs(home.path());
        assert_eq!(&got[2..], extras);
        assert!(extras.contains(&home.path().join(".bun").join("bin")));
        assert!(extras.contains(&home.path().join(".cargo").join("bin")));
    }

    #[test]
    fn no_path_leaves_just_the_extras() {
        let home = TempDir::new();
        assert_eq!(entries(&compose_path(None, home.path())), extra_dirs(home.path()));
    }

    #[test]
    fn empty_and_repeated_entries_are_dropped() {
        let home = TempDir::new();
        let (a, b) = (home.path().join("a"), home.path().join("b"));
        let sep = if cfg!(windows) { ";" } else { ":" };
        let messy = format!("{sep}{}{sep}{sep}{}{sep}{}{sep}", a.display(), b.display(), a.display());
        let got = entries(&compose_path(Some(&messy), home.path()));
        assert_eq!(&got[..2], [a, b]);
        assert_eq!(got.len(), 2 + extra_dirs(home.path()).len());
    }

    #[cfg(unix)]
    #[test]
    fn a_unix_path_is_kept_as_colon_separated_text() {
        let home = Path::new("/home/me");
        assert_eq!(
            compose_path(Some("a:b"), home),
            "a:b:/home/me/.local/bin:/home/me/.bun/bin:/home/me/.cargo/bin:/home/me/.npm-global/bin:/home/me/.opencode/bin:/home/me/.factory/bin:/opt/homebrew/bin:/usr/local/bin"
        );
        // A folder already on it isn't added again.
        assert_eq!(compose_path(Some("/usr/local/bin:a"), home).matches("/usr/local/bin").count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_path_is_semicolon_separated_and_ignores_case() {
        let home = Path::new(r"C:\Users\me");
        let got = compose_path(Some(r"C:\Tools;c:\tools\;C:\Users\ME\.CARGO\BIN"), home);
        assert!(got.starts_with(r"C:\Tools;"), "{got}");
        assert_eq!(got.to_lowercase().matches(r"c:\tools").count(), 1, "{got}");
        assert_eq!(got.to_lowercase().matches(r".cargo\bin").count(), 1, "{got}");
        for dir in [r".local\bin", r".bun\bin", r".opencode\bin", r".factory\bin"] {
            assert!(got.contains(&format!(r"C:\Users\me\{dir}")), "{dir} in {got}");
        }
        assert!(!got.contains(";;") && !got.ends_with(';'), "{got}");
    }

    /// `windows_dirs` with only these variables set.
    #[cfg(windows)]
    fn windows_dirs_with(vars: &[(&str, &str)]) -> Vec<String> {
        let lookup = |name: &str| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| std::ffi::OsString::from(v));
        windows_dirs(Path::new(r"C:\Users\me"), lookup).iter().map(|d| d.display().to_string()).collect()
    }

    #[cfg(windows)]
    #[test]
    fn windows_install_folders_default_to_where_each_tool_puts_them() {
        let got = windows_dirs_with(&[
            ("APPDATA", r"C:\Users\me\AppData\Roaming"),
            ("LOCALAPPDATA", r"C:\Users\me\AppData\Local"),
            ("ProgramData", r"C:\ProgramData"),
            ("ProgramFiles", r"C:\Program Files"),
        ]);
        assert_eq!(
            got,
            [
                r"C:\Users\me\.local\bin",
                r"C:\Users\me\.bun\bin",
                r"C:\Users\me\.cargo\bin",
                r"C:\Users\me\.opencode\bin",
                r"C:\Users\me\.factory\bin",
                r"C:\Users\me\scoop\shims",
                r"C:\ProgramData\scoop\shims",
                r"C:\Users\me\AppData\Local\Microsoft\WinGet\Links",
                r"C:\Program Files\WinGet\Links",
                r"C:\ProgramData\chocolatey\bin",
                r"C:\Users\me\AppData\Roaming\npm",
                r"C:\Users\me\AppData\Local\pnpm",
                r"C:\Users\me\AppData\Local\Volta\bin",
                r"C:\nvm4w\nodejs",
            ]
        );
        // With nothing set, the profile folders are found under home.
        let bare = windows_dirs_with(&[]);
        assert!(bare.contains(&r"C:\Users\me\AppData\Roaming\npm".to_string()), "{bare:?}");
        assert!(bare.contains(&r"C:\Users\me\AppData\Local\Microsoft\WinGet\Links".to_string()), "{bare:?}");
    }

    #[cfg(windows)]
    #[test]
    fn a_tool_s_own_variable_moves_its_folder() {
        let got = windows_dirs_with(&[
            ("LOCALAPPDATA", r"D:\Local"),
            ("SCOOP", r"D:\scoop"),
            ("SCOOP_GLOBAL", r"E:\scoop-global"),
            ("ChocolateyInstall", r"D:\choco"),
            ("PNPM_HOME", r"D:\pnpm-home"),
            ("VOLTA_HOME", r"D:\volta"),
            ("NVM_SYMLINK", r"D:\nodejs"),
        ]);
        for want in [r"D:\scoop\shims", r"E:\scoop-global\shims", r"D:\choco\bin", r"D:\pnpm-home", r"D:\volta\bin", r"D:\nodejs", r"D:\Local\Microsoft\WinGet\Links"] {
            assert!(got.contains(&want.to_string()), "{want} in {got:?}");
        }
        for default in [r"C:\Users\me\scoop\shims", r"C:\ProgramData\chocolatey\bin", r"C:\nvm4w\nodejs"] {
            assert!(!got.contains(&default.to_string()), "{default} in {got:?}");
        }
        // A relative value isn't a folder to trust: the default stands.
        let got = windows_dirs_with(&[("SCOOP", r"scoop"), ("NVM_SYMLINK", "")]);
        assert!(got.contains(&r"C:\Users\me\scoop\shims".to_string()) && got.contains(&r"C:\nvm4w\nodejs".to_string()), "{got:?}");
    }

    /// Where Claude's own installer puts `claude.exe` comes before npm's `claude.cmd`.
    #[cfg(windows)]
    #[test]
    fn an_installer_s_exe_folder_comes_before_npm_s() {
        let got = windows_dirs_with(&[]);
        let at = |d: &str| got.iter().position(|g| g.ends_with(d)).unwrap();
        assert!(at(r".local\bin") < at(r"Roaming\npm"), "{got:?}");
        assert!(at(r"scoop\shims") < at(r"Roaming\npm") && at(r"chocolatey\bin") < at(r"Roaming\npm"), "{got:?}");
    }

    #[test]
    fn only_batch_scripts_on_windows_run_through_cmd() {
        for (name, batch) in [("claude.cmd", true), ("x.BAT", true), (r"C:\a b\Codex.Cmd", true), ("claude.exe", false), ("claude", false), ("cmd", false), ("x.cmd.exe", false)] {
            assert_eq!(runs_through_cmd(Path::new(name)), cfg!(windows) && batch, "{name}");
        }
    }

    #[test]
    fn arguments_any_program_but_a_batch_script_takes() {
        let long = "x".repeat(20_000);
        let awkward = ["line\nbreak", "cr\r", long.as_str(), "\" & %PATH% ^"];
        assert_eq!(batch_args_problem(Path::new("claude.exe"), awkward), None);
        assert_eq!(batch_args_problem(Path::new("claude"), awkward), None);
        assert_eq!(batch_args_problem(Path::new("claude.cmd"), ["\" & %PATH% ^", "", "日本語"]), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_batch_script_can_t_take_a_line_break_or_a_line_over_cmd_s_limit() {
        let cmd = Path::new(r"C:\Users\me\AppData\Roaming\npm\claude.cmd");
        let said = batch_args_problem(cmd, ["-p", "--append-system-prompt", "secret one\nsecret two"]).unwrap();
        assert!(said.starts_with("claude.cmd is a batch script") && said.contains("argument 3"), "{said}");
        assert!(!said.contains("secret"), "the argument isn't repeated: {said}");
        assert!(batch_args_problem(cmd, ["a\rb"]).is_some());
        let said = batch_args_problem(cmd, ["-p", "x".repeat(9000).as_str()]).unwrap();
        assert!(said.contains("over cmd.exe's limit of 8191"), "{said}");
        assert_eq!(batch_args_problem(cmd, ["-p", "x".repeat(4000).as_str()]), None);
        // `.bat` too, in any case.
        assert!(batch_args_problem(Path::new("run.BAT"), ["\n"]).is_some());
    }

    #[cfg(windows)]
    #[test]
    fn a_quoted_windows_entry_with_a_semicolon_survives() {
        let got = compose_path(Some(r#""C:\a;b";C:\c"#), Path::new(r"C:\Users\me"));
        let got = entries(&got);
        assert_eq!(&got[..2], [PathBuf::from(r"C:\a;b"), PathBuf::from(r"C:\c")]);
    }

    #[test]
    fn env_references_expand_where_known() {
        let lookup = |name: &str| match name {
            "SystemRoot" => Some(r"C:\Windows".to_string()),
            "A" => Some("x".to_string()),
            _ => None,
        };
        assert_eq!(expand_env_refs(r"%SystemRoot%\system32;%SystemRoot%", lookup), r"C:\Windows\system32;C:\Windows");
        // Unknown names, lone and empty `%`s stay as written; a failed reference doesn't swallow the next.
        assert_eq!(expand_env_refs("%NOPE%;100%;%%;%A%", lookup), "%NOPE%;100%;%%;x");
        assert_eq!(expand_env_refs("%NOPE%A%", lookup), "%NOPEx");
        assert_eq!(expand_env_refs("plain", lookup), "plain");
    }

    #[cfg(unix)]
    fn tool_name() -> &'static str {
        "tool"
    }
    #[cfg(windows)]
    fn tool_name() -> &'static str {
        "tool.cmd"
    }

    fn touch_executable(dir: &Path, name: &str) -> PathBuf {
        let file = dir.join(name);
        std::fs::write(&file, "").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        file
    }

    #[test]
    fn which_finds_a_tool_in_a_folder_of_the_path() {
        let (first, second) = (TempDir::new(), TempDir::new());
        let tool = touch_executable(second.path(), tool_name());
        let path = join(&[first.path(), second.path()]);
        assert_eq!(find_in(&path, "tool", ".COM;.EXE;.BAT;.CMD"), Some(tool.clone()));
        assert_eq!(find_in(&path, "tool", "").unwrap().file_name(), tool.file_name());
        assert_eq!(find_in(&path, "missing", ".COM;.EXE;.BAT;.CMD"), None);
        assert_eq!(find_in("", "tool", ""), None);
    }

    #[test]
    fn the_first_folder_of_the_path_wins() {
        let (first, second) = (TempDir::new(), TempDir::new());
        let wanted = touch_executable(first.path(), tool_name());
        touch_executable(second.path(), tool_name());
        assert_eq!(find_in(&join(&[first.path(), second.path()]), "tool", ""), Some(wanted));
    }

    #[cfg(windows)]
    #[test]
    fn pathext_decides_between_extensions_and_case_does_not_matter() {
        let dir = TempDir::new();
        let path = join(&[dir.path()]);
        let name = |p: Option<PathBuf>| p.map(|p| p.file_name().unwrap().to_string_lossy().to_lowercase());
        let pathext = ".COM;.EXE;.BAT;.CMD";

        touch_executable(dir.path(), "tool.cmd");
        assert_eq!(name(find_in(&path, "tool", pathext)), Some("tool.cmd".into()));
        assert_eq!(name(find_in(&path, "TOOL", pathext)), Some("tool.cmd".into()));
        assert_eq!(name(find_in(&path, "tool", ".cmd")), Some("tool.cmd".into()));
        // An explicit extension is tried as given, in any case.
        assert_eq!(name(find_in(&path, "tool.cmd", pathext)), Some("tool.cmd".into()));
        assert_eq!(name(find_in(&path, "Tool.CMD", pathext)), Some("tool.cmd".into()));
        assert_eq!(find_in(&path, "tool.exe", pathext), None);

        // npm's extensionless shell script beside `claude.cmd` isn't something Windows can run.
        touch_executable(dir.path(), "shim");
        assert_eq!(find_in(&path, "shim", pathext), None);
        touch_executable(dir.path(), "shim.cmd");
        assert_eq!(name(find_in(&path, "shim", pathext)), Some("shim.cmd".into()));

        // PATHEXT order, not the folder's: `.exe` before `.cmd`, and the reverse order reverses it.
        touch_executable(dir.path(), "tool.exe");
        assert_eq!(name(find_in(&path, "tool", pathext)), Some("tool.exe".into()));
        assert_eq!(name(find_in(&path, "tool", ".CMD;.EXE")), Some("tool.cmd".into()));
        // No usable PATHEXT falls back to the default four.
        assert_eq!(name(find_in(&path, "tool", "")), Some("tool.exe".into()));
        assert_eq!(name(find_in(&path, "tool", ";;nonsense")), Some("tool.exe".into()));
    }
}
