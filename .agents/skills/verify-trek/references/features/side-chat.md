# Side Chat

## What it does

Opens a conversation beside the main thread in the right panel, in the same project and a session of its own. It is the editor's AI side bar on a scope of its own (`Scope::Side`): chat tabs (each a side chat), + for a new one, the history of side chats in this project, ⋯, the compact transcript in VS Code / Copilot Chat's layout ("You" over each message, the agent's logo, name and model over each answer, "✓ Ran `cmd`" lines, "Used N references"), its request cards (approvals, questions, plans) and the input with context chips, Agent/Ask/Plan, model and access. Side chats stay off the sidebar and are named from their first message. They are Agents mode's: the editor's AI side bar (same drawing, `Scope::Ide`) keeps editor chats instead, and its menus have no **Open in Agents**; the side chat's do.

## Reach it as a user

Press ⌘J and choose **Side Chat**. Pick the agent/model in its input and send a message. Its + starts another side chat as a tab; the clock lists earlier ones.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open tool:sidechat
./.agents/skills/verify-trek/scripts/trek-dev screenshot side-chat
```

`control click ai-input`, `control type "…"`, `control key enter` send from it (CLI `send` always targets the main composer). Ids are the AI side bar's: `ai-input`, `ai-new-chat`, `ai-history`, `ai-tab`, `ai-draft`, `ai-user`, `ai-speaker`, `ai-command`, `ai-explore`, `ai-approval` / `ai-allow`.

## Code

`crates/trek-app/src/ide/ai/` (the panel, on `Scope::Side`), `workspace/ide.rs` (`side_chat`, `*_in(scope)` tab functions, `send_side_draft`), `panels/mod.rs`.

## Verify

A new tab has an independent transcript/session, streaming does not block the main thread, switching tabs preserves content, and closing a live tab shuts down its session cleanly.

