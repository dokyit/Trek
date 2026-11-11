//! Where the Direct harness's tools run. Trek's own sessions run them on this machine
//! ([`LocalExecutor`]); `trek-bench` can run them inside a remote sandbox instead
//! ([`StdioExecutor`]: a request on stdout, its reply on stdin, one JSON object per line).
//! The tools themselves (paths, line numbers, edits, output caps) stay in `direct.rs`: an
//! executor only runs a command and moves bytes.

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, BufReader};

/// What a command did. `exit_code` is `None` when it ended on a signal or timed out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

impl ExecOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }
}

/// The executor itself is gone (the sandbox's end of the pipe closed): no tool can run any more,
/// so the turn ends with this rather than handing the model one failure after another.
#[derive(Debug)]
pub struct ExecutorGone(pub String);

impl std::fmt::Display for ExecutorGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExecutorGone {}

/// Runs the Direct harness's tools: `bash` through `exec`, `read_file` through `read`,
/// `write_file` through `write`, and `edit_file` through both (the edit is worked out here).
/// Any other error is the one call's failure, told to the model as the tool's output; an
/// [`ExecutorGone`] ends the turn.
pub trait ToolExecutor: Send + Sync {
    /// Run `command` with the shell in `cwd`, for at most `timeout` (`None`: as long as it takes).
    /// Dropping the future stops the command and whatever it started.
    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a Path,
        timeout: Option<Duration>,
    ) -> BoxFuture<'a, Result<ExecOutput>>;
    /// A file's bytes, `None` when there is no file at `path`.
    fn read<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<Option<Vec<u8>>>>;
    /// Write `bytes` to `path`, making its folders first. A file already there keeps its mode;
    /// a new one is 0644.
    fn write<'a>(&'a self, path: &'a Path, bytes: &'a [u8]) -> BoxFuture<'a, Result<()>>;
    /// `exec` for a command that starts something in the background and returns (the `bash`
    /// tool's `background`), with what that something stays tied to: the session keeps the
    /// [`Held`], and dropping it ends what the command left running.
    fn exec_background<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a Path,
        timeout: Option<Duration>,
    ) -> BoxFuture<'a, Result<(ExecOutput, Held)>> {
        Box::pin(async move { Ok((self.exec(command, cwd, timeout).await?, Held::default())) })
    }
    /// Whether the tools run on this machine, where a search can run in-process and folders can be
    /// looked at directly.
    fn is_local(&self) -> bool {
        false
    }
    /// Where a tool's overflow output is saved, for the model to read later.
    fn temp_dir(&self) -> PathBuf {
        PathBuf::from("/tmp")
    }
}

/// The largest file `read` takes.
const MAX_READ: u64 = 128 * 1024 * 1024;
/// How long a command's output may stay open after its shell ended. Normally both pipes close with
/// the shell; one held by something it left running mustn't hold the answer too.
const PIPE_GRACE: Duration = Duration::from_millis(500);
/// How much of a command's output is held in memory: the start, and a rolling end. A command that
/// writes without stopping mustn't take the machine's memory with it.
const KEEP_HEAD: usize = 1024 * 1024;
const KEEP_TAIL: usize = 8 * 1024 * 1024;

#[derive(Default)]
struct Capped {
    bytes: Vec<u8>,
    dropped: u64,
}

impl Capped {
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        // Twice over before cutting, so it isn't a copy per chunk.
        if self.bytes.len() > 2 * (KEEP_HEAD + KEEP_TAIL) {
            let cut_to = self.bytes.len() - KEEP_TAIL;
            self.dropped += (cut_to - KEEP_HEAD) as u64;
            self.bytes.drain(KEEP_HEAD..cut_to);
        }
    }

    fn text(&self) -> String {
        if self.dropped == 0 {
            return String::from_utf8_lossy(&self.bytes).into_owned();
        }
        format!(
            "{}\n[… {} bytes of output were not kept …]\n{}",
            String::from_utf8_lossy(&self.bytes[..KEEP_HEAD]),
            self.dropped,
            String::from_utf8_lossy(&self.bytes[KEEP_HEAD..])
        )
    }
}

/// Reads a command's pipe into `into` until it closes. It runs on after the command's answer has
/// gone back if something it started still holds the pipe: with no reader that process would die
/// on its next write.
async fn drain(mut pipe: impl AsyncRead + Unpin, into: Arc<Mutex<Capped>>) {
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        match tokio::io::AsyncReadExt::read(&mut pipe, &mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => into.lock().expect("output").push(&chunk[..n]),
        }
    }
}

/// The shell commands run in: bash where there is one (the tool is called `bash`, and models write
/// for it), else `sh`.
#[cfg(unix)]
fn shell_program() -> &'static str {
    static SHELL: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    SHELL.get_or_init(|| {
        if trek_core::detect::which("bash").is_some() {
            "bash"
        } else {
            "sh"
        }
    })
}

