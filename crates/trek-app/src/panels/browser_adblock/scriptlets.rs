//! uBlock Origin's scriptlets and redirect resources, for Trek's ad blocker: the small scripts
//! its filters run in a page (`example.com##+js(set-constant, …)`, `json-prune`, the YouTube
//! fixes) and the stand-ins a `$redirect=` filter serves (`noop.js`, `1x1.gif`…).
//!
//! They are uBlock Origin's (GPLv3), and none of them ships with Trek (Apache-2.0): Trek
//! downloads uBlock Origin's own release from GitHub into the user's data folder
//! (`<data>/adblock/ubo/<version>`), as it downloads the filter lists, and reads them from
//! there. They are kept as uBlock Origin wrote them; Trek runs them, unchanged, in pages, as the
//! filter lists ask. The release's scriptlets are ES modules; [`bundle`] puts them into one plain
//! script that evaluates them (each module in a function of its own, `import`/`export` made into
//! a table of what each exports) and answers with each scriptlet's name, aliases, dependencies,
//! whether it needs a trusted list, and its function's source, which is what Brave's `adblock`
//! crate injects (`Resource`, `fn/javascript` for a dependency).

use regex::Regex;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

/// Where the releases are listed.
const RELEASES: &str = "https://api.github.com/repos/gorhill/uBlock/releases/latest";
/// How often the release is looked for again.
pub const EVERY_HOURS: u64 = 7 * 24;

pub fn dir(base: &Path) -> PathBuf {
    base.join("ubo")
}

/// The release unpacked last (its folder), if any.
pub fn current(base: &Path) -> Option<PathBuf> {
    let version = std::fs::read_to_string(dir(base).join("current")).ok()?;
    let folder = dir(base).join(version.trim());
    folder
        .join("js/resources/scriptlets.js")
        .is_file()
        .then_some(folder)
}

pub fn stale(base: &Path) -> bool {
    let age = std::fs::metadata(dir(base).join("current"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok());
    age.is_none_or(|a| a > Duration::from_secs(EVERY_HOURS * 3600))
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Whether a file of the release is one Trek keeps: the scriptlets' modules (and what they
/// import from `js/`), the redirect resources and their list.
fn kept(path: &str) -> bool {
    (path.starts_with("js/") && path.ends_with(".js"))
        || path.starts_with("web_accessible_resources/")
}

/// Download uBlock Origin's latest release (its Chromium package) and unpack what Trek reads
/// into `<base>/ubo/<version>`; the folder. Nothing is fetched when that version is there.
pub async fn fetch(base: &Path) -> Result<PathBuf, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent("Trek")
        .build()
        .map_err(|e| e.to_string())?;
    let release: Release = client
        .get(RELEASES)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("GitHub didn't list uBlock Origin's releases ({e})."))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let version: String = release
        .tag_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        .collect();
    if version.is_empty() {
        return Err("The release has no version.".into());
    }
    let folder = dir(base).join(&version);
    if folder.join("js/resources/scriptlets.js").is_file() {
        let _ = std::fs::write(dir(base).join("current"), &version);
        return Ok(folder);
    }
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with(".chromium.zip"))
        .ok_or("The release has no Chromium package.")?;
    if !asset
        .browser_download_url
        .starts_with("https://github.com/")
    {
        return Err("The package isn't on GitHub.".into());
    }
    let bytes = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("uBlock Origin's package didn't download ({e})."))?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    if bytes.len() > 32 << 20 {
        return Err("uBlock Origin's package is too large.".into());
    }
    let staging = dir(base).join(format!("{version}.partial"));
    let _ = std::fs::remove_dir_all(&staging);
    unpack(&bytes, &staging)?;
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::rename(&staging, &folder).map_err(|e| e.to_string())?;
    let _ = std::fs::write(dir(base).join("current"), &version);
    // Older releases go.
    if let Ok(entries) = std::fs::read_dir(dir(base)) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() && p != folder {
                let _ = std::fs::remove_dir_all(p);
            }
        }
    }
    Ok(folder)
}

/// Unpack the files Trek keeps from the package's zip (`bytes`) into `to` (its top folder, the
/// extension's own, taken off; nothing outside `to`).
fn unpack(bytes: &[u8], to: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut any = false;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        if file.is_dir() {
            continue;
        }
        let Some(name) = file.enclosed_name() else {
            continue;
        };
        // `uBlock0.chromium/js/…`: the path inside the extension.
        let inner: PathBuf = name.components().skip(1).collect();
        let rel = inner.to_string_lossy().replace('\\', "/");
        if !kept(&rel)
            || inner
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            continue;
        }
        let dest = to.join(&inner);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
        std::io::copy(&mut file, &mut out).map_err(|e| e.to_string())?;
        any = true;
    }
    if !any || !to.join("js/resources/scriptlets.js").is_file() {
        return Err("uBlock Origin's package has no scriptlets.".into());
    }
    Ok(())
}

static IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?ms)^import\s+(?:([^;]*?)\s+from\s+)?['"]([^'"]+)['"]\s*;"#).unwrap()
});
static EXPORT_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^export\s+((?:async\s+)?function\*?\s+|const\s+|let\s+|var\s+|class\s+)([A-Za-z_$][\w$]*)")
        .unwrap()
});
static EXPORT_LIST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^export\s*\{([^}]*)\}\s*;?").unwrap());
static EXPORT_DEFAULT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^export\s+default\s+").unwrap());
static NAME_AS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_$][\w$]*)\s+as\s+([A-Za-z_$][\w$]*)$").unwrap());

/// `spec` (`./x.js`, `../y.js`) from module `from` (a path under the release's `js/`), as a key.
fn resolve(from: &str, spec: &str) -> Option<String> {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for p in spec.split('/') {
        match p {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// One module (`key`, its `text`) as a function of the table: `__mods[key] = function (__exports,
/// __req) { … }`, its imports read from `__req`, its exports put on `__exports`.
fn transform(key: &str, text: &str) -> (String, Vec<String>) {
    let mut deps = vec![];
    let body = IMPORT.replace_all(text, |c: &regex::Captures| {
        let Some(target) = resolve(key, &c[2]) else {
            return String::new();
        };
        deps.push(target.clone());
        let req = format!("__req({})", js_string(&target));
        let Some(clause) = c.get(1).map(|m| m.as_str().trim()) else {
            return format!("{req};");
        };
        if let Some(name) = clause.strip_prefix("* as ") {
            return format!("const {} = {req};", name.trim());
        }
        let mut out = String::new();
        let mut rest = clause;
        if !rest.starts_with('{') {
            let (default, more) = rest.split_once(',').unwrap_or((rest, ""));
            out.push_str(&format!("const {} = {req}.default;", default.trim()));
            rest = more.trim();
        }
        if let Some(inner) = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
            let binds: Vec<String> = inner
                .split(',')
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .map(|b| match NAME_AS.captures(b) {
                    Some(m) => format!("{}: {}", &m[1], &m[2]),
                    None => b.to_string(),
                })
                .collect();
            out.push_str(&format!(" const {{ {} }} = {req};", binds.join(", ")));
        }
        out
    });
    let mut names: Vec<(String, String)> = vec![];
    let body = EXPORT_DECL.replace_all(&body, |c: &regex::Captures| {
        names.push((c[2].to_string(), c[2].to_string()));
        format!("{}{}", &c[1], &c[2])
    });
    let body = EXPORT_LIST.replace_all(&body, |c: &regex::Captures| {
        for b in c[1].split(',').map(str::trim).filter(|b| !b.is_empty()) {
            match NAME_AS.captures(b) {
                Some(m) => names.push((m[1].to_string(), m[2].to_string())),
                None => names.push((b.to_string(), b.to_string())),
            }
        }
        String::new()
    });
    let body = EXPORT_DEFAULT.replace_all(&body, "__exports.default = ");
    // Functions are hoisted: exported first too, for modules that reach each other early.
    let head: String = names
        .iter()
        .map(|(local, as_)| {
            format!(
                "try {{ __exports[{}] = {local}; }} catch {{}}",
                js_string(as_)
            )
        })
        .collect();
    let tail: String = names
        .iter()
        .map(|(local, as_)| format!("__exports[{}] = {local};", js_string(as_)))
        .collect();
    (
        format!(
            "__mods[{}] = function (__exports, __req) {{ {head}\n{body}\n{tail} }};",
            js_string(key)
        ),
        deps,
    )
}

/// One plain script from the release's scriptlets (`<release>/js/resources/scriptlets.js` and
/// what it imports), which evaluates to a JSON string of the scriptlets (see the module's notes).
pub fn bundle(release: &Path) -> Result<String, String> {
    let js = release.join("js");
    let start = "resources/scriptlets.js".to_string();
    let mut modules: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = vec![];
    let mut todo = vec![start.clone()];
    let mut seen = HashSet::new();
    while let Some(key) = todo.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        if key.contains("..") || key.starts_with('/') {
            return Err(format!("A module outside the release: {key}"));
        }
        let text = std::fs::read_to_string(js.join(&key))
            .map_err(|e| format!("uBlock Origin's {key}: {e}"))?;
        let (code, deps) = transform(&key, &text);
        modules.insert(key.clone(), code);
        order.push(key);
        todo.extend(deps);
    }
    if modules.len() < 5 {
        return Err("uBlock Origin's scriptlets look incomplete.".into());
    }
    let mut out = String::from(
        "(() => {\nconst __mods = {}, __cache = {};\n\
         function __req(k) { if (__cache[k]) return __cache[k]; const e = __cache[k] = {}; \
         if (!__mods[k]) throw new Error('no module ' + k); __mods[k](e, __req); return e; }\n",
    );
    for key in &order {
        out.push_str(&modules[key]);
        out.push('\n');
    }
    out.push_str(&format!(
        "return JSON.stringify(__req({}).builtinScriptlets.map(s => ({{ name: s.name, \
         aliases: s.aliases || [], dependencies: (s.dependencies || []).map(d => typeof d === 'function' ? d.details.name : d), \
         requiresTrust: !!s.requiresTrust, world: s.world || 'MAIN', fn: s.fn.toString() }})));\n}})()",
        js_string(&start)
    ));
    Ok(out)
}

/// A scriptlet as `bundle`'s script answers it.
#[derive(Debug, Clone, Deserialize)]
pub struct Scriptlet {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default, rename = "requiresTrust")]
    pub requires_trust: bool,
    #[serde(default)]
    pub world: String,
    #[serde(rename = "fn")]
    pub source: String,
}

