//! What the user's answers to an agent's requests do beyond a yes or no: permission rules added
//! from a card, a typed reply that declines a card, and secrets an agent asks for.
//!
//! A secret comes through `request_secret` on Trek's own MCP server, not through the chat. The
//! card's value goes to a private file (`trek_core::session_secrets`) and the agent gets the path
//! and nothing more. It's never in a `Call`'s reply, an item, a notice, a log line or anything the
//! phone is sent: the phone is told only that a request is waiting on this computer.

use super::{Item, LiveThread, PendingPermission, Workspace, WorkspaceEvent};
use crate::ipc::{Call, Reply};
use gpui_kit::Context;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use trek_agents::{AgentEvent, Decision, Prompt};
use trek_core::permission_rules::{self, PermissionRule, RuleAction};
use trek_core::{RunState, session_secrets};

/// Requests for a secret: their ids start so, which keeps them apart from an agent's own.
const ASK_PREFIX: &str = "trek-secret-ask-";

/// The most of an agent's reason a card shows.
const MAX_WHY: usize = 400;

/// What the composer says when it keeps a message back from a secret's card.
pub const SECRET_IN_CARD: &str =
    "The secret goes in the card, not the chat. Decline the card to say something else.";

/// Why a message from elsewhere (the phone) can't go while a secret's card is up.
pub fn secret_waits_here() -> String {
    format!(
        "This thread is waiting for a secret, which goes in its card on {}. Decline the card there to say something else.",
        crate::words::words().this_computer
    )
}

/// The reply each request waits for, by its request id.
static ASKS: LazyLock<Mutex<HashMap<String, async_channel::Sender<Reply>>>> =
    LazyLock::new(Default::default);

/// The user answered request `request_id` with a plain decision: whether it was for a secret (and
/// has now been refused: this is only ever a no, the value comes through `secret_answer`).
pub(super) fn answered(request_id: &str, _decision: Decision) -> bool {
    if !request_id.starts_with(ASK_PREFIX) {
        return false;
    }
    if let Some(tx) = ASKS.lock().unwrap().remove(request_id) {
        let _ = tx.try_send(Reply::Done(Err(
            "The user declined to give it. Don't ask for it in chat: carry on without it, or say what you can't do."
                .into(),
        )));
    }
    true
}

/// What the agent is told once the value is saved.
fn saved_message(name: &str, path: &std::path::Path) -> String {
    let path = path.display();
    format!(
        "{name} is saved in the file {path}. Its value isn't in this conversation and must stay out of it. Use it by reading the file inside a command, such as `curl -H \"Authorization: Bearer $(cat '{path}')\" …` or `{name}=$(cat '{path}') some-command`. Never print or echo it, never read the file by itself, and never write it into a file that's tracked or shared. The file lasts until Trek quits: if it's gone, call request_secret again."
    )
}

/// Whether the card `request` is up because one of the user's rules says to ask, every time.
pub(super) fn a_rule_asks(ws: &Workspace, thread: &str, request: &PendingPermission) -> bool {
    let cwd = ws.thread(thread).and_then(|t| t.cwd.clone());
    request.call.as_ref().is_some_and(|call| {
        permission_rules::decide(&ws.settings.permissions.rules, call, cwd.as_deref())
            .is_some_and(|v| v.action == RuleAction::Ask)
    })
}

/// What sending from the composer would do to the request on screen, for its quiet note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyNote {
    /// It declines an approval, and the words go with it.
    Declines,
    /// It keeps a plan from going ahead, and the words say why.
    KeepsPlanning,
    /// It won't go: a secret belongs in the card.
    SecretInCard,
}

/// What the composer's message would do to `request`, if anything (a question takes it as the
/// answer, and a plan offered after its turn is simply replaced by it).
fn note_for(request: &PendingPermission) -> Option<ReplyNote> {
    match &request.prompt {
        None => Some(ReplyNote::Declines),
        Some(Prompt::Plan(_)) if !request.after_turn => Some(ReplyNote::KeepsPlanning),
        Some(Prompt::Secret { .. }) => Some(ReplyNote::SecretInCard),
        Some(_) => None,
    }
}