/// The shell commands run in on Windows: Git for Windows' bash when it's installed (the tool is
/// called `bash`, and models write bash far more reliably than cmd.exe), else cmd.exe.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq)]
enum WindowsShell {
    GitBash(&'static Path),
    Cmd,
}

#[cfg(windows)]
fn windows_shell() -> WindowsShell {
    git_bash().map_or(WindowsShell::Cmd, WindowsShell::GitBash)
}

/// Whether commands on this machine run in Git Bash on Windows, rather than in the bash of a Mac
/// or Linux or in cmd.exe: the system prompt says which, and how its paths and flags go.
pub(crate) fn runs_git_bash() -> bool {
    #[cfg(windows)]
    let found = git_bash().is_some();
    #[cfg(not(windows))]
    let found = false;
    found
}

/// Git for Windows' `bin\bash.exe`, found once: in the Git folder of the `git` on the PATH, else
/// in the folders Git installs to. That `bash.exe` starts the real one (`usr\bin\bash.exe`) with
/// Git's own tools (`ls`, `grep`, `sed`) first on the PATH. Never `System32\bash.exe`, which is
/// WSL's: a Linux machine of its own, with other paths and other programs.
#[cfg(windows)]
pub(crate) fn git_bash() -> Option<&'static Path> {
    static BASH: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    BASH.get_or_init(|| {
        find_git_bash(trek_core::detect::which("git").as_deref(), |name| {
            std::env::var_os(name)
        })
    })
    .as_deref()
}

/// `git_bash` with `git` the `git` found on the PATH, and `var` reading the environment. Git's
/// folder is up to two above its `git.exe` (`cmd\git.exe`, `bin\git.exe`, `mingw64\bin\git.exe`);
/// it counts when it holds both the launcher and the shell it starts, and isn't in Windows' own
/// folder.
#[cfg(windows)]
fn find_git_bash(
    git: Option<&Path>,
    var: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let dir = |name: &str| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let system = dir("SystemRoot").unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let beside_git = git
        .into_iter()
        .flat_map(|git| git.ancestors().skip(1).take(3))
        .map(Path::to_path_buf);
    let scoop = |var_name: &str, default: Option<PathBuf>| {
        dir(var_name)
            .or(default)
            .map(|s| s.join(r"apps\git\current"))
    };
    let installed = [
        dir("ProgramFiles").map(|p| p.join("Git")),
        dir("ProgramW6432").map(|p| p.join("Git")),
        dir("ProgramFiles(x86)").map(|p| p.join("Git")),
        // A per-user install, as winget's user scope makes.
        dir("LOCALAPPDATA").map(|p| p.join(r"Programs\Git")),
        scoop("SCOOP", dir("USERPROFILE").map(|p| p.join("scoop"))),
        scoop("SCOOP_GLOBAL", dir("ProgramData").map(|p| p.join("scoop"))),
    ];
    beside_git
        .chain(installed.into_iter().flatten())
        .find(|root| {
            !is_inside(root, &system)
                && root.join(r"bin\bash.exe").is_file()
                && root.join(r"usr\bin\bash.exe").is_file()
        })
        .map(|root| root.join(r"bin\bash.exe"))
}

/// `path` is `folder` or inside it, in any case (Windows paths are case-blind).
#[cfg(windows)]
fn is_inside(path: &Path, folder: &Path) -> bool {
    let key = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    let (path, folder) = (key(path), key(folder));
    path == folder || path.starts_with(&format!("{folder}\\"))
}

/// The variable Git Bash finds its script in, and the command line that runs it. The script goes
/// in the environment, where it arrives exactly as written: Git Bash reads its command line by
/// rules of its own (a backslash escapes any backslash after it, not only one before a quote),
/// and cuts an argument past about 8,000 characters short. `export -n` keeps the script out of
/// the environment of whatever it runs.
#[cfg(windows)]
const SCRIPT_VAR: &str = "TREK_COMMAND";
#[cfg(windows)]
const RUN_SCRIPT_VAR: &str = r#"-c "export -n TREK_COMMAND; eval \"$TREK_COMMAND\"""#;

/// The command that runs `command` in the shell, before its folder and environment are set.
#[cfg(unix)]
fn shell_command(command: &str) -> tokio::process::Command {
    let mut shell = trek_core::procs::async_command(shell_program());
    shell.arg("-c").arg(command);
    shell
}

#[cfg(windows)]
fn shell_command(windows: WindowsShell, command: &str) -> tokio::process::Command {
    match windows {
        WindowsShell::GitBash(bash) => {
            let mut shell = trek_core::procs::async_command(bash);
            shell.raw_arg(RUN_SCRIPT_VAR).env(SCRIPT_VAR, command);
            shell
        }
        WindowsShell::Cmd => {
            let mut shell = trek_core::procs::async_command("cmd.exe");
            // As written, not with Rust's argument quoting: cmd.exe doesn't read `\"`, so
            // `git commit -m "a b"` would reach git as two words. `/s` with the outer quotes makes
            // it take everything between them literally.
            shell.raw_arg(format!("/s /c \"{command}\""));
            shell
        }
    }
}

/// What a command started in the background stays tied to, ended when this is dropped. On
/// Windows with Git Bash it's a job holding all the command started: Git Bash's processes can't
/// all be found from the first by parent ids (one that runs another of Git's programs hands its
/// place to it and exits), and a job closed with Trek, however Trek ends, ends what's in it.
/// Otherwise it holds nothing: a process group is ended by its id.
#[derive(Default)]
pub struct Held {
    #[cfg(windows)]
    _job: Option<crate::job::Job>,
}

