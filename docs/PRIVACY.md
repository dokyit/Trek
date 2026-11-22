# Privacy: usage data, crash reports and feedback

Trek asks once, on the first launch after onboarding (and once for anyone who used an earlier version), whether it may share anonymous usage data. The answer can be changed any time in **Settings › Privacy**, which also shows exactly what was sent last.

- **Shared, with a yes:** anonymous usage counts (how often Trek opens, which agents and tools are used), crash and error reports, the app version, OS and architecture.
- **Usage totals, with a yes of their own (from 0.4.58):** each day's token counts and their cost at API prices, by agent and model; which of Trek's features are used; how many projects, threads and agents there are, and the theme. The first card's yes covers them, since it lists them. Someone who said yes before 0.4.58 is asked about them once, in a card of its own ("Share usage totals too?"), and shares what they shared before whatever they answer. **Settings › Privacy › Share usage totals** changes it. Nothing counted before the yes is sent.
- **Never shared:** prompts, code, file names, paths, transcripts, keys or account emails.
- **With a no:** nothing is sent, ever, except feedback the user sends themselves.

Everything goes to [PostHog](https://posthog.com) (US cloud, `us.i.posthog.com`), through its capture API, from `crates/trek-telemetry`. There is no PostHog SDK in the app, the iPhone app or the site, so there is no autocapture, session replay, page view or heatmap: only the events listed below are sent. Every request Trek itself makes names the app and its version in its User-Agent (`Trek/<version>`).

Dictation is apart from all of this. On a Mac the mic uses Apple's speech recognition (`SFSpeechRecognizer`): on the Mac itself where the system can, and on Apple's servers where it can't; `watch_video`'s audio on a Mac goes the same way. On Windows the composer's mic turns speech into text on the PC itself, with Whisper (whisper.cpp running OpenAI's multilingual base model): what the mic hears never leaves the PC, and Trek sends it nowhere. The first time the mic is clicked, the speech model (141 MB) downloads from Hugging Face (`huggingface.co`, a fixed revision of `ggerganov/whisper.cpp`, checked against its SHA-256 before it's used). That download is the only request dictation makes, and nothing downloads before that click. The model is kept in `%LOCALAPPDATA%\trek\Trek\data\dictation` (the local data folder, which a roaming profile doesn't copy between PCs), and **Settings › General › Dictation** removes it. Agents' `watch_video` uses the same model to transcribe a video's audio on the PC when whisper isn't installed; it never downloads the model itself, and the audio stays on the PC too. A Trek installed as a Windows package would use Windows' online speech recognition instead, and then what the mic hears goes to Microsoft's speech service, as it does for Windows' own voice typing.

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
| `state.json` | The answer, the answer about usage totals, the install id, the days `app_opened` and `setup` last went, the version that last ran, and today's counts and token totals. |
| `queue.json` | Events waiting to be sent (at most 500; each is dropped after 5 failed tries or a week). |
| `last-sent.json` | The last request, exactly as it was sent, for Settings › Privacy. |
| `posthog.toml` | Only if you put one there (see above). |

A panic also leaves `crash-reports/panic-*.json` in the data folder (the newest five). Until the user answers they wait; with a yes the next launch sends them, with a no it deletes them.

## The install id

