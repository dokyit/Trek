# Git and turn changes

## What it does

Shows repository status and diffs beside the thread, including the exact file changes from one completed turn. It can stage/commit all changes, push, create a pull request, initialize a repo, and manage Trek-created worktrees.

## Reach it as a user

Open the right panel with ⌘J and select **Source Control**. Select files/hunks to inspect; use the footer actions for commit/push/PR. Open a completed turn’s changes from its transcript control.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev project .
./.agents/skills/verify-trek/scripts/trek-dev open tool:git
./.agents/skills/verify-trek/scripts/trek-dev screenshot git-panel
```

To see the breakdown of a "+N −M" count by kind of file (Code, Tests, Docs, Config, ... and a Total), run `trek-dev control hover git-lines` with the Git tool open (or `changes-lines-<ix>` on a turn's changes card, `diff-lines` in the IDE diff, `gf-lines-<path>` on a file row) and screenshot; the card is `<id>-card`, its rows `<id>-code`, `<id>-tests`, ... and `<id>-total`.

After a completed mock turn, `control diff` opens that turn’s change view when it has a recorded checkpoint. The verification CLI deliberately does not expose commit, push, PR, or worktree deletion.

## Code

`crates/trek-app/src/panels/git.rs`, `line_breakdown.rs` (with `trek-core/src/file_kind.rs`), `workspace/turn_changes.rs`, `workspace/worktrees.rs`, `crates/trek-app/src/remote/git.rs`.

## Verify

Branch/upstream and file states match `git status`; selecting a file shows the correct diff. Mutating Git actions require a disposable repository and explicit manual confirmation.

