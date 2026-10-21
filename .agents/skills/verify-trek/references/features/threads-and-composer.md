# Threads and composer

## What it does

Runs coding agents in a shared conversation UI. A draft chooses project, agent, model, effort, hand-holding, plan mode, and optional worktree. The composer supports queued or steering follow-ups, `/` commands, `@` files, `$` skills, pasted/dropped/picked images, app snapshots, and (Mac, bundled builds) voice mode. Permission, question, and plan requests appear as answerable cards.

## Reach it as a user

Click **New thread** or press ⌘N, choose a project and agent options, type in the bottom composer, then press Return. Open the composer’s option controls for model, effort, plan, worktree, or access level. Use the **+** menu for files, photos, and snapshots.

Voice mode (Mac): the mic button shows only in a bundle whose Info.plist has the microphone and speech usage keys (or with `TREK_DICTATE=1`). Pressing it asks for both permissions, then swaps the toolbar row for a scrolling bar waveform with a ✕ and a ✓. The text area reads "Listening…" until words come; what the recognizer hears shows at the caret in italic, muted text and is revised live. ✓ or Return puts it in as normal text (the ✓ spins briefly while the last words settle); ✕ or Esc drops it and leaves the message as it was. Trek going to the background, a recognizer error, or switching threads ends the take with a toast (background and error keep what was heard; switching drops it). Where the live audio-buffer request can't start, the take records to a file and the text arrives after ✓. Windows has no mic button (Win+H is its own dictation).

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open draft
./.agents/skills/verify-trek/scripts/trek-dev send "A short markdown answer"
./.agents/skills/verify-trek/scripts/trek-dev screenshot thread
```

Voice mode needs a bundled build and real permissions; the shots verbs are `dictate` (the mic button's path: the first call opens the take, the second is ✓) and `dictate-file <path>` (no mic: the file is transcribed and goes in as a take that's already settling).

Prompts containing `tools`, `permission`, `question`, `plan`, `error`, `mock:long 5s`, `mock:stream 5s`, or `mock:visualization` exercise those mock-agent paths. The isolated profile cannot invoke a real vendor CLI.

While a turn runs, the transcript's top shows a "Thinking Ns" timeline of the agent's steps (thoughts, tool calls with file chips); `tools` walks it, `agents`/`subagents` fan out two parallel scouts and merge back at a hollow node, `error` draws a dashed dead end, and `mock:history 60` folds older steps into "…and N earlier steps". When the turn ends the header reads "Thought for Ns" and the timeline collapses — click the header or its chevron to reopen it (the closing blue "Finished thinking" node is the last row).

## Code

`crates/trek-app/src/composer.rs` (voice mode in `composer/voice.rs`, audio and Speech in `dictate.rs`), `thread_view.rs`, `workspace.rs`, `workspace/agent_updates.rs`; agent sessions in `crates/trek-agents/src/` and mock scripts in `mock.rs`.

## Verify

The sent message appears once, the mock response streams and reaches idle, controls remain usable, and `logs --grep error` is empty. For permission/question/plan prompts, use `control wait <condition>` and capture the visible request card. For voice mode: the waveform moves while speaking, interim words are italic and grey and get revised, ✓ leaves upright text with the caret after it and the toolbar back, ✕ leaves the text as it was.
