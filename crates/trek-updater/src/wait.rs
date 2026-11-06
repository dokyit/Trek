//! An update that waits for running work: what the restart would stop, what the updater offers
//! while it would, and when a restart the user asked for may go. Plain functions over plain
//! values: `Workspace` says how things stand and does what they decide. The install itself only
//! happens at the very end, with the relaunch right behind it: a bundle replaced under a running
//! Trek loses its macOS permissions until it restarts, so nothing here installs and then waits.

use crate::{UpdateAction, UpdateStatus, UpdateView};

/// What a restart would cut short right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Running {
    /// Chats at work: a turn running or stopped on a card, messages waiting to go, or sub-agents
    /// out in the background. A sub-agent counts as the chat that started it.
    pub chats: usize,
    /// The iPhone app is being built and installed. A connected iPhone isn't work: it reconnects
    /// by itself once Trek is back.
    pub phone: bool,
    /// Trek's own work on files, or an agent's CLI being updated.
    pub other: bool,
}

impl Running {
    pub fn any(self) -> bool {
        self.chats > 0 || self.phone || self.other
    }

    /// "2 threads", "1 thread and the iPhone update", "Trek's work".
    fn named(self) -> String {
        let chats = match self.chats {
            0 => None,
            1 => Some("1 thread".to_string()),
            n => Some(format!("{n} threads")),
        };
        let others = self.phone.then_some("the iPhone update");
        match (chats, others) {
            (Some(c), Some(o)) => format!("{c} and {o}"),
            (Some(c), None) => c,
            (None, Some(o)) => o.to_string(),
            (None, None) => "Trek's work".to_string(),
        }
    }

    /// Whether what `named` says takes "finish" rather than "finishes".
    fn many(self) -> bool {
        self.chats > 1 || (self.chats == 1 && self.phone)
    }
}

/// "Installs when 2 chats finish".
fn installs_when(running: Running) -> String {
    let finish = if running.many() { "finish" } else { "finishes" };
    format!("Installs when {} {finish}", running.named())
}

/// How things stand when a restart is asked for or due.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Moment {
    pub running: Running,
    /// The user typed in a composer moments ago, and hasn't sent it: they're in the middle of it.
    pub typing: bool,
    /// Files open in the editor with edits not saved: nothing brings those back after a restart.
    pub unsaved: usize,
}

/// What holds a restart the user asked for back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    Work,
    Unsaved,
    Typing,
}

impl Moment {
    /// What the restart waits for, if anything. Once this is `None` it warns, counts down and goes.
    /// A message not sent doesn't hold it for long: it's kept for the next launch, so only someone
    /// typing right now is waited for, not a draft left alone.
    pub fn hold(self) -> Option<Hold> {
        if self.running.any() {
            Some(Hold::Work)
        } else if self.unsaved > 0 {
            Some(Hold::Unsaved)
        } else if self.typing {
            Some(Hold::Typing)
        } else {
            None
        }
    }

    /// What Install now costs, in a sentence; `None` when nothing is lost by it.
    fn cost(self) -> Option<String> {
        let edits = (self.unsaved > 0).then(|| {
            if self.unsaved == 1 {
                "unsaved edits in 1 file are lost".to_string()
            } else {
                format!("unsaved edits in {} files are lost", self.unsaved)
            }
        });
        match (self.running.any(), edits) {
            (true, Some(edits)) => Some(format!(
                "Install now stops {} first, and {edits}.",
                self.running.named()
            )),
            (true, None) => Some(format!("Install now stops {} first.", self.running.named())),
            (false, Some(edits)) => Some(format!("With Install now, {edits}.")),
            (false, None) => None,
        }
    }
}

/// What the updater offers while a restart would stop something.
#[derive(Debug, Clone, PartialEq)]
pub struct Choices {
    /// "Installs when 2 threads finish": the default, and what's under way once it's chosen.
    pub wait: String,
    /// It's been chosen: Trek is waiting.
    pub waiting: bool,
    /// What Install now costs: "Install now stops 2 threads first."
    pub now: Option<String>,
    /// Later is on offer: not yet chosen.
    pub later: bool,
}

