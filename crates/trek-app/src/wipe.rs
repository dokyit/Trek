//! Settings › General › "Delete all Trek data…": the confirmation, which says what goes and what
//! stays, and the deleting, after which Trek quits (the next launch is a first run).
//!
//! What goes is worked out by `trek_core::wipe::plan` from the folders Trek writes, each as the
//! function that writes it resolves it (`paths::data_dir`, `logging::log_dir`, …), never a path
//! put together here; the folders inside them that stay are named the same way. What stays: the
//! user's projects, Trek's worktrees of them (their work, on `trek/…` branches), threads' folders
//! without a project, the agents' own data and sign-ins (an agent's second sign-in Trek keeps
//! included), switched-off skills, and anything of another profile's.
//!
//! Trek quits rather than starting again: what's running would otherwise go on writing what was
//! just deleted, and someone deleting everything is often about to remove the app. Before it
//! deletes, turns are stopped, connected phones are told they're unpaired (as Unpair does) and
//! the phone server stops; after, the process `leave`s its profile, so what is saved on the way
//! out goes to a throwaway folder, and the same plan runs once more as Trek quits.
//!
//! Only this page starts it: no MCP tool, remote request or `trek://` link reaches `confirm` or
//! `run` (`tests/wipe.rs` checks every caller).

use crate::workspace::Workspace;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, v_flex};
use gpui_kit::*;
use trek_core::paths;
use trek_core::wipe::{Keep, Plan, Request, Root, Secrets};

/// What the user types to confirm.
pub const CONFIRM_WORD: &str = "delete";

/// The folders this profile has, what stays in them, and its secrets, as Trek resolves them.
pub fn request(ws: &Workspace) -> Request {
    let root = |path: std::path::PathBuf, what: &str| Root {
        path,
        what: what.to_string(),
    };
    let keep = |path: std::path::PathBuf, what: &str| Keep {
        path,
        what: what.to_string(),
    };
    let mut roots = vec![root(
        paths::data_dir(),
        "Threads and their transcripts, settings, notes, pictures, uploads from phones, pairings, plugins and themes, and what Trek downloaded (its Node.js, agents, the browser's extensions)",
    )];
    roots.push(root(
        paths::local_data_dir(),
        "Downloads kept on this computer only (the video tools, the speech model)",
    ));
    // Windows keeps its web views' profiles beside the logs (`browser::webview_data_dir`).
    #[cfg(windows)]
    {
        let logs = crate::logging::log_dir();
        if let (false, Some(beside)) = (paths::moved(), logs.parent()) {
            roots.push(root(
                beside.to_path_buf(),
                "Logs, and the Browser's cookies, site data and extensions",
            ));
        }
    }
    roots.push(root(
        crate::logging::log_dir(),
        "Logs, freeze and crash notes",
    ));
    roots.push(root(
        paths::cache_dir(),
        "Caches: videos' frames, pictures' previews, web pages",
    ));
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut files = vec![];
    // WebKit keeps the Browser's data by the app's bundle id, in folders of the system's naming.
    #[cfg(target_os = "macos")]
    if let (false, Some(id)) = (
        paths::moved(),
        bundle_id().filter(|id| id == "dev.trek.Trek"),
    ) {
        let library = paths::home().join("Library");
        roots.push(root(
            library.join("WebKit").join(&id),
            "The Browser's site data and extensions",
        ));
        roots.push(root(
            library.join("HTTPStorages").join(&id),
            "The Browser's cookies",
        ));
        roots.push(root(
            library
                .join("Saved Application State")
                .join(format!("{id}.savedState")),
            "Where Trek's windows were",
        ));
        files.push(root(
            library
                .join("HTTPStorages")
                .join(format!("{id}.binarycookies")),
            "The Browser's cookies",
        ));
        files.push(root(
            library.join("Preferences").join(format!("{id}.plist")),
            "The system's preferences file for Trek",
        ));
    }
    let mut kept = vec![
        keep(
            trek_core::worktree::worktrees_dir(),
            "Worktrees Trek made for threads: your work, on their trek/… branches. Remove one in the Git tool, or with git worktree remove",
        ),
        keep(
            trek_core::accounts::root(),
            "Second sign-ins to Claude Code or Codex added in Trek: the agents' own logins and history",
        ),
        keep(
            trek_core::skills::disabled_root(),
            "Skills of yours switched off in Trek",
        ),
        keep(
            paths::chats_dir(),
            "Folders of threads without a project, with what the agents made in them",
        ),
        keep(trek_core::study::dir(), "Your study folder"),
    ];
    let agents_home = paths::agents_home();
    if agents_home.starts_with(paths::data_dir()) {
        kept.push(keep(agents_home, "The agents' own setup, for this profile"));
    }
    Request {
        home: paths::home(),
        roots,
        files,
        keep: kept,
        secrets: secrets(ws),
    }
}

