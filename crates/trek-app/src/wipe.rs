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
//! What the system runs for Trek on a schedule is taken off the schedule first (`jobs`): the Mac's
//! iPhone updater (a LaunchAgent, which stays loaded until the next login when only its file
//! goes) and Windows' "Update while closed" task. Linux has neither.
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

/// The iPhone updater's LaunchAgent, by its label (`ios/scripts/install-phone-update.sh`); its
/// file is `~/Library/LaunchAgents/<label>.plist`.
pub const PHONE_UPDATE_AGENT: &str = "dev.trek.phone-update";

/// Something of Trek's the system runs on a schedule, outside Trek: deleting its files alone
/// leaves it on the schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// macOS: a LaunchAgent loaded for this user, by its label.
    LaunchAgent(String),
    /// Windows: the user's task in Task Scheduler, by its name.
    ScheduledTask(String),
}

impl Job {
    /// What it is, for the confirmation and for what couldn't go.
    pub fn what(&self) -> String {
        match self {
            Job::LaunchAgent(label) => format!("The iPhone updater's schedule ({label})"),
            Job::ScheduledTask(name) => {
                format!("The task that updates Trek while it's closed (Task Scheduler › {name})")
            }
        }
    }
}

/// The scheduled jobs this profile's Trek has, to take off the schedule before its files go.
/// None for a test's or the verify app's profile: the jobs are the user's own Trek's.
pub fn jobs() -> Vec<Job> {
    jobs_of(paths::isolated() || paths::moved())
}

/// `jobs`, for a profile that is the user's own or `elsewhere`.
fn jobs_of(elsewhere: bool) -> Vec<Job> {
    if elsewhere {
        return vec![];
    }
    #[cfg(target_os = "macos")]
    {
        vec![Job::LaunchAgent(PHONE_UPDATE_AGENT.to_string())]
    }
    #[cfg(windows)]
    {
        vec![Job::ScheduledTask(
            crate::workspace::update_task_name().to_string(),
        )]
    }
    // Linux: Trek sets up nothing that runs by itself (no systemd unit, no autostart entry).
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        vec![]
    }
}

