---
name: maintain-verification-skill
description: Bring this project's verification skill up to date with the app: its CLI, dev-environment notes and Feature Map. Use when asked to maintain or refresh the verification skill.
---

# Maintain the verification skill

The project's verification skill is how agents check their own work here. It drifts as the app
changes: commands break, features appear that the Feature Map doesn't know, setup steps change.
Your job is to bring it back in line with the app as it is today.

## 1. See what changed

- Read the skill's SKILL.md, its CLI and `references/features/`.
- Look at what changed in the app since the skill was last touched: `git log` for the app's code
  since the skill's last commit, new routes, screens, menus, commands, settings, removed features.

## 2. Run it

Follow SKILL.md from a clean state as a new agent would. Run `--help` and every subcommand,
`check` above all. Note everything that fails, misleads or is missing.

## 3. Fix it

- Repair broken commands. Keep the CLI agent-friendly: composable subcommands, `--dry-run` on
  anything with side effects, errors that say what to do instead, rich `--help`, JSON output.
- Add commands where agents keep writing throwaway scripts to do the same thing.
- Update the dev-environment notes.
- Update the Feature Map: a file for each new feature, edits for changed ones, and remove
  features that are gone. Keep `references/features/README.md` in step.
- Keep the front matter's `metadata` (`trek: verification` and `cli: …`) accurate, so Trek keeps
  finding the skill and noticing when it runs.

Change only what's out of date: the skill is shared by everyone working here.

## 4. Prove it

Run `check` and two features' flows through the Feature Map again. Finish with a short report of
what you fixed, added and removed, and anything that still needs a person.
