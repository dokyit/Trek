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
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, BufReader};

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
}

/// The tools on this machine: what Trek's own Direct sessions use.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalExecutor;

/// Kills a command's process group if it's dropped before the command ended (a Stop, a
/// timeout). A command that finishes may leave something running on purpose (a server it
/// started in the background), so nothing is ended then: `disarm` first.
struct GroupGuard(Option<u32>);

impl GroupGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0 {
            // SAFETY: a negative pid asks kill(2) to signal that process group, the child's own
            // (it isn't reaped yet while the guard is armed, so the id is still its).
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        }
    }
}

impl ToolExecutor for LocalExecutor {
    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a Path,
        timeout: Option<Duration>,
    ) -> BoxFuture<'a, Result<ExecOutput>> {
        Box::pin(async move {
            #[cfg(unix)]
            let mut shell = trek_core::procs::async_command("sh");
            #[cfg(unix)]
            shell.arg("-c");
            #[cfg(windows)]
            let mut shell = trek_core::procs::async_command("cmd.exe");
            #[cfg(windows)]
            shell.arg("/c");
            shell
                .arg(command)
                .current_dir(cwd)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            // In a group of its own, so a Stop ends what the command started too, not just the
            // shell.
            #[cfg(unix)]
            shell.process_group(0);
            let mut child = shell.spawn()?;
            let mut guard = GroupGuard(child.id());

            let mut stdout = child.stdout.take().expect("stdout");
            let mut stderr = child.stderr.take().expect("stderr");
            let mut stdout_buf = Vec::new();
            let mut stderr_buf = Vec::new();
            let status = {
                let read_out = tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut stdout_buf);
                let read_err = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut stderr_buf);
                let run = async {
                    tokio::try_join!(read_out, read_err)?;
                    child.wait().await
                };
                match timeout {
                    None => Some(run.await?),
                    Some(limit) => tokio::time::timeout(limit, run).await.ok().transpose()?,
                }
            };
            if status.is_some() {
                guard.disarm();
            }
            // A timed-out command's group goes with the guard; what it wrote so far is kept.
            drop(guard);
            Ok(ExecOutput {
                stdout: String::from_utf8_lossy(&stdout_buf).into_owned(),
                stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
                exit_code: status.and_then(|s| s.code()),
                timed_out: status.is_none(),
            })
        })
    }

    fn read<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<Option<Vec<u8>>>> {
        Box::pin(async move {
            if !path.exists() {
                return Ok(None);
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

/// Tools run by a peer at the other end of a pipe (`trek-bench --executor stdio`: a Harbor
/// adapter that runs them in the task's container). Each request is one JSON line sent through
/// `out` (the bench writes them to stdout, between its events), and the peer answers each with
/// one line on `replies` carrying the same `id`:
///
/// - `{"type":"exec","id":1,"command":"…","cwd":"/app","timeout_s":600}` →
///   `{"type":"result","id":1,"stdout":"…","stderr":"…","exit_code":0,"timed_out":false}`
///   (`timeout_s` is null when the command has no limit)
/// - `{"type":"read","id":2,"path":"/app/x"}` → `{"type":"result","id":2,"content_b64":"…"}`,
///   or `{"type":"result","id":2,"not_found":true}` when there's no such file
/// - `{"type":"write","id":3,"path":"/app/x","content_b64":"…","mode":"0644"}` →
///   `{"type":"result","id":3}` (make the folders; `mode` is for a new file, one already there
///   keeps its own)
/// - any of them → `{"type":"result","id":N,"error":"…"}` when it couldn't be done
///
/// Lines on `replies` that aren't results (or are for an id no one waits on) are skipped.
/// When `replies` ends, every call waiting and every later one fails with [`ExecutorGone`].
pub struct StdioExecutor {
    out: tokio::sync::mpsc::UnboundedSender<String>,
    pending: Arc<Mutex<Pending>>,
}

impl StdioExecutor {
    /// An executor that sends requests through `out` and reads replies from `replies` (stdin in
    /// trek-bench). Spawns the reader on the current tokio runtime.
    pub fn new(
        out: tokio::sync::mpsc::UnboundedSender<String>,
        replies: impl AsyncRead + Send + Unpin + 'static,
    ) -> Self {
        let pending = Arc::new(Mutex::new(Pending {
            waiting: HashMap::new(),
            gone: None,
            next_id: 1,
        }));
        let reader = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(replies).lines();
            let reason = loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
                            tracing::warn!("stdio executor: a reply that isn't JSON: {line}");
                            continue;
                        };
                        if v["type"] != "result" {
                            continue;
                        }
                        let Some(id) = v["id"].as_u64() else { continue };
                        let waiter = reader.lock().unwrap().waiting.remove(&id);
                        if let Some(tx) = waiter {
                            let _ = tx.send(v);
                        }
                    }
                    Ok(None) => break "the executor's peer closed its end (stdin)".to_string(),
                    Err(e) => break format!("reading the executor's replies failed: {e}"),
                }
            };
            let mut p = reader.lock().unwrap();
            p.gone = Some(reason);
            // Dropping the senders wakes everyone waiting with the same news.
            p.waiting.clear();
        });
        StdioExecutor { out, pending }
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
        request["id"] = json!(id);
        if self.out.send(request.to_string()).is_err() {
            self.pending.lock().unwrap().waiting.remove(&id);
            return Err(ExecutorGone("the executor's output closed".into()).into());
        }
        let reply = match tokio::time::timeout(limit, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => {
                let reason = self.pending.lock().unwrap().gone.clone();
                return Err(
                    ExecutorGone(reason.unwrap_or_else(|| "the executor stopped".into())).into(),
                );
            }
            Err(_) => {
                self.pending.lock().unwrap().waiting.remove(&id);
                bail!(
                    "the sandbox didn't answer within {} seconds",
                    limit.as_secs()
                );
            }
        };
        if let Some(e) = reply.get("error").filter(|e| !e.is_null()) {
            return Err(anyhow!(
                "{}",
                e.as_str().map_or_else(|| e.to_string(), String::from)
            ));
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
            let request = json!({
                "type": "exec",
                "command": command,
                "cwd": path_text(cwd),
                "timeout_s": timeout.map(|t| t.as_secs_f64().ceil() as u64),
            });
            // The call may take as long as the command may, and a while more.
            let limit = timeout.map_or(Duration::from_secs(24 * 3600), |t| t + PEER_GRACE);
            let reply = self.call(request, limit).await?;
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
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt as _;

    /// A peer that keeps files in a map, answers `exec` itself, fails the command `fail`, and
    /// hangs up at the command `hang up`. Returns every request it was sent.
    fn fake_peer() -> (StdioExecutor, tokio::task::JoinHandle<Vec<Value>>) {
        let (out, mut requests) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (mut to_exec, replies) = tokio::io::duplex(1 << 16);
        let exec = StdioExecutor::new(out, replies);
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
                // Noise first: the executor skips what isn't a result it waits on.
                to_exec
                    .write_all(b"not json\n{\"type\":\"result\",\"id\":999}\n")
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
        assert!(seen[4]["timeout_s"].is_null());
    }

    #[tokio::test]
    async fn the_local_executor_round_trips_and_times_out() {
        let dir = std::env::temp_dir().join(format!("trek_local_exec_{}", super::super::next_id()));
        let file = dir.join("deep/er/a.txt");
        assert_eq!(LocalExecutor.read(&file).await.unwrap(), None);
        LocalExecutor.write(&file, b"one\n").await.unwrap();
        assert_eq!(LocalExecutor.read(&file).await.unwrap().unwrap(), b"one\n");

        #[cfg(unix)]
        {
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
}
