//! Every keyboard shortcut Trek has, in one table, and how each is written for the person using
//! it: ⌘⇧K on a Mac, Ctrl+Shift+K on Windows.
//!
//! `TABLE` holds every binding (what `main.rs`, `notes.rs` and `editor.rs` bind), with the keys
//! each platform has it on. Where Windows differs, the row says why. Everything that shows a
//! shortcut draws on it: the Keyboard Shortcuts page (`page`), the palette's hints
//! (`hint`) and, for the tooltips and sentences written with Mac glyphs, `localize`, which turns
//! a glyph run like `⌥⌘E` into the keys the Windows build really binds (Alt+Shift+E), not just
//! the same letters with other modifiers.
//!
//! The Mac side is what Trek has always had, byte for byte; the Windows side is the same keys
//! with Ctrl where the Mac has ⌘, except where that would collide with Windows or with the shell
//! in the terminal panel (see the rows).
//!
//! The table is the defaults. The user can change any binding on the Keyboard Shortcuts page:
//! their changes (`Settings::keys`, only what differs from the table, per platform) are laid over
//! it here (`set_changes`), so the keymap (`rebind`), the menus, the hints and the sentences all
//! show what they chose. A Ctrl+letter they choose on Windows is still the shell's in the
//! terminal panel, as the table's are.

use gpui_kit::KeyBinding;
use std::borrow::Cow;
use std::sync::LazyLock;

