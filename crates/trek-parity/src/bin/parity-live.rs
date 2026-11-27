//! Runs the ledger's live steps (`live` in `docs/parity/ledger.toml`) against the capture build
//! running on this computer, one feature at a time, and says how each went: the same steps on a
//! Mac and on a PC, so a feature shown working on one is shown working, or not, on the other.
//!
//! Each feature's steps go to the app as one batch through its command folder, as the verify
//! driver sends them (`cmd` written whole, then `done`: `ok`, or `err <command>: <why>` for each
//! that failed), after a few that bring the app back to a new thread's composer, and before a
//! screenshot (`live-<id>.png` in that folder). Start the app with the driver first; `status`
//! names its runtime folder, and the commands' folder is its `shot` folder:
//!
//! ```sh
//! ./.agents/skills/verify-trek/scripts/trek-dev start            # pwsh trek-dev.ps1 on Windows
//! cargo run -p trek-parity --bin parity-live -- --shot-dir <runtime folder>/shot \
//!     [--feature <id>]... [--skip <id>=<why>]... [--report docs/parity/live-<platform>.md] [--timeout <seconds>] [--ci]
//! ```
//!
//! `--ci`: this computer is CI's, made for the run, so the features whose steps run only there
//! (`live_ci`: they open another app, listen on the network, bring a window to the front) run too.
//!
//! It exits 1 when a feature's steps failed, 2 when it couldn't run.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use trek_parity::{Feature, Ledger, Platform, Support};

/// Before each feature: what was open is closed, and the app is on a new thread's composer, in
/// Agents mode. A route first: a feature that failed partway can leave a page without the mode
/// switch on screen (onboarding, which is a route of its own), and the next feature mustn't fail
/// for it.
const FROM_THE_START: &[&str] = &[
    "key escape",
    "key escape",
    "tools off",
    "route draft",
    "click mode-agents",
    "route draft",
    "wait 250",
];

/// What steps can name, filled in here: `{project}`, a small git project made beside the
/// command folder (a README, a source file with a tab in it, and a crate one folder down whose
/// module is missing, for the language server); `{data}`, the app's data
/// folder; `{/}`, this platform's path separator (an Explorer row's id is its path).
fn fill(step: &str, project: &Path) -> String {
    // The app's data folder is beside the project, in the driver's runtime folder.
    let data = project.parent().unwrap_or(project).join("data");
    step.replace("{project}", &project.to_string_lossy())
        .replace("{data}", &data.to_string_lossy())
        .replace("{/}", std::path::MAIN_SEPARATOR_STR)
}

/// The project steps work in, made the first time: steps that change it change only it.
fn project_beside(shot_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = shot_dir.parent().unwrap_or(shot_dir).join("live-project");
    if dir.join(".git").is_dir() {
        return Ok(dir);
    }
    std::fs::create_dir_all(dir.join("src"))?;
    std::fs::create_dir_all(dir.join("lsp").join("src"))?;
    std::fs::write(
        dir.join("README.md"),
        "# Live project

What parity-live's steps work in.
",
    )?;
    // A crate of its own one folder down, which rust-analyzer finds from the project's root, and
    // in it what it reports by itself, no build needed: a module whose file isn't there. What
    // rust-analyzer's Cargo writes beside it is ignored, so the project's changes stay the steps'.
    std::fs::write(dir.join(".gitignore"), "target/\nCargo.lock\n")?;
    // A file of a kind no app has, for the preview to hand to the one a check names (`file-app`).
    std::fs::write(
        dir.join("preview.trekcheck"),
        "What the preview hands to the file's app.\n",
    )?;
    std::fs::write(
        dir.join("lsp").join("Cargo.toml"),
        "[package]
name = \"lsp\"
version = \"0.1.0\"
edition = \"2021\"
",
    )?;
    std::fs::write(
        dir.join("lsp").join("src").join("lib.rs"),
        "//! What the language server check opens: a module whose file isn't there.

mod missing;
",
    )?;
    std::fs::write(
        dir.join("src").join("main.rs"),
        "fn main() {
	println!(\"hello\");
}
",
    )?;
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["add", "-A"],
        &[
            "-c",
            "user.name=parity-live",
            "-c",
            "user.email=parity-live@example.invalid",
            "commit",
            "-q",
            "-m",
            "Start",
        ],
    ] {
        let ran = std::process::Command::new("git") // spawn: a developer's console tool; trek-parity has no trek-core
            .args(args)
            .current_dir(&dir)
            .status()?;
        if !ran.success() {
            return Err(std::io::Error::other(format!("git {} failed", args[0])));
        }
    }
    Ok(dir)
}