/// A job of its own for `child`, the shell of a command that starts something in the background,
/// besides the one `spawn_group` put it in (that one is let go once the shell is done). The child
/// has been running since it was spawned, a few microseconds, and what it started in that time
/// isn't in this job: a shell has to load and get going before it starts anything.
#[cfg(windows)]
fn hold(child: &tokio::process::Child) -> Held {
    let job = crate::job::Job::new().and_then(|job| {
        let handle = child
            .raw_handle()
            .ok_or_else(|| std::io::Error::other("it has exited"))?;
        job.assign(handle)?;
        Ok(job)
    });
    match job {
        Ok(job) => Held { _job: Some(job) },
        Err(e) => {
            tracing::warn!(
                "couldn't hold background command {:?} in a job; it will outlive the session: {e}",
                child.id()
            );
            Held::default()
        }
    }
}

/// Run `shell` (a `shell_command`) in `cwd` for at most `timeout`, and with `keep` hold what it
/// leaves running (on Windows; see [`Held`]).
async fn run_local(
    mut shell: tokio::process::Command,
    cwd: &Path,
    timeout: Option<Duration>,
    keep: bool,
) -> Result<(ExecOutput, Held)> {
    shell
        .current_dir(cwd)
        // The user's real PATH, as every other agent gets: an app started from the Dock
        // has a bare one, and "node: command not found" sends a model down the wrong road.
        .env("PATH", trek_core::detect::login_path())
        // Nothing waits on a person who isn't there.
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // In a group of its own (a job on Windows), so a Stop or a timeout ends what the
    // command started too, not just the shell: dropping `child` does that.
    let mut child = crate::spawn_group(&mut shell)?;
    #[cfg(windows)]
    let held = if keep { hold(&child) } else { Held::default() };
    #[cfg(not(windows))]
    let held = {
        let _ = keep;
        Held::default()
    };

    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let out = Arc::new(Mutex::new(Capped::default()));
    let err = Arc::new(Mutex::new(Capped::default()));
    let read_out = tokio::spawn(drain(stdout, out.clone()));
    let read_err = tokio::spawn(drain(stderr, err.clone()));
    let status = {
        let run = async {
            let status = child.wait().await?;
            let readers = async {
                let _ = read_out.await;
                let _ = read_err.await;
            };
            let _ = tokio::time::timeout(PIPE_GRACE, readers).await;
            anyhow::Ok(status)
        };
        match timeout {
            None => Some(run.await?),
            Some(limit) => tokio::time::timeout(limit, run).await.ok().transpose()?,
        }
    };
    match status {
        // A command that finished may leave something running on purpose (a server it
        // started in the background), so nothing is ended then.
        Some(_) => child.release(),
        // A timed-out one is ended now, group and all; what it wrote so far is kept.
        None => child.kill_now().await,
    }
    let stdout = out.lock().expect("output").text();
    let stderr = err.lock().expect("output").text();
    let ran = ExecOutput {
        stdout,
        stderr,
        exit_code: status.and_then(|s| s.code()),
        timed_out: status.is_none(),
    };
    Ok((ran, held))
}

/// The tools on this machine: what Trek's own Direct sessions use.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalExecutor;

impl ToolExecutor for LocalExecutor {
    fn is_local(&self) -> bool {
        true
    }

    fn temp_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }

    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a Path,
        timeout: Option<Duration>,
    ) -> BoxFuture<'a, Result<ExecOutput>> {
        Box::pin(async move {
            #[cfg(unix)]
            let shell = shell_command(command);
            #[cfg(windows)]
            let shell = shell_command(windows_shell(), command);
            Ok(run_local(shell, cwd, timeout, false).await?.0)
        })
    }

    /// On Windows with Git Bash, what the command leaves running is held in a job (see
    /// [`Held`]). With cmd.exe nothing is: what `start` began goes on as it always has.
    fn exec_background<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a Path,
        timeout: Option<Duration>,
    ) -> BoxFuture<'a, Result<(ExecOutput, Held)>> {
        Box::pin(async move {
            #[cfg(unix)]
            let (shell, keep) = (shell_command(command), false);
            #[cfg(windows)]
            let (shell, keep) = {
                let windows = windows_shell();
                (
                    shell_command(windows, command),
                    matches!(windows, WindowsShell::GitBash(_)),
                )
            };
            run_local(shell, cwd, timeout, keep).await
        })
    }

    fn read<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<Option<Vec<u8>>>> {
        Box::pin(async move {
            let meta = match tokio::fs::metadata(path).await {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            // `/dev/zero` never ends and a FIFO may never answer, and a Stop can't end a blocking
            // read: only regular files, of a size that can be held.
            if meta.is_dir() {
                bail!("Is a directory");
            }
            if !meta.is_file() {
                bail!("it isn't a regular file");
            }
            if meta.len() > MAX_READ {
                bail!(
                    "it's {} MB, too large to read whole. Use grep, or head and tail in bash",
                    meta.len() / (1024 * 1024)
                );
            }
            Ok(Some(tokio::fs::read(path).await?))
        })
    }

    fn write<'a>(&'a self, path: &'a Path, bytes: &'a [u8]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            tokio::fs::write(path, bytes).await?;
            Ok(())
        })
    }
}

/// Requests waiting for their reply, by id, and why the peer went if it has.
struct Pending {
    waiting: HashMap<u64, tokio::sync::oneshot::Sender<Value>>,
    gone: Option<String>,
    next_id: u64,
}

