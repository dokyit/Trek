//! Named pipes, for Windows: who may open one (`PipeSecurity`), who is at the other end
//! (`client_is_me`), and the client's blocking end of a connection (`PipeStream`).

use std::io::{self, Read, Write};
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::{AsHandle as _, AsRawHandle as _, BorrowedHandle, FromRawHandle as _, OwnedHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_NOT_CONNECTED, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAce, CopySid, EqualSid, GetLengthSid, GetTokenInformation, InitializeAcl, InitializeSecurityDescriptor, PSID, SECURITY_ATTRIBUTES,
    SECURITY_DESCRIPTOR, SetSecurityDescriptorDacl, SetSecurityDescriptorOwner, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_ALL_ACCESS, FILE_FLAG_OVERLAPPED, ReadFile, SECURITY_IDENTIFICATION, WriteFile};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};
use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentProcess, INFINITE, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, SetEvent, WaitForMultipleObjects, WaitForSingleObject};

/// `SECURITY_DESCRIPTOR_REVISION`, from a part of windows-sys not worth compiling for one number.
const SECURITY_DESCRIPTOR_REVISION: u32 = 1;

fn check(ok: windows_sys::core::BOOL) -> io::Result<()> {
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a handle the caller was just given and owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// A security identifier, copied (DWORD-aligned, as SIDs must be).
pub(crate) struct Sid(Vec<u32>);

impl Sid {
    /// SAFETY: `sid` points at a valid SID.
    unsafe fn copy(sid: PSID) -> io::Result<Sid> {
        // SAFETY: as the caller promises.
        let len = unsafe { GetLengthSid(sid) };
        let mut buf = vec![0u32; (len as usize).div_ceil(4)];
        // SAFETY: `buf` holds `len` bytes.
        check(unsafe { CopySid(len, buf.as_mut_ptr().cast(), sid) })?;
        Ok(Sid(buf))
    }

    pub(crate) fn as_ptr(&self) -> PSID {
        self.0.as_ptr().cast_mut().cast()
    }

    pub(crate) fn len(&self) -> u32 {
        // SAFETY: a SID `copy` made.
        unsafe { GetLengthSid(self.as_ptr()) }
    }
}

impl PartialEq for Sid {
    fn eq(&self, other: &Sid) -> bool {
        // SAFETY: both are SIDs `copy` made.
        unsafe { EqualSid(self.as_ptr(), other.as_ptr()) != 0 }
    }
}

/// The user a process runs as.
fn process_user(process: HANDLE) -> io::Result<Sid> {
    let mut token = null_mut();
    // SAFETY: `process` is open with at least PROCESS_QUERY_LIMITED_INFORMATION.
    check(unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) })?;
    let token = owned(token)?;
    let mut len = 0;
    // SAFETY: asks only for the size (and fails, saying it).
    unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut len) };
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    // TOKEN_USER holds a pointer: align the buffer for one.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds `len` bytes.
    check(unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, buf.as_mut_ptr().cast(), len, &mut len) })?;
    // SAFETY: GetTokenInformation filled in a TOKEN_USER, whose SID lies within `buf`.
    unsafe { Sid::copy((*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid) }
}

/// The user this process runs as.
pub(crate) fn me() -> io::Result<Sid> {
    // SAFETY: the pseudo-handle for this process; it needn't be closed.
    process_user(unsafe { GetCurrentProcess() })
}

/// Whether process `pid` runs as this process's user.
fn same_user(pid: u32) -> io::Result<bool> {
    // SAFETY: plain call; the handle is checked and then owned.
    let process = owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
    Ok(process_user(process.as_raw_handle())? == me()?)
}

/// Whether the client connected to `pipe` (the server's end) runs as this process's user: the
/// pipe's counterpart of comparing a Unix socket's peer uid with ours. Any doubt says no.
pub fn client_is_me(pipe: BorrowedHandle<'_>) -> bool {
    let mut pid = 0;
    // SAFETY: `pipe` is an open pipe handle (borrowed for the call).
    let known = unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut pid) != 0 };
    known && same_user(pid).unwrap_or(false)
}

