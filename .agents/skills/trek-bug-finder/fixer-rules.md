# Rules for every fixer

- Repo: /Users/tobias/Developer/Trek (Rust + gpui macOS app, Swift iPhone companion, shell and PowerShell scripts). Work in the working tree as it is on disk. It holds uncommitted work by the user; do not revert, stash, commit, or reformat anything you were not asked to touch. Never run `git stash`, `git checkout -- <file>`, `git reset`, `cargo fmt` on whole crates, or any command that rewrites files you don't own.
- Only edit the files your package names, plus a test file for them. If a fix truly needs a change elsewhere, keep it to the minimum and name it in your report.
- Other fixers are editing other files in the same checkout at the same time. If `cargo check`/`cargo test` fails with an error in a file that is not yours, that is their half-finished edit: wait 60 seconds and retry, up to five times. Do not "fix" their file.
- cargo is shared and slow. Use targeted commands only: `cargo check -p <crate>` and `cargo test -p <crate> <test_name_filter>`. Never `cargo test` the whole workspace, never `cargo clean`, never `cargo build --release`. Disk is tight (25 GB free): no new target dirs, no worktrees, no `CARGO_TARGET_DIR`.
- Do not start the app or the verify-trek driver (`trek-dev`). A separate pass verifies the app after all fixers finish.
- For each confirmed finding (status open): fix the root cause, keep the fix minimal and in the style of the surrounding code, and add or extend a test that fails before and passes after. Tests for trek-app live in crates/trek-app/src/tests/ or inline `#[cfg(test)]` modules; trek-core and trek-agents use inline modules. If a test is genuinely impossible (pure UI layout, a PowerShell script on a Mac without pwsh, an iOS view), say so and describe how you checked instead.
- For each unverified finding: assess it first. Default to "not a bug" when unsure, and give the reason. Fix it with a test only if it is clearly real.
- A behaviour that a passing existing test asserts is intended; do not change it, report the conflict instead.
- Before finishing, run `cargo check -p <your crate>` and your tests once more, and make sure you left no debugging output, no stray files, and no `/tmp` paths in the repo.
- Your final message is the report. Use exactly this shape, one block per finding id in your package:

### <id>
verdict: fixed | not-a-bug | could-not-fix
files: <files you changed>
test: <test name(s) added or extended, or "none: <why>">
notes: <one to three sentences: what the root cause was and what you changed; for not-a-bug, the reason; for could-not-fix, what blocks it>

End with a line `cargo: <the exact check/test commands you ran and their final result>`.
