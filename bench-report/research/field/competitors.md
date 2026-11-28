# Trek's field, 2026-11-27

All facts are from pages fetched on 2026-11-27. Star counts come from `gh api`. The full grid, with a source URL in every row, is `features.csv`. Raw notes are in `raw-group-a.md`, `raw-group-b.md` and `raw-group-c.md`. "Unverified" means no page I fetched confirmed it. "Not mentioned" means the README or site does not say, which does not prove the feature is missing.

## Recommended direct competitors (10)

| # | Product | Why it matters to Trek | Platforms | Price | Open source | Parallel and worktrees | Built-in browser | Mobile | Sources |
|---|---|---|---|---|---|---|---|---|---|
| 1 | **Claude desktop, Code tab** | Anthropic's own app. It has most of Trek's surface: parallel sessions, worktrees, a browser the agent drives, computer use, Remote Control and Dispatch, and sub-agents. It runs only Anthropic models. | macOS, Windows, Linux beta; cloud sessions on web and mobile | Needs a Pro, Max, Team or Enterprise plan or the API | No | Yes, worktrees opt-in | Yes, agent-driven | Yes | code.claude.com/docs/en/desktop.md |
| 2 | **OpenAI Codex app** | OpenAI's own app. It has parallel threads, worktrees, automations, an in-app browser, background computer use on macOS and a phone app. It runs only OpenAI models. | macOS, Windows (since 4 Mar 2026), Linux on a waitlist | Free tier with GPT-6 Luna only; Plus $20; Pro $100–500 | CLI only (Apache-2.0, 136.7k stars) | Yes | Yes. In April, wider agent control was still "planned". | Yes, via ChatGPT | developers.openai.com/codex/app ; openai.com/index/codex-for-almost-everything/ |
| 3 | **Cursor**, Agents window and cloud agents | The largest user base. It has an agent-driven browser, a coordinator with subagents, and an iOS app with voice. It is now owned by SpaceX, and its models are mostly Cursor's own and Grok. | macOS confirmed (other platforms not verified here); iOS | Free; paid $8–200 (pricing page looked garbled, so unverified) | No | Yes; worktrees unverified | Yes | Yes | cursor.com/changelog ; cursor.com/docs/agent/overview |
| 4 | **Orca** (Stably) | Closest feature match among the open-source apps. It runs several vendors' CLI agents with worktrees, has a browser agents drive, computer use, iOS and Android apps, orchestration, plugins, SSH and cloud VMs. 142k stars. | macOS, Windows, Linux, iOS, Android | Free | MIT | Yes | Yes, agent-driven | Yes | github.com/stablyai/orca ; orca.dev/docs/cli/computer-use |
| 5 | **T3 Code** (Ping, Theo) | Huge reach (82k stars). It runs any ACP agent, has a worktree per thread, phone apps, and is on every platform. No browser or computer use found. | macOS, Windows, Linux, web, iOS, Android | Free, bring your own subscriptions | MIT | Yes | Not mentioned | Yes | github.com/pingdotgg/t3code |
| 6 | **Paseo** | It sends the fewest tokens on ADEMark. It has native phone apps with full parity, agents that launch agents, 688 plugins, and voice. 26k stars. | macOS desktop, web, CLI and daemon, iOS, Android | Free | Apache-2.0 | Yes | The README says yes; agent control unverified | Yes | paseo.sh ; github.com/getpaseo/paseo |
| 7 | **Paradev** (formerly Superset) | Funded: an $11.5M seed reported by a secondary source. It runs several vendors with a worktree per workspace and has a CLI and MCP for orchestration. Its phone app is on the paid plan. 17k stars. | macOS, Linux (experimental), iOS, Android; no Windows yet | Free locally; Pro $20, Team $60, Ultra $200 | MIT | Yes | Preview pane | Paid plan | paradev.com/pricing ; github.com/ParadevAI/Paradev |
| 8 | **Conductor** | The category's best-known app. Meta bought it on 4 Nov. Its changelog then says "Conductor is back" (0.94.2, 13 Nov) and it kept shipping up to 0.101.0 on 25 Nov. It runs Claude Code, Codex and Cursor and has an iOS app. Its status is contested; see the notes below. | macOS, iOS | Unverified; sign-ups stopped on 4 Nov per the press | No | Yes | Preview | Yes | conductor.build/changelog ; thenextweb.com/news/meta-acquires-conductor-ai-coding-agents-superintelligence-labs |
| 9 | **Zed** | The same stack as Trek: Rust and GPUI. It runs any ACP agent, parallel threads and per-thread worktrees, is free with your own keys, and supports local models. It has no browser and no phone app. | macOS, Linux, Windows | Free; Pro $10 | Yes (GitHub says "Other") | Yes | No | No | zed.dev/docs/ai/parallel-agents ; zed.dev/pricing |
| 10 | **super.engineering** | A native Rust/GPUI peer, and ADEMark's leader: first in 4 of 12 tests, 134 MB at rest, and 5 of 5 on the orchestrated build. It runs 18 agents with a worktree per task. | macOS (Apple Silicon); Linux per the site | Free during the alpha (per a competitor's page) | No | Yes | Experimental, per a competitor's page | Not shipped | super.engineering ; ademark.dev |

