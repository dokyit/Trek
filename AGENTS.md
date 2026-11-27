# Notes for coding agents

Use `./bin/agent-cli` first; prefer it over re-reading files or guessing commands. Every subcommand has `--help` and `--json` (`./bin/agent-cli --help` lists them: status, ci, crates, feature, where, logs, worktrees, disk, release, phone, memory, tasks, digest, test, check, ios-build, sims, dock, perf, downloads, symbolicate).
- A freeze file, panic backtrace or macOS crash report from a release names Trek's frames only after `./bin/agent-cli symbolicate <file>`, which fetches that release's saved symbols (docs/RELEASING.md, "Symbols").

- Verify a change in the isolated app with the verify-trek skill: `.agents/skills/verify-trek/SKILL.md` and `./.agents/skills/verify-trek/scripts/trek-dev check`.
- **Before merging a pull request, run the live checks once:** `gh pr edit <number> --add-label live`, then `gh pr checks <number> --watch --interval 120` until Live (macOS, Windows, Linux) and the **Live** status pass. They don't run on ordinary pushes; once labelled, each later push runs them again (docs/CROSS_PLATFORM.md, "The live checks").
- Releases go through `.github/workflows/release.yml` (`./bin/agent-cli release` prints the dispatch line); never run `script/release.sh` from an agent.
