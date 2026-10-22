//! Threads an agent starts as the user's own (`start_thread`), and the thread its work moves to
//! when its context runs out (`handoff`). Unlike a sub-agent (`orchestrate`), such a thread is in
//! the sidebar like one the user started: it remembers the thread that started it
//! (`Thread::started_by`) or that it continues (`Thread::continued_from`), and both transcripts
//! carry a row linking the two (`trek_core::lineage`).
//!
//! The agent that started a thread can follow it (`list_threads`, `thread_status`, `read_thread`)
//! and message it (`send_to_thread`); no other thread of the user's answers to it. A started
//! thread runs with its starter's access level at most, and chains of them are capped
//! (`lineage::MAX_DEPTH`, `MAX_RUNNING`, `MAX_PER_REQUEST`, `MAX_HANDOFFS_IN_A_ROW`).

use super::{Prefs, Route, UndoAction, Workspace, WorkspaceEvent};
use crate::ipc::{Call, Reply};
use gpui_kit::Context;
use serde_json::{Value, json};
use trek_core::lineage::{
    self, Link, MAX_DEPTH, MAX_HANDOFFS_IN_A_ROW, MAX_PER_REQUEST, MAX_RUNNING,
};
use trek_core::orchestrate::{self as orch, Outcome, Report};
use trek_core::settings::FollowUp;
use trek_core::store::{Item, Thread, now_ms};
use trek_core::{RunState, ThreadSource};

/// Whether an agent started `t` (`start_thread`), or it took over another thread's work (`handoff`).
pub(crate) fn agent_started(t: &Thread) -> bool {
    t.started_by.is_some() || t.continued_from.is_some()
}

/// Where a thread an agent started stands, as its tools and its row say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Working,
    /// Waiting on the user: an approval, a question or a plan.
    NeedsApproval,
    /// Stopped at a usage limit until it resets.
    Paused,
    Finished,
    Failed,
}

impl ThreadState {
    pub fn key(self) -> &'static str {
        match self {
            ThreadState::Working => "working",
            ThreadState::NeedsApproval => "waiting_for_approval",
            ThreadState::Paused => "paused",
            ThreadState::Finished => "finished",
            ThreadState::Failed => "failed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThreadState::Working => "Working",
            ThreadState::NeedsApproval => "Needs your approval",
            ThreadState::Paused => "Paused at a usage limit",
            ThreadState::Finished => "Finished",
            ThreadState::Failed => "Failed",
        }
    }
}