/// The updater's view of an update ready to install, given what's running (`view` as
/// `describe_update` has it; other states, and builds that can't update themselves, pass through).
/// `later`: the version the user chose Later for.
pub fn with_running(
    mut view: UpdateView,
    status: &UpdateStatus,
    moment: Moment,
    later: Option<&str>,
) -> UpdateView {
    match status {
        UpdateStatus::Ready { version, .. } if view.action == Some(UpdateAction::Restart) => {
            let put_off = later == Some(version.as_str());
            // Nothing a restart would lose (someone typing has their message kept): a click on
            // Restart is the user's own say-so, and means what it says.
            if !moment.running.any() && moment.unsaved == 0 {
                if put_off {
                    view.line =
                        format!("Trek {version} installs when you quit. Restart whenever suits.");
                }
                return view;
            }
            let first = if put_off {
                format!("Trek {version} installs when you quit.")
            } else {
                format!("Trek {version} is ready.")
            };
            let running = moment.running;
            let (wait, runs) = if running.any() {
                let verb = if running.many() { "are" } else { "is" };
                (
                    installs_when(running),
                    format!(" {} {verb} running.", upper_first(&running.named())),
                )
            } else {
                (
                    "Installs once your edits are saved".to_string(),
                    String::new(),
                )
            };
            view.line = format!("{first}{runs}");
            view.secondary = Some(UpdateAction::InstallNow);
            view.choices = Some(Choices {
                wait,
                waiting: false,
                now: moment.cost(),
                later: !put_off,
            });
            view
        }
        UpdateStatus::RestartPending { version, .. }
            if view.action == Some(UpdateAction::RestartNow) =>
        {
            let (line, wait) = match moment.hold() {
                Some(Hold::Work) => {
                    let wait = installs_when(moment.running);
                    (format!("Trek {version} {}.", lower_first(&wait)), wait)
                }
                Some(Hold::Unsaved) => (
                    format!("Trek {version} installs once your edits are saved."),
                    "Installs once your edits are saved".to_string(),
                ),
                Some(Hold::Typing) => (
                    format!("Trek {version} installs once you've stopped typing."),
                    "Installs once you've stopped typing".to_string(),
                ),
                None => (
                    format!("Trek {version} installs in a moment."),
                    "Installs in a moment".to_string(),
                ),
            };
            view.line = line;
            view.action = Some(UpdateAction::InstallNow);
            view.secondary = None;
            view.choices = Some(Choices {
                wait,
                waiting: true,
                now: moment.cost(),
                later: true,
            });
            view
        }
        _ => view,
    }
}

fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::describe_update;
    use std::path::PathBuf;
    use trek_core::settings::Channel;
    use trek_core::update::Blocker;

    fn ready() -> UpdateStatus {
        UpdateStatus::Ready {
            version: "0.5.0".into(),
            staged: PathBuf::from("/tmp/Trek.app"),
        }
    }

    fn pending() -> UpdateStatus {
        UpdateStatus::RestartPending {
            version: "0.5.0".into(),
            staged: PathBuf::from("/tmp/Trek.app"),
        }
    }

    fn view(status: &UpdateStatus, moment: Moment, later: Option<&str>) -> UpdateView {
        with_running(
            describe_update(status, true, None, Channel::Stable),
            status,
            moment,
            later,
        )
    }

    fn chats(n: usize) -> Moment {
        Moment {
            running: Running {
                chats: n,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn says_how_many_chats_run_and_offers_all_three() {
        let v = view(&ready(), chats(2), None);
        assert_eq!(v.line, "Trek 0.5.0 is ready. 2 threads are running.");
        assert_eq!(
            v.action,
            Some(UpdateAction::Restart),
            "waiting is the default"
        );
        assert_eq!(v.secondary, Some(UpdateAction::InstallNow));
        let c = v.choices.unwrap();
        assert_eq!(c.wait, "Installs when 2 threads finish");
        assert!(!c.waiting && c.later);
        assert_eq!(c.now.as_deref(), Some("Install now stops 2 threads first."));

        let one = view(&ready(), chats(1), None);
        assert_eq!(one.line, "Trek 0.5.0 is ready. 1 thread is running.");
        let c = one.choices.unwrap();
        assert_eq!(c.wait, "Installs when 1 thread finishes");
        assert_eq!(c.now.as_deref(), Some("Install now stops 1 thread first."));
    }

    #[test]
    fn nothing_running_is_the_plain_restart() {
        let plain = describe_update(&ready(), true, None, Channel::Stable);
        assert_eq!(view(&ready(), Moment::default(), None), plain);
        assert_eq!(plain.action, Some(UpdateAction::Restart));
        assert_eq!(plain.choices, None);
    }

    #[test]
    fn the_iphone_update_and_background_work_count_too() {
        let phone = Moment {
            running: Running {
                phone: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let c = view(&ready(), phone, None).choices.unwrap();
        assert_eq!(c.wait, "Installs when the iPhone update finishes");
        let both = Moment {
            running: Running {
                chats: 1,
                phone: true,
                other: false,
            },
            ..Default::default()
        };
        let c = view(&ready(), both, None).choices.unwrap();
        assert_eq!(
            c.wait,
            "Installs when 1 thread and the iPhone update finish"
        );
        let trek = Moment {
            running: Running {
                other: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let c = view(&ready(), trek, None).choices.unwrap();
        assert_eq!(c.wait, "Installs when Trek's work finishes");
    }

    #[test]
    fn a_wait_chosen_says_so_and_can_still_go_now_or_be_put_off() {
        let v = view(&pending(), chats(2), None);
        assert_eq!(v.line, "Trek 0.5.0 installs when 2 threads finish.");
        assert_eq!(v.action, Some(UpdateAction::InstallNow), "never stuck");
        let c = v.choices.unwrap();
        assert!(c.waiting && c.later);
        assert_eq!(c.now.as_deref(), Some("Install now stops 2 threads first."));

        // The chats done, the line follows what it waits for now.
        let typing = Moment {
            typing: true,
            ..Default::default()
        };
        assert_eq!(
            view(&pending(), typing, None).line,
            "Trek 0.5.0 installs once you've stopped typing."
        );
        assert_eq!(
            view(&pending(), Moment::default(), None).line,
            "Trek 0.5.0 installs in a moment."
        );
    }

    #[test]
    fn later_stays_chosen_until_a_newer_version() {
        let v = view(&ready(), Moment::default(), Some("0.5.0"));
        assert_eq!(
            v.line,
            "Trek 0.5.0 installs when you quit. Restart whenever suits."
        );
        assert_eq!(v.action, Some(UpdateAction::Restart));
        let v = view(&ready(), chats(2), Some("0.5.0"));
        assert_eq!(
            v.line,
            "Trek 0.5.0 installs when you quit. 2 threads are running."
        );
        let c = v.choices.unwrap();
        assert!(!c.later, "Later is what it already is");
        assert_eq!(
            c.wait, "Installs when 2 threads finish",
            "and can be changed"
        );
        // Another release takes its place: it's asked afresh.
        assert_eq!(
            view(&ready(), chats(2), Some("0.4.9")).line,
            "Trek 0.5.0 is ready. 2 threads are running."
        );
    }

    #[test]
    fn unsaved_edits_are_said_and_held_for() {
        let m = Moment {
            unsaved: 2,
            ..Default::default()
        };
        assert_eq!(m.hold(), Some(Hold::Unsaved));
        let v = view(&ready(), m, None);
        let c = v.choices.unwrap();
        assert_eq!(c.wait, "Installs once your edits are saved");
        assert_eq!(
            c.now.as_deref(),
            Some("With Install now, unsaved edits in 2 files are lost.")
        );
        let m = Moment {
            unsaved: 1,
            running: chats(1).running,
            ..Default::default()
        };
        assert_eq!(
            view(&pending(), m, None).choices.unwrap().now.as_deref(),
            Some("Install now stops 1 thread first, and unsaved edits in 1 file are lost.")
        );
    }

    #[test]
    fn what_holds_a_restart_goes_in_order_and_a_draft_alone_holds_nothing() {
        assert_eq!(
            Moment::default().hold(),
            None,
            "a draft left alone is kept for next time"
        );
        let typing = Moment {
            typing: true,
            ..Default::default()
        };
        assert_eq!(typing.hold(), Some(Hold::Typing));
        let all = Moment {
            typing: true,
            unsaved: 1,
            running: chats(1).running,
        };
        assert_eq!(all.hold(), Some(Hold::Work));
        assert_eq!(
            Moment {
                running: Running::default(),
                ..all
            }
            .hold(),
            Some(Hold::Unsaved)
        );
    }

    #[test]
    fn typing_doesnt_change_what_restart_says_or_does() {
        // The user clicking Restart is their own say-so.
        let typing = Moment {
            typing: true,
            ..Default::default()
        };
        let v = view(&ready(), typing, None);
        assert_eq!(v.action, Some(UpdateAction::Restart));
        assert_eq!(v.choices, None);
    }

    #[test]
    fn a_build_that_cant_update_itself_is_left_alone() {
        let blocked = describe_update(&ready(), true, Some(Blocker::DevBuild), Channel::Stable);
        let v = with_running(blocked.clone(), &ready(), chats(2), None);
        assert_eq!(v, blocked);
        let v = with_running(blocked.clone(), &pending(), chats(2), None);
        assert_eq!(v, blocked);
    }

    #[test]
    fn other_states_pass_through() {
        let idle = describe_update(&UpdateStatus::Idle, true, None, Channel::Stable);
        assert_eq!(
            with_running(idle.clone(), &UpdateStatus::Idle, chats(2), None),
            idle
        );
    }
}
