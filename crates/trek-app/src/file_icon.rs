//! Small coloured badges for file types ("RS", "TS", a lock for lockfiles), drawn beside paths in
//! the live activity rows, tool rows, path chips and the explorer. Glyph badges rather than
//! images: they stay crisp at any size and need no assets.

use gpui_kit::component::{ActiveTheme as _, Icon, IconName};
use gpui_kit::*;

/// What a badge shows on its fill.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mark {
    Text(&'static str),
    Lock,
    Image,
    /// A plain file: a neutral badge with a page glyph.
    File,
}

/// A file type's badge: its mark, fill and whether the mark is dark (on a light fill).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FileType {
    pub mark: Mark,
    /// `None`: the neutral fill (lockfiles, plain files).
    pub fill: Option<u32>,
    pub dark_mark: bool,
}

const fn text(label: &'static str, fill: u32) -> FileType {
    FileType { mark: Mark::Text(label), fill: Some(fill), dark_mark: false }
}

const PLAIN: FileType = FileType { mark: Mark::File, fill: None, dark_mark: false };

/// The badge for `path`, from its name and extension.
pub fn file_type(path: &str) -> FileType {
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_lowercase();
    if name.ends_with(".lock") || matches!(name.as_str(), "package-lock.json" | "pnpm-lock.yaml" | "npm-shrinkwrap.json" | "go.sum") {
        return FileType { mark: Mark::Lock, fill: None, dark_mark: false };
    }
    let Some((_, ext)) = name.rsplit_once('.') else {
        return match name.as_str() {
            "makefile" | "dockerfile" | "justfile" => text("$", 0x4E9A2E),
            _ => PLAIN,
        };
    };
    match ext {
        "rs" => text("RS", 0xC8572D),
        "ts" | "tsx" | "mts" | "cts" => text("TS", 0x3178C6),
        "js" | "jsx" | "mjs" | "cjs" => FileType { mark: Mark::Text("JS"), fill: Some(0xF0D23C), dark_mark: true },
        "py" | "pyi" => text("PY", 0x3572A5),
        "go" => text("GO", 0x00A3CC),
        "swift" => text("SW", 0xF05138),
        "kt" | "kts" => text("KT", 0x7F52FF),
        "java" => text("JV", 0xB07219),
        "rb" => text("RB", 0xCC342D),
        "md" | "mdx" | "markdown" => text("MD", 0x56677A),
        "json" | "jsonc" | "json5" => FileType { mark: Mark::Text("{}"), fill: Some(0xD9B23A), dark_mark: true },
        "toml" => text("TM", 0x9C4A26),
        "yaml" | "yml" => text("YM", 0xB8466A),
        "html" | "htm" => text("<>", 0xE34C26),
        "css" | "scss" | "sass" | "less" => text("#", 0x6B45B5),
        "sh" | "bash" | "zsh" | "fish" => text("$", 0x4E9A2E),
        "sql" => text("DB", 0x336791),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "heic" | "bmp" | "tiff" | "avif" => {
            FileType { mark: Mark::Image, fill: Some(0x8E6CC9), dark_mark: false }
        }
        _ => PLAIN,
    }
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
    match file_type(path).fill {
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

/// The badge for `path`, `size` points square.
pub fn badge(path: &str, size: Pixels, cx: &App) -> AnyElement {
    let t = file_type(path);
    let theme = cx.theme();
    let (fill, ink): (Hsla, Hsla) = match t.fill {
        Some(c) => (rgb(c).into(), if t.dark_mark { rgb(0x1F1F1F).into() } else { gpui_kit::white() }),
        None => (theme.foreground.opacity(0.09), theme.muted_foreground),
    };
    let el = div().size(size).flex_none().rounded(size * 0.24).bg(fill).flex().items_center().justify_center();
    match t.mark {
        Mark::Text(label) => el
            .child(
                div()
                    .text_size(size * if label.len() > 1 { 0.5 } else { 0.62 })
                    .line_height(size)
                    .font_weight(FontWeight::BOLD)
                    .text_color(ink)
                    .child(label),
            )
            .into_any_element(),
        Mark::Lock => el.child(Icon::new(crate::assets::Lucide::Lock).size(size * 0.66).text_color(ink)).into_any_element(),
        Mark::Image => el.child(Icon::new(crate::assets::Lucide::Image).size(size * 0.66).text_color(ink)).into_any_element(),
        Mark::File => el.child(Icon::new(IconName::File).size(size * 0.7).text_color(ink)).into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Mark, file_type};

    fn mark(path: &str) -> Mark {
        file_type(path).mark
    }

    #[test]
    fn common_types_get_their_badge() {
        for (path, want) in [
            ("src/main.rs", "RS"),
            ("web/App.tsx", "TS"),
            ("index.ts", "TS"),
            ("vite.config.mjs", "JS"),
            ("Button.jsx", "JS"),
            ("scripts/build.py", "PY"),
            ("cmd/server/main.go", "GO"),
            ("Sources/App.swift", "SW"),
            ("build.gradle.kts", "KT"),
            ("Main.java", "JV"),
            ("Gemfile.rb", "RB"),
            ("README.md", "MD"),
            ("package.json", "{}"),
            ("Cargo.toml", "TM"),
            (".github/workflows/ci.yml", "YM"),
            ("index.html", "<>"),
            ("styles/app.scss", "#"),
            ("scripts/release.sh", "$"),
            ("Makefile", "$"),
            ("migrations/0001_init.sql", "DB"),
        ] {
            assert_eq!(mark(path), Mark::Text(want), "{path}");
        }
    }

    #[test]
    fn lockfiles_images_and_the_rest() {
        assert_eq!(mark("Cargo.lock"), Mark::Lock);
        assert_eq!(mark("frontend/package-lock.json"), Mark::Lock, "a lockfile, though it's JSON");
        assert_eq!(mark("pnpm-lock.yaml"), Mark::Lock);
        assert_eq!(mark("assets/logo.PNG"), Mark::Image, "case doesn't matter");
        assert_eq!(mark("icon.svg"), Mark::Image);
        assert_eq!(mark("LICENSE"), Mark::File);
        assert_eq!(mark("notes.xyz"), Mark::File);
        assert_eq!(mark("src/"), Mark::File);
        assert_eq!(file_type("LICENSE").fill, None, "plain files are neutral");
        // Light fills carry a dark mark, so both read in Night and Paper.
        assert!(file_type("a.js").dark_mark && file_type("a.json").dark_mark && !file_type("a.rs").dark_mark);
    }
}
