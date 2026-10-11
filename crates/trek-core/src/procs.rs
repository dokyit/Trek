//! Every process group Trek starts (agent CLIs, language servers, status probes), in one place:
//! signalled when Trek quits by any path, and recorded in the data folder so the next launch can
//! end the ones a crash left running.
//!
//! On Windows a "group" is a job object (`trek_agents::spawn_group`) whose id here is its first
//! process's pid. The job is made to end everything in it when its last handle closes, so a Trek
//! that crashes takes its agents with it and the records rarely find anything; what's here ends
//! a process and the tree under it by walking parent ids, as the job handles are the agents'.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// A live group: its id (the leader's pid) and when the leader started (unix seconds), so a
/// later launch can tell it from an unrelated process that got the same id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Group {
    id: i32,
    started: i64,
}

static LIVE: Mutex<Vec<Group>> = Mutex::new(Vec::new());

/// How old a record may be for its groups to still be ended at launch: past this, ids may have
/// been handed out again.
const STALE_AFTER_SECS: i64 = 3 * 24 * 3600;

/// Track the process group `group` (a child started with `process_group(0)`, or on Windows put in
/// a job object of its own; its pid).
pub fn register(group: i32) {
    if group <= 0 {
        return;
    }
    let started = start_time(group).unwrap_or(0);
    let mut live = LIVE.lock().unwrap_or_else(PoisonError::into_inner);
    if !live.iter().any(|g| g.id == group) {
        live.push(Group { id: group, started });
    }
    write_record(&live);
}

/// Stop tracking `group`: it has been ended, or its leader was reaped.
pub fn unregister(group: i32) {
    let mut live = LIVE.lock().unwrap_or_else(PoisonError::into_inner);
    let before = live.len();
    live.retain(|g| g.id != group);
    if live.len() != before {
        write_record(&live);
    }
}

/// The groups being tracked.
pub fn live() -> Vec<i32> {
    LIVE.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|g| g.id).collect()
}

/// End every tracked group: TERM, up to `grace` for them to go, then KILL whatever is left.
/// Blocks the caller for at most `grace`. For quitting.
#[cfg(not(windows))]
pub fn end_all(grace: Duration) {
    let groups = live();
    if groups.is_empty() {
        return;
    }
    tracing::info!("ending {} child process group(s)", groups.len());
    for g in &groups {
        signal(*g, Signal::Term);
    }
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline && groups.iter().any(|g| group_exists(*g)) {
        std::thread::sleep(Duration::from_millis(10));
    }
    for g in &groups {
        signal(*g, Signal::Kill);
    }
    let mut live = LIVE.lock().unwrap_or_else(PoisonError::into_inner);
    live.retain(|g| !groups.contains(&g.id));
    write_record(&live);
}

/// End every tracked group: up to `grace` for the first processes to go, then each that still
/// runs with the start time recorded is ended, and everything under it. Blocks the caller for at
/// most `grace` (and the ending). For quitting.
///
/// Windows has no TERM to send from here: the gentle stop, closing stdin, belongs to whoever holds
/// the child, and Trek has just asked its sessions to stop that way, so `grace` is theirs to do it
/// in. What can't be found from a first process (one nobody holds open any more, or one whose
/// parent exited) is left to its job, which ends it as Trek exits.
#[cfg(windows)]
pub fn end_all(grace: Duration) {
    let groups = LIVE.lock().unwrap_or_else(PoisonError::into_inner).clone();
    if groups.is_empty() {
        return;
    }
    tracing::info!("ending {} child process group(s)", groups.len());
    // Held open while waiting, so that no id goes to another process meanwhile.
    let roots: Vec<win::Proc> = groups.iter().filter_map(|g| win::Proc::open(g.id).filter(|p| win::unix_secs(p.created) == g.started)).collect();
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline && roots.iter().any(win::Proc::running) {
        std::thread::sleep(Duration::from_millis(10));
    }
    for g in &groups {
        win::end_tree(*g);
    }
    drop(roots);
    let mut live = LIVE.lock().unwrap_or_else(PoisonError::into_inner);
    live.retain(|l| !groups.iter().any(|g| g.id == l.id));
    write_record(&live);
}

