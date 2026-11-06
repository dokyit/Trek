//! Permission rules: a short list the user writes in Settings › Permissions, each a pattern plus
//! allow, ask or deny, on top of the thread's hand-holding level. A deny holds at every level.
//!
//! Everything here fails closed. A rule that says no matches widely: anywhere on a command line,
//! whatever the quoting, in any flag order, and on anything that can't be read. A rule that says
//! yes matches narrowly: every command on the line, as literally written, with nothing it can't
//! read. A wrong answer then costs a question or a refusal the user can see in the tester, never
//! a command that got past them.
//!
//! The rules are read where Trek answers a permission (`trek_agents`, one place per agent). They
//! see what an agent is about to do, never what a program does once it runs.

mod shell;

pub use shell::{Line, Part, Word, WordKind, parse as parse_line, program_name};

use crate::HandHolding;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};

/// What a rule looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleKind {
    /// A command line, by its words.
    Command,
    /// A file an agent's edit tool would change, by its path.
    Edit,
    /// An address an agent would fetch or reach.
    Fetch,
    /// A tool, by the name its agent gives it (`WebSearch`, `mcp__github__*`).
    Tool,
}

impl RuleKind {
    pub const ALL: [RuleKind; 4] = [
        RuleKind::Command,
        RuleKind::Edit,
        RuleKind::Fetch,
        RuleKind::Tool,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RuleKind::Command => "Command",
            RuleKind::Edit => "File edit",
            RuleKind::Fetch => "Web fetch",
            RuleKind::Tool => "Tool",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleAction {
    Allow,
    Ask,
    Deny,
}

impl RuleAction {
    /// In the order they're tried.
    pub const ALL: [RuleAction; 3] = [RuleAction::Deny, RuleAction::Ask, RuleAction::Allow];

    pub fn label(self) -> &'static str {
        match self {
            RuleAction::Allow => "Allow",
            RuleAction::Ask => "Ask",
            RuleAction::Deny => "Deny",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRule {
    pub kind: RuleKind,
    pub pattern: String,
    pub action: RuleAction,
}

impl PermissionRule {
    /// "Deny command “rm”", for where a backtick would show as one.
    pub fn shown(&self) -> String {
        format!(
            "{} {} “{}”",
            self.action.label(),
            self.kind.label().to_lowercase(),
            self.pattern
        )
    }

    pub fn new(kind: RuleKind, pattern: impl Into<String>, action: RuleAction) -> Self {
        PermissionRule {
            kind,
            pattern: pattern.into(),
            action,
        }
    }

    /// "Deny command `rm`".
    pub fn describe(&self) -> String {
        format!(
            "{} {} `{}`",
            self.action.label(),
            self.kind.label().to_lowercase(),
            self.pattern
        )
    }
}

/// What's wrong with `pattern` as a rule, in words for the page. `None`: it's fine.
pub fn problem(kind: RuleKind, pattern: &str) -> Option<&'static str> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Some("Type what the rule is for.");
    }
    match kind {
        RuleKind::Command => {
            if pattern.starts_with('-') {
                return Some("Start with the program's name.");
            }
            // A rule names one command; a line is several.
            if pattern.contains([';', '|', '&', '<', '>', '`', '\n']) || pattern.contains("$(") {
                return Some("One command at a time. Make a rule for each.");
            }
            None
        }
        RuleKind::Edit | RuleKind::Fetch | RuleKind::Tool => None,
    }
}

/// What an agent is about to do, as a rule reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub kind: RuleKind,
    /// The command line, the path or the address. Empty: the agent didn't say (a deny matches,
    /// an allow doesn't).
    pub target: String,
    /// The agent's name for the tool (`Bash`, `Write`, `mcp__github__create_issue`).
    pub tool: String,
}

impl Call {
    pub fn command(tool: &str, line: impl Into<String>) -> Call {
        Call {
            kind: RuleKind::Command,
            target: line.into(),
            tool: tool.to_string(),
        }
    }

    pub fn edit(tool: &str, path: impl Into<String>) -> Call {
        Call {
            kind: RuleKind::Edit,
            target: path.into(),
            tool: tool.to_string(),
        }
    }

    pub fn fetch(tool: &str, address: impl Into<String>) -> Call {
        Call {
            kind: RuleKind::Fetch,
            target: address.into(),
            tool: tool.to_string(),
        }
    }

