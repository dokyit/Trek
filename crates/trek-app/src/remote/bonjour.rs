//! The phone server announced on the local network (Bonjour, `_trek._tcp`), so a paired phone
//! finds this Mac again when its address changes: the router handed out another one, or the
//! phone was paired on another network. The record carries this Mac's lasting id (`id=`), which
//! the phone matches against the Mac it paired with, and its name (`name=`), so another Mac on the
//! same Wi-Fi can offer it by name when pairing a machine; the connection is still TLS pinned to
//! this Mac's certificate, so a record that lies gets nowhere.
//!
//! A Mac registers it with its mDNS responder (dns_sd's DNSServiceRegister); Windows with its own
//! (windns's DnsServiceRegister, Windows 10 1809 and later), which answers for the PC's name on
//! `.local` too. Elsewhere nothing is announced.

// Elsewhere the record is built only for the tests; Windows gives the system its entries.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

/// The service type phones browse for.
pub const SERVICE: &str = "_trek._tcp";

/// The TXT record's entries, `id=<host id>` and `name=<this Mac's name>`, each at most the 255
/// bytes DNS-SD allows (a longer name is cut at a character boundary).
fn entries(host_id: &str, name: &str) -> [String; 2] {
    [format!("id={host_id}"), format!("name={name}")].map(|entry| {
        let mut end = entry.len().min(255);
        while !entry.is_char_boundary(end) {
            end -= 1;
        }
        entry[..end].to_string()
    })
}

/// The TXT record as DNS-SD wants it: each of its entries led by its length.
pub fn txt_record(host_id: &str, name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in entries(host_id, name) {
        out.push(entry.len() as u8);
        out.extend_from_slice(entry.as_bytes());
    }
    out
}

/// A registration with the system's mDNS responder, withdrawn when dropped.
pub struct Advert {
    #[cfg(target_os = "macos")]
    sd_ref: ffi::DNSServiceRef,
    #[cfg(windows)]
    registration: Option<win::Registration>,
}

// The handle is only deallocated, on whichever thread drops it.
unsafe impl Send for Advert {}

impl Advert {
    /// Announce the server on `port` under this Mac's name. `None` when the responder refused it
    /// (the phone then only knows the address it paired with, as before).
    #[cfg(target_os = "macos")]
    pub fn new(name: &str, host_id: &str, port: u16) -> Option<Self> {
        use std::ffi::CString;
        let txt = txt_record(host_id, name);
        let name = CString::new(name.replace('\0', "")).ok()?;
        let kind = CString::new(SERVICE).ok()?;
        let mut sd_ref: ffi::DNSServiceRef = std::ptr::null_mut();
        // SAFETY: every pointer is valid for the call; the responder copies what it keeps. No
        // callback: the registration stands (renamed on a clash) for as long as `sd_ref` lives.
        let err = unsafe {
            ffi::DNSServiceRegister(
                &mut sd_ref,
                0,
                0,
                name.as_ptr(),
                kind.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                port.to_be(),
                txt.len() as u16,
                txt.as_ptr().cast(),
                None,
                std::ptr::null_mut(),
            )
        };
        if err != 0 || sd_ref.is_null() {
            tracing::warn!("phone server: Bonjour registration failed ({err})");
            return None;
        }
        Some(Self { sd_ref })
    }

