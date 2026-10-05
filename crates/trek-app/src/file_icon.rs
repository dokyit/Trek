//! File and folder icons, and the colours of file chips. The icons are Catppuccin's
//! (https://github.com/catppuccin/zed-icons, MIT; imported by `script/file-icons.py`): Mocha in
//! the dark theme, Latte in the light one, drawn beside paths in the live activity rows, tool
//! rows, path chips, changed-files cards and the explorer. A chip's colours come from its file
//! type (`file_type`).

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::sync::LazyLock;

/// The colour of `path`'s type, for its chip: `None` for lockfiles and plain files, which stay
/// neutral.
pub fn type_colour(path: &str) -> Option<u32> {
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_lowercase();
    if name.ends_with(".lock") || matches!(name.as_str(), "package-lock.json" | "pnpm-lock.yaml" | "npm-shrinkwrap.json" | "go.sum") {
        return None;
    }
    let Some((_, ext)) = name.rsplit_once('.') else {
        return matches!(name.as_str(), "makefile" | "dockerfile" | "justfile").then_some(0x4E9A2E);
    };
    Some(match ext {
        "rs" => 0xC8572D,
        "ts" | "tsx" | "mts" | "cts" => 0x3178C6,
        "js" | "jsx" | "mjs" | "cjs" => 0xF0D23C,
        "py" | "pyi" => 0x3572A5,
        "go" => 0x00A3CC,
        "swift" => 0xF05138,
        "kt" | "kts" => 0x7F52FF,
        "java" => 0xB07219,
        "rb" => 0xCC342D,
        "md" | "mdx" | "markdown" => 0x56677A,
        "json" | "jsonc" | "json5" => 0xD9B23A,
        "toml" => 0x9C4A26,
        "yaml" | "yml" => 0xB8466A,
        "html" | "htm" => 0xE34C26,
        "css" | "scss" | "sass" | "less" => 0x6B45B5,
        "sh" | "bash" | "zsh" | "fish" => 0x4E9A2E,
        "sql" => 0x336791,
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "heic" | "bmp" | "tiff" | "avif" => 0x8E6CC9,
        _ => return None,
    })
}

/// A file chip's colours, from its type: a wash of the type's colour behind the name, a rim of
/// it, and the name in a shade of it that reads on either theme. Plain files and lockfiles
/// stay neutral.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tint {
    pub fill: Hsla,
    pub edge: Hsla,
    pub ink: Hsla,
}

/// The chip colours for `path`'s type.
pub fn tint(path: &str, cx: &App) -> Tint {
    match type_colour(path) {
        Some(c) => tint_of(rgb(c).into(), cx),
        None => {
            let theme = cx.theme();
            Tint { fill: theme.foreground.opacity(0.06), edge: theme.foreground.opacity(0.14), ink: theme.foreground.opacity(0.9) }
        }
    }
}

/// Chip colours in `color`'s hue (a type's colour, or a project's for its folders).
pub fn tint_of(color: Hsla, cx: &App) -> Tint {
    tint_in(color, cx.theme().mode.is_dark())
}

/// `tint_of` in the dark theme or the light one.
pub fn tint_in(color: Hsla, dark: bool) -> Tint {
    // Yellows through cyans are bright for their shade: on paper their ink comes down further.
    let bright = (0.09..0.55).contains(&color.h);
    let ink = Hsla { s: color.s.min(0.75), l: if dark { 0.78 } else if bright { 0.26 } else { 0.34 - (color.l - 0.5).max(0.) * 0.3 }, a: 1., ..color };
    Tint {
        fill: Hsla { a: if dark { 0.16 } else { 0.11 }, ..color },
        edge: Hsla { a: if dark { 0.38 } else { 0.3 }, ..color },
        ink,
    }
}

/// A file's name in a chip of its type's colour, with its badge: what tool rows, live rows and
/// path chips show for a file. `label` is what's written (a name, or a path); `path` picks the
/// type.
pub fn chip(id: impl Into<ElementId>, path: &str, label: impl Into<SharedString>, size: Pixels, cx: &App) -> Stateful<Div> {
    let t = tint(path, cx);
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .min_w_0()
        .max_w_full()
        .h(size * 1.62)
        .px(size * 0.42)
        .gap(size * 0.34)
        .rounded(size * 0.42)
        .border_1()
        .border_color(t.edge)
        .bg(t.fill)
        .text_color(t.ink)
        .text_size(size)
        .font_family(cx.theme().mono_font_family.clone())
        .child(badge(path, size * 1.02, cx))
        .child(div().min_w_0().truncate().child(label.into()))
}

