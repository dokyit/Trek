# CI

`.github/workflows/ci.yml` builds and tests Trek on macOS, Windows and Linux. This page says what a
run tests, how to make it test everything, and why it is shaped the way it is. The release gate that
reads CI's runs is in docs/RELEASING.md ("The CI gate"); the live checks are `live.yml`
(docs/CROSS_PLATFORM.md).

## What a run tests

The first job, **Plan** (`script/ci-plan.py`, tests in `script/tests/ci-plan.py`), decides:

- **A push to `main`, a dispatch, or a pull request labelled `ci:full`**: the full suite, every crate on
  every platform. So does a pull request that touches what every crate builds on: the workspace
  `Cargo.toml` or `Cargo.lock`, `rust-toolchain.toml`, `vendor/`, `.github/`, any `build.rs`,
  `crates/test-fixtures/`, the scripts CI itself runs (`script/ci-*`, `script/linux/`,
  `script/bundle-linux.sh`), or a file outside the crates whose readers it can't tell.
- **Any other pull request**: the crates its files belong to and every crate that depends on them
  (`cargo metadata`'s dependency lists, followed in reverse), plus any crate whose sources name a
  changed file outside the crates (`docs/parity/ledger.toml` is trek-parity's, `ios/Trek/Views/LivePage.swift`
  trek-core's). A change to trek-app alone tests trek-app alone; a change to trek-core tests most of
  the workspace.
- **Platforms**: a platform is left out only when every changed crate file is a module whose `mod` line
  (or an enclosing module's) carries a `#[cfg(...)]` that is false there (`#[cfg(windows)] mod win;`).
  Anything the check can't follow counts for every platform.
- Docs, the site, `ios/` and the like build nothing; scripts run the script tests (`Scripts (…)`).

Jobs the plan leaves out are skipped and report as passed under their own names. **CI result** is the
one check that sums the run up: it fails when any job failed or was cancelled. It is the check to require
on `main` and the one an agent waits for before merging (`gh pr checks <n> --watch --interval 120`).

A partial run doesn't count for the release gate (its record's scope is `partial`): after the merge,
`main`'s own run (full) does, or label the pull request `ci:full` before merging so the gate can take
its run when nothing else reached `main` in between.

## The jobs

| Job | What | When |
| --- | --- | --- |
| Plan | the plan, the tested-tree record, rustfmt, shard coverage | always |
| Test (macOS / Windows / Linux) | `cargo nextest` over the affected crates but trek-app | a crate other than trek-app is affected |
| Test (…, trek-app N/M) | trek-app's test targets, split by `script/ci-shard.py` | trek-app is affected |
| Doc tests (Linux) | `cargo test --doc` of the affected crates | any crate is affected |
| Doc tests (Windows), and in Test (macOS) | every crate's doc tests | full runs |
| Clippy (macOS / Windows / Linux) | correctness and suspicious lints of the affected crates, the capture build | any crate is affected on that platform |
| Check (Linux, Arm) | `cargo check` of trek-app and trek-mcp on Arm | either is affected |
| Rust dependency audit | `cargo audit` | `Cargo.lock` changed, or full runs |
| Scripts (macOS / Windows / Linux) | the scripts' own tests | a script or what they test changed, or full runs |
| CI result | every job above passed or was skipped | always |

The Intel Mac check (`Clippy`'s last step) runs on full runs only.

## Why it is fast

Measured over the last 30 successful runs before this design (2026-11-27), the CI workflow's wall time
had a median of 76 min (pull requests 79, pushes 58) and a maximum of 137. Where it went:

- **The cache missed half the time.** The repository's caches came to 13.0 GB against GitHub's 10 GB
  limit, so they evicted each other: hit rates of 43–80% per job, and a miss roughly tripled a job
  (Test (macOS) 10 min on a hit, 29 on a miss).
- **Every hit still rebuilt a third of the graph.** The registry's sources aren't cached; Cargo unpacks
  them again with new times, and ring's build script watches its sources, so ring and everything on
  rustls, reqwest and quinn compiled again on every job. And the workspace's own crates were never
  cached.
- **Mac runners queue.** A run took five macOS runners at once (plus Live's); the account has few, so a
  macOS job waited a median 10–17 min to start, up to two hours.
- **Tests ran one binary after another** (`cargo test`), and each of trek-app's 87 test binaries links
  the whole app (~170 MB each with debug info).

What changed: only what the change reaches is built (Plan); the cache keeps the workspace's libraries
and dates unchanged files and registry sources so Cargo finds them fresh (`script/ci-cache.py`); no debug
info in CI builds; `cargo nextest` runs each job's binaries side by side (`.config/nextest.toml`); doc
tests run on Linux except on full runs; and a typical pull request starts three or four macOS jobs instead
of six.

The numbers per job, before and after, are in the pull request that made this change.

## Runners

Every job names its runner in its own `runs-on` (`macos-15`, `windows-2022`, `ubuntu-22.04`,
`ubuntu-22.04-arm`); the setup every Rust job shares is `.github/actions/rust-setup`. Moving to another
runner provider is one line per job.
