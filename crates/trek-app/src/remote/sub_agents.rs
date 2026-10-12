//! Sub-agents on the phone. Trek's sub-agents run in threads of their own, which the list leaves
//! out (on the Mac they show only under their parent): phones get them as `unlisted` rows, so one
//! can be opened from its parent to read it, answer it or stop it. Threads a phone has open that
//! the list leaves out (found by a search) come the same way. A parent whose sub-agent waits on
//! the user says so in its own row, as its sidebar card does, and is in the inbox for it.

use super::Remote;
use crate::workspace::Workspace;
use std::collections::{HashMap, HashSet};
use trek_core::RunState;
use trek_core::store::Thread;
use trek_remote as tr;

impl Workspace {
    /// The rows phones get besides `listed`: every sub-agent under a listed thread (theirs too),
    /// then the threads phones have open that neither list holds.
    pub(super) fn remote_unlisted(&self, listed: &[tr::ThreadSummary], now: i64) -> Vec<tr::ThreadSummary> {
        let mut kids: HashMap<&str, Vec<&Thread>> = HashMap::new();
        for t in self.threads.iter().filter(|t| t.own_section(now).is_some()) {
            if let Some(parent) = t.parent_id.as_deref() {
                kids.entry(parent).or_default().push(t);
            }
        }
        let listed: HashSet<&str> = listed.iter().map(|r| r.id.as_str()).collect();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut out = vec![];
        let mut queue: Vec<&str> = listed.iter().copied().collect();
        while let Some(parent) = queue.pop() {
            for kid in kids.get(parent).into_iter().flatten() {
                if !listed.contains(kid.id.as_str()) && seen.insert(&kid.id) {
                    out.push(self.remote_summary(kid, now));
                    queue.push(&kid.id);
                }
            }
        }
        let open: Vec<&String> = self.remote.as_ref().map(|r| r.watched.keys().collect()).unwrap_or_default();
        for id in open {
            if listed.contains(id.as_str()) || seen.contains(id.as_str()) {
                continue;
            }
            if let Some(t) = self.thread(id).filter(|t| t.own_section(now).is_some()) {
                out.push(self.remote_summary(t, now));
            }
        }
        out
    }

    /// A sub-agent under `t` (at any depth) that waits on the user, when `t` itself doesn't: its
    /// request is the row's `needs`, pointing at its thread, and `t` is in the inbox for it (as a
    /// snoozed thread that needs the user is).
    pub(super) fn remote_sub_agent_needs(&self, t: &Thread, row: &mut tr::ThreadSummary) {
        if row.needs.is_some() {
            return;
        }
        let Some(kid) = self.waiting_sub_agent(&t.id) else { return };
        let kind = match self.live.get(&kid.id).and_then(|l| l.permissions.first()).map(|p| &p.prompt) {
            Some(Some(trek_agents::Prompt::Questions(_))) => tr::NeedsKind::Question,
            Some(Some(trek_agents::Prompt::Plan(_))) => tr::NeedsKind::Plan,
            _ => tr::NeedsKind::Approval,
        };
        row.needs = Some(tr::Needs { kind, text: "Sub-agent needs you".into() });
        row.needs_thread = Some(kid.id.clone());
        if matches!(row.section, tr::Section::Working | tr::Section::Snoozed) {
            row.section = tr::Section::Inbox;
        }
    }

    /// The first sub-agent under `id`, nearest first, that waits on the user.
    fn waiting_sub_agent(&self, id: &str) -> Option<&Thread> {
        let mut queue = std::collections::VecDeque::from([id.to_string()]);
        let mut seen = HashSet::new();
        while let Some(at) = queue.pop_front() {
            for kid in self.children(&at) {
                if kid.run_state == RunState::NeedsYou {
                    return Some(kid);
                }
                if seen.insert(kid.id.clone()) && seen.len() < 1000 {
                    queue.push_back(kid.id.clone());
                }
            }
        }
        None
    }
}

/// Send the unlisted rows that changed. A row that's no longer among them is forgotten (it's
/// listed now, or no phone has it open), or, when its thread is gone (`existing` doesn't hold it),
/// removed from phones.
pub(super) fn send_unlisted(remote: &mut Remote, rows: Vec<tr::ThreadSummary>, existing: &HashSet<String>) {
    let ids: HashSet<String> = rows.iter().map(|r| r.id.clone()).collect();
    let gone: Vec<String> = remote.unlisted.keys().filter(|id| !ids.contains(*id)).cloned().collect();
    for id in gone {
        remote.unlisted.remove(&id);
        if !existing.contains(&id) && !remote.sent.contains_key(&id) {
            remote.watched.remove(&id);
            remote.dormant.remove(&id);
            remote.handle.push(tr::HostEvent::ThreadRemoved(id));
        }
    }
    for row in rows {
        if remote.unlisted.get(&row.id) != Some(&row) {
            remote.unlisted.insert(row.id.clone(), row.clone());
            remote.handle.push(tr::HostEvent::Unlisted(row));
        }
    }
}
