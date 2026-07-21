use super::*;
use crate::credentials::{
    save_github_copilot_tokens, save_provider_api_key, GitHubCopilotTokens, MemoryCredentialStore,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[test]
fn openai_model_filter_keeps_chat_families() {
    assert!(is_supported_openai_model("gpt-5.5"));
    assert!(is_supported_openai_model("o3"));
    assert!(!is_supported_openai_model("text-embedding-3-large"));
    assert!(!is_supported_openai_model("whisper-1"));
}

#[test]
fn load_api_key_auth_reads_the_supplied_store() {
    let store = MemoryCredentialStore::default();
    save_provider_api_key(&store, "anthropic", "sk-ant-test").unwrap();

    assert_eq!(
        load_api_key_auth("anthropic", &store).unwrap(),
        "sk-ant-test"
    );
}

#[test]
fn parses_github_copilot_models_from_data_objects_and_deduplicates() {
    let value = serde_json::json!({
        "data": [
            {"id": "gpt-4.1"},
            {"name": "claude-sonnet-4"},
            {"id": "gpt-4.1"}
        ]
    });

    assert_eq!(
        parse_github_copilot_models("github-copilot", &value).unwrap(),
        vec![
            ProviderModel {
                provider: "github-copilot".into(),
                model: "claude-sonnet-4".into(),
                display_name: "claude-sonnet-4".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            },
            ProviderModel {
                provider: "github-copilot".into(),
                model: "gpt-4.1".into(),
                display_name: "gpt-4.1".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            },
        ]
    );
}

#[tokio::test]
async fn ollama_discovery_uses_resolved_v1_url_without_auth() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = Url::parse(&format!("http://{}/v1", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 2048];
        let bytes = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..bytes]);
        assert!(request.starts_with("GET /v1/models HTTP/1.1"));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        let body = r#"{"data":[{"id":"qwen3-coder"},{"id":"qwen3-coder"},{"id":"devstral","name":"Devstral"}]}"#;
        let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let descriptor = provider::provider_descriptor("ollama").unwrap();
    let cache = tempfile::tempdir().unwrap();
    set_provider_models_cache_dir_for_tests(Some(cache.path().to_path_buf()));
    let store = MemoryCredentialStore::default();

    let models = refresh_provider_models_with_store(
        descriptor.name,
        &store,
        ProviderModelEndpoint::OpenAiCompatible(&api_base),
    )
    .await
    .unwrap()
    .models;

    assert_eq!(
        models
            .iter()
            .map(|model| (model.model.as_str(), model.display_name.as_str()))
            .collect::<Vec<_>>(),
        vec![("devstral", "Devstral"), ("qwen3-coder", "qwen3-coder")]
    );
    assert_eq!(
        cached_provider_models("ollama"),
        models,
        "refresh should persist models for the generic picker path"
    );
    assert!(crate::model::catalog::available_models_for_auths(
        &crate::credentials::available_auth_modes(&store)
    )
    .iter()
    .any(|model| model.provider == "ollama" && model.model == "qwen3-coder"));
    set_provider_models_cache_dir_for_tests(None);
    server.await.unwrap();
}

async fn serve_models_response(status: &str, body: &'static str) -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = Url::parse(&format!("http://{}/v1", listener.local_addr().unwrap())).unwrap();
    let status = status.to_string();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 2048];
        let _ = stream.read(&mut request).await.unwrap();
        let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    api_base
}

#[tokio::test]
async fn ollama_probe_distinguishes_models_empty_invalid_and_unreachable() {
    let store = MemoryCredentialStore::default();
    let models_url = serve_models_response("200 OK", r#"{"data":[{"id":"model"}]}"#).await;
    assert_eq!(
        probe_provider_models("ollama", &models_url, &store).await,
        ProviderModelHealth::ReachableWithModels { model_count: 1 }
    );

    let empty_url = serve_models_response("200 OK", r#"{"data":[]}"#).await;
    assert_eq!(
        probe_provider_models("ollama", &empty_url, &store).await,
        ProviderModelHealth::ReachableWithoutModels
    );

    let invalid_url = serve_models_response("200 OK", r#"{"models":[]}"#).await;
    assert!(matches!(
        probe_provider_models("ollama", &invalid_url, &store).await,
        ProviderModelHealth::InvalidResponse { .. }
    ));

    let unsuccessful_url =
        serve_models_response("503 Service Unavailable", r#"{"error":"starting"}"#).await;
    assert!(matches!(
        probe_provider_models("ollama", &unsuccessful_url, &store).await,
        ProviderModelHealth::InvalidResponse { error } if error.contains("503")
    ));

    let oversized_body = Box::leak("x".repeat(32 * 1024).into_boxed_str());
    let oversized_url = serve_models_response("500 Internal Server Error", oversized_body).await;
    assert!(matches!(
        probe_provider_models("ollama", &oversized_url, &store).await,
        ProviderModelHealth::InvalidResponse { error }
            if error.contains("[response body truncated]") && error.len() < 20 * 1024
    ));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unreachable = Url::parse(&format!("http://{}/v1", listener.local_addr().unwrap())).unwrap();
    drop(listener);
    assert!(matches!(
        probe_provider_models("ollama", &unreachable, &store).await,
        ProviderModelHealth::Unreachable { .. }
    ));
}

#[tokio::test]
async fn openai_compatible_models_preserve_account_context_length() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 2048];
        let bytes = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..bytes]);
        assert!(request.starts_with("GET /models HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer moonshot-secret"));
        let body = r#"{"data":[{"id":"kimi-k3","name":"Kimi K3","context_length":1048576}]}"#;
        let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let store = MemoryCredentialStore::default();
    save_provider_api_key(&store, "moonshot", "moonshot-secret").unwrap();
    let descriptor = provider::provider_descriptor("moonshot").unwrap();

    let models = openai_compatible::fetch(descriptor, &api_base, &store)
        .await
        .unwrap();

    assert_eq!(
        models,
        vec![ProviderModel {
            provider: "moonshot".into(),
            model: "kimi-k3".into(),
            display_name: "Kimi K3".into(),
            context_window: Some(1_048_576),
            max_output_tokens: None,
            reasoning_capabilities: ReasoningCapabilities::Unknown,
        }]
    );
    server.await.unwrap();
}

