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

    /// Listen on the pipe named `address` (`\\.\pipe\…`), which must not exist yet: a name
    /// someone else made first is refused rather than shared. Call within a tokio runtime.
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
pub fn peer_is_me(stream: &Stream) -> bool {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd as _;
        crate::peer_uid(stream.as_raw_fd()).ok() == Some(crate::my_uid())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        crate::client_is_me(stream.as_raw_handle())
    }
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

    fn address(tag: &str) -> PathBuf {
        let unique = format!("trek-ipc-{tag}-{}-{}", std::process::id(), &crate::token().unwrap()[..8]);
        if cfg!(windows) { format!(r"\\.\pipe\{unique}").into() } else { std::env::temp_dir().join(format!("{unique}.sock")) }
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
}
