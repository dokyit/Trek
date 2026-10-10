//! Every keyboard shortcut Trek has, in one table, and how each is written for the person using
//! it: ⌘⇧K on a Mac, Ctrl+Shift+K on Windows.
//!
//! `TABLE` holds every binding (what `main.rs`, `notes.rs` and `editor.rs` bind), with the keys
//! each platform has it on. Where Windows differs, the row says why. Everything that shows a
//! shortcut draws on it: the Keyboard Shortcuts page (`shortcut_groups`), the palette's hints
//! (`hint`) and, for the tooltips and sentences written with Mac glyphs, `localize`, which turns
//! a glyph run like `⌥⌘E` into the keys the Windows build really binds (Alt+Shift+E), not just
//! the same letters with other modifiers.
//!
//! The Mac side is what Trek has always had, byte for byte; the Windows side is the same keys
//! with Ctrl where the Mac has ⌘, except where that would collide with Windows or with the shell
//! in the terminal panel (see the rows).

use gpui_kit::{Keystroke, KeyBinding};
use std::borrow::Cow;
use std::sync::LazyLock;

/// Whether this build is shown the Mac way. Tests can say otherwise for one closure
/// (`with_mac`), so both spellings are checked on whichever machine runs them.
pub fn is_mac() -> bool {
    #[cfg(test)]
    if let Some(mac) = FORCE_MAC.with(|f| f.get()) {
        return mac;
    }
    cfg!(target_os = "macos")
}

#[cfg(test)]
thread_local! {
    static FORCE_MAC: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Run `f` as if on a Mac (`true`) or on Windows (`false`): everything here that asks `is_mac`.
#[cfg(test)]
pub fn with_mac<T>(mac: bool, f: impl FnOnce() -> T) -> T {
    let before = FORCE_MAC.with(|c| c.replace(Some(mac)));
    let out = f();
    FORCE_MAC.with(|c| c.set(before));
    out
}

// ───────────────────────────── keystrokes ─────────────────────────────

/// A keystroke as modifiers and a key, whichever way it was written (`cmd-shift-k`,
/// `secondary-k`, `⌘⇧K`). `cmd` is ⌘ on a Mac and the Windows key on Windows.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct Stroke {
    ctrl: bool,
    alt: bool,
    shift: bool,
    cmd: bool,
    key: String,
}

impl Stroke {
    /// `s` as gpui reads it. `secondary` is ⌘ on a Mac and Ctrl elsewhere (`mac` says which).
    fn parse(s: &str, mac: bool) -> Option<Stroke> {
        let mut out = Stroke::default();
        let mut parts: Vec<&str> = s.split('-').collect();
        // A key that is itself "-" leaves an empty part at the end ("cmd--").
        let mut key = None;
        if parts.len() > 1 && parts.last() == Some(&"") {
            parts.pop();
            parts.pop();
            key = Some("-".to_string());
        }
        let last = parts.len();
        for (i, part) in parts.iter().enumerate() {
            let lower = part.to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" if i + 1 < last || key.is_some() => out.ctrl = true,
                "alt" | "option" if i + 1 < last || key.is_some() => out.alt = true,
                "shift" if i + 1 < last || key.is_some() => out.shift = true,
                "cmd" | "super" | "win" if i + 1 < last || key.is_some() => out.cmd = true,
                "secondary" if i + 1 < last || key.is_some() => {
                    if mac {
                        out.cmd = true
                    } else {
                        out.ctrl = true
                    }
                }
                _ if i + 1 == last && key.is_none() => {
                    let single_upper = part.len() == 1 && part.as_bytes()[0].is_ascii_uppercase();
                    out.shift |= single_upper;
                    key = Some(lower);
                }
                _ => return None,
            }
        }
        out.key = key.filter(|k| !k.is_empty())?;
        Some(out)
    }

    fn from_gpui(k: &Keystroke) -> Stroke {
        Stroke { ctrl: k.modifiers.control, alt: k.modifiers.alt, shift: k.modifiers.shift, cmd: k.modifiers.platform, key: k.key.to_ascii_lowercase() }
    }

    /// The keystroke as gpui parses it (`ctrl-shift-k`).
    fn keystroke(&self) -> String {
        let mut s = String::new();
        for (on, name) in [(self.ctrl, "ctrl-"), (self.alt, "alt-"), (self.shift, "shift-"), (self.cmd, "cmd-")] {
            if on {
                s.push_str(name);
            }
        }
        s + &self.key
    }

    /// The same keys on Windows when nothing says otherwise: Ctrl where the Mac has ⌘.
    fn ctrl_for_cmd(&self) -> Stroke {
        Stroke { ctrl: self.ctrl || self.cmd, cmd: false, ..self.clone() }
    }

    /// A Ctrl+letter with nothing else held: the keys a shell uses (readline's, and the terminal's
    /// own Ctrl+C / Ctrl+L / Ctrl+V).
    fn is_plain_ctrl_letter(&self) -> bool {
        self.ctrl && !self.alt && !self.shift && !self.cmd && self.key.len() == 1 && self.key.as_bytes()[0].is_ascii_lowercase()
    }

    /// The whole keystroke as one run of text: `⌘⇧K` or `Ctrl+Shift+K`.
    fn label(&self, mac: bool) -> String {
        if mac {
            let mut s = self.mac_modifiers();
            s.push_str(&mac_key(&self.key));
            s
        } else {
            let mut parts = self.windows_modifiers();
            parts.push(windows_key(&self.key));
            parts.join("+")
        }
    }

    /// ⌃⌥⌘⇧, in the order Trek's labels have always had.
    fn mac_modifiers(&self) -> String {
        let mut s = String::new();
        for (on, glyph) in [(self.ctrl, '⌃'), (self.alt, '⌥'), (self.cmd, '⌘'), (self.shift, '⇧')] {
            if on {
                s.push(glyph);
            }
        }
        s
    }

    fn windows_modifiers(&self) -> Vec<String> {
        let mut parts = vec![];
        for (on, name) in [(self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.shift, "Shift"), (self.cmd, "Win")] {
            if on {
                parts.push(name.to_string());
            }
        }
        parts
    }

    /// One key cap per modifier and one for the key, as the Keyboard Shortcuts page draws them.
    fn chips(&self, mac: bool) -> Vec<String> {
        if mac {
            let mut chips: Vec<String> = self.mac_modifiers().chars().map(String::from).collect();
            chips.push(if self.key == "escape" { "esc".into() } else { mac_key(&self.key) });
            chips
        } else {
            let mut chips = self.windows_modifiers();
            chips.push(windows_key(&self.key));
            chips
        }
    }
}

