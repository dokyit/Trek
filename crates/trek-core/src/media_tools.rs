//! ffmpeg, ffprobe and yt-dlp for the video watcher (`trek-mcp video`), on a computer that has no
//! Homebrew or winget to install them with, or a Linux whose user would rather not `sudo`.
//!
//! The watcher looks for them on PATH and in the usual package managers' folders first. With none
//! there, Trek downloads a build for this machine into its own folder (`dir`): no
//! administrator rights, nothing outside Trek's folder. ffmpeg and ffprobe are pinned to one
//! release each with its SHA-256 written here; yt-dlp, which sites outrun within weeks, is its
//! latest release, checked against the SHA2-256SUMS that release publishes. A file whose hash
//! doesn't match is never unpacked, and an unpacked program that won't start is never kept.
//!
//! Sources, chosen for stable versioned URLs, a published checksum and a build that runs with no
//! installer:
//!
//! - macOS ffmpeg, ffprobe (Apple silicon and Intel): Martin Riedl's build server,
//!   ffmpeg.martin-riedl.de. Release 9.0.2, one download per program, signed with his Developer
//!   ID (the system accepts them) and listed with a `.sha256` file beside each. GPLv3 builds;
//!   Trek runs them as programs and doesn't link or redistribute them.
//! - Windows ffmpeg, ffprobe (x86-64): Gyan Doshi's "essentials" release 8.1.2 (gyan.dev, also
//!   mirrored on GitHub as GyanD/codexffmpeg, where the same checksum is published). GPLv3, the
//!   same way.
//! - Linux ffmpeg, ffprobe (x86-64 and Arm64): the same build server's Linux builds of the same
//!   release, 9.0.2, one zip per program with its `.sha256` beside it. They link nothing but the
//!   system's C and C++ libraries (glibc 2.35 and GCC 12's libstdc++, which Ubuntu 22.04 has: the
//!   oldest Linux Trek's own build runs on), so they start wherever Trek does.
//! - yt-dlp: the project's own standalone programs on GitHub (`yt-dlp_macos`, `yt-dlp.exe`,
//!   `yt-dlp_linux`, `yt-dlp_linux_aarch64`).
//!
//! Files Trek downloads itself aren't quarantined, so Gatekeeper never asks about them.
//!
//! The other way to get them is the computer's package manager (`package_managers`): what
//! Settings offers when a download can't be made, and what an agent is told to ask the user for.

use anyhow::{Context as _, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where Trek tells `trek-mcp video` its downloaded tools are.
pub const ENV_TOOLS_DIR: &str = "TREK_TOOLS_DIR";

/// No single download is larger than this (the biggest is Windows' ffmpeg, about 105 MB).
const MAX_DOWNLOAD_BYTES: u64 = 400 << 20;
const MAX_SUMS_BYTES: usize = 1 << 20;

/// What Trek can download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// ffmpeg and ffprobe, which `watch_video` reads videos with.
    Ffmpeg,
    /// yt-dlp, which gets the video out of a YouTube or X page.
    YtDlp,
}

impl Tool {
    /// How it's said to a person.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "ffmpeg",
            Tool::YtDlp => "yt-dlp",
        }
    }

    /// The programs it puts in `dir`, by the names they're run as.
    pub fn programs(self) -> &'static [&'static str] {
        match self {
            Tool::Ffmpeg => &["ffmpeg", "ffprobe"],
            Tool::YtDlp => &["yt-dlp"],
        }
    }
}

/// The file name of a program (`ffmpeg.exe` on Windows).
fn file_name(program: &str) -> String {
    if cfg!(windows) {
        format!("{program}.exe")
    } else {
        program.to_string()
    }
}

/// Where Trek keeps the tools it downloaded: with what it can download again (on Windows, the
/// local app data, which a roaming profile doesn't copy; the data folder on a Mac).
pub fn dir() -> PathBuf {
    crate::paths::local_data_dir().join("tools")
}

/// `dir`, or the folder `TREK_TOOLS_DIR` names: what Trek hands the programs it starts, so
/// `trek-mcp` reads the same folder Trek writes whatever its own environment says.
pub fn dir_from_env() -> PathBuf {
    std::env::var_os(ENV_TOOLS_DIR)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(dir)
}

/// Where the downloaded `program` is, in `dir`, if it's there.
pub fn find_in(dir: &Path, program: &str) -> Option<PathBuf> {
    let path = dir.join(file_name(program));
    path.is_file().then_some(path)
}

/// Whether every program of `tool` is in `dir`.
pub fn installed_in(dir: &Path, tool: Tool) -> bool {
    tool.programs().iter().all(|p| find_in(dir, p).is_some())
}

/// Where a file's SHA-256 comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Hash {
    /// Written here, for a build pinned to one release.
    Fixed(String),
    /// The line for `file` in a `SHA2-256SUMS` file at `url`, which the release publishes.
    Sums { url: String, file: String },
}

/// What a download holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Pack {
    /// A zip: each `(end of an entry's path, program)` is taken out of it.
    Zip(Vec<(String, String)>),
    /// The program itself.
    File(String),
}

/// One download.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub url: String,
    /// About how many bytes, for the percentage and "about N MB".
    pub size: u64,
    pub hash: Hash,
    pub pack: Pack,
}

