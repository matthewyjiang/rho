//! Shared HTTP server security primitives used by `rho mcp serve --http` and
//! `rho daemon serve --http`: private-network bind checks, `Host` and `Origin`
//! validation, bearer token extraction, and token secret/hash helpers.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use hyper::{header, Request, StatusCode};
use sha2::{Digest, Sha256};

/// The `Host` names every local HTTP server accepts by default.
pub(crate) const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// Random bytes in a generated bearer token secret (256 bits).
const SECRET_BYTES: usize = 32;

/// Whether `ip` is loopback or on a private network (RFC 1918, RFC 6598
/// shared space used by Tailscale/NetBird, or IPv6 ULA). Link-local and
/// unspecified addresses are excluded.
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

/// The `Host` names requests to private address `ip` carry, which are allowed
/// without `--allow-host`: none for loopback, which is always allowed. An
/// IPv6 address can be written two ways, as Rust prints it and as a URL does,
/// which writes a mapped IPv4 address in hex; both are allowed.
pub(crate) fn own_hosts(ip: IpAddr) -> Vec<String> {
    if ip.is_loopback() {
        return Vec::new();
    }
    let mut hosts = vec![ip.to_string()];
    if let IpAddr::V6(v6) = ip {
        let url_form = url::Host::<String>::Ipv6(v6).to_string();
        let url_form = url_form.trim_start_matches('[').trim_end_matches(']');
        if url_form != hosts[0] {
            hosts.push(url_form.to_owned());
        }
    }
    hosts
}

/// Extracts the token from an `Authorization: Bearer <token>` header.
pub(crate) fn bearer_token<B>(request: &Request<B>) -> Option<String> {
    let value = request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then(|| token.to_owned())
}

/// A request refused before authentication: its status and the reason the
/// response body gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) status: StatusCode,
    pub(crate) reason: &'static str,
}

/// One `Host` a server accepts: a name or IP, and the port it must carry,
/// if any. Matches rmcp's `--allow-host` semantics for `rho mcp serve`, so
/// one entry means the same thing to both HTTP servers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AllowedHost {
    /// Lowercase, without IPv6 brackets.
    host: String,
    port: Option<u16>,
}

