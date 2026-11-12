//! Trek runs on macOS and Windows from one tree, and a feature or a fix made on one of them can
//! quietly skip the other: its other side is a `cfg` branch nobody ran. This crate makes each such
//! difference a decision someone wrote down, checked by `cargo test` on every platform's CI:
//!
//! - The ledger (`docs/parity/ledger.toml`): every user-facing feature, what it does on each
//!   platform, and for one that's missing or partial somewhere, why and what would bring it.
//! - The gate count (`docs/parity/gates.toml`): how many platform gates (`cfg(windows)`,
//!   `cfg!(target_os = "macos")`, ...) each source file has. A file gaining or losing one fails the
//!   check until the count is recorded again, in the same change as the ledger entry that says
//!   what the other platform does there. The count's diff is how a review sees it happen.
//! - Ownership: a file with gates belongs to a feature, which names it in its `code` or `tests`.
//!   The files from before the rule are listed as `unclaimed` in `gates.toml`; that list only
//!   shrinks, and a file new to it, or one on it that gains a gate, fails even when the counts are
//!   being recorded: a feature naming the file is the only way through.
//!
//! See `docs/CROSS_PLATFORM.md` for the reasoning and the rest of the plan.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The names a `cfg` uses to pick a platform. A gate on a feature or on `test` isn't one.
const PLATFORM_WORDS: &[&str] = &[
    "target_os",
    "target_family",
    "target_vendor",
    "windows",
    "unix",
];

/// Trek's own compile-time switches between platforms, which aren't spelled `cfg`: the Windows
/// styling switch (`fluent::WINDOWS`, a `cfg!(windows)`) and its pick between two values. Three
/// fixes made on Windows sat behind it, so the Mac never had them: a branch on it is a gate.
const PLATFORM_SWITCHES: &[&str] = &["fluent::WINDOWS", "fluent::pick("];

/// How many platform gates `source` (a Rust file) has: each `cfg(…)`, `cfg!(…)` and `cfg_attr(…)`
/// whose condition names a platform, wherever it's written (an attribute rustfmt spread over
/// several lines counts once), and each use of a `PLATFORM_SWITCHES`. Line comments are passed
/// over: a comment about a gate isn't one.
pub fn platform_gates(source: &str) -> usize {
    let code: String = source
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let bytes = code.as_bytes();
    let mut count = 0;
    let mut at = 0;
    while let Some(found) = code[at..].find("cfg") {
        let start = at + found;
        at = start + 3;
        // `cfg` on its own, not the end of another name (`mycfg`).
        if start > 0 && is_ident(bytes[start - 1]) {
            continue;
        }
        let mut i = at;
        if code[i..].starts_with("_attr") {
            i += "_attr".len();
        }
        if code[i..].starts_with('!') {
            i += 1;
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'(' {
            continue;
        }
        let Some(end) = closing(bytes, i) else {
            continue;
        };
        if names_a_platform(&code[i + 1..end]) {
            count += 1;
        }
        at = end;
    }
    count
        + PLATFORM_SWITCHES
            .iter()
            .map(|switch| {
                code.match_indices(switch)
                    .filter(|(i, _)| {
                        // The name itself, not a longer one it starts (`fluent::WINDOWS_X`).
                        switch.ends_with('(')
                            || !code[i + switch.len()..]
                                .bytes()
                                .next()
                                .is_some_and(is_ident)
                    })
                    .count()
            })
            .sum::<usize>()
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The `)` that closes the `(` at `open`.
fn closing(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// A condition names a platform: one of `PLATFORM_WORDS` as a whole word.
fn names_a_platform(condition: &str) -> bool {
    PLATFORM_WORDS.iter().any(|word| {
        condition.match_indices(word).any(|(i, _)| {
            let before = condition[..i].bytes().last();
            let after = condition[i + word.len()..].bytes().next();
            !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
        })
    })
}

/// The Rust files of the workspace's crates whose gates are counted, by their path from `root`
/// with `/` between folders: every crate but this one (its tests hold gates as examples).
pub fn counted_files(root: &Path) -> Vec<(String, PathBuf)> {
    let crates = root.join("crates");
    let mut files: Vec<(String, PathBuf)> = walkdir::WalkDir::new(&crates)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !(e.file_type().is_dir() && (name == "target" || name.starts_with('.')))
        })
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "rs"))
        .map(|e| {
            let shown = e
                .path()
                .strip_prefix(root)
                .unwrap_or(e.path())
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            (shown, e.path().to_path_buf())
        })
        .filter(|(shown, _)| !shown.starts_with("crates/trek-parity/"))
        .collect();
    files.sort();
    files
}