/// How long past a command's own timeout the peer may take to answer before the call fails,
/// and how long a read or write may take: a sandbox that never answers mustn't hang the run.
const PEER_GRACE: Duration = Duration::from_secs(120);

/// The longest reply line taken. A longer one fails its call (when its id can be found), not
/// the run.
const MAX_REPLY_LINE: usize = 64 << 20;

/// Tools run by a peer at the other end of a pipe (`trek-bench --executor stdio`: a Harbor
/// adapter that runs them in the task's container). Each request is one JSON line sent through
/// `out` (the bench writes them to stdout, between its events), and the peer answers each with
/// one line on `replies` carrying the same `id`:
///
/// - `{"type":"exec","id":1,"command":"…","cwd":"/app","timeout_s":600}` →
///   `{"type":"result","id":1,"stdout":"…","stderr":"…","exit_code":0,"timed_out":false}`
///   (`timeout_s` is the command's own limit, else the executor's `command_limit`)
/// - `{"type":"read","id":2,"path":"/app/x"}` → `{"type":"result","id":2,"content_b64":"…"}`,
///   or `{"type":"result","id":2,"not_found":true}` when there's no such file
/// - `{"type":"write","id":3,"path":"/app/x","content_b64":"…","mode":"0644"}` →
///   `{"type":"result","id":3}` (make the folders; `mode` is for a new file, one already there
///   keeps its own)
/// - any of them → `{"type":"result","id":N,"error":"…"}` when it couldn't be done (`error` is
///   a string; anything else there isn't a failure)
///
/// And one with no reply: `{"type":"cancel","id":N}` when the run no longer wants request N's
/// answer (the turn was stopped, or the call timed out here). Best effort: the peer should end
/// an `exec` still running (its whole process group), and may ignore one that's done or
/// unknown. A late reply for it is skipped.
///
/// Lines on `replies` that aren't results (or are for an id no one waits on) are skipped, and
/// so is one that isn't JSON (said on stderr). One longer than 64 MiB fails its call. When
/// `replies` ends, every call waiting and every later one fails with [`ExecutorGone`].
pub struct StdioExecutor {
    out: tokio::sync::mpsc::UnboundedSender<String>,
    pending: Arc<Mutex<Pending>>,
    /// An `exec`'s limit when the call gives none.
    command_limit: Duration,
}

/// One line from the peer, as far as it was taken.
enum Line {
    Eof,
    Whole(Vec<u8>),
    /// Longer than the cap: the id found in it, if any.
    TooLong(Option<u64>),
}

/// The next line from `reader`, at most `cap` bytes of it kept. A last line with no newline
/// counts as a line.
async fn next_line(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
    cap: usize,
) -> std::io::Result<Line> {
    use tokio::io::AsyncBufReadExt as _;
    let mut line = Vec::new();
    // Past the cap, the line's last bytes are kept too: its `id` may come last.
    let mut tail = Vec::new();
    let mut over = false;
    loop {
        let buf = reader.fill_buf().await?;
        let ended = buf.is_empty();
        let newline = buf.iter().position(|&b| b == b'\n');
        let chunk = &buf[..newline.unwrap_or(buf.len())];
        if !over && line.len() + chunk.len() <= cap {
            line.extend_from_slice(chunk);
        } else {
            let room = if over { 0 } else { cap - line.len() };
            line.extend_from_slice(&chunk[..room]);
            tail.extend_from_slice(&chunk[room..]);
            if tail.len() > 4096 {
                tail.drain(..tail.len() - 1024);
            }
            over = true;
        }
        let used = chunk.len() + usize::from(newline.is_some());
        reader.consume(used);
        if newline.is_some() || ended {
            return Ok(match (over, ended && line.is_empty()) {
                (true, _) => Line::TooLong(find_id(&line).or_else(|| find_id(&tail))),
                (false, true) => Line::Eof,
                (false, false) => Line::Whole(line),
            });
        }
    }
}

/// The number after a top-level-looking `"id":` in a reply too long to parse (one inside a JSON
/// string has its quotes escaped, so it doesn't look like this).
fn find_id(bytes: &[u8]) -> Option<u64> {
    let key = b"\"id\"";
    let mut from = 0;
    while let Some(at) = bytes[from..].windows(key.len()).position(|w| w == key) {
        let at = from + at;
        from = at + key.len();
        if at > 0 && bytes[at - 1] == b'\\' {
            continue;
        }
        let rest = &bytes[from..];
        let rest = &rest[rest.iter().take_while(|b| b.is_ascii_whitespace()).count()..];
        let Some(rest) = rest.strip_prefix(b":") else {
            continue;
        };
        let rest = &rest[rest.iter().take_while(|b| b.is_ascii_whitespace()).count()..];
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        if let Some(id) = std::str::from_utf8(&rest[..digits])
            .ok()
            .and_then(|d| d.parse().ok())
        {
            return Some(id);
        }
    }
    None
}

/// A request sent and not yet answered. Dropped before its answer (a Stop, a timeout here), it
/// stops waiting and tells the peer to cancel it.
struct InFlight<'a> {
    executor: &'a StdioExecutor,
    id: u64,
    answered: bool,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if self.answered {
            return;
        }
        let mut p = self.executor.pending.lock().unwrap();
        let waiting = p.waiting.remove(&self.id).is_some();
        let gone = p.gone.is_some();
        drop(p);
        if waiting && !gone {
            let cancel = json!({ "type": "cancel", "id": self.id });
            let _ = self.executor.out.send(cancel.to_string());
        }
    }
}

