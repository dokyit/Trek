//! A real terminal: the user's login shell in a PTY, parsed by vt100, painted as styled rows.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

const FONT_SIZE: f32 = 12.5;
const LINE_HEIGHT: f32 = 18.;

/// Prefer an installed Nerd Font so prompt glyphs (powerline, icons) render.
const PREFERRED: &[&str] = &["FiraCode Nerd Font Mono", "JetBrainsMono Nerd Font Mono", "MesloLGS NF", "MesloLGM Nerd Font Mono", "Hack Nerd Font Mono", "SauceCodePro Nerd Font Mono"];

fn terminal_font(window: &Window, fallback: SharedString) -> SharedString {
    let names = window.text_system().all_font_names();
    PREFERRED.iter().find(|p| names.iter().any(|n| n == *p)).map(|p| SharedString::from(*p)).unwrap_or(fallback)
}

/// The screen, shared with the thread that reads the shell's output: output is parsed there, so a
/// flood of it (`yes`, `cat` of a big file) never queues up for or stalls the main thread. The
/// view only hears that the screen changed, at most once per frame.
type Screen = Arc<Mutex<vt100::Parser>>;

/// Repaints at most this often while output streams in.
const FRAME: Duration = Duration::from_millis(16);

/// Feed the shell's output into `screen` until it ends, waking the view after each read. `wake`
/// holds one wake-up at most: while one is waiting, more output only changes the screen.
fn pump(mut reader: impl Read, screen: &Screen, wake: &async_channel::Sender<()>) {
    let mut buf = [0u8; 16384];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                screen.lock().unwrap_or_else(|e| e.into_inner()).process(&buf[..n]);
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
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program).to_ascii_lowercase();
    match name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem) {
        "pwsh" | "powershell" => Flavor::PowerShell,
        "cmd" => Flavor::Cmd,
        _ => Flavor::Posix,
    }
}

/// The shell to start. `setting` is `[terminal] shell`: a path is used as written, a bare name is
/// looked up with `find`. Empty, it's `$SHELL` (zsh without one) on a Mac; on Windows PowerShell 7,
/// else Windows PowerShell, else `%COMSPEC%` (cmd.exe).
fn choose_shell(setting: &str, env_shell: Option<&str>, windows: bool, find: impl Fn(&str) -> Option<PathBuf>, comspec: Option<&str>) -> String {
    let setting = setting.trim().trim_matches('"');
    if !setting.is_empty() {
        if setting.contains(['/', '\\']) {
            return setting.to_string();
        }
        return find(setting).map_or_else(|| setting.to_string(), |p| p.to_string_lossy().into_owned());
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
                format!("printf '\\033[1m$ %s\\033[0m\\n\\n' {quoted}; {c}; code=$?; printf '\\n\\033[2m[finished with exit code %s]\\033[0m\\n' $code; exit $code"),
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
        // `%errorlevel%` to when `call` reaches it, after the command has set it.
        (Flavor::Cmd, Some(c)) => vec!["/c".into(), format!("{c} & call echo. & call echo [finished with exit code %^errorlevel%]")],
    }
}

/// `path` without the `\\?\` that canonicalising puts on a Windows path: `\\?\C:\dir` is `C:\dir`
/// and `\\?\UNC\host\share` is `\\host\share`. Shells show the prefix and `cmd.exe` mishandles it.
fn plain_path(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else { return path };
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(drive) if drive.as_bytes().get(1) == Some(&b':') => PathBuf::from(drive),
        _ => path,
    }
}

/// Where the shell starts: `cwd` (the project) or home. On Windows, as a plain path; and not a
/// network one for `cmd.exe`, which can't be in one and would start in the Windows folder.
fn shell_cwd(cwd: Option<PathBuf>, flavor: Flavor, windows: bool, home: PathBuf) -> PathBuf {
    let cwd = cwd.unwrap_or_else(|| home.clone());
    if !windows {
        return cwd;
    }
    let cwd = plain_path(cwd);
    if flavor == Flavor::Cmd && cwd.to_str().is_some_and(|p| p.starts_with(r"\\")) { home } else { cwd }
}

