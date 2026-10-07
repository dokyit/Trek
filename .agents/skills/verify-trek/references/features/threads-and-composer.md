# Threads and composer

## What it does

Runs coding agents in a shared conversation UI. A draft chooses project, agent, model, effort, hand-holding, plan mode, and optional worktree. The composer supports queued or steering follow-ups, `/` commands, `@` files, `$` skills, pasted/dropped/picked images, and app snapshots. Permission, question, and plan requests appear as answerable cards.

## Reach it as a user

Click **New thread** or press ⌘N, choose a project and agent options, type in the bottom composer, then press Return. Open the composer’s option controls for model, effort, plan, worktree, or access level. Use the **+** menu for files, photos, and snapshots.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open draft
./.agents/skills/verify-trek/scripts/trek-dev send "A short markdown answer"
./.agents/skills/verify-trek/scripts/trek-dev screenshot thread
```

Prompts containing `tools`, `permission`, `question`, `plan`, `error`, `mock:long 5s`, `mock:stream 5s`, or `mock:visualization` exercise those mock-agent paths. The isolated profile cannot invoke a real vendor CLI.

## Code

`crates/trek-app/src/composer.rs`, `thread_view.rs`, `workspace.rs`, `workspace/agent_updates.rs`; agent sessions in `crates/trek-agents/src/` and mock scripts in `mock.rs`.

## Verify

The sent message appears once, the mock response streams and reaches idle, controls remain usable, and `logs --grep error` is empty. For permission/question/plan prompts, use `control wait <condition>` and capture the visible request card.
