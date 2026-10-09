//! Account, plan, usage limits, commands and models for Claude Code and Codex — no prompt is
//! sent, so this costs nothing: `cargo run -p trek-agents --example status -- [full|commands]`
//! (both by default). `full` is what the Usage card reads (`claude_status`, `codex_status`);
//! `commands` is the read without the plan's usage (`claude_commands`, `codex_commands`).
//! Each read is timed, and any process it leaves behind is listed.
//!
//! To see the protocol traffic, point `SHELL` at a script that prints a PATH with logging
//! shims of `claude` and `codex` first: Trek finds the CLIs on the login shell's PATH.
use std::future::Future;
use std::time::Instant;
use trek_agents::{AgentStatus, claude_commands, claude_status, codex_commands, codex_status};

fn print(name: &str, s: &anyhow::Result<AgentStatus>) {
    println!("== {name}");
    let s = match s {
        Ok(s) => s,
        Err(e) => return println!("  failed: {e:#}"),
    };
    println!("  logged_in: {}  account: {}  plan: {:?}  billing: {:?}", s.logged_in, if s.account.is_some() { "(set)" } else { "none" }, s.plan, s.billing);
    if let Some(e) = &s.error {
        println!("  error: {e}");
    }
    for l in &s.limits {
        let resets = l.resets_at.and_then(chrono::DateTime::from_timestamp_millis).map(|d| d.to_rfc3339()).unwrap_or_default();
        println!("  limit: {:<24} {:>5.1}%  window {:<4} resets {resets}", l.label, l.percent, l.window);
    }
    for r in &s.resets {
        println!("  reset credit: {:?} expires {:?}", r.title, r.expires_at);
    }
    for m in &s.models {
        println!("  model: {:<28} {:<24} tier {} fast {:?} efforts {:?}", m.id, m.name, m.tier, m.fast, m.efforts);
    }
    let count = |k| s.commands.iter().filter(|c| c.kind == k).count();
    use trek_agents::CommandKind::*;
    println!("  commands: {} commands, {} skills, {} agents", count(Command), count(Skill), count(Agent));
    for c in s.commands.iter().take(8) {
        let d: String = c.description.chars().take(60).collect();
        println!("    {:?} /{} — {d}", c.kind, c.name);
    }
}

/// Processes still running under this one.
fn leftovers() -> Vec<String> {
    let me = std::process::id().to_string();
    let out = std::process::Command::new("/bin/ps").args(["-A", "-o", "pid=,ppid=,pgid=,command="]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let rows: Vec<Vec<&str>> = text.lines().map(|l| l.split_whitespace().collect()).filter(|r: &Vec<&str>| r.len() >= 4).collect();
    let mut ours = vec![me];
    let mut found = Vec::new();
    while let Some(pid) = ours.pop() {
        for r in rows.iter().filter(|r| r[1] == pid && !r[3].ends_with("/ps")) {
            ours.push(r[0].to_string());
            found.push(r.join(" "));
        }
    }
    found
}

async fn timed<F: Future<Output = anyhow::Result<AgentStatus>>>(name: &str, read: F) {
    let started = Instant::now();
    let s = read.await;
    let took = started.elapsed();
    print(name, &s);
    println!("  took {:.2}s", took.as_secs_f32());
    // `terminate` reaps what it ends; give stragglers a moment before looking.
    std::thread::sleep(std::time::Duration::from_millis(300));
    match leftovers() {
        left if left.is_empty() => println!("  no processes left behind"),
        left => left.iter().for_each(|p| println!("  LEFT RUNNING: {p}")),
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let (full, commands) = match mode.as_str() {
        "full" => (true, false),
        "commands" => (false, true),
        _ => (true, true),
    };
    // Trek's process records go here, not in the app's data folder.
    trek_core::paths::isolate(std::env::temp_dir().join("trek-status-example"));
    let cwd = std::path::PathBuf::from("/tmp/trek-e2e");
    std::fs::create_dir_all(&cwd).unwrap();
    trek_core::runtime().block_on(async {
        if full {
            timed("Claude Code (full: claude_status)", claude_status(&cwd)).await;
            timed("Codex (full: codex_status)", codex_status(&cwd)).await;
        }
        if commands {
            timed("Claude Code (commands only: claude_commands)", claude_commands(&cwd)).await;
            timed("Codex (commands only: codex_commands)", codex_commands(&cwd)).await;
        }
    });
}