    pub fn tool(name: &str) -> Call {
        Call {
            kind: RuleKind::Tool,
            target: String::new(),
            tool: name.to_string(),
        }
    }
}

/// Which rule decided a call, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub action: RuleAction,
    /// Its place in the list.
    pub rule: usize,
    /// The part of the call it caught (`rm -rf x` of `rm -rf x; echo y`). `None`: all of it.
    pub part: Option<String>,
    /// It counted because this can't be read, not because it's known to match.
    pub unreadable: Option<&'static str>,
}

/// The rule that decides `call`: deny first, then ask, then allow, whatever their places in the
/// list (among rules of one action, the first named takes the credit). `None`: the thread's
/// level decides.
pub fn decide(rules: &[PermissionRule], call: &Call, cwd: Option<&Path>) -> Option<Verdict> {
    if rules.is_empty() {
        return None;
    }
    let line = (call.kind == RuleKind::Command).then(|| shell::parse(&call.target));
    for action in [RuleAction::Deny, RuleAction::Ask] {
        for (i, rule) in rules.iter().enumerate().filter(|(_, r)| r.action == action) {
            if let Some((part, unreadable)) = catches(rule, call, line.as_ref(), cwd) {
                return Some(Verdict {
                    action,
                    rule: i,
                    part,
                    unreadable,
                });
            }
        }
    }
    covered_by_allow(rules, call, line.as_ref(), cwd).map(|rule| Verdict {
        action: RuleAction::Allow,
        rule,
        part: None,
        unreadable: None,
    })
}

/// Whether a deny or ask `rule` could apply to `call`: what it caught, and if it only counts
/// because something can't be read, why.
fn catches(
    rule: &PermissionRule,
    call: &Call,
    line: Option<&Line>,
    cwd: Option<&Path>,
) -> Option<(Option<String>, Option<&'static str>)> {
    match rule.kind {
        RuleKind::Tool => (call.tool.is_empty()
            || glob(&rule.pattern.to_lowercase(), &call.tool.to_lowercase()))
        .then_some((None, None)),
        RuleKind::Command => {
            let line = line?;
            let pattern = command_pattern(&rule.pattern);
            if let Some(part) = line.parts.iter().find(|p| part_could_match(&pattern, p)) {
                return Some((Some(part.display()), None));
            }
            if let Some(why) = line.opaque {
                return Some((None, Some(why)));
            }
            call.target
                .trim()
                .is_empty()
                .then_some((None, Some("The agent didn't say what it would run.")))
        }
        RuleKind::Edit => {
            if call.kind != RuleKind::Edit {
                return None;
            }
            if call.target.trim().is_empty() {
                return Some((None, Some("The agent didn't say which file.")));
            }
            path_matches(&rule.pattern, &call.target, cwd, true).then_some((None, None))
        }
        RuleKind::Fetch => {
            if call.kind != RuleKind::Fetch {
                return None;
            }
            match address(&call.target) {
                Some(address) => {
                    address_matches(&rule.pattern, &address, true).then_some((None, None))
                }
                None => Some((None, Some("Trek can't read this address."))),
            }
        }
    }
}

/// The allow rule that covers all of `call`, if one does (for a command: an allow for each of its
/// commands, and nothing on the line that can't be read).
fn covered_by_allow(
    rules: &[PermissionRule],
    call: &Call,
    line: Option<&Line>,
    cwd: Option<&Path>,
) -> Option<usize> {
    let allows = || {
        rules
            .iter()
            .enumerate()
            .filter(|(_, r)| r.action == RuleAction::Allow)
    };
    match call.kind {
        RuleKind::Command => {
            let line = line?;
            if line.opaque.is_some() || call.target.trim().is_empty() {
                return None;
            }
            // An allow on the shell tool itself: every command, as the user asked.
            let by_tool = allows().find(|(_, r)| {
                r.kind == RuleKind::Tool
                    && !call.tool.is_empty()
                    && glob(&r.pattern.to_lowercase(), &call.tool.to_lowercase())
            });
            if let Some((i, _)) = by_tool {
                return Some(i);
            }
            let mut first = None;
            for part in line.parts.iter().filter(|p| !p.transparent) {
                let by = allows()
                    .filter(|(_, r)| r.kind == RuleKind::Command)
                    .find(|(_, r)| part_is_literally(&command_pattern(&r.pattern), part))?;
                first.get_or_insert(by.0);
            }
            first
        }
        RuleKind::Edit | RuleKind::Fetch | RuleKind::Tool => {
            let by_tool = allows()
                .find(|(_, r)| {
                    r.kind == RuleKind::Tool
                        && !call.tool.is_empty()
                        && glob(&r.pattern.to_lowercase(), &call.tool.to_lowercase())
                })
                .map(|(i, _)| i);
            let own = allows().find(|(_, r)| {
                r.kind == call.kind
                    && match call.kind {
                        RuleKind::Edit => {
                            !call.target.trim().is_empty()
                                && path_matches(&r.pattern, &call.target, cwd, false)
                        }
                        RuleKind::Fetch => address(&call.target)
                            .is_some_and(|a| address_matches(&r.pattern, &a, false)),
                        _ => false,
                    }
            });
            own.map(|(i, _)| i).or(by_tool)
        }
    }
}

