# Trek Mobile — Design

> Status: phase 1 in progress (2026-10). Mac side: `crates/trek-remote`. iPhone app: `ios/`.

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
   code: the first 16 hex characters, uppercased, as `ABCD-1234-EF56-7890`. A link without `fp` comes from a
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
- **What is pinned.** From a QR code or link, the full `fp`. From typed pairing, the 16 hex
  characters the user entered (`ABCD-1234-EF56-7890`, case, spaces and dashes ignored) match any
  certificate whose fingerprint starts with them; the full fingerprint seen during that handshake is
  what gets kept. 16 characters are 64 bits: the fingerprint isn't secret (anyone on the network can
  fetch the certificate), but making a certificate whose fingerprint starts the same takes about 2^63
  tries, far beyond the ten minutes a code lasts. (The first design showed 8 characters, 32 bits,
  which a GPU could match in minutes.)
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
- **Capabilities, not a remote shell.** A phone can't reach Full access unless the Mac has unlocked
  it (`settings.permissions.full_access_unlocked`), whichever way it asks (`set_prefs`, `new_thread`,
  `/permissions full`, `/access`, the default for new threads). It changes only an allowlist of
  settings (never the unlock, API keys or the phone server), reads only changed files' diffs (not
  arbitrary files), runs no shell commands, and git work that would lose something (uncommitted
  changes, unmerged commits) is refused until the phone says `force`. "Allow for
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
  `direct:openai`…). `agent.logo` (rows, `agents[]`, everywhere an agent is named): the Mac's logo
  key, its `assets/logos/{dark,light}/<logo>.png` (`claude-code`, `codex`, `opencode`, `droid`,
  `cursor`, `gemini`, `openai`, `anthropic`…); absent: a neutral glyph.
- `activity`: what a working thread is doing now ("Editing src/auth.rs"), else null.
- `agents[].default_model`: the model the Mac's model menu starts on for that agent (Opus 5.5 for
  Claude Code, else the first). Switching a thread's agent (`set_prefs {agent}`) moves it to that
  model, and its effort to the nearest the model takes.

Thread details, each optional and left out when there's nothing to say (older Macs send none):

```json
{"effort":"xhigh","effort_label":"Extra high","access":"auto","plan":false,
 "context":{"used":171000,"window":200000,"percent":85},
 "cost":{"label":"≈ $1.84 at API prices","billing":"plan","plan":"Claude Max","detail":"Included in your Claude Max plan"},
 "sub_agents":[{"agent":{"key":"codex","name":"Codex","logo":"codex"},"model":"Sol","title":"Review the theme tokens","state":"running","since":1791020300000},
               {"agent":{"key":"claude-code","name":"Claude Code","logo":"claude-code"},"title":"Find hard-coded colours","state":"running","since":1791020355000}],
 "background":["pnpm dev"],
 "branch":"trek/dark-settings","worktree":true,"base":"main",
 "git":{"changed":5,"ahead":1,"behind":2,"default_branch":"main"}}
```

- `context` and `cost` are the composer's context ring and the cost line under it, as the
  thread's session last reported them: present once the thread has been opened (on the Mac or by
  a phone's `subscribe`). `cost.label` is worded exactly as the Mac's: `$1.24` (billed per token),
  `≈ $1.24 at API prices` (a plan covers it), `12.3K tokens · price unknown`, `202K tokens · free`;
  `billing` is `plan` · `metered` · `local`, `detail` the Mac's tooltip.
- `sub_agents`: what the sidebar card's pill and tooltip count: Trek's sub-agents (with their
  `model`) and the agent's own (no `model`), `state` `running` · `needs_you` · `done` · `failed` ·
  `stopped`, `since` when it started (ms, to the second). `background`: the rest of what the agent
  runs in the background, by title.