A random UUID, made at the first yes. It's the event's `distinct_id`; nothing else about the install or the person is known. Every event sets `$process_person_profile: false` (PostHog makes no person profile) and `$geoip_disable: true` (no country or city from the address it came from). The PostHog project Trek's releases send to has *Discard client IP data* on (checked in its settings on 21 November 2026) and a one-year retention, so no address is stored; a build that sends to another project depends on that project's own settings. **Reset** in Settings › Privacy makes a new id and drops what was waiting under the old one.

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
| `feature_used` | The day after: one per thing used that day. | `kind` (`agent`, `tool`, or with usage totals `action`), `name`, `count`, `day` (`YYYY-MM-DD`) |
| `tokens_used` | With usage totals, the day after: one per agent and model used that day. | `agent`, `model`, `day`, `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `api_cost_usd`, `reports`, `unpriced_reports` |
| `setup` | With usage totals, at most once a day. | `projects`, `threads`, `agents` (counts), `theme` (`system`, `night`, `paper`), `glass`, `phone` (whether each is on) |
| `$exception` | The launch after a crash (a Rust panic). | `$exception_list` (`type: panic`, the message), `$exception_level: fatal`, `location` (Trek's source file and line), `thread`, `backtrace` (at most 16,000 characters), `crashed_version` |
| `feedback` | The user sends feedback (no yes needed: sending it is the consent). | See below. |

`feature_used` names are Trek's own words, never anything typed:

- agents: `claude-code`, `codex`, `opencode`, `droid`, `acp:<registry id>` (`acp:cursor`), `direct:<provider>` (`direct:anthropic`, `direct:ollama`). An agent the user added as their own command is `acp:other`; a custom endpoint is `direct:other`. One count per message sent to it.
- tools (the tools panel): `git`, `explorer`, `terminal`, `browser`, `side-chat`, `simulator`, `iphone`, `plugin`. One count per opening.
- actions (usage totals only): `thread-started`, `sub-agent`, `handoff`, `study-learn`, `study-practice`, `note-created`, `turn-failed`, `turn-stopped`. One count each time.

`tokens_used` adds up what agents report at the end of each turn, on this computer, and sends the day's sums: never a thread, a project, a prompt or an answer. `agent` is named as in `feature_used`. `model` is the provider's own id for a model Trek has a price for (`claude-opus-5-5`), and `other` for anything else: a model the user named, a custom endpoint's, or one running on their computer. `api_cost_usd` is what those tokens would cost at the provider's API prices (the agent's own figure where it gives one, else Trek's price table on that day); it is not what anyone paid, since most people run agents on a subscription. `reports` is how many reports were added up, and `unpriced_reports` how many of them had no price, so their tokens are in the sums and their cost isn't. History imported from an agent's own files, and anything from before the yes, is never counted.

## Scrubbing

A crash report, and the log lines in feedback, go through `trek_telemetry::Scrubber` first:

- Secrets agents were given this run (`trek_core::session_secrets`) become `[NAME hidden]`; a panic's report is scrubbed of them when it's written, too.
- Anything shaped like a key or token (Anthropic, OpenAI, GitHub, Slack, AWS, Google, GitLab, npm, JWTs, `API_KEY=…`, `https://user:pass@…`, private keys) becomes `[redacted]` (the same matcher as Share's "Hide file paths and secrets").
- Email addresses become `[email]`.
- Web addresses (from 0.4.64): any `scheme://…` (the browser's pages, a `file://` path, one with a login in it) becomes `[url]`. The exception is an `http(s)` address whose host is one Trek or its model providers contact (`KNOWN_HOSTS` in `scrub.rs`: GitHub, the npm and Homebrew feeds, the agent registry, PostHog, ntfy, Hugging Face, the providers' API hosts, `localhost`): it keeps its scheme and host, never its path or query. The host alone says which of Trek's own requests failed, and where someone's browser went is theirs. Before 0.4.64 addresses were left in.
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
| `log_tail` | With diagnostics: the last 200 lines of Trek's log, scrubbed, each cut to 400 characters. |

With diagnostics it also carries the version, OS and architecture properties above; without, none of them. With a yes to telemetry the `distinct_id` is the install id; otherwise it's a new random id each time, so feedback can't be tied to anything.

Without a key (or in the verify profile) the form says so, and its button copies the report instead. **Report a bug on GitHub** opens a new issue on `dokyit/Trek` with the message and diagnostics filled in (never the email: an issue is public), the log cut from its start to fit a link.

### On the iPhone

The iPhone app has the same form in Settings › Send feedback…. It sends the `feedback` event straight to PostHog (`$lib: trek-ios`, a new random id each time) with the project key it was built with: the `TREK_POSTHOG_KEY` and `TREK_POSTHOG_HOST` build settings, which the build's environment sets, become `TrekPostHogKey` and `TrekPostHogHost` in its Info.plist. Its diagnostics are the app's version, the iOS version and the architecture; it has no log to send. Without a key, and in the demo (`-TrekDemo YES`), nothing is sent and the button copies the report. The iPhone app has no usage data or crash reports.

