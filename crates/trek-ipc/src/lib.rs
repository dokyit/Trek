//! The local channel between a running Trek and the `trek-mcp orchestrate` servers its agents
//! start, so an agent can hand work to other agents (`delegate_task` and friends).
//!
//! Trek listens on a Unix socket of its own (one per process, in a folder only its user can
//! open) and hands each agent session the socket's path, a token and a session key through the
//! environment (`ENV_*`). A connection opens with a hello carrying both; Trek checks the peer is
//! the same user, the token matches and the session is one it started, then answers requests.
//!
//! Frames are single lines of JSON, at most [`MAX_FRAME`] bytes:
//!
//! ```text
//! → {"hello":{"version":1,"token":"…","session":"…"}}
//! ← {"ok":true}                      or {"error":"…"} and the connection closes
//! → {"id":1,"method":"delegate_task","params":{…}}
//! ← {"id":1,"result":{…}}            or {"id":1,"error":"…"}
//! ```
//!
//! Prompts and results travel only over this socket: nothing here logs them.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// Where Trek listens.
pub const ENV_SOCKET: &str = "TREK_IPC_SOCKET";
/// The token of the Trek process that started the session.
pub const ENV_TOKEN: &str = "TREK_IPC_TOKEN";
/// Which session is asking (Trek maps it to the thread the session belongs to).
pub const ENV_SESSION: &str = "TREK_IPC_SESSION";

pub const VERSION: u64 = 1;

/// The longest frame either side accepts. Requests are small; results are capped well below.
pub const MAX_FRAME: usize = 1 << 20;

/// Read one frame (a line, without its newline). `Ok(None)` at the end of the stream; an error
/// for a frame longer than `max` (the rest of the stream is then unusable).
pub fn read_frame(r: &mut impl BufRead, max: usize) -> std::io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = r.take(max as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > max {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("frame longer than {max} bytes")));
    }
    String::from_utf8(buf).map(Some).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "frame isn't UTF-8"))
}

/// A frame as written: the JSON on one line, then a newline.
pub fn encode(v: &Value) -> String {
    let mut s = v.to_string();
    s.push('\n');
    s
}

pub fn hello(token: &str, session: &str) -> Value {
    json!({ "hello": { "version": VERSION, "token": token, "session": session } })
}

/// `(version, token, session)` from a hello frame.
pub fn parse_hello(v: &Value) -> Option<(u64, &str, &str)> {
    let h = v.get("hello")?;
    Some((h.get("version")?.as_u64()?, h.get("token")?.as_str()?, h.get("session")?.as_str()?))
}

pub fn request(id: u64, method: &str, params: &Value) -> Value {
    json!({ "id": id, "method": method, "params": params })
}

pub fn reply(id: &Value, result: Result<Value, String>) -> Value {
    match result {
        Ok(result) => json!({ "id": id, "result": result }),
        Err(error) => json!({ "id": id, "error": error }),
    }
}

