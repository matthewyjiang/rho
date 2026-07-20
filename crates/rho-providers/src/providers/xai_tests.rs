use std::sync::Arc;

use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::*;
use crate::{
    credentials::{save_xai_tokens, MemoryCredentialStore, XaiTokens},
    model::{
        models_dev::with_models_dev_cache_dir_for_tests,
        provider_models::with_provider_models_cache_dir_for_tests, Message, ModelIdentity,
    },
    reasoning::ReasoningLevel,
};
use rho_tools::tool::ToolSpec;

#[test]
fn empty_cache_construction_preserves_static_wire_semantics_for_both_identities() {
    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir_for_tests(cache.path().join("models-dev"), || {
        with_provider_models_cache_dir_for_tests(cache.path().join("provider-models"), || {
            for provider_name in ["xai", "xai-oauth"] {
                for (model, off, high) in [
                    ("grok-build-0.1", None, None),
                    ("grok-composer-2.5-fast", None, None),
                    ("grok-4.3", Some("none"), Some("high")),
                    ("grok-4.5", None, Some("high")),
                ] {
                    let store = Arc::new(MemoryCredentialStore::default());
                    save_xai_tokens(
                        store.as_ref(),
                        &XaiTokens {
                            access_token: "access-token".into(),
                            refresh_token: None,
                            expires_at_unix: None,
                            id_token: None,
                        },
                    )
                    .unwrap();
                    let provider = XaiProvider::new_with_transport(
                        provider_name,
                        model.into(),
                        XaiAuthManager::new(store).unwrap(),
                        reqwest::Client::new(),
                        "https://api.x.ai/v1".into(),
                    );
                    assert_eq!(
                        provider.model_identity(),
                        ModelIdentity::new(provider_name, "openai-responses", model)
                    );
                    assert_eq!(provider.reasoning.effort(ReasoningLevel::Off), off);
                    assert_eq!(provider.reasoning.effort(ReasoningLevel::High), high);
                }
            }
        });
    });
}

#[test]
fn unknown_grok_4_5_off_does_not_enable_reasoning_on_the_wire() {
    let profile = reasoning::XaiReasoningProfile::from_metadata("grok-4.5", None);
    let body = build_xai_responses_body(
        "xai",
        "grok-4.5",
        &profile,
        ModelRequest {
            messages: &[],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Off,
            prompt_cache_key: None,
        },
    )
    .unwrap();

    assert!(body.get("reasoning").is_none());
}

#[test]
fn responses_body_preserves_tools_cache_key_and_supported_reasoning() {
    let messages = [
        Message::System("follow instructions".into()),
        Message::user_text("fix it"),
    ];
    let tools = [ToolSpec {
        name: "read_file".into(),
        description: "read a file".into(),
        input_schema: json!({"type": "object"}),
    }];

    let profile = reasoning::XaiReasoningProfile::exact([
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
    ]);
    let body = build_xai_responses_body(
        "xai",
        "grok-4.5",
        &profile,
        ModelRequest {
            messages: &messages,
            tools: &tools,
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::High,
            prompt_cache_key: Some("rho:session"),
        },
    )
    .unwrap();

    assert_eq!(body["model"], "grok-4.5");
    assert_eq!(body["instructions"], "follow instructions");
    assert_eq!(body["input"][0]["role"], "user");
    assert_eq!(body["tools"][0]["name"], "read_file");
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["prompt_cache_key"], "rho:session");
    assert_eq!(body["reasoning"], json!({"effort": "high"}));
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
}

#[test]
fn responses_body_uses_each_request_reasoning_level() {
    let messages = [Message::user_text("hello")];
    let profile = reasoning::XaiReasoningProfile::exact([
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
    ]);
    let low = build_xai_responses_body(
        "xai",
        "grok-4.5",
        &profile,
        ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Low,
            prompt_cache_key: None,
        },
    )
    .unwrap();
    let high = build_xai_responses_body(
        "xai",
        "grok-4.5",
        &profile,
        ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::High,
            prompt_cache_key: None,
        },
    )
    .unwrap();

    assert_eq!(low["reasoning"], json!({"effort": "low"}));
    assert_eq!(high["reasoning"], json!({"effort": "high"}));
}

#[tokio::test]
async fn provider_posts_to_responses_and_collects_stream() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let count = stream.read(&mut chunk).await.unwrap();
            request.extend_from_slice(&chunk[..count]);
            let text = String::from_utf8_lossy(&request);
            let Some((headers, body)) = text.split_once("\r\n\r\n") else {
                continue;
            };
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            if body.len() >= content_length {
                break;
            }
        }
        let request = String::from_utf8(request).unwrap();
        assert!(request.starts_with("POST /responses HTTP/1.1\r\n"));
        assert!(request.contains("authorization: Bearer access-token\r\n"));
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["reasoning"], json!({"effort": "medium"}));

        let event = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"response-1\"}}\n\ndata: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{event}",
            event.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let store = Arc::new(MemoryCredentialStore::default());
    save_xai_tokens(
        store.as_ref(),
        &XaiTokens {
            access_token: "access-token".into(),
            refresh_token: None,
            expires_at_unix: None,
            id_token: None,
        },
    )
    .unwrap();
    let provider = XaiProvider::new_with_api_base("grok-4.5".into(), store, api_base).unwrap();

    let response = provider
        .complete_turn(ModelRequest {
            messages: &[Message::user_text("hello")],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Medium,
            prompt_cache_key: None,
        })
        .await
        .unwrap();

    let ModelResponse::Assistant(blocks) = response;
    assert!(matches!(
        blocks.as_slice(),
        [crate::model::ContentBlock::Text(text)] if text == "hello"
    ));
    server.await.unwrap();
}