fn command_pattern(pattern: &str) -> Vec<String> {
    pattern.split_whitespace().map(str::to_string).collect()
}

/// Whether a deny or ask for `pattern` catches this command: it's that program whatever its path
/// or case, and it has the pattern's other words in order and its flags anywhere, with room for
/// what can't be known. `rm -rf` catches `/bin/RM -f -r x`; `git push` catches `git -C y push`.
fn part_could_match(pattern: &[String], part: &Part) -> bool {
    let Some(program) = pattern.first() else {
        return false;
    };
    if part.words.is_empty() {
        return false;
    }
    let starts = 0..part.wraps_from.map_or(1, |_| part.words.len());
    for start in starts {
        let words = &part.words[start..];
        // A wrapper's words past the first may be its own flags: they can't start a command.
        if start > 0 && words[0].text.starts_with('-') {
            continue;
        }
        if program_could_be(program, &words[0]) {
            // Not known yet, it could be the whole command at once: `$X` may hold `git push`.
            if words[0].kind != WordKind::Fixed || rest_could_match(&pattern[1..], &words[1..]) {
                return true;
            }
        }
    }
    if let Some(code) = &part.code {
        // Code in a string: a name that's in it counts, however it would run.
        let words: Vec<Word> = code
            .split(|c: char| !(c.is_alphanumeric() || "_-./+".contains(c)))
            .filter(|t| !t.is_empty())
            .map(Word::fixed)
            .collect();
        for start in 0..words.len() {
            if program_could_be(program, &words[start])
                && rest_could_match(&pattern[1..], &words[start + 1..])
            {
                return true;
            }
        }
    }
    false
}

fn program_could_be(pattern: &str, word: &Word) -> bool {
    if word.kind != WordKind::Fixed {
        return true;
    }
    let wanted = program_name(pattern);
    glob(&wanted, &program_name(&word.text))
        || word
            .raw
            .as_deref()
            .is_some_and(|raw| glob(&wanted, &program_name(raw)))
}

/// The rest of a deny's words against the rest of a command's: plain words in order, flags
/// anywhere. A word that isn't known yet could be any of them.
fn rest_could_match(pattern: &[String], words: &[Word]) -> bool {
    if words.iter().any(|w| w.kind != WordKind::Fixed) {
        return true;
    }
    let (flags, plain): (Vec<&String>, Vec<&String>) =
        pattern.iter().partition(|p| p.starts_with('-'));
    let mut rest = words.iter().filter(|w| !w.text.starts_with('-'));
    let in_order = plain.iter().all(|p| {
        let p = p.to_lowercase();
        rest.any(|w| glob(&p, &w.text.to_lowercase()))
    });
    in_order && flags.iter().all(|flag| flag_present(flag, words))
}

/// Whether the command has `flag`, the way tools read flags: short ones cluster (`-rf` is `-f
/// -r`), case aside, long ones can be cut short or carry a value, and a short and a long with
/// the same first letter count as the same (`-f` for `--force`: wrong sometimes, on the safe side).
fn flag_present(flag: &str, words: &[Word]) -> bool {
    let flags: Vec<String> = words
        .iter()
        .map(|w| w.text.to_lowercase())
        .filter(|w| w.starts_with('-') && w.len() > 1)
        .collect();
    let flag = flag.to_lowercase();
    if let Some(long) = flag.strip_prefix("--") {
        let initial = long.chars().next();
        return flags.iter().any(|w| match w.strip_prefix("--") {
            Some(have) => {
                let name = have.split('=').next().unwrap_or(have);
                name == long || (name.len() >= 3 && long.starts_with(name))
            }
            None => initial.is_some_and(|i| w[1..].contains(i)),
        });
    }
    let letters = flag.trim_start_matches('-');
    letters.chars().all(|letter| {
        flags.iter().any(|w| match w.strip_prefix("--") {
            Some(have) => have.starts_with(letter),
            None => w[1..].contains(letter),
        })
    })
}

