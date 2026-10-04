//! Agent CLI updates in the app: what the last check found (`trek_core::agent_update`), the
//! updates asked for and how each went, and the queue that holds an update back while one of
//! that agent's threads is mid-turn (replacing a CLI under a running session can break it).
//! `Workspace` runs the checks and updates (`workspace/agent_updates.rs`) and reports back here.
//!
//! Shown as a quiet card from the sidebar's footer (a pill with the count, only while there's
//! something to install) and on Settings > Agents: one row per agent with installed → latest and
//! an Update button, MonoCode's harness updates in Trek's design.

use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use trek_core::AgentId;
use trek_core::agent_update::{AgentVersion, CHECK_EVERY_MS, Outcome};

/// Where an update asked for stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    /// Waiting its turn: behind another update, or for the agent's running turns to end.
    Queued,
    Running,
    /// `from`: the version it was at.
    Updated { from: Option<String>, to: String, output: String },
    Failed { summary: String, output: String },
}

/// What runs an update.
#[derive(Clone, Default)]
pub enum Runner {
    /// The agent's real update command.
    #[default]
    Real,
    /// Pretends (`TREK_AGENT_UPDATES=dry-run` or `mock`): waits a moment and changes nothing.
    DryRun,
    /// Tests decide the outcome.
    #[cfg(test)]
    Fake(std::sync::Arc<dyn Fn(&AgentVersion) -> Outcome + Send + Sync>),
}

impl Runner {
    pub async fn run(&self, v: AgentVersion) -> Outcome {
        match self {
            Runner::Real => trek_core::agent_update::run_update(&v).await,
            Runner::DryRun => trek_core::agent_update::dry_run(&v).await,
            #[cfg(test)]
            Runner::Fake(f) => f(&v),
        }
    }
}

#[derive(Default)]
pub struct AgentUpdates {
    /// Every agent CLI on this Mac, as the last check found it.
    pub found: Vec<AgentVersion>,
    /// When that check finished (unix ms); 0 = never.
    pub checked_at: i64,
    pub checking: bool,
    /// Updates asked for, by agent key, and the order they were asked in.
    jobs: HashMap<String, Job>,
    order: Vec<String>,
    pub runner: Runner,
    /// Showing made-up agents (`TREK_AGENT_UPDATES=mock`): nothing is checked for real.
    pub mock: bool,
    /// The agent whose update output is open under its row.
    pub output_open: Option<String>,
}

/// The ways `TREK_AGENT_UPDATES` sets the feature up for design review and end-to-end checks.
fn mode() -> Option<String> {
    std::env::var("TREK_AGENT_UPDATES").ok().map(|v| v.trim().to_string())
}

impl AgentUpdates {
    /// As a launch starts it: the cached last check, or what `TREK_AGENT_UPDATES` asks for.
    pub fn at_launch() -> Self {
        let mode = mode();
        if mode.as_deref() == Some("mock") {
            return Self { found: trek_core::agent_update::mock_versions(), checked_at: trek_core::store::now_ms(), runner: Runner::DryRun, mock: true, ..Default::default() };
        }
        let snapshot = trek_core::agent_update::Snapshot::load();
        let runner = if mode.as_deref() == Some("dry-run") { Runner::DryRun } else { Runner::Real };
        Self { found: snapshot.agents, checked_at: snapshot.checked_at, runner, ..Default::default() }
    }

    /// `TREK_AGENT_UPDATES_START=1` with a dry run: "Update all" as soon as there's something to
    /// update, so a pretend update can be watched end to end without a click. Never with the
    /// real runner.
    pub fn start_at_launch(&self) -> bool {
        matches!(self.runner, Runner::DryRun) && std::env::var_os("TREK_AGENT_UPDATES_START").is_some()
    }

    /// A background check is due: on, none running, and the last one 12 hours ago or more.
    pub fn due(&self, enabled: bool, now: i64) -> bool {
        enabled && !self.checking && !self.mock && (self.checked_at == 0 || now - self.checked_at >= CHECK_EVERY_MS)
    }

