//! OpenCode runs edits and commands without asking unless its config says otherwise, but Trek's
//! access levels need those prompts. Sessions therefore start with "ask" rules for the agents
//! Trek drives, added only where the user's own OpenCode config says nothing: OpenCode applies
//! agent rules after everything else, so a rule Trek adds would replace one the user wrote (a
//! deny, a pattern list, `tools: false`). Plan and explore keep their own edit rules (deny), so
//! plan mode still can't change files.
//!
//! The user's config is read from the places OpenCode reads it on this machine. Remote and
//! organisation configs aren't fetched. OpenCode 2 renamed the keys (`agents`, `permissions` as
//! a list of `{action, resource, effect}` rules, `shell` for `bash`) and still takes 1.x's, so
//! both are read, and Trek's own rules are written the 1.x way, which both versions take.

use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use trek_core::import::opencode::share;

/// The permissions Trek's access levels gate.
const GATED: [&str; 3] = ["edit", "bash", "webfetch"];

/// The built-in agents a Trek session can run, and the permissions Trek asks about for each.
const BUILT_IN: [(&str, &[&str]); 4] = [
    ("build", &GATED),
    ("general", &GATED),
    ("plan", &["bash", "webfetch"]),
    ("explore", &["bash", "webfetch"]),
];

/// Every agent OpenCode ships; the hidden ones (compaction, title, summary) deny all tools and
/// must stay that way.
const NATIVE: [&str; 7] = ["build", "plan", "general", "explore", "compaction", "title", "summary"];

/// What the user's own OpenCode config already decides.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct UserRules {
    /// Gated permissions set for every agent.
    global: BTreeSet<&'static str>,
    /// Gated permissions set for one agent.
    agents: BTreeMap<String, BTreeSet<&'static str>>,
    /// Agents the user's config defines (in JSON or Markdown); Trek's rules can name them
    /// safely (naming an agent that doesn't exist would create it).
    custom: BTreeSet<String>,
}

/// `*` matches any run of characters; nothing else is special.
fn glob(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, rest)) => {
            let Some(tail) = name.strip_prefix(head) else { return false };
            (0..=tail.len()).filter(|i| tail.is_char_boundary(*i)).any(|i| glob(rest, &tail[i..]))
        }
    }
}

/// The gated permissions a permission or tools key covers. Edit tools all fall under "edit";
/// OpenCode 2's `shell` is 1.x's `bash`.
fn gated(key: &str) -> impl Iterator<Item = &'static str> + '_ {
    GATED.into_iter().filter(move |g| {
        let names: &[&str] = match *g {
            "edit" => &["edit", "write", "patch", "apply_patch", "multiedit"],
            "bash" => &["bash", "shell"],
            _ => &[g],
        };
        names.iter().any(|n| glob(key, n))
    })
}

/// Gated permissions a config section (the top level, or one agent) sets: 1.x's `permission`
/// (a map, or one action for everything) and OpenCode 2's `permissions` (rules naming an action).
fn set_in(section: &Value) -> BTreeSet<&'static str> {
    let mut out = BTreeSet::new();
    for key in ["permission", "permissions"] {
        match &section[key] {
            Value::Null => {}
            Value::Object(rules) => out.extend(rules.keys().flat_map(|k| gated(k))),
            Value::Array(rules) => {
                for rule in rules {
                    match rule["action"].as_str() {
                        Some(action) => out.extend(gated(action)),
                        None => out.extend(GATED),
                    }
                }
            }
            _ => out.extend(GATED),
        }
    }
    for k in section["tools"].as_object().into_iter().flat_map(|t| t.keys()) {
        out.extend(gated(k));
    }
    out
}

impl UserRules {
    fn add_config(&mut self, c: &Value) {
        self.global.extend(set_in(c));
        for (name, agent) in ["agent", "agents", "mode"].iter().filter_map(|k| c[*k].as_object()).flatten() {
            self.agents.entry(name.clone()).or_default().extend(set_in(agent));
            self.custom.insert(name.clone());
        }
    }

