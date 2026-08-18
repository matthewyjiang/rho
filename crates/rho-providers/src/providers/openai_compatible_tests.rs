use super::*;
use crate::model::{
    provider_models::{
        replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
        ProviderModel,
    },
    ContentBlock, Message, ReasoningCapabilities, ReasoningLevelSet,
};
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use crate::model::ToolSpec;

fn sample_tool() -> ToolSpec {
    ToolSpec {
        name: "edit".into(),
        description: "test tool".into(),
        input_schema: json!({"type":"object","properties":{}}),
    }
}

#[tokio::test]
async fn moonshot_posts_chat_completions_with_bearer_auth() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 8192];
        let bytes = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..bytes]);
        assert!(request.starts_with("POST /chat/completions HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer moonshot-secret"));
        assert!(request.contains("\"model\":\"kimi-k3\""));
        let request_body = request.split("\r\n\r\n").nth(1).unwrap();
        let body: serde_json::Value = serde_json::from_str(request_body).unwrap();
        assert_eq!(body["reasoning_effort"], "low");
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning").is_none());
        assert!(body.get("output_config").is_none());
        let schema = &body["tools"][0]["function"]["parameters"];
        assert_eq!(schema["type"], "object");
        assert!(schema.get("anyOf").is_none());

        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hello"}}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let mut provider = OpenAiCompatibleProvider::new(
        reqwest::Client::new(),
        "moonshot",
        "kimi-k3".into(),
        OpenAiCompatibleDialect::Moonshot,
        CompatibleAuth::ApiKey("moonshot-secret".into()),
        api_base,
    );
    provider.reasoning =
        reasoning::DialectReasoning::Moonshot(reasoning::MoonshotReasoningProfile::exact([
            crate::reasoning::ReasoningLevel::Off,
            crate::reasoning::ReasoningLevel::Low,
            crate::reasoning::ReasoningLevel::Max,
        ]));
    let tool = sample_tool();
    let response = provider
        .complete_turn(ModelRequest {
            messages: &[Message::user_text("hello")],
            tools: &[tool],
            cancellation: Default::default(),
            reasoning_level: crate::reasoning::ReasoningLevel::Low,
            prompt_cache_key: None,
        })
        .await
        .unwrap();
    assert!(matches!(
        response,
        ModelResponse::Assistant(blocks)
            if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "hello")
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn openrouter_posts_reasoning_to_chat_completions() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 8192];
        let bytes = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..bytes]);
        assert!(request.starts_with("POST /chat/completions HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer openrouter-secret"));
        let body: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["model"], "anthropic/claude-sonnet-4");
        assert_eq!(body["reasoning"]["effort"], "high");
        assert!(body.get("reasoning_effort").is_none());

        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hello"}}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let provider = OpenAiCompatibleProvider::new(
        reqwest::Client::new(),
        "openrouter",
        "anthropic/claude-sonnet-4".into(),
        OpenAiCompatibleDialect::OpenRouter,
        CompatibleAuth::ApiKey("openrouter-secret".into()),
        api_base,
    );
    provider
        .complete_turn(ModelRequest {
            messages: &[Message::user_text("hello")],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: crate::reasoning::ReasoningLevel::High,
            prompt_cache_key: None,
        })
        .await
        .unwrap();
    server.await.unwrap();
}

#[test]
fn kimi_code_k3_serializes_each_reasoning_mode_as_a_whole_request() {
    for (reasoning_level, thinking) in [
        (
            crate::reasoning::ReasoningLevel::Off,
            json!({"type": "disabled"}),
        ),
        (
            crate::reasoning::ReasoningLevel::Low,
            json!({"type": "enabled", "effort": "low"}),
        ),
        (
            crate::reasoning::ReasoningLevel::High,
            json!({"type": "enabled", "effort": "high"}),
        ),
        (
            crate::reasoning::ReasoningLevel::Max,
            json!({"type": "enabled", "effort": "max"}),
        ),
    ] {
        assert_eq!(
            request_body(OpenAiCompatibleDialect::KimiCode, "k3", reasoning_level),
            json!({
                "model": "k3",
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": "hello"}]
                }],
                "stream": false,
                "thinking": thinking
            })
        );
    }
}

