# Running a thread on another machine

Trek on one Mac can start a thread on another of your machines running Trek, follow it live, send
follow-ups and answer approvals, the way the phone does today. The first time a repo is needed on the
other machine, Trek clones it there and copies the files git ignores (an `.env`), in one dialog.

Written 2026-10-20 from a read of what Trek already has.

## What exists

The phone link (`crates/trek-remote`) is a WebSocket over TLS on port 7420 with a self-signed
certificate the client pins by SHA-256 fingerprint, single-use 8-character pairing codes, a per-device
token whose hash lives in `devices.json`, pings both ways, and a JSON protocol (`protocol.rs`) that
lists threads, subscribes to transcripts, sends, interrupts, answers approvals, starts threads in a
project, and more. The Mac side (`crates/trek-app/src/remote.rs`) never asks what kind of client it is
talking to. The only client is Swift (`ios/Trek/Net/TrekClient.swift`). Bonjour (`_trek._tcp`) is
advertised on Wi-Fi with only an `id` in the TXT record.

## Shape of the feature

1. **Machines pair like phones.** Settings › Phone becomes Settings › Phone & Machines. "Pair another
   machine" on machine B shows the same code and QR as for a phone, plus the `trek://pair?…` link as
   text to copy. On machine A, "Add a machine…" takes that link (or host, code and fingerprint typed),
   pairs, and keeps the token, fingerprint, name and routes in `machines.json` beside `devices.json`.
   A machine on the same Wi-Fi is offered by name from Bonjour, so the TXT record gains `name`.
   Either side can revoke: B from its devices list (a machine is a device), A by forgetting it.

2. **"On this Mac ▾" in the composer**, next to the project and worktree chips. It lists this Mac and
   every paired machine with a dot for reachable, and "Add to another machine…" at the end. Picking a
   machine means the draft's thread starts there. The chip is remembered per project.

3. **"Add to another machine…"** is one dialog: source (this Mac, the project path) → `git clone` →
   target machine; clone over SSH or HTTPS (the origin URL from `git remote get-url origin`, rewritten
   to the chosen scheme); the folder on the target (default `~/<repo>`); and "Copy ignored files": the
   files in the checkout that git ignores and that are not inside a wholly ignored folder like
   `node_modules` or `target`, listed with sizes and ticked by default when under 1 MiB each and
   8 MiB together. "Clone to <machine>" runs the clone there, copies the ticked files after it
   finishes, registers the folder as a project on B, and starts the thread.

4. **A remote thread looks like any thread.** It appears in A's sidebar under the machine's name,
   opens in the normal thread view, and its composer sends to B. Its card reads its row as a card
   here reads the thread: the turn clock from `working_since`, the sub-agent pill and tooltip from
   `sub_agents` and `background`, "Sub-agent needs you" from `needs_thread`, and "Waiting" while its
   turn is over and sub-agents are out. Like any card it shows its provider's logo, and its pill
   the logo of each provider its sub-agents run on and how many there are (the tooltip names
   provider, model and each sub-agent). What's live on B (at work, or waiting on the user) is in
   A's Active section as "trek on T-Book"; B's own group holds its quiet threads. A settled row
   with work still out stays listed. Approvals, questions, plan reviews,
   Stop, model and effort changes go through the protocol's `answer`, `interrupt` and `set_prefs`.
   A keeps nothing in its store: B owns the thread; A holds what the snapshot and subscription gave it
   and re-fetches on reconnect. The right panel's Git and Changes read B's repo through `git_*`.

