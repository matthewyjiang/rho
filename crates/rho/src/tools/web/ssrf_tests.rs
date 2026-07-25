use super::*;

#[test]
fn blocks_loopback_link_local_private_and_reserved() {
    for literal in [
        "127.0.0.1",
        "169.254.169.254",
        "10.0.0.5",
        "172.16.0.1",
        "192.168.1.1",
        "0.0.0.0",
        "100.64.0.1",
        "198.18.0.1",
        "224.0.0.1",
        "::1",
        "::",
        "fd00::1",
        "fe80::1",
        "ff02::1",
        "::ffff:127.0.0.1",
        "2001:db8::1",
        "2001:2::1",
        "100::1",
    ] {
        let ip: IpAddr = literal.parse().unwrap();
        assert!(is_blocked(ip), "{literal} should be blocked");
    }
}

#[test]
fn blocks_ipv6_transition_addresses_that_embed_internal_ipv4() {
    for literal in [
        "64:ff9b::7f00:1",    // NAT64 well-known prefix to 127.0.0.1
        "64:ff9b::a9fe:a9fe", // NAT64 well-known prefix to 169.254.169.254
        "64:ff9b:1::1",       // local-use NAT64 prefix
        "2002:7f00:1::1",     // 6to4 to 127.0.0.1
        "2002:c0a8:1::1",     // 6to4 to 192.168.0.1
        "::127.0.0.1",        // deprecated IPv4-compatible address
        "::169.254.169.254",  // deprecated IPv4-compatible address
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

#[tokio::test]
async fn rejects_private_ip_literals_and_localhost() {
    for url in [
        "http://127.0.0.1/x",
        "http://169.254.169.254/latest/meta-data/",
        "http://[::1]:9229/json",
        "http://192.168.0.1/admin",
        "http://localhost/",
        "http://app.localhost/health",
    ] {
        let error = ensure_public_url(url, &[]).await.unwrap_err();
        assert!(
            error.to_string().contains("blocked"),
            "{url} should be rejected, got {error}"
        );
    }
}

#[tokio::test]
async fn allows_public_ip_literals() {
    ensure_public_url("https://8.8.8.8/", &[])
        .await
        .expect("public literal");
}

#[tokio::test]
async fn allow_range_exempts_matching_addresses() {
    let range = Cidr::parse("198.18.0.0/15").unwrap();
    ensure_public_url("http://198.18.0.10/proxy", &[range])
        .await
        .expect("fake-IP range should be exempt");
    let error = ensure_public_url("http://10.0.0.1/", &[range])
        .await
        .unwrap_err();
    assert!(error.to_string().contains("blocked"));
}

#[test]
fn allow_range_matches_ipv4_mapped_forms() {
    let range = Cidr::parse("127.0.0.0/8").unwrap();
    let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
    assert!(range.contains(mapped));
}

#[test]
fn cidr_parse_rejects_open_and_malformed() {
    assert!(Cidr::parse("0.0.0.0/0").is_err());
    assert!(Cidr::parse("::/0").is_err());
    assert!(Cidr::parse("not-an-ip/24").is_err());
    assert!(Cidr::parse("10.0.0.0/").is_err());
    assert_eq!(
        Cidr::parse("198.18.0.0/15").unwrap(),
        Cidr {
            network: "198.18.0.0".parse().unwrap(),
            prefix: 15,
        }
    );
}
