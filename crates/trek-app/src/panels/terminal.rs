//! A real terminal: the user's login shell in a PTY, parsed by vt100, painted as styled rows.

use crate::workspace::Workspace;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::cell::Cell;
use std::io::{Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The text size a terminal starts at; ⌘+ and ⌘- move it within `FONT_SIZES`, ⌘0 brings it back.
const FONT_SIZE: f32 = 12.5;
const FONT_SIZES: std::ops::RangeInclusive<f32> = 8.0..=28.0;
/// A row's height for the text size: 18 points at 12.5.
fn line_height_for(font_size: f32) -> f32 {
    (font_size * 18. / FONT_SIZE).round()
}

/// How long the bell's flash shows.
const BELL: Duration = Duration::from_millis(150);

/// Prefer an installed Nerd Font so prompt glyphs (powerline, icons) render.
const PREFERRED: &[&str] = &[
    "FiraCode Nerd Font Mono",
    "JetBrainsMono Nerd Font Mono",
    "MesloLGS NF",
    "MesloLGM Nerd Font Mono",
    "Hack Nerd Font Mono",
    "SauceCodePro Nerd Font Mono",
];

fn terminal_font(window: &Window, fallback: SharedString) -> SharedString {
    let names = window.text_system().all_font_names();
    PREFERRED
        .iter()
        .find(|p| names.iter().any(|n| n == *p))
        .map(|p| SharedString::from(*p))
        .unwrap_or(fallback)
}

/// What the shell's output asked of the terminal besides drawing it, kept until the view looks:
/// the bell, and a window title (OSC 0 / 2, which shells and programs set to say what's running).
#[derive(Default)]
pub(crate) struct Events {
    bell: bool,
    title: Option<String>,
}

impl vt100::Callbacks for Events {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(String::from_utf8_lossy(title).trim().to_string());
    }
}

type Parser = vt100::Parser<Events>;

/// The screen, shared with the thread that reads the shell's output: output is parsed there, so a
/// flood of it (`yes`, `cat` of a big file) never queues up for or stalls the main thread. The
/// view only hears that the screen changed, at most once per frame.
type Screen = Arc<Mutex<Parser>>;

/// A screen `rows` by `cols`, keeping `scrollback` lines that have scrolled off it.
fn new_screen(rows: u16, cols: u16, scrollback: usize) -> Screen {
    Arc::new(Mutex::new(Parser::new_with_callbacks(
        rows,
        cols,
        scrollback,
        Events::default(),
    )))
}

/// Repaints at most this often while output streams in.
const FRAME: Duration = Duration::from_millis(16);

/// Write `bytes` to the shell's input.
fn write_to(writer: &Mutex<Box<dyn Write + Send>>, bytes: &[u8]) {
    let mut w = writer.lock().unwrap_or_else(|e| e.into_inner());
    let _ = w.write_all(bytes);
    let _ = w.flush();
}

/// "Where is the cursor?" (DSR). Windows' ConPTY asks it before anything else and sends no output
/// until it's answered, so without a reply the terminal stays blank.
const CURSOR_QUERY: &[u8] = b"\x1b[6n";

/// How many times `chunk` asks where the cursor is, counting a query that began in the last read
/// (`carry`, at most the query's length less one) and ends in this one.
fn cursor_queries(carry: &[u8], chunk: &[u8]) -> usize {
    let joined = [carry, chunk].concat();
    joined
        .windows(CURSOR_QUERY.len())
        .filter(|w| *w == CURSOR_QUERY)
        .count()
}

/// The cursor-position report for a cursor at `(row, col)`, which count from 0: `ESC [ row ; col R`.
fn cursor_report((row, col): (u16, u16)) -> String {
    format!("\x1b[{};{}R", row + 1, col + 1)
}

/// Feed the shell's output into `screen` until it ends, waking the view after each read. `wake`
/// holds one wake-up at most: while one is waiting, more output only changes the screen. `reply`
/// writes back to the shell what its output asks for (the cursor position, on Windows).
fn pump(
    mut reader: impl Read,
    screen: &Screen,
    wake: &async_channel::Sender<()>,
    reply: impl Fn(&[u8]),
) {
    let mut buf = [0u8; 16384];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                // Answered after the screen is let go: a write to a busy shell mustn't hold up painting.
                let queries = cursor_queries(&carry, &buf[..n]);
                let cursor = {
                    let mut screen = screen.lock().unwrap_or_else(|e| e.into_inner());
                    screen.process(&buf[..n]);
                    screen.screen().cursor_position()
                };
                for _ in 0..queries {
                    reply(cursor_report(cursor).as_bytes());
                }
                // The last bytes of what was read so far, carry included: a query may arrive one
                // byte per read.
                let mut tail = std::mem::take(&mut carry);
                tail.extend_from_slice(&buf[..n]);
                carry = tail[tail.len().saturating_sub(CURSOR_QUERY.len() - 1)..].to_vec();
                if let Err(async_channel::TrySendError::Closed(_)) = wake.try_send(()) {
                    break;
                }
            }
        }
    }
}

/// How a shell takes its arguments: what "log in" and "run this command" look like.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Flavor {
    /// `-l`, and `-c <script>` (zsh, bash, fish, Git Bash).
    Posix,
    /// PowerShell 7 and Windows PowerShell: no login flag.
    PowerShell,
    /// `cmd.exe`.
    Cmd,
}

/// The kind of shell `program` is. Only Windows tells the three apart: `pwsh` on a Mac is still
/// started the way every shell there is.
fn flavor(program: &str, windows: bool) -> Flavor {
    if !windows {
        return Flavor::Posix;
    }
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    match name
        .rsplit_once('.')
        .map_or(name.as_str(), |(stem, _)| stem)
    {
        "pwsh" | "powershell" => Flavor::PowerShell,
        "cmd" => Flavor::Cmd,
        _ => Flavor::Posix,
    }
}

/// The shell to start. `setting` is `[terminal] shell`: a path is used as written, a bare name is
/// looked up with `find`. Empty, it's `$SHELL` (zsh without one) on a Mac; on Windows PowerShell 7,
/// else Windows PowerShell, else `%COMSPEC%` (cmd.exe).
fn choose_shell(
    setting: &str,
    env_shell: Option<&str>,
    windows: bool,
    find: impl Fn(&str) -> Option<PathBuf>,
    comspec: Option<&str>,
) -> String {
    let setting = setting.trim().trim_matches('"');
    if !setting.is_empty() {
        if setting.contains(['/', '\\']) {
            return setting.to_string();
        }
        return find(setting)
            .map_or_else(|| setting.to_string(), |p| p.to_string_lossy().into_owned());
    }
    if !windows {
        return env_shell.unwrap_or("/bin/zsh").to_string();
    }
    ["pwsh", "powershell"]
        .into_iter()
        .find_map(|name| find(name))
        .map(|p| p.to_string_lossy().into_owned())
        .or_else(|| comspec.filter(|c| !c.is_empty()).map(str::to_string))
        .unwrap_or_else(|| "cmd.exe".into())
}

/// `s` as a PowerShell single-quoted string: nothing inside is expanded, and a quote is doubled.
/// PowerShell reads the typographic single quotes (U+2018 to U+201B) as quotes too.
fn powershell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        out.push(c);
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// The arguments that start a `flavor` shell, or that run `command` in one. A command's shell
/// exits when it's done (Install / Sign in rescan agents when it does), after printing what ran
/// and how it ended; the screen stays up.
fn shell_args(flavor: Flavor, command: Option<&str>) -> Vec<String> {
    match (flavor, command) {
        (Flavor::Posix, None) => vec!["-l".into()],
        (Flavor::Posix, Some(c)) => {
            let quoted = format!("'{}'", c.replace('\'', "'\\''"));
            vec![
                "-l".into(),
                "-c".into(),
                format!(
                    "printf '\\033[1m$ %s\\033[0m\\n\\n' {quoted}; {c}; code=$?; printf '\\n\\033[2m[finished with exit code %s]\\033[0m\\n' $code; exit $code"
                ),
            ]
        }
        (Flavor::PowerShell, None) => vec!["-NoLogo".into()],
        (Flavor::PowerShell, Some(c)) => vec![
            "-NoLogo".into(),
            "-Command".into(),
            // `$?` is only the command's until the next statement; a native program's code is in `$LASTEXITCODE`.
            format!(
                "Write-Host {}; Write-Host ''; $global:LASTEXITCODE = $null; {c}; $ok = $?; $code = if ($null -ne $LASTEXITCODE) {{ $LASTEXITCODE }} elseif ($ok) {{ 0 }} else {{ 1 }}; Write-Host ''; Write-Host ('[finished with exit code ' + $code + ']'); exit $code",
                powershell_quote(&format!("$ {c}"))
            ),
        ],
        (Flavor::Cmd, None) => vec![],
        // cmd reads the rest of its command line as the command, quotes and all; `^%` defers
        // `%errorlevel%` to when `call` reaches it, after the command has set it. Only that one
        // `call`: another before it (`call echo.`) would reset the level to 0.
        (Flavor::Cmd, Some(c)) => vec![
            "/c".into(),
            format!("{c} & echo. & call echo [finished with exit code %^errorlevel%]"),
        ],
    }
}

/// `path` without the `\\?\` that canonicalising puts on a Windows path: `\\?\C:\dir` is `C:\dir`
/// and `\\?\UNC\host\share` is `\\host\share`. Shells show the prefix and `cmd.exe` mishandles it.
fn plain_path(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path;
    };
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(drive) if drive.as_bytes().get(1) == Some(&b':') => PathBuf::from(drive),
        _ => path,
    }
}

/// Where the shell starts: `cwd` (the project) or home. On Windows, as a plain path; and not a
/// network one for `cmd.exe`, which can't be in one and would start in the Windows folder. A folder
/// that's gone is home, and with no home either (`None`) the shell starts where Trek is, so a
/// terminal always opens.
fn shell_cwd(
    cwd: Option<PathBuf>,
    flavor: Flavor,
    windows: bool,
    home: PathBuf,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let cwd = cwd.unwrap_or_else(|| home.clone());
    let cwd = if !windows {
        cwd
    } else {
        let cwd = plain_path(cwd);
        if flavor == Flavor::Cmd && cwd.to_str().is_some_and(|p| p.starts_with(r"\\")) {
            home.clone()
        } else {
            cwd
        }
    };
    [cwd, home].into_iter().find(|dir| exists(dir))
}