/// Each counted file's platform gates, leaving out the files with none.
pub fn census(root: &Path) -> BTreeMap<String, usize> {
    counted_files(root)
        .into_iter()
        .filter_map(|(shown, path)| {
            let text = std::fs::read_to_string(&path).ok()?;
            let n = platform_gates(&text);
            (n > 0).then_some((shown, n))
        })
        .collect()
}

/// The gate counts recorded in `docs/parity/gates.toml`, and the gated files no feature claims
/// yet (`unclaimed`), which only shrinks. A file without the list predates the rule: recording the
/// counts adopts the files unclaimed then, once.
#[derive(Debug, Default, Deserialize)]
pub struct Recorded {
    #[serde(default)]
    pub unclaimed: Option<BTreeSet<String>>,
    #[serde(default)]
    pub gates: BTreeMap<String, usize>,
}

/// The files with platform gates (`found`, a `census`) that no feature in `ledger` names in its
/// `code` or `tests`: differences between the platforms nobody has tied to what a user sees.
pub fn unclaimed(found: &BTreeMap<String, usize>, ledger: &Ledger) -> BTreeSet<String> {
    let claimed: BTreeSet<&str> = ledger
        .feature
        .iter()
        .flat_map(|f| f.code.iter().chain(&f.tests))
        .map(String::as_str)
        .collect();
    found
        .keys()
        .filter(|file| !claimed.contains(file.as_str()))
        .cloned()
        .collect()
}

/// The gated files no feature claims that can't be recorded as they are, one line each: a file
/// unclaimed now that wasn't before (`recorded`, the list in `docs/parity/gates.toml`), and one
/// on that list that gained gates (`was` the recorded counts, `found` the census). Each is a new
/// difference between the platforms that no feature owns; recording the counts again doesn't
/// clear it: a feature in the ledger naming the file does.
pub fn unclaimed_growth(
    recorded: &BTreeSet<String>,
    unclaimed: &BTreeSet<String>,
    was: &BTreeMap<String, usize>,
    found: &BTreeMap<String, usize>,
) -> Vec<String> {
    const CLAIM: &str = "Add it to the `code` or `tests` of the feature in docs/parity/ledger.toml whose platforms it makes differ (or add that feature), so the difference is one someone wrote down.";
    unclaimed
        .iter()
        .filter_map(|file| {
            let now = found.get(file).copied().unwrap_or(0);
            if !recorded.contains(file) {
                Some(format!(
                    "{file}: {now} platform gate(s), and no feature claims it. {CLAIM}"
                ))
            } else {
                let before = was.get(file).copied().unwrap_or(0);
                (now > before).then(|| {
                    format!(
                        "{file}: {now} platform gates, {} more than recorded, and no feature claims it yet. {CLAIM}",
                        now - before
                    )
                })
            }
        })
        .collect()
}

/// The files on the recorded `unclaimed` list that aren't unclaimed any more (a feature names
/// them, or their gates went): recording the counts again takes them off.
pub fn claimed_since(recorded: &BTreeSet<String>, unclaimed: &BTreeSet<String>) -> Vec<String> {
    recorded
        .difference(unclaimed)
        .map(|file| {
            format!(
                "{file}: a feature claims it now, or it has no gates left: it comes off docs/parity/gates.toml's `unclaimed`."
            )
        })
        .collect()
}

