//! What an agent CLI installed as a `.cmd` shim (npm's `claude.cmd`, `codex.cmd`) receives when
//! Trek passes it arguments. `std::process::Command` starts a `.cmd` or `.bat` as `cmd.exe /c`,
//! quoting each argument by cmd.exe's rules and refusing the ones it can't pass safely (since
//! the BatBadBut fix, CVE-2024-24576); `tokio::process::Command` is std's underneath. These
//! tests pin down exactly what arrives, through both shim styles npm writes.
//!
//! The stand-in CLI is this test binary run again with `ECHO` set: it prints its argv as JSON
//! and exits before libtest reads it (see `echo_argv`). The shims call it as npm's call node.

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub(crate) const ECHO: &str = "TREK_TEST_ARGV_ECHO";

/// The stand-in CLI: with `ECHO` set, print argv as a JSON array and exit.
#[ctor::ctor(unsafe)]
fn echo_argv() {
    if std::env::var_os(ECHO).is_none() {
        return;
    }
    let args: Vec<String> = std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", serde_json::to_string(&args).unwrap());
    let _ = out.flush();
    std::process::exit(0);
}

/// npm's shim as `cmd-shim` writes it today (`codex.cmd` from npm 10), with the stand-in as
/// `node.exe` beside it. Each shim sets `ECHO` itself (as the first thing, out of the way of what
/// npm's does), so a caller that sets no environment, an agent's own spawn, still gets the echo.
const NPM_SHIM: &str = r#"@ECHO off
SET "TREK_TEST_ARGV_ECHO=1"
GOTO start
:find_dp0
SET dp0=%~dp0
EXIT /b
:start
SETLOCAL
CALL :find_dp0

IF EXIST "%dp0%\node.exe" (
  SET "_prog=%dp0%\node.exe"
) ELSE (
  SET "_prog=node"
  SET PATHEXT=%PATHEXT:;.JS;=;%
)

endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & "%_prog%"  "%dp0%\..\pkg\bin\x.js" %*
"#;

/// The short form older npm versions (and hand-written wrappers) use.
const SIMPLE_SHIM: &str = "@SET \"TREK_TEST_ARGV_ECHO=1\"\r\n@\"%~dp0\\node.exe\" \"%~dp0\\..\\pkg\\bin\\x.js\" %*\r\n";

/// A folder (with a space in its name, as under `C:\Users\First Last`) holding the stand-in as
/// `node.exe` and the two shims; removed when dropped.
pub(crate) struct Shims(pub(crate) PathBuf);

