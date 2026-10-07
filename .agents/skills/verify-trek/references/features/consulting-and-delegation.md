# Consulting and delegation

## What it does

One agent can delegate a scoped task to another through Trek’s MCP server. The composer’s Consult flow asks one or more models to advise or discuss toward agreement, then optionally implement. Child-agent status and answers appear inline and on their parent thread.

## Reach it as a user

Open the composer’s **Consult** control, choose agents/models and advise/discuss/implement behavior, enter the request, and send. Agents can also invoke `delegate_task`; `/consult …` is the typed shortcut.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev send "mock:delegate Review this plan"
./.agents/skills/verify-trek/scripts/trek-dev screenshot delegation
```

The mock path exercises child-thread presentation without launching another vendor process. Full multi-agent selection is covered by headless UI tests rather than the shots command protocol.

## Code

`crates/trek-app/src/composer.rs`, `thread_view.rs`, `workspace/orchestrate.rs`, `ipc.rs`; delegation tools in `crates/trek-mcp/src/orchestrate.rs` and session handling in `crates/trek-agents/src/`.

## Verify

The child appears once with agent identity and live state, its answer returns to the parent, stop propagates, and advisory work remains read-only.