/// End tracked group `group` now: its first process and everything under it, found by parent
/// ids, for a group with no job to end it by (putting the child in one failed). The first process
/// must be one `register` dated, still about (the caller holds it open, so its id is still its
/// own), running or not. True when all of it could be ended. It stays tracked: `unregister` it.
#[cfg(windows)]
pub fn end_tree(group: i32) -> bool {
    let Some(g) = LIVE.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|g| g.id == group).copied() else { return false };
    win::end_tree(g)
}

/// End the groups an earlier Trek recorded and didn't end (it crashed, or was killed): those of
/// records whose Trek is gone. Returns how many groups were signalled. Run once at launch.
pub fn reap_stale() -> usize {
    reap_stale_in(&records_dir())
}

fn reap_stale_in(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let me = std::process::id() as i32;
    let now = chrono::Utc::now().timestamp();
    let mut ended = 0;
    for e in entries.flatten() {
        let path = e.path();
        let Some(record) = std::fs::read_to_string(&path).ok().and_then(|t| Record::parse(&t)) else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        if record.owner == me {
            continue;
        }
        // That Trek still runs: its groups are its own.
        if record.owner > 0 && alive_since(record.owner, record.owner_started) {
            continue;
        }
        if same_boot(record.boot, boot_time()) && now - record.written < STALE_AFTER_SECS {
            for g in &record.groups {
                if end_stale(*g) {
                    ended += 1;
                }
            }
        }
        let _ = std::fs::remove_file(&path);
    }
    if ended > 0 {
        tracing::info!("ended {ended} process group(s) an earlier run left behind");
    }
    ended
}

/// End a group an earlier run recorded, when it can be told apart from an unrelated one: its
/// leader still runs with the start time recorded, or (the leader gone) its members started
/// after the leader did.
#[cfg(not(windows))]
fn end_stale(g: Group) -> bool {
    if !group_exists(g.id) {
        return false;
    }
    let leader_matches = start_time(g.id).is_some_and(|s| s == g.started && g.started > 0);
    let leader_gone = start_time(g.id).is_none();
    let members_younger = leader_gone && g.started > 0 && members(g.id).iter().all(|pid| start_time(*pid).is_some_and(|s| s >= g.started));
    if !(leader_matches || members_younger) {
        return false;
    }
    signal(g.id, Signal::Term);
    let deadline = std::time::Instant::now() + Duration::from_millis(300);
    while std::time::Instant::now() < deadline && group_exists(g.id) {
        std::thread::sleep(Duration::from_millis(10));
    }
    signal(g.id, Signal::Kill);
    true
}

/// End a group an earlier run recorded when its first process still runs with the start time
/// recorded, and everything under it. With that process gone there's nothing to tell the tree from
/// an unrelated one by (Windows hands ids out again quickly), and nothing to do: its job ended it
/// when the Trek that held the job went.
#[cfg(windows)]
fn end_stale(g: Group) -> bool {
    win::end_tree(g)
}

/// The boot recorded and this one are the same. Windows gives the boot time only as now less the
/// time since, which wanders a little between readings (the clock's tick, its corrections), so
/// a few seconds apart is the same boot: no machine reboots and runs Trek again that quickly.
#[cfg(windows)]
fn same_boot(recorded: i64, now: i64) -> bool {
    (recorded - now).abs() <= 10
}

#[cfg(not(windows))]
fn same_boot(recorded: i64, now: i64) -> bool {
    recorded == now
}

/// What a Trek wrote about its live groups.
#[derive(Debug, PartialEq)]
struct Record {
    owner: i32,
    owner_started: i64,
    boot: i64,
    written: i64,
    groups: Vec<Group>,
}

impl Record {
    fn render(&self) -> String {
        let mut out = format!("owner {} {}\nboot {}\nwritten {}\n", self.owner, self.owner_started, self.boot, self.written);
        for g in &self.groups {
            out.push_str(&format!("group {} {}\n", g.id, g.started));
        }
        out
    }

