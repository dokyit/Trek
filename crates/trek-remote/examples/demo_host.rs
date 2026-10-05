//! A Trek remote server over an in-memory fake Trek, for driving the iPhone app end to end.
//!
//! ```sh
//! cargo run -p trek-remote --example demo_host
//! # TREK_REMOTE_ADDR=0.0.0.0:7420   where to listen
//! # TREK_REMOTE_CODE=K7Q2-9XMV      the pairing code (re-armed after every pairing)
//! # TREK_REMOTE_ADVERTISE=host:port what the pairing URL points at (default: this Mac's LAN address)
//! # TREK_REMOTE_DEVICES=path        where paired devices are kept (default: a file in $TMPDIR)
//! # TREK_REMOTE_PLAIN=1             plain ws:// instead of TLS (the identity is kept beside the
//! #                                 devices file, so phones stay paired across runs)
//! ```
//!
//! Seven threads across three projects and three agents. "Dark mode for the settings page" keeps
//! working in the background (tools, streaming text, sometimes an approval); sends get an echoing
//! turn; answers resolve their cards and continue the turn; interrupts stop it. "Harden the
//! request parser" is 2,500 items long, for opening at its end (`subscribe` with a `limit`) and
//! paging back (`transcript_before`). Turns can be undone, retried, forked and rewound.

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::time::sleep;
use trek_remote::pairing::now_ms;
use trek_remote::*;

// ---------------------------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Log {
    seq: u64,
    items: Vec<Item>,
}

struct State {
    threads: Vec<ThreadSummary>,
    projects: Vec<ProjectSummary>,
    agents: Vec<AgentOption>,
    logs: HashMap<String, Log>,
    /// Per-thread turn generation: a turn's task stops when it changes (interrupt, new turn).
    turns: HashMap<String, u64>,
    next_id: u64,
    notes: Vec<Note>,
    settings: MacSettings,
}

struct Shared {
    state: Mutex<State>,
    handle: OnceLock<RemoteHandle>,
}

#[derive(Clone)]
struct DemoHost(Arc<Shared>);

impl std::ops::Deref for DemoHost {
    type Target = Shared;
    fn deref(&self) -> &Shared {
        &self.0
    }
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    fn push(&self, event: HostEvent) {
        if let Some(handle) = self.handle.get() {
            handle.push(event);
        }
    }

    fn new_id(&self, prefix: &str) -> String {
        let mut st = self.lock();
        st.next_id += 1;
        format!("{prefix}{}", st.next_id)
    }

    /// Append a new item; returns its id.
    fn add(&self, thread_id: &str, body: ItemBody) -> String {
        let id = self.new_id("i");
        self.put(thread_id, &id, body);
        id
    }

    /// Upsert an item by id, with the thread's next seq, and push it.
    fn put(&self, thread_id: &str, id: &str, body: ItemBody) {
        let item = {
            let mut st = self.lock();
            let log = st.logs.entry(thread_id.to_string()).or_default();
            log.seq += 1;
            let seq = log.seq;
            match log.items.iter_mut().find(|i| i.id == id) {
                Some(existing) => {
                    existing.seq = seq;
                    existing.body = body;
                    existing.clone()
                }
                None => {
                    let item = Item { id: id.to_string(), seq, at: Some(now_ms()), body };
                    log.items.push(item.clone());
                    item
                }
            }
        };
        self.push(HostEvent::Item { thread_id: thread_id.to_string(), item });
    }

    fn find_item(&self, thread_id: &str, pred: impl Fn(&Item) -> bool) -> Option<Item> {
        self.lock().logs.get(thread_id)?.items.iter().find(|i| pred(i)).cloned()
    }

    fn update(&self, thread_id: &str, f: impl FnOnce(&mut ThreadSummary)) {
        let thread = {
            let mut st = self.lock();
            let Some(t) = st.threads.iter_mut().find(|t| t.id == thread_id) else { return };
            f(t);
            t.updated_at = now_ms();
            t.clone()
        };
        self.push(HostEvent::Thread(thread));
    }

    fn thread(&self, thread_id: &str) -> Option<ThreadSummary> {
        self.lock().threads.iter().find(|t| t.id == thread_id).cloned()
    }

    fn start_turn(&self, thread_id: &str, activity: &str) -> u64 {
        let generation = {
            let mut st = self.lock();
            let turn = st.turns.entry(thread_id.to_string()).or_default();
            *turn += 1;
            *turn
        };
        let activity = activity.to_string();
        self.update(thread_id, |t| {
            t.run_state = RunState::Working;
            t.needs = None;
            t.activity = Some(activity);
            t.working_since = Some(now_ms());
            t.section = if t.pinned { Section::Pinned } else { Section::Working };
        });
        generation
    }

    fn alive(&self, thread_id: &str, generation: u64) -> bool {
        let st = self.lock();
        st.turns.get(thread_id) == Some(&generation)
            && st.threads.iter().any(|t| t.id == thread_id && t.run_state == RunState::Working)
    }

    fn activity(&self, thread_id: &str, activity: &str) {
        let activity = activity.to_string();
        self.update(thread_id, |t| t.activity = Some(activity));
    }

    /// End a turn: `turn_end`, then the thread settles into the inbox, unseen.
    fn finish_turn(&self, thread_id: &str, generation: u64) {
        if !self.alive(thread_id, generation) {
            return;
        }
        let took = self
            .thread(thread_id)
            .and_then(|t| t.working_since)
            .map(|since| ((now_ms() - since) / 1000) as u32)
            .unwrap_or(0);
        self.add(thread_id, ItemBody::TurnEnd { took_secs: took });
        self.update(thread_id, |t| {
            t.run_state = RunState::Idle;
            t.activity = None;
            t.working_since = None;
            t.unseen = true;
            t.section = if t.pinned { Section::Pinned } else { Section::Inbox };
        });
    }

    /// Stop a turn waiting on the user: the thread raises its hand.
    fn needs_you(&self, thread_id: &str, generation: u64, kind: NeedsKind, text: &str) {
        if !self.alive(thread_id, generation) {
            return;
        }
        let text = text.to_string();
        self.update(thread_id, |t| {
            t.run_state = RunState::NeedsYou;
            t.needs = Some(Needs { kind, text });
            t.activity = None;
            t.unseen = true;
            t.section = if t.pinned { Section::Pinned } else { Section::Inbox };
        });
    }

