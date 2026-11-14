//! Settings › Permissions: the user's permission rules (`trek_core::permission_rules`), a tester
//! that shows which rule decides a call at each level, and what rules can and can't reach.

use super::SettingsView;
use crate::palette;
use crate::scale::text;
use crate::ui;
use crate::workspace::Scope;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::HandHolding;
use trek_core::permission_rules::{
    self as rules, Call, Outcome, PermissionRule, RuleAction, RuleKind,
};

/// The page's fields, and what's picked next to them.
pub(super) struct RulesForm {
    pattern: Entity<InputState>,
    kind: RuleKind,
    action: RuleAction,
    /// Why Add didn't take what's typed.
    problem: Option<&'static str>,
    test: Entity<InputState>,
    test_kind: RuleKind,
    _subscriptions: Vec<Subscription>,
}

impl RulesForm {
    pub(super) fn new(window: &mut Window, cx: &mut Context<SettingsView>) -> Self {
        let pattern =
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder(RuleKind::Command)));
        let test = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("A command to try, like rm -rf build; echo done")
        });
        let on_pattern = cx.subscribe_in(
            &pattern,
            window,
            |this: &mut SettingsView, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.add_rule(window, cx),
                InputEvent::Change => {
                    this.rules.problem = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        let on_test = cx.subscribe_in(
            &test,
            window,
            |_: &mut SettingsView, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        );
        RulesForm {
            pattern,
            kind: RuleKind::Command,
            action: RuleAction::Deny,
            problem: None,
            test,
            test_kind: RuleKind::Command,
            _subscriptions: vec![on_pattern, on_test],
        }
    }
}

fn placeholder(kind: RuleKind) -> &'static str {
    match kind {
        RuleKind::Command => "A command, like rm or git push",
        RuleKind::Edit => "A file or folder, like ~/.ssh/** or *.lock",
        RuleKind::Fetch => "An address, like example.com",
        RuleKind::Tool => "A tool's name, like WebSearch or mcp__github__*",
    }
}

fn test_placeholder(kind: RuleKind) -> &'static str {
    match kind {
        RuleKind::Command => "A command to try, like rm -rf build; echo done",
        RuleKind::Edit => "A path to try, like src/main.rs",
        RuleKind::Fetch => "An address to try, like https://example.com/x",
        RuleKind::Tool => "A tool's name to try, like mcp__github__create_issue",
    }
}

fn action_tint(action: RuleAction, cx: &App) -> Hsla {
    match action {
        RuleAction::Deny => palette::red(cx),
        RuleAction::Ask => palette::amber(cx),
        RuleAction::Allow => palette::emerald(cx),
    }
}

fn outcome_tint(outcome: Outcome, cx: &App) -> Hsla {
    match outcome {
        Outcome::Refused => palette::red(cx),
        Outcome::Asks => palette::amber(cx),
        Outcome::Runs => palette::emerald(cx),
    }
}

/// The call the tester tries.
pub(crate) fn test_call(kind: RuleKind, text: &str) -> Call {
    match kind {
        RuleKind::Command => Call::command("Bash", text),
        RuleKind::Edit => Call::edit("Write", text),
        RuleKind::Fetch => Call::fetch("WebFetch", text),
        RuleKind::Tool => Call::tool(text),
    }
}

/// Which rule decides what the tester holds, in words. `None` when nothing decides it.
pub(crate) fn decided_by(rule: &PermissionRule, verdict: &rules::Verdict, whole: &str) -> String {
    let named = rule.shown();
    if let Some(why) = verdict.unreadable {
        return format!("{named} decides. {why} That counts as matching.");
    }
    match verdict.part.as_deref().filter(|p| *p != whole) {
        Some(part) => format!("{named} decides, because this line runs “{part}”."),
        None => format!("{named} decides."),
    }
}

