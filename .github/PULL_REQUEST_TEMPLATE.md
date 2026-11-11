## What this changes

<!-- What someone using Trek will notice. Link the issue: "Fixes #123". -->

## Why

<!-- What was wrong or missing. -->

## How it was checked

<!-- Tests added or run, and how you tried it in the app (the verify driver, the mock agent).
     For UI changes, add a before/after screenshot. -->

- [ ] `cargo fmt --all` and `cargo test --workspace` pass
- [ ] Checked on macOS
- [ ] Checked on Windows (or not affected)
- [ ] Docs updated where behaviour changed

## The other platform

<!-- A fix or a feature made on one platform: what happens on the other? Same code (say which
     test covers both), done there too, or a gap recorded in docs/parity/ledger.toml with why.
     A change to a platform gate (`cfg(windows)`, `cfg!(target_os = "macos")`) fails
     `cargo test -p trek-parity` until the ledger says what each platform does and the counts are
     recorded: TREK_PARITY_UPDATE=1 cargo test -p trek-parity. See docs/CROSS_PLATFORM.md. -->

- [ ] Same on both platforms, done on both, or the difference is in docs/parity/ledger.toml
