//! `grep`, `glob` and `list_dir`: finding things without guessing at file names or shell flags.
//! They read only, so a read-only session has them too. On this machine they run in-process, which
//! works the same on every OS and doesn't need `rg` installed; in a remote sandbox the same
//! questions go through its shell, since all an executor offers is a command and bytes.

use regex::RegexBuilder;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

/// Most matches `grep` reports, and most paths `glob` does.
pub const MAX_RESULTS: usize = 200;
/// Most entries `list_dir` reports.
pub const MAX_ENTRIES: usize = 500;
/// A longer line is cut: minified files would otherwise fill the context with one match.
const MAX_LINE: usize = 300;
/// A larger file isn't searched.
const MAX_FILE: u64 = 8 * 1024 * 1024;
/// How long one search may walk the tree.
const MAX_WALK: Duration = Duration::from_secs(30);

fn walker(root: &Path) -> ignore::WalkBuilder {
    let mut walk = ignore::WalkBuilder::new(root);
    // Hidden folders hold real work (`.github`); `.gitignore` already leaves out build output.
    walk.hidden(false)
        .filter_entry(|e| e.file_name() != ".git")
        // A project that isn't a repository yet still means its `.gitignore`.
        .require_git(false)
        .follow_links(false);
    walk
}

/// The path to show for `path`: relative to `root`, as the model will pass it on.
fn shown(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn clip_line(line: &str) -> String {
    if line.len() <= MAX_LINE {
        return line.to_string();
    }
    let mut end = MAX_LINE;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} …", &line[..end])
}

/// Lines matching `pattern` under `root` (a folder or one file), as `path:line:text`. `Err` is what
/// to tell the model about a pattern that doesn't compile.
pub fn grep(
    root: &Path,
    pattern: &str,
    glob: Option<&str>,
    case_insensitive: bool,
) -> Result<String, String> {
    let re = RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .size_limit(10 * 1024 * 1024)
        .build()
        .map_err(|e| {
            format!(
                "Error: pattern isn't a valid regular expression: {e}\nEscape special characters to search for them as text."
            )
        })?;
    let filter = glob.map(Filter::new).transpose()?;
    let walk = walker(root);
    let started = Instant::now();
    let mut out = String::new();
    let (mut matches, mut more, mut cut_short) = (0usize, false, false);
    'walk: for entry in walk.build().flatten() {
        if started.elapsed() > MAX_WALK {
            cut_short = true;
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Some(filter) = &filter {
            // A file asked for by name is searched whatever the filter says.
            let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
            if !relative.as_os_str().is_empty() && !filter.wants(relative) {
                continue;
            }
        }
        if entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        // Binary: no line of it is worth showing.
        if bytes[..bytes.len().min(8192)].contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let path = shown(entry.path(), root);
        for (i, line) in text.lines().enumerate() {
            if !re.is_match(line) {
                continue;
            }
            if matches == MAX_RESULTS {
                more = true;
                break 'walk;
            }
            matches += 1;
            out.push_str(&format!("{path}:{}:{}\n", i + 1, clip_line(line)));
        }
    }
    if matches == 0 {
        return Ok(if cut_short {
            "No matches before the search ran out of time: search a smaller path.".to_string()
        } else {
            "No matches.".to_string()
        });
    }
    if more {
        out.push_str(&format!(
            "(Stopped at the first {MAX_RESULTS} matches: narrow it with path, glob or a more exact pattern.)\n"
        ));
    } else if cut_short {
        out.push_str(
            "(The search ran out of time, so there may be more: search a smaller path.)\n",
        );
    }
    Ok(out)
}

/// Which files a `glob` argument means. A pattern with no folder in it goes by file name, at any
/// depth; `!` excludes. Filtering here rather than with the walker's overrides, which would beat
/// `.gitignore`: `*` would walk `target/` and answer with its build output.
struct Filter {
    by_name: globset::GlobSet,
    by_path: globset::GlobSet,
    not_by_name: globset::GlobSet,
    not_by_path: globset::GlobSet,
    any_wanted: bool,
}

