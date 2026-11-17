use pretty_assertions::assert_eq;

use super::is_private;

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