impl AllowedHost {
    /// Parses `entry`, such as `localhost`, `rho.example.com:443`, `::1`, or
    /// `[::1]:8766`.
    pub(crate) fn parse(entry: &str) -> anyhow::Result<Self> {
        let (host, port) = parse_authority(entry.trim()).ok_or_else(|| {
            anyhow::anyhow!(
                "--allow-host {entry:?} is not a host name or IP with an optional port, such as rho.example.com or rho.example.com:443"
            )
        })?;
        Ok(Self {
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    fn matches(&self, host: &str, port: Option<u16>) -> bool {
        self.host.eq_ignore_ascii_case(host)
            && self.port.is_none_or(|allowed| port == Some(allowed))
    }
}

/// One browser `Origin` a server accepts, in its canonical ASCII form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AllowedOrigin(String);

impl AllowedOrigin {
    /// Parses `entry`, such as `https://app.example.com`.
    pub(crate) fn parse(entry: &str) -> anyhow::Result<Self> {
        canonical_origin(entry).map(Self).ok_or_else(|| {
            anyhow::anyhow!(
                "--allow-origin {entry:?} is not an http or https origin, such as https://app.example.com"
            )
        })
    }
}

/// Validates the HTTP `Host` header against `allowed_hosts` (DNS rebinding
/// protection): `400 Bad Request` for a missing or malformed `Host`, and
/// `403 Forbidden` for one not allowed.
pub(crate) fn check_host<B>(
    request: &Request<B>,
    allowed_hosts: &[AllowedHost],
) -> Result<(), Refusal> {
    let malformed = Refusal {
        status: StatusCode::BAD_REQUEST,
        reason: "missing or invalid Host header",
    };
    let raw = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(malformed)?;
    let (host, port) = parse_authority(raw).ok_or(malformed)?;
    if allowed_hosts
        .iter()
        .any(|allowed| allowed.matches(host, port))
    {
        Ok(())
    } else {
        Err(Refusal {
            status: StatusCode::FORBIDDEN,
            reason: "Host header is not allowed",
        })
    }
}

/// Validates the HTTP `Origin` header against `allowed_origins` (CSWSH / CSRF
/// protection). Requests without an `Origin` header pass (non-browser clients).
/// Requests with an `Origin` header must match an entry in `allowed_origins`.
pub(crate) fn check_origin<B>(
    request: &Request<B>,
    allowed_origins: &[AllowedOrigin],
) -> Result<(), Refusal> {
    let Some(header_value) = request.headers().get(header::ORIGIN) else {
        return Ok(());
    };
    let presented = header_value.to_str().ok().and_then(canonical_origin);
    if presented
        .is_some_and(|presented| allowed_origins.iter().any(|allowed| allowed.0 == presented))
    {
        Ok(())
    } else {
        Err(Refusal {
            status: StatusCode::FORBIDDEN,
            reason: "Origin header is not allowed",
        })
    }
}

/// Splits an authority (`host`, `host:port`, `[v6]`, `[v6]:port`, or a bare
/// IPv6 address) into its host, without brackets, and port.
fn parse_authority(raw: &str) -> Option<(&str, Option<u16>)> {
    if let Some(rest) = raw.strip_prefix('[') {
        let (ipv6, tail) = rest.split_once(']')?;
        if ipv6.is_empty() {
            return None;
        }
        let port = match tail {
            "" => None,
            tail => Some(tail.strip_prefix(':')?.parse::<u16>().ok()?),
        };
        return Some((ipv6, port));
    }
    if raw.parse::<Ipv6Addr>().is_ok() {
        return Some((raw, None));
    }
    let (host, port) = match raw.split_once(':') {
        Some((host, port)) => (host, Some(port.parse::<u16>().ok()?)),
        None => (raw, None),
    };
    let plain = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
    plain.then_some((host, port))
}

fn canonical_origin(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    if parsed.path() != "/" && !parsed.path().is_empty() {
        return None;
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return None;
    }
    let origin = parsed.origin();
    if !origin.is_tuple() {
        return None;
    }
    Some(origin.ascii_serialization())
}

/// Refuses a `kind` name (a token's, a webhook's, a remote's) that refusals
/// and lists could not show plainly.
pub(crate) fn validate_name(kind: &str, name: &str) -> anyhow::Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        anyhow::bail!("{kind} name {name:?} must be 1 to 64 ASCII letters, digits, '-', or '_'");
    }
    Ok(())
}

/// Generates a random 256-bit bearer secret prefixed with `prefix`.
pub(crate) fn new_secret(prefix: &str) -> String {
    use rand::{rand_core::UnwrapErr, rngs::SysRng, Rng};
    let mut bytes = [0_u8; SECRET_BYTES];
    UnwrapErr(SysRng).fill_bytes(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

/// Lowercase hex SHA-256 of `secret`.
pub(crate) fn sha256_hex(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// The token in `tokens` whose secret `presented` is, if any, by comparing
/// `presented`'s hash against every token's, each in constant time, so the
/// time taken does not tell which matched.
pub(crate) fn find_by_secret<'a, T>(
    tokens: &'a [T],
    presented: &str,
    sha256: impl Fn(&T) -> &str,
) -> Option<&'a T> {
    let presented = sha256_hex(presented);
    tokens.iter().fold(None, |found, token| {
        if constant_time_eq(sha256(token).as_bytes(), presented.as_bytes()) {
            Some(token)
        } else {
            found
        }
    })
}

/// Constant-time byte-slice equality for equal-length digests.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

#[cfg(test)]
#[path = "http_guard_tests.rs"]
mod tests;