/// A key as the Mac writes it: a glyph where there is one, else the key upper-case.
fn mac_key(key: &str) -> String {
    match key {
        "enter" => "↩",
        "backspace" => "⌫",
        "escape" => "⎋",
        "tab" => "⇥",
        "space" => "␣",
        "delete" => "⌦",
        "up" => "↑",
        "down" => "↓",
        "left" => "←",
        "right" => "→",
        "pageup" => "⇞",
        "pagedown" => "⇟",
        "home" => "↖",
        "end" => "↘",
        k => return k.to_uppercase(),
    }
    .to_string()
}

/// A key as Windows keyboards are labelled.
fn windows_key(key: &str) -> String {
    match key {
        "enter" => "Enter".into(),
        "backspace" => "Backspace".into(),
        "escape" => "Esc".into(),
        "tab" => "Tab".into(),
        "space" => "Space".into(),
        "delete" => "Delete".into(),
        "insert" => "Insert".into(),
        "up" => "Up".into(),
        "down" => "Down".into(),
        "left" => "Left".into(),
        "right" => "Right".into(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        k => k.to_uppercase(),
    }
}

/// `keystroke` (`cmd-shift-k`, `secondary-k`, `ctrl-tab`) as the active platform writes it:
/// `⌘⇧K` or `Ctrl+Shift+K`. Text that isn't a keystroke comes back as it is.
pub fn label(keystroke: &str) -> String {
    label_for(keystroke, is_mac())
}

/// `label` for the Mac (`mac`) or Windows.
pub fn label_for(keystroke: &str, mac: bool) -> String {
    Stroke::parse(keystroke, mac).map_or_else(|| keystroke.to_string(), |s| s.label(mac))
}

/// A keystroke a window received (or a binding holds), written like `label`. (The menu bar's
/// shortcut labels use it.)
pub fn label_keystroke(keystroke: &Keystroke) -> String {
    Stroke::from_gpui(keystroke).label(is_mac())
}

// ───────────────────────────── the table ─────────────────────────────

/// Every shortcut Trek binds, and a few of the system's it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Id {
    Quit,
    HideApp,
    Minimize,
    NewThread,
    OpenFolder,
    OpenSettings,
    ToggleSidebar,
    SettleThread,
    TogglePlan,
    TakeSnapshot,
    CycleHandHolding,
    Interrupt,
    ToggleRightPanel,
    OpenPalette,
    OpenPaletteAnywhere,
    InlineEdit,
    OpenInNewWindow,
    OpenBasecamp,
    LeaveBasecamp,
    OpenNotes,
    SwitchMode,
    ToggleIde,
    ToggleIdeSearch,
    QuickOpen,
    ToggleAiBar,
    ToggleTerminal,
    FocusScm,
    AddSelectionToChat,
    AddSelectionToNewChat,
    KeepHunk,
    UndoHunk,
    NextHunk,
    PreviousHunk,
    UndoAllOrStop,
    CloseWindow,
    CloseTab,
    NextTab,
    PreviousTab,
    NoteBold,
    NoteItalic,
    NoteUnderline,
    NoteStrike,
    NoteBullets,
    NoteNumbers,
    NoteChecklist,
    NoteHeading1,
    NoteHeading2,
    NoteQuote,
    NoteCode,
    NoteToggleCheck,
    NewNote,
    SaveFile,
    /// The text fields' own redo (the kit binds it): ⇧⌘Z on a Mac, Ctrl+Y on Windows.
    Redo,
}

/// Which list of bindings a row goes into (each file binds its own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    App,
    Notes,
    Editor,
    /// Not Trek's to bind: the system's, or the text fields', named so labels can show them.
    System,
}

/// What Windows has a binding on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Win {
    /// The Mac's keys with Ctrl for ⌘.
    Same,
    /// These keys instead (written as gpui reads them, `ctrl-` for Ctrl).
    Keys(&'static str),
    /// Not on Windows: there's nothing for it to do there.
    Dropped,
}

type Build = fn(&str, Option<&str>) -> KeyBinding;

struct Binding {
    id: Id,
    group: Group,
    /// The Mac's keys (none: a Windows-only binding).
    mac: Option<&'static str>,
    windows: Win,
    context: Option<&'static str>,
    /// Makes the `KeyBinding` (none for a system's shortcut).
    build: Option<Build>,
}

macro_rules! act {
    ($a:expr) => {
        Some((|keys: &str, context: Option<&str>| KeyBinding::new(keys, $a, context)) as Build)
    };
}

fn row(id: Id, group: Group, mac: &'static str, windows: Win, context: Option<&'static str>, build: Option<Build>) -> Binding {
    Binding { id, group, mac: Some(mac), windows, context, build }
}

