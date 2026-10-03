//! OpenCode runs edits and commands without asking unless its config says otherwise, but Trek's
//! access levels need those prompts. Sessions therefore start with "ask" rules for the agents
//! Trek drives, added only where the user's own OpenCode config says nothing: OpenCode applies
//! agent rules after everything else, so a rule Trek adds would replace one the user wrote (a
//! deny, a pattern list, `tools: false`). Plan and explore keep their own edit rules (deny), so
//! plan mode still can't change files.
//!
//! The user's config is read from the places OpenCode reads it on this machine. Remote and
//! organisation configs aren't fetched.

use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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

/// The gated permissions a permission or tools key covers. Edit tools all fall under "edit".
fn gated(key: &str) -> impl Iterator<Item = &'static str> + '_ {
    GATED.into_iter().filter(move |g| {
        let names: &[&str] = if *g == "edit" { &["edit", "write", "patch", "apply_patch", "multiedit"] } else { &[g] };
        names.iter().any(|n| glob(key, n))
    })
}

/// Gated permissions a config section (the top level, or one agent) sets.
fn set_in(section: &Value) -> BTreeSet<&'static str> {
    let mut out = BTreeSet::new();
    match &section["permission"] {
        Value::Null => {}
        Value::Object(rules) => out.extend(rules.keys().flat_map(|k| gated(k))),
        _ => out.extend(GATED),
    }
    for k in section["tools"].as_object().into_iter().flat_map(|t| t.keys()) {
        out.extend(gated(k));
    }
    out
}

impl UserRules {
    fn add_config(&mut self, c: &Value) {
        self.global.extend(set_in(c));
        for (name, agent) in ["agent", "mode"].iter().filter_map(|k| c[*k].as_object()).flatten() {
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
        if front.iter().any(|l| l.starts_with("permission:") || l.starts_with("tools:")) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