#[test]
fn kimi_code_k3_serializes_an_unnormalized_effort_as_is() {
    let mut provider = OpenAiCompatibleProvider::new(
        reqwest::Client::new(),
        "kimi-code",
        "k3".into(),
        OpenAiCompatibleDialect::KimiCode,
        CompatibleAuth::ApiKey("secret".into()),
        "https://example.com".into(),
    );
    provider.reasoning = reasoning::DialectReasoning::KimiCode(
        reasoning::KimiReasoningProfile::new(ReasoningCapabilities::Unknown),
    );
    let messages = [Message::user_text("hello")];
    let request = provider
        .request_body(
            ModelRequest {
                messages: &messages,
                tools: &[],
                cancellation: Default::default(),
                reasoning_level: crate::reasoning::ReasoningLevel::Medium,
                prompt_cache_key: None,
            },
            /*stream*/ false,
        )
        .unwrap();

    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({
            "model": "k3",
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": "hello"}]
            }],
            "stream": false,
            "thinking": {"type": "enabled", "effort": "medium"}
        })
    );
}

#[test]
fn authenticated_capabilities_normalize_before_kimi_request_serialization() {
    let cache_dir = std::env::temp_dir().join(format!(
        "rho-kimi-request-capabilities-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models_for_tests(
            "kimi-code",
            &[ProviderModel {
                provider: "kimi-code".into(),
                model: "k3".into(),
                display_name: "Kimi K3".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Levels(ReasoningLevelSet::new(
                    vec![
                        crate::reasoning::ReasoningLevel::Off,
                        crate::reasoning::ReasoningLevel::Low,
                        crate::reasoning::ReasoningLevel::High,
                        crate::reasoning::ReasoningLevel::Max,
                    ],
                )),
            }],
        )
        .unwrap();
        assert_eq!(
            request_body(
                OpenAiCompatibleDialect::KimiCode,
                "k3",
                crate::reasoning::ReasoningLevel::Medium,
            ),
            json!({
                "model": "k3",
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": "hello"}]
                }],
                "stream": false,
                "thinking": {"type": "enabled", "effort": "high"}
            })
        );
    });
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn moonshot_k3_never_serializes_off_and_preserves_offline_non_off_requests() {
    let exact = reasoning::MoonshotReasoningProfile::exact([
        crate::reasoning::ReasoningLevel::Off,
        crate::reasoning::ReasoningLevel::Low,
        crate::reasoning::ReasoningLevel::High,
    ]);
    assert_eq!(
        exact.effort(crate::reasoning::ReasoningLevel::Off),
        Some("low")
    );

    let offline = reasoning::MoonshotReasoningProfile::from_metadata("kimi-k3", None);
    assert_eq!(offline.effort(crate::reasoning::ReasoningLevel::Off), None);
    assert_eq!(
        offline.effort(crate::reasoning::ReasoningLevel::High),
        Some("high")
    );

    let unknown_model = reasoning::MoonshotReasoningProfile::from_metadata("future-model", None);
    assert_eq!(
        unknown_model.effort(crate::reasoning::ReasoningLevel::High),
        None
    );
}

#[test]
fn moonshot_exact_metadata_drives_top_level_reasoning_effort() {
    assert_eq!(
        request_body(
            OpenAiCompatibleDialect::Moonshot,
            "moonshot-reasoner",
            crate::reasoning::ReasoningLevel::Max,
        ),
        json!({
            "model": "moonshot-reasoner",
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": "hello"}]
            }],
            "stream": false,
            "reasoning_effort": "max"
        })
    );
}

#[test]
fn openrouter_omits_reasoning_for_non_configurable_models() {
    let profile =
        reasoning::DialectReasoning::OpenRouter(reasoning::EffortProfile::not_configurable());
    let fields = profile.fields("fixed-model", crate::reasoning::ReasoningLevel::High);

    assert!(fields.reasoning.is_none());
}