What's typed, dictated or attached on the iPhone goes to the paired computer and from there to the agent chosen on it, so to that agent's provider when it is a cloud one; Trek has no server in that path. The app asks before the first message, and again before the first to a provider it hasn't named (`ios/Trek/Model/Sharing.swift`; the yes is kept with the wording's version, and Settings › Privacy and feedback withdraws it). Its privacy manifest is `ios/Trek/PrivacyInfo.xcprivacy`; `ios/README.md` has the App Store notes.

## Other hosts the app talks to

Apart from PostHog, and apart from the agents' and model providers' own servers (which receive what the user sends them, as in a terminal), Trek makes these requests. The privacy page on the site lists them in plain words; keep the two in step.

| Host | When |
| --- | --- |
| `github.com/dokyit/Trek/releases` (the update feed, `stable.json` or the channel's) and `api.github.com` (What's new) | Update check, every hour, on by default (Settings › Updates › Check automatically); the update is fetched in the background (Download automatically, on by default). |
| `registry.npmjs.org`, `formulae.brew.sh`, `raw.githubusercontent.com`, `api.github.com`, `community.chocolatey.org`, `downloads.claude.ai`, `x.ai`, `static.devin.ai`, `cursor.com`, `app.factory.ai` | Version checks for the agents' CLIs, at launch and every 12 hours, on by default (Settings › Updates); the hosts asked follow from the agents found. |
| `cdn.agentclientprotocol.com` and the binary URLs it names | The registry's list when Add agent opens (cached); a download when an agent is installed from it. |
| `nodejs.org`, `ffmpeg.martin-riedl.de`, `github.com/GyanD`, GitHub releases for yt-dlp | When the user sets up Node.js, ffmpeg or yt-dlp. yt-dlp then reaches the video's own site when an agent watches a YouTube or X link. |
| `huggingface.co` | The Windows dictation model, on the first click of the mic. |
| `clients2.google.com`, `edge.microsoft.com` | A browser extension's download, when the user installs one. |
| `www.google.com` | The built-in Browser's start page asks for six sites' icons (`/s2/favicons`) when it opens; its address bar searches Google for text that isn't an address. |
| `cdn.jsdelivr.net` | Before 0.4.64, study question cards loaded KaTeX from it every time one was drawn; since then it is bundled (`crates/trek-assets/assets/katex`, served by the card's own origin at `/_trek/katex/`) and the card asks no host. A `trek-html` page an agent writes may still load from any CDN by design. |
| Any `https` host | A note's remote images (fetched by the Mac, and by the iPhone itself); a `trek-html` artifact's own scripts, styles and fonts, which its card names. |
| `ntfy.sh` or the user's own server | Phone notifications, off by default. A note says the kind of news and carries a `trek://` link; it names the thread and project only when the `push_names` setting is on (off by default). ntfy reads whatever is sent through it. |
| The browser's push service (`fcm.googleapis.com`, `web.push.apple.com`, `updates.push.services.mozilla.com`) | Web Push to the web remote, when the user turns its notifications on. Encrypted. |

The web remote (`site/remote`) loads nothing from another host. Its dictation uses the browser's own `SpeechRecognition`, which in Chrome and on Android is the browser maker's cloud service.

## Known gaps

- Error text an agent printed can still appear in `log_tail` and `recent_errors`, whatever it says (a thread title, a host name without a scheme). The form shows everything before it goes, and the privacy page says so.
- The one-year retention and the discarding of client IPs are settings of the PostHog project, not of the code (see "The install id"); the privacy page states them as they were found on 21 November 2026. Session replay and autocapture are off in the code (there is no SDK), but that project's own toggles for them were not looked at.

## Download counts

`./bin/agent-cli downloads` adds up the release assets' download counts on GitHub, per version and platform. With a PostHog personal API key in `~/.trek-bench/posthog-personal.key` it also asks PostHog for unique and active installs (installs seen, and those that opened Trek in the last 7 and 30 days).