/// The permission a scriptlet that needs a trusted list asks (uBlock Origin's `requiresTrust`):
/// only uBlock Origin's own lists and the user's filters may use those.
pub const TRUSTED: adblock::resources::PermissionMask =
    adblock::resources::PermissionMask::from_bits(1);

/// The scriptlets as the `adblock` crate's resources: a `.fn` one is a dependency
/// (`fn/javascript`), the others are injected by name.
pub fn resources(scriptlets: &[Scriptlet]) -> Vec<adblock::resources::Resource> {
    use adblock::resources::{MimeType, Resource, ResourceType};
    use base64::Engine as _;
    scriptlets
        .iter()
        .filter(|s| !s.name.is_empty() && !s.source.is_empty())
        .map(|s| Resource {
            name: s.name.clone(),
            aliases: s.aliases.clone(),
            kind: ResourceType::Mime(if s.name.ends_with(".fn") {
                MimeType::FnJavascript
            } else {
                MimeType::ApplicationJavascript
            }),
            content: base64::engine::general_purpose::STANDARD.encode(&s.source),
            dependencies: s.dependencies.clone(),
            permission: if s.requires_trust {
                TRUSTED
            } else {
                Default::default()
            },
        })
        .collect()
}

/// The redirect resources (`noop.js`, `1x1.gif`, `googletagservices_gpt.js`…) of release
/// `release`, from its `web_accessible_resources` and the list of their names and aliases.
pub fn redirect_resources(release: &Path) -> Vec<adblock::resources::Resource> {
    let map = release.join("js/redirect-resources.js");
    let war = release.join("web_accessible_resources");
    if !map.is_file() || !war.is_dir() {
        return vec![];
    }
    // The crate's reader stops on a file it expects and doesn't find: none of that reaches Trek.
    std::panic::catch_unwind(|| {
        adblock::resources::resource_assembler::assemble_web_accessible_resources(&war, &map)
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_and_exports_become_the_table() {
        let (code, deps) = transform(
            "resources/a.js",
            "import { b, c as d } from './b.js';\nimport e from '../e.js';\nimport './side.js';\n\
             export function f() { return b + d; }\nexport const g = 1;\nconst h = 2;\nexport { h as i };\nexport default f;\n",
        );
        assert_eq!(deps, vec!["resources/b.js", "e.js", "resources/side.js"]);
        assert!(code.contains(r#"const { b, c: d } = __req("resources/b.js");"#));
        assert!(code.contains(r#"const e = __req("e.js").default;"#));
        assert!(code.contains(r#"__req("resources/side.js");"#));
        assert!(code.contains("function f() {"));
        assert!(code.contains(r#"__exports["f"] = f;"#));
        assert!(code.contains(r#"__exports["g"] = g;"#));
        assert!(code.contains(r#"__exports["i"] = h;"#));
        assert!(code.contains("__exports.default = f;"));
        assert!(!code.contains("export "));
        assert!(!code.contains("import "));
    }

    #[test]
    fn a_path_never_leaves_the_release() {
        assert_eq!(
            resolve("resources/a.js", "./b.js").as_deref(),
            Some("resources/b.js")
        );
        assert_eq!(
            resolve("resources/a.js", "../c.js").as_deref(),
            Some("c.js")
        );
        assert_eq!(resolve("a.js", "../../x.js"), None);
        assert!(kept("js/resources/set-constant.js"));
        assert!(kept("web_accessible_resources/noop.js"));
        assert!(!kept("_locales/en/messages.json"));
    }

    #[test]
    fn scriptlets_become_resources() {
        let s = vec![
            Scriptlet {
                name: "safe-self.fn".into(),
                aliases: vec![],
                dependencies: vec![],
                requires_trust: false,
                world: "MAIN".into(),
                source: "function safeSelf() {}".into(),
            },
            Scriptlet {
                name: "trusted-set.js".into(),
                aliases: vec!["trusted-set-constant.js".into()],
                dependencies: vec!["safe-self.fn".into()],
                requires_trust: true,
                world: "MAIN".into(),
                source: "function trustedSet() {}".into(),
            },
        ];
        let r = resources(&s);
        assert_eq!(r.len(), 2);
        assert!(matches!(
            r[0].kind,
            adblock::resources::ResourceType::Mime(adblock::resources::MimeType::FnJavascript)
        ));
        assert_eq!(r[1].dependencies, vec!["safe-self.fn"]);
        assert_eq!(r[1].permission.to_bits(), 1);
    }
}
