# Onboarding

## What it does

A short intro (the cairn stacks itself while the hiker walks up to it), then one step at a time: Appearance (theme tiles, glass and frost, reduce motion, text size), Permissions (macOS only: Accessibility, Screen & System Audio Recording, Microphone, Speech Recognition, Notifications, Full Disk Access, Local Network — each with its status, the system prompt or the exact System Settings pane, and Trek's icon to drag into list panes), Agents (scan of the agent CLIs, signed in or not, subscription or API key, with Install and Sign in run in the terminal dock), API keys, Local models (Ollama, LM Studio, llama.cpp/MLX), Approvals, Phone & machines, and Finish (summary with Change links, first project, Start a thread).

It shows on a first run and once more when `ONBOARDING_VERSION` (`crates/trek-core/src/settings.rs`) is bumped; quitting part way resumes on the same step (`[onboarding] step`). Skip setup finishes it from any step.

## Reach it as a user

First launch; **Settings → General → Show onboarding again**; ⌘K **Show onboarding**.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open onboarding
./.agents/skills/verify-trek/scripts/trek-dev control onboarding permissions   # any step key
./.agents/skills/verify-trek/scripts/trek-dev control onboarding demo          # agents signed in, Ollama with models, some permissions on
./.agents/skills/verify-trek/scripts/trek-dev control wait 2600                # the intro's length
./.agents/skills/verify-trek/scripts/trek-dev screenshot onboarding-agents
```

Step keys: `welcome`, `appearance`, `permissions`, `agents`, `providers`, `local-models`, `approvals`, `phone`, `finish`. The isolated app finds no agents and has no permissions of its own, so use `demo` before screenshots that should show a set-up Mac. Don't press the permission buttons from the driver: they open the real System Settings.

## Code

`crates/trek-app/src/onboarding.rs` (flow, scan mapping), `onboarding/steps.rs` (views), `onboarding/helper.rs` (the floating drag window), `crates/trek-app/src/permissions.rs`; tests in `crates/trek-app/tests/screens.rs` and `upkeep.rs`.

## Verify

Every step renders in Night and Paper without clipping; the intro is still with reduce motion; Continue never moves; a permission row flips to Allowed within a second of being granted; Skip setup and Start a thread leave `[onboarding] version` at the current one.
