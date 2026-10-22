//! Format document: the file's text through the formatter its language has (`rustfmt`,
//! `prettier`, `black`, `gofmt`, `shfmt`), found on the PATH Trek composes for agents, so the
//! one a terminal would run is the one that runs. Its text goes in on stdin and the result comes
//! back on stdout; nothing is written to the file, and the buffer is replaced only when the
//! formatter exits cleanly and printed something (see `EditorView::format`).

use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a formatter gets before it's given up on.
pub const LIMIT: Duration = Duration::from_secs(30);

/// A formatter by the program's bare name and its arguments (stdin to stdout).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatter {
    pub program: &'static str,
    pub args: Vec<String>,
}

fn fmt(program: &'static str, args: &[&str]) -> Option<Formatter> {
    Some(Formatter {
        program,
        args: args.iter().map(|a| a.to_string()).collect(),
    })
}

/// The formatter for the file at `path`, by its extension; `None` for a language that has none.
/// The file's own path goes to the ones that read their config by it (`prettier`, `black`).
pub fn formatter_for(path: &Path) -> Option<Formatter> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    let file = path.to_string_lossy();
    match ext.as_str() {
        "rs" => fmt("rustfmt", &["--edition", "2021"]),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "json" | "jsonc" | "css"
        | "scss" | "html" | "htm" | "md" | "markdown" | "yaml" | "yml" => {
            fmt("prettier", &["--stdin-filepath", &file])
        }
        "py" | "pyi" => fmt("black", &["-q", "--stdin-filename", &file, "-"]),
        "go" => fmt("gofmt", &[]),
        "sh" | "bash" => fmt("shfmt", &[]),
        _ => None,
    }
}

/// What a missing formatter is called to the user: the file's extension (`.cs`), or its name.
pub fn kind_of(path: &Path) -> String {
    match path.extension() {
        Some(e) => format!(".{}", e.to_string_lossy()),
        None => path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}

/// The program to run for `formatter`, with its arguments: where the PATH Trek composes has it;
/// `None` when it isn't installed.
pub fn resolve(formatter: &Formatter) -> Option<(PathBuf, Vec<String>)> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(found) = tests::OVERRIDE.with(|o| o.borrow().clone()) {
        return found;
    }
    trek_core::detect::which(formatter.program).map(|program| (program, formatter.args.clone()))
}

/// The formatter `program` with `args`, in `cwd` (the file's folder: `rustfmt` reads its config
/// from there), on the PATH Trek composes. On Windows a `.cmd` shim (`prettier.cmd`) runs `node`
/// by its bare name, which cmd.exe looks for in the current folder before the PATH: told not to,
/// a `node.exe` in a folder someone downloaded isn't what Format document runs.
fn command(program: &Path, args: &[String], cwd: &Path) -> tokio::process::Command {
    let mut command = trek_core::procs::async_command(program);
    command
        .args(args)
        .current_dir(cwd)
        .env("PATH", trek_core::detect::login_path());
    #[cfg(windows)]
    command.env("NoDefaultCurrentDirectoryInExePath", "1");
    command
}

