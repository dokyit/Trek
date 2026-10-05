//! Basecamp for the phone: the recap the Mac's Basecamp shows, worded the same, with the numbers
//! behind it so the phone can draw the profile and the tiles. Worked out off the main thread, as
//! the Mac's is.

use super::agent_ref;
use crate::basecamp::{self as screen, TileKind, Waiting};
use crate::workspace::Workspace;
use gpui_kit::Context;
use trek_core::RunState;
use trek_core::basecamp::{self, Range, Recap, Span};
use trek_core::store::now_ms;
use trek_remote as tr;

impl Workspace {
    pub(super) fn remote_basecamp(&mut self, range: tr::BasecampRange, reply: tr::Reply<tr::Basecamp>, cx: &mut Context<Self>) {
        let range = match range {
            tr::BasecampRange::Today => Range::Today,
            tr::BasecampRange::Week => Range::Week,
            tr::BasecampRange::All => Range::All,
        };
        // The plan tiles want the agents' limits, asked for as Basecamp asks on opening.
        if self.agent_status.is_empty() && !self.usage_loading && !self.detecting {
            self.refresh_usage(cx);
            self.refresh_devin_usage(cx);
        }
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let now = chrono::Local::now();
            basecamp::recap(&store, range, &now).unwrap_or_else(|e| {
                tracing::warn!("basecamp: {e:#}");
                Recap::compute(range.window(&now), now.timestamp_millis(), &[])
            })
        });
        cx.spawn(async move |this, cx| {
            let recap = work.await;
            let out = this.read_with(cx, |ws, _| ws.basecamp_for_phone(range, &recap)).map_err(|_| tr::HostError::other("Trek is closing"));
            let _ = reply.send(out);
        })
        .detach();
    }

    fn basecamp_for_phone(&self, range: Range, recap: &Recap) -> tr::Basecamp {
        let project_ref = |id: &str| self.project(id).map(|p| tr::ProjectRef { id: p.id.clone(), name: p.name.clone(), hue: tr::project_hue(&p.name, self.project_prefs(&p.path).color), monogram: tr::monogram(&p.name) });
        let by_name = |name: &str| self.projects.iter().find(|p| p.name == name).and_then(|p| project_ref(&p.id));
        let empty = recap.is_empty();
        tr::Basecamp {
            range: match range {
                Range::Today => tr::BasecampRange::Today,
                Range::Week => tr::BasecampRange::Week,
                Range::All => tr::BasecampRange::All,
            },
            greeting: screen::greeting_line(range, Some(recap)),
            title: screen::trek_title(range).to_string(),
            updated_at: recap.now,
            review: self.review_for_phone(),
            empty,
            invitation: empty.then(|| screen::invitation(range)),
            narrative: recap
                .narrative(basecamp::model_label)
                .into_iter()
                .map(|s| match s {
                    Span::Text(text) => tr::Span::Text { text },
                    Span::Strong(text) => tr::Span::Strong { text },
                    Span::Project(name) => tr::Span::Project { project: by_name(&name), text: name },
                    Span::Model { agent, label } => tr::Span::Model { text: label, agent: agent_ref(&agent) },
                })
                .collect(),
            summary: (!empty).then(|| tr::RecapSummary {
                prompts: recap.prompts as u32,
                threads: recap.threads as u32,
                turns: recap.turns as u32,
                agent_secs: recap.agent_secs,
                agent_time: basecamp::duration(recap.agent_secs),
                tokens: recap.tokens.total(),
                failed: recap.failed as u32,
                top_project: recap.projects.first().and_then(|p| Some(tr::ProjectShare { project: project_ref(&p.id).or_else(|| by_name(&p.name))?, prompts: p.prompts as u32, tokens: p.tokens })),
                best_model: recap.best_model().map(|m| tr::ModelShare {
                    agent: agent_ref(&m.agent),
                    label: basecamp::model_label(&m.agent, m.model.as_deref()),
                    tokens: m.tokens,
                    turns: m.turns as u32,
                    share: recap.token_share(m),
                }),
            }),
            profile: Some(profile(recap)),
            tiles: if empty { vec![] } else { self.tiles_for_phone(recap, project_ref) },
        }
    }

    /// "Ready for review", as Basecamp lists it.
    fn review_for_phone(&self) -> Vec<tr::ReviewRow> {
        let now = self.now();
        let sub_agents = self.waiting_on_sub_agents();
        self.ready_for_review()
            .into_iter()
            .map(|t| {
                let waiting = match t.run_state {
                    RunState::NeedsYou => Some(Waiting::of(self.pending_request(&t.id))),
                    _ if sub_agents.contains(&t.id) => Some(Waiting::SubAgent),
                    _ => None,
                };
                let (status, label) = match (t.run_state, waiting) {
                    (RunState::NeedsYou, w) | (_, w @ Some(Waiting::SubAgent)) => (tr::ReviewStatus::NeedsYou, Some(w.unwrap_or(Waiting::Unknown).label().to_string())),
                    (RunState::Failed, _) => (tr::ReviewStatus::Failed, Some("Failed".to_string())),
                    (RunState::Idle, _) if t.paused.is_some() => {
                        let until = match t.paused.as_ref().and_then(|p| p.resets_at) {
                            Some(at) => format!("Paused until {}", crate::time::reset_clock(at, now)),
                            None => "Paused at its limit".to_string(),
                        };
                        (tr::ReviewStatus::Paused, Some(until))
                    }
                    _ => (tr::ReviewStatus::Done, None),
                };
                tr::ReviewRow {
                    thread_id: t.id.clone(),
                    title: t.title.clone(),
                    status,
                    label,
                    agent: agent_ref(&t.agent),
                    project: t.project_id.as_deref().and_then(|p| self.project(p)).map(|p| tr::ProjectRef {
                        id: p.id.clone(),
                        name: p.name.clone(),
                        hue: tr::project_hue(&p.name, self.project_prefs(&p.path).color),
                        monogram: tr::monogram(&p.name),
                    }),
                    additions: t.additions.max(0) as u32,
                    deletions: t.deletions.max(0) as u32,
                    updated_at: t.updated_at,
                    unseen: t.is_unseen(),
                }
            })
            .collect()
    }

    fn tiles_for_phone(&self, recap: &Recap, project_ref: impl Fn(&str) -> Option<tr::ProjectRef>) -> Vec<tr::Tile> {
        screen::tile_data(recap, self)
            .into_iter()
            .map(|t| tr::Tile {
                kind: match t.kind {
                    TileKind::BestModel => tr::TileKind::BestModel,
                    TileKind::WorkedMostOn => tr::TileKind::WorkedMostOn,
                    TileKind::Tokens => tr::TileKind::Tokens,
                    TileKind::AgentTime => tr::TileKind::AgentTime,
                    TileKind::PlanLeft => tr::TileKind::PlanLeft,
                },
                sparkline: if t.kind == TileKind::Tokens { sparkline(recap) } else { vec![] },
                label: t.label,
                figure: t.figure,
                note: t.note,
                agent: t.agent.as_ref().map(agent_ref),
                project: t.project.and_then(|(id, _)| project_ref(&id)),
                percent: t.left,
                resets_at: t.resets_at,
            })
            .collect()
    }
}