#[test]
fn ollama_cloud_metadata_drives_top_level_reasoning_effort() {
    let mut provider = OpenAiCompatibleProvider::new(
        reqwest::Client::new(),
        "ollama-cloud",
        "glm-5.2".into(),
        OpenAiCompatibleDialect::Standard,
        CompatibleAuth::ApiKey("secret".into()),
        "https://ollama.com/v1".into(),
    );
    provider.reasoning = reasoning::DialectReasoning::Standard(reasoning::EffortProfile::levels([
        crate::reasoning::ReasoningLevel::Off,
        crate::reasoning::ReasoningLevel::High,
        crate::reasoning::ReasoningLevel::Max,
    ]));
    let messages = [Message::user_text("hello")];
    let high = serde_json::to_value(
        provider
            .request_body(
                ModelRequest {
                    messages: &messages,
                    tools: &[],
                    cancellation: Default::default(),
                    reasoning_level: crate::reasoning::ReasoningLevel::High,
                    prompt_cache_key: None,
                },
                /*stream*/ false,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        high,
        json!({
            "model": "glm-5.2",
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": "hello"}]
            }],
            "stream": false,
            "reasoning_effort": "high"
        })
    );

    let off = serde_json::to_value(
        provider
            .request_body(
                ModelRequest {
                    messages: &messages,
                    tools: &[],
                    cancellation: Default::default(),
                    reasoning_level: crate::reasoning::ReasoningLevel::Off,
                    prompt_cache_key: None,
                },
                /*stream*/ false,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(off["reasoning_effort"], "none");
}

#[test]
fn off_as_none_standard_hosts_send_reasoning_effort_when_catalog_is_unknown() {
    let _lock = crate::provider::custom_provider_registry_test_lock();
    crate::provider::reset_custom_openai_compatible_providers_for_tests();
    struct RestoreCustomProviders;
    impl Drop for RestoreCustomProviders {
        fn drop(&mut self) {
            crate::provider::reset_custom_openai_compatible_providers_for_tests();
        }
    }
    let _restore = RestoreCustomProviders;
    crate::provider::install_custom_openai_compatible_providers(["composer"]).unwrap();

    for provider in ["ollama", "ollama-cloud", "composer"] {
        assert_eq!(
            standard_request_reasoning_effort(provider, crate::reasoning::ReasoningLevel::Off)
                .as_deref(),
            Some("none"),
            "{provider}"
        );
        assert_eq!(
            standard_request_reasoning_effort(provider, crate::reasoning::ReasoningLevel::High)
                .as_deref(),
            Some("high"),
            "{provider}"
        );
    }
}

// Covers: unknown Ollama models must not send effort values the server rejects
// Owner: openai-compatible wire encoding
#[test]
fn unknown_ollama_models_send_only_accepted_reasoning_effort() {
    let _lock = crate::provider::custom_provider_registry_test_lock();
    crate::provider::reset_custom_openai_compatible_providers_for_tests();
    struct RestoreCustomProviders;
    impl Drop for RestoreCustomProviders {
        fn drop(&mut self) {
            crate::provider::reset_custom_openai_compatible_providers_for_tests();
        }
    }
    let _restore = RestoreCustomProviders;
    crate::provider::install_custom_openai_compatible_providers(["composer"]).unwrap();

    for (provider, requested, expected) in [
        ("ollama", crate::reasoning::ReasoningLevel::Off, "none"),
        ("ollama", crate::reasoning::ReasoningLevel::Minimal, "low"),
        ("ollama", crate::reasoning::ReasoningLevel::Low, "low"),
        ("ollama", crate::reasoning::ReasoningLevel::Medium, "medium"),
        ("ollama", crate::reasoning::ReasoningLevel::High, "high"),
        ("ollama", crate::reasoning::ReasoningLevel::Xhigh, "max"),
        ("ollama", crate::reasoning::ReasoningLevel::Max, "max"),
        (
            "ollama-cloud",
            crate::reasoning::ReasoningLevel::Minimal,
            "low",
        ),
        (
            "ollama-cloud",
            crate::reasoning::ReasoningLevel::Xhigh,
            "max",
        ),
        (
            "composer",
            crate::reasoning::ReasoningLevel::Minimal,
            "minimal",
        ),
        ("composer", crate::reasoning::ReasoningLevel::Xhigh, "xhigh"),
    ] {
        assert_eq!(
            standard_request_reasoning_effort(provider, requested).as_deref(),
            Some(expected),
            "{provider} {requested:?}"
        );
    }
}

// Covers: ExactAdvertised Standard hosts must not guess effort for unlisted models
// Owner: openai-compatible wire encoding
#[test]
fn exact_advertised_standard_hosts_omit_reasoning_when_catalog_is_unknown() {
    assert_eq!(
        standard_request_reasoning_effort("meta", crate::reasoning::ReasoningLevel::High),
        None
    );
}

fn standard_request_reasoning_effort(
    provider: &'static str,
    reasoning_level: crate::reasoning::ReasoningLevel,
) -> Option<String> {
    let client = OpenAiCompatibleProvider::new(
        reqwest::Client::new(),
        provider,
        "unlisted-local".into(),
        OpenAiCompatibleDialect::Standard,
        CompatibleAuth::None,
        "http://127.0.0.1:11434/v1".into(),
    );
    let messages = [Message::user_text("hello")];
    let body = serde_json::to_value(
        client
            .request_body(
                ModelRequest {
                    messages: &messages,
                    tools: &[],
                    cancellation: Default::default(),
                    reasoning_level,
                    prompt_cache_key: None,
                },
                /*stream*/ false,
            )
            .unwrap(),
    )
    .unwrap();
    body.get("reasoning_effort")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn request_body(
    dialect: OpenAiCompatibleDialect,
    model: &str,
    reasoning_level: crate::reasoning::ReasoningLevel,
) -> serde_json::Value {
    let provider_name = match dialect {
        OpenAiCompatibleDialect::Standard => "standard",
        OpenAiCompatibleDialect::Custom => "composer",
        OpenAiCompatibleDialect::QwenTokenPlan => "qwen-token-plan",
        OpenAiCompatibleDialect::Poolside => "poolside",
        OpenAiCompatibleDialect::Moonshot => "moonshot",
        OpenAiCompatibleDialect::OpenRouter => "openrouter",
        OpenAiCompatibleDialect::KimiCode => "kimi-code",
    };
    let mut provider = OpenAiCompatibleProvider::new(
        reqwest::Client::new(),
        provider_name,
        model.into(),
        dialect,
        CompatibleAuth::ApiKey("secret".into()),
        "https://example.com".into(),
    );
    if dialect == OpenAiCompatibleDialect::Moonshot {
        provider.reasoning =
            reasoning::DialectReasoning::Moonshot(reasoning::MoonshotReasoningProfile::exact([
                crate::reasoning::ReasoningLevel::Off,
                crate::reasoning::ReasoningLevel::Low,
                crate::reasoning::ReasoningLevel::High,
                crate::reasoning::ReasoningLevel::Max,
            ]));
    }
    let messages = [Message::user_text("hello")];
    let request = provider
        .request_body(
            ModelRequest {
                messages: &messages,
                tools: &[],
                cancellation: Default::default(),
                reasoning_level,
                prompt_cache_key: None,
            },
            /*stream*/ false,
        )
        .unwrap();
    serde_json::to_value(request).unwrap()
}

#[test]
fn poolside_request_body_uses_namespaced_model_and_thinking_control() {
    let provider = OpenAiCompatibleProvider::new(
        reqwest::Client::new(),
        "poolside",
        "laguna-m.1".into(),
        OpenAiCompatibleDialect::Poolside,
        CompatibleAuth::ApiKey("secret".into()),
        "https://inference.poolside.ai/v1".into(),
    );
    let messages = [Message::user_text("hello")];
    let body = provider
        .request_body(
            ModelRequest {
                messages: &messages,
                tools: &[],
                cancellation: Default::default(),
                reasoning_level: crate::reasoning::ReasoningLevel::Max,
                prompt_cache_key: None,
            },
            /*stream*/ false,
        )
        .unwrap();

    assert_eq!(provider.model_identity().model, "laguna-m.1");
    assert_eq!(body.model, "poolside/laguna-m.1");
    assert_eq!(body.chat_template_kwargs, None);

    let body = provider
        .request_body(
            ModelRequest {
                messages: &messages,
                tools: &[],
                cancellation: Default::default(),
                reasoning_level: crate::reasoning::ReasoningLevel::Off,
                prompt_cache_key: None,
            },
            /*stream*/ false,
        )
        .unwrap();
    assert_eq!(
        body.chat_template_kwargs,
        Some(crate::protocol::openai_chat::ChatTemplateKwargs {
            enable_thinking: false,
        })
    );
    assert_eq!(
        serde_json::to_value(body).unwrap()["chat_template_kwargs"],
        json!({"enable_thinking": false})
    );
}

#[tokio::test]
async fn poolside_publishes_only_final_stream_usage_snapshot() {
    // Poolside enables thinking by omission for non-Off requests, so this
    // aggregate usage payload (no reasoning-token details, nothing streamed)
    // may hide off-wire reasoning: throughput reports unavailable rather than
    // the inflated aggregate completion total.
    let events = poolside_stream_events(
        crate::reasoning::ReasoningLevel::Max,
        concat!(
            "data: {\"usage\":{\"prompt_tokens\":6900,\"completion_tokens\":1,\"total_tokens\":6901},\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n",
            "data: {\"usage\":{\"prompt_tokens\":7200,\"completion_tokens\":2,\"total_tokens\":7202},\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"usage\":{\"prompt_tokens\":6900,\"completion_tokens\":2,\"total_tokens\":6902},\"choices\":[]}\n\n",
            "data: [DONE]\n\n"
        ),
    )
    .await;

    assert_eq!(
        events,
        vec![
            ModelEvent::OutputDelta("hel".into()),
            ModelEvent::OutputDelta("lo".into()),
            ModelEvent::ProviderContext {
                kind: "rho_model_call_generation_output_tokens".into(),
                position: None,
                data: json!({ "tokens": null }),
            },
            ModelEvent::Usage(ModelUsage {
                input_tokens: Some(6_900),
                output_tokens: Some(2),
                total_tokens: Some(6_902),
                ..ModelUsage::default()
            }),
        ]
    );
}

#[tokio::test]
async fn poolside_off_trusts_aggregate_output_without_reasoning_details() {
    // Off serializes `enable_thinking: false`, so an aggregate usage payload
    // without reasoning-token details stays unreported (no carrier) rather
    // than unavailable.
    let events = poolside_stream_events(
        crate::reasoning::ReasoningLevel::Off,
        concat!(
            "data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2},\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: [DONE]\n\n"
        ),
    )
    .await;

    assert_eq!(
        events,
        vec![
            ModelEvent::OutputDelta("hi".into()),
            ModelEvent::Usage(ModelUsage {
                input_tokens: Some(10),
                output_tokens: Some(2),
                ..ModelUsage::default()
            }),
        ]
    );
}

#[tokio::test]
async fn poolside_streamed_reasoning_reports_full_output_total() {
    // Reasoning that streamed spent its wall time inside the measured
    // generation window, so the full output total is the matching throughput
    // numerator even without reasoning-token details.
    let events = poolside_stream_events(
        crate::reasoning::ReasoningLevel::Max,
        concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking\"}}]}\n\n",
            "data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":230},\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: [DONE]\n\n"
        ),
    )
    .await;

    assert_eq!(
        events,
        vec![
            ModelEvent::ReasoningDelta("thinking".into()),
            ModelEvent::OutputDelta("hi".into()),
            ModelEvent::ProviderContext {
                kind: "rho_model_call_generation_output_tokens".into(),
                position: None,
                data: json!({ "tokens": 230 }),
            },
            ModelEvent::Usage(ModelUsage {
                input_tokens: Some(10),
                output_tokens: Some(230),
                ..ModelUsage::default()
            }),
            ModelEvent::ProviderContext {
                kind: "openai_chat_reasoning_content".into(),
                position: Some(0),
                data: json!("thinking"),
            },
        ]
    );
}

