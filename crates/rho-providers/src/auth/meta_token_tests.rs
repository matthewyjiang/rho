use std::sync::Arc;

use super::*;
use crate::credentials::MemoryCredentialStore;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

// Covers: a minted key is replaced only inside the refresh margin, not halfway through its day
// Owner: Meta subscription token refresh
#[test]
fn api_key_is_expiring_only_inside_the_refresh_margin() {
    let now = 1_000_000_i64;
    let cases = [
        (now + REFRESH_MARGIN_SECONDS, true),
        (now + REFRESH_MARGIN_SECONDS + 1, false),
        (now - 1, true),
    ];
    for (expires_at, expected) in cases {
        let tokens = MetaTokens {
            identity_token: "identity".into(),
            api_key: "LLM|old".into(),
            api_key_expires_at_unix: expires_at,
        };
        assert_eq!(
            meta_api_key_is_expiring(&tokens, now),
            expected,
            "expires_at={expires_at}"
        );
    }
}

// Covers: the next request after expiry mints a new key and stores it
// Owner: Meta subscription token refresh
#[tokio::test]
async fn expired_api_key_is_reminted_before_use() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/muse-code/key", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let read = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..read]);
        assert!(request.contains("authorization: Bearer identity-token"));
        let body = r#"{"api_key":"LLM|fresh"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let store = Arc::new(MemoryCredentialStore::default());
    let manager = MetaAuthManager::from_tokens_at(
        store.clone(),
        MetaTokens {
            identity_token: "identity-token".into(),
            api_key: "LLM|old".into(),
            api_key_expires_at_unix: now_unix() - 1,
        },
        endpoint,
    );

    let api_key = manager.access_token().await.unwrap();
    assert_eq!(api_key, "LLM|fresh");
    assert_eq!(
        crate::credentials::load_meta_tokens(store.as_ref())
            .unwrap()
            .unwrap()
            .api_key,
        "LLM|fresh"
    );
    server.await.unwrap();
}

// Covers: a rejected identity token stays the session-expired error, not a missing-login error
// Owner: Meta subscription token refresh
#[tokio::test]
async fn meta_rejected_identity_keeps_the_session_expired_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/muse-code/key", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let _ = stream.read(&mut request).await.unwrap();
        let body = r#"{"detail":"invalid_token"}"#;
        let response = format!(
            "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let store = Arc::new(MemoryCredentialStore::default());
    let tokens = MetaTokens {
        identity_token: "identity-token".into(),
        api_key: "LLM|old".into(),
        api_key_expires_at_unix: now_unix() - 1,
    };
    crate::credentials::save_meta_tokens(store.as_ref(), &tokens).unwrap();
    let manager = MetaAuthManager::from_tokens_at(store.clone(), tokens, endpoint);

    let error = manager.access_token().await.unwrap_err();
    pretty_assertions::assert_eq!(error.to_string(), SESSION_EXPIRED);
    pretty_assertions::assert_eq!(
        crate::credentials::load_meta_tokens(store.as_ref())
            .unwrap()
            .unwrap()
            .api_key,
        "LLM|old"
    );
    server.await.unwrap();
}