/// The files of `unclaimed` (now) that may be recorded as unclaimed: those that already were.
/// The list only shrinks; a newly unclaimed file is refused by `claim_changes`.
pub fn still_unclaimed(
    recorded: &BTreeSet<String>,
    unclaimed: &BTreeSet<String>,
) -> BTreeSet<String> {
    recorded.intersection(unclaimed).cloned().collect()
}

/// What `found` (a `census`) says that `recorded` doesn't, one line for each file, in words that
/// say what to do about it. Empty when they agree.
pub fn gate_changes(
    recorded: &BTreeMap<String, usize>,
    found: &BTreeMap<String, usize>,
) -> Vec<String> {
    let files: BTreeSet<&String> = recorded.keys().chain(found.keys()).collect();
    let gates = |n: usize| {
        if n == 1 {
            "1 platform gate".to_string()
        } else {
            format!("{n} platform gates")
        }
    };
    files
        .into_iter()
        .filter_map(|file| {
            let was = recorded.get(file).copied().unwrap_or(0);
            let now = found.get(file).copied().unwrap_or(0);
            match now.cmp(&was) {
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(format!(
                    "{file}: {}, {} more than recorded ({was}). Each is a place one platform does something the other doesn't: say what the other one does in docs/parity/ledger.toml (the feature's `how`, or `partial`/`no` with a note), or make the code the same on both.",
                    gates(now),
                    now - was
                )),
                std::cmp::Ordering::Less => Some(format!(
                    "{file}: {}, {} fewer than recorded ({was}). If a difference went, update its feature in docs/parity/ledger.toml.",
                    gates(now),
                    was - now
                )),
            }
        })
        .collect()
}

/// `found` and `unclaimed` as `docs/parity/gates.toml` records them.
pub fn render_gates(found: &BTreeMap<String, usize>, unclaimed: &BTreeSet<String>) -> String {
    let mut out = String::from(
        "# How many platform gates (`cfg(windows)`, `cfg!(target_os = \"macos\")`, ...) each file has.\n\
         # Written by `TREK_PARITY_UPDATE=1 cargo test -p trek-parity`, not by hand: a change to a count\n\
         # goes with the change to docs/parity/ledger.toml that says what each platform does there.\n\
         #\n\
         # `unclaimed`: the gated files no feature in the ledger names in its `code` or `tests` yet. It\n\
         # only shrinks: a file that gains gates must belong to a feature, and one claimed comes off.\n\
         \nunclaimed = [\n",
    );
    for file in unclaimed {
        out.push_str(&format!("    \"{file}\",\n"));
    }
    out.push_str("]\n\n[gates]\n");
    for (file, n) in found {
        out.push_str(&format!("\"{file}\" = {n}\n"));
    }
    out
}

/// What a feature does on one platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Support {
    /// It works as on the other platform (in that platform's own way, if `how` says so).
    Yes,
    /// It works, but with less: `note` says what's missing.
    Partial,
    /// It isn't there: `note` says why, and what would bring it.
    No,
    /// It has no meaning there (a Dock badge on Windows).
    #[serde(rename = "n/a")]
    NotApplicable,
}

/// One user-facing feature in the ledger.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feature {
    /// Kebab-case, unique.
    pub id: String,
    /// Where it is in Trek: "Composer", "Tools", "Agents".
    pub area: String,
    /// What it is, for someone using Trek.
    pub what: String,
    pub macos: Support,
    pub windows: Support,
    /// How each platform does it, where they differ.
    #[serde(default)]
    pub how: Option<How>,
    /// Why a platform is `partial` or `no`, and what would bring it.
    #[serde(default)]
    pub note: Option<String>,
    /// Where its platform split is in the code (files that exist).
    #[serde(default)]
    pub code: Vec<String>,
    /// The tests that cover it (files that exist), on whichever platforms they run.
    #[serde(default)]
    pub tests: Vec<String>,
    /// What it holds to beyond being there (`[[feature.promise]]`).
    #[serde(default)]
    pub promise: Vec<Promise>,
}

