//! Windows job objects for the language servers: the stand-in for the process group a server
//! gets on Unix (`process_group(0)`, then KILL to the group). Everything a server starts is in
//! its job, and the job ends with its last handle, so a Trek that crashes takes the servers (and
//! what they started) along. Agents get the same from `trek_agents`; these are `std` children,
//! which that one's tokio-only types don't take.

use std::collections::HashMap;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::process::Child;
use std::sync::{Mutex, PoisonError};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};

/// A job that ends what's in it when its handle closes.
struct Job(OwnedHandle);

/// The jobs of the servers running, by the pid of the child in each.
static JOBS: Mutex<Option<HashMap<u32, Job>>> = Mutex::new(None);

/// Start `command` with no console window, and in a process group of its own (so Ctrl+C aimed at
/// Trek's console doesn't reach it).
pub fn prepare(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

/// Put the just-spawned `child` in a job of its own. A child that can't be (it's in a job that
/// forbids it) runs without one, and is ended on its own.
pub fn adopt(child: &Child) {
    match make(child) {
        Ok(job) => {
            JOBS.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(HashMap::new).insert(child.id(), job);
        }
        Err(e) => tracing::warn!("couldn't put process {} in a job; what it starts may outlive it: {e}", child.id()),
    }
}

/// End everything in the job of the child `pid`, if it has one.
pub fn terminate(pid: u32) {
    let job = JOBS.lock().unwrap_or_else(PoisonError::into_inner).as_mut().and_then(|jobs| jobs.remove(&pid));
    if let Some(Job(handle)) = job {
        // SAFETY: a job handle this owns, with all access.
        unsafe { TerminateJobObject(handle.as_raw_handle(), 1) };
    }
}

fn make(child: &Child) -> std::io::Result<Job> {
    // SAFETY: no security attributes and no name: both may be null.
    let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if h.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `h` was just created, and is owned from here on.
    let job = Job(unsafe { OwnedHandle::from_raw_handle(h) });
    // SAFETY: all zeroes is a valid limit information (integers only): no limits.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let size = std::mem::size_of_val(&info) as u32;
    // SAFETY: `info` is the structure the class names, `size` long.
    if unsafe { SetInformationJobObject(h, JobObjectExtendedLimitInformation, &info as *const _ as *const _, size) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both handles are open: the job is this one's, the process the child's.
    if unsafe { AssignProcessToJobObject(h, child.as_raw_handle()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(job)
}
