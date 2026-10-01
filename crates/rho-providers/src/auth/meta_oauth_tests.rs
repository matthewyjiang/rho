use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

// Covers: device authorization posts the Muse client id and keeps the user code
// Owner: Meta subscription login
#[tokio::test]
async fn device_authorization_posts_client_id_and_parses_user_code() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/oidc/device/authorization/",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let read = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..read]);
        assert!(request.starts_with("POST /oidc/device/authorization/ HTTP/1.1"));
        assert!(request.contains("application/x-www-form-urlencoded"));
        assert!(request.contains(&format!("client_id={CLIENT_ID}")));
        let body = r#"{"device_code":"device-secret","user_code":"ABCD-1234","verification_uri":"https://auth.meta.com/oauth/device","verification_uri_complete":"https://auth.meta.com/oauth/device/?code=ABCD-1234","expires_in":300,"interval":5}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let login = start_with_endpoint(&crate::reqwest_client(), &endpoint)
        .await
        .unwrap();
    assert_eq!(login.user_code, "ABCD-1234");
    assert_eq!(login.verification_uri, "https://auth.meta.com/oauth/device");
    assert_eq!(
        login.verification_uri_complete.as_deref(),
        Some("https://auth.meta.com/oauth/device/?code=ABCD-1234")
    );
    server.await.unwrap();
}

// Covers: a Muse identity token is exchanged for the Model API key Rho will send
// Owner: Meta subscription login
#[tokio::test]
async fn mint_posts_identity_token_and_returns_api_key() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/muse-code/key", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let read = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..read]);
        assert!(request.starts_with("POST /muse-code/key HTTP/1.1"));
        assert!(request.contains("authorization: Bearer identity-token"));
        assert!(request.contains("x-api-version: 1.0.0"));
        assert!(request.contains("\r\n\r\n{}"));
        let body = r#"{"api_key":"LLM|minted"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let tokens = mint_meta_tokens(&crate::reqwest_client(), "identity-token", &endpoint)
        .await
        .unwrap();
    assert_eq!(tokens.identity_token, "identity-token");
    assert_eq!(tokens.api_key, "LLM|minted");
    assert!(tokens.api_key_expires_at_unix > now_unix());
    server.await.unwrap();
}

// Covers: a mint that needs billing setup surfaces the URL instead of an empty key
// Owner: Meta subscription login
#[tokio::test]
async fn mint_without_key_reports_setup_url() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/muse-code/key", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 1024];
        let _ = stream.read(&mut request).await.unwrap();
        let body = r#"{"action_url":"https://accountscenter.meta.com/subscriptions"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let error = mint_meta_tokens(&crate::reqwest_client(), "identity-token", &endpoint)
        .await
        .unwrap_err();
    assert!(matches!(error, MetaOAuthError::Setup(_)));
    assert!(error
        .to_string()
        .contains("https://accountscenter.meta.com/subscriptions"));
    server.await.unwrap();
}

// Covers: an expired identity session tells the user to sign in again
// Owner: Meta subscription login
#[tokio::test]
async fn mint_unauthorized_asks_for_login() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/muse-code/key", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 1024];
        let _ = stream.read(&mut request).await.unwrap();
        let body = r#"{"error":"invalid_token"}"#;
        let response = format!(
            "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let error = mint_meta_tokens(&crate::reqwest_client(), "identity-token", &endpoint)
        .await
        .unwrap_err();
    let MetaOAuthError::Unauthorized(message) = error else {
        panic!("expected unauthorized, got {error}");
    };
    assert!(message.contains("meta-muse"), "{message}");
    assert!(message.contains("invalid_token"), "{message}");
    server.await.unwrap();
}
