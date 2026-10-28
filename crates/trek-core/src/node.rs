//! Node.js for the agents that ship only as npm packages (Amp's and Pi's ACP adapters, OpenCode on
//! Windows). The others install with their vendor's own installer and need no Node.
//!
//! Before an install that runs npm, Trek looks for npm on the login PATH and where Node's
//! installers and version managers put it (`usual_dirs`). With none, it downloads the official
//! Node.js LTS build for this machine from nodejs.org, checks it against that release's
//! SHASUMS256.txt, and unpacks it into its own data folder (`root`): no administrator rights, and
//! nothing outside Trek's folder. Agents installed with that npm land beside it
//! (`npm_config_prefix`, `setup_env`), in the folder `detect::login_path` ends with, so Trek finds
//! them, runs them with that `node`, and updates them with that npm (`agent_update`: their npm
//! prefix is this folder).

use anyhow::{Context as _, Result, anyhow, bail};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where Node.js's releases are published.
pub const DIST: &str = "https://nodejs.org/dist";

/// The release index (`index.json`) is a few hundred KB; an archive some tens of MB.
const MAX_INDEX_BYTES: usize = 16 << 20;
const MAX_ARCHIVE_BYTES: u64 = 512 << 20;

/// Trek's own Node.js, laid out as nodejs.org's archive is: `<data>/node`.
pub fn root() -> PathBuf {
    crate::paths::data_dir().join("node")
}

/// The folder of `node` and `npm` in a Node.js at `root`, which is also where npm puts the
/// commands of the packages it installs there globally: `bin` on macOS, the folder itself on
/// Windows.
pub fn bin_dir_in(root: &Path) -> PathBuf {
    if cfg!(windows) {
        root.to_path_buf()
    } else {
        root.join("bin")
    }
}

/// The folder of Trek's own `node` and `npm` (whether or not it's installed yet).
pub fn bin_dir() -> PathBuf {
    bin_dir_in(&root())
}

/// npm's file name: on Windows the `.cmd` (the extensionless script beside it is for Git Bash).
const NPM: &str = if cfg!(windows) { "npm.cmd" } else { "npm" };

/// Whether Trek has installed its own Node.js.
pub fn installed() -> bool {
    bin_dir().join(NPM).is_file()
}

/// Whether running `line` needs Node: one of its commands is `npm`, `npx` or `node`.
pub fn needs_node(line: &str) -> bool {
    line.split(['&', '|', ';'])
        .any(|part| matches!(part.split_whitespace().next(), Some("npm" | "npx" | "node")))
}

/// Folders where Node's installers and version managers keep `npm` that a login PATH can lack:
/// Homebrew's and the official installer's, Volta's, the newest Node nvm has, fnm's default, asdf's
/// and mise's shims; on Windows Node's installer's, nvm for Windows', Volta's, fnm's, Scoop's and
/// Chocolatey's. `var` reads the environment.
pub fn usual_dirs(home: &Path, var: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let dir = |name: &str| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let mut dirs = Vec::new();
    if cfg!(windows) {
        let local = dir("LOCALAPPDATA").unwrap_or_else(|| home.join(r"AppData\Local"));
        let roaming = dir("APPDATA").unwrap_or_else(|| home.join(r"AppData\Roaming"));
        let program_data = dir("ProgramData").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        let program_files =
            dir("ProgramFiles").unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        dirs.push(program_files.join("nodejs"));
        dirs.extend(dir("ProgramFiles(x86)").map(|p| p.join("nodejs")));
        dirs.push(dir("NVM_SYMLINK").unwrap_or_else(|| PathBuf::from(r"C:\nvm4w\nodejs")));
        dirs.push(
            dir("VOLTA_HOME")
                .unwrap_or_else(|| local.join("Volta"))
                .join("bin"),
        );
        dirs.push(
            dir("FNM_DIR")
                .unwrap_or_else(|| roaming.join("fnm"))
                .join(r"aliases\default"),
        );
        dirs.push(
            dir("SCOOP")
                .unwrap_or_else(|| home.join("scoop"))
                .join("shims"),
        );
        dirs.push(
            dir("ChocolateyInstall")
                .unwrap_or_else(|| program_data.join("chocolatey"))
                .join("bin"),
        );
        // npm updated by itself (`npm i -g npm`) lands in its global prefix.
        dirs.push(roaming.join("npm"));
    } else {
        // Homebrew on Apple silicon; Homebrew on Intel, and nodejs.org's installer, in /usr/local.
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(
            dir("VOLTA_HOME")
                .unwrap_or_else(|| home.join(".volta"))
                .join("bin"),
        );
        let nvm = dir("NVM_DIR").unwrap_or_else(|| home.join(".nvm"));
        dirs.extend(newest_version(&nvm.join("versions/node")).map(|v| v.join("bin")));
        let fnm = dir("FNM_DIR").into_iter().chain([
            home.join(".local/share/fnm"),
            home.join("Library/Application Support/fnm"),
            home.join(".fnm"),
        ]);
        dirs.extend(fnm.map(|d| d.join("aliases/default/bin")));
        dirs.push(home.join(".asdf/shims"));
        dirs.push(home.join(".local/share/mise/shims"));
    }
    dirs
}