/// Take `job` off the system's schedule, ending a run of it under way. One that isn't there is
/// fine; an error is why it's still there.
fn stop(job: &Job) -> Result<(), String> {
    match job {
        #[cfg(target_os = "macos")]
        Job::LaunchAgent(label) => {
            // SAFETY: getuid has no failure and touches nothing.
            let uid = unsafe { libc::getuid() };
            let target = format!("gui/{uid}/{label}");
            let launchctl = |verb: &str| {
                trek_core::procs::command("/bin/launchctl")
                    .args([verb, &target])
                    .output()
            };
            let out = launchctl("bootout").map_err(|e| format!("launchctl didn't run: {e}"))?;
            // It fails for one that isn't loaded, which is all that's wanted: what counts is
            // whether launchd still has it.
            match launchctl("print") {
                Ok(still) if still.status.success() => Err(format!(
                    "launchd still has it: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )),
                _ => Ok(()),
            }
        }
        #[cfg(windows)]
        Job::ScheduledTask(name) => {
            trek_core::update::closed::task::unregister(name).map_err(|e| format!("{e:#}"))
        }
        // The other platforms' jobs: nothing of this one's to stop.
        #[allow(unreachable_patterns)]
        _ => Ok(()),
    }
}

/// `stop` each of `jobs`; those still there, as `Outcome::left` lists what couldn't go.
fn stop_jobs(jobs: &[Job]) -> Vec<(std::path::PathBuf, String)> {
    jobs.iter()
        .filter_map(|job| {
            let why = stop(job).err()?;
            tracing::warn!("delete all data: {}: {why}", job.what());
            Some((std::path::PathBuf::from(job.what()), why))
        })
        .collect()
}

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
    // A Mac's data folder also holds the passkeys its Browser keeps (`trek_passkeys::Store`):
    // deleted here, their Secure Enclave keys can't be used again.
    let data = if cfg!(target_os = "macos") {
        "Threads and their transcripts, settings, notes, pictures, uploads from phones, pairings, the Browser's passkeys, plugins and themes, and what Trek downloaded (its Node.js, agents, the browser's extensions)"
    } else {
        "Threads and their transcripts, settings, notes, pictures, uploads from phones, pairings, plugins and themes, and what Trek downloaded (its Node.js, agents, the browser's extensions)"
    };
    let mut roots = vec![root(paths::data_dir(), data)];
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
    // Any Trek bundle id counts (a dev or nightly build), not only the release one.
    #[cfg(target_os = "macos")]
    if let (false, Some(id)) = (
        paths::moved(),
        bundle_id().filter(|id| id.to_ascii_lowercase().contains("trek")),
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
    // The iPhone updater runs from a LaunchAgent of its own, outside the data folder: its
    // folder (a copy of the script, and the iPhone's name and version), the agent, and its log.
    // The agent is unloaded before its file goes (`jobs`).
    #[cfg(target_os = "macos")]
    if !paths::isolated() {
        let home = paths::home();
        roots.push(root(
            home.join("Library/Application Support/dev.trek.phone-update"),
            "The iPhone updater's folder: its copy of the script, and what it last installed",
        ));
        files.push(root(
            home.join(format!("Library/LaunchAgents/{PHONE_UPDATE_AGENT}.plist")),
            "The iPhone updater's schedule",
        ));
        files.push(root(
            home.join("Library/Logs/trek-phone-update.log"),
            "The iPhone updater's log",
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
    // The Mac's updater is listed by its file; Windows' task has none.
    let tasks: Vec<String> = jobs()
        .iter()
        .filter(|job| matches!(job, Job::ScheduledTask(_)))
        .filter(|_| ws.read(cx).settings.updates.while_closed)
        .map(Job::what)
        .collect();
    let lines = Lines::of(&plan, running, &tasks);
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
    /// `tasks`: the scheduled jobs that go and have no file in the plan to stand for them.
    fn of(plan: &Plan, running: usize, tasks: &[String]) -> Lines {
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
        goes.extend(tasks.iter().cloned());
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
        let (plan, jobs) = ws2.update(cx, |ws, cx| {
            let plan = trek_core::wipe::plan(&request(ws));
            // Worked out while the profile is still this one.
            let jobs = jobs();
            ws.leave_profile_for_wipe(cx);
            (plan, jobs)
        });
        let kept = plan.clone();
        let outcome = cx
            .background_executor()
            .spawn(async move {
                // Off the schedule before their files go: a LaunchAgent whose file is gone can
                // no longer be found by it, and stays loaded until the next login.
                let still = stop_jobs(&jobs);
                let mut outcome = trek_core::wipe::execute(&kept, forget_secret);
                outcome.left.extend(still);
                outcome
            })
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

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's `test` attribute would come with it.
    use super::{Job, Lines, PHONE_UPDATE_AGENT, jobs_of};

    /// Deleting everything takes Trek's scheduled jobs off the system's schedule, not only
    /// their files: the Mac's iPhone updater, Windows' update task. Never another profile's.
    #[test]
    fn scheduled_jobs_are_stopped_for_the_users_own_profile_only() {
        assert_eq!(jobs_of(true), []);
        let jobs = jobs_of(false);
        #[cfg(target_os = "macos")]
        assert_eq!(jobs, [Job::LaunchAgent("dev.trek.phone-update".into())]);
        #[cfg(windows)]
        assert_eq!(
            jobs,
            [Job::ScheduledTask(
                crate::workspace::update_task_name().to_string()
            )]
        );
        #[cfg(not(any(target_os = "macos", windows)))]
        assert_eq!(jobs, []);
        // The label the installer loads it under, which is also its file's name.
        assert_eq!(PHONE_UPDATE_AGENT, "dev.trek.phone-update");
    }

    /// The confirmation names a scheduled task that goes: it has no file to be listed by.
    #[test]
    fn the_confirmation_lists_a_task_that_goes() {
        let task = Job::ScheduledTask(r"Trek\Update while closed".into()).what();
        let lines = Lines::of(&Default::default(), 0, std::slice::from_ref(&task));
        assert!(lines.goes.contains(&task), "{:?}", lines.goes);
        assert!(task.contains("Update while closed"));
    }
}