impl LiveThread {
    /// The cards a typed reply declines: approvals and plans waiting mid-turn. All of them, since
    /// the agent can't read the reply until it has its answers.
    fn declined_by_reply(&self) -> Vec<String> {
        self.permissions
            .iter()
            .filter(|p| {
                matches!(
                    note_for(p),
                    Some(ReplyNote::Declines | ReplyNote::KeepsPlanning)
                )
            })
            .map(|p| p.request_id.clone())
            .collect()
    }
}

impl Workspace {
    /// Save `id`'s transcript now, as a turn's end would.
    #[cfg(any(test, feature = "test-support"))]
    pub fn persist_for_test(&mut self, id: &str, cx: &mut Context<Self>) {
        self.persist_items(id, cx);
    }

    /// Add a rule (from a card's "Always allow…"), unless it's already there. It's in force from
    /// here on, in sessions already running, and listed in Settings › Permissions.
    pub fn add_permission_rule(&mut self, rule: PermissionRule, cx: &mut Context<Self>) {
        let rules = &mut self.settings.permissions.rules;
        if !rules.contains(&rule) {
            rules.push(rule);
        }
        self.save_settings(cx);
    }

    pub fn remove_permission_rule(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.settings.permissions.rules.len() {
            self.settings.permissions.rules.remove(index);
        }
        self.save_settings(cx);
    }

    /// Send from a composer, as the user's own typing: if it's waiting on the thread's card, the
    /// words decline it (`send_as` reads `typed_reply`). `follow`: how they said to send it, if
    /// they used the other key from their default.
    pub fn send_typed(
        &mut self,
        scope: &crate::workspace::Scope,
        text: String,
        images: Vec<std::path::PathBuf>,
        follow: Option<trek_core::settings::FollowUp>,
        cx: &mut Context<Self>,
    ) {
        self.typed_reply = true;
        let running = self
            .thread_id_in(scope)
            .map(str::to_string)
            .filter(|id| self.turn_running(id));
        match (follow, running) {
            (Some(follow), Some(id)) => self.send_as(&id, text, images, Some(follow), cx),
            _ => self.send_in(scope, text, images, cx),
        }
        self.typed_reply = false;
    }

    /// `send_typed` for thread `id` itself, saying how a message sent while a turn runs goes:
    /// what the phone's composer sends, which declines a card as the Mac's does.
    pub fn send_typed_to(
        &mut self,
        id: &str,
        text: String,
        images: Vec<std::path::PathBuf>,
        follow: Option<trek_core::settings::FollowUp>,
        cx: &mut Context<Self>,
    ) {
        self.typed_reply = true;
        self.send_as(id, text, images, follow, cx);
        self.typed_reply = false;
    }

    /// The quiet note the composer shows for thread `id`, if sending would answer its card.
    pub fn reply_note(&self, id: &str) -> Option<ReplyNote> {
        self.live.get(id)?.permissions.iter().find_map(note_for)
    }

    /// Thread `id` waits on a secret's card, whatever else waits with it. Words for it from
    /// anywhere but that card may be the secret itself, pasted in the wrong place.
    pub fn asks_for_secret(&self, id: &str) -> bool {
        self.live.get(id).is_some_and(|l| {
            l.permissions
                .iter()
                .any(|p| note_for(p) == Some(ReplyNote::SecretInCard))
        })
    }

    /// Whether the card `request_id` of `id` may be allowed for the rest of the session: not when
    /// one of the user's rules asks about it every time (the session would never ask again).
    pub fn session_approval_offered(&self, id: &str, request_id: &str) -> bool {
        self.live
            .get(id)
            .and_then(|l| l.permissions.iter().find(|p| p.request_id == request_id))
            .is_none_or(|p| !a_rule_asks(self, id, p))
    }

