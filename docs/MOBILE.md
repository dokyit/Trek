# Trek Mobile — Design

> Status: phase 1 in progress (2026-10). Mac side: `crates/trek-remote`. iPhone app: `ios/`.
> Background research: `reports/Trek competitors and mobile app plan.md`.

## Goal

Keep working on your projects away from the laptop. The agents keep running on the Mac; the phone is
an **inbox and a remote, not an IDE**:

1. See every thread and its status (the same Pinned · Needs you · Working · Recent model as the
   desktop sidebar), across Claude Code, Codex, OpenCode and the ACP agents.
2. Read transcripts: user turns, the agent's Markdown, grouped tool rows, file changes.
3. Send follow-ups (steer the running turn, or queue for after it) and start new threads.
4. Answer approvals, questions and plans. **Trek never answers on the user's behalf**: a request
   waits, on the Mac and on the phone, until a human decides. No timeouts, no defaults.
5. Stop a running turn.

What users build for themselves today (tmux + Tailscale + Termius, ntfy hooks, 60-line approval
bridges) ranks the needs: know when something is done or blocked; approve or deny in one tap; answer
questions; glance across all agents; steer; then review diffs. Phase 1 follows that order.

## Architecture

```
 iPhone (SwiftUI)                           Mac (Trek.app)
┌──────────────────┐   WebSocket (JSON)    ┌──────────────────────────────────────────────┐
│ TrekClient       │ ◄──────────────────► │ trek-remote::RemoteServer (tokio)            │
│  URLSession WS   │   LAN / Tailscale     │   pairing + per-device tokens                │
│ Store (Observ.)  │   wss://, pinned cert │   fan-out of HostEvents to subscribed phones │
│ Views            │   phase 2: E2E relay  │        │ RemoteHost trait (or ChannelHost)    │
└──────────────────┘                       │        ▼                                     │
                                           │ trek-app Workspace / Store / sessions        │
                                           └──────────────────────────────────────────────┘
```

### Phase 1: direct connection (LAN or Tailscale)

- Trek runs a WebSocket server (`trek-remote`) inside the app process, **off by default**, turned on
  in Settings › Mobile. It binds `0.0.0.0:7420` (configurable; "Tailscale only" binds the `100.x`
  address). Sessions live as long as Trek does; the UI says so.
- The phone reaches the Mac on the same Wi-Fi, or anywhere over Tailscale (the tailnet IP or MagicDNS
  name goes in the QR code when Tailscale is up). No port forwarding, no Trek servers.
