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
- **Files a crate reads.** A file any crate names in a string that ends with its path (`include_str!`,
  `include_bytes!`, rust-embed's folder, a `CARGO_MANIFEST_DIR` join, `../` in a test), in or out of
  `crates/`, reaches that crate: trek-words' test reads trek-app's sources, trek-palette includes
  trek-assets' theme, trek-core reads the iPhone app's `LivePage.swift`. One embedded is compiled in,
  so what depends on the crate runs too; one read at run time runs that crate alone. Tests that build
  their paths piece by piece are listed in `WALKERS` (a plan test fails when a new one isn't), and
  programs a crate starts without depending on them in `LAUNCHES` (trek-app starts trek-mcp).
- **Moved files** count at both paths (`git diff --no-renames`).
- Docs, the site, `ios/` and the like that no crate reads build nothing; scripts run the script tests
  (`Scripts (…)`). There are no path filters on the triggers: a push to main of the docs alone runs
  the Plan job and nothing else.

Jobs the plan leaves out are skipped and report as passed under their own names. **CI result** is the
one check that sums the run up: it fails when any job failed or was cancelled. It is the check to require
on `main` and the one an agent waits for before merging (`gh pr checks <n> --watch --interval 120`).

A partial run doesn't count for the release gate (its record's scope is `partial`): after the merge,
`main`'s own run (full) does, or label the pull request `ci:full` before merging so the gate can take
its run when nothing else reached `main` in between. **The label is read from the push's event**:
add it, then push (an empty commit will do, `git commit --allow-empty`); re-running a run reuses its
old event and its old labels.

## The jobs

| Job | What | When |
| --- | --- | --- |
| Plan | the plan, the tested-tree record, rustfmt, shard coverage | always |
| Test (macOS / Windows / Linux) | `cargo nextest` over the affected crates but trek-app | a crate other than trek-app is affected |
| Test (…, trek-app N/M) | trek-app's test targets, split by `script/ci-shard.py` | trek-app is affected |
| Doc tests (Linux) | `cargo test --doc` of the affected crates | any crate is affected |
| (macOS, Windows) | the same, in the jobs that test those crates: the workspace job, and trek-app's first shard | those crates are affected |
| Clippy (macOS / Windows / Linux) | correctness and suspicious lints of the affected crates, the capture build | any crate is affected on that platform |
| Packages (Linux) | the debug build packaged as AppImage, .deb and tarball, and opened | trek-app is affected |
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

## The cache budget

GitHub keeps 10 GB of Actions caches per repository and evicts the least recently used past that,
within minutes when runs are busy. One cache per kind of job per platform comes to about that on its
own: per platform about 1–1.5 GB for `test-workspace`, 1–1.2 GB for `test-app` and 0.6–0.9 GB for
`clippy` (with no debug info, workspace libraries kept, executables and trek-app pruned), 0.55 GB
for the Arm check, and Live's three (`live`, 1.5–1.7 GB each) and the release's (`warm-release`,
~0.9 GB) on top. Measured on 2026-11-28, a macOS trek-app cache saved at 03:13 was gone by 03:26.
A missed cache makes a job three times as long, so:

- Only `main` saves caches (pull requests read them), one job of each kind, and the jobs save before
  builds that resolve features of their own (doc tests, the CoreDevice example, the Linux packages).
- The workflow raises nothing itself: **raising the repository's cache limit** (Settings › Actions ›
  Caches, where the plan allows more than 10 GB) is what lets every kind stay warm; about 25 GB fits
  all of them with Live's.

**When the toolchain is bumped**, check that Cargo still lays target/ out as
`target/<profile>/{deps,.fingerprint,build}` (`script/ci-cache.py` prunes and keeps units by that
layout, and `script/tests/ci-cache.py` runs on the pinned toolchain): newer Cargo's build-dir layout
(`target/debug/build/<crate>/<hash>/…`) would need prune taught it.

## Runners

Every job names its runner in its own `runs-on` (`macos-15`, `windows-2022`, `ubuntu-22.04`,
`ubuntu-22.04-arm`); the setup every Rust job shares is `.github/actions/rust-setup`. Moving to another
runner provider is one line per job.