/// Which of Catppuccin's icons goes with what (`assets/file-icons/map.json`).
#[derive(Default, serde::Deserialize)]
struct IconMap {
    /// Whole file names (`Cargo.toml`, `README.md`).
    names: HashMap<String, String>,
    /// Suffixes after a dot (`rs`, `component.ts`), and some whole names (`Dockerfile`).
    suffixes: HashMap<String, String>,
    /// Folder names; `<icon>_open` is the open one.
    folders: HashMap<String, String>,
}

static ICONS: LazyLock<IconMap> =
    LazyLock::new(|| crate::assets::brand_bytes("file-icons/map.json").and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default());

/// Looks `key` up as written, then in lower case.
fn find<'a>(map: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    map.get(key).or_else(|| map.get(&key.to_lowercase())).map(String::as_str)
}

/// The icon for a file at `path`: by its whole name, then by its suffixes, longest first.
pub fn icon_name(path: &str) -> &'static str {
    let icons = &*ICONS;
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or(path);
    if let Some(icon) = find(&icons.names, name).or_else(|| find(&icons.suffixes, name)) {
        return icon;
    }
    name.match_indices('.').filter(|(at, _)| *at > 0 || name.len() > 1).find_map(|(at, _)| find(&icons.suffixes, &name[at + 1..])).unwrap_or("_file")
}

/// The icon for a folder at `path` (`src`, `docs`, `.github`), open or closed.
pub fn folder_icon_name(path: &str, open: bool) -> String {
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or(path);
    let icon = find(&ICONS.folders, name).unwrap_or("_folder");
    if open { format!("{icon}_open") } else { icon.to_string() }
}

fn icon(name: &str, size: Pixels, cx: &App) -> AnyElement {
    let flavour = if cx.theme().mode.is_dark() { "mocha" } else { "latte" };
    img(SharedString::from(format!("file-icons/{flavour}/{name}.svg"))).size(size).flex_none().into_any_element()
}

/// The icon for `path`'s type, `size` points square.
pub fn badge(path: &str, size: Pixels, cx: &App) -> AnyElement {
    icon(icon_name(path), size, cx)
}

/// The icon for the folder at `path`, `size` points square.
pub fn folder(path: &str, open: bool, size: Pixels, cx: &App) -> AnyElement {
    icon(&folder_icon_name(path, open), size, cx)
}

#[cfg(test)]
mod tests {
    use super::{folder_icon_name, icon_name, type_colour};

    #[test]
    fn types_have_their_colours_and_the_rest_stay_neutral() {
        assert_eq!(type_colour("src/main.rs"), Some(0xC8572D));
        assert_eq!(type_colour("web/App.tsx"), type_colour("index.ts"));
        assert_eq!(type_colour("assets/logo.PNG"), Some(0x8E6CC9), "case doesn't matter");
        assert_eq!(type_colour("Makefile"), type_colour("scripts/release.sh"));
        for neutral in ["Cargo.lock", "frontend/package-lock.json", "pnpm-lock.yaml", "LICENSE", "notes.xyz", "src/"] {
            assert_eq!(type_colour(neutral), None, "{neutral}");
        }
    }

    #[test]
    fn catppuccin_icons_by_name_then_suffix() {
        for (path, want) in [
            ("README.md", "readme"),
            ("docs/notes.md", "markdown"),
            ("src/main.rs", "rust"),
            ("Cargo.toml", "cargo"),
            ("Cargo.lock", "cargo-lock"),
            ("web/app.component.ts", "angular-component"),
            ("web/App.tsx", "typescript-react"),
            ("Dockerfile", "docker"),
            ("unknown.qqq", "_file"),
            ("LICENSE", "license"),
        ] {
            assert_eq!(icon_name(path), want, "{path}");
        }
        assert_eq!(folder_icon_name("crates/app/src", false), "folder_src");
        assert_eq!(folder_icon_name("src/", true), "folder_src_open");
        assert_eq!(folder_icon_name("zzz-not-a-name", false), "_folder");
        for name in ["readme", "markdown", "rust", "_file", "folder_src_open", "_folder"] {
            for flavour in ["mocha", "latte"] {
                assert!(crate::assets::brand_bytes(&format!("file-icons/{flavour}/{name}.svg")).is_some(), "{flavour}/{name}");
            }
        }
    }
}
