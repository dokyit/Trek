//! Moving a chat to another agent, with no surprise. Picking another agent's model in the
//! composer only holds the pick (`PendingSwitch`): the banner says so, the old agent keeps its
//! session, its running turn and its queue, and "Stay on" drops it. The move happens as the next
//! message goes (`commit_switch_in`), and that message carries a handover
//! (`trek_core::handover`) the user can read first.
//!
//! Another model of the same agent is no move: it goes straight through `set_prefs_in`, in place,
//! as it always has. So do moves with nothing to carry (a thread nothing has been said in), and
//! those from elsewhere (the phone's own picker, a thread on another machine).

use super::{Prefs, Scope, Workspace};
use gpui_kit::Context;
use std::collections::BTreeMap;
use trek_core::handover::{self, Changed, Checkout, Facts, Handover};
use trek_core::store::{Item, Thread};
use trek_core::{AgentId, Effort};

/// The agent a thread moves to when its next message goes, and the account it runs on there
/// (`None`: the user's normal login).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSwitch {
    pub agent: AgentId,
    pub model: Option<String>,
    pub effort: Effort,
    pub account: Option<String>,
}

/// `items` end in a move to another agent, with nothing asked of the new one yet: the session
/// about to start is the one a handover is for.
fn just_moved(items: &[Item]) -> bool {
    let moved = items
        .iter()
        .rposition(|i| matches!(i, Item::Handoff { .. }));
    let asked = items
        .iter()
        .rposition(|i| matches!(i, Item::User { aside: false, .. }));
    moved.is_some_and(|m| asked.is_none_or(|a| a < m))
}

impl Workspace {
    /// `prefs_in`, with the switch waiting for the next message in place of the thread's agent,
    /// model, effort and account: what the composer's model pill shows and its menus act on.
    pub fn chosen_prefs_in(&self, scope: &Scope) -> Prefs {
        let mut prefs = self.prefs_in(scope);
        if let Some(p) = self
            .thread_id_in(scope)
            .and_then(|id| self.pending_switch(id))
        {
            prefs.agent = p.agent.clone();
            prefs.model = p.model.clone();
            prefs.effort = p.effort;
            prefs.account = p.account.clone();
        }
        prefs
    }

    /// The switch `id` waits to make with its next message.
    pub fn pending_switch(&self, id: &str) -> Option<&PendingSwitch> {
        self.live.get(id).and_then(|l| l.pending_switch.as_ref())
    }

    /// The composer's way of changing `scope`'s prefs. Another agent (or another account of it),
    /// in a thread with something to carry over, waits for the next message: either would start
    /// a new session, ending a turn under way. Everything else applies now, as `set_prefs_in`
    /// does. The rest of `prefs` (access, plan, fast) belongs to the thread and applies now too.
    pub fn choose_prefs_in(&mut self, scope: &Scope, prefs: Prefs, cx: &mut Context<Self>) {
        // What waits is kept across a restart with the unsent messages (`unsent`).
        self.switch_changed();
        let prefs = Prefs {
            account: self.account_on(&prefs.agent, prefs.account.clone()),
            ..prefs
        };
        let held = self
            .thread_id_in(scope)
            .filter(|id| !crate::machines::is_remote(id))
            .map(str::to_string)
            .filter(|id| {
                let moves = self
                    .thread(id)
                    .is_some_and(|t| t.agent != prefs.agent || t.account != prefs.account);
                moves && self.has_conversation(id)
            });
        let Some(id) = held else {
            // Back on its own agent (or nothing was waiting): the pick is simply the thread's.
            if let Some(live) = self
                .thread_id_in(scope)
                .map(str::to_string)
                .and_then(|id| self.live.get_mut(&id))
            {
                live.pending_switch = None;
            }
            return self.set_prefs_in(scope, prefs, cx);
        };
        let now = self.prefs_in(scope);
        let thread_settings = Prefs {
            hand_holding: prefs.hand_holding,
            plan: prefs.plan,
            fast: prefs.fast,
            worktree: prefs.worktree,
            ..now.clone()
        };
        self.live.entry(id).or_default().pending_switch = Some(PendingSwitch {
            agent: prefs.agent,
            model: prefs.model,
            effort: prefs.effort,
            account: prefs.account,
        });
        if thread_settings != now {
            self.set_prefs_in(scope, thread_settings, cx);
        }
        cx.notify();
    }

