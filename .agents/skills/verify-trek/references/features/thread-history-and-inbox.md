# Thread history and inbox

## What it does

The sidebar is an inbox: "Needs you" lists on top, in its own amber block, every thread waiting on an approval, a question or a plan, every failure, and turns cut short when Trek closed, each with its project and why (a thread that only finished stays in its project, on top with its unread dot), in the words its alert uses; it takes no room when nothing does. Active work follows, then the projects under a "Projects" label (each with its badge), Study's sessions under "Study" (a cap in the state column, the topic, Learn or Practice), other machines and paired phones under "Devices" (a machine shows its unread threads and newest three, the rest behind "Show N more"), and the folded "Settled" section. Every row starts with the same state column: ember loader working, amber dot asked, red failed, emerald unread, sky background work. Finished work can settle, pin, snooze, rename, or delete. The design is the Figma page "07 Sidebar"; its sizes are in `crates/trek-app/src/sidebar/look.rs`. Full-text search finds titles and transcript messages. A message can be edited and resent; turns can retry, rewind files to Trek’s Git checkpoint, or fork. Threads can open in separate windows.

## Reach it as a user

⌘⇧I (Ctrl+Shift+I) goes through what needs you, round and round, and says so when nothing does; ⌘⇧B (Ctrl+Shift+B) goes back to where you last sent a prompt. Both are in ⌘K and Settings › Keyboard Shortcuts. Use the sidebar search or ⌘K, then select a result. Open a thread’s **…** menu for pin, snooze, rename, settle, delete, or separate window. Use message/turn menus inside a transcript for edit, retry, rewind, or fork.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev send "Remember the phrase alpine compass"
./.agents/skills/verify-trek/scripts/trek-dev open first
./.agents/skills/verify-trek/scripts/trek-dev open title:alpine
./.agents/skills/verify-trek/scripts/trek-dev control settled on
```

The shots protocol can route by title but cannot type into the visible search palette or activate transcript context menus; those remain a manual UI/test-platform check. A row's own menu opens with `control rclick live-line-<id>` (or `card-<id>`), then `control click thread-rename` etc.; ⌘E (`control key secondary-e`) settles the thread on screen. Sidebar ids: `needs-you`, `section-active`, `section-projects`, `live-proj-head-<project id>`, `study-sessions`, `devices`, `machine-head-<host>`, `machine-more-<host>` / `machine-less-<host>`, `phone-<device id>`, `settled-toggle`, `inbox-zero`, `search-empty`.

## Code

`crates/trek-app/src/sidebar.rs`, `command_palette.rs`, `thread_view.rs`, `thread_window.rs`, `workspace.rs`; persistence/search/rewind in `crates/trek-core/src/store/` and `rewind.rs`.

## Verify

Search selects the right thread or message, inbox sections update without duplicates, and state survives an isolated stop/start. A rewind changes both transcript and repository files only after confirmation.
