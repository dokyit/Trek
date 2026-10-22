//! Every process Trek starts goes through `trek_core::procs::command` or `async_command`, so on
//! Windows none opens a console window (see `quiet_spawns.rs`). This keeps the next spawn from
//! going around them: it reads the workspace's sources, on every platform, and fails on a
//! `Command::new(` outside the helper that isn't test code or marked.
//!
//! What a new `Command::new(` may do instead:
//! - use `trek_core::procs::command` (or `async_command`): the usual answer;
//! - start a program by an absolute Unix path in a literal (`"/usr/bin/open"`): it can't exist on
//!   Windows, and on macOS the helper is `Command::new`;
//! - say why not, on the same line: `// spawn: <reason>` (the updater's detached helper; a
//!   crate that has no trek-core to call).

use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the workspace root")
        .to_path_buf()
}

/// The sources that ship: each crate's `src` and `examples`, except the test fixtures' crate and
/// files that are tests (a `tests` folder, `tests*.rs`, `harness.rs`).
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") && !name.starts_with("tests") && name != "harness.rs" {
            out.push(path);
        }
    }
}

/// Whether a column-0 attribute line gates the item after it to tests: `#[cfg(test)]`,
/// `#[cfg(all(test, windows))]`, not `#[cfg(not(test))]`.
fn test_gate(line: &str) -> bool {
    line.starts_with("#[cfg(") && line.contains("test") && !line.contains("not(test)")
}

/// The line numbers (1-based) of `Command::new(` in `text` that are neither in test items, nor
/// comments, nor allowed.
fn offenders(text: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if test_gate(line) {
            // Skip the item it gates: to its closing brace in column 0, or its `;`.
            i += 1;
            while i < lines.len() && lines[i].starts_with("#[") {
                i += 1;
            }
            let opens = i < lines.len() && lines[i].trim_end().ends_with('{');
            let one_line = i < lines.len() && !opens && lines[i].trim_end().ends_with([';', '}']);
            if opens {
                while i < lines.len() && lines[i] != "}" {
                    i += 1;
                }
            } else if !one_line {
                // A signature over several lines: up to the end of its body or its `;`.
                while i < lines.len() && lines[i] != "}" && !lines[i].trim_end().ends_with(';') {
                    i += 1;
                }
            }
            i += 1;
            continue;
        }
        let code = line.trim_start();
        if !code.starts_with("//") && !line.contains("// spawn:") {
            let mut rest = line;
            while let Some(at) = rest.find("Command::new(") {
                let after = &rest[at + "Command::new(".len()..];
                // The part before is a path (`std::process::`), not another name (`UpdateCommand::`).
                let named = rest[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
                if !named && !after.starts_with("\"/") {
                    found.push((i + 1, line.trim().to_string()));
                }
                rest = after;
            }
        }
        i += 1;
    }
    found
}

#[test]
fn every_process_is_started_through_the_helper() {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(workspace().join("crates"))
        .unwrap()
        .flatten()
    {
        if entry.file_name() == "test-fixtures" {
            continue;
        }
        for sub in ["src", "examples"] {
            sources(&entry.path().join(sub), &mut files);
        }
    }
    assert!(
        files.len() > 50,
        "the scan found only {} sources: has the layout changed?",
        files.len()
    );
    let mut wrong = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for (line, code) in offenders(&text) {
            wrong.push(format!(
                "{}:{line}: {code}",
                file.strip_prefix(workspace()).unwrap().display()
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "these start a process without trek_core::procs::command / async_command, so on Windows each opens a console window (see the top of this file for what to do instead):\n{}",
        wrong.join("\n")
    );
}

#[test]
fn the_scan_sees_what_it_should() {
    let sample = r#"
fn a() {
    let _ = std::process::Command::new("git");
    let _ = tokio::process::Command::new(bin);
    let _ = Command::new("/usr/bin/open");
    let _ = Command::new(helper); // spawn: detached on purpose
    let _ = UpdateCommand::new("npm", &[]);
    // Command::new(x) in a comment
}
#[cfg(test)]
mod tests {
    fn t() {
        let _ = Command::new("git");
    }
}
#[cfg(not(test))]
fn b() {
    let _ = Command::new(c);
}
#[cfg(test)]
fn helper() -> Command {
    Command::new("git")
}
#[cfg(all(test, windows))]
use std::process::Command;
fn c() {
    let _ = Command::new("x");
}
"#;
    let found: Vec<usize> = offenders(sample).into_iter().map(|(n, _)| n).collect();
    assert_eq!(found, vec![3, 4, 18, 27]);
}
