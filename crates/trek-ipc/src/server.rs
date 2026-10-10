//! The listening end, for Trek itself (on tokio): a Unix socket only its user can connect to, or
//! on Windows a named pipe only its user can open (see the crate's docs).

use std::io;
use std::path::Path;

/// Trek's end of one connection.
#[cfg(unix)]
pub type Stream = tokio::net::UnixStream;
#[cfg(windows)]
pub type Stream = tokio::net::windows::named_pipe::NamedPipeServer;

pub struct Listener {
    #[cfg(unix)]
    socket: tokio::net::UnixListener,
    /// The instance waiting for the next client.
    #[cfg(windows)]
    pipe: Stream,
    #[cfg(windows)]
    name: std::ffi::OsString,
    #[cfg(windows)]
    security: crate::PipeSecurity,
}

impl Listener {
    /// Listen at `address`, a socket's path (whose folder the caller made private, see
    /// `socket_dir`). Call within a tokio runtime.
    #[cfg(unix)]
    pub fn bind(address: &Path) -> io::Result<Listener> {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::remove_file(address);
        let listener = std::os::unix::net::UnixListener::bind(address)?;
        std::fs::set_permissions(address, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok(Listener { socket: tokio::net::UnixListener::from_std(listener)? })
    }

    #[cfg(unix)]
    pub async fn accept(&mut self) -> io::Result<Stream> {
        self.socket.accept().await.map(|(stream, _)| stream)
    }

    /// Listen on the pipe named `address` (`pipe_name`), which must not exist yet: a name someone
    /// else made first is refused rather than shared. Only this user may open the pipe or add an
    /// instance to it (`PipeSecurity`); an instance another process of this user's adds is one a
    /// client refuses, as the name carries this process's id. Call within a tokio runtime.
    #[cfg(windows)]
    pub fn bind(address: &Path) -> io::Result<Listener> {
        let security = crate::PipeSecurity::new()?;
        let pipe = instance(address.as_os_str(), &security, true)?;
        Ok(Listener { pipe, name: address.as_os_str().to_owned(), security })
    }

    #[cfg(windows)]
    pub async fn accept(&mut self) -> io::Result<Stream> {
        let connected = self.pipe.connect().await;
        // An instance serves one client: leave the next one waiting before handing this one out.
        let next = instance(&self.name, &self.security, false)?;
        let this = std::mem::replace(&mut self.pipe, next);
        connected.map(|()| this)
    }
}

#[cfg(windows)]
fn instance(name: &std::ffi::OsStr, security: &crate::PipeSecurity, first: bool) -> io::Result<Stream> {
    use tokio::net::windows::named_pipe::ServerOptions;
    // SAFETY: `security` holds valid SECURITY_ATTRIBUTES, and outlives the call.
    unsafe { ServerOptions::new().first_pipe_instance(first).reject_remote_clients(true).create_with_security_attributes_raw(name, security.as_ptr()) }
}

/// Whether the process at the other end of a connection Trek accepted runs as Trek's own user.
/// One that doesn't is logged (who it was, never what it sent).
pub fn peer_is_me(stream: &Stream) -> bool {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd as _;
        let uid = crate::peer_uid(stream.as_raw_fd());
        let me = uid.as_ref().ok() == Some(&crate::my_uid());
        if !me {
            tracing::warn!("refused an IPC connection from another user (uid {uid:?})");
        }
        me
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle as _;
        let me = crate::client_is_me(stream.as_handle());
        if !me {
            tracing::warn!("refused an IPC connection from another user, or a process Trek can't look at (pid {:?})", crate::windows::client_pid(stream.as_handle()));
        }
        me
    }
}

/// Read one frame, at most `max` bytes. `Ok(None)` at the end of the stream; an error for a
/// longer one (the rest of the stream is then unusable).
pub async fn read_frame<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R, max: usize) -> io::Result<Option<String>> {
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _};
    let mut buf = Vec::new();
    let n = (&mut *r).take(max as u64 + 1).read_until(b'\n', &mut buf).await?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame longer than {max} bytes")));
    }
    String::from_utf8(buf).map(Some).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame isn't UTF-8"))
}

