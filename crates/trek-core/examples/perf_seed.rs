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
//! For the memory stress (`agent-cli perf memory`, `docs/PERFORMANCE.md`), all off by default:
//!
//! - `SEED_HEAVY` threads titled "Heavy thread #n" of `SEED_HEAVY_ROUNDS` turns (40 by default)
//!   whose answers are long markdown (about 24 KB: prose, lists, a table and two code blocks),
//!   every tenth a 120 KB one, and whose tool calls print 5 KB.
//! - `SEED_IMAGE_THREADS` threads titled "Image thread #n" of `SEED_IMAGE_ROUNDS` turns (5 by
//!   default): each message attaches two of the pictures in `SEED_IMAGE_DIR` and each answer shows
//!   a third, no picture used twice while the folder has enough.
//! - `SEED_ARTIFACTS` threads titled "Artifact thread #n", four turns each, each answer a small
//!   `trek-html` page of its own.
//!
//! Run with `TREK_DATA_DIR=<new folder> cargo run --release -p trek-core --example perf_seed`.
//! It refuses a folder that already has a database: it never adds to someone's data.
use std::path::PathBuf;
use std::time::Instant;
use trek_core::store::{Item, Store, ToolStatus};
use trek_core::{AgentId, Effort, HandHolding, TokenUsage};

const PROJECTS: [&str; 4] = ["atlas-api", "harbor-web", "lumen-cli", "orbit-mobile"];
const VERBS: [&str; 10] = [
    "Fix", "Add", "Refactor", "Speed up", "Document", "Test", "Clean up", "Migrate", "Profile",
    "Review",
];
const NOUNS: [&str; 10] = [
    "the login flow",
    "pagination",
    "the build cache",
    "retry backoff",
    "the settings page",
    "CSV export",
    "the websocket client",
    "search ranking",
    "error toasts",
    "the release script",
];