impl Filter {
    fn new(patterns: &str) -> Result<Filter, String> {
        let mut sets = [
            globset::GlobSetBuilder::new(),
            globset::GlobSetBuilder::new(),
            globset::GlobSetBuilder::new(),
            globset::GlobSetBuilder::new(),
        ];
        let mut any_wanted = false;
        for one in split_outside_braces(patterns) {
            let one = one.trim();
            let (negated, body) = match one.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, one),
            };
            let body = body.strip_prefix("./").unwrap_or(body);
            if body.is_empty() {
                continue;
            }
            any_wanted |= !negated;
            let by_path = body.contains('/');
            let glob = globset::GlobBuilder::new(body.trim_start_matches('/'))
                .literal_separator(true)
                .build()
                .map_err(|e| format!("Error: invalid glob {one}: {e}"))?;
            sets[usize::from(by_path) + 2 * usize::from(negated)].add(glob);
        }
        let build = |set: globset::GlobSetBuilder| {
            set.build().map_err(|e| format!("Error: invalid glob: {e}"))
        };
        let [by_name, by_path, not_by_name, not_by_path] = sets;
        Ok(Filter {
            by_name: build(by_name)?,
            by_path: build(by_path)?,
            not_by_name: build(not_by_name)?,
            not_by_path: build(not_by_path)?,
            any_wanted,
        })
    }

    fn wants(&self, relative: &Path) -> bool {
        let name = relative.file_name().unwrap_or(relative.as_os_str());
        if self.not_by_name.is_match(name) || self.not_by_path.is_match(relative) {
            return false;
        }
        !self.any_wanted || self.by_name.is_match(name) || self.by_path.is_match(relative)
    }
}

