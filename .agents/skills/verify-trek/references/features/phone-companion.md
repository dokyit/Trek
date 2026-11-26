# Phone companion

## What it does

Pairs Trek with its iPhone companion so a user can view/start/steer threads and answer approval, question, or plan requests while agents continue on the Mac. Push preferences control when the phone is alerted.

## Reach it as a user

Open **Settings → Phone**, enable the phone connection, and scan the pairing QR code in the iOS app. Choose push behavior on the same page.

An iPhone can pair with several machines at once: all stay connected, their threads merge into one list (qualified `"<hostId>:<id>"` on the phone), and each is unpaired independently in Settings › Machines.

Attachments: the composer's + (and the new-thread sheet's paperclip) offers **Photos & Videos** and **Files** when the Mac lists the `uploads` feature (an older Mac: the photo picker alone, as before). Photos go as today (base64 JPEG `images`). A video or any file is copied into the app's temporary folder, shown in the strip (a video as its frame with ▶ and its length, a file as icon, name and size; each removable), and on send goes up first in pieces of at most 4 MiB (`upload_part`, each answered with `upload_progress`, a ring per tile meanwhile), then the message names them by upload id (`send.files`, `new_thread.files`). The Mac writes each to `<data dir>/uploads/<id>.part`, renames it `iPhone <date> at <time> <name>` once whole, and adds an `@path` mention of it after the message's words, as its own composer mentions a file: the agent reads it there, and a video plays under the message on the Mac and the phone. The phone's bubble shows such trailing mentions as chips by name. Limits: 2 GiB a file, 4 files part-way up per connection (`rate_limited`), 16 on the Mac; a gap, an overrun, a bad id or an unusable name is `bad_request` and ends that upload; an unknown or unfinished id in `files` is `bad_request` and nothing is sent; `.part` files a day old go when the phone server starts. A failed upload or send leaves the draft and its attachments in place.

What the phone does beside the thread itself: a Live Activity (Lock Screen and Dynamic Island) with the thread at work and what waits, and Allow and Deny for an approval that doesn't look destructive (`ask` on the row; no push, so past its stale date it says when it last heard); the Mac's "Interrupted" bar over the composer with Resume and Dismiss (`cut_short` on the row, feature `resume_interrupted`); and comments on a diff's lines, sent to the agent as one follow-up.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open settings:phone
./.agents/skills/verify-trek/scripts/trek-dev screenshot phone-settings
```

Pairing and push are off by design under `TREK_SHOT_DIR`: the isolated process does not start the remote service by itself, has notifications off, and has no real thread or account data. Two commands start its phone server when a check asks: `control pair-phone` (a stand-in iPhone on the loopback, as `parity-live` uses) and, for a real iPhone:

```sh
./.agents/skills/verify-trek/scripts/trek-dev control pair-device 7437   # any free port but the owner's 7420
cat "$(./.agents/skills/verify-trek/scripts/trek-dev status | jq -r .runtime_dir)/shot/pair-device.txt"   # address, code, fingerprint, link
```

The server listens on the local network at that port under the name "Trek test (isolated)", with an id of its own, so the iPhone lists it as one more machine beside the owner's Trek and neither pairing touches the other. On the phone: Settings › Add a machine… › Enter address and code (the address, the code and the short fingerprint from the file). Its threads are the run's mock ones, so sending, approvals, background commands and sub-agents can be tried end to end from a real phone without touching a real thread. Unpair it on the phone afterwards (Settings › that machine › Unpair).

## Code

`crates/trek-app/src/settings_view/mobile.rs`, `remote.rs`, `remote/`; protocol/client notes in `docs/MOBILE.md` and iOS sources in `ios/`.

## Verify

Pairing requires user action, secrets are not logged, connection state updates on both devices, thread/request state converges, and remote answers affect the intended request exactly once.

