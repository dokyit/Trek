# File Explorer and Terminal

## What it does

The Explorer browses the current project tree and opens files. The Terminal supplies real PTY-backed shell tabs in the project folder.

## Reach it as a user

Press ⌘J, then choose **Files** or **Terminal** from the right-panel toolbar. Select folders/files in Explorer. Create, switch, and close terminal tabs from the panel header.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev project .
./.agents/skills/verify-trek/scripts/trek-dev open tool:explorer
./.agents/skills/verify-trek/scripts/trek-dev screenshot explorer
./.agents/skills/verify-trek/scripts/trek-dev open tool:terminal
./.agents/skills/verify-trek/scripts/trek-dev screenshot terminal
```

The shots protocol opens panels but does not click tree nodes or type shell commands.

## Code

`crates/trek-app/src/panels/explorer.rs`, `panels/terminal.rs`, `panels/mod.rs`.

## Verify

Explorer reflects the selected project and file changes; Terminal starts in the project folder, renders output/input correctly, resizes its PTY, and closes child shells with their tabs.