    fn interrupt_turn(&self, thread_id: &str) -> bool {
        let Some(thread) = self.thread(thread_id) else { return false };
        if thread.run_state != RunState::Working {
            return false;
        }
        *self.lock().turns.entry(thread_id.to_string()).or_default() += 1;
        // Anything still running stops.
        let running: Vec<Item> = self
            .lock()
            .logs
            .get(thread_id)
            .map(|log| {
                log.items
                    .iter()
                    .filter(|i| matches!(i.body, ItemBody::Tool { status: ToolStatus::Running, .. } | ItemBody::Assistant { streaming: true, .. }))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for item in running {
            let body = match item.body {
                ItemBody::Tool { call_id, tool, title, detail, output, added, removed, .. } => {
                    ItemBody::Tool { call_id, tool, title, detail, status: ToolStatus::Failed, output, added, removed }
                }
                ItemBody::Assistant { text, .. } => ItemBody::Assistant { text, streaming: false },
                other => other,
            };
            self.put(thread_id, &item.id, body);
        }
        self.add(thread_id, ItemBody::Error { text: "Interrupted".into() });
        self.update(thread_id, |t| {
            t.run_state = RunState::Idle;
            t.activity = None;
            t.working_since = None;
            t.section = if t.pinned { Section::Pinned } else { Section::Inbox };
        });
        true
    }
}

// ---------------------------------------------------------------------------------------------
// Turn scripts
// ---------------------------------------------------------------------------------------------

impl DemoHost {
    /// Wait, then whether the turn is still going.
    async fn step(&self, thread_id: &str, generation: u64, ms: u64) -> bool {
        sleep(Duration::from_millis(ms)).await;
        self.alive(thread_id, generation)
    }

    async fn tool(
        &self,
        thread_id: &str,
        generation: u64,
        title: &str,
        detail: &str,
        output: &[&str],
        diff: Option<(u32, u32)>,
    ) -> bool {
        let id = self.new_id("i");
        let call_id = self.new_id("toolu_");
        let tool = ToolKind::from_title(title);
        let body = |status, out: String, done: bool| ItemBody::Tool {
            call_id: call_id.clone(),
            tool,
            title: title.into(),
            detail: detail.into(),
            status,
            output: truncate_output(&out).to_string(),
            added: if done { diff.map(|d| d.0) } else { None },
            removed: if done { diff.map(|d| d.1) } else { None },
        };
        let verb = match tool {
            ToolKind::Command => "Running",
            ToolKind::Edit => "Editing",
            ToolKind::Read => "Reading",
            ToolKind::Search => "Searching",
            _ => "Using",
        };
        self.activity(thread_id, &format!("{verb} {detail}"));
        self.put(thread_id, &id, body(ToolStatus::Running, String::new(), false));
        let mut out = String::new();
        for line in output {
            if !self.step(thread_id, generation, 900).await {
                return false;
            }
            out.push_str(line);
            out.push('\n');
            self.put(thread_id, &id, body(ToolStatus::Running, out.clone(), false));
        }
        if !self.step(thread_id, generation, 1200).await {
            return false;
        }
        self.put(thread_id, &id, body(ToolStatus::Done, out, true));
        true
    }

    async fn say(&self, thread_id: &str, generation: u64, chunks: &[&str]) -> bool {
        let id = self.new_id("i");
        self.activity(thread_id, "Writing");
        let mut text = String::new();
        for (n, chunk) in chunks.iter().enumerate() {
            if !self.step(thread_id, generation, 700).await {
                return false;
            }
            text.push_str(chunk);
            let streaming = n + 1 < chunks.len();
            self.put(thread_id, &id, ItemBody::Assistant { text: text.clone(), streaming });
        }
        true
    }

    async fn think(&self, thread_id: &str, generation: u64, text: &str) -> bool {
        self.activity(thread_id, "Thinking");
        if !self.step(thread_id, generation, 800).await {
            return false;
        }
        self.add(thread_id, ItemBody::Reasoning { text: text.into() });
        true
    }

    /// The reply to a message from the phone.
    async fn echo_turn(self, thread_id: String, text: String) {
        let generation = self.start_turn(&thread_id, "Thinking");
        let g = generation;
        let id = thread_id.as_str();
        let short: String = text.chars().take(60).collect();
        let ok = self.think(id, g, &format!("The user wants: \u{201c}{short}\u{201d}. Let me check the relevant code first.")).await
            && self.tool(id, g, "Search", "\"TODO\" in src/", &[], None).await
            && self
                .say(
                    id,
                    g,
                    &[
                        "Got it. ",
                        &format!("You asked: *{short}*.\n\n"),
                        "Here's the plan:\n\n1. Read the module that owns this\n",
                        "2. Make the smallest change that does it\n3. Run the tests\n\n",
                        "This is the **demo host**, so nothing actually changed on disk.",
                    ],
                )
                .await;
        if ok {
            self.finish_turn(id, generation);
        }
    }

    /// The working thread's endless script.
    async fn background(self, thread_id: String) {
        let id = thread_id.as_str();
        let mut round = 0u32;
        loop {
            // Wait until the thread is idle (an answer or a phone turn may be running), then pause.
            loop {
                sleep(Duration::from_secs(3)).await;
                if self.thread(id).is_some_and(|t| t.run_state == RunState::Idle) {
                    break;
                }
            }
            if round > 0 {
                sleep(Duration::from_secs(12)).await;
                if self.thread(id).is_none_or(|t| t.run_state != RunState::Idle) {
                    continue;
                }
                let prompts = [
                    "Also respect the system appearance setting",
                    "The toggle flickers on first load, fix that",
                    "Add a test for the theme persistence",
                ];
                let prompt = prompts[(round as usize - 1) % prompts.len()];
                self.add(id, ItemBody::User { text: prompt.into(), images: 0 });
            }
            round += 1;
            let g = self.start_turn(id, "Thinking");
            let ok = self.think(id, g, "The theme tokens live in src/theme/tokens.ts; the settings page reads them through useTheme().").await
                && self.tool(id, g, "Read", "src/theme/tokens.ts", &[], None).await
                && self.say(id, g, &["I'll add a `dark` palette ", "next to the light one ", "and switch on `prefers-color-scheme`."]).await
                && self.tool(id, g, "Edit", "src/pages/Settings.tsx", &[], Some((24 + round, 6))).await
                && self.tool(id, g, "Edit", "src/theme/tokens.ts", &[], Some((41, 2))).await
                && self
                    .tool(
                        id,
                        g,
                        "Run",
                        "pnpm test settings",
                        &[
                            "> lumen-web@2.4.0 test",
                            "> vitest run settings",
                            " ✓ src/pages/Settings.test.tsx (6 tests) 412ms",
                            " Test Files  1 passed (1)",
                            "      Tests  6 passed (6)",
                        ],
                        None,
                    )
                    .await;
            if !ok {
                continue;
            }
            if round.is_multiple_of(2) {
                // Every other round, stop on an approval.
                let request_id = self.new_id("req_");
                self.add(
                    id,
                    ItemBody::Approval {
                        request_id,
                        title: "Run command".into(),
                        detail: "pnpm add -D @testing-library/user-event".into(),
                        state: ApprovalState::Pending,
                    },
                );
                self.needs_you(id, g, NeedsKind::Approval, "Run `pnpm add -D @testing-library/user-event`");
                continue;
            }
            let ok = self
                .say(
                    id,
                    g,
                    &[
                        "Dark mode is in. ",
                        "The settings page now follows the system appearance, ",
                        "with a **Theme** picker (System · Light · Dark) that persists to `localStorage`.\n\n",
                        "Tests pass (6/6).",
                    ],
                )
                .await;
            if ok {
                self.finish_turn(id, g);
            }
        }
    }

    /// Continue a turn after an approval.
    async fn after_approval(self, thread_id: String, command: String, allowed: bool) {
        let id = thread_id.as_str();
        let g = self.start_turn(id, "Thinking");
        let ok = if allowed {
            self.tool(id, g, "Run", &command, &["done in 2.1s"], None).await
                && self.say(id, g, &["That worked. ", "Continuing from where I stopped; ", "everything is green now."]).await
        } else {
            self.say(id, g, &["Understood, ", "I won't run that. ", "I'll leave it for you to do by hand."]).await
        };
        if ok {
            self.finish_turn(id, g);
        }
    }

    async fn after_answer(self, thread_id: String, summary: String) {
        let id = thread_id.as_str();
        let g = self.start_turn(id, "Thinking");
        let ok = self.think(id, g, &format!("The user chose: {summary}.")).await
            && self.tool(id, g, "Edit", "src/middleware/rate_limit.rs", &[], Some((58, 4))).await
            && self.say(id, g, &[&format!("Going with **{summary}**. "), "The limiter is in place; ", "`/login` now allows 5 attempts per minute per IP."]).await;
        if ok {
            self.finish_turn(id, g);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// RemoteHost
// ---------------------------------------------------------------------------------------------

impl RemoteHost for DemoHost {
    async fn snapshot(&self) -> HostResult<Snapshot> {
        let st = self.lock();
        let mut threads = st.threads.clone();
        threads.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        Ok(Snapshot { threads, projects: st.projects.clone(), agents: st.agents.clone(), full_access: true })
    }

    async fn transcript(&self, thread_id: &str) -> HostResult<Transcript> {
        let st = self.lock();
        let log = st.logs.get(thread_id).ok_or_else(|| HostError::not_found(format!("No thread {thread_id}")))?;
        Ok(Transcript { seq: log.seq, items: log.items.clone(), ..Default::default() })
    }

    async fn send(&self, req: SendRequest) -> HostResult<Option<Open>> {
        let thread = self.thread(&req.thread_id).ok_or_else(|| HostError::not_found(format!("No thread {}", req.thread_id)))?;
        if req.text.trim().is_empty() {
            return Err(HostError::bad_request("Empty message"));
        }
        // Trek's own commands: `/new` opens the phone's new-thread sheet; the rest are answered
        // with a notice, as the Mac answers them.
        if let Some(cmd) = req.text.trim().strip_prefix('/').and_then(|c| c.split_whitespace().next()) {
            match cmd {
                "new" | "clear" => return Ok(Some(Open::NewThread { project_id: Some(thread.project.id.clone()) })),
                "usage" | "context" | "cost" | "model" | "permissions" => {
                    self.add(&req.thread_id, ItemBody::User { text: req.text.clone(), images: 0 });
                    let reply = match cmd {
                        "usage" => "**Claude Code** · Claude Max\n- 5-hour limit: 42% used, resets in 2h 10m\n- Weekly limit: 18% used, resets in 4d",
                        "context" => "61.2K of 200K tokens in context (31%).",
                        "cost" => "≈ $1.84 at API prices (included in your Claude Max plan).",
                        "model" => "Claude Code · claude-opus-5-5",
                        _ => "Hand-holding is **Auto-accept edits**.",
                    };
                    self.add(&req.thread_id, ItemBody::Notice { text: reply.into() });
                    return Ok(None);
                }
                _ => {}
            }
        }
        self.add(&req.thread_id, ItemBody::User { text: req.text.clone(), images: 0 });
        self.update(&req.thread_id, |t| t.unseen = false);
        if thread.run_state == RunState::Working {
            match req.mode.unwrap_or(SendMode::Steer) {
                SendMode::Steer => {
                    self.add(&req.thread_id, ItemBody::Notice { text: "Steered the running turn".into() });
                }
                SendMode::Queue => {
                    self.add(&req.thread_id, ItemBody::Notice { text: "Queued for after this turn".into() });
                }
            }
            return Ok(None);
        }
        tokio::spawn(self.clone().echo_turn(req.thread_id, req.text));
        Ok(None)
    }

    async fn new_thread(&self, req: NewThreadRequest) -> HostResult<String> {
        let (project, agent) = {
            let st = self.lock();
            let project = st.projects.iter().find(|p| p.id == req.project_id).cloned();
            let agent = st.agents.iter().find(|a| a.key == req.agent).cloned();
            (project, agent)
        };
        let project = project.ok_or_else(|| HostError::not_found(format!("No project {}", req.project_id)))?;
        let agent = agent.ok_or_else(|| HostError::not_found(format!("No agent {}", req.agent)))?;
        if req.text.trim().is_empty() {
            return Err(HostError::bad_request("Empty message"));
        }
        let model = req.model.clone().or(agent.default_model.clone());
        let model_label = model.as_ref().and_then(|m| agent.models.iter().find(|o| &o.id == m)).map(|o| o.label.clone());
        let id = self.new_id("t-");
        let worktree = req.worktree && project.is_repo;
        let mut title: String = req.text.chars().take(48).collect();
        if req.text.chars().count() > 48 {
            title.push('…');
        }
        let thread = ThreadSummary {
            id: id.clone(),
            title,
            project: project.to_ref(),
            agent: AgentRef { key: agent.key.clone(), name: agent.name.clone(), logo: agent.logo.clone() },
            model,
            model_label,
            branch: if worktree { Some(format!("trek/{id}")) } else { project.branch.clone() },
            worktree,
            base: worktree.then(|| project.branch.clone()).flatten(),
            updated_at: now_ms(),
            effort: req.effort.clone().or(Some("high".into())),
            access: req.access.or(Some(Access::AutoAcceptEdits)),
            plan: req.plan,
            ..Default::default()
        };
        {
            let mut st = self.lock();
            st.threads.push(thread.clone());
            st.logs.insert(id.clone(), Log::default());
        }
        self.push(HostEvent::Thread(thread));
        self.add(&id, ItemBody::User { text: req.text.clone(), images: 0 });
        tokio::spawn(self.clone().echo_turn(id.clone(), req.text));
        Ok(id)
    }

    async fn turn_action(&self, req: TurnActionRequest) -> HostResult<TurnActionDone> {
        let thread = self.thread(&req.thread_id).ok_or_else(|| HostError::not_found(format!("No thread {}", req.thread_id)))?;
        let items = self.lock().logs.get(&req.thread_id).map(|l| l.items.clone()).unwrap_or_default();
        let pos = items.iter().position(|i| i.id == req.item_id).ok_or_else(|| HostError::not_found("No such item"))?;
        let busy = matches!(thread.run_state, RunState::Working | RunState::NeedsYou);
        let ends_turn = matches!(items[pos].body, ItemBody::TurnEnd { .. } | ItemBody::Error { .. } | ItemBody::Limit { .. });
        // The message that started the turn ending at `pos`.
        let start = || {
            let from = items[..pos].iter().rposition(|i| matches!(i.body, ItemBody::TurnEnd { .. } | ItemBody::Error { .. } | ItemBody::Limit { .. })).map_or(0, |b| b + 1);
            items[from..pos].iter().position(|i| matches!(i.body, ItemBody::User { .. })).map(|p| from + p)
        };
        let text_at = |at: usize| match &items[at].body {
            ItemBody::User { text, .. } => text.clone(),
            _ => String::new(),
        };
        let (cut, text) = match req.action {
            TurnAction::Fork => {
                let (keep, text) = match &items[pos].body {
                    ItemBody::User { text, .. } => (pos, Some(text.clone())),
                    _ if ends_turn => (pos + 1, None),
                    _ => return Err(HostError::bad_request("Fork from one of your messages or the end of a turn")),
                };
                let id = self.new_id("t-");
                let fork = ThreadSummary { id: id.clone(), title: format!("{} (fork)", thread.title), run_state: RunState::Idle, needs: None, updated_at: now_ms(), ..thread };
                {
                    let mut st = self.lock();
                    let kept: Vec<Item> = items[..keep].iter().filter(|i| !i.body.is_request()).cloned().collect();
                    st.logs.insert(id.clone(), Log { seq: kept.iter().map(|i| i.seq).max().unwrap_or(0), items: kept });
                    st.threads.push(fork.clone());
                }
                self.push(HostEvent::Thread(fork));
                return Ok(TurnActionDone { thread_id: Some(id), text });
            }
            TurnAction::Rewind => {
                if !matches!(items[pos].body, ItemBody::User { .. }) {
                    return Err(HostError::bad_request("Only your own messages can be rewound to"));
                }
                if busy {
                    return Err(HostError::bad_request("Stop the running turn to rewind"));
                }
                (pos, text_at(pos))
            }
            TurnAction::Undo | TurnAction::Retry => {
                if !ends_turn {
                    return Err(HostError::bad_request("That item doesn't end a turn"));
                }
                if busy {
                    let what = if req.action == TurnAction::Undo { "undo" } else { "retry" };
                    return Err(HostError::bad_request(format!("Stop the running turn to {what}")));
                }
                let at = start().ok_or_else(|| HostError::bad_request("This turn didn't start from a message of yours"))?;
                (at, text_at(at))
            }
        };
        if let Some(log) = self.lock().logs.get_mut(&req.thread_id) {
            log.items.truncate(cut);
        }
        self.push(HostEvent::TranscriptReset(req.thread_id.clone()));
        if req.action == TurnAction::Retry {
            self.add(&req.thread_id, ItemBody::User { text: text.clone(), images: 0 });
            tokio::spawn(self.clone().echo_turn(req.thread_id, text));
            return Ok(TurnActionDone::default());
        }
        Ok(TurnActionDone { thread_id: None, text: Some(text) })
    }

    async fn answer(&self, req: AnswerRequest) -> HostResult<()> {
        let AnswerRequest { thread_id, request_id, response } = req;
        let item = self
            .find_item(&thread_id, |i| match &i.body {
                ItemBody::Approval { request_id: r, .. }
                | ItemBody::Question { request_id: r, .. }
                | ItemBody::Plan { request_id: r, .. } => *r == request_id,
                _ => false,
            })
            .ok_or_else(|| HostError::not_found(format!("No request {request_id} in {thread_id}")))?;
        match (item.body, response) {
            (ItemBody::Approval { request_id, title, detail, state }, AnswerResponse::Approval { decision }) => {
                if state != ApprovalState::Pending {
                    return Err(HostError::conflict("Already answered"));
                }
                let state = match decision {
                    Decision::Allow => ApprovalState::Allowed,
                    Decision::AllowForSession => ApprovalState::AllowedForSession,
                    Decision::Deny => ApprovalState::Denied,
                };
                self.put(&thread_id, &item.id, ItemBody::Approval { request_id, title, detail: detail.clone(), state });
                tokio::spawn(self.clone().after_approval(thread_id, detail, decision != Decision::Deny));
            }
            (ItemBody::Question { request_id, questions, state, .. }, AnswerResponse::Questions { answers }) => {
                if state != QuestionState::Pending {
                    return Err(HostError::conflict("Already answered"));
                }
                let summary = answers.iter().map(|a| a.answer.clone()).collect::<Vec<_>>().join(", ");
                self.put(
                    &thread_id,
                    &item.id,
                    ItemBody::Question { request_id, questions, state: QuestionState::Answered, answers: Some(answers) },
                );
                tokio::spawn(self.clone().after_answer(thread_id, summary));
            }
            (ItemBody::Plan { request_id, markdown, state }, AnswerResponse::Plan { approve, feedback }) => {
                if state != PlanState::Pending {
                    return Err(HostError::conflict("Already answered"));
                }
                let state = if approve { PlanState::Approved } else { PlanState::Rejected };
                self.put(&thread_id, &item.id, ItemBody::Plan { request_id, markdown, state });
                if let Some(feedback) = feedback.filter(|f| !f.trim().is_empty()) {
                    self.add(&thread_id, ItemBody::User { text: feedback, images: 0 });
                }
                let summary = if approve { "the plan as written".to_string() } else { "a revised plan".to_string() };
                tokio::spawn(self.clone().after_answer(thread_id, summary));
            }
            _ => return Err(HostError::bad_request("The answer doesn't match the request")),
        }
        Ok(())
    }

    async fn interrupt(&self, thread_id: &str) -> HostResult<()> {
        if self.thread(thread_id).is_none() {
            return Err(HostError::not_found(format!("No thread {thread_id}")));
        }
        if !self.interrupt_turn(thread_id) {
            return Err(HostError::conflict("The thread isn't running"));
        }
        Ok(())
    }

    async fn mark_seen(&self, thread_id: &str) -> HostResult<()> {
        if self.thread(thread_id).is_none() {
            return Err(HostError::not_found(format!("No thread {thread_id}")));
        }
        self.update(thread_id, |t| t.unseen = false);
        Ok(())
    }

    async fn set_prefs(&self, req: PrefsRequest) -> HostResult<()> {
        let agents = self.lock().agents.clone();
        if self.thread(&req.thread_id).is_none() {
            return Err(HostError::not_found(format!("No thread {}", req.thread_id)));
        }
        self.update(&req.thread_id, |t| {
            if let Some(a) = req.agent.as_ref().and_then(|k| agents.iter().find(|a| &a.key == k)) {
                t.agent = AgentRef { key: a.key.clone(), name: a.name.clone(), logo: a.logo.clone() };
                t.model = a.default_model.clone();
                t.model_label = a.models.iter().find(|o| Some(&o.id) == a.default_model.as_ref()).map(|o| o.label.clone());
            }
            if let Some(m) = &req.model {
                t.model = Some(m.clone());
                t.model_label = agents.iter().flat_map(|a| &a.models).find(|o| &o.id == m).map(|o| o.label.clone());
            }
            if let Some(e) = &req.effort {
                t.effort = Some(e.clone());
            }
            if let Some(a) = req.access {
                t.access = Some(a);
            }
            if let Some(p) = req.plan {
                t.plan = p;
            }
        });
        Ok(())
    }

    async fn thread_action(&self, req: ThreadActionRequest) -> HostResult<()> {
        if self.thread(&req.thread_id).is_none() {
            return Err(HostError::not_found(format!("No thread {}", req.thread_id)));
        }
        self.update(&req.thread_id, |t| match &req.action {
            ThreadAction::Pin => {
                t.pinned = true;
                t.section = Section::Pinned;
            }
            ThreadAction::Unpin | ThreadAction::Unsettle => {
                t.pinned = false;
                t.section = Section::Inbox;
            }
            ThreadAction::Settle => {
                t.pinned = false;
                t.section = Section::Settled;
            }
            ThreadAction::Archive => t.section = Section::Settled,
            ThreadAction::Rename { title } => t.title = title.clone(),
        });
        Ok(())
    }

    async fn usage(&self) -> HostResult<Usage> {
        let hours = |h: i64| Some(now_ms() + h * 3_600_000);
        let limit = |label: &str, percent: f32, resets_at: Option<i64>, window: &str| UsageLimit { label: label.into(), percent, resets_at, window: window.into() };
        Ok(Usage {
            providers: vec![
                ProviderUsage {
                    agent: AgentRef::new("claude-code", "Claude Code", Some("claude-code")),
                    plan: Some("Claude Max".into()),
                    limits: vec![limit("5-hour limit", 42.0, hours(2), "5h"), limit("Weekly limit", 18.0, hours(96), "7d"), limit("Weekly · Opus", 64.0, hours(96), "7d")],
                    note: None,
                    error: None,
                },
                ProviderUsage {
                    agent: AgentRef::new("codex", "Codex", Some("codex")),
                    plan: Some("ChatGPT Pro".into()),
                    limits: vec![limit("5-hour limit", 91.0, hours(1), "5h"), limit("Weekly limit", 33.0, hours(130), "7d")],
                    note: None,
                    error: None,
                },
            ],
            loading: false,
        })
    }

    async fn basecamp(&self, range: BasecampRange) -> HostResult<Basecamp> {
        Ok(demo_basecamp(range, &self.lock().threads))
    }

    async fn notes(&self) -> HostResult<Vec<NoteSummary>> {
        let mut notes = self.lock().notes.clone();
        notes.sort_by_key(|n| std::cmp::Reverse(n.modified));
        let preview = |body: &str| body.lines().skip(1).filter(|l| !l.trim().is_empty()).take(3).collect::<Vec<_>>().join(" ");
        Ok(notes.into_iter().map(|n| NoteSummary { preview: preview(&n.body), id: n.id, title: n.title, modified: n.modified }).collect())
    }

    async fn note(&self, note_id: &str) -> HostResult<Note> {
        self.lock().notes.iter().find(|n| n.id == note_id).cloned().ok_or_else(|| HostError::not_found("No such note"))
    }

    async fn create_note(&self, body: String) -> HostResult<Note> {
        let id = self.new_id("n");
        let note = Note { id, title: note_title(&body), body, modified: now_ms() };
        self.lock().notes.push(note.clone());
        Ok(note)
    }

    async fn save_note(&self, req: SaveNoteRequest) -> HostResult<Note> {
        let mut st = self.lock();
        let note = st.notes.iter_mut().find(|n| n.id == req.note_id).ok_or_else(|| HostError::not_found("No such note"))?;
        if req.modified.is_some_and(|m| m != note.modified) {
            return Err(HostError::conflict("This note changed on the Mac since you opened it"));
        }
        note.title = note_title(&req.body);
        note.body = req.body;
        note.modified = now_ms();
        Ok(note.clone())
    }

    async fn delete_note(&self, note_id: &str) -> HostResult<()> {
        let mut st = self.lock();
        let before = st.notes.len();
        st.notes.retain(|n| n.id != note_id);
        if st.notes.len() == before {
            return Err(HostError::not_found("No such note"));
        }
        Ok(())
    }

    async fn git_status(&self, target: GitTarget) -> HostResult<GitStatus> {
        let thread = self.git_thread(&target)?;
        let file = |path: &str, status: FileStatus, added: u32, removed: u32| ChangedFile { path: path.into(), status, from: None, added, removed, binary: false };
        let mut status = GitStatus {
            is_repo: true,
            branch: thread.as_ref().and_then(|t| t.branch.clone()).or(Some("main".into())),
            default_branch: Some("main".into()),
            ..Default::default()
        };
        match thread.as_ref().filter(|t| t.worktree) {
            Some(t) => {
                status.files = vec![
                    file("src/auth/session.rs", FileStatus::Modified, 12, 3),
                    file("tests/auth/session_test.rs", FileStatus::Modified, 30, 4),
                    file("tests/auth/fixtures.rs", FileStatus::Untracked, 18, 0),
                ];
                status.ahead = 1;
                status.switch_blocked = Some("This thread works in a worktree: its branch stays checked out there.".into());
                status.worktree = Some(WorktreeStatus {
                    branch: t.branch.clone().unwrap_or_default(),
                    base: t.base.clone().unwrap_or_else(|| "main".into()),
                    uncommitted: 2,
                    unpushed: None,
                    merge_blocked: Some("2 files aren't committed yet. Commit or revert them first.".into()),
                    unmerged: 1,
                    missing: false,
                });
            }
            None => {
                status.files = vec![file("README.md", FileStatus::Modified, 31, 54)];
                status.has_upstream = true;
                status.behind = 2;
                status.can_switch = thread.as_ref().is_none_or(|t| t.run_state != RunState::Working);
            }
        }
        Ok(status)
    }

    async fn git_diff(&self, req: GitDiffRequest) -> HostResult<GitDiff> {
        self.git_thread(&req.target)?;
        let p = &req.path;
        let diff = format!(
            "diff --git a/{p} b/{p}\n--- a/{p}\n+++ b/{p}\n@@ -84,9 +84,14 @@ impl Session {{\n     pub async fn refresh(&self) -> Result<Token> {{\n-        let expired = self.token.expires_at < now();\n-        let mut guard = self.lock.lock().await;\n+        let mut guard = self.lock.lock().await;\n+        // Checked under the lock: another refresh may have just finished.\n+        if guard.expires_at > now() {{\n+            return Ok(guard.clone());\n+        }}\n         let fresh = self.client.refresh(&guard.refresh_token).await?;\n"
        );
        Ok(GitDiff { path: req.path, diff, truncated: false })
    }

    async fn git_commit(&self, req: GitCommitRequest) -> HostResult<()> {
        self.git_thread(&req.target)?;
        if req.message.trim().is_empty() {
            return Err(HostError::bad_request("Write a commit message first."));
        }
        Ok(())
    }

    async fn git_push(&self, target: GitTarget) -> HostResult<()> {
        self.git_thread(&target).map(|_| ())
    }

    async fn git_branches(&self, target: GitTarget) -> HostResult<GitBranches> {
        let current = self.git_thread(&target)?.and_then(|t| t.branch).or(Some("main".into()));
        let branches = ["main", "fix/auth-flake", "feat/dark-settings", "release/2.4"].map(String::from).to_vec();
        Ok(GitBranches { current, default_branch: Some("main".into()), branches })
    }

    async fn git_switch(&self, req: GitSwitchRequest) -> HostResult<()> {
        if let Some(t) = self.git_thread(&req.target)? {
            if t.worktree {
                return Err(HostError::conflict("This thread works in a worktree: its branch stays checked out there."));
            }
            if t.run_state == RunState::Working {
                return Err(HostError::conflict("The thread is working: switch once it's done."));
            }
            self.update(&t.id, |t| t.branch = Some(req.branch.clone()));
        }
        Ok(())
    }

    async fn worktree_merge(&self, thread_id: &str) -> HostResult<()> {
        let t = self.thread(thread_id).ok_or_else(|| HostError::not_found("No such thread"))?;
        if !t.worktree {
            return Err(HostError::bad_request("This thread has no worktree."));
        }
        Err(HostError::conflict("2 files aren't committed yet. Commit or revert them first."))
    }

    async fn worktree_remove(&self, req: WorktreeRemoveRequest) -> HostResult<()> {
        let t = self.thread(&req.thread_id).ok_or_else(|| HostError::not_found("No such thread"))?;
        if !t.worktree {
            return Err(HostError::bad_request("This thread has no worktree."));
        }
        if !req.force {
            return Err(HostError::conflict("2 uncommitted changes in the worktree would be lost."));
        }
        self.update(&req.thread_id, |t| {
            t.worktree = false;
            t.base = None;
            t.branch = Some("main".into());
        });
        self.add(&req.thread_id, ItemBody::Notice { text: "Its worktree was removed. The thread runs in the project folder now, in a new agent session.".into() });
        Ok(())
    }

    async fn commands(&self, thread_id: &str) -> HostResult<Vec<CommandInfo>> {
        self.thread(thread_id).ok_or_else(|| HostError::not_found("No such thread"))?;
        let c = |name: &str, description: &str, kind: CommandKind, trek: bool| CommandInfo { name: name.into(), description: description.into(), kind, trek };
        Ok(vec![
            c("new", "Start a new thread in this project", CommandKind::Command, true),
            c("usage", "Show plan usage and reset times", CommandKind::Command, true),
            c("context", "Show how much of the context window is used", CommandKind::Command, true),
            c("cost", "Show this session's estimated cost", CommandKind::Command, true),
            c("model", "Show the model this thread uses", CommandKind::Command, true),
            c("permissions", "Show or change how much the agent asks first", CommandKind::Command, true),
            c("permissions full", "No prompts and no sandbox", CommandKind::Command, true),
            c("consult", "Ask other models first: /consult sol high, opus max: your message", CommandKind::Command, true),
            c("restate", "Have the agent say back what you asked before it starts", CommandKind::Command, true),
            c("compact", "Clear the conversation but keep a summary in context", CommandKind::Command, false),
            c("review", "Review a pull request", CommandKind::Command, false),
            c("frontend-design", "Create distinctive, production-grade frontend interfaces", CommandKind::Skill, false),
            c("code-reviewer", "Reviews code for bugs and style", CommandKind::Agent, false),
        ])
    }

    async fn settings(&self) -> HostResult<MacSettings> {
        Ok(self.lock().settings.clone())
    }

    async fn set_settings(&self, change: SettingsChange) -> HostResult<MacSettings> {
        let mut st = self.lock();
        let s = &mut st.settings;
        if change.default_access == Some(Access::FullAccess) && !s.full_access {
            return Err(HostError::bad_request("Full access is locked on this Mac: unlock it in Trek's Permissions settings first."));
        }
        if let Some(a) = change.default_agent {
            s.default_agent = a;
            s.default_model = None;
        }
        if let Some(m) = change.default_model {
            s.default_model = Some(m).filter(|m| !m.is_empty());
        }
        if let Some(e) = change.default_effort {
            s.default_effort = e;
        }
        if let Some(a) = change.default_access {
            s.default_access = a;
        }
        if let Some(f) = change.follow_up {
            s.follow_up = f;
        }
        if let Some(n) = change.notifications {
            s.notifications = n;
        }
        if let Some(on) = change.push {
            s.push.enabled = on;
            if on && s.push.topic.is_empty() {
                s.push.topic = "trek-demo7Hq2xVb9KpL3mN8RtY4wZ6cE1fJ5".into();
            }
        }
        if let Some(w) = change.push_when {
            s.push.when = w;
        }
        if let Some(server) = change.push_server {
            s.push.server = server;
        }
        if change.push_test {
            println!("demo: a test notification would go to {}/{}", s.push.server, s.push.topic);
        }
        if change.new_push_topic {
            s.push.topic = format!("trek-demo{}", now_ms());
        }
        if let Some(d) = change.auto_settle_days {
            s.auto_settle_days = d;
        }
        if let Some(t) = change.theme {
            s.theme = t;
        }
        (s.push.topic_url, s.push.subscribe_url) = ntfy_links(&s.push.server, &s.push.topic);
        Ok(s.clone())
    }
}

impl DemoHost {
    /// The thread a git request names (`None` for a project), or why there's none.
    fn git_thread(&self, target: &GitTarget) -> HostResult<Option<ThreadSummary>> {
        match (&target.thread_id, &target.project_id) {
            (Some(id), _) => self.thread(id).map(Some).ok_or_else(|| HostError::not_found("No such thread")),
            (None, Some(p)) if self.lock().projects.iter().any(|x| &x.id == p) => Ok(None),
            (None, Some(_)) => Err(HostError::not_found("No such project")),
            (None, None) => Err(HostError::bad_request("Name a thread or a project")),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Sample data
// ---------------------------------------------------------------------------------------------

fn project(id: &str, name: &str, hue: Option<u16>, branch: &str, is_repo: bool) -> ProjectSummary {
    ProjectSummary {
        id: id.into(),
        name: name.into(),
        hue: project_hue(name, hue),
        monogram: monogram(name),
        branch: Some(branch.into()),
        is_repo,
    }
}

fn agents() -> Vec<AgentOption> {
    let efforts = || ["low", "medium", "high", "xhigh", "max"].map(String::from).to_vec();
    let m = |id: &str, label: &str| ModelOption { id: id.into(), label: label.into(), efforts: efforts() };
    vec![
        AgentOption {
            key: "claude-code".into(),
            name: "Claude Code".into(),
            logo: Some("claude-code".into()),
            default_model: Some("claude-opus-5-5".into()),
            models: vec![m("claude-opus-5-5", "Opus 5.5"), m("claude-sonnet-5-5", "Sonnet 5.5"), m("claude-haiku-5", "Haiku 5")],
        },
        AgentOption {
            key: "codex".into(),
            name: "Codex".into(),
            logo: Some("codex".into()),
            default_model: Some("gpt-6".into()),
            models: vec![m("gpt-6", "GPT-6"), m("gpt-6-mini", "GPT-6 mini")],
        },
        AgentOption {
            key: "opencode".into(),
            name: "OpenCode".into(),
            logo: Some("opencode".into()),
            default_model: None,
            models: vec![m("anthropic/claude-sonnet-5-5", "Sonnet 5.5"), m("openai/gpt-6", "GPT-6")],
        },
    ]
}

struct Seed {
    thread: ThreadSummary,
    items: Vec<ItemBody>,
}

fn tool(title: &str, detail: &str, status: ToolStatus, output: &str, diff: Option<(u32, u32)>) -> ItemBody {
    ItemBody::Tool {
        call_id: format!("toolu_{}", detail.len() * 7 + title.len()),
        tool: ToolKind::from_title(title),
        title: title.into(),
        detail: detail.into(),
        status,
        output: output.into(),
        added: diff.map(|d| d.0),
        removed: diff.map(|d| d.1),
    }
}

fn user(text: &str) -> ItemBody {
    ItemBody::User { text: text.into(), images: 0 }
}

fn assistant(text: &str) -> ItemBody {
    ItemBody::Assistant { text: text.into(), streaming: false }
}

fn reasoning(text: &str) -> ItemBody {
    ItemBody::Reasoning { text: text.into() }
}

#[allow(clippy::too_many_arguments)]
fn summary(
    id: &str,
    title: &str,
    project: &ProjectSummary,
    agent: (&str, &str),
    model: (&str, &str),
    run_state: RunState,
    section: Section,
    minutes_ago: i64,
) -> ThreadSummary {
    ThreadSummary {
        id: id.into(),
        title: title.into(),
        project: project.to_ref(),
        agent: AgentRef::new(agent.0, agent.1, Some(agent.0)),
        model: Some(model.0.into()),
        model_label: Some(model.1.into()),
        run_state,
        section,
        branch: project.branch.clone(),
        updated_at: now_ms() - minutes_ago * 60_000,
        effort: Some("high".into()),
        effort_label: Some("High".into()),
        access: Some(Access::AutoAcceptEdits),
        git: Some(GitSummary { changed: 0, ahead: 0, behind: 0, default_branch: Some("main".into()) }),
        ..Default::default()
    }
}

fn seed() -> (Vec<ProjectSummary>, Vec<Seed>) {
    let api = project("p-api", "trek-api", Some(212), "main", true);
    let web = project("p-web", "lumen-web", None, "main", true);
    let infra = project("p-infra", "infra", Some(135), "main", true);
    let claude = ("claude-code", "Claude Code");
    let codex = ("codex", "Codex");
    let opencode = ("opencode", "OpenCode");
    let opus = ("claude-opus-5-5", "Opus 5.5");
    let sonnet = ("claude-sonnet-5-5", "Sonnet 5.5");
    let gpt = ("gpt-6", "GPT-6");

    let mut seeds = Vec::new();

    // 1. Needs an approval.
    let mut t = summary("t-auth", "Fix the flaky auth test", &api, claude, opus, RunState::NeedsYou, Section::Inbox, 2);
    t.needs = Some(Needs { kind: NeedsKind::Approval, text: "Run `rm -rf target/`".into() });
    t.unseen = true;
    t.branch = Some("fix/auth-flake".into());
    t.worktree = true;
    t.base = Some("main".into());
    t.additions = 42;
    t.deletions = 7;
    t.context = Some(ContextUse { used: 61_200, window: 200_000, percent: 31 });
    t.cost = Some(Cost {
        label: "≈ $1.84 at API prices".into(),
        billing: Some(Billing::Plan),
        plan: Some("Claude Max".into()),
        detail: Some("Included in your Claude Max plan".into()),
    });
    t.git = Some(GitSummary { changed: 2, ahead: 1, behind: 0, default_branch: Some("main".into()) });
    seeds.push(Seed {
        thread: t,
        items: vec![
            user("The auth test is flaky, find out why"),
            reasoning("Let me look at the test first. It fails about one run in five on CI, never locally, which smells like a race."),
            tool("Read", "tests/auth/session_test.rs", ToolStatus::Done, "", None),
            tool("Read", "src/auth/session.rs", ToolStatus::Done, "", None),
            tool("Search", "\"refresh(\" in src/auth", ToolStatus::Done, "src/auth/session.rs:88\nsrc/auth/token.rs:12", None),
            assistant("The race is in **`refresh()`**: two requests can both see an expired token and both refresh it, and the second refresh revokes the first one's token.\n\nI'll put the refresh behind a `Mutex` and re-check expiry after taking it."),
            tool("Edit", "src/auth/session.rs", ToolStatus::Done, "", Some((12, 3))),
            tool("Edit", "tests/auth/session_test.rs", ToolStatus::Done, "", Some((30, 4))),
            tool(
                "Run",
                "cargo test -p auth",
                ToolStatus::Failed,
                "   Compiling auth v0.1.0\nerror: failed to remove file `target/debug/deps/libauth.rlib`: stale lock\n",
                None,
            ),
            assistant("The build directory has a stale lock from an earlier crashed run. I'd like to clear it and rebuild."),
            ItemBody::Approval {
                request_id: "req-auth-1".into(),
                title: "Run command".into(),
                detail: "rm -rf target/".into(),
                state: ApprovalState::Pending,
            },
        ],
    });

    // 2. Needs an answer.
    let mut t = summary("t-rate", "Add rate limiting to /login", &api, codex, gpt, RunState::NeedsYou, Section::Inbox, 9);
    t.needs = Some(Needs { kind: NeedsKind::Question, text: "Per IP or per account?".into() });
    t.unseen = true;
    t.context = Some(ContextUse { used: 22_400, window: 272_000, percent: 8 });
    t.cost = Some(Cost { label: "$0.41".into(), billing: Some(Billing::Metered), plan: None, detail: Some("Billed per token by your API provider".into()) });
    seeds.push(Seed {
        thread: t,
        items: vec![
            user("Add rate limiting to /login"),
            reasoning("There's no limiter yet. tower-governor is already in Cargo.lock through the admin API."),
            tool("Search", "\"governor\" in Cargo.lock", ToolStatus::Done, "name = \"tower_governor\"\nversion = \"0.6.1\"", None),
            tool("Read", "src/routes/login.rs", ToolStatus::Done, "", None),
            assistant("I can reuse `tower_governor`. One decision first:"),
            ItemBody::Question {
                request_id: "req-rate-1".into(),
                questions: vec![Question {
                    header: "Scope".into(),
                    question: "Should the limit be per IP or per account?".into(),
                    options: vec![
                        QuestionOption { label: "Per IP".into(), description: "Simple; shared NATs share a budget".into() },
                        QuestionOption { label: "Per account".into(), description: "Stops credential stuffing on one user".into() },
                        QuestionOption { label: "Both".into(), description: "Strictest; a little more code".into() },
                    ],
                    multi: false,
                    secret: false,
                }],
                state: QuestionState::Pending,
                answers: None,
            },
        ],
    });

    // 3. Working (driven by the background task).
    let mut t = summary("t-dark", "Dark mode for the settings page", &web, claude, sonnet, RunState::Idle, Section::Inbox, 1);
    t.branch = Some("feat/dark-settings".into());
    t.worktree = true;
    t.base = Some("main".into());
    t.additions = 65;
    t.deletions = 8;
    t.context = Some(ContextUse { used: 171_000, window: 200_000, percent: 85 });
    t.sub_agents = vec![
        SubAgent { agent: AgentRef::new("codex", "Codex", Some("codex")), model: Some("Sol".into()), title: "Review the theme tokens".into(), state: SubAgentState::Running, since: Some(now_ms() - 95_000) },
        SubAgent { agent: AgentRef::new("claude-code", "Claude Code", Some("claude-code")), model: None, title: "Find every hard-coded colour".into(), state: SubAgentState::Running, since: Some(now_ms() - 40_000) },
    ];
    t.background = vec!["pnpm dev".into()];
    t.git = Some(GitSummary { changed: 5, ahead: 0, behind: 2, default_branch: Some("main".into()) });
    seeds.push(Seed { thread: t, items: vec![user("Add dark mode to the settings page")] });

    // 4. Needs a plan approval.
    let mut t = summary("t-ci", "Migrate CI caching to GitHub Actions cache v4", &infra, opencode, ("anthropic/claude-sonnet-5-5", "Sonnet 5.5"), RunState::NeedsYou, Section::Inbox, 25);
    t.needs = Some(Needs { kind: NeedsKind::Plan, text: "Plan ready: 4 steps".into() });
    seeds.push(Seed {
        thread: t,
        items: vec![
            user("Our CI cache keeps missing. Move us to actions/cache v4 and key it properly."),
            tool("Read", ".github/workflows/ci.yml", ToolStatus::Done, "", None),
            tool("Ran", "gh run list --limit 20 --json conclusion,databaseId", ToolStatus::Done, "[{\"conclusion\":\"success\",\"databaseId\":991}]", None),
            ItemBody::Plan {
                request_id: "req-ci-1".into(),
                markdown: "## Plan\n\n1. Replace `actions/cache@v2` with `@v4` in `ci.yml` and `release.yml`\n2. Key the cargo cache on `hashFiles('**/Cargo.lock')` plus the toolchain\n3. Split the `target/` cache per job so test and clippy stop evicting each other\n4. Add a weekly job that prunes caches older than 14 days".into(),
                state: PlanState::Pending,
            },
        ],
    });

    // 5. Done, unseen.
    let mut t = summary("t-readme", "Rewrite the README quick start", &web, codex, gpt, RunState::Idle, Section::Inbox, 48);
    t.unseen = true;
    t.additions = 31;
    t.deletions = 54;
    seeds.push(Seed {
        thread: t,
        items: vec![
            user("The README quick start is out of date, rewrite it for pnpm"),
            ItemBody::Handoff { from: "Claude Opus 5.5".into(), to: "Codex GPT-6".into() },
            tool("Read", "README.md", ToolStatus::Done, "", None),
            tool("Fetch", "https://pnpm.io/installation", ToolStatus::Done, "", None),
            tool("Write", "README.md", ToolStatus::Done, "", Some((31, 54))),
            assistant("Rewrote **Quick start**:\n\n- `pnpm install` instead of `npm ci`\n- Node 22 as the minimum\n- a `pnpm dev` section with the env vars it needs\n\nI also removed the Docker section; it pointed at an image we no longer publish."),
            ItemBody::TurnEnd { took_secs: 95 },
            ItemBody::Changes {
                files: vec![
                    ChangedFile { path: "README.md".into(), status: FileStatus::Modified, from: None, added: 31, removed: 54, binary: false },
                    ChangedFile { path: "docs/quick-start.md".into(), status: FileStatus::Renamed, from: Some("docs/getting-started.md".into()), added: 4, removed: 2, binary: false },
                    ChangedFile { path: "docs/img/docker.png".into(), status: FileStatus::Deleted, from: None, added: 0, removed: 0, binary: true },
                ],
                added: 35,
                removed: 56,
            },
        ],
    });

    // 6. Hit a usage limit.
    let mut t = summary("t-perf", "Profile the slow dashboard query", &api, claude, opus, RunState::Idle, Section::Inbox, 70);
    t.needs = Some(Needs { kind: NeedsKind::Limit, text: "5-hour limit reached".into() });
    seeds.push(Seed {
        thread: t,
        items: vec![
            user("The dashboard takes 4s to load. Profile the query and fix it."),
            tool("Run", "EXPLAIN ANALYZE (dashboard query)", ToolStatus::Done, "Seq Scan on events  (cost=0.00..48211.20 rows=1203 width=64) (actual time=0.03..3811.22)", None),
            assistant("It's a sequential scan over `events`; there's no index on `(account_id, created_at)`."),
            ItemBody::Limit { text: "5-hour limit reached".into(), resets_at: Some(now_ms() + 2 * 3_600_000) },
        ],
    });

    // 7. Pinned, settled.
    let mut t = summary("t-release", "Release checklist for 2.4", &infra, opencode, ("openai/gpt-6", "GPT-6"), RunState::Idle, Section::Pinned, 60 * 26);
    t.pinned = true;
    seeds.push(Seed {
        thread: t,
        items: vec![
            user("Walk me through what's left for the 2.4 release"),
            tool("Web search", "GitHub release notes best practices", ToolStatus::Done, "", None),
            tool("Subagent", "Collect merged PRs since v2.3.0", ToolStatus::Done, "38 PRs", None),
            ItemBody::Notice { text: "Switched to Full access".into() },
            assistant("Left for 2.4:\n\n- [ ] Changelog (draft ready)\n- [ ] Bump versions\n- [x] Migrations reviewed\n- [ ] Tag and publish"),
            ItemBody::Error { text: "Interrupted".into() },
        ],
    });

    // A long one (2,500 items), for opening at the end and paging back.
    let t = summary("t-history", "Harden the request parser", &api, claude, opus, RunState::Idle, Section::Settled, 60 * 24);
    let mut items = Vec::new();
    for n in 1..=500 {
        items.push(user(&format!("Step {n}: make `parse_header` reject case {n} with a typed error")));
        items.push(tool("Read", "src/parser.rs", ToolStatus::Done, "fn parse_header(line: &str) -> Result<Header> { … }", None));
        items.push(tool("Edit", "src/parser.rs", ToolStatus::Done, "Applied 1 edit", Some((3 + n % 7, n % 3))));
        items.push(assistant(&format!(
            "### Step {n}\n\n`parse_header` now returns `ParseError::BadHeader` for case {n}:\n\n```rust\nErr(ParseError::BadHeader) => reply(400, \"bad header ({n})\"),\n```\n\n| Case | Before | After |\n| --- | --- | --- |\n| {n} | panic | 400 |"
        )));
        items.push(ItemBody::TurnEnd { took_secs: 20 + n % 40 });
    }
    seeds.push(Seed { thread: t, items });

    (vec![api, web, infra], seeds)
}

fn note_title(body: &str) -> String {
    body.lines().map(|l| l.trim_start_matches(['#', '-', '*', ' ']).trim()).find(|l| !l.is_empty()).unwrap_or("Untitled").chars().take(80).collect()
}

fn notes() -> Vec<Note> {
    let note = |id: &str, body: &str, minutes_ago: i64| Note { id: id.into(), title: note_title(body), body: body.into(), modified: now_ms() - minutes_ago * 60_000 };
    vec![
        note("n-release", "# 2.4 release\n\n- [x] Migrations reviewed\n- [ ] Changelog\n- [ ] Tag and publish\n\nAsk **Mara** about the <mark>pricing page</mark> copy.", 35),
        note("n-ideas", "Ideas\n\n- Rate limit per account *and* per IP\n- Cache the dashboard query for 30 s\n- <span style=\"color: #e5484d\">Drop</span> the Docker image", 60 * 20),
    ]
}

fn settings() -> MacSettings {
    let (topic_url, subscribe_url) = ntfy_links("https://ntfy.sh", "");
    MacSettings {
        default_agent: "claude-code".into(),
        default_model: None,
        default_effort: "high".into(),
        default_access: Access::AutoAcceptEdits,
        follow_up: SendMode::Steer,
        notifications: NotifyMode::BannerAndSound,
        push: PushSettings { enabled: false, when: PushWhen::Away, server: "https://ntfy.sh".into(), topic: String::new(), topic_url, subscribe_url },
        auto_settle_days: 3,
        theme: Theme::System,
        full_access: true,
    }
}

/// A recap like the Mac's, made up: today by the hour, a week by three hours, all time by day.
fn demo_basecamp(range: BasecampRange, threads: &[ThreadSummary]) -> Basecamp {
    let (n, title, label): (usize, &str, fn(usize) -> String) = match range {
        BasecampRange::Today => (24, "Today's trek", |i| format!("{}–{} {}", (i + 11) % 12 + 1, (i + 12) % 12 + 1, if i < 12 { "AM" } else { "PM" })),
        BasecampRange::Week => (56, "This week's trek", |i| format!("{} {}", ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][i / 8], ["12–3 AM", "3–6 AM", "6–9 AM", "9 AM–12 PM", "12–3 PM", "3–6 PM", "6–9 PM", "9 PM–12 AM"][i % 8])),
        BasecampRange::All => (60, "Your trek so far", |i| format!("Day {}", i + 1)),
    };
    let shape = |i: usize| ((i as f32 * 0.7).sin() * 0.5 + 0.5) * if range == BasecampRange::Today && !(9..20).contains(&i) { 0.0 } else { 1.0 };
    let buckets: Vec<ProfileBucket> = (0..n)
        .map(|i| {
            let prompts = (shape(i) * 6.0).round() as u32;
            let agent_secs = (shape(i) * 1_500.0) as u64;
            let label = label(i);
            let line = if prompts == 0 { format!("{label} · quiet") } else { format!("{label} · {prompts} prompts · {}m of agent time", agent_secs / 60) };
            ProfileBucket { value: agent_secs as f32 / 60.0, label, line, prompts, agent_secs, tokens: agent_secs * 900 }
        })
        .collect();
    let summit = (0..n).max_by(|a, b| buckets[*a].value.total_cmp(&buckets[*b].value));
    let prompts: u32 = buckets.iter().map(|b| b.prompts).sum();
    let agent_secs: u64 = buckets.iter().map(|b| b.agent_secs).sum();
    let tokens: u64 = buckets.iter().map(|b| b.tokens).sum();
    let api = ProjectRef { id: "p-api".into(), name: "trek-api".into(), hue: 212, monogram: "TA".into() };
    let opus = AgentRef::new("claude-code", "Claude Code", Some("claude-code"));
    let codex = AgentRef::new("codex", "Codex", Some("codex"));
    let text = |t: &str| Span::Text { text: t.into() };
    let strong = |t: String| Span::Strong { text: t };
    let mut sum = 0u64;
    let sparkline = buckets
        .iter()
        .map(|b| {
            sum += b.tokens;
            sum as f32 / tokens.max(1) as f32
        })
        .collect();
    let review = threads
        .iter()
        .filter(|t| t.needs.is_some() || t.unseen)
        .map(|t| {
            let (status, label) = match (&t.needs, t.run_state) {
                (Some(n), _) if n.kind == NeedsKind::Limit => (ReviewStatus::Paused, Some("Paused until 4 PM".to_string())),
                (_, RunState::Failed) => (ReviewStatus::Failed, Some("Failed".to_string())),
                (Some(n), _) => (ReviewStatus::NeedsYou, Some(match n.kind {
                    NeedsKind::Question => "Question",
                    NeedsKind::Plan => "Plan to review",
                    _ => "Approval",
                }.to_string())),
                _ => (ReviewStatus::Done, None),
            };
            ReviewRow { thread_id: t.id.clone(), title: t.title.clone(), status, label, agent: t.agent.clone(), project: Some(t.project.clone()), additions: t.additions, deletions: t.deletions, updated_at: t.updated_at, unseen: t.unseen }
        })
        .collect();
    Basecamp {
        range,
        greeting: match range {
            BasecampRange::All => "Good evening — on the trail since 3 June".into(),
            _ => "Good evening, Monday 5 October".into(),
        },
        title: title.into(),
        updated_at: now_ms(),
        review,
        empty: false,
        invitation: None,
        narrative: vec![
            text(match range {
                BasecampRange::Today => "You sent ",
                BasecampRange::Week => "This week you sent ",
                BasecampRange::All => "So far you've sent ",
            }),
            strong(format!("{prompts} prompts")),
            text(" across "),
            strong("7 threads".into()),
            text(". Most of it went into "),
            Span::Project { text: "trek-api".into(), project: Some(api.clone()) },
            text(", with "),
            Span::Model { text: "Claude Opus 5.5".into(), agent: opus.clone() },
            text(" carrying "),
            strong("77%".into()),
            text(" of the tokens, ahead of "),
            Span::Model { text: "GPT-6".into(), agent: codex },
            text(". Your agents were on the trail for "),
            strong(format!("{}h {}m", agent_secs / 3600, agent_secs % 3600 / 60)),
            text("."),
        ],
        summary: Some(RecapSummary {
            prompts,
            threads: 7,
            turns: prompts + 4,
            agent_secs,
            agent_time: format!("{}h {}m", agent_secs / 3600, agent_secs % 3600 / 60),
            tokens,
            failed: 1,
            top_project: Some(ProjectShare { project: api.clone(), prompts: prompts * 2 / 3, tokens: tokens / 2 }),
            best_model: Some(ModelShare { agent: opus.clone(), label: "Claude Opus 5.5".into(), tokens: tokens * 77 / 100, turns: 9, share: Some(77) }),
        }),
        profile: Some(Profile {
            summit,
            now: Some(n * 3 / 4),
            now_at: 0.75,
            line: summit.map(|i| format!("Summit {}", buckets[i].label)).unwrap_or_else(|| "A flat trail so far".into()),
            total: format!("{prompts} prompts"),
            ticks: match range {
                BasecampRange::Today => [(0.25, "6 AM"), (0.5, "Noon"), (0.75, "6 PM")].map(|(at, l)| Tick { at, label: l.into() }).to_vec(),
                _ => vec![Tick { at: 0.1, label: "Sep 14".into() }, Tick { at: 0.5, label: "Sep 21".into() }, Tick { at: 0.9, label: "Oct 1".into() }],
            },
            buckets,
        }),
        tiles: vec![
            Tile { kind: TileKind::BestModel, label: "Your best model".into(), figure: "Claude Opus 5.5".into(), note: "77% of tokens · 9 turns".into(), agent: Some(opus.clone()), project: None, sparkline: vec![], percent: None, resets_at: None },
            Tile { kind: TileKind::WorkedMostOn, label: "You worked most on".into(), figure: "trek-api".into(), note: format!("{} prompts · 1.2M tokens", prompts * 2 / 3), agent: None, project: Some(api), sparkline: vec![], percent: None, resets_at: None },
            Tile { kind: TileKind::Tokens, label: "You used".into(), figure: format!("{:.1}M tokens", tokens as f64 / 1e6), note: "≈ $14.20 at API prices today".into(), agent: None, project: None, sparkline, percent: None, resets_at: None },
            Tile { kind: TileKind::AgentTime, label: "Your agents worked for".into(), figure: format!("{}h {}m", agent_secs / 3600, agent_secs % 3600 / 60), note: "1 turn failed today".into(), agent: None, project: None, sparkline: vec![], percent: None, resets_at: None },
            Tile { kind: TileKind::PlanLeft, label: "Left on Claude Max".into(), figure: "36%".into(), note: "Weekly · Opus · resets in 4d".into(), agent: Some(opus), project: None, sparkline: vec![], percent: Some(36.0), resets_at: Some(now_ms() + 96 * 3_600_000) },
        ],
    }
}

// ---------------------------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------------------------

/// This Mac's LAN address (no packets are sent).
fn lan_ip() -> Option<std::net::IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,trek_remote=debug".into()),
        )
        .init();

    let bind: SocketAddr = std::env::var("TREK_REMOTE_ADDR")
        .ok()
        .map(|a| a.parse().expect("TREK_REMOTE_ADDR must be ip:port"))
        .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)));
    let code = std::env::var("TREK_REMOTE_CODE").unwrap_or_else(|_| "K7Q2-9XMV".into());
    let advertise = std::env::var("TREK_REMOTE_ADVERTISE").ok().unwrap_or_else(|| {
        let ip = if bind.ip().is_unspecified() { lan_ip().unwrap_or(bind.ip()) } else { bind.ip() };
        SocketAddr::new(ip, bind.port()).to_string()
    });
    let devices_path = std::env::var("TREK_REMOTE_DEVICES")
        .map(Into::into)
        .unwrap_or_else(|_| std::env::temp_dir().join("trek-remote-demo-devices.json"));

    let (projects, seeds) = seed();
    let mut state = State {
        threads: Vec::new(),
        projects,
        agents: agents(),
        logs: HashMap::new(),
        turns: HashMap::new(),
        next_id: 100,
        notes: notes(),
        settings: settings(),
    };
    for Seed { thread, items } in seeds {
        let base = thread.updated_at - items.len() as i64 * 20_000;
        let mut log = Log::default();
        for (n, body) in items.into_iter().enumerate() {
            log.seq += 1;
            let at = matches!(body, ItemBody::User { .. } | ItemBody::TurnEnd { .. }).then_some(base + n as i64 * 20_000);
            log.items.push(Item { id: format!("{}-{}", thread.id, n + 1), seq: log.seq, at, body });
        }
        state.logs.insert(thread.id.clone(), log);
        state.threads.push(thread);
    }

    let host = DemoHost(Arc::new(Shared { state: Mutex::new(state), handle: OnceLock::new() }));
    let mut config = ServerConfig::new(HostInfo {
        id: "demo-host-7f3c".into(),
        name: "Trek Demo Mac".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    });
    config.bind = bind;
    config.advertise = Some(advertise.clone());
    config.devices_path = Some(devices_path.clone());
    if std::env::var_os("TREK_REMOTE_PLAIN").is_none() {
        let dir = devices_path.parent().map(|p| p.to_path_buf()).unwrap_or_else(std::env::temp_dir);
        config.tls = Some(TlsIdentity::load_or_create(&dir, "Trek Demo Mac").expect("TLS identity"));
    }
    let fp = config.tls.as_ref().map(|t| (t.fingerprint.clone(), t.short_fingerprint()));
    let handle = RemoteServer::start(config, Arc::new(host.clone())).await?;
    let _ = host.handle.set(handle.clone());

    let offer = handle.pairing_offer_with_code(&code);
    let port = handle.local_addr().port();
    println!();
    println!("  Trek remote demo host on {}", handle.local_addr());
    println!("  Pairing code  {}", offer.code);
    println!("  Pairing URL   {}", offer.url);
    println!(
        "  Simulator     {}",
        trek_remote::pairing::pairing_url(&format!("127.0.0.1:{port}"), &offer.code, "Trek Demo Mac", "demo-host-7f3c", fp.as_ref().map(|(f, _)| f.as_str()))
    );
    match &fp {
        Some((full, short)) => println!("  TLS           wss://, fingerprint {short} ({full})"),
        None => println!("  TLS           off (plain ws://)"),
    }
    println!("  Devices file  {}", devices_path.display());
    println!("  Paired        {}", handle.devices().len());
    println!();

    // Keep the same code usable: re-arm it after each pairing and before it expires.
    {
        let handle = handle.clone();
        let code = code.clone();
        let mut notices = handle.notices();
        tokio::spawn(async move {
            let mut rearm = tokio::time::interval(Duration::from_secs(8 * 60));
            loop {
                tokio::select! {
                    notice = notices.recv() => match notice {
                        Ok(ServerNotice::Paired { device_id, name }) => {
                            println!("  paired: {name} ({device_id})");
                            handle.pairing_offer_with_code(&code);
                        }
                        Ok(ServerNotice::Connected { device_id }) => println!("  connected: {device_id}"),
                        Ok(ServerNotice::Disconnected { device_id }) => println!("  disconnected: {device_id}"),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(_) => break,
                    },
                    _ = rearm.tick() => { handle.pairing_offer_with_code(&code); }
                }
            }
        });
    }

    tokio::spawn(host.clone().background("t-dark".into()));

    // Runs until Ctrl-C.
    std::future::pending::<()>().await;
    Ok(())
}
