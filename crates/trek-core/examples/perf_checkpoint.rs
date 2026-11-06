//! What a file checkpoint costs in a real repository (`agent-cli perf checkpoint`): the same
//! snapshot a turn takes (`trek_core::checkpoint`), timed, and its refs deleted again. Prints how
//! long the first one took, then the median of the rest, as one line of JSON at the end.
//!
//! A snapshot never touches the user's index, HEAD or branches. It does leave a few unreachable
//! objects in the repository, as any turn does, for `git gc` to collect.
//!
//! Run with `cargo run --release -p trek-core --example perf_checkpoint -- <repo> [runs]`.
use std::path::PathBuf;
use std::time::Instant;
use trek_core::checkpoint::Repo;

const THREAD: &str = "perf-checkpoint";

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().map(PathBuf::from).expect("give a repository");
    let runs: usize = args.next().and_then(|r| r.parse().ok()).unwrap_or(5).max(1);
    let repo = Repo::open(&dir).expect("a git repository");
    let mut took = Vec::with_capacity(runs);
    let mut items = Vec::with_capacity(runs);
    // A snapshot that fails (past `SNAPSHOT_LIMIT`, say) is timed to its failure, as a turn waited
    // for it, and said.
    let mut failed = None;
    for run in 0..runs {
        let item = format!("run-{run}");
        let started = Instant::now();
        if let Err(e) = repo.snapshot(THREAD, &item) {
            failed = Some(format!("{e:#}"));
        }
        took.push(started.elapsed().as_secs_f64() * 1000.);
        items.push(item);
    }
    repo.delete(THREAD, &items).expect("its refs deleted");
    let first = took[0];
    let mut rest = took[1.min(took.len() - 1)..].to_vec();
    rest.sort_by(f64::total_cmp);
    let p50 = rest[(rest.len() - 1) / 2];
    let files = trek_core::procs::command("git")
        .args(["ls-files"])
        .current_dir(&repo.top)
        .output()
        .map(|o| o.stdout.iter().filter(|b| **b == b'\n').count())
        .unwrap_or(0);
    let failed = failed.map_or("null".to_string(), |e| format!("{e:?}"));
    println!(
        "{{\"files\":{files},\"first_ms\":{first:.0},\"p50_ms\":{p50:.0},\"runs_ms\":{took:.0?},\"failed\":{failed}}}"
    );
}
