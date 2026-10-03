//! A real terminal: the user's login shell in a PTY, parsed by vt100, painted as styled rows.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;

const FONT_SIZE: f32 = 12.5;
const LINE_HEIGHT: f32 = 18.;

/// Prefer an installed Nerd Font so prompt glyphs (powerline, icons) render.
const PREFERRED: &[&str] = &["FiraCode Nerd Font Mono", "JetBrainsMono Nerd Font Mono", "MesloLGS NF", "MesloLGM Nerd Font Mono", "Hack Nerd Font Mono", "SauceCodePro Nerd Font Mono"];

fn terminal_font(window: &Window, fallback: SharedString) -> SharedString {
    let names = window.text_system().all_font_names();
    PREFERRED.iter().find(|p| names.iter().any(|n| n == *p)).map(|p| SharedString::from(*p)).unwrap_or(fallback)
}

pub struct TerminalPanel {
    parser: vt100::Parser,
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
            parser: vt100::Parser::new(size.0, size.1, 2000),
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
            this.parser.process(format!("$ {}\r\n(no shell in tests)\r\n", command.unwrap_or_default()).as_bytes());
            return this;
        }
        if let Err(e) = this.spawn(cwd, command, cx) {
            this.parser.process(format!("Couldn't start a shell: {e}\r\n").as_bytes());
        }
        this
    }

    fn spawn(&mut self, cwd: Option<PathBuf>, command: Option<String>, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let pty = native_pty_system().openpty(PtySize { rows: self.size.0, cols: self.size.1, pixel_width: 0, pixel_height: 0 })?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let mut cmd = CommandBuilder::new(shell);
        cmd.arg("-l");
        if let Some(c) = &command {
            let quoted = format!("'{}'", c.replace('\'', "'\\''"));
            cmd.arg("-c");
            cmd.arg(format!(
                "printf '\\033[1m$ %s\\033[0m\\n\\n' {quoted}; {c}; code=$?; printf '\\n\\033[2m[finished with exit code %s]\\033[0m\\n' $code; exit $code"
            ));
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Trek");
        cmd.cwd(cwd.unwrap_or_else(trek_core::paths::home));
        let mut child = pty.slave.spawn_command(cmd)?;
        let mut reader = pty.master.try_clone_reader()?;
        self.writer = Some(pty.master.take_writer()?);
        self.master = Some(pty.master);
        let (tx, rx) = async_channel::unbounded::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 16384];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send_blocking(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = child.wait();
        });
        self._reader = Some(cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut bytes = first;
                while let Ok(more) = rx.try_recv() {
                    bytes.extend(more);
                }
                if this
                    .update(cx, |this, cx| {
                        this.parser.process(&bytes);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
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
        self.parser.screen_mut().set_size(rows, cols);
        if let Some(m) = &self.master {
            let _ = m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let m = &k.modifiers;
        if m.platform {
            match k.key.as_str() {
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        let paste = if self.parser.screen().bracketed_paste() { format!("\x1b[200~{text}\x1b[201~") } else { text };
                        self.write(paste.as_bytes());
                    }
                }
                "k" => {
                    self.write(b"\x0c");
                }
                _ => return, // let app shortcuts through
            }
            cx.stop_propagation();
            return;
        }
        let app = self.parser.screen().application_cursor();
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
        let screen = self.parser.screen();
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
