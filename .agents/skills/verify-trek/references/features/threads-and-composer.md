# Threads and composer

## What it does

Runs coding agents in a shared conversation UI. A draft chooses project, agent, model, effort, hand-holding, plan mode, and optional worktree. The composer supports queued or steering follow-ups, `/` commands, `@` files, `$` skills, pasted/dropped/picked images, app snapshots, and (Mac, bundled builds) voice mode. Permission, question, and plan requests appear as answerable cards.

## Reach it as a user

Click **New thread** or press ⌘N, choose a project and agent options, type in the bottom composer, then press Return. Open the composer’s option controls for model, effort, plan, worktree, or access level. Use the **+** menu for files, photos, and snapshots.

The + menu's first group: on the Mac, **Photos & videos…** and **Files…**; on Windows, **Add photos & files** (its open panel reaches photos). Files… (and Windows' item) is the open panel.

The attachment strip above the prompt holds what goes with the next message: images as thumbnails (HEIC, TIFF and BMP as a PNG copy) and every other file or folder, however it came (Files…, Photos & videos…, a drop, a paste that brings images too), as a tile beside them. A video's tile has its poster frame (Quick Look's thumbnail, made once into the data folder's `cache/posters`; Mac only, the file's icon elsewhere or until it's made) with a play badge; any other file has its type's icon, a folder its folder icon; then the name (cut in the middle so the extension shows) and `PDF · 13 KB` (or `Folder`). The full path is the tooltip; a click opens it (Quick Look on the Mac, its app on Windows, a folder in the file manager); the × on hover takes it off. As the message goes, each tile becomes an `@path` mention after the text (quoted when the path has a space), so the agent, the transcript, the phone, the queue and edits get the message as before. A message that comes back (a draft put away and brought back, a rewind or a follow-up put back, a queued message or a sent one being edited) has the mentions at its end back as tiles, when the files are still there. The `@` picker's mentions are words in the text, as before. A Finder copy of only non-image files still pastes their names as text.

In the transcript, the files a message was sent with (absolute `@path` mentions after its words that are still on disk) show as chips under the bubble, not as paths in it: the type's icon and the name, the full path as tooltip, a click opens it. A video plays under it instead. Copy copies the message as sent.

Photos & videos… puts the system's Photos picker (PhotosUI, out of process, no Photos permission) up as a sheet on the window: photos and videos only, up to 20, in the order picked, Add or Cancel (Escape too) closes it. A second open while it's up brings it forward. Picks are moved into the data folder's `snapshots` (as `IMG_1234-<time>.jpeg`, `ScreenRecording_…-<time>.mp4`; pruned with snapshots), the strip shows a spinner tile meanwhile and a send waits for them; then photos go on the strip (as JPEG or PNG where Photos offers one, else the HEIC becomes a PNG beside it) and videos as tiles. One that can't be had (not downloaded from iCloud) says so in a toast; the log has `photos: pick N as <type> (offered …)` for each. Picks that arrive after a switch to another thread go to the draft they were picked for.

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

`control attach <path>|<path>…` puts files in the composer as a drop does (images as thumbnails, the rest as tiles). A tile's id is `attachment-file#<n>` (its × `att-file-x#<n>`), a sent file's chip `user-file#<message index × 100 + n>`.

The + menu opens with `control click attach`; its items have no ids, so `control key "down enter"` picks the first (Photos & videos… on the Mac). The Photos sheet is a native window the `screenshot` frames leave out: find its id (owner `trek`, title "Photos") and capture it with `screencapture -x -o -l <id>`. The picker runs out of process and ignores input posted to Trek's process; it takes real clicks and keys only, with the isolated app in front. Without them, `take_photos` is driven by tests (`tests/composer.rs`).

Voice mode needs a bundled build and real permissions; the shots verbs are `dictate` (the mic button's path: the first call opens the take, the second is ✓) and `dictate-file <path>` (no mic: the file is transcribed and goes in as a take that's already settling). `control dictate-demo [text]` works in the isolated profile with neither: it opens voice mode, with `text` as heard so far.

Prompts containing `tools`, `permission`, `question`, `plan`, `error`, `mock:long 5s`, `mock:stream 5s`, or `mock:visualization` exercise those mock-agent paths. The isolated profile cannot invoke a real vendor CLI.

## Code

`crates/trek-app/src/composer.rs` (voice mode in `composer/voice.rs`, audio and Speech in `dictate.rs`; the switch bar in `composer/switch_bar.rs`, the queue's rows in `composer/queue.rs`, the chip in `composer/compact_chip.rs`; the + menu's first group and the Photos picker in `photos.rs`, its sheet in `photos/picker.rs`, the strip, its tiles and the transcript's file chips in `attachments.rs`, mentions read back off a message in `trek-core/src/media.rs` `split_attached`), `workspace/switching.rs`, `workspace/follow_ups.rs`, `workspace/compact_offer.rs`, `trek-core/src/handover.rs`, `thread_view.rs`, `workspace.rs`, `workspace/agent_updates.rs`; agent sessions in `crates/trek-agents/src/` and mock scripts in `mock.rs`.

## Verify

The sent message appears once, the mock response streams and reaches idle, controls remain usable, and `logs --grep error` is empty. For permission/question/plan prompts, use `control wait <condition>` and capture the visible request card. A switch: no handoff row and the old agent's session stay until the message goes; "Stay on" leaves nothing changed. For voice mode: the waveform moves while speaking, interim words are italic and grey and get revised, ✓ leaves upright text with the caret after it and the toolbar back, ✕ leaves the text as it was.
