//! `trek plugin …`: install, list, turn on and off, reload, remove and develop plugins from a
//! terminal (an agent's, usually). It works on the plugins folder alone, with no window: the
//! running Trek sees the folder change and reloads within a couple of seconds, or at once after
//! `reload`.
//!
//! Turning on a plugin that asks for permissions only asks: it waits in Settings › Plugins for the
//! user to allow them (`State::ask_on`), so an agent can't grant itself anything.

use super::*;

const HELP: &str = "trek plugin: Trek's plugins, from a terminal.

  trek plugin install <folder | git URL | example:<name>>
  trek plugin list
  trek plugin enable <id>
  trek plugin disable <id>
  trek plugin reload [<id>]
  trek plugin remove <id>
  trek plugin dev <folder> [--once]   read the folder in place; print problems and its log as
                                      they change (--once: link it, say how it is, and return)
  trek plugin new <id>                a starting folder, ./<id>, to edit
  trek plugin examples                the examples Trek ships

Plugins live in <data>/plugins; the running Trek reloads when that folder changes.
The format is in Trek's built-in skill trek-plugin.";

/// `trek plugin …`, if that's what this launch is: the exit code, the work done.
pub fn from_args() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("plugin") {
        return None;
    }
    // On Windows a release Trek has no output of its own when typed at a console: it writes to
    // the console it was started from (`procs::parent_console`).
    Some(match crate::procs::parent_console() {
        Some(mut console) => {
            use std::io::Write as _;
            // The prompt is back already: start on a line of its own.
            let _ = writeln!(console);
            run(&args[1..], &dir(), &mut console)
        }
        None => run(&args[1..], &dir(), &mut std::io::stdout()),
    })
}

/// Run `args` (after `plugin`) against the plugins folder `dir`, writing to `out`.
pub fn run(args: &[String], dir: &Path, out: &mut dyn std::io::Write) -> i32 {
    let arg = |i: usize| args.get(i).map(String::as_str);
    let result = match (arg(0), arg(1)) {
        (Some("install"), Some(src)) => install(src, dir, out),
        (Some("list"), None) => list(dir, out),
        (Some("enable"), Some(id)) => enable(id, dir, out),
        (Some("disable"), Some(id)) => disable(id, dir, out),
        (Some("reload"), _) => reload(dir, out),
        (Some("remove"), Some(id)) => remove(id, dir, out),
        (Some("dev"), Some(src)) => dev(src, args.iter().any(|a| a == "--once"), dir, out),
        (Some("new"), Some(id)) => new(id, out),
        (Some("examples"), None) => {
            for (name, files) in examples::ALL {
                let (_, about) = examples::about(files);
                let _ = writeln!(out, "example:{name}  {about}");
            }
            Ok(())
        }
        (None, _) | (Some("help" | "--help" | "-h"), _) => {
            let _ = writeln!(out, "{HELP}");
            Ok(())
        }
        _ => Err(format!("that isn't a plugin command.\n\n{HELP}")),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(out, "error: {e}");
            1
        }
    }
}

/// The id a folder holds a plugin under: its manifest's `id`, else the folder's name.
fn id_in(folder: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(folder.join(MANIFEST))
        .map_err(|_| format!("there's no {MANIFEST} in {}", folder.display()))?;
    let table: toml::Table = toml::from_str(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let id = match table.get("id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => crate::themes::slug(
            &folder
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
    };
    if !id_ok(&id) {
        return Err(format!(
            "\"{id}\" isn't a plugin id: give {MANIFEST} an id of lower case letters, digits and dashes"
        ));
    }
    Ok(id)
}

fn copy_dir(from: &Path, to: &Path) -> Result<usize, String> {
    let mut n = 0;
    for e in walkdir::WalkDir::new(from)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            e.depth() == 0 || !(name == ".git" || name == "node_modules" || name == ".DS_Store")
        })
        .flatten()
    {
        let rel = e.path().strip_prefix(from).unwrap_or(e.path());
        let dest = to.join(rel);
        if e.file_type().is_dir() {
            std::fs::create_dir_all(&dest).map_err(|err| format!("{}: {err}", dest.display()))?;
        } else if e.file_type().is_file() {
            n += 1;
            if n > MAX_FILES {
                return Err(format!(
                    "it has more than {MAX_FILES} files: a plugin is small"
                ));
            }
            std::fs::copy(e.path(), &dest).map_err(|err| format!("{}: {err}", dest.display()))?;
        }
    }
    Ok(n)
}

