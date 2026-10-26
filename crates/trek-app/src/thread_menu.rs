//! What can be done to a thread, as menu items: the title bar's "⋯" menu (`button`) and the
//! sidebar's right-click menu build theirs from the same items here (open in a new window, fork,
//! pin, rename, archive, the Copy submenu), so the two can't drift apart. Also the thread's
//! copies (`copy_text`), which the menus, the shortcuts and the palette share.

use crate::keys::{self, Id};
use crate::workspace::{Workspace, WorkspaceEvent};
use crate::worktree_ui::Leave;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, IconName, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use trek_core::store::Thread;

/// A thread's `trek://` link: it opens the thread in Trek (`deep_link`).
pub fn link(id: &str) -> String {
    format!("trek://thread/{id}")
}

/// The command that resumes the thread's session in its agent's own CLI, where it has one.
pub fn resume_command(t: &Thread) -> Option<String> {
    let native = t.native_id.as_ref()?;
    match &t.agent {
        trek_core::AgentId::ClaudeCode => Some(format!("claude --resume {native}")),
        trek_core::AgentId::Codex => Some(format!("codex resume {native}")),
        trek_core::AgentId::OpenCode => Some(format!("opencode -s {native}")),
        _ => None,
    }
}

/// What the menus need to know of a thread, read as one opens.
#[derive(Debug, Clone)]
pub struct Facts {
    pub id: String,
    pub title: String,
    pub pinned: bool,
    pub cwd: Option<PathBuf>,
    pub resume: Option<String>,
    pub in_worktree: bool,
    /// What archiving does to its sub-agents (`None`: it has none).
    pub archive_note: Option<String>,
    /// It lives on another machine (`machines`).
    pub remote: bool,
}

impl Facts {
    pub fn read(ws: &Workspace, id: &str) -> Option<Facts> {
        let t = ws.thread(id)?;
        Some(Facts {
            id: t.id.clone(),
            title: t.title.clone(),
            pinned: t.pinned_at.is_some(),
            cwd: t.cwd.clone(),
            resume: resume_command(t),
            in_worktree: t.worktree.is_some(),
            archive_note: ws.sub_agents_note(&t.id, "archived"),
            remote: crate::machines::is_remote(&t.id),
        })
    }
}

// ───────────────────────────── copies ─────────────────────────────

/// One of the thread's copies (the Copy submenu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Copy {
    Link,
    Markdown,
    PlainText,
    LastAnswer,
    Folder,
    Resume,
    Id,
    Title,
}

impl Copy {
    pub const ALL: [Copy; 8] = [
        Copy::Link,
        Copy::Markdown,
        Copy::PlainText,
        Copy::LastAnswer,
        Copy::Folder,
        Copy::Resume,
        Copy::Id,
        Copy::Title,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Copy::Link => "Copy link",
            Copy::Markdown => "Copy as Markdown",
            Copy::PlainText => "Copy as plain text",
            Copy::LastAnswer => "Copy last answer",
            Copy::Folder => "Copy working directory",
            Copy::Resume => "Copy resume command",
            Copy::Id => "Copy thread ID",
            Copy::Title => "Copy title",
        }
    }

    /// The item's id, for the verify driver.
    pub fn element_id(self) -> &'static str {
        match self {
            Copy::Link => "thread-copy-link",
            Copy::Markdown => "thread-copy-markdown",
            Copy::PlainText => "thread-copy-text",
            Copy::LastAnswer => "thread-copy-answer",
            Copy::Folder => "thread-copy-folder",
            Copy::Resume => "thread-copy-resume",
            Copy::Id => "thread-copy-id",
            Copy::Title => "thread-copy-title",
        }
    }

    pub fn shortcut(self) -> Option<Id> {
        match self {
            Copy::Link => Some(Id::CopyThreadLink),
            Copy::Markdown => Some(Id::CopyThreadMarkdown),
            Copy::Folder => Some(Id::CopyThreadFolder),
            _ => None,
        }
    }

    /// The toast once it's on the clipboard.
    fn copied(self) -> &'static str {
        match self {
            Copy::Link => "Copied the thread's link",
            Copy::Markdown => "Copied the thread as Markdown",
            Copy::PlainText => "Copied the thread as plain text",
            Copy::LastAnswer => "Copied the last answer",
            Copy::Folder => "Copied the working directory",
            Copy::Resume => "Copied the resume command",
            Copy::Id => "Copied the thread ID",
            Copy::Title => "Copied the title",
        }
    }
}