- `base`: for a worktree thread, the branch it merges into (`branch` is the worktree's own).
- `git`: the folder's state as the Mac last read it (it reads it again on `git_status`).

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
{"type":"subscribe","id":"7","thread_id":"01J…","after_seq":null,"limit":200}
{"type":"unsubscribe","thread_id":"01J…"}
```

`limit` (optional) is for long threads: a full transcript (`reset: true`) then holds only the last
`limit` items (`i…`), each turn's changed files after its end (`c…`, not counted), and every open
request (`r…`, always), with `"more":true` when earlier items were left out. Replies to an
`after_seq` the Mac can serve aren't limited (and have no `more`).

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

Earlier items come in pages, oldest last-asked first; a read, so it never waits behind an action:

```json
{"type":"transcript_before","id":"8","thread_id":"01J…","before":"i340","limit":200}
{"type":"transcript_page","re":"8","thread_id":"01J…","items":[ …i140…i339, with their c… items… ],"more":true}
```

`before` is an item id (`i<n>`, or `c<n>`: up to and with `i<n>`); the page holds the `limit` (at
most 500) items just before it, in order, with their turns' changed files, and `more` says whether
still earlier ones exist. Unknown ids answer `not_found`. Items in a page are as they are now, with
the seq they were last sent with. The Mac only follows items some phone was sent: an `item` may
still arrive for an id earlier than the phone's first (another phone paged further back); a phone
that doesn't hold it can drop it (paging it in brings it as it is).

`seq`s carry on across subscriptions, also after the last phone let go of the thread: re-subscribe
with `after_seq` to get only what changed since. They start at the time the Mac first served the
thread (in µs), so a seq from before Trek restarted gets the transcript whole.

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
{"id":"c10","seq":15,"at":1791020400000,"kind":"changes","files":[
   {"path":"README.md","status":"modified","added":31,"removed":54},
   {"path":"docs/quick-start.md","status":"renamed","from":"docs/getting-started.md","added":4,"removed":2},
   {"path":"docs/img/docker.png","status":"deleted","added":0,"removed":0,"binary":true}],"added":35,"removed":56}
```

- `changes`: the files a turn changed, right after its `turn_end` (`c<n>` follows `i<n>`), with
  totals. `status`: `added` · `modified` · `deleted` · `renamed` (with `from`); `binary` files have
  no line counts. It may arrive a moment after the turn end (worked out from the turn's
  checkpoints), and may be replaced as the Mac learns more.

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
- Trek's own commands work in `send` as typed on the Mac (`/permissions edits`, `/usage`, `/context`,
  `/cost`, `/model`, `/consult sol high: …`, `/restate …`); the Mac's reply arrives as a `notice`
  item. Full access needs the Mac's unlock however it's asked for (the notice says so). `/new` and
  `/clear` don't move the Mac's window: the ack asks the phone to open its new-thread sheet,
  `{"type":"ack","re":"9","open":{"screen":"new_thread","project_id":"p1"}}`. A thread can start
  (`new_thread`) with `/consult` or `/restate`; the other commands are for a thread under way.
- `new_thread` acks with `{"type":"ack","re":"10","thread_id":"01J…"}`; the thread then arrives as a
  `thread` message. `model` null = the project's or agent's default. `worktree` is a request; a
  project that isn't a git repo runs locally.
