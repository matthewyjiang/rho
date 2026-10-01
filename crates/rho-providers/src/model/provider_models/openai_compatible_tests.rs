use reqwest::Url;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::fetch_with_mint;
use crate::{
    credentials::{load_meta_tokens, save_meta_tokens, MemoryCredentialStore, MetaTokens},
    model::{provider_models::ProviderModel, ModelError, ReasoningCapabilities},
    provider,
};

// Covers: model discovery remints once after HTTP 401 even when the key is not near expiry
// Owner: Meta model discovery
#[tokio::test]
async fn meta_model_discovery_remints_once_after_unauthorized() {
    let models = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let models_addr = models.local_addr().unwrap();
    let mint = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mint_addr = mint.local_addr().unwrap();

    let models_server = tokio::spawn(async move {
        for (expected, status, body) in [
            ("llm|old", "401 Unauthorized", r#"{"detail":"expired"}"#),
            (
                "llm|fresh",
                "200 OK",
                r#"{"data":[{"id":"muse-spark-1.2"}]}"#,
            ),
        ] {
            let (mut stream, _) = models.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let read = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            assert!(
                request.contains(&format!("authorization: bearer {expected}")),
                "{request}"
            );
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });

    let mint_server = tokio::spawn(async move {
        let (mut stream, _) = mint.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let read = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
        assert!(
            request.contains("authorization: bearer identity-token"),
            "{request}"
        );
        let body = r#"{"api_key":"LLM|fresh"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let store = MemoryCredentialStore::default();
    save_meta_tokens(
        &store,
        &MetaTokens {
            identity_token: "identity-token".into(),
            api_key: "LLM|old".into(),
            api_key_expires_at_unix: crate::auth::meta_oauth::now_unix() + 10_000,
        },
    )
    .unwrap();
    let descriptor = provider::provider_descriptor("meta").unwrap();
    let auth = descriptor.auth_mode("meta-muse").unwrap();

    let models = fetch_with_mint(
        descriptor,
        auth,
        &Url::parse(&format!("http://{models_addr}")).unwrap(),
        &store,
        &format!("http://{mint_addr}/muse-code/key"),
        None,
    )
    .await
    .unwrap();

    pretty_assertions::assert_eq!(
        models,
        vec![ProviderModel {
            provider: "meta".into(),
            model: "muse-spark-1.2".into(),
            display_name: "muse-spark-1.2".into(),
            context_window: None,
            max_output_tokens: None,
            reasoning_capabilities: ReasoningCapabilities::Unknown,
        }]
    );
    pretty_assertions::assert_eq!(
        load_meta_tokens(&store).unwrap().unwrap().api_key,
        "LLM|fresh"
    );
    models_server.await.unwrap();
    mint_server.await.unwrap();
}

// Covers: META_API_KEY is a static key, so a 401 does not remint or overwrite the session
// Owner: Meta model discovery
#[tokio::test]
async fn meta_env_key_does_not_remint_on_unauthorized() {
    let models = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let models_addr = models.local_addr().unwrap();
    let models_server = tokio::spawn(async move {
        let (mut stream, _) = models.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let read = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
        assert!(
            request.contains("authorization: bearer llm|static"),
            "{request}"
        );
        let body = r#"{"detail":"nope"}"#;
        let response = format!(
            "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let store = MemoryCredentialStore::default();
    save_meta_tokens(
        &store,
        &MetaTokens {
            identity_token: "identity-token".into(),
            api_key: "LLM|stored".into(),
            api_key_expires_at_unix: crate::auth::meta_oauth::now_unix() + 10_000,
        },
    )
    .unwrap();
    let descriptor = provider::provider_descriptor("meta").unwrap();
    let auth = descriptor.auth_mode("meta-muse").unwrap();

    let error = fetch_with_mint(
        descriptor,
        auth,
        &Url::parse(&format!("http://{models_addr}")).unwrap(),
        &store,
        "http://127.0.0.1:9/muse-code/key",
        Some("LLM|static".into()),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, ModelError::HttpStatus { status, .. } if status == reqwest::StatusCode::UNAUTHORIZED),
        "{error}"
    );
    pretty_assertions::assert_eq!(
        load_meta_tokens(&store).unwrap().unwrap().api_key,
        "LLM|stored"
    );
    models_server.await.unwrap();
}