/// What it takes to install a tool on this machine.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub tool: Tool,
    /// Which release, for what's said and logged ("9.0.2", "latest").
    pub version: &'static str,
    pub parts: Vec<Part>,
}

impl Plan {
    /// How many bytes it downloads, about.
    pub fn bytes(&self) -> u64 {
        self.parts.iter().map(|p| p.size).sum()
    }

    /// "about 57 MB", for the words in front of the button.
    pub fn size_text(&self) -> String {
        let mb = self.bytes().div_ceil(1_000_000);
        format!("about {} MB", (mb + 4) / 5 * 5)
    }
}

fn zip_part(url: &str, size: u64, sha256: &str, entry: &str, program: &str) -> Part {
    Part {
        url: url.into(),
        size,
        hash: Hash::Fixed(sha256.into()),
        pack: Pack::Zip(vec![(entry.into(), program.into())]),
    }
}

const MAC_RIEDL: &str = "https://ffmpeg.martin-riedl.de/download/macos";
const LINUX_RIEDL: &str = "https://ffmpeg.martin-riedl.de/download/linux";

/// The downloads that put `tool` on a computer of `os` and `arch` (`std::env::consts`): none where
/// there's no build to give it.
pub fn plan_for(tool: Tool, os: &str, arch: &str) -> Option<Plan> {
    let parts = match (tool, os, arch) {
        (Tool::Ffmpeg, "macos", "aarch64") => {
            let dir = format!("{MAC_RIEDL}/arm64/1789931890_9.0.2");
            vec![
                zip_part(
                    &format!("{dir}/ffmpeg.zip"),
                    28_395_699,
                    "c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924",
                    "ffmpeg",
                    "ffmpeg",
                ),
                zip_part(
                    &format!("{dir}/ffprobe.zip"),
                    28_317_701,
                    "fcbe839537485eaee7a7a8bc5cbc0f90d53617e80943e8a5b2e31cb851197ea6",
                    "ffprobe",
                    "ffprobe",
                ),
            ]
        }
        (Tool::Ffmpeg, "macos", "x86_64") => {
            let dir = format!("{MAC_RIEDL}/amd64/1789931006_9.0.2");
            vec![
                zip_part(
                    &format!("{dir}/ffmpeg.zip"),
                    33_816_391,
                    "7c6b4125b191cbf773832dc51f424cf2b6bb7da43007d1e066f95909e47cacd4",
                    "ffmpeg",
                    "ffmpeg",
                ),
                zip_part(
                    &format!("{dir}/ffprobe.zip"),
                    33_719_233,
                    "2322438ed2f6319a691291b247d09c69dcaa3a982460d1f269a7e1af335cfdfd",
                    "ffprobe",
                    "ffprobe",
                ),
            ]
        }
        (Tool::Ffmpeg, "windows", "x86_64") => vec![Part {
            url: "https://github.com/GyanD/codexffmpeg/releases/download/8.1.2/ffmpeg-8.1.2-essentials_build.zip".into(),
            size: 109_728_040,
            hash: Hash::Fixed(
                "db580001caa24ac104c8cb856cd113a87b0a443f7bdf47d8c12b1d740584a2ec".into(),
            ),
            pack: Pack::Zip(vec![
                ("bin/ffmpeg.exe".into(), "ffmpeg".into()),
                ("bin/ffprobe.exe".into(), "ffprobe".into()),
            ]),
        }],
        (Tool::Ffmpeg, "linux", "x86_64") => {
            let dir = format!("{LINUX_RIEDL}/amd64/1789931100_9.0.2");
            vec![
                zip_part(
                    &format!("{dir}/ffmpeg.zip"),
                    33_326_445,
                    "fa8ecf4abbd290d98f7d188b8649cc6b391ae209a98452be955a15aab1909d7f",
                    "ffmpeg",
                    "ffmpeg",
                ),
                zip_part(
                    &format!("{dir}/ffprobe.zip"),
                    33_223_234,
                    "3f428c49070be3d24ec338602b76d412e401ffcb8a5641ef0e729181a232fc32",
                    "ffprobe",
                    "ffprobe",
                ),
            ]
        }
        (Tool::Ffmpeg, "linux", "aarch64") => {
            let dir = format!("{LINUX_RIEDL}/arm64/1789931697_9.0.2");
            vec![
                zip_part(
                    &format!("{dir}/ffmpeg.zip"),
                    28_753_826,
                    "93a76ae90db5474eecdf951a729857c64f3de23567228d6a7d5e6e8e3cd1021b",
                    "ffmpeg",
                    "ffmpeg",
                ),
                zip_part(
                    &format!("{dir}/ffprobe.zip"),
                    28_655_683,
                    "bcbe80fb741c180083327afaf5434812e006b33cacde2016b9aeaf6936128330",
                    "ffprobe",
                    "ffprobe",
                ),
            ]
        }
        (Tool::YtDlp, "macos", _) => vec![latest_yt_dlp("yt-dlp_macos", 37_500_000)],
        (Tool::YtDlp, "windows", "x86_64") => vec![latest_yt_dlp("yt-dlp.exe", 18_500_000)],
        (Tool::YtDlp, "linux", "x86_64") => vec![latest_yt_dlp("yt-dlp_linux", 41_200_000)],
        (Tool::YtDlp, "linux", "aarch64") => {
            vec![latest_yt_dlp("yt-dlp_linux_aarch64", 40_800_000)]
        }
        _ => return None,
    };
    Some(Plan {
        tool,
        version: match tool {
            Tool::Ffmpeg if os == "windows" => "8.1.2",
            Tool::Ffmpeg => "9.0.2",
            Tool::YtDlp => "latest",
        },
        parts,
    })
}

