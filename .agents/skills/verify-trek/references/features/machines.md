# Machines (threads on another computer)

## What it does

Pairs Trek with another computer running Trek ("machine B") the way a phone pairs, then starts and follows threads there from this one. The composer's **On this Mac ▾** chip picks, per project, where a new thread runs; **Add to another machine…** clones the project there (SSH or HTTPS), copies the files git ignores that are ticked (an `.env`), and starts the draft's thread. B's threads show in the sidebar under its name and open in the normal thread view: follow-ups (with the files and pictures attached here, sent to B first), Stop, approval/question/plan cards and model or effort changes go to B. Nothing of B's threads is written to this Mac's store; `machines.json` in the data folder keeps the pairing (token, pinned certificate, routes, last seen, which projects were cloned where).

## Reach it as a user

Settings → **Phone & Machines**. On B: turn on "Let your phone connect", choose **Show pairing code**, copy the link under **Pair another machine**. On A: **Machines → Add a machine…**, paste the link (or type address, code and the whole fingerprint), **Pair**. Then in a project's new thread, **On this Mac ▾** → B (or **Add to another machine…**), type, send.

## Reach it with the CLI (the isolated app as its own machine B)

The isolated app can pair with itself: its phone server is B, and the same window is A. Its seeded settings put the phone server on port 7431, clear of the user's own Trek on 7420.

```sh
T=./.agents/skills/verify-trek/scripts/trek-dev
$T start
$T project .                                   # a git project with an origin, for cloning
$T open settings:phone
$T control click mobile-on                     # B: the phone server (wait a moment for it to listen)
$T control pair                                # B: a pairing code; its link is written to <runtime_dir>/shot/pair.txt
LINK="$(cat "$($T status | python3 -c 'import json,sys; print(json.load(sys.stdin)["runtime_dir"])')/shot/pair.txt")"
$T control scroll settings-content 3000
$T control click machine-add                   # A: "Add a machine…" (the link field has the keys)
$T control type "$LINK"
$T control click pair-submit
$T control elements                            # machine-row-<host id>, machine-dot-<host id>, machine-forget-<host id>
$T screenshot machines-settings
$T project .
$T control click machine-chip                  # "On this Mac ▾"
$T control click machine-pick-<host id>        # or machine-pick-here, machine-add-to
$T control type "Show a short streamed answer" # the composer's (not `send`, which always runs here)
$T control key enter                           # starts on B; the thread opens here under B's name
$T control elements                            # machine-head-<host id>, live-line-machine:<host id>:<thread id>, env-machine
$T screenshot machines-thread
```

(`status` prints `runtime_dir`; reading `shot/pair.txt` under it by hand works as well.) The pairing sheet's ids: `pair-sheet`, `pair-mode#0` (link) / `pair-mode#1` (typed), `pair-link-input`, `pair-host-input`, `pair-code-input`, `pair-fp-input`, `pair-submit`, `pair-cancel`, `pair-busy`, `pair-error`. Settings shows this Mac's own link as `pair-link` with `pair-link-copy`.