fn is_git(src: &str) -> bool {
    src.starts_with("https://") || src.starts_with("git@") || src.ends_with(".git")
}

/// A scratch folder of its own.
fn scratch(dir: &Path, what: &str) -> PathBuf {
    dir.join(format!(".{what}-{}", std::process::id()))
}

fn install(src: &str, dir: &Path, out: &mut dyn std::io::Write) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let fetched = scratch(dir, "fetch");
    let _ = std::fs::remove_dir_all(&fetched);
    let folder = if let Some(name) = src.strip_prefix("example:") {
        examples::write(name, &fetched)?;
        fetched.clone()
    } else if is_git(src) {
        let _ = writeln!(out, "cloning {src}…");
        let status = crate::procs::command(crate::git::git_program())
            .args(["clone", "--depth", "1", "--quiet", src])
            .arg(&fetched)
            .env("GIT_TERMINAL_PROMPT", "0")
            .status()
            .map_err(|e| format!("git couldn't be run: {e}"))?;
        if !status.success() {
            let _ = std::fs::remove_dir_all(&fetched);
            return Err(format!("git couldn't clone {src}"));
        }
        fetched.clone()
    } else {
        let p = PathBuf::from(src);
        if !p.is_dir() {
            return Err(format!("{src} isn't a folder, a git URL or example:<name>"));
        }
        p
    };
    let result = install_folder(&folder, dir, out);
    let _ = std::fs::remove_dir_all(&fetched);
    result
}

fn install_folder(folder: &Path, dir: &Path, out: &mut dyn std::io::Write) -> Result<(), String> {
    let id = id_in(folder)?;
    let staged = scratch(dir, &format!("{id}-new"));
    let _ = std::fs::remove_dir_all(&staged);
    copy_dir(folder, &staged)?;
    let manifest = match read(&staged, &id) {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(format!("{id} wasn't installed: {e}"));
        }
    };
    let dest = dir.join(&id);
    let _ = std::fs::remove_file(dir.join(format!("{id}.{LINK_EXT}")));
    // The old copy steps aside and the new one takes its place, two renames apart: a running
    // Trek that looks at the folder meanwhile doesn't find the plugin gone (and turn its theme
    // or its page off) for as long as a delete would take. Folders that start with a dot aren't
    // plugins.
    let old = scratch(dir, &format!("{id}-old"));
    let _ = std::fs::remove_dir_all(&old);
    let had = dest.exists();
    if had {
        std::fs::rename(&dest, &old).map_err(|e| format!("the old {id} stays: {e}"))?;
    }
    if let Err(e) = std::fs::rename(&staged, &dest) {
        if had {
            let _ = std::fs::rename(&old, &dest);
        }
        return Err(e.to_string());
    }
    let _ = std::fs::remove_dir_all(&old);
    let mut state = State::read(dir);
    state.ask_on(&manifest);
    state.write(dir).map_err(|e| e.to_string())?;
    let _ = writeln!(
        out,
        "installed {} {} ({id}){}",
        manifest.name,
        manifest.version,
        if manifest.summary().is_empty() {
            String::new()
        } else {
            format!(": {}", manifest.summary())
        }
    );
    let plugin = Plugin {
        hash: grant::content_hash(&dest),
        manifest,
        dir: dest,
        origin: Origin::Installed,
    };
    say_on(&plugin, &state, out);
    let _ = std::fs::write(dir.join(RELOAD_FILE), "");
    Ok(())
}

/// `state` as Trek will take it: every grant whose seal doesn't verify for its plugin as it is
/// now counts for nothing (`State::verify`). A view to report from; the file keeps what it has.
fn as_trek_sees(state: &State, plugins: &[Plugin]) -> State {
    let mut seen = state.clone();
    seen.verify(plugins, grant::key().as_ref());
    seen
}