    pub fn job(&self, agent: &str) -> Option<&Job> {
        self.jobs.get(agent)
    }

    /// The agents the updates card lists: those with an update out, and those whose update is
    /// under way or just finished (so how it went stays readable).
    pub fn listed(&self) -> Vec<&AgentVersion> {
        self.found.iter().filter(|v| v.update_available() || self.jobs.contains_key(&v.agent)).collect()
    }

    /// Updates out that haven't been installed: the sidebar badge's count.
    pub fn pending(&self) -> usize {
        self.found.iter().filter(|v| v.update_available() && !matches!(self.jobs.get(&v.agent), Some(Job::Updated { .. }))).count()
    }

    /// The agents an "Update all" would update.
    pub fn updatable(&self) -> Vec<String> {
        self.found
            .iter()
            .filter(|v| v.update_available() && matches!(self.jobs.get(&v.agent), None | Some(Job::Failed { .. })))
            .map(|v| v.agent.clone())
            .collect()
    }

    /// Ask for `agent`'s update; `false` when there's none to install or it's already asked for.
    pub fn request(&mut self, agent: &str) -> bool {
        let ready = self.found.iter().any(|v| v.agent == agent && v.update_available());
        if !ready || matches!(self.jobs.get(agent), Some(Job::Queued | Job::Running | Job::Updated { .. })) {
            return false;
        }
        self.jobs.insert(agent.to_string(), Job::Queued);
        self.order.retain(|a| a != agent);
        self.order.push(agent.to_string());
        true
    }

    /// The next update to start, if one can start now: one at a time (package managers lock
    /// their folders), in the order asked, skipping agents with a turn running (`busy`).
    pub fn next(&self, busy: impl Fn(&str) -> bool) -> Option<String> {
        if self.jobs.values().any(|j| *j == Job::Running) {
            return None;
        }
        self.order.iter().find(|a| self.jobs.get(*a) == Some(&Job::Queued) && !busy(a)).cloned()
    }

    pub fn start(&mut self, agent: &str) -> Option<AgentVersion> {
        let v = self.found.iter().find(|v| v.agent == agent)?.clone();
        self.jobs.insert(agent.to_string(), Job::Running);
        Some(v)
    }

    /// Note how `agent`'s update went; once it's in, the agent counts as at its new version.
    pub fn finish(&mut self, agent: &str, outcome: Outcome) {
        self.order.retain(|a| a != agent);
        let job = match outcome {
            Outcome::Updated { version, output } => {
                let v = self.found.iter_mut().find(|v| v.agent == agent);
                let from = v.and_then(|v| v.installed.replace(version.clone()));
                Job::Updated { from, to: version, output }
            }
            Outcome::Failed { summary, output } => Job::Failed { summary, output },
        };
        self.jobs.insert(agent.to_string(), job);
    }

    pub fn queued(&self) -> bool {
        self.jobs.values().any(|j| *j == Job::Queued)
    }

    pub fn running(&self) -> bool {
        self.jobs.values().any(|j| *j == Job::Running)
    }

    /// A check came back with `fresh`. Feeds it couldn't reach keep what the last check found;
    /// finished updates are let go (the check now says where they stand), others carry on.
    pub fn checked(&mut self, mut fresh: Vec<AgentVersion>, now: i64) {
        trek_core::agent_update::carry_over(&mut fresh, &self.found);
        // An update under way outlives the check: its row keeps the versions it started from.
        for v in fresh.iter_mut() {
            if let (Some(Job::Queued | Job::Running), Some(old)) = (self.jobs.get(&v.agent), self.found.iter().find(|o| o.agent == v.agent)) {
                *v = old.clone();
            }
        }
        self.jobs.retain(|_, j| matches!(j, Job::Queued | Job::Running));
        self.found = fresh;
        self.checked_at = now;
        self.checking = false;
    }
}

