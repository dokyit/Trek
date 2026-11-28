# raw notes 2026-11-27
## tbench.ai/leaderboard (fetched 2026-11-27) = Terminal-Bench 4.1, 54 entries (WebFetch summary, small-model extraction)
1 Copilot Fusion(max) GitHub Copilot 71.2±2.9 Nov17; 2 Opus5.5 CC 65.8 Sep22; 2 Fable5.2 CC 65.8 Nov9; 4 Gemini4 Carbon Antigravity CLI 64.5 Oct28; 5 Sonnet5.5 CC 63.9 Sep28; 6 GPT-6.2 Astra Codex 62.1 Nov17; 7 GPT-6.2 Sol Codex 61.5; 8 Gemini4 Neon Antigravity 61.2 Nov18; 9 GPT-6.1 Sol Codex 60.9; 10 GPT-6 Astra Codex 60.3 Sep3; 11 Kimi K3.1 Pro CC 59.7; 12 Fable5.1 CC 57.6; 18 Muse Wonder Muse Code 47.0; 22 Grok4.7 Grok Build 38.8; mini-swe-agent entries (MiMo V2.6 Pro 27.0 etc). No Droid/OpenCode/Warp/Amp/Cursor/Goose/Junie/Terminus entries on 4.1 page.
Search: TB 3.0 exists (tbench.ai/news/terminal-bench-3-0); Opus 5 42.7% earlier snapshot; 4.0 superseded.
## tbench.ai/news (fetched 11-27): TB2.1 May 6 2026; TB3.0 Jul 30; TB4.0 Aug 28; TB4.1 Nov 9. Posts: Warp SOTA Jun 25 2025; Leaderboard Integrity Update Apr 19 2026; Harbor-Index Jun 29 2026
## vals.ai/benchmarks/terminal-bench-2-1 (page updated Oct 1 2026, archived, 76 entries; 72 Terminus 2)
Opus5.5 T2 87.64; GPT-6 Astra T2 87.27; Sonnet5.5 T2 83.15; Opus 4.8 T2 71.91 vs Opus 4.8 Claude Code 69.66 (same model, -2.25); GPT5.5 T2 76.40 vs GPT5.5 Codex 57.30 vs GPT5.5 "Factory" 57.30 (-19.1); Composer 2.5 Cursor CLI 58.43.
## tensorlake.ai/blog/droid-scaffold-design-benchmark (Apr 30 2026): Droid 77.3% TB (GPT-5.3-Codex), Opus4.6 69.9, GPT-5.2 64.9, Gemini3Pro 61.1; ForgeCode 81.8 TB2.0. (version unclear)
## fourweekmba Droid beats Claude Code 17% fewer tokens: 403, unverified
## factory.ai/news/terminal-bench: Sep 2025 Droid 58.8 (Opus4.1) #1 (search snippet)
## factory.com/news/frontier-harness (Oct 29 2026, fetched; vendor-run TB4, Oct 23-28, 3 trials, tasks passed by both harnesses in all 3)
GPT-6.1 Sol: Droid 57.6 ($0.79/pass, 25.8min), Codex 54.0, OpenCode 51.0, Devin 45.0
Opus 5.5: Droid 65.2 ($4.73, 42.6min), Cursor 62.1, OpenCode 62.1, Devin 59.6, Claude Code 57.6 (Droid 17% fewer tokens, 26% faster than CC)
versions codex 0.163.0, claude-code 2.1.306, cursor-cli 2026.10.19, devin 3000.11.92, opencode 2.1.2
Droid own perf: first paint median 1839->488ms; time to first prompt 1876->618ms; quit 648->30ms; daemon ready Mac 1956->392ms; CI fails if first-keystroke p95>250ms; memory: before ~350MB per session process, 5 subagents+main >2GB; now ~20MB per session added on shared daemon. Cache 98%. First-req input 20,887->13,055 tokens.
## factory.com/news/terminal-bench (Sep 25 2025): TB core v0.1.1: Droid 58.75 (Opus4.1) #1; Claude Code Opus 43.2; Codex CLI GPT-5 42.8
NOTE: TB4.1 official leaderboard (Nov 17) shows CC Opus5.5 65.8 -- Droid not listed there; Droid vendor numbers 65.2 same-model with CC 57.6 on own run (conflict: CC 65.8 official vs 57.6 Factory run).
## SWE-bench Pro
labs.scale.com/leaderboard/swe_bench_pro_public (fetched; no dates; mini-swe-agent: Muse Spark 1.1 61.5, gpt-5.4 xHigh 59.1, opus-4.6 51.9; SWE-Agent opus-4.5 45.89). No agent products.
augmentcode.com/blog/auggie-tops-swe-bench-pro (Feb 4 2026, upd Jun 18 2026; vendor-run, 731 tasks): Auggie CLI Opus4.5 51.80; Cursor Opus4.5 50.21; Claude Code Opus4.5 49.75; Codex GPT-5.2-codex 46.47; SWE-Agent Scale baseline 45.89
futureagi.com/blog/coding-agent-harness-benchmark (checked Oct 18 2026): mentions Artificial Analysis Coding Agent Index v1.5 Sep 2026 (DeepSWE v1.1 113 tasks, TB4.0 66 tasks, SWE-Atlas-QnA 124); arXiv 2608.26218 harness-change study (unverified). No token/cost per harness.
quesma: Blitzy 66.5% SWE-Bench Pro public (not fetched)
## Artificial Analysis Coding Agent Index v2.1 ("Nov 2026 - current"; FrontierCode-AA 2.0 132 tasks, TB4.1 66 tasks, SWE-Atlas-QnA-AA 124; 3 attempts) artificialanalysis.ai/methodology/coding-agents-benchmarking ; comparisons pages undated (c) 2026, fetched 11-27
Harness legend: Codex, mini-swe-agent, Grok Build, Muse Code, OpenCode, Claude Code, DeepSeek Harness, Mistral Vibe, Pi, GitHub Copilot, Kimi Code CLI, Devin CLI, Qwen Code. NO Droid/Cursor CLI/Gemini CLI on v2.1.
Claude Code: Fable5.2 xhigh 58 $15.47 9.75M tok 53.3m; Fable5.2 high 58 $7.51 5.12M 31.3m; Opus5.5 max 58 $12.61 15.9M 1.1h; Sonnet5.5 max 56 $11.40 26.4M; Fable5.2 max 56 $44.22 24M 2.2h; Fable5.2 med 53 $2.89 1.9M 15.1m; Fable5.2 low 48 $2.49
Codex: GPT-6.2 Astra max 53 $5.12 4.67M 25.1m; Astra xhigh 53 $3.20 3.09M 16.5m; GPT-6.2 Sol xhigh 50 $0.90 12.6m
Same model, different harness: GLM-5.3-Flash(max): Claude Code 39 ($0.58, 14.8M tok, 1.1h), OpenCode 39 ($0.60, 17.2M, 1.2h), Codex 28 ($0.59, 18M, 1.5h). GPT-6 Luna(max): CC 22 ($0.18), OpenCode 23 ($0.16), Codex 26 ($0.19). OpenCode GLM-5.3 max 40 $5.41 17.7M 52.8m.
## Token overhead (search summaries, not fetched): Systima Jul 2026 dev.to CC ~33k tokens vs OpenCode ~7k on trivial prompt; Morph 3-4x; thenewstack Aider/CC/OpenClaw 70-fold.
## App perf (search snippets only, weak): tech-insider.org/?p=19695 Zed 142MB/VSCode 730/Cursor 920 idle; cold start 180ms/2.1s/2.4s; keystroke 8/18/22ms (method unknown, vendor-unverified). zed.dev/compare/cursor (May 1 2026) qualitative only, no numbers (fetched). Cursor forum reports: 22GB across helpers (forum.cursor.com/t/158844), Composer2 exhausts 32GB (159486), 10GB+ v3.19.7 Ubuntu (170546) - anecdotal.
Claude Desktop issues (claudeissues.com mirrors of GitHub; fetch failed ENOTFOUND): main process 3.7GB/h to 47GB macOS (#84693); 2.4GB/70% CPU MacBook Air M3 (#77560); Windows 9 procs 1.18GB idle (#40633); Win Hyper-V VM 1.8GB (dev.to gentic_news Feb 26 2026). Unverified by fetch.
Warp terminals (snippets): idle mem Warp 210-340MB vs Ghostty 28MB, iTerm 185MB, Alacritty 14MB; latency Warp ~8ms Ghostty ~2ms; Warp VTEbench docs.warp.dev/terminal/comparisons/performance (vendor).
## SWE-rebench swe-rebench.com (fetched 11-27) window 15/05/2026-01/07/2026, 111 problems/65 repos; agent rows: Junie 61.8±0.54, Claude Code 60.4±1.03, Codex 58.0±1.29, Cursor 51.7±0.84 (underlying model per agent not stated on page). Models: Fable 5 high 64.5, Grok 4.5 63.8, Opus 5 63.4, GPT-5.6 Sol med 62.3. Note: newer windows may exist; page shows this window.
## T3 Code/Conductor/Codex app: no RAM/startup comparisons found (searched). Accessibility: Warp docs say VoiceOver WIP (docs.warp.dev/features/accessibility); Cursor forum VoiceOver thread 130919; AppleVis thread on Claude Code TUI cursor. Parallel agents: MemMux arXiv 2610.07257 (stub default 120MiB, param only).
## vals.ai/benchmarks/swebench (SWE-bench Verified, updated Sep 1 2026, fetched): Opus 5 mini-SWE-agent 97.00; Opus 4.8 mini-SWE 88.60 vs Opus 4.8 Claude Code 85.80 (-2.8); GPT 5.5 mini-SWE 82.60 vs Codex 76.40 vs Factory 76.20; Composer 2.5 Cursor CLI 79.60. Vals says saturated/archived.
## METR note Feb 13 2026 (fetched): Claude Code vs ReAct on Opus4.5: CC better in 50.7% bootstrap; Codex vs Triframe GPT-5: 14.5%; neither significant. 
## OpenHands Index Jan 29 2026 (fetched): OpenHands SDK only, 9 models, Opus 4.5 #1; no CC/Codex harness comparison.
## arXiv 2604.00362 (Apr 1 2026): gpt-oss harness reproduction; the "tens of points" harness claim is from search summary, not abstract I fetched (abstract only about gpt-oss: SWE-V 60.4 vs 60.7).
## arXiv 2607.22585 Scaffold Effect (Jun 8 2026, fetched abstract): Goose/OpenCode/OpenHands-SDK, Qwen3.6 Plus & MiniMax M2.5, 50-task TB Pro subset; tokens per solved task up to 40x between harnesses; pass-rate gap 0-8 pts.
## Search-only (unfetched): preprint TB2 pass@1 69.7->77.0 harness only; Opus4.5 SWE-Pro 45.9 SEAL vs 55.4 Claude Code. Aider polyglot GPT-5 high 88.0 (stale). Not found: Context-Bench, SWE-Lancer, GSO, Multi-SWE-bench, SWE-bench Live, Warp/Amp/Cline/Kilo/Crush vendor posts.