/// The text `what` puts on the clipboard for thread `id`; `None` when the thread has none (no
/// answer yet, no folder, an agent with no resume command).
pub fn copy_text(ws: &Workspace, id: &str, what: Copy) -> Option<String> {
    let t = ws.thread(id)?;
    match what {
        Copy::Link => Some(link(id)),
        Copy::Markdown => Some(ws.transcript_markdown(id)),
        Copy::PlainText => {
            let items =
                crate::share::render::prepare(ws.transcript_items(id), Default::default(), None);
            Some(crate::share::render::plain_text(
                &t.title,
                &t.agent.display_name(),
                &items,
            ))
        }
        Copy::LastAnswer => ws.last_answer(id).map(|a| a.trim().to_string()),
        Copy::Folder => t.cwd.as_ref().map(|d| d.display().to_string()),
        Copy::Resume => resume_command(t),
        Copy::Id => Some(id.to_string()),
        Copy::Title => Some(t.title.clone()),
    }
}

/// Put `what` of thread `id` on the clipboard, and say so.
pub fn copy(ws: &Entity<Workspace>, id: &str, what: Copy, cx: &mut App) {
    let text = copy_text(ws.read(cx), id, what);
    let message = match text {
        Some(text) => {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            what.copied().to_string()
        }
        None => match what {
            Copy::LastAnswer => "There's no answer to copy yet".into(),
            Copy::Folder => "This thread has no working directory".into(),
            Copy::Resume => "This thread's agent has no command to resume it with".into(),
            _ => "Nothing to copy".into(),
        },
    };
    ws.update(cx, |_, cx| {
        cx.emit(WorkspaceEvent::Toast {
            message,
            undo: None,
        })
    });
}

// ───────────────────────────── items ─────────────────────────────

/// A menu row with an id (the verify driver clicks by id) and, at its right, the keys that do
/// the same.
fn entry(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    shortcut: Option<Id>,
) -> PopupMenuItem {
    let id: SharedString = id.into();
    let label: SharedString = label.into();
    let hint = shortcut.map(keys::hint).filter(|h| !h.is_empty());
    PopupMenuItem::element(move |_, cx| {
        h_flex()
            .id(id.clone())
            .test_support()
            .w_full()
            .gap_4()
            .justify_between()
            .child(label.clone())
            .when_some(hint.clone(), |el, hint| {
                el.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(hint),
                )
            })
    })
}

pub fn open_in_new_window(ws: &WeakEntity<Workspace>, id: &str) -> PopupMenuItem {
    let (ws, id) = (ws.clone(), id.to_string());
    entry(
        "thread-open-window",
        "Open in new window",
        Some(Id::OpenInNewWindow),
    )
    .icon(crate::assets::Lucide::SquareArrowOutUpRight)
    .on_click(move |_, _, cx| {
        if let Some(ws) = ws.upgrade() {
            crate::thread_window::open(ws, &id, cx);
        }
    })
}

pub fn fork(ws: &WeakEntity<Workspace>, id: &str) -> PopupMenuItem {
    let (ws, id) = (ws.clone(), id.to_string());
    entry("thread-fork", "Fork thread", None)
        .icon(crate::assets::Lucide::GitFork)
        .on_click(move |_, _, cx| {
            let _ = ws.update(cx, |ws, cx| {
                ws.fork_thread(
                    &id,
                    crate::workspace::ForkAt::End,
                    &crate::workspace::Scope::Main,
                    cx,
                )
            });
        })
}

