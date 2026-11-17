//! The example plugins Trek ships (`plugins/examples/` in Trek's source): each one's files, written
//! out when the user installs it (`trek plugin install example:<name>`, or Settings › Plugins).
//! Each is what an agent could make from one sentence (its README's first line, "Ask an agent:
//! …"), and together they use every way a plugin changes Trek: a look, slots, a page, a panel, a
//! replacement, commands with keys, hooks and logic. A test installs each.

use std::path::Path;

/// A file of an example: its path in the plugin's folder, and its bytes.
pub type File = (&'static str, &'static [u8]);

/// An example's name and its files, read from `plugins/examples/<name>/` at build time.
#[allow(unused_macros)]
macro_rules! example {
    ($name:literal: $($file:literal),* $(,)?) => {
        (
            $name,
            &[$(($file, include_bytes!(concat!("../../../../plugins/examples/", $name, "/", $file)) as &[u8])),*] as &[File],
        )
    };
}

/// Every example: its name (its id) and its files.
pub const ALL: &[(&str, &[File])] = &[
    example!("campfire": "plugin.toml", "README.md", "campfire.toml"),
    example!(
        "trailhead": "plugin.toml",
        "README.md",
        "main.js",
        "ui/screen.json",
        "art/morning.svg",
        "art/afternoon.svg",
        "art/evening.svg",
        "art/night.svg",
        "pages/prompts/index.html",
        "pages/prompts/style.css",
        "pages/prompts/app.js",
    ),
    example!(
        "marmot": "plugin.toml",
        "README.md",
        "main.js",
        "ui/buddy.json",
        "art/asleep.svg",
        "art/working.svg",
        "art/whistling.svg",
        "art/cheering.svg",
    ),
    example!(
        "soft-sounds": "plugin.toml",
        "README.md",
        "main.js",
        "panels/sounds/index.html",
        "panels/sounds/style.css",
        "panels/sounds/app.js",
        "sounds/chime.wav",
        "sounds/knock.wav",
        "sounds/drop.wav",
        "sounds/bell.wav",
    ),
    example!(
        "now-playing": "plugin.toml",
        "README.md",
        "main.js",
        "ui/status.json"
    ),
    example!("focus-timer": "plugin.toml", "README.md"),
];

/// The text of an example's file `rel`, if it has it and it's text.
fn text<'a>(files: &'a [File], rel: &str) -> Option<&'a str> {
    files
        .iter()
        .find(|(p, _)| *p == rel)
        .and_then(|(_, bytes)| std::str::from_utf8(bytes).ok())
}

/// An example's name and description, from its manifest.
pub fn about(files: &[File]) -> (String, String) {
    let table = text(files, super::MANIFEST).and_then(|t| toml::from_str::<toml::Table>(t).ok());
    let field = |k: &str| {
        table
            .as_ref()
            .and_then(|m| m.get(k))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    (field("name"), field("description"))
}

/// The sentence an example was made from: its README's first line, after "Ask an agent:".
pub fn asked(files: &[File]) -> Option<String> {
    text(files, "README.md")?
        .lines()
        .next()?
        .trim()
        .strip_prefix("Ask an agent:")
        .map(|s| {
            s.trim()
                .trim_matches('"')
                .trim_matches('“')
                .trim_matches('”')
                .to_string()
        })
        .filter(|s| !s.is_empty())
}

/// Write example `name`'s files into `dest`.
pub fn write(name: &str, dest: &Path) -> Result<(), String> {
    let (_, files) = ALL.iter().find(|(n, _)| *n == name).ok_or_else(|| {
        format!(
            "there's no example called {name}; Trek has {}",
            ALL.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
        )
    })?;
    for (rel, bytes) in *files {
        let path = dest.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}