/// Streams `sse_body` from a local server through a Poolside provider and
/// returns the emitted events, covering the request-construction-to-stream
/// path so risk classification cannot be masked by a manually supplied value.
async fn poolside_stream_events(
    reasoning_level: crate::reasoning::ReasoningLevel,
    sse_body: &'static str,
) -> Vec<ModelEvent> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 8192];
        let request_size = stream.read(&mut request).await.unwrap();
        assert!(request_size > 0);
        let request = String::from_utf8_lossy(&request[..request_size]);
        let body: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        match reasoning_level {
            crate::reasoning::ReasoningLevel::Off => assert_eq!(
                body["chat_template_kwargs"],
                json!({"enable_thinking": false})
            ),
            _ => assert!(body.get("chat_template_kwargs").is_none()),
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse_body}",
            sse_body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let provider = OpenAiCompatibleProvider::new(
        reqwest::Client::new(),
        "poolside",
        "laguna-m.1".into(),
        OpenAiCompatibleDialect::Poolside,
        CompatibleAuth::ApiKey("secret".into()),
        api_base,
    );
    let messages = [Message::user_text("hello")];
    let mut events = Vec::new();
    provider
        .stream_turn(
            ModelRequest {
                messages: &messages,
                tools: &[],
                cancellation: Default::default(),
                reasoning_level,
                prompt_cache_key: None,
            },
            &mut |event| {
                events.push(event);
                Ok(())
            },
            &mut |_| Ok(()),
        )
        .await
        .unwrap();
    server.await.unwrap();
    events
}