    /// The rule of the user's that asks about card `request_id` of `id` every time, as Settings
    /// shows it.
    pub fn rule_asking(&self, id: &str, request_id: &str) -> Option<String> {
        let request = self
            .live
            .get(id)?
            .permissions
            .iter()
            .find(|p| p.request_id == request_id)?;
        let cwd = self.thread(id).and_then(|t| t.cwd.clone());
        let rules = &self.settings.permissions.rules;
        let verdict = permission_rules::decide(rules, request.call.as_ref()?, cwd.as_deref())?;
        (verdict.action == RuleAction::Ask).then(|| rules[verdict.rule].shown())
    }

    /// The user typed a reply while cards wait on `id`: they're declined, for the words to go
    /// straight to the agent as their reason. Whether any was.
    pub(super) fn decline_for_reply(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let waiting = self
            .live
            .get(id)
            .map(LiveThread::declined_by_reply)
            .unwrap_or_default();
        for request in &waiting {
            self.respond(id, request, Decision::Deny, cx);
        }
        !waiting.is_empty()
    }

    /// A message typed while a secret's card is up can't go: it's where a token pasted by
    /// mistake would land in the transcript for good. It goes back to the composer it came from
    /// (whichever window or pane shows the thread), and whether that happened.
    pub(super) fn hold_reply_for_secret(
        &mut self,
        id: &str,
        text: &str,
        images: &[std::path::PathBuf],
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.asks_for_secret(id) {
            return false;
        }
        cx.emit(WorkspaceEvent::Toast {
            message: SECRET_IN_CARD.into(),
            undo: None,
        });
        cx.emit(WorkspaceEvent::RestoreQueued {
            thread: id.to_string(),
            text: text.to_string(),
            images: images.to_vec(),
        });
        true
    }

    /// `request_secret` from `call.thread`'s agent: its card goes up, and the reply waits for it.
    pub(super) fn secret_call(&mut self, call: Call, cx: &mut Context<Self>) {
        let text = |key: &str| {
            call.params
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_string()
        };
        let (name, why) = (text("name"), text("why"));
        self.ask_for_secret(&call.thread, &name, &why, call.reply, cx);
    }

    /// Put up thread `thread`'s card for secret `name`, with the agent's reason. `reply` hears
    /// the file's path, or that the user said no.
    pub fn ask_for_secret(
        &mut self,
        thread: &str,
        name: &str,
        why: &str,
        reply: async_channel::Sender<Reply>,
        cx: &mut Context<Self>,
    ) {
        if !session_secrets::valid_name(name) {
            let _ = reply.try_send(Reply::Done(Err(
                "name has to be a plain variable name, like GITHUB_TOKEN.".into(),
            )));
            return;
        }
        let why: String = why.split_whitespace().collect::<Vec<_>>().join(" ");
        let why: String = why.chars().take(MAX_WHY).collect();
        if why.is_empty() {
            let _ = reply.try_send(Reply::Done(Err(
                "why is required: say what it's for, so the user can decide.".into(),
            )));
            return;
        }
        if !self.live.contains_key(thread) {
            let _ = reply.try_send(Reply::Done(Err(
                "This thread isn't in Trek any more.".into()
            )));
            return;
        }
        static N: AtomicU64 = AtomicU64::new(0);
        let request_id = format!("{ASK_PREFIX}{}", N.fetch_add(1, Ordering::Relaxed) + 1);
        // Requests whose turn ended took their cards with them: their replies go too.
        {
            let open: Vec<String> = self
                .live
                .values()
                .flat_map(|l| l.permissions.iter().map(|p| p.request_id.clone()))
                .collect();
            let mut asks = ASKS.lock().unwrap();
            asks.retain(|id, _| open.contains(id));
            asks.insert(request_id.clone(), reply);
        }
        let Some(live) = self.live.get_mut(thread) else {
            return;
        };
        let prompt = Prompt::Secret {
            name: name.to_string(),
            why,
        };
        let title = format!("Secret: {name}");
        let reason = super::attention::Why::asking(&title, "", Some(&prompt));
        live.permissions.push(PendingPermission {
            request_id,
            title,
            detail: String::new(),
            prompt: Some(prompt),
            after_turn: false,
            call: None,
        });
        live.revision += 1;
        self.mutate_thread(thread, cx, |t| t.run_state = RunState::NeedsYou);
        let shown = self
            .thread(thread)
            .map(|t| t.title.as_str())
            .unwrap_or_default();
        let message = reason.alert(shown);
        cx.emit(WorkspaceEvent::Attention {
            message,
            thread: thread.to_string(),
            button: None,
        });
        cx.notify();
    }