    /// Announce the server on `port` under this PC's name (Windows' own responder answers for
    /// the PC's `.local` name). `None` when it refused it, or this Windows has no
    /// DnsServiceRegister.
    #[cfg(windows)]
    pub fn new(name: &str, host_id: &str, port: u16) -> Option<Self> {
        let registration = win::Registration::new(name, host_id, port)?;
        Some(Self {
            registration: Some(registration),
        })
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    pub fn new(_name: &str, _host_id: &str, _port: u16) -> Option<Self> {
        None
    }
}

#[cfg(windows)]
impl Drop for Advert {
    fn drop(&mut self) {
        if let Some(registration) = self.registration.take() {
            registration.withdraw();
        }
    }
}

/// windns.h's service registration (dnsapi.dll), declared here as dns_sd's is for the Mac.
#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::mem::ManuallyDrop;
    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, Instant};

    const DNS_QUERY_REQUEST_VERSION1: u32 = 1;
    /// What DnsServiceRegister and DnsServiceDeRegister return when they'll answer by callback.
    const DNS_REQUEST_PENDING: u32 = 9506;
    /// How long a withdrawal may take to be answered before what it used is kept for good.
    const WITHDRAW_WAIT: Duration = Duration::from_secs(5);

    type Complete =
        unsafe extern "system" fn(status: u32, context: *const c_void, instance: *const c_void);

    /// DNS_SERVICE_REGISTER_REQUEST.
    #[repr(C)]
    struct Request {
        version: u32,
        interface_index: u32,
        instance: *mut c_void,
        callback: Option<Complete>,
        context: *mut c_void,
        credentials: *mut c_void,
        unicast: i32,
    }

    #[link(name = "dnsapi", kind = "raw-dylib")]
    unsafe extern "system" {
        #[allow(clippy::too_many_arguments)]
        fn DnsServiceConstructInstance(
            service: *const u16,
            host: *const u16,
            ip4: *const u32,
            ip6: *const c_void,
            port: u16,
            priority: u16,
            weight: u16,
            properties: u32,
            keys: *const *const u16,
            values: *const *const u16,
        ) -> *mut c_void;
        fn DnsServiceFreeInstance(instance: *const c_void);
        fn DnsServiceRegister(request: *const Request, cancel: *mut c_void) -> u32;
        fn DnsServiceDeRegister(request: *const Request, cancel: *mut c_void) -> u32;
    }

    /// What the responder's answers come back to: the registration's, then the withdrawal's.
    struct Heard {
        /// The instance the request carries. The responder may hand it back in an answer, which
        /// isn't the answer's to free; an instance of its own it hands over is.
        ours: *mut c_void,
        answers: Mutex<u32>,
        changed: Condvar,
    }

    /// An answer from the responder, on one of its threads.
    unsafe extern "system" fn heard(status: u32, context: *const c_void, instance: *const c_void) {
        // SAFETY: `context` is the registration's `Heard`, which stays until both its answers
        // came (`withdraw`), or for good.
        let heard = unsafe { &*context.cast::<Heard>() };
        if !instance.is_null() && instance != heard.ours.cast_const() {
            // SAFETY: an instance the responder made for this answer, given to us to free.
            unsafe { DnsServiceFreeInstance(instance) };
        }
        if status != 0 {
            tracing::warn!("phone server: mDNS registration answered {status}");
        }
        *heard.answers.lock().unwrap() += 1;
        heard.changed.notify_all();
    }

    /// This PC's name on the network, as `.local` names it.
    fn host_name() -> Option<String> {
        use windows_sys::Win32::System::SystemInformation::{
            ComputerNameDnsHostname, GetComputerNameExW,
        };
        let mut len = 0u32;
        // SAFETY: a size query (no buffer), then a buffer of the size it gave.
        unsafe { GetComputerNameExW(ComputerNameDnsHostname, std::ptr::null_mut(), &mut len) };
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        // SAFETY: as above; `len` is the buffer's size in characters.
        let ok = unsafe { GetComputerNameExW(ComputerNameDnsHostname, buf.as_mut_ptr(), &mut len) };
        let name = String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]);
        (ok != 0 && !name.is_empty()).then_some(name)
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// The service registered: what the responder holds on to, kept until it lets go.
    pub struct Registration {
        request: Box<Request>,
        heard: Box<Heard>,
    }

    // Raw pointers that only the responder's calls use, from whichever thread.
    unsafe impl Send for Registration {}

    impl Registration {
        pub fn new(name: &str, host_id: &str, port: u16) -> Option<Self> {
            // The instance name is the service name's first label: a dot would split it.
            let label = name.replace(['.', '\0'], " ");
            let service = wide(&format!("{}.{}.local", label.trim(), super::SERVICE));
            let host = wide(&format!("{}.local", host_name()?));
            let (keys, values): (Vec<Vec<u16>>, Vec<Vec<u16>>) = super::entries(host_id, name)
                .iter()
                .map(|e| {
                    let (k, v) = e.split_once('=').unwrap_or((e.as_str(), ""));
                    (wide(k), wide(v))
                })
                .unzip();
            let keys: Vec<*const u16> = keys.iter().map(|k| k.as_ptr()).collect();
            let values: Vec<*const u16> = values.iter().map(|v| v.as_ptr()).collect();
            // SAFETY: every string is NUL-terminated and outlives the call, which copies them
            // into the instance (freed with DnsServiceFreeInstance). No addresses: the system
            // answers for the PC's own name.
            let ours = unsafe {
                DnsServiceConstructInstance(
                    service.as_ptr(),
                    host.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    port,
                    0,
                    0,
                    keys.len() as u32,
                    keys.as_ptr(),
                    values.as_ptr(),
                )
            };
            if ours.is_null() {
                tracing::warn!("phone server: couldn't make the mDNS record");
                return None;
            }
            let heard = Box::new(Heard {
                ours,
                answers: Mutex::new(0),
                changed: Condvar::new(),
            });
            let request = Box::new(Request {
                version: DNS_QUERY_REQUEST_VERSION1,
                interface_index: 0,
                instance: ours,
                callback: Some(self::heard),
                context: (&*heard as *const Heard).cast_mut().cast(),
                credentials: std::ptr::null_mut(),
                unicast: 0,
            });
            // SAFETY: the request, its instance and its context are boxed and live until the
            // registration is withdrawn and that's answered (`withdraw`).
            let status = unsafe { DnsServiceRegister(&*request, std::ptr::null_mut()) };
            if status != DNS_REQUEST_PENDING {
                tracing::warn!("phone server: mDNS registration failed ({status})");
                // Nothing was registered, so nothing will answer.
                // SAFETY: the instance made above, used by nothing now.
                unsafe { DnsServiceFreeInstance(ours) };
                return None;
            }
            Some(Self { request, heard })
        }

        /// Take the service off the network. On a thread of its own (the answer comes by
        /// callback, and the UI mustn't wait for it); what the responder used is freed once
        /// both answers came, else kept for good (it's small, and a late answer finds it there).
        pub fn withdraw(self) {
            let me = ManuallyDrop::new(self);
            let _ = std::thread::Builder::new()
                .name("trek-mdns-withdraw".into())
                .spawn(move || {
                    // SAFETY: the request that registered, still alive.
                    let status =
                        unsafe { DnsServiceDeRegister(&*me.request, std::ptr::null_mut()) };
                    let deadline = Instant::now() + WITHDRAW_WAIT;
                    let mut answers = me.heard.answers.lock().unwrap();
                    while status == DNS_REQUEST_PENDING && *answers < 2 {
                        let left = deadline.saturating_duration_since(Instant::now());
                        if left.is_zero() {
                            break;
                        }
                        answers = me.heard.changed.wait_timeout(answers, left).unwrap().0;
                    }
                    let done = status == DNS_REQUEST_PENDING && *answers >= 2;
                    drop(answers);
                    if !done {
                        tracing::warn!("phone server: mDNS withdrawal unanswered ({status})");
                        return;
                    }
                    let me = ManuallyDrop::into_inner(me);
                    // SAFETY: the instance made in `new`; both answers came, so the responder
                    // has let go of it.
                    unsafe { DnsServiceFreeInstance(me.heard.ours) };
                    drop(me);
                });
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for Advert {
    fn drop(&mut self) {
        // SAFETY: `sd_ref` came from a successful DNSServiceRegister and is freed once.
        unsafe { ffi::DNSServiceRefDeallocate(self.sd_ref) }
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use std::os::raw::{c_char, c_void};
    pub type DNSServiceRef = *mut c_void;
    type RegisterReply = extern "C" fn(
        DNSServiceRef,
        u32,
        i32,
        *const c_char,
        *const c_char,
        *const c_char,
        *mut c_void,
    );
    // dns_sd.h, in libSystem.
    unsafe extern "C" {
        #[allow(clippy::too_many_arguments)]
        pub fn DNSServiceRegister(
            sd_ref: *mut DNSServiceRef,
            flags: u32,
            interface_index: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            host: *const c_char,
            port_network_order: u16,
            txt_len: u16,
            txt_record: *const c_void,
            callback: Option<RegisterReply>,
            context: *mut c_void,
        ) -> i32;
        pub fn DNSServiceRefDeallocate(sd_ref: DNSServiceRef);
    }
}

/// What `dns-sd -L <name> _trek._tcp local.` (a Mac's look-up of one record) printed so far: the
/// record's port and its TXT entries once both have come. It prints where the record can be
/// reached (`… can be reached at <host>:<port> (interface 1)`), then the TXT entries on the next
/// line, space-separated, with a backslash before a space or backslash inside one (`name=Trek\
/// live\ check`, as CI's Mac printed it) and `\DDD` for a byte it won't print. Plain text, so both
/// CIs test it; the capture build's `bonjour found` reads a Mac's answer with it.
#[cfg(any(test, feature = "shots"))]
pub fn read_lookup(lines: &[String]) -> Option<(u16, Vec<String>)> {
    let reached = lines
        .iter()
        .position(|line| line.contains(" can be reached at "))?;
    let port = lines[reached]
        .split_once(" can be reached at ")
        .and_then(|(_, at)| at.split(" (").next())
        .and_then(|at| at.rsplit_once(':'))
        .and_then(|(_, port)| port.trim().parse().ok())?;
    let line = lines.get(reached + 1)?.as_bytes();
    let (mut entries, mut entry) = (Vec::new(), Vec::new());
    let mut at = 0;
    while at < line.len() {
        match line[at] {
            b'\\' => {
                let digits = line
                    .get(at + 1..at + 4)
                    .filter(|d| d.iter().all(u8::is_ascii_digit));
                match digits.and_then(|d| std::str::from_utf8(d).ok()?.parse::<u8>().ok()) {
                    Some(byte) => {
                        entry.push(byte);
                        at += 4;
                    }
                    None => {
                        entry.extend(line.get(at + 1));
                        at += 2;
                    }
                }
            }
            b' ' => {
                if !entry.is_empty() {
                    entries.push(String::from_utf8_lossy(&std::mem::take(&mut entry)).into_owned());
                }
                at += 1;
            }
            byte => {
                entry.push(byte);
                at += 1;
            }
        }
    }
    if !entry.is_empty() {
        entries.push(String::from_utf8_lossy(&entry).into_owned());
    }
    Some((port, entries))
}

#[cfg(test)]
mod tests {
    /// A Mac's `dns-sd -L` as it prints a record: the header, then where it can be reached, then
    /// its TXT entries; read once the entries have come.
    #[test]
    fn a_macs_lookup_reads_as_the_port_and_the_entries() {
        let lines: Vec<String> = [
            "Lookup Trek live check._trek._tcp.local",
            "DATE: ---Thu 12 Nov 2026---",
            "14:20:01.123  ...STARTING...",
            "14:20:01.456  Trek\\032live\\032check._trek._tcp.local. can be reached at runner.local.:7431 (interface 1) Flags: 1",
            // As CI's Mac printed it.
            " id=032ed02c3793cd0500006f16 name=Trek\\ live\\ check",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(super::read_lookup(&lines[..4]), None, "no entries yet");
        let (port, entries) = super::read_lookup(&lines).expect("the record");
        assert_eq!(port, 7431);
        assert_eq!(
            entries,
            ["id=032ed02c3793cd0500006f16", "name=Trek live check"]
        );
        assert_eq!(super::read_lookup(&lines[..3]), None);
        // A byte written as `\DDD`, and a backslash of its own.
        let mut other = lines[..4].to_vec();
        other.push(r" name=Caf\195\169\032one path=C:\\Trek".into());
        let (_, entries) = super::read_lookup(&other).expect("the record");
        assert_eq!(entries, ["name=Café one", r"path=C:\Trek"]);
    }

    #[test]
    fn the_txt_record_carries_the_id_and_the_name_length_led() {
        assert_eq!(
            super::txt_record("abc", "Studio"),
            b"\x06id=abc\x0bname=Studio".to_vec()
        );
        assert_eq!(super::txt_record(&"x".repeat(300), "M").len(), 256 + 7);
        // A long name is cut whole characters at a time ("é" is two bytes: 5 + 125 * 2 = 255).
        let txt = super::txt_record("a", &"é".repeat(200));
        let name = &txt[5..];
        assert_eq!(name[0], 255);
        assert!(std::str::from_utf8(&name[1..]).is_ok());
    }
}