#[tokio::test]
async fn standard_dialect_streams_without_auth_or_usage() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 8192];
        let bytes = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..bytes]);
        assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        let body: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert!(body.get("reasoning").is_none());
        assert_eq!(body["reasoning_effort"], "none");
        assert!(body.get("thinking").is_none());

        // Ollama may omit the optional usage chunk.
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let provider = OpenAiCompatibleProvider::new(
        reqwest::Client::new(),
        "ollama",
        "qwen3-coder".into(),
        OpenAiCompatibleDialect::Standard,
        CompatibleAuth::None,
        api_base,
    );
    let messages = [Message::user_text("hello")];
    let mut events = Vec::new();
    let response = provider
        .stream_turn(
            ModelRequest {
                messages: &messages,
                tools: &[],
                cancellation: Default::default(),
                reasoning_level: crate::reasoning::ReasoningLevel::Off,
                prompt_cache_key: None,
            },
            &mut |event| {
                events.push(event);
                Ok(())
            },
            &mut |_| Ok(()),
        )
        .await
        .unwrap();

    assert!(matches!(
        response,
        ModelResponse::Assistant(blocks)
            if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "hello")
    ));
    assert!(!events.is_empty());
    server.await.unwrap();
}

#[tokio::test]
async fn standard_dialect_converts_tool_calls_and_http_errors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 8192];
            let bytes = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..bytes]);
            assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            let (status, body) = if index == 0 {
                (
                    "200 OK",
                    r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"bash","arguments":"{\"command\":\"pwd\"}"}}]}}]}"#,
                )
            } else {
                (
                    "503 Service Unavailable",
                    r#"{"error":"model unavailable"}"#,
                )
            };
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let provider = OpenAiCompatibleProvider::new(
        reqwest::Client::new(),
        "ollama",
        "qwen3-coder".into(),
        OpenAiCompatibleDialect::Standard,
        CompatibleAuth::None,
        api_base,
    );
    let messages = [Message::user_text("run pwd")];
    let request = || ModelRequest {
        messages: &messages,
        tools: &[],
        cancellation: Default::default(),
        reasoning_level: crate::reasoning::ReasoningLevel::Off,
        prompt_cache_key: None,
    };

    let response = provider.complete_turn(request()).await.unwrap();
    assert!(matches!(
        response,
        ModelResponse::Assistant(blocks)
            if matches!(blocks.as_slice(), [ContentBlock::ToolCall(call)]
                if call.id == "call-1" && call.name == "bash")
    ));
    let error = provider.complete_turn(request()).await.unwrap_err();
    assert!(matches!(
        error,
        ModelError::HttpStatus { status, body, retry_after: _ }
            if status == reqwest::StatusCode::SERVICE_UNAVAILABLE
                && body.contains("model unavailable")
    ));
    server.await.unwrap();
}
