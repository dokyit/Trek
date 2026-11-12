# Privacy: usage data, crash reports and feedback

Trek asks once, on the first launch after onboarding (and once for anyone who used an earlier version), whether it may share anonymous usage data. The answer can be changed any time in **Settings › Privacy**, which also shows exactly what was sent last.

- **Shared, with a yes:** anonymous usage counts (how often Trek opens, which agents and tools are used), crash and error reports, the app version, OS and architecture.
- **Never shared:** prompts, code, file names, paths, transcripts, keys or account emails.
- **With a no:** nothing is sent, ever, except feedback the user sends themselves.

Everything goes to [PostHog](https://posthog.com) (US cloud, `us.i.posthog.com`), through its capture API, from `crates/trek-telemetry`.

Dictation is apart from all of this. On Windows the composer's mic turns speech into text on the PC itself, with Whisper (whisper.cpp running OpenAI's multilingual base model): what the mic hears never leaves the PC, and Trek sends it nowhere. The first time the mic is clicked, the speech model (141 MB) downloads from Hugging Face (`huggingface.co`, a fixed revision of `ggerganov/whisper.cpp`, checked against its SHA-256 before it's used). That download is the only request dictation makes, and nothing downloads before that click. The model is kept in `%LOCALAPPDATA%\trek\Trek\data\dictation` (the local data folder, which a roaming profile doesn't copy between PCs), and **Settings › General › Dictation** removes it. Agents' `watch_video` uses the same model to transcribe a video's audio on the PC when whisper isn't installed; it never downloads the model itself, and the audio stays on the PC too. A Trek installed as a Windows package would use Windows' online speech recognition instead, and then what the mic hears goes to Microsoft's speech service, as it does for Windows' own voice typing.

## When nothing is sent

- The user said no, or hasn't answered yet.
- `DO_NOT_TRACK` is set (to anything but empty, `0` or `false`). Trek doesn't ask, and the Privacy page says why.
- The build has no PostHog key. The card still asks, so the answer is ready once a build has one.
- A test, or the isolated verify profile (`TREK_SHOT_DIR`): a process whose data folder is isolated sends nothing at all, feedback included, and never puts the card up by itself.

## The key

The key is a PostHog **project** API key (`phc_…`). It can only add events, never read them, so it's safe in a build anyone downloads. It's compiled in from `TREK_POSTHOG_KEY` (and `TREK_POSTHOG_HOST`, default `https://us.i.posthog.com`) at build time. The release workflow passes the repository variables of the same names (Settings › Secrets and variables › Actions › Variables); unset, the build has no key.

To try a build out, put a key in the data folder instead: `telemetry/posthog.toml` with `key = "phc_…"` and optionally `host = "https://…"`. It wins over the build's. A personal key (`phx_…`) is refused.

## What's kept on this computer

In `telemetry/` in Trek's data folder:

| File | What |
| --- | --- |
| `state.json` | The answer, the install id, the day `app_opened` last went, the version that last ran, and today's counts. |
| `queue.json` | Events waiting to be sent (at most 500; each is dropped after 5 failed tries or a week). |
| `last-sent.json` | The last request, exactly as it was sent, for Settings › Privacy. |
| `posthog.toml` | Only if you put one there (see above). |

A panic also leaves `crash-reports/panic-*.json` in the data folder (the newest five). Until the user answers they wait; with a yes the next launch sends them, with a no it deletes them.

## The install id

A random UUID, made at the first yes. It's the event's `distinct_id`; nothing else about the install or the person is known. Every event sets `$process_person_profile: false` (PostHog makes no person profile) and `$geoip_disable: true` (no country or city from the address it came from). For no IP at all, turn on *Discard client IP data* in the PostHog project's settings. **Reset** in Settings › Privacy makes a new id and drops what was waiting under the old one.

## Events

Every usage event carries these properties:

| Property | Example |
| --- | --- |
| `$app_version` | `0.4.41` |
| `channel` | `stable`, `beta`, `nightly` |
| `$os` | `macOS`, `Windows` |
| `$os_version` | `26.1`, `11 (26100)` |
| `arch` | `aarch64`, `x86_64` |
| `$lib` | `trek` |
| `$process_person_profile` | `false` |
| `$geoip_disable` | `true` |

