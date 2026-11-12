//! A Bonjour record looked up as a phone finds one (`bonjour found`): resolved through the
//! system's own DNS-SD, the same responder Trek registered it with (`remote::bonjour`). Windows
//! asks its DNS client (DnsServiceResolve); a Mac asks mDNSResponder through `dns-sd`.

use std::time::Duration;

/// What a resolved record says: its port, and its TXT entries as `key=value`.
pub struct Found {
    pub port: u16,
    pub entries: Vec<String>,
}

/// The record of service instance `name` (`<name>._trek._tcp.local`), within `wait`.
#[cfg(windows)]
pub fn resolve(name: &str, wait: Duration) -> anyhow::Result<Found> {
    use std::ffi::c_void;
    use std::sync::mpsc::{Sender, channel};
    use windows_sys::Win32::NetworkManagement::Dns::{
        DNS_QUERY_REQUEST_VERSION1, DNS_SERVICE_CANCEL, DNS_SERVICE_INSTANCE,
        DNS_SERVICE_RESOLVE_REQUEST, DnsServiceFreeInstance, DnsServiceResolve,
        DnsServiceResolveCancel,
    };

    /// What DnsServiceResolve returns when it'll answer by callback.
    const DNS_REQUEST_PENDING: i32 = 9506;
    type Answer = Result<Found, u32>;

    /// The request and what it points to, kept until its one answer comes.
    struct Asked {
        tx: Sender<Answer>,
        _query: Vec<u16>,
        request: DNS_SERVICE_RESOLVE_REQUEST,
    }

    /// A NUL-terminated UTF-16 string from the responder.
    unsafe fn text(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        // SAFETY: the caller's NUL-terminated string.
        let len = (0..).take_while(|&n| unsafe { *p.add(n) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
    }

    unsafe extern "system" fn resolved(
        status: u32,
        context: *const c_void,
        instance: *const DNS_SERVICE_INSTANCE,
    ) {
        // SAFETY: the `Asked` boxed below, handed back with its one answer (or its cancelling).
        let asked = unsafe { Box::from_raw(context.cast_mut().cast::<Asked>()) };
        let answer = if status == 0 && !instance.is_null() {
            // SAFETY: the responder's instance, freed here once read.
            let found = unsafe {
                let i = &*instance;
                let entries = (0..i.dwPropertyCount as usize)
                    .map(|n| format!("{}={}", text(*i.keys.add(n)), text(*i.values.add(n))))
                    .collect();
                Found {
                    port: i.wPort,
                    entries,
                }
            };
            unsafe { DnsServiceFreeInstance(instance) };
            Ok(found)
        } else {
            Err(status)
        };
        let _ = asked.tx.send(answer);
    }

    let (tx, rx) = channel::<Answer>();
    let mut query: Vec<u16> = format!("{name}.{}.local", crate::remote::bonjour::SERVICE)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let asked = Box::into_raw(Box::new(Asked {
        tx,
        request: DNS_SERVICE_RESOLVE_REQUEST {
            Version: DNS_QUERY_REQUEST_VERSION1,
            InterfaceIndex: 0,
            QueryName: query.as_mut_ptr(),
            pResolveCompletionCallback: Some(resolved),
            pQueryContext: std::ptr::null_mut(),
        },
        _query: query,
    }));
    let mut cancel = DNS_SERVICE_CANCEL::default();
    // SAFETY: `asked` lives until the callback takes it back; the request points into it.
    let status = unsafe {
        (*asked).request.pQueryContext = asked.cast();
        DnsServiceResolve(&(*asked).request, &mut cancel)
    };
    if status != DNS_REQUEST_PENDING {
        // SAFETY: nothing was asked, so no callback will take it.
        drop(unsafe { Box::from_raw(asked) });
        anyhow::bail!("DnsServiceResolve: {status}");
    }
    match rx.recv_timeout(wait) {
        Ok(Ok(found)) => Ok(found),
        Ok(Err(status)) => anyhow::bail!("DNS-SD found no {name} ({status})"),
        Err(_) => {
            // The callback still comes, cancelled, and lets go of what it was given.
            // SAFETY: the handle DnsServiceResolve filled.
            unsafe { DnsServiceResolveCancel(&cancel) };
            anyhow::bail!("DNS-SD found no {name} within {wait:?}")
        }
    }
}

/// The record of service instance `name`, within `wait`: `dns-sd -L` (macOS's own tool for its
/// responder) prints where it can be reached, then its TXT entries, and runs until stopped.
#[cfg(target_os = "macos")]
pub fn resolve(name: &str, wait: Duration) -> anyhow::Result<Found> {
    use std::io::BufRead as _;
    use std::process::{Command, Stdio};
    let mut child = Command::new("/usr/bin/dns-sd")
        .args(["-L", name, crate::remote::bonjour::SERVICE, "local."])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("dns-sd: no output"))?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    // What it printed so far, read as it comes (`remote::bonjour::read_lookup`, tested on both).
    let deadline = std::time::Instant::now() + wait;
    let mut lines = Vec::new();
    let mut found = None;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        let Ok(line) = rx.recv_timeout(left) else {
            break;
        };
        lines.push(line);
        if let Some((port, entries)) = crate::remote::bonjour::read_lookup(&lines) {
            found = Some(Found { port, entries });
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    found.ok_or_else(|| {
        anyhow::anyhow!("dns-sd found no {name} within {wait:?} (it said {lines:?})")
    })
}