/// The card's footer line, as MonoCode puts it.
pub const WHY: &str = "New models often need the latest version.";

/// One row per agent with an update out (or one under way): logo, name, installed → latest,
/// and what can be done. `ws` acts on clicks.
pub fn rows(ws: &Entity<Workspace>, cx: &App) -> Vec<AnyElement> {
    let w = ws.read(cx);
    let u = &w.agent_updates;
    u.listed().into_iter().map(|v| row(ws, v, u.job(&v.agent), w.agent_busy(&v.agent), u.output_open.as_deref() == Some(v.agent.as_str()), cx)).collect()
}

fn row(ws: &Entity<Workspace>, v: &AgentVersion, job: Option<&Job>, busy: bool, open: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let agent = AgentId::from_key(&v.agent);
    let key = v.agent.clone();
    let shown = v.command.as_ref().map(|c| c.shown()).unwrap_or_default();
    let versions = match job {
        Some(Job::Updated { from: Some(from), to, .. }) => format!("{from} → {to}"),
        Some(Job::Updated { to, .. }) => to.clone(),
        _ => format!("{} → {}", v.installed.as_deref().unwrap_or("?"), v.latest.as_deref().unwrap_or("?")),
    };
    // What the row says under the name.
    let (detail, tone): (String, Hsla) = match job {
        None => match v.install {
            trek_core::agent_update::Install::Native => ("Via its own installer".into(), muted),
            ref i => (format!("Via {}", i.label()), muted),
        },
        Some(Job::Queued) if busy => (format!("Waiting: {} is mid-turn. It updates when the turn ends.", agent.display_name()), muted),
        Some(Job::Queued) => ("Next, after the update under way".into(), muted),
        Some(Job::Running) => (format!("Running {shown}"), muted),
        Some(Job::Updated { .. }) => ("New sessions use it".into(), muted),
        Some(Job::Failed { summary, .. }) => (summary.clone(), crate::palette::red(cx)),
    };
    let output = match job {
        Some(Job::Updated { output, .. } | Job::Failed { output, .. }) if !output.trim().is_empty() => Some(output.clone()),
        _ => None,
    };
    let update = {
        let (ws, key) = (ws.clone(), key.clone());
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| ws.update(cx, |ws, cx| ws.update_agent(&key, cx))
    };
    let control = match job {
        None | Some(Job::Failed { .. }) => Button::new(SharedString::from(format!("agent-update-{key}")))
            .small()
            .outline()
            .label(if job.is_some() { "Retry" } else { "Update" })
            .tooltip(SharedString::from(format!("Runs {shown}")))
            .on_click(update)
            .into_any_element(),
        Some(Job::Running) => h_flex()
            .id(SharedString::from(format!("agent-updating-{key}")))
            .test_support()
            .h(px(24.))
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(6.))
            .bg(theme.foreground.opacity(0.05))
            .text_size(px(12.5))
            .child(Spinner::new().xsmall().color(muted))
            .child("Updating")
            .into_any_element(),
        Some(Job::Queued) => h_flex()
            .id(SharedString::from(format!("agent-update-waiting-{key}")))
            .test_support()
            .h(px(24.))
            .px(px(8.))
            .gap(px(5.))
            .rounded(px(6.))
            .bg(theme.foreground.opacity(0.05))
            .text_size(px(12.5))
            .text_color(muted)
            .child(Icon::new(crate::assets::Lucide::Clock).xsmall())
            .child(if busy { "Waiting" } else { "Queued" })
            .into_any_element(),
        Some(Job::Updated { .. }) => h_flex()
            .id(SharedString::from(format!("agent-updated-{key}")))
            .test_support()
            .h(px(24.))
            .px(px(4.))
            .gap(px(5.))
            .text_size(px(12.5))
            .child(Icon::new(IconName::Check).xsmall().text_color(crate::palette::emerald(cx)))
            .child("Updated")
            .into_any_element(),
    };
    let toggle = output.as_ref().map(|_| {
        let (ws, key) = (ws.clone(), key.clone());
        div()
            .id(SharedString::from(format!("agent-update-output-{key}")))
            .test_support()
            .flex_none()
            .cursor_pointer()
            .text_color(muted)
            .hover(|s| s.text_color(theme.foreground))
            .child(if open { "Hide output" } else { "Show output" })
            .on_click(move |_, _, cx| {
                ws.update(cx, |ws, cx| {
                    let u = &mut ws.agent_updates;
                    u.output_open = if u.output_open.as_deref() == Some(key.as_str()) { None } else { Some(key.clone()) };
                    cx.notify();
                })
            })
    });
    v_flex()
        .id(SharedString::from(format!("agent-update-row-{key}")))
        .test_support()
        .w_full()
        .py(px(10.))
        .gap(px(4.))
        .child(
            h_flex()
                .w_full()
                .gap(px(12.))
                .child(crate::ui::agent_logo(&agent, px(20.), cx))
                .child(div().flex_1().min_w_0().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).truncate().child(v.name.clone()))
                .child(div().flex_none().font_family(theme.mono_font_family.clone()).text_size(px(12.)).text_color(muted).child(versions))
                .child(h_flex().flex_none().min_w(px(86.)).justify_end().child(control)),
        )
        // Under the name, the width of the row: how it's installed, or how its update goes.
        .child(
            h_flex()
                .pl(px(32.))
                .gap(px(10.))
                .text_size(px(12.))
                .child(div().min_w_0().truncate().text_color(tone).child(detail))
                .children(toggle),
        )
        .when_some(output.filter(|_| open), |el, out| {
            el.child(
                div()
                    .id(SharedString::from(format!("agent-update-log-{key}")))
                    .test_support()
                    .ml(px(32.))
                    .mt(px(4.))
                    .max_h(px(160.))
                    .overflow_y_scroll()
                    .p(px(10.))
                    .rounded(px(8.))
                    .bg(theme.foreground.opacity(0.035))
                    .border_1()
                    .border_color(theme.foreground.opacity(0.07))
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(11.5))
                    .line_height(relative(1.5))
                    .text_color(theme.foreground.opacity(0.8))
                    .child(out),
            )
        })
        .into_any_element()
}