/// The secret store's accounts this profile has: every one under Trek's service where the store
/// lists them (the Keychain; an isolated process's memory), and those its settings name (Windows'
/// and Linux's stores can't list). None for a profile in another folder that uses the user's real
/// store: those keys are the user's own Trek's too.
fn secrets(ws: &Workspace) -> Secrets {
    if paths::moved() && !paths::isolated() {
        return Secrets::Keep(
            "Its keys stay: this profile's data folder is elsewhere, and it shares the secret store with Trek's usual one.".into(),
        );
    }
    use trek_core::settings::secrets::{mcp_account, mcp_env_account};
    let s = &ws.settings;
    let mut named: Vec<String> = trek_core::catalog::DIRECT_PROVIDERS
        .iter()
        .map(|p| p.id.to_string())
        .collect();
    for server in &s.tools.mcp_servers {
        named.extend(
            server
                .headers
                .iter()
                .filter(|h| h.secret)
                .map(|h| mcp_account(&server.name, &h.name)),
        );
        named.extend(
            server
                .env
                .iter()
                .filter(|e| e.secret)
                .map(|e| mcp_env_account(&server.name, &e.name)),
        );
    }
    for agent in &s.added_agents {
        named.extend(
            agent
                .env
                .iter()
                .filter(|e| e.secret)
                .map(|e| trek_core::registry::secret_account(&agent.id, &e.name)),
        );
    }
    named.push(trek_core::plugins::grant::KEY_ACCOUNT.to_string());
    // Tests share one process's memory store: only what this profile's settings name.
    if !cfg!(any(test, feature = "test-support")) {
        named.extend(trek_core::keychain::accounts().unwrap_or_default());
    }
    Secrets::Delete(named)
}

#[cfg(target_os = "macos")]
fn bundle_id() -> Option<String> {
    objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|id| id.to_string())
}

/// The confirmation: what goes, what stays, and a word to type before Delete is offered. Cancel
/// is the first button and Escape closes it; Return in the field does nothing.
pub fn confirm(ws: Entity<Workspace>, window: &mut Window, cx: &mut App) {
    let plan = trek_core::wipe::plan(&request(ws.read(cx)));
    let running = {
        let ws = ws.read(cx);
        ws.live.keys().filter(|id| ws.turn_running(id)).count()
    };
    let input = cx.new(|cx| {
        InputState::new(window, cx).placeholder(format!("Type {CONFIRM_WORD} to confirm"))
    });
    window
        .subscribe(&input, cx, |_, event: &InputEvent, window, _| {
            if matches!(event, InputEvent::Change) {
                window.refresh();
            }
        })
        .detach();
    let focus = input.clone();
    window.defer(cx, move |window, cx| {
        focus.update(cx, |s, cx| s.focus(window, cx))
    });
    let lines = Lines::of(&plan, running);
    window.open_dialog(cx, move |dialog, _, cx| {
        let typed = input
            .read(cx)
            .value()
            .trim()
            .eq_ignore_ascii_case(CONFIRM_WORD);
        let (ws, input2) = (ws.clone(), input.clone());
        let theme = cx.theme().clone();
        let section = |title: &str, items: &[String]| {
            v_flex()
                .gap(px(4.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.to_string()),
                )
                .children(items.iter().map(|i| {
                    div()
                        .text_color(theme.muted_foreground)
                        .child(format!("• {i}"))
                }))
        };
        dialog
            .title("Delete all Trek data?")
            .w(px(560.))
            .child(
                v_flex()
                    .gap(px(14.))
                    .text_size(px(crate::scale::text::BODY))
                    .child(
                        v_flex()
                            .id("wipe-confirm")
                            .test_support()
                            .gap(px(14.))
                            .max_h(px(400.))
                            .overflow_y_scroll()
                            .child(div().child(lines.intro.clone()))
                            .child(section("Deleted", &lines.goes))
                            .child(section("Kept", &lines.stays)),
                    )
                    // Under the list, always in view.
                    .child(Input::new(&input).id("wipe-word")),
            )
            .footer(
                gpui_kit::component::dialog::DialogFooter::new()
                    .gap_2()
                    .child(
                        gpui_kit::component::dialog::DialogClose::new()
                            .child(Button::new("wipe-cancel").outline().label("Cancel")),
                    )
                    .child(
                        gpui_kit::component::dialog::DialogAction::new().child(
                            Button::new("wipe-delete")
                                .danger()
                                .label("Delete and quit")
                                .disabled(!typed)
                                .on_click(move |_, window, cx| {
                                    let typed = input2
                                        .read(cx)
                                        .value()
                                        .trim()
                                        .eq_ignore_ascii_case(CONFIRM_WORD);
                                    if typed {
                                        run(ws.clone(), window, cx);
                                    }
                                }),
                        ),
                    ),
            )
    });
}