/// Run `program` with `args` in `cwd` (where the project's own config is found from), `input` on
/// its stdin: its stdout when it exits 0 and printed something, else a sentence for a toast.
pub async fn run(
    program: &Path,
    args: &[String],
    cwd: &Path,
    input: String,
    limit: Duration,
) -> Result<String, String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt as _;
    let name = program
        .file_stem()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| program.display().to_string());
    if let Some(problem) = trek_core::detect::batch_args_problem(program, args) {
        return Err(format!("Can't run {name}: {problem}"));
    }
    let mut command = command(program, args, cwd);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // On the limit the wait is dropped, and the formatter with it.
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Couldn't run {name}: {e}"))?;
    // On Windows `prettier` is `prettier.cmd`: cmd.exe running node. Dropping the wait ends
    // cmd.exe only; the job, dropped with this, ends node too. One that can't be had (Trek runs
    // in a job that forbids it) leaves the formatter to be ended alone.
    #[cfg(windows)]
    let _job = crate::job::hold(&child)
        .map_err(|e| {
            tracing::warn!("couldn't put {name} in a job; what it starts may outlive it: {e}")
        })
        .ok();
    // Written from a task of its own: a formatter that answers before it has read everything
    // would otherwise block on its full stdout while this blocks on its full stdin.
    if let Some(mut stdin) = child.stdin.take() {
        tokio::spawn(async move {
            let _ = stdin.write_all(input.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }
    let output = match tokio::time::timeout(limit, child.wait_with_output()).await {
        Err(_) => {
            return Err(format!(
                "{name} took too long, so the file was left as it is"
            ));
        }
        Ok(Err(e)) => return Err(format!("Couldn't run {name}: {e}")),
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let line = stderr.lines().map(str::trim).find(|l| !l.is_empty());
        return Err(match (line, output.status.code()) {
            (Some(line), _) => format!("{name} couldn't format it: {line}"),
            (None, Some(code)) => {
                format!("{name} couldn't format it (it stopped with code {code})")
            }
            (None, None) => format!("{name} couldn't format it"),
        });
    }
    let text = String::from_utf8(output.stdout).map_err(|_| {
        format!("{name} printed something that isn't text, so the file was left as it is")
    })?;
    if text.trim().is_empty() {
        return Err(format!(
            "{name} printed nothing, so the file was left as it is"
        ));
    }
    Ok(text)
}

/// `formatted` with the line endings of `before`, the text it was made from: a file with Windows
/// line endings (`\r\n`) keeps them, though `prettier`, `gofmt` and `shfmt` print `\n` whatever
/// they're given (and the whole file would otherwise change on its next save). Output that has
/// `\r\n` of its own is the formatter's choice, and kept.
pub fn keep_line_endings(before: &str, formatted: String) -> String {
    let crlf = before
        .find('\n')
        .is_some_and(|at| before[..at].ends_with('\r'));
    if crlf && !formatted.contains('\r') {
        formatted.replace('\n', "\r\n")
    } else {
        formatted
    }
}

/// Where the caret goes in `new`, a formatted `old` it was at byte `caret` of: after the same
/// number of characters that aren't white space, which a formatter moves around but keeps
/// (reflowed lines, new indents), or the end when `new` has fewer.
pub fn keep_caret(old: &str, caret: usize, new: &str) -> usize {
    let caret = caret.min(old.len());
    let before = old
        .get(..caret)
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_whitespace())
        .count();
    let mut seen = 0;
    for (at, c) in new.char_indices() {
        if seen == before {
            return at;
        }
        if !c.is_whitespace() {
            seen += 1;
        }
    }
    new.len()
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        /// What `resolve` finds in a test: `Some(found)` stands in for the PATH (and a
        /// formatter that is `found`, with the arguments it's given, for the real one).
        pub static OVERRIDE: RefCell<Option<Option<(PathBuf, Vec<String>)>>> = const { RefCell::new(None) };
    }

    pub fn of(file: &str) -> Option<(&'static str, Vec<String>)> {
        formatter_for(Path::new(file)).map(|f| (f.program, f.args))
    }

    #[test]
    fn each_language_has_the_formatter_its_community_uses() {
        assert_eq!(
            of("main.rs"),
            Some(("rustfmt", vec!["--edition".into(), "2021".into()]))
        );
        assert_eq!(of("go/main.go"), Some(("gofmt", vec![])));
        assert_eq!(of("run.sh"), Some(("shfmt", vec![])));
        assert_eq!(of("app.py").map(|f| f.0), Some("black"));
        for web in [
            "a.ts", "a.tsx", "a.js", "a.mjs", "a.json", "a.css", "a.html", "a.md", "a.yaml",
            "a.yml", "A.TS",
        ] {
            assert_eq!(of(web).map(|f| f.0), Some("prettier"), "{web}");
        }
        for none in ["Program.cs", "q.sql", "Makefile", "x.ps1", "README"] {
            assert_eq!(of(none), None, "{none}");
        }
    }

    #[test]
    fn prettier_and_black_are_told_the_files_own_path_to_find_their_config() {
        let path = Path::new("proj").join("src").join("a.ts");
        let f = formatter_for(&path).unwrap();
        assert_eq!(
            f.args,
            vec![
                "--stdin-filepath".to_string(),
                path.to_string_lossy().to_string()
            ]
        );
        let f = formatter_for(&Path::new("proj").join("a.py")).unwrap();
        assert_eq!(f.args.last().map(String::as_str), Some("-"));
        assert!(f.args.contains(&"--stdin-filename".to_string()));
    }

    #[test]
    fn a_missing_formatter_is_named_by_the_files_kind() {
        assert_eq!(kind_of(Path::new("a/Program.cs")), ".cs");
        assert_eq!(kind_of(Path::new("a/Makefile")), "Makefile");
    }

    #[test]
    fn the_caret_stays_after_the_same_code_when_the_layout_changes() {
        let old = "fn  main( ) {let x=1;}";
        let new = "fn main() {\n    let x = 1;\n}\n";
        // After `let` in both.
        let at = old.find("let").unwrap() + 3;
        let to = keep_caret(old, at, new);
        assert_eq!(&new[..to], "fn main() {\n    let");
        // At the start and past the end.
        assert_eq!(keep_caret(old, 0, new), 0);
        // After the last character of code, not past the formatter's closing newline.
        assert_eq!(keep_caret(old, old.len(), new), new.trim_end().len());
        assert_eq!(keep_caret("a b c d", 100, "a"), 1);
    }

    #[test]
    fn a_file_with_windows_line_endings_keeps_them() {
        // `prettier` and `gofmt` print `\n` only.
        assert_eq!(
            keep_line_endings("a\r\nb\r\n", "a;\nb;\n".into()),
            "a;\r\nb;\r\n"
        );
        // A file with `\n` stays so, and so does one with no line ending at all.
        assert_eq!(keep_line_endings("a\nb\n", "a;\nb;\n".into()), "a;\nb;\n");
        assert_eq!(keep_line_endings("a", "a;\n".into()), "a;\n");
        // `rustfmt` and `black` keep `\r\n` themselves: not doubled.
        assert_eq!(
            keep_line_endings("a\r\nb\r\n", "a;\r\nb;\r\n".into()),
            "a;\r\nb;\r\n"
        );
    }

    #[test]
    fn the_caret_lands_on_a_character_boundary() {
        let old = "é = 1";
        let new = "é  =  1\n";
        let to = keep_caret(old, "é =".len(), new);
        assert!(new.is_char_boundary(to));
        assert_eq!(&new[..to], "é  =");
    }

    pub fn fixture(args: &[&str]) -> (PathBuf, Vec<String>) {
        (
            trek_test_fixtures::bin("fixture"),
            args.iter().map(|a| a.to_string()).collect(),
        )
    }

    pub fn run_it(args: &[&str], input: &str, limit: Duration) -> Result<String, String> {
        let (program, args) = fixture(args);
        trek_core::runtime().block_on(run(
            &program,
            &args,
            &std::env::temp_dir(),
            input.to_string(),
            limit,
        ))
    }

    #[test]
    fn the_text_goes_in_on_stdin_and_the_formatted_text_comes_out_on_stdout() {
        assert_eq!(
            run_it(&["upper"], "let x = 1;\n", Duration::from_secs(60)).as_deref(),
            Ok("LET X = 1;\n")
        );
        // A formatter that answers before reading everything, with more than a pipe holds.
        let big = "a\n".repeat(400_000);
        assert_eq!(
            run_it(&["upper"], &big, Duration::from_secs(60)).map(|t| t.len()),
            Ok(big.len())
        );
    }

    #[test]
    fn a_formatter_that_fails_says_why_in_its_first_line() {
        let err = run_it(
            &["fail", "2", "error: expected `;`", "second line"],
            "x",
            Duration::from_secs(60),
        )
        .unwrap_err();
        assert_eq!(err, "fixture couldn't format it: error: expected `;`");
        let err = run_it(&["exit", "3"], "x", Duration::from_secs(60)).unwrap_err();
        assert_eq!(err, "fixture couldn't format it (it stopped with code 3)");
    }

    #[test]
    fn a_formatter_that_prints_nothing_leaves_the_file_alone() {
        let err = run_it(&["print"], "x", Duration::from_secs(60)).unwrap_err();
        assert_eq!(
            err,
            "fixture printed nothing, so the file was left as it is"
        );
    }

    #[test]
    fn a_formatter_that_hangs_is_given_up_on() {
        // The fixture first: the first test to ask for it builds it, which a busy runner can
        // take longer than the limit to do.
        let (program, args) = fixture(&["sleep", "30"]);
        let started = std::time::Instant::now();
        let err = trek_core::runtime()
            .block_on(run(
                &program,
                &args,
                &std::env::temp_dir(),
                "x".to_string(),
                Duration::from_millis(500),
            ))
            .unwrap_err();
        assert_eq!(err, "fixture took too long, so the file was left as it is");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the wait ended with the limit"
        );
    }

    /// `prettier` on Windows is `prettier.cmd`, cmd.exe running node: given up on, node goes too,
    /// not only the cmd.exe in front of it.
    #[cfg(windows)]
    #[test]
    fn a_formatter_behind_a_cmd_shim_that_hangs_is_ended_with_what_it_started() {
        let dir = std::env::temp_dir().join(format!("trek-format-shim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("held.txt");
        let shim = dir.join("slowfmt.cmd");
        // The program the shim starts has `held.txt` open (its stdout) for as long as it runs.
        std::fs::write(
            &shim,
            format!(
                "@\"{}\" sleep 30 > \"{}\"\r\n",
                trek_test_fixtures::bin("fixture").display(),
                marker.display()
            ),
        )
        .unwrap();
        let err = trek_core::runtime()
            .block_on(run(
                &shim,
                &[],
                &dir,
                "x".into(),
                Duration::from_millis(1500),
            ))
            .unwrap_err();
        assert_eq!(err, "slowfmt took too long, so the file was left as it is");
        assert!(
            marker.exists(),
            "the shim got as far as starting its program"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while let Err(e) = std::fs::remove_file(&marker) {
            assert!(
                std::time::Instant::now() < deadline,
                "what the shim started is still running (it holds its output open: {e})"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `.cmd` shim (`prettier.cmd`) runs `node` by its bare name, which cmd.exe looks for in the
    /// current folder, the file's, before the PATH: a `node.cmd` in a folder someone downloaded
    /// must not be what runs.
    #[cfg(windows)]
    #[test]
    fn a_formatter_shim_never_runs_a_program_from_the_files_folder() {
        let root = std::env::temp_dir().join(format!("trek-format-cwd-{}", std::process::id()));
        let (bin, project) = (root.join("bin"), root.join("project"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let ran = root.join("ran.txt");
        let shim = bin.join("fmtshim.cmd");
        std::fs::write(&shim, "@planted\r\n").unwrap();
        std::fs::write(
            project.join("planted.cmd"),
            format!("@echo ran> \"{}\"\r\n", ran.display()),
        )
        .unwrap();
        let result = trek_core::runtime().block_on(run(
            &shim,
            &[],
            &project,
            "x".into(),
            Duration::from_secs(30),
        ));
        assert!(
            !ran.exists(),
            "the program in the file's folder ran ({result:?})"
        );
        assert!(
            result.is_err(),
            "nothing named `planted` on the PATH: {result:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
        // Whatever Trek's own environment says (a terminal may have set it already, which is
        // all the above can see there): the formatter is told.
        let command = command(&shim, &[], &project);
        let told = command.as_std().get_envs().any(|(k, v)| {
            k == "NoDefaultCurrentDirectoryInExePath" && v == Some(std::ffi::OsStr::new("1"))
        });
        assert!(
            told,
            "the formatter's cmd.exe is told not to look in the current folder"
        );
    }

    #[test]
    fn a_program_that_isnt_there_says_so() {
        let err = trek_core::runtime()
            .block_on(run(
                Path::new("no-such-formatter-here"),
                &[],
                &std::env::temp_dir(),
                "x".into(),
                Duration::from_secs(5),
            ))
            .unwrap_err();
        assert!(
            err.starts_with("Couldn't run no-such-formatter-here"),
            "{err}"
        );
    }
}