/// Whether the server of a pipe we connected to (`pipe`, our end) runs as this process's user.
fn server_is_me(pipe: BorrowedHandle<'_>) -> bool {
    let mut pid = 0;
    // SAFETY: `pipe` is an open pipe handle (borrowed for the call).
    let known = unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) != 0 };
    known && same_user(pid).unwrap_or(false)
}

/// Security attributes for a pipe only this process's user may open: its DACL holds one entry,
/// allowing that user everything, and the user owns it. No other user or group gets in, not even
/// for reading (a pipe made without these would let Everyone read it).
pub struct PipeSecurity(Box<Parts>);

struct Parts {
    attributes: SECURITY_ATTRIBUTES,
    descriptor: SECURITY_DESCRIPTOR,
    /// The DACL, DWORD-aligned.
    _acl: Vec<u32>,
    _user: Sid,
}

// SAFETY: the raw pointers inside point into the box's own fields and buffers, which nothing
// changes once made.
unsafe impl Send for PipeSecurity {}
unsafe impl Sync for PipeSecurity {}

impl PipeSecurity {
    pub fn new() -> io::Result<PipeSecurity> {
        let user = me()?;
        let size = size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>() + user.len() as usize;
        let mut acl = vec![0u32; size.div_ceil(4)];
        let pacl: *mut ACL = acl.as_mut_ptr().cast();
        // SAFETY: `acl` holds `size` bytes, enough for the header and one entry for `user`.
        unsafe {
            check(InitializeAcl(pacl, (acl.len() * 4) as u32, ACL_REVISION))?;
            check(AddAccessAllowedAce(pacl, ACL_REVISION, FILE_ALL_ACCESS, user.as_ptr()))?;
        }
        let mut parts = Box::new(Parts {
            attributes: SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: null_mut(), bInheritHandle: 0 },
            // SAFETY: plain data, initialised just below.
            descriptor: unsafe { std::mem::zeroed() },
            _acl: acl,
            _user: user,
        });
        let descriptor: *mut SECURITY_DESCRIPTOR = &mut parts.descriptor;
        // SAFETY: the descriptor, ACL and SID live in the box (or its buffers) as long as it does.
        unsafe {
            check(InitializeSecurityDescriptor(descriptor.cast(), SECURITY_DESCRIPTOR_REVISION))?;
            check(SetSecurityDescriptorDacl(descriptor.cast(), 1, pacl, 0))?;
            check(SetSecurityDescriptorOwner(descriptor.cast(), parts._user.as_ptr(), 0))?;
        }
        parts.attributes.lpSecurityDescriptor = descriptor.cast();
        Ok(PipeSecurity(parts))
    }

    /// The `SECURITY_ATTRIBUTES`, for `CreateNamedPipeW` (or tokio's `ServerOptions`), valid as
    /// long as `self` is.
    pub fn as_ptr(&self) -> *mut std::ffi::c_void {
        (&self.0.attributes as *const SECURITY_ATTRIBUTES).cast_mut().cast()
    }
}

/// An event, manual-reset, not set.
fn event() -> io::Result<OwnedHandle> {
    // SAFETY: plain call; the handle is checked and then owned.
    owned(unsafe { CreateEventW(null(), 1, 0, null()) })
}

/// The client's end of a connection to Trek's pipe, with blocking reads and writes.
///
/// The pipe is opened for overlapped I/O, so that a read blocked on another thread can be given
/// up: `Handle::shutdown` sets an event every read and write also waits on, and once set,
/// nothing more goes in or out (reads see the end of the stream, as on a socket shut down).
pub struct PipeStream {
    pipe: Arc<OwnedHandle>,
    shut: Arc<OwnedHandle>,
    /// Signals this end's own I/O finishing (each clone has its own).
    done: OwnedHandle,
}

/// A connection as another thread holds it, to shut it down.
pub struct Handle(Arc<OwnedHandle>);

impl Handle {
    pub fn shutdown(&self) -> io::Result<()> {
        // SAFETY: the connection's own event.
        check(unsafe { SetEvent(self.0.as_raw_handle()) })
    }
}

