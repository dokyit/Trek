//! Who the phone server answers, and what Settings › Phone can say of the link.
//!
//! The server listens on every interface (`[::]:7420`, IPv4 too), so a new address (another
//! network, DHCP, Wi-Fi back after sleep, Tailscale coming up) never needs it to start again, and
//! a phone reaches it on whichever of this Mac's addresses it dials. What the `reach` setting
//! keeps out is decided per connection instead, from where it comes from and which address it
//! reached ([`LinkPolicy::admits`]): on "Wi-Fi", phones on the local network and on the tailnet
//! (as before, when the server listened on both addresses); on "Tailscale", the tailnet alone.
//! Connections from anywhere else (the internet, a carrier's shared address space) are closed
//! before TLS. This Mac itself is always let in.

// `PhoneLink` is for Settings › Phone, which is yet to show it.
#![cfg_attr(not(test), allow(dead_code))]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use trek_core::settings::Reach;

/// Which network a connection came over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// This Mac (loopback).
    ThisMac,
    /// The local network: a private or link-local address, or IPv6 on the same /64.
    Wifi,
    /// Tailscale: from a tailnet address to this Mac's own.
    Tailnet,
    /// Anywhere else.
    Elsewhere,
}

/// What decides which connections the server takes: the setting, and this Mac's tailnet address
/// as last found (it can come and go while the server runs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkPolicy {
    pub reach: Reach,
    pub tailscale: Option<Ipv4Addr>,
}

impl LinkPolicy {
    /// Whether to serve a connection from `peer` that reached this Mac at `local`.
    pub fn admits(&self, peer: IpAddr, local: IpAddr) -> bool {
        match via(peer, local, self.tailscale) {
            Via::ThisMac | Via::Tailnet => true,
            Via::Wifi => self.reach == Reach::Wifi,
            Via::Elsewhere => false,
        }
    }
}

/// Which network a connection from `peer` to this Mac's `local` came over. `tailscale` is this
/// Mac's tailnet address: a 100.64/10 address alone is also carrier-grade NAT (hotspots, some
/// ISPs), shared with strangers, so only one that reached Tailscale's own address counts.
pub fn via(peer: IpAddr, local: IpAddr, tailscale: Option<Ipv4Addr>) -> Via {
    let (peer, local) = (peer.to_canonical(), local.to_canonical());
    if peer.is_loopback() {
        return Via::ThisMac;
    }
    let local_tailnet = match local {
        IpAddr::V4(v4) => Some(v4) == tailscale,
        IpAddr::V6(v6) => tailnet_v6(v6),
    };
    if local_tailnet {
        let peer_tailnet = match peer {
            IpAddr::V4(v4) => shared_v4(v4),
            IpAddr::V6(v6) => tailnet_v6(v6),
        };
        return if peer_tailnet { Via::Tailnet } else { Via::Elsewhere };
    }
    let local_network = match (peer, local) {
        (IpAddr::V4(p), _) => p.is_private() || p.is_link_local(),
        (IpAddr::V6(p), IpAddr::V6(l)) => {
            let seg = p.segments();
            let link_local = seg[0] & 0xffc0 == 0xfe80;
            let unique_local = seg[0] & 0xfe00 == 0xfc00 && !tailnet_v6(p);
            let same_link = u128::from(p) >> 64 == u128::from(l) >> 64;
            link_local || unique_local || same_link
        }
        (IpAddr::V6(p), IpAddr::V4(_)) => {
            let seg = p.segments();
            seg[0] & 0xffc0 == 0xfe80 || (seg[0] & 0xfe00 == 0xfc00 && !tailnet_v6(p))
        }
    };
    if local_network { Via::Wifi } else { Via::Elsewhere }
}

/// 100.64.0.0/10: Tailscale's addresses, and carrier-grade NAT's.
fn shared_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (64..128).contains(&b)
}

/// fd7a:115c:a1e0::/48: Tailscale's IPv6 addresses.
fn tailnet_v6(ip: Ipv6Addr) -> bool {
    let seg = ip.segments();
    seg[0] == 0xfd7a && seg[1] == 0x115c && seg[2] == 0xa1e0
}

/// The phone link as it stands, for Settings › Phone ([`crate::workspace::Workspace::phone_link`]).
#[derive(Debug, Clone, PartialEq)]
pub struct PhoneLink {
    /// The sockets the server listens on: `[::]:7420` is every interface, IPv4 too.
    pub listening: Vec<SocketAddr>,
    /// What phones are told to dial (in the pairing code and every `welcome`), the first first.
    pub routes: Vec<String>,
    /// The name Bonjour announces this Mac under on the Wi-Fi; `None` when it doesn't (the
    /// "Tailscale" setting, no Wi-Fi, or the responder refused).
    pub advertised_name: Option<String>,
    /// Which connections it takes.
    pub policy: LinkPolicy,
    /// The last phone (or Mac) that connected since the server started.
    pub last_connection: Option<LastConnection>,
}

/// The last phone that connected.
#[derive(Debug, Clone, PartialEq)]
pub struct LastConnection {
    pub device_id: String,
    /// Its name, when it's still paired.
    pub device_name: Option<String>,
    /// Unix ms.
    pub at: i64,
    /// The address of this Mac it reached (`192.168.1.2:7420`): the route it dialled.
    pub route: String,
    pub via: Via,
}