/// The shell for `[terminal] shell = setting` as a process to start in a pty, in `cwd`, running
/// `command` when there is one.
fn shell_command(setting: &str, cwd: Option<PathBuf>, command: Option<&str>) -> CommandBuilder {
    let windows = cfg!(windows);
    let shell = choose_shell(setting, std::env::var("SHELL").ok().as_deref(), windows, |name| trek_core::detect::which(name), std::env::var("COMSPEC").ok().as_deref());
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
    cmd.cwd(shell_cwd(cwd, flavor, windows, trek_core::paths::home()));
    cmd
}

/// What a key press does in the terminal besides going to the shell.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shortcut {
    Paste,
    /// Clear the screen (Ctrl+L, which the shell understands).
    Clear,
    /// Not the terminal's: the app's shortcuts get it.
    PassThrough,
}

/// The terminal's shortcuts. On a Mac they're Cmd+V and Cmd+K, and every other Cmd key is the
/// app's. Elsewhere `secondary` (Ctrl) is what the shell's own keys use (Ctrl+C is interrupt,
/// Ctrl+V is PowerShell's paste, which reads the clipboard itself), so the terminal doesn't take
/// it as the app's: pasting is Ctrl+Shift+V or Shift+Insert, as in Windows Terminal. There's no
/// Ctrl+Shift+C: the panel has no selection to copy, and it mustn't turn into Ctrl+C.
fn shortcut(key: &str, m: &Modifiers, windows: bool) -> Option<Shortcut> {
    if !windows {
        return m.platform.then(|| match key {
            "v" => Shortcut::Paste,
            "k" => Shortcut::Clear,
            _ => Shortcut::PassThrough,
        });
    }
    match key {
        _ if m.platform => Some(Shortcut::PassThrough),
        "v" if m.control && m.shift && !m.alt => Some(Shortcut::Paste),
        "insert" if m.shift && !m.control && !m.alt => Some(Shortcut::Paste),
        "c" if m.control && m.shift && !m.alt => Some(Shortcut::PassThrough),
        _ => None,
    }
}

/// What to write to the shell for pasted `text`: wrapped when it asked for bracketed paste, and
/// on Windows with line ends as Enter (CR) like a typed line, not the clipboard's CRLF.
fn paste_bytes(text: &str, bracketed: bool, windows: bool) -> String {
    let text = if windows { text.replace("\r\n", "\r").replace('\n', "\r") } else { text.to_string() };
    if bracketed { format!("\x1b[200~{text}\x1b[201~") } else { text }
}

pub struct TerminalPanel {
    parser: Screen,
    writer: Option<Box<dyn Write + Send>>,
    master: Option<Box<dyn MasterPty + Send>>,
    size: (u16, u16),
    focus: FocusHandle,
    exited: bool,
    /// Resolved once: font family and its cell width.
    font: Option<(SharedString, f32)>,
    /// Runs once when the shell exits (used by Install / Sign in to rescan agents).
    on_exit: Option<Box<dyn FnOnce(&mut App)>>,
    _reader: Option<Task<()>>,
}

fn ansi(idx: u8) -> Hsla {
    const BASE: [u32; 16] = [
        0x1A1A1D, 0xFF5A5F, 0x3FCF8E, 0xFFB020, 0x5AA9FF, 0xB48CFF, 0x4FD1D9, 0xD7DAE0, 0x5C6370, 0xFF7B80, 0x6BE3A8, 0xFFC857, 0x7FBDFF, 0xC9A8FF,
        0x7FE0E6, 0xFFFFFF,
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
        vt100::Color::Rgb(r, g, b) => Some(rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into()),
    }
}

impl TerminalPanel {
    pub fn new(cwd: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        Self::with_command(cwd, None, cx)
    }

    /// A terminal that runs one command in a login shell (agent install / sign-in), shows its output
    /// and stays open after it exits.
    pub fn with_command(cwd: Option<PathBuf>, command: Option<String>, cx: &mut Context<Self>) -> Self {
        let size = (30u16, 90u16);
        let mut this = Self {
            parser: Arc::new(Mutex::new(vt100::Parser::new(size.0, size.1, 2000))),
            writer: None,
            master: None,
            size,
            focus: cx.focus_handle(),
            exited: false,
            font: None,
            on_exit: None,
            _reader: None,
        };
        // An isolated (test) process starts no shell: one would run the user's login profile,
        // and the command (a project's tests, an agent's sign-in) for real.
        if trek_core::paths::isolated() {
            this.screen().process(format!("$ {}\r\n(no shell in tests)\r\n", command.unwrap_or_default()).as_bytes());
            return this;
        }
        if let Err(e) = this.spawn(cwd, command, cx) {
            this.screen().process(format!("Couldn't start a shell: {e}\r\n").as_bytes());
        }
        this
    }