/// Whether an allow for `pattern` covers this command: the pattern's words, literally, at its
/// start, with nothing on it only known when it runs and no file written. `/tmp/git status` and
/// `PATH=/tmp git status` aren't `git status`.
fn part_is_literally(pattern: &[String], part: &Part) -> bool {
    if part.writes
        || part
            .words
            .iter()
            .chain(part.assignments.iter())
            .any(|w| w.kind == WordKind::Dynamic)
        || part.words.first().is_none_or(|w| w.kind != WordKind::Fixed)
    {
        return false;
    }
    let words: Vec<&str> = part
        .assignments
        .iter()
        .chain(part.words.iter())
        .map(|w| w.text.as_str())
        .collect();
    pattern.len() <= words.len() && pattern.iter().zip(&words).all(|(p, w)| glob(p, w))
}

/// `*` for any run of characters, `?` for one.
pub fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let mut back: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) && p[pi] != '*' {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            back = Some((pi, ti));
            pi += 1;
        } else if let Some((star, at)) = back {
            pi = star + 1;
            ti = at + 1;
            back = Some((star, at + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// A path's places to compare: as it reads with `..` gone, and with its symlinks followed, which
/// is where a write would really land.
fn path_forms(target: &str, cwd: Option<&Path>) -> Vec<PathBuf> {
    let target = target.trim();
    let expanded = match target.strip_prefix("~/") {
        Some(rest) => crate::paths::home().join(rest),
        None => PathBuf::from(target),
    };
    let absolute = match (expanded.is_absolute(), cwd) {
        (false, Some(cwd)) => cwd.join(&expanded),
        _ => expanded,
    };
    let lexical = clean(&absolute);
    let mut forms = vec![lexical.clone()];
    let mut base = lexical.clone();
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = base.canonicalize() {
            let real = rest
                .iter()
                .rev()
                .fold(real, |acc: PathBuf, part| acc.join(part));
            if !forms.contains(&real) {
                forms.push(real);
            }
            break;
        }
        match (base.file_name().map(|n| n.to_os_string()), base.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                base = parent.to_path_buf();
            }
            _ => break,
        }
    }
    forms
}

fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn slashed(path: &Path) -> String {
    // Windows's long form of a real path (`\\?\C:\x`) is the same place as `C:\x`.
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\")
        .unwrap_or(&text)
        .replace('\\', "/")
}

/// Whether a file rule's `pattern` applies to `target`. A bare name (`.env`, `*.lock`) means that
/// name anywhere, a pattern with a slash means that path and what's under it: from the project
/// unless it starts at `/` or `~/`. `wide` (deny, ask): any spelling of the path, case aside, and
/// a bare name also matches a folder on the way. Otherwise (allow): every spelling must match.
fn path_matches(pattern: &str, target: &str, cwd: Option<&Path>, wide: bool) -> bool {
    let forms = path_forms(target, cwd);
    let roots: Vec<PathBuf> = cwd
        .map(|cwd| {
            let lexical = clean(cwd);
            let mut roots = vec![lexical.clone()];
            if let Ok(real) = lexical.canonicalize()
                && real != lexical
            {
                roots.push(real);
            }
            roots
        })
        .unwrap_or_default();
    let mut one = forms
        .iter()
        .map(|form| form_matches(pattern, form, &roots, wide));
    if wide { one.any(|m| m) } else { one.all(|m| m) }
}

fn form_matches(pattern: &str, form: &Path, roots: &[PathBuf], wide: bool) -> bool {
    let fold = |s: &str| {
        if wide {
            s.to_lowercase()
        } else {
            s.to_string()
        }
    };
    let mut pattern = fold(pattern.trim()).replace('\\', "/");
    // Windows's long form (`\\?\C:\x`), as `slashed` reads a path: the same place as `C:/x`.
    if let Some(rest) = pattern.strip_prefix("//?/") {
        pattern = rest.to_string();
    }
    if pattern.len() > 1 && pattern.ends_with('/') {
        pattern.pop();
    }
    let path = fold(&slashed(form));
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if !pattern.contains('/') {
        return if wide {
            parts.iter().any(|p| glob(&pattern, p))
        } else {
            parts.last().is_some_and(|p| glob(&pattern, p))
        };
    }
    let whole = if let Some(rest) = pattern.strip_prefix("~/") {
        let home = fold(&slashed(&crate::paths::home()));
        let pattern = format!("{home}/{rest}");
        components_match(&split_path(&pattern), &parts)
    } else if pattern.starts_with('/') || pattern.get(1..3) == Some(":/") {
        components_match(&split_path(&pattern), &parts)
    } else {
        // From the project; with none (the tester, no project open), the path as typed.
        let wanted = split_path(&pattern);
        if roots.is_empty() {
            components_match(&wanted, &parts) || tail_match(&wanted, &parts)
        } else {
            roots.iter().any(|root| {
                let root = fold(&slashed(root));
                let root: Vec<&str> = root.split('/').filter(|p| !p.is_empty()).collect();
                parts.len() >= root.len()
                    && parts[..root.len()] == root[..]
                    && components_match(&wanted, &parts[root.len()..])
            })
        }
    };
    whole
}

fn split_path(pattern: &str) -> Vec<&str> {
    pattern.split('/').filter(|p| !p.is_empty()).collect()
}

fn tail_match(pattern: &[&str], parts: &[&str]) -> bool {
    (0..parts.len()).any(|i| components_match(pattern, &parts[i..]))
}

/// Components against components: `**` for any number of them. Past the pattern's end what's left
/// is what's under it.
fn components_match(pattern: &[&str], parts: &[&str]) -> bool {
    match pattern.first() {
        None => true,
        Some(&"**") => (0..=parts.len()).any(|i| components_match(&pattern[1..], &parts[i..])),
        Some(p) => {
            !parts.is_empty() && glob(p, parts[0]) && components_match(&pattern[1..], &parts[1..])
        }
    }
}

/// An address, split the way a client reads it. `https://good.com@evil.com/` goes to `evil.com`.
struct Address {
    host: String,
    /// From the first slash, or empty.
    path: String,
}

fn address(target: &str) -> Option<Address> {
    let target = target.trim().replace('\\', "/");
    if target.is_empty() || target.chars().any(char::is_whitespace) {
        return None;
    }
    let after_scheme = target.split_once("://").map_or(target.as_str(), |(_, r)| r);
    let (authority, path) = match after_scheme.find(['/', '?', '#']) {
        Some(at) => after_scheme.split_at(at),
        None => (after_scheme, ""),
    };
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = match host_port.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(v6),
        None => host_port.split(':').next().unwrap_or(host_port),
    };
    if host.is_empty() {
        return None;
    }
    Some(Address {
        host: host.trim_end_matches('.').to_lowercase(),
        path: path.to_string(),
    })
}