/// `text` split at commas, except those inside `{…}`: `*.{ts,tsx},*.rs` is two patterns.
fn split_outside_braces(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Files under `root` matching `pattern`, most recently changed first.
pub fn glob(root: &Path, pattern: &str) -> Result<String, String> {
    // An absolute pattern under the root means the same relative one.
    let root_text = format!("{}/", root.display());
    let relative_pattern = pattern.strip_prefix(root_text.as_str()).unwrap_or(pattern);
    let filter = Filter::new(relative_pattern)?;
    let walk = walker(root);
    let started = Instant::now();
    let mut found: Vec<(SystemTime, String)> = Vec::new();
    let mut cut_short = false;
    for entry in walk.build().flatten() {
        if started.elapsed() > MAX_WALK {
            cut_short = true;
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
        if !filter.wants(relative) {
            continue;
        }
        let changed = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        found.push((changed, shown(entry.path(), root)));
    }
    if found.is_empty() {
        return Ok(format!("No files match {pattern}."));
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let total = found.len();
    let mut out: String = found
        .iter()
        .take(MAX_RESULTS)
        .map(|(_, p)| format!("{p}\n"))
        .collect();
    if total > MAX_RESULTS {
        out.push_str(&format!(
            "(Showing {MAX_RESULTS} of {total} files, most recently changed first: narrow the pattern.)\n"
        ));
    } else if cut_short {
        out.push_str("(The search ran out of time, so there may be more.)\n");
    }
    Ok(out)
}

/// What's in the folder `path`: folders first, each with a `/`.
pub fn list(path: &Path) -> Result<String, String> {
    let entries =
        std::fs::read_dir(path).map_err(|e| format!("Error listing {}: {e}", path.display()))?;
    let mut names: Vec<(bool, String)> = entries
        .flatten()
        .map(|e| {
            let dir = e.file_type().is_ok_and(|t| t.is_dir());
            (dir, e.file_name().to_string_lossy().into_owned())
        })
        .collect();
    names.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    if names.is_empty() {
        return Ok("(Empty folder)".to_string());
    }
    let total = names.len();
    let mut out: String = names
        .iter()
        .take(MAX_ENTRIES)
        .map(|(dir, name)| format!("{name}{}\n", if *dir { "/" } else { "" }))
        .collect();
    if total > MAX_ENTRIES {
        out.push_str(&format!(
            "(Showing {MAX_ENTRIES} of {total} entries: use glob for what you're after.)\n"
        ));
    }
    Ok(out)
}

/// `text` as one shell word.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// `grep` as a command for a sandbox's shell: `rg` when it has it, else `grep`.
pub fn grep_command(
    root: &str,
    pattern: &str,
    glob: Option<&str>,
    case_insensitive: bool,
) -> String {
    let (root, pattern) = (quote(root), quote(pattern));
    let rg_i = if case_insensitive { " -i" } else { "" };
    let grep_i = if case_insensitive { "i" } else { "" };
    let rg_glob = glob.map_or(String::new(), |g| format!(" --glob {}", quote(g)));
    let grep_glob = glob.map_or(String::new(), |g| format!(" --include={}", quote(g)));
    let limit = MAX_RESULTS + 1;
    format!(
        "{{ if command -v rg >/dev/null 2>&1; then \
         rg -n --no-heading --color never --hidden --glob '!.git'{rg_i}{rg_glob} -e {pattern} {root}; \
         else grep -rnIE{grep_i} --exclude-dir=.git{grep_glob} -e {pattern} {root}; fi; }} | head -n {limit}"
    )
}

/// `glob` as a command for a sandbox's shell.
pub fn glob_command(root: &str, pattern: &str) -> String {
    let root_word = quote(root);
    let limit = MAX_RESULTS + 1;
    let find = if pattern.contains('/') {
        let path = pattern.replace("**/", "*").replace("/**", "/*");
        format!(
            "find {root_word} -path '*/.git' -prune -o -type f -path {} -print",
            quote(&format!("*{}", path.trim_start_matches('*')))
        )
    } else {
        format!(
            "find {root_word} -path '*/.git' -prune -o -type f -name {} -print",
            quote(pattern)
        )
    };
    format!(
        "{{ if command -v rg >/dev/null 2>&1; then \
         rg --files --hidden --glob '!.git' --glob {} {root_word}; else {find}; fi; }} | head -n {limit}",
        quote(pattern)
    )
}

/// `list_dir` as a command for a sandbox's shell.
pub fn list_command(path: &str) -> String {
    format!("ls -1Ap {} | head -n {}", quote(path), MAX_ENTRIES + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile_like::Dir {
        let dir = tempfile_like::Dir::new();
        dir.file("a.rs", "fn alpha() {}\nfn beta() {}\n");
        dir.file("sub/b.rs", "// beta here\n");
        dir.file("sub/c.txt", "BETA in caps\n");
        dir.file(".github/ci.yml", "beta: true\n");
        dir.file(".git/config", "beta\n");
        dir.file("bin.dat", "beta\0beta\n");
        dir
    }

    // A small stand-in: this crate's tests have no tempfile dependency.
    mod tempfile_like {
        use std::path::{Path, PathBuf};
        pub struct Dir(PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "trek-search-test-{}-{}",
                    std::process::id(),
                    super::super::super::next_id()
                ));
                std::fs::create_dir_all(&path).unwrap();
                Dir(path)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
            pub fn file(&self, name: &str, text: &str) {
                let path = self.0.join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn grep_finds_lines_with_paths_and_numbers() {
        let dir = tree();
        let out = grep(dir.path(), "beta", None, false).unwrap();
        assert!(out.contains("a.rs:2:fn beta() {}"), "{out}");
        assert!(out.contains("sub/b.rs:1:// beta here"), "{out}");
        assert!(out.contains(".github/ci.yml:1:beta: true"), "{out}");
        assert!(!out.contains(".git/config"), "{out}");
        assert!(!out.contains("bin.dat"), "{out}");
        assert!(!out.contains("BETA"), "{out}");
    }

    #[test]
    fn grep_honours_case_and_globs() {
        let dir = tree();
        let out = grep(dir.path(), "beta", Some("*.txt"), true).unwrap();
        assert_eq!(out, "sub/c.txt:1:BETA in caps\n");
    }

    #[test]
    fn grep_says_when_nothing_matches_and_when_the_pattern_is_wrong() {
        let dir = tree();
        assert_eq!(
            grep(dir.path(), "zzzz", None, false).unwrap(),
            "No matches."
        );
        let bad = grep(dir.path(), "(", None, false).unwrap_err();
        assert!(bad.contains("valid regular expression"), "{bad}");
    }

    #[test]
    fn grep_stops_at_its_limit_and_says_so() {
        let dir = tempfile_like::Dir::new();
        dir.file("many.txt", &"hit\n".repeat(MAX_RESULTS + 50));
        let out = grep(dir.path(), "hit", None, false).unwrap();
        assert_eq!(out.lines().count(), MAX_RESULTS + 1);
        assert!(out.contains("Stopped at the first"), "{out}");
    }

    #[test]
    fn grep_cuts_long_lines() {
        let dir = tempfile_like::Dir::new();
        dir.file("min.js", &format!("needle{}\n", "x".repeat(5000)));
        let out = grep(dir.path(), "needle", None, false).unwrap();
        assert!(out.len() < MAX_LINE + 40, "{}", out.len());
    }

    #[test]
    fn grep_searches_one_file_given_as_the_root() {
        let dir = tree();
        let out = grep(&dir.path().join("a.rs"), "alpha", None, false).unwrap();
        assert!(out.ends_with(":1:fn alpha() {}\n"), "{out}");
    }

    #[test]
    fn glob_finds_files_by_pattern() {
        let dir = tree();
        let mut lines: Vec<String> = glob(dir.path(), "*.rs")
            .unwrap()
            .lines()
            .map(String::from)
            .collect();
        lines.sort();
        assert_eq!(lines, ["a.rs", "sub/b.rs"]);
        assert_eq!(glob(dir.path(), "sub/*.txt").unwrap(), "sub/c.txt\n");
        assert_eq!(
            glob(dir.path(), "*.nope").unwrap(),
            "No files match *.nope."
        );
    }

    #[test]
    fn what_git_ignores_stays_out_whatever_the_pattern() {
        let dir = tree();
        dir.file(".gitignore", "target/\n");
        dir.file("target/out.rs", "beta\n");
        for pattern in ["*", "**/*", "*.rs", "target/*"] {
            let out = glob(dir.path(), pattern).unwrap();
            assert!(!out.contains("target/out.rs"), "{pattern}: {out}");
        }
        let out = grep(dir.path(), "beta", Some("*.rs"), false).unwrap();
        assert!(!out.contains("target/"), "{out}");
    }

    #[test]
    fn braces_and_several_patterns_work() {
        let dir = tree();
        dir.file("x.ts", "1");
        dir.file("y.tsx", "2");
        let mut lines: Vec<String> = glob(dir.path(), "*.{ts,tsx},*.txt")
            .unwrap()
            .lines()
            .map(String::from)
            .collect();
        lines.sort();
        assert_eq!(lines, ["sub/c.txt", "x.ts", "y.tsx"]);
        assert_eq!(split_outside_braces("a,{b,c},d"), ["a", "{b,c}", "d"]);
        let out = glob(dir.path(), "*.rs,!b.rs").unwrap();
        assert_eq!(out, "a.rs\n");
        assert!(glob(dir.path(), "a[").is_err());
    }

    #[test]
    fn a_full_path_under_the_root_means_the_relative_one() {
        let dir = tree();
        let pattern = format!("{}/sub/*.txt", dir.path().display());
        assert_eq!(glob(dir.path(), &pattern).unwrap(), "sub/c.txt\n");
    }

    #[test]
    fn list_shows_folders_first() {
        let dir = tree();
        let out = list(dir.path()).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[..3], [".git/", ".github/", "sub/"]);
        assert!(lines.contains(&"a.rs"));
        assert!(list(&dir.path().join("nope")).is_err());
    }

    #[test]
    fn words_are_quoted_for_the_shell() {
        assert_eq!(quote("it's"), r"'it'\''s'");
        let command = grep_command("/a b", "x'y", Some("*.rs"), true);
        assert!(command.contains(r"-e 'x'\''y' '/a b'"), "{command}");
        assert!(command.contains("--include='*.rs'"), "{command}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_shell_forms_answer_the_same_questions() {
        use super::super::executor::{LocalExecutor, ToolExecutor as _};
        let dir = tree();
        let root = dir.path().to_string_lossy().into_owned();
        let run = |command: String| {
            let dir = dir.path().to_path_buf();
            async move {
                LocalExecutor
                    .exec(&command, &dir, Some(Duration::from_secs(20)))
                    .await
                    .unwrap()
                    .stdout
            }
        };
        let found = run(grep_command(&root, "alpha", None, false)).await;
        assert!(found.contains("a.rs:1:fn alpha() {}"), "{found}");
        let found = run(glob_command(&root, "*.txt")).await;
        assert!(found.trim_end().ends_with("sub/c.txt"), "{found}");
        let found = run(list_command(&root)).await;
        assert!(found.contains("sub/") && found.contains("a.rs"), "{found}");
    }
}
