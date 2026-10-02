//! Process, file and image helpers shared by the tool families.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;

/// Longest side of any image we hand to the model.
pub const MAX_IMAGE_SIDE: u32 = 1568;

pub struct Output {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    /// Short human-readable reason for a failure.
    pub fn reason(&self) -> String {
        let msg = if self.stderr.trim().is_empty() {
            self.stdout.trim()
        } else {
            self.stderr.trim()
        };
        match self.status {
            Some(code) if msg.is_empty() => format!("exit status {code}"),
            Some(_) => msg.to_string(),
            None => format!("timed out or killed{}", if msg.is_empty() { String::new() } else { format!(": {msg}") }),
        }
    }
}

/// Run `program args…`, optionally feeding `stdin`, killing it after `timeout`.
pub fn run(program: &str, args: &[&str], stdin: Option<&str>, timeout: Duration) -> Result<Output, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run {program}: {e}"))?;

    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let input = input.to_string();
        std::thread::spawn(move || {
            let _ = pipe.write_all(input.as_bytes());
        });
    }
    let mut out_pipe = child.stdout.take().expect("piped stdout");
    let mut err_pipe = child.stderr.take().expect("piped stderr");
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("Failed waiting for {program}: {e}")),
        }
    };
    let stdout = String::from_utf8_lossy(&out_thread.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_thread.join().unwrap_or_default()).into_owned();
    Ok(Output { status, stdout, stderr })
}

/// Run and require success, returning stdout.
pub fn run_ok(program: &str, args: &[&str], stdin: Option<&str>, timeout: Duration) -> Result<String, String> {
    let out = run(program, args, stdin, timeout)?;
    if out.success() {
        Ok(out.stdout)
    } else {
        Err(out.reason())
    }
}

/// A unique temp file path that is removed on drop.
pub struct TempFile(pub PathBuf);

impl TempFile {
    pub fn new(ext: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!("trek-mcp-{}-{n}.{ext}", std::process::id())))
    }

    pub fn path_str(&self) -> &str {
        self.0.to_str().expect("temp dir is valid UTF-8")
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Width and height from a PNG's IHDR chunk.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

/// Resize the PNG at `path` in place so its longest side is `max_side`
/// (no-op if already that size), using `sips -Z`.
pub fn sips_fit(path: &str, max_side: u32) -> Result<(), String> {
    let side = max_side.max(1).to_string();
    run_ok(
        "/usr/bin/sips",
        &["-s", "format", "png", "-Z", &side, path, "--out", path],
        None,
        Duration::from_secs(30),
    )
    .map(|_| ())
    .map_err(|e| format!("sips failed to resize screenshot: {e}"))
}

/// Read a PNG file and return (base64, width, height).
pub fn load_png(path: &Path) -> Result<(String, u32, u32), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    let (w, h) = png_size(&bytes).ok_or_else(|| "Screenshot is not a valid PNG".to_string())?;
    Ok((base64::engine::general_purpose::STANDARD.encode(&bytes), w, h))
}

/// Find an executable by name on PATH plus common install dirs.
pub fn find_executable(name: &str, extra_dirs: &[&str]) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(extra_dirs.iter().map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/bin"));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// Format a float compactly: 1.0 → "1", 1.6333 → "1.633".
pub fn fmt_num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_header() {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        b.extend_from_slice(&1568u32.to_be_bytes());
        b.extend_from_slice(&1018u32.to_be_bytes());
        assert_eq!(png_size(&b), Some((1568, 1018)));
        assert_eq!(png_size(b"nope"), None);
    }

    #[test]
    fn numbers() {
        assert_eq!(fmt_num(1.0), "1");
        assert_eq!(fmt_num(1.63333), "1.633");
        assert_eq!(fmt_num(2.5), "2.5");
    }

    #[test]
    fn run_captures_and_times_out() {
        let out = run("/bin/sh", &["-c", "cat; echo err >&2"], Some("hello"), Duration::from_secs(5)).unwrap();
        assert!(out.success());
        assert_eq!(out.stdout, "hello");
        assert_eq!(out.stderr.trim(), "err");
        let out = run("/bin/sleep", &["5"], None, Duration::from_millis(100)).unwrap();
        assert_eq!(out.status, None);
    }
}
