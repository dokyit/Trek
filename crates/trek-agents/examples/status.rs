//! Account, plan, usage limits, commands and models for Claude Code and Codex — no prompt is
//! sent, so this costs nothing: `cargo run -p trek-agents --example status`
use trek_agents::{AgentStatus, claude_status, codex_status};

fn print(name: &str, s: &anyhow::Result<AgentStatus>) {
    println!("== {name}");
    let s = match s {
        Ok(s) => s,
        Err(e) => return println!("  failed: {e:#}"),
    };
    println!("  logged_in: {}  account: {:?}  plan: {:?}", s.logged_in, s.account, s.plan);
    if let Some(e) = &s.error {
        println!("  error: {e}");
    }
    for l in &s.limits {
        let resets = l.resets_at.and_then(chrono::DateTime::from_timestamp_millis).map(|d| d.to_rfc3339()).unwrap_or_default();
        println!("  limit: {:<24} {:>5.1}%  window {:<4} resets {resets}", l.label, l.percent, l.window);
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

fn main() {
    let cwd = std::path::PathBuf::from("/tmp/trek-e2e");
    std::fs::create_dir_all(&cwd).unwrap();
    trek_core::runtime().block_on(async {
        let (claude, codex) = tokio::join!(claude_status(&cwd), codex_status(&cwd));
        print("Claude Code", &claude);
        print("Codex", &codex);
    });
}
