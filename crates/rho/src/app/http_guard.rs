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

/// Validates the HTTP `Host` header against `allowed_hosts` (DNS rebinding
/// protection). Returns `400 Bad Request` for a missing or malformed `Host`
/// header, and `403 Forbidden` for an unallowed host.
pub(crate) fn check_host<B>(
    request: &Request<B>,
    allowed_hosts: &[String],
) -> Result<(), StatusCode> {
    let raw = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let host = parse_host_header(raw).ok_or(StatusCode::BAD_REQUEST)?;
    let matched = allowed_hosts
        .iter()
        .any(|allowed| normalize_host(allowed).eq_ignore_ascii_case(host));
    if matched {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

/// Validates the HTTP `Origin` header against `allowed_origins` (CSWSH / CSRF
/// protection). Requests without an `Origin` header pass (non-browser clients).
/// Requests with an `Origin` header must match an entry in `allowed_origins`.
pub(crate) fn check_origin<B>(
    request: &Request<B>,
    allowed_origins: &[String],
) -> Result<(), StatusCode> {
    let Some(header_value) = request.headers().get(header::ORIGIN) else {
        return Ok(());
    };
    let raw = header_value.to_str().map_err(|_| StatusCode::FORBIDDEN)?;
    let Some(presented) = canonical_origin(raw) else {
        return Err(StatusCode::FORBIDDEN);
    };
    let matched = allowed_origins
        .iter()
        .filter_map(|allowed| canonical_origin(allowed))
        .any(|allowed| allowed == presented);
    if matched {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

/// Extracts the bare host (without brackets or `:port`) from a `Host` header.
fn parse_host_header(raw: &str) -> Option<&str> {
    if let Some(rest) = raw.strip_prefix('[') {
        let (ipv6, tail) = rest.split_once(']')?;
        if ipv6.is_empty() {
            return None;
        }
        if !tail.is_empty() {
            let port = tail.strip_prefix(':')?;
            port.parse::<u16>().ok()?;
        }
        return Some(ipv6);
    }
    match raw.split_once(':') {
        Some((host, port)) => {
            if host.is_empty() || port.contains(':') {
                return None;
            }
            port.parse::<u16>().ok()?;
            Some(host)
        }
        None => Some(raw),
    }
}

fn normalize_host(host: &str) -> &str {
    let trimmed = host.trim();
    trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(trimmed)
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

/// Constant-time byte-slice equality for equal-length digests.
pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
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