/// A behaviour a feature keeps on each platform it's on, beyond being there: what a fix made on
/// one platform promised, so the other is held to it too. "yes" on both sides of the ledger said
/// nothing of Windows' dictation writing "use the chain rule" where the Mac's writes "Use the
/// chain rule.": a promise names, for each platform the feature is on, the test that checks it
/// there (`path::name`, a `fn name` in that file, which its `cfg` doesn't keep off that
/// platform), or says why there isn't one (`untested`). One test that runs on both can be named
/// on both.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Promise {
    pub what: String,
    #[serde(default)]
    pub macos: Option<String>,
    #[serde(default)]
    pub windows: Option<String>,
    #[serde(default)]
    pub untested: Option<Untested>,
}

/// Why a platform has no test holding it to a promise.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Untested {
    #[serde(default)]
    pub macos: Option<String>,
    #[serde(default)]
    pub windows: Option<String>,
}

/// The two platforms, as the ledger names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Windows,
}

impl Platform {
    fn name(self) -> &'static str {
        match self {
            Platform::Macos => "macOS",
            Platform::Windows => "Windows",
        }
    }
}

/// Where `fn name` is in `source`, and the one platform its attributes keep it to, if they do
/// (`cfg(windows)`; `cfg(target_os = "macos")`, `cfg(unix)` or `cfg(not(windows))`). `None` when
/// there's no such `fn`. A gate on the module the file is (in its parent) isn't seen.
pub fn test_runs_on(source: &str, name: &str) -> Option<Option<Platform>> {
    let lines: Vec<&str> = source.lines().collect();
    let at = lines.iter().position(|l| {
        let l = l.trim_start();
        let l = l.strip_prefix("async ").unwrap_or(l);
        l.strip_prefix("fn ")
            .and_then(|rest| rest.strip_prefix(name))
            .is_some_and(|rest| rest.starts_with('(') || rest.starts_with('<'))
    })?;
    let mut only = None;
    for line in lines[..at].iter().rev() {
        let l = line.trim();
        if l.is_empty() || l.starts_with("//") {
            continue;
        }
        if !l.starts_with("#[") {
            break;
        }
        let squeezed: String = l.chars().filter(|c| !c.is_whitespace()).collect();
        if squeezed.contains("cfg(windows)") || squeezed.contains("target_os=\"windows\"") {
            only = Some(Platform::Windows);
        } else if squeezed.contains("target_os=\"macos\"")
            || squeezed.contains("cfg(unix)")
            || squeezed.contains("cfg(not(windows))")
        {
            only = Some(Platform::Macos);
        }
    }
    Some(only)
}

