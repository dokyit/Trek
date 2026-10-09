//! Trek's end of the local channel its agents' `trek-mcp orchestrate` servers call (see
//! `trek_ipc` for the protocol). One socket per Trek process, in a folder only the user can
//! enter (on Windows a named pipe only the user can open); a connection must come from the same
//! user, carry this process's token and name a session Trek started. Calls go to the workspace
//! (`Workspace::handle_call`) on the main thread; their answers come back here. Nothing that
//! passes through is logged.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};

/// How long a new connection has to say hello.
const HELLO_WITHIN: Duration = Duration::from_secs(5);
/// How long `delegate_task` with `wait` waits unless asked otherwise, and the bounds it may ask for.
pub const WAIT_DEFAULT: Duration = Duration::from_secs(600);
/// Shorter in tests, so a wait that runs out doesn't hold them up.
const WAIT_MIN: u64 = if cfg!(test) { 1 } else { 10 };
const WAIT_MAX: u64 = 1800;

/// A tool call from an agent's session, for the workspace to answer.
pub struct Call {
    /// The thread whose agent made it.
    pub thread: String,
    pub method: String,
    pub params: Value,
    pub reply: async_channel::Sender<Reply>,
}

/// The workspace's answer to a `Call`.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// The caller waits for a sub-agent's answer (one it started, or asked for), for as long as given:
    /// another reply follows when it ends, unless the wait runs out first.
    Waiting(String, Duration),
    Done(Result<Value, String>),
}

/// Sessions Trek started, by key: the thread each belongs to (`None` for a session warmed up for
/// a thread that doesn't exist yet).
type Sessions = Arc<Mutex<HashMap<String, Option<String>>>>;

pub struct IpcServer {
    /// The socket's path, or the pipe's name on Windows.
    pub path: PathBuf,
    token: String,
    sessions: Sessions,
    listener: tokio::task::JoinHandle<()>,
}