/// What a terminal runs besides an interactive shell, and whose command it is.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    /// One of Trek's own (an agent's install or sign-in, a plugin). On Windows these are written
    /// for PowerShell (`irm ... | iex`), so they run in it whatever `[terminal] shell` says.
    Setup(String),
    /// The user's (a project action): written for the shell they chose, and run by it.
    Action(String),
}

impl Job {
    fn command(&self) -> &str {
        match self {
            Job::Setup(c) | Job::Action(c) => c,
        }
    }
}

/// The shell to start for `job` (none: an interactive shell). The setting picks the interactive
/// shell and the user's own commands' shell; Trek's own commands get the automatic choice.
fn shell_for(
    setting: &str,
    job: Option<&Job>,
    env_shell: Option<&str>,
    windows: bool,
    find: impl Fn(&str) -> Option<PathBuf>,
    comspec: Option<&str>,
) -> String {
    let setting = if matches!(job, Some(Job::Setup(_))) {
        ""
    } else {
        setting
    };
    choose_shell(setting, env_shell, windows, find, comspec)
}

/// The shell for `[terminal] shell = setting` as a process to start in a pty, in `cwd`, running
/// `job` when there is one.
/// The shell's name, as a terminal tab shows it ("zsh", "pwsh"): what `[terminal] shell` (`setting`)
/// resolves to on this machine, without its folder or extension.
pub fn shell_name(setting: &str) -> String {
    let shell = shell_for(
        setting,
        None,
        std::env::var("SHELL").ok().as_deref(),
        cfg!(windows),
        |name| trek_core::detect::which(name),
        std::env::var("COMSPEC").ok().as_deref(),
    );
    std::path::Path::new(&shell)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or(shell)
}

fn shell_command(setting: &str, cwd: Option<PathBuf>, job: Option<&Job>) -> CommandBuilder {
    let windows = cfg!(windows);
    let shell = shell_for(
        setting,
        job,
        std::env::var("SHELL").ok().as_deref(),
        windows,
        |name| trek_core::detect::which(name),
        std::env::var("COMSPEC").ok().as_deref(),
    );
    let command = job.map(Job::command);
    let flavor = flavor(&shell, windows);
    let mut cmd = CommandBuilder::new(shell);
    cmd.args(shell_args(flavor, command));
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "Trek");
    if windows {
        // A shell started from the Start menu has the PATH of whatever started that; a terminal
        // has the one Windows would give a new session (and `login_path` adds agent CLIs' folders).
        cmd.env("PATH", trek_core::detect::login_path());
    }
    if let Some(dir) = shell_cwd(cwd, flavor, windows, trek_core::paths::home(), |dir| {
        dir.is_dir()
    }) {
        cmd.cwd(dir);
    }
    cmd
}

/// What a key press does in the terminal besides going to the shell.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shortcut {
    Paste,
    /// Copy the selection. With nothing selected the key goes on as if this weren't here.
    Copy,
    /// Clear the screen (Ctrl+L, which the shell understands).
    Clear,
    /// To the start or the end of the line, or delete back to its start: ⌘←, ⌘→ and ⌘⌫ on a Mac,
    /// as Terminal's and iTerm's text editing have them.
    LineStart,
    LineEnd,
    DeleteLine,
    /// The text a size bigger or smaller, or back to the usual size.
    Bigger,
    Smaller,
    ActualSize,
    /// Not the terminal's: the app's shortcuts get it.
    PassThrough,
}

/// The terminal's shortcuts. On a Mac they're Cmd+V, Cmd+C, Cmd+K, Cmd+arrows, Cmd+Backspace and
/// Cmd+= / - / 0, and every other Cmd key is the app's. Elsewhere `secondary` (Ctrl) is what the
/// shell's own keys use (Ctrl+C is interrupt, Ctrl+V is PowerShell's paste, which reads the
/// clipboard itself), so the terminal doesn't take it as the app's: copying and pasting are
/// Ctrl+Shift+C and Ctrl+Shift+V (or Shift+Insert), and the text size Ctrl+= / - / 0, as in
/// Windows Terminal.
fn shortcut(key: &str, m: &Modifiers, windows: bool) -> Option<Shortcut> {
    if !windows {
        let plain = !m.alt && !m.control && !m.shift;
        return m.platform.then(|| match key {
            "v" => Shortcut::Paste,
            "c" => Shortcut::Copy,
            "k" => Shortcut::Clear,
            "left" if plain => Shortcut::LineStart,
            "right" if plain => Shortcut::LineEnd,
            "backspace" if plain => Shortcut::DeleteLine,
            "=" | "+" => Shortcut::Bigger,
            "-" => Shortcut::Smaller,
            "0" => Shortcut::ActualSize,
            _ => Shortcut::PassThrough,
        });
    }
    match key {
        _ if m.platform => Some(Shortcut::PassThrough),
        "v" if m.control && m.shift && !m.alt => Some(Shortcut::Paste),
        "insert" if m.shift && !m.control && !m.alt => Some(Shortcut::Paste),
        "c" if m.control && m.shift && !m.alt => Some(Shortcut::Copy),
        "=" | "+" if m.control && !m.alt => Some(Shortcut::Bigger),
        "-" if m.control && !m.shift && !m.alt => Some(Shortcut::Smaller),
        "0" if m.control && !m.shift && !m.alt => Some(Shortcut::ActualSize),
        _ => None,
    }
}

/// Home or End as the screen asks for them: ESC O in application cursor mode (full-screen
/// programs), else ESC [.
fn home_end(end: bool, application: bool) -> &'static [u8] {
    match (end, application) {
        (false, true) => b"\x1bOH",
        (false, false) => b"\x1b[H",
        (true, true) => b"\x1bOF",
        (true, false) => b"\x1b[F",
    }
}

/// What a wheel turned `up` (or down) a notch over the cell at `(row, col)` sends a program that
/// asked for the mouse: button 64 or 65, in the encoding it asked for.
fn wheel_report(
    up: bool,
    (row, col): (u16, u16),
    encoding: vt100::MouseProtocolEncoding,
) -> Vec<u8> {
    let button: u32 = if up { 64 } else { 65 };
    let (x, y) = (col as u32 + 1, row as u32 + 1);
    match encoding {
        vt100::MouseProtocolEncoding::Sgr => format!("\x1b[<{button};{x};{y}M").into_bytes(),
        // X10: each value plus 32 in a byte, which only goes so far.
        vt100::MouseProtocolEncoding::Default => vec![
            0x1b,
            b'[',
            b'M',
            (32 + button) as u8,
            (32 + x).min(255) as u8,
            (32 + y).min(255) as u8,
        ],
        vt100::MouseProtocolEncoding::Utf8 => {
            let mut out = b"\x1b[M".to_vec();
            for v in [32 + button, 32 + x, 32 + y] {
                out.extend(char::from_u32(v).unwrap_or(' ').to_string().into_bytes());
            }
            out
        }
    }
}

/// What to write to the shell for pasted `text`: wrapped when it asked for bracketed paste, and
/// on Windows with line ends as Enter (CR) like a typed line, not the clipboard's CRLF.
fn paste_bytes(text: &str, bracketed: bool, windows: bool) -> String {
    let text = if windows {
        text.replace("\r\n", "\r").replace('\n', "\r")
    } else {
        text.to_string()
    };
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~")
    } else {
        text
    }
}

/// Ends the shell when the terminal goes away. The output thread holds duplicates of the pty's
/// master, so dropping the panel hangs nothing up by itself: an idle shell would stay at its
/// prompt and the thread would stay blocked in `read` for good. Once the shell is gone the pty
/// closes, the read ends, and the thread reaps the child.
struct Hangup {
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Set by the output thread once it has reaped the child: its pid may be someone else's by then.
    reaped: Arc<AtomicBool>,
}

impl Drop for Hangup {
    fn drop(&mut self) {
        if !self.reaped.load(Ordering::SeqCst) {
            let _ = self.killer.kill();
        }
    }
}

/// A shell on a pty with the thread that feeds its output into the screen.
struct Session {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Box<dyn MasterPty + Send>,
    hangup: Hangup,
    /// Ends after the shell has: its output is over and the child is reaped.
    thread: std::thread::JoinHandle<()>,
}

