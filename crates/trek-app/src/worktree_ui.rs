//! Worktree pieces shared by the sidebar, the title bars, the composer and the Git tool: the
//! branch chip, and the confirmations before a worktree goes (each says what would be lost).

use crate::palette;
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::*;
use trek_core::worktree::{BRANCH_PREFIX, Removal, Worktree};

/// A thread's worktree branch: small and neutral, amber once its folder is gone.
pub fn branch_chip(id: impl Into<ElementId>, wt: &Worktree, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let missing = wt.is_missing();
    let label = if missing { "Worktree missing".to_string() } else { wt.branch.trim_start_matches(BRANCH_PREFIX).to_string() };
    let tip = if missing { format!("{}: its folder is gone", wt.branch) } else { format!("{}, in a worktree off {}", wt.branch, wt.base) };
    h_flex()
        .id(id)
        .test_support()
        // Gives way before the names beside it (the project's, on a sidebar card) do.
        .flex_shrink(100.)
        .min_w(px(48.))
        .max_w(px(160.))
        .h(px(20.))
        .px(px(6.))
        .gap(px(4.))
        .rounded(px(6.))
        .bg(theme.foreground.opacity(0.06))
        .text_xs()
        .text_color(if missing { palette::amber(cx) } else { theme.muted_foreground })
        .child(Icon::new(crate::assets::Lucide::GitBranch).xsmall())
        .child(div().min_w_0().truncate().child(label))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .into_any_element()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The lines of a confirmation: plain, or a warning (amber).
fn body(lines: Vec<(String, bool)>, cx: &App) -> AnyElement {
    v_flex()
        .gap_2()
        .text_sm()
        .children(lines.into_iter().map(|(text, warn)| div().text_color(if warn { palette::amber(cx) } else { cx.theme().muted_foreground }).child(text)))
        .into_any_element()
}

/// What goes with the worktree, in words: uncommitted work (lost) and the branch (kept with its
/// unmerged commits, or deleted).
fn losses(wt: &Worktree, r: &Removal, delete_unmerged: Option<bool>) -> Vec<(String, bool)> {
    let mut out = vec![];
    if r.missing {
        out.push(("Its worktree folder is already gone.".to_string(), false));
    }
    if r.uncommitted > 0 {
        out.push((format!("{} in the worktree would be lost.", plural(r.uncommitted, "uncommitted change", "uncommitted changes")), true));
    }
    match (r.unmerged, delete_unmerged) {
        (0, _) => out.push((format!("{} has nothing {} doesn't, so it goes too.", wt.branch, wt.base), false)),
        (n, None) => out.push((format!("{} keeps its {} that {} doesn't have.", wt.branch, plural(n, "commit", "commits"), wt.base), false)),
        (n, Some(_)) => out.push((format!("{} has {} that {} doesn't have.", wt.branch, plural(n, "commit", "commits"), wt.base), false)),
    }
    out
}

/// The other threads working in `id`'s worktree, named: “Title”, or “Title” and 2 more.
fn sharer_names(ws: &Entity<Workspace>, id: &str, cx: &App) -> Option<String> {
    let ws = ws.read(cx);
    let others = ws.worktree_sharers(id);
    let first = ws.thread(others.first()?)?.title.clone();
    Some(match others.len() {
        1 => format!("“{first}”"),
        n => format!("“{first}” and {}", plural(n - 1, "more", "more")),
    })
}

/// A footer button that closes the dialog and runs `f`.
fn action(button: Button, f: impl Fn(&mut App) + 'static) -> DialogAction {
    DialogAction::new().child(button.on_click(move |_, _, cx| f(cx)))
}

fn cancel() -> DialogClose {
    DialogClose::new().child(Button::new("wt-cancel").outline().label("Cancel"))
}

/// How a thread in a worktree leaves the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leave {
    Archive,
    Delete,
}