/// The capture build the verify driver started for a command folder: its process (the driver's
/// `state.json` beside the folder names it) and its log (`trek.log` there). Both drivers, the Mac
/// and Linux one and `trek-dev.ps1`, keep them so.
struct App {
    pid: Option<u32>,
    log: PathBuf,
}

impl App {
    fn beside(shot_dir: &Path) -> App {
        let home = shot_dir.parent().unwrap_or(shot_dir);
        let pid = std::fs::read_to_string(home.join("state.json"))
            .ok()
            .and_then(|state| pid_in(&state));
        App {
            pid,
            log: home.join("trek.log"),
        }
    }

    /// Whether its process still runs (`None`: no process to ask about).
    fn alive(&self) -> Option<bool> {
        let pid = self.pid?;
        #[cfg(target_os = "linux")]
        let alive = std::path::Path::new(&format!("/proc/{pid}")).exists();
        #[cfg(all(unix, not(target_os = "linux")))]
        let alive = std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?
            .success();
        #[cfg(windows)]
        let alive = std::process::Command::new("tasklist") // spawn: trek-parity has no trek-core, and runs in a console already
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))?;
        Some(alive)
    }

    /// The end of its log, for a run that stops because the app did.
    fn log_tail(&self, lines: usize) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let all: Vec<&str> = text.lines().collect();
        all[all.len().saturating_sub(lines)..].join("\n")
    }
}

