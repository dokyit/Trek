# Consulting, delegation, started threads and handoffs

## What it does

One agent can delegate a scoped task to another through Trek’s MCP server. The composer’s Consult flow asks one or more models to advise or discuss toward agreement, then optionally implement. Child-agent status and answers appear inline and on their parent thread.

An agent can also start a thread of the user's own (`start_thread`): a real top-level thread in the sidebar, in the same project, another project Trek knows, or a new git worktree, on any agent/model (default: the caller's). It runs with the caller's access level at most (plan mode carries over). The caller follows it with `list_threads`, `thread_status`, `read_thread` (optionally waiting) and steers it with `send_to_thread`, which reaches only threads that caller started, never imported ones, and never answers an approval the user owes. When a started thread finishes and the user isn't looking at it, a toast says "Thread completed · <title>" with an **Open thread** button.

`handoff` moves the work to a fresh thread for context-window relief: the agent writes a self-contained brief, Trek starts the new thread in the same folder (worktree and all) with the brief as its first message, and shows it if the user was looking at the old one (otherwise a toast with **Open thread**). The agent then ends its turn.

Both transcripts carry a **lineage** row: "Started thread · <title>" / "Continued in · <title>" (with the other thread's agent and live state) in the starter, "Started by · <title>" / "Continued from · <title>" across the top of the new thread. Each opens the other thread. Caps: three levels of agent-started threads, eight at work per thread, sixteen per user message, three handoffs in a row without a word from the user; sub-agents and side chats can do neither.

## Reach it as a user

Open the composer’s **Consult** control, choose agents/models and advise/discuss/implement behavior, enter the request, and send. Agents can also invoke `delegate_task`; `/consult …` is the typed shortcut. Started threads and handoffs come from the agent's own tool calls (`start_thread`, `handoff`); the user sees the new thread in the sidebar and the lineage rows in both transcripts.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev send "mock:delegate Review this plan"
./.agents/skills/verify-trek/scripts/trek-dev screenshot delegation
# A thread of the user's own (append a prompt for it, e.g. `mock:long 5s`):
./.agents/skills/verify-trek/scripts/trek-dev send "mock:start-thread mock:long 5s"
# The same in a new worktree (the project must be a git repo), or waiting for its answer:
./.agents/skills/verify-trek/scripts/trek-dev send "mock:start-worktree-thread explain the startup"
./.agents/skills/verify-trek/scripts/trek-dev send "mock:start-thread-wait mock:long 2s"
# A handoff whose brief ends in "Next: <what follows>":
./.agents/skills/verify-trek/scripts/trek-dev send "mock:handoff Run the parser tests."
./.agents/skills/verify-trek/scripts/trek-dev screenshot lineage
```

The mock path exercises child-thread presentation without launching another vendor process. Full multi-agent selection is covered by headless UI tests rather than the shots command protocol. `mock:start-thread` leaves the started thread working in the background; the completion toast appears only while another thread is on screen (send from the starter and stay there).

## Code

`crates/trek-app/src/composer.rs`, `thread_view.rs` (`Row::SubAgent`, `Row::Lineage`), `workspace/orchestrate.rs`, `workspace/lineage.rs`, `ipc.rs`; tool schemas in `crates/trek-ipc/src/lib.rs`, served by `crates/trek-mcp/src/orchestrate.rs`; links and caps in `crates/trek-core/src/lineage.rs` (thread columns `started_by`, `continued_from` in `store.rs`); phone mapping in `remote.rs` (`item_body`); mock scripts in `crates/trek-agents/src/mock.rs`. Tests: `crates/trek-app/src/tests/orchestrate.rs`, `tests/lineage.rs`.

## Verify

The child appears once with agent identity and live state, its answer returns to the parent, stop propagates, and advisory work remains read-only. A started thread shows in the sidebar, both transcripts show their lineage rows and each opens the other, the completion toast appears when the user is elsewhere, and a handoff opens the new thread when the user was on the old one.
