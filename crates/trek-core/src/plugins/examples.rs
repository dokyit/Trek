//! The example plugins Trek ships (`plugins/examples/` in Trek's source): each one's files, written
//! out when the user installs it (`trek plugin install example:<name>`, or Settings › Plugins).
//! They're Trek's own work and its showcase of what a plugin can do; a test installs each.

use std::path::Path;

/// A file of an example: its path in the plugin's folder, and its text.
pub type File = (&'static str, &'static str);

/// An example's name and its files, read from `plugins/examples/<name>/` at build time.
#[allow(unused_macros)]
macro_rules! example {
    ($name:literal: $($file:literal),* $(,)?) => {
        (
            $name,
            &[$(($file, include_str!(concat!("../../../../plugins/examples/", $name, "/", $file)))),*] as &[File],
        )
    };
}

/// Every example: its name (its id) and its files.
pub const ALL: &[(&str, &[File])] = &[];

/// An example's name and description, from its manifest.
pub fn about(files: &[File]) -> (String, String) {
    let table = files
        .iter()
        .find(|(p, _)| *p == super::MANIFEST)
        .and_then(|(_, t)| toml::from_str::<toml::Table>(t).ok());
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

/// Write example `name`'s files into `dest`.
pub fn write(name: &str, dest: &Path) -> Result<(), String> {
    let (_, files) = ALL.iter().find(|(n, _)| *n == name).ok_or_else(|| {
        format!(
            "there's no example called {name}; Trek has {}",
            ALL.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
        )
    })?;
    for (rel, text) in *files {
        let path = dest.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
    }
    Ok(())
}
