# Threads and composer

## What it does

Runs coding agents in a shared conversation UI. A draft chooses project, agent, model, effort, hand-holding, plan mode, and optional worktree. The composer supports queued or steering follow-ups, `/` commands, `@` files, `$` skills, pasted/dropped/picked images, app snapshots, and (Mac, bundled builds) voice mode. Permission, question, and plan requests appear as answerable cards.

## Reach it as a user

Click **New thread** or press ⌘N, choose a project and agent options, type in the bottom composer, then press Return. Open the composer’s option controls for model, effort, plan, worktree, or access level. Use the **+** menu for files, photos, and snapshots.

Switching agent: pick another agent's model in the model menu, in a thread with a conversation. Nothing moves yet: a bar above the prompt says "Your next message switches to <agent>. The conversation so far goes with it." with its size ("3.1k tokens": click it for the text, what it was limited to and what was left out) and "Stay on <current>". The next message moves the chat, stopping a running turn if there is one (the bar says so), and carries the handover. Another model of the same agent changes in place, as before, and so does a thread nothing has been said in.

Queue: messages queued behind a running turn each get a row above the prompt. With the pointer on a row: move up, move down, edit (the message goes to the prompt; sending puts it back in its place, Esc cancels) and remove (with Undo in a toast). ⌥↩ sends a message the other way from Settings › General for once, mid-turn (steer or queue).

Compact chip: a large idle thread (40% of its window or more) whose prompt cache has gone (about an hour on a subscription, five minutes on an API key) on an agent that can compact gets a Compact chip above the prompt's right edge. Its tooltip says why; a click sends `/compact`; its ✕ dismisses it until the thread has been active again.

Voice mode (Mac): the mic button shows only in a bundle whose Info.plist has the microphone and speech usage keys (or with `TREK_DICTATE=1`). Pressing it asks for both permissions, then swaps the toolbar row for a scrolling bar waveform with a ✕ and a ✓. The text area reads "Listening…" until words come; what the recognizer hears shows at the caret in italic, muted text and is revised live. ✓ or Return puts it in as normal text (the ✓ spins briefly while the last words settle); ✕ or Esc drops it and leaves the message as it was. Trek going to the background, a recognizer error, or switching threads ends the take with a toast (background and error keep what was heard; switching drops it). Where the live audio-buffer request can't start, the take records to a file and the text arrives after ✓. Windows has no mic button (Win+H is its own dictation).

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open draft
./.agents/skills/verify-trek/scripts/trek-dev send "A short markdown answer"
./.agents/skills/verify-trek/scripts/trek-dev screenshot thread
```

`control compact-demo` makes the thread on screen large, billed per token and idle past its cache, for the Compact chip. Pick another agent with `control click model-pill`, `control click mm-model`, `control click rail-direct:mock-relay`, then `control click m-direct:mock-relay-relay-swift`: the isolated app has the Mock and Mock Relay agents. Send through the composer (`control type`, `control key enter`), as `trek-dev send` goes round it and leaves the switch waiting. A row's controls show with `control hover "queue-row#0"`.

Voice mode needs a bundled build and real permissions; the shots verbs are `dictate` (the mic button's path: the first call opens the take, the second is ✓) and `dictate-file <path>` (no mic: the file is transcribed and goes in as a take that's already settling). `control dictate-demo [text]` works in the isolated profile with neither: it opens voice mode, with `text` as heard so far.

Prompts containing `tools`, `permission`, `question`, `plan`, `error`, `mock:long 5s`, `mock:stream 5s`, or `mock:visualization` exercise those mock-agent paths. The isolated profile cannot invoke a real vendor CLI.

## Code

`crates/trek-app/src/composer.rs` (voice mode in `composer/voice.rs`, audio and Speech in `dictate.rs`; the switch bar in `composer/switch_bar.rs`, the queue's rows in `composer/queue.rs`, the chip in `composer/compact_chip.rs`), `workspace/switching.rs`, `workspace/follow_ups.rs`, `workspace/compact_offer.rs`, `trek-core/src/handover.rs`, `thread_view.rs`, `workspace.rs`, `workspace/agent_updates.rs`; agent sessions in `crates/trek-agents/src/` and mock scripts in `mock.rs`.

## Verify

The sent message appears once, the mock response streams and reaches idle, controls remain usable, and `logs --grep error` is empty. For permission/question/plan prompts, use `control wait <condition>` and capture the visible request card. A switch: no handoff row and the old agent's session stay until the message goes; "Stay on" leaves nothing changed. For voice mode: the waveform moves while speaking, interim words are italic and grey and get revised, ✓ leaves upright text with the caret after it and the toolbar back, ✕ leaves the text as it was.