static TABLE: LazyLock<Vec<Binding>> = LazyLock::new(|| {
    use Group::*;
    use Id::*;
    use Win::{Dropped, Keys, Same};
    const NOTES: Option<&str> = Some(crate::notes::CONTEXT);
    vec![
        row(Quit, App, "cmd-q", Same, None, act!(crate::Quit)),
        // No such thing as hiding an app on Windows.
        row(HideApp, App, "cmd-h", Dropped, None, act!(crate::HideApp)),
        // Win+Down minimizes there (the system's), and so does the title bar's button.
        row(Minimize, App, "cmd-m", Dropped, None, act!(crate::Minimize)),
        // ⌘N only ever makes something new: never an undo, whatever has focus.
        row(NewThread, App, "cmd-n", Same, None, act!(crate::NewThread)),
        row(OpenFolder, App, "cmd-o", Same, None, act!(crate::OpenFolder)),
        row(OpenSettings, App, "cmd-,", Same, None, act!(crate::OpenSettings)),
        row(ToggleSidebar, App, "cmd-b", Same, None, act!(crate::ToggleSidebar)),
        row(SettleThread, App, "cmd-e", Same, None, act!(crate::SettleThread)),
        row(TogglePlan, App, "shift-tab", Same, Some("Composer"), act!(crate::TogglePlan)),
        row(TakeSnapshot, App, "cmd-shift-s", Same, None, act!(crate::TakeSnapshot)),
        row(CycleHandHolding, App, "cmd-shift-a", Same, None, act!(crate::CycleHandHolding)),
        row(Interrupt, App, "cmd-.", Same, None, act!(crate::Interrupt)),
        row(ToggleRightPanel, App, "cmd-j", Same, None, act!(crate::ToggleRightPanel)),
        // Not in the editor's text, where ⌘K is an inline edit (below).
        row(OpenPalette, App, "cmd-k", Same, Some("!IdeEditor"), act!(crate::OpenPalette)),
        // Ctrl+K belongs to the shell in the terminal panel (kill to end of line) and is the inline
        // edit in the editor's text, so the palette gets a second way in that works there too,
        // as in VS Code.
        Binding { id: OpenPaletteAnywhere, group: App, mac: None, windows: Keys("ctrl-shift-p"), context: None, build: act!(crate::OpenPalette) },
        row(OpenInNewWindow, App, "cmd-shift-enter", Same, None, act!(crate::OpenInNewWindow)),
        row(OpenBasecamp, App, "cmd-shift-h", Same, None, act!(crate::OpenBasecamp)),
        row(LeaveBasecamp, App, "escape", Same, Some("Basecamp"), act!(crate::basecamp::Leave)),
        row(OpenNotes, App, "cmd-shift-j", Same, None, act!(crate::OpenNotes)),
        // ⌥⌘E switches Agents ⇄ Editor. On Windows Ctrl+Alt is AltGr, which types characters on
        // many layouts (German, French, Polish…), so ⌥⌘ chords become Alt+Shift.
        row(SwitchMode, App, "alt-cmd-e", Keys("alt-shift-e"), None, act!(crate::SwitchMode)),
        // ⌘⇧E is the Explorer, in the editor only (as in VS Code).
        row(ToggleIde, App, "cmd-shift-e", Same, Some("TrekIde"), act!(crate::ToggleIde)),
        row(ToggleIdeSearch, App, "cmd-shift-f", Same, Some("TrekWindow"), act!(crate::ToggleIdeSearch)),
        row(QuickOpen, App, "cmd-p", Same, None, act!(crate::QuickOpen)),
        // Ctrl+Alt+B would be AltGr+B.
        row(ToggleAiBar, App, "alt-cmd-b", Keys("alt-shift-b"), None, act!(crate::ToggleAiBar)),
        // Ctrl+` as in VS Code; no ⌘ in it, so it's the same on both and the terminal panel
        // (which has focus when it's open) lets it through.
        row(ToggleTerminal, App, "ctrl-`", Same, None, act!(crate::ToggleTerminal)),
        // The same on both (VS Code's too).
        row(FocusScm, App, "ctrl-shift-g", Same, None, act!(crate::FocusScm)),
        // The editor's selection to the AI side bar: ⌘⇧L the chat in front, ⌘L a new one.
        row(AddSelectionToChat, App, "cmd-shift-l", Same, Some("TrekIde"), act!(crate::AddSelectionToChat)),
        row(AddSelectionToNewChat, App, "cmd-l", Same, Some("TrekIde"), act!(crate::AddSelectionToNewChat)),
        // In the editor's text: ⌘K edits the picked lines inline (elsewhere it's the palette);
        // with a review's hunks in the file, ⌘Y keeps the one the bar is on and ⌥⌘⌫ undoes it
        // (its toast takes that back), ⌥⌘↑/↓ step between them.
        row(InlineEdit, App, "cmd-k", Same, Some("IdeEditor"), act!(crate::editor::InlineEdit)),
        // Ctrl+Y is redo in Windows text fields, so Keep takes Ctrl+Shift+Y.
        row(KeepHunk, App, "cmd-y", Keys("ctrl-shift-y"), Some("IdeEditor && hunks"), act!(crate::editor::KeepHunk)),
        // Ctrl+Alt+Backspace is AltGr+Backspace.
        row(UndoHunk, App, "alt-cmd-backspace", Keys("alt-shift-backspace"), Some("IdeEditor && hunks"), act!(crate::editor::UndoHunk)),
        // Ctrl+Alt+Up/Down rotates the screen on some graphics drivers, and the text field's own
        // Alt+Shift+Up/Down add a cursor; Alt+F5 and Shift+Alt+F5 are VS Code's next/previous change.
        row(NextHunk, App, "alt-cmd-down", Keys("alt-f5"), Some("IdeEditor && hunks"), act!(crate::editor::NextHunk)),
        row(PreviousHunk, App, "alt-cmd-up", Keys("alt-shift-f5"), Some("IdeEditor && hunks"), act!(crate::editor::PreviousHunk)),
        // In the AI input: undo every pending change, or stop the turn (⌘↩, its pair, comes
        // through the input's Enter).
        row(UndoAllOrStop, App, "cmd-shift-backspace", Same, Some("AiInput"), act!(crate::ide::ai::UndoAllOrStop)),
        // Only thread windows close with ⌘W; the main window stays put.
        row(CloseWindow, App, "cmd-w", Same, Some("ThreadWindow"), act!(crate::CloseWindow)),
        // In the main window ⌘W closes the tab in front; the window stays put.
        row(CloseTab, App, "cmd-w", Same, Some("TrekWindow"), act!(crate::CloseTab)),
        // Standard on Windows too.
        row(NextTab, App, "ctrl-tab", Same, None, act!(crate::NextTab)),
        row(PreviousTab, App, "ctrl-shift-tab", Same, None, act!(crate::PreviousTab)),
        // Notes.
        row(NoteBold, Notes, "cmd-b", Same, NOTES, act!(crate::notes::Bold)),
        row(NoteItalic, Notes, "cmd-i", Same, NOTES, act!(crate::notes::Italic)),
        row(NoteUnderline, Notes, "cmd-u", Same, NOTES, act!(crate::notes::Underline)),
        row(NoteStrike, Notes, "cmd-shift-x", Same, NOTES, act!(crate::notes::Strike)),
        row(NoteBullets, Notes, "cmd-shift-8", Same, NOTES, act!(crate::notes::Bullets)),
        row(NoteNumbers, Notes, "cmd-shift-7", Same, NOTES, act!(crate::notes::Numbers)),
        row(NoteChecklist, Notes, "cmd-shift-9", Same, NOTES, act!(crate::notes::Checklist)),
        // Ctrl+Alt+1 is AltGr+1.
        row(NoteHeading1, Notes, "cmd-alt-1", Keys("alt-shift-1"), NOTES, act!(crate::notes::Heading1)),
        row(NoteHeading2, Notes, "cmd-alt-2", Keys("alt-shift-2"), NOTES, act!(crate::notes::Heading2)),
        row(NoteQuote, Notes, "cmd-shift-.", Same, NOTES, act!(crate::notes::Quote)),
        row(NoteCode, Notes, "cmd-e", Same, NOTES, act!(crate::notes::Code)),
        row(NoteToggleCheck, Notes, "cmd-enter", Same, NOTES, act!(crate::notes::ToggleCheck)),
        row(NewNote, Notes, "cmd-n", Same, Some("Notes"), act!(crate::notes::NewNote)),
        // The editor.
        row(SaveFile, Editor, "cmd-s", Same, Some("TrekEditor"), act!(crate::editor::SaveFile)),
        // Not Trek's.
        row(Redo, System, "cmd-shift-z", Keys("ctrl-y"), None, None),
    ]
});

