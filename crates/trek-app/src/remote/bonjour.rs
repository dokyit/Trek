//! The phone server announced on the local network (Bonjour, `_trek._tcp`), so a paired phone
//! finds this Mac again when its address changes: the router handed out another one, or the
//! phone was paired on another network. The record carries this Mac's lasting id (`id=`), which
//! the phone matches against the Mac it paired with, and its name (`name=`), so another Mac on the
//! same Wi-Fi can offer it by name when pairing a machine; the connection is still TLS pinned to
//! this Mac's certificate, so a record that lies gets nowhere.

// Only a Mac announces itself (DNSServiceRegister); elsewhere the record is built for the tests.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

/// The service type phones browse for.
pub const SERVICE: &str = "_trek._tcp";

/// The TXT record: `id=<host id>` and `name=<this Mac's name>`, as DNS-SD wants it (each entry
/// led by its length, at most 255 bytes: a longer name is cut at a character boundary).
pub(crate) fn txt_record(host_id: &str, name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in [format!("id={host_id}"), format!("name={name}")] {
        let mut end = entry.len().min(255);
        while !entry.is_char_boundary(end) {
            end -= 1;
        }
        out.push(end as u8);
        out.extend_from_slice(&entry.as_bytes()[..end]);
    }
    out
}

/// A registration with the system's mDNS responder, withdrawn when dropped.
pub struct Advert {
    #[cfg(target_os = "macos")]
    sd_ref: ffi::DNSServiceRef,
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

    #[cfg(not(target_os = "macos"))]
    pub fn new(_name: &str, _host_id: &str, _port: u16) -> Option<Self> {
        None
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
    type RegisterReply = extern "C" fn(DNSServiceRef, u32, i32, *const c_char, *const c_char, *const c_char, *mut c_void);
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

#[cfg(test)]
mod tests {
    #[test]
    fn the_txt_record_carries_the_id_and_the_name_length_led() {
        assert_eq!(super::txt_record("abc", "Studio"), b"\x06id=abc\x0bname=Studio".to_vec());
        assert_eq!(super::txt_record(&"x".repeat(300), "M").len(), 256 + 7);
        // A long name is cut whole characters at a time ("é" is two bytes: 5 + 125 * 2 = 255).
        let txt = super::txt_record("a", &"é".repeat(200));
        let name = &txt[5..];
        assert_eq!(name[0], 255);
        assert!(std::str::from_utf8(&name[1..]).is_ok());
    }
}
