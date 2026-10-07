# Phone companion

## What it does

Pairs Trek with its iPhone companion so a user can view/start/steer threads and answer approval, question, or plan requests while agents continue on the Mac. Push preferences control when the phone is alerted.

## Reach it as a user

Open **Settings → Phone**, enable the phone connection, and scan the pairing QR code in the iOS app. Choose push behavior on the same page.

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

