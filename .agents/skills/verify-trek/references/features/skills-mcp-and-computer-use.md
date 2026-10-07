# Skills, MCP, and computer use

## What it does

Discovers agent skills across supported homes, lets users enable/disable them, and configures Trek’s built-in computer/simulator/delegation tools plus additional MCP servers. The composer exposes matching `$` skill mentions.

## Reach it as a user

Open **Settings → Skills** to scan/filter/toggle skills. Open **Settings → Tools & MCP** to inspect built-ins and add or remove MCP server commands. Type `$` in a composer to choose an enabled skill.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open settings:skills
./.agents/skills/verify-trek/scripts/trek-dev screenshot skills
./.agents/skills/verify-trek/scripts/trek-dev open settings:tools-&-mcp
```

The isolated profile may discover project skills but does not launch a real agent to consume them. Adding/removing MCP configuration is intentionally not automated by this verification CLI.

## Code

`crates/trek-app/src/settings_view.rs`, `settings_view/pages.rs`; skill discovery in `crates/trek-core/src/skills.rs`; built-in tools in `crates/trek-mcp/src/`.

## Verify

This `verify-trek` skill appears with its description, toggles persist, `$` results match enabled skills, MCP rows report actionable startup failures, and removal affects only the selected server.