    /// "Stay on": the pick goes, and nothing has changed.
    pub fn stay_on_agent(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(live) = self.live.get_mut(id) {
            live.pending_switch = None;
        }
        self.switch_changed();
        cx.notify();
    }

    /// A message is going from `scope`'s composer: the switch waiting for it happens now, unless
    /// the message isn't one for the new agent. A command Trek answers itself the agent never
    /// sees; `/compact` compacts the session there is; an answer to the old agent's question is
    /// its; and with a secret's card up the message won't go at all (it may be the secret, and the
    /// move would take the card down with the old agent's session). The switch waits on.
    pub fn commit_switch_in(&mut self, scope: &Scope, text: &str, cx: &mut Context<Self>) {
        let Some(id) = self.thread_id_in(scope).map(str::to_string) else {
            return;
        };
        if is_trek_command(text)
            || is_compact(text)
            || self.asks_for_secret(&id)
            || self.waits_on_question(&id)
        {
            return;
        }
        let Some(pending) = self.live.get_mut(&id).and_then(|l| l.pending_switch.take()) else {
            return;
        };
        self.switch_changed();
        let prefs = Prefs {
            agent: pending.agent,
            model: pending.model,
            effort: pending.effort,
            account: pending.account,
            ..self.prefs_in(scope)
        };
        self.set_prefs_in(scope, prefs, cx);
    }

    /// `id`'s agent waits on a question of its own, which the next message answers (`send_as`).
    pub(super) fn waits_on_question(&self, id: &str) -> bool {
        self.live
            .get(id)
            .and_then(|l| l.permissions.first())
            .is_some_and(|p| matches!(p.prompt, Some(trek_agents::Prompt::Questions(_))))
    }

    /// Something was said in `id`, so there's a conversation to carry over.
    fn has_conversation(&self, id: &str) -> bool {
        self.live.get(id).is_some_and(|l| {
            l.items
                .iter()
                .any(|i| matches!(i, Item::User { aside: false, .. }))
        })
    }

    /// What `id`'s next message would carry as things stand, for the banner's preview.
    pub fn pending_handover(&self, id: &str) -> Option<Handover> {
        let pending = self.pending_switch(id)?;
        let thread = self.thread(id)?;
        let from = self.switch_name(&thread.agent, thread.model.clone());
        let searchable = self.store.transcript_log(id).is_some();
        self.handover(
            id,
            thread,
            &from,
            &pending.agent,
            pending.model.as_deref(),
            searchable,
        )
    }

    /// What a session that starts now gets instead of a recap: the handover, when its thread has
    /// just moved here. `searchable`: it can search the transcript.
    pub(super) fn handover_for_session(
        &self,
        id: &str,
        thread: &Thread,
        searchable: bool,
    ) -> Option<String> {
        let live = self.live.get(id)?;
        if !just_moved(&live.items) {
            return None;
        }
        let from = live.items.iter().rev().find_map(|i| match i {
            Item::Handoff { from_name, .. } => from_name.clone(),
            _ => None,
        })?;
        self.handover(
            id,
            thread,
            &from,
            &thread.agent,
            thread.model.as_deref(),
            searchable,
        )
        .map(|h| h.text)
        .filter(|t| !t.is_empty())
    }