/// A fresh random token: 32 bytes from the system's generator, as hex.
pub fn token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Compare secrets in time that doesn't depend on where they differ.
pub fn same_secret(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The user id of the process at the other end of a connected Unix socket.
pub fn peer_uid(fd: std::os::fd::RawFd) -> std::io::Result<u32> {
    let (mut uid, mut gid): (libc::uid_t, libc::gid_t) = (0, 0);
    // SAFETY: getpeereid only writes the two ids it's given; `fd` is the caller's open socket.
    if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(uid)
}

/// This process's effective user id.
pub fn my_uid() -> u32 {
    // SAFETY: no arguments, can't fail.
    unsafe { libc::geteuid() }
}

/// Where a Trek process keeps its socket: `preferred` (in its data folder) when the path fits a
/// socket address, else a folder of this user's own under /tmp. Made only its user can enter.
pub fn socket_dir(preferred: &Path, name: &str) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
    // sockaddr_un holds 104 bytes on macOS, the terminating NUL included.
    let fits = |dir: &Path| dir.join(name).as_os_str().len() < 100;
    let dir = if fits(preferred) { preferred.to_path_buf() } else { PathBuf::from(format!("/tmp/trek-{}", my_uid())) };
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    let meta = std::fs::symlink_metadata(&dir)?;
    if !meta.is_dir() || meta.uid() != my_uid() {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{} isn't a folder of this user's", dir.display())));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// What a `trek-mcp` server needs to reach the Trek that started its agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    pub socket: PathBuf,
    pub token: String,
    pub session: String,
}

impl Client {
    /// From `ENV_*`, as Trek sets them for the session.
    pub fn from_env() -> Option<Client> {
        let var = |k| std::env::var(k).ok().filter(|v| !v.is_empty());
        Some(Client { socket: var(ENV_SOCKET)?.into(), token: var(ENV_TOKEN)?, session: var(ENV_SESSION)? })
    }

    /// From an MCP server's environment as Trek hands it to a session (`(name, value)` pairs).
    pub fn from_pairs(env: &[(String, String)]) -> Option<Client> {
        let var = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        Some(Client { socket: var(ENV_SOCKET)?.into(), token: var(ENV_TOKEN)?, session: var(ENV_SESSION)? })
    }

    /// Connect and introduce ourselves. The stream can be shut down from another thread (a
    /// clone of it) to give up on a call.
    pub fn connect(&self) -> Result<Connection, String> {
        let stream = UnixStream::connect(&self.socket).map_err(|e| format!("Trek isn't reachable ({e}). Is it still running?"))?;
        let mut conn = Connection { reader: BufReader::new(stream.try_clone().map_err(|e| e.to_string())?), writer: stream, next: 0 };
        conn.send(&hello(&self.token, &self.session))?;
        let answer = conn.receive()?;
        match answer.get("error").and_then(Value::as_str) {
            Some(e) => Err(e.to_string()),
            None if answer.get("ok") == Some(&Value::Bool(true)) => Ok(conn),
            None => Err("Trek answered the hello with something unexpected".into()),
        }
    }

    /// One request on a connection of its own.
    pub fn call(&self, method: &str, params: &Value) -> Result<Value, String> {
        self.connect()?.call(method, params)
    }
}

pub struct Connection {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next: u64,
}

impl Connection {
    /// A handle to the same socket, to shut it down from elsewhere.
    pub fn handle(&self) -> std::io::Result<UnixStream> {
        self.writer.try_clone()
    }

    fn send(&mut self, v: &Value) -> Result<(), String> {
        self.writer.write_all(encode(v).as_bytes()).map_err(|e| format!("Lost the connection to Trek: {e}"))
    }

    fn receive(&mut self) -> Result<Value, String> {
        let line = read_frame(&mut self.reader, MAX_FRAME).map_err(|e| format!("Lost the connection to Trek: {e}"))?;
        let line = line.ok_or("Trek closed the connection")?;
        serde_json::from_str(&line).map_err(|e| format!("Trek sent something that isn't JSON: {e}"))
    }

    pub fn call(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        self.send(&request(id, method, params))?;
        let answer = self.receive()?;
        if answer.get("id").and_then(Value::as_u64) != Some(id) {
            return Err("Trek answered another request".into());
        }
        match (answer.get("result"), answer.get("error")) {
            (_, Some(e)) => Err(e.as_str().map(str::to_string).unwrap_or_else(|| e.to_string())),
            (Some(r), None) => Ok(r.clone()),
            (None, None) => Err("Trek's answer has no result".into()),
        }
    }
}

/// The orchestration tools: `(name, description, input schema)`, as `trek-mcp orchestrate`
/// lists them. Trek checks the arguments itself (`trek-mcp` passes them on as they are).
pub fn tools() -> Vec<(&'static str, &'static str, Value)> {
    let id = json!({ "type": "object", "properties": { "id": { "type": "string", "description": "The sub-agent's id, as delegate_task returned it." } }, "required": ["id"] });
    vec![
        (
            "list_models",
            "List the agents, models and reasoning efforts Trek can run right now, for delegate_task: every provider the user has set up in Trek, not only yours. Also says how deep this thread is among sub-agents and how many it may run at once.",
            json!({ "type": "object", "properties": {} }),
        ),
        (
            "delegate_task",
            "Start a sub-agent: a child thread in Trek that does one task with the agent, model and effort you choose (any that list_models offers, from any provider), in this thread's folder. It sees only the prompt you give it, not this conversation, so make the prompt self-contained: the goal, the files that matter, what you already know and what you want back. mode \"advise\" (the default) runs it read-only, to review, research or recommend; it can't change files. mode \"implement\" lets it change files, with this thread's access level. wait=true blocks until it finishes and returns its final answer (after timeout_seconds it returns its id with status \"running\" instead). wait=false returns its id at once; when it finishes Trek sends you its result in a message, so end your turn rather than polling, or collect it in this turn with task_result wait=true. Each call starts a new sub-agent: for another round with the same model, call delegate_task again with the brief, the findings so far and any open objections. The user sees every sub-agent inline with its model and status, and can open it.",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "A few words naming the task, shown to the user, e.g. \"Review the cache design\"." },
                    "prompt": { "type": "string", "description": "The complete task for the sub-agent." },
                    "agent": { "type": "string", "description": "Agent key from list_models, e.g. \"codex\" or \"claude-code\". Defaults to this thread's agent." },
                    "model": { "type": "string", "description": "Model id or name from list_models. Defaults to this thread's model (the agent's own default when the sub-agent is another agent)." },
                    "effort": { "type": "string", "enum": ["off", "minimal", "low", "medium", "high", "xhigh", "max"], "description": "Reasoning effort; clamped to what the model supports. Defaults to this thread's effort." },
                    "mode": { "type": "string", "enum": ["advise", "implement"], "description": "advise: read-only review or research (default). implement: may change files." },
                    "wait": { "type": "boolean", "description": "Wait for the result (true) or return at once and be woken with it (false, the default)." },
                    "timeout_seconds": { "type": "integer", "minimum": 10, "maximum": 1800, "description": "With wait=true, how long to wait before returning \"running\" (default 600; some agents' tool calls can't wait that long, and get \"running\" sooner). The sub-agent keeps going either way." }
                },
                "required": ["title", "prompt"]
            }),
        ),
        ("task_status", "How a sub-agent you started is doing (running, needs approval, done, failed or cancelled), how long it has run, and a preview of its answer once done.", id.clone()),
        (
            "task_result",
            "A sub-agent's final answer in full (very long answers are cut short). wait=true blocks until it finishes (after timeout_seconds it returns its status \"running\" instead): start several with delegate_task wait=false, then collect each with task_result wait=true, and they run side by side meanwhile. An answer read here isn't sent to you again in a message.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The sub-agent's id, as delegate_task returned it." },
                    "wait": { "type": "boolean", "description": "Wait for it to finish (true) or answer at once (false, the default)." },
                    "timeout_seconds": { "type": "integer", "minimum": 10, "maximum": 1800, "description": "With wait=true, how long to wait before returning \"running\" (default 600). The sub-agent keeps going either way." }
                },
                "required": ["id"]
            }),
        ),
        ("cancel_task", "Stop a sub-agent you started. Its thread stays, for the user to read.", id),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frames_are_lines_with_a_size_limit() {
        let mut r = Cursor::new(b"{\"a\":1}\n{\"b\":2}\nlast".to_vec());
        assert_eq!(read_frame(&mut r, 64).unwrap().as_deref(), Some("{\"a\":1}"));
        assert_eq!(read_frame(&mut r, 64).unwrap().as_deref(), Some("{\"b\":2}"));
        assert_eq!(read_frame(&mut r, 64).unwrap().as_deref(), Some("last"), "a last frame without its newline still counts");
        assert_eq!(read_frame(&mut r, 64).unwrap(), None);
        let mut long = Cursor::new(vec![b'x'; 100]);
        assert!(read_frame(&mut long, 64).is_err());
        // Exactly the limit, newline after it, is fine.
        let mut edge = Cursor::new([vec![b'y'; 64], vec![b'\n']].concat());
        assert_eq!(read_frame(&mut edge, 64).unwrap().map(|l| l.len()), Some(64));
        let mut bad = Cursor::new(vec![0xff, 0xfe, b'\n']);
        assert!(read_frame(&mut bad, 64).is_err());
        assert!(encode(&json!({"x": "a\nb"})).ends_with("}\n") && encode(&json!({"x": "a\nb"})).matches('\n').count() == 1, "newlines inside strings are escaped");
    }

    #[test]
    fn hello_round_trips() {
        let h = hello("tok", "ses");
        assert_eq!(parse_hello(&h), Some((VERSION, "tok", "ses")));
        assert_eq!(parse_hello(&json!({"hello": {"token": "t"}})), None);
        assert_eq!(parse_hello(&json!({"id": 1})), None);
    }

    #[test]
    fn tokens_are_random_and_compared_whole() {
        let (a, b) = (token().unwrap(), token().unwrap());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(same_secret(&a, &a.clone()));
        assert!(!same_secret(&a, &b));
        assert!(!same_secret(&a, &a[..63]));
        assert!(!same_secret("", "x"));
    }

    #[test]
    fn clients_come_from_the_session_environment() {
        let env = vec![(ENV_SOCKET.to_string(), "/tmp/s.sock".to_string()), (ENV_TOKEN.into(), "t".into()), (ENV_SESSION.into(), "s".into())];
        assert_eq!(Client::from_pairs(&env), Some(Client { socket: "/tmp/s.sock".into(), token: "t".into(), session: "s".into() }));
        assert_eq!(Client::from_pairs(&env[..2]), None);
    }

    #[test]
    fn tool_schemas_are_objects_with_their_required_fields() {
        let tools = tools();
        let names: Vec<&str> = tools.iter().map(|t| t.0).collect();
        assert_eq!(names, ["list_models", "delegate_task", "task_status", "task_result", "cancel_task"]);
        for (name, description, schema) in &tools {
            assert_eq!(schema["type"], "object", "{name}");
            assert!(description.len() > 40, "{name} says what it does");
            for req in schema["required"].as_array().into_iter().flatten() {
                assert!(schema["properties"].get(req.as_str().unwrap()).is_some(), "{name}: {req} is described");
            }
        }
        let delegate = &tools[1].2;
        assert_eq!(delegate["required"], json!(["title", "prompt"]));
        assert_eq!(delegate["properties"]["mode"]["enum"], json!(["advise", "implement"]));
    }

    #[test]
    fn long_socket_paths_move_to_a_short_private_folder() {
        let deep = std::env::temp_dir().join("trek-ipc-test").join("x".repeat(90));
        let dir = socket_dir(&deep, "trek-1.sock").unwrap();
        assert!(dir.join("trek-1.sock").as_os_str().len() < 100);
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let short = std::env::temp_dir().join(format!("trek-ipc-short-{}", std::process::id()));
        assert_eq!(socket_dir(&short, "t.sock").unwrap(), short);
        let _ = std::fs::remove_dir_all(short);
    }

    #[test]
    fn a_client_talks_to_a_server_over_a_socket() {
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("trek-ipc-c-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                assert_eq!(peer_uid(std::os::fd::AsRawFd::as_raw_fd(&stream)).unwrap(), my_uid());
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut w = stream;
                let h: Value = serde_json::from_str(&read_frame(&mut r, MAX_FRAME).unwrap().unwrap()).unwrap();
                let (_, token, _) = parse_hello(&h).unwrap();
                if token != "good" {
                    w.write_all(encode(&json!({"error": "bad token"})).as_bytes()).unwrap();
                    continue;
                }
                w.write_all(encode(&json!({"ok": true})).as_bytes()).unwrap();
                let req: Value = serde_json::from_str(&read_frame(&mut r, MAX_FRAME).unwrap().unwrap()).unwrap();
                let echo = req["params"].clone();
                w.write_all(encode(&reply(&req["id"], if req["method"] == "echo" { Ok(echo) } else { Err("no such method".into()) })).as_bytes()).unwrap();
            }
        });
        let bad = Client { socket: path.clone(), token: "bad".into(), session: "s".into() };
        assert_eq!(bad.call("echo", &json!({})).unwrap_err(), "bad token");
        let good = Client { socket: path.clone(), token: "good".into(), session: "s".into() };
        assert_eq!(good.call("echo", &json!({"x": 1})).unwrap(), json!({"x": 1}));
        server.join().unwrap();
        let gone = Client { socket: dir.join("nothing.sock"), token: "t".into(), session: "s".into() };
        assert!(gone.call("echo", &json!({})).unwrap_err().contains("isn't reachable"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