| Event | When | Its own properties |
| --- | --- | --- |
| `install` | The first launch with a yes (once per data folder). | `set_up_before`: Trek was already set up there (an existing user counted for the first time, not a new install). |
| `app_opened` | A launch, at most once a day (and once more if Trek is still open the next day). | none |
| `app_updated` | The first launch of a new version. | `from_version`, `to_version` |
| `feature_used` | The day after: one per thing used that day. | `kind` (`agent` or `tool`), `name`, `count`, `day` (`YYYY-MM-DD`) |
| `$exception` | The launch after a crash (a Rust panic). | `$exception_list` (`type: panic`, the message), `$exception_level: fatal`, `location` (Trek's source file and line), `thread`, `backtrace` (at most 16,000 characters), `crashed_version` |
| `feedback` | The user sends feedback (no yes needed: sending it is the consent). | See below. |

`feature_used` names are Trek's own words, never anything typed:

- agents: `claude-code`, `codex`, `opencode`, `droid`, `acp:<registry id>` (`acp:cursor`), `direct:<provider>` (`direct:anthropic`, `direct:ollama`). An agent the user added as their own command is `acp:other`; a custom endpoint is `direct:other`. One count per message sent to it.
- tools (the tools panel): `git`, `explorer`, `terminal`, `browser`, `side-chat`, `simulator`, `iphone`. One count per opening.

## Scrubbing

A crash report, and the log lines in feedback, go through `trek_telemetry::Scrubber` first:

- Secrets agents were given this run (`trek_core::session_secrets`) become `[NAME hidden]`; a panic's report is scrubbed of them when it's written, too.
- Anything shaped like a key or token (Anthropic, OpenAI, GitHub, Slack, AWS, Google, GitLab, npm, JWTs, `API_KEY=…`, `https://user:pass@…`, private keys) becomes `[redacted]` (the same matcher as Share's "Hide file paths and secrets").
- Email addresses become `[email]`.
- A project's folder, and whatever path follows it, becomes `[path]`. So does any other absolute path, except the system's (`/usr/…`, `/opt/homebrew/…`, `/Applications/…`, `C:\Windows\…`); one under the home folder is `~/[path]`.
- Trek's own data and log folders become `[Trek data]` and `[Trek logs]`; a crate in cargo's registry is named from the crate (`gpui-pre-0.3.7/src/window.rs`).
- The home folder anywhere else is `~`.

## Feedback

Help › Send Feedback…, the command palette's "Send feedback…", and Settings › About and › Privacy open the form: what happened, the kind (bug, idea, other), an optional email for a reply, and **Include diagnostics** (on by default). The form says in a line what it will send and shows all of it on request before anything goes.

The `feedback` event:

| Property | |
| --- | --- |
| `kind` | `bug`, `idea`, `other` |
| `message` | What the user wrote. |
| `email` | Only if they gave one. |
| `diagnostics` | Whether diagnostics are included. |
| `recent_errors` | With diagnostics: up to 20 warning and error lines from the last two days' logs, scrubbed. |
| `log_tail` | With diagnostics: the last 200 lines of Trek's log, scrubbed. |

With diagnostics it also carries the version, OS and architecture properties above; without, none of them. With a yes to telemetry the `distinct_id` is the install id; otherwise it's a new random id each time, so feedback can't be tied to anything.

Without a key (or in the verify profile) the form says so, and its button copies the report instead. **Report a bug on GitHub** opens a new issue on `dokyit/Trek` with the message and diagnostics filled in (never the email: an issue is public), the log cut from its start to fit a link.

### On the iPhone

The iPhone app has the same form in Settings › Send feedback…. It sends the `feedback` event straight to PostHog (`$lib: trek-ios`, a new random id each time) with the project key it was built with: the `TREK_POSTHOG_KEY` and `TREK_POSTHOG_HOST` build settings, which the build's environment sets, become `TrekPostHogKey` and `TrekPostHogHost` in its Info.plist. Its diagnostics are the app's version, the iOS version and the architecture; it has no log to send. Without a key, and in the demo (`-TrekDemo YES`), nothing is sent and the button copies the report. The iPhone app has no usage data or crash reports.

## Download counts

`./bin/agent-cli downloads` adds up the release assets' download counts on GitHub, per version and platform. With a PostHog personal API key in `~/.trek-bench/posthog-personal.key` it also asks PostHog for unique and active installs (installs seen, and those that opened Trek in the last 7 and 30 days).
