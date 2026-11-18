//! Which addresses `rho mcp serve` treats as a private network, where plain
//! HTTP is the owner's call: a server may listen on one (`--http`), and a
//! hub may reach a remote at one without TLS (`rho mcp remote add`).
//!
//! Private means an address the public internet does not route: loopback,
//! the RFC 1918 ranges, the RFC 6598 shared range that overlay networks such
//! as Tailscale and NetBird hand their peers, and IPv6 unique local
//! addresses. Link-local and unspecified addresses are not: one is any
//! neighbor on the wire, the other every interface.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether `ip` is loopback or on a private network.
pub(crate) fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_private_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(mapped) => is_private_v4(mapped),
            None => ip.is_loopback() || is_unique_local(ip),
        },
    }
}

fn is_private_v4(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    // RFC 6598 shared address space, 100.64.0.0/10.
    let shared = first == 100 && (second & 0b1100_0000) == 64;
    ip.is_loopback() || ip.is_private() || shared
}

/// RFC 4193 unique local addresses, fc00::/7.
fn is_unique_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

#[cfg(test)]
#[path = "private_network_tests.rs"]
mod tests;
