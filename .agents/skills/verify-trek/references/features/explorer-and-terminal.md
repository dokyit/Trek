# File Explorer and Terminal

## What it does

The Explorer browses the current project tree and opens files. The Terminal supplies real PTY-backed shells in the project folder: a dock along the bottom of the chat column (under the composer, as T3 keeps it), which comes back as it was left (shown or hidden, and its height), and a tab of the right panel beside the thread.

## Reach it as a user

Press ⌘J, then choose **Files** from the right-panel toolbar. Select folders/files in Explorer.

Press ⌃` or click **Terminal** in the composer's status row to show or hide the dock under the chat; its header has + (another shell, then tabs), the bin (close the shell; the dock goes with the last) and × (hide, shells kept). Drag its top edge to resize, double-click the edge for the default height. The right panel's launcher still offers a **Terminal** tile for a shell beside the thread, with tabs in the panel header.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev project .
./.agents/skills/verify-trek/scripts/trek-dev open tool:explorer
./.agents/skills/verify-trek/scripts/trek-dev screenshot explorer
./.agents/skills/verify-trek/scripts/trek-dev open tool:terminal
./.agents/skills/verify-trek/scripts/trek-dev screenshot terminal
```

`tool:terminal` shows the dock under the chat (on a thread or draft route). The shots protocol opens panels but does not click tree nodes or type shell commands.

## Code

`crates/trek-app/src/panels/explorer.rs`, `panels/terminal.rs` (the shell view), `terminal_dock.rs` (the dock), `panels/mod.rs` (the right panel's tab).

## Verify

Explorer reflects the selected project and file changes; Terminal starts in the project folder, renders output/input correctly, resizes its PTY, and closes child shells with their tabs. The dock sits under the composer the column wide, ⌃` and the composer's Terminal toggle it, and its shells survive hiding.

