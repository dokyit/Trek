use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
struct Embedded;

// Extra Lucide icons beyond GPUI Kit's default set, embedded in the binary.
gpui_kit::assets::icon_assets!(pub TrekIcons, [FilePen, Zap, Lock, ListChecks, Square, ShieldCheck, Wrench, MessageSquare, CircleDashed, Asterisk, SquarePen, Paperclip, GitBranch, LockOpen, Hand, ChevronsUpDown, Sparkle, FolderPlus, ChartNoAxesColumn, GitCompare, Laptop, Camera, Crosshair, Code, Smartphone, Image, Monitor, Plug, Puzzle, ZoomIn, Eraser, CodeXml, House, Tablet, Power, Link, PackagePlus, AppWindow, Keyboard, Rocket, Star, Heart, Flame, Leaf, Mountain, Tent, Compass, Map, Gamepad2, Database, Server, BookOpen, Terminal, Box, Package, Music, Bug, FlaskConical, Sun, Moon, Cloud, Shield, Key, Lightbulb, Hammer, Brain, Pencil, Trash, Pin, PinOff, Archive, Clock, Play, LoaderCircle, Users, Type, BellOff, FolderCog, SquareArrowOutUpRight, Undo2, RefreshCw, GitFork]);

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