/// Start `cmd` on a new pty of `size`, pumping its output into `screen` and waking `wake`.
fn start_session(
    size: (u16, u16),
    cmd: CommandBuilder,
    screen: Screen,
    wake: async_channel::Sender<()>,
) -> anyhow::Result<Session> {
    let pty = native_pty_system().openpty(PtySize {
        rows: size.0,
        cols: size.1,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut child = pty.slave.spawn_command(cmd)?;
    let hangup = Hangup {
        killer: child.clone_killer(),
        reaped: Arc::new(AtomicBool::new(false)),
    };
    let reader = pty.master.try_clone_reader()?;
    let writer = Arc::new(Mutex::new(pty.master.take_writer()?));
    let answering = writer.clone();
    let reaped = hangup.reaped.clone();
    let thread = std::thread::spawn(move || {
        // Only ConPTY asks where the cursor is, and a Mac's shells never got an answer.
        pump(reader, &screen, &wake, |bytes| {
            if cfg!(windows) {
                write_to(&answering, bytes);
            }
        });
        let _ = child.wait();
        reaped.store(true, Ordering::SeqCst);
    });
    Ok(Session {
        writer,
        master: pty.master,
        hangup,
        thread,
    })
}

/// A place in the output: a line counted from the top of the live screen (negative back in the
/// scrollback, so it stays put as the view scrolls), and a column boundary (0 before the first cell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Pos {
    line: isize,
    col: u16,
}

/// Text picked with the pointer: from where the button went down (`anchor`) to where it went
/// (`head`), and what that held when last looked at.
#[derive(Debug, Clone)]
struct Selection {
    anchor: Pos,
    head: Pos,
    text: String,
}

impl Selection {
    /// Its start and end, the start first.
    fn ordered(&self) -> (Pos, Pos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// Whether the cell at `col` of `line` is between `start` and `end`.
fn is_selected((start, end): (Pos, Pos), line: isize, col: u16) -> bool {
    Pos { line, col } >= start && Pos { line, col: col + 1 } <= end
}

/// The text from `start` to `end` (`start` first), wherever in the output it is now: a line a
/// row, blanks at a row's end dropped, but no line break where a long line ran on to the next
/// row. What has left the scrollback since is left out.
fn selected_text(parser: &mut Parser, start: Pos, end: Pos) -> String {
    let screen = parser.screen_mut();
    let kept = screen.scrollback();
    let (rows, cols) = screen.size();
    let mut out = String::new();
    for line in start.line..=end.line {
        // The view scrolled so the line is on it: at the top when it's back in the scrollback.
        screen.set_scrollback(usize::try_from(-line).unwrap_or(0));
        let row = line + screen.scrollback() as isize;
        if !(0..rows as isize).contains(&row) {
            continue;
        }
        let row = row as u16;
        let from = if line == start.line { start.col } else { 0 };
        let to = if line == end.line {
            end.col.min(cols)
        } else {
            cols
        };
        let mut text = String::new();
        for c in from..to {
            let Some(cell) = screen.cell(row, c) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let s = cell.contents();
            text.push_str(if s.is_empty() { " " } else { s });
        }
        let runs_on = line != end.line && screen.row_wrapped(row);
        out.push_str(if runs_on { &text } else { text.trim_end() });
        if line != end.line && !runs_on {
            out.push('\n');
        }
    }
    screen.set_scrollback(kept);
    out
}

/// Row `row` of `screen` as text, a cell at a time (a blank cell a space), with the column each
/// byte of it came from.
fn row_text(screen: &vt100::Screen, row: u16) -> (String, Vec<u16>) {
    let (mut text, mut at) = (String::new(), Vec::new());
    for c in 0..screen.size().1 {
        let Some(cell) = screen.cell(row, c) else {
            continue;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let s = cell.contents();
        let s = if s.is_empty() { " " } else { s };
        text.push_str(s);
        at.extend(std::iter::repeat_n(c, s.len()));
    }
    (text, at)
}

/// A link on screen: its row, the columns it covers there, and the address it opens.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub row: u16,
    pub cols: Range<u16>,
    pub url: String,
}

/// The links on row `row` of `screen`.
fn row_links(screen: &vt100::Screen, row: u16) -> Vec<Link> {
    let (text, at) = row_text(screen, row);
    crate::panels::terminal_links::find_links(&text)
        .into_iter()
        .map(|f| Link {
            row,
            cols: at[f.range.start]..at[f.range.end - 1] + 1,
            url: f.url,
        })
        .collect()
}

/// The word around the cell at `col` of `row` (a double-click's): a run of cells that aren't
/// blank, quotes, brackets or a TUI's frame. A cell that isn't in one is a word of its own.
fn word_bounds(screen: &vt100::Screen, row: u16, col: u16) -> (u16, u16) {
    let cols = screen.size().1;
    let in_word = |c: u16| {
        screen.cell(row, c).is_some_and(|cell| {
            let s = cell.contents();
            cell.is_wide_continuation()
                || (!s.trim().is_empty()
                    && !matches!(
                        s,
                        "\"" | "'"
                            | "`"
                            | "("
                            | ")"
                            | "["
                            | "]"
                            | "{"
                            | "}"
                            | "<"
                            | ">"
                            | "|"
                            | "│"
                    ))
        })
    };
    if !in_word(col) {
        return (col, (col + 1).min(cols));
    }
    let (mut a, mut b) = (col, col + 1);
    while a > 0 && in_word(a - 1) {
        a -= 1;
    }
    while b < cols && in_word(b) {
        b += 1;
    }
    (a, b)
}

/// A terminal's window title changed (OSC 0 / 2): its tab says it.
pub struct TitleChanged;

pub struct TerminalPanel {
    /// Where a ⌘-clicked link opens (`terminal_links::open`).
    workspace: WeakEntity<Workspace>,
    parser: Screen,
    /// Shared with the thread that reads the shell's output, which answers its queries (ConPTY's).
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    master: Option<Box<dyn MasterPty + Send>>,
    /// Hangs the shell up when the panel is dropped (a closed tab).
    _hangup: Option<Hangup>,
    size: (u16, u16),
    focus: FocusHandle,
    exited: bool,
    /// The text size (⌘+ / ⌘-).
    font_size: f32,
    /// Resolved for a size: font family, that size, and a cell's width at it.
    font: Option<(SharedString, f32, f32)>,
    /// Where the rows were last drawn, in the window.
    grid: Rc<Cell<Bounds<Pixels>>>,
    /// Runs once when the shell exits (used by Install / Sign in to rescan agents).
    on_exit: Option<Box<dyn FnOnce(&mut App)>>,
    _reader: Option<Task<()>>,
    /// What the wheel has turned short of a whole line, in points.
    wheel: f32,
    selection: Option<Selection>,
    /// The left button is down after a single click here: where, for a drag to select from.
    press: Option<Pos>,
    /// The scrollback as last seen: output that grows it while the view is back there moves the
    /// selection with the lines it's on.
    seen_scrollback: usize,
    /// The link under the pointer while ⌘ (Ctrl on Windows) is held: underlined, a click opens it.
    hovered: Option<Link>,
    /// The pointer over the rows, and whether ⌘ was held, as last seen.
    pointer: Option<(Point<Pixels>, bool)>,
    /// The window title the shell or a program set, if any.
    title: Option<String>,
    /// When the bell last rang, while its flash shows.
    bell_at: Option<Instant>,
}

impl EventEmitter<TitleChanged> for TerminalPanel {}

fn ansi(idx: u8) -> Hsla {
    const BASE: [u32; 16] = [
        0x1A1A1D, 0xFF5A5F, 0x3FCF8E, 0xFFB020, 0x5AA9FF, 0xB48CFF, 0x4FD1D9, 0xD7DAE0, 0x5C6370,
        0xFF7B80, 0x6BE3A8, 0xFFC857, 0x7FBDFF, 0xC9A8FF, 0x7FE0E6, 0xFFFFFF,
    ];
    let v = match idx {
        0..=15 => BASE[idx as usize],
        16..=231 => {
            let i = idx - 16;
            let c = |n: u8| if n == 0 { 0 } else { 55 + n as u32 * 40 };
            (c(i / 36) << 16) | (c((i / 6) % 6) << 8) | c(i % 6)
        }
        _ => {
            let g = 8 + (idx as u32 - 232) * 10;
            (g << 16) | (g << 8) | g
        }
    };
    rgb(v).into()
}

fn color(c: vt100::Color) -> Option<Hsla> {
    match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(ansi(i)),
        vt100::Color::Rgb(r, g, b) => {
            Some(rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into())
        }
    }
}

impl TerminalPanel {
    /// A terminal in `cwd`, for `workspace`. `shell` is `[terminal] shell` as the workspace holds it now.
    pub fn new(
        workspace: WeakEntity<Workspace>,
        cwd: Option<PathBuf>,
        shell: String,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_job(workspace, cwd, None, shell, cx)
    }

    /// A terminal that runs one command in a login shell (agent install / sign-in, a project
    /// action), shows its output and stays open after it exits.
    pub fn with_command(
        workspace: WeakEntity<Workspace>,
        cwd: Option<PathBuf>,
        job: Job,
        shell: String,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_job(workspace, cwd, Some(job), shell, cx)
    }

    fn with_job(
        workspace: WeakEntity<Workspace>,
        cwd: Option<PathBuf>,
        job: Option<Job>,
        shell: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let size = (30u16, 90u16);
        let mut this = Self {
            workspace,
            parser: new_screen(size.0, size.1, 2000),
            writer: None,
            master: None,
            _hangup: None,
            size,
            focus: cx.focus_handle(),
            exited: false,
            font_size: FONT_SIZE,
            font: None,
            grid: Rc::new(Cell::new(Bounds::default())),
            on_exit: None,
            _reader: None,
            wheel: 0.,
            selection: None,
            press: None,
            seen_scrollback: 0,
            hovered: None,
            pointer: None,
            title: None,
            bell_at: None,
        };
        // An isolated (test) process starts no shell: one would run the user's login profile,
        // and the command (a project's tests, an agent's sign-in) for real.
        if trek_core::paths::isolated() {
            this.screen().process(
                format!(
                    "$ {}\r\n(no shell in tests)\r\n",
                    job.as_ref().map_or("", Job::command)
                )
                .as_bytes(),
            );
            return this;
        }
        if let Err(e) = this.spawn(cwd, job, &shell, cx) {
            this.screen()
                .process(format!("Couldn't start a shell: {e}\r\n").as_bytes());
        }
        this
    }

    fn screen(&self) -> MutexGuard<'_, Parser> {
        self.parser.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn spawn(
        &mut self,
        cwd: Option<PathBuf>,
        job: Option<Job>,
        shell: &str,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let cmd = shell_command(shell, cwd, job.as_ref());
        let (tx, rx) = async_channel::bounded::<()>(1);
        let session = start_session(self.size, cmd, self.parser.clone(), tx)?;
        let Session {
            writer,
            master,
            hangup,
            thread,
        } = session;
        drop(thread); // detached: it ends by itself once the shell has
        self.writer = Some(writer);
        self.master = Some(master);
        self._hangup = Some(hangup);
        self._reader = Some(cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                if this.update(cx, |this, cx| this.output_arrived(cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(FRAME).await;
            }
            let _ = this.update(cx, |this, cx| {
                this.exited = true;
                if let Some(f) = this.on_exit.take() {
                    f(cx);
                }
                cx.notify();
            });
        }));
        Ok(())
    }

    /// Runs once when the process exits.
    pub fn on_exit(&mut self, f: impl FnOnce(&mut App) + 'static) {
        self.on_exit = Some(Box::new(f));
    }

    /// The window title the shell or the program running set (OSC 0 / 2), if it set one.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref().filter(|t| !t.is_empty())
    }

    /// The screen changed under the view (the shell's output, parsed on its own thread): hear
    /// the bell and the title it asked for, keep the selection on the lines it was on, or let it
    /// go when what it covered has changed, and redraw.
    fn output_arrived(&mut self, cx: &mut Context<Self>) {
        let (bell, title, scrollback) = {
            let mut parser = self.screen();
            let scrollback = parser.screen().scrollback();
            let events = parser.callbacks_mut();
            (
                std::mem::take(&mut events.bell),
                events.title.take(),
                scrollback,
            )
        };
        if let Some(title) = title.filter(|t| Some(t) != self.title.as_ref()) {
            self.title = Some(title);
            cx.emit(TitleChanged);
        }
        if bell {
            self.ring(cx);
        }
        // Lines came in while the view was back in the scrollback: it stayed on its lines, and
        // they're that many further back now.
        let grew = scrollback.saturating_sub(self.seen_scrollback) as isize;
        self.seen_scrollback = scrollback;
        if let Some(sel) = self.selection.as_mut() {
            sel.anchor.line -= grew;
            sel.head.line -= grew;
            let (start, end) = sel.ordered();
            let now = selected_text(
                &mut self.parser.lock().unwrap_or_else(|e| e.into_inner()),
                start,
                end,
            );
            if now != sel.text {
                self.selection = None;
                self.press = None;
            }
        }
        if let Some((position, held)) = self.pointer {
            self.hover(position, held, cx);
        }
        cx.notify();
    }

    /// The bell: the terminal flashes, once however often it rings in a flash.
    fn ring(&mut self, cx: &mut Context<Self>) {
        if self.bell_at.is_some() {
            return;
        }
        self.bell_at = Some(Instant::now());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BELL).await;
            let _ = this.update(cx, |this, cx| {
                this.bell_at = None;
                cx.notify();
            });
        })
        .detach();
    }

    /// Write to the shell; the view goes back to the bottom, where what's typed shows.
    fn write(&mut self, bytes: &[u8]) {
        self.set_scrollback(0);
        if let Some(w) = &self.writer {
            write_to(w, bytes);
        }
    }

    /// Move the view to `lines` back into the scrollback (0: the bottom); how far it got.
    fn set_scrollback(&mut self, lines: usize) -> usize {
        let mut screen = self.screen();
        screen.screen_mut().set_scrollback(lines);
        let now = screen.screen().scrollback();
        drop(screen);
        self.seen_scrollback = now;
        now
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) == self.size || rows < 2 || cols < 10 {
            return;
        }
        self.size = (rows, cols);
        self.set_scrollback(0);
        self.screen().screen_mut().set_size(rows, cols);
        // What was picked is somewhere else now.
        self.selection = None;
        self.press = None;
        self.hovered = None;
        if let Some(m) = &self.master {
            let _ = m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            let bracketed = self.screen().screen().bracketed_paste();
            self.write(paste_bytes(&text, bracketed, cfg!(windows)).as_bytes());
        }
    }

    /// Copy the selection to the clipboard; false when nothing is selected.
    fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(sel) = &self.selection else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(sel.text.clone()));
        true
    }

    /// The text size, within `FONT_SIZES`: the rows and columns follow on the next frame.
    fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = size.clamp(*FONT_SIZES.start(), *FONT_SIZES.end());
        if size != self.font_size {
            self.font_size = size;
            self.wheel = 0.;
            self.hovered = None;
            cx.notify();
        }
    }

    fn line_height(&self) -> f32 {
        line_height_for(self.font_size)
    }

    fn cell_width(&self) -> f32 {
        self.font.as_ref().map_or(self.font_size * 0.6, |f| f.2)
    }

    /// The font family and a cell's width at the text size, resolved once per size.
    fn metrics(&mut self, window: &Window, fallback: &SharedString) -> (SharedString, f32) {
        if let Some((family, size, width)) = &self.font
            && *size == self.font_size
        {
            return (family.clone(), *width);
        }
        let family = self
            .font
            .as_ref()
            .map(|f| f.0.clone())
            .unwrap_or_else(|| terminal_font(window, fallback.clone()));
        let id = window.text_system().resolve_font(&font(family.clone()));
        let width = window
            .text_system()
            .em_advance(id, px(self.font_size))
            .map(|w| w.as_f32())
            .unwrap_or(self.font_size * 0.6);
        self.font = Some((family.clone(), self.font_size, width));
        (family, width)
    }

    /// The cell under `position` (in the window), as a row and column of the view.
    fn cell_at(&self, position: Point<Pixels>) -> Option<(u16, u16)> {
        let grid = self.grid.get();
        let (rows, cols) = self.screen().screen().size();
        let (x, y) = (
            (position.x - grid.origin.x).as_f32(),
            (position.y - grid.origin.y).as_f32(),
        );
        if x < 0. || y < 0. {
            return None;
        }
        let (row, col) = (
            (y / self.line_height()) as u16,
            (x / self.cell_width()) as u16,
        );
        (row < rows && col < cols).then_some((row, col))
    }

    /// The cell boundary nearest `position`, as a place in the output; above the rows is the
    /// first row's start, below them the last row's end.
    fn pos_at(&self, position: Point<Pixels>) -> Pos {
        let grid = self.grid.get();
        let screen = self.screen();
        let (rows, cols) = screen.screen().size();
        let back = screen.screen().scrollback() as isize;
        drop(screen);
        let (x, y) = (
            (position.x - grid.origin.x).as_f32(),
            (position.y - grid.origin.y).as_f32(),
        );
        let (row, col) = if y < 0. {
            (0, 0)
        } else if y >= rows as f32 * self.line_height() {
            (rows.saturating_sub(1), cols)
        } else {
            (
                (y / self.line_height()) as u16,
                (x / self.cell_width()).round().clamp(0., cols as f32) as u16,
            )
        };
        Pos {
            line: row as isize - back,
            col,
        }
    }

    /// Select from `anchor` to `head`; nothing when they're the same place.
    fn select(&mut self, anchor: Pos, head: Pos) {
        self.selection = (anchor != head).then(|| {
            let (start, end) = if anchor <= head {
                (anchor, head)
            } else {
                (head, anchor)
            };
            Selection {
                anchor,
                head,
                text: selected_text(&mut self.screen(), start, end),
            }
        });
    }

    /// The left button went down over the rows: with ⌘ (Ctrl on Windows) on a link, it opens;
    /// a click lets a selection go and may start a new one by dragging; a double click picks a
    /// word, a triple click the row.
    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        if e.modifiers.secondary()
            && let Some(link) = self.link_at(e.position)
        {
            self.open_link(link.url, window, cx);
            return;
        }
        self.press = None;
        match e.click_count {
            0 | 1 => {
                self.press = Some(self.pos_at(e.position));
                if self.selection.take().is_some() {
                    cx.notify();
                }
            }
            clicks => {
                let Some((row, col)) = self.cell_at(e.position) else {
                    return;
                };
                let (line, (a, b)) = {
                    let parser = self.screen();
                    let screen = parser.screen();
                    let span = if clicks == 2 {
                        word_bounds(screen, row, col)
                    } else {
                        (0, screen.size().1)
                    };
                    (row as isize - screen.scrollback() as isize, span)
                };
                self.select(Pos { line, col: a }, Pos { line, col: b });
                cx.notify();
            }
        }
    }

    /// The pointer moved with the left button down (anywhere in the window): the selection
    /// follows it from where the button went down here.
    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(anchor) = self.press else { return };
        let head = self.pos_at(position);
        if self.selection.as_ref().map_or(anchor, |s| s.head) == head {
            return;
        }
        self.select(anchor, head);
        cx.notify();
    }

    /// The link under `position`, if there's one.
    fn link_at(&self, position: Point<Pixels>) -> Option<Link> {
        let (row, col) = self.cell_at(position)?;
        row_links(self.screen().screen(), row)
            .into_iter()
            .find(|l| l.cols.contains(&col))
    }

    /// The pointer is at `position` with ⌘ held or not: the link under it, while held, is
    /// underlined and the pointer a hand.
    fn hover(&mut self, position: Point<Pixels>, held: bool, cx: &mut Context<Self>) {
        self.pointer = Some((position, held));
        let link = if held { self.link_at(position) } else { None };
        if link != self.hovered {
            self.hovered = link;
            cx.notify();
        }
    }

    /// Open `url`, a link in the output, where the user wants terminal links to go.
    fn open_link(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspace.upgrade() {
            crate::panels::terminal_links::open(&ws, url, window, cx);
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let m = &k.modifiers;
        // Shift+PageUp / PageDown page through the scrollback, as in Terminal.
        if m.shift
            && !m.control
            && !m.alt
            && !m.platform
            && matches!(k.key.as_str(), "pageup" | "pagedown")
        {
            let page = self.size.0.saturating_sub(1).max(1) as isize;
            self.scroll_by(if k.key == "pageup" { page } else { -page }, cx);
            cx.stop_propagation();
            return;
        }
        let (app, alternate) = {
            let parser = self.screen();
            (
                parser.screen().application_cursor(),
                parser.screen().alternate_screen(),
            )
        };
        if let Some(shortcut) = shortcut(&k.key, m, cfg!(windows)) {
            match shortcut {
                Shortcut::Paste => self.paste(cx),
                Shortcut::Copy => {
                    if !self.copy(cx) {
                        return;
                    }
                }
                Shortcut::Clear => self.write(b"\x0c"),
                // A full-screen program (an editor, less) reads Home and End; a shell's line
                // editor knows Ctrl+A, Ctrl+E and Ctrl+U.
                Shortcut::LineStart => self.write(if alternate {
                    home_end(false, app)
                } else {
                    b"\x01"
                }),
                Shortcut::LineEnd => self.write(if alternate {
                    home_end(true, app)
                } else {
                    b"\x05"
                }),
                Shortcut::DeleteLine => self.write(b"\x15"),
                Shortcut::Bigger => self.set_font_size(self.font_size + 1., cx),
                Shortcut::Smaller => self.set_font_size(self.font_size - 1., cx),
                Shortcut::ActualSize => self.set_font_size(FONT_SIZE, cx),
                Shortcut::PassThrough => return, // let app shortcuts through
            }
            cx.stop_propagation();
            return;
        }
        let seq: Option<Vec<u8>> = match k.key.as_str() {
            // Shift+Enter (and Alt+Enter) is Meta+Return: a new line in the prompt without running
            // it, in zsh and in agents' CLIs (Claude Code, Codex).
            "enter" => Some(if m.shift || m.alt {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }),
            "backspace" => Some(if m.alt {
                b"\x1b\x7f".to_vec()
            } else {
                b"\x7f".to_vec()
            }),
            "tab" => Some(if m.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }),
            "escape" => Some(b"\x1b".to_vec()),
            "up" => Some(if app { b"\x1bOA" } else { b"\x1b[A" }.to_vec()),
            "down" => Some(if app { b"\x1bOB" } else { b"\x1b[B" }.to_vec()),
            "right" => Some(if m.alt {
                b"\x1bf".to_vec()
            } else if app {
                b"\x1bOC".to_vec()
            } else {
                b"\x1b[C".to_vec()
            }),
            "left" => Some(if m.alt {
                b"\x1bb".to_vec()
            } else if app {
                b"\x1bOD".to_vec()
            } else {
                b"\x1b[D".to_vec()
            }),
            "home" => Some(home_end(false, app).to_vec()),
            "end" => Some(home_end(true, app).to_vec()),
            "delete" => Some(b"\x1b[3~".to_vec()),
            "pageup" => Some(b"\x1b[5~".to_vec()),
            "pagedown" => Some(b"\x1b[6~".to_vec()),
            "space" => Some(if m.control { vec![0] } else { b" ".to_vec() }),
            key if m.control && key.len() == 1 => {
                let c = key.as_bytes()[0].to_ascii_lowercase();
                Some(vec![c & 0x1f])
            }
            _ => k.key_char.as_ref().map(|s| {
                let mut v = Vec::new();
                if m.alt {
                    v.push(0x1b);
                }
                v.extend_from_slice(s.as_bytes());
                v
            }),
        };
        if let Some(seq) = seq {
            self.write(&seq);
            cx.stop_propagation();
        }
        let _ = window;
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Move the view `lines` back into the scrollback (forward, towards the bottom, when
    /// negative). New output while it's back there leaves it on the same lines.
    fn scroll_by(&mut self, lines: isize, cx: &mut Context<Self>) {
        let before = self.screen().screen().scrollback();
        if self.set_scrollback(before.saturating_add_signed(lines)) != before {
            self.hovered = None;
            cx.notify();
        }
    }

    /// The wheel (or trackpad) over the terminal: whole lines, what's left of a line kept for the
    /// next turn. A full-screen program that asked for the mouse gets the wheel as it asked; one
    /// that didn't (less, man) gets arrow keys, as the alternate screen has no scrollback; else
    /// the view scrolls back through the output.
    fn wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let line_height = self.line_height();
        self.wheel += event.delta.pixel_delta(px(line_height)).y.as_f32();
        let lines = (self.wheel / line_height).trunc();
        self.wheel -= lines * line_height;
        cx.stop_propagation();
        if lines == 0. {
            return;
        }
        let (mode, encoding, alternate, app) = {
            let parser = self.screen();
            let s = parser.screen();
            (
                s.mouse_protocol_mode(),
                s.mouse_protocol_encoding(),
                s.alternate_screen(),
                s.application_cursor(),
            )
        };
        let up = lines > 0.;
        let count = lines.abs() as usize;
        if mode != vt100::MouseProtocolMode::None {
            let at = self.cell_at(event.position).unwrap_or((0, 0));
            self.write(&wheel_report(up, at, encoding).repeat(count));
        } else if alternate {
            let arrow: &[u8] = match (up, app) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.write(&arrow.repeat(count));
        } else {
            self.scroll_by(lines as isize, cx);
        }
    }

    /// How many lines back into the scrollback the view is (0: at the bottom).
    #[cfg(test)]
    pub fn scrolled_back(&self) -> usize {
        self.screen().screen().scrollback()
    }

    /// Output as if the shell wrote it (tests start no shell).
    #[cfg(test)]
    pub fn feed(&self, bytes: &[u8]) {
        self.screen().process(bytes);
    }

    /// Output as if the shell printed it, heard as the shell's is (the bell, the title, the
    /// selection kept or let go): the shots harness's `terminal feed` and tests.
    #[cfg(any(test, feature = "shots"))]
    pub fn print(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.screen().process(bytes);
        self.output_arrived(cx);
    }

    /// Every link on screen, top to bottom.
    #[cfg(any(test, feature = "shots"))]
    pub fn links(&self) -> Vec<Link> {
        let parser = self.screen();
        let screen = parser.screen();
        (0..screen.size().0)
            .flat_map(|row| row_links(screen, row))
            .collect()
    }

    /// Open the `n`th link on screen (from 1), as ⌘-clicking it does.
    #[cfg(any(test, feature = "shots"))]
    pub fn open_nth_link(
        &mut self,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<String> {
        let links = self.links();
        let Some(link) = n.checked_sub(1).and_then(|i| links.get(i)).cloned() else {
            anyhow::bail!("no link {n}: {} on screen", links.len());
        };
        self.open_link(link.url.clone(), window, cx);
        Ok(link.url)
    }

    /// What the terminal sends its shell from now on (keys, pastes, wheel reports), kept for the
    /// test to read: tests start no shell.
    #[cfg(test)]
    pub fn capture_input(&mut self) -> Arc<Mutex<Vec<u8>>> {
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let sent = Arc::new(Mutex::new(Vec::new()));
        self.writer = Some(Arc::new(Mutex::new(Box::new(Sink(sent.clone())))));
        sent
    }

    /// Just inside the left edge of the cell at `(row, col)`, in the window, as last drawn: the
    /// boundary before it, to a press.
    #[cfg(test)]
    pub fn cell_start(&self, row: u16, col: u16) -> Point<Pixels> {
        let grid = self.grid.get();
        point(
            grid.origin.x + px(col as f32 * self.cell_width() + 1.),
            grid.origin.y + px((row as f32 + 0.5) * self.line_height()),
        )
    }

    /// The middle of the cell at `(row, col)`, in the window, as last drawn.
    #[cfg(test)]
    pub fn cell_center(&self, row: u16, col: u16) -> Point<Pixels> {
        let grid = self.grid.get();
        point(
            grid.origin.x + px((col as f32 + 0.5) * self.cell_width()),
            grid.origin.y + px((row as f32 + 0.5) * self.line_height()),
        )
    }

    /// The text under the selection now, if there's one (what ⌘C copies is what it held when
    /// last looked at, which must be the same).
    #[cfg(test)]
    pub fn selected(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let (start, end) = sel.ordered();
        let now = selected_text(&mut self.screen(), start, end);
        assert_eq!(now, sel.text, "the selection moved off what it held");
        Some(now)
    }

    /// The link drawn underlined (⌘ held over it).
    #[cfg(test)]
    pub fn hovered_link(&self) -> Option<String> {
        self.hovered.as_ref().map(|l| l.url.clone())
    }

    #[cfg(test)]
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// The rows on view, scrolled back or not.
    #[cfg(test)]
    pub fn visible_rows(&self) -> Vec<String> {
        let parser = self.screen();
        let screen = parser.screen();
        screen.rows(0, screen.size().1).collect()
    }

    /// The last `n` lines with something on them, as the screen shows them ("Add to Chat").
    pub fn recent_output(&self, n: usize) -> String {
        let text = self.screen().screen().contents();
        let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
        let end = lines
            .iter()
            .rposition(|l| !l.is_empty())
            .map_or(0, |i| i + 1);
        lines[..end]
            .iter()
            .skip(end.saturating_sub(n))
            .copied()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (family, cell_width) = self.metrics(window, &theme.mono_font_family);
        let (font_size, line_height) = (self.font_size, self.line_height());
        let parser = self.parser.clone();
        let parser = parser.lock().unwrap_or_else(|e| e.into_inner());
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let scrollback = screen.scrollback();
        // Scrolled back, the cursor's row is that many further down the view (or off it).
        let (cur_row, cur_col) = screen.cursor_position();
        let cur_row = cur_row as usize + scrollback;
        let show_cursor = !screen.hide_cursor() && !self.exited;
        let selection = self.selection.as_ref().map(Selection::ordered);
        let mut lines = Vec::with_capacity(rows as usize);
        for r in 0..rows {
            let line = r as isize - scrollback as isize;
            let link = self
                .hovered
                .as_ref()
                .filter(|l| l.row == r)
                .map(|l| l.cols.clone());
            let mut text = String::new();
            let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
            for c in 0..cols {
                let Some(cell) = screen.cell(r, c) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let start = text.len();
                let s = cell.contents();
                text.push_str(if s.is_empty() { " " } else { s });
                let mut fg = color(cell.fgcolor());
                let mut bg = color(cell.bgcolor());
                if cell.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                    fg = fg.or(Some(theme.background));
                    bg = bg.or(Some(theme.foreground));
                }
                if show_cursor && r as usize == cur_row && c == cur_col {
                    bg = Some(theme.foreground.opacity(0.75));
                    fg = Some(theme.background);
                }
                if selection.is_some_and(|s| is_selected(s, line, c)) {
                    bg = Some(theme.selection);
                }
                let underline = cell.underline() || link.as_ref().is_some_and(|l| l.contains(&c));
                if fg.is_some() || bg.is_some() || cell.bold() || underline {
                    highlights.push((
                        start..text.len(),
                        HighlightStyle {
                            color: fg,
                            background_color: bg,
                            font_weight: cell.bold().then_some(FontWeight::BOLD),
                            underline: underline.then(|| UnderlineStyle {
                                thickness: px(1.),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ));
                }
            }
            let trimmed = text
                .trim_end()
                .len()
                .max(highlights.last().map(|h| h.0.end).unwrap_or(0));
            text.truncate(trimmed);
            highlights.retain(|h| h.0.end <= text.len());
            lines.push(
                div()
                    .h(px(line_height))
                    .whitespace_nowrap()
                    .child(StyledText::new(text).with_highlights(highlights)),
            );
        }
        drop(parser);
        let entity = cx.entity().downgrade();
        let grid = self.grid.clone();
        let rows_view = div()
            .relative()
            .size_full()
            .overflow_hidden()
            .child(
                canvas(
                    {
                        let entity = entity.clone();
                        move |bounds, _, cx| {
                            grid.set(bounds);
                            let rows = (bounds.size.height.as_f32() / line_height).floor() as u16;
                            let cols = (bounds.size.width.as_f32() / cell_width).floor() as u16;
                            let entity = entity.clone();
                            cx.defer(move |cx| {
                                let _ = entity.update(cx, |this, cx| {
                                    let before = this.size;
                                    this.resize(rows, cols);
                                    if before != this.size {
                                        cx.notify();
                                    }
                                });
                            });
                        }
                    },
                    {
                        let entity = entity.clone();
                        // A drag that started here goes on outside the rows, and ends wherever
                        // the button comes up.
                        move |_, _, window, _| {
                            let moved = entity.clone();
                            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble
                                    && e.pressed_button == Some(MouseButton::Left)
                                {
                                    let _ =
                                        moved.update(cx, |this, cx| this.drag_to(e.position, cx));
                                }
                            });
                            let released = entity.clone();
                            window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                                    let _ = released.update(cx, |this, _| this.press = None);
                                }
                            });
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .children(lines);
        // Scrolled back: how to get down again.
        let to_bottom = (scrollback > 0).then(|| {
            h_flex()
                .id("terminal-to-bottom")
                .test_support()
                .absolute()
                .right(px(14.))
                .bottom(px(10.))
                .h(px(24.))
                .px(px(10.))
                .gap(px(5.))
                .items_center()
                .rounded(px(12.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .font_family(theme.font_family.clone())
                .text_size(px(11.5))
                .text_color(theme.muted_foreground)
                .cursor_pointer()
                .hover(|s| s.text_color(theme.foreground))
                .child(Icon::new(IconName::ArrowDown).xsmall())
                .child(format!("{scrollback} more below"))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_scrollback(0);
                    this.focus.focus(window, cx);
                    cx.notify();
                }))
        });
        let me = cx.weak_entity();
        div()
            .id("terminal")
            .test_support()
            .key_context("Terminal")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .px_3()
            .py_2()
            .bg(theme.background)
            .font_family(family)
            .text_size(px(font_size))
            .line_height(px(line_height))
            .text_color(theme.foreground)
            .overflow_hidden()
            .cursor(if self.hovered.is_some() {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                this.hover(e.position, e.modifiers.secondary(), cx)
            }))
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, window, cx| {
                this.hover(window.mouse_position(), e.modifiers.secondary(), cx)
            }))
            .on_hover(cx.listener(|this, over: &bool, _, cx| {
                if !*over {
                    this.pointer = None;
                    if this.hovered.take().is_some() {
                        cx.notify();
                    }
                }
            }))
            .on_key_down(cx.listener(Self::key))
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| this.wheel(e, cx)))
            .child(rows_view)
            .children(to_bottom)
            .when(self.bell_at.is_some(), |el| {
                el.child(
                    div()
                        .id("terminal-bell")
                        .test_support()
                        .absolute()
                        .inset_0()
                        .bg(theme.foreground.opacity(0.06)),
                )
            })
            .context_menu(move |menu, _, cx| {
                let selected = me.upgrade().is_some_and(|t| t.read(cx).selection.is_some());
                let (copy, paste) = (me.clone(), me.clone());
                menu.min_w(px(160.))
                    .item(PopupMenuItem::new("Copy").disabled(!selected).on_click(
                        move |_, _, cx| {
                            let _ = copy.update(cx, |t, cx| t.copy(cx));
                        },
                    ))
                    .item(PopupMenuItem::new("Paste").on_click(move |_, _, cx| {
                        let _ = paste.update(cx, |t, cx| t.paste(cx));
                    }))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Flavor, Job, Screen, Shortcut, choose_shell, cursor_queries, cursor_report, flavor,
        home_end, new_screen, paste_bytes, plain_path, powershell_quote, pump, shell_args,
        shell_cwd, shell_for, shortcut, wheel_report,
    };
    use gpui_kit::Modifiers;
    use std::path::{Path, PathBuf};

    fn found(names: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |n| {
            names
                .contains(&n)
                .then(|| PathBuf::from(format!(r"C:\bin\{n}.exe")))
        }
    }

    #[test]
    fn windows_picks_pwsh_then_windows_powershell_then_comspec() {
        let pick = |names, comspec| choose_shell("", None, true, found(names), comspec);
        assert_eq!(
            pick(
                &["pwsh", "powershell"],
                Some(r"C:\Windows\System32\cmd.exe")
            ),
            r"C:\bin\pwsh.exe"
        );
        assert_eq!(
            pick(&["powershell"], Some(r"C:\Windows\System32\cmd.exe")),
            r"C:\bin\powershell.exe"
        );
        assert_eq!(
            pick(&[], Some(r"C:\Windows\System32\cmd.exe")),
            r"C:\Windows\System32\cmd.exe"
        );
        assert_eq!(
            pick(&[], Some("")),
            "cmd.exe",
            "an empty COMSPEC is no COMSPEC"
        );
        assert_eq!(pick(&[], None), "cmd.exe");
    }

    #[test]
    fn a_mac_keeps_its_login_shell_and_the_setting_overrides_either() {
        assert_eq!(
            choose_shell(
                "",
                Some("/opt/homebrew/bin/fish"),
                false,
                found(&["pwsh"]),
                None
            ),
            "/opt/homebrew/bin/fish"
        );
        assert_eq!(choose_shell("", None, false, found(&[]), None), "/bin/zsh");
        // A name is looked up; a path, quoted or not, is taken as it is; a name nothing has stays a name.
        assert_eq!(
            choose_shell("pwsh", Some("/bin/zsh"), false, found(&["pwsh"]), None),
            r"C:\bin\pwsh.exe"
        );
        assert_eq!(
            choose_shell(r#" "D:\tools\nu.exe" "#, None, true, found(&["pwsh"]), None),
            r"D:\tools\nu.exe"
        );
        assert_eq!(choose_shell("nu", None, true, found(&[]), None), "nu");
    }

    #[test]
    fn the_shell_kind_comes_from_its_name_on_windows_only() {
        assert_eq!(
            flavor(r"C:\Program Files\PowerShell\7\pwsh.exe", true),
            Flavor::PowerShell
        );
        assert_eq!(flavor("PowerShell.EXE", true), Flavor::PowerShell);
        assert_eq!(flavor(r"C:\Windows\System32\cmd.exe", true), Flavor::Cmd);
        assert_eq!(
            flavor(r"C:\Program Files\Git\bin\bash.exe", true),
            Flavor::Posix
        );
        assert_eq!(flavor("/usr/local/bin/pwsh", false), Flavor::Posix);
    }

    #[test]
    fn each_kind_of_shell_gets_its_own_arguments() {
        // macOS: exactly what it always was.
        assert_eq!(shell_args(Flavor::Posix, None), ["-l"]);
        assert_eq!(
            shell_args(Flavor::Posix, Some("echo 'hi'")),
            [
                "-l",
                "-c",
                r"printf '\033[1m$ %s\033[0m\n\n' 'echo '\''hi'\'''; echo 'hi'; code=$?; printf '\n\033[2m[finished with exit code %s]\033[0m\n' $code; exit $code"
            ]
        );
        // PowerShell has no login flag.
        assert_eq!(shell_args(Flavor::PowerShell, None), ["-NoLogo"]);
        let ps = shell_args(Flavor::PowerShell, Some("winget install it's"));
        assert_eq!((ps[0].as_str(), ps[1].as_str()), ("-NoLogo", "-Command"));
        assert!(
            ps[2].starts_with("Write-Host '$ winget install it''s'; "),
            "{}",
            ps[2]
        );
        assert!(
            ps[2].contains("; winget install it's; $ok = $?;"),
            "{}",
            ps[2]
        );
        assert!(shell_args(Flavor::Cmd, None).is_empty());
        assert_eq!(
            shell_args(Flavor::Cmd, Some("dir")),
            [
                "/c",
                "dir & echo. & call echo [finished with exit code %^errorlevel%]"
            ],
            "one `call`: an earlier one resets the exit code"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dropping_a_terminal_ends_its_shell_and_the_thread_reading_it() {
        use super::{CommandBuilder, Duration, Session, start_session};
        let screen: Screen = new_screen(30, 100, 0);
        let (tx, _rx) = async_channel::bounded::<()>(1);
        // An idle shell that prints nothing: nothing but a hangup can end it.
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "exec sleep 600"]);
        let session = start_session((30, 100), cmd, screen, tx).unwrap();
        let Session {
            writer,
            master,
            hangup,
            thread,
        } = session;
        // What the panel keeps and what it hands the thread; dropping them is closing the tab.
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(thread.join().is_ok());
        });
        assert!(
            done_rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "the thread ended with the shell still running"
        );
        drop((writer, master, hangup));
        assert_eq!(
            done_rx.recv_timeout(Duration::from_secs(10)),
            Ok(true),
            "the shell and the thread outlived the terminal"
        );
    }

    #[test]
    fn powershell_quoting_doubles_every_kind_of_single_quote() {
        assert_eq!(powershell_quote("plain"), "'plain'");
        assert_eq!(powershell_quote("it's"), "'it''s'");
        assert_eq!(
            powershell_quote("a $b `c \"d\""),
            "'a $b `c \"d\"'",
            "nothing else is special in single quotes"
        );
        assert_eq!(
            powershell_quote("\u{2018}x\u{2019}"),
            "'\u{2018}\u{2018}x\u{2019}\u{2019}'"
        );
    }

    #[test]
    fn the_shell_starts_in_a_plain_folder() {
        let home = PathBuf::from(r"C:\Users\me");
        assert_eq!(
            plain_path(PathBuf::from(r"\\?\C:\code\app")),
            PathBuf::from(r"C:\code\app")
        );
        assert_eq!(
            plain_path(PathBuf::from(r"\\?\UNC\nas\share\app")),
            PathBuf::from(r"\\nas\share\app")
        );
        assert_eq!(
            plain_path(PathBuf::from(r"\\?\Volume{1234}\x")),
            PathBuf::from(r"\\?\Volume{1234}\x"),
            "only a drive or share path has a plain form"
        );
        assert_eq!(
            plain_path(PathBuf::from(r"C:\code")),
            PathBuf::from(r"C:\code")
        );
        let all = |_: &Path| true;
        let verbatim = Some(PathBuf::from(r"\\?\C:\code\app"));
        assert_eq!(
            shell_cwd(
                verbatim.clone(),
                Flavor::PowerShell,
                true,
                home.clone(),
                all
            ),
            Some(PathBuf::from(r"C:\code\app"))
        );
        assert_eq!(
            shell_cwd(None, Flavor::Cmd, true, home.clone(), all),
            Some(home.clone())
        );
        // A network folder is fine for PowerShell, and not for cmd.exe.
        let unc = Some(PathBuf::from(r"\\nas\share\app"));
        assert_eq!(
            shell_cwd(unc.clone(), Flavor::PowerShell, true, home.clone(), all),
            Some(PathBuf::from(r"\\nas\share\app"))
        );
        assert_eq!(
            shell_cwd(
                Some(PathBuf::from(r"\\?\UNC\nas\share\app")),
                Flavor::Cmd,
                true,
                home.clone(),
                all
            ),
            Some(home.clone())
        );
        // Off Windows nothing is rewritten.
        assert_eq!(
            shell_cwd(verbatim.clone(), Flavor::Posix, false, home.clone(), all),
            verbatim
        );
    }

    #[test]
    fn a_folder_that_is_gone_starts_the_shell_in_home_and_without_home_where_trek_is() {
        let home = PathBuf::from(r"C:\Users\me");
        let only_home = |p: &Path| p == Path::new(r"C:\Users\me");
        assert_eq!(
            shell_cwd(
                Some(PathBuf::from(r"C:\gone")),
                Flavor::PowerShell,
                true,
                home.clone(),
                only_home
            ),
            Some(home.clone())
        );
        // cmd.exe and a network folder: home too, and if that is gone, nothing (no panic, no error).
        assert_eq!(
            shell_cwd(
                Some(PathBuf::from(r"\\nas\share")),
                Flavor::Cmd,
                true,
                home.clone(),
                only_home
            ),
            Some(home.clone())
        );
        assert_eq!(
            shell_cwd(
                Some(PathBuf::from(r"C:\gone")),
                Flavor::Cmd,
                true,
                home.clone(),
                |_: &Path| false
            ),
            None
        );
        assert_eq!(
            shell_cwd(None, Flavor::Posix, false, home, |_: &Path| false),
            None
        );
    }

    #[test]
    fn treks_own_commands_run_in_powershell_whatever_shell_the_user_chose() {
        let find = found(&["pwsh", "powershell", "bash", "nu"]);
        let comspec = Some(r"C:\Windows\System32\cmd.exe");
        let pick = |setting, job: Option<&Job>| shell_for(setting, job, None, true, &find, comspec);
        let setup = Job::Setup("irm https://example.com/install.ps1 | iex".into());
        let action = Job::Action("./build.sh && make".into());
        // The setting picks the interactive shell and the user's own commands' shell.
        assert_eq!(pick("nu", None), r"C:\bin\nu.exe");
        assert_eq!(pick("bash", Some(&action)), r"C:\bin\bash.exe");
        // Install / Sign in are PowerShell one-liners on Windows: Git Bash, nu or cmd would choke on them.
        assert_eq!(pick("bash", Some(&setup)), r"C:\bin\pwsh.exe");
        assert_eq!(
            pick(r"C:\Windows\System32\cmd.exe", Some(&setup)),
            r"C:\bin\pwsh.exe"
        );
        assert_eq!(
            shell_for(
                "nu",
                Some(&setup),
                None,
                true,
                found(&["powershell"]),
                comspec
            ),
            r"C:\bin\powershell.exe"
        );
        // A Mac has one kind of shell: its own command runs in the one it chose, else `$SHELL`.
        assert_eq!(
            shell_for("", Some(&setup), Some("/bin/zsh"), false, found(&[]), None),
            "/bin/zsh"
        );
        assert_eq!(
            shell_for(
                "fish",
                Some(&action),
                Some("/bin/zsh"),
                false,
                found(&[]),
                None
            ),
            "fish"
        );
    }

    #[test]
    fn paste_keys_leave_ctrl_v_to_the_shell() {
        let keys = |control, shift, platform| Modifiers {
            control,
            shift,
            platform,
            ..Default::default()
        };
        // Windows: Ctrl+Shift+V and Shift+Insert paste; Ctrl+V is the shell's, and so are Ctrl+C and the Win key's.
        assert_eq!(
            shortcut("v", &keys(true, true, false), true),
            Some(Shortcut::Paste)
        );
        assert_eq!(
            shortcut("insert", &keys(false, true, false), true),
            Some(Shortcut::Paste)
        );
        assert_eq!(shortcut("v", &keys(true, false, false), true), None);
        assert_eq!(
            shortcut("c", &keys(true, false, false), true),
            None,
            "Ctrl+C interrupts"
        );
        assert_eq!(
            shortcut("c", &keys(true, true, false), true),
            Some(Shortcut::Copy),
            "the selection, and never an interrupt"
        );
        assert_eq!(
            shortcut("v", &keys(false, false, true), true),
            Some(Shortcut::PassThrough)
        );
        assert_eq!(shortcut("a", &keys(false, false, false), true), None);
        // macOS: Cmd+V pastes, Cmd+K clears, the rest of Cmd is the app's, Ctrl is the shell's.
        assert_eq!(
            shortcut("v", &keys(false, false, true), false),
            Some(Shortcut::Paste)
        );
        assert_eq!(
            shortcut("k", &keys(false, false, true), false),
            Some(Shortcut::Clear)
        );
        assert_eq!(
            shortcut("t", &keys(false, false, true), false),
            Some(Shortcut::PassThrough)
        );
        assert_eq!(shortcut("v", &keys(true, false, false), false), None);
    }

    #[test]
    fn a_macs_command_keys_copy_move_along_the_line_and_size_the_text() {
        let cmd = Modifiers {
            platform: true,
            ..Default::default()
        };
        let cmd_shift = Modifiers {
            platform: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(shortcut("c", &cmd, false), Some(Shortcut::Copy));
        assert_eq!(shortcut("left", &cmd, false), Some(Shortcut::LineStart));
        assert_eq!(shortcut("right", &cmd, false), Some(Shortcut::LineEnd));
        assert_eq!(
            shortcut("backspace", &cmd, false),
            Some(Shortcut::DeleteLine)
        );
        assert_eq!(
            shortcut("left", &cmd_shift, false),
            Some(Shortcut::PassThrough),
            "⌘⇧← isn't the line's start"
        );
        assert_eq!(shortcut("=", &cmd, false), Some(Shortcut::Bigger));
        assert_eq!(shortcut("+", &cmd_shift, false), Some(Shortcut::Bigger));
        assert_eq!(shortcut("-", &cmd, false), Some(Shortcut::Smaller));
        assert_eq!(shortcut("0", &cmd, false), Some(Shortcut::ActualSize));
        // Windows: Ctrl+= / - / 0, as Windows Terminal has them; ⌥ keys stay the shell's.
        let ctrl = Modifiers {
            control: true,
            ..Default::default()
        };
        assert_eq!(shortcut("=", &ctrl, true), Some(Shortcut::Bigger));
        assert_eq!(shortcut("0", &ctrl, true), Some(Shortcut::ActualSize));
        assert_eq!(
            shortcut("left", &ctrl, true),
            None,
            "the shell's word movement"
        );
        assert_eq!(home_end(false, false), b"\x1b[H");
        assert_eq!(home_end(true, true), b"\x1bOF");
    }

    #[test]
    fn a_wheel_notch_is_reported_as_the_program_asked() {
        use vt100::MouseProtocolEncoding::{Default, Sgr, Utf8};
        assert_eq!(wheel_report(true, (4, 9), Sgr), b"\x1b[<64;10;5M");
        assert_eq!(wheel_report(false, (0, 0), Sgr), b"\x1b[<65;1;1M");
        assert_eq!(
            wheel_report(true, (4, 9), Default),
            [0x1b, b'[', b'M', 96, 42, 37]
        );
        // Far out, X10 stops at 255; UTF-8 goes on.
        assert_eq!(wheel_report(false, (0, 300), Default)[4], 255);
        assert_eq!(
            wheel_report(false, (0, 300), Utf8),
            [
                b"\x1b[M".as_slice(),
                "a".as_bytes(),
                "\u{14d}".as_bytes(),
                "!".as_bytes()
            ]
            .concat()
        );
    }

    fn parser(rows: u16, cols: u16, text: &str) -> super::Parser {
        let mut p = super::Parser::new_with_callbacks(rows, cols, 100, super::Events::default());
        p.process(text.as_bytes());
        p
    }

    #[test]
    fn a_selection_reads_across_rows_as_lines() {
        use super::{Pos, selected_text};
        let at = |line, col| Pos { line, col };
        let mut p = parser(5, 20, "first line\r\nsecond   \r\nthird one\r\n");
        // From the middle of one row to the middle of another: the rows between whole, blanks at a row's end dropped.
        assert_eq!(
            selected_text(&mut p, at(0, 6), at(2, 5)),
            "line\nsecond\nthird"
        );
        // Within a row.
        assert_eq!(selected_text(&mut p, at(1, 0), at(1, 3)), "sec");
        // A long line that ran on to the next row reads as one line.
        let mut p = parser(5, 10, "0123456789abcdef\r\nnext\r\n");
        assert_eq!(
            selected_text(&mut p, at(0, 0), at(2, 4)),
            "0123456789abcdef\nnext"
        );
        // Lines scrolled off the screen are still there, before the screen's: negative lines.
        let lines: String = (1..=8).map(|i| format!("line {i}\r\n")).collect();
        let mut p = parser(4, 20, &lines);
        assert_eq!(p.screen().contents().lines().next(), Some("line 6"));
        assert_eq!(
            selected_text(&mut p, at(-2, 0), at(1, 6)),
            "line 4\nline 5\nline 6\nline 7"
        );
        assert_eq!(p.screen().scrollback(), 0, "the view is left where it was");
    }

    #[test]
    fn links_and_words_are_found_by_cell() {
        use super::{row_links, word_bounds};
        // A wide character before the link: columns, not bytes or characters.
        let p = parser(3, 60, "界 Local: http://localhost:5173/ (ready)\r\n");
        let links = row_links(p.screen(), 0);
        assert_eq!(links.len(), 1);
        assert_eq!(
            (links[0].cols.clone(), links[0].url.as_str()),
            (10..32, "http://localhost:5173/")
        );
        // A double-click's word: the address whole, not the bracket after it.
        assert_eq!(word_bounds(p.screen(), 0, 15), (10, 32));
        assert_eq!(word_bounds(p.screen(), 0, 34), (34, 39));
        assert_eq!(
            word_bounds(p.screen(), 0, 9),
            (9, 10),
            "a blank is a word of its own"
        );
    }

    #[test]
    fn pasted_text_is_bracketed_when_asked_and_enters_as_cr_on_windows() {
        assert_eq!(
            paste_bytes("a\nb", false, false),
            "a\nb",
            "a Mac is untouched"
        );
        assert_eq!(paste_bytes("a\r\nb\nc", false, true), "a\rb\rc");
        assert_eq!(paste_bytes("ls", true, true), "\x1b[200~ls\x1b[201~");
    }

    #[test]
    fn a_flood_of_output_is_parsed_off_the_main_thread_with_one_wake_up() {
        // 8 MB of `yes`, with nobody draining the wake-ups: the reader never blocks on the view,
        // nothing queues up but the one wake-up, and the screen holds the latest output.
        let flood = "y\r\n".repeat(2 << 20) + "done\r\n";
        let screen: Screen = new_screen(24, 80, 2000);
        let (tx, rx) = async_channel::bounded::<()>(1);
        pump(
            std::io::Cursor::new(flood.into_bytes()),
            &screen,
            &tx,
            |_| panic!("nothing asked for a reply"),
        );
        assert_eq!(rx.len(), 1, "one wake-up stands for every read");
        let text = screen.lock().unwrap().screen().contents();
        assert!(text.trim_end().ends_with("done"), "{text}");
        // Scrollback stays at its cap however much went by.
        let mut parser = screen.lock().unwrap();
        parser.screen_mut().set_scrollback(usize::MAX);
        assert_eq!(parser.screen().scrollback(), 2000);
    }

    #[test]
    fn the_reader_stops_once_the_view_is_gone() {
        let screen: Screen = new_screen(24, 80, 0);
        let (tx, rx) = async_channel::bounded::<()>(1);
        drop(rx);
        // An endless stream: `pump` returns because nothing listens any more.
        pump(std::io::repeat(b'y'), &screen, &tx, |_| {
            panic!("nothing asked for a reply")
        });
    }

    #[test]
    fn a_shell_that_asks_where_the_cursor_is_is_told() {
        assert_eq!(cursor_report((0, 0)), "\x1b[1;1R");
        assert_eq!(cursor_report((4, 11)), "\x1b[5;12R");
        assert_eq!(cursor_queries(b"", b"\x1b[6n"), 1);
        assert_eq!(cursor_queries(b"", b"hello\x1b[6nworld\x1b[6n"), 2);
        assert_eq!(cursor_queries(b"", b"\x1b[6"), 0);
        assert_eq!(
            cursor_queries(b"\x1b[6", b"n"),
            1,
            "a query split across two reads"
        );
        assert_eq!(cursor_queries(b"\x1b[6", b"ok"), 0);
        // ConPTY's first output is the query alone; the prompt follows only once it is answered.
        let screen: Screen = new_screen(24, 80, 0);
        let (tx, _rx) = async_channel::bounded::<()>(1);
        let said = std::cell::RefCell::new(Vec::<u8>::new());
        let stream = std::io::Cursor::new(b"ab\r\n\x1b[6n".to_vec());
        pump(stream, &screen, &tx, |bytes| {
            said.borrow_mut().extend_from_slice(bytes)
        });
        assert_eq!(said.into_inner(), b"\x1b[2;1R");
        // One byte per read, which is what a slow pipe can do: the query is still seen once.
        struct ByteAtATime(std::io::Cursor<Vec<u8>>);
        impl std::io::Read for ByteAtATime {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let one = buf.len().min(1);
                self.0.read(&mut buf[..one])
            }
        }
        let screen: Screen = new_screen(24, 80, 0);
        let said = std::cell::RefCell::new(Vec::<u8>::new());
        pump(
            ByteAtATime(std::io::Cursor::new(b"\x1b[6n".to_vec())),
            &screen,
            &tx,
            |bytes| said.borrow_mut().extend_from_slice(bytes),
        );
        assert_eq!(said.into_inner(), b"\x1b[1;1R");
    }

    /// The shell, in a real pseudo-terminal (ConPTY), as the panel starts it.
    #[cfg(windows)]
    mod conpty {
        use super::super::{Job, Screen, new_screen, pump, shell_command, write_to};
        use portable_pty::{PtySize, native_pty_system};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        /// How long a shell gets for each step: generous, as profiles are slow and the machine
        /// may be building something else.
        const LIMIT: Duration = Duration::from_secs(180);

        /// Starts `setting`'s shell running `job` (if any) in a temp folder, types `input`
        /// (if any), and returns the screen once `done` likes it, or after `LIMIT`. The shell is
        /// killed afterwards. One shell at a time: six PowerShells starting at once, each running
        /// the user's profile, took longer than `LIMIT` on a loaded machine.
        fn run(
            setting: &str,
            job: Option<Job>,
            input: Option<&str>,
            done: impl Fn(&str) -> bool,
        ) -> String {
            static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
            let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
            // A folder of its own, as each test removes its folder.
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "trek-terminal-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let pty = native_pty_system()
                .openpty(PtySize {
                    rows: 30,
                    cols: 100,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .unwrap();
            let mut child = pty
                .slave
                .spawn_command(shell_command(setting, Some(dir.clone()), job.as_ref()))
                .unwrap();
            let reader = pty.master.try_clone_reader().unwrap();
            // As the panel runs it: the output thread answers ConPTY's cursor query on the same writer.
            let writer = Arc::new(Mutex::new(pty.master.take_writer().unwrap()));
            let screen: Screen = new_screen(30, 100, 0);
            let (tx, _rx) = async_channel::bounded::<()>(1);
            let pumped = screen.clone();
            let answering = writer.clone();
            std::thread::spawn(move || {
                pump(reader, &pumped, &tx, |bytes| write_to(&answering, bytes))
            });
            if let Some(input) = input {
                // The shell reads keys once it has started (a profile can take seconds to run, and
                // five shells start at once here): wait for output, then for it to settle at its prompt.
                let start = Instant::now();
                let (mut last, mut changed) = (String::new(), Instant::now());
                while start.elapsed() < LIMIT {
                    let now = screen.lock().unwrap().screen().contents();
                    if now != last {
                        (last, changed) = (now, Instant::now());
                    } else if !last.trim().is_empty()
                        && changed.elapsed() > Duration::from_millis(2000)
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                write_to(&writer, input.as_bytes());
            }
            let start = Instant::now();
            let mut text = String::new();
            while start.elapsed() < LIMIT {
                text = screen.lock().unwrap().screen().contents();
                if done(&text) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = child.kill();
            let _ = child.wait();
            drop(pty.master);
            let _ = std::fs::remove_dir_all(&dir);
            text
        }

        fn has_line(text: &str, line: &str) -> bool {
            text.lines().any(|l| l.trim() == line)
        }

        #[test]
        fn the_chosen_shell_runs_what_is_typed_into_it() {
            // The typed line is on the screen too ("PS C:\> echo trek-ok"): the output is a line of its own.
            let text = run("", None, Some("echo trek-ok\r"), |t| has_line(t, "trek-ok"));
            assert!(
                has_line(&text, "trek-ok"),
                "no output from the shell:\n{text}"
            );
        }

        #[test]
        fn the_shell_starts_in_the_folder_it_was_given() {
            let text = run("", None, Some("echo trek-ok\r"), |t| has_line(t, "trek-ok"));
            assert!(
                text.contains("trek-terminal-test-"),
                "no prompt in the folder:\n{text}"
            );
        }

        #[test]
        fn a_command_runs_in_each_shell_and_ends_with_its_exit_code() {
            for (name, command, shows) in [
                ("pwsh", "Write-Output 'it''s trek'", "it's trek"),
                ("powershell", "Write-Output 'it''s trek'", "it's trek"),
                ("cmd", "echo it's trek", "it's trek"),
            ] {
                if trek_core::detect::which(name).is_none() {
                    eprintln!("{name} isn't installed here; skipped");
                    continue;
                }
                let text = run(name, Some(Job::Action(command.into())), None, |t| {
                    t.contains("[finished with exit code")
                });
                assert!(
                    has_line(&text, shows),
                    "{name}: the command's output is missing:\n{text}"
                );
                assert!(
                    text.contains("[finished with exit code 0]"),
                    "{name}: no exit code:\n{text}"
                );
            }
        }

        #[test]
        fn a_failing_native_command_reports_its_code_in_each_shell() {
            for name in ["pwsh", "powershell", "cmd"] {
                if trek_core::detect::which(name).is_none() {
                    eprintln!("{name} isn't installed here; skipped");
                    continue;
                }
                let text = run(name, Some(Job::Action("cmd /c exit 3".into())), None, |t| {
                    t.contains("[finished with exit code")
                });
                assert!(
                    text.contains("[finished with exit code 3]"),
                    "{name}:\n{text}"
                );
            }
        }

        #[test]
        fn a_command_that_runs_no_program_reports_success_or_failure_in_powershell() {
            // `$LASTEXITCODE` is empty when no program ran: a failed cmdlet is 1, a good one 0.
            for name in ["pwsh", "powershell"] {
                if trek_core::detect::which(name).is_none() {
                    eprintln!("{name} isn't installed here; skipped");
                    continue;
                }
                let fails = run(
                    name,
                    Some(Job::Action("Get-Item .\\no-such-file".into())),
                    None,
                    |t| t.contains("[finished with exit code"),
                );
                assert!(
                    fails.contains("[finished with exit code 1]"),
                    "{name}:\n{fails}"
                );
            }
        }

        #[test]
        fn treks_own_command_runs_in_powershell_when_the_chosen_shell_is_git_bash() {
            let bash = r"C:\Program Files\Git\bin\bash.exe";
            if !std::path::Path::new(bash).exists()
                || trek_core::detect::which("pwsh")
                    .or_else(|| trek_core::detect::which("powershell"))
                    .is_none()
            {
                eprintln!("Git Bash or PowerShell isn't installed here; skipped");
                return;
            }
            // `Write-Output` is a cmdlet: bash would say "command not found".
            let text = run(
                bash,
                Some(Job::Setup("Write-Output 'ran in powershell'".into())),
                None,
                |t| t.contains("[finished with exit code"),
            );
            assert!(
                has_line(&text, "ran in powershell")
                    && text.contains("[finished with exit code 0]"),
                "{text}"
            );
            // The user's own command goes to the shell they chose.
            let text = run(
                bash,
                Some(Job::Action("echo ran in bash".into())),
                None,
                |t| t.contains("[finished with exit code"),
            );
            assert!(
                has_line(&text, "ran in bash") && text.contains("[finished with exit code 0]"),
                "{text}"
            );
        }
    }
}
