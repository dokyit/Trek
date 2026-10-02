# Competitive Research (2026-10-01)

Sources: local inspection of installed apps, product docs/changelogs, source repos (T3 Code, OpenCode, GPUI Kit),
GitHub issues, HN/Reddit. Full agent transcripts were summarized here.

## Landscape at a glance

| Product | Stack | Shape | Notable |
|---|---|---|---|
| **T3 Code** | Electron 44, React, Effect, SQLite | Inbox sidebar over subscriptions (Codex app-server, Claude Agent SDK, ACP for Cursor/Grok/Antigravity, OpenCode server) | Settle/snooze/pin inbox, traits menu, multi-account providers, transcript scanner onboarding |
| **Claude Code desktop** | Electron | Code tab in Claude app; sessions sidebar with filters, panes (diff, browser, terminal, plan, tasks) | Modes Manual/Accept edits/Plan/Auto/Bypass; update restart kills sessions (bug) |
| **Codex (in ChatGPT app since Jul 9)** | Electron, Sparkle | Projects + chats, Activity bell, review pane, terminal drawer | **Power slider Faster↔Smarter of (model, effort) presets + Advanced**; perms Ask/Approve for me/Full access; 6–8 GB RAM complaints |
| **OpenCode desktop** | Electron 42 + SolidJS (left Tauri: WebKit slow) | Tabs = sessions, projects sidebar | 75+ providers via models.dev; allow/ask/deny per tool; lost Claude OAuth (Anthropic legal) |
| **Cursor 3 "Glass"** | VS Code fork | Agents Window: one sidebar for local/cloud agents, **Needs Attention** group | Auto-review run mode with classifier + sandbox |
| **Zed** | Rust/GPUI | Threads sidebar grouped by project, agent panel, ACP external agents | Effort selector, steer toggle, Write/Ask/Minimal profiles, 120–200 MB idle |
| **Antigravity 2.0** | Electron | Projects → conversations, artifacts pane, Inbox | Default/Request Review/Turbo presets; forced auto-update backlash |
| **Factory Droid** | CLI + Electron desktop | Sessions sidebar, Missions | Autonomy Off/Low/Medium/High (Ctrl+L), Spec mode, effort off→max |
| **Pi** | TS TUI | Minimal 4 tools, no permission prompts | Border color shows thinking level; steer vs queue; session tree |
| **Conductor** | Tauri/WebKit | Workspaces per worktree, grouped by PR status | "Restart when idle" updater, ⌘⌥L next-needs-attention |
| **Zeron** | Rust/GPUI | Sessions grouped by machine, branch diff sidebar | Local-first, in-place updater |
| **MonoCode** | Tauri | Tabs are sessions | /operator meta-agent |
| **Synara** | Electron (T3 fork) | Project → thread | Recaps, side chats, provider handoff |
| **Orca** | Electron (83k★) | Worktree per task, 40+ CLI presets | Design Mode click-to-comment; focus stealing, ordering bugs |
| **Capy** | Electron, cloud VMs | Captain (spec) / Build (PR) | Per-task cost label; opaque credit burn |
| **"DeepSeek harness"** | — | No official one; community **Codewhale** (ex-DeepSeek-TUI, Rust) | Plan/Agent/YOLO |

Measured locally: on-disk size T3 414 MB, Cursor 963 MB, Antigravity 410 MB, OpenCode 408 MB, Orca 568 MB,
Synara 704 MB, Capy 320 MB, Zed 266 MB. T3 Code Nightly RSS ≈ 1.2 GB.

## Visual identity
Icons on this machine: T3 (black squircle, chrome "T3"), Cursor (dark, isometric cube), Antigravity (white,
rainbow-gradient arch), OpenCode (dark, framed rectangle), Orca (black, white swoosh), Synara (white, black Y),
Capy (white, capybara line art), Zed (dark, Z in squares). Codex: blue-violet cloud with `>_`.
**Category = monochrome squircles.** Trek uses a warm sunrise gradient on ink to stand apart.