    /// A Markdown agent, named by its path unless its front matter says otherwise. Any
    /// permission or tools in its front matter count as the user's.
    fn add_agent_file(&mut self, name: &str, text: &str) {
        let mut lines = text.lines();
        let front: Vec<&str> = match lines.next().map(str::trim) {
            Some("---") => lines.take_while(|l| l.trim() != "---").collect(),
            _ => vec![],
        };
        let name = front
            .iter()
            .find_map(|l| l.strip_prefix("name:"))
            .map(|n| n.trim().trim_matches(['"', '\'']).to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| name.to_string());
        if front.iter().any(|l| ["permission:", "permissions:", "tools:"].iter().any(|k| l.starts_with(k))) {
            self.agents.entry(name.clone()).or_default().extend(GATED);
        }
        self.custom.insert(name);
    }

    /// A config file OpenCode would read; one Trek can't parse counts as setting everything.
    fn add_config_text(&mut self, text: &str) {
        match parse_jsonc(text) {
            Some(c) => self.add_config(&c),
            None if text.contains("permission") || text.contains("tools") => self.global.extend(GATED),
            None => {}
        }
    }
}

/// `{"agent": {name: {"permission": {gated: "ask"}}}}` for what the user left unset, if anything.
pub(crate) fn ask_rules(user: &UserRules) -> Option<Value> {
    let custom = user.custom.iter().filter(|n| !NATIVE.contains(&n.as_str())).map(|n| (n.as_str(), &GATED[..]));
    let mut agents = Map::new();
    for (name, keys) in BUILT_IN.into_iter().chain(custom) {
        let theirs = user.agents.get(name);
        let ask: Map<String, Value> = keys
            .iter()
            .filter(|k| !user.global.contains(*k) && !theirs.is_some_and(|t| t.contains(*k)))
            .map(|k| (k.to_string(), json!("ask")))
            .collect();
        if !ask.is_empty() {
            agents.insert(name.to_string(), json!({ "permission": ask }));
        }
    }
    (!agents.is_empty()).then(|| json!({ "agent": agents }))
}

/// JSON with comments and trailing commas, as OpenCode accepts it.
fn parse_jsonc(text: &str) -> Option<Value> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for c in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
            }
            (',', _) => {
                // Dropped when only whitespace and comments stand before a closing bracket.
                let rest: String = chars.clone().collect();
                let next = strip_leading_comments(&rest);
                if !next.starts_with('}') && !next.starts_with(']') {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    serde_json::from_str(&out).ok()
}

fn strip_leading_comments(mut s: &str) -> &str {
    loop {
        s = s.trim_start();
        if let Some(rest) = s.strip_prefix("//") {
            s = rest.split_once('\n').map_or("", |(_, r)| r);
        } else if let Some(rest) = s.strip_prefix("/*") {
            s = rest.split_once("*/").map_or("", |(_, r)| r);
        } else {
            return s;
        }
    }
}

/// Where OpenCode looks for config, for a session in `cwd`: JSON files, then the folders whose
/// `agent(s)/` and `mode(s)/` hold Markdown agents.
fn config_sources(cwd: &Path, env: &dyn Fn(&str) -> Option<String>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let home = trek_core::paths::home();
    let global = env("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("opencode");
    let mut files: Vec<PathBuf> = ["config.json", "opencode.json", "opencode.jsonc"].iter().map(|f| global.join(f)).collect();
    let mut dirs = vec![global];
    files.extend(env("OPENCODE_CONFIG").map(PathBuf::from));
    if env("OPENCODE_DISABLE_PROJECT_CONFIG").is_none() {
        // From the project's root (its git worktree) down to the session's folder.
        let mut up: Vec<&Path> = vec![];
        for dir in cwd.ancestors() {
            up.push(dir);
            if dir.join(".git").exists() {
                break;
            }
        }
        for dir in up.into_iter().rev() {
            files.extend(["opencode.json", "opencode.jsonc"].iter().map(|f| dir.join(f)));
            dirs.push(dir.join(".opencode"));
        }
    }
    dirs.push(home.join(".opencode"));
    let config_dir = env("OPENCODE_CONFIG_DIR").map(PathBuf::from);
    dirs.extend(config_dir.clone());
    for dir in dirs.iter().filter(|d| d.ends_with(".opencode") || Some(*d) == config_dir.as_ref()) {
        files.extend(["opencode.json", "opencode.jsonc"].iter().map(|f| dir.join(f)));
    }
    let managed = PathBuf::from("/Library/Application Support/opencode");
    files.extend(["opencode.json", "opencode.jsonc"].iter().map(|f| managed.join(f)));
    (files, dirs)
}

/// Markdown agents under `dir`: `(name, text)`.
fn agent_files(dir: &Path) -> Vec<(String, String)> {
    let mut out = vec![];
    for (sub, deep) in [("agent", true), ("agents", true), ("mode", false), ("modes", false)] {
        let root = dir.join(sub);
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if deep {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|e| e == "md")
                    && let (Ok(text), Ok(rel)) = (std::fs::read_to_string(&path), path.strip_prefix(&root))
                {
                    out.push((rel.with_extension("").to_string_lossy().to_string(), text));
                }
            }
        }
    }
    out
}

