use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::Url;

use rho_tools::tool::ToolError;

tokio::task_local! {
    static ALLOW_RANGES_OVERRIDE: Vec<Cidr>;
}

/// Rejects remote fetch targets that resolve to private, loopback, link-local,
/// or other non-global addresses. Call this before the HTTP request. Content
/// fetches must also disable redirects (or re-check every hop); the shared web
/// clients use `redirect::Policy::none()`.
///
/// Same shape as pi-web-access: resolve the hostname, check every address, no
/// custom DNS resolver on the client. Optional CIDR allow-ranges cover TUN /
/// fake-IP proxy pools without opening all private space.
pub(super) async fn ensure_public_url(
    raw_url: &str,
    allow_ranges: &[Cidr],
) -> Result<(), ToolError> {
    let url =
        Url::parse(raw_url).map_err(|error| ToolError::Message(format!("invalid url: {error}")))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(ToolError::Message(
            "only http and https URLs are supported".into(),
        ));
    }

    let host = url
        .host_str()
        .ok_or_else(|| ToolError::Message("URL must include a hostname".into()))?;
    let hostname = normalize_hostname(host);
    if hostname.is_empty() {
        return Err(ToolError::Message("URL must include a hostname".into()));
    }
    if hostname == "localhost" || hostname.ends_with(".localhost") {
        return Err(ToolError::Message(format!(
            "blocked internal hostname: {hostname}"
        )));
    }

    if let Ok(ip) = hostname.parse::<IpAddr>() {
        return assert_public_address(ip, &hostname, allow_ranges);
    }

    let addrs = tokio::net::lookup_host((hostname.as_str(), 0))
        .await
        .map_err(|error| ToolError::Message(format!("failed to resolve {hostname}: {error}")))?
        .map(|addr| addr.ip())
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        return Err(ToolError::Message(format!(
            "failed to resolve {hostname}: no addresses returned"
        )));
    }
    for ip in addrs {
        assert_public_address(ip, &hostname, allow_ranges)?;
    }
    Ok(())
}

/// Allow-ranges used by the content-fetch choke point.
///
/// Production reads `RHO_SSRF_ALLOW_RANGES`. Tests may override via
/// [`with_allow_ranges`] without threading policy through tool plan types.
pub(super) fn configured_allow_ranges() -> Result<Vec<Cidr>, ToolError> {
    if let Ok(ranges) = ALLOW_RANGES_OVERRIDE.try_with(Clone::clone) {
        return Ok(ranges);
    }
    allow_ranges_from_env()
}

/// Run `f` with an explicit allow-range list (tests only).
#[cfg(test)]
pub(super) async fn with_allow_ranges<F>(ranges: Vec<Cidr>, f: F) -> F::Output
where
    F: std::future::Future,
{
    ALLOW_RANGES_OVERRIDE.scope(ranges, f).await
}

fn assert_public_address(
    ip: IpAddr,
    hostname: &str,
    allow_ranges: &[Cidr],
) -> Result<(), ToolError> {
    let ip = normalize_ip(ip);
    if allow_ranges.iter().any(|range| range.contains(ip)) {
        return Ok(());
    }
    if is_blocked(ip) {
        return Err(ToolError::Message(format!(
            "blocked internal address for {hostname}: {ip}"
        )));
    }
    Ok(())
}

fn normalize_hostname(hostname: &str) -> String {
    hostname
        .trim()
        .trim_matches(|c| c == '[' || c == ']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        IpAddr::V4(_) => ip,
    }
}

fn is_blocked(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    }
}

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || a == 0
        || (a == 100 && (64..=127).contains(&b)) // 100.64.0.0/10 CGNAT
        || (a == 198 && (18..=19).contains(&b)) // 198.18.0.0/15 benchmark / fake-IP
        || a >= 224 // multicast and reserved
}

fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    ip.is_unspecified()
        || ip.is_loopback()
        || (segments[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
        || (segments[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        || (segments[0] & 0xff00) == 0xff00 // ff00::/8 multicast
        || (segments[0] == 0x2001 && segments[1] == 0x0db8) // 2001:db8::/32 documentation
        || (segments[0] == 0x2001 && segments[1] == 0x0002 && segments[2] == 0) // 2001:2::/48 benchmarking
        || (segments[0] == 0x0100
            && segments[1] == 0
            && segments[2] == 0
            && segments[3] == 0) // 100::/64 discard-only
}

/// IPv4 or IPv6 CIDR used to exempt addresses from the private-range block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Cidr {
    network: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub(super) fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("empty CIDR".into());
        }
        let (addr_part, prefix_part) = match raw.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (raw, None),
        };
        if let Some(prefix) = prefix_part {
            if !prefix.chars().all(|c| c.is_ascii_digit()) {
                return Err(format!("invalid CIDR prefix in {raw:?}"));
            }
        }
        let network = normalize_ip(
            addr_part
                .parse()
                .map_err(|_| format!("invalid CIDR address in {raw:?}"))?,
        );
        let max_prefix = match network {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        let prefix = match prefix_part {
            Some(value) => value
                .parse::<u8>()
                .map_err(|_| format!("invalid CIDR prefix in {raw:?}"))?,
            None => max_prefix,
        };
        if prefix == 0 || prefix > max_prefix {
            // Reject /0 so a misconfig cannot open every address.
            return Err(format!("CIDR prefix out of range in {raw:?}"));
        }
        Ok(Self { network, prefix })
    }

    fn contains(self, ip: IpAddr) -> bool {
        let ip = normalize_ip(ip);
        match (self.network, ip) {
            (IpAddr::V4(network), IpAddr::V4(candidate)) => {
                prefix_match(&network.octets(), &candidate.octets(), self.prefix)
            }
            (IpAddr::V6(network), IpAddr::V6(candidate)) => {
                prefix_match(&network.octets(), &candidate.octets(), self.prefix)
            }
            _ => false,
        }
    }
}

fn prefix_match(network: &[u8], candidate: &[u8], prefix: u8) -> bool {
    let full_bytes = (prefix / 8) as usize;
    let rem_bits = prefix % 8;
    if network[..full_bytes] != candidate[..full_bytes] {
        return false;
    }
    if rem_bits == 0 {
        return true;
    }
    let mask = 0xffu8 << (8 - rem_bits);
    (network[full_bytes] & mask) == (candidate[full_bytes] & mask)
}

/// Comma-separated CIDRs from `RHO_SSRF_ALLOW_RANGES`, e.g. `198.18.0.0/15`.
/// Invalid entries fail closed so a misconfigured escape hatch is never silent.
pub(super) fn allow_ranges_from_env() -> Result<Vec<Cidr>, ToolError> {
    let Ok(raw) = std::env::var("RHO_SSRF_ALLOW_RANGES") else {
        return Ok(Vec::new());
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            Cidr::parse(part).map_err(|error| {
                ToolError::Message(format!(
                    "invalid RHO_SSRF_ALLOW_RANGES entry {part:?}: {error}"
                ))
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "ssrf_tests.rs"]
mod tests;
