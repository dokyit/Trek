//! Every process group Trek starts (agent CLIs, language servers, status probes), in one place:
//! signalled when Trek quits by any path, and recorded in the data folder so the next launch can
//! end the ones a crash left running.

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

/// Track the process group `group` (a child started with `process_group(0)`; its pid).
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
        if record.boot == boot_time() && now - record.written < STALE_AFTER_SECS {
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

#[cfg(not(target_os = "macos"))]
fn signal(_group: i32, _s: Signal) {}

/// Some process is still in `group`.
#[cfg(target_os = "macos")]
fn group_exists(group: i32) -> bool {
    // SAFETY: signal 0 only checks; nothing is sent.
    group > 1 && (unsafe { libc::kill(-group, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

#[cfg(not(target_os = "macos"))]
fn group_exists(_group: i32) -> bool {
    false
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

#[cfg(not(target_os = "macos"))]
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

#[cfg(not(target_os = "macos"))]
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

#[cfg(not(target_os = "macos"))]
fn boot_time() -> i64 {
    0
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn sleeper() -> std::process::Child {
        use std::os::unix::process::CommandExt as _;
        std::process::Command::new("/bin/sleep").arg("30").process_group(0).spawn().unwrap()
    }

    #[test]
    fn records_round_trip() {
        let r = Record { owner: 42, owner_started: 7, boot: 9, written: 11, groups: vec![Group { id: 100, started: 5 }, Group { id: 200, started: 6 }] };
        assert_eq!(Record::parse(&r.render()), Some(r));
        assert_eq!(Record::parse("garbage"), None);
        assert_eq!(Record::parse("owner 5 1\ngroup 1 0\n"), None, "never group 1 (launchd)");
    }

    #[test]
    fn end_all_ends_registered_groups() {
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
}
