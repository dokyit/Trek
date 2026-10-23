# Running a thread on another machine

Trek on one Mac can start a thread on another of your machines running Trek, follow it live, send
follow-ups and answer approvals, the way the phone does today. The first time a repo is needed on the
other machine, Trek clones it there and copies the files git ignores (an `.env`), in one dialog.

Written 2026-10-20 from T3 Code's "Run on" and "Add to another machine" (Theo's demo) and from a
read of what Trek already has.

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
   opens in the normal thread view, and its composer sends to B. Approvals, questions, plan reviews,
   Stop, model and effort changes go through the protocol's `answer`, `interrupt` and `set_prefs`.
   A keeps nothing in its store: B owns the thread; A holds what the snapshot and subscription gave it
   and re-fetches on reconnect. The right panel's Git and Changes read B's repo through `git_*`.

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
of its own, or protocol mismatch, and resubscribe after `welcome` (not before: the phone's bug). The
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
