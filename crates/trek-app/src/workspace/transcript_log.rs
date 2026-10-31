//! The transcript tools every agent gets (`transcript_search`, `transcript_read`): Trek keeps
//! each thread's transcript as a plain-text log (`trek_core::session_log`, written as the store
//! saves), and an agent searches it for exact earlier content after a compaction, a recap or a
//! handoff. A call comes over Trek's local channel like the sub-agent tools', for the caller's
//! own thread or another of its project's (the one a handoff came from, say); the log is
//! brought up to date first, then read off the main thread.

use super::Workspace;
use crate::ipc::{Call, Reply};
use gpui_kit::{AppContext as _, Context};
use serde_json::Value;
use std::path::PathBuf;
use trek_core::session_log;
use trek_core::store::Thread;

impl Workspace {
    /// `id`'s transcript log, brought up to date: its unsaved items saved first, and with
    /// nothing in progress there, its last message and running calls written as they stand.
    /// Returns its path and line count; `None` without a log (an in-memory store).
    pub(super) fn transcript_log(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Option<(PathBuf, u64)> {
        let live = self.live.get(id).filter(|l| l.loaded && !l.loading);
        let synced = match live {
            Some(_) => {
                self.persist_items(id, cx);
                let live = self.live.get(id)?;
                let at_work = live.turn_started.is_some();
                self.store.sync_log(id, Some(&live.items), !at_work)
            }
            None => self.store.sync_log(id, None, true),
        };
        match synced {
            Ok(log) => log,
            Err(e) => {
                tracing::warn!("transcript log for {id}: {e}");
                self.store.transcript_log(id)
            }
        }
    }

    /// Whether `caller`'s agent may read `other`'s transcript: a thread of the same project, or
    /// one linked to it (the thread it continues or was started by, its parent or side chat).
    fn may_read_transcript(caller: &Thread, other: &Thread) -> bool {
        let linked = |a: &Thread, b: &Thread| {
            [&a.continued_from, &a.started_by, &a.parent_id, &a.side_of]
                .iter()
                .any(|l| l.as_deref() == Some(b.id.as_str()))
        };
        (caller.project_id.is_some() && caller.project_id == other.project_id)
            || linked(caller, other)
            || linked(other, caller)
    }

    /// A `transcript_search` or `transcript_read` from `call.thread`'s agent.
    pub(super) fn transcript_call(&mut self, call: Call, cx: &mut Context<Self>) {
        let reply = call.reply.clone();
        let fail = |e: String| {
            let _ = reply.try_send(Reply::Done(Err(e)));
        };
        let Some(caller) = self.thread(&call.thread).cloned() else {
            return fail("This thread isn't in Trek any more.".into());
        };
        let target = call
            .params
            .get("thread")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty() && *t != caller.id)
            .map(str::to_string);
        if let Some(other) = &target {
            let found = self
                .thread(other)
                .cloned()
                .or_else(|| self.store.thread(other).ok().flatten());
            match found {
                None => return fail(format!("Trek has no thread {other}.")),
                Some(t) if !Self::may_read_transcript(&caller, &t) => {
                    return fail(
                        "That thread is in another project: only this project's threads (and ones linked to this one) can be read.".into(),
                    );
                }
                Some(_) => {}
            }
        }
        let id = target.unwrap_or_else(|| caller.id.clone());
        let Some((path, _)) = self.transcript_log(&id, cx) else {
            return fail("This Trek keeps no transcript logs (its database is in memory).".into());
        };
        let (method, params) = (call.method.clone(), call.params.clone());
        // The search reads the whole log: off the main thread.
        cx.background_spawn(async move {
            let answer = match method.as_str() {
                "transcript_search" => session_log::Query::from_args(&params)
                    .and_then(|q| session_log::search(&path, &q)),
                _ => {
                    let n = |k: &str| params.get(k).and_then(Value::as_u64).map(|v| v as usize);
                    session_log::read(&path, n("start").unwrap_or(1), n("end"))
                }
            };
            let _ = reply.try_send(Reply::Done(answer.map(Value::String)));
        })
        .detach();
    }
}
