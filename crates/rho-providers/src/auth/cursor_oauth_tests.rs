use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn jwt_with_exp(exp: i64) -> String {
    let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
    format!("aaa.{payload}.sig")
}

// Covers: poll success must return access and refresh tokens without waiting on 404 backoff
// Owner: cursor oauth
#[tokio::test]
async fn poll_success_returns_tokens_on_first_ok_response() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/auth/poll", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let read = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..read]);
        assert!(request.starts_with("GET /auth/poll?"));
        assert!(request.contains("uuid=login-id"));
        assert!(request.contains("verifier=pkce-verifier"));
        let body = r#"{"accessToken":"access-1","refreshToken":"refresh-1"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let tokens = complete_with_endpoint(
        &reqwest::Client::new(),
        CursorOAuthLogin::for_test("login-id", "pkce-verifier"),
        &endpoint,
    )
    .await
    .unwrap();

    assert_eq!(tokens.access_token, "access-1");
    assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-1"));
    server.await.unwrap();
}

// Covers: token refresh must POST the refresh token as a bearer and keep it when omitted
// Owner: cursor oauth
#[tokio::test]
async fn refresh_posts_bearer_refresh_token_and_preserves_refresh_when_omitted() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/auth/exchange_user_api_key",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let read = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..read]);
        assert!(request.starts_with("POST /auth/exchange_user_api_key HTTP/1.1"));
        assert!(request.contains("authorization: Bearer old-refresh"));
        assert!(request.contains("{}"));
        let body = r#"{"accessToken":"new-access"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let tokens =
        refresh_cursor_tokens_with_endpoint(&reqwest::Client::new(), "old-refresh", &endpoint)
            .await
            .unwrap();

    assert_eq!(tokens.access_token, "new-access");
    assert_eq!(tokens.refresh_token.as_deref(), Some("old-refresh"));
    server.await.unwrap();
}

// Covers: JWT exp must be stored with a 5-minute skew so refresh happens before expiry
// Owner: cursor oauth
#[test]
fn token_expiry_uses_jwt_exp_minus_five_minutes() {
    let exp = 1_700_000_000;
    assert_eq!(token_expiry_unix(&jwt_with_exp(exp)), exp - 5 * 60);
}

#[test]
fn debug_redacts_pkce_verifier_and_uuid() {
    let login = CursorOAuthLogin::for_test("secret-uuid", "secret-verifier");
    let debug = format!("{login:?}");
    assert!(debug.contains("loginDeepControl"));
    assert!(!debug.contains("secret-uuid"));
    assert!(!debug.contains("secret-verifier"));
}