/// The `"pid": <n>` of the driver's state file (JSON, read without a JSON parser: one number).
fn pid_in(state: &str) -> Option<u32> {
    let after = &state[state.find("\"pid\"")? + 5..];
    let after = after.trim_start().strip_prefix(':')?.trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// After how many features in a row the app didn't answer the run stops: by then it has frozen,
/// and every later feature would only wait out its own timeout (a frozen app cost a Linux run
/// its two hours).
const SILENT_IN_A_ROW: usize = 2;

/// How a feature's batch went, as the app answered it (or didn't).
enum Answer {
    Said(Outcome),
    /// No answer within the feature's time, the app still running.
    Silent(Outcome),
    /// The app's process ended while it waited.
    Gone(Outcome),
}

enum Outcome {
    Passed,
    Failed(Vec<String>),
    /// No live steps here, and why.
    Skipped(String),
    /// No live steps here: a person checked it live (who, when and how).
    Person(String),
    /// The feature isn't on this platform.
    NotHere(Support),
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let mut args = std::env::args().skip(1);
    let mut shot_dir = std::env::var_os("TREK_SHOT_DIR").map(PathBuf::from);
    let mut only: Vec<String> = vec![];
    // Features this computer can't run, and why (a CI runner without a codec): said, not run.
    let mut not_here: Vec<(String, String)> = vec![];
    let mut report: Option<PathBuf> = None;
    let mut timeout = Duration::from_secs(90);
    let mut ci = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shot-dir" => shot_dir = args.next().map(PathBuf::from),
            "--feature" => only.extend(args.next()),
            "--skip" => {
                let Some((id, why)) = args.next().and_then(|a| {
                    a.split_once('=')
                        .map(|(i, w)| (i.trim().to_string(), w.trim().to_string()))
                }) else {
                    eprintln!("parity-live: --skip takes <id>=<why>");
                    return 2;
                };
                not_here.push((id, why));
            }
            "--report" => report = args.next().map(PathBuf::from),
            "--ci" => ci = true,
            "--timeout" => {
                let Some(secs) = args.next().and_then(|s| s.parse().ok()) else {
                    eprintln!("parity-live: --timeout takes seconds");
                    return 2;
                };
                timeout = Duration::from_secs(secs);
            }
            other => {
                eprintln!(
                    "parity-live: unknown argument {other} (see the top of src/bin/parity-live.rs)"
                );
                return 2;
            }
        }
    }
    let Some(shot_dir) = shot_dir.filter(|d| d.is_dir()) else {
        eprintln!(
            "parity-live: no capture build's command folder: start one (trek-dev start) and pass its \
             <runtime folder>/shot as --shot-dir"
        );
        return 2;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ledger: Ledger = match std::fs::read_to_string(root.join("docs/parity/ledger.toml"))
        .map_err(|e| e.to_string())
        .and_then(|text| toml::from_str(&text).map_err(|e| e.to_string()))
    {
        Ok(ledger) => ledger,
        Err(e) => {
            eprintln!("parity-live: docs/parity/ledger.toml: {e}");
            return 2;
        }
    };
    let unknown: Vec<&String> = only
        .iter()
        .filter(|id| !ledger.feature.iter().any(|f| &f.id == *id))
        .collect();
    if !unknown.is_empty() {
        eprintln!("parity-live: no such feature: {unknown:?}");
        return 2;
    }
    let platform = Platform::here();
    let project = match project_beside(&shot_dir) {
        Ok(project) => project,
        Err(e) => {
            eprintln!("parity-live: couldn't make the project the steps work in: {e}");
            return 2;
        }
    };
    let mut features: Vec<&Feature> = ledger
        .feature
        .iter()
        .filter(|f| only.is_empty() || only.contains(&f.id))
        .collect();
    features.sort_by(|a, b| a.area.cmp(&b.area).then(a.id.cmp(&b.id)));

    let started = Instant::now();
    let app = App::beside(&shot_dir);
    // Features in a row the app didn't answer, and why the run stopped sending, once it has.
    let mut silent = 0;
    let mut stopped: Option<String> = None;
    let mut results = vec![];
    for f in features {
        let outcome = match f.support(platform) {
            Support::Yes | Support::Partial
                if let Some((_, why)) = not_here.iter().find(|(id, _)| *id == f.id) =>
            {
                Outcome::Skipped(format!("Not run on this computer: {why}"))
            }
            Support::Yes | Support::Partial
                if !ci
                    && !f.live_on(platform).is_empty()
                    && let Some(why) = &f.live_ci =>
            {
                Outcome::Skipped(format!("On CI only: {why}"))
            }
            Support::Yes | Support::Partial
                if !f.live_on(platform).is_empty()
                    && let Some(why) = &stopped =>
            {
                Outcome::Failed(vec![format!("not run: {why}")])
            }
            Support::Yes | Support::Partial if !f.live_on(platform).is_empty() => {
                match send(&shot_dir, &project, f, platform, timeout, &app) {
                    Answer::Said(outcome) => {
                        silent = 0;
                        outcome
                    }
                    Answer::Silent(outcome) => {
                        silent += 1;
                        if silent >= SILENT_IN_A_ROW {
                            stopped = Some(format!(
                                "the app stopped answering ({silent} features in a row, from before {})",
                                f.id
                            ));
                        }
                        outcome
                    }
                    Answer::Gone(outcome) => {
                        stopped = Some(format!("the app's process ended during {}", f.id));
                        outcome
                    }
                }
            }
            Support::Yes | Support::Partial if let Some(who) = &f.live_person => {
                Outcome::Person(who.clone())
            }
            Support::Yes | Support::Partial => {
                Outcome::Skipped(f.live_skip.clone().unwrap_or_default())
            }
            other => Outcome::NotHere(other),
        };
        let said = match &outcome {
            Outcome::Passed => "ok".to_string(),
            Outcome::Failed(why) => format!("FAILED: {}", why.join("; ")),
            Outcome::Skipped(_) => "skipped".to_string(),
            Outcome::Person(_) => "checked by a person".to_string(),
            Outcome::NotHere(_) => format!("not on {}", platform.name()),
        };
        println!("{:<28} {said}", f.id);
        results.push((f, outcome));
        if let Some(why) = &stopped
            && silent != usize::MAX
        {
            // Said once, with what the app last wrote: the rest only say they weren't run.
            silent = usize::MAX;
            println!(
                "--- {why}; the rest aren't run. The app's log ({}), last lines:\n{}\n---",
                app.log.display(),
                app.log_tail(40)
            );
        }
    }
    let failed = results
        .iter()
        .filter(|(_, o)| matches!(o, Outcome::Failed(_)))
        .count();
    let passed = results
        .iter()
        .filter(|(_, o)| matches!(o, Outcome::Passed))
        .count();
    let person = results
        .iter()
        .filter(|(_, o)| matches!(o, Outcome::Person(_)))
        .count();
    println!(
        "{passed} shown working, {person} checked by a person, {failed} failed, {} without live steps here, in {:.0} s",
        results.len() - passed - failed - person,
        started.elapsed().as_secs_f32()
    );
    if let Some(path) = report {
        let text = render(&results, platform, &root);
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("parity-live: {}: {e}", path.display());
            return 2;
        }
    }
    i32::from(failed > 0)
}