/// The keys and context a row has on a platform (none: it has none there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub keystroke: String,
    pub context: Option<String>,
}

fn windows_stroke(b: &Binding) -> Option<Stroke> {
    match b.windows {
        Win::Dropped => None,
        Win::Keys(keys) => Stroke::parse(keys, false),
        Win::Same => Stroke::parse(b.mac?, true).map(|s| s.ctrl_for_cmd()),
    }
}

fn stroke_of(b: &Binding, mac: bool) -> Option<Stroke> {
    if mac { Stroke::parse(b.mac?, true) } else { windows_stroke(b) }
}

fn resolve(b: &Binding, mac: bool) -> Option<Resolved> {
    let stroke = stroke_of(b, mac)?;
    // The Mac's keys go to gpui as they're written in the table.
    let keystroke = if mac { b.mac?.to_string() } else { stroke.keystroke() };
    let context = match (mac, &stroke) {
        (true, _) => b.context.map(str::to_string),
        // Ctrl+letter is what a shell reads (Ctrl+C interrupts, Ctrl+L clears, Ctrl+K and Ctrl+W
        // edit the line…): while the terminal panel has focus those keys are its, not Trek's.
        (false, s) if s.is_plain_ctrl_letter() => Some(match b.context {
            Some(c) => format!("({c}) && !Terminal"),
            None => "!Terminal".to_string(),
        }),
        (false, _) => b.context.map(str::to_string),
    };
    Some(Resolved { keystroke, context })
}

/// The bindings of `group` for the platform running.
pub fn bindings(group: Group) -> Vec<KeyBinding> {
    bindings_for(group, is_mac())
}

/// The bindings of `group` on a Mac (`mac`) or Windows.
pub fn bindings_for(group: Group, mac: bool) -> Vec<KeyBinding> {
    TABLE
        .iter()
        .filter(|b| b.group == group)
        .filter_map(|b| {
            let r = resolve(b, mac)?;
            Some((b.build?)(&r.keystroke, r.context.as_deref()))
        })
        .collect()
}

fn row_of(id: Id) -> Option<&'static Binding> {
    TABLE.iter().find(|b| b.id == id)
}

/// The keystroke that triggers `id` here, as gpui reads it: what a test presses.
#[cfg(test)]
pub fn keystroke(id: Id) -> String {
    row_of(id).and_then(|b| resolve(b, is_mac())).map(|r| r.keystroke).unwrap_or_else(|| panic!("{id:?} has no keys here"))
}

/// The keys `id` has on the platform running, as text (`⌘N`, `Ctrl+N`); empty where it has none.
pub fn hint(id: Id) -> String {
    hint_for(id, is_mac())
}

pub fn hint_for(id: Id, mac: bool) -> String {
    row_of(id).and_then(|b| stroke_of(b, mac)).map(|s| s.label(mac)).unwrap_or_default()
}

// ───────────────────────────── glyphs in sentences ─────────────────────────────

fn is_modifier_glyph(c: char) -> bool {
    matches!(c, '⌃' | '⌥' | '⇧' | '⌘')
}

fn is_key_glyph(c: char) -> bool {
    matches!(c, '↩' | '↵' | '⌫' | '⎋' | '⇥' | '␣' | '⌦')
}

fn is_glyph(c: char) -> bool {
    is_modifier_glyph(c) || is_key_glyph(c)
}

/// Whether `text` has a Mac key glyph in it.
pub fn has_glyphs(text: &str) -> bool {
    text.chars().any(is_glyph)
}

/// `text` with every run of key glyphs (`⌥⌘E`, `⌘⇧↩`, a lone `↩`, `⌘` in "⌘-click") written for
/// Windows (`Alt+Shift+E`, `Ctrl+Shift+Enter`, `Enter`, `Ctrl`); on a Mac, as it is. Arrows alone
/// (`↑↓ to move`) stay: every keyboard has them.
pub fn localize(text: &str) -> Cow<'_, str> {
    localize_for(text, is_mac())
}

/// `localize` as a `SharedString`, for the strings gpui keeps (tooltips, labels).
pub fn shared(text: &str) -> gpui_kit::SharedString {
    gpui_kit::SharedString::from(localize(text).into_owned())
}

pub fn localize_for(text: &str, mac: bool) -> Cow<'_, str> {
    if mac || !has_glyphs(text) {
        return Cow::Borrowed(text);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        if !is_glyph(chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut stroke = Stroke::default();
        let mut modifiers = 0;
        while i < chars.len() && is_modifier_glyph(chars[i]) {
            match chars[i] {
                '⌃' => stroke.ctrl = true,
                '⌥' => stroke.alt = true,
                '⇧' => stroke.shift = true,
                _ => stroke.cmd = true,
            }
            modifiers += 1;
            i += 1;
        }
        let next = chars.get(i).copied();
        let after = chars.get(i + 1).copied();
        let key: Option<String> = match next {
            Some('↩' | '↵') => Some("enter".into()),
            Some('⌫') => Some("backspace".into()),
            Some('⎋') => Some("escape".into()),
            Some('⇥') => Some("tab".into()),
            Some('␣') => Some("space".into()),
            Some('⌦') => Some("delete".into()),
            Some('↑') if modifiers > 0 => Some("up".into()),
            Some('↓') if modifiers > 0 => Some("down".into()),
            Some('←') if modifiers > 0 => Some("left".into()),
            Some('→') if modifiers > 0 => Some("right".into()),
            // One letter or digit, not the start of a word.
            Some(c) if modifiers > 0 && c.is_ascii_alphanumeric() && !after.is_some_and(char::is_alphanumeric) => Some(c.to_ascii_lowercase().to_string()),
            Some(c) if modifiers > 0 && matches!(c, ',' | '.' | '`' | '/' | ';' | '\'' | '[' | ']' | '=') => Some(c.to_string()),
            _ => None,
        };
        match key {
            Some(key) => {
                i += 1;
                if modifiers == 0 {
                    out.push_str(&windows_key(&key));
                } else {
                    stroke.key = key;
                    out.push_str(&on_windows(&stroke).label(false));
                }
            }
            None => out.push_str(&windows_modifiers_alone(&stroke)),
        }
    }
    Cow::Owned(out)
}

/// A modifier with no key after it ("⌘-click", "hold ⌥"): Ctrl, Alt…
fn windows_modifiers_alone(stroke: &Stroke) -> String {
    stroke.ctrl_for_cmd().windows_modifiers().join("+")
}