/// The confirmation's words, from the plan.
#[derive(Clone)]
struct Lines {
    intro: String,
    goes: Vec<String>,
    stays: Vec<String>,
}

impl Lines {
    fn of(plan: &Plan, running: usize) -> Lines {
        let this = crate::words::words().this_computer;
        let mut intro = format!(
            "Trek deletes everything it keeps for you on {this}, then quits. The next time you open it, it starts as new. This can't be undone."
        );
        match running {
            0 => {}
            1 => intro.push_str(" The thread that's running is stopped first."),
            n => intro.push_str(&format!(
                " The {n} threads that are running are stopped first."
            )),
        }
        let mut goes: Vec<String> = plan
            .listed
            .iter()
            .map(|r| format!("{} ({})", r.what, paths::tildify(&r.path)))
            .collect();
        match &plan.secrets_kept {
            None => goes.push(format!(
                "API keys and tokens Trek saved in {}",
                crate::words::words().your_credential_store
            )),
            Some(_) => {}
        }
        goes.push("Paired phones and machines: a phone connected now is told it's unpaired; any other is turned away when it next connects".into());
        let mut stays = vec![
            "Your projects and every file in them".to_string(),
            "Claude Code's, Codex's and the other agents' own history and sign-ins".to_string(),
            "Trek itself".to_string(),
        ];
        stays.extend(
            plan.kept
                .iter()
                .map(|k| format!("{} ({})", k.what, paths::tildify(&k.path))),
        );
        if let Some(why) = &plan.secrets_kept {
            stays.push(why.clone());
        }
        stays.extend(
            plan.refused
                .iter()
                .map(|(p, why)| format!("{}: not deleted, as {why}", paths::tildify(p))),
        );
        Lines { intro, goes, stays }
    }
}

/// Delete everything the plan lists and quit: running turns stop, phones are told, the phone
/// server stops; then the deleting, off the main thread; then Trek quits (or, when something
/// couldn't go, says what and where first).
pub fn run(ws: Entity<Workspace>, window: &mut Window, cx: &mut App) {
    window.close_dialog(cx);
    ws.update(cx, |ws, cx| ws.wind_down_for_wipe(cx));
    let handle = window.window_handle();
    let ws2 = ws.clone();
    cx.spawn(async move |cx| {
        // Long enough for a stopped agent to end and stop writing.
        cx.background_executor()
            .timer(std::time::Duration::from_millis(1500))
            .await;
        let plan = ws2.update(cx, |ws, cx| {
            let plan = trek_core::wipe::plan(&request(ws));
            ws.leave_profile_for_wipe(cx);
            plan
        });
        let kept = plan.clone();
        let outcome = cx
            .background_executor()
            .spawn(async move { trek_core::wipe::execute(&kept, forget_secret) })
            .await;
        ws2.update(cx, |ws, cx| {
            ws.wiped = Some((plan, outcome.clone()));
            cx.notify();
        });
        if cfg!(any(test, feature = "test-support")) {
            return;
        }
        cx.update(|cx| {
            if outcome.left.is_empty() && outcome.secrets_left.is_empty() {
                cx.quit();
                return;
            }
            let _ = handle.update(cx, |_, window, cx| left_behind(&outcome, window, cx));
        });
    })
    .detach();
}

