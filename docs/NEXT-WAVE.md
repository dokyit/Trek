# Next wave: voice, media, a video watcher, generative UI

Written 2026-10-20 from the user's asks and ten demo videos (pitus_, wheresryan22, chazza; frame
descriptions in the session's scratch folder). The model picker and the thinking-level control stay as
they are. Everything below lands on the Mac, on Windows where it applies, and on the phone.

## 1. Speak instead of typing

- **Phone:** a mic button in the composer. Tapping it swaps the toolbar for a live waveform with ✕ and ✓
  (pitus_, "Voice mode"): interim speech appears in italic grey and is revised live; ✓ commits it as
  normal text, ✕ discards. Speech framework on device (SFSpeechRecognizer, on-device when available),
  microphone and speech permissions with plain explanations, works while connected or not (text is just
  text). Long holds keep listening; silence does not stop it until ✓.
- **Mac / Windows:** the composer already has dictation (dictate.rs); give it the same waveform and
  interim-text treatment so both feel the same.

## 2. Prompt area interactions (pitus_, "AI text area", "Prompt area interactions")

- Typing `/` opens the skills popover above the field with a ghost completion in the field; Tab or Enter
  turns it into a chip token; the token clears on send. Trek has slash commands and skills already; this
  is the ghost completion and the chip.
- The project chip opens a dropdown with search, Recents, and "On This Mac / Remote / New Folder"
  rows: Trek has the project picker and now the machine chip; align the dropdown.
- Thinking process (pitus_): while an agent thinks, a timeline header "Thinking Ns" with a shimmer,
  file chips for what it read, branches for parallel sub-steps merging at a node, a dashed dead end for
  an abandoned path, and a closing "Finished thinking" node; afterwards "Thought for 14s", collapsible.
  Trek shows thinking and tool groups already; this is the timeline presentation. Not the effort control.

## 3. Notes media on the phone

- The Mac's notes hold pictures and files; the phone should preview what the Mac can: images inline,
  PDFs (PDFKit) and videos (AVPlayer) in a viewer sheet, other files with QuickLook, fetched through the
  existing `note_asset` message (chunked when large). Same for files an agent attached to a turn.

## 4. Built-in video watcher

Models cannot watch video. Trek gives every agent a `watch_video` tool (trek-mcp) and the user a
player.

- **Tool:** input a file path or https URL (YouTube and X links too when yt-dlp is installed; otherwise
  direct media URLs) and an optional time range and frame rate; Trek downloads to a scratch folder,
  reads metadata with ffprobe, extracts frames with ffmpeg at the requested rate (capped so the result
  stays under a size budget, with a contact-sheet option), transcribes the audio on the Mac with the
  Speech framework (whisper if installed), and returns the frames as images plus the transcript with
  timestamps and a summary of what each second shows is left to the model. Bundled ffmpeg/ffprobe
  binaries or a clear "install ffmpeg" message; Windows the same.
- **Player:** a video opened from a note, an attachment, a link in the terminal or a `watch_video`
  result plays inline in Trek (artifact web view with `<video>` for local files and https), with the
  frames the model saw shown as a strip under it so the user sees what the agent saw.
- **Phone:** the player via AVPlayer; the tool runs on the Mac.

## 5. Generative UI (pitus_ #1 Map, chazza POIs, #2 Merge conflicts, #3 Database migration)

What the demos show: an inline **map card** (pins and a "n / 5" pager that share one selection, a route
that redraws on selection, a palette swatch bar that re-themes the map, "Send to iPhone"); a **merge
conflict card** that is a PR shown as a commit list with a lane graph and one state machine (create PR →
3 conflicts → resolve → run tests → ready → merged), hover popovers showing each side of a conflict,
tooltips on check chips, and an action button that follows the state; a **database migration card**
started by an `@connector` mention, one tab per table, a Schema step then a Data step per table with
new-column highlighting and a sweep down the rows, ending in "Applied in 9.9s".

How Trek does it, on both platforms:

- **Cards are `trek-html` blocks.** They already render in a sandboxed web view on the Mac and in
  LivePage on the phone, may load https scripts (Leaflet with OpenStreetMap tiles for the map; `trek-viz`
  forbids URLs, so the map is not a viz kind), and are themed with Trek's CSS variables.