    /// A handover of `id`'s conversation from `from` to `to`'s `model`.
    fn handover(
        &self,
        id: &str,
        thread: &Thread,
        from: &str,
        to: &AgentId,
        model: Option<&str>,
        searchable: bool,
    ) -> Option<Handover> {
        let live = self.live.get(id)?;
        let changes = self.changes_so_far(id);
        let checkout = thread
            .cwd
            .as_ref()
            .and_then(|c| self.git_info.get(c))
            .filter(|g| g.is_repo)
            .map(|g| Checkout {
                branch: g.branch.clone(),
                changed: g.changed,
                ahead: g.ahead,
            });
        let models = self.models_for(to);
        let window = model
            .and_then(|m| {
                models
                    .iter()
                    .find(|i| crate::composer::same_model(m, &i.id))
            })
            .or_else(|| crate::composer::default_model(&models))
            .and_then(|m| m.context);
        Some(handover::build(
            &live.items,
            &Facts {
                from,
                window,
                changes: &changes,
                checkout,
                searchable,
            },
            crate::share::render::redact_secrets,
        ))
    }

    /// Every file the conversation changed so far, added up over its turns (sorted by path): as
    /// each turn's card counted it where it has been counted, else from the agent's edit tools.
    fn changes_so_far(&self, id: &str) -> Vec<Changed> {
        let Some(live) = self.live.get(id) else {
            return vec![];
        };
        let mut ends: Vec<usize> = live
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| trek_core::rewind::ends_turn(i))
            .map(|(ix, _)| ix)
            .collect();
        // What came after the last turn's end: the one running, or cut off.
        ends.push(live.items.len());
        let mut files: BTreeMap<String, Changed> = BTreeMap::new();
        for end in ends {
            let Some(turn) = self
                .turn_changes(id, end)
                .or_else(|| self.from_tools(id, end))
            else {
                continue;
            };
            for f in turn.files {
                let known = f.lines_known;
                let c = files.entry(f.path.clone()).or_insert_with(|| Changed {
                    path: f.path,
                    added: 0,
                    removed: 0,
                    lines_known: true,
                });
                c.added += f.added;
                c.removed += f.removed;
                c.lines_known &= known;
            }
        }
        files.into_values().collect()
    }

    /// How the transcript's handoff row would name `agent` on `model`.
    fn switch_name(&self, agent: &AgentId, model: Option<String>) -> String {
        let models = self.models_for(agent);
        let model = model.or_else(|| crate::composer::default_model(&models).map(|m| m.id.clone()));
        crate::thread_view::handoff_name(
            agent,
            model
                .as_deref()
                .map(|m| crate::composer::model_name(&models, m))
                .as_deref(),
        )
    }
}

/// `text` is a command Trek answers itself, whatever agent the thread is on.
fn is_trek_command(text: &str) -> bool {
    let Some(rest) = text.trim_start().strip_prefix('/') else {
        return false;
    };
    let Some(cmd) = rest.split_whitespace().next() else {
        return false;
    };
    cmd == "clear" || super::BUILTIN_COMMANDS.iter().any(|(n, _)| *n == cmd)
}

/// `text` is `/compact` (with what to keep, maybe): it compacts the session the thread has, which
/// a move to another agent would end first.
fn is_compact(text: &str) -> bool {
    text.trim_start()
        .strip_prefix("/compact")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> Item {
        Item::User {
            text: "a".into(),
            images: vec![],
            at: None,
            resume: None,
            aside: false,
        }
    }

    fn moved() -> Item {
        Item::Handoff {
            from: "a".into(),
            from_model: None,
            to: "b".into(),
            to_model: None,
            from_name: None,
            to_name: None,
        }
    }

    #[test]
    fn only_a_move_nothing_was_asked_after_gets_a_handover() {
        assert!(just_moved(&[user(), moved()]));
        assert!(!just_moved(&[user(), moved(), user()]));
        assert!(!just_moved(&[user()]));
        assert!(!just_moved(&[]));
    }

    #[test]
    fn trek_answers_its_own_commands() {
        assert!(is_trek_command("/usage"));
        assert!(is_trek_command("  /cost now"));
        assert!(is_trek_command("/clear"));
        assert!(!is_trek_command("/compact"), "that's the agent's");
        assert!(!is_trek_command("fix /usage"));
        assert!(!is_trek_command("/"));
    }
}