/// What Windows has for the Mac keystroke `mac`: the row's own keys where Trek binds it, else the
/// same with Ctrl for ⌘.
fn on_windows(mac: &Stroke) -> Stroke {
    TABLE
        .iter()
        .filter(|b| b.mac.and_then(|m| Stroke::parse(m, true)).as_ref() == Some(mac))
        .find_map(windows_stroke)
        .unwrap_or_else(|| mac.ctrl_for_cmd())
}

// ───────────────────────────── the Keyboard Shortcuts page ─────────────────────────────

/// What the page draws for one shortcut: a key cap or a word between key caps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chip {
    Key(String),
    Word(&'static str),
}

impl Chip {
    fn keys(chips: Vec<String>) -> Vec<Chip> {
        chips.into_iter().map(Chip::Key).collect()
    }
}

/// Where a row of the page gets its keys.
enum Show {
    /// A binding in the table.
    Bound(Id),
    /// Keys that aren't Trek's to bind (the clipboard's, the input's Enter), written as a keystroke.
    Keys(&'static str),
    /// One key cap with this text on both.
    Cap(&'static str),
    /// Next / previous change: ⌥⌘↓ ↑ on a Mac.
    Hunks,
}

use Show::{Bound, Cap, Hunks};
const PAGE: &[(&str, &[(&str, Show)])] = &[
    (
        "Threads",
        &[
            ("Search threads and commands", Bound(Id::OpenPalette)),
            ("New thread", Bound(Id::NewThread)),
            ("Open a folder", Bound(Id::OpenFolder)),
            ("Open the thread in a new window", Bound(Id::OpenInNewWindow)),
            ("Settle the current thread", Bound(Id::SettleThread)),
            ("Stop the agent", Bound(Id::Interrupt)),
        ],
    ),
    (
        "Composer",
        &[
            // "Send" and "New line" swap their keys with the setting (`shortcut_groups`).
            ("Send", Show::Keys("enter")),
            ("New line", Show::Keys("shift-enter")),
            ("Plan mode", Bound(Id::TogglePlan)),
            ("Cycle hand-holding", Bound(Id::CycleHandHolding)),
            ("Commands", Cap("/")),
            ("Mention a file", Cap("@")),
            ("Use a skill", Cap("$")),
            ("Attach a copied image", Show::Keys("secondary-v")),
            ("Take a snapshot", Bound(Id::TakeSnapshot)),
        ],
    ),
    (
        "Editor",
        &[
            ("Go to file", Bound(Id::QuickOpen)),
            ("Edit the picked lines", Bound(Id::InlineEdit)),
            ("Keep a change", Bound(Id::KeepHunk)),
            ("Undo a change", Bound(Id::UndoHunk)),
            ("Next or previous change", Hunks),
            ("Keep all changes", Show::Keys("secondary-enter")),
            ("Undo all changes (press twice)", Bound(Id::UndoAllOrStop)),
            ("New chat", Bound(Id::NewThread)),
            ("Switch to Agents", Bound(Id::SwitchMode)),
        ],
    ),
    (
        "Window",
        &[
            ("Basecamp", Bound(Id::OpenBasecamp)),
            ("Leave Basecamp", Bound(Id::LeaveBasecamp)),
            ("Toggle the sidebar", Bound(Id::ToggleSidebar)),
            ("Toggle the tools panel", Bound(Id::ToggleRightPanel)),
            ("Settings", Bound(Id::OpenSettings)),
            ("Close a thread window", Bound(Id::CloseWindow)),
            ("Hide Trek", Bound(Id::HideApp)),
            ("Minimize", Bound(Id::Minimize)),
            ("Quit", Bound(Id::Quit)),
        ],
    ),
];

/// The Keyboard Shortcuts page: groups of (what it does, its keys). `send_with_secondary_enter`
/// is the setting that swaps Send and New line. Shortcuts that don't exist on the platform (Hide
/// on Windows) are left out.
pub fn shortcut_groups(mac: bool, send_with_secondary_enter: bool) -> Vec<(&'static str, Vec<(&'static str, Vec<Chip>)>)> {
    let chips_of = |keystroke: &str| Stroke::parse(keystroke, mac).map(|s| Chip::keys(s.chips(mac))).unwrap_or_default();
    PAGE.iter()
        .map(|(group, rows)| {
            let rows = rows
                .iter()
                .filter_map(|(name, show)| {
                    let chips = match (*name, show) {
                        ("Send", _) => chips_of(if send_with_secondary_enter { "secondary-enter" } else { "enter" }),
                        ("New line", _) => chips_of(if send_with_secondary_enter { "enter" } else { "shift-enter" }),
                        (_, Show::Bound(id)) => Chip::keys(stroke_of(row_of(*id)?, mac)?.chips(mac)),
                        (_, Show::Keys(keystroke)) => chips_of(keystroke),
                        (_, Show::Cap(text)) => vec![Chip::Key((*text).to_string())],
                        (_, Show::Hunks) if mac => {
                            let mut chips = Chip::keys(stroke_of(row_of(Id::NextHunk)?, mac)?.chips(mac));
                            chips.pop();
                            chips.push(Chip::Key("↓ ↑".into()));
                            chips
                        }
                        (_, Show::Hunks) => {
                            let mut chips = Chip::keys(stroke_of(row_of(Id::NextHunk)?, mac)?.chips(mac));
                            chips.push(Chip::Word("or"));
                            chips.extend(Chip::keys(stroke_of(row_of(Id::PreviousHunk)?, mac)?.chips(mac)));
                            chips
                        }
                    };
                    Some((*name, chips))
                })
                .collect();
            (*group, rows)
        })
        .collect()
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn both() -> [bool; 2] {
        [true, false]
    }

    #[test]
    fn no_keystroke_is_bound_twice_in_one_context_on_either_platform() {
        for mac in both() {
            let mut seen: HashMap<(String, Option<String>), Id> = HashMap::new();
            for b in TABLE.iter().filter(|b| b.build.is_some()) {
                let Some(r) = resolve(b, mac) else { continue };
                let parsed = Stroke::parse(&r.keystroke, mac).unwrap_or_else(|| panic!("{:?}: {} doesn't parse", b.id, r.keystroke));
                if let Some(other) = seen.insert((parsed.keystroke(), r.context.clone()), b.id) {
                    panic!("{:?} and {:?} are both {} in {:?} (mac: {mac})", other, b.id, r.keystroke, r.context);
                }
            }
        }
    }

    #[test]
    fn every_action_has_keys_on_both_platforms_unless_dropped_there() {
        for b in TABLE.iter() {
            match (b.mac, b.windows) {
                (Some(_), Win::Dropped) => assert!(resolve(b, true).is_some() && resolve(b, false).is_none(), "{:?}", b.id),
                (Some(_), _) => assert!(resolve(b, true).is_some() && resolve(b, false).is_some(), "{:?}", b.id),
                (None, Win::Keys(_)) => assert!(resolve(b, false).is_some(), "{:?}", b.id),
                (None, _) => panic!("{:?} has no keys on either platform", b.id),
            }
            if let Some(mac) = b.mac {
                assert!(Stroke::parse(mac, true).is_some(), "{:?}: {mac}", b.id);
            }
        }
        // One row per id, so a hint asks for exactly one thing.
        let mut ids: Vec<_> = TABLE.iter().map(|b| format!("{:?}", b.id)).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "an id is in the table twice");
    }