/// What's wrong with `feature`'s promises, read against the tree at `root`.
fn promise_problems(f: &Feature, root: &Path) -> Vec<String> {
    let mut problems = vec![];
    let id = &f.id;
    for p in &f.promise {
        let what = p.what.trim();
        if what.is_empty() {
            problems.push(format!("{id}: a promise that says nothing"));
            continue;
        }
        let untested = p.untested.clone().unwrap_or_default();
        for (platform, support, test, why) in [
            (Platform::Macos, f.macos, &p.macos, &untested.macos),
            (Platform::Windows, f.windows, &p.windows, &untested.windows),
        ] {
            let on = matches!(support, Support::Yes | Support::Partial);
            let said = why.as_deref().is_some_and(|w| !w.trim().is_empty());
            let platform_name = platform.name();
            if !on {
                if test.is_some() || why.is_some() {
                    problems.push(format!(
                        "{id}: \"{what}\" names {platform_name}, where the feature isn't"
                    ));
                }
                continue;
            }
            let Some(test) = test else {
                if !said {
                    problems.push(format!(
                        "{id}: \"{what}\" has no test on {platform_name}, and no word in `untested` on why"
                    ));
                }
                continue;
            };
            if said {
                problems.push(format!(
                    "{id}: \"{what}\" names a test on {platform_name} and says it's untested there"
                ));
            }
            let Some((path, name)) = test.split_once("::") else {
                problems.push(format!("{id}: \"{what}\": {test} isn't `path::test_name`"));
                continue;
            };
            let Ok(source) = std::fs::read_to_string(root.join(path)) else {
                problems.push(format!("{id}: \"{what}\": {path} isn't there"));
                continue;
            };
            match test_runs_on(&source, name) {
                None => problems.push(format!("{id}: \"{what}\": {path} has no `fn {name}`")),
                Some(Some(only)) if only != platform => problems.push(format!(
                    "{id}: \"{what}\": `{name}` runs on {} only, so it holds {platform_name} to nothing",
                    only.name()
                )),
                Some(_) => {}
            }
        }
    }
    problems
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct How {
    pub macos: String,
    pub windows: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    pub feature: Vec<Feature>,
}

/// What's wrong with `ledger`, read against the tree at `root`: one line each. Empty when nothing is.
pub fn ledger_problems(ledger: &Ledger, root: &Path) -> Vec<String> {
    let mut problems = vec![];
    let mut seen = BTreeSet::new();
    for f in &ledger.feature {
        let id = &f.id;
        if !seen.insert(id.as_str()) {
            problems.push(format!("{id}: listed twice"));
        }
        if id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            problems.push(format!("{id:?}: an id is kebab-case"));
        }
        if f.area.trim().is_empty() || f.what.trim().is_empty() {
            problems.push(format!("{id}: says nothing of where it is or what it does"));
        }
        let lacking = [f.macos, f.windows]
            .iter()
            .any(|s| matches!(s, Support::Partial | Support::No));
        let noted = f.note.as_deref().is_some_and(|n| !n.trim().is_empty());
        if lacking && !noted {
            problems.push(format!(
                "{id}: partial or missing on a platform without a note saying why, and what would bring it"
            ));
        }
        if f.macos == Support::NotApplicable && f.windows == Support::NotApplicable {
            problems.push(format!("{id}: applies to no platform"));
        }
        for (kind, paths) in [("code", &f.code), ("tests", &f.tests)] {
            for path in paths {
                if !root.join(path).exists() {
                    problems.push(format!("{id}: its {kind} names {path}, which isn't there"));
                }
            }
        }
        problems.extend(promise_problems(f, root));
    }
    problems
}

/// The ledger as a table for people: docs/PARITY.md.
pub fn render_table(ledger: &Ledger) -> String {
    let mark = |s: Support| match s {
        Support::Yes => "yes",
        Support::Partial => "partial",
        Support::No => "**no**",
        Support::NotApplicable => "n/a",
    };
    let mut out = String::from(
        "# Platform parity\n\n\
         Generated from `docs/parity/ledger.toml` by `TREK_PARITY_UPDATE=1 cargo test -p trek-parity`: edit the\n\
         ledger, not this file. Why it exists and how it's kept true: `docs/CROSS_PLATFORM.md`.\n\n\
         | Area | Feature | macOS | Windows | Notes |\n|---|---|---|---|---|\n",
    );
    for f in features_in_order(ledger) {
        let mut notes = vec![];
        if let Some(how) = &f.how {
            notes.push(format!("macOS: {} Windows: {}", how.macos, how.windows));
        }
        if let Some(note) = &f.note {
            notes.push(note.clone());
        }
        let cell = |s: &str| s.replace('|', "\\|").replace('\n', " ");
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            cell(&f.area),
            cell(&f.what),
            mark(f.macos),
            mark(f.windows),
            cell(&notes.join(" ")),
        ));
    }
    let promised: Vec<&Feature> = features_in_order(ledger)
        .into_iter()
        .filter(|f| !f.promise.is_empty())
        .collect();
    if promised.is_empty() {
        return out;
    }
    out.push_str(
        "\n## Promises\n\n\
         What a feature keeps to on each platform beyond being there, and the test that holds each \
         platform to it (one that runs on both is named on both).\n\n\
         | Feature | Promise | macOS | Windows |\n|---|---|---|---|\n",
    );
    let cell = |s: &str| s.replace('|', "\\|").replace('\n', " ");
    for f in promised {
        for p in &f.promise {
            let untested = p.untested.clone().unwrap_or_default();
            let side = |support: Support, test: &Option<String>, why: &Option<String>| {
                if !matches!(support, Support::Yes | Support::Partial) {
                    return "—".to_string();
                }
                match (test, why) {
                    (Some(t), _) => format!("`{}`", t.rsplit("::").next().unwrap_or(t)),
                    (None, Some(w)) => format!("untested: {}", cell(w)),
                    (None, None) => "untested".to_string(),
                }
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                cell(&f.what),
                cell(&p.what),
                side(f.macos, &p.macos, &untested.macos),
                side(f.windows, &p.windows, &untested.windows),
            ));
        }
    }
    out
}