pub fn pin(ws: &WeakEntity<Workspace>, f: &Facts) -> PopupMenuItem {
    let (ws, id) = (ws.clone(), f.id.clone());
    entry(
        "thread-pin",
        if f.pinned {
            "Unpin thread"
        } else {
            "Pin thread"
        },
        None,
    )
    .icon(if f.pinned {
        crate::assets::Lucide::PinOff
    } else {
        crate::assets::Lucide::Pin
    })
    .on_click(move |_, _, cx| {
        let _ = ws.update(cx, |ws, cx| ws.toggle_pin(&id, cx));
    })
}

pub fn rename(ws: &WeakEntity<Workspace>, f: &Facts) -> PopupMenuItem {
    let (ws, id, title) = (ws.clone(), f.id.clone(), f.title.clone());
    entry("thread-rename", "Rename thread", None)
        .icon(crate::assets::Lucide::Pencil)
        .on_click(move |_, window, cx| {
            if let Some(ws) = ws.upgrade() {
                open_rename_dialog(ws, id.clone(), title.clone(), window, cx);
            }
        })
}

/// Archive: a thread in a worktree asks whether its worktree goes too, and one with sub-agents
/// says they go with it.
pub fn archive(ws: &WeakEntity<Workspace>, f: &Facts) -> PopupMenuItem {
    let (ws, id, title) = (ws.clone(), f.id.clone(), f.title.clone());
    if f.in_worktree {
        return entry("thread-archive", "Archive thread…", None)
            .icon(crate::assets::Lucide::Archive)
            .on_click(move |_, window, cx| {
                if let Some(ws) = ws.upgrade() {
                    crate::worktree_ui::confirm_leave(ws, id.clone(), Leave::Archive, window, cx);
                }
            });
    }
    match f.archive_note.clone() {
        None => entry("thread-archive", "Archive thread", None)
            .icon(crate::assets::Lucide::Archive)
            .on_click(move |_, _, cx| {
                let _ = ws.update(cx, |ws, cx| ws.archive(&id, cx));
            }),
        Some(note) => entry("thread-archive", "Archive thread…", None)
            .icon(crate::assets::Lucide::Archive)
            .on_click(move |_, window, cx| {
                let (ws, id, title, note) = (ws.clone(), id.clone(), title.clone(), note.clone());
                window.open_alert_dialog(cx, move |alert, _, _| {
                    let (ws, id) = (ws.clone(), id.clone());
                    alert
                        .title(format!("Archive “{title}”?"))
                        .description(note.clone())
                        .confirm()
                        .ok_text("Archive")
                        .on_ok(move |_, _, cx| {
                            let _ = ws.update(cx, |ws, cx| ws.archive(&id, cx));
                            true
                        })
                });
            }),
    }
}

/// The Copy submenu.
pub fn copy_submenu(
    menu: PopupMenu,
    ws: &WeakEntity<Workspace>,
    f: &Facts,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let (ws, f) = (ws.clone(), f.clone());
    menu.submenu_with_icon(
        Some(IconName::Copy.into()),
        "Copy",
        window,
        cx,
        move |mut menu, _, _| {
            for what in Copy::ALL {
                // Only what the thread has.
                let has = match what {
                    Copy::Folder => f.cwd.is_some(),
                    Copy::Resume => f.resume.is_some(),
                    _ => true,
                };
                if !has {
                    continue;
                }
                if what == Copy::Folder {
                    menu = menu.separator();
                }
                let (ws, id) = (ws.clone(), f.id.clone());
                menu = menu.item(
                    entry(what.element_id(), what.label(), what.shortcut()).on_click(
                        move |_, _, cx| {
                            if let Some(ws) = ws.upgrade() {
                                copy(&ws, &id, what, cx);
                            }
                        },
                    ),
                );
            }
            menu.min_w(px(250.))
        },
    )
}

