//! Paths on the phone, as AFC sees them: rooted at the media folder (`/var/mobile/Media`), or at
//! an app's container when it's that app's Documents.

/// A file or folder on the phone.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
}

/// `path` as AFC wants it: one leading slash, no trailing one, no empty or `.` parts.
pub fn normalize(path: &str) -> String {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    format!("/{}", parts.join("/"))
}

/// `name` inside folder `dir`.
pub fn join(dir: &str, name: &str) -> String {
    normalize(&format!("{dir}/{name}"))
}

/// The folders above `path`, outermost first (`/a/b/c.txt` → `/a`, `/a/b`).
pub fn parents(path: &str) -> Vec<String> {
    let path = normalize(path);
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    (1..parts.len())
        .map(|n| format!("/{}", parts[..n].join("/")))
        .collect()
}

/// A path that may be written or removed: normalized, inside the jail, and (in the media folder)
/// not in the Camera Roll, where Photos keeps its own index of the files.
pub fn check_remote(path: &str, in_app: bool) -> Result<String, String> {
    if path.split('/').any(|p| p == "..") {
        return Err(format!("{path}: \"..\" isn't allowed in iPhone paths"));
    }
    let path = normalize(path);
    if path == "/" {
        return Err("Give a file's path, not the top folder".into());
    }
    if !in_app {
        let top = path.split('/').nth(1).unwrap_or_default();
        if top.eq_ignore_ascii_case("DCIM") || top.eq_ignore_ascii_case("PhotoData") {
            return Err(
                "The Camera Roll (/DCIM) is Photos' own: files written or removed there leave Photos with broken entries. Use /Downloads, or an app's Documents."
                    .into(),
            );
        }
    } else if !path.starts_with("/Documents/") {
        return Err(format!(
            "{path}: in an app, files go under /Documents (e.g. /Documents/{})",
            path.trim_start_matches('/')
        ));
    }
    Ok(path)
}

/// Folders first, then by name.
pub fn sort(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// A size as people read it ("1.4 MB").
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut unit = "B";
    for u in UNITS {
        v /= 1000.;
        unit = u;
        if v < 1000. {
            break;
        }
    }
    format!("{v:.1} {unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_normalize_and_join() {
        assert_eq!(normalize("Downloads//a/./b/"), "/Downloads/a/b");
        assert_eq!(normalize(""), "/");
        assert_eq!(join("/Downloads", "x.txt"), "/Downloads/x.txt");
        assert_eq!(join("/", "x"), "/x");
        assert_eq!(parents("/a/b/c.txt"), vec!["/a", "/a/b"]);
        assert!(parents("/c.txt").is_empty());
    }

    #[test]
    fn writes_stay_out_of_the_camera_roll_and_the_jail() {
        assert_eq!(
            check_remote("Downloads/t.txt", false).unwrap(),
            "/Downloads/t.txt"
        );
        assert!(check_remote("/DCIM/100APPLE/IMG_1.PNG", false).is_err());
        assert!(check_remote("/dcim/x", false).is_err());
        assert!(check_remote("/Downloads/../DCIM/x", false).is_err());
        assert!(check_remote("/", false).is_err());
        // In an app it's Documents only, where DCIM is just a name.
        assert!(check_remote("/Documents/DCIM/x", true).is_ok());
        assert!(check_remote("/Library/Preferences/x.plist", true).is_err());
    }

    #[test]
    fn listings_put_folders_first() {
        let e = |name: &str, dir| Entry {
            name: name.into(),
            dir,
            size: 0,
        };
        let mut v = vec![
            e("b.txt", false),
            e("Z", true),
            e("a.txt", false),
            e("c", true),
        ];
        sort(&mut v);
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["c", "Z", "a.txt", "b.txt"]);
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(1_400_000), "1.4 MB");
        assert_eq!(human_size(2_000_000_000), "2.0 GB");
    }
}
