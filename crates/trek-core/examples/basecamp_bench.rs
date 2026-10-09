//! How long Basecamp's numbers take: a throwaway database with this machine's agent histories
//! imported (read only, from the agents' own folders) and Trek threads made up on top, then each
//! range read cold (nothing cached), warm (the same process again) and after a relaunch (only
//! what's kept on disk).
//!
//! Run with `TREK_DATA_DIR=$(mktemp -d) cargo run --release -p trek-core --example basecamp_bench`
//! (`BENCH_THREADS` made-up threads, 2000 by default; `BENCH_IMPORT=0` leaves the histories out).
//! Run it again with `BENCH_REUSE=1` (same folder) for a relaunch: the database as it was left.
use std::time::Instant;
use trek_core::basecamp::{self, Range};
use trek_core::store::{Item, Store, ToolStatus};
use trek_core::{AgentId, Effort, HandHolding, TokenUsage};

fn main() {
    let dir = std::env::var_os("TREK_DATA_DIR").map(std::path::PathBuf::from).expect("set TREK_DATA_DIR to a throwaway folder");
    let path = dir.join("bench.sqlite");
    let reuse = std::env::var("BENCH_REUSE").as_deref() == Ok("1") && path.exists();
    if !reuse {
        let _ = std::fs::remove_file(&path);
    }
    let store = Store::open(&path).unwrap();
    let now = chrono::Local::now();
    if reuse {
        measure(&store, &now, "relaunch");
        return;
    }
    if std::env::var("BENCH_IMPORT").as_deref() != Ok("0") {
        let t = Instant::now();
        let settings = trek_core::settings::Import::default();
        let s = trek_core::import::import_all(&store, &settings);
        println!("import: {} Claude Code, {} Codex, {} OpenCode threads in {:?}", s.claude_code, s.codex, s.opencode, t.elapsed());
    }
    let n: usize = std::env::var("BENCH_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(2_000);
    let t = Instant::now();
    let span = 180 * 86_400_000i64;
    let models = ["claude-opus-5-5", "claude-sonnet-5-5", "gpt-6-astra"];
    for i in 0..n {
        let agent = if i % 3 == 2 { AgentId::Codex } else { AgentId::ClaudeCode };
        let th = store.create_thread(None, agent.clone(), Some(models[i % 3].into()), Effort::High, HandHolding::Auto).unwrap();
        // The newest first: a few today, most of them weeks and months ago.
        let start = now.timestamp_millis() - 3_600_000 - span / n as i64 * i as i64;
        let mut items = vec![];
        for k in 0..10i64 {
            let p = start - (10 - k) * 300_000;
            items.push(Item::User { text: format!("step {k}"), images: vec![], at: Some(p), resume: None, aside: false });
            items.push(Item::Reasoning { text: "Thinking it over.".into() });
            items.push(Item::Tool { id: format!("t{k}"), title: "Read".into(), detail: "src/lib.rs".into(), output: "fn main() {}\n".repeat(200), status: ToolStatus::Done });
            items.push(Item::Assistant { text: "Done.".into() });
            items.push(Item::TurnEnd { at: p + 120_000, took_secs: 120 });
            store.record_usage(&th.id, p + 120_000, &agent, Some(models[i % 3]), &TokenUsage { input: 100, output: 900, cache_read: 9_000, cache_write: 0 }, None).unwrap();
        }
        store.save_transcript(&th.id, &mut trek_core::transcript::Transcript::unsaved(items)).unwrap();
        store.update_thread(&th.id, |t| t.updated_at = start).unwrap();
    }
    println!("made up {n} Trek threads in {:?}", t.elapsed());
    measure(&store, &now, "cold");
}

fn measure(store: &Store, now: &chrono::DateTime<chrono::Local>, pass: &str) {
    // One read of everything, then every range from it.
    let t = Instant::now();
    let g = basecamp::Gathered::gather(store, None).unwrap();
    println!("{pass:>8} read all     {:>9.1?}  ({} threads, {} history files read)", t.elapsed(), g.read, basecamp::HISTORIES_READ.load(std::sync::atomic::Ordering::Relaxed));
    for range in [Range::Today, Range::Week, Range::All] {
        let t = Instant::now();
        let r = g.recap(range, now);
        println!("{pass:>8} {:<10}  {:>9.1?}  ({} threads, {} turns, {} tokens)", range.label(), t.elapsed(), r.threads, r.turns, r.tokens.total());
    }
    // Nothing changed: nothing read.
    let t = Instant::now();
    let g = basecamp::Gathered::gather(store, Some(g)).unwrap();
    println!("{pass:>8} unchanged    {:>9.1?}  ({} threads read)", t.elapsed(), g.read);
    // A turn ends in one thread: only that one is read again.
    let id = g.threads.iter().find(|t| t.thread.source == trek_core::ThreadSource::Trek).map(|t| t.thread.id.clone()).unwrap();
    let at = trek_core::store::now_ms();
    store.record_usage(&id, at, &AgentId::ClaudeCode, Some("claude-opus-5-5"), &TokenUsage { input: 1, output: 1, cache_read: 1, cache_write: 0 }, None).unwrap();
    store.update_thread(&id, |t| t.updated_at = at).unwrap();
    let t = Instant::now();
    let g = basecamp::Gathered::gather(store, Some(g)).unwrap();
    let read = t.elapsed();
    let t = Instant::now();
    for range in [Range::Today, Range::Week, Range::All] {
        g.recap(range, now);
    }
    println!("{pass:>8} a turn later {:>9.1?}  ({} thread read), every range again in {:.1?}", read, g.read, t.elapsed());
}
