---
name: create-verification-skill
description: Build this project's verification skill, so any agent can drive, debug and check the app on its own. A small agent-friendly CLI, dev-environment notes, and a Feature Map of every feature and how to reach it. Use when asked to set up verification for a project.
---

# Create a verification skill

Verification means an agent can check its own work: run the app, reach the feature it changed, see
what happened, and keep going until it works, without a person in the loop. Your job is to give
this project that ability, as a skill every agent working here will use.

Treat it as infrastructure, not documentation. Prefer tools to prose: a command an agent can run is
worth more than a paragraph telling it what to click.

## 1. Learn how the app runs

Read the project before writing anything:

- How it's built, started and tested (package scripts, Makefiles, Cargo, Xcode schemes, CI files).
- What kind of app it is and what runtime hooks it offers: a browser app has the Chrome DevTools
  Protocol, an Electron app has CDP too, an iOS app has the simulator (`xcrun simctl`), a native
  app may have accessibility APIs, a debug socket, logs or `lldb`, a server has HTTP and logs.
- What a developer needs before it runs: dependencies, environment variables, a seeded database,
  test users, credentials for a staging API.

Run the app yourself. Whatever you had to figure out to get it running is what the skill must say.

## 2. Build the CLI

Write a small CLI that scripts driving and debugging the app. Keep it in the skill folder
(`scripts/`) or in the repo's own tools folder if it has one. Use the project's own language when
that's natural; a shell or Python script is fine for a thin wrapper.

Design it for agents:

- **Composable subcommands**, disclosed gradually: `app start`, `app status`, `app open <screen>`,
  `app click <target>`, `app screenshot <file>`, `app logs --since 1m`, `app check`, `app reset`.
  A few deep commands beat many shallow ones.
- **`check`**: one command that builds and runs whatever proves the app works (a smoke run,
  the tests that matter) and says plainly what passed and what didn't.
- **`--dry-run`** on every command with side effects (resetting data, deleting files, sending
  requests to shared environments): say what would happen, change nothing.
- **Descriptive errors** that tell the agent what to do instead ("The app isn't running: start it
  with `app start`").
- **Rich `--help`** at every level, with an example for each subcommand.
- **JSON output** (`--json`, or by default) so results can be read without scraping text.
- Safe defaults: never touch production, never use a person's real account or data.
- **Works on the checkout it's run from.** Agents often work in a git worktree of the project:
  find the project root from the current folder (`git rev-parse --show-toplevel`), not from where
  the script lives, so a run from a worktree builds and checks that worktree's code.

Run every subcommand you write and fix what fails. A CLI that errors on first use teaches agents to
avoid it.

## 3. Write the dev-environment notes

In the skill, say how to get from a fresh checkout to a running app: install steps, environment
variables (names and where values come from, never the secrets themselves), seeding the dev
database, signing in as a test user, pointing at a test or staging API. Keep it to what an agent
needs to act.

## 4. Map the features

Catalog what the app does from a user's point of view, as a Feature Map:

- `references/features/README.md`: the map. One line per major feature, grouped by area, each
  linking to its own file.
- `references/features/<feature>.md`, one per feature: what it does, how a user reaches it (the
  screens and clicks, or the URL), how to reach it with the CLI, where it lives in the code, and
  how to tell it works.

Go through the app itself (its routes, menus, screens and commands), not just the code, so the map
matches what a user sees.

## 5. Write SKILL.md

Put the skill in this project at `.agents/skills/<name>/` (or `.claude/skills/<name>/` if the
project already keeps its skills there). Give it a short name, such as `control-app` or
`verify-<project>`.

Its front matter must look like this, so Trek can find it and tell when an agent ran it:

```yaml
---
name: control-app
description: Drive, debug and verify <app>. Use it to check any change works before calling it done.
metadata:
  trek: verification
  cli: <how to run the CLI from the project root, e.g. ./.agents/skills/control-app/scripts/app>
---
```

Trek tells a turn verified its work when a shell command runs the CLI by that path, so the body
should show every example that way (`./.agents/skills/control-app/scripts/app check`), run from
the project root.

The body says, briefly: when to use the skill (to verify any change before calling it done), the
CLI's subcommands with one example each, the dev-environment notes, and a pointer to the Feature
Map. Keep SKILL.md short; the details live in the CLI's `--help` and the references.

## 6. Prove it

Use the skill as another agent would: start from a clean state, follow SKILL.md alone, run `check`,
reach two features through the Feature Map, and take a screenshot or capture output as proof. Fix
whatever got in the way.

Finish with a short report: where the skill lives, the CLI's commands, what `check` covers, and
anything you couldn't automate (and why).
