//! The speech model Trek's on-device engine runs (`local`): which one, where it's kept
//! (`dictation/` in Trek's local data folder), and getting it there the first time dictation is
//! used. It downloads over WinHTTP, so the system's proxy settings and certificates apply; an
//! interrupted download carries on where it stopped (`resume`), and a finished one is checked
//! against its SHA-256 before it's put in place. Nothing downloads until the mic is clicked.
//!
//! What's decided from what the server says is plain code every platform's tests run; the
//! download itself is Windows'.

/// A speech model: what it's called, where it comes from, and what it must be.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// What Settings calls it.
    pub name: &'static str,
    /// Its file in the model folder.
    pub file: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

/// Whisper's base model, multilingual, as whisper.cpp publishes it (f16 weights), pinned to one
/// revision of its repository so the checksum always matches. 74M parameters: on a laptop CPU
/// it decodes a few seconds of speech in a fraction of a second, so the text keeps up as the
/// user speaks, and it knows the 99 languages Whisper does. Larger models are markedly better
/// only in languages with less training data, and several times slower; smaller and quantized
/// ones save download but lose words.
pub const BASE: Spec = Spec {
    name: "Whisper base (multilingual)",
    file: "ggml-base.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.bin",
    bytes: 147_951_465,
    sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
};

/// The model folder's name in Trek's local data folder.
pub const FOLDER: &str = "dictation";

/// The model's size as the sentences about it say it: "141 MB".
pub fn size(spec: &Spec) -> String {
    format!("{} MB", crate::megabytes(spec.bytes, false))
}

/// What to do with a response to a request for the file, given the `have` bytes already on disk
/// (asked for as `Range: bytes=have-` when there are any).
#[derive(Debug, PartialEq, Eq)]
pub enum Resume {
    /// The rest of the file follows: add it to what's there.
    Append,
    /// The whole file follows: start again.
    Restart,
    /// There's nothing after what's there: it's all on disk already.
    Complete,
    /// The server refused (an HTTP status).
    Failed(u32),
}

/// The response's status and the start of its `Content-Range`, read against `have`.
pub fn resume(have: u64, status: u32, range_start: Option<u64>) -> Resume {
    match status {
        200 => Resume::Restart,
        206 if range_start == Some(have) => Resume::Append,
        // A range other than the one asked for: take the whole file again.
        206 => Resume::Restart,
        416 if have > 0 => Resume::Complete,
        status => Resume::Failed(status),
    }
}

/// The first byte a `Content-Range` header covers: `bytes 1000-1999/2000` is 1000.
pub fn range_start(header: &str) -> Option<u64> {
    let rest = header.trim().strip_prefix("bytes")?.trim_start();
    rest.split('-').next()?.trim().parse().ok()
}

/// Why getting the model failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// The server couldn't be reached: no connection, a name that didn't resolve, a timeout.
    Offline,
    /// The server answered with an HTTP error.
    Status(u32),
    /// The connection dropped part of the way (the next try carries on from there).
    Interrupted,
    /// All of it came, but it isn't the model: its checksum is wrong (it's deleted).
    Corrupt,
    /// The disk filled up.
    NoRoom,
    /// It couldn't be written for another reason.
    Disk(String),
    /// The ask that wanted it was given up.
    Cancelled,
}

impl Fetch {
    /// What a failed write means: a full disk, or something else.
    pub fn writing(e: &std::io::Error) -> Fetch {
        // ERROR_DISK_FULL, ERROR_HANDLE_DISK_FULL.
        if e.kind() == std::io::ErrorKind::StorageFull || matches!(e.raw_os_error(), Some(112 | 39))
        {
            Fetch::NoRoom
        } else {
            Fetch::Disk(e.to_string())
        }
    }

    /// What a WinHTTP error (its Win32 code) means: unreachable, or dropped part of the way.
    pub fn network(code: u32, started: bool) -> Fetch {
        // Timed out, name not resolved, can't connect, connection error, connection reset.
        let unreachable = matches!(code, 12002 | 12007 | 12029 | 12030 | 12031);
        if unreachable && !started {
            Fetch::Offline
        } else {
            Fetch::Interrupted
        }
    }
}

#[cfg(windows)]
pub use self::download::{fetch, present, remove};

