# Raw notes group B (2026-11-27)
## Codex
- App on macOS + Windows (Windows Mar 4 2026), worktrees + automations built in: https://developers.openai.com/codex/app (search summary), https://openai.com/index/introducing-the-codex-app/
- Pricing: Free $0 (GPT-6 Luna), Go $8, Plus $20, Pro $100/200/500, Business $20-25/user; API key billed at API rates: https://learn.chatgpt.com/docs/pricing
- Codex mobile (start tasks, approve, review from phone); ChatGPT desktop app has browser + computer use: https://learn.chatgpt.com/docs/features.md
- Docs moved to learn.chatgpt.com; Codex now appears inside ChatGPT desktop app ("ChatGPT or Codex" choice): https://learn.chatgpt.com/docs/app.md
- CLI repo openai/codex Apache-2.0, 136,663 stars, rust-v0.172.0 2026-11-24 (gh api repos/openai/codex)
## Claude Code desktop
- Code tab: macOS universal, Windows x64/ARM64, Linux beta; parallel sessions, worktrees (opt-in, .claude/worktrees), Browser pane (agent drives, screenshots/DOM/clicks), computer use (Pro/Max/Ultra only, macOS 14+), Dispatch from phone (not for new users), cloud sessions, Remote Control, SSH, background tasks pane w/ subagents, third-party inference config: https://code.claude.com/docs/en/desktop.md
- Redesign Apr 14 2026 for parallel sessions: https://macrumors.com/2026/04/15/anthropic-rebuilds-claude-code-desktop-app
- claude-code repo 157,325 stars, no license in GitHub metadata (proprietary), pushed 2026-11-27 (gh api)
## OpenCode
- anomalyco/opencode (formerly sst/opencode) MIT, 229,638 stars, TypeScript, v1.19.6 2026-11-17 (gh api)
- Desktop mac/win/linux incl built-in browser, tabs, diffs; mobile app TestFlight+Google Play; 200+ providers via Models.dev incl local; login with ChatGPT/Copilot; free models; Go $10, Go Plus $40: https://opencode.ai/index.md
- OpenCode 2.0 beta exists: https://v2.opencode.ai/
## Factory
- Desktop app released Apr 8 2026 mac+Windows; parallel droids; controls browser/other apps; mobile; BYOK Ollama/vLLM: https://factory.com/news/factory-desktop (factory.ai redirects to factory.com)
- Free plan = BYOK (ChatGPT, Copilot, OpenRouter sign-in); Pro/Plus/Max managed; rolling 5h/7d/30d limits: https://docs.factory.com/pricing/individuals.md ; Pro $20, Plus $100 per search summary (unverified on official page)
- Factory-AI/factory repo 106 stars, no license (docs/ issues repo likely)
## Devin
- Windsurf renamed Devin Desktop Jun 2 2026; Agent Command Center; Devin Local replaces Cascade (Jul 1 2026): https://devin.ai/blog/windsurf-is-now-devin-desktop
## Amp
- Amp Inc (independent from Sourcegraph?), web app, CLI, Mac app beta (Apple silicon macOS 26+), iOS TestFlight: https://ampcode.com/docs/pricing (via search, check)
## Jules
- Out of beta Aug 2025; CLI Oct 2025; 2026 changelog not found: https://jules.google/docs/changelog
## Goose
- aaif-goose/goose (moved from block/goose) Apache-2.0, 57,081 stars, Rust, v1.61.1 2026-11-26 (gh api)
## Terragon
- terragon-labs/terragon-oss Apache-2.0, 262 stars, pushed 2026-02-10; "formally known as Terragon Labs" = shut down (gh api)
## Updates
- Jules SHUTTING DOWN Dec 31 2026 (read-only after Nov 11): https://jules.google/docs/changelog
- Terragon shut down; README snapshot notice Jan 16 2026, Apache-2.0: https://github.com/terragon-labs/terragon-oss
- Goose desktop = Electron 45.4.1 (ui/desktop/package.json, aaif-goose/goose); Linux Foundation AAIF; 85+ providers, local models, ACP both ways, subagents: https://goose-docs.ai/
- OpenCode desktop = Electron 44.9.1 + SolidJS (packages/desktop/package.json); repo latest release v2.4.2
- Devin Desktop (ex-Windsurf): any ACP agent can run inside; macOS/Win/Linux; Devin Local supports subagents; pricing Free/Pro $20/Max $200/Teams $80: https://cognition.com/blog/introducing-devin-desktop , https://docs.devin.ai/desktop/devin-desktop-faq , https://devin.ai/pricing
- Amp: Hobby free (BYOK), Individual $20; apps macOS/iOS/Android/Win/Linux; Mac app beta macOS 26+; runner worktrees since Sep 22 2026: https://ampcode.com/docs/pricing , https://ampcode.com/docs/macos-and-ios
- Codex Apr 16 2026: background computer use mac, in-app browser (comment on DOM; control beyond localhost planned), 90+ plugins, SSH alpha: https://openai.com/index/codex-for-almost-everything/
- Claude cloud sessions on web/mobile Code tab: Pro/Max/Ultra/Team; GitHub only; subagents work: https://code.claude.com/docs/en/claude-code-on-the-web.md
- Copilot Agent HQ: Claude + Codex agents in GitHub/VS Code/mobile since Feb 2026: https://github.blog/news-insights/company-news/welcome-home-agents/
