//! A side chat from the phone (`side_chat`): the Side Chat panel's quick question beside a
//! thread, as a sheet over the thread there. A side chat is a thread with `side_of` set, which no
//! list holds: the phone finds a thread's newest one (or starts one with its first message) and
//! gets its row; from there it's a thread the phone has open like any other, its row sent
//! `unlisted` while it's watched (`sub_agents::remote_unlisted`). Starting one leaves the Mac's
//! window and the panel's tabs alone: it's in the panel's history.

use super::save_uploads;
use crate::workspace::Workspace;
use gpui_kit::Context;
use trek_core::store::now_ms;
use trek_remote as tr;

impl Workspace {
    pub(super) fn remote_side_chat(
        &mut self,
        req: tr::SideChatRequest,
        cx: &mut Context<Self>,
    ) -> tr::HostResult<tr::SideChat> {
        let Some(t) = self.thread(&req.thread_id).cloned() else {
            return Err(tr::HostError::not_found("No such thread"));
        };
        if t.side_of.is_some() {
            return Err(tr::HostError::bad_request(
                "A side chat has no side chat of its own",
            ));
        }
        let Some(text) = req.text else {
            let now = now_ms();
            let side = self
                .side_chat_of(&t.id)
                .map(|side| self.remote_summary(side, now));
            return Ok(tr::SideChat {
                thread_id: t.id,
                side,
            });
        };
        let text = text.trim().to_string();
        if text.is_empty() && req.images.is_empty() {
            return Err(tr::HostError::bad_request(
                "Say what to ask in the side chat",
            ));
        }
        if !self.ready_agents().contains(&t.agent) {
            return Err(tr::HostError::not_found(format!(
                "{} isn't set up on {}",
                t.agent.display_name(),
                crate::words::words().this_computer
            )));
        }
        let images = save_uploads(&req.images)?;
        let id = self
            .start_side_chat_beside(&t.id, text, images, cx)
            .ok_or_else(|| tr::HostError::other("The side chat didn't start"))?;
        let side = self
            .thread(&id)
            .map(|side| self.remote_summary(side, now_ms()));
        Ok(tr::SideChat {
            thread_id: t.id,
            side,
        })
    }
}