/// Whether a fetch rule's `pattern` applies. A bare host (`example.com`, `*.example.com`) is about
/// the host, wherever on it; a deny or ask for one also covers its subdomains. A pattern with a
/// slash is about the host and path together.
fn address_matches(pattern: &str, address: &Address, wide: bool) -> bool {
    let pattern = pattern.trim().to_lowercase();
    let pattern = pattern
        .split_once("://")
        .map_or(pattern.as_str(), |(_, r)| r);
    if pattern.contains('/') {
        let path = if wide {
            address.path.to_lowercase()
        } else {
            address.path.clone()
        };
        return glob(pattern, &format!("{}{}", address.host, path));
    }
    let pattern = pattern.trim_end_matches('.');
    glob(pattern, &address.host) || (wide && address.host.ends_with(&format!(".{pattern}")))
}

/// What would happen to a call at one level, for the tester.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Refused,
    Asks,
    Runs,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Refused => "Refused",
            Outcome::Asks => "Asks",
            Outcome::Runs => "Runs",
        }
    }

    /// `verdict` on top of what the level would do by itself.
    pub fn of(verdict: Option<&Verdict>, level_asks: bool) -> Outcome {
        match verdict.map(|v| v.action) {
            Some(RuleAction::Deny) => Outcome::Refused,
            Some(RuleAction::Ask) => Outcome::Asks,
            Some(RuleAction::Allow) => Outcome::Runs,
            None if level_asks => Outcome::Asks,
            None => Outcome::Runs,
        }
    }
}

/// What a refused agent is told: why, and not to look for a way round.
pub fn refusal(rule: &PermissionRule) -> String {
    format!(
        "Refused by a permission rule the user set in Trek ({}). It holds at every access level. Don't try another way to do this: say what you wanted to do, and let the user decide.",
        rule.describe()
    )
}

