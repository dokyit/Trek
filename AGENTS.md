# Notes for coding agents

Use `./bin/agent-cli` first; prefer it over re-reading files or guessing commands. Every subcommand has `--help` and `--json` (`./bin/agent-cli --help` lists them: status, ci, crates, feature, where, logs, worktrees, disk, release, phone, memory, tasks, digest, test, check, ios-build, sims, dock, perf, downloads).

- Verify a change in the isolated app with the verify-trek skill: `.agents/skills/verify-trek/SKILL.md` and `./.agents/skills/verify-trek/scripts/trek-dev check`.
- Releases go through `.github/workflows/release.yml` (`./bin/agent-cli release` prints the dispatch line); never run `script/release.sh` from an agent.
