# Agents, models, and permissions

## What it does

Detects installed CLI/ACP agents, exposes subscription and usage information, and supports direct API and local-model providers. Each thread chooses model/effort and one of four hand-holding levels. Settings control default agent behavior and permission policy; API keys live in the macOS Keychain.

## Reach it as a user

Open **Settings → Agents & Subscriptions**, **API Keys**, **Local Models**, **General**, or **Permissions**. Thread-specific choices live below the composer.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open settings:agents-&-subscriptions
./.agents/skills/verify-trek/scripts/trek-dev screenshot agents
./.agents/skills/verify-trek/scripts/trek-dev open settings:permissions
```

The isolated app exposes only the Mock agent and never reads the Keychain. Test real-agent detection manually in a normal profile; do not put credentials into verification commands or files.

## Code

`crates/trek-app/src/settings_view/pages.rs`, `composer.rs`, `workspace.rs`; detection/catalog/settings in `crates/trek-core/src/`; sessions in `crates/trek-agents/src/`.

## Verify

Detection status and model lists are accurate, unsupported combinations are unavailable, changed defaults affect only new drafts, and permission escalation requires the designed user acknowledgement.