    /// The card's value: into the private file, and the agent told where. The transcript says the
    /// name was given; the value stays out of it, and comes out of whatever the agent prints.
    pub(super) fn secret_answer(
        &mut self,
        id: &str,
        request_id: &str,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        let value = value.trim();
        let name = self.live.get(id).and_then(|l| {
            l.permissions
                .iter()
                .find(|p| p.request_id == request_id)
                .and_then(|p| match &p.prompt {
                    Some(Prompt::Secret { name, .. }) => Some(name.clone()),
                    _ => None,
                })
        });
        let Some(name) = name.filter(|_| !value.is_empty()) else {
            return;
        };
        let reply = ASKS.lock().unwrap().remove(request_id);
        let saved = session_secrets::save(id, &name, value);
        let Some(live) = self.live.get_mut(id) else {
            return;
        };
        live.settle_prompt(request_id);
        live.revision += 1;
        match saved {
            Ok(path) => {
                if let Some(tx) = reply {
                    let _ = tx.try_send(Reply::Done(Ok(
                        json!({ "text": saved_message(&name, &path) }),
                    )));
                }
                live.items.push(Item::Notice {
                    text: format!("{name} saved for this session. The agent has its file's path, not the value."),
                });
            }
            Err(e) => {
                if let Some(tx) = reply {
                    let _ = tx.try_send(Reply::Done(Err(format!(
                        "Trek couldn't save it ({}). Ask again, or carry on without it.",
                        e.kind()
                    ))));
                }
                cx.emit(WorkspaceEvent::Toast {
                    message: format!("Trek couldn't save {name}."),
                    undo: None,
                });
            }
        }
        let (waiting, running) = (!live.permissions.is_empty(), live.turn_started.is_some());
        if !waiting {
            let state = if running {
                RunState::Working
            } else {
                RunState::Idle
            };
            self.mutate_thread(id, cx, |t| t.run_state = state);
        }
        self.persist_items(id, cx);
        cx.notify();
    }
}