- `decision`: `allow` · `allow_for_session` · `deny`.
- `turn_action`: what a turn's footer and a message's actions do on the Mac, after its
  confirmation:

  ```json
  {"type":"turn_action","id":"16","thread_id":"01J…","item_id":"i412","action":"undo","restore_files":true}
  {"type":"turn_action","id":"17","thread_id":"01J…","item_id":"i412","action":"retry","model":"gpt-6"}
  {"type":"turn_action","id":"18","thread_id":"01J…","item_id":"i412","action":"fork"}
  {"type":"turn_action","id":"19","thread_id":"01J…","item_id":"i405","action":"rewind"}
  ```

  `undo` · `retry` (optionally with another of the agent's `model`s) · `fork` take the item that
  ends a turn (its `turn_end`; a turn that failed, hit a limit or was interrupted ends with that
  `error`/`limit`/`notice` instead). `rewind` takes one of the user's messages (`user`) and goes
  back to just before it; `fork` of a message forks from just before it, as the message's own fork
  button does. `restore_files` (default `true`) also puts the files back as they were when the
  message was sent, when Trek has a checkpoint for it. Acks: `undo` and `rewind` carry the message
  taken back, for the composer (that's "Edit"): `{"type":"ack","re":"16","text":"Fix the parser"}`;
  `fork` the new thread, `{"type":"ack","re":"18","thread_id":"01K…"}` (and `text`, from a
  message); `retry` a plain ack. The Mac's screen doesn't move. Refused, with the Mac's words, as
  `bad_request`: while a turn runs ("Stop the running turn to undo" / "…to retry" / "…to rewind"),
  a turn no message of the user's started ("This turn didn't start from a message of yours"), an
  item the action doesn't take; `not_found` for an unknown thread, item or model.
- `ping` → `{"type":"pong","re":"15"}`. The Mac also sends WebSocket pings every 20 s.

### Everything else the Mac has (phone → Mac, answered with a reply of its own)

Reads (`usage`, `basecamp`, `notes`, `note`, `git_status`, `git_diff`, `git_branches`, `commands`,
`settings`) may be answered out of order with what the phone sends after them; actions keep their
order. A Mac too old for one answers `bad_request` ("Unknown message type").

**Usage**, as the Mac's Usage popover shows it (asking refreshes it as opening the popover does: the
agents at most every 30 s, Devin every 10 min; the Mac waits up to 15 s for them, and `loading`
says one hadn't answered yet):

```json
{"type":"usage","id":"1"}
{"type":"usage","re":"1","providers":[{"agent":{"key":"claude-code","name":"Claude Code","logo":"claude-code"},"plan":"Claude Max",
  "limits":[{"label":"5-hour limit","percent":42.0,"resets_at":1791030000000,"window":"5h"},{"label":"Weekly · Opus","percent":64.0,"resets_at":1791370000000,"window":"7d"}]}]}
```

**Basecamp**, worked out off the main thread as the Mac's is, worded as it words it:

```json
{"type":"basecamp","id":"2","range":"today"}
{"type":"basecamp","re":"2","range":"today","greeting":"Good evening, Monday 5 October","title":"Today's trek","updated_at":1791020400000,
 "review":[{"thread_id":"t","title":"Fix the flaky test","status":"needs_you","label":"Approval","agent":{…},"project":{…},"additions":4,"deletions":1,"updated_at":…,"unseen":true}],
 "narrative":[{"kind":"text","text":"You sent "},{"kind":"strong","text":"18 prompts"},{"kind":"text","text":" across "},…,
              {"kind":"project","text":"trek-api","project":{…}},{"kind":"model","text":"Claude Opus 5.5","agent":{…}}],
 "summary":{"prompts":18,"threads":4,"turns":20,"agent_secs":3720,"agent_time":"1h 2m","tokens":182000,"failed":1,
            "top_project":{"project":{…},"prompts":12,"tokens":90000},"best_model":{"agent":{…},"label":"Claude Opus 5.5","tokens":140000,"turns":9,"share":77}},
 "profile":{"buckets":[{"value":12.5,"label":"2–3 PM","line":"2–3 PM · 4 prompts · 12m of agent time","prompts":4,"agent_secs":750,"tokens":3000},…],
            "summit":14,"now":20,"now_at":0.86,"line":"Summit at 2 PM","total":"18 prompts","ticks":[{"at":0.25,"label":"6 AM"},…]},
 "tiles":[{"kind":"best_model","label":"Your best model","figure":"Claude Opus 5.5","note":"77% of tokens · 9 turns","agent":{…}},
          {"kind":"tokens","label":"You used","figure":"182K tokens","note":"≈ $1.28 at API prices today","sparkline":[0.0,0.1,…,1.0]},
          {"kind":"plan_left","label":"Left on Claude Max","figure":"36%","note":"Weekly · Opus · resets in 4d","agent":{…},"percent":36.0,"resets_at":…}]}
```

`range`: `today` · `week` · `all`. `empty: true` with `invitation` when nothing happened in the
range. Bucket `value` is agent minutes when turns were timed, else prompts; `line` is what the Mac
shows over a hovered stretch. Tile kinds: `best_model` · `worked_most_on` · `tokens` (with
`sparkline`, tokens so far 0–1 a stretch) · `agent_time` (failed turns in its note) · `plan_left`
(one per agent reporting limits, `percent` left).

**Notes**, the Mac's markdown files (`notes/` in Trek's data folder; deleting moves one to
`notes/Deleted/`). The Mac's Notes screen reads them again when a phone changes one.

```json
{"type":"notes","id":"3"}                         → {"type":"notes","re":"3","notes":[{"id":"0192…","title":"Groceries","preview":"milk bread","modified":1791020400000}]}
{"type":"note","id":"4","note_id":"0192…"}        → {"type":"note","re":"4","note":{"id":"0192…","title":"Groceries","body":"Groceries\n- milk","modified":1791020400000}}
{"type":"create_note","id":"5","body":"Groceries"} → note
{"type":"save_note","id":"6","note_id":"0192…","body":"…","modified":1791020400000} → note (conflict: it changed on the Mac since)
{"type":"delete_note","id":"7","note_id":"0192…"} → ack
```

**Git** for a thread's folder (its worktree, for one in a worktree) or a project, as the Mac's Git
panel does it; git runs off the Mac's main thread.

```json
{"type":"git_status","id":"8","thread_id":"t"}    (or "project_id":"p1")
{"type":"git_status","re":"8","is_repo":true,"branch":"trek/fix","default_branch":"main","ahead":2,"behind":0,"has_upstream":false,
 "files":[{"path":"src/a.rs","status":"modified","added":12,"removed":3},{"path":"tests/new.rs","status":"untracked","added":18,"removed":0}],
 "worktree":{"branch":"trek/fix","base":"main","uncommitted":1,"merge_blocked":"1 file isn't committed yet. Commit or revert it first.","unmerged":2},
 "can_switch":false,"switch_blocked":"This thread works in a worktree: trek/fix stays checked out there. Merge it into main instead."}
{"type":"git_diff","id":"9","thread_id":"t","path":"src/a.rs"}   → {"type":"git_diff","re":"9","path":"src/a.rs","diff":"diff --git …","truncated":false}
{"type":"git_commit","id":"10","thread_id":"t","message":"Fix the race"}   → ack (commits everything, as the panel does)
{"type":"git_push","id":"11","thread_id":"t"}                               → ack
{"type":"git_branches","id":"12","project_id":"p1"}  → {"type":"git_branches","re":"12","current":"main","default_branch":"main","branches":["main","dev"]}
{"type":"git_switch","id":"13","project_id":"p1","branch":"dev"}            → ack
{"type":"worktree_merge","id":"14","thread_id":"t"}                         → ack, or conflict saying why (the Mac's words)
{"type":"worktree_remove","id":"15","thread_id":"t","delete_branch":false,"force":false} → ack, or conflict saying what would be lost
```

- A diff is only served for one of the folder's changed files, cut at 256 KiB (`truncated`).
- `git_switch` takes a local branch only, and is refused (`conflict`) in a worktree thread or while
  a thread works in that folder (`can_switch`/`switch_blocked` say so beforehand).
- `worktree_remove` without `force` is refused when it would lose uncommitted changes, or (with
  `delete_branch`) commits the base doesn't have: the message is the Mac's confirmation text ("1
  uncommitted change in the worktree would be lost. trek/fix has 2 commits that main doesn't
  have."). Sending it again with `force` is the user agreeing; no more uncommitted changes go than
  were counted then. The thread carries on in the project folder, as on the Mac.
- `worktree_merge` merges into the base in the project folder (and settles the thread when the
  Mac's settings say so); blocked merges answer `conflict` with the reason.

**Slash commands**, as the composer's `/` picker lists them for the thread: Trek's own first
(`trek: true`), then the agent's commands, skills and agents in its folder.

```json
{"type":"commands","id":"16","thread_id":"t"}
{"type":"commands","re":"16","thread_id":"t","commands":[{"name":"permissions full","description":"No prompts and no sandbox","kind":"command","trek":true},
  {"name":"compact","description":"…","kind":"command"},{"name":"frontend-design","description":"…","kind":"skill"}]}
```

**Settings** the phone may see and change (an allowlist; one value the Mac won't take refuses the
whole change):

```json
{"type":"settings","id":"17"}
{"type":"settings","re":"17","default_agent":"claude-code","default_effort":"high","default_access":"auto-accept-edits","follow_up":"steer",
 "notifications":"banner_and_sound","push":{"enabled":true,"when":"away","server":"https://ntfy.sh","topic":"trek-…",
 "topic_url":"https://ntfy.sh/trek-…","subscribe_url":"ntfy://ntfy.sh/trek-…"},"auto_settle_days":3,"theme":"system","full_access":false}
{"type":"set_settings","id":"18","default_agent":"codex","default_model":"gpt-6","default_effort":"high","default_access":"auto",
 "follow_up":"queue","notifications":"banner","push":true,"push_when":"always","push_server":"https://ntfy.example.com",
 "new_push_topic":true,"auto_settle_days":7,"theme":"paper"}       → settings, as they are now
```

- `default_model: ""` goes back to the agent's default; `default_access: "full-access"` needs the
  Mac's unlock. `full_access` is read-only. Turning `push` on makes a topic the first time.
- `push_test: true` sends a test notification once the rest is changed (refused while `push` is
  off): what the phone's "Send a test" does.
- ntfy: `topic_url` opens the topic in ntfy's web app. `subscribe_url` (`ntfy://<host>/<topic>`,
  `?secure=false` for an `http://` server) is ntfy's subscribe link, documented for its Android
  app only; the iOS app (Philipp Heckel's, checked against its source as of 2026-06) registers no
  URL scheme or universal links, so no link can subscribe it. On an iPhone: copy the topic, then
  in ntfy tap +, paste it (and pick "Use another server" for one that isn't ntfy.sh).

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
    // Defaults answer "This Mac can't …" (bad_request):
    fn set_prefs, thread_action, usage, basecamp, notes, note, create_note, save_note, delete_note,
       git_status, git_diff, git_commit, git_push, git_branches, git_switch, worktree_merge,
       worktree_remove, commands, settings, set_settings, turn_action
    // Defaults built on `transcript`:
    fn transcript_for(&self, thread_id, after_seq, limit)   // what `subscribe` calls
    fn transcript_before(&self, thread_id, before, limit)   // a page, cut from the whole transcript
    // Called (not awaited) when a thread's last subscriber, across every phone, unsubscribes or
    // disconnects: the host can stop following it. In order with the calls after it.
    fn unwatch(&self, thread_id: &str) {}
}
```

`transcript_for` may serve just what the phone needs: items newer than `after_seq` when
`Transcript::base <= after_seq <= seq`, else the last `limit` items with `more` set. The server
cuts what it gets to the same shape either way, so a host can return everything.

(`send` returns `Option<Open>`: a screen the phone opens, for `/new`.) A new message type needs
its name in `server.rs`'s `CLIENT_TYPES` too: anything not listed there is refused as unknown
before it's parsed (`tests/server.rs` sends every one over the wire).

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

Editing files or browsing the tree from the phone; terminals; side-by-side diffs; file attachments
from the phone; unlocking Full access, API keys, accounts and the phone server remotely; cloud execution;
Android (later: Kotlin on the same protocol); waking a sleeping Mac (show "Mac asleep since 14:02"
instead and queue).

## Open questions

- Whether vendor terms tolerate a third-party remote steering their CLIs (Happy, Omnara and Moshi do
  it openly; precedent, not permission).
- "Allow for session" asks for Face ID on the phone only; the Mac takes the phone's word.

## The Mac side as built (Trek 0.3.4)

- **Settings › Phone** turns the server on (off by default; `[mobile]` in settings.toml: `enabled`,
  `port` 7420, `reach` wifi or tailscale). It binds `0.0.0.0:<port>` with TLS from
  `mobile/identity.{der,key}` in Trek's data folder (the key 0600), keeps paired devices in
  `mobile/devices.json`, and advertises the Wi-Fi or Tailscale address in the pairing code. The page
  shows the QR code, the code and the 16-character fingerprint, and the paired phones with Unpair.
- **Requests** (`crates/trek-app/src/remote.rs`) come through `ChannelHost` and are answered on the
  main thread from the workspace. Transcript items are numbered by index (`i<n>`), requests as
  `r<request id>`; each thread keeps its own `seq`, bumped for every new or changed item; a
  transcript shorter than what was sent (a rewind) sends `transcript_reset`. Answered requests are
  re-sent with state `resolved`.
- **Changes** go out every 250 ms while a phone is connected: thread rows that changed (and
  removals), and the items of transcripts a phone has open. A tick costs what changed, not how
  long the thread is: a thread whose transcript didn't move costs nothing, and one that did is
  looked at from the first item edited on (`Transcript::take_edited_from`, which every change goes
  through; plus the item streaming before and now, and everything if the thread's folder moved).
  Requests are compared when the revision moved or they differ; turns' changed files are counted
  for turns that just ended, when a count comes in (`WorkspaceEvent::TurnChanges`), or while one
  waits. Threads no phone has open are dropped from the tick (`unwatch`) and kept as last sent, so
  a phone opening one again carries on (seq, and a delta for its `after_seq`). Items no phone was
  sent (left out by a `limit`) aren't followed until a page asks for them.
- **Turn actions** (`turn_action`) call the same `Workspace` methods as the buttons:
  `undo_turn`, `retry`, `rewind` (files restored when asked and `restorable_checkpoint` has one),
  and `fork_quietly`, `fork_thread` without opening the fork.
- **Timing** (a 3000-item thread, test profile, `tests::remote::timing_of_a_long_thread`): a
  subscribe with `limit: 200` takes 0.2 ms (the whole transcript 13 ms), a page of 200 0.2 ms, a
  re-subscribe with nothing new 5 µs; a tick 2–8 µs idle and 4 µs while streaming (before: 2 ms
  re-subscribe, 0.35 ms idle tick, 1.6 ms streaming tick, and every thread ever opened stayed in
  the tick).
- **Mock history**: `mock:history <n>` (with `TREK_MOCK_AGENT=1`) plays `n` rounds of five items at
  once (a thought, a read, a search, a command, an edit with its lines, an answer with code and a
  table): `mock:history 500` makes a 2500-item thread for trying all this end to end.
- **Trek's own slash commands** run from the phone as typed on the Mac (`remote/commands.rs`):
  `/permissions` and its aliases through `set_hand_holding`, which keeps Full access behind the
  unlock; `/consult` and `/restate` rewritten as the composer does; `/new` and `/clear` answered
  with `open` for the phone's sheet. A thread started from the phone doesn't move the Mac's screen
  or open a tab.
- **Rows** carry the composer's context ring and cost line (once the thread is loaded), the effort
  label, sub-agents and background work (as the sidebar card counts them), the worktree's base and
  the folder's git state. Times come from `Instant`s through a fixed anchor, so an unchanged row
  isn't sent again every tick.
- **Usage** (`remote/usage.rs`), **Basecamp** (`remote/basecamp.rs`, sharing `basecamp.rs`'s
  wording: `greeting_line`, `tile_data`, `profile_line`…), **notes** (`remote/notes.rs`, bumping
  `Workspace::notes_epoch` so the Notes screen reloads), **git** (`remote/git.rs`, the Git panel's
  `snapshot`/`git` and `trek_core::worktree`'s review, merge and removal, the confirmations'
  wording from `worktree_ui::losses`), **settings** (`remote/settings.rs`).
- **Changed files per turn** go out as `changes` items from one function, `turn_changes`, which
  answers `None` until the Mac works them out (`Workspace::turn_changes`, another branch: marked
  `TODO(merge)`).
- `cargo run -p trek-remote --example probe -- '<trek://pair link>' [--thread <id> [--limit 200]
  [--before first] [--send "…"] [--answer allow|deny]]` is a phone in a terminal, for testing a
  running Trek (`--limit` and `--before` open a thread at its end and page back to its start,
  timing each). It sends only to a
  thread named explicitly: a message reaches a real agent in that thread's folder.