/// One connection from a second launch (`instance`): an open request within `within`, at most
/// `MAX_FRAME` bytes, which `dispatch` takes; its answer goes back. Another user's process gets
/// nothing; one that sends too much, too late or not an open request gets the connection closed
/// (with a word why, when it sent something). Nothing it sent is logged.
pub async fn serve_open<F, Fut>(stream: Stream, within: std::time::Duration, dispatch: F)
where
    F: FnOnce(crate::instance::Open) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    use serde_json::json;
    use tokio::io::AsyncWriteExt as _;
    if !peer_is_me(&stream) {
        return;
    }
    let (rd, mut w) = tokio::io::split(stream);
    let mut r = tokio::io::BufReader::new(rd);
    let open = match tokio::time::timeout(within, read_frame(&mut r, crate::MAX_FRAME)).await {
        Ok(Ok(Some(line))) => serde_json::from_str(&line).ok().as_ref().and_then(crate::instance::Open::parse),
        Ok(Err(e)) => {
            tracing::warn!("refused a second launch's request: {e}");
            return;
        }
        // Nothing in time, or nothing at all.
        _ => return,
    };
    let answer = match open {
        Some(open) => match dispatch(open).await {
            Ok(()) => json!({ "ok": true }),
            Err(e) => json!({ "error": e }),
        },
        None => json!({ "error": "not an open request Trek understands" }),
    };
    let _ = w.write_all(crate::encode(&answer).as_bytes()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Client, encode, parse_hello, reply};
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
    }

    #[cfg(unix)]
    fn address(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("trek-ipc-{tag}-{}-{}.sock", std::process::id(), &crate::token().unwrap()[..8]))
    }

    /// A pipe's name as Trek's are (a client refuses others); the random part keeps it unique.
    #[cfg(windows)]
    fn address(_tag: &str) -> PathBuf {
        crate::pipe_name(0).unwrap()
    }

    /// Answers `echo`, refuses any token but "good", and never answers `hang`: it reports how
    /// that connection ended instead (the bytes it read after the request: 0 when the client went).
    async fn serve(stream: Stream, ended: std::sync::mpsc::Sender<usize>) {
        assert!(peer_is_me(&stream), "the test talks to itself");
        let (rd, mut w) = tokio::io::split(stream);
        let mut r = tokio::io::BufReader::new(rd);
        let mut line = String::new();
        let _ = r.read_line(&mut line).await;
        let Ok(hello) = serde_json::from_str::<Value>(&line) else { return };
        if parse_hello(&hello).map(|h| h.1) != Some("good") {
            let _ = w.write_all(encode(&json!({"error": "bad token"})).as_bytes()).await;
            return;
        }
        let _ = w.write_all(encode(&json!({"ok": true})).as_bytes()).await;
        line.clear();
        let _ = r.read_line(&mut line).await;
        let Ok(req) = serde_json::from_str::<Value>(&line) else { return };
        if req["method"] == "hang" {
            line.clear();
            let _ = ended.send(r.read_line(&mut line).await.unwrap_or(0));
            return;
        }
        let _ = w.write_all(encode(&reply(&req["id"], Ok(req["params"].clone()))).as_bytes()).await;
    }

    #[test]
    fn a_listener_serves_its_user_and_a_shut_down_call_gives_up() {
        let rt = runtime();
        let address = address("serve");
        let mut listener = {
            let _enter = rt.enter();
            Listener::bind(&address).unwrap()
        };
        let (ended_tx, ended) = std::sync::mpsc::channel();
        rt.spawn(async move {
            loop {
                if let Ok(stream) = listener.accept().await {
                    tokio::spawn(serve(stream, ended_tx.clone()));
                }
            }
        });

        let bad = Client { socket: address.clone(), token: "bad".into(), session: "s".into() };
        assert_eq!(bad.call("echo", &json!({})).unwrap_err(), "bad token");
        let good = Client { token: "good".into(), ..bad.clone() };
        // Several at once, so some find every instance of a pipe busy and wait their turn.
        let calls: Vec<_> = (0..8).map(|i| (i, good.clone())).map(|(i, c)| std::thread::spawn(move || c.call("echo", &json!({ "i": i })))).collect();
        for (i, call) in calls.into_iter().enumerate() {
            assert_eq!(call.join().unwrap().unwrap(), json!({ "i": i }));
        }

        // A call given up from another thread (trek-mcp's cancel) ends at once, and the server
        // sees its client go.
        let mut conn = good.connect().unwrap();
        let handle = conn.handle().unwrap();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            handle.shutdown().unwrap();
        });
        let started = Instant::now();
        assert_eq!(conn.call("hang", &json!({})).unwrap_err(), "Trek closed the connection");
        assert!(started.elapsed() < Duration::from_secs(5));
        canceller.join().unwrap();
        drop(conn);
        assert_eq!(ended.recv_timeout(Duration::from_secs(5)).unwrap(), 0);

        // Shut down before the call: it fails rather than waits.
        let mut conn = good.connect().unwrap();
        conn.handle().unwrap().shutdown().unwrap();
        assert!(conn.call("echo", &json!({})).is_err());

        drop(rt);
        #[cfg(unix)]
        let _ = std::fs::remove_file(&address);
        assert!(good.call("echo", &json!({})).unwrap_err().contains("isn't reachable"), "gone with its listener");
    }

    #[test]
    fn a_client_takes_a_frame_up_to_the_limit_and_refuses_a_longer_one() {
        let rt = runtime();
        let address = address("frames");
        let mut listener = {
            let _enter = rt.enter();
            Listener::bind(&address).unwrap()
        };
        // Answers the hello, then a first request with a result exactly MAX_FRAME long (a JSON
        // string of `x`s), a second with one a byte longer.
        rt.spawn(async move {
            let stream = listener.accept().await.unwrap();
            let (rd, mut w) = tokio::io::split(stream);
            let mut r = tokio::io::BufReader::new(rd);
            let mut line = String::new();
            let _ = r.read_line(&mut line).await;
            let _ = w.write_all(encode(&json!({"ok": true})).as_bytes()).await;
            for extra in [0, 1] {
                line.clear();
                let _ = r.read_line(&mut line).await;
                let req: Value = serde_json::from_str(&line).unwrap();
                let frame = encode(&reply(&req["id"], Ok(json!(""))));
                let fill = crate::MAX_FRAME + extra - (frame.len() - 1);
                let frame = encode(&reply(&req["id"], Ok(json!("x".repeat(fill)))));
                assert_eq!(frame.len() - 1, crate::MAX_FRAME + extra);
                let _ = w.write_all(frame.as_bytes()).await;
            }
        });
        let mut conn = Client { socket: address, token: "t".into(), session: "s".into() }.connect().unwrap();
        assert_eq!(conn.call("big", &json!({})).unwrap().as_str().unwrap().len(), crate::MAX_FRAME - r#"{"id":1,"result":""}"#.len());
        let err = conn.call("bigger", &json!({})).unwrap_err();
        assert!(err.contains(&format!("frame longer than {} bytes", crate::MAX_FRAME)), "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn only_this_user_may_open_the_pipe_and_no_one_can_take_its_name() {
        use std::os::windows::io::AsRawHandle as _;
        use std::ptr::null_mut;
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
        use windows_sys::Win32::Security::{ACCESS_ALLOWED_ACE, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, OWNER_SECURITY_INFORMATION};
        use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
        const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

        let rt = runtime();
        let address = address("dacl");
        let _enter = rt.enter();
        let listener = Listener::bind(&address).unwrap();
        let me = crate::windows::me().unwrap();
        let (mut owner, mut dacl, mut descriptor) = (null_mut(), null_mut(), null_mut());
        // SAFETY: the listener's open pipe; the descriptor comes back for us to free.
        let err = unsafe { GetSecurityInfo(listener.pipe.as_raw_handle(), SE_KERNEL_OBJECT, DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION, &mut owner, null_mut(), &mut dacl, null_mut(), &mut descriptor) };
        assert_eq!(err, 0);
        assert!(!dacl.is_null(), "a DACL, not none (which would let anyone in)");
        // SAFETY: `dacl` and `owner` point into `descriptor`, freed only at the end.
        unsafe {
            let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
            assert_ne!(GetAclInformation(dacl, (&mut info as *mut ACL_SIZE_INFORMATION).cast(), size_of::<ACL_SIZE_INFORMATION>() as u32, AclSizeInformation), 0);
            assert_eq!(info.AceCount, 1, "one entry, and nobody else's");
            let mut ace = null_mut();
            assert_ne!(GetAce(dacl, 0, &mut ace), 0);
            let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
            assert_eq!(ace.Header.AceType, ACCESS_ALLOWED_ACE_TYPE);
            assert_eq!(ace.Mask, FILE_ALL_ACCESS);
            assert_ne!(EqualSid((&ace.SidStart as *const u32).cast_mut().cast(), me.as_ptr()), 0, "the entry is for this user");
            assert_ne!(EqualSid(owner, me.as_ptr()), 0, "this user owns it");
            LocalFree(descriptor);
        }
        assert!(Listener::bind(&address).is_err(), "a name in use isn't shared");
    }

    /// Another process of this user's adds an instance of Trek's pipe while Trek has none waiting
    /// (between a client taking one and `accept` making the next): the next client lands in it,
    /// and must refuse it rather than say the token. This test plays both: the pipe is named for
    /// another process (as Trek's would be), so the instance the test adds is a stranger's.
    #[cfg(windows)]
    #[test]
    fn a_client_refuses_an_instance_another_process_added() {
        use std::os::windows::io::{FromRawHandle as _, OwnedHandle};
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX};
        use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES};

        let rt = runtime();
        let trek = std::process::id() + 4;
        let address = PathBuf::from(crate::windows::name_for(trek, 0, &crate::token().unwrap()[..16]));
        let _enter = rt.enter();
        let _listener = Listener::bind(&address).unwrap();
        // A client takes the one instance waiting; `accept` hasn't run to make the next.
        let _first = std::fs::OpenOptions::new().read(true).write(true).open(&address).unwrap();
        // An instance serves one client, so each attempt below gets one of its own.
        let wide: Vec<u16> = address.as_os_str().to_str().unwrap().encode_utf16().chain([0]).collect();
        let add_instance = || {
            // SAFETY: a NUL-terminated name; no security attributes (an added instance has the pipe's).
            let h = unsafe { CreateNamedPipeW(wide.as_ptr(), PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED, PIPE_TYPE_BYTE | PIPE_REJECT_REMOTE_CLIENTS, PIPE_UNLIMITED_INSTANCES, 4096, 4096, 0, std::ptr::null()) };
            assert_ne!(h, INVALID_HANDLE_VALUE, "this user may add an instance: {}", io::Error::last_os_error());
            // SAFETY: just created, and owned from here on.
            unsafe { OwnedHandle::from_raw_handle(h) }
        };

        let _rogue = add_instance();
        let err = crate::Stream::connect(&address).err().expect("a stranger's instance is refused");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        assert_eq!(err.to_string(), format!("the pipe isn't served by Trek (pid {trek}) but by process {}", std::process::id()));
        let _rogue = add_instance();
        let client = Client { socket: address, token: "secret".into(), session: "s".into() };
        let err = client.connect().err().unwrap();
        assert!(err.starts_with("Didn't connect to Trek: the pipe isn't served by Trek"), "a refusal, not Trek gone: {err}");
    }

    use crate::instance::{ForwardError, Open, forward};
    use std::sync::{Arc, Mutex};

    /// Serves open requests as a running Trek does, with a fake dispatcher that keeps what it's
    /// handed and answers as `answer` says (`None`: never, as a hung Trek).
    fn open_server(rt: &tokio::runtime::Runtime, within: Duration, answer: fn(&Open) -> Option<Result<(), String>>) -> (PathBuf, Arc<Mutex<Vec<Open>>>) {
        let address = address("open");
        let mut listener = {
            let _enter = rt.enter();
            Listener::bind(&address).unwrap()
        };
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = seen.clone();
        rt.spawn(async move {
            loop {
                let Ok(stream) = listener.accept().await else { continue };
                let kept = kept.clone();
                tokio::spawn(serve_open(stream, within, move |open: Open| async move {
                    kept.lock().unwrap().push(open.clone());
                    match answer(&open) {
                        Some(result) => result,
                        None => std::future::pending().await,
                    }
                }));
            }
        });
        (address, seen)
    }

    /// Send `bytes` as they are, and read what comes back: `None` when the server closed the
    /// connection without a word.
    fn raw(address: &Path, bytes: &[u8]) -> Option<Value> {
        use std::io::Write as _;
        let mut stream = crate::Stream::connect(address).unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        // A server that gives up half way through a long frame breaks the pipe under the write.
        let _ = stream.write_all(bytes);
        crate::read_frame(&mut reader, crate::MAX_FRAME).ok().flatten().map(|line| serde_json::from_str(&line).unwrap())
    }

    #[test]
    fn a_second_launch_hands_its_arguments_to_the_running_one() {
        let rt = runtime();
        let (address, seen) = open_server(&rt, Duration::from_secs(5), |open| Some(if open.args.iter().any(|a| a == "refuse") { Err("busy".into()) } else { Ok(()) }));
        let open = Open { args: vec!["trek://edit?path=%2Ftmp%2Fx.rs&line=3".into(), "/tmp/folder with spaces".into()], background: false };
        forward(&address, &open, Duration::from_secs(5)).unwrap();
        assert_eq!(*seen.lock().unwrap(), [open], "handed over as sent");
        assert_eq!(forward(&address, &Open { args: vec!["refuse".into()], background: true }, Duration::from_secs(5)), Err(ForwardError::Refused("busy".into())));
        forward(&address, &Open::default(), Duration::from_secs(5)).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[test]
    fn junk_and_oversized_requests_reach_no_one() {
        let rt = runtime();
        let (address, seen) = open_server(&rt, Duration::from_secs(5), |_| Some(Ok(())));
        for junk in [&b"not json\n"[..], b"{\"hello\":{\"version\":1,\"token\":\"t\",\"session\":\"s\"}}\n", b"{\"open\":{\"version\":1,\"args\":[1]}}\n"] {
            let answer = raw(&address, junk).expect("a word why");
            assert!(answer["error"].is_string(), "{answer}");
        }
        // A frame a byte over the limit: the server stops reading there and hangs up.
        assert_eq!(raw(&address, &vec![b'x'; crate::MAX_FRAME + 1]), None);
        // So do too many arguments, though the frame itself is small.
        let many = Open { args: vec!["x".into(); crate::instance::MAX_ARGS + 1], background: false };
        assert!(raw(&address, crate::encode(&many.to_frame()).as_bytes()).unwrap()["error"].is_string());
        assert!(seen.lock().unwrap().is_empty(), "the dispatcher saw none of it");
        // And it still serves a proper request after all that.
        forward(&address, &Open { args: vec!["trek://ask?path=%2Fa".into()], background: false }, Duration::from_secs(5)).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_client_that_says_nothing_is_dropped() {
        let rt = runtime();
        let (address, _) = open_server(&rt, Duration::from_millis(300), |_| Some(Ok(())));
        let started = Instant::now();
        assert_eq!(raw(&address, b""), None);
        assert!(started.elapsed() < Duration::from_secs(3), "dropped after the time it's given, not left open");
    }

    #[test]
    fn a_hung_trek_times_the_second_launch_out() {
        let rt = runtime();
        let (address, seen) = open_server(&rt, Duration::from_secs(5), |_| None);
        let started = Instant::now();
        assert_eq!(forward(&address, &Open { args: vec!["trek://edit?path=%2Fx".into()], background: false }, Duration::from_millis(500)), Err(ForwardError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        assert_eq!(seen.lock().unwrap().len(), 1, "it got there; the answer never came");
    }
}
