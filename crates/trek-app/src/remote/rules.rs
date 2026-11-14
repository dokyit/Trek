//! The user's permission rules from a phone, as Settings › Permissions and an approval card's
//! "Always allow…" have them: the list, the tester, adding and removing a rule, and "Add rule and
//! allow". Reading and trying the rules is always open. Writing one needs this Mac's say-so
//! (`settings.mobile.phone_rules`, off unless turned on here): a rule lasts until it's removed,
//! and anything that holds the phone's token can ask, whatever the phone's own check was.

use crate::workspace::{PendingPermission, Scope, Workspace};
use gpui_kit::Context;
use std::path::Path;
use trek_agents::Decision;
use trek_core::HandHolding;
use trek_core::permission_rules::{self as rules, Outcome, PermissionRule, RuleAction, RuleKind};
use trek_remote as tr;

/// The longest pattern a phone may write (a command line is one command).
const MAX_PATTERN: usize = 1 << 10;

fn kind_of(kind: tr::RuleKind) -> RuleKind {
    match kind {
        tr::RuleKind::Command => RuleKind::Command,
        tr::RuleKind::Edit => RuleKind::Edit,
        tr::RuleKind::Fetch => RuleKind::Fetch,
        tr::RuleKind::Tool => RuleKind::Tool,
    }
}

fn wire_kind(kind: RuleKind) -> tr::RuleKind {
    match kind {
        RuleKind::Command => tr::RuleKind::Command,
        RuleKind::Edit => tr::RuleKind::Edit,
        RuleKind::Fetch => tr::RuleKind::Fetch,
        RuleKind::Tool => tr::RuleKind::Tool,
    }
}

fn action_of(action: tr::RuleVerdict) -> RuleAction {
    match action {
        tr::RuleVerdict::Allow => RuleAction::Allow,
        tr::RuleVerdict::Ask => RuleAction::Ask,
        tr::RuleVerdict::Deny => RuleAction::Deny,
    }
}

fn wire_rule(rule: &PermissionRule) -> tr::Rule {
    tr::Rule {
        kind: wire_kind(rule.kind),
        pattern: rule.pattern.clone(),
        action: match rule.action {
            RuleAction::Allow => tr::RuleVerdict::Allow,
            RuleAction::Ask => tr::RuleVerdict::Ask,
            RuleAction::Deny => tr::RuleVerdict::Deny,
        },
    }
}

fn rule_of(rule: &tr::Rule) -> PermissionRule {
    PermissionRule::new(
        kind_of(rule.kind),
        rule.pattern.trim(),
        action_of(rule.action),
    )
}

fn level(level: HandHolding) -> tr::Access {
    match level {
        HandHolding::Supervised => tr::Access::Supervised,
        HandHolding::AutoAcceptEdits => tr::Access::AutoAcceptEdits,
        HandHolding::Auto => tr::Access::Auto,
        HandHolding::FullAccess => tr::Access::FullAccess,
    }
}

/// What's wrong with `pattern` as a rule of `kind` written from a phone, in the page's words.
fn problem(kind: RuleKind, pattern: &str) -> Option<&'static str> {
    if pattern.len() > MAX_PATTERN {
        return Some("That's too long for a rule.");
    }
    rules::problem(kind, pattern)
}

/// The rule card `p`'s "Always allow…" would add, for the approval the phone shows: none where
/// one of the user's rules asks about it every time, or where no allow could cover it.
pub(super) fn always_allow_draft(
    ws: &Workspace,
    id: &str,
    p: &PendingPermission,
) -> Option<tr::RuleDraft> {
    let call = p.call.as_ref()?;
    if ws.rule_asking(id, &p.request_id).is_some() {
        return None;
    }
    let cwd = ws.thread(id).and_then(|t| t.cwd.clone());
    let draft = rules::allow_draft(call, cwd.as_deref())?;
    Some(tr::RuleDraft {
        kind: wire_kind(draft.kind),
        pattern: draft.pattern,
    })
}

impl Workspace {
    /// Refused unless this Mac lets phones write rules.
    fn phone_may_write_rules(&self) -> tr::HostResult<()> {
        if self.settings.mobile.phone_rules {
            return Ok(());
        }
        Err(tr::HostError::bad_request(format!(
            "Writing permission rules from the phone is off on {} (Settings › Phone). Change them there, or turn it on.",
            crate::words::words().this_computer
        )))
    }

    pub(super) fn remote_rules(&self) -> tr::RulesList {
        tr::RulesList {
            rules: self
                .settings
                .permissions
                .rules
                .iter()
                .map(wire_rule)
                .collect(),
            editable: self.settings.mobile.phone_rules,
        }
    }