/// The Open in submenu: the file manager, a terminal and the editors installed.
fn open_in_submenu(
    menu: PopupMenu,
    dir: PathBuf,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    menu.submenu_with_icon(
        Some(IconName::FolderOpen.into()),
        "Open in",
        window,
        cx,
        move |menu, _, _| open_in_items(menu.min_w(px(180.)), dir.clone()),
    )
}

/// The "Open in" targets for `dir`, as items of `menu`. On a Mac the apps are launched through
/// `open -a`; on Windows each is found through the registry and PATH (`trek_core::open_in`) and
/// started with the folder as its argument. Checked when the menu opens, not on every frame.
pub fn open_in_items(mut menu: PopupMenu, dir: PathBuf) -> PopupMenu {
    if cfg!(windows) {
        let folder = dir.clone();
        menu = menu.item(
            PopupMenuItem::new(crate::words::words().file_manager)
                .on_click(move |_, _, cx| cx.open_with_system(&folder)),
        );
        for target in trek_core::open_in::windows_targets() {
            let (label, dir) = (target.label, dir.clone());
            menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, _| {
                let Some(launch) = target.launch(&dir) else {
                    return tracing::warn!(
                        "open in {label}: {} isn't an absolute path",
                        dir.display()
                    );
                };
                if let Err(e) = trek_core::open_in::spawn(&launch) {
                    tracing::warn!("open in {label}: {e}");
                }
            }));
        }
        return menu;
    }
    let apps = [
        (crate::words::words().file_manager, "Finder"), // words: ok, the app's own name for `open -a`
        ("Terminal", "Terminal"),
        ("Ghostty", "Ghostty"),
        ("Zed", "Zed"),
        ("Cursor", "Cursor"),
        ("VS Code", "Visual Studio Code"),
        ("Xcode", "Xcode"),
    ]
    .into_iter()
    .filter(|(_, app)| {
        matches!(*app, "Finder" | "Terminal") // words: ok
            || std::path::Path::new(&format!("/Applications/{app}.app")).exists()
    });
    for (label, app) in apps {
        let dir = dir.clone();
        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, _| {
            let _ = std::process::Command::new("/usr/bin/open")
                .arg("-a")
                .arg(app)
                .arg(&dir)
                .spawn();
        }));
    }
    menu
}

/// "Rename thread": a dialog with the title selected; Enter or Rename saves it.
pub fn open_rename_dialog(
    ws: Entity<Workspace>,
    id: String,
    title: String,
    window: &mut Window,
    cx: &mut App,
) {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("Thread title"));
    input.update(cx, |s, cx| s.set_value(title, window, cx));
    // Enter saves, as the button does.
    {
        let (ws, id) = (ws.clone(), id.clone());
        window
            .subscribe(&input, cx, move |input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let title = input.read(cx).value().to_string();
                    ws.update(cx, |ws, cx| ws.rename(&id, title, cx));
                    window.close_dialog(cx);
                }
            })
            .detach();
    }
    // The cursor in the field once the dialog is up, with the old title selected.
    let focus = input.clone();
    window.defer(cx, move |window, cx| {
        focus.update(cx, |s, cx| {
            s.focus(window, cx);
            let len = s.value().len();
            s.set_selected_range(0..len, cx);
        })
    });
    window.open_dialog(cx, move |dialog, _, _| {
        let (input2, ws, id) = (input.clone(), ws.clone(), id.clone());
        dialog
            .title("Rename thread")
            .w(px(440.))
            .child(Input::new(&input))
            .footer(
                gpui_kit::component::dialog::DialogFooter::new()
                    .gap_2()
                    .child(
                        gpui_kit::component::dialog::DialogClose::new()
                            .child(Button::new("cancel-rename").outline().label("Cancel")),
                    )
                    .child(gpui_kit::component::dialog::DialogAction::new().child(
                        Button::new("do-rename").primary().label("Rename").on_click(
                            move |_, _, cx| {
                                let title = input2.read(cx).value().to_string();
                                ws.update(cx, |ws, cx| ws.rename(&id, title, cx));
                            },
                        ),
                    )),
            )
    });
}