#[tokio::test]
async fn github_copilot_models_retry_once_after_unauthorized() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let base_url_for_server = base_url.clone();
    tokio::spawn(async move {
        for index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 1024];
            let bytes = stream.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..bytes]);
            let is_model_request = request.contains("GET /models");
            let (status, body) = match (index, is_model_request) {
                    (0, true) => ("401 Unauthorized", String::new()),
                    (1, false) => (
                        "200 OK",
                        format!(
                            "{{\"token\":\"second\",\"endpoints\":{{\"api\":\"{base_url_for_server}\"}}}}"
                        ),
                    ),
                    (2, true) => (
                        "200 OK",
                        r#"{"data":[{"id":"gpt-4.1"}]}"#.to_string(),
                    ),
                    _ => ("500 Internal Server Error", String::new()),
                };
            let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(), body
                );
            stream.write_all(reply.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    let store = MemoryCredentialStore::default();
    save_github_copilot_tokens(
        &store,
        &GitHubCopilotTokens {
            github_access_token: "github".into(),
            github_refresh_token: None,
            github_expires_at_unix: None,
            copilot_token: Some("first".into()),
            copilot_expires_at_unix: Some(i64::MAX),
            copilot_refresh_after_unix: None,
            copilot_token_endpoint: Some(base_url.clone()),
            copilot_chat_endpoint: None,
            copilot_models_endpoint: Some(format!("{base_url}/models")),
        },
    )
    .unwrap();

    assert_eq!(
        fetch_github_copilot_models("github-copilot", &store)
            .await
            .unwrap(),
        vec![ProviderModel {
            provider: "github-copilot".into(),
            model: "gpt-4.1".into(),
            display_name: "gpt-4.1".into(),
            context_window: None,
            max_output_tokens: None,
            reasoning_capabilities: ReasoningCapabilities::Unknown,
        }]
    );
}

#[test]
fn provider_model_cache_replaces_one_provider_and_preserves_capabilities() {
    let cache_dir = unique_test_cache_dir("replace");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models(
            "openai",
            &[ProviderModel {
                provider: "openai".into(),
                model: "gpt-5.5".into(),
                display_name: "gpt-5.5".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            }],
        )
        .unwrap();
        replace_cached_provider_models(
            "anthropic",
            &[
                ProviderModel {
                    provider: "anthropic".into(),
                    model: "claude-b".into(),
                    display_name: "Claude B".into(),
                    context_window: None,
                    max_output_tokens: Some(64_000),
                    reasoning_capabilities: ReasoningCapabilities::Unknown,
                },
                ProviderModel {
                    provider: "anthropic".into(),
                    model: "claude-a".into(),
                    display_name: "Claude A".into(),
                    context_window: None,
                    max_output_tokens: Some(32_000),
                    reasoning_capabilities: ReasoningCapabilities::Unknown,
                },
            ],
        )
        .unwrap();
        replace_cached_provider_models(
            "anthropic",
            &[ProviderModel {
                provider: "anthropic".into(),
                model: "claude-c".into(),
                display_name: "Claude C".into(),
                context_window: Some(200_000),
                max_output_tokens: Some(16_000),
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            }],
        )
        .unwrap();

        assert_eq!(
            cached_provider_models("openai"),
            vec![ProviderModel {
                provider: "openai".into(),
                model: "gpt-5.5".into(),
                display_name: "gpt-5.5".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            }]
        );
        assert_eq!(
            cached_provider_models("anthropic"),
            vec![ProviderModel {
                provider: "anthropic".into(),
                model: "claude-c".into(),
                display_name: "Claude C".into(),
                context_window: Some(200_000),
                max_output_tokens: Some(16_000),
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            }]
        );
    });
    let _ = fs::remove_dir_all(cache_dir);
}

#[test]
fn provider_model_cache_migrates_old_schema() {
    let cache_dir = unique_test_cache_dir("migration");
    fs::create_dir_all(&cache_dir).unwrap();
    let connection = Connection::open(cache_dir.join("provider-models.sqlite3")).unwrap();
    connection
        .execute_batch(
            "create table provider_models (
                    provider text not null,
                    model text not null,
                    display_name text not null,
                    raw_json text,
                    updated_at integer not null,
                    primary key(provider, model)
                );
                create table provider_model_refresh (
                    provider text primary key,
                    updated_at integer not null,
                    error text
                );",
        )
        .unwrap();
    drop(connection);

    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models(
            "anthropic",
            &[ProviderModel {
                provider: "anthropic".into(),
                model: "claude-sonnet".into(),
                display_name: "Claude Sonnet".into(),
                context_window: None,
                max_output_tokens: Some(64_000),
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            }],
        )
        .unwrap();

        assert_eq!(
            cached_provider_model("anthropic", "claude-sonnet")
                .and_then(|model| model.max_output_tokens),
            Some(64_000)
        );
    });
    let _ = fs::remove_dir_all(cache_dir);
}