### Next tier: watch, but not the head-to-head set
- **Herdr.** A Rust runtime for terminal agents (53k stars), plus a native app (3.6k stars) and phone apps. It is free under Apache-2.0, and helpers start with `hc run --bg`. It is terminal-first and has no browser. Sources: herdr.dev, github.com/herdrdev/herdr.
- **Nimbalyst** (formerly Crystal). An Electron app under MIT with an iOS companion, worktrees and a Chromium preview. 2.1k stars. Source: github.com/Nimbalyst/nimbalyst.
- **Devin Desktop** (formerly Windsurf, renamed 2 Jun 2026). It hosts "any ACP-compatible agent" beside Devin, with an Agent Command Center. It is free, then $20–200. Its browser, worktrees and mobile support are unverified. Source: cognition.com/blog/introducing-devin-desktop.
- **Google Antigravity.** It has an agent manager with parallel agents, browser use and subagents, and is free with a quota. It runs Gemini first and no third-party vendor agents. Source: antigravity.google.
- **Factory Desktop and Droid.** It has the strongest published harness numbers (see `benchmarks.md`), a browser it drives, and is free with your own inference. It runs only Droid. Source: factory.com/news/frontier-harness.
- **OpenCode desktop.** A single agent with 200+ providers, a built-in browser and phone apps. MIT, 230k stars, Electron. Source: opencode.ai.
- **Diri.** Rust and GPUI with computer use and an MCP that spawns agents. 502 stars, AGPL. Source: github.com/cristicretu/diri.
- **Synara.** Electron with an agent-driven browser (WebMCP). It was the fastest 5/5 build on ADEMark at 5.2 minutes. Source: github.com/Emanuele-web04/synara.
- **Traycer.** Free and claimed to be open source. It runs 19 agents, with a browser panel, mobile app, agent-to-agent messages, and 11k+ developers claimed. Source: traycer.ai.
- **cmux.** A native Swift terminal on libghostty (29k stars, GPL). Agents drive its browser through a socket API, and an iOS beta comes with Pro. It is terminal-first. Source: cmux.com.
- **Emdash.** Classic is Apache-2.0 with 6.3k stars; Emdash 2.0 is in early access, self-hosted or cloud. Neither has a phone app or browser. Source: emdash.com.
- **Zeron.** Native Rust on its own UI framework, MIT, 3.8k stars. Source: github.com/zeronsh/zeron.
- **Amp.** A Mac app in beta needing macOS 26+, an iOS app, and runners that create worktrees. It runs its own agent only. Source: ampcode.com/docs/pricing.

