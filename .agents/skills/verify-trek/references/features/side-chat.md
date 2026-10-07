# Side Chat

## What it does

Opens an additional lightweight agent conversation in the right panel while the main thread remains visible. Multiple side chats can exist as panel tabs.

## Reach it as a user

Press ⌘J and choose **Side Chat**. Pick the desired agent/model in its composer and send a message. Use panel tabs to keep or close separate chats.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open tool:sidechat
./.agents/skills/verify-trek/scripts/trek-dev screenshot side-chat
```

The shots protocol does not type into the side-chat composer; CLI `send` always targets the main workspace composer.

## Code

`crates/trek-app/src/panels/side_chat.rs`, `panels/mod.rs`.

## Verify

A new tab has an independent transcript/session, streaming does not block the main thread, switching tabs preserves content, and closing a live tab shuts down its session cleanly.