impl Workspace {
    /// One of the lineage tools, called by `caller`'s agent. `None`: the call waits on a thread
    /// (`read_thread` with `wait`), and its answer comes when that's done.
    pub(super) fn handle_thread_call(
        &mut self,
        call: &Call,
        cx: &mut Context<Self>,
    ) -> Option<Result<Value, String>> {
        let caller = call.thread.as_str();
        let text = |k: &str| {
            call.params
                .get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let id = text("id").unwrap_or_default().to_string();
        Some(match call.method.as_str() {
            "start_thread" => self.start_agent_thread(caller, &call.params, cx).map(|id| {
                let mut v = self.thread_json(&id, false);
                v["note"] = json!("It's in the user's sidebar and runs on its own. Follow it with thread_status or read_thread (wait true to wait for it); send it more with send_to_thread.");
                v
            }),
            "handoff" => self.handoff(caller, &call.params, cx).map(|id| {
                let mut v = self.thread_json(&id, false);
                v["note"] = json!("Handed off: the new thread has your brief and carries the work on. End your turn now, without doing more here.");
                v
            }),
            "list_threads" => Ok(self.list_threads(caller)),
            "thread_status" => self.thread_status(caller, &id),
            "read_thread" => {
                let wait = call.params.get("wait").and_then(Value::as_bool) == Some(true);
                if wait && self.follows(caller, &id).is_ok() && self.thread_state(&id) == ThreadState::Working {
                    let waiters = self.thread_waiters.entry(id.clone()).or_default();
                    // Calls given up on since (their connections closed) wait no more.
                    waiters.retain(|w| !w.is_closed());
                    waiters.push(call.reply.clone());
                    let wait = crate::ipc::wait_for(&call.params).min(self.longest_wait(caller));
                    let _ = call.reply.try_send(Reply::WaitingOnThread(id, wait));
                    return None;
                }
                let read = self.read_thread(caller, &id);
                // Read now, its news needn't wake the agent later.
                if read.is_ok() && self.thread_state(&id) != ThreadState::Working {
                    self.heard(caller, &id);
                }
                read
            }
            "send_to_thread" => self.send_to_thread(caller, &id, text("prompt"), cx),
            other => Err(format!("Trek has no tool called {other}.")),
        })
    }

    /// How many levels of agent-started threads (or sub-agents) `id` sits under.
    pub fn lineage_depth(&self, id: &str) -> usize {
        lineage::depth(id, |t| {
            let t = self
                .thread(t)
                .cloned()
                .or_else(|| self.store.thread(t).ok().flatten())?;
            t.started_by.or(t.parent_id)
        })
    }

    /// `caller` as a thread whose agent may start threads of the user's own: one the user can
    /// see in the sidebar (not a sub-agent or a side chat).
    fn may_spawn(&self, caller: &str, what: &str) -> Result<Thread, String> {
        let t = self
            .thread(caller)
            .cloned()
            .ok_or("This thread isn't in Trek any more.")?;
        if t.parent_id.is_some() {
            return Err(format!(
                "You're a sub-agent (started with delegate_task): you can't {what}. Do your task and report back; the agent that started you decides what's next."
            ));
        }
        if t.side_of.is_some() {
            return Err(format!("A side chat can't {what}."));
        }
        Ok(t)
    }

    /// Threads `caller`'s agent started (and their continuations), oldest first.
    pub fn started_threads(&self, caller: &str) -> Vec<&Thread> {
        let mut out: Vec<&Thread> = self
            .threads
            .iter()
            .filter(|t| {
                t.started_by.as_deref() == Some(caller)
                    || t.continued_from.as_deref() == Some(caller)
            })
            .collect();
        out.sort_by_key(|t| t.created_at);
        out
    }

    /// When the user last wrote in `id` (wake-ups and asides don't count).
    fn last_asked(&self, id: &str) -> Option<i64> {
        self.live.get(id).and_then(|l| {
            l.items.iter().rev().find_map(|i| match i {
                Item::User {
                    text,
                    at,
                    aside: false,
                    ..
                } if !orch::is_wake(text) => Some(at.unwrap_or(0)),
                _ => None,
            })
        })
    }

    /// Messages `id` was sent that started its turns: its first (a brief, say) and the user's since.
    fn prompts(&self, id: &str) -> usize {
        let items = match self.live.get(id).filter(|l| l.loaded) {
            Some(l) => l.items.to_vec(),
            None => self.store.items(id).unwrap_or_default(),
        };
        items
            .iter()
            .filter(|i| matches!(i, Item::User { text, aside: false, .. } if !orch::is_wake(text)))
            .count()
    }

    /// Handoffs in a row that led to `id`, with no word from the user in any of the threads.
    fn handoffs_in_a_row(&self, id: &str) -> usize {
        let mut n = 0;
        let mut at = id.to_string();
        while let Some(t) = self
            .thread(&at)
            .cloned()
            .or_else(|| self.store.thread(&at).ok().flatten())
        {
            let Some(from) = t.continued_from.clone() else {
                break;
            };
            // The user (or the agent that started it) said something there besides the brief.
            if self.prompts(&at) > 1 || n > 16 {
                break;
            }
            n += 1;
            at = from;
        }
        n
    }

    /// Start a thread of the user's own for `caller`'s agent, as `params` ask (`start_thread`).
    /// Returns its id.
    pub fn start_agent_thread(
        &mut self,
        caller: &str,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let parent = self.may_spawn(caller, "start threads")?;
        let depth = self.lineage_depth(caller);
        if depth >= MAX_DEPTH {
            return Err(format!(
                "This thread is {depth} levels down a chain of threads that agents started, and Trek allows no deeper: do this part yourself, or leave it for the user."
            ));
        }
        let running = self
            .started_threads(caller)
            .into_iter()
            .filter(|t| {
                t.started_by.as_deref() == Some(caller)
                    && self.thread_state(&t.id) == ThreadState::Working
            })
            .count();
        if running >= MAX_RUNNING {
            return Err(format!(
                "This thread already has {running} threads it started at work, the most Trek allows at once. Wait for one to finish (read_thread with wait true), then start the next."
            ));
        }
        let since = self.last_asked(caller);
        let started = self
            .started_threads(caller)
            .into_iter()
            .filter(|t| t.continued_from.is_none() && since.is_none_or(|s| t.created_at >= s))
            .count();
        if started >= MAX_PER_REQUEST {
            return Err(format!(
                "You've started {MAX_PER_REQUEST} threads since the user's last message, the most Trek allows. Report what you have and let the user decide what's next."
            ));
        }
        let text = |k: &str| {
            params
                .get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let prompt = text("prompt")
            .ok_or("start_thread needs a prompt: the thread's whole task, complete on its own.")?;
        let title = text("title").map(|t| t.chars().take(80).collect::<String>());
        let agent = self.resolve_agent(text("agent"), &parent.agent)?;
        let (model, effort) = self.pick_model(&agent, &parent, text("model"), text("effort"))?;
        if let Some(why) = self.at_limit(&agent, model.as_deref()) {
            return Err(why);
        }
        // Another project Trek knows, by name or folder; else this thread's (its main checkout,
        // for a thread in a worktree: the new thread gets a worktree of its own, or none).
        let folder = match text("project") {
            Some(p) => Some(self.find_project(p)?),
            None => self.draft_folder(&parent),
        };
        let worktree = params.get("worktree").and_then(Value::as_bool) == Some(true);
        // Its starter hears when it's done unless it asked not to.
        let notify = params.get("notify").and_then(Value::as_bool) != Some(false);
        if worktree && !super::in_repo(folder.as_deref()) {
            let name = folder
                .as_deref()
                .map(trek_core::paths::tildify)
                .unwrap_or_else(|| "This thread's folder".into());
            return Err(format!(
                "{name} isn't a git repository, so there's no worktree to make: start it without worktree."
            ));
        }
        let prefs = Prefs {
            agent,
            model,
            effort,
            // Never more access than the thread that starts it has.
            hand_holding: parent.hand_holding,
            // In plan mode, it plans too: nothing it does goes past what its starter may do.
            plan: self.live.get(caller).is_some_and(|l| l.plan),
            fast: false,
            worktree,
        };
        let by = caller.to_string();
        let named = title.clone();
        // A worktree's branch is named after the title when there is one.
        let id = self
            .make_thread(
                folder,
                prefs,
                title.as_deref().unwrap_or(prompt),
                false,
                move |t| {
                    t.started_by = Some(by);
                    t.notify_starter = notify;
                    if let Some(title) = named {
                        t.title = title;
                    }
                },
                cx,
            )
            .map_err(|e| e.to_string())?;
        let new_title = self
            .thread(&id)
            .map(|t| t.title.clone())
            .unwrap_or_default();
        self.link_rows(
            caller,
            &parent.title,
            Link::Started,
            &id,
            &new_title,
            Link::StartedBy,
            cx,
        );
        if notify {
            self.awaited.insert(id.clone());
        }
        self.send_to(&id, prompt.to_string(), vec![], cx);
        Ok(id)
    }

    /// The folder of the project `name` (by name, or by path) among those Trek knows.
    fn find_project(&self, name: &str) -> Result<std::path::PathBuf, String> {
        let want = std::path::Path::new(name);
        let found = self
            .projects
            .iter()
            .find(|p| p.path == want || trek_core::paths::tildify(&p.path) == name)
            .or_else(|| {
                self.projects
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(name))
            });
        match found {
            Some(p) if p.path.is_dir() => Ok(p.path.clone()),
            Some(p) => Err(format!(
                "The project “{}” isn't at {} any more.",
                p.name,
                p.path.display()
            )),
            None => {
                let known: Vec<String> = self
                    .workspace_projects()
                    .iter()
                    .map(|p| format!("{} ({})", p.name, trek_core::paths::tildify(&p.path)))
                    .collect();
                Err(format!(
                    "Trek knows no project “{name}”. These it knows: {}.",
                    if known.is_empty() {
                        "none yet".to_string()
                    } else {
                        known.join(", ")
                    }
                ))
            }
        }
    }

    /// The rows linking `from` and `to`: `from_link` in `from`'s transcript (after its agent's
    /// call), `to_link` at the start of `to`'s.
    #[allow(clippy::too_many_arguments)]
    fn link_rows(
        &mut self,
        from: &str,
        from_title: &str,
        from_link: Link,
        to: &str,
        to_title: &str,
        to_link: Link,
        cx: &mut Context<Self>,
    ) {
        if let Some(live) = self.live.get_mut(to) {
            live.items.push(lineage::row(to_link, from, from_title));
            live.revision += 1;
        }
        self.persist_items(to, cx);
        self.push_row(from, lineage::row(from_link, to, to_title), cx);
    }

    /// Move `caller`'s work to a fresh thread with the brief its agent wrote (`handoff`). The new
    /// thread works in the same folder (worktree and all) and is shown if `caller` was. Returns
    /// its id.
    pub fn handoff(
        &mut self,
        caller: &str,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let parent = self.may_spawn(caller, "hand off")?;
        let text = |k: &str| {
            params
                .get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let brief = text("brief").ok_or("handoff needs a brief: the goal, what's done, decisions, the files that matter and the next steps, complete on its own.")?;
        let since = self.last_asked(caller);
        if let Some(done) = self.threads.iter().find(|t| {
            t.continued_from.as_deref() == Some(caller) && since.is_none_or(|s| t.created_at >= s)
        }) {
            return Err(format!(
                "This thread already handed off to “{}” (id {}). End your turn: the work goes on there.",
                done.title, done.id
            ));
        }
        let row = self.handoffs_in_a_row(caller);
        if row >= MAX_HANDOFFS_IN_A_ROW {
            return Err(format!(
                "This work has been handed off {row} times in a row without a word from the user, the most Trek allows. Stop here and tell the user where things stand."
            ));
        }
        let agent = self.resolve_agent(text("agent"), &parent.agent)?;
        let (model, effort) = self.pick_model(&agent, &parent, text("model"), text("effort"))?;
        if let Some(why) = self.at_limit(&agent, model.as_deref()) {
            return Err(why);
        }
        // Its project's, as a fork's is: it works in the same folder, worktree and all.
        let project = self.project_dir(&parent);
        let mut t = self
            .store
            .create_thread(
                project.as_deref(),
                agent,
                model,
                effort,
                parent.hand_holding,
            )
            .map_err(|e| format!("Couldn't start the new thread: {e}"))?;
        t.title = text("title")
            .map(|t| t.chars().take(80).collect())
            .unwrap_or_else(|| parent.title.clone());
        t.cwd = parent.cwd.clone();
        t.worktree = parent.worktree.clone();
        // The thread that started this one follows its continuation too.
        t.started_by = parent.started_by.clone();
        t.continued_from = Some(caller.to_string());
        self.store
            .save_thread(&t)
            .map_err(|e| format!("Couldn't start the new thread: {e}"))?;
        let (id, title) = (t.id.clone(), t.title.clone());
        self.reload(cx);
        let (plan, fast) = self
            .live
            .get(caller)
            .map_or((false, false), |l| (l.plan, l.fast));
        let live = self.live.entry(id.clone()).or_default();
        live.loaded = true;
        live.plan = plan;
        live.fast = fast;
        self.link_rows(
            caller,
            &parent.title,
            Link::ContinuedIn,
            &id,
            &title,
            Link::ContinuedFrom,
            cx,
        );
        // The old thread's turn ending isn't news: its work goes on in the new one. (A handoff's
        // thread wakes nobody: the thread it continues is done.)
        if let Some(l) = self
            .live
            .get_mut(caller)
            .filter(|l| l.turn_started.is_some())
        {
            l.quiet_turn = true;
        }
        self.send_to(&id, lineage::handoff_prompt(brief), vec![], cx);
        // The user looking at the old thread follows the work to the new one; anywhere else, a
        // toast says where it went.
        if self.in_main(caller) {
            // Taken off it by its own handoff, the user has seen all it has to say: its last
            // words (the agent's own "handed off") leave it read.
            match self
                .live
                .get_mut(caller)
                .filter(|l| l.turn_started.is_some())
            {
                Some(l) => l.seen_at_turn_end = true,
                None => {
                    self.mutate_thread(caller, cx, |t| t.last_seen_at = now_ms().max(t.updated_at))
                }
            }
            self.open_thread_here(&id, cx);
        } else {
            cx.emit(WorkspaceEvent::Toast {
                message: format!("Handed off to a new thread · {title}"),
                undo: Some(UndoAction::OpenThread(id.clone())),
            });
        }
        Ok(id)
    }

    /// `id`, if `caller`'s agent may follow it: a thread it started, or handed off to.
    fn follows(&self, caller: &str, id: &str) -> Result<&Thread, String> {
        let known = self
            .thread(id)
            .cloned()
            .or_else(|| self.store.thread(id).ok().flatten());
        let ours = known.as_ref().is_some_and(|t| {
            t.started_by.as_deref() == Some(caller) || t.continued_from.as_deref() == Some(caller)
        });
        if id.is_empty() || !ours {
            return Err(format!(
                "This thread didn't start a thread “{id}”. Use the id start_thread or handoff returned (list_threads lists them)."
            ));
        }
        self.thread(id)
            .ok_or_else(|| "That thread was archived or deleted.".to_string())
    }

    /// Where thread `id` stands.
    pub fn thread_state(&self, id: &str) -> ThreadState {
        let Some(t) = self.thread(id) else {
            return ThreadState::Finished;
        };
        let live = self.live.get(id);
        if t.run_state == RunState::NeedsYou || live.is_some_and(|l| !l.permissions.is_empty()) {
            return ThreadState::NeedsApproval;
        }
        if t.paused.is_some() {
            return ThreadState::Paused;
        }
        let busy = live.is_some_and(|l| {
            l.turn_started.is_some()
                || !l.queued.is_empty()
                || l.preparing
                || l.background_agents().next().is_some()
        });
        if busy || t.run_state == RunState::Working || self.waiting(id) {
            return ThreadState::Working;
        }
        if t.run_state == RunState::Failed {
            return ThreadState::Failed;
        }
        ThreadState::Finished
    }

    /// `id`'s last answer: what it said in its last turn, or why that failed.
    fn thread_answer(&self, id: &str) -> Option<String> {
        let items = match self.live.get(id).filter(|l| l.loaded) {
            Some(l) => l.items.to_vec(),
            None => self.store.items(id).unwrap_or_default(),
        };
        match self.thread_state(id) {
            ThreadState::Failed => {
                orch::last_error(&items).or_else(|| orch::final_answer(&items, None))
            }
            _ => orch::final_answer(&items, None),
        }
    }

    /// A thread as the lineage tools describe it; with its last answer (capped) when `answer`.
    fn thread_json(&self, id: &str, answer: bool) -> Value {
        let Some(t) = self.thread(id) else {
            return json!({ "id": id, "status": "archived" });
        };
        let state = self.thread_state(id);
        let mut v = json!({
            "id": id,
            // Opens the thread in Trek (`deep_link`): an agent can show it to the user.
            "link": format!("trek://thread/{id}"),
            "title": t.title,
            "status": state.key(),
            "agent": t.agent.key(),
            "model": self.model_label(t),
            "effort": t.effort.as_str(),
            "access": t.hand_holding.label(),
        });
        if let Some(p) = self.project_dir(t) {
            v["project"] = json!(trek_core::paths::tildify(&p));
        }
        if let Some(w) = &t.worktree {
            v["worktree_branch"] = json!(w.branch);
        }
        if let Some(from) = &t.continued_from {
            v["continued_from"] = json!(from);
        }
        if let Some(next) = self
            .threads
            .iter()
            .find(|n| n.continued_from.as_deref() == Some(id))
        {
            v["continued_in"] = json!(next.id);
        }
        if let Some(a) = self
            .thread_answer(id)
            .filter(|_| answer && state != ThreadState::Working)
        {
            v["answer"] = json!(orch::cap(
                &a,
                orch::RESULT_CAP,
                "Open the thread for the rest."
            ));
        }
        v
    }

    pub fn list_threads(&self, caller: &str) -> Value {
        let threads: Vec<Value> = self
            .started_threads(caller)
            .into_iter()
            .map(|t| {
                let mut v = self.thread_json(&t.id, false);
                v["relation"] = json!(if t.continued_from.as_deref() == Some(caller) {
                    "handed_off_to"
                } else {
                    "started"
                });
                v
            })
            .collect();
        let me = self.thread(caller);
        let depth = self.lineage_depth(caller);
        let running = self
            .started_threads(caller)
            .into_iter()
            .filter(|t| {
                t.started_by.as_deref() == Some(caller)
                    && self.thread_state(&t.id) == ThreadState::Working
            })
            .count();
        let projects: Vec<Value> = self
            .workspace_projects()
            .iter()
            .map(|p| json!({ "name": p.name, "path": trek_core::paths::tildify(&p.path) }))
            .collect();
        json!({
            "threads": threads,
            "this_thread": {
                "id": caller,
                "started_by": me.and_then(|t| t.started_by.clone()),
                "continued_from": me.and_then(|t| t.continued_from.clone()),
                "depth": depth,
                "max_depth": MAX_DEPTH,
                "can_start_threads": depth < MAX_DEPTH && me.is_some_and(|t| t.parent_id.is_none() && t.side_of.is_none()),
                "working": running,
                "max_working": MAX_RUNNING,
            },
            "projects": projects,
        })
    }

    pub fn thread_status(&self, caller: &str, id: &str) -> Result<Value, String> {
        self.follows(caller, id)?;
        let mut v = self.thread_json(id, false);
        match self.thread_state(id) {
            ThreadState::Working => {
                // What it's doing right now: its latest step.
                let step = self.live.get(id).and_then(|l| {
                    l.items.iter().rev().find_map(|i| match i {
                        Item::Tool {
                            id, title, detail, ..
                        } if lineage::of_row(id).is_none() => {
                            Some(orch::preview(&format!("{title} {detail}"), 120))
                        }
                        _ => None,
                    })
                });
                if let Some(s) = step {
                    v["latest_step"] = json!(s);
                }
            }
            _ => {
                if let Some(a) = self.thread_answer(id) {
                    v["preview"] = json!(orch::preview(&a, orch::PREVIEW_CAP));
                }
            }
        }
        Ok(v)
    }

    pub fn read_thread(&self, caller: &str, id: &str) -> Result<Value, String> {
        self.follows(caller, id)?;
        let mut v = self.thread_json(id, true);
        if self.thread_state(id) == ThreadState::Working {
            v["note"] =
                json!("Still working. Call read_thread with wait true to wait for its answer.");
        }
        Ok(v)
    }

    /// Send `prompt` to thread `id` for `caller`'s agent, as if the user typed it there: only to
    /// a thread it started, never to one Trek imported, and not over a request the user hasn't
    /// answered (the message would answer it).
    pub fn send_to_thread(
        &mut self,
        caller: &str,
        id: &str,
        prompt: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let t = self.follows(caller, id)?.clone();
        if t.started_by.as_deref() != Some(caller)
            || t.source != ThreadSource::Trek
            || t.imported_title.is_some()
        {
            return Err("You can only message threads you started with start_thread.".into());
        }
        let prompt = prompt.ok_or("send_to_thread needs a prompt: the message.")?;
        if self.thread_state(id) == ThreadState::NeedsApproval {
            return Err("It's waiting on the user (an approval or a question): only they can answer that. Try again once they have.".into());
        }
        let working = self.thread_state(id) == ThreadState::Working;
        // The agent that sent it hears how this goes, as it heard of the thread's first work.
        if t.notify_starter {
            self.awaited.insert(id.to_string());
        }
        self.send_as(id, prompt.to_string(), vec![], Some(FollowUp::Queue), cx);
        let mut v = self.thread_json(id, false);
        v["note"] = json!(if working {
            "It's working: your message waits for its turn to end."
        } else {
            "Sent."
        });
        Ok(v)
    }

    /// A turn of `id` ended (`interrupted`: stopped), or it asked the user something. Unless it's
    /// still at work, calls waiting on it get its answer; if none was waiting and the agent that
    /// started it awaits its work (`awaited`), that agent is woken with how it stands, as it is by
    /// a sub-agent's report.
    pub(super) fn lineage_turn_ended(
        &mut self,
        id: &str,
        interrupted: bool,
        cx: &mut Context<Self>,
    ) {
        let state = self.thread_state(id);
        if state == ThreadState::Working {
            return;
        }
        let mut heard = false;
        if let Some(waiters) = self.thread_waiters.remove(id) {
            let answer = self.thread_json(id, true);
            for w in waiters {
                heard |= w.try_send(Reply::Done(Ok(answer.clone()))).is_ok();
            }
        }
        // Its answer reached the agent in a call, or the user stopped it: no message needed.
        let finished = matches!(state, ThreadState::Finished | ThreadState::Failed);
        if heard || interrupted {
            if finished || interrupted {
                self.awaited.remove(id);
            }
            return;
        }
        if !self.awaited.contains(id)
            || !matches!(
                state,
                ThreadState::Finished | ThreadState::Failed | ThreadState::NeedsApproval
            )
        {
            return;
        }
        // Done with this stretch of work: the next wake-up waits for the next send_to_thread.
        if finished {
            self.awaited.remove(id);
        }
        self.wake_starter(id, state, cx);
    }

    /// Wake the agent that started `id` with how it stands (`state`): in line with its
    /// sub-agents' reports (`gather_wakes`), kept in the store until it hears it. Not for a
    /// handoff's thread, not for a starter that's gone or isn't the user's own thread, and not
    /// past `MAX_PER_REQUEST` such wake-ups since the user's last message there.
    fn wake_starter(&mut self, id: &str, state: ThreadState, cx: &mut Context<Self>) {
        let Some(t) = self.thread(id).cloned() else {
            return;
        };
        let Some(parent) = t
            .started_by
            .clone()
            .filter(|_| t.notify_starter && t.continued_from.is_none())
        else {
            return;
        };
        if self
            .thread(&parent)
            .is_none_or(|p| p.archived_at.is_some() || p.parent_id.is_some() || p.side_of.is_some())
        {
            return;
        }
        let pending = self.wakes.get(&parent).map_or(0, |w| {
            w.iter()
                .filter(|r| r.thread.is_some() && r.id != id)
                .count()
        });
        if self.thread_wakes_since_asked(&parent) + pending >= MAX_PER_REQUEST {
            tracing::info!(
                "not waking {parent} for thread {id}: {MAX_PER_REQUEST} wake-ups since the user's last message"
            );
            return;
        }
        let outcome = match state {
            ThreadState::Failed => Outcome::Failed(
                self.thread_answer(id)
                    .unwrap_or_else(|| "It stopped without saying why.".into()),
            ),
            ThreadState::NeedsApproval => {
                let asked = self
                    .live
                    .get(id)
                    .and_then(|l| l.permissions.first())
                    .map(|p| {
                        if p.detail.is_empty() {
                            p.title.clone()
                        } else {
                            format!("{}: {}", p.title, p.detail)
                        }
                    });
                Outcome::Done(asked.unwrap_or_else(|| "a request".into()))
            }
            _ => Outcome::Done(
                self.thread_answer(id)
                    .unwrap_or_else(|| "(It finished without saying anything.)".into()),
            ),
        };
        let report = Report {
            id: id.to_string(),
            title: t.title.clone(),
            model: self.model_label(&t),
            outcome,
            thread: Some(state.key().into()),
        };
        if let Err(e) = self.store.hold_report(&parent, &report) {
            tracing::warn!("keep the thread's report: {e:#}");
        }
        // The latest news of a thread replaces any still on its way.
        let line = self.wakes.entry(parent.clone()).or_default();
        line.retain(|r| r.id != id);
        line.push(report);
        self.gather_wakes(&parent, cx);
    }

    /// Wake-ups about threads it started that `id`'s agent has had since the user last wrote there.
    fn thread_wakes_since_asked(&self, id: &str) -> usize {
        let Some(live) = self.live.get(id) else {
            return 0;
        };
        let mut n = 0;
        for i in live.items.iter().rev() {
            match i {
                Item::User { text, .. } if orch::is_thread_wake(text) => {
                    n += text
                        .matches(&format!("{}Thread “", orch::WAKE_PREFIX))
                        .count()
                }
                Item::User {
                    text, aside: false, ..
                } if !orch::is_wake(text) => break,
                _ => {}
            }
        }
        n
    }
}

/// Where the link in a lineage row leads, and how it reads in the transcript: the other thread's
/// title (as it is now, else as it was) and how it's doing.
pub struct LineageRow {
    pub link: Link,
    pub thread: String,
    /// It's still in Trek (not archived or deleted): the row opens it.
    pub open: bool,
    pub title: String,
    pub agent: Option<trek_core::AgentId>,
    /// "Working", "Finished"…; for the thread it came from, nothing.
    pub state: Option<ThreadState>,
}

impl LineageRow {
    pub fn read(ws: &Workspace, row_id: &str, detail: &str) -> Option<LineageRow> {
        let (link, thread) = lineage::of_row(row_id)?;
        let t = ws.thread(thread);
        Some(LineageRow {
            link,
            thread: thread.to_string(),
            open: t.is_some(),
            title: t
                .map(|t| t.title.clone())
                .unwrap_or_else(|| detail.to_string()),
            agent: t.map(|t| t.agent.clone()),
            state: t
                .filter(|_| link.answers_a_call())
                .map(|t| ws.thread_state(&t.id)),
        })
    }

    /// Open its thread, from the transcript of `scope`.
    pub fn go(ws: &mut Workspace, thread: &str, from_window: bool, cx: &mut Context<Workspace>) {
        if from_window {
            ws.show_in_main(Route::Thread(thread.to_string()), cx)
        } else {
            ws.open_thread_here(thread, cx)
        }
    }
}
