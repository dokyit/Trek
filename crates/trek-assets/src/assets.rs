use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
// The fonts are compiled in by `fonts.rs` (DirectWrite needs them in memory for good): once is enough.
#[exclude = "fonts/*"]
struct Embedded;

// Extra Lucide icons beyond GPUI Kit's default set, embedded in the binary.
gpui_kit::assets::icon_assets!(pub TrekIcons, [FilePen, Zap, Lock, ListChecks, Square, ShieldCheck, Wrench, MessageSquare, CircleDashed, Asterisk, SquarePen, Paperclip, GitBranch, LockOpen, Hand, ChevronsUpDown, ChevronsDownUp, Sparkle, FolderPlus, ChartNoAxesColumn, GitCompare, Laptop, Camera, Crosshair, Code, Smartphone, Image, ImagePlus, ImageOff, Monitor, Plug, Puzzle, ZoomIn, Eraser, CodeXml, House, Tablet, Power, Link, PackagePlus, AppWindow, Keyboard, Rocket, Star, Heart, Flame, Leaf, Mountain, Tent, Compass, Map, Gamepad2, Database, Server, BookOpen, Terminal, Box, Package, Music, Bug, FlaskConical, Sun, Moon, Cloud, Shield, Key, Lightbulb, Hammer, Brain, Pencil, Trash, Pin, PinOff, Archive, Inbox, Clock, Play, LoaderCircle, Users, Type, BellOff, FolderCog, SquareArrowOutUpRight, GitBranchPlus, GitMerge, GitPullRequest, GitPullRequestCreate, FolderGit2, Sparkles, Undo2, Redo2, RefreshCw, GitFork, Gauge, ArrowLeftRight, MessagesSquare, CornerDownRight, CircleArrowUp, Activity, Radar, ScrollText, MessageSquareQuote, BadgeCheck, Scale, MessageSquarePlus, NotebookPen, Bold, Italic, Strikethrough, Underline, List, ListOrdered, ListTodo, Heading1, Heading2, Quote, Palette, Highlighter, StickyNote, Minus, Eye, FileDiff, Mic, Files, RotateCcwClock, Infinity, HandHelping, MessageCircleQuestionMark, TextCursorInput, ArrowUpRight, ArrowDownRight, Maximize2, Columns2, Rows2, FilePlus, ChartBarBig, ChartSpline, Table2, Grid3x3, ChartGantt, Workflow, Layers, Braces, Sheet, Wifi, SignalHigh, X, Share, SunMoon, Contrast, CircleSlash, CircleCheck, CircleX, Check, Watch, Tv, Glasses, Download, GraduationCap, Target, Route, Footprints, Repeat]);

/// The full Lucide catalog (only icons listed in `TrekIcons` or the defaults are embedded).
pub use gpui_kit::assets::IconName as Lucide;

/// Trek's own assets (brand images, themes) layered over GPUI Kit's icon set.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = Embedded::get(path) {
            return Ok(Some(file.data));
        }
        if let Some(bytes) = TrekIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths: Vec<SharedString> = Embedded::iter()
            .filter(|p| p.starts_with(path))
            .map(|p| p.to_string().into())
            .collect();
        paths.extend(gpui_kit::assets::Assets.list(path)?);
        paths.extend(TrekIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// The kit's radii on Windows: its controls take 4 px corners and its dialogs and notifications 8,
/// as `fluent` gives Trek's own.
const WINDOWS_RADIUS: u32 = 4;
const WINDOWS_RADIUS_LG: u32 = 8;

fn shipped_theme() -> String {
    Embedded::get("themes/trek.json")
        .map(|f| String::from_utf8_lossy(&f.data).to_string())
        .unwrap_or_default()
}

/// The theme the kit's components draw from. `trek.json` has the Mac's radii, and Windows' go in
/// here rather than on `Theme`, as a mode change reapplies the config over anything set there.
pub fn theme_json() -> String {
    let json = shipped_theme();
    if cfg!(windows) {
        with_radii(&json, WINDOWS_RADIUS, WINDOWS_RADIUS_LG)
    } else {
        json
    }
}

/// `json`, a theme set, with every theme's radii replaced. Parsed rather than searched, so each
/// theme gets both whatever the file's spacing; a text that doesn't parse is the loader's to report.
fn with_radii(json: &str, radius: u32, radius_lg: u32) -> String {
    let Ok(mut set) = serde_json::from_str::<serde_json::Value>(json) else {
        return json.to_string();
    };
    let themes = set.get_mut("themes").and_then(|t| t.as_array_mut());
    for theme in themes
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_object_mut())
    {
        theme.insert("radius".to_string(), radius.into());
        theme.insert("radius.lg".to_string(), radius_lg.into());
    }
    set.to_string()
}