    fn parse(text: &str) -> Option<Record> {
        let mut r = Record { owner: 0, owner_started: 0, boot: 0, written: 0, groups: vec![] };
        for line in text.lines() {
            let mut f = line.split_whitespace();
            match (f.next(), f.next().and_then(|v| v.parse::<i64>().ok()), f.next().and_then(|v| v.parse::<i64>().ok())) {
                (Some("owner"), Some(pid), Some(started)) => {
                    r.owner = i32::try_from(pid).ok()?;
                    r.owner_started = started;
                }
                (Some("boot"), Some(b), _) => r.boot = b,
                (Some("written"), Some(w), _) => r.written = w,
                (Some("group"), Some(id), Some(started)) => {
                    let id = i32::try_from(id).ok().filter(|id| *id > 1)?;
                    r.groups.push(Group { id, started });
                }
                _ => {}
            }
        }
        (r.owner > 0).then_some(r)
    }
}

fn records_dir() -> PathBuf {
    crate::paths::data_dir().join("processes")
}

fn record_path() -> PathBuf {
    records_dir().join(format!("{}.txt", std::process::id()))
}

/// Keep this process's record up to date; with nothing live, there's no record.
fn write_record(live: &[Group]) {
    let path = record_path();
    if live.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let me = std::process::id() as i32;
    let record = Record { owner: me, owner_started: start_time(me).unwrap_or(0), boot: boot_time(), written: chrono::Utc::now().timestamp(), groups: live.to_vec() };
    let _ = std::fs::create_dir_all(records_dir());
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, record.render()).and_then(|_| std::fs::rename(&tmp, &path)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(not(windows))]
enum Signal {
    Term,
    Kill,
}