impl IpcServer {
    /// Listen in `dir` (or a shorter private folder when its path is too long for a socket). On
    /// Windows, on a named pipe of its own instead, which lives in no folder.
    pub fn start(dir: &Path) -> std::io::Result<(IpcServer, async_channel::Receiver<Call>)> {
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = address(dir, N.fetch_add(1, Ordering::Relaxed))?;
        let token = trek_ipc::token()?;
        let sessions: Sessions = Default::default();
        let (calls_tx, calls) = async_channel::unbounded();
        let rt = trek_core::runtime();
        let mut listener = {
            let _enter = rt.enter();
            trek_ipc::server::Listener::bind(&path)?
        };
        let (tok, sess) = (Arc::<str>::from(token.as_str()), sessions.clone());
        let listener = rt.spawn(async move {
            loop {
                match listener.accept().await {
                    Ok(stream) => _ = tokio::spawn(serve(stream, tok.clone(), sess.clone(), calls_tx.clone())),
                    // Out of file descriptors, say: the error comes straight back, so pause
                    // rather than spin.
                    Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            }
        });
        Ok((IpcServer { path, token, sessions, listener }, calls))
    }

    /// A key for a new agent session, for `thread` (or one to name later with `bind`).
    pub fn open_session(&self, thread: Option<&str>) -> String {
        let key = trek_ipc::token().unwrap_or_else(|_| uuid_like());
        self.sessions.lock().unwrap().insert(key.clone(), thread.map(str::to_string));
        key
    }

    /// The thread a warmed-up session turned out to be for.
    pub fn bind(&self, key: &str, thread: &str) {
        if let Some(t) = self.sessions.lock().unwrap().get_mut(key) {
            *t = Some(thread.to_string());
        }
    }

    /// The session ended: its key is no good any more.
    pub fn close_session(&self, key: &str) {
        self.sessions.lock().unwrap().remove(key);
    }

    /// What `trek-mcp orchestrate` needs, as environment variables for session `key`.
    pub fn env(&self, key: &str) -> Vec<(String, String)> {
        vec![
            (trek_ipc::ENV_SOCKET.into(), self.path.display().to_string()),
            (trek_ipc::ENV_TOKEN.into(), self.token.clone()),
            (trek_ipc::ENV_SESSION.into(), key.to_string()),
        ]
    }

    #[cfg(test)]
    pub fn client(&self, key: &str) -> trek_ipc::Client {
        trek_ipc::Client { socket: self.path.clone(), token: self.token.clone(), session: key.to_string() }
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.listener.abort();
        // A pipe goes by itself, with its last instance.
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Where server number `n` of this process listens: a socket in `dir` (once sockets left by
/// Trek processes that have gone are cleared out of it).
#[cfg(unix)]
fn address(dir: &Path, n: usize) -> std::io::Result<PathBuf> {
    let name = format!("trek-{}-{n}.sock", std::process::id());
    let dir = trek_ipc::socket_dir(dir, &name)?;
    sweep(&dir);
    Ok(dir.join(name))
}

/// Where server number `n` of this process listens: a pipe, named by `trek_ipc::pipe_name` (at
/// random in part, and with this process's id, which clients check).
#[cfg(windows)]
fn address(_dir: &Path, n: usize) -> std::io::Result<PathBuf> {
    trek_ipc::pipe_name(n)
}

/// A session key when the system's random source can't be read (it always can on macOS).
fn uuid_like() -> String {
    format!("{:x}{:x}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0))
}

/// Remove sockets left by Trek processes that have gone (a crash leaves its socket behind).
/// Pipes need none of this: they go with their process.
#[cfg(unix)]
fn sweep(dir: &Path) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix("trek-")).and_then(|n| n.split('-').next()).and_then(|p| p.parse::<i32>().ok()) else { continue };
        // SAFETY: signal 0 only checks whether the process exists.
        if pid != std::process::id() as i32 && unsafe { libc::kill(pid, 0) } != 0 {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// How long a call may wait for a sub-agent: its `timeout_seconds` within bounds, for a
/// `delegate_task` or `task_result` that waits.
pub fn wait_for(params: &Value) -> Duration {
    params.get("timeout_seconds").and_then(Value::as_u64).map_or(WAIT_DEFAULT, |s| Duration::from_secs(s.clamp(WAIT_MIN, WAIT_MAX)))
}

/// Read one frame, at most `MAX_FRAME` bytes. `Ok(None)` at the end of the stream.
async fn read_frame<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = (&mut *r).take(trek_ipc::MAX_FRAME as u64 + 1).read_until(b'\n', &mut buf).await?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > trek_ipc::MAX_FRAME {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too long"));
    }
    String::from_utf8(buf).map(Some).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "frame isn't UTF-8"))
}

async fn write(w: &mut tokio::io::WriteHalf<trek_ipc::server::Stream>, v: &Value) -> std::io::Result<()> {
    w.write_all(trek_ipc::encode(v).as_bytes()).await
}

/// One connection: the hello, then requests one at a time.
async fn serve(stream: trek_ipc::server::Stream, token: Arc<str>, sessions: Sessions, calls: async_channel::Sender<Call>) {
    // Another user's process gets nothing, not even an error.
    if !trek_ipc::server::peer_is_me(&stream) {
        return;
    }
    let (rd, mut w) = tokio::io::split(stream);
    let mut r = tokio::io::BufReader::new(rd);
    let hello = match tokio::time::timeout(HELLO_WITHIN, read_frame(&mut r)).await {
        Ok(Ok(Some(line))) => serde_json::from_str::<Value>(&line).ok(),
        _ => return,
    };
    let session = match hello.as_ref().and_then(trek_ipc::parse_hello) {
        Some((trek_ipc::VERSION, t, s)) if trek_ipc::same_secret(t, &token) && sessions.lock().unwrap().contains_key(s) => s.to_string(),
        Some((trek_ipc::VERSION, t, _)) if trek_ipc::same_secret(t, &token) => {
            let _ = write(&mut w, &json!({ "error": "Trek doesn't know this agent session (it may have ended). Start a new message to get a fresh one." })).await;
            return;
        }
        Some((trek_ipc::VERSION, _, _)) => {
            let _ = write(&mut w, &json!({ "error": "Trek refused the connection: wrong token." })).await;
            return;
        }
        Some((v, _, _)) => {
            let _ = write(&mut w, &json!({ "error": format!("This trek-mcp speaks version {v} of Trek's protocol; this Trek speaks {}.", trek_ipc::VERSION) })).await;
            return;
        }
        None => {
            let _ = write(&mut w, &json!({ "error": "Expected a hello first." })).await;
            return;
        }
    };
    if write(&mut w, &json!({ "ok": true })).await.is_err() {
        return;
    }
    loop {
        let line = match read_frame(&mut r).await {
            Ok(Some(line)) => line,
            Ok(None) => return,
            Err(e) => {
                let _ = write(&mut w, &trek_ipc::reply(&Value::Null, Err(format!("Bad request: {e}")))).await;
                return;
            }
        };
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            let _ = write(&mut w, &trek_ipc::reply(&Value::Null, Err("Bad request: not JSON".into()))).await;
            continue;
        };
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let (Some(method), params) = (req.get("method").and_then(Value::as_str), req.get("params").cloned().unwrap_or_else(|| json!({}))) else {
            let _ = write(&mut w, &trek_ipc::reply(&id, Err("Bad request: no method".into()))).await;
            continue;
        };
        let thread = sessions.lock().unwrap().get(&session).cloned();
        let Some(Some(thread)) = thread else {
            let message = match thread {
                Some(None) => "This session isn't part of a thread yet. Try again in a moment.",
                _ => "Trek doesn't know this agent session any more.",
            };
            let _ = write(&mut w, &trek_ipc::reply(&id, Err(message.into()))).await;
            continue;
        };
        let (reply, replies) = async_channel::unbounded();
        if calls.send(Call { thread, method: method.to_string(), params, reply }).await.is_err() {
            let _ = write(&mut w, &trek_ipc::reply(&id, Err("Trek is shutting down.".into()))).await;
            return;
        }
        let answer = match replies.recv().await {
            Ok(Reply::Done(result)) => result,
            Ok(Reply::Waiting(child, wait)) => {
                // Wait for the sub-agent, for as long as asked, and for as long as the caller
                // stays (a cancelled call drops its connection).
                let deadline = tokio::time::sleep(wait);
                tokio::pin!(deadline);
                loop {
                    tokio::select! {
                        done = replies.recv() => break match done {
                            Ok(Reply::Done(result)) => result,
                            _ => Err("Trek stopped waiting for the sub-agent.".into()),
                        },
                        _ = &mut deadline => break Ok(json!({
                            "id": child,
                            "status": "running",
                            "note": format!("Still running after {} s. It carries on: Trek will send you its answer in a message when it finishes, or call task_status / task_result later.", wait.as_secs()),
                        })),
                        gone = r.fill_buf() => match gone {
                            Ok([]) | Err(_) => return,
                            // Nothing should arrive mid-call; whatever does is dropped.
                            Ok(buf) => {
                                let n = buf.len();
                                r.consume(n);
                            }
                        },
                    }
                }
            }
            Err(_) => Err("Trek didn't answer.".into()),
        };
        if write(&mut w, &trek_ipc::reply(&id, answer)).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("trek-ipc-app-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Answer every call with `answer`, from a thread standing in for the workspace.
    fn answer_with(calls: async_channel::Receiver<Call>, answer: impl Fn(&Call) -> Vec<Reply> + Send + 'static) -> std::thread::JoinHandle<Vec<(String, String, Value)>> {
        std::thread::spawn(move || {
            let mut seen = vec![];
            while let Ok(call) = calls.recv_blocking() {
                for r in answer(&call) {
                    let _ = call.reply.send_blocking(r);
                }
                seen.push((call.thread.clone(), call.method.clone(), call.params.clone()));
            }
            seen
        })
    }

    #[test]
    fn only_trek_s_own_sessions_get_in() {
        let (server, calls) = IpcServer::start(&dir()).unwrap();
        // (A pipe's DACL is checked in trek-ipc's tests.)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&server.path).unwrap().permissions().mode() & 0o777, 0o600, "only the user can connect");
        }
        let key = server.open_session(Some("thread-1"));
        let answers = answer_with(calls, |c| vec![Reply::Done(Ok(json!({ "echo": c.params.clone() })))]);

        let good = server.client(&key);
        assert_eq!(good.call("task_status", &json!({"id": "x"})).unwrap(), json!({"echo": {"id": "x"}}));
        let wrong_token = trek_ipc::Client { token: "0".repeat(64), ..good.clone() };
        assert!(wrong_token.call("list_models", &json!({})).unwrap_err().contains("wrong token"));
        let unknown = trek_ipc::Client { session: "made-up".into(), ..good.clone() };
        assert!(unknown.call("list_models", &json!({})).unwrap_err().contains("doesn't know this agent session"));

        // A warmed-up session waits to be named; then it speaks for its thread.
        let warm = server.open_session(None);
        assert!(server.client(&warm).call("list_models", &json!({})).unwrap_err().contains("isn't part of a thread yet"));
        server.bind(&warm, "thread-2");
        assert!(server.client(&warm).call("list_models", &json!({})).is_ok());
        server.close_session(&warm);
        assert!(server.client(&warm).call("list_models", &json!({})).is_err(), "an ended session is shut out");

        let path = server.path.clone();
        drop(server);
        #[cfg(unix)]
        assert!(!path.exists(), "the socket goes with the server");
        // A pipe goes once the runtime has dropped the listener's task, a moment later. (Asking
        // whether it exists would open it.)
        #[cfg(windows)]
        {
            let started = std::time::Instant::now();
            while trek_ipc::Stream::connect(&path).is_ok() {
                assert!(started.elapsed() < Duration::from_secs(5), "the pipe goes with the server");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let seen = answers.join().unwrap();
        assert_eq!(seen.iter().map(|(t, m, _)| (t.as_str(), m.as_str())).collect::<Vec<_>>(), [("thread-1", "task_status"), ("thread-2", "list_models")]);
    }

    #[test]
    fn garbage_is_answered_and_oversized_frames_end_the_connection() {
        use std::io::{BufRead as _, BufReader, Write as _};
        let (server, calls) = IpcServer::start(&dir()).unwrap();
        let key = server.open_session(Some("t"));
        let _answers = answer_with(calls, |_| vec![Reply::Done(Ok(json!("fine")))]);
        let mut s = trek_ipc::Stream::connect(&server.path).unwrap();
        s.write_all(trek_ipc::encode(&trek_ipc::hello(&server.token, &key)).as_bytes()).unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), r#"{"ok":true}"#);
        s.write_all(b"not json\n").unwrap();
        line.clear();
        r.read_line(&mut line).unwrap();
        assert!(line.contains("not JSON"), "{line}");
        s.write_all(b"{\"id\":3}\n").unwrap();
        line.clear();
        r.read_line(&mut line).unwrap();
        assert!(line.contains("no method") && line.contains("\"id\":3"), "{line}");
        // Over the limit: the connection closes rather than buffering it.
        let huge = vec![b'x'; trek_ipc::MAX_FRAME + 10];
        let _ = s.write_all(&huge);
        line.clear();
        let _ = r.read_line(&mut line);
        assert!(line.contains("Bad request"), "{line}");
        line.clear();
        assert_eq!(r.read_line(&mut line).unwrap_or(0), 0, "closed");
    }

    #[test]
    fn a_waiting_call_returns_the_answer_or_running_when_time_is_up() {
        let (server, calls) = IpcServer::start(&dir()).unwrap();
        let key = server.open_session(Some("t"));
        let held: Arc<Mutex<Vec<async_channel::Sender<Reply>>>> = Default::default();
        let keep = held.clone();
        let _answers = answer_with(calls, move |c| match c.params["case"].as_str() {
            Some("quick") => vec![Reply::Waiting("child-1".into(), WAIT_DEFAULT), Reply::Done(Ok(json!({"id": "child-1", "status": "done"})))],
            _ => {
                // Never answered: the wait runs out.
                keep.lock().unwrap().push(c.reply.clone());
                vec![Reply::Waiting("child-2".into(), wait_for(&c.params))]
            }
        });
        let client = server.client(&key);
        assert_eq!(client.call("delegate_task", &json!({"case": "quick", "wait": true})).unwrap()["status"], "done");
        let started = std::time::Instant::now();
        let out = client.call("delegate_task", &json!({"case": "slow", "wait": true, "timeout_seconds": 1})).unwrap();
        assert!(started.elapsed() >= Duration::from_secs(WAIT_MIN));
        assert_eq!(out["id"], "child-2");
        assert_eq!(out["status"], "running");
        // The connection gave up waiting: the workspace sees nobody listening any more.
        assert!(held.lock().unwrap()[0].is_closed());
    }

    #[test]
    fn a_call_given_up_stops_trek_waiting() {
        let (server, calls) = IpcServer::start(&dir()).unwrap();
        let key = server.open_session(Some("t"));
        let held: Arc<Mutex<Vec<async_channel::Sender<Reply>>>> = Default::default();
        let keep = held.clone();
        let _answers = answer_with(calls, move |c| {
            keep.lock().unwrap().push(c.reply.clone());
            vec![Reply::Waiting("child".into(), WAIT_DEFAULT)]
        });
        // As trek-mcp gives up a call its client cancelled: shut the connection from another thread.
        let mut conn = server.client(&key).connect().unwrap();
        let handle = conn.handle().unwrap();
        let call = std::thread::spawn(move || conn.call("delegate_task", &json!({"wait": true})));
        let started = std::time::Instant::now();
        let soon = |what: &str| {
            assert!(started.elapsed() < Duration::from_secs(5), "{what}");
            std::thread::sleep(Duration::from_millis(10));
        };
        while held.lock().unwrap().is_empty() {
            soon("the call reaches the workspace");
        }
        handle.shutdown().unwrap();
        assert!(call.join().unwrap().is_err());
        while !held.lock().unwrap()[0].is_closed() {
            soon("Trek stops waiting on the caller's behalf");
        }
    }

    #[test]
    fn waits_are_bounded() {
        assert_eq!(wait_for(&json!({})), WAIT_DEFAULT);
        assert_eq!(wait_for(&json!({"timeout_seconds": 1})), Duration::from_secs(WAIT_MIN));
        assert_eq!(wait_for(&json!({"timeout_seconds": 99999})), Duration::from_secs(WAIT_MAX));
    }
}