/// The features as the table lists them: by area, then id.
fn features_in_order(ledger: &Ledger) -> Vec<&Feature> {
    let mut features: Vec<&Feature> = ledger.feature.iter().collect();
    features.sort_by(|a, b| a.area.cmp(&b.area).then(a.id.cmp(&b.id)));
    features
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gate_is_a_cfg_that_names_a_platform() {
        assert_eq!(platform_gates("#[cfg(windows)]\nfn a() {}"), 1);
        assert_eq!(
            platform_gates("if cfg!(target_os = \"macos\") { 1 } else { 2 }"),
            1
        );
        assert_eq!(
            platform_gates("#![cfg_attr(windows, windows_subsystem = \"windows\")]"),
            1
        );
        assert_eq!(platform_gates("#[cfg(not(unix))]"), 1);
        // Spread over lines by rustfmt: still one.
        assert_eq!(
            platform_gates(
                "#[cfg(all(\n    target_os = \"macos\",\n    not(test)\n))]\nmod imp {}"
            ),
            1
        );
        // Not about a platform.
        assert_eq!(
            platform_gates("#[cfg(test)]\n#[cfg(feature = \"shots\")]"),
            0
        );
        assert_eq!(
            platform_gates("#[cfg(any(test, feature = \"test-support\"))]"),
            0
        );
        // A word that only contains a platform's name isn't it.
        assert_eq!(platform_gates("#[cfg(feature = \"windowsish\")]"), 0);
        assert_eq!(platform_gates("let mycfg = cfg_value(windows);"), 0);
        // A comment about a gate isn't one.
        assert_eq!(
            platform_gates("// #[cfg(windows)] would go here\n/// `cfg!(windows)`"),
            0
        );
        // Several on a line count apart.
        assert_eq!(
            platform_gates("if cfg!(windows) || cfg!(target_os = \"macos\") {}"),
            2
        );
        // Trek's styling switch is one too, branch or pick.
        assert_eq!(
            platform_gates(
                "if fluent::WINDOWS && opened {}\nconst C: f32 = fluent::pick(0.8, 0.6);"
            ),
            2
        );
        assert_eq!(platform_gates("let x = fluent::WINDOWS_MAX;"), 0);
    }

    #[test]
    fn a_count_that_moved_says_what_to_do() {
        let map = |pairs: &[(&str, usize)]| {
            pairs
                .iter()
                .map(|(f, n)| (f.to_string(), *n))
                .collect::<BTreeMap<_, _>>()
        };
        let recorded = map(&[("a.rs", 2), ("b.rs", 1), ("gone.rs", 3)]);
        let found = map(&[("a.rs", 2), ("b.rs", 2), ("new.rs", 1)]);
        let changes = gate_changes(&recorded, &found);
        assert_eq!(changes.len(), 3, "{changes:?}");
        assert!(changes[0].starts_with("b.rs: 2 platform gates, 1 more than recorded (1)."));
        assert!(changes[1].starts_with("gone.rs: 0 platform gates, 3 fewer than recorded (3)."));
        assert!(changes[2].starts_with("new.rs: 1 platform gate, 1 more than recorded (0)."));
        assert!(gate_changes(&found, &found).is_empty());
        // What's written reads back as it was.
        let unclaimed: BTreeSet<String> = ["crates/a/src/lib.rs".to_string()].into();
        let back: Recorded = toml::from_str(&render_gates(&found, &unclaimed)).unwrap();
        assert_eq!(back.gates, found);
        assert_eq!(back.unclaimed, Some(unclaimed));
        let none: Recorded = toml::from_str(&render_gates(&found, &BTreeSet::new())).unwrap();
        assert_eq!(
            none.unclaimed,
            Some(BTreeSet::new()),
            "an empty list, not none"
        );
    }

    fn ledger_with(code: &[&str], tests: &[&str]) -> Ledger {
        toml::from_str(&format!(
            "[[feature]]\nid = \"f\"\narea = \"A\"\nwhat = \"W\"\nmacos = \"yes\"\nwindows = \"yes\"\ncode = {code:?}\ntests = {tests:?}\n"
        ))
        .unwrap()
    }

    #[test]
    fn a_gated_file_belongs_to_a_feature_and_the_old_unclaimed_ones_only_dwindle() {
        let found: BTreeMap<String, usize> = [
            ("crates/a/src/lib.rs".to_string(), 2),
            ("crates/b/src/lib.rs".to_string(), 1),
            ("crates/c/tests/t.rs".to_string(), 1),
        ]
        .into();
        // A feature's `code` or `tests` claims a file.
        let ledger = ledger_with(&["crates/a/src/lib.rs"], &["crates/c/tests/t.rs"]);
        let open = unclaimed(&found, &ledger);
        assert_eq!(open, ["crates/b/src/lib.rs".to_string()].into());

        // Unclaimed and not recorded so: a new difference no feature owns.
        let none = BTreeSet::new();
        let growth = unclaimed_growth(&none, &open, &found, &found);
        assert_eq!(growth.len(), 1);
        assert!(
            growth[0]
                .starts_with("crates/b/src/lib.rs: 1 platform gate(s), and no feature claims it.")
        );

        // Recorded as unclaimed (from before the rule): fine as it is...
        let recorded: BTreeSet<String> = ["crates/b/src/lib.rs".to_string()].into();
        assert!(unclaimed_growth(&recorded, &open, &found, &found).is_empty());
        assert_eq!(still_unclaimed(&recorded, &open), recorded);
        // ...but not with more gates than recorded.
        let was: BTreeMap<String, usize> = [("crates/b/src/lib.rs".to_string(), 0)].into();
        let grew = unclaimed_growth(&recorded, &open, &was, &found);
        assert!(
            grew[0].contains("1 more than recorded, and no feature claims it yet"),
            "{grew:?}"
        );

        // Claimed since: it comes off the list, and the list never takes a file back on.
        let claimed = ledger_with(
            &["crates/a/src/lib.rs", "crates/b/src/lib.rs"],
            &["crates/c/tests/t.rs"],
        );
        let now = unclaimed(&found, &claimed);
        assert!(now.is_empty());
        assert_eq!(claimed_since(&recorded, &now).len(), 1);
        assert!(still_unclaimed(&recorded, &now).is_empty());
    }

    #[test]
    fn the_ledger_is_held_to_its_rules() {
        let ledger: Ledger = toml::from_str(
            r#"
            [[feature]]
            id = "dictation"
            area = "Composer"
            what = "Dictate a message"
            macos = "yes"
            windows = "no"

            [[feature]]
            id = "dictation"
            area = "Composer"
            what = "Again"
            macos = "n/a"
            windows = "n/a"
            code = ["crates/nowhere.rs"]
            "#,
        )
        .unwrap();
        let problems = ledger_problems(&ledger, Path::new("."));
        assert!(
            problems.iter().any(|p| p.contains("without a note")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("listed twice")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|p| p.contains("applies to no platform")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("which isn't there")),
            "{problems:?}"
        );
        // Unknown fields are a typo, not a note.
        assert!(
            toml::from_str::<Ledger>(
                "[[feature]]\nid = \"x\"\narea = \"A\"\nwhat = \"W\"\nmacos = \"yes\"\nwindows = \"yes\"\nnotes = \"typo\"\n"
            )
            .is_err()
        );
    }

    #[test]
    fn a_test_is_seen_with_the_platform_its_cfg_keeps_it_to() {
        let source = "#[test]\nfn both() {}\n\n#[test]\n#[cfg(windows)]\nfn pc() {}\n\n\
             #[cfg(target_os = \"macos\")]\n/// Doc.\n#[test]\nasync fn mac() {}\n\n\
             #[cfg(not(windows))]\n#[test]\nfn not_pc() {}\n";
        assert_eq!(test_runs_on(source, "both"), Some(None));
        assert_eq!(test_runs_on(source, "pc"), Some(Some(Platform::Windows)));
        assert_eq!(test_runs_on(source, "mac"), Some(Some(Platform::Macos)));
        assert_eq!(test_runs_on(source, "not_pc"), Some(Some(Platform::Macos)));
        assert_eq!(test_runs_on(source, "bot"), None, "a prefix isn't the name");
        assert_eq!(test_runs_on(source, "nowhere"), None);
    }

    #[test]
    fn a_promise_is_held_by_a_test_on_each_platform_or_says_why_not() {
        let dir = std::env::temp_dir().join(format!("trek-parity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("t.rs"),
            "#[test]\nfn held() {}\n\n#[cfg(windows)]\n#[test]\nfn on_a_pc() {}\n",
        )
        .unwrap();
        let problems = |promise: &str, windows: &str| {
            let ledger: Ledger = toml::from_str(&format!(
                "[[feature]]\nid = \"f\"\narea = \"A\"\nwhat = \"W\"\nmacos = \"yes\"\nwindows = \"{windows}\"\nnote = \"n\"\n\n[[feature.promise]]\n{promise}\n"
            ))
            .unwrap();
            ledger_problems(&ledger, &dir)
        };
        // One test on both, or one each, or a reason for the side without.
        assert!(
            problems(
                "what = \"P\"\nmacos = \"t.rs::held\"\nwindows = \"t.rs::held\"",
                "yes"
            )
            .is_empty()
        );
        assert!(
            problems(
                "what = \"P\"\nmacos = \"t.rs::held\"\nwindows = \"t.rs::on_a_pc\"",
                "yes"
            )
            .is_empty()
        );
        assert!(problems("what = \"P\"\nwindows = \"t.rs::held\"\nuntested = { macos = \"Speech needs permission.\" }", "yes").is_empty());
        // Nothing for a side the feature is on.
        let p = problems("what = \"P\"\nwindows = \"t.rs::held\"", "yes");
        assert!(p.iter().any(|p| p.contains("no test on macOS")), "{p:?}");
        // A test that isn't there, or that its cfg keeps off that side.
        let p = problems(
            "what = \"P\"\nmacos = \"t.rs::gone\"\nwindows = \"t.rs::held\"",
            "yes",
        );
        assert!(p.iter().any(|p| p.contains("has no `fn gone`")), "{p:?}");
        let p = problems(
            "what = \"P\"\nmacos = \"t.rs::on_a_pc\"\nwindows = \"t.rs::held\"",
            "yes",
        );
        assert!(
            p.iter().any(|p| p.contains("runs on Windows only")),
            "{p:?}"
        );
        let p = problems(
            "what = \"P\"\nmacos = \"nowhere.rs::held\"\nwindows = \"held\"",
            "yes",
        );
        assert!(
            p.iter().any(|p| p.contains("nowhere.rs isn't there")),
            "{p:?}"
        );
        assert!(
            p.iter().any(|p| p.contains("isn't `path::test_name`")),
            "{p:?}"
        );
        // A side the feature isn't on is named for nothing; a side can't be both.
        let p = problems(
            "what = \"P\"\nmacos = \"t.rs::held\"\nwindows = \"t.rs::held\"",
            "no",
        );
        assert!(
            p.iter().any(|p| p.contains("where the feature isn't")),
            "{p:?}"
        );
        let p = problems(
            "what = \"P\"\nmacos = \"t.rs::held\"\nwindows = \"t.rs::held\"\nuntested = { macos = \"x\" }",
            "yes",
        );
        assert!(
            p.iter().any(|p| p.contains("says it's untested there")),
            "{p:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