/// `plan_for` this computer.
pub fn plan(tool: Tool) -> Option<Plan> {
    plan_for(tool, std::env::consts::OS, std::env::consts::ARCH)
}

fn latest_yt_dlp(file: &str, size: u64) -> Part {
    const LATEST: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
    Part {
        url: format!("{LATEST}/{file}"),
        size,
        hash: Hash::Sums {
            url: format!("{LATEST}/SHA2-256SUMS"),
            file: file.into(),
        },
        pack: Pack::File("yt-dlp".into()),
    }
}

/// A package manager that has ffmpeg and yt-dlp: the other way to get them, where Trek has no
/// download for the computer or its download couldn't be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageManager {
    /// The program it's found by on PATH.
    pub program: &'static str,
    /// As a button names it ("Use Homebrew").
    pub name: &'static str,
    ffmpeg: &'static str,
    yt_dlp: &'static str,
}

impl PackageManager {
    /// The line that installs `tool` with it, as typed in a terminal.
    pub fn install_line(&self, tool: Tool) -> &'static str {
        match tool {
            Tool::Ffmpeg => self.ffmpeg,
            Tool::YtDlp => self.yt_dlp,
        }
    }
}

/// The package managers of `os` (`std::env::consts::OS`) that have ffmpeg and yt-dlp, in the
/// order they're looked for: a Mac's Homebrew, Windows' winget, and on Linux the distribution's
/// own (Debian and Ubuntu's apt, Fedora's dnf, whose ffmpeg is `ffmpeg-free`, Arch's pacman,
/// openSUSE's zypper), each asking for the password itself in the terminal it's run in.
pub fn package_managers(os: &str) -> &'static [PackageManager] {
    match os {
        "macos" => &[PackageManager {
            program: "brew",
            name: "Homebrew",
            ffmpeg: "brew install ffmpeg",
            yt_dlp: "brew install yt-dlp",
        }],
        "windows" => &[PackageManager {
            program: "winget",
            name: "winget",
            ffmpeg: "winget install ffmpeg",
            yt_dlp: "winget install yt-dlp",
        }],
        "linux" => &[
            PackageManager {
                program: "apt-get",
                name: "apt",
                ffmpeg: "sudo apt install ffmpeg",
                yt_dlp: "sudo apt install yt-dlp",
            },
            PackageManager {
                program: "dnf",
                name: "dnf",
                ffmpeg: "sudo dnf install ffmpeg-free",
                yt_dlp: "sudo dnf install yt-dlp",
            },
            PackageManager {
                program: "pacman",
                name: "pacman",
                ffmpeg: "sudo pacman -S ffmpeg",
                yt_dlp: "sudo pacman -S yt-dlp",
            },
            PackageManager {
                program: "zypper",
                name: "zypper",
                ffmpeg: "sudo zypper install ffmpeg",
                yt_dlp: "sudo zypper install yt-dlp",
            },
        ],
        _ => &[],
    }
}

/// The package manager of `os` this computer has (`has`: whether a program is on PATH), the
/// first of them when it has several.
pub fn package_manager_on(os: &str, has: impl Fn(&str) -> bool) -> Option<PackageManager> {
    package_managers(os)
        .iter()
        .copied()
        .find(|m| has(m.program))
}

/// The line that installs `tool` on `os`: with `found`, the package manager the computer has, or
/// else with the one a Mac or a Windows PC is given (Homebrew, winget: the line to pass on to
/// someone who has yet to install it). None on a Linux with no package manager Trek knows: no one
/// line is right for every distribution.
pub fn install_line(tool: Tool, os: &str, found: Option<PackageManager>) -> Option<&'static str> {
    let only = match package_managers(os) {
        [only] => Some(*only),
        _ => None,
    };
    found.or(only).map(|m| m.install_line(tool))
}

/// How the user installs `tool` themselves on `os`, as an agent is told it ("ask the user to
/// install it with …"): the line in backticks, or on a Linux where none is known, their
/// distribution's package manager in words.
pub fn install_hint(tool: Tool, os: &str, found: Option<PackageManager>) -> String {
    match install_line(tool, os, found) {
        Some(line) => format!("`{line}`"),
        None => "their distribution's package manager".into(),
    }
}

/// How far a download has got.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Downloading {
        done: u64,
        total: u64,
    },
    /// Checking and unpacking, and making sure the programs start.
    Finishing,
}