    #[test]
    fn what_the_mac_binds_is_what_it_always_has() {
        let mac: Vec<String> = TABLE.iter().filter(|b| b.build.is_some()).filter_map(|b| resolve(b, true)).map(|r| format!("{}|{}", r.keystroke, r.context.unwrap_or_default())).collect();
        // The 37 in main.rs, the 13 in notes.rs and the 1 in editor.rs.
        assert_eq!(mac.len(), 37 + 13 + 1);
        for (keys, ctx) in [("cmd-q", ""), ("alt-cmd-e", ""), ("cmd-k", "!IdeEditor"), ("cmd-k", "IdeEditor"), ("alt-cmd-backspace", "IdeEditor && hunks"), ("ctrl-`", ""), ("cmd-shift-.", "NoteEditor"), ("cmd-s", "TrekEditor")] {
            let want = format!("{keys}|{ctx}");
            assert!(mac.contains(&want), "{want}");
        }
    }

    #[test]
    fn rows_that_share_a_mac_keystroke_share_their_windows_one() {
        // `localize` finds a Windows spelling by the Mac's keystroke alone.
        let mut seen: HashMap<String, Option<String>> = HashMap::new();
        for b in TABLE.iter() {
            let Some(mac) = b.mac.and_then(|m| Stroke::parse(m, true)) else { continue };
            let win = windows_stroke(b).map(|s| s.keystroke());
            if let Some(before) = seen.insert(mac.keystroke(), win.clone()) {
                assert_eq!(before, win, "{:?}", b.id);
            }
        }
    }

    #[test]
    fn windows_keys_dodge_what_windows_and_the_shell_keep() {
        for b in TABLE.iter() {
            let Some(s) = windows_stroke(b) else { continue };
            // Ctrl+Alt is AltGr, and the Windows key is Windows'.
            assert!(!(s.ctrl && s.alt), "{:?} is Ctrl+Alt: AltGr types characters on many layouts", b.id);
            assert!(!s.cmd, "{:?} uses the Windows key", b.id);
            // The system's.
            for taken in [("alt", "f4"), ("alt", "space"), ("alt", "tab"), ("alt", "enter"), ("ctrl", "escape")] {
                let held = if taken.0 == "alt" { s.alt && !s.ctrl && !s.shift } else { s.ctrl && !s.alt && !s.shift };
                assert!(!(held && s.key == taken.1), "{:?} is {taken:?}", b.id);
            }
            // The terminal panel's: paste.
            assert!(!(s.ctrl && s.shift && !s.alt && matches!(s.key.as_str(), "v" | "c")), "{:?} is the terminal's copy/paste", b.id);
            assert!(!(s.shift && s.key == "insert" && !s.ctrl && !s.alt), "{:?}", b.id);
            // Shell keys (Ctrl+C, Ctrl+L, Ctrl+K…) are left to the shell while the terminal has focus.
            if s.is_plain_ctrl_letter() {
                let r = resolve(b, false).unwrap();
                assert!(r.context.is_some_and(|c| c.ends_with("!Terminal")), "{:?} would take {} from the shell", b.id, r.keystroke);
            }
        }
    }

    #[test]
    fn windows_remaps_are_exactly_the_listed_ones() {
        let different: Vec<(Id, String)> = TABLE
            .iter()
            .filter_map(|b| {
                let mac = Stroke::parse(b.mac?, true)?;
                let win = windows_stroke(b)?;
                (win != mac.ctrl_for_cmd()).then(|| (b.id, win.keystroke()))
            })
            .collect();
        let want = [
            (Id::SwitchMode, "alt-shift-e"),
            (Id::ToggleAiBar, "alt-shift-b"),
            (Id::KeepHunk, "ctrl-shift-y"),
            (Id::UndoHunk, "alt-shift-backspace"),
            (Id::NextHunk, "alt-f5"),
            (Id::PreviousHunk, "alt-shift-f5"),
            (Id::NoteHeading1, "alt-shift-1"),
            (Id::NoteHeading2, "alt-shift-2"),
            (Id::Redo, "ctrl-y"),
        ];
        assert_eq!(different, want.iter().map(|(i, k)| (*i, k.to_string())).collect::<Vec<_>>());
    }

    #[test]
    fn terminal_gets_the_shell_keys_but_trek_keeps_its_own() {
        let ctx = |id: Id| resolve(row_of(id).unwrap(), false).unwrap().context;
        assert_eq!(ctx(Id::OpenPalette).as_deref(), Some("(!IdeEditor) && !Terminal"), "Ctrl+K is the shell's in the terminal");
        assert_eq!(ctx(Id::NewThread).as_deref(), Some("!Terminal"));
        assert_eq!(ctx(Id::ToggleTerminal), None, "Ctrl+` must reach Trek from inside the terminal");
        assert_eq!(ctx(Id::OpenPaletteAnywhere), None, "the palette is still there, in the terminal and the editor's text too");
        assert_eq!(ctx(Id::NextTab), None);
        assert_eq!(ctx(Id::FocusScm), None);
        // The Mac has no such thing: there ⌘ is never the shell's.
        assert_eq!(resolve(row_of(Id::NewThread).unwrap(), true).unwrap().context, None);
    }

    #[test]
    fn labels_on_a_mac() {
        for (keys, label) in [
            ("cmd-shift-k", "⌘⇧K"),
            ("secondary-k", "⌘K"),
            ("alt-cmd-e", "⌥⌘E"),
            ("cmd-shift-enter", "⌘⇧↩"),
            ("cmd-shift-backspace", "⌘⇧⌫"),
            ("alt-cmd-backspace", "⌥⌘⌫"),
            ("ctrl-shift-g", "⌃⇧G"),
            ("shift-tab", "⇧⇥"),
            ("cmd-,", "⌘,"),
            ("cmd-.", "⌘."),
            ("cmd-1", "⌘1"),
            ("escape", "⎋"),
            ("cmd-up", "⌘↑"),
            ("alt-f5", "⌥F5"),
            ("space", "␣"),
        ] {
            assert_eq!(label_for(keys, true), label, "{keys}");
        }
    }

