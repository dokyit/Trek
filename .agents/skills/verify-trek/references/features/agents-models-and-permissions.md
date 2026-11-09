# Agents, models, and permissions

## What it does

Detects installed CLI/ACP agents, exposes subscription and usage information, and supports direct API and local-model providers. Each thread chooses model/effort and one of four hand-holding levels. Settings control default agent behavior and permission policy; API keys live in the macOS Keychain. At **Full access** nothing waits on the user for an approval: what an agent still asks to do (Claude Code asks about some paths even with its checks bypassed) is allowed as it arrives, with no alert and no Needs-you, for a sub-agent working for such a thread too. Questions, plans and secrets still need the user, and so do the Browser's own asks and whatever a permission rule says to ask about every time (`Workspace::apply_agent_events`; test `full_access_allows_what_an_agent_still_asks` in `tests/orchestrate.rs`).

Permission rules sit on top of the levels: a short list in **Settings → Permissions**, each a command, file edit, web fetch or tool pattern plus allow, ask or deny. Deny wins, then ask, then allow; a deny holds at Full access. Command rules read a line the way a shell would (`trek_core::permission_rules`), and anything they can't read counts as matching a deny and never as matching an allow. The same page has a tester (which rule decides, and what happens at each level) and says where rules reach. They're enforced where Trek answers for an agent: Direct's `permitted`, a `PreToolUse` hook on Claude Code's control channel (every mode, `bypassPermissions` included), Codex's approvals (at Full access, its `untrusted` policy while a deny or ask rule exists, so each command checks with Trek; below that, only what asks to leave the sandbox, since an approval under `untrusted` lifts the sandbox for that command: a command inside it runs first, and Trek notes it and stops the turn for a deny), and ACP's permission prompts (only what the agent asks about). `crates/trek-agents/tests/rules_live.rs` checks this against the real CLIs and a local stand-in model.

An approval card's **Allow for session** remembers for the session; **Always allow…** writes a rule, editable first, that Settings lists. Where a rule says to ask every time, no card offers Allow for session (the Mac's, the editor's, the phone's, which hears the rule as `asked_by`), and `respond` turns one into Allow. While an approval or a mid-turn plan waits, sending from the composer (or the phone, or the side bar's ⌘↩) declines it and sends the words as the reason, whatever the queue-or-steer default; the composer's placeholder says so, and the queue key (`⌥↩` when steering is the default) leaves the card up instead. A question still takes typed words as its answer; diff comments wait for it (a toast says so).

An agent asks for a token with `request_secret` on `trek-orchestrate`: a masked card with the name and why. The value goes to a private file under `session-secrets/` in the data folder (0600, removed on quit, on the thread's deletion and at the next launch) and the agent gets only the path. It never goes into the transcript, the store, the logs or the phone, which sees only a note to answer on the computer (and is refused any message, queue edit or plan feedback while the card is up, and the file itself); a message typed while the card is up goes back to the composer it was typed in (a thread window's, the side bar's), a switch of agent waits, and nothing typed meanwhile is kept with the unsent messages. Once given, the value is taken out of every message from anywhere (another agent's brief, a new thread's title) and of what's kept unsent. If the agent prints the value, Trek takes it out of what it shows and stores (`scrub_event`), but can't reach the agent's own history. Direct sessions don't have the tool.

Claude Code and Codex can have several accounts: a label and a config folder of its own under Trek's data folder (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`), linked to the user's own setup. The default account is the user's normal login, no variable set. A thread runs on the account picked in the model menu; moving it starts a new session there from a recap, so in a thread under way the pick waits for the next message as a switch of agent does (the bar names the account), and with a switch waiting the menu lists the new agent's accounts. A thread started from the phone runs on the agent's normal login. The Usage card has a row per account, and a limit bar offers "Continue on <account>".

## Reach it as a user

Open **Settings → Agents & Subscriptions**, **API Keys**, **Local Models**, **General**, or **Permissions**. Thread-specific choices live below the composer.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open settings:agents-&-subscriptions
./.agents/skills/verify-trek/scripts/trek-dev screenshot agents
./.agents/skills/verify-trek/scripts/trek-dev open settings:permissions
./.agents/skills/verify-trek/scripts/trek-dev send "permission"   # the mock asks to run a command: card, "Always allow…", and a typed reply
./.agents/skills/verify-trek/scripts/trek-dev control secret-demo   # a secret's request card on the thread on screen
./.agents/skills/verify-trek/scripts/trek-dev control accounts-demo   # usage-demo, with a second account per agent
```

The isolated app exposes only the Mock agent and never reads the Keychain. Test real-agent detection manually in a normal profile; do not put credentials into verification commands or files.

## Code

`crates/trek-app/src/settings_view/pages.rs`, `composer.rs`, `workspace.rs`; rules and requests: `crates/trek-core/src/permission_rules.rs`, `session_secrets.rs`, `crates/trek-agents/src/rules.rs`, `crates/trek-app/src/settings_view/rules.rs`, `workspace/permission_requests.rs`, `thread_view/permission_cards.rs`; detection/catalog/settings in `crates/trek-core/src/`; sessions in `crates/trek-agents/src/`. Accounts: `crates/trek-core/src/accounts.rs`, `crates/trek-app/src/workspace/accounts.rs`, `crates/trek-app/src/settings_view/accounts.rs`.

## Verify

Detection status and model lists are accurate, unsupported combinations are unavailable, changed defaults affect only new drafts, and permission escalation requires the designed user acknowledgement.

