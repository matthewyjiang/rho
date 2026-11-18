use pretty_assertions::assert_eq;

use super::*;

/// What [`check_url`] made of a URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Checked {
    Refused,
    Accepted,
    /// Accepted, its host name pinned to the private addresses it resolved to.
    Pinned,
}

// Covers: a hub reaches a remote over TLS, or over plain http only on a
// private network, with a host name pinned to the private addresses it
// resolved to; and never with userinfo, which `list` would show, nor echoes
// a URL it cannot parse, which may carry a secret.
// Owner: MCP server remotes.
#[tokio::test]
async fn remote_urls_need_tls_off_a_private_network_and_no_userinfo() {
    let cases = [
        ("https://lab-box.example.com/mcp", Checked::Accepted),
        ("http://127.0.0.1:8765/mcp", Checked::Accepted),
        ("http://localhost:8765/mcp", Checked::Pinned),
        ("http://100.92.10.4:8765/mcp", Checked::Accepted),
        ("http://192.168.1.20:8765/mcp", Checked::Accepted),
        ("http://[fd7a:115c:a1e0::1]:8765/mcp", Checked::Accepted),
        ("http://8.8.8.8:8765/mcp", Checked::Refused),
        ("http://169.254.1.1:8765/mcp", Checked::Refused),
        (
            "https://user:pass@lab-box.example.com/mcp",
            Checked::Refused,
        ),
        ("ftp://lab-box.example.com/mcp", Checked::Refused),
        ("not a url", Checked::Refused),
    ];
    let mut checked = Vec::new();
    for (url, _) in cases {
        let outcome = match check_url(url).await {
            Err(_) => Checked::Refused,
            Ok(endpoint) if endpoint.pinned.is_empty() => Checked::Accepted,
            Ok(endpoint) => {
                assert!(endpoint
                    .pinned
                    .iter()
                    .all(|address| address.ip().is_loopback()));
                Checked::Pinned
            }
        };
        checked.push((url, outcome));
    }
    let unparsable = check_url("https://user:hunter2@lab-box:port/mcp")
        .await
        .err()
        .map(|error| error.to_string().contains("hunter2"));

    assert_eq!((checked, unparsable), (cases.to_vec(), Some(false)));
}