/// The folder in `versions` (nvm's `versions/node`) named for the highest version (`v22.11.0`).
fn newest_version(versions: &Path) -> Option<PathBuf> {
    let number = |name: &str| -> Option<Vec<u64>> {
        name.strip_prefix('v')?
            .split('.')
            .map(|n| n.parse().ok())
            .collect()
    };
    std::fs::read_dir(versions)
        .ok()?
        .flatten()
        .filter_map(|e| Some((number(&e.file_name().to_string_lossy())?, e.path())))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, path)| path)
}

/// Where npm is: on `path` (a PATH string), else in the first of `usual` that has it, else in
/// `own` (Trek's own Node's folder).
pub fn find_npm_in(path: &str, usual: &[PathBuf], own: &Path) -> Option<PathBuf> {
    if let Some(npm) = crate::detect::which_in(path, "npm") {
        return Some(npm);
    }
    let rest = std::env::join_paths(usual.iter().chain([&own.to_path_buf()])).ok()?;
    crate::detect::which_in(&rest.to_string_lossy(), "npm")
}

/// Where npm is on this computer, if anywhere (`find_npm_in` the login PATH).
pub fn find_npm() -> Option<PathBuf> {
    find_npm_in(
        crate::detect::login_path(),
        &usual_dirs(&crate::paths::home(), |name| std::env::var_os(name)),
        &bin_dir(),
    )
}

/// The environment a terminal that installs an agent runs with: `login` (the login PATH) with
/// the folder of `npm` in front when it was found outside it, and, when that `npm` is Trek's own
/// (in `own_root`), its global packages kept beside it, where `login_path` finds them.
pub fn setup_env_for(login: &str, npm: Option<&Path>, own_root: &Path) -> Vec<(String, String)> {
    let mut parts: Vec<PathBuf> = npm
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .into_iter()
        .collect();
    parts.extend(std::env::split_paths(login));
    let mut seen = std::collections::HashSet::new();
    parts.retain(|p| {
        let key = p
            .to_string_lossy()
            .trim_end_matches(['/', '\\'])
            .to_string();
        !key.is_empty()
            && seen.insert(if cfg!(windows) {
                key.to_lowercase()
            } else {
                key
            })
    });
    let path = std::env::join_paths(&parts)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| login.to_string());
    let mut env = vec![("PATH".to_string(), path)];
    if npm.and_then(Path::parent) == Some(bin_dir_in(own_root).as_path()) {
        env.push((
            "npm_config_prefix".to_string(),
            own_root.display().to_string(),
        ));
    }
    env
}

/// `setup_env_for` this computer.
pub fn setup_env() -> Vec<(String, String)> {
    setup_env_for(crate::detect::login_path(), find_npm().as_deref(), &root())
}