    #[test]
    fn labels_on_windows() {
        for (keys, label) in [
            ("secondary-shift-k", "Ctrl+Shift+K"),
            ("ctrl-shift-k", "Ctrl+Shift+K"),
            ("alt-shift-e", "Alt+Shift+E"),
            ("ctrl-alt-shift-k", "Ctrl+Alt+Shift+K"),
            ("cmd-k", "Win+K"),
            ("ctrl-shift-enter", "Ctrl+Shift+Enter"),
            ("ctrl-shift-backspace", "Ctrl+Shift+Backspace"),
            ("shift-tab", "Shift+Tab"),
            ("ctrl-tab", "Ctrl+Tab"),
            ("ctrl-,", "Ctrl+,"),
            ("ctrl-.", "Ctrl+."),
            ("ctrl-`", "Ctrl+`"),
            ("escape", "Esc"),
            ("ctrl-up", "Ctrl+Up"),
            ("ctrl-down", "Ctrl+Down"),
            ("ctrl-left", "Ctrl+Left"),
            ("ctrl-right", "Ctrl+Right"),
            ("space", "Space"),
            ("alt-f5", "Alt+F5"),
            ("ctrl-1", "Ctrl+1"),
            ("Ctrl-K", "Ctrl+Shift+K"),
        ] {
            assert_eq!(label_for(keys, false), label, "{keys}");
        }
        assert_eq!(label_for("secondary-k", false), "Ctrl+K");
    }

    #[test]
    fn a_keystroke_a_window_got_is_labelled_the_same() {
        let k = Keystroke::parse("ctrl-shift-k").unwrap();
        with_mac(false, || assert_eq!(label_keystroke(&k), "Ctrl+Shift+K"));
        let k = Keystroke::parse("cmd-shift-k").unwrap();
        with_mac(true, || assert_eq!(label_keystroke(&k), "⌘⇧K"));
    }

    #[test]
    fn hints_come_from_the_table() {
        assert_eq!(hint_for(Id::NewThread, true), "⌘N");
        assert_eq!(hint_for(Id::NewThread, false), "Ctrl+N");
        assert_eq!(hint_for(Id::SwitchMode, true), "⌥⌘E");
        assert_eq!(hint_for(Id::SwitchMode, false), "Alt+Shift+E");
        assert_eq!(hint_for(Id::OpenBasecamp, true), "⌘⇧H");
        assert_eq!(hint_for(Id::OpenBasecamp, false), "Ctrl+Shift+H");
        assert_eq!(hint_for(Id::OpenSettings, true), "⌘,");
        assert_eq!(hint_for(Id::HideApp, true), "⌘H");
        assert_eq!(hint_for(Id::HideApp, false), "", "Windows has no Hide");
    }

    #[test]
    fn on_a_mac_text_is_left_alone() {
        for text in ["Stop ⌘. · x", "⌥⌘E", "↩ sends; ⇧↩ adds a new line.", "⌘-click", "hold ⌥"] {
            assert!(matches!(localize_for(text, true), Cow::Borrowed(t) if t == text));
        }
    }

    #[test]
    fn sentences_from_the_app_on_windows() {
        for (mac, windows) in [
            ("Agents (⌥⌘E)", "Agents (Alt+Shift+E)"),
            ("AI side bar (⌥⌘B)", "AI side bar (Alt+Shift+B)"),
            ("Settings (⌘,)", "Settings (Ctrl+,)"),
            ("Open in new window (⌘⇧↩)", "Open in new window (Ctrl+Shift+Enter)"),
            ("Source Control (⌃⇧G)", "Source Control (Ctrl+Shift+G)"),
            ("Stop ⌘. · queued", "Stop Ctrl+. · queued"),
            ("Plan mode: the agent plans before it changes anything (⇧⇥)", "Plan mode: the agent plans before it changes anything (Shift+Tab)"),
            ("Steer the running turn · ⌥↩ queues for after it", "Steer the running turn · Alt+Enter queues for after it"),
            ("↩ adds a new line; ⌘↩ sends.", "Enter adds a new line; Ctrl+Enter sends."),
            ("↩ sends; ⇧↩ adds a new line.", "Enter sends; Shift+Enter adds a new line."),
            ("Screenshots you attach from the composer’s + menu or with ⌘⇧S.", "Screenshots you attach from the composer’s + menu or with Ctrl+Shift+S."),
            ("↑↓ to move · ↩ to pick · esc", "↑↓ to move · Enter to pick · esc"),
            ("Keep / Undo a Change ⌘Y / ⌥⌘⌫", "Keep / Undo a Change Ctrl+Shift+Y / Alt+Shift+Backspace"),
            ("Redo (⇧⌘Z)", "Redo (Ctrl+Y)"),
            ("Heading (⌘⌥1)", "Heading (Alt+Shift+1)"),
            ("Undo all (⌘⇧⌫, twice)", "Undo all (Ctrl+Shift+Backspace, twice)"),
            ("⌘1–9", "Ctrl+1–9"),
            ("Jump ⌘1–⌘9", "Jump Ctrl+1–Ctrl+9"),
            ("⌘-click a link, or hold ⌥", "Ctrl-click a link, or hold Alt"),
            ("Message (⌘↩ to commit)", "Message (Ctrl+Enter to commit)"),
            ("Bold (⌘B)", "Bold (Ctrl+B)"),
            ("Copy image (⌘C)", "Copy image (Ctrl+C)"),
            ("Next (⌥⌘↓)", "Next (Alt+F5)"),
            ("no glyphs here", "no glyphs here"),
        ] {
            assert_eq!(localize_for(mac, false), windows, "{mac}");
        }
    }