/// Whether this build is shown the Mac way. Tests can say otherwise for one closure
/// (`with_mac`), so both spellings are checked on whichever machine runs them.
pub fn is_mac() -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(mac) = FORCE_MAC.with(|f| f.get()) {
        return mac;
    }
    cfg!(target_os = "macos")
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static FORCE_MAC: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Run `f` as if on a Mac (`true`) or on Windows (`false`): everything here that asks `is_mac`.
#[cfg(any(test, feature = "test-support"))]
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

    /// The keystroke as gpui parses it (`ctrl-shift-k`).
    fn keystroke(&self) -> String {
        let mut s = String::new();
        for (on, name) in [
            (self.ctrl, "ctrl-"),
            (self.alt, "alt-"),
            (self.shift, "shift-"),
            (self.cmd, "cmd-"),
        ] {
            if on {
                s.push_str(name);
            }
        }
        s + &self.key
    }

    /// The same keys on Windows when nothing says otherwise: Ctrl where the Mac has ⌘.
    fn ctrl_for_cmd(&self) -> Stroke {
        Stroke {
            ctrl: self.ctrl || self.cmd,
            cmd: false,
            ..self.clone()
        }
    }

    /// A Ctrl+letter with nothing else held: the keys a shell uses (readline's, and the terminal's
    /// own Ctrl+C / Ctrl+L / Ctrl+V). Ctrl+\ too, a shell's other way to stop what's running.
    fn is_plain_ctrl_letter(&self) -> bool {
        self.ctrl
            && !self.alt
            && !self.shift
            && !self.cmd
            && self.key.len() == 1
            && (self.key.as_bytes()[0].is_ascii_lowercase() || self.key == "\\")
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
        for (on, glyph) in [
            (self.ctrl, '⌃'),
            (self.alt, '⌥'),
            (self.cmd, '⌘'),
            (self.shift, '⇧'),
        ] {
            if on {
                s.push(glyph);
            }
        }
        s
    }

    fn windows_modifiers(&self) -> Vec<String> {
        let mut parts = vec![];
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.cmd, "Win"),
        ] {
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
            chips.push(if self.key == "escape" {
                "esc".into()
            } else {
                mac_key(&self.key)
            });
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
    OpenBeside,
    CopyThreadLink,
    CopyThreadMarkdown,
    CopyThreadFolder,
    NextNeedsYou,
    BackToLastPrompt,
    OpenBasecamp,
    LeaveBasecamp,
    OpenNotes,
    OpenStudy,
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
    /// Windows' other way to close a tab, as in every editor there.
    CloseTabCtrlF4,
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
    FormatDocument,
    DiffNextLine,
    DiffPreviousLine,
    DiffDown,
    DiffUp,
    DiffExtendDown,
    DiffExtendUp,
    DiffExtendArrowDown,
    DiffExtendArrowUp,
    DiffNextHunk,
    DiffPreviousHunk,
    DiffNextFile,
    DiffPreviousFile,
    DiffViewed,
    DiffStage,
    DiffComment,
    DiffSend,
    DiffClear,
    /// The text fields' own redo (the kit binds it): ⇧⌘Z on a Mac, Ctrl+Y on Windows.
    Redo,
    /// The whole interface larger, smaller, as designed (`zoom`).
    ZoomIn,
    /// ⌘+ as typed with Shift (⌘= is the same key without it).
    ZoomInShift,
    ZoomOut,
    ResetZoom,
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

fn row(
    id: Id,
    group: Group,
    mac: &'static str,
    windows: Win,
    context: Option<&'static str>,
    build: Option<Build>,
) -> Binding {
    Binding {
        id,
        group,
        mac: Some(mac),
        windows,
        context,
        build,
    }
}

static TABLE: LazyLock<Vec<Binding>> = LazyLock::new(|| {
    use Group::*;
    use Id::*;
    use Win::{Dropped, Keys, Same};
    const NOTES: Option<&str> = Some(crate::notes::CONTEXT);
    const DIFF: Option<&str> = Some(crate::diff_review::KEY_CONTEXT);
    vec![
        row(Quit, App, "cmd-q", Same, None, act!(crate::Quit)),
        // No such thing as hiding an app on Windows.
        row(HideApp, App, "cmd-h", Dropped, None, act!(crate::HideApp)),
        // Win+Down minimizes there (the system's), and so does the title bar's button.
        row(Minimize, App, "cmd-m", Dropped, None, act!(crate::Minimize)),
        // ⌘N only ever makes something new: never an undo, whatever has focus.
        row(NewThread, App, "cmd-n", Same, None, act!(crate::NewThread)),
        row(
            OpenFolder,
            App,
            "cmd-o",
            Same,
            None,
            act!(crate::OpenFolder),
        ),
        row(
            OpenSettings,
            App,
            "cmd-,",
            Same,
            None,
            act!(crate::OpenSettings),
        ),
        row(
            ToggleSidebar,
            App,
            "cmd-b",
            Same,
            None,
            act!(crate::ToggleSidebar),
        ),
        row(
            SettleThread,
            App,
            "cmd-e",
            Same,
            None,
            act!(crate::SettleThread),
        ),
        row(
            TogglePlan,
            App,
            "shift-tab",
            Same,
            Some("Composer"),
            act!(crate::TogglePlan),
        ),
        row(
            TakeSnapshot,
            App,
            "cmd-shift-s",
            Same,
            None,
            act!(crate::TakeSnapshot),
        ),
        row(
            CycleHandHolding,
            App,
            "cmd-shift-a",
            Same,
            None,
            act!(crate::CycleHandHolding),
        ),
        row(Interrupt, App, "cmd-.", Same, None, act!(crate::Interrupt)),
        row(
            ToggleRightPanel,
            App,
            "cmd-j",
            Same,
            None,
            act!(crate::ToggleRightPanel),
        ),
        // Not in the editor's text, where ⌘K is an inline edit (below).
        row(
            OpenPalette,
            App,
            "cmd-k",
            Same,
            Some("!IdeEditor"),
            act!(crate::OpenPalette),
        ),
        // Ctrl+K belongs to the shell in the terminal panel (kill to end of line) and is the inline
        // edit in the editor's text, so the palette gets a second way in that works there too,
        // as in VS Code.
        Binding {
            id: OpenPaletteAnywhere,
            group: App,
            mac: None,
            windows: Keys("ctrl-shift-p"),
            context: None,
            build: act!(crate::OpenPalette),
        },
        row(
            OpenInNewWindow,
            App,
            "cmd-shift-enter",
            Same,
            None,
            act!(crate::OpenInNewWindow),
        ),
        // A thread beside the one on screen, or with a split open, the focus across it. Only the
        // main window has a split.
        row(
            OpenBeside,
            App,
            "cmd-\\",
            Same,
            Some("TrekWindow"),
            act!(crate::OpenBeside),
        ),
        // The thread on screen's copies (its "⋯" › Copy). Share… has no keys: ⇧⌘S is a snapshot.
        // Ctrl+Alt+L would be AltGr+L.
        row(
            CopyThreadLink,
            App,
            "alt-cmd-l",
            Keys("alt-shift-l"),
            None,
            act!(crate::CopyThreadLink),
        ),
        row(
            CopyThreadMarkdown,
            App,
            "cmd-shift-m",
            Same,
            None,
            act!(crate::CopyThreadMarkdown),
        ),
        // Ctrl+Shift+C is the terminal's copy on Windows: not taken from it.
        row(
            CopyThreadFolder,
            App,
            "cmd-shift-c",
            Dropped,
            None,
            act!(crate::CopyThreadFolder),
        ),
        // ⇧⌘U, the key rivals use for this, is Study here. I for "inbox", B for "back".
        row(
            NextNeedsYou,
            App,
            "cmd-shift-i",
            Same,
            None,
            act!(crate::NextNeedsYou),
        ),
        row(
            BackToLastPrompt,
            App,
            "cmd-shift-b",
            Same,
            None,
            act!(crate::BackToLastPrompt),
        ),
        row(
            OpenBasecamp,
            App,
            "cmd-shift-h",
            Same,
            None,
            act!(crate::OpenBasecamp),
        ),
        row(
            LeaveBasecamp,
            App,
            "escape",
            Same,
            Some("Basecamp"),
            act!(crate::basecamp::Leave),
        ),
        row(
            OpenNotes,
            App,
            "cmd-shift-j",
            Same,
            None,
            act!(crate::OpenNotes),
        ),
        row(
            OpenStudy,
            App,
            "cmd-shift-u",
            Same,
            None,
            act!(crate::OpenStudy),
        ),
        // ⌥⌘E switches Agents ⇄ Editor. On Windows Ctrl+Alt is AltGr, which types characters on
        // many layouts (German, French, Polish…), so ⌥⌘ chords become Alt+Shift.
        row(
            SwitchMode,
            App,
            "alt-cmd-e",
            Keys("alt-shift-e"),
            None,
            act!(crate::SwitchMode),
        ),
        // ⌘⇧E is the Explorer, in the editor only (as in VS Code).
        row(
            ToggleIde,
            App,
            "cmd-shift-e",
            Same,
            Some("TrekIde"),
            act!(crate::ToggleIde),
        ),
        row(
            ToggleIdeSearch,
            App,
            "cmd-shift-f",
            Same,
            Some("TrekWindow"),
            act!(crate::ToggleIdeSearch),
        ),
        row(QuickOpen, App, "cmd-p", Same, None, act!(crate::QuickOpen)),
        // Ctrl+Alt+B would be AltGr+B.
        row(
            ToggleAiBar,
            App,
            "alt-cmd-b",
            Keys("alt-shift-b"),
            None,
            act!(crate::ToggleAiBar),
        ),
        // Ctrl+` as in VS Code; no ⌘ in it, so it's the same on both and the terminal panel
        // (which has focus when it's open) lets it through.
        row(
            ToggleTerminal,
            App,
            "ctrl-`",
            Same,
            None,
            act!(crate::ToggleTerminal),
        ),
        // The same on both (VS Code's too).
        row(
            FocusScm,
            App,
            "ctrl-shift-g",
            Same,
            None,
            act!(crate::FocusScm),
        ),
        // The editor's selection to the AI side bar: ⌘⇧L the chat in front, ⌘L a new one.
        row(
            AddSelectionToChat,
            App,
            "cmd-shift-l",
            Same,
            Some("TrekIde"),
            act!(crate::AddSelectionToChat),
        ),
        row(
            AddSelectionToNewChat,
            App,
            "cmd-l",
            Same,
            Some("TrekIde"),
            act!(crate::AddSelectionToNewChat),
        ),
        // In the editor's text: ⌘K edits the picked lines inline (elsewhere it's the palette);
        // with a review's hunks in the file, ⌘Y keeps the one the bar is on and ⌥⌘⌫ undoes it
        // (its toast takes that back), ⌥⌘↑/↓ step between them.
        row(
            InlineEdit,
            App,
            "cmd-k",
            Same,
            Some("IdeEditor"),
            act!(crate::editor::InlineEdit),
        ),
        // Ctrl+Y is redo in Windows text fields, so Keep takes Ctrl+Shift+Y.
        row(
            KeepHunk,
            App,
            "cmd-y",
            Keys("ctrl-shift-y"),
            Some("IdeEditor && hunks"),
            act!(crate::editor::KeepHunk),
        ),
        // Ctrl+Alt+Backspace is AltGr+Backspace.
        row(
            UndoHunk,
            App,
            "alt-cmd-backspace",
            Keys("alt-shift-backspace"),
            Some("IdeEditor && hunks"),
            act!(crate::editor::UndoHunk),
        ),
        // Ctrl+Alt+Up/Down rotates the screen on some graphics drivers, and the text field's own
        // Alt+Shift+Up/Down add a cursor; Alt+F5 and Shift+Alt+F5 are VS Code's next/previous change.
        row(
            NextHunk,
            App,
            "alt-cmd-down",
            Keys("alt-f5"),
            Some("IdeEditor && hunks"),
            act!(crate::editor::NextHunk),
        ),
        row(
            PreviousHunk,
            App,
            "alt-cmd-up",
            Keys("alt-shift-f5"),
            Some("IdeEditor && hunks"),
            act!(crate::editor::PreviousHunk),
        ),
        // In the AI input: undo every pending change, or stop the turn (⌘↩, its pair, comes
        // through the input's Enter).
        row(
            UndoAllOrStop,
            App,
            "cmd-shift-backspace",
            Same,
            Some("AiInput"),
            act!(crate::ide::ai::UndoAllOrStop),
        ),
        // Only thread windows close with ⌘W; the main window stays put.
        row(
            CloseWindow,
            App,
            "cmd-w",
            Same,
            Some("ThreadWindow"),
            act!(crate::CloseWindow),
        ),
        // In the main window ⌘W closes the tab in front; the window stays put.
        row(
            CloseTab,
            App,
            "cmd-w",
            Same,
            Some("TrekWindow"),
            act!(crate::CloseTab),
        ),
        Binding {
            id: CloseTabCtrlF4,
            group: App,
            mac: None,
            windows: Keys("ctrl-f4"),
            context: Some("TrekWindow"),
            build: act!(crate::CloseTab),
        },
        // Standard on Windows too.
        row(NextTab, App, "ctrl-tab", Same, None, act!(crate::NextTab)),
        row(
            PreviousTab,
            App,
            "ctrl-shift-tab",
            Same,
            None,
            act!(crate::PreviousTab),
        ),
        // Notes.
        row(
            NoteBold,
            Notes,
            "cmd-b",
            Same,
            NOTES,
            act!(crate::notes::Bold),
        ),
        row(
            NoteItalic,
            Notes,
            "cmd-i",
            Same,
            NOTES,
            act!(crate::notes::Italic),
        ),
        row(
            NoteUnderline,
            Notes,
            "cmd-u",
            Same,
            NOTES,
            act!(crate::notes::Underline),
        ),
        row(
            NoteStrike,
            Notes,
            "cmd-shift-x",
            Same,
            NOTES,
            act!(crate::notes::Strike),
        ),
        row(
            NoteBullets,
            Notes,
            "cmd-shift-8",
            Same,
            NOTES,
            act!(crate::notes::Bullets),
        ),
        row(
            NoteNumbers,
            Notes,
            "cmd-shift-7",
            Same,
            NOTES,
            act!(crate::notes::Numbers),
        ),
        row(
            NoteChecklist,
            Notes,
            "cmd-shift-9",
            Same,
            NOTES,
            act!(crate::notes::Checklist),
        ),
        // Ctrl+Alt+1 is AltGr+1.
        row(
            NoteHeading1,
            Notes,
            "cmd-alt-1",
            Keys("alt-shift-1"),
            NOTES,
            act!(crate::notes::Heading1),
        ),
        row(
            NoteHeading2,
            Notes,
            "cmd-alt-2",
            Keys("alt-shift-2"),
            NOTES,
            act!(crate::notes::Heading2),
        ),
        row(
            NoteQuote,
            Notes,
            "cmd-shift-.",
            Same,
            NOTES,
            act!(crate::notes::Quote),
        ),
        row(
            NoteCode,
            Notes,
            "cmd-e",
            Same,
            NOTES,
            act!(crate::notes::Code),
        ),
        row(
            NoteToggleCheck,
            Notes,
            "cmd-enter",
            Same,
            NOTES,
            act!(crate::notes::ToggleCheck),
        ),
        row(
            NewNote,
            Notes,
            "cmd-n",
            Same,
            Some("Notes"),
            act!(crate::notes::NewNote),
        ),
        // The editor.
        row(
            SaveFile,
            Editor,
            "cmd-s",
            Same,
            Some("TrekEditor"),
            act!(crate::editor::SaveFile),
        ),
        // VS Code's: the same on both (no ⌘ or Ctrl in it).
        row(
            FormatDocument,
            Editor,
            "alt-shift-f",
            Same,
            Some("TrekEditor"),
            act!(crate::editor::FormatDocument),
        ),
        // A diff with the keyboard: its own while it has focus itself, never in a text field
        // (`diff_review`). The keys Orca's and cmux's diffs use, the same on both platforms.
        row(
            DiffNextLine,
            App,
            "j",
            Same,
            DIFF,
            act!(crate::diff_review::NextLine),
        ),
        row(
            DiffPreviousLine,
            App,
            "k",
            Same,
            DIFF,
            act!(crate::diff_review::PreviousLine),
        ),
        row(
            DiffDown,
            App,
            "down",
            Same,
            DIFF,
            act!(crate::diff_review::NextLine),
        ),
        row(
            DiffUp,
            App,
            "up",
            Same,
            DIFF,
            act!(crate::diff_review::PreviousLine),
        ),
        row(
            DiffExtendDown,
            App,
            "shift-j",
            Same,
            DIFF,
            act!(crate::diff_review::ExtendDown),
        ),
        row(
            DiffExtendUp,
            App,
            "shift-k",
            Same,
            DIFF,
            act!(crate::diff_review::ExtendUp),
        ),
        row(
            DiffExtendArrowDown,
            App,
            "shift-down",
            Same,
            DIFF,
            act!(crate::diff_review::ExtendDown),
        ),
        row(
            DiffExtendArrowUp,
            App,
            "shift-up",
            Same,
            DIFF,
            act!(crate::diff_review::ExtendUp),
        ),
        row(
            DiffNextHunk,
            App,
            "n",
            Same,
            DIFF,
            act!(crate::diff_review::NextHunk),
        ),
        row(
            DiffPreviousHunk,
            App,
            "p",
            Same,
            DIFF,
            act!(crate::diff_review::PreviousHunk),
        ),
        row(
            DiffNextFile,
            App,
            "]",
            Same,
            DIFF,
            act!(crate::diff_review::NextFile),
        ),
        row(
            DiffPreviousFile,
            App,
            "[",
            Same,
            DIFF,
            act!(crate::diff_review::PreviousFile),
        ),
        row(
            DiffViewed,
            App,
            "v",
            Same,
            DIFF,
            act!(crate::diff_review::ToggleViewed),
        ),
        row(
            DiffStage,
            App,
            "s",
            Same,
            DIFF,
            act!(crate::diff_review::StageHunk),
        ),
        row(
            DiffComment,
            App,
            "c",
            Same,
            DIFF,
            act!(crate::diff_review::AddComment),
        ),
        row(
            DiffSend,
            App,
            "cmd-enter",
            Same,
            DIFF,
            act!(crate::diff_review::SendComments),
        ),
        row(
            DiffClear,
            App,
            "escape",
            Same,
            DIFF,
            act!(crate::diff_review::ClearCursor),
        ),
        // The whole interface, as browsers and editors zoom: ⌘= and ⌘+ (one key, with Shift or
        // without), ⌘− and ⌘0. No letter in them, so the terminal on Windows doesn't need them.
        row(ZoomIn, App, "cmd-=", Same, None, act!(crate::ZoomIn)),
        row(ZoomInShift, App, "cmd-+", Same, None, act!(crate::ZoomIn)),
        row(ZoomOut, App, "cmd--", Same, None, act!(crate::ZoomOut)),
        row(ResetZoom, App, "cmd-0", Same, None, act!(crate::ResetZoom)),
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
    if mac {
        Stroke::parse(b.mac?, true)
    } else {
        windows_stroke(b)
    }
}

/// The keys a row has on a platform now: the user's (`set_changes`) where they changed them,
/// else the table's.
fn current_stroke(b: &Binding, mac: bool) -> Option<Stroke> {
    match change_for(&b.id.name(), mac) {
        Some(Some(keys)) => Stroke::parse(&keys, mac).or_else(|| stroke_of(b, mac)),
        Some(None) => None,
        None => stroke_of(b, mac),
    }
}

fn resolve(b: &Binding, mac: bool) -> Option<Resolved> {
    let stroke = current_stroke(b, mac)?;
    // The Mac's keys go to gpui as they're written in the table.
    let keystroke = match (mac, change_for(&b.id.name(), mac)) {
        (true, None) => b.mac?.to_string(),
        _ => stroke.keystroke(),
    };
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

/// The bindings of `group` on a Mac (`mac`) or Windows, as the user has them.
pub fn bindings_for(group: Group, mac: bool) -> Vec<KeyBinding> {
    TABLE
        .iter()
        .filter(|b| b.group == group)
        .filter_map(|b| {
            let r = resolve(b, mac)?;
            Some((b.build?)(&r.keystroke, r.context.as_deref()).with_meta(TREK))
        })
        .collect()
}

/// What marks a binding in gpui's keymap as one of Trek's own, so `rebind` can take Trek's out and
/// put them back as the user has them, leaving the kit's own (its text fields', its lists') be.
pub const TREK: gpui_kit::KeyBindingMetaIndex = gpui_kit::KeyBindingMetaIndex(0x7e3c);

/// While a shortcut is being recorded (Settings › Keyboard Shortcuts) Trek's keys are off, so
/// pressing ⌘Q there records it instead of quitting.
static SUSPENDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Every binding of Trek's, as the user has them, tagged `TREK`: none while recording.
pub fn trek_bindings() -> Vec<KeyBinding> {
    if SUSPENDED.load(std::sync::atomic::Ordering::Relaxed) {
        return Vec::new();
    }
    let mut out = crate::key_bindings();
    out.extend(bindings(Group::Notes));
    out.extend(bindings(Group::Editor));
    out
}

/// Put Trek's bindings in gpui's keymap again as the user has them now (after a change in
/// Settings, or a settings file read at start), the menus with them (the Mac's menu bar shows
/// each command's keys from the keymap) and the plugins' keys after them (a key the user gave a
/// command of Trek's isn't a plugin's to take).
pub fn rebind(cx: &mut gpui_kit::App) {
    let keymap = cx.key_bindings();
    let kept: Vec<KeyBinding> = keymap
        .borrow()
        .bindings()
        .filter(|b| b.meta() != Some(TREK))
        .cloned()
        .collect();
    cx.clear_key_bindings();
    cx.bind_keys(kept);
    cx.bind_keys(trek_bindings());
    cx.set_menus(crate::menus());
    crate::plugins::keys_changed(cx);
}

/// Turn Trek's keys off while a shortcut is recorded, and on again after.
pub fn suspend(on: bool, cx: &mut gpui_kit::App) {
    if SUSPENDED.swap(on, std::sync::atomic::Ordering::Relaxed) != on {
        rebind(cx);
    }
}

/// Whether Trek's keys are off for a recording (`suspend`).
pub fn suspended() -> bool {
    SUSPENDED.load(std::sync::atomic::Ordering::Relaxed)
}

// ───────────────────────────── the user's changes ─────────────────────────────

impl Id {
    /// The name a change to it is saved under (`new-thread`): its own, in kebab case.
    pub fn name(self) -> String {
        let debug = format!("{self:?}");
        let mut out = String::with_capacity(debug.len() + 4);
        for (i, c) in debug.chars().enumerate() {
            if c.is_ascii_uppercase() {
                if i > 0 {
                    out.push('-');
                }
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    }
}

/// The shortcuts the user changed, as the settings file has them (`Settings::keys`).
static CHANGES: LazyLock<std::sync::RwLock<trek_core::settings::Keys>> =
    LazyLock::new(Default::default);

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static TEST_CHANGES: std::cell::RefCell<Option<trek_core::settings::Keys>> =
        const { std::cell::RefCell::new(None) };
}

/// Take the user's changes (from the settings file): whether they differ from what was had.
/// `rebind` puts them in the keymap.
pub fn set_changes(keys: &trek_core::settings::Keys) -> bool {
    let mut held = CHANGES.write().unwrap_or_else(|e| e.into_inner());
    if *held == *keys {
        return false;
    }
    *held = keys.clone();
    true
}

/// Run `f` with these changes in place of the user's, on this thread only (the tests').
#[cfg(any(test, feature = "test-support"))]
pub fn with_changes<T>(keys: trek_core::settings::Keys, f: impl FnOnce() -> T) -> T {
    let before = TEST_CHANGES.with(|c| c.replace(Some(keys)));
    let out = f();
    TEST_CHANGES.with(|c| *c.borrow_mut() = before);
    out
}

fn any_changes(mac: bool) -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(found) =
        TEST_CHANGES.with(|c| c.borrow().as_ref().map(|k| !k.for_platform(mac).is_empty()))
    {
        return found;
    }
    !CHANGES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .for_platform(mac)
        .is_empty()
}

/// What the user changed `name` to on a platform: `None` where they left it, `Some(None)` where
/// they took its keys away.
fn change_for(name: &str, mac: bool) -> Option<Option<String>> {
    let pick = |keys: &trek_core::settings::Keys| {
        keys.for_platform(mac)
            .get(name)
            .map(|k| Some(k.trim().to_string()).filter(|k| !k.is_empty()))
    };
    #[cfg(any(test, feature = "test-support"))]
    if let Some(found) = TEST_CHANGES.with(|c| c.borrow().as_ref().map(pick)) {
        return found;
    }
    pick(&CHANGES.read().unwrap_or_else(|e| e.into_inner()))
}

/// The keys a plugin's command has on the platform running: the user's where they changed them
/// (`plugin:<id>/<command>`), else what its manifest says (`cmd` is Ctrl off the Mac). `None`:
/// none.
pub fn plugin_key(plugin: &str, command: &str, manifest: Option<&str>) -> Option<String> {
    plugin_key_for(plugin, command, manifest, is_mac())
}

pub fn plugin_key_for(
    plugin: &str,
    command: &str,
    manifest: Option<&str>,
    mac: bool,
) -> Option<String> {
    match change_for(&plugin_name(plugin, command), mac) {
        Some(changed) => changed,
        None => manifest.map(|m| manifest_keystroke(m, mac)),
    }
}

/// A manifest's key (`cmd-alt-f`) as a platform has it: `cmd` is Ctrl off the Mac.
pub fn manifest_keystroke(key: &str, mac: bool) -> String {
    if mac {
        key.to_string()
    } else {
        key.split('-')
            .map(|part| if part == "cmd" { "ctrl" } else { part })
            .collect::<Vec<_>>()
            .join("-")
    }
}

/// The name a plugin command's change is saved under.
pub fn plugin_name(plugin: &str, command: &str) -> String {
    format!("plugin:{plugin}/{command}")
}

fn row_of(id: Id) -> Option<&'static Binding> {
    TABLE.iter().find(|b| b.id == id)
}

/// The keystroke that triggers `id` here, as gpui reads it: what a test presses.
#[cfg(any(test, feature = "test-support"))]
pub fn keystroke(id: Id) -> String {
    row_of(id)
        .and_then(|b| resolve(b, is_mac()))
        .map(|r| r.keystroke)
        .unwrap_or_else(|| panic!("{id:?} has no keys here"))
}

/// The keys `id` has on the platform running, as text (`⌘N`, `Ctrl+N`); empty where it has none.
pub fn hint(id: Id) -> String {
    hint_for(id, is_mac())
}

pub fn hint_for(id: Id, mac: bool) -> String {
    row_of(id)
        .and_then(|b| current_stroke(b, mac))
        .map(|s| s.label(mac))
        .unwrap_or_default()
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
    // On a Mac the text is written as it's shown, unless the user changed a shortcut in it.
    if !has_glyphs(text) || (mac && !any_changes(true)) {
        return Cow::Borrowed(text);
    }
    let mut taken_away = false;
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
        let start = i;
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
            Some(c)
                if modifiers > 0
                    && c.is_ascii_alphanumeric()
                    && !after.is_some_and(char::is_alphanumeric) =>
            {
                Some(c.to_ascii_lowercase().to_string())
            }
            Some(c)
                if modifiers > 0
                    && matches!(c, ',' | '.' | '`' | '/' | ';' | '\'' | '[' | ']' | '=') =>
            {
                Some(c.to_string())
            }
            _ => None,
        };
        match key {
            Some(key) => {
                i += 1;
                if modifiers == 0 {
                    if mac {
                        out.extend(&chars[start..i]);
                    } else {
                        out.push_str(&windows_key(&key));
                    }
                } else {
                    stroke.key = key;
                    match (mac, on_platform(&stroke, mac)) {
                        // Left as it was: written as the text has it.
                        (true, Shown::Default) => out.extend(&chars[start..i]),
                        (false, Shown::Default) => {
                            out.push_str(&stroke.ctrl_for_cmd().label(false))
                        }
                        (_, Shown::Keys(s)) => out.push_str(&s.label(mac)),
                        (_, Shown::None) => taken_away = true,
                    }
                }
            }
            None if mac => out.extend(&chars[start..i]),
            None => out.push_str(&windows_modifiers_alone(&stroke)),
        }
    }
    if taken_away {
        // "Settings (⌘,)" with ⌘, taken away is "Settings", not "Settings ()".
        out = out.replace(" ()", "").replace("()", "");
    }
    Cow::Owned(out)
}

/// What a glyph run in a sentence shows on a platform.
enum Shown {
    /// What the text has (on Windows: its keys with Ctrl for ⌘): no row has those keys.
    Default,
    Keys(Stroke),
    /// The user took the row's keys away.
    None,
}

/// A modifier with no key after it ("⌘-click", "hold ⌥"): Ctrl, Alt…
fn windows_modifiers_alone(stroke: &Stroke) -> String {
    stroke.ctrl_for_cmd().windows_modifiers().join("+")
}

/// What a platform has for the Mac keystroke `stroke` as a sentence wrote it: the first row with
/// those keys on a Mac, as the user has it here (the row's own keys on Windows, or the user's).
fn on_platform(stroke: &Stroke, mac: bool) -> Shown {
    let rows = || {
        TABLE
            .iter()
            .filter(|b| b.mac.and_then(|m| Stroke::parse(m, true)).as_ref() == Some(stroke))
    };
    let Some(first) = rows().next() else {
        return Shown::Default;
    };
    match change_for(&first.id.name(), mac) {
        Some(_) => current_stroke(first, mac).map_or(Shown::None, Shown::Keys),
        None if mac => Shown::Default,
        // The first row Windows binds (a row dropped there leaves it to the next).
        None => rows()
            .find_map(windows_stroke)
            .map_or(Shown::Default, Shown::Keys),
    }
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
    /// A binding in the table, which the user can change.
    Bound(Id),
    /// Keys that aren't Trek's to bind (the clipboard's, the input's Enter), written as a keystroke.
    Keys(&'static str),
    /// One key cap with this text on both.
    Cap(&'static str),
}

use Show::{Bound, Cap};
/// Every binding Trek has, in the page's sections, and the keys that aren't its to change.
const PAGE: &[(&str, &[(&str, Show)])] = &[
    (
        "Threads",
        &[
            ("Search threads and commands", Bound(Id::OpenPalette)),
            (
                "Search threads and commands, anywhere",
                Bound(Id::OpenPaletteAnywhere),
            ),
            ("New thread", Bound(Id::NewThread)),
            ("Open a folder", Bound(Id::OpenFolder)),
            (
                "Open the thread in a new window",
                Bound(Id::OpenInNewWindow),
            ),
            (
                "Open a thread beside this one, or move across a split",
                Bound(Id::OpenBeside),
            ),
            ("Copy a link to the thread", Bound(Id::CopyThreadLink)),
            ("Copy the thread as Markdown", Bound(Id::CopyThreadMarkdown)),
            (
                "Copy the thread's working directory",
                Bound(Id::CopyThreadFolder),
            ),
            ("Next thread that needs you", Bound(Id::NextNeedsYou)),
            (
                "Back to where you last sent a prompt",
                Bound(Id::BackToLastPrompt),
            ),
            ("Settle the current thread", Bound(Id::SettleThread)),
            ("Stop the agent", Bound(Id::Interrupt)),
        ],
    ),
    (
        "Composer",
        &[
            // "Send" and "New line" swap their keys with the setting (`page`).
            ("Send", Show::Keys("enter")),
            ("New line", Show::Keys("shift-enter")),
            (
                "Send the other way mid-turn (steer or queue)",
                Show::Keys("alt-enter"),
            ),
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
            ("Save the file", Bound(Id::SaveFile)),
            ("Format the document", Bound(Id::FormatDocument)),
            ("Keep a change", Bound(Id::KeepHunk)),
            ("Undo a change", Bound(Id::UndoHunk)),
            ("Next change", Bound(Id::NextHunk)),
            ("Previous change", Bound(Id::PreviousHunk)),
            ("Keep all changes", Show::Keys("secondary-enter")),
            ("Undo all changes (press twice)", Bound(Id::UndoAllOrStop)),
            ("Explorer", Bound(Id::ToggleIde)),
            ("Search in files", Bound(Id::ToggleIdeSearch)),
            ("Source control", Bound(Id::FocusScm)),
            ("Terminal", Bound(Id::ToggleTerminal)),
            ("AI side bar", Bound(Id::ToggleAiBar)),
            (
                "Add the selection to the chat",
                Bound(Id::AddSelectionToChat),
            ),
            (
                "Add the selection to a new chat",
                Bound(Id::AddSelectionToNewChat),
            ),
        ],
    ),
    (
        "Window",
        &[
            ("Switch Agents / Editor", Bound(Id::SwitchMode)),
            ("Basecamp", Bound(Id::OpenBasecamp)),
            ("Leave Basecamp", Bound(Id::LeaveBasecamp)),
            ("Notes", Bound(Id::OpenNotes)),
            ("Study", Bound(Id::OpenStudy)),
            ("Toggle the sidebar", Bound(Id::ToggleSidebar)),
            ("Toggle the tools panel", Bound(Id::ToggleRightPanel)),
            ("Settings", Bound(Id::OpenSettings)),
            ("Next tab", Bound(Id::NextTab)),
            ("Previous tab", Bound(Id::PreviousTab)),
            ("Close the file or thread tab in front", Bound(Id::CloseTab)),
            (
                "Close the file or thread tab in front, again",
                Bound(Id::CloseTabCtrlF4),
            ),
            ("Close a thread window", Bound(Id::CloseWindow)),
            ("Hide Trek", Bound(Id::HideApp)),
            ("Minimize", Bound(Id::Minimize)),
            ("Quit", Bound(Id::Quit)),
        ],
    ),
    (
        "Zoom",
        &[
            ("Zoom in", Bound(Id::ZoomIn)),
            ("Zoom in, with Shift", Bound(Id::ZoomInShift)),
            ("Zoom out", Bound(Id::ZoomOut)),
            ("Actual size", Bound(Id::ResetZoom)),
        ],
    ),
    (
        "Notes",
        &[
            ("New note", Bound(Id::NewNote)),
            ("Bold", Bound(Id::NoteBold)),
            ("Italic", Bound(Id::NoteItalic)),
            ("Underline", Bound(Id::NoteUnderline)),
            ("Strikethrough", Bound(Id::NoteStrike)),
            ("Bulleted list", Bound(Id::NoteBullets)),
            ("Numbered list", Bound(Id::NoteNumbers)),
            ("Checklist", Bound(Id::NoteChecklist)),
            ("Heading", Bound(Id::NoteHeading1)),
            ("Subheading", Bound(Id::NoteHeading2)),
            ("Quote", Bound(Id::NoteQuote)),
            ("Code", Bound(Id::NoteCode)),
            ("Check or uncheck", Bound(Id::NoteToggleCheck)),
        ],
    ),
    (
        "Diff review",
        &[
            ("Next line", Bound(Id::DiffNextLine)),
            ("Next line, with the arrow", Bound(Id::DiffDown)),
            ("Previous line", Bound(Id::DiffPreviousLine)),
            ("Previous line, with the arrow", Bound(Id::DiffUp)),
            ("Select lines down", Bound(Id::DiffExtendDown)),
            (
                "Select lines down, with the arrow",
                Bound(Id::DiffExtendArrowDown),
            ),
            ("Select lines up", Bound(Id::DiffExtendUp)),
            (
                "Select lines up, with the arrow",
                Bound(Id::DiffExtendArrowUp),
            ),
            ("Next hunk", Bound(Id::DiffNextHunk)),
            ("Previous hunk", Bound(Id::DiffPreviousHunk)),
            ("Next file", Bound(Id::DiffNextFile)),
            ("Previous file", Bound(Id::DiffPreviousFile)),
            ("Comment on the line", Bound(Id::DiffComment)),
            ("Send comments to the agent", Bound(Id::DiffSend)),
            ("Mark the file viewed", Bound(Id::DiffViewed)),
            ("Stage or unstage the hunk", Bound(Id::DiffStage)),
            ("Clear the selection", Bound(Id::DiffClear)),
        ],
    ),
];

/// A plugin's command, for the page (`plugins::key_commands`).
#[derive(Clone, Debug, PartialEq)]
pub struct PluginCommand {
    pub plugin: String,
    pub plugin_name: String,
    pub command: String,
    pub title: String,
    /// The key its manifest asks for (`cmd-alt-f`).
    pub key: Option<String>,
}

/// A shortcut the user can change: one of Trek's, or a plugin's command.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    /// What a change is saved under: the id's name, or `plugin:<id>/<command>`.
    pub name: String,
    pub title: String,
    pub section: String,
    /// Its keys now, as gpui reads them (`None`: it has none).
    pub keys: Option<String>,
    /// Trek's (or the plugin manifest's) keys for it on this platform.
    pub default: Option<String>,
    /// Where it works (the table's key context; a plugin's are everywhere).
    pub context: Option<String>,
    pub plugin: bool,
    /// Keys on their own (no ⌘, ⌃ or ⌥) are fine for it: its default is one (a diff's letters,
    /// Escape, ⇧⇥ in the composer).
    pub plain_ok: bool,
}

impl Command {
    pub fn changed(&self) -> bool {
        self.keys != self.default
    }

    /// Its keys as caps: `⌘ ⇧ K`, `Ctrl Shift K`.
    pub fn chips(&self, mac: bool) -> Vec<Chip> {
        self.keys
            .as_deref()
            .map(|k| chips_for(k, mac))
            .unwrap_or_default()
    }
}

/// `keystroke` as the page's key caps.
pub fn chips_for(keystroke: &str, mac: bool) -> Vec<Chip> {
    Stroke::parse(keystroke, mac)
        .map(|s| Chip::keys(s.chips(mac)))
        .unwrap_or_default()
}

/// A row of the page.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Command(Command),
    /// Keys that aren't Trek's to change: the composer's Enter, `/`, the clipboard's paste.
    Fixed(&'static str, Vec<Chip>),
}

impl Row {
    pub fn title(&self) -> &str {
        match self {
            Row::Command(c) => &c.title,
            Row::Fixed(t, _) => t,
        }
    }
}

/// Whether a row has keys on a platform by default (Hide isn't on Windows, Ctrl+F4 not on a Mac).
fn exists(b: &Binding, mac: bool) -> bool {
    stroke_of(b, mac).is_some()
}

fn command_of(title: &str, section: &str, b: &Binding, mac: bool) -> Command {
    let keys = current_stroke(b, mac).map(|s| match (mac, change_for(&b.id.name(), mac)) {
        (true, None) => b.mac.unwrap_or_default().to_string(),
        _ => s.keystroke(),
    });
    let default = stroke_of(b, mac).map(|s| {
        if mac {
            b.mac.unwrap_or_default().to_string()
        } else {
            s.keystroke()
        }
    });
    let plain_ok = stroke_of(b, mac).is_some_and(|s| !s.ctrl && !s.alt && !s.cmd);
    Command {
        name: b.id.name(),
        title: title.to_string(),
        section: section.to_string(),
        keys,
        default,
        context: b.context.map(str::to_string),
        plugin: false,
        plain_ok,
    }
}

/// The Keyboard Shortcuts page on a Mac (`mac`) or Windows: its sections and their rows, the
/// plugins' commands with a key in a section of their own. `send_with_secondary_enter` is the
/// setting that swaps Send and New line. Shortcuts the platform doesn't have (Hide on Windows) are
/// left out.
pub fn page(
    mac: bool,
    send_with_secondary_enter: bool,
    plugins: &[PluginCommand],
) -> Vec<(String, Vec<Row>)> {
    let mut out: Vec<(String, Vec<Row>)> = PAGE
        .iter()
        .map(|(section, rows)| {
            let rows = rows
                .iter()
                .filter_map(|(name, show)| {
                    Some(match (*name, show) {
                        ("Send", _) => Row::Fixed(
                            name,
                            chips_for(
                                if send_with_secondary_enter {
                                    "secondary-enter"
                                } else {
                                    "enter"
                                },
                                mac,
                            ),
                        ),
                        ("New line", _) => Row::Fixed(
                            name,
                            chips_for(
                                if send_with_secondary_enter {
                                    "enter"
                                } else {
                                    "shift-enter"
                                },
                                mac,
                            ),
                        ),
                        (_, Show::Bound(id)) => {
                            let b = row_of(*id)?;
                            if !exists(b, mac) {
                                return None;
                            }
                            Row::Command(command_of(name, section, b, mac))
                        }
                        (_, Show::Keys(keystroke)) => Row::Fixed(name, chips_for(keystroke, mac)),
                        (_, Show::Cap(text)) => Row::Fixed(name, vec![Chip::Key((*text).into())]),
                    })
                })
                .collect();
            (section.to_string(), rows)
        })
        .collect();
    if !plugins.is_empty() {
        let rows = plugins
            .iter()
            .map(|p| {
                let default = p.key.as_deref().map(|k| manifest_keystroke(k, mac));
                Row::Command(Command {
                    name: plugin_name(&p.plugin, &p.command),
                    title: p.title.clone(),
                    section: p.plugin_name.clone(),
                    keys: plugin_key_for(&p.plugin, &p.command, p.key.as_deref(), mac),
                    default,
                    context: None,
                    plugin: true,
                    plain_ok: false,
                })
            })
            .collect();
        out.push(("Plugins".to_string(), rows));
    }
    out
}

/// Every command on the page, in its order.
pub fn commands(page: &[(String, Vec<Row>)]) -> Vec<&Command> {
    page.iter()
        .flat_map(|(_, rows)| rows)
        .filter_map(|r| match r {
            Row::Command(c) => Some(c),
            Row::Fixed(..) => None,
        })
        .collect()
}

/// The same keys, however they were written (`cmd-shift-k`, `shift-cmd-k`).
fn same_keys(a: &str, b: &str, mac: bool) -> bool {
    match (Stroke::parse(a, mac), Stroke::parse(b, mac)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// The commands that already have `keystroke` where `name` would have it: Trek's in the same
/// context, and a plugin's anywhere (a plugin's key is heard everywhere, and gives way to any of
/// Trek's).
pub fn conflicts<'a>(
    commands: &[&'a Command],
    name: &str,
    keystroke: &str,
    mac: bool,
) -> Vec<&'a Command> {
    let Some(me) = commands.iter().find(|c| c.name == name) else {
        return Vec::new();
    };
    commands
        .iter()
        .copied()
        .filter(|c| c.name != name)
        .filter(|c| {
            c.keys
                .as_deref()
                .is_some_and(|k| same_keys(k, keystroke, mac))
        })
        .filter(|c| c.plugin || me.plugin || c.context == me.context)
        .collect()
}

/// Why `keystroke` can't be a shortcut on a platform, if it can't: the system keeps it, it's what
/// text fields and the terminal need, or it's a key on its own where typing needs it (`plain_ok`
/// lets a command that already has one keep one: a diff's letters).
pub fn refusal(keystroke: &str, mac: bool, plain_ok: bool) -> Option<&'static str> {
    let Some(s) = Stroke::parse(keystroke, mac) else {
        return Some("That isn't a key Trek can use.");
    };
    let only = |ctrl: bool, alt: bool, shift: bool, cmd: bool, key: &str| {
        s.ctrl == ctrl && s.alt == alt && s.shift == shift && s.cmd == cmd && s.key == key
    };
    let function_key =
        s.key.len() > 1 && s.key.starts_with('f') && s.key[1..].parse::<u8>().is_ok();
    if mac {
        let system = [
            (false, false, false, true, "tab"),
            (false, false, true, true, "tab"),
            (false, false, false, true, "space"),
            (false, true, false, true, "space"),
            (true, false, false, true, "space"),
            (true, false, false, false, "space"),
            (false, false, true, true, "3"),
            (false, false, true, true, "4"),
            (false, false, true, true, "5"),
            (false, false, true, true, "6"),
            (true, false, false, true, "q"),
            (false, true, false, true, "escape"),
            (true, false, false, false, "up"),
            (true, false, false, false, "down"),
            (true, false, false, false, "left"),
            (true, false, false, false, "right"),
        ];
        if system
            .iter()
            .any(|(c, a, sh, m, k)| only(*c, *a, *sh, *m, k))
        {
            return Some("macOS keeps those keys for itself.");
        }
    } else {
        if s.cmd {
            return Some("Windows keeps the Windows key for itself.");
        }
        if s.ctrl && s.alt {
            return Some("Ctrl+Alt is AltGr, which types characters on many keyboards.");
        }
        let system = [
            (false, true, false, false, "tab"),
            (false, true, true, false, "tab"),
            (false, true, false, false, "f4"),
            (false, true, false, false, "space"),
            (false, true, false, false, "escape"),
            (true, false, false, false, "escape"),
            (true, false, true, false, "escape"),
        ];
        if system
            .iter()
            .any(|(c, a, sh, m, k)| only(*c, *a, *sh, *m, k))
        {
            return Some("Windows keeps those keys for itself.");
        }
        // The terminal's copy and paste.
        if (s.ctrl && s.shift && !s.alt && matches!(s.key.as_str(), "c" | "v"))
            || only(false, false, true, false, "insert")
        {
            return Some("The terminal copies and pastes with those keys.");
        }
    }
    // The clipboard's, select all, undo and redo: every text field needs them.
    let secondary = if mac {
        s.cmd && !s.ctrl
    } else {
        s.ctrl && !s.cmd
    };
    if secondary && !s.alt && matches!(s.key.as_str(), "c" | "v" | "x" | "a" | "z") && !s.shift
        || secondary && !s.alt && s.shift && s.key == "z"
        || (!mac && only(true, false, false, false, "y"))
    {
        return Some("Text fields need those keys: copy, paste, cut, select all, undo and redo.");
    }
    if !(s.ctrl || s.alt || s.cmd) && !plain_ok && !function_key {
        return Some(if mac {
            "Add ⌘, ⌥ or ⌃: a key on its own is what typing uses."
        } else {
            "Add Ctrl or Alt: a key on its own is what typing uses."
        });
    }
    None
}

/// Save that `command` now has `keys` (`None`: none) on a platform, in `changes`: only as a
/// difference from its default, so a default changed in a later version still arrives.
pub fn store(
    changes: &mut trek_core::settings::Keys,
    mac: bool,
    command: &Command,
    keys: Option<&str>,
) {
    let same = match (keys, command.default.as_deref()) {
        (None, None) => true,
        (Some(k), Some(d)) => same_keys(k, d, mac),
        _ => false,
    };
    let map = changes.for_platform_mut(mac);
    if same {
        map.remove(&command.name);
    } else {
        let keys = keys
            .and_then(|k| Stroke::parse(k, mac))
            .map(|s| s.keystroke())
            .unwrap_or_default();
        map.insert(command.name.clone(), keys);
    }
}

/// A key the user pressed, as gpui reads it (`shift-cmd-k`): Shift, Ctrl, Alt and ⌘ (the Windows
/// key there) with the key. `None` for a modifier on its own.
pub fn recorded(keystroke: &gpui_kit::Keystroke) -> Option<String> {
    let key = keystroke.key.to_ascii_lowercase();
    if key.is_empty()
        || matches!(
            key.as_str(),
            "shift" | "control" | "alt" | "platform" | "function" | "cmd" | "ctrl"
        )
    {
        return None;
    }
    let m = keystroke.modifiers;
    Some(
        Stroke {
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
            cmd: m.platform,
            key,
        }
        .keystroke(),
    )
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
                let parsed = Stroke::parse(&r.keystroke, mac)
                    .unwrap_or_else(|| panic!("{:?}: {} doesn't parse", b.id, r.keystroke));
                if let Some(other) = seen.insert((parsed.keystroke(), r.context.clone()), b.id) {
                    panic!(
                        "{:?} and {:?} are both {} in {:?} (mac: {mac})",
                        other, b.id, r.keystroke, r.context
                    );
                }
            }
        }
    }

    #[test]
    fn every_action_has_keys_on_both_platforms_unless_dropped_there() {
        for b in TABLE.iter() {
            match (b.mac, b.windows) {
                (Some(_), Win::Dropped) => assert!(
                    resolve(b, true).is_some() && resolve(b, false).is_none(),
                    "{:?}",
                    b.id
                ),
                (Some(_), _) => assert!(
                    resolve(b, true).is_some() && resolve(b, false).is_some(),
                    "{:?}",
                    b.id
                ),
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
        let mac: Vec<String> = TABLE
            .iter()
            .filter(|b| b.build.is_some())
            .filter_map(|b| resolve(b, true))
            .map(|r| format!("{}|{}", r.keystroke, r.context.unwrap_or_default()))
            .collect();
        // The 44 in main.rs, the 13 in notes.rs, the 2 in editor.rs, a diff's 17 and zoom's 4.
        assert_eq!(mac.len(), 44 + 13 + 2 + 17 + 4);
        for (keys, ctx) in [
            ("cmd-q", ""),
            ("alt-cmd-e", ""),
            ("alt-cmd-l", ""),
            ("cmd-shift-m", ""),
            ("cmd-shift-c", ""),
            ("cmd-shift-u", ""),
            ("cmd-k", "!IdeEditor"),
            ("cmd-k", "IdeEditor"),
            ("alt-cmd-backspace", "IdeEditor && hunks"),
            ("ctrl-`", ""),
            ("cmd-shift-.", "NoteEditor"),
            ("cmd-s", "TrekEditor"),
            ("alt-shift-f", "TrekEditor"),
            ("cmd-w", "TrekWindow"),
            ("cmd-w", "ThreadWindow"),
        ] {
            let want = format!("{keys}|{ctx}");
            assert!(mac.contains(&want), "{want}");
        }
    }

    #[test]
    fn rows_that_share_a_mac_keystroke_share_their_windows_one() {
        // `localize` finds a Windows spelling by the Mac's keystroke alone.
        let mut seen: HashMap<String, Option<String>> = HashMap::new();
        for b in TABLE.iter() {
            let Some(mac) = b.mac.and_then(|m| Stroke::parse(m, true)) else {
                continue;
            };
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
            assert!(
                !(s.ctrl && s.alt),
                "{:?} is Ctrl+Alt: AltGr types characters on many layouts",
                b.id
            );
            assert!(!s.cmd, "{:?} uses the Windows key", b.id);
            // The system's.
            for taken in [
                ("alt", "f4"),
                ("alt", "space"),
                ("alt", "tab"),
                ("alt", "enter"),
                ("ctrl", "escape"),
            ] {
                let held = if taken.0 == "alt" {
                    s.alt && !s.ctrl && !s.shift
                } else {
                    s.ctrl && !s.alt && !s.shift
                };
                assert!(!(held && s.key == taken.1), "{:?} is {taken:?}", b.id);
            }
            // The terminal panel's: paste.
            assert!(
                !(s.ctrl && s.shift && !s.alt && matches!(s.key.as_str(), "v" | "c")),
                "{:?} is the terminal's copy/paste",
                b.id
            );
            assert!(
                !(s.shift && s.key == "insert" && !s.ctrl && !s.alt),
                "{:?}",
                b.id
            );
            // Shell keys (Ctrl+C, Ctrl+L, Ctrl+K…) are left to the shell while the terminal has focus.
            if s.is_plain_ctrl_letter() {
                let r = resolve(b, false).unwrap();
                assert!(
                    r.context.is_some_and(|c| c.ends_with("!Terminal")),
                    "{:?} would take {} from the shell",
                    b.id,
                    r.keystroke
                );
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
            (Id::CopyThreadLink, "alt-shift-l"),
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
        assert_eq!(
            different,
            want.iter()
                .map(|(i, k)| (*i, k.to_string()))
                .collect::<Vec<_>>()
        );
    }

    /// Ctrl+W closes the tab or file in front in the main window and a thread window in its own
    /// (never the other), and Ctrl+F4 is the same as Ctrl+W on Windows only.
    #[test]
    fn closing_a_tab_and_closing_a_thread_window_are_told_apart_by_the_window() {
        let at =
            |id: Id, mac: bool| resolve(row_of(id).unwrap(), mac).map(|r| (r.keystroke, r.context));
        // On Windows Ctrl+W is the shell's (delete a word) while the terminal panel has the keys.
        for (mac, w, tab, window) in [
            (true, "cmd-w", "TrekWindow", "ThreadWindow"),
            (
                false,
                "ctrl-w",
                "(TrekWindow) && !Terminal",
                "(ThreadWindow) && !Terminal",
            ),
        ] {
            assert_eq!(at(Id::CloseTab, mac), Some((w.into(), Some(tab.into()))));
            assert_eq!(
                at(Id::CloseWindow, mac),
                Some((w.into(), Some(window.into())))
            );
        }
        assert_eq!(
            at(Id::CloseTabCtrlF4, false),
            Some(("ctrl-f4".into(), Some("TrekWindow".into())))
        );
        assert_eq!(at(Id::CloseTabCtrlF4, true), None);
        assert_eq!(hint_for(Id::CloseTab, false), "Ctrl+W");
        assert_eq!(hint_for(Id::CloseTab, true), "⌘W");
        assert_eq!(hint_for(Id::FormatDocument, false), "Alt+Shift+F");
        assert_eq!(hint_for(Id::FormatDocument, true), "⌥⇧F");
    }

    #[test]
    fn terminal_gets_the_shell_keys_but_trek_keeps_its_own() {
        let ctx = |id: Id| resolve(row_of(id).unwrap(), false).unwrap().context;
        assert_eq!(
            ctx(Id::OpenPalette).as_deref(),
            Some("(!IdeEditor) && !Terminal"),
            "Ctrl+K is the shell's in the terminal"
        );
        assert_eq!(ctx(Id::NewThread).as_deref(), Some("!Terminal"));
        assert_eq!(
            ctx(Id::OpenBeside).as_deref(),
            Some("(TrekWindow) && !Terminal"),
            "Ctrl+\\ is the shell's in the terminal"
        );
        assert_eq!(hint_for(Id::OpenBeside, false), "Ctrl+\\");
        assert_eq!(hint_for(Id::OpenBeside, true), "⌘\\");
        assert_eq!(
            ctx(Id::ToggleTerminal),
            None,
            "Ctrl+` must reach Trek from inside the terminal"
        );
        assert_eq!(
            ctx(Id::OpenPaletteAnywhere),
            None,
            "the palette is still there, in the terminal and the editor's text too"
        );
        assert_eq!(ctx(Id::NextTab), None);
        assert_eq!(ctx(Id::FocusScm), None);
        // The Mac has no such thing: there ⌘ is never the shell's.
        assert_eq!(
            resolve(row_of(Id::NewThread).unwrap(), true)
                .unwrap()
                .context,
            None
        );
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
        for text in [
            "Stop ⌘. · x",
            "⌥⌘E",
            "↩ sends; ⇧↩ adds a new line.",
            "⌘-click",
            "hold ⌥",
        ] {
            assert!(matches!(localize_for(text, true), Cow::Borrowed(t) if t == text));
        }
    }

    #[test]
    fn sentences_from_the_app_on_windows() {
        for (mac, windows) in [
            ("Agents (⌥⌘E)", "Agents (Alt+Shift+E)"),
            ("AI side bar (⌥⌘B)", "AI side bar (Alt+Shift+B)"),
            ("Settings (⌘,)", "Settings (Ctrl+,)"),
            (
                "Open in new window (⌘⇧↩)",
                "Open in new window (Ctrl+Shift+Enter)",
            ),
            ("Source Control (⌃⇧G)", "Source Control (Ctrl+Shift+G)"),
            ("Stop ⌘. · queued", "Stop Ctrl+. · queued"),
            (
                "Plan mode: the agent plans before it changes anything (⇧⇥)",
                "Plan mode: the agent plans before it changes anything (Shift+Tab)",
            ),
            (
                "Steer the running turn · ⌥↩ queues for after it",
                "Steer the running turn · Alt+Enter queues for after it",
            ),
            (
                "↩ adds a new line; ⌘↩ sends.",
                "Enter adds a new line; Ctrl+Enter sends.",
            ),
            (
                "↩ sends; ⇧↩ adds a new line.",
                "Enter sends; Shift+Enter adds a new line.",
            ),
            (
                "Screenshots you attach from the composer’s + menu or with ⌘⇧S.",
                "Screenshots you attach from the composer’s + menu or with Ctrl+Shift+S.",
            ),
            (
                "↑↓ to move · ↩ to pick · esc",
                "↑↓ to move · Enter to pick · esc",
            ),
            (
                "Keep / Undo a Change ⌘Y / ⌥⌘⌫",
                "Keep / Undo a Change Ctrl+Shift+Y / Alt+Shift+Backspace",
            ),
            ("Redo (⇧⌘Z)", "Redo (Ctrl+Y)"),
            ("Heading (⌘⌥1)", "Heading (Alt+Shift+1)"),
            (
                "Undo all (⌘⇧⌫, twice)",
                "Undo all (Ctrl+Shift+Backspace, twice)",
            ),
            ("⌘1–9", "Ctrl+1–9"),
            ("Jump ⌘1–⌘9", "Jump Ctrl+1–Ctrl+9"),
            (
                "⌘-click a link, or hold ⌥",
                "Ctrl-click a link, or hold Alt",
            ),
            ("Message (⌘↩ to commit)", "Message (Ctrl+Enter to commit)"),
            ("Bold (⌘B)", "Bold (Ctrl+B)"),
            ("Copy image (⌘C)", "Copy image (Ctrl+C)"),
            ("Next (⌥⌘↓)", "Next (Alt+F5)"),
            ("no glyphs here", "no glyphs here"),
        ] {
            assert_eq!(localize_for(mac, false), windows, "{mac}");
        }
    }

    /// A page's section as `title: keys` lines, the keys as caps joined by spaces.
    fn flat(page: &[(String, Vec<Row>)], section: &str, mac: bool) -> Vec<String> {
        let (_, rows) = page
            .iter()
            .find(|(s, _)| s == section)
            .unwrap_or_else(|| panic!("{section}"));
        rows.iter()
            .map(|r| {
                let chips = match r {
                    Row::Command(c) => c.chips(mac),
                    Row::Fixed(_, chips) => chips.clone(),
                };
                let keys: Vec<&str> = chips
                    .iter()
                    .map(|c| match c {
                        Chip::Key(k) => k.as_str(),
                        Chip::Word(w) => w,
                    })
                    .collect();
                format!("{}: {}", r.title(), keys.join(" "))
            })
            .collect()
    }

    #[test]
    fn the_shortcuts_page_on_a_mac_is_what_it_was() {
        let page = page(true, false, &[]);
        assert_eq!(
            page.iter().map(|(g, _)| g.as_str()).collect::<Vec<_>>(),
            [
                "Threads",
                "Composer",
                "Editor",
                "Window",
                "Zoom",
                "Notes",
                "Diff review"
            ]
        );
        assert_eq!(
            flat(&page, "Threads", true),
            [
                "Search threads and commands: ⌘ K",
                "New thread: ⌘ N",
                "Open a folder: ⌘ O",
                "Open the thread in a new window: ⌘ ⇧ ↩",
                "Open a thread beside this one, or move across a split: ⌘ \\",
                "Copy a link to the thread: ⌥ ⌘ L",
                "Copy the thread as Markdown: ⌘ ⇧ M",
                "Copy the thread's working directory: ⌘ ⇧ C",
                "Next thread that needs you: ⌘ ⇧ I",
                "Back to where you last sent a prompt: ⌘ ⇧ B",
                "Settle the current thread: ⌘ E",
                "Stop the agent: ⌘ .",
            ]
        );
        assert_eq!(
            flat(&page, "Composer", true),
            [
                "Send: ↩",
                "New line: ⇧ ↩",
                "Send the other way mid-turn (steer or queue): ⌥ ↩",
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
            flat(&page, "Editor", true)[..8],
            [
                "Go to file: ⌘ P",
                "Edit the picked lines: ⌘ K",
                "Save the file: ⌘ S",
                "Format the document: ⌥ ⇧ F",
                "Keep a change: ⌘ Y",
                "Undo a change: ⌥ ⌘ ⌫",
                "Next change: ⌥ ⌘ ↓",
                "Previous change: ⌥ ⌘ ↑",
            ]
        );
        assert_eq!(
            flat(&page, "Zoom", true),
            [
                "Zoom in: ⌘ =",
                "Zoom in, with Shift: ⌘ +",
                "Zoom out: ⌘ -",
                "Actual size: ⌘ 0",
            ]
        );
        let window = flat(&page, "Window", true);
        for line in [
            "Basecamp: ⌘ ⇧ H",
            "Leave Basecamp: esc",
            "Toggle the sidebar: ⌘ B",
            "Settings: ⌘ ,",
            "Close the file or thread tab in front: ⌘ W",
            "Hide Trek: ⌘ H",
            "Quit: ⌘ Q",
        ] {
            assert!(window.contains(&line.to_string()), "{line}: {window:?}");
        }
        let swapped = super::page(true, true, &[]);
        assert_eq!(
            flat(&swapped, "Composer", true)[..2],
            ["Send: ⌘ ↩", "New line: ↩"]
        );
    }

    /// Every binding Trek makes is on the page once, so every one can be changed; the system's
    /// (Redo) isn't Trek's to change.
    #[test]
    fn every_binding_is_on_the_page_once() {
        for mac in both() {
            let page = page(mac, false, &[]);
            let names: Vec<String> = commands(&page).iter().map(|c| c.name.clone()).collect();
            let mut unique = names.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), names.len(), "a row is on the page twice");
            for b in TABLE.iter() {
                let on = names.contains(&b.id.name());
                let should = b.build.is_some() && exists(b, mac);
                assert_eq!(on, should, "{:?} (mac: {mac})", b.id);
            }
        }
    }

    #[test]
    fn a_diffs_keys_are_on_the_page_and_only_in_the_diff() {
        for (mac, send) in [(true, "⌘"), (false, "Ctrl")] {
            let page = page(mac, false, &[]);
            let rows = flat(&page, "Diff review", mac);
            assert!(rows.contains(&"Next hunk: N".to_string()), "{rows:?}");
            assert!(rows.contains(&"Comment on the line: C".to_string()));
            assert!(rows.contains(&"Next file: ]".to_string()));
            assert!(
                rows.iter()
                    .any(|r| r.starts_with(&format!("Send comments to the agent: {send}")))
            );
        }
        // Plain letters: they must never be bound outside the diff, or they'd stop reaching text.
        for b in TABLE.iter().filter(|b| b.mac.is_some_and(|m| m.len() == 1)) {
            assert_eq!(
                b.context,
                Some(crate::diff_review::KEY_CONTEXT),
                "{:?}",
                b.id
            );
        }
    }

    #[test]
    fn the_shortcuts_page_on_windows_has_its_own_keys() {
        let page = page(false, false, &[]);
        let all: Vec<String> = page
            .iter()
            .flat_map(|(s, _)| flat(&page, s, false))
            .collect();
        for line in [
            "Search threads and commands: Ctrl K",
            "Search threads and commands, anywhere: Ctrl Shift P",
            "Open the thread in a new window: Ctrl Shift Enter",
            "New line: Shift Enter",
            "Plan mode: Shift Tab",
            "Switch Agents / Editor: Alt Shift E",
            "Undo a change: Alt Shift Backspace",
            "Next change: Alt F5",
            "Previous change: Alt Shift F5",
            "Format the document: Alt Shift F",
            "Close the file or thread tab in front: Ctrl W",
            "Close the file or thread tab in front, again: Ctrl F4",
            "Leave Basecamp: Esc",
            "Zoom in: Ctrl =",
            "Actual size: Ctrl 0",
            "Quit: Ctrl Q",
        ] {
            assert!(all.contains(&line.to_string()), "{line}");
        }
        assert!(
            !all.iter()
                .any(|l| l.starts_with("Hide Trek") || l.starts_with("Minimize")),
            "{all:?}"
        );
        let swapped = super::page(false, true, &[]);
        assert_eq!(flat(&swapped, "Composer", false)[0], "Send: Ctrl Enter");
    }

    fn changes(mac: &[(&str, &str)], windows: &[(&str, &str)]) -> trek_core::settings::Keys {
        trek_core::settings::Keys {
            mac: mac
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            windows: windows
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        }
    }

    fn bound(mac: bool) -> Vec<(String, Option<String>)> {
        [Group::App, Group::Notes, Group::Editor]
            .into_iter()
            .flat_map(|g| bindings_for(g, mac))
            .map(|b| {
                (
                    b.keystrokes()
                        .iter()
                        .map(|k| k.inner().unparse())
                        .collect::<Vec<_>>()
                        .join(" "),
                    b.predicate().map(|p| p.to_string()),
                )
            })
            .collect()
    }

    /// A change is what's bound, shown and named: the new keys work, the old ones don't, and every
    /// place that writes the shortcut (hints, sentences, the page) writes the new one.
    #[test]
    fn a_changed_shortcut_is_bound_and_shown_everywhere() {
        let keys = changes(
            &[("open-settings", "cmd-shift-,"), ("quit", "")],
            &[("open-settings", "ctrl-shift-,")],
        );
        with_changes(keys, || {
            assert_eq!(hint_for(Id::OpenSettings, true), "⌘⇧,");
            assert_eq!(hint_for(Id::OpenSettings, false), "Ctrl+Shift+,");
            assert_eq!(hint_for(Id::Quit, true), "", "taken away");
            assert_eq!(hint_for(Id::Quit, false), "Ctrl+Q", "Windows keeps its own");
            assert_eq!(localize_for("Settings (⌘,)", true), "Settings (⌘⇧,)");
            assert_eq!(
                localize_for("Settings (⌘,)", false),
                "Settings (Ctrl+Shift+,)"
            );
            assert_eq!(localize_for("Quit (⌘Q)", true), "Quit");
            // Untouched shortcuts read as they're written, glyph order and all.
            assert_eq!(localize_for("Redo (⇧⌘Z)", true), "Redo (⇧⌘Z)");
            assert_eq!(localize_for("Agents (⌥⌘E)", false), "Agents (Alt+Shift+E)");
            let mac = bound(true);
            let has = |keys: &str| mac.iter().any(|(k, _)| same_keys(k, keys, true));
            assert!(has("cmd-shift-,"), "{mac:?}");
            assert!(!has("cmd-,"), "the old keys are gone");
            assert!(!has("cmd-q"));
            let page = page(true, false, &[]);
            let settings = commands(&page)
                .into_iter()
                .find(|c| c.name == "open-settings")
                .unwrap()
                .clone();
            assert!(settings.changed());
            assert_eq!(settings.default.as_deref(), Some("cmd-,"));
        });
    }

    /// The terminal on Windows keeps Ctrl+letter for the shell, whoever chose the letter.
    #[test]
    fn a_ctrl_letter_the_user_chose_is_the_shells_in_the_terminal() {
        with_changes(changes(&[], &[("open-settings", "ctrl-u")]), || {
            let r = resolve(row_of(Id::OpenSettings).unwrap(), false).unwrap();
            assert_eq!(r.keystroke, "ctrl-u");
            assert_eq!(r.context.as_deref(), Some("!Terminal"));
        });
    }

    #[test]
    fn conflicts_are_the_same_keys_in_the_same_place() {
        let plugins = [PluginCommand {
            plugin: "marmot".into(),
            plugin_name: "Marmot".into(),
            command: "wake".into(),
            title: "Wake Pip".into(),
            key: Some("cmd-alt-m".into()),
        }];
        let page = page(true, false, &plugins);
        let all = commands(&page);
        let names = |found: Vec<&Command>| found.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        // ⌘N is New thread's, everywhere: a conflict.
        assert_eq!(
            names(conflicts(&all, "open-settings", "cmd-n", true)),
            ["new-thread"]
        );
        // ⌘B is the sidebar's and, in a note, bold's: a different place, so no conflict.
        assert_eq!(
            names(conflicts(&all, "open-settings", "cmd-b", true)),
            ["toggle-sidebar"]
        );
        assert!(
            conflicts(&all, "note-italic", "cmd-b", true)
                .iter()
                .any(|c| c.name == "note-bold")
        );
        // A plugin's key is heard everywhere: any of Trek's with it is a conflict.
        assert_eq!(
            names(conflicts(&all, "note-italic", "cmd-alt-m", true)),
            ["plugin:marmot/wake"]
        );
        assert_eq!(
            names(conflicts(&all, "plugin:marmot/wake", "cmd-shift-j", true)),
            ["open-notes"]
        );
        assert!(conflicts(&all, "open-settings", "cmd-alt-shift-9", true).is_empty());
    }

    #[test]
    fn keys_the_system_and_text_need_are_refused() {
        for (keys, mac) in [
            ("cmd-tab", true),
            ("cmd-space", true),
            ("cmd-shift-4", true),
            ("ctrl-up", true),
            ("cmd-c", true),
            ("cmd-shift-z", true),
            ("k", true),
            ("shift-k", true),
            ("alt-tab", false),
            ("alt-f4", false),
            ("ctrl-alt-k", false),
            ("cmd-k", false),
            ("ctrl-shift-v", false),
            ("ctrl-v", false),
            ("ctrl-y", false),
            ("j", false),
        ] {
            assert!(refusal(keys, mac, false).is_some(), "{keys} (mac: {mac})");
        }
        for (keys, mac) in [
            ("cmd-shift-k", true),
            ("alt-cmd-9", true),
            ("ctrl-shift-k", false),
            ("alt-shift-k", false),
            ("f6", true),
            ("f6", false),
        ] {
            assert_eq!(refusal(keys, mac, false), None, "{keys} (mac: {mac})");
        }
        // A diff's own letters may stay letters.
        assert_eq!(refusal("j", true, true), None);
    }

    #[test]
    fn only_differences_from_the_defaults_are_saved() {
        let page = page(true, false, &[]);
        let all = commands(&page);
        let settings = all.iter().find(|c| c.name == "open-settings").unwrap();
        let mut keys = trek_core::settings::Keys::default();
        store(&mut keys, true, settings, Some("shift-cmd-,"));
        assert_eq!(keys.mac["open-settings"], "shift-cmd-,");
        // Back to its default, however written: nothing kept.
        store(&mut keys, true, settings, Some("cmd-,"));
        assert!(keys.is_empty(), "{keys:?}");
        store(&mut keys, true, settings, None);
        assert_eq!(keys.mac["open-settings"], "");
        assert!(keys.windows.is_empty(), "the other platform keeps its own");
        assert_eq!(Id::OpenSettings.name(), "open-settings");
        assert_eq!(Id::CloseTabCtrlF4.name(), "close-tab-ctrl-f4");
    }

    #[test]
    fn a_plugins_key_is_the_users_when_they_change_it() {
        assert_eq!(
            plugin_key_for("marmot", "wake", Some("cmd-alt-m"), false).as_deref(),
            Some("ctrl-alt-m")
        );
        with_changes(changes(&[("plugin:marmot/wake", "")], &[]), || {
            assert_eq!(
                plugin_key_for("marmot", "wake", Some("cmd-alt-m"), true),
                None
            );
        });
        with_changes(changes(&[("plugin:marmot/wake", "cmd-alt-p")], &[]), || {
            assert_eq!(
                plugin_key_for("marmot", "wake", Some("cmd-alt-m"), true).as_deref(),
                Some("cmd-alt-p")
            );
        });
    }

    /// Trek's own tables of bindings are built (and parse) on both platforms.
    #[test]
    fn both_platforms_bindings_build() {
        for mac in both() {
            let n: usize = [Group::App, Group::Notes, Group::Editor]
                .into_iter()
                .map(|g| bindings_for(g, mac).len())
                .sum();
            // Windows drops Hide, Minimize and Copy working directory (Ctrl+Shift+C is the
            // terminal's copy there) and adds Ctrl+Shift+P and Ctrl+F4.
            assert_eq!(n, if mac { 80 } else { 80 - 3 + 2 }, "mac: {mac}");
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
                } else if path.extension().is_some_and(|e| e == "rs")
                    && path.file_name().is_some_and(|n| n != "keys.rs")
                {
                    out.push(path);
                }
            }
        }
        let mut files = vec![];
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        assert!(files.len() > 50, "found {} files", files.len());
        let mut bare = vec![];
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            let mut localized_where_used = false;
            for (n, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("#[cfg(test)]")
                    && text
                        .lines()
                        .nth(n + 1)
                        .is_some_and(|next| next.trim_start().starts_with("mod "))
                {
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
                if has_glyphs(code)
                    && !line.contains("keys::")
                    && !line.contains("icon_button(")
                    && !TITLE_BAR
                        .iter()
                        .any(|(f, s)| path.ends_with(f) && line.contains(s))
                {
                    bare.push(format!(
                        "{}:{}: {}",
                        path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                            .unwrap()
                            .display(),
                        n + 1,
                        line.trim()
                    ));
                }
            }
        }
        assert!(
            bare.is_empty(),
            "key glyphs shown as they are (wrap them in keys::localize):\n{}",
            bare.join("\n")
        );
    }
}