// ───────────────────────────── the title bar's menu ─────────────────────────────

/// Where the "⋯" button is: the main window (which can open the thread in another) or the
/// thread's own window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Main,
    OwnWindow,
}

/// The title bar's "⋯": Share…, Copy, where else the thread can go, and what can be done to it.
pub fn button(ws: Entity<Workspace>, id: String, place: Place) -> impl IntoElement {
    let weak = ws.downgrade();
    crate::ui::icon_button("thread-menu", IconName::Ellipsis, "Thread")
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, cx| {
            build(menu, &weak, &id, place, window, cx)
        })
}

fn build(
    menu: PopupMenu,
    weak: &WeakEntity<Workspace>,
    id: &str,
    place: Place,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(ws) = weak.upgrade() else {
        return menu;
    };
    let Some(f) = Facts::read(ws.read(cx), id) else {
        return menu;
    };
    let mut menu = menu.min_w(px(240.)).item(
        entry("thread-share", "Share…", None)
            .icon(crate::assets::Lucide::Share)
            .on_click({
                let (ws, id) = (ws.clone(), f.id.clone());
                move |_, window, cx| crate::share::open(ws.clone(), &id, window, cx)
            }),
    );
    menu = copy_submenu(menu, weak, &f, window, cx);
    menu = menu.separator();
    if place == Place::Main {
        menu = menu.item(open_in_new_window(weak, &f.id));
    }
    if let Some(dir) = f.cwd.clone().filter(|_| !f.remote) {
        menu = open_in_submenu(menu, dir, window, cx);
    }
    // Where else it can go on: the phone, and each paired machine (not the one it's on).
    menu = menu.item(
        entry("thread-to-phone", "Continue on iPhone", None)
            .icon(crate::assets::Lucide::Smartphone)
            .on_click({
                let (ws, id) = (ws.clone(), f.id.clone());
                move |_, _, cx| ws.update(cx, |ws, cx| ws.send_thread_to_phone(&id, cx))
            }),
    );
    let on = crate::machines::split_id(&f.id).map(|(host, _)| host.to_string());
    let machines: Vec<(String, String, bool)> = ws
        .read(cx)
        .machines
        .list
        .iter()
        .filter(|m| on.as_deref() != Some(m.id()))
        .map(|m| (m.id().to_string(), m.entry.name.clone(), m.reachable()))
        .collect();
    for (host, name, reachable) in machines {
        let item_id = SharedString::from(format!("thread-to-machine-{host}"));
        let (ws, id) = (ws.clone(), f.id.clone());
        menu = menu.item(
            entry(item_id, format!("Continue on {name}"), None)
                .icon(reach_icon(reachable, cx))
                .on_click(move |_, _, cx| {
                    let markdown = ws.read(cx).transcript_markdown(&id);
                    ws.update(cx, |ws, cx| {
                        ws.continue_on_machine(&host, &id, &markdown, cx)
                    })
                }),
        );
    }
    menu.separator()
        .item(fork(weak, &f.id))
        .item(rename(weak, &f))
        .item(pin(weak, &f))
        .item(archive(weak, &f))
}

/// A machine's reachability dot as a menu icon, so it sits in the icons' column: green while
/// it answers, quiet when it doesn't (`composer::reach_dot`'s colours).
fn reach_icon(reachable: bool, cx: &App) -> gpui_kit::component::Icon {
    const DOT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="5.5" fill="currentColor"/></svg>"#;
    gpui_kit::component::Icon::default()
        .data(DOT)
        .text_color(if reachable {
            crate::palette::emerald(cx)
        } else {
            cx.theme().foreground.opacity(0.22)
        })
}

/// The thread the main window acts on (the route's, or the editor's side bar chat), if any.
pub fn focused(ws: &Entity<Workspace>, cx: &App) -> Option<String> {
    ws.read(cx).focused_thread().map(str::to_string)
}
