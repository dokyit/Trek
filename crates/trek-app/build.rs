//! Windows: embed Trek's icon (and a name and version for Task Manager and the file's Properties)
//! in `trek.exe`. GPUI loads the window's icon from resource 1 of the executable, which is what
//! the title bar and the taskbar then show. Nothing on other platforms.
//!
//! The `.ico`s in `assets/brand` come from `script/windows-icons.py`: Ember (resource 1, the exe's
//! icon) and the two other choices of Appearance › App icon (resources 2 and 3), which Trek puts
//! on its windows at run time (`winsys::icon_resource` is the app's side of these ids, and a test
//! reads the list below). GPUI's own resource (its manifest) has other type and id, so they link
//! side by side.

/// (resource id, file in `assets/brand`).
#[cfg_attr(not(windows), allow(dead_code))]
const ICONS: [(u16, &str); 3] = [(1, "trek.ico"), (2, "trek-night.ico"), (3, "trek-glass.ico")];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows();
    }
}

#[cfg(windows)]
fn windows() {
    use std::path::PathBuf;
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // rc.exe reads the paths as C source: forward slashes, so no escapes to get wrong.
    let icons: String = ICONS
        .iter()
        .map(|(id, file)| {
            let ico = manifest.join("../../assets/brand").join(file).canonicalize().unwrap_or_else(|e| panic!("assets/brand/{file}: {e}"));
            println!("cargo:rerun-if-changed={}", ico.display());
            format!("{id} ICON \"{}\"\n", ico.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/"))
        })
        .collect();

    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    // 1.2.3-beta.4 → 1,2,3,0: the numeric fields take the numbers only.
    let mut numbers = version.split(['.', '-']).map(|p| p.parse::<u32>().unwrap_or(0));
    let [major, minor, patch] = std::array::from_fn(|_| numbers.next().unwrap_or(0));
    let rc = format!(
        r#"{icons}1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "Trek"
      VALUE "ProductName", "Trek"
      VALUE "InternalName", "trek"
      VALUE "OriginalFilename", "trek.exe"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("trek.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_required().unwrap();
}

#[cfg(not(windows))]
fn windows() {}