/// "Update all", when more than one agent has an update to install.
pub fn update_all(ws: &Entity<Workspace>, cx: &App) -> Option<AnyElement> {
    let n = ws.read(cx).agent_updates.updatable().len();
    let ws = ws.clone();
    (n > 1).then(|| {
        Button::new("agent-updates-all")
            .small()
            .primary()
            .label("Update all")
            .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.update_all_agents(cx)))
            .into_any_element()
    })
}

/// The sidebar's card: the rows between hairlines, why it matters, and "Update all".
pub fn card(ws: &Entity<Workspace>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let u = &ws.read(cx).agent_updates;
    let title = if u.pending() > 0 { "Agent updates available" } else { "Agent updates" };
    let line = theme.foreground.opacity(0.07);
    crate::ui::menu_surface(cx)
        .id("agent-updates-card")
        .test_support()
        .w(px(420.))
        .p(px(0.))
        .child(
            h_flex()
                .px(px(16.))
                .pt(px(14.))
                .pb(px(10.))
                .gap(px(8.))
                .child(div().flex_1().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).child(title))
                .when(u.checking, |el| el.child(Spinner::new().xsmall().color(theme.muted_foreground))),
        )
        .child(v_flex().px(px(16.)).border_t_1().border_color(line).children(rows(ws, cx).into_iter().enumerate().map(|(i, r)| div().when(i > 0, |el| el.border_t_1().border_color(line)).child(r))))
        .child(
            h_flex()
                .px(px(16.))
                .py(px(12.))
                .gap(px(12.))
                .border_t_1()
                .border_color(line)
                .child(div().flex_1().text_size(px(12.5)).text_color(theme.muted_foreground).child(WHY))
                .children(update_all(ws, cx)),
        )
        .into_any_element()
}

