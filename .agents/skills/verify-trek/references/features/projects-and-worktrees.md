# Projects and worktrees

## What it does

Groups threads by working folder, remembers per-project defaults, supplies title-bar actions, and can start a thread in an isolated Git worktree. Trek tracks the worktree’s branch and supports returning work to the main folder.

## Reach it as a user

Use **Add project** in the sidebar, pick a folder (or clone a repository), and open its **Project Settings** to change name, defaults, worktree copy paths, and actions. On a new thread in a Git project, enable **Worktree** before sending.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev project .
./.agents/skills/verify-trek/scripts/trek-dev open settings:project
./.agents/skills/verify-trek/scripts/trek-dev screenshot project-settings
```

This adds only the current checkout to the disposable profile. The command protocol does not toggle the worktree composer control.

## Code

`crates/trek-app/src/sidebar.rs`, `workspace/worktrees.rs`, `settings_view.rs`, `settings_view/pages.rs`, `root.rs`; project storage in `crates/trek-core/src/store/`.

## Verify

The project appears once, drafts use its folder/defaults, actions run in that folder, and a worktree thread displays a distinct branch/path without altering another checkout.