fn user_rules(cwd: &Path, env: &dyn Fn(&str) -> Option<String>) -> UserRules {
    let mut rules = UserRules::default();
    let (files, dirs) = config_sources(cwd, env);
    for f in files {
        if let Ok(text) = std::fs::read_to_string(&f) {
            rules.add_config_text(&text);
        }
    }
    for dir in dirs {
        for (name, text) in agent_files(&dir) {
            rules.add_agent_file(&name, &text);
        }
    }
    if let Some(text) = env("OPENCODE_PERMISSION") {
        rules.add_config_text(&format!(r#"{{"permission":{text}}}"#));
    }
    if let Some(text) = env("OPENCODE_CONFIG_CONTENT") {
        rules.add_config_text(&text);
    }
    rules
}

/// Environment for `opencode acp` in `cwd`: Trek's ask rules as inline config. Inline config the
/// user set themselves is kept, with Trek's rules added to it.
pub(crate) fn launch_env(cwd: &Path) -> Vec<(String, String)> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let Some(mut rules) = ask_rules(&user_rules(cwd, &env)) else { return vec![] };
    if let Some(theirs) = env("OPENCODE_CONFIG_CONTENT") {
        match parse_jsonc(&theirs).map(|t| merge_rules(t, &rules)) {
            Some(Some(merged)) => rules = merged,
            // Not something Trek can add to: leave it as the user wrote it.
            _ => return vec![],
        }
    }
    vec![("OPENCODE_CONFIG_CONTENT".to_string(), rules.to_string())]
}

/// `theirs` with Trek's agent rules added (they only name permissions `theirs` leaves unset).
fn merge_rules(mut theirs: Value, ours: &Value) -> Option<Value> {
    let agents = theirs.as_object_mut()?.entry("agent").or_insert_with(|| json!({})).as_object_mut()?;
    for (name, rules) in ours["agent"].as_object().into_iter().flatten() {
        let agent = agents.entry(name.clone()).or_insert_with(|| json!({})).as_object_mut()?;
        let permission = agent.entry("permission").or_insert_with(|| json!({})).as_object_mut()?;
        for (k, v) in rules["permission"].as_object().into_iter().flatten() {
            permission.insert(k.clone(), v.clone());
        }
    }
    Some(theirs)
}

/// What OpenCode 1.x says, and exits, when its database was made by OpenCode 2: it only migrates a
/// database that has 1.x's `session` table.
const MADE_BY_2: &str = "Database is not empty and has no session table";

/// `opencode --version` is 1.x's: `1.18.35`. OpenCode 2 says `2.0.26`, or `opencode2
/// v0.0.0-beta-19296` from a preview build.
fn is_1x_version(version: &str) -> bool {
    version.split_whitespace().last().map(|v| v.trim_start_matches('v')).is_some_and(|v| v.starts_with("1."))
}

/// Whether the OpenCode at `bin` (`stamp`: which install it is) is 1.x. Asked once per install.
async fn is_1x(bin: &Path, stamp: String) -> bool {
    static SEEN: std::sync::Mutex<Vec<(String, bool)>> = std::sync::Mutex::new(Vec::new());
    let seen = |s: &String| SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|(k, _)| k == s).map(|(_, v)| *v);
    if let Some(v) = seen(&stamp) {
        return v;
    }
    // Not cached when it didn't answer: it's asked again next time.
    let Some(version) = trek_core::detect::version_of(bin).await else { return false };
    let v = is_1x_version(&version);
    SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((stamp, v));
    v
}

/// OpenCode 2's command where it's installed beside 1.x (its preview package's name).
const OPENCODE_2: &str = "opencode2";