impl SettingsView {
    fn add_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (kind, action) = (self.rules.kind, self.rules.action);
        let pattern = self.rules.pattern.read(cx).value().trim().to_string();
        if let Some(problem) = rules::problem(kind, &pattern) {
            self.rules.problem = Some(problem);
            cx.notify();
            return;
        }
        self.workspace.update(cx, |ws, cx| {
            ws.add_permission_rule(PermissionRule::new(kind, pattern, action), cx)
        });
        self.rules
            .pattern
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.rules.problem = None;
        cx.notify();
    }

    /// The Rules, tester and "where rules reach" sections of the page.
    pub(super) fn rules_sections(
        &mut self,
        list: &[PermissionRule],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        // In the order they're tried: what's on screen is how it's read.
        let mut order: Vec<usize> = (0..list.len()).collect();
        order.sort_by_key(|&i| {
            RuleAction::ALL
                .iter()
                .position(|a| *a == list[i].action)
                .unwrap_or(0)
        });
        let rows: Vec<AnyElement> = order
            .into_iter()
            .map(|i| {
                let rule = &list[i];
                h_flex()
                    .id(SharedString::from(format!("rule-{i}")))
                    .h(px(40.))
                    .gap(px(12.))
                    .child(
                        div()
                            .w(px(44.))
                            .flex_none()
                            .text_size(px(text::BODY))
                            .font_medium()
                            .text_color(action_tint(rule.action, cx))
                            .child(rule.action.label()),
                    )
                    .child(
                        div()
                            .w(px(68.))
                            .flex_none()
                            .text_size(px(text::BODY))
                            .text_color(theme.muted_foreground)
                            .child(rule.kind.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(mono.clone())
                            .text_size(px(text::LABEL))
                            .child(rule.pattern.clone()),
                    )
                    .child(
                        ui::icon_button(
                            SharedString::from(format!("rule-del-{i}")),
                            IconName::Delete,
                            "Remove",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.workspace
                                .update(cx, |ws, cx| ws.remove_permission_rule(i, cx))
                        })),
                    )
                    .into_any_element()
            })
            .collect();

        let me = cx.entity().downgrade();
        let pick_kind = move |kind: RuleKind, window: &mut Window, cx: &mut App| {
            let _ = me.update(cx, |this, cx| {
                this.rules.kind = kind;
                this.rules
                    .pattern
                    .update(cx, |s, cx| s.set_placeholder(placeholder(kind), window, cx));
                cx.notify();
            });
        };
        let me = cx.entity().downgrade();
        let pick_action = move |action: RuleAction, _: &mut Window, cx: &mut App| {
            let _ = me.update(cx, |this, cx| {
                this.rules.action = action;
                cx.notify();
            });
        };
        let adding = v_flex()
            .pt(px(12.))
            .gap(px(8.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .flex_wrap()
                    .child(ui::segmented(
                        "rule-action",
                        RuleAction::ALL.iter().map(|a| (*a, a.label())).collect(),
                        self.rules.action,
                        pick_action,
                        cx,
                    ))
                    .child(ui::segmented(
                        "rule-kind",
                        RuleKind::ALL.iter().map(|k| (*k, k.label())).collect(),
                        self.rules.kind,
                        pick_kind,
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    // Typed in the monospace the list above shows rules in.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(mono.clone())
                            .child(Input::new(&self.rules.pattern).id("rule-pattern").small()),
                    )
                    .child(
                        Button::new("rule-add")
                            .small()
                            .outline()
                            .label("Add rule")
                            .on_click(cx.listener(|this, _, window, cx| this.add_rule(window, cx))),
                    ),
            )
            .when_some(self.rules.problem, |el, problem| {
                el.child(
                    div()
                        .text_size(px(text::BODY))
                        .text_color(palette::failed(cx))
                        .child(problem),
                )
            })
            .into_any_element();

        // The tester.
        let cwd = self.workspace.read(cx).cwd_in(&Scope::Main);
        let tried = self.rules.test.read(cx).value().trim().to_string();
        let call = test_call(self.rules.test_kind, &tried);
        let verdict = (!tried.is_empty())
            .then(|| rules::decide(list, &call, cwd.as_deref()))
            .flatten();
        let cwd_for_level = cwd.clone().unwrap_or_else(trek_core::paths::home);
        let level_cells: Vec<AnyElement> = HandHolding::ALL
            .into_iter()
            .map(|level| {
                let outcome = Outcome::of(
                    verdict.as_ref(),
                    trek_agents::level_would_ask(&call, &cwd_for_level, level),
                );
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(text::LABEL))
                            .text_color(theme.muted_foreground)
                            .child(level.label()),
                    )
                    .child(
                        div()
                            .text_size(px(text::BODY))
                            .font_medium()
                            .text_color(outcome_tint(outcome, cx))
                            .child(outcome.label()),
                    )
                    .into_any_element()
            })
            .collect();
        let parts = (self.rules.test_kind == RuleKind::Command && !tried.is_empty()).then(|| {
            let line = rules::parse_line(&tried);
            let mut said: Vec<String> = line
                .parts
                .iter()
                .filter(|p| !p.transparent)
                .map(|p| format!("“{}”", p.display()))
                .collect();
            said.dedup();
            (said, line.opaque)
        });
        let result: AnyElement = if tried.is_empty() {
            div()
                .py(px(12.))
                .text_size(px(text::BODY))
                .text_color(theme.muted_foreground)
                .child("Type something above to see which rule decides it, and what happens at each level.")
                .into_any_element()
        } else {
            let decided = match &verdict {
                Some(v) => decided_by(&list[v.rule], v, &tried),
                None => "No rule decides this, so the level does.".to_string(),
            };
            #[cfg(any(test, feature = "test-support"))]
            super::shown::text(&decided);
            v_flex()
                .py(px(12.))
                .gap(px(10.))
                .child(div().text_size(px(text::BODY)).child(decided))
                .when_some(parts, |el, (said, opaque)| {
                    el.when(said.len() > 1, |el| {
                        el.child(
                            div()
                                .text_size(px(text::BODY))
                                .text_color(theme.muted_foreground)
                                .child(format!("Read as {}.", said.join(", "))),
                        )
                    })
                    .when_some(opaque.filter(|_| verdict.is_none()), |el, why| {
                        el.child(
                            div()
                                .text_size(px(text::BODY))
                                .text_color(theme.muted_foreground)
                                .child(format!(
                                    "{why} A deny rule would catch it, and no allow rule can cover it."
                                )),
                        )
                    })
                })
                .child(h_flex().gap(px(12.)).children(level_cells))
                .child(
                    div()
                        .text_size(px(text::BODY))
                        .text_color(theme.muted_foreground)
                        .child(if verdict.is_none() {
                            "With no rule, this is what Direct does. Other agents apply their own checks at each level."
                        } else {
                            "This is what happens where Trek sees the request first. Codex inside its sandbox doesn't ask: see below."
                        }),
                )
                .into_any_element()
        };
        let me = cx.entity().downgrade();
        let pick_test_kind = move |kind: RuleKind, window: &mut Window, cx: &mut App| {
            let _ = me.update(cx, |this, cx| {
                this.rules.test_kind = kind;
                this.rules.test.update(cx, |s, cx| {
                    s.set_placeholder(test_placeholder(kind), window, cx)
                });
                cx.notify();
            });
        };
        let tester = v_flex()
            .gap(px(8.))
            .child(h_flex().child(ui::segmented(
                "rule-test-kind",
                RuleKind::ALL.iter().map(|k| (*k, k.label())).collect(),
                self.rules.test_kind,
                pick_test_kind,
                cx,
            )))
            .child(
                div()
                    .font_family(mono.clone())
                    .child(Input::new(&self.rules.test).id("rule-test").small()),
            )
            .child(result)
            .into_any_element();

        // Where rules reach. Each line was checked against the real agent; see `claude.rs` and
        // `codex.rs` on how.
        let reach = |agent: &'static str, what: &'static str| {
            h_flex()
                .py(px(10.))
                .gap(px(12.))
                .items_start()
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .text_size(px(text::BODY))
                        .child(agent),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(text::BODY))
                        .line_height(relative(1.5))
                        .text_color(theme.muted_foreground)
                        .child(what),
                )
                .into_any_element()
        };
        let mut out = vec![
            Self::heading("Rules", cx),
            Self::note(
                "Rules sit on top of a thread's level. Deny wins, then Ask, then Allow, wherever they are in the list. If no rule matches, the level decides. Full access still obeys a deny rule: that's what a deny rule is for.",
                cx,
            ),
        ];
        if rows.is_empty() {
            out.push(
                div()
                    .text_size(px(text::BODY))
                    .text_color(theme.muted_foreground)
                    .child("No rules yet.")
                    .into_any_element(),
            );
        } else {
            out.push(ui::group(rows, cx));
        }
        out.push(adding);
        out.push(
            div()
                .pt(px(12.))
                .max_w(px(560.))
                .text_size(px(text::BODY))
                .line_height(relative(1.5))
                .text_color(theme.muted_foreground)
                .child("A command rule reads a line the way a shell would, so “rm” also catches “echo y; rm x”, “FOO=1 rm x”, “/bin/rm x” and “sh -c 'rm x'”. A line Trek can't read counts as matching a deny. An allow has to cover every command on the line, exactly as written.")
                .into_any_element(),
        );
        out.push(Self::heading("Test a rule", cx));
        out.push(tester);
        out.push(Self::heading("Where rules reach", cx));
        out.push(ui::group(
            vec![
                reach(
                    "Direct",
                    "Every command, edit, fetch and MCP tool call, at every level. Reading files and searching don't ask, so rules don't reach them.",
                ),
                reach(
                    "Claude Code",
                    "Every tool call, at every level including Full access, through a hook Trek adds to its sessions.",
                ),
                reach(
                    "Codex",
                    "At Full access, every command and file change, once you have a deny or ask rule. At the other levels, only what asks to leave its sandbox reaches Trek first. Inside the sandbox Codex runs it without asking: Trek hears afterward, says so, and for a deny stops the turn, but it can't undo it. Tool rules don't reach its MCP tools.",
                ),
                reach(
                    "ACP agents",
                    "Only what the agent asks about. Trek answers for it at each level and rules come first, but an agent set up to run without asking never comes to Trek.",
                ),
            ],
            cx,
        ));
        out.push(
            div()
                .pt(px(12.))
                .max_w(px(560.))
                .text_size(px(text::BODY))
                .line_height(relative(1.5))
                .text_color(theme.muted_foreground)
                .child("Rules read what an agent is about to do, not what a program does once it runs. “python x.py” or “make clean” can still delete files, another program can do what a denied one does, and a file rule doesn't see a command that writes a file. A thread on another computer follows that computer's rules.")
                .into_any_element(),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    // By name: `gpui_kit::*` brings a `test` of its own.
    use super::{decided_by, test_call};
    use trek_core::permission_rules::{self as rules, Call, PermissionRule, RuleAction, RuleKind};

    #[test]
    fn the_tester_says_which_rule_and_why() {
        let rule = PermissionRule::new(RuleKind::Command, "rm", RuleAction::Deny);
        let list = [rule.clone()];
        let on = |line: &str| {
            let v = rules::decide(&list, &Call::command("Bash", line), None).unwrap();
            decided_by(&rule, &v, line)
        };
        assert_eq!(on("rm x"), "Deny command “rm” decides.");
        assert_eq!(
            on("echo y; rm x"),
            "Deny command “rm” decides, because this line runs “rm x”."
        );
        assert!(on("echo 'x").contains("A quote never closes. That counts as matching."));
    }

    #[test]
    fn the_tester_tries_the_kind_that_is_picked() {
        assert_eq!(test_call(RuleKind::Command, "ls").kind, RuleKind::Command);
        assert_eq!(test_call(RuleKind::Edit, "a").kind, RuleKind::Edit);
        assert_eq!(test_call(RuleKind::Fetch, "a").kind, RuleKind::Fetch);
        let tool = test_call(RuleKind::Tool, "mcp__x__y");
        assert_eq!(
            (tool.kind, tool.tool.as_str()),
            (RuleKind::Tool, "mcp__x__y")
        );
    }
}