- Transport security, staged:
  - **1a (done):** `ws://` with a per-device bearer token. Still served when the Mac runs without
    TLS (`TREK_REMOTE_PLAIN=1` on the demo host, older builds); the phone only uses it after an
    explicit "Unencrypted connection" warning (see below).
  - **1b (now, default):** `wss://` with a self-signed certificate generated once per Mac
    (`trek-remote/src/tls.rs`; kept as `identity.der`/`identity.key` beside the devices file, the
    key mode 0600). Its SHA-256 fingerprint is in the QR code and the phone pins it; no CA is
    involved. Same protocol, same tokens. Details in [TLS and pinning](#tls-and-pinning-phone-side).
  - **2:** Noise (XX, X25519 + ChaCha20-Poly1305) or iroh QUIC end to end, so the same bytes can go
    through a blind relay. The protocol below is the payload either way.

### Phase 2: relay and push

The research recommends iroh peer-to-peer with relay fallback plus a thin Trek service. That service
does three things: APNs pushes (Apple only accepts them from a server with Trek's key), an end-to-end
encrypted mailbox (last-known inbox and queued sends while the Mac sleeps), and optional relays.
Push payloads carry only a thread id and an event kind, encrypted to the device key; a Notification
Service Extension decrypts them into "Needs approval: `rm -rf build/` in trek-api" with
Approve / Deny / Reply… actions. Time-sensitive interruption only for blocking approvals and
questions; suppressed while the user is active at the Mac.

### Why WebSocket first

It ships in days on both sides (tokio-tungstenite, `URLSessionWebSocketTask`), it's debuggable with
`websocat`, and the protocol is transport-independent: moving to TLS, Noise or iroh later changes the
pipe, not the messages. LAN/Tailscale is what power users already run.

## Pairing

1. Settings › Mobile › **Pair iPhone** shows a QR code and the same details as text.
   ```
   trek://pair?host=192.168.1.20:7420&code=K7Q2-9XMV&name=Tobias%E2%80%99s%20MacBook%20Pro&hid=7f3c…&fp=b1380087…
   ```
   `code` is 8 Crockford base32 characters (40 bits) shown as `XXXX-XXXX`: one use, valid 10
   minutes, burned after 5 wrong attempts (from anyone). `fp` is the SHA-256 of the Mac's
   certificate (DER), 64 lowercase hex characters; the Mac also shows its short form beside the
   code: the first 8 hex characters, uppercased, as `ABCD-1234`. A link without `fp` comes from a
   Mac serving plain `ws://`.
2. The phone scans it (or opens it as a deep link, or the user types host:port, the code and the
   short fingerprint). A link never pairs by itself: the phone first shows **"Pair with
   <name>?"** with the address and the short fingerprint to compare against the Mac's screen.
   On confirmation it connects over pinned TLS and sends `pair` with the code, its device id (a
   UUID kept in the Keychain) and its name ("Tobias's iPhone").
3. The Mac answers `paired` with a fresh **device token** (32 random bytes, base64url) and shows
   "Tobias's iPhone paired" with an Undo. The Mac stores only `sha256(token)`, the device's id,
   name, pairing time and last-seen time (`devices.json` in Trek's support folder).
4. Every later connection starts with `hello {device_id, token}`, pinned to the same
   certificate. Tokens are compared in constant time. Settings › Mobile lists devices with last
   seen and a **Revoke** button; revoking drops the device's live connections at once.

### TLS and pinning (phone side)

What `ios/Trek/Net/TrekClient.swift` does, as built:

- **Default is TLS.** The phone connects to `wss://<host:port>/` with a `URLSession` whose delegate
  answers the server-trust challenge itself: first certificate of `SecTrustCopyCertificateChain`
  → `SecCertificateCopyData` → CryptoKit SHA-256 → compare with the pin. Match: `.useCredential`
  with `URLCredential(trust:)`. Anything else: `.cancelAuthenticationChallenge`. There is no CA or
  host-name check; the fingerprint is the whole trust decision. `pair` and `hello` are only sent
  once the socket is open, i.e. after the pin was checked, so a wrong server never sees the code or
  the token.
- **What is pinned.** From a QR code or link, the full `fp`. From typed pairing, the 8 hex
  characters the user entered (`ABCD-1234`, case and dash ignored) match any certificate whose
  fingerprint starts with them; the full fingerprint seen during that handshake is what gets kept.
  **Limitation:** an 8-hex-character prefix is only 32 bits, and the fingerprint isn't secret
  (anyone on the network can fetch the certificate), so an attacker in the path can grind a
  certificate with the same prefix in minutes of GPU time. Typed pairing is therefore weaker than
  scanning; prefer the QR code, and a longer short form (or a post-pairing check of the full
  fingerprint on both screens) is an open question below.
- **Storage.** The pin is stored with the paired Mac (address, device token, host name and id) as
  one Keychain item (`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`), and every later `hello`
  connection is pinned to it.
- **A changed certificate is never trusted silently.** If the Mac answers with another
  certificate, the phone stops (no retries, no fallback to plain) and shows **"This Mac's identity
  changed"** with the paired and the presented short fingerprints and a **Pair again** button.
  Pairing against a wrong fingerprint fails with "That Mac's fingerprint is X, not Y. Nothing was
  sent to it."
- **Plain `ws://` only by explicit choice.** A link without `fp`, or typed pairing with the
  fingerprint left empty, shows an **"Unencrypted connection"** warning first; only "Pair without
  encryption" / "Connect without encryption" pairs over `ws://`, and the choice is stored with the
  Mac. A TLS connection that fails its handshake (e.g. the Mac only speaks plain) is reported and
  stopped; the phone never downgrades by itself. Records paired before TLS existed have no pin and
  keep using `ws://`; Settings shows them as **Unencrypted** in amber, connected TLS Macs as
  **Encrypted · pinned** with a lock and the short fingerprint.
- **App Transport Security.** `NSAllowsArbitraryLoads` is gone. `NSAllowsLocalNetworking` covers
  LAN and tailnet IP addresses, `.local` and bare host names. One exception domain, `ts.net`
  (Tailscale MagicDNS) with `NSExceptionAllowsInsecureHTTPLoads`, because ATS rejects a
  self-signed certificate on a fully qualified name even after the delegate accepted it (verified
  with a `nip.io` name in the simulator); the pin still decides. Any other DNS name for the Mac
  won't connect; use its IP address.

## Security model

Trek runs agents with Full access on the same machine; a remote endpoint is a remote shell with
extra steps. Lessons applied (DeepSeek Harness CVE-2026-82533: a local UI that trusted the Host
header let a sandboxed agent upgrade itself):

- **Every connection authenticates cryptographically.** Nothing is trusted for being local. An
  unauthenticated socket may send only `pair` or `hello`; anything else closes it. A connection that
  hasn't authenticated within 10 seconds is closed.
- **Browsers are refused**: an upgrade request carrying an `Origin` header is rejected (403), so a web
  page (or an agent's browser) can't drive the server through the user's network position.
- **Off by default**; an explicit toggle, with the listening address shown.
- **Pairing codes** are short-lived, single-use and attempt-limited; tokens are random, stored hashed,
  per device and revocable.
- **Capabilities, not parity.** A phone can't raise a thread's hand-holding (no Full access from the
  phone), can't change settings, can't read arbitrary files, can't run shell commands. "Allow for
  session" sits behind the approval card's ••• menu and the app requires the device owner first
  (LocalAuthentication `.deviceOwnerAuthentication`: Face ID / Touch ID, falling back to the
  passcode; no passcode set means it can't be sent). That is enforced on the phone only; phase 2
  enforces it on the Mac side with a signed step-up.
- **Approvals are read before they're allowed.** The card shows the whole command, wrapped and
  monospaced (long ones fold at about seven lines with "Show all", then scroll). Commands that look
  destructive (`rm -rf`, `git push --force`, `git reset --hard`, `sudo`, `curl … | sh`, `dd of=/dev/…`,
  `DROP TABLE`… see `ios/Trek/Model/CommandRisk.swift`) get a red "Looks destructive: …" line and
  Allow loses its prominent style, so Allow and Deny carry equal weight. A heuristic for the eye,
  not a sandbox.
- **Links don't act.** A `trek://pair` link (deep link or scanned) only opens a confirmation sheet.
- **Never auto-answer.** The server has no timeouts on approvals; the desktop keeps its "requests
  wait indefinitely" rule. The phone and the Mac show the same request; whichever answers first
  wins and the other's card resolves (`item` update with `state != pending`).
- Message size capped at 1 MiB; one connection per device (a new one replaces the old).
- **Updater first**: before the server ships enabled, the updater must refuse unsigned manifests
  (see the report: a phone that can steer the Mac makes a compromised update remote code execution).

## Wire protocol (v1)

JSON text frames over WebSocket, one message per frame, `snake_case` keys, timestamps in unix
**milliseconds**. Every message is an object with a `"type"`. Requests from the phone may carry an
`"id"` (any string); the reply to it carries `"re"` with the same value. Unknown fields must be
ignored; unknown `type`s from the server are ignored by the phone, and the server answers unknown
client types with `error {code: "bad_request"}`.

The protocol version is an integer; `hello` and `pair` carry the phone's, `welcome` and `paired` the
Mac's. Mismatched majors get `error {code: "unsupported_protocol"}` and a close. Fields are only added
within v1.

### Handshake

Phone → Mac, first message, either:

```json
{"type":"pair","id":"1","protocol":1,"code":"K7Q2-9XMV","device_id":"6F1C…","device_name":"Tobias's iPhone","app_version":"0.1.0"}
{"type":"hello","id":"1","protocol":1,"device_id":"6F1C…","token":"q3t…","app_version":"0.1.0"}
```

Mac → phone:

```json
{"type":"paired","re":"1","protocol":1,"token":"q3t…","host":{"id":"7f3c…","name":"Tobias's MacBook Pro","version":"0.3.2"}}
{"type":"welcome","re":"1","protocol":1,"host":{"id":"7f3c…","name":"Tobias's MacBook Pro","version":"0.3.2"}}
```

`paired` authenticates the connection too (no second `hello` needed). Right after `welcome` or
`paired` the Mac sends a `snapshot`. Failures: `error` with code `unauthorized` (bad or revoked
token), `pairing_failed` (wrong, used or expired code), `rate_limited`, then the socket closes.

### Snapshot and thread updates (Mac → phone)

```json
{"type":"snapshot",
 "threads":[{
   "id":"01J…","title":"Fix the flaky auth test",
   "project":{"id":"p1","name":"trek-api","hue":212,"monogram":"TA"},
   "agent":{"key":"claude-code","name":"Claude Code"},
   "model":"claude-opus-5-5","model_label":"Opus 5.5",
   "run_state":"needs-you",
   "needs":{"kind":"approval","text":"Run `cargo test -p auth`"},
   "section":"inbox","unseen":true,"pinned":false,
   "branch":"fix/auth-flake","worktree":true,
   "activity":"Running cargo test","working_since":null,
   "updated_at":1791020400000,"additions":42,"deletions":7}],
 "projects":[{"id":"p1","name":"trek-api","hue":212,"monogram":"TA","branch":"main","is_repo":true}],
 "agents":[{"key":"claude-code","name":"Claude Code","default_model":"claude-opus-5-5",
            "models":[{"id":"claude-opus-5-5","label":"Opus 5.5"},{"id":"claude-sonnet-5-5","label":"Sonnet 5.5"}]}]}
```

- `run_state`: `idle` · `working` · `needs-you` · `failed` (Trek's `RunState`, kebab-case).
- `section`: `pinned` · `inbox` · `working` · `snoozed` · `settled` (Trek's `Section`). The phone
  groups by it: **Pinned**, **Needs you** (inbox threads that `needs` you, or failed), **Working**,
  **Recent** (the rest of inbox, then settled, newest first). Snoozed threads are hidden unless they
  raise their hand (then the Mac already reports them as `inbox`).
- `needs`: present when the thread waits on the user. `kind`: `approval` · `question` · `plan` ·
  `failed` · `limit`; `text` is a one-line summary for the row.
- `project.hue`: degrees 0–359 (the desktop's project colour, chosen or derived from the name);
  `monogram`: the two-letter badge.
- `agent.key`: Trek's `AgentId::key()` (`claude-code`, `codex`, `opencode`, `droid`, `acp:cursor`,
  `direct:openai`…).
- `activity`: what a working thread is doing now ("Editing src/auth.rs"), else null.

Live changes:

```json
{"type":"thread","thread":{…same shape as in snapshot…}}
{"type":"thread_removed","thread_id":"01J…"}
```

A `thread` message is an upsert. The Mac may send a fresh `snapshot` at any time (it replaces
everything: projects and agents included).

### Transcripts

Phone → Mac:

```json
{"type":"subscribe","id":"7","thread_id":"01J…","after_seq":null}
{"type":"unsubscribe","thread_id":"01J…"}
```

Mac → phone, the reply (`reset: true` replaces the phone's copy; `false` appends to it; the Mac
answers with `reset: true` whenever it can't serve `after_seq`, e.g. after a rewind):

```json
{"type":"transcript","re":"7","thread_id":"01J…","reset":true,"seq":118,"items":[ …items… ]}
```

then, while subscribed, upserts by `id` (a new id appends; a known id replaces in place, e.g.
streaming text growing, a tool finishing, an approval resolving):

```json
{"type":"item","thread_id":"01J…","item":{ …item… }}
{"type":"transcript_reset","thread_id":"01J…"}
```

`transcript_reset` tells the phone to `subscribe` again (the thread was rewound or rewritten).

Every item has `id` (stable, Trek's item id), `seq` (per-thread, increasing; an update gets a new,
higher seq), `at` (ms or null) and `kind`:

```json
{"id":"a1","seq":1,"at":1791020000000,"kind":"user","text":"The auth test is flaky, find out why","images":0}
{"id":"a2","seq":2,"at":null,"kind":"reasoning","text":"Let me look at the test first…"}
{"id":"a3","seq":3,"at":null,"kind":"tool","call_id":"toolu_1","tool":"read","title":"Read","detail":"src/auth/session.rs","status":"done","output":"","added":null,"removed":null}
{"id":"a4","seq":4,"at":null,"kind":"tool","call_id":"toolu_2","tool":"edit","title":"Edit","detail":"src/auth/session.rs","status":"done","output":"","added":12,"removed":3}
{"id":"a5","seq":5,"at":null,"kind":"tool","call_id":"toolu_3","tool":"command","title":"Run","detail":"cargo test -p auth","status":"running","output":"   Compiling auth v0.1.0…","added":null,"removed":null}
{"id":"a6","seq":6,"at":null,"kind":"assistant","text":"The race is in **`refresh()`**…","streaming":false}
{"id":"a7","seq":7,"at":null,"kind":"approval","request_id":"r1","title":"Run command","detail":"rm -rf target/","state":"pending"}
{"id":"a8","seq":8,"at":null,"kind":"question","request_id":"r2","questions":[{"header":"Scope","question":"Fix only the test, or the session code too?","options":[{"label":"Test only","description":"Smallest change"},{"label":"Both","description":"Fix the race"}],"multi":false,"secret":false}],"state":"pending","answers":null}
{"id":"a9","seq":9,"at":null,"kind":"plan","request_id":"r3","markdown":"1. …","state":"pending"}
{"id":"b1","seq":10,"at":1791020400000,"kind":"turn_end","took_secs":95}
{"id":"b2","seq":11,"at":null,"kind":"notice","text":"Switched to Full access"}
{"id":"b3","seq":12,"at":null,"kind":"error","text":"Interrupted"}
{"id":"b4","seq":13,"at":null,"kind":"limit","text":"5-hour limit reached","resets_at":1791030000000}
{"id":"b5","seq":14,"at":null,"kind":"handoff","from":"Claude Opus 5.5","to":"Codex GPT-6"}
```

- `tool`: `command` · `read` · `edit` · `search` · `web` · `agent` · `other` (from the row title,
  like the desktop's `tool_kind`). `status`: `running` · `done` · `failed` · `denied`. `output` is
  truncated by the Mac to its last 4 KiB. `added`/`removed`: line counts for file changes.
- `approval.state`: `pending` · `allowed` · `allowed_for_session` · `denied` · `resolved` (the agent
  moved on by itself). `question.state`: `pending` · `answered` · `resolved`, with `answers` once
  answered. `plan.state`: `pending` · `approved` · `rejected` · `resolved`.
- `user.images`: the number of attached images (phase 2 sends thumbnails).

### Actions (phone → Mac)

All are acknowledged with `{"type":"ack","re":"<id>"}` or answered with an `error`.

```json
{"type":"send","id":"9","thread_id":"01J…","text":"Also add a regression test","mode":"steer"}
{"type":"new_thread","id":"10","project_id":"p1","agent":"codex","model":"gpt-6","text":"Add rate limiting to /login","worktree":true}
{"type":"answer","id":"11","thread_id":"01J…","request_id":"r1","response":{"kind":"approval","decision":"allow"}}
{"type":"answer","id":"12","thread_id":"01J…","request_id":"r2","response":{"kind":"questions","answers":[{"question":"Fix only the test, or the session code too?","answer":"Both"}]}}
{"type":"answer","id":"13","thread_id":"01J…","request_id":"r3","response":{"kind":"plan","approve":false,"feedback":"Skip step 3"}}
{"type":"interrupt","id":"14","thread_id":"01J…"}
{"type":"mark_seen","thread_id":"01J…"}
{"type":"ping","id":"15"}
```

- `send.mode`: `steer` (inject into the running turn) · `queue` (deliver after it). On an idle thread
  both just start a turn. The Mac's own follow-up setting applies when omitted.
- `new_thread` acks with `{"type":"ack","re":"10","thread_id":"01J…"}`; the thread then arrives as a
  `thread` message. `model` null = the project's or agent's default. `worktree` is a request; a
  project that isn't a git repo runs locally.
- `decision`: `allow` · `allow_for_session` · `deny`.
- `ping` → `{"type":"pong","re":"15"}`. The Mac also sends WebSocket pings every 20 s.

### Errors

```json
{"type":"error","re":"9","code":"not_found","message":"No thread 01J…"}
```

Codes: `bad_request` · `unauthorized` · `pairing_failed` · `rate_limited` · `unsupported_protocol` ·
`not_found` · `conflict` (e.g. the request was already answered) · `host_error`.

## Mac side: `crates/trek-remote`

Independent of GPUI. The app implements one trait (or uses the channel adapter):

```rust
pub trait RemoteHost: Send + Sync + 'static {
    fn snapshot(&self) -> impl Future<Output = HostResult<Snapshot>> + Send;
    fn transcript(&self, thread_id: &str) -> impl Future<Output = HostResult<Transcript>> + Send;
    fn send(&self, req: SendRequest) -> impl Future<Output = HostResult<()>> + Send;
    fn new_thread(&self, req: NewThreadRequest) -> impl Future<Output = HostResult<String>> + Send;
    fn answer(&self, req: AnswerRequest) -> impl Future<Output = HostResult<()>> + Send;
    fn interrupt(&self, thread_id: &str) -> impl Future<Output = HostResult<()>> + Send;
    fn mark_seen(&self, thread_id: &str) -> impl Future<Output = HostResult<()>> + Send;
}
```

and pushes changes through `RemoteHandle::push(HostEvent::{Snapshot, Thread, ThreadRemoved, Item,
TranscriptReset})`. `ChannelHost` turns every trait call into a `HostRequest` with a oneshot reply on
an `async_channel`, which a GPUI app drains on its foreground executor. See the crate docs for the
integration checklist.

## iPhone app: `ios/`

SwiftUI, iOS 26 (Liquid Glass), bundle id `dev.trek.TrekMobile`, no third-party dependencies.

- **Threads** (home; the Mac app's word, used throughout the phone UI): large title, sections
  Pinned · Needs you · Working · Recent. Row: agent logo, title, status pill (Trek status colours:
  working ember, approval amber, question indigo, plan violet, done-unseen emerald, failed red) or
  relative time; second line: project monogram in its hue, project name, branch, diff stat. Swipe
  to mark seen. Pull to refresh. Glass "New thread" pill (the tab view's bottom accessory) with
  "N working" count above the glass tab bar: Threads · Settings · Search. The list uses the hard
  scroll edge effect at the bottom, so rows pass cleanly under both bars instead of showing through.
- **Thread**: transcript with Markdown assistant text, collapsed tool groups ("Thought 2 times · ran
  4 commands · edited 1 file"), expandable rows with file chips coloured by file type, approval /
  question / plan cards pinned at the bottom while pending, a "Working… 1m 12s" line. Floating glass
  composer: text, Steer/Queue toggle while working, send, stop.
- **New thread** sheet: "What are we building?", project chip, agent/model chip, worktree chip.
- **Pairing**: scan QR (camera) or type host:port + code + short fingerprint; links and scans go
  through the "Pair with <Mac>?" confirmation. **Demo mode** runs the whole app on realistic
  sample data with no Mac.
- **Settings**: paired Mac, connection state, "Encrypted · pinned" / "Unencrypted", fingerprint,
  follow-up default, demo mode, unpair.

### Implementation notes (v1 as built)

- Before authentication, malformed JSON or any type other than `pair`/`hello` gets `unauthorized`
  and a close; a `pair`/`hello` with bad fields gets `bad_request` and a close. After it, bad input
  gets `bad_request` and the connection stays open.
- The protocol version is checked before the code, so a version mismatch doesn't spend one of the 5
  pairing attempts. `rate_limited` is reserved; burned or expired codes answer `pairing_failed`.
- Actions are always acked (without `re` when they carried no `id`); `unsubscribe` only with an `id`.
- Each connection runs its host calls one at a time in the order sent, off the socket loop, so a
  slow host never stalls pings or pushes and a `send` then `interrupt` can't be reordered.
- Items pushed while a `subscribe` is loading are held and sent right after the transcript (only
  those with a higher seq). A connection that falls behind on events gets a fresh `snapshot` and a
  `transcript_reset` for each subscribed thread. A phone silent for three ping intervals is dropped.
- `devices.json` is written with mode 0600; an unreadable one is moved aside and phones pair again.
- The Mac must set `advertise` (LAN or tailnet address) for the QR code; the bind address
  `0.0.0.0` is no use to a phone.

## Phases and milestones

| Phase | Mac | Phone | Exit |
|---|---|---|---|
| **1a. LAN remote** (this branch) | `trek-remote` crate, protocol v1, pairing, tests | App with demo mode, live client, all screens | Fake host ↔ simulator round trip |
| **1b. Wire it up** | Implement `RemoteHost` in trek-app over Workspace/Store; Settings › Mobile (toggle, QR, devices); stable item seqs; TLS (done in `trek-remote`) | QR scanner, reconnect/backoff, offline banner with queued sends; certificate pinning (done) | Approve a real Claude Code request from the phone |
| **2. Away from the network** | iroh/Noise transport; Trek push + mailbox service; presence-aware alerts; Mac-side step-up for "Allow for session" | APNs + Notification Service Extension with Approve/Deny/Reply; Live Activity; diff view | Approval round trip on cellular in seconds; nothing readable on Trek servers |
| **3. Always on** | `trek-hostd` LaunchAgent; multiple Macs | Multi-host inbox, widgets, Watch | Sessions survive quitting Trek |

## Out of scope (for now)

Editing files or browsing the tree from the phone; terminals; side-by-side diffs; image/file
attachments from the phone; changing hand-holding, settings or accounts remotely; cloud execution;
Android (later: Kotlin on the same protocol); waking a sleeping Mac (show "Mac asleep since 14:02"
instead and queue).

## Open questions

- Typed pairing's 8-hex fingerprint prefix (32 bits) can be ground by an active attacker who
  fetched the Mac's certificate. Options: show 16+ hex characters beside the code, or show the full
  fingerprint on both screens after pairing for comparison.
- Whether vendor terms tolerate a third-party remote steering their CLIs (Happy, Omnara and Moshi do
  it openly; precedent, not permission).
- Seq numbers: the store's `seq` is positional and rewinds truncate. The host should keep a
  per-thread monotonic counter for the protocol (or send `transcript_reset` after rewinds).