impl StdioExecutor {
    /// An executor that sends requests through `out` and reads replies from `replies` (stdin in
    /// trek-bench); an `exec` with no limit of its own gets `command_limit`. Spawns the reader
    /// on the current tokio runtime.
    pub fn new(
        out: tokio::sync::mpsc::UnboundedSender<String>,
        replies: impl AsyncRead + Send + Unpin + 'static,
        command_limit: Duration,
    ) -> Self {
        Self::with_line_cap(out, replies, command_limit, MAX_REPLY_LINE)
    }

    fn with_line_cap(
        out: tokio::sync::mpsc::UnboundedSender<String>,
        replies: impl AsyncRead + Send + Unpin + 'static,
        command_limit: Duration,
        cap: usize,
    ) -> Self {
        let pending = Arc::new(Mutex::new(Pending {
            waiting: HashMap::new(),
            gone: None,
            next_id: 1,
        }));
        let reader = pending.clone();
        tokio::spawn(async move {
            let mut replies = BufReader::new(replies);
            let reason = loop {
                let v = match next_line(&mut replies, cap).await {
                    Ok(Line::Whole(line)) if line.trim_ascii().is_empty() => continue,
                    Ok(Line::Whole(line)) => match serde_json::from_slice::<Value>(&line) {
                        Ok(v) => v,
                        Err(e) => {
                            let start =
                                String::from_utf8_lossy(&line[..line.len().min(200)]).into_owned();
                            tracing::warn!(
                                "stdio executor: skipped a reply that isn't JSON ({e}): {start}"
                            );
                            continue;
                        }
                    },
                    Ok(Line::TooLong(id)) => {
                        tracing::warn!(
                            "stdio executor: a reply over {} MiB (id {id:?}) was dropped",
                            cap >> 20
                        );
                        let Some(id) = id else { continue };
                        json!({
                            "type": "result",
                            "id": id,
                            "error": format!("the reply was over {} MiB", cap >> 20),
                        })
                    }
                    Ok(Line::Eof) => {
                        break "the executor's peer closed its end (stdin)".to_string();
                    }
                    Err(e) => break format!("reading the executor's replies failed: {e}"),
                };
                if v["type"] != "result" {
                    continue;
                }
                let Some(id) = v["id"].as_u64() else { continue };
                let waiter = reader.lock().unwrap().waiting.remove(&id);
                if let Some(tx) = waiter {
                    let _ = tx.send(v);
                }
            };
            let mut p = reader.lock().unwrap();
            p.gone = Some(reason);
            // Dropping the senders wakes everyone waiting with the same news.
            p.waiting.clear();
        });
        StdioExecutor {
            out,
            pending,
            command_limit,
        }
    }

    /// Send `request` (its `id` filled in) and wait up to `limit` for the reply. A reply with
    /// `error` is that error.
    async fn call(&self, mut request: Value, limit: Duration) -> Result<Value> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let id = {
            let mut p = self.pending.lock().unwrap();
            if let Some(reason) = &p.gone {
                return Err(ExecutorGone(reason.clone()).into());
            }
            let id = p.next_id;
            p.next_id += 1;
            p.waiting.insert(id, tx);
            id
        };
        let mut in_flight = InFlight {
            executor: self,
            id,
            answered: false,
        };
        request["id"] = json!(id);
        if self.out.send(request.to_string()).is_err() {
            return Err(ExecutorGone("the executor's output closed".into()).into());
        }
        let reply = match tokio::time::timeout(limit, rx).await {
            Ok(Ok(reply)) => {
                in_flight.answered = true;
                reply
            }
            Ok(Err(_)) => {
                let reason = self.pending.lock().unwrap().gone.clone();
                return Err(
                    ExecutorGone(reason.unwrap_or_else(|| "the executor stopped".into())).into(),
                );
            }
            // `in_flight` cancels it.
            Err(_) => bail!(
                "the sandbox didn't answer within {} seconds",
                limit.as_secs()
            ),
        };
        if let Some(e) = reply["error"].as_str() {
            return Err(anyhow!("{e}"));
        }
        Ok(reply)
    }
}

