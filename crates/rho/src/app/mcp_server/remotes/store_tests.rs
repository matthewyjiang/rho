use pretty_assertions::assert_eq;

use super::*;

// Covers: a hub reaches a remote over TLS, or over plain http only on a
// private network, and never with userinfo, which `list` would show.
// Owner: MCP server remotes.
#[tokio::test]
async fn remote_urls_need_tls_off_a_private_network_and_no_userinfo() {
    let cases = [
        ("https://lab-box.example.com/mcp", true),
        ("http://127.0.0.1:8765/mcp", true),
        ("http://localhost:8765/mcp", true),
        ("http://100.92.10.4:8765/mcp", true),
        ("http://192.168.1.20:8765/mcp", true),
        ("http://[fd7a:115c:a1e0::1]:8765/mcp", true),
        ("http://8.8.8.8:8765/mcp", false),
        ("http://169.254.1.1:8765/mcp", false),
        ("https://user:pass@lab-box.example.com/mcp", false),
        ("ftp://lab-box.example.com/mcp", false),
        ("not a url", false),
    ];
    let mut accepted = Vec::new();
    for (url, _) in cases {
        accepted.push((url, check_url(url).await.is_ok()));
    }
    assert_eq!(accepted, cases.to_vec());
}
