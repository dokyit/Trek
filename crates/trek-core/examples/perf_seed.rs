//! A data folder for performance and stress runs (`script/perf.ps1`): projects and threads made
//! through the store's own API, the same every run (only the times move with the clock).
//!
//! - `SEED_THREADS` threads (200 by default) on the mock agent across four projects, newest
//!   first: a handful today, more this week, the rest spread over four months. Each has four
//!   turns (a prompt, a thought, a tool call with output, a markdown answer, the turn's end) and
//!   the tokens each turn reported, so Basecamp's ranges all have something to count.
//! - One thread titled "Long transcript" with `SEED_LONG` rounds of the same (400 by default:
//!   2,000 items), the newest, for scrolling.
//! - In each project a `README.md` and a 3,000-line `src/pipeline.rs`, for the editor.
//!
//! Run with `TREK_DATA_DIR=<new folder> cargo run --release -p trek-core --example perf_seed`.
//! It refuses a folder that already has a database: it never adds to someone's data.
use std::path::PathBuf;
use std::time::Instant;
use trek_core::store::{Item, Store, ToolStatus};
use trek_core::{AgentId, Effort, HandHolding, TokenUsage};

const PROJECTS: [&str; 4] = ["atlas-api", "harbor-web", "lumen-cli", "orbit-mobile"];
const VERBS: [&str; 10] = ["Fix", "Add", "Refactor", "Speed up", "Document", "Test", "Clean up", "Migrate", "Profile", "Review"];
const NOUNS: [&str; 10] = ["the login flow", "pagination", "the build cache", "retry backoff", "the settings page", "CSV export", "the websocket client", "search ranking", "error toasts", "the release script"];

fn main() {
    let dir = std::env::var_os("TREK_DATA_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).expect("set TREK_DATA_DIR to a new, empty folder");
    trek_core::paths::isolate(dir.clone());
    let db = trek_core::paths::database_file();
    assert!(!db.exists(), "{} already has a database: give a new folder", dir.display());
    let n: usize = std::env::var("SEED_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    let long: usize = std::env::var("SEED_LONG").ok().and_then(|v| v.parse().ok()).unwrap_or(400);
    let store = Store::open(&db).unwrap();
    let t = Instant::now();
    let folders: Vec<PathBuf> = PROJECTS
        .iter()
        .map(|p| {
            let f = dir.join("projects").join(p);
            std::fs::create_dir_all(&f).unwrap();
            std::fs::write(f.join("README.md"), format!("# {p}\n\nA project for Trek's performance runs.\n")).unwrap();
            // A long source file to type into (the editor's highlighting has a lot to do).
            std::fs::create_dir_all(f.join("src")).unwrap();
            let code: String = (0..500).map(|n| format!("/// Step {n} of the pipeline.\npub fn step_{n}(input: &[u8]) -> Result<Vec<u8>, Error> {{\n    let out = input.iter().map(|b| b.wrapping_add({n} as u8)).collect();\n    tracing::debug!(\"step {n}: {{}} bytes\", input.len());\n    Ok(out)\n}}\n")).collect();
            std::fs::write(f.join("src").join("pipeline.rs"), code).unwrap();
            f
        })
        .collect();
    let now = trek_core::store::now_ms();
    let agent = AgentId::Direct(trek_core::catalog::MOCK_PROVIDER.into());
    let span = 120 * 86_400_000i64;
    let mut items_made = 0;
    for i in 0..n {
        // Newest first, more of them recent: (i/n)² of four months back.
        let f = i as f64 / n as f64;
        let last = now - 300_000 - (span as f64 * f * f) as i64 - i as i64 * 60_000;
        let (title, rounds) = if i == 0 { ("Long transcript".to_string(), long) } else { (format!("{} {} #{i}", VERBS[i % 10], NOUNS[(i / 10) % 10]), 4) };
        let model = if i % 2 == 0 { "mock-swift" } else { "mock-deep" };
        let th = store.create_thread(Some(&folders[i % folders.len()]), agent.clone(), Some(model.into()), Effort::High, HandHolding::Auto).unwrap();
        let mut items = Vec::with_capacity(rounds * 5);
        for k in 0..rounds {
            let at = last - (rounds - k) as i64 * 150_000;
            items.extend(round(i, k, at));
            store.record_usage(&th.id, at + 90_000, &agent, Some(model), &TokenUsage { input: 1_200 + k as u64, output: 800, cache_read: 12_000, cache_write: 400 }, None).unwrap();
        }
        items_made += items.len();
        store.save_transcript(&th.id, &mut trek_core::transcript::Transcript::unsaved(items)).unwrap();
        store
            .update_thread(&th.id, |t| {
                t.title = title;
                t.created_at = last - rounds as i64 * 150_000 - 60_000;
                t.updated_at = last;
                // One in seven is unread.
                t.last_seen_at = if i % 7 == 3 { last - 1 } else { last };
            })
            .unwrap();
    }
    println!("seeded {n} threads ({items_made} items) in {} projects under {} in {:?}", PROJECTS.len(), dir.display(), t.elapsed());
}

/// Round `k` of thread `i`, asked at `at`: the five items of a turn.
fn round(i: usize, k: usize, at: i64) -> [Item; 5] {
    let file = ["src/lib.rs", "src/server/routes.rs", "web/app.tsx", "scripts/release.sh"][(i + k) % 4];
    let output: String = (1..=30).map(|l| format!("{l:>4} | let value_{l} = compute(input, {l});\n")).collect();
    let answer = format!(
        "## Round {k}\n\nI read `{file}` and changed **three** things:\n\n\
         1. The retry loop now backs off exponentially, capped at 30 s.\n\
         2. Errors carry the request id, so the log lines up with the trace.\n\
         3. A test covers the empty-page case in `tests/pagination.rs`.\n\n\
         ```rust\nfn backoff(attempt: u32) -> Duration {{\n    Duration::from_millis(250 * 2u64.pow(attempt.min(7)))\n}}\n```\n\n\
         | check | before | after |\n|---|---|---|\n| p50 | 120 ms | 80 ms |\n| p95 | 900 ms | 210 ms |\n\n\
         Next I'd look at the cache headers in {file}."
    );
    [
        Item::User { text: format!("Round {k}: look at {file} and make the retries saner (thread {i})."), images: vec![], at: Some(at), resume: None, aside: false },
        Item::Reasoning { text: format!("The retries in {file} hammer the server when it's down; an exponential backoff with a cap fixes that, and the tests need a case for an empty page.") },
        Item::Tool { id: format!("t{i}-{k}"), title: "Read".into(), detail: file.into(), output, status: ToolStatus::Done },
        Item::Assistant { text: answer },
        Item::TurnEnd { at: at + 90_000, took_secs: 90 },
    ]
}
