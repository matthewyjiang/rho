use super::*;
use crate::credentials::{
    save_github_copilot_tokens, save_openrouter_oauth_key, save_provider_api_key,
    GitHubCopilotTokens, MemoryCredentialStore,
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

struct CacheDirReset;

impl Drop for CacheDirReset {
    fn drop(&mut self) {
        set_provider_models_cache_dir_for_tests(None);
    }
}

fn http_json_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut request = [0; 4096];
    let bytes = stream.read(&mut request).await.unwrap();
    String::from_utf8_lossy(&request[..bytes]).into_owned()
}

fn request_target(request: &str) -> &str {
    request.lines().next().unwrap_or_default()
}

async fn refresh_ollama(api_base: &Url) -> Vec<ProviderModel> {
    let descriptor = provider::provider_descriptor("ollama").unwrap();
    let store = MemoryCredentialStore::default();
    refresh_provider_models_with_store(
        descriptor.name,
        descriptor.default_auth().id,
        &store,
        ProviderModelEndpoint::OpenAiCompatible(api_base),
    )
    .await
    .unwrap()
    .models
}

// Covers: complete /api/tags rows must not N+1 /api/show, and embedding-only
// models stay out of the picker
// Owner: ollama native discovery
#[tokio::test]
async fn ollama_tags_complete_skips_show_and_hides_embedding_only() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = Url::parse(&format!("http://{}/v1", listener.local_addr().unwrap())).unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_for_server = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let request = read_http_request(&mut stream).await;
            seen_for_server
                .lock()
                .unwrap()
                .push(request_target(&request).to_string());
            assert!(
                !request.to_ascii_lowercase().contains("authorization:"),
                "{request}"
            );
            let body = r#"{"models":[
                {"name":"qwen3.8:27b","details":{"context_length":262144},"capabilities":["completion","tools","thinking","vision"]},
                {"name":"nomic-embed","details":{},"capabilities":["embedding"]}
            ]}"#;
            stream
                .write_all(http_json_response("200 OK", body).as_bytes())
                .await
                .unwrap();
        }
    });
    let cache = tempfile::tempdir().unwrap();
    set_provider_models_cache_dir_for_tests(Some(cache.path().to_path_buf()));
    let _cache_dir_reset = CacheDirReset;

    let models = refresh_ollama(&api_base).await;
    assert_eq!(
        models,
        vec![ProviderModel {
            provider: "ollama".into(),
            model: "qwen3.8:27b".into(),
            display_name: "qwen3.8:27b".into(),
            context_window: Some(262_144),
            max_output_tokens: None,
            reasoning_capabilities: ReasoningCapabilities::Levels(
                crate::model::ReasoningLevelSet::new(
                    crate::provider::OLLAMA_UNKNOWN_REASONING_LEVELS.to_vec()
                )
            ),
        }]
    );
    assert_eq!(cached_provider_models("ollama"), models);
    assert_eq!(seen.lock().unwrap().as_slice(), ["GET /api/tags HTTP/1.1"]);
}

// Covers: missing tags context_length must come from /api/show model_info
// Owner: ollama native discovery
#[tokio::test]
async fn ollama_incomplete_tags_fill_context_from_show() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = Url::parse(&format!("http://{}/v1", listener.local_addr().unwrap())).unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_for_server = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let request = read_http_request(&mut stream).await;
            seen_for_server
                .lock()
                .unwrap()
                .push(request_target(&request).to_string());
            let body = if request.starts_with("GET /api/tags ") {
                r#"{"models":[{"name":"gemma4:31b","details":{},"capabilities":["completion","tools","thinking"]}]}"#
            } else if request.starts_with("POST /api/show ") {
                r#"{"details":{},"capabilities":["completion","vision","tools","thinking"],"model_info":{"gemma4.context_length":262144}}"#
            } else {
                panic!("unexpected request: {request}");
            };
            stream
                .write_all(http_json_response("200 OK", body).as_bytes())
                .await
                .unwrap();
        }
    });
    let cache = tempfile::tempdir().unwrap();
    set_provider_models_cache_dir_for_tests(Some(cache.path().to_path_buf()));
    let _cache_dir_reset = CacheDirReset;

    let models = refresh_ollama(&api_base).await;
    assert_eq!(
        models,
        vec![ProviderModel {
            provider: "ollama".into(),
            model: "gemma4:31b".into(),
            display_name: "gemma4:31b".into(),
            context_window: Some(262_144),
            max_output_tokens: None,
            reasoning_capabilities: ReasoningCapabilities::Levels(
                crate::model::ReasoningLevelSet::new(
                    crate::provider::OLLAMA_UNKNOWN_REASONING_LEVELS.to_vec()
                )
            ),
        }]
    );
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["GET /api/tags HTTP/1.1", "POST /api/show HTTP/1.1"]
    );
}