/// Say whether `plugin` runs, or what it waits for the user to allow.
fn say_on(plugin: &Plugin, state: &State, out: &mut dyn std::io::Write) {
    let m = &plugin.manifest;
    let seen = as_trek_sees(state, std::slice::from_ref(plugin));
    if seen.running(m) {
        let _ = writeln!(out, "{} is on", m.id);
    } else {
        let _ = writeln!(
            out,
            "{} is waiting for you to allow it in Trek › Settings › Plugins:",
            m.id
        );
        for p in seen.missing(m) {
            let _ = writeln!(out, "  - {}", p.words());
        }
    }
}

fn find(id: &str, dir: &Path) -> Result<Plugin, String> {
    let s = scan(dir);
    if let Some(p) = s.plugins.into_iter().find(|p| p.manifest.id == id) {
        return Ok(p);
    }
    match s.problems.into_iter().find(|p| p.id == id) {
        Some(p) => Err(format!("{id} can't be used: {}", p.message)),
        None => Err(format!("there's no plugin called {id} (trek plugin list)")),
    }
}

fn list(dir: &Path, out: &mut dyn std::io::Write) -> Result<(), String> {
    let s = scan(dir);
    let state = as_trek_sees(&State::read(dir), &s.plugins);
    if s.plugins.is_empty() && s.problems.is_empty() {
        let _ = writeln!(
            out,
            "no plugins in {} (trek plugin examples, then trek plugin install example:<name>)",
            dir.display()
        );
        return Ok(());
    }
    for p in &s.plugins {
        let m = &p.manifest;
        let c = state.choice(&m.id);
        let how = if state.running(m) {
            "on".to_string()
        } else if let Some(why) = &c.stopped {
            format!("stopped: {why}")
        } else if c.on || c.waiting {
            "waiting for permissions".to_string()
        } else {
            "off".to_string()
        };
        let linked = if p.origin == Origin::Linked {
            format!("  (linked: {})", p.dir.display())
        } else {
            String::new()
        };
        let _ = writeln!(out, "{}  {}  {how}{linked}", m.id, m.version);
    }
    for p in &s.problems {
        let _ = writeln!(out, "{}  problem: {}", p.id, p.message);
    }
    Ok(())
}

fn enable(id: &str, dir: &Path, out: &mut dyn std::io::Write) -> Result<(), String> {
    let p = find(id, dir)?;
    let mut state = State::read(dir);
    state.ask_on(&p.manifest);
    state.write(dir).map_err(|e| e.to_string())?;
    say_on(&p, &state, out);
    Ok(())
}

fn disable(id: &str, dir: &Path, out: &mut dyn std::io::Write) -> Result<(), String> {
    let mut state = State::read(dir);
    if !state.plugins.contains_key(id) && find(id, dir).is_err() {
        return Err(format!("there's no plugin called {id}"));
    }
    state.turn_off(id);
    state.write(dir).map_err(|e| e.to_string())?;
    let _ = writeln!(out, "{id} is off");
    Ok(())
}

fn reload(dir: &Path, out: &mut dyn std::io::Write) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(RELOAD_FILE), "").map_err(|e| e.to_string())?;
    let _ = writeln!(
        out,
        "asked Trek to read its plugins again; problems, if any, are in {}",
        dir.join(PROBLEMS_FILE).display()
    );
    Ok(())
}

