//! A sampling profiler for one thread of another process on Windows, with no admin rights needed
//! for a process of the same user (docs/PERF.md, "Where a frame's time goes"): suspend the thread,
//! read its registers, walk its stack with dbghelp, resume; every `interval` ms for `seconds`.
//! Then each address is named from the process's symbols (the PDB beside its executable, or where
//! its build left it) and the samples are summed:
//!
//!   sampler <pid> [tid|main] [seconds] [interval_ms] [out.folded]
//!
//! `main` (the default) is the process's first thread, the UI thread for Trek. It prints the
//! functions most often on the stack (inclusive) and at its top (self), says which modules had
//! symbols, and writes the stacks folded (`root;...;leaf count`) for a closer look. A release build
//! keeps its symbols with `CARGO_PROFILE_RELEASE_STRIP=none CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Diagnostics::Debug::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::SystemInformation::IMAGE_FILE_MACHINE_AMD64;
use windows_sys::Win32::System::Threading::*;

#[repr(C, align(16))]
struct Ctx(CONTEXT);

fn main_thread(pid: u32) -> Option<u32> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut te: THREADENTRY32 = std::mem::zeroed();
        te.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut best: Option<(u64, u32)> = None;
        if Thread32First(snap, &mut te) != 0 {
            loop {
                if te.th32OwnerProcessID == pid {
                    let h = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, te.th32ThreadID);
                    if !h.is_null() {
                        let (mut c, mut e, mut k, mut u) = std::mem::zeroed();
                        if GetThreadTimes(h, &mut c, &mut e, &mut k, &mut u) != 0 {
                            let t = ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64;
                            if best.is_none_or(|(b, _)| t < b) {
                                best = Some((t, te.th32ThreadID));
                            }
                        }
                        CloseHandle(h);
                    }
                }
                if Thread32Next(snap, &mut te) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        best.map(|(_, id)| id)
    }
}

fn name_of(process: HANDLE, pc: u64) -> String {
    unsafe {
        const MAX: usize = 1024;
        let mut buf = vec![0u8; std::mem::size_of::<SYMBOL_INFOW>() + MAX * 2];
        let sym = buf.as_mut_ptr() as *mut SYMBOL_INFOW;
        (*sym).SizeOfStruct = std::mem::size_of::<SYMBOL_INFOW>() as u32;
        (*sym).MaxNameLen = MAX as u32;
        let mut displacement = 0u64;
        if SymFromAddrW(process, pc, &mut displacement, sym) != 0 {
            let len = (*sym).NameLen as usize;
            let name = std::slice::from_raw_parts((*sym).Name.as_ptr(), len.min(MAX));
            return String::from_utf16_lossy(name);
        }
        let mut m: IMAGEHLP_MODULEW64 = std::mem::zeroed();
        m.SizeOfStruct = std::mem::size_of::<IMAGEHLP_MODULEW64>() as u32;
        if SymGetModuleInfoW64(process, pc, &mut m) != 0 {
            let n = m
                .ModuleName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(m.ModuleName.len());
            return format!(
                "{}!{:#x}",
                String::from_utf16_lossy(&m.ModuleName[..n]),
                pc - m.BaseOfImage
            );
        }
        format!("{pc:#x}")
    }
}