/// Another OpenCode to resume session `id` with than the one at `bin`: 1.x can't open a session
/// only OpenCode 2 has, so it's resumed with OpenCode 2 when that's installed beside it.
pub(crate) async fn bin_for(bin: &Path, id: &str) -> Option<PathBuf> {
    let id = id.to_string();
    if !is_1x(bin, crate::acp::stamp(bin)).await || !tokio::task::spawn_blocking(move || trek_core::import::opencode::only_2_opens(&id)).await.unwrap_or(false) {
        return None;
    }
    let other = trek_core::detect::which(OPENCODE_2)?;
    (!is_1x(&other, crate::acp::stamp(&other)).await).then_some(other)
}

/// How Trek starts OpenCode 1.x on the user's history.
#[derive(Default)]
pub(crate) struct DbEnv {
    pub env: Vec<(String, String)>,
    /// What the session should tell the user about it.
    pub notice: Option<String>,
    /// Held while 1.x runs on `opencode-1x.db`, so its sessions aren't moved out from under it.
    pub own: Option<OwnInUse>,
}

/// 1.x runs on `opencode-1x.db` while one of these is alive.
pub(crate) struct OwnInUse(());

static OWN_IN_USE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl OwnInUse {
    fn new() -> Self {
        OWN_IN_USE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        OwnInUse(())
    }
}

impl Drop for OwnInUse {
    fn drop(&mut self) {
        OWN_IN_USE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Environment for the OpenCode at `bin` (`stamp`: which install it is). 1.x and 2 share
/// `opencode.db` (see `trek_core::import::opencode::share`); when it can't be shared, 1.x keeps
/// its sessions in `opencode-1x.db`, which Trek reads as well. An `OPENCODE_DB` the user set
/// stands.
pub(crate) async fn db_env(bin: &Path, stamp: String) -> DbEnv {
    if std::env::var_os("OPENCODE_DB").is_some() || !is_1x(bin, stamp.clone()).await {
        return DbEnv::default();
    }
    let dir = trek_core::import::opencode::data_dir();
    match prepare(bin, &stamp, &dir).await {
        Ok(notice) => DbEnv { notice, ..DbEnv::default() },
        Err(why) => {
            // Said once a run, not in every session.
            static TOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            let tell = !TOLD.swap(true, std::sync::atomic::Ordering::SeqCst);
            let own = share::own_db(&dir).display().to_string();
            DbEnv { env: vec![("OPENCODE_DB".to_string(), own)], notice: tell.then(|| refused_notice(&why)), own: Some(OwnInUse::new()) }
        }
    }
}

/// Make the OpenCode history 1.x opens, when 2.0 made it: run as Trek finds its agents, so
/// `opencode` 1.x works in a terminal too, not only once Trek has started it.
pub async fn share_history() {
    let Some(bin) = trek_core::detect::which(trek_core::detect::OPENCODE) else { return };
    if std::env::var_os("OPENCODE_DB").is_some() || !is_1x(&bin, crate::acp::stamp(&bin)).await {
        return;
    }
    match prepare(&bin, &crate::acp::stamp(&bin), &trek_core::import::opencode::data_dir()).await {
        Ok(Some(done)) => tracing::info!("opencode: {done}"),
        Ok(None) => {}
        Err(why) => tracing::info!("opencode: 1.x keeps opencode-1x.db: {why}"),
    }
}

/// Get `opencode.db` in `dir` ready for the 1.x at `bin`. `Ok`: 1.x opens it, with what the user
/// should know about what changed; `Err`: it can't be shared, and why.
async fn prepare(bin: &Path, stamp: &str, dir: &Path) -> Result<Option<String>, String> {
    // Not at once from two sessions.
    static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    // Databases that can't be shared with an install (its stamp), and why: not tried again in
    // this run.
    static REFUSED: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
    let _turn = ONE_AT_A_TIME.lock().await;
    let in_use = OWN_IN_USE.load(std::sync::atomic::Ordering::SeqCst) > 0;
    let at = dir.to_path_buf();
    match share::state(dir) {
        share::State::Opens => {
            // Sessions 1.x kept on its own before move in, unless it's still writing there. Not
            // tried again in this run once it failed (each try makes a copy of the database).
            static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if in_use || FAILED.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(None);
            }
            match tokio::task::spawn_blocking(move || share::merge_own(&at)).await {
                Ok(Ok(Some(c))) => Ok((c.merged > 0).then(|| merged_notice(&c))),
                Ok(Ok(None)) => Ok(None),
                Ok(Err(e)) => {
                    FAILED.store(true, std::sync::atomic::Ordering::SeqCst);
                    tracing::warn!("opencode: couldn't move opencode-1x.db's sessions into opencode.db: {e}");
                    Ok(None)
                }
                Err(e) => {
                    tracing::warn!("opencode: {e}");
                    Ok(None)
                }
            }
        }
        share::State::Made2 => {
            let key = format!("{stamp}:{}", dir.display());
            if let Some((_, why)) = REFUSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|(k, _)| *k == key) {
                return Err(why.clone());
            }
            let refuse = |why: String| {
                REFUSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((key.clone(), why.clone()));
                why
            };
            let scratch = scratch_schema(bin).await.map_err(|e| refuse(format!("OpenCode 1.x didn't show its tables: {e:#}")))?;
            let db = scratch.db.clone();
            let done = tokio::task::spawn_blocking(move || share::share(&at, &db, !in_use)).await.map_err(|e| e.to_string())?;
            drop(scratch);
            match done {
                Ok(Some(c)) => Ok(Some(shared_notice(&c))),
                Ok(None) => Ok(None),
                Err(share::Refused::Shape(why)) => Err(refuse(why)),
                Err(share::Refused::Failed(why)) => Err(why),
            }
        }
    }
}

