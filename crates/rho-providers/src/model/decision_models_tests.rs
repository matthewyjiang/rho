use std::future::Future;

use pretty_assertions::assert_eq;
use reqwest::Url;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

use super::{
    cached_decision_models, refresh_decision_models_with_store,
    replace_cached_decision_models_for_tests,
};
use crate::{
    credentials::{save_provider_api_key, MemoryCredentialStore},
    model::{provider_models::with_provider_models_cache_dir_for_tests, ModelError},
    provider,
};

// Keep the thread-local cache override on the same thread across async requests.
fn with_cache(test: impl Future<Output = ()>) {
    let cache = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache.path().to_path_buf(), || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(test);
    });
}

async fn models_server(status: &'static str, body: &'static str) -> (Url, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = Url::parse(&format!(
        "http://{}/proxy/v1/",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            assert_ne!(stream.read_buf(&mut request).await.unwrap(), 0);
        }
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    (api_base, server)
}

// Covers: decision discovery must filter chat/embedding tags and replace stale cache rows
// Owner: provider discovery wire contract and cache
#[test]
fn ollama_tags_keep_only_decision_models_and_cache_them() {
    with_cache(async {
        let body = r#"{"models":[
            {"name":" clef:latest ","capabilities":["decision"]},
            {"name":"clef-flash:latest","capabilities":["decision","completion"]},
            {"name":"clef:latest","capabilities":["decision"]},
            {"name":"gemma4:31b","capabilities":["completion","vision","tools","thinking"]},
            {"name":"nomic-embed","capabilities":["embedding"]},
            {"name":"legacy"},
            {"name":"empty","capabilities":[]},
            {"name":"  ","capabilities":["decision"]}
        ]}"#;
        let store = MemoryCredentialStore::default();
        save_provider_api_key(&store, "ollama", "ollama-secret").unwrap();
        for auth in ["none", "ollama-api-key"] {
            replace_cached_decision_models_for_tests("ollama", vec!["stale".into()]);
            let (api_base, server) = models_server("200 OK", body).await;
            let models = refresh_decision_models_with_store("ollama", auth, &store, &api_base)
                .await
                .unwrap();
            assert_eq!(models, vec!["clef-flash:latest", "clef:latest"]);
            assert_eq!(cached_decision_models("ollama"), models);
            let request = server.await.unwrap();
            assert_eq!(request.lines().next(), Some("GET /proxy/api/tags HTTP/1.1"));
            let authorization = request
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("authorization:"));
            let expected = match auth {
                "none" => None,
                "ollama-api-key" => Some(format!(
                    "authorization: Bearer {}",
                    std::env::var("RHO_OLLAMA_API_KEY")
                        .ok()
                        .filter(|key| !key.trim().is_empty())
                        .unwrap_or_else(|| "ollama-secret".into())
                )),
                _ => unreachable!(),
            };
            assert_eq!(authorization, expected.as_deref());
        }
    });
}

// Covers: TypeSafe uses its name-based /models schema and resolved base with bearer auth
// Owner: provider discovery wire contract and cache
#[test]
fn typesafe_models_use_names_and_bearer_auth() {
    with_cache(async {
        let body = r#"{"models":[
            {"name":"jev-preview","description":"preview","release_date":"2026-04-01"},
            {"name":" jev-latest "},
            {"name":"jev-latest"},
            {"name":" "}
        ]}"#;
        let (api_base, server) = models_server("200 OK", body).await;
        let store = MemoryCredentialStore::default();
        save_provider_api_key(&store, "typesafe", "typesafe-secret").unwrap();
        let descriptor = provider::provider_descriptor("typesafe").unwrap();
        let models = refresh_decision_models_with_store(
            "typesafe",
            descriptor.default_auth().id,
            &store,
            &api_base,
        )
        .await
        .unwrap();
        assert_eq!(models, vec!["jev-latest", "jev-preview"]);
        assert_eq!(cached_decision_models("typesafe"), models);
        let request = server.await.unwrap();
        assert_eq!(
            request.lines().next(),
            Some("GET /proxy/v1/models HTTP/1.1")
        );
        let authorization = request
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("authorization:"));
        let key = std::env::var("TYPESAFE_API_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty())
            .unwrap_or_else(|| "typesafe-secret".into());
        assert_eq!(
            authorization,
            Some(format!("authorization: Bearer {key}").as_str())
        );
    });
}

// Covers: failed HTTP/JSON discovery must preserve previous cache and redact echoed secrets
// Owner: provider discovery wire contract and cache
#[test]
fn failed_refresh_preserves_cache_and_redacts_response() {
    with_cache(async {
        let store = MemoryCredentialStore::default();
        save_provider_api_key(&store, "typesafe", "typesafe-secret").unwrap();
        let cases = [
            (
                "ollama",
                "503 Service Unavailable",
                r#"{"error":"echoed-secret"}"#,
                "Ollama /api/tags returned 503 Service Unavailable",
            ),
            (
                "typesafe",
                "401 Unauthorized",
                r#"{"error":"echoed-secret"}"#,
                "TypeSafe /models returned 401 Unauthorized",
            ),
            (
                "ollama",
                "200 OK",
                r#"{"models":"echoed-secret"}"#,
                "invalid Ollama /api/tags response",
            ),
            (
                "typesafe",
                "200 OK",
                r#"{"models":"echoed-secret"}"#,
                "invalid TypeSafe /models response",
            ),
        ];
        for (provider, status, body, expected) in cases {
            let previous = vec!["previous-decision-model".into()];
            replace_cached_decision_models_for_tests(provider, previous.clone());
            let (api_base, server) = models_server(status, body).await;
            let descriptor = provider::provider_descriptor(provider).unwrap();
            let error = refresh_decision_models_with_store(
                provider,
                descriptor.default_auth().id,
                &store,
                &api_base,
            )
            .await
            .unwrap_err();
            let ModelError::InvalidResponse(error) = error else {
                panic!("expected InvalidResponse, got {error:?}");
            };
            assert_eq!(error, expected, "{provider} {status}");
            assert_eq!(cached_decision_models(provider), previous);
            server.await.unwrap();
        }
    });
}

// Covers: bases without a native root clear Ollama's list without fetching; unsupported hosts fail
// Owner: provider discovery dispatch
#[test]
fn non_native_ollama_base_is_empty_and_other_hosts_are_unsupported() {
    with_cache(async {
        let store = MemoryCredentialStore::default();
        // A non-HTTP URL would fail immediately if discovery attempted a request.
        let api_base = Url::parse("file:///ollama").unwrap();
        replace_cached_decision_models_for_tests("ollama", vec!["stale".into()]);
        assert_eq!(
            refresh_decision_models_with_store("ollama", "none", &store, &api_base)
                .await
                .unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(cached_decision_models("ollama"), Vec::<String>::new());
        for provider in ["openai", "ollama-cloud", "not-a-provider"] {
            let error = refresh_decision_models_with_store(provider, "none", &store, &api_base)
                .await
                .unwrap_err();
            let ModelError::UnsupportedProvider(name) = error else {
                panic!("expected UnsupportedProvider, got {error:?}");
            };
            assert_eq!(name, provider);
        }
    });
}
