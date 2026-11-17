use pretty_assertions::assert_eq;

// Covers: a server on a private address accepts requests that name it as
// their `Host` however a client writes it, an IPv4-mapped IPv6 address in
// a URL's hex form included; loopback needs no entry of its own.
// Owner: MCP HTTP server host allowlist.
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
        assert_eq!(super::own_hosts(ip.parse().unwrap()), hosts, "{ip}");
    }
}
