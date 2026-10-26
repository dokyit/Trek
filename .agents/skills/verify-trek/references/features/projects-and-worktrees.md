# Projects and worktrees

## What it does

Groups threads by working folder, remembers per-project defaults, supplies the title bar's hub (the Overview button: Open in, Run, the branch, Pull, the PR, context, usage, sources and lineage), and can start a thread in an isolated Git worktree. Trek tracks the worktree’s branch and supports returning work to the main folder.

## Reach it as a user

Use **Add project** in the sidebar, pick a folder (or clone a repository), and open its **Project Settings** to change name, defaults, worktree copy paths, and actions. On a new thread in a Git project, enable **Worktree** before sending.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev project .
./.agents/skills/verify-trek/scripts/trek-dev open settings:project
./.agents/skills/verify-trek/scripts/trek-dev screenshot project-settings
```

This adds only the current checkout to the disposable profile. The command protocol does not toggle the worktree composer control. `trek-dev control click hub` opens the hub; its rows are `hub-open`, `hub-run`, `hub-branch`, `hub-pull`, `hub-pr`, `hub-changes`, `hub-context`, and carets `hub-open-more`, `hub-run-more`, `hub-pull-more`.

## Code

`crates/trek-app/src/sidebar.rs`, `hub.rs`, `workspace/worktrees.rs`, `settings_view.rs`, `settings_view/pages.rs`, `root.rs`; project storage in `crates/trek-core/src/store/`.

## Verify

The project appears once, drafts use its folder/defaults, actions run in that folder, and a worktree thread displays a distinct branch/path without altering another checkout.