- **Cards get a way to act.** Today a page can only report height, focus, Esc, a link and wheel. Add
  `trek.send(text)` (posts `text` as the user's next message in that thread, marked as sent from the
  card, with the card's title) and `trek.copy(text)`. The merge card's "Resolve with mine", "Run tests"
  and "Create PR" become messages the agent acts on; the migration card's "Apply" the same. Nothing
  touches the repo except through the agent, as every other user message. The phone's LivePage gets the
  same two messages. Pages keep no other capability.
- **Three card recipes ship as skills** the agent can read before answering: map card (Leaflet,
  pins + pager + route + palette swatches), PR / merge-conflict card (commit list, lane graph, state
  machine, side-by-side conflict popover, check chips), migration card (tabs, schema then data steps,
  progress). Each recipe is a short Markdown skill with the HTML and the `trek.send` contract, under
  the built-in skills, listed on the Skills page, so any agent on any platform produces the same card.
- **"Send to iPhone"** on a card means: the thread is already on the phone; the button sends
  `trek.send("Open this on my iPhone")` (`artifact::OPEN_ON_PHONE`) which Trek turns into a phone notification deep link to that thread.
- **Anatomy** (wheresryan22) is an agent skill (`npx skills add wheresryan22/anatomy`); it is listed
  among suggested skills on the Skills page with a one-click install where Trek installs skills today.

### Built so far (trek-core, the phone, the Skills page)

- **The contract** (`crates/trek-core/src/artifact.rs`): a page calls `trek.send(text)` or
  `trek.copy(text)`. The helper (`ACTIONS_JS`) is the same text on both platforms: the Mac's
  bootstrap runs it with its secret channel, the phone's `LivePage.actions` holds it line for line
  (a trek-core test reads the Swift file). On the wire: `{"n":<secret>,"t":"send"|"copy","text":…}`.
  Text must be non-blank, at most `MAX_ACTION_BYTES` (4 KiB of UTF-8), with no control characters
  but tab and line breaks (`action_text`); a send is trimmed, a copy kept as it is. A page sends at
  most once per `SEND_INTERVAL` (1 s, `SendGate`), and both only count within `GESTURE_WINDOW`
  (5 s) of the user's own click or key in the page. Anything else is ignored, silently.
  `parse_action` reads them; `parse_message` ignores them, so a handler that doesn't take them yet
  is unchanged.
- **Agents are told** in `artifact::AGENT_INSTRUCTIONS` (under its 1,200-character test): buttons may
  call `trek.send` / `trek.copy`, and the three recipes are the built-in skills `map-card`,
  `pull-request-card` and `database-migration-card` (`crates/trek-core/skills/`, listed on the
  Skills page as "Built into Trek", written into the folder every session may read the first time
  `skills::shipped_root` is asked for).
- **The phone** (`ios/Trek/Views/LivePage.swift`): `send` posts the text into the open thread through
  `AppModel.send(_:to:mode: .steer)` with a haptic and a "Sent from <title>" toast; `copy` goes to the
  pasteboard with a success haptic and "Copied". The thread comes from `\.liveThreadID`, set by the
  thread screen. The phone checks the same limits; its isolated script reports touches (`tap`).
- **Anatomy** is under "Suggested" on the Skills page (`skills::SUGGESTED`): Install runs
  `npx skills add wheresryan22/anatomy` in the dock (`WorkspaceEvent::RunInTerminal`); it leaves
  the list once a skill called `anatomy` is installed.

### Mac wiring for send/copy (not done: `crates/trek-app/src/artifact/mod.rs`)

The bootstrap already gives every Mac card `trek.send` and `trek.copy`; their messages arrive in
`ArtifactView::on_event` as `native::Event::Message(body)` and are ignored today
(`parse_message` gives `None`). To wire them:

1. When `core::parse_message(&body, &self.nonce)` is `None`, try
   `core::parse_action(&body, &self.nonce)`.
2. Keep a `core::SendGate` per card (a field on the view, new with each document); a send goes only
   if `gate.allow(Instant::now())`.
3. `Action::Send(text)`: post `text` as the user's next message in the thread the card is in, by
   the composer's own send path (it steers a running turn or queues, as the user's follow-up setting
   says), marked as from the card: the user item records the card's title (`core::title(html)`,
   else "Interactive page") and the transcript shows it under the bubble as "From <title>". A card
   outside a thread (a note, the expanded preview of another thread) or in an archived thread says
   so in a toast instead. When `text == core::OPEN_ON_PHONE`, also send the paired phone a push that
   opens this thread (the notification deep link it already handles).
4. `Action::Copy(text)`: `cx.write_to_clipboard(ClipboardItem::new_string(text))` and a "Copied"
   toast.
5. Tests (`crates/trek-app/src/tests/artifacts.rs`): a send message with the card's secret adds
   one user item with the text and the card's title, a second within a second adds nothing, one
   with the wrong secret adds nothing; a copy puts the text on the clipboard.

## Order and who does what

1. Phone voice input (Swift) and phone notes media (Swift): independent of the Mac work.
2. `watch_video` tool and the player (Rust, trek-mcp and trek-app).
3. Prompt area: ghost completion and chips, project dropdown alignment, thinking timeline (Rust).
4. Generative UI kinds (Rust + Swift renderers), after the frame descriptions are read.
