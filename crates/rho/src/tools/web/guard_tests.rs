use super::*;

#[test]
fn blocks_loopback_link_local_and_private_ranges() {
    for literal in [
        "127.0.0.1",
        "169.254.169.254",
        "10.0.0.5",
        "172.16.0.1",
        "192.168.1.1",
        "0.0.0.0",
        "100.64.0.1",
        "::1",
        "::",
        "fd00::1",
        "fe80::1",
        "::ffff:127.0.0.1",
    ] {
        let ip: IpAddr = literal.parse().unwrap();
        assert!(is_blocked(ip), "{literal} should be blocked");
    }
}

#[test]
fn allows_globally_routable_addresses() {
    for literal in [
        "8.8.8.8",
        "1.1.1.1",
        "93.184.216.34",
        "2606:4700:4700::1111",
    ] {
        let ip: IpAddr = literal.parse().unwrap();
        assert!(!is_blocked(ip), "{literal} should be allowed");
    }
}

#[test]
fn pre_check_rejects_private_ip_literals() {
    for url in [
        "http://127.0.0.1/x",
        "http://169.254.169.254/latest/meta-data/",
        "http://[::1]:9229/json",
        "http://192.168.0.1/admin",
    ] {
        assert!(
            ensure_allowed_url(url, NetworkAccess::PublicOnly).is_err(),
            "{url} should be rejected"
        );
    }
}

#[test]
fn pre_check_allows_public_literals_and_defers_hostnames() {
    assert!(ensure_allowed_url("https://example.com/", NetworkAccess::PublicOnly).is_ok());
    assert!(ensure_allowed_url("https://8.8.8.8/", NetworkAccess::PublicOnly).is_ok());
    assert!(ensure_allowed_url("http://localhost/", NetworkAccess::PublicOnly).is_ok());
}

#[test]
fn escape_hatch_allows_private_literals() {
    assert!(ensure_allowed_url("http://127.0.0.1/", NetworkAccess::AllowPrivate).is_ok());
    assert!(ensure_allowed_url("http://169.254.169.254/", NetworkAccess::AllowPrivate).is_ok());
}