    #[test]
    fn the_shortcuts_page_on_a_mac_is_what_it_was() {
        let page = shortcut_groups(true, false);
        let flat = |rows: &Vec<(&str, Vec<Chip>)>| -> Vec<String> {
            rows.iter()
                .map(|(name, chips)| {
                    let keys: Vec<&str> = chips.iter().map(|c| if let Chip::Key(k) = c { k.as_str() } else { "?" }).collect();
                    format!("{name}: {}", keys.join(" "))
                })
                .collect()
        };
        assert_eq!(page.iter().map(|(g, _)| *g).collect::<Vec<_>>(), ["Threads", "Composer", "Editor", "Window"]);
        assert_eq!(
            flat(&page[0].1),
            [
                "Search threads and commands: ⌘ K",
                "New thread: ⌘ N",
                "Open a folder: ⌘ O",
                "Open the thread in a new window: ⌘ ⇧ ↩",
                "Settle the current thread: ⌘ E",
                "Stop the agent: ⌘ .",
            ]
        );
        assert_eq!(
            flat(&page[1].1),
            [
                "Send: ↩",
                "New line: ⇧ ↩",
                "Plan mode: ⇧ ⇥",
                "Cycle hand-holding: ⌘ ⇧ A",
                "Commands: /",
                "Mention a file: @",
                "Use a skill: $",
                "Attach a copied image: ⌘ V",
                "Take a snapshot: ⌘ ⇧ S",
            ]
        );
        assert_eq!(
            flat(&page[2].1),
            [
                "Go to file: ⌘ P",
                "Edit the picked lines: ⌘ K",
                "Keep a change: ⌘ Y",
                "Undo a change: ⌥ ⌘ ⌫",
                "Next or previous change: ⌥ ⌘ ↓ ↑",
                "Keep all changes: ⌘ ↩",
                "Undo all changes (press twice): ⌘ ⇧ ⌫",
                "New chat: ⌘ N",
                "Switch to Agents: ⌥ ⌘ E",
            ]
        );
        assert_eq!(
            flat(&page[3].1),
            [
                "Basecamp: ⌘ ⇧ H",
                "Leave Basecamp: esc",
                "Toggle the sidebar: ⌘ B",
                "Toggle the tools panel: ⌘ J",
                "Settings: ⌘ ,",
                "Close a thread window: ⌘ W",
                "Hide Trek: ⌘ H",
                "Minimize: ⌘ M",
                "Quit: ⌘ Q",
            ]
        );
        let swapped = shortcut_groups(true, true);
        assert_eq!(flat(&swapped[1].1)[..2], ["Send: ⌘ ↩", "New line: ↩"]);
    }

    #[test]
    fn the_shortcuts_page_on_windows_has_its_own_keys() {
        let page = shortcut_groups(false, false);
        let keys = |group: usize, name: &str| -> Vec<Chip> { page[group].1.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("{name}")).1.clone() };
        let k = |s: &[&str]| s.iter().map(|s| Chip::Key(s.to_string())).collect::<Vec<_>>();
        assert_eq!(keys(0, "Search threads and commands"), k(&["Ctrl", "K"]));
        assert_eq!(keys(0, "Open the thread in a new window"), k(&["Ctrl", "Shift", "Enter"]));
        assert_eq!(keys(1, "New line"), k(&["Shift", "Enter"]));
        assert_eq!(keys(1, "Plan mode"), k(&["Shift", "Tab"]));
        assert_eq!(keys(2, "Switch to Agents"), k(&["Alt", "Shift", "E"]));
        assert_eq!(keys(2, "Undo a change"), k(&["Alt", "Shift", "Backspace"]));
        assert_eq!(
            keys(2, "Next or previous change"),
            [Chip::Key("Alt".into()), Chip::Key("F5".into()), Chip::Word("or"), Chip::Key("Alt".into()), Chip::Key("Shift".into()), Chip::Key("F5".into())]
        );
        assert_eq!(keys(3, "Leave Basecamp"), k(&["Esc"]));
        let window: Vec<&str> = page[3].1.iter().map(|(n, _)| *n).collect();
        assert!(!window.contains(&"Hide Trek") && !window.contains(&"Minimize"), "{window:?}");
        assert!(window.contains(&"Quit"));
        let swapped = shortcut_groups(false, true);
        assert_eq!(swapped[1].1[0].1, k(&["Ctrl", "Enter"]));
    }

    /// Trek's own tables of bindings are built (and parse) on both platforms.
    #[test]
    fn both_platforms_bindings_build() {
        for mac in both() {
            let n: usize = [Group::App, Group::Notes, Group::Editor].into_iter().map(|g| bindings_for(g, mac).len()).sum();
            assert_eq!(n, if mac { 51 } else { 51 - 2 + 1 }, "mac: {mac}");
        }
    }

    /// The editor's title bar, which `wp/titlebar-menus` is changing: its literals are localized when
    /// that merges (`"Agents (⌥⌘E)"` and `"⌘P"`; the toggles' tooltips reach `icon_button`).
    const TITLE_BAR: [(&str, &str); 5] = [
        ("root.rs", "Tooltip::new(if m == Mode::Agents"),
        ("root.rs", ".child(\"⌘P\")"),
        ("root.rs", "toggle(\"toggle-primary\""),
        ("root.rs", "toggle(\"toggle-panel\""),
        ("root.rs", "toggle(\"toggle-ai\""),
    ];

    /// No glyph reaches the screen without passing `keys`: every line of the app's own code that
    /// has a key glyph in it (outside comments, tests and this file) says `keys::` too, or in a
    /// comment on the line, where the string is localized further down.
    #[test]
    fn no_key_glyph_is_shown_without_being_localized() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n != "tests") {
                        walk(&path, out);
                    }
                } else if path.extension().is_some_and(|e| e == "rs") && path.file_name().is_some_and(|n| n != "keys.rs") {
                    out.push(path);
                }
            }
        }
        let mut files = vec![];
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        assert!(files.len() > 50, "found {} files", files.len());
        let mut bare = vec![];
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            let mut localized_where_used = false;
            for (n, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("#[cfg(test)]") && text.lines().nth(n + 1).is_some_and(|next| next.trim_start().starts_with("mod ")) {
                    break; // the test module closes the file
                }
                // A table of texts that are localized where they're shown says so around it.
                match line.trim() {
                    "// keys: localized where used" => localized_where_used = true,
                    "// keys: end" => localized_where_used = false,
                    _ => {}
                }
                if localized_where_used {
                    continue;
                }
                let code = match line.find("//") {
                    Some(at) if line[..at].matches('"').count() % 2 == 0 => &line[..at],
                    _ => line,
                };
                // A line that builds a string for `keys::localize` further down says so in a comment;
                // `ui::icon_button` localizes its tooltip itself.
                if has_glyphs(code) && !line.contains("keys::") && !line.contains("icon_button(") && !TITLE_BAR.iter().any(|(f, s)| path.ends_with(f) && line.contains(s)) {
                    bare.push(format!("{}:{}: {}", path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().display(), n + 1, line.trim()));
                }
            }
        }
        assert!(bare.is_empty(), "key glyphs shown as they are (wrap them in keys::localize):\n{}", bare.join("\n"));
    }
}