/// Whether a shell's `output` says `npm`, `npx` or `node` couldn't be found: zsh's "command not
/// found: npm", bash's "npm: command not found", sh's "npm: not found", fish's "Unknown command:
/// npm", PowerShell's and cmd's "'npm' is not recognized", or `env` failing to find the `node` an
/// npm script starts with.
pub fn missing_in_output(output: &str) -> bool {
    let text = output.to_lowercase();
    ["npm", "npx", "node"].iter().any(|tool| {
        [
            format!("command not found: {tool}"),
            format!("unknown command: {tool}"),
            format!("{tool}: command not found"),
            format!("{tool}: not found"),
            format!("'{tool}' is not recognized"),
            format!("env: {tool}: no such file"),
            format!("env: \u{2018}{tool}\u{2019}: no such file"),
        ]
        .iter()
        .any(|pattern| contains_word(&text, pattern))
    })
}

/// Whether `text` has `pattern` with no letter, digit or `-` running on from either end
/// ("command not found: npm" isn't "command not found: npmx").
fn contains_word(text: &str, pattern: &str) -> bool {
    let joins = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
    text.match_indices(pattern).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + pattern.len()..].chars().next();
        !before.is_some_and(|c| joins(c) && pattern.starts_with(joins))
            && !after.is_some_and(|c| joins(c) && pattern.ends_with(joins))
    })
}

// ---------------------------------------------------------------------------------------------
// Installing

/// nodejs.org's names for a platform: its key in a release's `files` (`index.json`), and the end
/// of its archive's name.
fn platform(os: &str, arch: &str) -> Option<(&'static str, &'static str)> {
    Some(match (os, arch) {
        ("macos", "aarch64") => ("osx-arm64-tar", "darwin-arm64.tar.gz"),
        ("macos", "x86_64") => ("osx-x64-tar", "darwin-x64.tar.gz"),
        ("windows", "x86_64") => ("win-x64-zip", "win-x64.zip"),
        ("windows", "aarch64") => ("win-arm64-zip", "win-arm64.zip"),
        ("linux", "x86_64") => ("linux-x64", "linux-x64.tar.gz"),
        ("linux", "aarch64") => ("linux-arm64", "linux-arm64.tar.gz"),
        _ => return None,
    })
}

/// The newest LTS release in `index` (nodejs.org's `index.json`, newest first) that has a build
/// for `files_key`: its version (`v24.11.1`).
pub fn latest_lts(index: &str, files_key: &str) -> Option<String> {
    let releases: Vec<serde_json::Value> = serde_json::from_str(index).ok()?;
    releases
        .iter()
        .find(|r| {
            // `lts` is the line's codename, or `false`.
            r["lts"].as_str().is_some_and(|c| !c.is_empty())
                && r["files"]
                    .as_array()
                    .is_some_and(|f| f.iter().any(|f| f == files_key))
        })
        .and_then(|r| r["version"].as_str())
        .filter(|v| {
            v.starts_with('v')
                && v[1..]
                    .split('.')
                    .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(String::from)
}

/// The SHA-256 `sums` (a release's `SHASUMS256.txt`: `<hex>  <file>` lines) gives for `file`.
pub fn checksum_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == file
            && hash.len() == 64
            && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
    })
}

/// How far installing Node.js has got.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    /// Asking nodejs.org for its newest LTS release.
    Finding,
    Downloading {
        version: String,
        done: u64,
        total: Option<u64>,
    },
    Unpacking {
        version: String,
    },
}

impl Progress {
    /// As a line under "Trek is installing it": "Downloading Node.js v24.11.1 · 42%".
    pub fn label(&self) -> String {
        match self {
            Progress::Finding => "Finding the current Node.js LTS release…".into(),
            Progress::Downloading {
                version,
                done,
                total: Some(total),
            } if *total > 0 => format!(
                "Downloading Node.js {version} from nodejs.org · {}%",
                (done * 100 / total).min(100)
            ),
            Progress::Downloading { version, done, .. } => format!(
                "Downloading Node.js {version} from nodejs.org · {:.1} MB",
                *done as f64 / 1e6
            ),
            Progress::Unpacking { version } => format!("Unpacking Node.js {version}…"),
        }
    }
}

fn client(timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(20))
        .user_agent(format!("Trek/{}", crate::VERSION))
        .build()?)
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<String> {
    let mut resp = client.get(url).send().await?.error_for_status()?;
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_INDEX_BYTES {
            bail!("{url} is larger than expected");
        }
    }
    String::from_utf8(body).map_err(|_| anyhow!("{url} isn't text"))
}

