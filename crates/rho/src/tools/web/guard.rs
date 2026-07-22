use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::Url;

use rho_tools::tool::ToolError;

const ALLOW_ENV: &str = "RHO_ALLOW_PRIVATE_NETWORK";

/// Whether the fetch tools may reach private, loopback, or link-local network
/// destinations. The default is [`NetworkAccess::PublicOnly`] so a model-chosen
/// URL cannot make Rho request an internal service (SSRF).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tools) enum NetworkAccess {
    PublicOnly,
    AllowPrivate,
}

impl NetworkAccess {
    /// Reads the opt-out from the environment once, at tool construction. Absent
    /// or falsey keeps the secure `PublicOnly` default.
    pub(super) fn from_env() -> Self {
        match std::env::var(ALLOW_ENV) {
            Ok(value) if is_truthy(&value) => Self::AllowPrivate,
            _ => Self::PublicOnly,
        }
    }

    fn allows_private(self) -> bool {
        matches!(self, Self::AllowPrivate)
    }
}

fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Rejects a URL whose host is a literal private/loopback IP address. Hostnames
/// are validated at connection time by [`PrivateNetworkResolver`], which also
/// closes the DNS-rebinding gap; this pre-check only covers the literal-IP case
/// that never reaches the resolver.
pub(super) fn ensure_allowed_url(url: &str, access: NetworkAccess) -> Result<(), ToolError> {
    if access.allows_private() {
        return Ok(());
    }
    let parsed =
        Url::parse(url).map_err(|error| ToolError::Message(format!("invalid url: {error}")))?;
    let ip = match parsed.host() {
        Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip),
        Some(url::Host::Ipv6(ip)) => IpAddr::V6(ip),
        _ => return Ok(()),
    };
    match blocked_reason(ip) {
        Some(reason) => Err(ToolError::Message(reason)),
        None => Ok(()),
    }
}

/// Installs the private-address-blocking resolver on `builder` when access is
/// restricted; otherwise leaves reqwest's default resolver in place.
pub(super) fn apply(
    builder: reqwest::ClientBuilder,
    access: NetworkAccess,
) -> reqwest::ClientBuilder {
    match access {
        NetworkAccess::PublicOnly => builder.dns_resolver(Arc::new(PrivateNetworkResolver)),
        NetworkAccess::AllowPrivate => builder,
    }
}

/// Resolves hostnames and rejects the resolution if any address is private, so a
/// name that resolves to an internal IP (including a rebinding attempt between
/// check and connect) never connects.
struct PrivateNetworkResolver;

impl Resolve for PrivateNetworkResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            for addr in &addrs {
                if let Some(reason) = blocked_reason(addr.ip()) {
                    return Err(reason.into());
                }
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

fn blocked_reason(ip: IpAddr) -> Option<String> {
    is_blocked(ip).then(|| {
        format!("refusing to fetch private or local address {ip}; set {ALLOW_ENV}=1 to allow")
    })
}

fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] == 0
                || is_shared_v4(v4)
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(mapped) => is_blocked(IpAddr::V4(mapped)),
            None => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || is_unique_local_v6(v6)
                    || is_link_local_v6(v6)
            }
        },
    }
}

/// `100.64.0.0/10`, carrier-grade NAT shared address space.
fn is_shared_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (0x40..=0x7f).contains(&b)
}

/// `fc00::/7`, IPv6 unique local addresses.
fn is_unique_local_v6(ip: Ipv6Addr) -> bool {
    (ip.octets()[0] & 0xfe) == 0xfc
}

/// `fe80::/10`, IPv6 link-local unicast.
fn is_link_local_v6(ip: Ipv6Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 0xfe && (b & 0xc0) == 0x80
}

#[cfg(test)]
#[path = "guard_tests.rs"]
mod tests;