/// A folder with the empty database the OpenCode 1.x at `bin` makes. Removed when dropped.
struct Scratch {
    dir: PathBuf,
    db: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Have 1.x create its database, empty, somewhere of its own: a home and data folders of its
/// own (nothing of the user's is read or written), no plugins, no model list fetched.
/// `session list` creates the database and lists nothing.
async fn scratch_schema(bin: &Path) -> anyhow::Result<Scratch> {
    use anyhow::Context as _;
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("trek-opencode-1x-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let scratch = Scratch { db: dir.join("opencode.db"), dir };
    let mut command = tokio::process::Command::new(bin);
    command.args(["session", "list", "--pure"]).current_dir(&scratch.dir).env("PATH", trek_core::detect::login_path());
    for var in ["OPENCODE_CONFIG", "OPENCODE_CONFIG_DIR", "OPENCODE_CONFIG_CONTENT"] {
        command.env_remove(var);
    }
    for (var, sub) in [("HOME", "home"), ("XDG_DATA_HOME", "data"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"), ("XDG_STATE_HOME", "state")] {
        command.env(var, scratch.dir.join(sub));
    }
    command.env("OPENCODE_DB", &scratch.db).env("OPENCODE_DISABLE_MODELS_FETCH", "1").env("OPENCODE_DISABLE_AUTOUPDATE", "1");
    let out = crate::output_group(&mut command, None, std::time::Duration::from_secs(30)).await.context("opencode session list")?;
    if !out.status.success() || !scratch.db.is_file() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("opencode session list: {}", err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no database").trim());
    }
    Ok(scratch)
}

fn shared_notice(c: &share::Changed) -> String {
    let mut s = format!(
        "OpenCode 1.x and 2 now keep their sessions in one opencode.db: Trek added 1.x's tables to the one OpenCode 2 \
         made, so 1.x opens it again, in a terminal too. A copy from before is at {}.",
        c.backup.display()
    );
    if c.merged > 0 {
        s.push_str(&format!(" {} 1.x kept in opencode-1x.db moved in.", sessions(c.merged)));
    }
    s
}

fn merged_notice(c: &share::Changed) -> String {
    format!(
        "{} OpenCode 1.x kept in opencode-1x.db moved into opencode.db. A copy of opencode.db from before is at {}.",
        sessions(c.merged),
        c.backup.display()
    )
}

fn sessions(n: usize) -> String {
    if n == 1 { "The session".into() } else { format!("The {n} sessions") }
}

fn refused_notice(why: &str) -> String {
    format!(
        "OpenCode 1.x keeps its sessions in opencode-1x.db: Trek couldn't add 1.x's tables to the opencode.db OpenCode 2 \
         made ({why}). Trek shows both; in a terminal, 1.x needs OPENCODE_DB=opencode-1x.db."
    )
}

/// Why OpenCode exited, said plainly, when it's 1.x refusing OpenCode 2's database.
pub(crate) fn exit_reason(stderr: &[String]) -> Option<String> {
    stderr.iter().any(|l| l.contains(MADE_BY_2)).then(|| {
        format!(
            "OpenCode 1.x can't open the history OpenCode 2 started (\"{MADE_BY_2}\"). Use OpenCode 2, or point \
             OPENCODE_DB at a database of 1.x's own; without OPENCODE_DB set, Trek makes opencode.db one both open."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_1x_is_told_by_its_version() {
        assert!(is_1x_version("1.18.35"));
        assert!(!is_1x_version("2.0.26"));
        assert!(!is_1x_version("opencode2 v0.0.0-beta-19296"));
        assert!(!is_1x_version(""));
    }

    /// A folder with an `opencode` that acts as 1.x does when asked for its schema: it creates
    /// `OPENCODE_DB` (as 1.18.35 does, from the recorded schema), and only in a home of its own.
    fn fake_1x(dir: &Path) -> PathBuf {
        let schema = concat!(env!("CARGO_MANIFEST_DIR"), "/../trek-core/fixtures/opencode-1x-empty.sql");
        let bin = dir.join("opencode");
        let script = format!(
            "#!/bin/sh\n\
             [ \"$1\" = --version ] && {{ echo 1.18.35; exit 0; }}\n\
             [ \"$1 $2 $3\" = 'session list --pure' ] || exit 2\n\
             [ \"$HOME\" = \"$(dirname \"$OPENCODE_DB\")/home\" ] && [ \"$XDG_DATA_HOME\" = \"$(dirname \"$OPENCODE_DB\")/data\" ] || exit 3\n\
             [ \"$OPENCODE_DISABLE_MODELS_FETCH\" = 1 ] || exit 4\n\
             exec /usr/bin/sqlite3 \"$OPENCODE_DB\" < '{schema}'\n"
        );
        std::fs::write(&bin, script).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    /// Run `sql` on the database at `path` (macOS's own sqlite3).
    fn sqlite(path: &Path, sql: &str) {
        use std::io::Write as _;
        let mut child = std::process::Command::new("/usr/bin/sqlite3").arg(path).stdin(std::process::Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(sql.as_bytes()).unwrap();
        assert!(child.wait().unwrap().success());
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek-agents-opencode-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("data")).unwrap();
        dir
    }

    #[tokio::test]
    async fn opencode_2s_database_is_shared_with_1x() {
        let dir = temp_dir("share");
        let bin = fake_1x(&dir);
        let data = dir.join("data");
        let made_by_2 = concat!(env!("CARGO_MANIFEST_DIR"), "/../trek-core/fixtures/opencode-2-fresh.sql");
        sqlite(&data.join("opencode.db"), &std::fs::read_to_string(made_by_2).unwrap());
        let told = prepare(&bin, "fake", &data).await.unwrap().unwrap();
        assert!(told.starts_with("OpenCode 1.x and 2 now keep their sessions in one opencode.db") && told.contains("opencode.db.backup-"), "{told}");
        assert_eq!(share::state(&data), share::State::Opens);
        // Done: 1.x opens it as it is from now on.
        assert_eq!(prepare(&bin, "fake", &data).await, Ok(None));
        // The schema was made, and cleared away, outside the user's folders.
        let left: Vec<_> = std::fs::read_dir(&data).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with("opencode.db")).collect();
        assert!(left.is_empty(), "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_database_that_cant_be_shared_leaves_1x_its_own() {
        let dir = temp_dir("refuse");
        let bin = fake_1x(&dir);
        let data = dir.join("data");
        // Not 1.x's tables, and no journal: not something to add to.
        sqlite(&data.join("opencode.db"), "CREATE TABLE session_v2 (id TEXT PRIMARY KEY);");
        let why = prepare(&bin, "fake-refused", &data).await.unwrap_err();
        assert_eq!(why, "opencode.db has no migration journal, so it isn't one OpenCode made");
        assert!(refused_notice(&why).starts_with("OpenCode 1.x keeps its sessions in opencode-1x.db: Trek couldn't add"));
        // Not asked again in this run.
        std::fs::remove_file(&bin).unwrap();
        assert_eq!(prepare(&bin, "fake-refused", &data).await.unwrap_err(), why);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With the real `opencode` 1.x on PATH, on the OpenCode data folder in
    /// `TREK_OPENCODE_DATA` (a scratch one: this changes it).
    #[tokio::test]
    #[ignore = "runs the real OpenCode 1.x on TREK_OPENCODE_DATA"]
    async fn share_with_the_real_opencode() {
        let data = PathBuf::from(std::env::var_os("TREK_OPENCODE_DATA").expect("TREK_OPENCODE_DATA"));
        let bin = trek_core::detect::which(trek_core::detect::OPENCODE).expect("opencode");
        let done = prepare(&bin, &crate::acp::stamp(&bin), &data).await;
        println!("{done:?}");
        assert!(done.is_ok());
        assert_eq!(share::state(&data), share::State::Opens);
    }

    #[test]
    fn a_refused_database_is_explained() {
        let stderr = ["\u{1b}[91mError: \u{1b}[0mUnexpected error".to_string(), String::new(), MADE_BY_2.to_string()];
        let why = exit_reason(&stderr).unwrap();
        assert!(why.starts_with("OpenCode 1.x can't open the history OpenCode 2 started") && why.contains("OPENCODE_DB"), "{why}");
        assert_eq!(exit_reason(&["Error: something else".to_string()]), None);
    }

    /// Sorted: key order depends on serde_json's `preserve_order`, which other crates may turn on.
    fn agent_names(rules: &Value) -> Vec<String> {
        let mut names: Vec<String> = rules["agent"].as_object().unwrap().keys().cloned().collect();
        names.sort();
        names
    }

    fn rules(configs: &[&str]) -> UserRules {
        let mut r = UserRules::default();
        for c in configs {
            r.add_config_text(c);
        }
        r
    }

    #[test]
    fn with_nothing_set_every_agent_asks() {
        assert_eq!(
            ask_rules(&UserRules::default()),
            Some(json!({"agent":{
                "build":{"permission":{"edit":"ask","bash":"ask","webfetch":"ask"}},
                "general":{"permission":{"edit":"ask","bash":"ask","webfetch":"ask"}},
                // Plan and explore keep OpenCode's own edit rule: deny.
                "plan":{"permission":{"bash":"ask","webfetch":"ask"}},
                "explore":{"permission":{"bash":"ask","webfetch":"ask"}},
            }}))
        );
    }

    #[test]
    fn the_users_own_rules_are_left_alone() {
        // A pattern list, a plain deny and a disabled tool all stay the user's.
        let r = rules(&[r#"{
            // mine
            "permission": { "bash": { "*": "allow", "git push*": "deny" }, },
            "tools": { "webfetch": false },
            "agent": { "build": { "permission": { "edit": "deny" } }, "review": { "model": "x" } },
        }"#]);
        assert_eq!(
            ask_rules(&r),
            Some(json!({"agent":{
                "general":{"permission":{"edit":"ask"}},
                "review":{"permission":{"edit":"ask"}},
            }}))
        );
        // `*` and edit tool aliases count too.
        assert_eq!(ask_rules(&rules(&[r#"{"permission":{"*":"deny"}}"#])), None);
        let write_off = rules(&[r#"{"tools":{"write":false,"bash":true}}"#]);
        assert_eq!(write_off.global, BTreeSet::from(["edit", "bash"]));
    }

    #[test]
    fn opencode_2_rules_are_the_users_too() {
        // OpenCode 2's keys: rule lists, `agents`, `shell` for bash. A rule Trek added after the
        // user's deny would win (the last matching rule does), so none is.
        let r = rules(&[r#"{
            "permissions": [ { "action": "shell", "resource": "git push *", "effect": "deny" } ],
            "agents": {
                "build": { "permissions": [ { "action": "webfetch", "resource": "*", "effect": "deny" } ] },
                "reviewer": { "mode": "subagent", "permissions": [ { "action": "edit", "resource": "*", "effect": "deny" } ] },
            },
        }"#]);
        assert_eq!(r.global, BTreeSet::from(["bash"]));
        assert_eq!(
            ask_rules(&r),
            Some(json!({"agent":{
                "build":{"permission":{"edit":"ask"}},
                "general":{"permission":{"edit":"ask","webfetch":"ask"}},
                "plan":{"permission":{"webfetch":"ask"}},
                "explore":{"permission":{"webfetch":"ask"}},
                "reviewer":{"permission":{"webfetch":"ask"}},
            }}))
        );
        // A rule for every action, or one Trek can't read, sets everything.
        assert_eq!(ask_rules(&rules(&[r#"{"permissions":[{"action":"*","resource":"*","effect":"allow"}]}"#])), None);
        assert_eq!(ask_rules(&rules(&[r#"{"permissions":[{"resource":"*","effect":"allow"}]}"#])), None);
        let mut md = UserRules::default();
        md.add_agent_file("reviewer", "---\nmode: subagent\npermissions:\n  - action: edit\n---\n");
        assert_eq!(md.agents["reviewer"], BTreeSet::from(GATED));
    }

    #[test]
    fn agent_files_and_unreadable_configs_count_as_set() {
        let mut r = UserRules::default();
        r.add_agent_file("build", "---\ndescription: mine\npermission:\n  bash: deny\n---\nYou build.");
        r.add_agent_file("plan", "---\ndescription: no rules\n---\nYou plan.");
        assert_eq!(r.agents["build"], BTreeSet::from(GATED));
        assert!(!r.agents.contains_key("plan"));
        r.add_config_text(r#"{ "permission": { "bash": "deny" "#);
        assert_eq!(r.global, BTreeSet::from(GATED));
    }

    #[test]
    fn markdown_agents_without_rules_ask_too() {
        let mut r = UserRules::default();
        r.add_agent_file("review", "---\ndescription: reviews\nmode: subagent\n---\nYou review.");
        r.add_agent_file("docs/writer", "---\nname: \"scribe\"\n---\nYou write.");
        r.add_agent_file("strict", "---\ntools:\n  bash: false\n---\n");
        r.add_agent_file("title", "---\nmodel: m\n---\n");
        r.add_agent_file("notes", "Just a prompt, no front matter.");
        let rules = ask_rules(&r).unwrap();
        // OpenCode's hidden agents stay as they are; "strict" set its own rules.
        assert_eq!(agent_names(&rules), vec!["build", "explore", "general", "notes", "plan", "review", "scribe"]);
        assert_eq!(rules["agent"]["scribe"]["permission"], json!({"edit":"ask","bash":"ask","webfetch":"ask"}));
    }

    #[test]
    fn jsonc_comments_and_trailing_commas() {
        let v = parse_jsonc("{\n  // a\n  \"a\": \"x // not a comment\", /* b */\n  \"b\": [1, 2,],\n}").unwrap();
        assert_eq!(v, json!({"a":"x // not a comment","b":[1,2]}));
        assert_eq!(parse_jsonc(r#"{"s":"q\"uote,]"}"#).unwrap(), json!({"s":"q\"uote,]"}));
    }

    #[test]
    fn globs() {
        assert!(glob("*", "bash"));
        assert!(glob("web*", "webfetch"));
        assert!(!glob("web*", "bash"));
        assert_eq!(gated("write").collect::<Vec<_>>(), vec!["edit"]);
        assert_eq!(gated("mcp_*").count(), 0);
    }

    #[test]
    fn project_config_is_read_up_to_the_repository_root() {
        let root = std::env::temp_dir().join(format!("trek-opencode-cfg-{}", std::process::id()));
        let sub = root.join("app/src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".opencode/agent")).unwrap();
        std::fs::write(root.join("opencode.jsonc"), r#"{"permission":{"bash":"deny"}}"#).unwrap();
        std::fs::write(root.join(".opencode/agent/general.md"), "---\ntools:\n  write: false\n---\n").unwrap();
        let none = |_: &str| None;
        let r = user_rules(&sub, &none);
        assert!(r.global.contains("bash"));
        assert_eq!(r.agents.get("general"), Some(&BTreeSet::from(GATED)));
        let (files, _) = config_sources(&sub, &none);
        assert!(files.contains(&root.join("app/opencode.json")));
        assert!(!files.contains(&root.parent().unwrap().join("opencode.json")), "stops at the repository root");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn inline_config_the_user_set_is_kept() {
        let ours = ask_rules(&rules(&[r#"{"agent":{"build":{"permission":{"bash":"deny"}}}}"#])).unwrap();
        let merged = merge_rules(json!({"model":"m","agent":{"build":{"permission":{"bash":"deny"}}}}), &ours).unwrap();
        assert_eq!(merged["model"], "m");
        assert_eq!(merged["agent"]["build"]["permission"], json!({"bash":"deny","edit":"ask","webfetch":"ask"}));
        assert_eq!(merge_rules(json!({"agent":"odd"}), &ours), None);
    }

    #[test]
    fn hidden_agents_are_never_loosened() {
        let r = rules(&[r#"{"agent":{"title":{"model":"m"},"review":{}}}"#]);
        assert_eq!(agent_names(&ask_rules(&r).unwrap()), vec!["build", "explore", "general", "plan", "review"]);
    }
}