/// What the user sees in the transcript when a rule stops something.
pub fn stopped_note(rule: &PermissionRule, call: &Call) -> String {
    let target = if call.target.is_empty() {
        call.tool.as_str()
    } else {
        call.target.as_str()
    };
    let one_line: String = target.split_whitespace().collect::<Vec<_>>().join(" ");
    let shown: String = one_line.chars().take(120).collect();
    let cut = if one_line.chars().count() > 120 {
        "…"
    } else {
        ""
    };
    format!("Your rule stopped `{shown}{cut}` ({}).", rule.describe())
}

/// The rule "Always allow…" offers for `call`, or `None` when no allow could cover it (more than
/// one command, or something it can't read). As narrow as it can be while still useful: a tool
/// with subcommands by its subcommand, anything else as it was written.
pub fn allow_draft(call: &Call, cwd: Option<&Path>) -> Option<PermissionRule> {
    let pattern = match call.kind {
        RuleKind::Command => command_draft(&call.target)?,
        RuleKind::Edit => {
            if call.target.trim().is_empty() {
                return None;
            }
            let forms = path_forms(&call.target, cwd);
            let form = forms.first()?;
            match cwd.and_then(|cwd| form.strip_prefix(clean(cwd)).ok()) {
                Some(inside) if !inside.as_os_str().is_empty() => slashed(inside),
                _ => crate::paths::tildify(form),
            }
        }
        RuleKind::Fetch => address(&call.target)?.host,
        RuleKind::Tool => {
            if call.tool.is_empty() {
                return None;
            }
            call.tool.clone()
        }
    };
    Some(PermissionRule::new(call.kind, pattern, RuleAction::Allow))
}

/// Tools whose first plain word says what they do, and the ones where two words do.
const SUBCOMMAND_TOOLS: &[&str] = &[
    "git", "cargo", "npm", "pnpm", "yarn", "bun", "deno", "go", "pip", "pip3", "uv", "brew",
    "docker", "kubectl", "gh", "swift", "dotnet", "rustup", "make",
];
const RUN_WORDS: &[&str] = &["run", "exec", "x"];

fn command_draft(line: &str) -> Option<String> {
    let parsed = shell::parse(line);
    if parsed.opaque.is_some() {
        return None;
    }
    let mut own = parsed.parts.iter().filter(|p| !p.transparent);
    let part = own.next()?;
    if own.next().is_some()
        || part.writes
        || !part.assignments.is_empty()
        || part.words.iter().any(|w| w.kind == WordKind::Dynamic)
        || part.words.first().is_none_or(|w| w.kind != WordKind::Fixed)
    {
        return None;
    }
    let words: Vec<&str> = part.words.iter().map(|w| w.text.as_str()).collect();
    let program = *words.first()?;
    if SUBCOMMAND_TOOLS.contains(&program) {
        // Up to the first flag or value: `git push`, `npm run test`.
        let mut take = 1;
        if let Some(sub) = words.get(1).filter(|w| !w.starts_with('-')) {
            take = 2;
            if RUN_WORDS.contains(sub) && words.get(2).is_some_and(|w| !w.starts_with('-')) {
                take = 3;
            }
        }
        return Some(words[..take].join(" "));
    }
    Some(words.join(" "))
}

/// The rules in force in this process: what the agents' permission paths read. Set from
/// Settings; a rule added or removed applies from the next decision, in sessions already running.
static CURRENT: LazyLock<RwLock<Arc<Vec<PermissionRule>>>> = LazyLock::new(Default::default);

pub fn set_current(rules: &[PermissionRule]) {
    let mut current = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    if current.as_slice() != rules {
        *current = Arc::new(rules.to_vec());
    }
}

pub fn current() -> Arc<Vec<PermissionRule>> {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether `rules` would stop or question an agent's commands or edits, so a session has to let
/// Trek see each one (Codex, which otherwise runs what its sandbox allows without a word).
pub fn watches_work(rules: &[PermissionRule]) -> bool {
    rules
        .iter()
        .any(|r| r.action != RuleAction::Allow && r.kind != RuleKind::Fetch)
}

/// A short phrase of how a call fared at a level, for tests and logs.
pub fn outcome_at(
    rules: &[PermissionRule],
    call: &Call,
    cwd: Option<&Path>,
    level_asks: impl Fn(HandHolding) -> bool,
) -> Vec<(HandHolding, Outcome)> {
    let verdict = decide(rules, call, cwd);
    HandHolding::ALL
        .into_iter()
        .map(|level| (level, Outcome::of(verdict.as_ref(), level_asks(level))))
        .collect()
}

#[cfg(test)]
mod tests;
