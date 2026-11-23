use hyper::{header, Request, StatusCode};
use pretty_assertions::assert_eq;

use super::{bearer_token, check_host, check_origin, is_private, own_hosts};

// Covers: private-network bind policy accepts loopback, RFC 1918, RFC 6598
// (Tailscale/NetBird), and IPv6 ULA, and refuses public, link-local, and
// wildcard addresses.
// Owner: shared HTTP guard.
#[test]
fn private_addresses_are_the_ranges_the_public_internet_does_not_route() {
    let cases = [
        ("127.0.0.1", true),
        ("10.1.2.3", true),
        ("172.16.0.1", true),
        ("172.32.0.1", false),
        ("192.168.1.10", true),
        // NetBird and Tailscale peers.
        ("100.64.0.1", true),
        ("100.127.255.254", true),
        ("100.128.0.1", false),
        ("169.254.1.1", false),
        ("0.0.0.0", false),
        ("8.8.8.8", false),
        ("::1", true),
        ("fd7a:115c:a1e0::1", true),
        ("fe80::1", false),
        ("::", false),
        ("2001:db8::1", false),
        ("::ffff:192.168.1.10", true),
        ("::ffff:8.8.8.8", false),
    ];
    let actual: Vec<_> = cases
        .iter()
        .map(|(ip, _)| (*ip, is_private(ip.parse().unwrap())))
        .collect();
    assert_eq!(actual, cases.to_vec());
}

// Covers: a server on a private address accepts requests naming it in either
// Rust or URL IPv6 form, while loopback needs no extra entry.
// Owner: shared HTTP guard.
#[test]
fn a_private_address_is_its_own_host_in_either_form() {
    let cases = [
        ("127.0.0.1", Vec::<&str>::new()),
        ("192.168.1.10", vec!["192.168.1.10"]),
        ("fd7a:115c:a1e0::1", vec!["fd7a:115c:a1e0::1"]),
        (
            "::ffff:192.168.1.10",
            vec!["::ffff:192.168.1.10", "::ffff:c0a8:10a"],
        ),
    ];
    for (ip, hosts) in cases {
        assert_eq!(own_hosts(ip.parse().unwrap()), hosts, "{ip}");
    }
}

// Covers: Host header validation rejects missing/malformed headers (400) and
// DNS-rebinding hosts (403), while accepting loopback, IPv6 brackets, ports,
// and explicitly allowed tunnel hostnames.
// Owner: shared HTTP guard.
#[test]
fn host_validation_blocks_rebinding_and_accepts_allowed_hosts() {
    let allowed: Vec<String> = ["localhost", "127.0.0.1", "::1", "rho.example.com"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let cases = [
        (None, Err(StatusCode::BAD_REQUEST)),
        (Some(""), Err(StatusCode::BAD_REQUEST)),
        (Some("localhost:notaport"), Err(StatusCode::BAD_REQUEST)),
        (Some("localhost:8766"), Ok(())),
        (Some("LOCALHOST"), Ok(())),
        (Some("127.0.0.1:8766"), Ok(())),
        (Some("[::1]:8766"), Ok(())),
        (Some("[::1]"), Ok(())),
        (Some("rho.example.com:443"), Ok(())),
        (Some("evil.example.com:8766"), Err(StatusCode::FORBIDDEN)),
    ];
    for (host, expected) in cases {
        let mut builder = Request::builder().uri("/ws");
        if let Some(host) = host {
            builder = builder.header(header::HOST, host);
        }
        let request = builder.body(()).unwrap();
        assert_eq!(check_host(&request, &allowed), expected, "{host:?}");
    }
}

// Covers: Origin validation allows non-browser requests (no Origin), rejects
// browser requests unless explicitly allowlisted, and normalizes case/default
// ports.
// Owner: shared HTTP guard.
#[test]
fn origin_validation_blocks_unlisted_browser_origins() {
    let allowed = vec!["https://App.Example.com:443".to_owned()];
    let cases = [
        (None, Ok(())),
        (Some("https://app.example.com"), Ok(())),
        (Some("https://app.example.com:443"), Ok(())),
        (Some("http://app.example.com"), Err(StatusCode::FORBIDDEN)),
        (Some("https://evil.example.com"), Err(StatusCode::FORBIDDEN)),
        (Some("null"), Err(StatusCode::FORBIDDEN)),
        (
            Some("https://app.example.com/path"),
            Err(StatusCode::FORBIDDEN),
        ),
    ];
    for (origin, expected) in cases {
        let mut builder = Request::builder().uri("/ws");
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        let request = builder.body(()).unwrap();
        assert_eq!(check_origin(&request, &allowed), expected, "{origin:?}");
    }
}

// Covers: bearer token extraction accepts case-insensitive scheme and rejects
// missing, empty, or non-Bearer authorization headers.
// Owner: shared HTTP guard.
#[test]
fn bearer_token_extracts_nonempty_bearer_only() {
    let cases = [
        (None, None),
        (Some("Basic abc"), None),
        (Some("Bearer "), None),
        (Some("Bearer secret-1"), Some("secret-1")),
        (Some("bearer  secret-2 "), Some("secret-2")),
    ];
    for (header_value, expected) in cases {
        let mut builder = Request::builder().uri("/ws");
        if let Some(value) = header_value {
            builder = builder.header(header::AUTHORIZATION, value);
        }
        let request = builder.body(()).unwrap();
        assert_eq!(bearer_token(&request).as_deref(), expected);
    }
}