### Minor or out
- **Small or new:**
  - MonoCode: Tauri, MIT, 4.2k stars.
  - bb: Electron, MIT, 6.1k stars.
  - Clonk: Mac and iPhone app. Free, Pro $15 a month, lifetime $99. Not stated as open source. It claims "up to 80% less CPU, GPU and memory" than T3 Code, Codex and the Claude app, a marketing claim with no method (clonk.ai).
  - Superharness: MIT, 28 stars, repo created 25 Nov 2026. Desktop on Mac, Linux and Windows, iOS built from source, a "Master Orchestrator" thread (superharness.tech).
- **Cloud-first:** Capy (Free, then $20–200) and Cloudroom (Apache-2.0).
- **Different shape:**
  - Warp: terminal plus Factories at $3.50 per agent-hour.
  - Kiro: AWS; no third-party harnesses.
  - Goose: single agent, Electron.
  - Copilot Agent HQ: GitHub-centric.
  - Claude Squad: TUI.
  - Sculptor: research preview.
- **Legacy:** Vibe Kanban. Bloop shut down on 10 Apr 2026 and the project is now community-maintained.
- **Gone:** Jules (closes 31 Dec 2026) and Terragon (shut down 16 Jan 2026).

## How Trek compares on the feature grid (from features.csv)
- **Third-party vendor agents plus API and local models:** Trek, Zed and Devin Desktop do both. Partly so: Goose (ACP providers) and Traycer (OpenRouter and Hugging Face). The open-source managers (Orca, T3, Paseo, Paradev, Herdr) host CLI agents but do not run API or local models themselves. Claude desktop, Codex, Cursor, Factory, Amp and Antigravity run mainly their own vendor's agent and models.
- **Built-in browser the agent drives:** Claude desktop, Cursor, Orca, Synara, cmux, Antigravity, Kiro and Factory. Codex is partial. For Paseo it is claimed in the README and unverified.
- **Computer use:** Claude desktop, Codex (macOS), Orca and Diri. Factory is partial.
- **Phone companion:** almost everyone now has one: Claude, Codex, Cursor, Orca, T3, Paseo, Paradev (paid), Conductor, Herdr, Nimbalyst, Traycer, Clonk, cmux (beta) and Amp. Zed, Emdash, Diri, Synara, bb and MonoCode do not.
- **Native, not Electron:** Trek, Zed, super.engineering, Diri, Zeron, Herdr and cmux. Electron: Claude desktop, Orca, T3, Paseo, Paradev, Synara, Traycer, Emdash, bb, Capy, Cloudroom and Nimbalyst. Tauri: MonoCode.
- **No competitor I found has all of Trek's listed set:** iOS Simulator plus a real iPhone tool, video watching, notes and study mode, and connected machines. Traycer's docs mention iOS simulator and Android emulator panels (unverified). Orca, Herdr and super.engineering do remote and SSH machines.

## Notes and contradictions
- **Conductor.** ADEMark excluded it, saying "since 4 November new users can no longer download the app." The press (The Next Web, The Register) confirms that sign-ups stopped and Cloud shut down on 20 Nov. However, the changelog shows "Conductor is back" (0.94.2, 13 Nov) and releases up to 0.101.0 (25 Nov). Whether new users can download it today is unverified.
- **Orca's 142k stars** in eight months (repo created 17 Mar 2026) and T3 Code's 82k are what `gh api` returns. I did not audit them.
- **Paradev's version.** ADEMark measured 2.0.8, but the GitHub latest release is v0.1.13 on the renamed repo. I have not resolved this.
- **super.engineering** has no official pricing or licence page I could find. Its browser and mobile details come from a competitor's comparison page (paradev.com/compare/superset-vs-super-engineering).
- **Not verified:**
  - Cursor's Windows and Linux support, and its pricing.
  - Worktrees in Cursor, Antigravity, Kiro, Devin Desktop and Factory.
  - Sub-agents in the Codex app.
  - The browser, mobile and worktree status of Devin Desktop.
  - The licences of Zed and Diri.
  - Traycer's repo.
  - Nimbalyst's Android support.