**Add to another machine…** (`machine-add-to` in the chip's menu) opens `clone-sheet`: `clone-source`, `clone-url` (or `clone-no-origin`), `clone-scheme#0` SSH / `clone-scheme#1` HTTPS, `clone-target#<n>`, `clone-folder-input`, `clone-files` with `clone-file#<n>` (click to tick), `clone-progress`, `clone-error`, `clone-cancel`, `clone-submit`. Cloning to the isolated app itself really runs `git clone` into the folder shown (default `<home>/<repo>`, the isolated home): pick a throwaway folder, and it needs an origin the machine can fetch (an SSH origin needs a key; HTTPS works for public repos).

### Files and pictures to a thread on B

What's attached here (the + menu, a drop, a paste, a screenshot or photo, and files of this computer typed as `@/path` or `@~/path`) goes to B ahead of the message, in a new thread's first message and in a follow-up alike (B on Trek 0.4.61 or later, feature `machine_files`). Send leaves the draft as it is while they go: each tile or thumbnail fills along its foot (`attachment-sending#<n>`, `attachment-file-sending#<n>`), `machine-upload` under the strip says "Sending 2 files to <B> · 1.2 MB of 2.6 MB", and `machine-upload-cancel` stops them (so do taking a file off and moving to another thread). Once all are in, the message goes: B's copy of each file mentioned in place of this computer's path, each picture attached there as an image. B keeps them in its data folder's `uploads/` as `<this computer's name> <date> at <time> <name>`, at most 2 GB a file. Here the transcript shows this computer's own files again (the file chip, a video that plays, the picture).

A folder goes as one zip of its files (0.4.68: what git ignores and `.git` stay behind; "Packing a folder to send to <B>…" while it's made), named `<folder> folder.zip`.

What can't go says why in a toast and leaves the draft: B on an older Trek ("<B> needs Trek 0.4.61 or later to receive files…"; pictures alone still go to it inline, as before), a file or folder over 2 GB, a folder with nothing to send, a file gone, B offline, or what B refused (no room, say) as "Couldn't send <file> to <B>: …".

With the isolated app as its own machine B, in one go:

```sh
$T start
$T open no-project                              # a new thread in no project
$T control pair-self                           # pairs with its own phone server (wait for it)
$T control wait 3000
$T control machine-draft                       # the draft starts on B (`machine-draft off`: here)
$T control attach "$PWD/README.md|$PWD/docs/screenshot.png"   # a file and a picture, as a drop
$T control type "What do these say?"
$T control key enter
$T control wait 3000
$T control elements                            # machine-head-<host>, user-file#<n> under the message
$T control file "uploads/*README.md"           # B's copy, in the data folder's uploads/
$T control pair-self leave                     # forget B, revoke the pairing, archive what it started
```

While the app is paired with itself, Devices lists it as a machine with every thread of the app's own (they're all B's too), which pushes a phone's row down: `pair-self leave` puts it back as it was. A project with no threads yet isn't one B lists, so a new thread in it can't start there ("<project> isn't on <B> yet…", said before anything is sent); `$T open no-project` starts one in no project instead.

### Sub-agents and background work of a thread on B

A thread on B shows what it has out beyond its turn as the same thread does at home (Trek 0.4.64; until then only the sidebar's badge did): a row per sub-agent in the turn that started it (`subagent#<n>`, its label, status line and steps, running/done/failed, its clock; opened, its latest calls), with Stop (`subagent-stop#<n>`) and, where its agent takes one, a message (`subagent-message#<n>`), both going to B; Trek's own (`delegate_task`) opens its thread on B (`subagent-open#<n>`; the thread comes as B's `unlisted` row and is never in the sidebar's list), whose own sub-agents show the same way. The working bar says "Waiting on 2 sub-agents" (`waiting-on`, its list `sub-agents-at-work`), the hub has "Sub-agents · 2" (`hub-subagents`), and the strip above the composer lists B's background commands (`background-strip`, `bg-task#<n>`; output read from B, Stop goes to B; no "Full output", B's file can't open here). The sidebar's badge, the working bar and the rows read one list, so they agree.

```sh
$T open no-project
$T control pair-self
$T control wait 3000
$T control machine-draft
$T control type "Scout the code with subagents 240s"   # the mock's two background scouts (`mock:delegate subagents 60s` for Trek's own with its own)
$T control key enter
$T control wait 4000
$T control elements                                     # subagent#2, subagent#3, waiting-on, hub-subagents
$T control click "subagent#2"                           # after the first step (a third of the time): its calls
$T screenshot machine-sub-agents
$T control pair-self leave
```

A B before 0.4.64 sends no `activity` (an opened row lists no calls) and no `continued` (no onward arrow for a sub-agent woken again); a B before `sub_agent_view` sends no links (rows read from the call alone, no Stop for the agent's own).

### Everything else from here (0.4.68)

What a thread of this computer offers, a thread on B offers too, with B doing the work: the
message's **Edit and send again** (`edit-user#<n>`), **Rewind to here** (`rewind-user#<n>`, its
confirmation `confirm-card` listing the files B would put back, `restore-files`), **Fork**
(`fork-user#<n>`, `fork-turn#<n>`), the turn footer's Undo and Retry, the thread menu's
**Snooze**, **Wake now** and **Delete…** (B words the confirmation), and a **secret** B's agent
asks for (its card here, `secret-value`; B on Trek 0.4.68 or later, feature `secret_answer`).
A **folder** attached here goes as one zip (`<folder> folder.zip` in B's `uploads/`). A long
thread reads its start from B in pages after it opens. The model menu lists B's agents and
models, and the access menu shows B's own Full access lock ("Locked on <B>…"). B's threads that
start waiting on you, finish unread or fail raise this computer's banner and sound.

Until 0.4.67 each of the first seven said "Do that in Trek on <B>", a folder said "Folders can't
go to <B> yet", and a secret was a notice ("Needs GITHUB_TOKEN. Enter it on <B>.").

```sh
$T open no-project
$T control pair-self
$T control wait 3000
$T control machine-draft
$T control type "hello there"
$T control key enter
$T control wait 4000
$T control hover "user-msg#0" // click "rewind-user#0"   # one batch (`//`): the message's actions show only while the pointer is over it. confirm-card: what B says it would put back
$T control click "confirm-go"                  # B rewinds; "hello there" is back in the composer
$T control key enter                            # sent again
$T control wait 4000
$T control secret-demo                          # B's agent asks for GITHUB_TOKEN (pair-self: B is this app)
$T control wait 1500
$T control click secret-value                   # the card here (secret-value, secret-save, secret-decline)
$T control type not-a-real-secret
$T control click secret-save                    # it goes to B, which saves it for the thread
$T control attach "$PWD/docs"                   # a folder, as a drop
$T control type "what's in here?"
$T control key enter                            # "Packing a folder to send to <B>…", then it goes
$T control wait 5000
$T control file "uploads/*docs folder.zip"      # B's copy
$T control pair-self leave
```

With the app paired with itself the same thread is in the sidebar twice (its own, and as B's
under Devices), and each of B's alerts comes twice (this app is both computers).

**Show more** on a long message of yours (`user-more#<n>`, the text `user-text#<n>`): closed, ten
lines, every one wrapped; open, the whole message, wrapped. `type` a paragraph, `key shift-enter`,
another, six times, `key enter`, then `click "user-more#0"`. (0.4.67: "Show less" over the same cut
text, the paragraphs past the tenth line each one line running off the bubble; local threads too.)

## Code

`crates/trek-app/src/machines.rs` (registry, `machines.json`, the client per machine, mapping B's rows and items to `Thread`/`Item`/`PendingPermission`, the actions), `machines/clone.rs` (origin URL, ignored files, uploads), `machines/files.rs` (what goes with a message, why it can't, the message shaped for B, B's copies shown as the files here), `machines/sheets.rs` (Add a machine…, Add to another machine…), `machines/turns.rs` (rewind, undo, retry, edit, fork, snooze, delete: `turn_action`, `turn_preview`, `thread_action`), `machines/earlier.rs` (the start of a long thread, in pages), `machines/prefs.rs` (B's agents, models and Full access lock for the composer), `machines/sub_agents.rs` (steering B's sub-agents and background commands; the rows are `SubAgentRow::of_machine` in `thread_view.rs`, the list `Workspace::machine_sub_agents` in `workspace/sub_agents.rs`), `composer/machine_chip.rs`, `composer/machine_upload.rs` (the attachments on their way, progress, Cancel), the Machines group in `settings_view/mobile.rs`, the machine groups in `sidebar.rs`, the `machines::is_remote` guards in `workspace.rs`. Client: `crates/trek-remote/src/client.rs` (`Client::upload_file`); host side: `remote.rs`, `remote/machines.rs`, `remote/uploads.rs`. Design: `docs/MULTI-MACHINE.md`; wire: `docs/MOBILE.md` (`uploads`, `machine_files`). Tests: `crates/trek-app/tests/machines.rs` (`cargo test -p trek-app --test machines`), `machines/files.rs`'s own, `crates/trek-remote/tests/client.rs` (`a_file_goes_up_in_pieces_and_comes_out_whole`), `crates/trek-app/tests/remote.rs` (`another_machines_files_…`); B's sub-agents and background work: `tests/machines.rs` (`a_thread_on_another_machine_shows_its_agents_sub_agents_at_work_as_it_does_at_home`, `trek_s_own_sub_agent_on_another_machine_opens_here_with_its_own_and_stops_from_here`, `background_work_of_a_thread_on_another_machine_shows_reads_and_stops_from_here`) and `crates/trek-remote/tests/protocol.rs` (`a_sub_agent_link_carries_its_steps_and_where_it_went_and_older_links_still_read`).

## Verify

Pairing needs the code B shows (one use, minutes); the token never appears in logs or the UI; `machines.json` is readable by its owner only. The dot is green while B answers and the row says when it was last seen when it doesn't. A thread started on B runs there (B's own sidebar has it) and streams here; while it works, its row here, under B's name in Devices (`machine-head-<host>`, its count `machine-count-<host>` on the same right-hand column as a project's), has the turn clock (`live-line-working-<id>`), the pill of its sub-agents (`live-line-kids-<id>`, `live-line-kids-count-<id>`, named in the tooltip) and, its turn over with them still out, `live-line-waiting-<id>`; the tooltip names its provider, model and each sub-agent by provider; what waits on the user there is in the Inbox ("<project> on <machine>"); Stop, Allow/Deny, plan approval and effort changes take effect on B once. Forgetting B removes its row, its threads here and its entry in `machines.json`, and the project's chip goes back to this Mac. Nothing under `machine:` ids is in this Mac's database. A file attached here reaches B byte for byte under B's `uploads/` (never elsewhere there: its name is made safe and nothing is written over), B's agent is given B's path, and the message here still shows this computer's file.