// Covers: a stored ollama-api-key must authorize native tags and show
// Owner: ollama native discovery
#[tokio::test]
async fn ollama_native_discovery_sends_stored_api_key() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = Url::parse(&format!("http://{}/v1", listener.local_addr().unwrap())).unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_for_server = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let request = read_http_request(&mut stream).await;
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer ollama-secret"),
                "{request}"
            );
            seen_for_server.lock().unwrap().push(request.clone());
            let body = if request.starts_with("GET /api/tags ") {
                r#"{"models":[{"name":"gemma4:31b","details":{},"capabilities":["completion","tools","thinking"]}]}"#
            } else if request.starts_with("POST /api/show ") {
                r#"{"details":{},"capabilities":["completion","tools","thinking"],"model_info":{"gemma4.context_length":262144}}"#
            } else {
                panic!("unexpected request: {request}");
            };
            stream
                .write_all(http_json_response("200 OK", body).as_bytes())
                .await
                .unwrap();
        }
    });
    let cache = tempfile::tempdir().unwrap();
    set_provider_models_cache_dir_for_tests(Some(cache.path().to_path_buf()));
    let _cache_dir_reset = CacheDirReset;
    let store = MemoryCredentialStore::default();
    save_provider_api_key(&store, "ollama-api-key", "ollama-secret").unwrap();
    let descriptor = provider::provider_descriptor("ollama").unwrap();
    let models = refresh_provider_models_with_store(
        descriptor.name,
        "ollama-api-key",
        &store,
        ProviderModelEndpoint::OpenAiCompatible(&api_base),
    )
    .await
    .unwrap()
    .models;
    assert_eq!(
        models
            .iter()
            .map(|model| (model.model.as_str(), model.context_window))
            .collect::<Vec<_>>(),
        [("gemma4:31b", Some(262_144))]
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(seen[0].starts_with("GET /api/tags "));
    assert!(seen[1].starts_with("POST /api/show "));
}

// Covers: capability-less tags that /api/show later marks embedding-only stay out
// Owner: ollama native discovery
#[tokio::test]
async fn ollama_show_embedding_only_stays_out_of_the_picker() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = Url::parse(&format!("http://{}/v1", listener.local_addr().unwrap())).unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let request = read_http_request(&mut stream).await;
            let body = if request.starts_with("GET /api/tags ") {
                r#"{"models":[
                    {"name":"gemma4:31b","details":{"context_length":262144},"capabilities":["completion","tools","thinking"]},
                    {"name":"nomic-embed","details":{}}
                ]}"#
            } else if request.starts_with("POST /api/show ") {
                r#"{"details":{},"capabilities":["embedding"]}"#
            } else {
                panic!("unexpected request: {request}");
            };
            stream
                .write_all(http_json_response("200 OK", body).as_bytes())
                .await
                .unwrap();
        }
    });
    let cache = tempfile::tempdir().unwrap();
    set_provider_models_cache_dir_for_tests(Some(cache.path().to_path_buf()));
    let _cache_dir_reset = CacheDirReset;

    let models = refresh_ollama(&api_base).await;
    assert_eq!(
        models
            .iter()
            .map(|model| model.model.as_str())
            .collect::<Vec<_>>(),
        ["gemma4:31b"]
    );
}