/// Install the current Node.js LTS as Trek's own (`root`) and return its `bin_dir`. Never from a
/// test process: nothing is downloaded there.
pub async fn install(progress: &(dyn Fn(Progress) + Send + Sync)) -> Result<PathBuf> {
    anyhow::ensure!(
        !crate::paths::isolated(),
        "Node.js isn't installed from a test process."
    );
    let root = root();
    install_into(&root, DIST, progress).await?;
    Ok(bin_dir_in(&root))
}

/// Download the newest Node.js LTS for this machine from `dist`, check it against the release's
/// SHASUMS256.txt, and unpack it as `root`: next to it first, then moved into place whole, so a
/// failed install leaves nothing half there (and an earlier one as it was).
pub async fn install_into(
    root: &Path,
    dist: &str,
    progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<()> {
    use sha2::Digest as _;
    use tokio::io::AsyncWriteExt as _;
    let (files_key, suffix) = platform(std::env::consts::OS, std::env::consts::ARCH)
        .ok_or_else(|| anyhow!("nodejs.org has no Node.js build for this computer"))?;
    let parent = root
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent folder", root.display()))?;
    std::fs::create_dir_all(parent)?;
    let client = client(Duration::from_secs(600))?;
    progress(Progress::Finding);
    let index = get_text(&client, &format!("{dist}/index.json"))
        .await
        .context("Couldn't reach nodejs.org")?;
    let version = latest_lts(&index, files_key)
        .ok_or_else(|| anyhow!("nodejs.org lists no LTS release for this computer"))?;
    let file = format!("node-{version}-{suffix}");
    let sums = get_text(&client, &format!("{dist}/{version}/SHASUMS256.txt"))
        .await
        .context("Couldn't read Node.js's checksums from nodejs.org")?;
    let expected = checksum_for(&sums, &file)
        .ok_or_else(|| anyhow!("nodejs.org publishes no checksum for {file}"))?;

    let id = std::process::id();
    let download = parent.join(format!(".node-download-{id}"));
    let staging = parent.join(format!(".node-unpack-{id}"));
    let _ = std::fs::remove_dir_all(&staging);
    let result = async {
        let mut resp = client
            .get(format!("{dist}/{version}/{file}"))
            .send()
            .await
            .context("Couldn't start the download")?
            .error_for_status()?;
        let total = resp.content_length();
        if total.is_some_and(|n| n > MAX_ARCHIVE_BYTES) {
            bail!("The download is larger than expected");
        }
        let mut out = tokio::fs::File::create(&download).await?;
        let mut hasher = sha2::Sha256::new();
        let (mut done, mut shown) = (0u64, 0u64);
        progress(Progress::Downloading {
            version: version.clone(),
            done: 0,
            total,
        });
        while let Some(chunk) = resp.chunk().await.context("The download was cut off")? {
            done += chunk.len() as u64;
            if done > MAX_ARCHIVE_BYTES {
                bail!("The download is larger than expected");
            }
            hasher.update(&chunk);
            out.write_all(&chunk).await?;
            if done - shown >= total.map_or(1 << 20, |n| (n / 100).max(1)) {
                shown = done;
                progress(Progress::Downloading {
                    version: version.clone(),
                    done,
                    total,
                });
            }
        }
        out.flush().await?;
        drop(out);
        if !hex::encode(hasher.finalize()).eq_ignore_ascii_case(&expected) {
            bail!("The download doesn't match nodejs.org's checksum, so it wasn't installed");
        }
        progress(Progress::Unpacking {
            version: version.clone(),
        });
        let (archive, into) = (download.clone(), staging.clone());
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&into)?;
            if cfg!(windows) {
                crate::registry::unpack_zip(&archive, &into)
            } else {
                crate::registry::unpack_tar(&archive, &into)
            }
        })
        .await??;
        // The archive holds one folder, `node-v24.11.1-darwin-arm64`.
        let top = staging.join(file.trim_end_matches(".tar.gz").trim_end_matches(".zip"));
        if !bin_dir_in(&top).join(NPM).is_file() {
            bail!("The download has no npm");
        }
        let _ = std::fs::remove_dir_all(root);
        std::fs::rename(&top, root)?;
        Ok(())
    }
    .await;
    let _ = std::fs::remove_file(&download);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_needs_node_when_one_of_its_commands_is_npm_npx_or_node() {
        assert!(needs_node("npm i -g @sourcegraph/amp amp-acp"));
        assert!(needs_node("brew install node && npm i -g pi-acp"));
        assert!(needs_node("npx -y skills add x"));
        assert!(!needs_node(
            "curl -fsSL https://claude.ai/install.sh | bash"
        ));
        assert!(!needs_node("irm https://claude.ai/install.ps1 | iex"));
        assert!(!needs_node("brew install block-goose-cli"));
        // A word that only starts with one isn't one.
        assert!(!needs_node("npmx install"));
        assert!(!needs_node("echo npm"));
    }

    #[test]
    fn a_missing_npm_or_node_is_heard_in_every_shell_s_words() {
        for said in [
            "zsh:1: command not found: npm",
            "bash: line 1: npm: command not found",
            "sh: 1: npm: not found",
            "fish: Unknown command: npm",
            "npm : The term 'npm' is not recognized as a name of a cmdlet, function, script file, or executable program.",
            "'npm' is not recognized as an internal or external command,",
            "env: node: No such file or directory",
            "/usr/bin/env: \u{2018}node\u{2019}: No such file or directory",
            "zsh:1: command not found: npx",
        ] {
            assert!(missing_in_output(said), "{said}");
        }
        for fine in [
            "added 12 packages in 3s",
            "zsh:1: command not found: npmx",
            "[finished with exit code 0]",
            "error: package not found: npm-foo",
            "pnpm: command not found",
        ] {
            assert!(!missing_in_output(fine), "{fine}");
        }
    }

    #[test]
    fn the_newest_lts_with_a_build_here_is_chosen() {
        let index = r#"[
            {"version":"v25.1.0","lts":false,"files":["osx-arm64-tar","win-x64-zip"]},
            {"version":"v24.11.1","lts":"Krypton","files":["osx-x64-tar","win-x64-zip","linux-x64"]},
            {"version":"v24.11.0","lts":"Krypton","files":["osx-arm64-tar","osx-x64-tar","win-x64-zip"]},
            {"version":"v22.21.1","lts":"Jod","files":["osx-arm64-tar"]}
        ]"#;
        // The current line isn't LTS; the newest LTS has no arm64 Mac build, the one before does.
        assert_eq!(
            latest_lts(index, "osx-arm64-tar").as_deref(),
            Some("v24.11.0")
        );
        assert_eq!(
            latest_lts(index, "win-x64-zip").as_deref(),
            Some("v24.11.1")
        );
        assert_eq!(latest_lts(index, "win-arm64-zip"), None);
        // A version that isn't one can't name a download.
        let odd = r#"[{"version":"v24/../x","lts":"K","files":["osx-arm64-tar"]}]"#;
        assert_eq!(latest_lts(odd, "osx-arm64-tar"), None);
        assert_eq!(latest_lts("not json", "osx-arm64-tar"), None);
    }

    #[test]
    fn each_platform_has_nodejs_org_s_names() {
        assert_eq!(
            platform("macos", "aarch64"),
            Some(("osx-arm64-tar", "darwin-arm64.tar.gz"))
        );
        assert_eq!(
            platform("macos", "x86_64"),
            Some(("osx-x64-tar", "darwin-x64.tar.gz"))
        );
        assert_eq!(
            platform("windows", "x86_64"),
            Some(("win-x64-zip", "win-x64.zip"))
        );
        assert_eq!(
            platform("windows", "aarch64"),
            Some(("win-arm64-zip", "win-arm64.zip"))
        );
        assert_eq!(platform("freebsd", "x86_64"), None);
    }

    #[test]
    fn the_checksum_is_the_one_for_that_file() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!(
            "{a}  node-v24.11.1-darwin-arm64.tar.gz.sig\n{b}  node-v24.11.1-darwin-arm64.tar.gz\n{a}  node-v24.11.1-darwin-x64.tar.gz\n"
        );
        assert_eq!(
            checksum_for(&sums, "node-v24.11.1-darwin-arm64.tar.gz"),
            Some(b.to_lowercase())
        );
        assert_eq!(checksum_for(&sums, "node-v24.11.1-win-x64.zip"), None);
        assert_eq!(checksum_for("short  node.zip", "node.zip"), None);
    }

    #[test]
    fn trek_s_node_keeps_its_commands_in_bin_on_a_mac_and_beside_node_on_windows() {
        let root = Path::new("/data/node");
        let bin = bin_dir_in(root);
        if cfg!(windows) {
            assert_eq!(bin, root);
        } else {
            assert_eq!(bin, root.join("bin"));
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek-node-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch_npm(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let npm = dir.join(NPM);
        std::fs::write(&npm, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        npm
    }

    /// A fresh computer: no npm on the PATH, none where Node's installers put it, none of
    /// Trek's. Then each place in turn.
    #[test]
    fn npm_is_found_on_the_path_then_where_installers_put_it_then_trek_s_own() {
        let home = temp("find");
        let bare = home.join("bare-bin");
        std::fs::create_dir_all(&bare).unwrap();
        let path = bare.display().to_string();
        let own_root = home.join("data/node");
        let own = bin_dir_in(&own_root);
        // Only the folders under this home: a test can't tell the machine's own (/usr/local,
        // Program Files) apart from a user's.
        let usual = || -> Vec<PathBuf> {
            usual_dirs(&home, |_| None)
                .into_iter()
                .filter(|d| d.starts_with(&home))
                .collect()
        };
        assert_eq!(find_npm_in(&path, &usual(), &own), None, "{:?}", usual());

        // Trek's own, installed.
        let trek_npm = touch_npm(&own);
        assert_eq!(find_npm_in(&path, &usual(), &own), Some(trek_npm.clone()));

        // A version manager's Node comes before Trek's.
        if cfg!(windows) {
            let volta = touch_npm(&home.join(r"AppData\Local\Volta\bin"));
            assert_eq!(find_npm_in(&path, &usual(), &own), Some(volta));
        } else {
            touch_npm(&home.join(".nvm/versions/node/v20.9.0/bin"));
            let newest = touch_npm(&home.join(".nvm/versions/node/v22.11.0/bin"));
            touch_npm(&home.join(".nvm/versions/node/v9.0.0/bin"));
            assert_eq!(find_npm_in(&path, &usual(), &own), Some(newest));
        }
        let usual = usual();

        // One on the PATH wins over all of them.
        let on_path = touch_npm(&bare);
        assert_eq!(find_npm_in(&path, &usual, &own), Some(on_path));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_variable_of_a_version_manager_moves_its_folder() {
        let home = Path::new(if cfg!(windows) {
            r"C:\Users\me"
        } else {
            "/Users/me"
        });
        let custom = if cfg!(windows) {
            r"D:\volta"
        } else {
            "/opt/volta"
        };
        let dirs = usual_dirs(home, |name| (name == "VOLTA_HOME").then(|| custom.into()));
        assert!(dirs.contains(&Path::new(custom).join("bin")), "{dirs:?}");
        // A relative one is ignored.
        let dirs = usual_dirs(home, |name| (name == "VOLTA_HOME").then(|| "volta".into()));
        assert!(!dirs.contains(&Path::new("volta").join("bin")), "{dirs:?}");
    }

    #[test]
    fn an_install_terminal_finds_npm_and_keeps_trek_s_packages_beside_trek_s_node() {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let (a, b) = if cfg!(windows) {
            (r"C:\a", r"C:\b")
        } else {
            ("/a", "/b")
        };
        let login = format!("{a}{sep}{b}");
        let get = |env: &[(String, String)], name: &str| {
            env.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
        };
        let own_root = PathBuf::from(if cfg!(windows) {
            r"C:\data\node"
        } else {
            "/data/node"
        });

        // npm on the login PATH: as it was, and npm's own prefix.
        let npm = Path::new(b).join(NPM);
        let env = setup_env_for(&login, Some(&npm), &own_root);
        assert_eq!(get(&env, "PATH"), Some(format!("{b}{sep}{a}")));
        assert_eq!(get(&env, "npm_config_prefix"), None);

        // npm found outside it (nvm's, not loaded by a login shell that doesn't run .zshrc): its
        // folder goes in front.
        let nvm = PathBuf::from(if cfg!(windows) { r"C:\nvm" } else { "/nvm/bin" });
        let env = setup_env_for(&login, Some(&nvm.join(NPM)), &own_root);
        assert_eq!(
            get(&env, "PATH"),
            Some(format!("{}{sep}{a}{sep}{b}", nvm.display()))
        );

        // Trek's own: its packages go in its own folder.
        let own_bin = bin_dir_in(&own_root);
        let env = setup_env_for(&login, Some(&own_bin.join(NPM)), &own_root);
        assert_eq!(
            get(&env, "PATH"),
            Some(format!("{}{sep}{a}{sep}{b}", own_bin.display()))
        );
        assert_eq!(
            get(&env, "npm_config_prefix"),
            Some(own_root.display().to_string())
        );

        // None at all: the login PATH alone.
        let env = setup_env_for(&login, None, &own_root);
        assert_eq!(env, vec![("PATH".to_string(), login.clone())]);
    }

    /// Downloads the real Node.js LTS from nodejs.org into a temp folder, checks it, and runs it.
    /// Off by default (network, ~50 MB): `cargo test -p trek-core node::tests::real -- --ignored`.
    #[test]
    #[ignore]
    fn real_download_into_a_temp_folder() {
        let dir = temp("real");
        let root = dir.join("node");
        let seen = std::sync::Mutex::new(Vec::new());
        crate::runtime()
            .block_on(install_into(&root, DIST, &|p| seen.lock().unwrap().push(p)))
            .unwrap();
        let node = bin_dir_in(&root).join(if cfg!(windows) { "node.exe" } else { "node" });
        let out = std::process::Command::new(&node)
            .arg("--version")
            .output()
            .unwrap();
        let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
        eprintln!("installed {version} at {}", root.display());
        assert!(version.starts_with('v'), "{version}");
        assert!(bin_dir_in(&root).join(NPM).is_file());
        let seen = seen.lock().unwrap();
        assert!(matches!(seen.first(), Some(Progress::Finding)));
        assert!(matches!(seen.last(), Some(Progress::Unpacking { .. })));
        if std::env::var_os("TREK_KEEP_NODE").is_none() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A download whose bytes don't match the checksum is refused, and nothing is left.
    #[test]
    fn a_download_that_doesn_t_match_its_checksum_is_refused() {
        let dir = temp("served");
        let (key, suffix) = platform(std::env::consts::OS, std::env::consts::ARCH).unwrap();
        let version = "v24.0.0";
        let file = format!("node-{version}-{suffix}");
        std::fs::create_dir_all(dir.join(version)).unwrap();
        std::fs::write(
            dir.join("index.json"),
            format!(r#"[{{"version":"{version}","lts":"Krypton","files":["{key}"]}}]"#),
        )
        .unwrap();
        std::fs::write(dir.join(version).join(&file), b"not node").unwrap();
        std::fs::write(
            dir.join(version).join("SHASUMS256.txt"),
            format!("{}  {file}\n", "0".repeat(64)),
        )
        .unwrap();
        let server = serve(&dir);
        let root = dir.join("data/node");
        let err = crate::runtime()
            .block_on(install_into(&root, &server, &|_| {}))
            .unwrap_err();
        assert!(format!("{err:#}").contains("checksum"), "{err:#}");
        assert!(!root.exists());
        let leftovers: Vec<_> = std::fs::read_dir(dir.join("data"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tiny HTTP server for `dir`'s files, on a port of its own: `http://127.0.0.1:<port>`.
    fn serve(dir: &Path) -> String {
        use std::io::{BufRead as _, BufReader, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let dir = dir.to_path_buf();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) <= 2 {
                        break;
                    }
                }
                let mut stream = stream;
                match std::fs::read(dir.join(path.trim_start_matches('/'))) {
                    Ok(body) => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(&body);
                    }
                    Err(_) => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                }
            }
        });
        format!("http://{addr}")
    }
}
