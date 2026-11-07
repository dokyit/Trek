# Thread history and inbox

## What it does

The sidebar is inbox first. Under Basecamp, Notes and Study sits the **Inbox**, a soft card: "Inbox N" with **Mark all read** at its right, then two-line rows. First what needs the user (an approval, a question, a plan, a failure, a sub-agent asking, failing checks, a turn cut short when Trek closed): an amber or red dot, the title in bold, the time, and under it why, in the words its alert used, and its project (a cut-off turn has Resume there, failing checks Send to agent). Then threads that finished unread: an emerald dot, the bold title, "Finished · <project>", the time; past five of those (turns cut short count too) the rest wait behind "Show N more". Opening an Inbox row reads it: a finished one goes back to its project (under the pointer it holds its place till the pointer leaves). Mark all read reads every finished one and leaves what needs the user. With nothing in it the Inbox isn't there at all, and while a search is on there's none (unread threads show in their projects with their dot). A thread in the Inbox leaves its project's list while it's there, though the project's header still counts it (a project whose only threads are in the Inbox shows its header alone).

Then **Projects N**: each project's badge, name and count, and its other threads one line each: pinned ones first (a pin in the state column), then what's at work (the ember loader in the state column, its elapsed time in ember in the right-hand column, "4m 12s"; waiting on its sub-agents, how long it's waited), then the rest newest first, three on show and the others behind "Show N more" (unread ones and the open one always show). Folding a project tucks its quiet threads away; what's pinned or at work stays. There's no Active, Pinned or Snoozed section: a snoozed thread sits in its project with a clock. Then Study's sessions under "Study N" (a cap in the state column, the topic, Learn or Practice at the right), **Devices N** (other machines, each with its glyph, name and count and its threads under it the way a project's are, at work with their clock; the paired iPhone with "2m ago", "Just now", "Connected" or "Paired 3d" at the right), and **Settled N** at the end, folding (its chevron after the word), grouped by project when open. Every row starts with the same state column (ember loader working, amber asked, red failed, emerald unread, sky background work, pin, clock, cap) and ends on one right-hand column: every count, time and device word ends at the same x (a project header's "New thread here" lies over its count on hover instead of keeping room for itself). No ticks hang off the rows' left edge. Finished work can settle, pin, snooze, rename, or delete. The design is the Figma page "11 Sidebar options", Option 1 · Inbox first (frame 106:1798, Paper 106:1823); its sizes are in `crates/trek-app/src/sidebar/look.rs`. Full-text search finds titles and transcript messages. A message can be edited and resent; turns can retry, rewind files to Trek’s Git checkpoint, or fork. Threads can open in separate windows.

## Reach it as a user

⌘⇧I (Ctrl+Shift+I) goes through what needs you, round and round, and says so when nothing does; ⌘⇧B (Ctrl+Shift+B) goes back to where you last sent a prompt. Both are in ⌘K and Settings › Keyboard Shortcuts. Use the sidebar search or ⌘K, then select a result. Open a thread’s **…** menu for pin, snooze, rename, settle, delete, or separate window. Use message/turn menus inside a transcript for edit, retry, rewind, or fork.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev send "Remember the phrase alpine compass"
./.agents/skills/verify-trek/scripts/trek-dev open first
./.agents/skills/verify-trek/scripts/trek-dev open title:alpine
./.agents/skills/verify-trek/scripts/trek-dev control settled on
```

The shots protocol can route by title but cannot type into the visible search palette or activate transcript context menus; those remain a manual UI/test-platform check. A row's own menu opens with `control rclick live-line-<id>` (or `inbox-<id>` for an Inbox row), then `control click thread-rename` etc.; ⌘E (`control key secondary-e`) settles the thread on screen. Sidebar ids: `inbox` (its header; absent when empty), `inbox-mark-read`, `inbox-more` / `inbox-less`, `inbox-<id>` with `inbox-needs-<id>`, `inbox-failed-<id>`, `inbox-finished-<id>`, `inbox-sub-needs-<id>`, `inbox-checks-failed-<id>`, `inbox-send-checks-<id>`, `inbox-resume-<id>`, `inbox-time-<id>`; `section-projects`, `live-proj-head-<project id>`, `live-proj-count-<project id>`, `live-fold-<project id>`, `live-proj-new-<project id>`, `live-more-<project id>` / `live-less-<project id>`; a project row `live-line-<id>` with `live-line-working-<id>` (its clock), `live-line-waiting-<id>`, `live-line-time-<id>`, `live-line-kids-<id>`, `live-line-at-work-<id>`, `live-line-unseen-<id>`, `live-line-pinned-<id>`, `live-line-snoozed-<id>`, `live-line-paused-<id>`, `live-line-resume-<id>`; `study-sessions`, `devices`, `machine-head-<host>`, `machine-count-<host>`, `machine-more-<host>` / `machine-less-<host>`, `phone-<device id>`, `phone-seen-<device id>`, `settled-toggle`, `settled-count`, `inbox-zero`, `search-empty`.

To see each state in the isolated app: `send "mock:permission please"` (an approval in the Inbox), `send "mock:long 600s"` (a thread at work with its clock), a reply that finishes while another thread is open (an unread one in the Inbox), `control settled on`, `control theme paper`, and the machines demo in `machines.md` for Devices.

## Code

`crates/trek-app/src/sidebar.rs`, `command_palette.rs`, `thread_view.rs`, `thread_window.rs`, `workspace.rs`; persistence/search/rewind in `crates/trek-core/src/store/` and `rewind.rs`.

## Verify

Search selects the right thread or message, a thread is in the Inbox or its project and never both, the Inbox goes when it's empty, counts and times end on one right-hand column (`cargo test -p trek-app --test sidebar`), and state survives an isolated stop/start. A rewind changes both transcript and repository files only after confirmation.