fn remove(id: &str, dir: &Path, out: &mut dyn std::io::Write) -> Result<(), String> {
    if !id_ok(id) {
        return Err(format!("\"{id}\" isn't a plugin id"));
    }
    let folder = dir.join(id);
    let link = dir.join(format!("{id}.{LINK_EXT}"));
    let had = folder.is_dir() || link.is_file();
    if folder.is_dir() {
        std::fs::remove_dir_all(&folder).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_dir_all(data_dir(id));
    let mut state = State::read(dir);
    state.forget(id);
    state.write(dir).map_err(|e| e.to_string())?;
    if !had {
        return Err(format!("there's no plugin called {id}"));
    }
    let _ = writeln!(out, "removed {id}");
    Ok(())
}

fn dev(src: &str, once: bool, dir: &Path, out: &mut dyn std::io::Write) -> Result<(), String> {
    let folder = std::fs::canonicalize(src).map_err(|e| format!("{src}: {e}"))?;
    let id = id_in(&folder)?;
    if dir.join(&id).is_dir() {
        return Err(format!(
            "{id} is installed: trek plugin remove {id} first, then dev links the folder in its place"
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join(format!("{id}.{LINK_EXT}")),
        folder.display().to_string(),
    )
    .map_err(|e| e.to_string())?;
    let _ = writeln!(out, "linked {id} to {}", folder.display());
    let check = |out: &mut dyn std::io::Write, last: &mut Option<Result<(), String>>| {
        let now = read(&folder, &id).map(|m| {
            let mut state = State::read(dir);
            if !state.choice(&id).on && !state.choice(&id).waiting {
                state.ask_on(&m);
                let _ = state.write(dir);
                let plugin = Plugin {
                    hash: grant::content_hash(&folder),
                    manifest: m,
                    dir: folder.clone(),
                    origin: Origin::Linked,
                };
                say_on(&plugin, &state, out);
            }
        });
        if last.as_ref() != Some(&now) {
            match &now {
                Ok(()) => {
                    let _ = writeln!(out, "{id}: read, no problems");
                }
                Err(e) => {
                    let _ = writeln!(out, "{id}: problem: {e}");
                }
            }
            *last = Some(now);
        }
    };
    let mut last = None;
    check(out, &mut last);
    let _ = std::fs::write(dir.join(RELOAD_FILE), "");
    if once {
        return Ok(());
    }
    let _ = writeln!(out, "watching (Ctrl+C to stop; the link stays)");
    let mut print = fingerprint(dir);
    let mut lines = log_tail(&id, 1000).len();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(700));
        let now = fingerprint(dir);
        if now != print {
            print = now;
            check(out, &mut last);
        }
        let log = log_tail(&id, 1000);
        if log.len() != lines {
            for l in log.iter().skip(lines.min(log.len())) {
                let _ = writeln!(out, "log: {l}");
            }
            lines = log.len();
        }
        let _ = out.flush();
    }
}

fn new(id: &str, out: &mut dyn std::io::Write) -> Result<(), String> {
    if !id_ok(id) {
        return Err(format!(
            "\"{id}\" isn't an id: lower case letters, digits and dashes, starting with a letter"
        ));
    }
    let folder = PathBuf::from(id);
    if folder.exists() {
        return Err(format!("./{id} is already there"));
    }
    let name: String = id
        .split('-')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    std::fs::create_dir_all(folder.join("ui")).map_err(|e| e.to_string())?;
    let files = [
        (
            MANIFEST,
            format!(
                "id = \"{id}\"\nname = \"{name}\"\nversion = \"0.1.0\"\ndescription = \"\"\ntrek = \">={}\"\nmain = \"main.js\"\n\n[[slot]]\nat = \"composer.actions\"\nui = \"ui/button.json\"\n\n[[command]]\nid = \"hello\"\ntitle = \"Say hello\"\nrun = \"hello\"\n",
                trek_version()
            ),
        ),
        (
            "main.js",
            "// Runs in Trek's plugin host (QuickJS). See the trek-plugin skill for what `trek` offers.\nlet n = 0;\nfunction hello() {\n  n += 1;\n  trek.state.set({ clicks: n });\n  trek.toast(`Hello from a plugin (${n})`);\n}\n".to_string(),
        ),
        (
            "ui/button.json",
            "{ \"type\": \"button\", \"text\": \"Hello {state.clicks|or:}\", \"on\": \"command:hello\" }\n".to_string(),
        ),
    ];
    for (rel, text) in files {
        std::fs::write(folder.join(rel), text).map_err(|e| e.to_string())?;
    }
    let _ = writeln!(
        out,
        "made ./{id}: edit it, then trek plugin dev ./{id} to try it in Trek"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_in(dir: &Path, args: &[&str]) -> (i32, String) {
        let mut out = Vec::new();
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let code = run(&args, dir, &mut out);
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn install_list_enable_disable_remove() {
        let root = super::super::tests::scratch("cli");
        let plugins = root.join("plugins");
        let src = root.join("my-src");
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::write(src.join(".git/HEAD"), "x").unwrap();
        std::fs::write(
            src.join(MANIFEST),
            "id = \"meter\"\nname = \"Meter\"\nversion = \"1.0.0\"\npermissions = [\"usage:read\"]\n",
        )
        .unwrap();
        let (code, text) = run_in(&plugins, &["install", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("installed Meter 1.0.0 (meter)"), "{text}");
        assert!(
            text.contains("waiting") && text.contains("tokens"),
            "{text}"
        );
        assert!(plugins.join("meter/plugin.toml").is_file());
        assert!(
            !plugins.join("meter/.git").exists(),
            "git's own files stay behind"
        );
        assert!(plugins.join(RELOAD_FILE).exists());
        let (_, text) = run_in(&plugins, &["list"]);
        assert!(text.contains("meter  1.0.0  waiting"), "{text}");
        // The user allows it in Trek; the same plugin installed again stays on.
        let mut st = State::read(&plugins);
        assert!(st.turn_on_allowing(&find("meter", &plugins).unwrap(), grant::key().as_ref()));
        st.write(&plugins).unwrap();
        let (_, text) = run_in(&plugins, &["install", src.to_str().unwrap()]);
        assert!(text.contains("meter is on"), "{text}");
        let (_, text) = run_in(&plugins, &["list"]);
        assert!(text.contains("meter  1.0.0  on"), "{text}");
        // An update is another plugin as far as its grant goes: it asks again.
        std::fs::write(src.join("main.js"), "// new code").unwrap();
        let (_, text) = run_in(&plugins, &["install", src.to_str().unwrap()]);
        assert!(
            text.contains("waiting") && !text.contains("meter is on"),
            "{text}"
        );
        // And a grant written into the file by hand is no grant.
        let forged = "[plugins.meter]\non = true\ngranted = [\"usage:read\"]\nseal = \"00\"\n";
        std::fs::write(plugins.join(STATE_FILE), forged).unwrap();
        let (_, text) = run_in(&plugins, &["list"]);
        assert!(text.contains("meter  1.0.0  waiting"), "{text}");
        let (_, text) = run_in(&plugins, &["enable", "meter"]);
        assert!(
            text.contains("waiting") && text.contains("tokens"),
            "{text}"
        );
        let (_, text) = run_in(&plugins, &["disable", "meter"]);
        assert!(text.contains("off"), "{text}");
        let (code, _) = run_in(&plugins, &["remove", "meter"]);
        assert_eq!(code, 0);
        assert!(!plugins.join("meter").exists());
        assert!(State::read(&plugins).plugins.is_empty());
        let (code, text) = run_in(&plugins, &["remove", "meter"]);
        assert_eq!(code, 1, "{text}");
        let (code, text) = run_in(&plugins, &["frobnicate"]);
        assert_eq!(code, 1);
        assert!(text.contains("trek plugin install"), "{text}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_broken_plugin_isnt_installed() {
        let root = super::super::tests::scratch("cli-broken");
        let plugins = root.join("plugins");
        let src = root.join("broken");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(MANIFEST), "name = \"B\"\nmain = \"gone.js\"\n").unwrap();
        let (code, text) = run_in(&plugins, &["install", src.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(text.contains("gone.js"), "{text}");
        assert!(!plugins.join("broken").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dev_links_a_folder_in_place() {
        let root = super::super::tests::scratch("cli-dev");
        let plugins = root.join("plugins");
        let src = root.join("wip");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(MANIFEST), "name = \"Wip\"\n").unwrap();
        let (code, text) = run_in(&plugins, &["dev", src.to_str().unwrap(), "--once"]);
        assert_eq!(code, 0, "{text}");
        assert!(
            text.contains("no problems") && text.contains("wip is on"),
            "{text}"
        );
        let s = scan(&plugins);
        assert_eq!(s.plugins[0].origin, Origin::Linked);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn every_example_installs() {
        let root = super::super::tests::scratch("cli-examples");
        let plugins = root.join("plugins");
        for (name, _) in examples::ALL {
            let (code, text) = run_in(&plugins, &["install", &format!("example:{name}")]);
            assert_eq!(code, 0, "{name}: {text}");
        }
        let s = scan(&plugins);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert_eq!(s.plugins.len(), examples::ALL.len());
        let _ = std::fs::remove_dir_all(root);
    }
}
