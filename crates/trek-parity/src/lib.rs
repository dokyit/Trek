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
    let mut features: Vec<&Feature> = ledger.feature.iter().collect();
    features.sort_by(|a, b| a.area.cmp(&b.area).then(a.id.cmp(&b.id)));
    for f in features {
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
    out
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
}