pub fn brand_bytes(path: &str) -> Option<Vec<u8>> {
    Embedded::get(path).map(|f| f.data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The themes as the kit reads them: the same parse its registry does.
    fn themes(json: &str) -> Vec<gpui_kit::component::ThemeConfig> {
        serde_json::from_str::<gpui_kit::component::ThemeSet>(json)
            .expect("a theme set the kit can read")
            .themes
    }

    #[test]
    fn windows_radii_change_nothing_else_in_the_theme() {
        let shipped = shipped_theme();
        let patched = with_radii(&shipped, WINDOWS_RADIUS, WINDOWS_RADIUS_LG);
        let read = themes(&patched);
        assert_eq!(read.len(), 2, "Night and Paper");
        for theme in &read {
            assert_eq!(theme.radius, Some(4), "{}", theme.name);
            assert_eq!(theme.radius_lg, Some(8), "{}", theme.name);
            // `tests/look.rs` holds the app to these, in both themes.
            assert_eq!(
                theme.font_family.as_deref(),
                Some("Inter"),
                "{}",
                theme.name
            );
            assert_eq!(
                theme.mono_font_family.as_deref(),
                Some("JetBrains Mono NL"),
                "{}",
                theme.name
            );
        }
        // With the radii taken out, what's left is what shipped.
        let bare = |json: &str| {
            let mut set: serde_json::Value = serde_json::from_str(json).unwrap();
            for theme in set["themes"].as_array_mut().unwrap() {
                let theme = theme.as_object_mut().unwrap();
                theme.remove("radius");
                theme.remove("radius.lg");
            }
            set
        };
        assert_eq!(bare(&patched), bare(&shipped));
    }

    #[test]
    fn trek_json_keeps_the_macs_radii_and_only_windows_gets_its_own() {
        for theme in themes(&shipped_theme()) {
            assert_eq!((theme.radius, theme.radius_lg), (Some(8), Some(12)));
        }
        let want = if cfg!(windows) { (4, 8) } else { (8, 12) };
        for theme in themes(&theme_json()) {
            assert_eq!(
                (theme.radius, theme.radius_lg),
                (Some(want.0), Some(want.1))
            );
        }
    }

    /// `Lucide::Name` in kebab case, as Lucide names its files (`Grid3x3` → `grid-3x3`).
    fn file_name(name: &str) -> String {
        let mut out = String::new();
        let mut prev: Option<char> = None;
        for c in name.chars() {
            let starts = match prev {
                Some(p) => {
                    c.is_ascii_uppercase()
                        || (c.is_ascii_digit() && p.is_ascii_alphabetic() && p != 'x')
                }
                None => false,
            };
            if starts {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
            prev = Some(c);
        }
        out
    }

    /// Every icon the code draws is embedded: one left out draws nothing and logs an error on
    /// every frame (the browser's Stop button did).
    #[test]
    fn every_icon_used_is_embedded() {
        let catalog = gpui_kit::assets::AllAssets;
        let mut missing = vec![];
        let mut files = vec![std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src"
        ))];
        while let Some(at) = files.pop() {
            if at.is_dir() {
                files.extend(std::fs::read_dir(&at).unwrap().map(|e| e.unwrap().path()));
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&at) else {
                continue;
            };
            for prefix in ["Lucide::", "IconName::"] {
                for (i, _) in text.match_indices(prefix) {
                    let name: String = text[i + prefix.len()..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric())
                        .collect();
                    let path = format!("icons/{}.svg", file_name(&name));
                    // Not a Lucide file name (the guess is wrong, or it isn't an icon): skipped.
                    if name.is_empty() || !catalog.load(&path).is_ok_and(|f| f.is_some()) {
                        continue;
                    }
                    if !Assets.load(&path).is_ok_and(|f| f.is_some()) {
                        missing.push(format!("{name} ({})", at.display()));
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "not embedded (add to TrekIcons): {missing:#?}"
        );
    }
}
