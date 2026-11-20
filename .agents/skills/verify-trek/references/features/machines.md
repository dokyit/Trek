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

What can't go says why in a toast and leaves the draft: B on an older Trek ("<B> needs Trek 0.4.61 or later to receive files…"; pictures alone still go to it inline, as before), a folder ("Folders can't go to <B> yet…"), a file over 2 GB, a file gone, B offline, or what B refused (no room, say) as "Couldn't send <file> to <B>: …".

With the isolated app as its own machine B, in one go:

```sh
$T start
$T project .
$T control pair-self                           # pairs with its own phone server (wait for it)
$T control wait 3000
$T control machine-draft                       # the draft starts on B (`machine-draft off`: here)
$T control attach "$PWD/README.md|$PWD/docs/screenshot.png"   # a file and a picture, as a drop
$T control type "What do these say?"
$T control key enter
$T control wait 3000
$T control elements                            # machine-head-<host>, user-file#<n> under the message
$T control file "uploads/*README.md"           # B's copy, in the data folder's uploads/
```

## Code

`crates/trek-app/src/machines.rs` (registry, `machines.json`, the client per machine, mapping B's rows and items to `Thread`/`Item`/`PendingPermission`, the actions), `machines/clone.rs` (origin URL, ignored files, uploads), `machines/files.rs` (what goes with a message, why it can't, the message shaped for B, B's copies shown as the files here), `machines/sheets.rs` (Add a machine…, Add to another machine…), `composer/machine_chip.rs`, `composer/machine_upload.rs` (the attachments on their way, progress, Cancel), the Machines group in `settings_view/mobile.rs`, the machine groups in `sidebar.rs`, the `machines::is_remote` guards in `workspace.rs`. Client: `crates/trek-remote/src/client.rs` (`Client::upload_file`); host side: `remote.rs`, `remote/machines.rs`, `remote/uploads.rs`. Design: `docs/MULTI-MACHINE.md`; wire: `docs/MOBILE.md` (`uploads`, `machine_files`). Tests: `crates/trek-app/tests/machines.rs` (`cargo test -p trek-app --test machines`), `machines/files.rs`'s own, `crates/trek-remote/tests/client.rs` (`a_file_goes_up_in_pieces_and_comes_out_whole`), `crates/trek-app/tests/remote.rs` (`another_machines_files_…`).

## Verify

Pairing needs the code B shows (one use, minutes); the token never appears in logs or the UI; `machines.json` is readable by its owner only. The dot is green while B answers and the row says when it was last seen when it doesn't. A thread started on B runs there (B's own sidebar has it) and streams here; while it works, its row here, under B's name in Devices (`machine-head-<host>`, its count `machine-count-<host>` on the same right-hand column as a project's), has the turn clock (`live-line-working-<id>`), the pill of its sub-agents (`live-line-kids-<id>`, `live-line-kids-count-<id>`, named in the tooltip) and, its turn over with them still out, `live-line-waiting-<id>`; the tooltip names its provider, model and each sub-agent by provider; what waits on the user there is in the Inbox ("<project> on <machine>"); Stop, Allow/Deny, plan approval and effort changes take effect on B once. Forgetting B removes its row, its threads here and its entry in `machines.json`, and the project's chip goes back to this Mac. Nothing under `machine:` ids is in this Mac's database. A file attached here reaches B byte for byte under B's `uploads/` (never elsewhere there: its name is made safe and nothing is written over), B's agent is given B's path, and the message here still shows this computer's file.