#[cfg(target_os = "macos")]
fn signal(group: i32, s: Signal) {
    if group > 1 {
        let s = match s {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        // SAFETY: a negative pid asks kill(2) to signal that process group.
        unsafe { libc::kill(-group, s) };
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn signal(_group: i32, _s: Signal) {}

/// Some process is still in `group`.
#[cfg(target_os = "macos")]
fn group_exists(group: i32) -> bool {
    // SAFETY: signal 0 only checks; nothing is sent.
    group > 1 && (unsafe { libc::kill(-group, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn group_exists(_group: i32) -> bool {
    false
}

/// The processes `pid` started that still run (its direct children; ask again of each for the
/// tree under it). Empty when `pid` is gone or the system won't list them.
#[cfg(not(windows))]
pub fn children(pid: u32) -> Vec<u32> {
    let out = std::process::Command::new("/usr/bin/pgrep").args(["-P", &pid.to_string()]).stderr(std::process::Stdio::null()).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().parse::<u32>().ok()).collect()).unwrap_or_default()
}

/// The processes `pid` started that still run (its direct children; ask again of each for the
/// tree under it). A process counts when it was created no earlier than `pid` was: a child whose
/// parent id belonged to an earlier process isn't `pid`'s. Empty when `pid` is gone or the system
/// won't list them.
#[cfg(windows)]
pub fn children(pid: u32) -> Vec<u32> {
    win::children(pid)
}

/// When process `pid` started (unix seconds), if it runs.
#[cfg(target_os = "macos")]
fn start_time(pid: i32) -> Option<i64> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: `info` is a properly sized, writable proc_bsdinfo.
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
    (n == size).then_some(info.pbi_start_tvsec as i64)
}

#[cfg(windows)]
fn start_time(pid: i32) -> Option<i64> {
    win::Proc::open(pid).map(|p| win::unix_secs(p.created))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn start_time(_pid: i32) -> Option<i64> {
    None
}

/// `pid` runs and started at `started` (unix seconds): it's the same process, not a reused id.
fn alive_since(pid: i32, started: i64) -> bool {
    start_time(pid).is_some_and(|s| started == 0 || s == started)
}

/// The processes in `group`.
#[cfg(target_os = "macos")]
fn members(group: i32) -> Vec<i32> {
    const PROC_PGRP_ONLY: u32 = 2;
    let mut pids = vec![0i32; 512];
    let bytes = (pids.len() * std::mem::size_of::<i32>()) as i32;
    // SAFETY: the buffer is `bytes` long and writable.
    let n = unsafe { libc::proc_listpids(PROC_PGRP_ONLY, group as u32, pids.as_mut_ptr() as *mut libc::c_void, bytes) };
    if n <= 0 {
        return vec![];
    }
    pids.truncate(n as usize / std::mem::size_of::<i32>());
    pids.retain(|p| *p > 0);
    pids
}

#[cfg(not(any(target_os = "macos", windows)))]
fn members(_group: i32) -> Vec<i32> {
    vec![]
}

/// When the Mac booted (unix seconds): ids from before a reboot mean nothing.
#[cfg(target_os = "macos")]
fn boot_time() -> i64 {
    let mut tv: libc::timeval = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::timeval>();
    let mut mib = [libc::CTL_KERN, libc::KERN_BOOTTIME];
    // SAFETY: `tv` and `size` describe a writable timeval.
    let r = unsafe { libc::sysctl(mib.as_mut_ptr(), 2, &mut tv as *mut _ as *mut libc::c_void, &mut size, std::ptr::null_mut(), 0) };
    if r == 0 { tv.tv_sec } else { 0 }
}

/// When Windows booted (unix seconds): now less the time since boot, so it can come out a second
/// apart between readings (see `same_boot`). Ids from before a reboot mean nothing.
#[cfg(windows)]
fn boot_time() -> i64 {
    // SAFETY: takes nothing; reads a counter.
    let up = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (now.as_millis() as i64 - up as i64) / 1000
}

#[cfg(not(any(target_os = "macos", windows)))]
fn boot_time() -> i64 {
    0
}

/// Windows processes, told apart by when they were created: Windows hands a process's id out
/// again soon after it exits, and a child keeps its parent's id after the parent has gone.
#[cfg(windows)]
mod win {
    use super::Group;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows_sys::Win32::Foundation::{FILETIME, INVALID_HANDLE_VALUE, STILL_ACTIVE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    };

    /// FILETIME ticks (100 ns since 1601) at the unix epoch, and in a second.
    const UNIX_EPOCH: u64 = 116_444_736_000_000_000;
    const TICKS_PER_SEC: u64 = 10_000_000;

    pub(super) fn unix_secs(ticks: u64) -> i64 {
        (ticks.saturating_sub(UNIX_EPOCH) / TICKS_PER_SEC) as i64
    }

    /// A process, held open: while it is, its id can't go to another process.
    pub(super) struct Proc {
        pub(super) pid: u32,
        /// When it was created, in FILETIME ticks.
        pub(super) created: u64,
        /// Opened with the right to end it.
        can_end: bool,
        handle: OwnedHandle,
    }

    impl Proc {
        /// Process `pid`, if it runs and Trek may look at it.
        pub(super) fn open(pid: i32) -> Option<Proc> {
            Proc::open_any(pid).filter(Proc::running)
        }

        /// Process `pid` whether it runs or has exited, as long as it's still about (someone holds
        /// it open) and Trek may look at it.
        fn open_any(pid: i32) -> Option<Proc> {
            // 0 and 4 are the idle and system processes.
            let pid = u32::try_from(pid).ok().filter(|p| *p > 4)?;
            let query = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE;
            // One Trek can't end (elevated, say) can still be looked at; one it can't wait on, too.
            let (h, can_end) = [(query | PROCESS_TERMINATE, true), (query, false), (PROCESS_QUERY_LIMITED_INFORMATION, false)]
                .into_iter()
                // SAFETY: plain calls; a null handle is passed over.
                .map(|(access, can_end)| (unsafe { OpenProcess(access, 0, pid) }, can_end))
                .find(|(h, _)| !h.is_null())?;
            // SAFETY: `h` was just opened, and is owned from here on.
            let mut p = Proc { pid, created: 0, can_end, handle: unsafe { OwnedHandle::from_raw_handle(h) } };
            let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
            // SAFETY: four writable FILETIMEs, and a handle with query rights.
            if unsafe { GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
                return None;
            }
            p.created = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
            Some(p)
        }

        /// It hasn't exited. (A process that has stays around while a handle to it is open.)
        pub(super) fn running(&self) -> bool {
            // SAFETY: a process handle this owns; a wait of 0 only asks.
            match unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) } {
                WAIT_TIMEOUT => true,
                WAIT_OBJECT_0 => false,
                // Opened without SYNCHRONIZE: its exit code says, unless it exited with 259.
                _ => {
                    let mut code = 0u32;
                    // SAFETY: a handle with query rights, and a writable u32.
                    let ok = unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &mut code) } != 0;
                    ok && code == STILL_ACTIVE as u32
                }
            }
        }

        /// End it. Fails harmlessly without the right to, or once it's exited.
        fn terminate(&self) {
            if self.can_end {
                // SAFETY: a handle this owns, with PROCESS_TERMINATE.
                unsafe { TerminateProcess(self.handle.as_raw_handle(), 1) };
            }
        }
    }

    /// Every process: its id and its parent's. `None` when the system won't say.
    fn snapshot() -> Option<Vec<(u32, u32)>> {
        // SAFETY: plain call; the handle is checked, then owned.
        let h = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let snap = unsafe { OwnedHandle::from_raw_handle(h) };
        // SAFETY: all zeroes is a valid PROCESSENTRY32W (integers and a u16 array).
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut out = vec![];
        // SAFETY: `entry` is writable, with its size set.
        let mut more = unsafe { Process32FirstW(snap.as_raw_handle(), &mut entry) } != 0;
        while more {
            out.push((entry.th32ProcessID, entry.th32ParentProcessID));
            more = unsafe { Process32NextW(snap.as_raw_handle(), &mut entry) } != 0;
        }
        (!out.is_empty()).then_some(out)
    }

    /// What runs under `root`, held open; `None` when the processes can't be listed. A process
    /// counts when its parent does and was created no later than it was: one whose parent id
    /// belonged to an earlier process is passed over. A process whose parent has exited can't be
    /// found this way; its job ends it.
    ///
    /// The list is read before the processes are opened, and an id can go to a new process in
    /// between. So it's read again once they're held (when their ids can't change hands): one
    /// whose parent isn't the one it had is someone else's, and so is all under it.
    pub(super) fn descendants(root: &Proc) -> Option<Vec<Proc>> {
        let all = snapshot()?;
        let mut found: Vec<(Proc, u32)> = vec![];
        let mut parents = vec![(root.pid, root.created)];
        while let Some((parent, created)) = parents.pop() {
            for &(pid, ppid) in &all {
                if ppid != parent || pid == root.pid || found.iter().any(|(p, _)| p.pid == pid) {
                    continue;
                }
                let Some(p) = Proc::open(pid as i32).filter(|p| p.created >= created) else { continue };
                parents.push((p.pid, p.created));
                found.push((p, ppid));
            }
        }
        let ids: Vec<(u32, u32)> = found.iter().map(|(p, ppid)| (p.pid, *ppid)).collect();
        let keep = unchanged(&ids, root.pid, &snapshot()?);
        Some(found.into_iter().zip(keep).filter_map(|((p, _), keep)| keep.then_some(p)).collect())
    }

    /// The ids of `pid`'s running children, created no earlier than it.
    pub(super) fn children(pid: u32) -> Vec<u32> {
        let Some(parent) = i32::try_from(pid).ok().and_then(Proc::open) else { return vec![] };
        let Some(all) = snapshot() else { return vec![] };
        all.into_iter().filter(|&(child, ppid)| ppid == pid && child != pid).filter(|&(child, _)| i32::try_from(child).ok().and_then(Proc::open).is_some_and(|c| c.created >= parent.created)).map(|(child, _)| child).collect()
    }

    /// Which of `found` (ids and parent ids, each parent before its children, all under `root`)
    /// are still where they were by the later list `again`: with the same parent, under a parent
    /// that is.
    pub(super) fn unchanged(found: &[(u32, u32)], root: u32, again: &[(u32, u32)]) -> Vec<bool> {
        let mut keep: Vec<bool> = Vec::with_capacity(found.len());
        for (i, &(pid, ppid)) in found.iter().enumerate() {
            let parent_kept = ppid == root || found[..i].iter().zip(&keep).any(|(&(p, _), &k)| k && p == ppid);
            keep.push(parent_kept && again.contains(&(pid, ppid)));
        }
        keep
    }

    /// End `g`'s first process and everything under it, when that process, running or exited but
    /// held open by someone (so its id is still its own), has the start time recorded. Twice over,
    /// for anything started while the first pass ran. True when all of it could be ended: false
    /// when it isn't `g`'s, when the processes couldn't be listed, or when one of them couldn't
    /// be opened with the right to end it.
    pub(super) fn end_tree(g: Group) -> bool {
        let Some(root) = Proc::open_any(g.id).filter(|p| g.started > 0 && unix_secs(p.created) == g.started) else { return false };
        let mut all = root.can_end;
        for _ in 0..2 {
            let tree = descendants(&root);
            root.terminate();
            match tree {
                Some(tree) => tree.iter().for_each(|p| {
                    all &= p.can_end;
                    p.terminate();
                }),
                None => all = false,
            }
        }
        all
    }
}

#[cfg(all(test, any(target_os = "macos", windows)))]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    fn sleeper() -> std::process::Child {
        use std::os::unix::process::CommandExt as _;
        std::process::Command::new("/bin/sleep").arg("30").process_group(0).spawn().unwrap()
    }

    #[cfg(windows)]
    fn sleeper() -> std::process::Child {
        quiet("ping", &["-n", "30", "127.0.0.1"])
    }

    /// `program` started with no console at all, its output thrown away. Detached, not just
    /// windowless: a new console would come with a conhost.exe of the process's own, which
    /// `children()` would count as one the process started (CI run 38048790217 caught two).
    #[cfg(windows)]
    fn quiet(program: &str, args: &[&str]) -> std::process::Child {
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::DETACHED_PROCESS;
        std::process::Command::new(program).args(args).stdout(std::process::Stdio::null()).creation_flags(DETACHED_PROCESS).spawn().unwrap()
    }

    /// The image `pid` runs, lower-cased ("c:\windows\system32\conhost.exe"), "" when it's gone.
    #[cfg(windows)]
    fn image_name(pid: u32) -> String {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: a handle this call owns (closed below), and a buffer written to `n` bytes.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return String::new();
            }
            let mut buf = [0u16; 1024];
            let mut n = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut n);
            CloseHandle(h);
            if ok == 0 { String::new() } else { String::from_utf16_lossy(&buf[..n as usize]).to_lowercase() }
        }
    }

    #[test]
    fn records_round_trip() {
        let r = Record { owner: 42, owner_started: 7, boot: 9, written: 11, groups: vec![Group { id: 100, started: 5 }, Group { id: 200, started: 6 }] };
        assert_eq!(Record::parse(&r.render()), Some(r));
        assert_eq!(Record::parse("garbage"), None);
        assert_eq!(Record::parse("owner 5 1\ngroup 1 0\n"), None, "never group 1 (launchd)");
    }

    /// For tests that register groups: the registry is the whole process's, and `end_all` in one
    /// test would end another's.
    static REGISTRY: Mutex<()> = Mutex::new(());

    fn registry() -> std::sync::MutexGuard<'static, ()> {
        REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn end_all_ends_registered_groups() {
        let _registry = registry();
        let mut child = sleeper();
        let id = child.id() as i32;
        register(id);
        assert!(live().contains(&id));
        end_all(Duration::from_millis(500));
        assert!(!live().contains(&id));
        let status = child.wait().unwrap();
        assert!(!status.success());
    }

    #[test]
    fn stale_records_of_a_dead_trek_are_reaped_and_live_ones_kept() {
        let dir = std::env::temp_dir().join(format!("trek-procs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut child = sleeper();
        let id = child.id() as i32;
        let group = Group { id, started: start_time(id).unwrap() };
        let now = chrono::Utc::now().timestamp();
        // A record from a Trek that's gone (pid 1's start time never matches 1 s after the epoch).
        let dead = Record { owner: 1, owner_started: 1, boot: boot_time(), written: now, groups: vec![group] };
        // One from a Trek that still runs (this test process): left alone.
        let me = std::process::id() as i32;
        let running = Record { owner: me, owner_started: start_time(me).unwrap(), boot: boot_time(), written: now, groups: vec![group] };
        std::fs::write(dir.join("1.txt"), dead.render()).unwrap();
        std::fs::write(dir.join("me.txt"), running.render()).unwrap();
        assert_eq!(reap_stale_in(&dir), 1);
        assert!(!child.wait().unwrap().success());
        assert!(!dir.join("1.txt").exists());
        assert!(dir.join("me.txt").exists(), "a running Trek's record stays");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reused_id_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("trek-procs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut child = sleeper();
        let id = child.id() as i32;
        // Recorded with another start time: some other process that got the id.
        let r = Record { owner: 1, owner_started: 1, boot: boot_time(), written: chrono::Utc::now().timestamp(), groups: vec![Group { id, started: 12345 }] };
        std::fs::write(dir.join("1.txt"), r.render()).unwrap();
        assert_eq!(reap_stale_in(&dir), 0);
        assert!(child.try_wait().unwrap().is_none(), "still running");
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_tree_is_ended_with_its_first_process() {
        // cmd waits for the ping it starts: a child under the group's first process.
        let mut child = quiet("cmd.exe", &["/d", "/c", "ping -n 60 127.0.0.1 >nul"]);
        let id = child.id() as i32;
        let root = win::Proc::open(id).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut tree = win::descendants(&root).unwrap();
        while tree.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            tree = win::descendants(&root).unwrap();
        }
        assert!(!tree.is_empty(), "cmd started its ping");
        // Recorded with another start time, it's someone else's: left alone.
        assert!(!win::end_tree(Group { id, started: 12345 }));
        assert!(tree.iter().all(win::Proc::running));
        assert!(win::end_tree(Group { id, started: start_time(id).unwrap() }));
        assert!(!child.wait().unwrap().success());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while tree.iter().any(win::Proc::running) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!tree.iter().any(win::Proc::running), "the ping under cmd was ended too");
    }

    #[test]
    fn a_process_lists_the_children_it_started() {
        // The shell waits for the sleeper it starts (the `;` keeps it from exec'ing it away).
        #[cfg(windows)]
        let mut parent = quiet("cmd.exe", &["/d", "/c", "ping -n 30 127.0.0.1 >nul"]);
        #[cfg(target_os = "macos")]
        let mut parent = std::process::Command::new("/bin/sh").args(["-c", "/bin/sleep 30; true"]).spawn().unwrap();
        let id = parent.id();
        // Console children may bring a conhost.exe (or OpenConsole.exe) of their own — Windows
        // parents each console's host under the console's owner — so it isn't counted; inside
        // the wait too, or a snapshot could catch it alone first.
        #[cfg(windows)]
        let is_console_host = |pid: u32| { let n = image_name(pid); n.ends_with("conhost.exe") || n.ends_with("openconsole.exe") };
        #[cfg(windows)]
        let kids_of = |pid: u32| children(pid).into_iter().filter(|k| !is_console_host(*k)).collect::<Vec<_>>();
        #[cfg(not(windows))]
        let kids_of = children;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut kids = kids_of(id);
        while kids.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            kids = kids_of(id);
        }
        assert_eq!(kids.len(), 1, "one child: {kids:?}");
        assert!(kids_of(kids[0]).is_empty(), "and none under it");
        assert!(children(u32::MAX - 1).is_empty(), "no such process");
        let _ = parent.kill();
        let _ = parent.wait();
        #[cfg(windows)]
        // Ending cmd leaves its ping running; it's ours to end.
        assert!(win::end_tree(Group { id: kids[0] as i32, started: start_time(kids[0] as i32).unwrap() }));
        #[cfg(not(windows))]
        // SAFETY: the child of a shell this test started and ended.
        unsafe {
            libc::kill(kids[0] as i32, libc::SIGKILL);
        }
    }

    #[cfg(windows)]
    #[test]
    fn end_all_gives_groups_their_grace_before_ending_them() {
        let _registry = registry();
        // One goes by itself within the grace (cmd waits a second for its ping), one doesn't.
        let mut quick = quiet("cmd.exe", &["/d", "/c", "ping -n 2 127.0.0.1 >nul"]);
        let mut slow = sleeper();
        register(quick.id() as i32);
        register(slow.id() as i32);
        let grace = Duration::from_secs(4);
        let started = std::time::Instant::now();
        end_all(grace);
        let took = started.elapsed();
        assert!(quick.wait().unwrap().success(), "left to finish on its own");
        assert!(!slow.wait().unwrap().success(), "ended once the grace ran out");
        assert!(took >= grace - Duration::from_millis(50) && took < grace + Duration::from_secs(5), "{took:?}");
        assert!(live().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn a_group_with_no_job_is_ended_by_its_tree_even_once_its_first_process_has_exited() {
        let _registry = registry();
        // cmd starts a ping it doesn't wait for, then exits a second later. `child` holds cmd open,
        // as trek-agents holds its children, so cmd's id stays its own after it exits.
        let mut child = quiet("cmd.exe", &["/d", "/c", "start /b ping -n 60 127.0.0.1 >nul & ping -n 2 127.0.0.1 >nul"]);
        let id = child.id() as i32;
        register(id);
        assert!(!end_tree(0), "only a tracked group");
        let root = win::Proc::open(id).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut tree = win::descendants(&root).unwrap();
        while tree.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            tree = win::descendants(&root).unwrap();
        }
        assert!(!tree.is_empty(), "cmd started its ping");
        drop(root);
        assert!(child.wait().unwrap().success());
        assert!(tree.iter().any(win::Proc::running), "the first ping outlives cmd");
        assert!(end_tree(id), "cmd and all under it could be ended");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while tree.iter().any(win::Proc::running) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!tree.iter().any(win::Proc::running), "the ping cmd left behind was ended");
        assert!(live().contains(&id), "still tracked until unregistered");
        unregister(id);
    }

    #[cfg(windows)]
    #[test]
    fn processes_whose_ids_changed_hands_are_passed_over() {
        // 10 and 20 under the root (1), 11 under 10, 12 under 11. By the second list, 11 has a
        // parent it didn't have (its id went to another process): it and 12 under it are passed
        // over, and so is 20, gone.
        let found = [(10, 1), (11, 10), (12, 11), (20, 1)];
        let again = [(10, 1), (11, 99), (12, 11), (30, 1)];
        assert_eq!(win::unchanged(&found, 1, &again), [true, false, false, false]);
        assert_eq!(win::unchanged(&found, 1, &found), [true; 4]);
    }

    #[cfg(windows)]
    #[test]
    fn a_finished_process_is_gone_while_its_handle_is_open() {
        let mut child = quiet("cmd.exe", &["/d", "/c", "exit 0"]);
        let id = child.id() as i32;
        assert!(child.wait().unwrap().success());
        // `child` still holds the process open, so the id is still its own.
        assert_eq!(start_time(id), None);
        assert!(!alive_since(id, 0));
    }

    #[cfg(windows)]
    #[test]
    fn this_process_and_boot_are_dated() {
        let now = chrono::Utc::now().timestamp();
        let me = std::process::id() as i32;
        let started = start_time(me).unwrap();
        assert!(started <= now && now - started < 3600, "{started} vs {now}");
        assert!(alive_since(me, started));
        let boot = boot_time();
        assert!(boot > 0 && boot <= started, "booted {boot}, started {started}");
        assert!(same_boot(boot, boot_time()));
        assert!(!same_boot(boot, boot - 600));
    }
}