Motion: T3 defaults panel animations to 0 ms (slider to 400 ms), uses a stepped "live-tool-shine" shimmer that
pauses offscreen, status pulse dots. Zed is restrained (spinners, pulses). Codex/Claude: shimmer "Thinking".

## T3 Code inbox model (from source)
- Sections: `pinned | active | working | snoozed | settled`; project-scope dropdown filter; legacy project tree optional.
- Settle = inbox zero: removes pin, closes idle terminals. **Auto-settle** after 3 days idle (configurable) and on PR merge;
  blocked by live work / pending approvals. Per-thread opt-out.
- Snooze presets (1 h, 3 h, this evening, tomorrow, next week, custom). Snoozed threads **raise their hand**
  (wake early) on approval/input/failure/turn complete → "Woke" marker.
- Status pills: Pending approval amber · Awaiting input indigo · Working sky pulsing · Plan ready violet ·
  Completed-unseen emerald. "Five states, three colors": color only for act-now / in-motion / broken; others recede.
- Drag between sections changes state with a verb badge; every lifecycle action gets a 5 s Undo toast + ⌘Z.
- Row: project favicon/monogram, provider icon, title, status pill, live elapsed timer, PR badge, relative time.

## Model selector patterns
- Codex: one **Power slider** Faster→Smarter; stops are (model, effort) presets; **Advanced** exposes model/effort/speed.
  Efforts Light/Medium/High/Extra High/Max/Ultra.
- T3: provider rail + searchable models + favorites; separate **Traits** menu (effort, fast, thinking);
  shift-click to fan out one prompt to N models (each in a worktree).
- Zed: favorites + effort selector under input. Pi/Factory: Tab / Shift+Tab cycles effort.

## Permission patterns
| Product | Levels |
|---|---|
| Claude Code | Manual · Accept edits · Plan · Auto (classifier) · Bypass (enable in settings) |
| Codex | Ask for approval · Approve for me (auto-review) · Full access |
| T3 Code | Supervised · Auto-accept edits · Auto · Full access (default Full access) |
| Factory | Off · Low · Medium · High (+ Spec) |
| Cursor | Auto-review · Allowlist · Run everything |
| Antigravity | Default (sandbox) · Request review · Turbo |
| OpenCode/Zed | per-tool allow/ask/deny rules |

## Settings structures
- T3: Project, General, Appearance (light/dark themes, VS Code theme import, per-surface fonts, contrast, glass,
  chat width), Keybindings, SnapShots, Providers, Integrations, Source Control, Storage, Connections, Archive;
  searchable; environment → project inheritance.
- Codex: General, Profile, Shortcuts, Notifications, Appearance (theme, accent, fonts), Pets, Browser, Computer use,
  Personalization, Suggested prompts, Memories, Archived.
- Antigravity: Account, Permissions, Appearance, Browser, Models, Customizations (MCP/skills/plugins); per project.
- OpenCode: General, Shortcuts, Providers, Models, Desktop, Server.

## Onboarding patterns
- T3: Connect (this computer / pair device) → Agents → Projects (found by scanning Claude/Codex transcripts,
  grouped by git remote, preselect repos with ≥3 convos in 30 days) → import recent conversations.
- Antigravity 1.x: import VS Code/Cursor settings → theme → autonomy preset → Google sign-in.
- OpenCode: none ("queries went to free models without consent" complaint). Conductor: GitHub OAuth (pushback → gh CLI).

## Updaters
Electron apps: electron-updater/Squirrel; Codex: Sparkle. T3: sidebar pill (check → downloading N% → install) +
release-notes popover + provider-CLI update pills. Conductor: restart when idle. Anti-patterns: Claude desktop's
update restart kills sessions; Antigravity forced update removed features mid-session.

## Complaints to design against
Electron memory (Codex 6–8 GB, Claude long-thread slowness), process sprawl, out-of-order/missing messages, focus
stealing, IME bugs, unsecured remote modes, broken `~` paths, opaque cost/credit burn, forced updates, onboarding
that sends prompts to providers without consent.