/// `path` as the peer gets it, as written.
fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl ToolExecutor for StdioExecutor {
    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a Path,
        timeout: Option<Duration>,
    ) -> BoxFuture<'a, Result<ExecOutput>> {
        Box::pin(async move {
            let timeout = timeout.unwrap_or(self.command_limit);
            let request = json!({
                "type": "exec",
                "command": command,
                "cwd": path_text(cwd),
                "timeout_s": timeout.as_secs_f64().ceil() as u64,
            });
            // The call may take as long as the command may, and a while more.
            let reply = self.call(request, timeout + PEER_GRACE).await?;
            Ok(ExecOutput {
                stdout: reply["stdout"].as_str().unwrap_or_default().to_string(),
                stderr: reply["stderr"].as_str().unwrap_or_default().to_string(),
                exit_code: reply["exit_code"].as_i64().map(|c| c as i32),
                timed_out: reply["timed_out"].as_bool().unwrap_or(false),
            })
        })
    }

    fn read<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<Option<Vec<u8>>>> {
        Box::pin(async move {
            let request = json!({ "type": "read", "path": path_text(path) });
            let reply = self.call(request, PEER_GRACE).await?;
            if reply["not_found"].as_bool() == Some(true) {
                return Ok(None);
            }
            let b64 = reply["content_b64"]
                .as_str()
                .context("the reply has no content_b64")?;
            Ok(Some(
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .context("content_b64 isn't base64")?,
            ))
        })
    }

    fn write<'a>(&'a self, path: &'a Path, bytes: &'a [u8]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let request = json!({
                "type": "write",
                "path": path_text(path),
                "content_b64": base64::engine::general_purpose::STANDARD.encode(bytes),
                "mode": "0644",
            });
            self.call(request, PEER_GRACE).await?;
            Ok(())
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt as _;

    /// A peer that keeps files in a map, answers `exec` itself, fails the command `fail`, and
    /// hangs up at the command `hang up`. Returns every request it was sent.
    fn fake_peer() -> (StdioExecutor, tokio::task::JoinHandle<Vec<Value>>) {
        let (out, mut requests) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (mut to_exec, replies) = tokio::io::duplex(1 << 16);
        let exec = StdioExecutor::new(out, replies, Duration::from_secs(600));
        let peer = tokio::spawn(async move {
            let mut files: HashMap<String, String> = HashMap::new();
            let mut seen = Vec::new();
            while let Some(line) = requests.recv().await {
                let r: Value = serde_json::from_str(&line).unwrap();
                seen.push(r.clone());
                let id = r["id"].clone();
                let reply = match r["type"].as_str().unwrap() {
                    "exec" if r["command"] == "hang up" => break,
                    "exec" if r["command"] == "fail" => {
                        json!({ "type": "result", "id": id, "error": "no shell" })
                    }
                    "exec" => json!({
                        "type": "result", "id": id,
                        "stdout": format!("ran {} in {}", r["command"].as_str().unwrap(), r["cwd"].as_str().unwrap()),
                        "stderr": "warn", "exit_code": 3, "timed_out": false,
                        // Not a string: not a failure.
                        "error": false,
                    }),
                    "read" => match files.get(r["path"].as_str().unwrap()) {
                        Some(b64) => json!({ "type": "result", "id": id, "content_b64": b64 }),
                        None => json!({ "type": "result", "id": id, "not_found": true }),
                    },
                    "write" => {
                        files.insert(
                            r["path"].as_str().unwrap().into(),
                            r["content_b64"].as_str().unwrap().into(),
                        );
                        json!({ "type": "result", "id": id })
                    }
                    other => panic!("unexpected {other}"),
                };
                // Noise first: the executor skips what isn't a result it waits on, a line that
                // isn't UTF-8, and a blank one.
                to_exec
                    .write_all(b"not json\n\xff\xfe{\n\n{\"type\":\"result\",\"id\":999}\n")
                    .await
                    .unwrap();
                to_exec
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
            seen
        });
        (exec, peer)
    }

    #[tokio::test]
    async fn the_stdio_executor_speaks_the_protocol() {
        let (exec, peer) = fake_peer();
        let path = Path::new("/app/notes.txt");
        assert_eq!(exec.read(path).await.unwrap(), None);
        exec.write(path, "héllo\n".as_bytes()).await.unwrap();
        assert_eq!(
            exec.read(path).await.unwrap().unwrap(),
            "héllo\n".as_bytes()
        );
        let out = exec
            .exec("ls", Path::new("/app"), Some(Duration::from_secs(5)))
            .await
            .unwrap();
        assert_eq!(out.stdout, "ran ls in /app");
        assert_eq!(out.stderr, "warn");
        assert_eq!(out.exit_code, Some(3));
        assert!(!out.timed_out && !out.success());
        let e = exec
            .exec("fail", Path::new("/app"), None)
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "no shell");
        assert!(e.downcast_ref::<ExecutorGone>().is_none());

        // The peer hangs up mid-call: that call and every later one are told it's gone.
        let e = exec
            .exec("hang up", Path::new("/app"), None)
            .await
            .unwrap_err();
        assert!(e.downcast_ref::<ExecutorGone>().is_some(), "{e:#}");
        let e = exec.read(path).await.unwrap_err();
        assert!(e.downcast_ref::<ExecutorGone>().is_some(), "{e:#}");

        let seen = peer.await.unwrap();
        let ids: Vec<u64> = seen.iter().map(|r| r["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6]);
        assert_eq!(seen[1]["mode"], "0644");
        assert_eq!(seen[3]["timeout_s"], 5);
        // No limit of its own: the executor's.
        assert_eq!(seen[4]["timeout_s"], 600);
    }

    #[tokio::test]
    async fn a_dropped_call_is_cancelled_and_its_late_reply_skipped() {
        let (out, mut requests) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (mut to_exec, replies) = tokio::io::duplex(1 << 16);
        let exec = StdioExecutor::new(out, replies, Duration::from_secs(600));
        // Given up on here, as a Stop or a timeout does.
        let gave_up = tokio::time::timeout(
            Duration::from_millis(100),
            exec.exec("sleep 99", Path::new("/app"), None),
        )
        .await;
        assert!(gave_up.is_err());
        let sent: Value = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
        assert_eq!((&sent["type"], &sent["id"]), (&json!("exec"), &json!(1)));
        let cancel: Value = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
        assert_eq!(cancel, json!({ "type": "cancel", "id": 1 }));
        assert!(exec.pending.lock().unwrap().waiting.is_empty());

        // A late reply for it finds no one waiting; the next call gets its own answer (and an
        // `error` that isn't a string isn't a failure).
        to_exec
            .write_all(b"{\"type\":\"result\",\"id\":1,\"stdout\":\"late\"}\n")
            .await
            .unwrap();
        let answer = async {
            let r: Value = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
            assert_eq!(r["id"], 2);
            let reply = json!({ "type": "result", "id": 2, "stdout": "fresh", "error": 0 });
            to_exec
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        };
        let (ran, ()) = tokio::join!(
            exec.exec("echo", Path::new("/app"), Some(Duration::from_secs(5))),
            answer
        );
        assert_eq!(ran.unwrap().stdout, "fresh");
        // Answered calls send no cancel.
        assert!(requests.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_reply_too_long_fails_its_call_not_the_run() {
        let (out, mut requests) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (mut to_exec, replies) = tokio::io::duplex(1 << 16);
        let exec = StdioExecutor::with_line_cap(out, replies, Duration::from_secs(600), 64);
        let peer = async {
            let r: Value = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
            assert_eq!(r["id"], 1);
            // Its id comes last, past the cap.
            let long = format!(
                "{{\"type\":\"result\",\"content_b64\":\"{}\",\"id\":1}}\n",
                "A".repeat(500)
            );
            to_exec.write_all(long.as_bytes()).await.unwrap();
            // One with no id at all is skipped.
            to_exec
                .write_all(format!("{}\n", "y".repeat(500)).as_bytes())
                .await
                .unwrap();
            let r: Value = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
            assert_eq!(r["id"], 2);
            to_exec
                .write_all(b"{\"type\":\"result\",\"id\":2,\"not_found\":true}\n")
                .await
                .unwrap();
        };
        let calls = async {
            let e = exec.read(Path::new("/app/big")).await.unwrap_err();
            assert!(e.to_string().contains("over"), "{e:#}");
            assert!(e.downcast_ref::<ExecutorGone>().is_none());
            exec.read(Path::new("/app/none")).await.unwrap()
        };
        let (found, ()) = tokio::join!(calls, peer);
        assert_eq!(found, None);
    }

    #[test]
    fn an_id_is_found_outside_strings_only() {
        assert_eq!(find_id(br#"{"type":"result", "id" : 42 ,"x":1}"#), Some(42));
        assert_eq!(find_id(br#"{"stdout":"\"id\":7","id":8}"#), Some(8));
        assert_eq!(find_id(br#"{"stdout":"\"id\":7"}"#), None);
    }

    #[tokio::test]
    async fn the_local_executor_round_trips_and_times_out() {
        let dir = std::env::temp_dir().join(format!("trek_local_exec_{}", super::super::next_id()));
        let file = dir.join("deep/er/a.txt");
        assert_eq!(LocalExecutor.read(&file).await.unwrap(), None);
        LocalExecutor.write(&file, b"one\n").await.unwrap();
        assert_eq!(LocalExecutor.read(&file).await.unwrap().unwrap(), b"one\n");

        // The same in Git Bash on Windows; cmd.exe has its own test below.
        #[cfg(windows)]
        let shell = git_bash_here();
        #[cfg(unix)]
        let shell = true;
        if shell {
            let out = LocalExecutor
                .exec("cat deep/er/a.txt; echo oops >&2; exit 4", &dir, None)
                .await
                .unwrap();
            assert_eq!(
                (
                    out.stdout.as_str(),
                    out.stderr.as_str(),
                    out.exit_code,
                    out.timed_out
                ),
                ("one\n", "oops\n", Some(4), false)
            );
            let started = std::time::Instant::now();
            let out = LocalExecutor
                .exec(
                    "echo early; sleep 30",
                    &dir,
                    Some(Duration::from_millis(500)),
                )
                .await
                .unwrap();
            assert!(out.timed_out && out.exit_code.is_none(), "{out:?}");
            assert_eq!(out.stdout, "early\n");
            assert!(started.elapsed() < Duration::from_secs(10));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whether Git Bash is here for a Windows test that needs it, saying so when it isn't.
    #[cfg(windows)]
    pub(crate) fn git_bash_here() -> bool {
        let found = git_bash().is_some();
        if !found {
            eprintln!("Git for Windows isn't installed here; skipped");
        }
        found
    }

    /// A process held open by id, so the id stays its own while a test watches it end.
    #[cfg(windows)]
    pub(crate) struct Watched(std::os::windows::io::OwnedHandle);

    #[cfg(windows)]
    impl Watched {
        /// Process `pid`, which must still be running.
        pub(crate) fn open(pid: u32) -> Watched {
            use std::os::windows::io::FromRawHandle as _;
            use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
            // SAFETY: a plain call; a null handle is checked for.
            let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            assert!(!h.is_null(), "process {pid} isn't running");
            let watched = Watched(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(h) });
            assert!(
                !watched.ends_within(Duration::ZERO),
                "process {pid} has exited"
            );
            watched
        }

        /// It has ended, or does within `limit`.
        pub(crate) fn ends_within(&self, limit: Duration) -> bool {
            use std::os::windows::io::AsRawHandle as _;
            use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
            use windows_sys::Win32::System::Threading::WaitForSingleObject;
            let ms = u32::try_from(limit.as_millis()).unwrap_or(u32::MAX);
            // SAFETY: a process handle this owns, opened with SYNCHRONIZE.
            unsafe { WaitForSingleObject(self.0.as_raw_handle(), ms) == WAIT_OBJECT_0 }
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_script_reaches_git_bash_exactly_as_written() {
        if !git_bash_here() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("trek_git_bash_{}", super::super::next_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("one.txt"), "").unwrap();
        // What a command line would change: a backslash before one that isn't before a quote,
        // quotes, what globs and braces expand, a line break, and past 8,000 characters.
        let tricky = format!(
            "C:\\\\x \\\" \\ \"q\" 'q' *.txt {{a,b}} ~ é 日本\nline two {}",
            "y".repeat(20_000)
        );
        let script = format!("cat <<'EOF'\n{tricky}\nEOF\nexit 7");
        let out = LocalExecutor.exec(&script, &dir, None).await.unwrap();
        assert_eq!(out.exit_code, Some(7), "{}", out.stderr);
        assert_eq!(out.stdout, format!("{tricky}\n"));
        // Nothing the script runs sees it in its environment.
        let out = LocalExecutor
            .exec("env | grep -c TREK_COMMAND", &dir, None)
            .await
            .unwrap();
        assert_eq!(out.stdout.trim(), "0", "{out:?}");
        // A syntax error is told as bash tells it, and nothing after it runs.
        let out = LocalExecutor
            .exec("echo before\nif then fi\necho after", &dir, None)
            .await
            .unwrap();
        assert_eq!((out.stdout.as_str(), out.exit_code), ("before\n", Some(2)));
        assert!(out.stderr.contains("syntax error"), "{}", out.stderr);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn without_git_bash_commands_run_in_cmd_exe_as_before() {
        let dir = std::env::temp_dir();
        let shell = shell_command(
            WindowsShell::Cmd,
            r#"echo "a b"& echo oops 1>&2& exit /b 3"#,
        );
        let (out, _) = run_local(shell, &dir, None, false).await.unwrap();
        assert_eq!(
            (out.stdout.as_str(), out.stderr.trim(), out.exit_code),
            ("\"a b\"\r\n", "oops", Some(3))
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn what_a_background_command_leaves_running_ends_with_its_hold() {
        if !git_bash_here() {
            return;
        }
        // `sleep` is one of Git Bash's programs: the shell forked to run it has handed it its
        // place and gone (give it a second to), so no parent id leads to it from the command's
        // shell. `/proc/…/winpid` then says the `sleep`'s Windows id.
        let (out, held) = LocalExecutor
            .exec_background(
                "sleep 30 > /dev/null 2>&1 &\nsleep 1; cat /proc/$!/winpid",
                &std::env::temp_dir(),
                Some(Duration::from_secs(10)),
            )
            .await
            .unwrap();
        assert!(out.success(), "{out:?}");
        let pid: u32 = out.stdout.trim().parse().expect("a pid");
        let sleep = Watched::open(pid);
        // The command is done, and what it left goes on...
        assert!(!sleep.ends_within(Duration::from_millis(500)));
        // ...until the hold goes.
        drop(held);
        assert!(sleep.ends_within(Duration::from_secs(5)));
    }

    #[cfg(windows)]
    #[test]
    fn git_bash_is_found_beside_git_or_where_git_installs_never_wsls() {
        let root = std::env::temp_dir().join(format!("trek_find_bash_{}", super::super::next_id()));
        let make = |files: &[&str]| {
            for f in files {
                let path = root.join(f);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, "").unwrap();
            }
        };
        make(&[
            r"Portable\bin\bash.exe",
            r"Portable\usr\bin\bash.exe",
            r"Portable\cmd\git.exe",
            r"Portable\mingw64\bin\git.exe",
            r"Programs\Git\bin\bash.exe",
            r"Programs\Git\usr\bin\bash.exe",
            // WSL's bash, what looks like Git in Windows' own folder, and a Git folder that's
            // only the launcher, with no shell behind it.
            r"Windows\System32\bash.exe",
            r"Windows\System32\git.exe",
            r"Windows\bin\bash.exe",
            r"Windows\usr\bin\bash.exe",
            r"Half\bin\bash.exe",
            r"Half\cmd\git.exe",
        ]);
        let vars = |name: &str| match name {
            "SystemRoot" => Some(root.join("Windows").into_os_string()),
            "LOCALAPPDATA" => Some(root.clone().into_os_string()),
            _ => None,
        };
        let found = |git: &str| find_git_bash(Some(&root.join(git)), vars);
        // Beside the git on the PATH, whichever of Git's folders that one is in.
        let portable = Some(root.join(r"Portable\bin\bash.exe"));
        assert_eq!(found(r"Portable\cmd\git.exe"), portable);
        assert_eq!(found(r"Portable\mingw64\bin\git.exe"), portable);
        // Else where Git installs (here the per-user one): not WSL's, nor half of Git.
        let installed = Some(root.join(r"Programs\Git\bin\bash.exe"));
        assert_eq!(found(r"Windows\System32\git.exe"), installed);
        assert_eq!(found(r"Half\cmd\git.exe"), installed);
        assert_eq!(find_git_bash(None, vars), installed);
        // No git, and nowhere to look.
        assert_eq!(find_git_bash(None, |_| None), None);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(find_git_bash(None, vars), None);
    }
}