/// The footer pill's count: updates out, or one under way.
pub fn badge(u: &AgentUpdates) -> Option<(usize, bool)> {
    let running = u.running();
    (u.pending() > 0 || running).then(|| (u.pending(), running))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the gpui glob import brings its own `test` attribute.
    use super::{AgentUpdates, Job};
    use trek_core::agent_update::{CHECK_EVERY_MS, Outcome, mock_versions};

    fn updates() -> AgentUpdates {
        AgentUpdates { found: mock_versions(), ..Default::default() }
    }

    #[test]
    fn updates_wait_for_the_agents_turns_and_run_one_at_a_time() {
        let mut u = updates();
        assert_eq!(u.pending(), 3);
        assert_eq!(u.updatable(), ["codex", "opencode", "acp:pi"]);
        for a in u.updatable() {
            assert!(u.request(&a));
        }
        assert!(!u.request("codex"), "asked once");
        assert!(!u.request("claude-code"), "nothing to install");
        // Codex is mid-turn: OpenCode goes first.
        let busy = |a: &str| a == "codex";
        assert_eq!(u.next(busy).as_deref(), Some("opencode"));
        u.start("opencode");
        assert_eq!(u.next(busy), None, "one at a time");
        u.finish("opencode", Outcome::Updated { version: "1.18.34".into(), output: String::new() });
        assert_eq!(u.job("opencode"), Some(&Job::Updated { from: Some("1.18.32".into()), to: "1.18.34".into(), output: String::new() }));
        assert_eq!(u.pending(), 2);
        assert_eq!(u.found.iter().find(|v| v.agent == "opencode").unwrap().installed.as_deref(), Some("1.18.34"));
        assert_eq!(u.next(busy).as_deref(), Some("acp:pi"));
        u.start("acp:pi");
        u.finish("acp:pi", Outcome::Failed { summary: "npm stopped with code 1.".into(), output: "EACCES".into() });
        // Codex still busy: it waits; once its turn ends, it's next.
        assert_eq!(u.next(busy), None);
        assert!(u.queued());
        assert_eq!(u.next(|_| false).as_deref(), Some("codex"));
        // A failed one can be asked again (Codex is still asked for); an updated one is done.
        assert_eq!(u.updatable(), ["acp:pi"]);
        assert!(u.request("acp:pi"));
        assert!(!u.request("opencode"));
        // The card still lists the updated agent with how it went.
        assert_eq!(u.listed().len(), 3);
    }

    #[test]
    fn a_check_keeps_updates_under_way_and_lets_finished_ones_go() {
        let mut u = updates();
        u.request("codex");
        u.request("opencode");
        u.start("opencode");
        u.finish("opencode", Outcome::Updated { version: "1.18.34".into(), output: String::new() });
        u.request("acp:pi");
        u.start("acp:pi");
        let mut fresh = mock_versions();
        fresh[1].installed = Some("1.18.34".into());
        fresh[2].installed = Some("9.9.9".into());
        u.checked(fresh, 1);
        assert_eq!(u.job("codex"), Some(&Job::Queued));
        assert_eq!(u.job("acp:pi"), Some(&Job::Running));
        assert_eq!(u.found[2].installed.as_deref(), Some("0.85.1"), "its row stays as it started");
        assert_eq!(u.job("opencode"), None);
        assert!(!u.found[1].update_available());
    }

    #[test]
    fn checks_come_every_twelve_hours_when_on() {
        let u = AgentUpdates { checked_at: 1_000, ..Default::default() };
        assert!(!u.due(true, 1_000 + CHECK_EVERY_MS - 1));
        assert!(u.due(true, 1_000 + CHECK_EVERY_MS));
        assert!(!u.due(false, 1_000 + CHECK_EVERY_MS));
        assert!(AgentUpdates::default().due(true, 5), "never checked");
        let checking = AgentUpdates { checking: true, ..Default::default() };
        assert!(!checking.due(true, i64::MAX));
    }
}
