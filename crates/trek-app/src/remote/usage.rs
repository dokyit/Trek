//! Plan usage for the phone, as the Mac's Usage popover shows it: each agent's plan and its
//! 5-hour, weekly and per-model windows. Asking refreshes them as opening the popover does (the
//! agents are asked at most every 30 seconds, Devin every ten minutes).

use super::agent_ref;
use crate::workspace::Workspace;
use gpui_kit::Context;
use std::time::Duration;
use trek_remote as tr;

/// How long a phone waits for the agents to answer before it gets what's known.
const WAIT: Duration = Duration::from_secs(15);

impl Workspace {
    pub(super) fn remote_usage(&mut self, reply: tr::Reply<tr::Usage>, cx: &mut Context<Self>) {
        self.refresh_usage(cx);
        self.refresh_devin_usage(cx);
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            while started.elapsed() < WAIT {
                let busy = this.read_with(cx, |ws, _| ws.usage_loading || ws.devin_loading).unwrap_or(false);
                if !busy {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(100)).await;
            }
            let usage = this.read_with(cx, |ws, _| ws.usage_now()).map_err(|_| tr::HostError::other("Trek is closing"));
            let _ = reply.send(usage);
        })
        .detach();
    }

    /// Usage as known now, in the agent picker's order.
    pub(crate) fn usage_now(&self) -> tr::Usage {
        let providers = self
            .ready_agents()
            .into_iter()
            .filter_map(|agent| {
                let st = self.agent_status.get(&agent.key())?;
                Some(tr::ProviderUsage {
                    agent: agent_ref(&agent),
                    plan: st.plan.clone(),
                    limits: st.limits.iter().map(|l| tr::UsageLimit { label: l.label.clone(), percent: l.percent, resets_at: l.resets_at, window: l.window.clone() }).collect(),
                    note: st.note.clone(),
                    error: st.error.clone(),
                })
            })
            .collect();
        tr::Usage { providers, loading: self.usage_loading || self.devin_loading }
    }
}
