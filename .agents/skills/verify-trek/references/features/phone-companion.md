# Phone companion

## What it does

Pairs Trek with its iPhone companion so a user can view/start/steer threads and answer approval, question, or plan requests while agents continue on the Mac. Push preferences control when the phone is alerted.

## Reach it as a user

Open **Settings → Phone**, enable the phone connection, and scan the pairing QR code in the iOS app. Choose push behavior on the same page.

An iPhone can pair with several machines at once: all stay connected, their threads merge into one list (qualified `"<hostId>:<id>"` on the phone), and each is unpaired independently in Settings › Machines.

What the phone does beside the thread itself: a Live Activity (Lock Screen and Dynamic Island) with the thread at work and what waits, and Allow and Deny for an approval that doesn't look destructive (`ask` on the row; no push, so past its stale date it says when it last heard); the Mac's "Interrupted" bar over the composer with Resume and Dismiss (`cut_short` on the row, feature `resume_interrupted`); and comments on a diff's lines, sent to the agent as one follow-up.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open settings:phone
./.agents/skills/verify-trek/scripts/trek-dev screenshot phone-settings
```

Pairing and push are unavailable by design under `TREK_SHOT_DIR`: the isolated process does not start the remote service, has notifications off, and has no real thread/account data. Validate the settings UI here and use a dedicated test Mac/iPhone pair for protocol E2E work.

## Code

`crates/trek-app/src/settings_view/mobile.rs`, `remote.rs`, `remote/`; protocol/client notes in `docs/MOBILE.md` and iOS sources in `ios/`.

## Verify

Pairing requires user action, secrets are not logged, connection state updates on both devices, thread/request state converges, and remote answers affect the intended request exactly once.

