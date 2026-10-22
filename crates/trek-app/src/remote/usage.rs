//! Plan usage for the phone, as the Mac's Usage card shows it: the providers it shows (picked
//! there, three at most), each agent's plan and its 5-hour, weekly and per-model windows, and
//! Codex's granted resets, which the phone can spend. Asking refreshes them as opening the card
//! does (the agents are asked at most every 30 seconds, Devin every ten minutes).

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
                let busy = this
                    .read_with(cx, |ws, _| ws.usage_loading || ws.devin_loading)
                    .unwrap_or(false);
                if !busy {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            let usage = this
                .read_with(cx, |ws, _| ws.usage_now())
                .map_err(|_| tr::HostError::other("Trek is closing"));
            let _ = reply.send(usage);
        })
        .detach();
    }

    /// Usage as known now, as the Usage card shows it: its providers in its order (what was kept
    /// from an earlier run marked with when, today's tokens for those without limits), and the
    /// resets Codex granted.
    pub fn usage_now(&self) -> tr::Usage {
        let providers = self
            .usage_rows()
            .into_iter()
            .map(|u| {
                // As the card says it: only for a provider with no limits to show.
                let today = u.limits.is_empty().then(|| match &u.today {
                    Some((n, spend)) if spend.priced() => Some(format!(
                        "Today in Trek: {} tokens · ≈ {}",
                        crate::cost::fmt_tokens(*n),
                        crate::cost::usd(spend.usd())
                    )),
                    Some((n, _)) => Some(format!(
                        "Today in Trek: {} tokens",
                        crate::cost::fmt_tokens(*n)
                    )),
                    None if u.plan.is_none() && !u.no_limits && u.error.is_none() => {
                        Some("Nothing used today.".to_string())
                    }
                    None => None,
                });
                tr::ProviderUsage {
                    agent: agent_ref(&u.agent),
                    plan: u.plan,
                    limits: u
                        .limits
                        .iter()
                        .map(|l| tr::UsageLimit {
                            label: l.label.clone(),
                            percent: l.percent,
                            resets_at: l.resets_at,
                            window: l.window.clone(),
                        })
                        .collect(),
                    note: u.note,
                    error: u.error,
                    no_limits: u.no_limits,
                    as_of: u.as_of,
                    today: today.flatten(),
                    resets: u
                        .resets
                        .into_iter()
                        .map(|r| tr::ResetCredit {
                            id: r.id,
                            title: r.title,
                            description: r.description,
                            expires_at: r.expires_at,
                        })
                        .collect(),
                }
            })
            .collect();
        tr::Usage {
            providers,
            loading: self.usage_loading || self.devin_loading,
            picked: Some(self.usage_picked()),
        }
    }

    /// "Use reset" from the phone (it asks first, as the Mac's dialog does): answered once the
    /// reset is spent, or not.
    pub(super) fn remote_use_reset(
        &mut self,
        credit_id: String,
        reply: tr::Reply<()>,
        cx: &mut Context<Self>,
    ) {
        if !self
            .agent_status
            .values()
            .any(|st| st.resets.iter().any(|r| r.id == credit_id))
        {
            let _ = reply.send(Err(tr::HostError::not_found(
                "That reset isn't available any more",
            )));
            return;
        }
        self.use_reset_credit_then(
            credit_id,
            move |done| {
                let _ = reply.send(done.map_err(tr::HostError::other));
            },
            cx,
        );
    }
}
