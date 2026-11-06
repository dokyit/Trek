# Imports and updates

## What it does

Reads existing Claude Code, Codex, and OpenCode history without changing their stores, filters non-conversation sessions, and can resume supported threads. The updater checks signed Stable/Beta/Nightly releases, downloads in the background, verifies them, and installs on restart/quit without interrupting an agent. With threads running it says how many and offers "Installs when N threads finish" (the default), "Install now" (says it stops them first) and "Later".

Coming from another app: when Conductor, T3 Code or Orca left worktrees on this computer, **Settings → Import Threads** and onboarding's last step offer them ("Coming from another app"; shown only then). **Review…** shows what would come across, with counts and what's left out, before anything happens. **Bring over** makes each repository a project and each agent session in a worktree a thread (found by the importers above, whatever "How far back" says; an importer turned off stays off), with one thread to carry on in for a worktree that has none. Each worktree stays where it is on its branch, and only reads happen: folders, `.git` files, and Conductor's `conductor.db`. Threads already imported from those folders move under the repository's project. Those worktrees stay the other app's: nothing in Trek removes one or makes it again, and archiving or deleting a thread leaves it.

After a relaunch, chats a quit, an update or a crash cut off are offered back: "N threads were interrupted" above the sidebar's footer with Resume all, Resume on a row, and a bar above the composer with Resume, Edit message, Dismiss and, if it fails, Retry. A chat waiting on an approval comes back waiting (Ask again); one stopped or archived on purpose stays that way. Nothing runs by itself unless **Settings → General → Resume interrupted threads at launch** is on. Messages not sent, and those queued behind a turn, come back in their composers.

## Reach it as a user

Open **Settings → Import Threads** to enable providers and review skipped sessions. Open **Settings → Updates** to choose a channel, check, download, or restart into a ready update. The sidebar's update pill opens the same choices.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open settings:import-threads
./.agents/skills/verify-trek/scripts/trek-dev screenshot imports
./.agents/skills/verify-trek/scripts/trek-dev open settings:updates
./.agents/skills/verify-trek/scripts/trek-dev screenshot updates
# An update ready with a chat running (a dev build can't get one any other way):
./.agents/skills/verify-trek/scripts/trek-dev send "mock:long 600s"
./.agents/skills/verify-trek/scripts/trek-dev control update-demo ready   # or wait, later, clear
# What Conductor and T3 Code leave on disk, made up in the disposable data folder (real git worktrees,
# a database with Conductor's tables, Claude Code sessions). Then look at the offer and bring it over.
./.agents/skills/verify-trek/scripts/trek-dev control import-demo
./.agents/skills/verify-trek/scripts/trek-dev open settings:import-threads
./.agents/skills/verify-trek/scripts/trek-dev control click worktree-app-conductor
./.agents/skills/verify-trek/scripts/trek-dev control click worktree-app-adopt
# A chat cut off, for real: stop the app mid-turn and start it again.
./.agents/skills/verify-trek/scripts/trek-dev stop
./.agents/skills/verify-trek/scripts/trek-dev start
# States a mock turn passes too fast to catch, on the thread on screen:
./.agents/skills/verify-trek/scripts/trek-dev control resume-demo resuming   # or failed, offered, waiting
```

Imports and automatic updates are disabled in the isolated profile to protect real history and avoid network/system mutation: it never looks in the real home folder for other apps' worktrees either, only in `import-demo`'s. Import parsers are covered by Rust tests; updater E2E uses `script/update-e2e.sh` and the procedures in `docs/RELEASING.md`.

## Code

`crates/trek-app/src/settings_view.rs`, `workspace.rs`, updater UI in `settings_view/pages.rs`; import and updater engines in `crates/trek-core/src/import/` and `update.rs`. Other apps' worktrees: `crates/trek-core/src/import/worktree_apps.rs`, `store/adopted.rs`, `crates/trek-app/src/workspace/worktree_apps.rs`, `worktree_apps_ui.rs`, `settings_view/coming_from.rs`; tests in `crates/trek-app/tests/worktree_apps.rs`. What an update waits for: `crates/trek-updater/src/wait.rs`. Chats offered back: `crates/trek-core/src/interrupted.rs`, `crates/trek-core/src/store/restart.rs`, `crates/trek-app/src/workspace/resume.rs`; messages kept: `workspace/unsent.rs`. Tests: `crates/trek-app/tests/resume.rs`.

## Verify

Import preview/classification matches provider history without modifying it; resume targets the selected source session. Bringing worktrees over changes no file under the home folder, and offers nothing a second time. Updates reject bad signatures/hashes, wait for running agents, preserve launch environment, and roll forward only after explicit restart/quit. The install itself comes last, with the relaunch right behind it: never install and then wait, as a bundle replaced under a running Trek loses its macOS permissions until it restarts. A resume never replays a tool call.

