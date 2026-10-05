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
//! turn; answers resolve their cards and continue the turn; interrupts stop it.

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
        Ok(Transcript { seq: log.seq, items: log.items.clone() })
    }

    async fn send(&self, req: SendRequest) -> HostResult<()> {
        let thread = self.thread(&req.thread_id).ok_or_else(|| HostError::not_found(format!("No thread {}", req.thread_id)))?;
        if req.text.trim().is_empty() {
            return Err(HostError::bad_request("Empty message"));
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
            return Ok(());
        }
        tokio::spawn(self.clone().echo_turn(req.thread_id, req.text));
        Ok(())
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
            agent: AgentRef { key: agent.key.clone(), name: agent.name.clone() },
            model,
            model_label,
            run_state: RunState::Idle,
            needs: None,
            section: Section::Inbox,
            unseen: false,
            pinned: false,
            branch: if worktree { Some(format!("trek/{id}")) } else { project.branch.clone() },
            worktree,
            activity: None,
            working_since: None,
            updated_at: now_ms(),
            additions: 0,
            deletions: 0,
            effort: req.effort.clone().or(Some("high".into())),
            access: req.access.or(Some(Access::AutoAcceptEdits)),
            plan: req.plan,
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
                t.agent = AgentRef { key: a.key.clone(), name: a.name.clone() };
                t.model = a.default_model.clone();
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
            default_model: Some("claude-opus-5-5".into()),
            models: vec![m("claude-opus-5-5", "Opus 5.5"), m("claude-sonnet-5-5", "Sonnet 5.5"), m("claude-haiku-5", "Haiku 5")],
        },
        AgentOption {
            key: "codex".into(),
            name: "Codex".into(),
            default_model: Some("gpt-6".into()),
            models: vec![m("gpt-6", "GPT-6"), m("gpt-6-mini", "GPT-6 mini")],
        },
        AgentOption {
            key: "opencode".into(),
            name: "OpenCode".into(),
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
        agent: AgentRef { key: agent.0.into(), name: agent.1.into() },
        model: Some(model.0.into()),
        model_label: Some(model.1.into()),
        run_state,
        needs: None,
        section,
        unseen: false,
        pinned: false,
        branch: project.branch.clone(),
        worktree: false,
        activity: None,
        working_since: None,
        updated_at: now_ms() - minutes_ago * 60_000,
        additions: 0,
        deletions: 0,
        effort: Some("high".into()),
        access: Some(Access::AutoAcceptEdits),
        plan: false,
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
    t.additions = 42;
    t.deletions = 7;
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
    t.additions = 65;
    t.deletions = 8;
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

    (vec![api, web, infra], seeds)
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