impl Shims {
    pub(crate) fn new(label: &str) -> Self {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("trek cmd args {label} {}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let node = dir.join("node.exe");
        let exe = std::env::current_exe().unwrap();
        if std::fs::hard_link(&exe, &node).is_err() {
            std::fs::copy(&exe, &node).unwrap();
        }
        // CRLF, as cmd.exe expects of a batch file.
        std::fs::write(dir.join("npm.cmd"), NPM_SHIM.replace("\r\n", "\n").replace('\n', "\r\n")).unwrap();
        std::fs::write(dir.join("simple.cmd"), SIMPLE_SHIM).unwrap();
        Self(dir)
    }

    pub(crate) fn node(&self) -> PathBuf {
        self.0.join("node.exe")
    }
    pub(crate) fn npm(&self) -> PathBuf {
        self.0.join("npm.cmd")
    }
    pub(crate) fn simple(&self) -> PathBuf {
        self.0.join("simple.cmd")
    }
}

impl Drop for Shims {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What came of passing some arguments.
#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    /// The program started and saw these arguments (after the script a shim passes first).
    Arrived(Vec<String>),
    /// `Command` refused to start it: an `InvalidInput` error.
    Refused,
    /// It started, but cmd.exe gave up before the program ran; what it said.
    CmdFailed(String),
}

fn outcome(program: &Path, result: std::io::Result<std::process::Output>) -> Outcome {
    let out = match result {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => return Outcome::Refused,
        Err(e) => panic!("{} didn't start: {e:?}", program.display()),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let Some(line) = stdout.lines().find(|l| l.starts_with('[')) else {
        return Outcome::CmdFailed(String::from_utf8_lossy(&out.stderr).trim().to_string());
    };
    let argv: Vec<String> = serde_json::from_str(line).unwrap();
    // argv[0] is the stand-in; a shim passes its script next.
    let skip = if program.extension() == Some(OsStr::new("exe")) {
        1
    } else {
        assert!(argv[1].ends_with(r"\pkg\bin\x.js"), "{argv:?}");
        2
    };
    Outcome::Arrived(argv[skip..].to_vec())
}

fn run_std(program: &Path, args: &[&str]) -> Outcome {
    let result = std::process::Command::new(program).args(args).env(ECHO, "1").stdin(Stdio::null()).output();
    outcome(program, result)
}

async fn run_tokio(program: &Path, args: &[&str]) -> Outcome {
    let result = tokio::process::Command::new(program).args(args).env(ECHO, "1").stdin(Stdio::null()).output().await;
    outcome(program, result)
}

/// What should happen to an argument passed to a shim.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Expect {
    /// It arrives exactly as passed.
    Same,
    /// `Command` refuses it.
    Refused,
    /// cmd.exe's own limit: a command line over 8191 characters.
    TooLong,
}

/// The table: a name, the argument, and what a shim does with it.
fn cases() -> Vec<(&'static str, String, Expect)> {
    use Expect::*;
    let s = |name, arg: &str, expect| (name, arg.to_string(), expect);
    vec![
        s("plain word", "app-server", Same),
        s("flag=value", "--model=claude-opus-4-5[1m]", Same),
        s("spaces", "a b  c", Same),
        s("path with spaces", r"C:\Users\First Last\My Project", Same),
        s("trailing backslash", r"C:\Users\First Last\", Same),
        s("UNC path", r"\\server\share\dir", Same),
        s("double quote", r#"say "hi""#, Same),
        s("lone double quote", "\"", Same),
        s("backslash before quote", r#"a\"b"#, Same),
        s("backslashes then quote at end", r#"a\\""#, Same),
        s("single quotes", "it's 'quoted'", Same),
        s("%PATH%", "%PATH%", Same),
        s("%~dp0 and %1", "%~dp0 %1 %*", Same),
        s("lone %", "100% done", Same),
        s("%% and %cd%", "%%cd%%", Same),
        s("quoted %VAR%", r#""%USERNAME%""#, Same),
        s("caret", "a^b^^c^", Same),
        s("ampersand", "a & calc.exe", Same),
        s("pipe", "a | more", Same),
        s("redirects", "a < b > c >> d 2>&1", Same),
        s("parentheses", "(a) ((b)", Same),
        s("bang", "!PATH! wow!", Same),
        s("quote then ampersand", r#"" & echo pwned & ""#, Same),
        s("JSON", r#"{"sandbox":{"filesystem":{"denyWrite":["C:\\a b\\"]}},"x":"%TEMP% & ^"}"#, Same),
        s("tab", "a\tb", Same),
        s("umlauts", "Grüße, Jürgen", Same),
        s("emoji", "ship it 🚀👍🏽", Same),
        s("CJK", "日本語のテスト 中文", Same),
        s("empty", "", Same),
        s("long (4000)", &"x".repeat(4000), Same),
        s("very long (9000)", &"x".repeat(9000), TooLong),
        s("newline", "line one\nline two", Refused),
        s("CR", "a\rb", Refused),
        s("CRLF", "a\r\nb", Refused),
        s("trailing newline", "notes\n", Refused),
        s("NUL", "a\0b", Refused),
    ]
}

fn expected(arg: &str, expect: Expect) -> Box<dyn Fn(&Outcome) -> bool + '_> {
    match expect {
        Expect::Same => Box::new(move |o| *o == Outcome::Arrived(vec!["before".into(), arg.into(), "after".into()])),
        Expect::Refused => Box::new(|o| *o == Outcome::Refused),
        Expect::TooLong => Box::new(|o| matches!(o, Outcome::CmdFailed(said) if said.contains("too long"))),
    }
}

fn short(o: &Outcome) -> String {
    match o {
        Outcome::Arrived(argv) if argv.len() == 3 => {
            let a = &argv[1];
            if a.chars().count() > 40 { format!("arrived ({} chars)", a.chars().count()) } else { format!("arrived {a:?}") }
        }
        Outcome::Arrived(argv) => format!("arrived MANGLED {argv:?}"),
        Outcome::Refused => "refused (InvalidInput)".into(),
        Outcome::CmdFailed(said) => format!("cmd.exe: {said}"),
    }
}

/// Each argument between two others (so a split or a swallowed neighbour shows), straight to
/// the program and through each shim. Run with `--nocapture` to see the table.
#[test]
fn what_a_cmd_shim_receives_from_std() {
    let shims = Shims::new("std");
    let mut wrong = vec![];
    println!("| case | direct .exe | simple .cmd | npm .cmd |\n|---|---|---|---|");
    for (name, arg, expect) in cases() {
        let args = ["before", arg.as_str(), "after"];
        let direct = run_std(&shims.node(), &args);
        let simple = run_std(&shims.simple(), &args);
        let npm = run_std(&shims.npm(), &args);
        println!("| {name} | {} | {} | {} |", short(&direct), short(&simple), short(&npm));
        // Straight to a program, only NUL is refused: the shims are what's being tested.
        let direct_ok = if arg.contains('\0') { direct == Outcome::Refused } else { expected(&arg, Expect::Same)(&direct) };
        let ok = expected(&arg, expect);
        if !direct_ok || !ok(&simple) || !ok(&npm) {
            wrong.push(format!("{name}: expected {expect:?}; direct {direct:?}, simple {simple:?}, npm {npm:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// tokio's `Command` builds the same command line as std's (it wraps it): every case comes out
/// the same both ways.
#[tokio::test]
async fn tokio_passes_arguments_to_a_cmd_shim_as_std_does() {
    let shims = Shims::new("tokio");
    let mut differ = vec![];
    for (name, arg, _) in cases() {
        let args = ["before", arg.as_str(), "after"];
        for shim in [shims.simple(), shims.npm()] {
            let (s, t) = (run_std(&shim, &args), run_tokio(&shim, &args).await);
            if s != t {
                differ.push(format!("{name} via {}: std {s:?}, tokio {t:?}", shim.display()));
            }
        }
    }
    assert!(differ.is_empty(), "{}", differ.join("\n"));
}

/// `detect::batch_args_problem`, which Trek asks before starting a `.cmd`, objects to exactly the
/// rows std refuses or cmd.exe can't take, and to nothing it passes.
#[test]
fn the_check_before_starting_agrees_with_the_table() {
    let shims = Shims::new("check");
    for (name, arg, expect) in cases() {
        let args = ["before", arg.as_str(), "after"];
        let problem = trek_core::detect::batch_args_problem(&shims.npm(), args);
        // NUL is refused for any program, by std; it's not the shim's to say.
        let objects = matches!(expect, Expect::Refused | Expect::TooLong) && !arg.contains('\0');
        assert_eq!(problem.is_some(), objects, "{name}: {problem:?}");
        assert_eq!(trek_core::detect::batch_args_problem(&shims.node(), args), None, "{name}");
    }
}

/// A user folder that isn't ASCII (`C:\Users\Jürgen`) holds the shim: it still runs, and its
/// arguments arrive.
#[test]
fn a_shim_in_a_folder_with_a_non_ascii_name_runs() {
    let shims = Shims::new("Jürgen 日本");
    for shim in [shims.simple(), shims.npm()] {
        assert_eq!(run_std(&shim, &["a b", "%PATH%"]), Outcome::Arrived(vec!["a b".into(), "%PATH%".into()]), "{}", shim.display());
    }
}

/// `Command` looks a bare name up as `name.exe` only: `npx` alone doesn't find `npx.cmd`.
#[test]
fn a_bare_name_never_finds_a_cmd() {
    let shims = Shims::new("bare");
    let path = std::env::join_paths([&shims.0]).unwrap();
    let err = std::process::Command::new("simple").env("PATH", &path).env(ECHO, "1").stdin(Stdio::null()).output().unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    let found = std::process::Command::new("simple.cmd").env("PATH", &path).env(ECHO, "1").stdin(Stdio::null()).output();
    assert_eq!(outcome(Path::new("simple.cmd"), found), Outcome::Arrived(vec![]));
}

/// An MCP server the user typed as a `.cmd` with an argument cmd.exe can't take is told so in
/// words, not started; its other arguments are passed as written (the stand-in isn't a server,
/// so the check then fails for that reason, having started it).
#[tokio::test]
async fn an_mcp_check_says_why_a_cmd_can_t_take_its_arguments() {
    let shims = Shims::new("mcp");
    let npm = shims.npm().display().to_string();
    let server = crate::McpServer::stdio("x", npm.clone(), vec!["--note".into(), "two\nlines".into()], vec![]);
    let said = crate::mcp_check::list_tools(&server).await.unwrap_err();
    assert!(said.contains("line break") && said.contains("argument 2"), "{said}");
    let server = crate::McpServer::stdio("x", npm, vec!["-y".into(), "@scope/server & more".into()], vec![]);
    let said = crate::mcp_check::list_tools(&server).await.unwrap_err();
    assert!(!said.contains("line break") && !said.contains("couldn't start"), "{said}");
}