/// Send `f`'s steps as one batch and wait for the app's answer, as long as its process runs.
fn send(
    dir: &Path,
    project: &Path,
    f: &Feature,
    platform: Platform,
    timeout: Duration,
    app: &App,
) -> Answer {
    let mut lines: Vec<String> = FROM_THE_START.iter().map(|s| s.to_string()).collect();
    lines.extend(f.live_on(platform).iter().map(|s| fill(s, project)));
    lines.push(format!("shot live-{}", f.id));
    // The steps' own waits on top: a feature that waits two minutes for a language server isn't
    // given up on after one and a half.
    let timeout = timeout + waits(&lines);
    let done = dir.join("done");
    let _ = std::fs::remove_file(&done);
    let staged = dir.join("cmd.parity-live");
    if let Err(e) = std::fs::write(&staged, lines.join("\n") + "\n")
        .and_then(|()| std::fs::rename(&staged, dir.join("cmd")))
    {
        return Answer::Said(Outcome::Failed(vec![format!(
            "couldn't send the steps: {e}"
        )]));
    }
    let start = Instant::now();
    let mut asked = Instant::now();
    loop {
        // `done` is written in one go but not renamed into place: read it until it holds still.
        if let Ok(first) = std::fs::read_to_string(&done) {
            std::thread::sleep(Duration::from_millis(100));
            if std::fs::read_to_string(&done).is_ok_and(|again| again == first) && !first.is_empty()
            {
                let _ = std::fs::remove_file(&done);
                return Answer::Said(if first.trim() == "ok" {
                    Outcome::Passed
                } else {
                    Outcome::Failed(
                        first
                            .lines()
                            .map(|l| l.strip_prefix("err ").unwrap_or(l).to_string())
                            .collect(),
                    )
                });
            }
        }
        // Once a second: an app whose process ended answers nothing, however long it's given.
        if asked.elapsed() > Duration::from_secs(1) {
            asked = Instant::now();
            if app.alive() == Some(false) {
                return Answer::Gone(Outcome::Failed(vec![format!(
                    "the app's process ({}) ended after {:.0} s of this feature's steps",
                    app.pid.unwrap_or_default(),
                    start.elapsed().as_secs_f32()
                )]));
            }
        }
        if start.elapsed() > timeout {
            // Its answer may yet come: waited out and dropped, or it would be taken for the next
            // feature's.
            let late = Instant::now();
            while late.elapsed() < timeout && !done.exists() && app.alive() != Some(false) {
                std::thread::sleep(Duration::from_millis(200));
            }
            std::thread::sleep(Duration::from_millis(200));
            let _ = std::fs::remove_file(&done);
            return Answer::Silent(Outcome::Failed(vec![format!(
                "no answer from the app in {} s (is the capture build running from {}?)",
                timeout.as_secs(),
                dir.display()
            )]));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// How long `steps` may take by their own waits: a `wait`'s milliseconds, or the cap of what it
/// waits for (the app's own default where a step gives none: 30 s for `shown` and `file`, 60 s
/// otherwise).
fn waits(steps: &[String]) -> Duration {
    let ms = |w: Option<&str>| w.and_then(|v| v.parse::<u64>().ok());
    steps
        .iter()
        .map(|step| {
            let mut words = step.split_whitespace();
            if words.next() != Some("wait") {
                return 0;
            }
            match words.next() {
                Some("shown" | "file") => ms(words.nth(1)).unwrap_or(30_000),
                Some(what) => ms(Some(what)).or(ms(words.next())).unwrap_or(60_000),
                None => 0,
            }
        })
        .map(Duration::from_millis)
        .sum()
}

/// The run as a page: what was shown working on this platform, what failed and why, and what has
/// no live steps here.
fn render(results: &[(&Feature, Outcome)], platform: Platform, root: &Path) -> String {
    let commit = std::process::Command::new("git") // spawn: a developer's console tool; trek-parity has no trek-core
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let count = |want: fn(&Outcome) -> bool| results.iter().filter(|(_, o)| want(o)).count();
    let mut out = format!(
        "# Live check on {}\n\n\
         Written by `cargo run -p trek-parity --bin parity-live` against the capture build of `{commit}`: each \
         feature's `live` steps in `docs/parity/ledger.toml`, run in an isolated Trek, then a screenshot \
         (`live-<id>.png` in its command folder). Run it again after a change; the steps are the same on \
         every platform unless the ledger says otherwise.\n\n\
         **{} shown working, {} checked by a person, {} failed, {} with no live steps here, {} not on {}.**\n\n\
         | Area | Feature | Live | Steps or reason |\n|---|---|---|---|\n",
        platform.name(),
        count(|o| matches!(o, Outcome::Passed)),
        count(|o| matches!(o, Outcome::Person(_))),
        count(|o| matches!(o, Outcome::Failed(_))),
        count(|o| matches!(o, Outcome::Skipped(_))),
        count(|o| matches!(o, Outcome::NotHere(_))),
        platform.name(),
    );
    let cell = |s: &str| s.replace('|', "\\|").replace('\n', " ");
    for (f, outcome) in results {
        let steps = || {
            f.live_on(platform)
                .iter()
                .map(|s| format!("`{}`", s.replace('`', "'")))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let (mark, detail) = match outcome {
            Outcome::Passed => ("shown working".to_string(), steps()),
            Outcome::Failed(why) => (
                "**failed**".to_string(),
                format!("{} — {}", steps(), why.join("; ")),
            ),
            Outcome::Skipped(why) => ("no live steps".to_string(), why.clone()),
            Outcome::Person(who) => ("checked by a person".to_string(), who.clone()),
            Outcome::NotHere(Support::NotApplicable) => {
                (format!("n/a on {}", platform.name()), String::new())
            }
            Outcome::NotHere(_) => (format!("not on {}", platform.name()), String::new()),
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            cell(&f.area),
            cell(&f.what),
            mark,
            cell(&detail)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::waits;
    use std::time::Duration;

    #[test]
    fn the_app_is_found_by_the_pid_the_driver_keeps() {
        use super::pid_in;
        // As trek-dev writes it (indented) and as trek-dev.ps1's ConvertTo-Json does.
        assert_eq!(
            pid_in("{\n  \"root\": \"/x\",\n  \"pid\": 4242,\n  \"isolated\": true\n}"),
            Some(4242)
        );
        assert_eq!(pid_in("{\"pid\":17,\"binary\":\"x\"}"), Some(17));
        // Stopped: the driver keeps the state with no pid.
        assert_eq!(pid_in("{\"pid\": null}"), None);
        assert_eq!(pid_in("{}"), None);
    }

    /// A feature is given its steps' own waits on top of the base time.
    #[test]
    fn a_features_waits_add_up() {
        let steps: Vec<String> = [
            "route draft",
            "wait 1500",
            "wait idle 30000",
            "wait permission",
            "wait shown ide-problem#0 120000",
            "wait shown sim-devices",
            "wait file tools/ffprobe 180000",
            "wait file tools/yt-dlp",
            "count x 1",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            waits(&steps),
            Duration::from_millis(1_500 + 30_000 + 60_000 + 120_000 + 30_000 + 180_000 + 30_000)
        );
    }
}