impl Progress {
    /// As a line under the button: "Downloading ffmpeg · 42%".
    pub fn label(&self, tool: Tool) -> String {
        match self {
            Progress::Downloading { done, total } if *total > 0 => format!(
                "Downloading {} · {}%",
                tool.name(),
                (done * 100 / total).min(99)
            ),
            Progress::Downloading { done, .. } => {
                format!("Downloading {} · {:.1} MB", tool.name(), *done as f64 / 1e6)
            }
            Progress::Finishing => format!("Setting up {}…", tool.name()),
        }
    }
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(1800))
        .connect_timeout(Duration::from_secs(20))
        .user_agent(format!("Trek/{}", crate::VERSION))
        .build()?)
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<String> {
    let mut resp = client.get(url).send().await?.error_for_status()?;
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_SUMS_BYTES {
            bail!("{url} is larger than expected");
        }
    }
    String::from_utf8(body).map_err(|_| anyhow!("{url} isn't text"))
}

/// Install `tool` into Trek's own folder (`dir`). Never from a test process: nothing is
/// downloaded there.
pub async fn install(tool: Tool, progress: &(dyn Fn(Progress) + Send + Sync)) -> Result<()> {
    anyhow::ensure!(
        !crate::paths::isolated(),
        "{} isn't downloaded from a test process.",
        tool.name()
    );
    let plan = plan(tool).ok_or_else(|| {
        anyhow!(
            "Trek has no {} to download for this computer. Install it yourself and Trek will find it.",
            tool.name()
        )
    })?;
    install_into(&dir(), &plan, progress).await
}