fn main() {
    let dir = std::env::var_os("TREK_DATA_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .expect("set TREK_DATA_DIR to a new, empty folder");
    trek_core::paths::isolate(dir.clone());
    let db = trek_core::paths::database_file();
    assert!(
        !db.exists(),
        "{} already has a database: give a new folder",
        dir.display()
    );
    let n: usize = std::env::var("SEED_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let long: usize = std::env::var("SEED_LONG")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400);
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
        let (title, rounds) = if i == 0 {
            ("Long transcript".to_string(), long)
        } else {
            (
                format!("{} {} #{i}", VERBS[i % 10], NOUNS[(i / 10) % 10]),
                4,
            )
        };
        let model = if i % 2 == 0 {
            "mock-swift"
        } else {
            "mock-deep"
        };
        let th = store
            .create_thread(
                Some(&folders[i % folders.len()]),
                agent.clone(),
                Some(model.into()),
                Effort::High,
                HandHolding::Auto,
            )
            .unwrap();
        let mut items = Vec::with_capacity(rounds * 5);
        for k in 0..rounds {
            let at = last - (rounds - k) as i64 * 150_000;
            items.extend(round(i, k, at));
            store
                .record_usage(
                    &th.id,
                    at + 90_000,
                    &agent,
                    Some(model),
                    &TokenUsage {
                        input: 1_200 + k as u64,
                        output: 800,
                        cache_read: 12_000,
                        cache_write: 400,
                    },
                    None,
                )
                .unwrap();
        }
        items_made += items.len();
        store
            .save_transcript(
                &th.id,
                &mut trek_core::transcript::Transcript::unsaved(items),
            )
            .unwrap();
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
    let env_n = |name: &str, default: usize| -> usize {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    // The memory stress's threads, newest of all (each a minute older than the one before).
    let mut extra: Vec<(String, Vec<Item>)> = Vec::new();
    let heavy_rounds = env_n("SEED_HEAVY_ROUNDS", 40);
    for i in 0..env_n("SEED_HEAVY", 0) {
        let items = (0..heavy_rounds)
            .flat_map(|k| heavy_round(i, k, now - (heavy_rounds - k) as i64 * 150_000))
            .collect();
        extra.push((format!("Heavy thread #{i:02}"), items));
    }
    let mut pictures: Vec<PathBuf> = std::env::var_os("SEED_IMAGE_DIR")
        .map(PathBuf::from)
        .and_then(|d| std::fs::read_dir(d).ok())
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    pictures.retain(|p| p.extension().is_some_and(|e| e == "png"));
    pictures.sort();
    let image_rounds = env_n("SEED_IMAGE_ROUNDS", 5);
    for i in 0..env_n("SEED_IMAGE_THREADS", 0) {
        assert!(
            !pictures.is_empty(),
            "SEED_IMAGE_THREADS needs SEED_IMAGE_DIR, a folder of .png files"
        );
        let items = (0..image_rounds)
            .flat_map(|k| {
                let at = now - (image_rounds - k) as i64 * 150_000;
                image_round(i, k, at, &pictures, (i * image_rounds + k) * 3)
            })
            .collect();
        extra.push((format!("Image thread #{i:02}"), items));
    }
    for i in 0..env_n("SEED_ARTIFACTS", 0) {
        let items = (0..4)
            .flat_map(|k| artifact_round(i, k, now - (4 - k) as i64 * 150_000))
            .collect();
        extra.push((format!("Artifact thread #{i:02}"), items));
    }
    let extras = extra.len();
    for (e, (title, items)) in extra.into_iter().enumerate() {
        let th = store
            .create_thread(
                Some(&folders[e % folders.len()]),
                agent.clone(),
                Some("mock-swift".into()),
                Effort::High,
                HandHolding::Auto,
            )
            .unwrap();
        items_made += items.len();
        store
            .save_transcript(
                &th.id,
                &mut trek_core::transcript::Transcript::unsaved(items),
            )
            .unwrap();
        let last = now - 60_000 - e as i64 * 60_000;
        store
            .update_thread(&th.id, |t| {
                t.title = title;
                t.created_at = last - 3_600_000;
                t.updated_at = last;
                t.last_seen_at = last;
            })
            .unwrap();
    }
    let n = n + extras;
    println!(
        "seeded {n} threads ({items_made} items) in {} projects under {} in {:?}",
        PROJECTS.len(),
        dir.display(),
        t.elapsed()
    );
}

/// Round `k` of thread `i`, asked at `at`: the five items of a turn.
fn round(i: usize, k: usize, at: i64) -> [Item; 5] {
    let file = [
        "src/lib.rs",
        "src/server/routes.rs",
        "web/app.tsx",
        "scripts/release.sh",
    ][(i + k) % 4];
    let output: String = (1..=30)
        .map(|l| format!("{l:>4} | let value_{l} = compute(input, {l});\n"))
        .collect();
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
        Item::User {
            text: format!("Round {k}: look at {file} and make the retries saner (thread {i})."),
            images: vec![],
            at: Some(at),
            resume: None,
            aside: false,
        },
        Item::Reasoning {
            text: format!(
                "The retries in {file} hammer the server when it's down; an exponential backoff with a cap fixes that, and the tests need a case for an empty page."
            ),
        },
        Item::Tool {
            id: format!("t{i}-{k}"),
            title: "Read".into(),
            detail: file.into(),
            output,
            status: ToolStatus::Done,
        },
        Item::Assistant { text: answer },
        Item::TurnEnd {
            at: at + 90_000,
            took_secs: 90,
        },
    ]
}

/// A turn of a "Heavy thread": the same five items, with a long answer and a tool call that
/// printed a lot.
fn heavy_round(i: usize, k: usize, at: i64) -> [Item; 5] {
    let output: String = (1..=80)
        .map(|l| format!("{l:>4} | let value_{l} = compute(input, {l}, thread_{i}, round_{k});\n"))
        .collect();
    // Every tenth answer is a very long one (a file written out whole).
    let sections = if k % 10 == 9 { 10 } else { 2 };
    let mut answer = format!("## Round {k} of thread {i}\n\n");
    for s in 0..sections {
        answer.push_str(&format!(
            "### Step {s}: the retry path\n\n\
             The retries in `src/server/routes.rs` hammered the server whenever it was down, so I \
             moved the loop into **one** place and gave it an exponential backoff with a cap. The \
             request id now travels with every error, which lines the log up with the trace, and \
             the empty-page case has a test of its own in `tests/pagination.rs`. Nothing else in \
             the handler changed: the cache headers, the auth check and the rate limiter read as \
             they did before, and the public types are the same.\n\n\
             - The backoff starts at 250 ms and doubles, capped at 30 s.\n\
             - A request that fails five times is given up on, with the last error.\n\
             - Timeouts count as failures; a `429` waits for `Retry-After` when there is one.\n\
             - The loop is cancelled with the request, so nothing retries after a disconnect.\n\n\
             ```rust\n"
        ));
        for f in 0..14 {
            answer.push_str(&format!(
                "/// Attempt {f} of step {s} (thread {i}, round {k}).\n\
                 pub async fn attempt_{s}_{f}(client: &Client, request: &Request) -> Result<Response, Error> {{\n\
                 \x20   let delay = Duration::from_millis(250 * 2u64.pow({f}u32.min(7)));\n\
                 \x20   tokio::time::sleep(delay.min(Duration::from_secs(30))).await;\n\
                 \x20   let response = client.send(request.clone()).await.map_err(|e| Error::with_id(request.id(), e))?;\n\
                 \x20   tracing::debug!(id = %request.id(), status = %response.status(), \"attempt {f}\");\n\
                 \x20   Ok(response)\n\
                 }}\n\n"
            ));
        }
        answer.push_str(
            "```\n\n\
             | check | before | after | note |\n|---|---|---|---|\n\
             | p50 | 120 ms | 80 ms | the cache is warm |\n\
             | p95 | 900 ms | 210 ms | one retry at most |\n\
             | p99 | 4.1 s | 640 ms | two retries |\n\
             | errors | 2.1 % | 0.3 % | over an hour of traffic |\n\n\
             And the client's side of it, which reads the same header:\n\n\
             ```typescript\n",
        );
        for f in 0..10 {
            answer.push_str(&format!(
                "export async function fetchPage{s}x{f}(cursor: string | null): Promise<Page> {{\n\
                 \x20 const response = await request(`/api/items?cursor=${{cursor ?? \"\"}}`, {{ retries: {f} }});\n\
                 \x20 if (!response.ok) throw new RequestError(response.status, response.headers.get(\"x-request-id\"));\n\
                 \x20 return (await response.json()) as Page;\n\
                 }}\n\n"
            ));
        }
        answer.push_str(
            "```\n\n\
             Next I'd look at the cache headers: `ETag` is set but `Cache-Control` isn't, so every \
             page is fetched again on a reload. That's a separate change.\n\n",
        );
    }
    [
        Item::User {
            text: format!(
                "Round {k}: make the retries in src/server/routes.rs saner (heavy thread {i})."
            ),
            images: vec![],
            at: Some(at),
            resume: None,
            aside: false,
        },
        Item::Reasoning {
            text: "The retries hammer the server when it's down; an exponential backoff with a cap fixes that, and the tests need a case for an empty page.".into(),
        },
        Item::Tool {
            id: format!("h{i}-{k}"),
            title: "Read".into(),
            detail: "src/server/routes.rs".into(),
            output,
            status: ToolStatus::Done,
        },
        Item::Assistant { text: answer },
        Item::TurnEnd {
            at: at + 90_000,
            took_secs: 90,
        },
    ]
}

/// A turn of an "Image thread": a message with two pictures attached, and an answer that shows
/// a third. `from`: where in `pictures` this turn's three start.
fn image_round(i: usize, k: usize, at: i64, pictures: &[PathBuf], from: usize) -> [Item; 3] {
    let pick = |j: usize| pictures[(from + j) % pictures.len()].display().to_string();
    [
        Item::User {
            text: format!(
                "Round {k}: here are two screenshots of the settings page (image thread {i}). What's off?"
            ),
            images: vec![pick(0), pick(1)],
            at: Some(at),
            resume: None,
            aside: false,
        },
        Item::Assistant {
            text: format!(
                "## Round {k}\n\nThe sidebar's rows are two points lower than the header's, and the \
                 toggle in the second row has lost its label. After the fix it looks like this:\n\n\
                 ![after]({})\n\n\
                 The rows line up with the header again, and the label is back. I checked both \
                 themes and the narrow window.\n\n\
                 - The row height comes from the scale now, not a constant.\n\
                 - The label's colour follows the theme's muted foreground.\n",
                pick(2)
            ),
        },
        Item::TurnEnd {
            at: at + 60_000,
            took_secs: 60,
        },
    ]
}

/// A turn of an "Artifact thread": an answer with a small live page of its own.
fn artifact_round(i: usize, k: usize, at: i64) -> [Item; 3] {
    [
        Item::User {
            text: format!(
                "Round {k}: show the milestones as something I can click through (artifact thread {i})."
            ),
            images: vec![],
            at: Some(at),
            resume: None,
            aside: false,
        },
        Item::Assistant {
            text: format!(
                "Here it is, live:\n\n```trek-html\n<!doctype html>\n<html><head><title>Milestones {i}.{k}</title>\n\
                 <style>body{{font:var(--trek-font-size) var(--trek-font);color:var(--trek-fg);margin:16px}}\n\
                 li{{padding:6px 0;border-bottom:1px solid var(--trek-border)}}</style></head>\n\
                 <body><h2>Milestones {i}.{k}</h2><ol>\n\
                 <li>Design review</li><li>Prototype</li><li>Beta to ten users</li><li>Release</li>\n\
                 </ol><p id=\"n\">Clicked 0 times</p>\n\
                 <script>let n=0;document.body.onclick=()=>{{document.getElementById('n').textContent='Clicked '+(++n)+' times'}}</script>\n\
                 </body></html>\n```\n\nClick anywhere in it to count.\n"
            ),
        },
        Item::TurnEnd {
            at: at + 60_000,
            took_secs: 60,
        },
    ]
}