pub(crate) fn handle_of(stream: &PipeStream) -> io::Result<Handle> {
    Ok(Handle(stream.shut.clone()))
}

impl PipeStream {
    /// Connect to the pipe named `name`, served by this user.
    pub fn connect(name: impl AsRef<Path>) -> io::Result<PipeStream> {
        let started = Instant::now();
        let file = loop {
            // Identification only: the server may learn who we are, not act as us.
            match std::fs::OpenOptions::new().read(true).write(true).custom_flags(FILE_FLAG_OVERLAPPED).security_qos_flags(SECURITY_IDENTIFICATION).open(name.as_ref()) {
                // Every instance is taken: Trek makes the next as soon as it picks one up.
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && started.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(5)),
                other => break other?,
            }
        };
        let pipe = OwnedHandle::from(file);
        // Anyone could name a pipe this: make sure it's this user's before saying the token.
        if !server_is_me(pipe.as_handle()) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the pipe belongs to another user"));
        }
        Ok(PipeStream { pipe: Arc::new(pipe), shut: Arc::new(event()?), done: event()? })
    }

    /// Another end on the same connection (to read on while this one writes).
    pub fn try_clone(&self) -> io::Result<PipeStream> {
        Ok(PipeStream { pipe: self.pipe.clone(), shut: self.shut.clone(), done: event()? })
    }

    /// Run one overlapped read or write (`op`) to its end. `None` when the connection was shut
    /// down, before or during it.
    fn io(&self, op: impl FnOnce(HANDLE, *mut OVERLAPPED) -> windows_sys::core::BOOL) -> io::Result<Option<usize>> {
        let (pipe, shut, done) = (self.pipe.as_raw_handle(), self.shut.as_raw_handle(), self.done.as_raw_handle());
        // SAFETY: our own event, polled.
        if unsafe { WaitForSingleObject(shut, 0) } == WAIT_OBJECT_0 {
            return Ok(None);
        }
        // SAFETY: plain data; zero is how an OVERLAPPED starts.
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = done;
        if op(pipe, &mut overlapped) == 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(e);
            }
        }
        let events = [done, shut];
        // SAFETY: two open events. If the shut one wins (or the wait fails), give up on the I/O.
        if unsafe { WaitForMultipleObjects(2, events.as_ptr(), 0, INFINITE) } != WAIT_OBJECT_0 {
            // SAFETY: cancels only this operation; it may have finished already.
            unsafe { CancelIoEx(pipe, &overlapped) };
        }
        let mut n = 0;
        // SAFETY: waits for the operation to end, cancelled or not, so neither `overlapped` nor
        // the caller's buffer is let go while the system may still use them.
        if unsafe { GetOverlappedResult(pipe, &overlapped, &mut n, 1) } == 0 {
            let e = io::Error::last_os_error();
            return if e.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32) { Ok(None) } else { Err(e) };
        }
        Ok(Some(n as usize))
    }
}

impl Read for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize) as u32;
        // SAFETY: `buf` holds `len` bytes, and `io` doesn't return until the read has ended.
        match self.io(|pipe, overlapped| unsafe { ReadFile(pipe, buf.as_mut_ptr(), len, null_mut(), overlapped) }) {
            Ok(Some(n)) => Ok(n),
            // Shut down, or Trek closed its end: the end of the stream, as on a socket.
            Ok(None) => Ok(0),
            Err(e) if [ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED].map(|c| Some(c as i32)).contains(&e.raw_os_error()) => Ok(0),
            Err(e) => Err(e),
        }
    }
}

impl Write for PipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len = buf.len().min(u32::MAX as usize) as u32;
        // SAFETY: `buf` holds `len` bytes, and `io` doesn't return until the write has ended.
        match self.io(|pipe, overlapped| unsafe { WriteFile(pipe, buf.as_ptr(), len, null_mut(), overlapped) })? {
            Some(n) => Ok(n),
            None => Err(io::Error::new(io::ErrorKind::BrokenPipe, "the connection was shut down")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
