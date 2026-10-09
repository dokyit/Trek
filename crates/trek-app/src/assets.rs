use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
struct Embedded;

// Extra Lucide icons beyond GPUI Kit's default set, embedded in the binary.
gpui_kit::assets::icon_assets!(pub TrekIcons, [FilePen, Zap, Lock, ListChecks, Square, ShieldCheck, Wrench, MessageSquare, CircleDashed, Asterisk, SquarePen, Paperclip, GitBranch, LockOpen, Hand, ChevronsUpDown, ChevronsDownUp, Sparkle, FolderPlus, ChartNoAxesColumn, GitCompare, Laptop, Camera, Crosshair, Code, Smartphone, Image, Monitor, Plug, Puzzle, ZoomIn, Eraser, CodeXml, House, Tablet, Power, Link, PackagePlus, AppWindow, Keyboard, Rocket, Star, Heart, Flame, Leaf, Mountain, Tent, Compass, Map, Gamepad2, Database, Server, BookOpen, Terminal, Box, Package, Music, Bug, FlaskConical, Sun, Moon, Cloud, Shield, Key, Lightbulb, Hammer, Brain, Pencil, Trash, Pin, PinOff, Archive, Clock, Play, LoaderCircle, Users, Type, BellOff, FolderCog, SquareArrowOutUpRight, GitBranchPlus, GitMerge, GitPullRequest, GitPullRequestCreate, FolderGit2, Sparkles, Undo2, Redo2, RefreshCw, GitFork, Gauge, ArrowLeftRight, MessagesSquare, CornerDownRight, CircleArrowUp, Activity, Radar, ScrollText, MessageSquareQuote, BadgeCheck, Scale, MessageSquarePlus, NotebookPen, Bold, Italic, Strikethrough, Underline, List, ListOrdered, ListTodo, Heading1, Heading2, Quote, Palette, Highlighter, StickyNote, Minus, Eye, FileDiff, Mic, Files, RotateCcwClock, Infinity, HandHelping, MessageCircleQuestionMark, TextCursorInput, ArrowUpRight, ArrowDownRight, Maximize2, Columns2, Rows2, FilePlus, ChartBarBig, ChartSpline, Table2, Grid3x3, ChartGantt, Workflow, Layers, Braces, Sheet, Wifi, SignalHigh, X]);

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
        let mut paths: Vec<SharedString> = Embedded::iter().filter(|p| p.starts_with(path)).map(|p| p.to_string().into()).collect();
        paths.extend(gpui_kit::assets::Assets.list(path)?);
        paths.extend(TrekIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

pub fn theme_json() -> String {
    Embedded::get("themes/trek.json").map(|f| String::from_utf8_lossy(&f.data).to_string()).unwrap_or_default()
}

pub fn brand_bytes(path: &str) -> Option<Vec<u8>> {
    Embedded::get(path).map(|f| f.data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Lucide::Name` in kebab case, as Lucide names its files (`Grid3x3` → `grid-3x3`).
    fn file_name(name: &str) -> String {
        let mut out = String::new();
        let mut prev: Option<char> = None;
        for c in name.chars() {
            let starts = match prev {
                Some(p) => c.is_ascii_uppercase() || (c.is_ascii_digit() && p.is_ascii_alphabetic() && p != 'x'),
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
        let mut files = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))];
        while let Some(at) = files.pop() {
            if at.is_dir() {
                files.extend(std::fs::read_dir(&at).unwrap().map(|e| e.unwrap().path()));
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&at) else { continue };
            for prefix in ["Lucide::", "IconName::"] {
                for (i, _) in text.match_indices(prefix) {
                    let name: String = text[i + prefix.len()..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
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
        assert!(missing.is_empty(), "not embedded (add to TrekIcons): {missing:#?}");
    }
}