/// Archive or delete a thread that has a worktree, asking whether the worktree goes too. Its
/// state is checked first, so the question names uncommitted work that would be lost; removing
/// loses no more than that (anything written since stops it). A deleted thread takes its worktree
/// with it: nothing in Trek would lead back to one left behind. An archived one can keep it.
/// A worktree another thread works in too (a fork) stays, and isn't asked about.
pub fn confirm_leave(ws: Entity<Workspace>, id: String, leave: Leave, window: &mut Window, cx: &mut App) {
    let Some((title, wt)) = ws.read(cx).thread(&id).and_then(|t| Some((t.title.clone(), t.worktree.clone()?))) else { return };
    if let Some(other) = sharer_names(&ws, &id, cx) {
        match leave {
            Leave::Archive => ws.update(cx, |ws, cx| ws.archive(&id, cx)),
            Leave::Delete => window.open_alert_dialog(cx, move |alert, _, _| {
                let (ws, id) = (ws.clone(), id.clone());
                alert
                    .title(format!("Delete “{title}”?"))
                    .description(format!("The thread and its transcript are deleted from Trek. This can't be undone. Its worktree stays: {other} works in it too."))
                    .footer(DialogFooter::new().child(cancel()).child(action(Button::new("wt-leave").with_variant(ButtonVariant::Danger).label("Delete"), move |cx| {
                        ws.update(cx, |ws, cx| ws.delete_thread(&id, cx))
                    })))
            }),
        }
        return;
    }
    let check = ws.update(cx, |ws, cx| ws.worktree_removal(&id, cx));
    window
        .spawn(cx, async move |cx| {
            let Some(r) = check.await else { return };
            let _ = cx.update(|window, cx| {
                let (verb, intro) = match leave {
                    Leave::Archive => ("Archive", "Its worktree can go with it, or stay for later."),
                    Leave::Delete => ("Delete", "The thread and its transcript are deleted from Trek, and its worktree with them. This can't be undone."),
                };
                let mut lines = vec![(intro.to_string(), false)];
                lines.extend(losses(&wt, &r, None));
                let remove_variant = if r.uncommitted > 0 || leave == Leave::Delete { ButtonVariant::Danger } else { ButtonVariant::Primary };
                let (keep_ws, remove_ws, keep_id, remove_id) = (ws.clone(), ws.clone(), id.clone(), id.clone());
                let discard = r.uncommitted;
                window.open_alert_dialog(cx, move |alert, _, cx| {
                    let (keep_ws, remove_ws, keep_id, remove_id) = (keep_ws.clone(), remove_ws.clone(), keep_id.clone(), remove_id.clone());
                    let remove = move |cx: &mut App| {
                        let id = remove_id.clone();
                        remove_ws.update(cx, |ws, cx| match leave {
                            Leave::Archive => ws.archive_removing_worktree(&id, discard, cx),
                            Leave::Delete => ws.delete_removing_worktree(&id, discard, cx),
                        })
                    };
                    let footer = DialogFooter::new().child(cancel());
                    let footer = if r.missing || leave == Leave::Delete {
                        footer.child(action(Button::new("wt-leave").with_variant(remove_variant).label(verb), remove))
                    } else {
                        footer
                            .child(action(Button::new("wt-keep").outline().label("Archive, keep worktree"), move |cx| {
                                let id = keep_id.clone();
                                keep_ws.update(cx, |ws, cx| ws.archive(&id, cx))
                            }))
                            .child(action(Button::new("wt-remove").with_variant(remove_variant).label("Archive and remove it"), remove))
                    };
                    alert.title(format!("{verb} “{title}”?")).description(body(lines.clone(), cx)).footer(footer)
                });
            });
        })
        .detach();
}

/// The Git tool's "Remove worktree": asks first, and separately about a branch with commits its
/// base doesn't have.
pub fn confirm_remove(ws: Entity<Workspace>, id: String, window: &mut Window, cx: &mut App) {
    let Some(wt) = ws.read(cx).thread(&id).and_then(|t| t.worktree.clone()) else { return };
    let others = sharer_names(&ws, &id, cx);
    let check = ws.update(cx, |ws, cx| ws.worktree_removal(&id, cx));
    window
        .spawn(cx, async move |cx| {
            let Some(r) = check.await else { return };
            let _ = cx.update(|window, cx| {
                let mut lines = vec![(format!("Removes {}. The thread runs in the project folder afterwards.", trek_core::paths::tildify(&wt.path)), false)];
                if let Some(other) = &others {
                    lines.push((format!("{other} works in it too, and moves to the project folder as well."), false));
                }
                lines.extend(losses(&wt, &r, Some(true)));
                let danger = if r.uncommitted > 0 { ButtonVariant::Danger } else { ButtonVariant::Primary };
                let discard = r.uncommitted;
                let (ws1, ws2, id1, id2) = (ws.clone(), ws.clone(), id.clone(), id.clone());
                window.open_alert_dialog(cx, move |alert, _, cx| {
                    let (ws1, ws2, id1, id2) = (ws1.clone(), ws2.clone(), id1.clone(), id2.clone());
                    let footer = DialogFooter::new().child(cancel());
                    let footer = if r.unmerged > 0 {
                        footer
                            .child(action(Button::new("wt-remove-keep").outline().label("Remove, keep branch"), move |cx| {
                                let id = id1.clone();
                                ws1.update(cx, |ws, cx| ws.remove_worktree(&id, discard, false, cx).detach())
                            }))
                            .child(action(Button::new("wt-remove-all").with_variant(ButtonVariant::Danger).label("Remove with branch"), move |cx| {
                                let id = id2.clone();
                                ws2.update(cx, |ws, cx| ws.remove_worktree(&id, discard, true, cx).detach())
                            }))
                    } else {
                        footer.child(action(Button::new("wt-remove").with_variant(danger).label("Remove worktree"), move |cx| {
                            let id = id1.clone();
                            ws1.update(cx, |ws, cx| ws.remove_worktree(&id, discard, false, cx).detach())
                        }))
                    };
                    alert.title("Remove this thread's worktree?").description(body(lines.clone(), cx)).footer(footer)
                });
            });
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::losses;
    use trek_core::worktree::{Removal, Worktree};

    fn wt() -> Worktree {
        Worktree { path: "/tmp/w".into(), branch: "trek/fix".into(), base: "main".into() }
    }

    #[test]
    fn confirmations_name_what_would_be_lost() {
        let clean = losses(&wt(), &Removal { uncommitted: 0, unmerged: 0, missing: false }, None);
        assert_eq!(clean, [("trek/fix has nothing main doesn't, so it goes too.".to_string(), false)]);
        let dirty = losses(&wt(), &Removal { uncommitted: 2, unmerged: 1, missing: false }, None);
        assert_eq!(dirty[0], ("2 uncommitted changes in the worktree would be lost.".to_string(), true));
        assert_eq!(dirty[1].0, "trek/fix keeps its 1 commit that main doesn't have.");
        let asked = losses(&wt(), &Removal { uncommitted: 0, unmerged: 3, missing: true }, Some(true));
        assert_eq!(asked[0].0, "Its worktree folder is already gone.");
        assert_eq!(asked[1].0, "trek/fix has 3 commits that main doesn't have.");
    }
}
