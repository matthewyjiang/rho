use super::*;
use pretty_assertions::assert_eq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[test]
fn oauth_debug_redacts_codes_and_pkce_secrets() {
    let request = AnthropicOAuthRequest {
        authorize_url: "https://example.test?state=oauth-state-secret".into(),
        redirect_uri: REDIRECT_URI.into(),
        state: "oauth-state-secret".into(),
        verifier: "pkce-verifier-secret".into(),
    };
    let debug = format!("{request:?}");
    assert!(!debug.contains("oauth-state-secret"));
    assert!(!debug.contains("pkce-verifier-secret"));
}

#[test]
fn authorization_url_uses_console_callback_and_pkce() {
    let request = build_oauth_request_with_values("state".into(), "verifier".into());
    let url = Url::parse(&request.authorize_url).unwrap();
    let query = url
        .query_pairs()
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();

    assert_eq!(url.as_str().split('?').next().unwrap(), AUTHORIZE_URL);
    assert_eq!(query.get("client_id").unwrap(), CLIENT_ID);
    assert_eq!(query.get("redirect_uri").unwrap(), REDIRECT_URI);
    assert_eq!(query.get("scope").unwrap(), SCOPE);
    assert_eq!(query.get("code").unwrap(), "true");
    assert_eq!(query.get("response_type").unwrap(), "code");
    assert_eq!(query.get("code_challenge_method").unwrap(), "S256");
    assert_eq!(query.get("state").unwrap(), "state");
    assert!(query.contains_key("code_challenge"));
}

#[test]
fn parse_authorization_code_splits_code_hash_state() {
    let parsed = parse_authorization_code("  auth-code#returned-state  ").unwrap();
    assert_eq!(parsed.code, "auth-code");
    assert_eq!(parsed.state.as_deref(), Some("returned-state"));
    assert!(parse_authorization_code("   ").is_none());
}

#[tokio::test]
async fn token_exchange_posts_json_with_pkce() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            request.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&request);
            let Some((headers, received_body)) = text.split_once("\r\n\r\n") else {
                continue;
            };
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if received_body.len() >= content_length {
                break;
            }
        }
        let request = String::from_utf8(request).unwrap();
        let body = r#"{"access_token":"access","refresh_token":"refresh","expires_in":3600}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        request
    });

    let request = build_oauth_request_with_values("state".into(), "verifier".into());
    let tokens = complete_anthropic_oauth_with_endpoint(
        &reqwest::Client::new(),
        request,
        "auth-code#state",
        &endpoint,
    )
    .await
    .unwrap();
    let posted = handle.await.unwrap();

    assert_eq!(tokens.access_token, "access");
    assert_eq!(tokens.refresh_token.as_deref(), Some("refresh"));
    assert!(posted.contains("\"grant_type\":\"authorization_code\""));
    assert!(posted.contains("\"code\":\"auth-code\""));
    assert!(posted.contains("\"code_verifier\":\"verifier\""));
    assert!(!posted.contains("pkce-verifier-secret"));
}
