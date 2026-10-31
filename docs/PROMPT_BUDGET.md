# Prompt budget: what Trek adds to every request

Trek adds text to every request a vendor agent makes: its notes (what Trek can render, the recap
rule, the project's verification skill) and its MCP servers (server instructions and tool
schemas). This doc measures that overhead per agent, records live usage from Claude Code and
Codex, and names the tests that hold it to a budget.

Measured 2026-10-31 on Trek 0.4.28 with Claude Code 2.1.316 (Opus 5.5) and Codex CLI 0.166.0
(gpt-6-astra).

## What Trek sends

| Piece | Where it comes from | Claude Code | Codex | OpenCode, ACP agents (Cursor, Copilot, Droid) | Direct models |
| --- | --- | --- | --- | --- | --- |
| Notes | `Workspace::session_notes` → `agent_notes` (crates/trek-app/src/workspace.rs) | `--append-system-prompt`, every launch | wrapped in `<trek-project-notes>` around the first message | same as Codex | system prompt |
| MCP servers | `Workspace::mcp_servers` + `session_mcp` (trek-app), served by `trek-mcp <family>` | `--mcp-config` | `mcp_servers` in `thread/start`'s config | ACP `mcpServers` | none (Direct models get no Trek tools) |
| Trek's skills folder | `trek_core::skills::shipped_root()` | `--add-dir` | not passed | not passed | not passed |
| Tool-search line | `trek_agents::tool_search_note`, added by `message_notes` | not added | after the notes, naming the Trek servers handed over | not added | not added |

The notes, in order (each goes in only when its setting is on, every one is on by default):

1. `trek_core::visualization::AGENT_INSTRUCTIONS`: the `trek-viz` spec and TeX math. Always.
2. `trek_core::artifact::AGENT_INSTRUCTIONS`: `trek-html` artifacts (General › Interactive artifacts).
3. `trek_core::media::AGENT_INSTRUCTIONS`: the video watcher (Tools › Video, and only when trek-mcp is in the build).
4. `trek_core::verification::instructions`: the project's verification skill, when it has one.
5. `trek_core::changes::RECAP`: the Done / Still to do / Found recap (General › Ask for a recap), not for advising sub-agents or side chats.
6. Codex only: one line naming the Trek servers the session was handed, since Codex shows the model
   neither their tool names nor their instructions (see "Codex, live"): "Trek's tools (the browser,
   the user's iPhone, the iOS Simulator, watching video, sub-agents and threads) are MCP servers you
   reach with a tool search: look there before saying you can't."

MCP servers by default: trek-simulator, trek-video, trek-browser, trek-phone, trek-orchestrate (57
tools). Everything on adds trek-computer (Tools › Computer use, background mode by default): 67
tools. Figma's desktop server (off by default) and the user's own MCP servers are theirs and aren't
counted here. Trek writes no CLAUDE.md, AGENTS.md or skill listing for the agent: the skills folder
it passes with `--add-dir` isn't a `.claude/skills` folder, so Claude Code only notes the folder.

## Method

- Notes: `TREK_PROMPT_DUMP=<dir> cargo test -p trek-app --lib prompt_budget` writes the notes exactly
  as `agent_notes` assembles them (default settings; with a verification skill; with every optional
  note off; and wrapped for message-carried agents by `trek_agents::instructions_prompt`).
- MCP: each `trek-mcp <family>` binary was started and sent `initialize` and `tools/list` over stdio,
  as any MCP client does; the server instructions and tool list are what it answered.
- Tokens: tiktoken 0.14.0, `o200k_base` (Codex's tokenizer; Anthropic's differs, see the live numbers).
  Tool schemas counted as compact JSON, either as MCP returns them or as an OpenAI function
  (`{"type":"function","name":"mcp__trek-x__tool","description":…,"parameters":…}`), the shape
  OpenCode and other non-deferring agents send. chars/4 is a rough cross-check: 3,512 chars of
  default notes is 878 by chars/4 against 952 tokens.
- Live: `claude -p "say hi" --output-format json --strict-mcp-config` with and without
  `--append-system-prompt-file <notes>`, `--mcp-config <trek-mcp servers>` and `--add-dir <skills>`;
  `codex exec --json --ignore-user-config -m gpt-6-astra "say hi"` with `-c mcp_servers.trek-*`
  overrides and the wrapped notes before the prompt. `--strict-mcp-config` and
  `--ignore-user-config` keep the user's own servers and plugins out: without them Claude Code's
  baseline flipped between 38,126 and 44,713 tokens on identical runs (the user's claude.ai
  connectors connecting in time or not). Each configuration ran two or three times; the numbers
  below repeated to within a few tokens unless noted.

## Notes (o200k tokens)