    pub(super) fn remote_rule_action(
        &mut self,
        req: tr::RuleActionRequest,
        cx: &mut Context<Self>,
    ) -> tr::HostResult<tr::RulesList> {
        self.phone_may_write_rules()?;
        match req {
            tr::RuleActionRequest::Add { rule } => {
                let rule = rule_of(&rule);
                if let Some(problem) = problem(rule.kind, &rule.pattern) {
                    return Err(tr::HostError::bad_request(problem));
                }
                self.add_permission_rule(rule, cx);
            }
            tr::RuleActionRequest::Remove { index, rule } => {
                let at = index as usize;
                let named = rule_of(&rule);
                if self.settings.permissions.rules.get(at) != Some(&named) {
                    return Err(tr::HostError::conflict(
                        "Your rules have changed since. Look at them again.",
                    ));
                }
                self.remove_permission_rule(at, cx);
            }
        }
        Ok(self.remote_rules())
    }

    /// The tester on Settings › Permissions: which rule decides the call and why, and what happens
    /// at each level, in the project folder the Mac's page tries it in.
    pub(super) fn remote_rule_test(
        &self,
        req: tr::RuleTestRequest,
    ) -> tr::HostResult<tr::RuleTest> {
        let tried = req.text.trim().to_string();
        if tried.is_empty() {
            return Err(tr::HostError::bad_request("Type something to try."));
        }
        let list = &self.settings.permissions.rules;
        let call = crate::settings_view::rules::test_call(kind_of(req.kind), &tried);
        let cwd = self.cwd_in(&Scope::Main);
        let verdict = rules::decide(list, &call, cwd.as_deref());
        let at = cwd.clone().unwrap_or_else(trek_core::paths::home);
        let levels = HandHolding::ALL
            .into_iter()
            .map(|l| tr::LevelOutcome {
                level: level(l),
                outcome: match Outcome::of(
                    verdict.as_ref(),
                    trek_agents::level_would_ask(&call, Path::new(&at), l),
                ) {
                    Outcome::Refused => tr::RuleOutcome::Refused,
                    Outcome::Asks => tr::RuleOutcome::Asks,
                    Outcome::Runs => tr::RuleOutcome::Runs,
                },
            })
            .collect();
        Ok(tr::RuleTest {
            kind: req.kind,
            decided_by: verdict
                .as_ref()
                .map(|v| crate::settings_view::rules::decided_by(&list[v.rule], v, &tried)),
            rule: verdict.as_ref().map(|v| v.rule as u32),
            text: req.text,
            levels,
        })
    }

    /// "Add rule and allow", as the card's own: the rule must let this card's call through with
    /// the rest of the list, or it would be a rule that does nothing for what the user is looking
    /// at.
    pub(super) fn remote_always_allow(
        &mut self,
        req: tr::AlwaysAllowRequest,
        cx: &mut Context<Self>,
    ) -> tr::HostResult<()> {
        self.phone_may_write_rules()?;
        let id = req.thread_id.clone();
        let cwd = self
            .thread(&id)
            .ok_or_else(|| tr::HostError::not_found("No such thread"))?
            .cwd
            .clone();
        let request = self
            .live
            .get(&id)
            .and_then(|l| {
                l.permissions
                    .iter()
                    .find(|p| p.request_id == req.request_id)
            })
            .cloned()
            .ok_or_else(|| tr::HostError::conflict("Already answered"))?;
        let draft = always_allow_draft(self, &id, &request).ok_or_else(|| {
            tr::HostError::bad_request(
                "No rule can allow this one. Allow it just this once, or deny it.",
            )
        })?;
        let kind = kind_of(draft.kind);
        let pattern = req.pattern.trim().to_string();
        if let Some(problem) = problem(kind, &pattern) {
            return Err(tr::HostError::bad_request(problem));
        }
        let rule = PermissionRule::new(kind, pattern, RuleAction::Allow);
        let mut all = self.settings.permissions.rules.clone();
        all.push(rule.clone());
        let call = request.call.as_ref().ok_or_else(|| {
            tr::HostError::bad_request(
                "No rule can allow this one. Allow it just this once, or deny it.",
            )
        })?;
        match rules::decide(&all, call, cwd.as_deref()).map(|v| v.action) {
            Some(RuleAction::Allow) => {}
            Some(RuleAction::Deny | RuleAction::Ask) => {
                return Err(tr::HostError::bad_request(
                    "Another of your rules would still stop this.",
                ));
            }
            None => {
                return Err(tr::HostError::bad_request(
                    "That wouldn't cover what's on this card.",
                ));
            }
        }
        if let Some(remote) = self.remote.as_mut() {
            remote.answered.insert(
                (id.clone(), req.request_id.clone()),
                tr::AnswerResponse::Approval {
                    decision: tr::Decision::Allow,
                },
            );
        }
        self.add_permission_rule(rule, cx);
        self.respond(&id, &req.request_id, Decision::Allow, cx);
        Ok(())
    }
}