    fn screen(&self) -> MutexGuard<'_, vt100::Parser> {
        self.parser.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn spawn(&mut self, cwd: Option<PathBuf>, command: Option<String>, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let pty = native_pty_system().openpty(PtySize { rows: self.size.0, cols: self.size.1, pixel_width: 0, pixel_height: 0 })?;
        // Read when a terminal opens, so a change in settings applies to the next one.
        let cmd = shell_command(&trek_core::settings::Settings::load().terminal.shell, cwd, command.as_deref());
        let mut child = pty.slave.spawn_command(cmd)?;
        let reader = pty.master.try_clone_reader()?;
        self.writer = Some(pty.master.take_writer()?);
        self.master = Some(pty.master);
        let (tx, rx) = async_channel::bounded::<()>(1);
        let screen = self.parser.clone();
        std::thread::spawn(move || {
            pump(reader, &screen, &tx);
            let _ = child.wait();
        });
        self._reader = Some(cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
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

    fn write(&mut self, bytes: &[u8]) {
        if let Some(w) = self.writer.as_mut() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) == self.size || rows < 2 || cols < 10 {
            return;
        }
        self.size = (rows, cols);
        self.screen().screen_mut().set_size(rows, cols);
        if let Some(m) = &self.master {
            let _ = m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let m = &k.modifiers;
        if let Some(shortcut) = shortcut(&k.key, m, cfg!(windows)) {
            match shortcut {
                Shortcut::Paste => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        let bracketed = self.screen().screen().bracketed_paste();
                        self.write(paste_bytes(&text, bracketed, cfg!(windows)).as_bytes());
                    }
                }
                Shortcut::Clear => self.write(b"\x0c"),
                Shortcut::PassThrough => return, // let app shortcuts through
            }
            cx.stop_propagation();
            return;
        }
        let app = self.screen().screen().application_cursor();
        let seq: Option<Vec<u8>> = match k.key.as_str() {
            "enter" => Some(b"\r".to_vec()),
            "backspace" => Some(if m.alt { b"\x1b\x7f".to_vec() } else { b"\x7f".to_vec() }),
            "tab" => Some(if m.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() }),
            "escape" => Some(b"\x1b".to_vec()),
            "up" => Some(if app { b"\x1bOA" } else { b"\x1b[A" }.to_vec()),
            "down" => Some(if app { b"\x1bOB" } else { b"\x1b[B" }.to_vec()),
            "right" => Some(if m.alt { b"\x1bf".to_vec() } else if app { b"\x1bOC".to_vec() } else { b"\x1b[C".to_vec() }),
            "left" => Some(if m.alt { b"\x1bb".to_vec() } else if app { b"\x1bOD".to_vec() } else { b"\x1b[D".to_vec() }),
            "home" => Some(b"\x1b[H".to_vec()),
            "end" => Some(b"\x1b[F".to_vec()),
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

    /// The last `n` lines with something on them, as the screen shows them ("Add to Chat":
    /// the terminal has no selection of its own).
    pub fn recent_output(&self, n: usize) -> String {
        let text = self.screen().screen().contents();
        let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
        let end = lines.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
        lines[..end].iter().skip(end.saturating_sub(n)).copied().collect::<Vec<_>>().join("\n")
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (family, cell_width) = self
            .font
            .get_or_insert_with(|| {
                let family = terminal_font(window, theme.mono_font_family.clone());
                let id = window.text_system().resolve_font(&font(family.clone()));
                let w = window.text_system().em_advance(id, px(FONT_SIZE)).map(|w| w.as_f32()).unwrap_or(FONT_SIZE * 0.6);
                (family, w)
            })
            .clone();
        let parser = self.parser.clone();
        let parser = parser.lock().unwrap_or_else(|e| e.into_inner());
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let (cur_row, cur_col) = screen.cursor_position();
        let show_cursor = !screen.hide_cursor() && !self.exited;
        let mut lines = Vec::with_capacity(rows as usize);
        for r in 0..rows {
            let mut text = String::new();
            let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
            for c in 0..cols {
                let Some(cell) = screen.cell(r, c) else { continue };
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
                if show_cursor && r == cur_row && c == cur_col {
                    bg = Some(theme.foreground.opacity(0.75));
                    fg = Some(theme.background);
                }
                if fg.is_some() || bg.is_some() || cell.bold() || cell.underline() {
                    highlights.push((
                        start..text.len(),
                        HighlightStyle {
                            color: fg,
                            background_color: bg,
                            font_weight: cell.bold().then_some(FontWeight::BOLD),
                            underline: cell.underline().then(|| UnderlineStyle { thickness: px(1.), ..Default::default() }),
                            ..Default::default()
                        },
                    ));
                }
            }
            let trimmed = text.trim_end().len().max(highlights.last().map(|h| h.0.end).unwrap_or(0));
            text.truncate(trimmed);
            highlights.retain(|h| h.0.end <= text.len());
            lines.push(div().h(px(LINE_HEIGHT)).whitespace_nowrap().child(StyledText::new(text).with_highlights(highlights)));
        }
        let entity = cx.entity().downgrade();
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .px_3()
            .py_2()
            .bg(theme.background)
            .font_family(family)
            .text_size(px(FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .text_color(theme.foreground)
            .overflow_hidden()
            .cursor_text()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.focus.focus(window, cx);
            }))
            .on_key_down(cx.listener(Self::key))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let rows = ((bounds.size.height.as_f32()) / LINE_HEIGHT).floor() as u16;
                        let cols = ((bounds.size.width.as_f32()) / cell_width).floor() as u16;
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
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .children(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::{Flavor, Screen, Shortcut, choose_shell, flavor, paste_bytes, plain_path, powershell_quote, pump, shell_args, shell_cwd, shortcut};
    use gpui_kit::Modifiers;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    fn found(names: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |n| names.contains(&n).then(|| PathBuf::from(format!(r"C:\bin\{n}.exe")))
    }

    #[test]
    fn windows_picks_pwsh_then_windows_powershell_then_comspec() {
        let pick = |names, comspec| choose_shell("", None, true, found(names), comspec);
        assert_eq!(pick(&["pwsh", "powershell"], Some(r"C:\Windows\System32\cmd.exe")), r"C:\bin\pwsh.exe");
        assert_eq!(pick(&["powershell"], Some(r"C:\Windows\System32\cmd.exe")), r"C:\bin\powershell.exe");
        assert_eq!(pick(&[], Some(r"C:\Windows\System32\cmd.exe")), r"C:\Windows\System32\cmd.exe");
        assert_eq!(pick(&[], Some("")), "cmd.exe", "an empty COMSPEC is no COMSPEC");
        assert_eq!(pick(&[], None), "cmd.exe");
    }

    #[test]
    fn a_mac_keeps_its_login_shell_and_the_setting_overrides_either() {
        assert_eq!(choose_shell("", Some("/opt/homebrew/bin/fish"), false, found(&["pwsh"]), None), "/opt/homebrew/bin/fish");
        assert_eq!(choose_shell("", None, false, found(&[]), None), "/bin/zsh");
        // A name is looked up; a path, quoted or not, is taken as it is; a name nothing has stays a name.
        assert_eq!(choose_shell("pwsh", Some("/bin/zsh"), false, found(&["pwsh"]), None), r"C:\bin\pwsh.exe");
        assert_eq!(choose_shell(r#" "D:\tools\nu.exe" "#, None, true, found(&["pwsh"]), None), r"D:\tools\nu.exe");
        assert_eq!(choose_shell("nu", None, true, found(&[]), None), "nu");
    }

    #[test]
    fn the_shell_kind_comes_from_its_name_on_windows_only() {
        assert_eq!(flavor(r"C:\Program Files\PowerShell\7\pwsh.exe", true), Flavor::PowerShell);
        assert_eq!(flavor("PowerShell.EXE", true), Flavor::PowerShell);
        assert_eq!(flavor(r"C:\Windows\System32\cmd.exe", true), Flavor::Cmd);
        assert_eq!(flavor(r"C:\Program Files\Git\bin\bash.exe", true), Flavor::Posix);
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
        assert!(ps[2].starts_with("Write-Host '$ winget install it''s'; "), "{}", ps[2]);
        assert!(ps[2].contains("; winget install it's; $ok = $?;"), "{}", ps[2]);
        assert!(shell_args(Flavor::Cmd, None).is_empty());
        assert_eq!(shell_args(Flavor::Cmd, Some("dir")).first().map(String::as_str), Some("/c"));
    }

    #[test]
    fn powershell_quoting_doubles_every_kind_of_single_quote() {
        assert_eq!(powershell_quote("plain"), "'plain'");
        assert_eq!(powershell_quote("it's"), "'it''s'");
        assert_eq!(powershell_quote("a $b `c \"d\""), "'a $b `c \"d\"'", "nothing else is special in single quotes");
        assert_eq!(powershell_quote("\u{2018}x\u{2019}"), "'\u{2018}\u{2018}x\u{2019}\u{2019}'");
    }

    #[test]
    fn the_shell_starts_in_a_plain_folder() {
        let home = PathBuf::from(r"C:\Users\me");
        assert_eq!(plain_path(PathBuf::from(r"\\?\C:\code\app")), PathBuf::from(r"C:\code\app"));
        assert_eq!(plain_path(PathBuf::from(r"\\?\UNC\nas\share\app")), PathBuf::from(r"\\nas\share\app"));
        assert_eq!(plain_path(PathBuf::from(r"\\?\Volume{1234}\x")), PathBuf::from(r"\\?\Volume{1234}\x"), "only a drive or share path has a plain form");
        assert_eq!(plain_path(PathBuf::from(r"C:\code")), PathBuf::from(r"C:\code"));
        let verbatim = Some(PathBuf::from(r"\\?\C:\code\app"));
        assert_eq!(shell_cwd(verbatim.clone(), Flavor::PowerShell, true, home.clone()), PathBuf::from(r"C:\code\app"));
        assert_eq!(shell_cwd(None, Flavor::Cmd, true, home.clone()), home);
        // A network folder is fine for PowerShell, and not for cmd.exe.
        let unc = Some(PathBuf::from(r"\\nas\share\app"));
        assert_eq!(shell_cwd(unc.clone(), Flavor::PowerShell, true, home.clone()), PathBuf::from(r"\\nas\share\app"));
        assert_eq!(shell_cwd(Some(PathBuf::from(r"\\?\UNC\nas\share\app")), Flavor::Cmd, true, home.clone()), home);
        // Off Windows nothing is rewritten.
        assert_eq!(shell_cwd(verbatim.clone(), Flavor::Posix, false, home), verbatim.unwrap());
    }

    #[test]
    fn paste_keys_leave_ctrl_v_to_the_shell() {
        let keys = |control, shift, platform| Modifiers { control, shift, platform, ..Default::default() };
        // Windows: Ctrl+Shift+V and Shift+Insert paste; Ctrl+V is the shell's, and so are Ctrl+C and the Win key's.
        assert_eq!(shortcut("v", &keys(true, true, false), true), Some(Shortcut::Paste));
        assert_eq!(shortcut("insert", &keys(false, true, false), true), Some(Shortcut::Paste));
        assert_eq!(shortcut("v", &keys(true, false, false), true), None);
        assert_eq!(shortcut("c", &keys(true, false, false), true), None, "Ctrl+C interrupts");
        assert_eq!(shortcut("c", &keys(true, true, false), true), Some(Shortcut::PassThrough), "no selection to copy, and it isn't an interrupt");
        assert_eq!(shortcut("v", &keys(false, false, true), true), Some(Shortcut::PassThrough));
        assert_eq!(shortcut("a", &keys(false, false, false), true), None);
        // macOS: Cmd+V pastes, Cmd+K clears, the rest of Cmd is the app's, Ctrl is the shell's.
        assert_eq!(shortcut("v", &keys(false, false, true), false), Some(Shortcut::Paste));
        assert_eq!(shortcut("k", &keys(false, false, true), false), Some(Shortcut::Clear));
        assert_eq!(shortcut("t", &keys(false, false, true), false), Some(Shortcut::PassThrough));
        assert_eq!(shortcut("v", &keys(true, false, false), false), None);
    }

    #[test]
    fn pasted_text_is_bracketed_when_asked_and_enters_as_cr_on_windows() {
        assert_eq!(paste_bytes("a\nb", false, false), "a\nb", "a Mac is untouched");
        assert_eq!(paste_bytes("a\r\nb\nc", false, true), "a\rb\rc");
        assert_eq!(paste_bytes("ls", true, true), "\x1b[200~ls\x1b[201~");
    }

    #[test]
    fn a_flood_of_output_is_parsed_off_the_main_thread_with_one_wake_up() {
        // 8 MB of `yes`, with nobody draining the wake-ups: the reader never blocks on the view,
        // nothing queues up but the one wake-up, and the screen holds the latest output.
        let flood = "y\r\n".repeat(2 << 20) + "done\r\n";
        let screen: Screen = Arc::new(Mutex::new(vt100::Parser::new(24, 80, 2000)));
        let (tx, rx) = async_channel::bounded::<()>(1);
        pump(std::io::Cursor::new(flood.into_bytes()), &screen, &tx);
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
        let screen: Screen = Arc::new(Mutex::new(vt100::Parser::new(24, 80, 0)));
        let (tx, rx) = async_channel::bounded::<()>(1);
        drop(rx);
        // An endless stream: `pump` returns because nothing listens any more.
        pump(std::io::repeat(b'y'), &screen, &tx);
    }

    /// The shell, in a real pseudo-terminal (ConPTY), as the panel starts it.
    #[cfg(windows)]
    mod conpty {
        use super::super::{Screen, pump, shell_command};
        use portable_pty::{PtySize, native_pty_system};
        use std::io::Write as _;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        /// Starts `setting`'s shell running `command` (if any) in a temp folder, types `input`
        /// (if any), and returns the screen once `done` likes it, or after 20 s. The shell is
        /// killed afterwards.
        fn run(setting: &str, command: Option<&str>, input: Option<&str>, done: impl Fn(&str) -> bool) -> String {
            let dir = std::env::temp_dir().join(format!("trek-terminal-test-{}-{}", std::process::id(), setting.replace(|c: char| !c.is_ascii_alphanumeric(), "_")));
            std::fs::create_dir_all(&dir).unwrap();
            let pty = native_pty_system().openpty(PtySize { rows: 30, cols: 100, pixel_width: 0, pixel_height: 0 }).unwrap();
            let mut child = pty.slave.spawn_command(shell_command(setting, Some(dir.clone()), command)).unwrap();
            let reader = pty.master.try_clone_reader().unwrap();
            let mut writer = pty.master.take_writer().unwrap();
            let screen: Screen = Arc::new(Mutex::new(vt100::Parser::new(30, 100, 0)));
            let (tx, _rx) = async_channel::bounded::<()>(1);
            let pumped = screen.clone();
            std::thread::spawn(move || pump(reader, &pumped, &tx));
            if let Some(input) = input {
                // The shell reads keys once it has started: wait for its first output (a prompt).
                let start = Instant::now();
                while screen.lock().unwrap().screen().contents().trim().is_empty() && start.elapsed() < Duration::from_secs(20) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                std::thread::sleep(Duration::from_millis(500));
                writer.write_all(input.as_bytes()).unwrap();
                writer.flush().unwrap();
            }
            let start = Instant::now();
            let mut text = String::new();
            while start.elapsed() < Duration::from_secs(20) {
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
            assert!(has_line(&text, "trek-ok"), "no output from the shell:\n{text}");
        }

        #[test]
        fn the_shell_starts_in_the_folder_it_was_given() {
            let text = run("", None, Some("echo trek-ok\r"), |t| has_line(t, "trek-ok"));
            assert!(text.contains("trek-terminal-test-"), "no prompt in the folder:\n{text}");
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
                let text = run(name, Some(command), None, |t| t.contains("[finished with exit code"));
                assert!(has_line(&text, shows), "{name}: the command's output is missing:\n{text}");
                assert!(text.contains("[finished with exit code 0]") || name == "cmd", "{name}: no exit code:\n{text}");
            }
        }

        #[test]
        fn a_failing_native_command_reports_its_code_in_powershell() {
            let Some(name) = ["pwsh", "powershell"].into_iter().find(|n| trek_core::detect::which(n).is_some()) else { return };
            let text = run(name, Some("cmd /c exit 3"), None, |t| t.contains("[finished with exit code"));
            assert!(text.contains("[finished with exit code 3]"), "{text}");
        }
    }
}