| Piece | Before | After | Chars before → after |
| --- | ---: | ---: | --- |
| Visualization (`trek-viz` spec, math) | 591 | 481 | 1,997 → 1,595 |
| Artifacts (`trek-html`) | 285 | 275 | 1,155 → 1,095 |
| Video | 89 | 80 | 364 → 321 |
| Verification skill (this repo's) | 108 | 108 | 442 |
| Recap | 137 | 116 | 599 → 495 |
| **Default** (no verification skill) | **1,102** | **952** | 4,121 → 3,512 |
| With a verification skill | 1,210 | 1,060 | 4,565 → 3,956 |
| Every optional note off | 591 | 481 | |
| `<trek-project-notes>` wrapper (Codex, OpenCode, ACP) | +14 | +14 | |
| Codex's tool-search line, default servers / with computer use | | +44 / +47 | about 200 |
| **Codex's first-message notes, default** | 1,116 | **1,010** | 3,745 |

## MCP servers (o200k tokens)

"Names" is the deferred-tool list Claude Code shows instead of schemas (`mcp__trek-x__tool`, one
per line). "Schemas" are the tools as MCP JSON, "function wire" as an OpenAI function.

| Server | Tools | Instructions before → after | Names | Schemas before → after | Function wire before → after |
| --- | ---: | --- | ---: | --- | --- |
| trek-computer (in front) | 10 | 128 → 128 | 104 | 1,025 → 1,025 | 1,125 → 1,125 |
| trek-computer (background, the default mode) | 10 | 218 → 196 | 104 | 1,179 → 1,179 | 1,279 → 1,279 |
| trek-simulator | 12 | 141 → 141 | 135 | 1,169 → 1,142 | 1,289 → 1,262 |
| trek-video | 1 | 134 → 54 | 9 | 273 → 273 | 282 → 282 |
| trek-orchestrate | 11 | 163 → 163 | 132 | 2,137 → 2,049 | 2,258 → 2,170 |
| trek-browser | 18 | 190 → 183 | 180 | 1,517 → 1,517 | 1,679 → 1,679 |
| trek-phone | 15 | 337 → 252 | 156 | 1,458 → 1,458 | 1,593 → 1,593 |
| **Default** (no computer use) | 57 | **965 → 793** | 612 | 6,554 → 6,439 | **7,101 → 6,986** |
| **Everything on** (+ computer, background) | 67 | **1,183 → 989** | 716 | 7,733 → 7,618 | **8,380 → 8,265** |

## Per agent, per request

Estimates in o200k tokens, then what the vendor reported live.

| Agent | What Trek's additions reach the model as | Default before → after | Everything on before → after | Live, default before → after | Live, everything on before → after |
| --- | --- | --- | --- | --- | --- |
| Claude Code | notes + server instructions + deferred tool names | 2,679 → 2,357 | 3,001 → 2,657 | **+4,081 → +3,595** | **+4,545 → +4,030** |
| Codex | notes and the tool-search line in the first message | 1,116 → 1,010 | 1,116 → 1,013 | **+1,116 → +1,010** | **+1,116 → +1,010** (line listed the default servers) |
| OpenCode, ACP agents | notes in the first message + every tool schema (+ instructions, if the agent passes them on) | 8,217 → 7,952 (9,182 → 8,745) | 9,496 → 9,231 (10,679 → 10,220) | not measured | not measured |
| Direct models | notes in the system prompt (no Trek tools) | 1,102 → 952, on top of the harness's own 451 | same | not measured | not measured |

### Claude Code, live

`usage` input tokens (`input_tokens + cache_creation_input_tokens + cache_read_input_tokens`), with
`--strict-mcp-config`:

| Run | Before | Δ | After | Δ |
| --- | ---: | ---: | ---: | ---: |
| Bare `claude -p "say hi"` | 37,913 | | 37,914 | |
| + notes | 39,508 | +1,595 | 39,272 | +1,358 |
| + `--add-dir` skills folder | 37,954 | +41 | 37,956 | +42 |
| + default MCP servers | 40,361 | +2,448 | 40,109 | +2,195 |
| Trek's default (all three) | 41,994 | **+4,081** | 41,509 | **+3,595** |
| Everything on (+ computer use) | 42,458 | **+4,545** | 41,944 | **+4,030** |

Claude Code defers MCP tool schemas: the MCP delta (2,195) matches the instructions plus the tool
names (793 + 612 o200k, times Anthropic's ratio), not the 6,439 tokens of schemas. Anthropic's
tokenizer counts Trek's text at about 1.4 to 1.5 times o200k.

How much is cache-read in practice: the notes sit in the stable part of the system prompt, so even
the first request of a new process reads them from cache (`cache_read_input_tokens` rose from
25,847 to 27,438 with the notes, exactly their size). The MCP instructions and tool names sit in a
later block that each new process writes once (`cache_creation` 14.3k against 12.1k bare). A
session run as one process, which is how Trek runs Claude Code, then reads everything: in a
three-turn session the third request read 41,521 tokens and wrote 15. So after a session's first
request, all of Trek's ~3.6k is billed as cache reads.

### Codex, live

| Run | Before | After the trims | With the tool-search line |
| --- | ---: | ---: | ---: |
| Bare `codex exec "say hi"` | 18,081 | 18,081 | 18,081 |
| + default MCP servers | 18,081 (+0) | not rerun | not rerun |
| + notes | 19,197 (+1,116) | 19,047 (+966) | |
| Trek's default | 19,197 (+1,116) | 19,047 (+966) | 19,091 (+1,010) |
| Everything on | 19,197 (+1,116) | 19,047 (+966) | 19,091 (+1,010) |

The notes cost exactly their o200k count. MCP servers cost nothing up front: Codex 0.166 defers
every MCP tool behind its tool search and sends neither the schemas, the tool names nor the server
instructions (asked to list its tools, the model answered that further deferred tools, MCP tools
among them, exist but their names aren't shown). Caching is the provider's own and best-effort:
identical one-shot runs read anywhere from 0 to 19,072 of 19,197 tokens from cache. Within a
session the notes are part of the first message, so each later request carries them in its prefix.
Three of the twelve "after" runs reported 18,501 instead of 19,047 for the same input (also without
any MCP server); the cause is Codex's, unexplained.

Does Codex find Trek's tools? Asked with Trek's default servers and notes, without and then with the
tool-search line (one run each, `codex exec --json --ignore-user-config -m gpt-6-astra`):

| Request | Without the line | With it |
| --- | --- | --- |
| "Open https://example.com in Trek's browser and tell me the page title." | calls `trek-browser` `browser_open` | calls `trek-browser` `browser_open` |
| "Which of Trek's tools can you use here? Give their exact names." | searches, lists all 57 | searches, lists all 57 |
| "What's on my iPhone's screen right now?" | calls `trek-phone` `phone_screenshot` | calls `trek-phone` `phone_screenshot` |
| "Boot an iOS simulator and take a screenshot of it." | tries `xcrun simctl` in the shell first, then `trek-simulator` `sim_list` | goes straight to `trek-simulator` `sim_list` |

(`codex exec` refuses MCP calls that need approval under its "never" policy, so each call stopped
there; inside Trek, approvals come to Trek.) So gpt-6-astra searches its tools on its own even
without the line. With the line it went straight to Trek's tool instead of the shell. The line
costs 44 tokens and is for models that search less readily.

## What was trimmed

Nothing was removed that an agent needs to do something; the parsers and tools behave as before.

- **Visualization:** the "Common mistakes (coerced only if unambiguous)" list is gone. It told the
  agent which slips Trek's parser repairs, which doesn't help it write a correct block; the parser
  still repairs them. The one hint that prevented a real slip stays: "A unit goes in `unit`, never in
  `ticks`." −110 tokens.
- **Artifacts:** the sizing and loading sentences say the same in fewer words; "it can't navigate
  away" was implied by "links open in the user's browser". −10.
- **Video:** the note and trek-video's server instructions both said "you can't watch video
  yourself" and what `watch_video` returns, which its tool description also says. The note keeps
  the why and the inline-playback rule (Codex sees only the note); the server instructions keep only
  what the tool description lacks (`start`/`end`, `fps`, the player). −9 and −80.
- **Recap:** the same three parts and rules, shorter. −21.
- **trek-phone instructions:** the Bluetooth / iPhone Mirroring mechanics, how `phone_open_app`
  works and which tools ask first are in the tool descriptions and phone_status; the instructions now
  say each once. −85.
- **trek-computer (background), trek-browser:** tighter wording, same rules. −22, −7.
- **trek-orchestrate tool descriptions:** delegate_task, start_thread, handoff, task_result and
  list_models shortened; "the user sees every sub-agent inline" and "the user is told when it
  finishes too" (things the agent can't act on) dropped. −88 schema tokens.
- **trek-simulator:** the `udid` description repeated on nine tools is shorter. −27 schema tokens.

## The budget tests

- `crates/trek-app/src/workspace/prompt_budget.rs` builds the notes with `agent_notes`, the function
  `session_notes` uses, for default settings and with a verification skill, and asserts their size
  (3,850 and 4,350 chars, about 10% over today's), that the wrapper stays small, and that each piece
  goes in once. It also builds Codex's first-message notes with `trek_agents::message_notes`: the
  tool-search line is held to 220 chars and Codex's notes with everything on to 4,650.
- `crates/trek-mcp/src/main.rs`, `prompt_budget`: each family's server instructions and tool schemas,
  as `initialize` and `tools/list` return them, against a per-family budget, and all of them
  together (40,500 chars).
- `trek_core::visualization`'s and `trek_core::artifact`'s own compactness tests now stop at 1,750
  and 1,150 chars.

To raise a budget on purpose: re-measure as above, update the tables here, and set the constant to
the new size plus about 10%. Every character goes out with every request of every thread, so look
for words to cut first, and say a thing once: in the tool description or the server instructions,
not both.