/// The elevation profile: each stretch's height and words, the summit, "now" and the axis.
fn profile(recap: &Recap) -> tr::Profile {
    let w = &recap.window;
    let span = (w.end - w.start).max(1) as f32;
    tr::Profile {
        buckets: recap
            .buckets
            .iter()
            .enumerate()
            .map(|(i, b)| tr::ProfileBucket {
                value: recap.elevation(i),
                label: screen::stretch_label(recap, i),
                line: screen::profile_line(recap, Some(i)),
                prompts: b.prompts as u32,
                agent_secs: b.agent_secs,
                tokens: b.tokens,
            })
            .collect(),
        summit: recap.peak(),
        now: recap.now_bucket(),
        now_at: ((recap.now.min(now_ms()) - w.start) as f32 / span).clamp(0., 1.),
        line: screen::profile_line(recap, None),
        total: basecamp::count(recap.prompts, "prompt", "prompts"),
        ticks: screen::ticks(w).into_iter().map(|(at, label)| tr::Tick { at, label }).collect(),
    }
}

/// Tokens used so far through the range, 0–1, a point a stretch (the tokens tile's line).
fn sparkline(recap: &Recap) -> Vec<f32> {
    let total: u64 = recap.buckets.iter().map(|b| b.tokens).sum();
    let mut sum = 0;
    recap
        .buckets
        .iter()
        .map(|b| {
            sum += b.tokens;
            if total > 0 { sum as f32 / total as f32 } else { 0. }
        })
        .collect()
}