5. **Work out beyond the turn shows inside the thread too** (0.4.64). Until 0.4.63 the sidebar's
   badge counted B's sub-agents but the thread view showed none of them: A dropped the `sub_agent`
   link on each tool item, so the agent's own sub-agents (Claude Code's Agent tool) folded into
   "Used 2 tools", Trek's own (`delegate_task`) read as gone, the working bar never said "Waiting
   on 2 sub-agents" (it asked for a local session), the hub had no sub-agents line, and the
   `background_tasks` on the row were never drawn. Now A draws each sub-agent's row from its link
   as B words it (label, status line, steps, running/done/failed, its clock), its row opened lists
   its latest calls (`activity`, new in 0.4.64), the working bar and hub read B's row `sub_agents`
   (the list the badge reads, so they agree), and the strip above the composer lists B's background
   commands with their last line. Opening Trek's own sub-agent opens its thread on B (its row comes
   `unlisted`, as phones get it; never in A's sidebar list), where its own sub-agents show the same
   way. Stop on a sub-agent goes to B (`interrupt` its thread for Trek's own, `sub_agent_action`
   for the agent's own; message too, where the agent takes one), and Stop and the output of a
   background command go as `stop_background` and `background_output`. B also re-sends a
   sub-agent's row whenever where it stands changed with nothing else in the transcript changing
   (Trek's own works in a thread of its own), which phones gain from as well.
   Mixed versions: a 0.4.63 B already sends the links, unlisted rows and background tasks, so a
   newer A shows the rows, the bar, the strip and opening; the opened row's call list and the
   onward arrow of a sub-agent woken again need B on 0.4.64. A B before `sub_agent_view` (no links)
   gives rows read from the call alone. An older A ignores the new fields. Stop and message on the
   agent's own need B to list `sub_agent_view`; without it the buttons don't show.

## Files and pictures attached here (0.4.61)

Until 0.4.60 a message to a thread on B with a file attached was refused ("Files can't go to B
yet"): the message would have named a path on A that B can't open. Now what's attached on A (the +
menu, a drop, a paste, a screenshot or photo, and A's files typed as `@/path` or `@~/path`) goes to
B ahead of the message, over B's own authenticated connection and the route the phone already uses
for its videos (`upload_part` in pieces of 4 MiB, `MAX_UPLOAD` 2 GiB a file, `docs/MOBILE.md`
`uploads` and `machine_files`). B keeps each in its data folder's `uploads/` as `<A's name> <date>
at <time> <name>` (name made safe, never over another file, free space checked, a quiet upload
dropped after an hour) and says where in the last piece's reply. The message then goes as a local
send would be shaped on B: each mention of A's file becomes a mention of B's copy, in place (B's
path as B writes it, a Windows B's with backslashes), and each picture goes by its upload id in
`files`, which B attaches as an image the model sees. A remembers which copy on B is which file on
A (`machines.json`, the last 500 a machine), so its transcript shows A's own files: a video plays
from A's copy, a picture shows.

The composer sends them when send is pressed and holds the draft meanwhile: a bar fills along each
tile, a line says how far ("Sending 2 files to T-Book · 1.2 MB of 2.6 MB") with Cancel. A failure
(B refused it, the connection dropped, a file changed while it went), Cancel, taking a file off or
moving to another thread stops them and leaves the draft and its attachments as they were, the
failure said in a toast. Still refused, with why: a file over 2 GB, and a B on a Trek before
0.4.61, which doesn't list `machine_files` ("T-Book needs Trek 0.4.61 or later to receive
files"; pictures alone still go to it inline, as before). A folder goes as a zip since 0.4.68
(below).

## Everything from here (0.4.68)

Until 0.4.67 a thread on B could be followed, sent to, stopped and answered from A, and anything
else said "Do that in Trek on T-Book". B already did those things for a phone; A never asked.
Now A asks, B does the work on its own thread, and A shows what B answers:

- **Rewind, undo, retry, edit and send again, fork** go as `turn_action`. The confirmation lists
  the files B would put back, as B says (`turn_preview`); the message taken back comes into A's
  composer once B has done it (A's own copy of it, with A's files); an edited message is B's
  rewind followed by a send; a fork is B's new thread, opened on A when its row arrives.
- **Snooze, Wake now, Delete…** go as `thread_action`. Delete asks B first, unconfirmed: B deletes
  nothing and answers with its own confirmation's words (a worktree's uncommitted work,
  sub-agents that go too), which A's dialog shows before asking again, confirmed.
- **A secret** B's agent asks for is given in A's card (`secret_answer`, new in 0.4.68: see
  `docs/MOBILE.md`). It goes to B's private file for the thread over the paired, pinned
  connection, and nowhere on A.
- **A folder** attached on A goes as one zip of its files (what git ignores, and `.git`, stay
  behind), named `<folder> folder.zip`, over the same route as any file; A's transcript still
  shows the folder. B needs nothing new (0.4.61 takes files). What in it can't be read is left
  out, and a toast says how much.
- **The start of a long thread**: a thread opens with its last 300 items, and A now reads the
  ones before them in pages (`transcript_before`) until the whole conversation is there.
- **The composer reads B**: the model menu lists the agents and models set up on B (its
  snapshot's `agents`), and Full access is offered when B has it unlocked (`full_access`).
- **B's threads call for you on A**: a banner and sound when one starts waiting on you, finishes
  unread or fails, as for a thread of A's own. A's phone isn't pushed for it: B pushes its own.
- **`/new` and `/clear`** typed in a thread on B open a new draft on A, set to start on B (B
  answers them with "open a new thread", which A used to drop).

**B's item ids are positions.** `i200` is the 200th item, so after a rewind on B it can be
another message. When B says a thread was rewritten (`transcript_reset`), or sends a transcript
again whose items aren't the ones their ids named (`machines::rewritten`), A lets go of what it
kept by id: messages opened with "Show more", a rewind confirmation left waiting, an edit begun
in the composer ("That message can't be edited any more."). A page of earlier items asked for
before the transcript came again is dropped and asked for anew. An edited message is B's rewind
and then a send: if B rewinds and then doesn't take the message, the words and pictures go back
into A's composer.

**Where the line is.** B keeps its own locks. A can't lift B's Full access lock (it's a setting of
B's Trek that nothing arriving over a network may change: a paired device that could would make
pairing equal to full control of B), can't add B's permission rules unless B allows it, and B
still checks every request against its own access level and rules. A says what it is ("Locked on
T-Book. Only T-Book can unlock it…") and offers the most that can be chosen from here (Auto).

**Mixed versions.** Everything above but the secret uses requests B has served since 0.4.8, so a
0.4.68 A does them with a 0.4.67 B. The secret needs B on 0.4.68: until then B's notice stays,
with "T-Book needs Trek 0.4.68 or later to take it from here." A request B doesn't know at all is
answered `bad_request` "Unknown message type", which A words as "T-Book needs a newer Trek to …
from here", never "do it there". A 0.4.67 A paired with a 0.4.68 B ignores the row's `secret`.

**Still on B only.** A doesn't draw these yet: B's git panel and changes diffs, its terminal,
file explorer and editor, the side chat, B's queue of follow-ups, the limit bar's buttons, PR
cards' buttons, `@` mentions of B's files and B's own slash-command list (the composer lists
A's), the pictures B's agent made, B's usage, and search across B's threads. The requests for
most exist and phones use them (`git_*`, `queue_action`, `side_chat`, `media`, `card_action`,
`commands`, `usage`, `search`); a terminal, a file listing and reading or writing B's files need
new ones. "Auto-settle behavior" isn't offered for B's threads (there's no request for it; it
used to be offered and change nothing).

## Protocol additions (v1, additive)

- `machine_info` → `{name, platform, trek_version, home}`: so A can name B and default the folder.
- `clone_project {url, folder, name}` → `ack {project_id}` after `git clone`, with
  `clone_progress {folder, line}` events while it runs and `error {code: clone_failed, detail}` on
  failure. Refuses a folder that exists and is not empty.
- `put_files {project_id, files: [{path, bytes_b64, mode}]}` → `ack`: writes under the project
  folder only (no `..`, no absolute paths, no symlink targets), total under the 8 MiB frame limit;
  A sends several messages for more.
- Bonjour TXT gains `name`. `welcome.features` gains `machines` so an old B is recognised.

## Rust client (`crates/trek-remote/src/client.rs`)

Mirrors `TrekClient.swift`: dial every route at once (the one that answered last 300 ms ahead of
the next, and so on) and keep the first that answers, pin the fingerprint, `pair` or `hello`,
request/reply matched by `id`, an event stream for pushes, app-level `ping` every 25 s, reconnect
with exponential backoff capped at 30 s, stop on `unauthorized`, certificate mismatch on every route
of its own, or protocol mismatch, and resubscribe after `welcome` (not before: the phone's bug). A
thread is opened with `subscribe()`, whose `transcript` comes out of the event stream, ahead of the
items B sends after it: taken as the request's reply it reached the app by another way than they
do, and one put in after them replaced what B had said since. The
routes `welcome` names lead, the ones it named before follow (up to 8), so a tailnet address B left
out while its Tailscale was down still reaches it later. `network_changed()` dials at once (or pings a
connection that's up and drops it after 5 s of silence); `also_try()` adds guesses that are kept only
once they answer with the pinned certificate. Each route's dial result is logged at info when it
changes. The protocol types already derive `Deserialize`, so the client reuses them. Tested against
the in-process server the existing tests use.

## Away from B's network

A watches its own network every 5 s while machines are paired. A change (another Wi-Fi, Tailscale up)
has every client dial again at once. When A is on Tailscale, it looks B up among its tailnet's peers
by name (`tailscale status --json`, again each minute while B can't be reached) and has the client
try that address too, so B is reached over the tailnet with no re-pairing even if A never saw B's
tailnet address. While B can't be reached, its threads stay in the sidebar as they last were, none of
them running (no spinner, phrase or clock), under "Offline since 3:19 PM", and the sidebar and
Settings › Machines say why when Trek can tell: every route known for B is on a local network
("install Tailscale on it and on this Mac, signed in to the same account"), Tailscale is off here, or B
is offline in the tailnet.

## Rendering a remote thread

The protocol's `Item` is a lossy copy of `trek_core::store::Item` (positional ids, output cut to
4 KiB, images as counts, requests as separate cards). A mapper turns protocol items back into store
items for display, good enough for the thread view; what it cannot carry (full tool output, images)
it marks so the view can fetch on demand (`attachment`, `background_output`). A `RemoteSession`
implements the session hooks `LiveThread` uses (send, interrupt, answer, set_prefs) by sending
protocol messages, and feeds pushed `item`s into the same update path a local agent's events take.

## Order of work

1. trek-remote: client, protocol additions, Bonjour name, tests. (No UI.)
2. trek-app host side: handle `machine_info`, `clone_project`, `put_files`; machines registry;
   Settings › Phone & Machines; "Add a machine…" pairing from a link.
3. trek-app client side: "On this Mac ▾" chip, "Add to another machine…" dialog, remote threads in
   the sidebar and thread view, RemoteSession.
4. Verify with one Mac playing both parts: the isolated app's own server as "machine B".

## Out of scope for the first cut

Relaying through the internet (needs Tailscale or a relay; the phone link's Tailscale path applies
as is), moving a running thread between machines, syncing uncommitted tracked changes (use a branch),
Windows as the target machine (should work, untested).