/// `ev` with any of `secrets`' values taken out of what it says: an agent that reads its file and
/// prints it doesn't get it into the transcript, the store, a recap or the phone.
pub(super) fn scrub_event(ev: &mut AgentEvent, secrets: &[(String, String)]) {
    use session_secrets::scrub_string as clean;
    match ev {
        AgentEvent::TextDelta(t)
        | AgentEvent::TextDone(t)
        | AgentEvent::ReasoningDelta(t)
        | AgentEvent::Notice(t)
        | AgentEvent::Error(t) => clean(t, secrets),
        AgentEvent::ToolStarted { title, detail, .. }
        | AgentEvent::TaskStep { title, detail, .. } => {
            clean(title, secrets);
            clean(detail, secrets);
        }
        AgentEvent::ToolFinished { output, .. } | AgentEvent::TaskOutput { output, .. } => {
            clean(output, secrets)
        }
        AgentEvent::PermissionRequest {
            title,
            detail,
            call,
            ..
        } => {
            clean(title, secrets);
            clean(detail, secrets);
            if let Some(call) = call {
                clean(&mut call.target, secrets);
            }
        }
        AgentEvent::Task {
            description,
            activity,
            ..
        } => {
            if let Some(d) = description {
                clean(d, secrets);
            }
            if let Some(a) = activity {
                clean(a, secrets);
            }
        }
        AgentEvent::Background(tasks) => {
            for task in tasks {
                clean(&mut task.title, secrets);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(prompt: Option<Prompt>, after_turn: bool) -> PendingPermission {
        PendingPermission {
            request_id: "r".into(),
            title: "t".into(),
            detail: String::new(),
            prompt,
            after_turn,
            call: None,
        }
    }

    #[test]
    fn a_typed_reply_only_declines_what_it_should() {
        assert_eq!(note_for(&card(None, false)), Some(ReplyNote::Declines));
        assert_eq!(
            note_for(&card(Some(Prompt::Plan("p".into())), false)),
            Some(ReplyNote::KeepsPlanning)
        );
        // A plan offered after its turn: the next message replaces it, as it did.
        assert_eq!(note_for(&card(Some(Prompt::Plan("p".into())), true)), None);
        // A question takes the words as its answer.
        assert_eq!(
            note_for(&card(Some(Prompt::Questions(vec![])), false)),
            None
        );
        assert_eq!(
            note_for(&card(
                Some(Prompt::Secret {
                    name: "T".into(),
                    why: "w".into()
                }),
                false
            )),
            Some(ReplyNote::SecretInCard)
        );
    }

    #[test]
    fn a_requests_plain_answer_is_only_ever_a_no() {
        let (tx, rx) = async_channel::bounded(1);
        ASKS.lock().unwrap().insert(format!("{ASK_PREFIX}t1"), tx);
        assert!(!answered("toolu_1", Decision::Allow), "an agent's own");
        assert!(answered(&format!("{ASK_PREFIX}t1"), Decision::Allow));
        let Ok(Reply::Done(Err(why))) = rx.try_recv() else {
            panic!("no refusal");
        };
        assert!(why.contains("declined"), "{why}");
    }

    #[test]
    fn what_the_agent_is_told_has_the_path_and_how_to_use_it() {
        let text = saved_message("TOKEN", std::path::Path::new("/x/TOKEN"));
        assert!(
            text.contains("/x/TOKEN") && text.contains("$(cat '/x/TOKEN')"),
            "{text}"
        );
    }

    #[test]
    fn a_printed_secret_comes_out_of_everything_an_event_says() {
        let secrets = vec![("T".to_string(), "s3cret-value".to_string())];
        let hidden = "[T hidden]";
        let mut events = vec![
            AgentEvent::TextDelta("a s3cret-value".into()),
            AgentEvent::TextDone("b s3cret-value".into()),
            AgentEvent::ReasoningDelta("c s3cret-value".into()),
            AgentEvent::Notice("d s3cret-value".into()),
            AgentEvent::Error("e s3cret-value".into()),
            AgentEvent::ToolStarted {
                id: "i".into(),
                title: "curl s3cret-value".into(),
                detail: "curl s3cret-value".into(),
            },
            AgentEvent::ToolFinished {
                id: "i".into(),
                output: "out s3cret-value".into(),
                ok: true,
            },
            AgentEvent::TaskOutput {
                id: "i".into(),
                output: "out s3cret-value".into(),
            },
            AgentEvent::TaskStep {
                task: "i".into(),
                title: "s3cret-value".into(),
                detail: "s3cret-value".into(),
            },
            AgentEvent::PermissionRequest {
                request_id: "r".into(),
                title: "s3cret-value".into(),
                detail: "s3cret-value".into(),
                prompt: None,
                call: Some(trek_core::permission_rules::Call::command(
                    "b",
                    "s3cret-value",
                )),
            },
            AgentEvent::Task {
                id: "i".into(),
                description: Some("s3cret-value".into()),
                activity: Some("s3cret-value".into()),
                tool_uses: None,
                done: None,
            },
        ];
        for ev in &mut events {
            scrub_event(ev, &secrets);
            let shown = format!("{ev:?}");
            assert!(!shown.contains("s3cret-value"), "{shown}");
            assert!(shown.contains(hidden), "{shown}");
        }
    }
}
