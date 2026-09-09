use std::num::NonZeroUsize;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{Message, ModelRequest},
    provider::provider_event_channel,
};
use tokio::{io::AsyncWriteExt, net::TcpListener};
use url::Url;

use super::{
    tests::read_complete_http_request, ProviderBuildOptions, ProviderBuilder, ProviderCredential,
};
use crate::{
    model::models_dev::{
        with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests, ModelMetadata,
    },
    providers::openai_compatible::CompatibleAuth,
    reasoning::ReasoningLevel,
};

// Covers: Go routing must retain conversation identity across turns on every
// adapter, including callers without cache keys. Other providers must not get it.
// Owner: provider builder through the SDK streaming API and HTTP wire.
#[tokio::test]
async fn opencode_go_session_headers_reach_every_adapter() {
    for (provider_name, npm, path) in [
        (
            "opencode-go",
            "@ai-sdk/openai-compatible",
            "/chat/completions",
        ),
        ("opencode-go", "@ai-sdk/openai", "/responses"),
        ("opencode-go", "@ai-sdk/anthropic", "/messages"),
        (
            "openrouter",
            "@ai-sdk/openai-compatible",
            "/chat/completions",
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let provider = with_models_dev_cache_dir_for_tests(cache.path().to_path_buf(), || {
            write_cached_model_metadata_for_tests(
                provider_name,
                "routing-test",
                &ModelMetadata {
                    sdk_package: Some(npm.into()),
                    reasoning_metadata_complete: false,
                    ..ModelMetadata::default()
                },
            );
            ProviderBuilder::new(
                ProviderBuildOptions::new(provider_name, "routing-test", ReasoningLevel::Off)
                    .unwrap()
                    .endpoint(endpoint)
                    .unwrap(),
                ProviderCredential::OpenAiCompatible(CompatibleAuth::ApiKey("test-key".into())),
            )
            .build()
            .unwrap()
        });
        let mut sessions = Vec::new();
        for cache_key in [
            Some("rho:session-a"),
            Some("rho:session-a"),
            Some("rho:session-b"),
            None,
            None,
        ] {
            let messages = [Message::user_text("hello")];
            // No events are emitted by this deliberate HTTP rejection; keep the
            // receiver alive so stream cancellation cannot mask the wire request.
            let (sender, _receiver) = provider_event_channel(NonZeroUsize::MIN);
            let request = provider.send_turn_stream(
                ModelRequest {
                    messages: &messages,
                    tools: &[],
                    cancellation: Default::default(),
                    reasoning_level: ReasoningLevel::Off,
                    prompt_cache_key: cache_key,
                },
                sender,
            );
            let server = async {
                let (mut stream, _) = listener.accept().await.unwrap();
                let raw = read_complete_http_request(&mut stream).await;
                // Stop at the HTTP boundary: this test owns request headers,
                // not the three independent response parsers.
                stream.write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await.unwrap();
                raw
            };
            let (result, raw) = tokio::join!(request, server);
            assert!(result.is_err());
            let headers = String::from_utf8(raw).unwrap();
            let mut lines = headers.split("\r\n\r\n").next().unwrap().lines();
            assert_eq!(lines.next(), Some(format!("POST {path} HTTP/1.1").as_str()));
            let headers: std::collections::BTreeMap<_, _> = lines
                .filter_map(|line| {
                    line.split_once(":")
                        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
                })
                .collect();
            let session = headers.get("x-opencode-session").cloned();
            if provider_name == "opencode-go" {
                assert_eq!(headers.get("user-agent"), Some(&crate::rho_user_agent()));
                if let Some(key) = cache_key {
                    assert_eq!(session.as_deref(), Some(key));
                } else {
                    assert!(session.as_ref().is_some_and(|value| !value.is_empty()));
                }
            } else {
                assert_eq!(session, None);
            }
            sessions.push(session);
        }
        assert_eq!(sessions[3], sessions[4]);
    }
}
