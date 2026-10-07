# Thread history and inbox

## What it does

The sidebar is an inbox: active and waiting work stays prominent; finished work can settle, pin, snooze, rename, or delete. Full-text search finds titles and transcript messages. A message can be edited and resent; turns can retry, rewind files to Trek’s Git checkpoint, or fork. Threads can open in separate windows.

## Reach it as a user

Use the sidebar search or ⌘K, then select a result. Open a thread’s **…** menu for pin, snooze, rename, settle, delete, or separate window. Use message/turn menus inside a transcript for edit, retry, rewind, or fork.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev send "Remember the phrase alpine compass"
./.agents/skills/verify-trek/scripts/trek-dev open first
./.agents/skills/verify-trek/scripts/trek-dev open title:alpine
./.agents/skills/verify-trek/scripts/trek-dev control settled on
```

The shots protocol can route by title but cannot type into the visible search palette or activate transcript context menus; those remain a manual UI/test-platform check.

## Code

`crates/trek-app/src/sidebar.rs`, `command_palette.rs`, `thread_view.rs`, `thread_window.rs`, `workspace.rs`; persistence/search/rewind in `crates/trek-core/src/store/` and `rewind.rs`.

## Verify

Search selects the right thread or message, inbox sections update without duplicates, and state survives an isolated stop/start. A rewind changes both transcript and repository files only after confirmation.