#[cfg(windows)]
mod download {
    use super::{Fetch, Resume, Spec, range_start, resume};
    use ::windows::Win32::Networking::WinHttp::*;
    use ::windows::core::{HSTRING, PCWSTR, w};
    use sha2::{Digest, Sha256};
    use std::io::{Read, Seek, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The model, if it's in `dir` already (whole: the size it should be).
    pub fn present(spec: &Spec, dir: &Path) -> Option<PathBuf> {
        let path = dir.join(spec.file);
        std::fs::metadata(&path)
            .is_ok_and(|m| m.is_file() && m.len() == spec.bytes)
            .then_some(path)
    }

    /// Delete the model and any download of it under way.
    pub fn remove(spec: &Spec, dir: &Path) -> std::io::Result<()> {
        for path in [dir.join(spec.file), part(spec, dir)] {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }

    fn part(spec: &Spec, dir: &Path) -> PathBuf {
        dir.join(format!("{}.part", spec.file))
    }

    /// The model in `dir`, downloaded first if it isn't there: into a `.part` file, carrying on
    /// from what an earlier try left, then checked and renamed into place. `progress` hears the
    /// bytes on disk as they come; `cancelled` stops it between reads (what came stays, for the
    /// next try).
    pub fn fetch(
        spec: &Spec,
        dir: &Path,
        cancelled: &AtomicBool,
        progress: &dyn Fn(u64),
    ) -> Result<PathBuf, Fetch> {
        if let Some(path) = present(spec, dir) {
            return Ok(path);
        }
        std::fs::create_dir_all(dir).map_err(|e| Fetch::writing(&e))?;
        let part = part(spec, dir);
        // Written at its end, not opened to append: on Windows an append-only handle can't be
        // cut back (`set_len`, when the server sends the whole file again), which fails as
        // "Access is denied".
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&part)
            .map_err(|e| Fetch::writing(&e))?;
        let mut have = file.metadata().map(|m| m.len()).unwrap_or(0);
        if have > spec.bytes {
            file.set_len(0).map_err(|e| Fetch::writing(&e))?;
            have = 0;
        }
        file.seek(std::io::SeekFrom::Start(have))
            .map_err(|e| Fetch::writing(&e))?;
        progress(have);
        if have < spec.bytes {
            get(spec, &mut file, have, cancelled, progress)?;
        }
        drop(file);
        // All of it is there: it must be the model, byte for byte.
        if !matches(&part, spec.sha256).map_err(|e| Fetch::writing(&e))? {
            let _ = std::fs::remove_file(&part);
            return Err(Fetch::Corrupt);
        }
        let path = dir.join(spec.file);
        let _ = std::fs::remove_file(&path);
        std::fs::rename(&part, &path).map_err(|e| Fetch::writing(&e))?;
        Ok(path)
    }

    /// The file's SHA-256 is `want` (lowercase hex).
    fn matches(path: &Path, want: &str) -> std::io::Result<bool> {
        let mut file = std::fs::File::open(path)?;
        let mut hash = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        let got: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        Ok(got == want)
    }

    /// A WinHTTP handle, closed when it goes.
    struct Handle(*mut core::ffi::c_void);

    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: a handle WinHTTP gave out, closed once.
                let _ = unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    fn last_error() -> u32 {
        ::windows::core::Error::from_thread().code().0 as u32 & 0xFFFF
    }

    /// Request the rest of the file and append it to `file`, which has `have` bytes of it.
    fn get(
        spec: &Spec,
        file: &mut std::fs::File,
        mut have: u64,
        cancelled: &AtomicBool,
        progress: &dyn Fn(u64),
    ) -> Result<(), Fetch> {
        let url = spec.url.strip_prefix("https://").ok_or(Fetch::Status(0))?;
        let (host, path) = url.split_at(url.find('/').ok_or(Fetch::Status(0))?);
        let agent = HSTRING::from(format!("Trek/{}", env!("CARGO_PKG_VERSION")));
        // SAFETY: every call below gets handles WinHTTP gave out and are still open (each
        // `Handle` outlives its children), and buffers that outlive the call.
        unsafe {
            let session = Handle(WinHttpOpen(
                &agent,
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ));
            if session.0.is_null() {
                return Err(Fetch::network(last_error(), false));
            }
            // Resolve, connect, send, receive: a stalled connection gives up after 30 s.
            let _ = WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 30_000);
            let connection = Handle(WinHttpConnect(
                session.0,
                &HSTRING::from(host),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            ));
            if connection.0.is_null() {
                return Err(Fetch::network(last_error(), false));
            }
            let request = Handle(WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                &HSTRING::from(path),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ));
            if request.0.is_null() {
                return Err(Fetch::network(last_error(), false));
            }
            if have > 0 {
                let range: Vec<u16> = format!("Range: bytes={have}-").encode_utf16().collect();
                let _ = WinHttpAddRequestHeaders(request.0, &range, WINHTTP_ADDREQ_FLAG_ADD);
            }
            WinHttpSendRequest(request.0, None, None, 0, 0, 0)
                .and_then(|()| WinHttpReceiveResponse(request.0, std::ptr::null_mut()))
                .map_err(|e| Fetch::network(e.code().0 as u32 & 0xFFFF, false))?;

            let mut status = 0u32;
            let mut len = std::mem::size_of::<u32>() as u32;
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(&mut status as *mut u32 as *mut _),
                &mut len,
                std::ptr::null_mut(),
            )
            .map_err(|_| Fetch::Interrupted)?;
            let mut range = [0u16; 128];
            let mut len = std::mem::size_of_val(&range) as u32;
            let range = WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_CONTENT_RANGE,
                PCWSTR::null(),
                Some(range.as_mut_ptr() as *mut _),
                &mut len,
                std::ptr::null_mut(),
            )
            .ok()
            .and_then(|()| range_start(&String::from_utf16_lossy(&range[..len as usize / 2])));
            match resume(have, status, range) {
                Resume::Append => {}
                Resume::Restart => {
                    file.set_len(0).map_err(|e| Fetch::writing(&e))?;
                    file.seek(std::io::SeekFrom::Start(0))
                        .map_err(|e| Fetch::writing(&e))?;
                    have = 0;
                    progress(0);
                }
                Resume::Complete => return Ok(()),
                Resume::Failed(status) => return Err(Fetch::Status(status)),
            }

            let mut buf = vec![0u8; 256 << 10];
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(Fetch::Cancelled);
                }
                let mut read = 0u32;
                WinHttpReadData(
                    request.0,
                    buf.as_mut_ptr() as *mut _,
                    buf.len() as u32,
                    &mut read,
                )
                .map_err(|e| Fetch::network(e.code().0 as u32 & 0xFFFF, true))?;
                if read == 0 {
                    break;
                }
                file.write_all(&buf[..read as usize])
                    .map_err(|e| Fetch::writing(&e))?;
                have += read as u64;
                progress(have);
            }
        }
        file.flush().map_err(|e| Fetch::writing(&e))?;
        if have == spec.bytes {
            Ok(())
        } else {
            // The response ended short (or long) of the file.
            Err(Fetch::Interrupted)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_explorer_shows_them() {
        use crate::{Progress, megabytes};
        let said = |done| {
            Progress {
                done,
                total: BASE.bytes,
            }
            .said()
        };
        assert_eq!(size(&BASE), "141 MB");
        assert_eq!(said(31 << 20), "31 of 141 MB");
        assert_eq!(said(0), "0 of 141 MB");
        assert_eq!(said(BASE.bytes - (1 << 20)), "140 of 141 MB");
        assert_eq!(
            said(BASE.bytes + 10),
            "141 of 141 MB",
            "never past the total"
        );
        assert_eq!(megabytes((1 << 20) - 1, true), 0, "a part done rounds down");
        assert_eq!(megabytes((1 << 20) - 1, false), 1);
        let half = Progress {
            done: 50,
            total: 100,
        };
        assert_eq!(half.fraction(), 0.5);
        assert_eq!(Progress { done: 5, total: 0 }.fraction(), 0.);
        assert_eq!(
            Progress {
                done: 500,
                total: 100
            }
            .fraction(),
            1.
        );
    }

    #[test]
    fn a_download_carries_on_only_from_where_it_stopped() {
        assert_eq!(resume(0, 200, None), Resume::Restart);
        assert_eq!(resume(1_000, 206, Some(1_000)), Resume::Append);
        assert_eq!(
            resume(1_000, 200, None),
            Resume::Restart,
            "a server that ignores Range sends it all"
        );
        assert_eq!(
            resume(1_000, 206, Some(0)),
            Resume::Restart,
            "the wrong range: start again rather than splice"
        );
        assert_eq!(resume(1_000, 206, None), Resume::Restart);
        assert_eq!(
            resume(BASE.bytes, 416, None),
            Resume::Complete,
            "nothing after the end: it's all there (the checksum says whether it's right)"
        );
        assert_eq!(resume(0, 416, None), Resume::Failed(416));
        assert_eq!(resume(0, 404, None), Resume::Failed(404));
        assert_eq!(resume(5, 503, None), Resume::Failed(503));
    }

    #[test]
    fn content_ranges_are_read_for_their_start() {
        assert_eq!(range_start("bytes 1000-147951464/147951465"), Some(1_000));
        assert_eq!(range_start(" bytes 0-9/10"), Some(0));
        assert_eq!(range_start("bytes */147951465"), None);
        assert_eq!(range_start("items 1-2/3"), None);
        assert_eq!(range_start(""), None);
    }

    #[test]
    fn errors_say_whether_the_server_was_reached() {
        for code in [12002, 12007, 12029, 12030] {
            assert_eq!(Fetch::network(code, false), Fetch::Offline, "{code}");
            assert_eq!(Fetch::network(code, true), Fetch::Interrupted, "{code}");
        }
        assert_eq!(Fetch::network(12175, false), Fetch::Interrupted, "TLS");
        let full = std::io::Error::from_raw_os_error(112);
        assert_eq!(Fetch::writing(&full), Fetch::NoRoom);
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        assert_eq!(Fetch::writing(&denied), Fetch::Disk("denied".into()));
    }

    #[test]
    fn the_model_is_pinned_and_checksummed() {
        assert!(
            BASE.url
                .starts_with("https://huggingface.co/ggerganov/whisper.cpp/resolve/")
        );
        assert!(BASE.url.ends_with(BASE.file));
        assert!(
            !BASE.url.contains("/main/"),
            "a revision, so the checksum can't drift"
        );
        assert_eq!(BASE.sha256.len(), 64);
        assert!(
            BASE.sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    /// The real download, from Hugging Face into a folder of the test's own: stopped part of the
    /// way (as ✕ stops it), then carried on from there rather than started again, and checked
    /// against its SHA-256 before it's in place. 141 MB over the network, so it runs only when
    /// asked: `cargo test -p trek-dictate -- --ignored a_download`.
    #[cfg(windows)]
    #[test]
    #[ignore = "downloads the 141 MB speech model"]
    fn a_download_stops_carries_on_and_is_checked() {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        let dir = std::env::temp_dir().join(format!("trek-dictate-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cancel = AtomicBool::new(false);
        let seen = AtomicU64::new(0);
        let first = fetch(&BASE, &dir, &cancel, &|done| {
            seen.store(done, Ordering::Relaxed);
            if done >= 20 << 20 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(first, Err(Fetch::Cancelled));
        let part = dir.join(format!("{}.part", BASE.file));
        let kept = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        assert!(kept >= 20 << 20 && kept < BASE.bytes, "{kept}");
        assert_eq!(present(&BASE, &dir), None, "not in place until it's whole");

        cancel.store(false, Ordering::Relaxed);
        let from = AtomicU64::new(u64::MAX);
        let started = std::time::Instant::now();
        let done = fetch(&BASE, &dir, &cancel, &|done| {
            let _ = from.compare_exchange(u64::MAX, done, Ordering::Relaxed, Ordering::Relaxed);
        });
        assert_eq!(done, Ok(dir.join(BASE.file)), "checked and in place");
        assert_eq!(
            from.load(Ordering::Relaxed),
            kept,
            "it carried on from what was there"
        );
        assert!(!part.exists());
        eprintln!(
            "resumed at {} MB, the rest in {:?}",
            kept >> 20,
            started.elapsed()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real download path, over a folder of the test's own: a model already in place (the
    /// right size) is used as it is, and removing it takes any half-finished download too.
    #[cfg(windows)]
    #[test]
    fn a_model_in_place_is_used_and_removed() {
        let dir = std::env::temp_dir().join(format!("trek-dictate-model-{}", std::process::id()));
        let tiny = Spec {
            name: "test",
            file: "model.bin",
            url: "https://example.invalid/model.bin",
            bytes: 4,
            sha256: "",
        };
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.bin"), b"1234").unwrap();
        std::fs::write(dir.join("model.bin.part"), b"12").unwrap();
        assert_eq!(present(&tiny, &dir), Some(dir.join("model.bin")));
        let never = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(
            fetch(&tiny, &dir, &never, &|_| {}),
            Ok(dir.join("model.bin")),
            "nothing downloads when it's there"
        );
        remove(&tiny, &dir).unwrap();
        assert_eq!(present(&tiny, &dir), None);
        assert!(!dir.join("model.bin.part").exists());
        remove(&tiny, &dir).unwrap();
        // The wrong size isn't the model.
        std::fs::write(dir.join("model.bin"), b"123").unwrap();
        assert_eq!(present(&tiny, &dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