/// Download `plan` into `dir`: each file checked against its SHA-256, unpacked next to where it
/// goes, started once (`-version`) to see it runs here, then moved into place. A failure leaves
/// nothing new in `dir`, and what was there before as it was.
pub async fn install_into(
    dir: &Path,
    plan: &Plan,
    progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<()> {
    use sha2::Digest as _;
    use tokio::io::AsyncWriteExt as _;
    std::fs::create_dir_all(dir)?;
    let client = client()?;
    let id = std::process::id();
    let total = plan.bytes();
    // (staged file, program) for each program the plan makes.
    let mut staged: Vec<(PathBuf, String)> = Vec::new();
    let mut base = 0u64;
    let result: Result<()> = async {
        for (i, part) in plan.parts.iter().enumerate() {
            let expected = match &part.hash {
                Hash::Fixed(hash) => hash.clone(),
                Hash::Sums { url, file } => {
                    let sums = get_text(&client, url)
                        .await
                        .context("Couldn't read the checksums of the download")?;
                    crate::node::checksum_for(&sums, file)
                        .ok_or_else(|| anyhow!("There's no checksum published for {file}"))?
                }
            };
            let download = dir.join(format!(".download-{id}-{i}"));
            let part_result: Result<()> = async {
                let mut resp = client
                    .get(&part.url)
                    .send()
                    .await
                    .context(
                        "Couldn't reach the download. Check your internet connection or proxy, then try again.",
                    )?
                    .error_for_status()
                    .context("The download isn't there")?;
                let length = resp.content_length();
                if length.is_some_and(|n| n > MAX_DOWNLOAD_BYTES) {
                    bail!("The download is larger than expected");
                }
                let mut out = tokio::fs::File::create(&download).await?;
                let mut hasher = sha2::Sha256::new();
                let (mut done, mut shown) = (0u64, 0u64);
                progress(Progress::Downloading { done: base, total });
                while let Some(chunk) = resp.chunk().await.context("The download was cut off")? {
                    done += chunk.len() as u64;
                    if done > MAX_DOWNLOAD_BYTES {
                        bail!("The download is larger than expected");
                    }
                    hasher.update(&chunk);
                    out.write_all(&chunk).await?;
                    if done - shown >= (total / 100).max(1 << 20) {
                        shown = done;
                        progress(Progress::Downloading {
                            done: base + done,
                            total: total.max(base + done),
                        });
                    }
                }
                out.flush().await?;
                drop(out);
                base += part.size.max(done);
                if !hex::encode(hasher.finalize()).eq_ignore_ascii_case(&expected) {
                    bail!("The download doesn't match its published checksum, so it wasn't installed");
                }
                progress(Progress::Finishing);
                let (archive, pack) = (download.clone(), part.pack.clone());
                let dir = dir.to_path_buf();
                let made = tokio::task::spawn_blocking(move || unpack(&archive, &pack, &dir, id))
                    .await??;
                staged.extend(made);
                Ok(())
            }
            .await;
            let _ = std::fs::remove_file(&download);
            part_result?;
        }
        for (path, program) in &staged {
            make_runnable(path)?;
            check_starts(path, program)?;
        }
        for (path, program) in &staged {
            std::fs::rename(path, dir.join(file_name(program)))
                .with_context(|| format!("Couldn't put {program} in place"))?;
        }
        Ok(())
    }
    .await;
    if result.is_err() {
        for (path, _) in &staged {
            let _ = std::fs::remove_file(path);
        }
    }
    result
}

/// The staged file's name: in the tools folder, hidden, and ending as a program's name does (a
/// Windows program must end in `.exe` to start).
fn staged_path(dir: &Path, id: u32, program: &str) -> PathBuf {
    dir.join(format!(".new-{id}-{}", file_name(program)))
}

/// Take what `pack` names out of `archive` into `dir` under staged names: `(path, program)`.
fn unpack(archive: &Path, pack: &Pack, dir: &Path, id: u32) -> Result<Vec<(PathBuf, String)>> {
    match pack {
        Pack::File(program) => {
            let to = staged_path(dir, id, program);
            std::fs::rename(archive, &to)?;
            Ok(vec![(to, program.clone())])
        }
        Pack::Zip(wanted) => {
            let mut zip =
                zip::ZipArchive::new(std::io::BufReader::new(std::fs::File::open(archive)?))
                    .map_err(|_| anyhow!("The download isn't an archive Trek can open"))?;
            let mut made: Vec<(PathBuf, String)> = Vec::new();
            let result: Result<()> = (|| {
                for i in 0..zip.len() {
                    let mut entry = zip
                        .by_index(i)
                        .map_err(|e| anyhow!("Couldn't unpack the download: {e}"))?;
                    if !entry.is_file() {
                        continue;
                    }
                    let Ok(name) = entry.name().map(|n| n.replace('\\', "/")) else {
                        continue;
                    };
                    let Some((_, program)) = wanted
                        .iter()
                        .find(|(end, _)| name == *end || name.ends_with(&format!("/{end}")))
                    else {
                        continue;
                    };
                    let to = staged_path(dir, id, program);
                    std::io::copy(&mut entry, &mut std::fs::File::create(&to)?)?;
                    made.push((to, program.clone()));
                }
                Ok(())
            })();
            let missing = wanted
                .iter()
                .find(|(_, program)| !made.iter().any(|(_, p)| p == program))
                .map(|(_, program)| program.clone());
            match (result, missing) {
                (Ok(()), None) => Ok(made),
                (result, missing) => {
                    for (path, _) in &made {
                        let _ = std::fs::remove_file(path);
                    }
                    result?;
                    Err(anyhow!(
                        "The download has no {}",
                        missing.unwrap_or_else(|| "program".into())
                    ))
                }
            }
        }
    }
}

fn make_runnable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Whether starting a program failed only because a process still had it open for writing
/// (`ETXTBSY` on Linux; Windows has no such error, so it never matches there).
fn text_file_busy(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::ExecutableFileBusy
}

/// Start `path` once with its version flag: a build the system won't run here (blocked, wrong
/// processor, a missing runtime) is found now, not when an agent needs it.
fn check_starts(path: &Path, program: &str) -> Result<()> {
    let flag = if program == "yt-dlp" {
        "--version"
    } else {
        "-version"
    };
    let spawn = || {
        crate::procs::command(path)
            .arg(flag)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
    };
    // Linux refuses to start a file some process still has open for writing ("Text file
    // busy"). Trek closed its own handle, but a child another thread forked while the file
    // was being written holds a copy until it execs: a moment, so the start is tried again.
    let mut tries = 0;
    let mut child = loop {
        match spawn() {
            Err(e) if text_file_busy(&e) && tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            other => {
                break other.map_err(|e| {
                    anyhow!("The downloaded {program} won't start on this computer: {e}")
                })?;
            }
        }
    };
    let started = std::time::Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => bail!("The downloaded {program} won't run on this computer ({status})"),
            None if started.elapsed() > Duration::from_secs(60) => {
                let _ = child.kill();
                bail!("The downloaded {program} didn't start within a minute");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trek-media-tools-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Every computer Trek is built for (release.yml's targets).
    const SHIPPED: [(&str, &str); 5] = [
        ("macos", "aarch64"),
        ("macos", "x86_64"),
        ("windows", "x86_64"),
        ("linux", "x86_64"),
        ("linux", "aarch64"),
    ];

    #[test]
    fn every_platform_trek_ships_for_has_a_plan_with_a_hash_written_in_the_code() {
        for (os, arch) in SHIPPED {
            let plan = plan_for(Tool::Ffmpeg, os, arch).unwrap_or_else(|| panic!("{os} {arch}"));
            let mut made: Vec<String> = Vec::new();
            for part in &plan.parts {
                assert!(part.url.starts_with("https://"), "{}", part.url);
                let Hash::Fixed(hash) = &part.hash else {
                    panic!("ffmpeg is pinned: {}", part.url)
                };
                assert!(
                    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
                    "{hash}"
                );
                match &part.pack {
                    Pack::Zip(entries) => made.extend(entries.iter().map(|e| e.1.clone())),
                    Pack::File(p) => made.push(p.clone()),
                }
            }
            made.sort();
            assert_eq!(made, ["ffmpeg", "ffprobe"], "{os} {arch}");
            assert!(plan.bytes() > 50_000_000, "{os} {arch}");
        }
        // yt-dlp's hash comes from its release's own sums, never from this file.
        for (os, arch) in SHIPPED {
            let plan = plan_for(Tool::YtDlp, os, arch).unwrap_or_else(|| panic!("{os} {arch}"));
            let [part] = plan.parts.as_slice() else {
                panic!("yt-dlp is one file: {os} {arch}")
            };
            let Hash::Sums { url, file } = &part.hash else {
                panic!("yt-dlp is checked against its release's sums: {os} {arch}")
            };
            assert!(url.ends_with("/SHA2-256SUMS"), "{url}");
            assert!(part.url.ends_with(&format!("/{file}")), "{}", part.url);
            assert_eq!(part.pack, Pack::File("yt-dlp".into()));
        }
        assert!(plan_for(Tool::Ffmpeg, "windows", "aarch64").is_none());
        assert!(plan_for(Tool::Ffmpeg, "linux", "riscv64").is_none());
        assert!(plan_for(Tool::YtDlp, "linux", "riscv64").is_none());
    }

    /// Each processor gets its own build: a download for the other one would be checked, unpacked
    /// and then not start.
    #[test]
    fn each_processor_has_its_own_download() {
        let urls = |tool, os, arch| -> Vec<String> {
            let plan: Plan = plan_for(tool, os, arch).unwrap();
            plan.parts.into_iter().map(|p| p.url).collect()
        };
        for os in ["macos", "linux"] {
            let arm = urls(Tool::Ffmpeg, os, "aarch64");
            let intel = urls(Tool::Ffmpeg, os, "x86_64");
            assert!(arm.iter().all(|u| u.contains("/arm64/")), "{arm:?}");
            assert!(intel.iter().all(|u| u.contains("/amd64/")), "{intel:?}");
            let folder = format!("/download/{os}/");
            assert!(
                arm.iter().chain(&intel).all(|u| u.contains(&folder)),
                "{os}: {arm:?} {intel:?}"
            );
        }
        assert!(urls(Tool::YtDlp, "linux", "x86_64")[0].ends_with("/yt-dlp_linux"));
        assert!(urls(Tool::YtDlp, "linux", "aarch64")[0].ends_with("/yt-dlp_linux_aarch64"));
        assert!(urls(Tool::YtDlp, "macos", "aarch64")[0].ends_with("/yt-dlp_macos"));
        assert!(urls(Tool::YtDlp, "windows", "x86_64")[0].ends_with("/yt-dlp.exe"));
        // The same release of ffmpeg on a Mac and on Linux; Windows' is its own.
        for (os, version) in [("macos", "9.0.2"), ("linux", "9.0.2"), ("windows", "8.1.2")] {
            assert_eq!(
                plan_for(Tool::Ffmpeg, os, "x86_64").unwrap().version,
                version
            );
        }
    }

    /// The other way to get a tool is the computer's own package manager: the one that's there,
    /// with the line it takes. A Linux with none Trek knows gets words, never another system's
    /// line.
    #[test]
    fn the_package_manager_is_the_one_the_computer_has() {
        let none = |_: &str| false;
        assert_eq!(
            install_line(Tool::Ffmpeg, "macos", None),
            Some("brew install ffmpeg"),
            "the line to pass on, with or without Homebrew"
        );
        assert_eq!(
            install_line(Tool::YtDlp, "windows", None),
            Some("winget install yt-dlp")
        );
        assert_eq!(
            install_hint(Tool::Ffmpeg, "windows", None),
            "`winget install ffmpeg`"
        );
        assert_eq!(package_manager_on("macos", none), None);
        assert_eq!(
            package_manager_on("macos", |p| p == "brew").map(|m| m.name),
            Some("Homebrew")
        );
        assert_eq!(
            package_manager_on("windows", |p| p == "winget").map(|m| m.name),
            Some("winget")
        );
        // Linux: the distribution's, by the program it has.
        for (program, name, ffmpeg, yt_dlp) in [
            (
                "apt-get",
                "apt",
                "sudo apt install ffmpeg",
                "sudo apt install yt-dlp",
            ),
            (
                "dnf",
                "dnf",
                "sudo dnf install ffmpeg-free",
                "sudo dnf install yt-dlp",
            ),
            (
                "pacman",
                "pacman",
                "sudo pacman -S ffmpeg",
                "sudo pacman -S yt-dlp",
            ),
            (
                "zypper",
                "zypper",
                "sudo zypper install ffmpeg",
                "sudo zypper install yt-dlp",
            ),
        ] {
            let found = package_manager_on("linux", |p| p == program);
            assert_eq!(found.map(|m| m.name), Some(name));
            assert_eq!(install_line(Tool::Ffmpeg, "linux", found), Some(ffmpeg));
            assert_eq!(install_line(Tool::YtDlp, "linux", found), Some(yt_dlp));
            assert_eq!(
                install_hint(Tool::Ffmpeg, "linux", found),
                format!("`{ffmpeg}`")
            );
        }
        // The first of the list where a computer has two.
        assert_eq!(
            package_manager_on("linux", |_| true).map(|m| m.name),
            Some("apt")
        );
        assert_eq!(package_manager_on("linux", none), None);
        assert_eq!(install_line(Tool::Ffmpeg, "linux", None), None);
        assert_eq!(
            install_hint(Tool::Ffmpeg, "linux", None),
            "their distribution's package manager"
        );
        // No line of another system's anywhere on Linux.
        for manager in package_managers("linux") {
            for tool in [Tool::Ffmpeg, Tool::YtDlp] {
                let line = manager.install_line(tool);
                assert!(
                    line.starts_with("sudo ") && !line.contains("brew") && !line.contains("winget"),
                    "{line}"
                );
            }
        }
        assert!(package_managers("freebsd").is_empty());
    }

    #[test]
    fn the_size_is_said_in_round_megabytes() {
        let plan = plan_for(Tool::Ffmpeg, "macos", "aarch64").unwrap();
        assert_eq!(plan.size_text(), "about 60 MB");
        let plan = plan_for(Tool::Ffmpeg, "windows", "x86_64").unwrap();
        assert_eq!(plan.size_text(), "about 110 MB");
        let plan = plan_for(Tool::Ffmpeg, "linux", "x86_64").unwrap();
        assert_eq!(plan.size_text(), "about 70 MB");
        let plan = plan_for(Tool::Ffmpeg, "linux", "aarch64").unwrap();
        assert_eq!(plan.size_text(), "about 60 MB");
    }

    #[test]
    fn progress_is_said_in_plain_words() {
        let p = Progress::Downloading {
            done: 42,
            total: 100,
        };
        assert_eq!(p.label(Tool::Ffmpeg), "Downloading ffmpeg · 42%");
        assert_eq!(Progress::Finishing.label(Tool::YtDlp), "Setting up yt-dlp…");
    }

    #[test]
    fn a_tool_is_found_in_the_folder_only_when_all_its_programs_are_there() {
        let dir = temp("find");
        assert!(!installed_in(&dir, Tool::Ffmpeg));
        std::fs::write(dir.join(file_name("ffmpeg")), "").unwrap();
        assert!(!installed_in(&dir, Tool::Ffmpeg));
        assert!(find_in(&dir, "ffmpeg").is_some());
        std::fs::write(dir.join(file_name("ffprobe")), "").unwrap();
        assert!(installed_in(&dir, Tool::Ffmpeg));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A made-up release served from this computer: a zip with ffmpeg and ffprobe stand-ins (the
    /// fixture program answers `-version`, as they do), and a SHA2-256SUMS for yt-dlp.
    struct Served {
        dir: PathBuf,
        base: String,
        zip_hash: String,
    }

    fn fixture() -> PathBuf {
        trek_test_fixtures::bin("fixture")
    }

    fn serve_release(name: &str) -> Served {
        use sha2::Digest as _;
        use std::io::Write as _;
        let dir = temp(name);
        let program = std::fs::read(fixture()).unwrap();
        let zip_path = dir.join("release.zip");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            for entry in [
                "release-1/bin/ffmpeg",
                "release-1/bin/ffprobe",
                "release-1/bin/ffplay",
                "release-1/bin/ffmpeg.exe",
                "release-1/bin/ffprobe.exe",
                "release-1/doc/ffmpeg.html",
            ] {
                zip.start_file(entry, options).unwrap();
                zip.write_all(&program).unwrap();
            }
            zip.finish().unwrap();
        }
        let zip_hash = hex::encode(sha2::Sha256::digest(std::fs::read(&zip_path).unwrap()));
        std::fs::write(dir.join("yt-dlp_test"), &program).unwrap();
        std::fs::write(
            dir.join("SHA2-256SUMS"),
            format!(
                "{}  yt-dlp_test\n",
                hex::encode(sha2::Sha256::digest(&program))
            ),
        )
        .unwrap();
        let base = serve(&dir);
        Served {
            dir,
            base,
            zip_hash,
        }
    }

    fn ffmpeg_plan(served: &Served, hash: &str) -> Plan {
        Plan {
            tool: Tool::Ffmpeg,
            version: "test",
            parts: vec![Part {
                url: format!("{}/release.zip", served.base),
                size: 1000,
                hash: Hash::Fixed(hash.into()),
                pack: Pack::Zip(vec![
                    (format!("bin/{}", file_name("ffmpeg")), "ffmpeg".into()),
                    (format!("bin/{}", file_name("ffprobe")), "ffprobe".into()),
                ]),
            }],
        }
    }

    #[test]
    fn a_checked_download_is_unpacked_run_once_and_put_in_place() {
        let served = serve_release("ok");
        let tools = served.dir.join("data/tools");
        let seen = std::sync::Mutex::new(Vec::new());
        crate::runtime()
            .block_on(install_into(
                &tools,
                &ffmpeg_plan(&served, &served.zip_hash),
                &|p| seen.lock().unwrap().push(p),
            ))
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert!(installed_in(&tools, Tool::Ffmpeg));
        // Only the two programs asked for, nothing left over.
        let mut files: Vec<_> = std::fs::read_dir(&tools)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files, [file_name("ffmpeg"), file_name("ffprobe")]);
        let seen = seen.lock().unwrap();
        assert!(matches!(seen.first(), Some(Progress::Downloading { .. })));
        assert_eq!(seen.last(), Some(&Progress::Finishing));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(tools.join("ffmpeg"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }
        let _ = std::fs::remove_dir_all(&served.dir);
    }

    #[test]
    fn a_download_that_doesn_t_match_its_checksum_is_refused_and_nothing_is_left() {
        let served = serve_release("bad");
        let tools = served.dir.join("data/tools");
        let err = crate::runtime()
            .block_on(install_into(
                &tools,
                &ffmpeg_plan(&served, &"0".repeat(64)),
                &|_| {},
            ))
            .unwrap_err();
        assert!(format!("{err:#}").contains("checksum"), "{err:#}");
        assert_eq!(std::fs::read_dir(&tools).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&served.dir);
    }

    #[test]
    fn an_archive_without_a_program_is_refused() {
        let served = serve_release("missing");
        let tools = served.dir.join("data/tools");
        let mut plan = ffmpeg_plan(&served, &served.zip_hash);
        if let Pack::Zip(entries) = &mut plan.parts[0].pack {
            entries.push(("bin/nothing-here".into(), "ffplay".into()));
        }
        let err = crate::runtime()
            .block_on(install_into(&tools, &plan, &|_| {}))
            .unwrap_err();
        assert!(err.to_string().contains("has no ffplay"), "{err:#}");
        assert_eq!(std::fs::read_dir(&tools).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&served.dir);
    }

    #[test]
    fn a_program_that_won_t_start_is_not_kept_and_an_older_one_stays() {
        let served = serve_release("broken");
        let tools = served.dir.join("data/tools");
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::write(tools.join(file_name("ffmpeg")), "older").unwrap();
        // The release's "ffmpeg" is text, which no system starts.
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(served.dir.join("release.zip")).unwrap());
        {
            use std::io::Write as _;
            let options = zip::write::SimpleFileOptions::default();
            for name in [file_name("ffmpeg"), file_name("ffprobe")] {
                zip.start_file(format!("bin/{name}"), options).unwrap();
                zip.write_all(b"not a program").unwrap();
            }
        }
        zip.finish().unwrap();
        use sha2::Digest as _;
        let hash = hex::encode(sha2::Sha256::digest(
            std::fs::read(served.dir.join("release.zip")).unwrap(),
        ));
        let err = crate::runtime()
            .block_on(install_into(&tools, &ffmpeg_plan(&served, &hash), &|_| {}))
            .unwrap_err();
        assert!(err.to_string().contains("won't"), "{err:#}");
        assert_eq!(
            std::fs::read_to_string(tools.join(file_name("ffmpeg"))).unwrap(),
            "older"
        );
        assert_eq!(std::fs::read_dir(&tools).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&served.dir);
    }

    #[test]
    fn a_program_downloaded_whole_is_checked_against_its_releases_sums() {
        let served = serve_release("sums");
        let tools = served.dir.join("data/tools");
        let plan = Plan {
            tool: Tool::YtDlp,
            version: "latest",
            parts: vec![Part {
                url: format!("{}/yt-dlp_test", served.base),
                size: 1000,
                hash: Hash::Sums {
                    url: format!("{}/SHA2-256SUMS", served.base),
                    file: "yt-dlp_test".into(),
                },
                pack: Pack::File("yt-dlp".into()),
            }],
        };
        crate::runtime()
            .block_on(install_into(&tools, &plan, &|_| {}))
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert!(installed_in(&tools, Tool::YtDlp));
        // Another file's name has no line in the sums: refused.
        let mut other = plan.clone();
        other.parts[0].hash = Hash::Sums {
            url: format!("{}/SHA2-256SUMS", served.base),
            file: "yt-dlp_other".into(),
        };
        let _ = std::fs::remove_dir_all(&tools);
        let err = crate::runtime()
            .block_on(install_into(&tools, &other, &|_| {}))
            .unwrap_err();
        assert!(err.to_string().contains("no checksum"), "{err:#}");
        let _ = std::fs::remove_dir_all(&served.dir);
    }

    /// Downloads the real builds for this computer into a temp folder and runs them. Off by
    /// default (network, 60 to 110 MB): `cargo test -p trek-core media_tools::tests::real -- --ignored`.
    #[test]
    #[ignore]
    fn real_download_into_a_temp_folder() {
        let dir = temp("real");
        for tool in [Tool::Ffmpeg, Tool::YtDlp] {
            let plan = plan(tool).expect("a build for this computer");
            let tools = dir.join("tools");
            let seen = std::sync::Mutex::new(Vec::new());
            crate::runtime()
                .block_on(install_into(&tools, &plan, &|p| {
                    seen.lock().unwrap().push(p)
                }))
                .unwrap_or_else(|e| panic!("{e:#}"));
            assert!(installed_in(&tools, tool));
            for program in tool.programs() {
                let flag = if *program == "yt-dlp" {
                    "--version"
                } else {
                    "-version"
                };
                let out = std::process::Command::new(find_in(&tools, program).unwrap())
                    .arg(flag)
                    .output()
                    .unwrap();
                eprintln!(
                    "{program}: {}",
                    String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .next()
                        .unwrap_or("")
                );
                assert!(out.status.success());
            }
            let seen = seen.lock().unwrap();
            assert!(seen.len() > 3, "{} progress steps", seen.len());
        }
        if std::env::var_os("TREK_KEEP_TOOLS").is_none() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn no_network_is_said_plainly() {
        let dir = temp("offline");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let plan = Plan {
            tool: Tool::Ffmpeg,
            version: "test",
            parts: vec![Part {
                url: format!("{base}/x.zip"),
                size: 1,
                hash: Hash::Fixed("0".repeat(64)),
                pack: Pack::File("ffmpeg".into()),
            }],
        };
        let err = crate::runtime()
            .block_on(install_into(&dir.join("tools"), &plan, &|_| {}))
            .unwrap_err();
        assert!(
            err.to_string().starts_with("Couldn't reach the download."),
            "{err:#}"
        );
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
