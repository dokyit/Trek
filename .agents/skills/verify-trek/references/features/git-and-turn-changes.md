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

Comment on a diff (the Git tool's and the editor's diff tabs): hover a line for its "+", or press `c`; drag or `shift-j`/`shift-k` selects a range. Comments wait as drafts on the thread (`diff_review.rs`, `workspace/diff_notes.rs`, kept in the store) in the strip under the diff: open it to edit or remove each, Clear (with an Undo) or Send to agent (`⌘↩`), one message through the normal send path. Keys while the diff itself has focus: `j`/`k` line, `n`/`p` hunk, `]`/`[` file, `v` viewed, `s` stage or unstage the hunk (editor diffs of Source Control only), `c` comment, `esc` clear. Elements: `diff-comment-input`, `diff-comment-save`, `diff-comment-<n>`, `diff-comments`, `diff-comments-toggle`, `diff-comments-clear`, `diff-comments-send`, `diff-line-plus-<row>`, `gf-viewed-<path>`, `git-viewed`, `diff-viewed`, `diff-viewed-file-<n>`, `diff-stage-hunk-<key>`. `control key "n j c"` then `control type <text>` and `control key enter` drive it; click a file (or `control diff`) first so the diff has focus.

When a pull request a thread opened has a failing check, the thread's Inbox row says so ("CI failed: clippy") and offers **Send to agent**, as does a bar above its composer (`checks-bar`, `checks-send`, `checks-open`, `checks-dismiss`) and the PR card (`pr-send-failure-<ix>`). It sends one message through the normal send path: the check's name and the end of its log, from `gh run view --log-failed`. It never goes by itself unless **Project Settings → Send failed checks to the agent** is on, and then at most twice per pull request, after which the row and the bar say Trek stopped. The PR card follows a PR closely for an hour after something happens; a slower look (`workspace/pr_checks.rs`) takes over from there and across restarts, one PR at a time, less often the longer nothing changes, and nothing at all goes to GitHub while offline or with little of its rate limit left. `trek-dev control checks-demo failed` gives the thread on screen such a PR, from a made-up `gh` in the disposable data folder; also `failed-more`, `again` (a new commit fails, each time: with `auto` on, three in a row reach the limit), `pending`, `passing`, `auto`, `off`, `offline`. Tests: `crates/trek-app/tests/pr_checks.rs`.

After a completed mock turn, `control diff` opens that turn’s change view when it has a recorded checkpoint. The verification CLI deliberately does not expose commit, push, PR, or worktree deletion.

## Code

`crates/trek-app/src/panels/git.rs`, `ide/diff_view.rs`, `diff_review.rs`, `line_breakdown.rs` (with `trek-core/src/file_kind.rs`), `workspace/turn_changes.rs`, `workspace/worktrees.rs`, `workspace/pr_checks.rs`, `composer/checks_bar.rs`, `pr_card.rs`, `crates/trek-app/src/remote/git.rs`.

## Verify

Branch/upstream and file states match `git status`; selecting a file shows the correct diff. A comment's message names the file and line, quotes a little code and carries the words; keys never fire while typing in a text field; a file marked viewed comes back unviewed when its diff changes. Mutating Git actions require a disposable repository and explicit manual confirmation.

