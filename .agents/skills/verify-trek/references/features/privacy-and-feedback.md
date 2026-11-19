# Privacy and feedback

## What it does

Trek asks once, in a small card on the first launch after onboarding (and once for anyone upgrading from a version without it), whether it may share anonymous usage data: what's shared (usage counts, crash and error reports, the version, OS and architecture), what never is (prompts, code, file names, paths, transcripts, keys, account emails), Yes or No. With a yes it keeps a random install id and sends a few events to PostHog in the background: `install` once, `app_opened` at most once a day, `app_updated`, `feature_used` counts of agents and tools, and the panics of earlier runs with paths and secrets taken out. A yes also covers **usage totals** (from 0.4.58): `tokens_used` (each day's token counts and their API-price cost by agent and model, summed on this computer), `feature_used` counts of a few actions (`thread-started`, `sub-agent`, `handoff`, `study-learn`, `study-practice`, `note-created`, `turn-failed`, `turn-stopped`) and `setup` once a day (counts of projects, threads and agents, the theme, glass and the phone server on or off). Someone who said yes before the totals existed gets a second card once ("Share usage totals too?", `totals-card`, `totals-yes` / `totals-no`); whatever they answer, what they shared before goes on. Without a PostHog key in the build, with `DO_NOT_TRACK` set, in tests and in the isolated verify profile, nothing is sent. docs/PRIVACY.md lists every event and property.

**Settings → Privacy** changes the answer (`privacy-consent`) and the one about usage totals (`privacy-totals`, off and disabled while nothing is shared), shows the install id (Reset makes a new one), where it goes, what's waiting, and exactly what was last sent (Show). **Send feedback…** (Help menu, ⌘K, Settings → About and → Privacy) opens a form: what happened, Bug / Idea / Other, an optional email, Include diagnostics (version, OS, architecture, recent errors and the last 200 log lines, scrubbed), a line saying what it sends and Show all for the whole report, then Send (PostHog's `feedback` event, no yes to telemetry needed), Copy report, or Report a bug on GitHub (a pre-filled issue, never the email).

## Reach it as a user

The card comes up by itself once. Settings → Privacy, or Help → Send Feedback…

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open settings:privacy
./.agents/skills/verify-trek/scripts/trek-dev screenshot privacy
# The verify profile never puts the card up by itself, and sends nothing:
./.agents/skills/verify-trek/scripts/trek-dev control privacy-demo card
./.agents/skills/verify-trek/scripts/trek-dev control click consent-yes      # or consent-no
./.agents/skills/verify-trek/scripts/trek-dev control privacy-demo totals    # the card for an earlier yes
./.agents/skills/verify-trek/scripts/trek-dev control click totals-no        # or totals-yes
# The feedback form as a build with a key draws it, without one, after a failed send, and sent:
./.agents/skills/verify-trek/scripts/trek-dev control privacy-demo feedback   # or nokey, failed, sent
./.agents/skills/verify-trek/scripts/trek-dev control click feedback-show
```

## Code

`crates/trek-telemetry/` (consent and state, events, queue and retries, scrubbing, feedback, the background loop); `crates/trek-app/src/privacy.rs` (start, the card, counted names), `feedback.rs` (the form), `settings_view/privacy.rs` (the page); panics' reports in `crates/trek-logging/src/lib.rs`. The key comes from `TREK_POSTHOG_KEY`/`TREK_POSTHOG_HOST` at build time (release.yml's repository variables) or `telemetry/posthog.toml` in the data folder. Tests: `cargo test -p trek-telemetry`, `crates/trek-app/tests/privacy.rs`. Download counts: `./bin/agent-cli downloads`.

## Verify

A yes given before the totals existed doesn't cover them (no `tokens_used`, `setup` or action counts until the second card's yes), and a no to them drops what was counted and queued for them alone. Token totals carry only numbers, Trek's own agent names and a priced model's id (anything else is `other`). Nothing goes before a yes, without a key, with `DO_NOT_TRACK`, or from a test profile; a no drops what was waiting. A crash report and feedback logs carry no path under the home folder, no project or file name, no email and no secret. The card shows once and only after onboarding. Feedback says where it goes before it goes, and copies the report when it can't.