/// Delete a secret; one that still reads as there (another build of Trek's, which macOS won't let
/// this one delete) is an error.
fn forget_secret(account: &str) -> anyhow::Result<()> {
    trek_core::keychain::delete(account)?;
    anyhow::ensure!(
        trek_core::keychain::read(account) == trek_core::keychain::Lookup::Missing,
        "still saved"
    );
    Ok(())
}

/// What couldn't be deleted, and where, before Trek quits.
fn left_behind(outcome: &trek_core::wipe::Outcome, window: &mut Window, cx: &mut App) {
    let mut items: Vec<String> = outcome
        .left
        .iter()
        .map(|(p, why)| format!("{} ({why})", paths::tildify(p)))
        .collect();
    if !outcome.secrets_left.is_empty() {
        items.push(format!(
            "{} keys another build of Trek saved in {}, which this one may not delete: remove them there (search for dev.trek.Trek)",
            outcome.secrets_left.len(),
            crate::words::words().your_credential_store
        ));
    }
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert
            .title("Some of Trek's data is still there")
            .description(format!(
                "Everything else is deleted. These were in use or not Trek's to delete; Trek tries them again as it quits, and you can delete what's left yourself:\n\n{}",
                items.join("\n")
            ))
            .ok_text("Quit Trek")
            .on_ok(|_, _, cx| {
                cx.quit();
                true
            })
    });
}

impl Workspace {
    /// Before deleting: stop every running turn, tell connected phones they're unpaired (as
    /// Unpair does) and stop the phone server, and let go of the machines this one drives.
    pub fn wind_down_for_wipe(&mut self, cx: &mut Context<Self>) {
        let running: Vec<String> = self
            .live
            .keys()
            .filter(|id| self.turn_running(id))
            .cloned()
            .collect();
        for id in running {
            self.interrupt(&id, cx);
        }
        for device in self.paired_devices() {
            self.revoke_device(&device.device_id, cx);
        }
        self.settings.mobile.enabled = false;
        self.sync_remote(cx);
        let machines: Vec<String> = self
            .machines
            .list
            .iter()
            .map(|m| m.id().to_string())
            .collect();
        for host in machines {
            self.forget_machine(&host, cx);
        }
        cx.notify();
    }

    /// Right before deleting: nothing more is logged to files, counted or kept in the session's
    /// secrets folder, the database file is let go of, and (outside tests) the process leaves its profile
    /// for a throwaway folder, so what's saved on the way out doesn't make it again. As Trek
    /// quits, the same plan runs once more for anything written meanwhile.
    pub fn leave_profile_for_wipe(&mut self, cx: &mut Context<Self>) {
        trek_core::session_secrets::forget_all();
        if cfg!(any(test, feature = "test-support")) {
            return;
        }
        // Resolved while the profile is still this one.
        let request = request(self);
        trek_telemetry::stop();
        crate::logging::stop_files();
        crate::logging::freeze::stop();
        let throwaway = std::env::temp_dir().join(format!("trek-deleted-{}", std::process::id()));
        paths::leave(throwaway.clone());
        if let Err(e) = self.store.close() {
            tracing::warn!("delete all data: the database stays open: {e:#}");
        }
        cx.on_app_quit(move |_, _| {
            let plan = trek_core::wipe::plan(&request);
            let throwaway = throwaway.clone();
            async move {
                trek_core::wipe::execute(&plan, |_| Ok(()));
                let _ = std::fs::remove_dir_all(&throwaway);
            }
        })
        .detach();
    }
}
