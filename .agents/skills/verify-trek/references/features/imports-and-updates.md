# Imports and updates

## What it does

Reads existing Claude Code, Codex, and OpenCode history without changing their stores, filters non-conversation sessions, and can resume supported threads. The updater checks signed Stable/Beta/Nightly releases, downloads in the background, verifies them, and installs on restart/quit without interrupting an agent.

## Reach it as a user

Open **Settings → Import Threads** to enable providers and review skipped sessions. Open **Settings → Updates** to choose a channel, check, download, or restart into a ready update.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open settings:import-threads
./.agents/skills/verify-trek/scripts/trek-dev screenshot imports
./.agents/skills/verify-trek/scripts/trek-dev open settings:updates
./.agents/skills/verify-trek/scripts/trek-dev screenshot updates
```

Imports and automatic updates are disabled in the isolated profile to protect real history and avoid network/system mutation. Import parsers are covered by Rust tests; updater E2E uses `script/update-e2e.sh` and the procedures in `docs/RELEASING.md`.

## Code

`crates/trek-app/src/settings_view.rs`, `workspace.rs`, updater UI in `settings_view/pages.rs`; import and updater engines in `crates/trek-core/src/import/` and `update.rs`.

## Verify

Import preview/classification matches provider history without modifying it; resume targets the selected source session. Updates reject bad signatures/hashes, wait for running agents, preserve launch environment, and roll forward only after explicit restart/quit.