/// Shortens a Rust symbol for reading: generic arguments and hashes go.
fn short(name: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in name.chars() {
        match c {
            '<' => {
                depth += 1;
                if depth == 1 {
                    out.push_str("<…>");
                }
            }
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pid: u32 = args
        .get(1)
        .and_then(|a| a.parse().ok())
        .expect("sampler <pid> [tid|main] [seconds] [interval_ms] [out.folded]");
    let tid = match args.get(2).map(String::as_str) {
        None | Some("main") => main_thread(pid).expect("no threads"),
        Some(t) => t.parse().expect("tid"),
    };
    let seconds: f64 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(10.0);
    let interval: u64 = args.get(4).and_then(|a| a.parse().ok()).unwrap_or(2);
    let folded_out = args.get(5).cloned();
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        assert!(!process.is_null(), "OpenProcess failed: {}", GetLastError());
        let thread = OpenThread(
            THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
            0,
            tid,
        );
        assert!(!thread.is_null(), "OpenThread failed: {}", GetLastError());
        SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_LOAD_LINES);
        // The executable's folder first: its PDB sits beside it.
        let mut image = vec![0u16; 1024];
        let mut len = image.len() as u32;
        let mut search = String::new();
        if QueryFullProcessImageNameW(process, 0, image.as_mut_ptr(), &mut len) != 0 {
            let path = std::path::PathBuf::from(String::from_utf16_lossy(&image[..len as usize]));
            if let Some(dir) = path.parent() {
                search = dir.display().to_string();
            }
        }
        let wide: Vec<u16> = search.encode_utf16().chain(std::iter::once(0)).collect();
        assert!(
            SymInitializeW(process, wide.as_ptr(), 1) != 0,
            "SymInitialize failed"
        );
        eprintln!("symbols searched in {search:?}");
        eprintln!("sampling pid {pid} thread {tid} for {seconds} s every {interval} ms");
        let mut stacks: Vec<Vec<u64>> = Vec::new();
        let mut suspended_total = Duration::ZERO;
        let end = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < end {
            let began = Instant::now();
            if SuspendThread(thread) == u32::MAX {
                break;
            }
            let mut ctx: Box<Ctx> = Box::new(std::mem::zeroed());
            ctx.0.ContextFlags = CONTEXT_FULL_AMD64;
            if GetThreadContext(thread, &mut ctx.0) != 0 {
                let mut frame: STACKFRAME64 = std::mem::zeroed();
                frame.AddrPC = ADDRESS64 {
                    Offset: ctx.0.Rip,
                    Segment: 0,
                    Mode: AddrModeFlat,
                };
                frame.AddrStack = ADDRESS64 {
                    Offset: ctx.0.Rsp,
                    Segment: 0,
                    Mode: AddrModeFlat,
                };
                frame.AddrFrame = ADDRESS64 {
                    Offset: ctx.0.Rbp,
                    Segment: 0,
                    Mode: AddrModeFlat,
                };
                let mut pcs = Vec::with_capacity(64);
                for _ in 0..512 {
                    let ok = StackWalk64(
                        IMAGE_FILE_MACHINE_AMD64 as u32,
                        process,
                        thread,
                        &mut frame,
                        &mut ctx.0 as *mut CONTEXT as *mut core::ffi::c_void,
                        None,
                        Some(SymFunctionTableAccess64),
                        Some(SymGetModuleBase64),
                        None,
                    );
                    if ok == 0 || frame.AddrPC.Offset == 0 {
                        break;
                    }
                    pcs.push(frame.AddrPC.Offset);
                }
                stacks.push(pcs);
            }
            ResumeThread(thread);
            suspended_total += began.elapsed();
            std::thread::sleep(Duration::from_millis(interval));
        }
        eprintln!(
            "{} samples; the thread was held {:.1} ms a sample on average",
            stacks.len(),
            suspended_total.as_secs_f64() * 1000.0 / stacks.len().max(1) as f64
        );
        // What symbols each module came with (the first address seen in it).
        let mut modules_seen = HashSet::new();
        for s in &stacks {
            for &pc in s {
                let base = SymGetModuleBase64(process, pc);
                if base != 0 && modules_seen.insert(base) {
                    let mut m: IMAGEHLP_MODULEW64 = std::mem::zeroed();
                    m.SizeOfStruct = std::mem::size_of::<IMAGEHLP_MODULEW64>() as u32;
                    if SymGetModuleInfoW64(process, pc, &mut m) != 0 {
                        let z = |w: &[u16]| {
                            let n = w.iter().position(|&c| c == 0).unwrap_or(w.len());
                            String::from_utf16_lossy(&w[..n])
                        };
                        // A name forces a deferred load.
                        let _ = name_of(process, pc);
                        let mut m2: IMAGEHLP_MODULEW64 = std::mem::zeroed();
                        m2.SizeOfStruct = m.SizeOfStruct;
                        SymGetModuleInfoW64(process, pc, &mut m2);
                        eprintln!(
                            "module {} symbols {} pdb {:?}",
                            z(&m2.ModuleName),
                            m2.SymType,
                            z(&m2.LoadedPdbName)
                        );
                    }
                }
            }
        }
        // Name every address once.
        let mut names: HashMap<u64, String> = HashMap::new();
        for s in &stacks {
            for &pc in s {
                // The return address is the instruction after the call: look one byte back.
                let at = if Some(&pc) == s.first() { pc } else { pc - 1 };
                names
                    .entry(pc)
                    .or_insert_with(|| short(&name_of(process, at)));
            }
        }
        let n = stacks.len().max(1) as f64;
        let mut inclusive: HashMap<&str, usize> = HashMap::new();
        let mut self_: HashMap<&str, usize> = HashMap::new();
        let mut folded: HashMap<String, usize> = HashMap::new();
        for s in &stacks {
            let named: Vec<&str> = s.iter().map(|pc| names[pc].as_str()).collect();
            if let Some(top) = named.first() {
                *self_.entry(top).or_default() += 1;
            }
            let mut seen = HashSet::new();
            for f in &named {
                if seen.insert(*f) {
                    *inclusive.entry(f).or_default() += 1;
                }
            }
            let mut root_first = named.clone();
            root_first.reverse();
            *folded.entry(root_first.join(";")).or_default() += 1;
        }
        let print = |title: &str, m: &HashMap<&str, usize>, top: usize| {
            println!("\n== {title}");
            let mut v: Vec<_> = m.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1));
            for (name, c) in v.into_iter().take(top) {
                println!("{:5.1}%  {:6}  {}", 100.0 * *c as f64 / n, c, name);
            }
        };
        print("inclusive (on the stack)", &inclusive, 90);
        print("self (at the top)", &self_, 40);
        if let Some(path) = folded_out {
            let mut v: Vec<_> = folded.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            let text: String = v.into_iter().map(|(k, c)| format!("{k} {c}\n")).collect();
            std::fs::write(&path, text).expect("write folded");
            eprintln!("folded stacks in {path}");
        }
        SymCleanup(process);
        CloseHandle(thread);
        CloseHandle(process);
    }
}