// Covers: native tags failure still lists models from /v1/models without auth
// Owner: ollama native discovery
#[tokio::test]
async fn ollama_discovery_falls_back_to_v1_models_without_auth() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = Url::parse(&format!("http://{}/v1", listener.local_addr().unwrap())).unwrap();
    tokio::spawn(async move {
        for expected in ["GET /api/tags HTTP/1.1", "GET /v1/models HTTP/1.1"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            assert!(
                request.starts_with(expected),
                "expected {expected}, got {request}"
            );
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            let (status, body) = if expected.starts_with("GET /api/tags") {
                ("404 Not Found", r#"{"error":"not found"}"#)
            } else {
                (
                    "200 OK",
                    r#"{"data":[{"id":"qwen3-coder"},{"id":"qwen3-coder"},{"id":"devstral","name":"Devstral"}]}"#,
                )
            };
            stream
                .write_all(http_json_response(status, body).as_bytes())
                .await
                .unwrap();
        }
    });
    let cache = tempfile::tempdir().unwrap();
    set_provider_models_cache_dir_for_tests(Some(cache.path().to_path_buf()));
    let _cache_dir_reset = CacheDirReset;
    let store = MemoryCredentialStore::default();

    let models = refresh_ollama(&api_base).await;
    assert_eq!(
        models
            .iter()
            .map(|model| (model.model.as_str(), model.display_name.as_str()))
            .collect::<Vec<_>>(),
        vec![("devstral", "Devstral"), ("qwen3-coder", "qwen3-coder")]
    );
    assert_eq!(cached_provider_models("ollama"), models);
    assert!(crate::model::catalog::available_models_for_auths(
        &crate::credentials::available_auth_modes(&store)
    )
    .iter()
    .any(|model| model.provider == "ollama" && model.model == "qwen3-coder"));
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
async fn legacy_openrouter_refresh_writes_canonical_provider_models() {
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
            .contains("authorization: bearer oauth-secret"));
        let body = r#"{"data":[{"id":"anthropic/claude-sonnet-4"}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let cache = tempfile::tempdir().unwrap();
    set_provider_models_cache_dir_for_tests(Some(cache.path().to_path_buf()));
    let _cache_dir_reset = CacheDirReset;
    let store = MemoryCredentialStore::default();
    save_openrouter_oauth_key(&store, "oauth-secret").unwrap();

    let refresh = refresh_provider_models_with_store(
        "openrouter-oauth",
        "openrouter-api-key",
        &store,
        ProviderModelEndpoint::OpenAiCompatible(&api_base),
    )
    .await
    .unwrap();

    assert_eq!(refresh.provider, "openrouter");
    assert_eq!(refresh.models[0].provider, "openrouter");
    assert_eq!(cached_provider_models("openrouter"), refresh.models);
    assert!(cached_provider_models("openrouter-oauth").is_empty());
    server.await.unwrap();
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

    let models = openai_compatible::fetch(descriptor, descriptor.default_auth(), &api_base, &store)
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

// Covers: exact model lookup returns the targeted model directly from SQLite
// Owner: provider models cache
#[test]
fn cached_provider_model_exact_returns_single_model() {
    let cache_dir = unique_test_cache_dir("exact_lookup");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models(
            "test-provider",
            &[
                ProviderModel {
                    provider: "test-provider".into(),
                    model: "model-a".into(),
                    display_name: "Model A".into(),
                    context_window: Some(128_000),
                    max_output_tokens: Some(4096),
                    reasoning_capabilities: ReasoningCapabilities::Unknown,
                },
                ProviderModel {
                    provider: "test-provider".into(),
                    model: "model-b".into(),
                    display_name: "Model B".into(),
                    context_window: Some(256_000),
                    max_output_tokens: Some(8192),
                    reasoning_capabilities: ReasoningCapabilities::Unknown,
                },
            ],
        )
        .unwrap();

        let model = cached_provider_model("test-provider", "model-b").unwrap();
        assert_eq!(model.model, "model-b");
        assert_eq!(model.display_name, "Model B");
        assert_eq!(model.context_window, Some(256_000));
        assert_eq!(model.max_output_tokens, Some(8192));

        assert!(cached_provider_model("test-provider", "model-c").is_none());
    });
    let _ = fs::remove_dir_all(cache_dir);
}
