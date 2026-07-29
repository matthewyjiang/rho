use std::sync::Arc;

use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::*;
use crate::{
    credentials::{save_xai_tokens, MemoryCredentialStore, XaiTokens},
    model::{Message, ToolSpec},
    reasoning::ReasoningLevel,
};

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
        /*hosted_web_search*/ true,
    )
    .unwrap();

    assert!(body.get("reasoning").is_none());
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["tools"], json!([{ "type": "x_search" }]));
    assert_eq!(body["tool_choice"], "auto");
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
        /*hosted_web_search*/ true,
    )
    .unwrap();

    assert_eq!(body["model"], "grok-4.5");
    assert_eq!(body["instructions"], "follow instructions");
    assert_eq!(body["input"][0]["role"], "user");
    assert_eq!(
        body["tools"],
        json!([
            {
                "type": "function",
                "name": "read_file",
                "description": "read a file",
                "parameters": {"type": "object"},
                "strict": false,
            },
            { "type": "x_search" },
        ])
    );
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["prompt_cache_key"], "rho:session");
    assert_eq!(body["reasoning"], json!({"effort": "high"}));
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
}

#[test]
fn responses_body_uses_hosted_web_search_and_adds_hosted_x_search() {
    let tools = [
        ToolSpec {
            name: "web_search".into(),
            description: "search the web".into(),
            input_schema: json!({"type": "object"}),
        },
        ToolSpec {
            name: "x_search".into(),
            description: "ignored client form".into(),
            input_schema: json!({"type": "object"}),
        },
    ];
    let profile = reasoning::XaiReasoningProfile::from_metadata("grok-4.5", None);
    let body = build_xai_responses_body(
        "xai",
        "grok-4.5",
        &profile,
        ModelRequest {
            messages: &[Message::user_text("what are people saying?")],
            tools: &tools,
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Off,
            prompt_cache_key: None,
        },
        /*hosted_web_search*/ true,
    )
    .unwrap();

    assert_eq!(
        body["tools"],
        json!([
            { "type": "web_search" },
            { "type": "x_search" },
        ])
    );
}

#[test]
fn responses_body_keeps_function_web_search_when_hosted_disabled() {
    let tools = [ToolSpec {
        name: "web_search".into(),
        description: "search the web".into(),
        input_schema: json!({"type": "object"}),
    }];
    let profile = reasoning::XaiReasoningProfile::from_metadata("grok-4.5", None);
    let body = build_xai_responses_body(
        "xai",
        "grok-4.5",
        &profile,
        ModelRequest {
            messages: &[Message::user_text("find docs")],
            tools: &tools,
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Medium,
            prompt_cache_key: None,
        },
        /*hosted_web_search*/ false,
    )
    .unwrap();

    assert_eq!(
        body["tools"],
        json!([
            {
                "type": "function",
                "name": "web_search",
                "description": "search the web",
                "parameters": {"type": "object"},
                "strict": false,
            },
            { "type": "x_search" },
        ])
    );
}

#[test]
fn responses_body_always_includes_hosted_x_search() {
    let profile = reasoning::XaiReasoningProfile::from_metadata("grok-4.5", None);
    let body = build_xai_responses_body(
        "xai",
        "grok-4.5",
        &profile,
        ModelRequest {
            messages: &[Message::user_text("hello")],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Off,
            prompt_cache_key: None,
        },
        /*hosted_web_search*/ true,
    )
    .unwrap();

    assert_eq!(body["tools"], json!([{ "type": "x_search" }]));
    assert_eq!(body["tool_choice"], "auto");
}

#[test]
fn compact_body_is_model_and_full_input_only() {
    let messages = [
        Message::System("be helpful".into()),
        Message::user_text("hello"),
    ];
    let tools = [ToolSpec {
        name: "read_file".into(),
        description: "read a file".into(),
        input_schema: json!({"type": "object"}),
    }];
    let body = build_xai_compact_body(
        "xai",
        "grok-4.5",
        ModelRequest {
            messages: &messages,
            tools: &tools,
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::High,
            prompt_cache_key: Some("session-1"),
        },
    )
    .unwrap();

    assert_eq!(body["model"], "grok-4.5");
    assert_eq!(body["input"][0]["role"], "system");
    assert_eq!(body["input"][0]["content"], "be helpful");
    assert_eq!(body["input"][1]["role"], "user");
    // Documented compact body is only model + input.
    assert!(body.get("instructions").is_none());
    assert!(body.get("store").is_none());
    assert!(body.get("prompt_cache_key").is_none());
    assert!(body.get("reasoning").is_none());
    assert!(body.get("stream").is_none());
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
    assert!(body.get("include").is_none());
    let keys: std::collections::BTreeSet<_> = body.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        ["input".to_string(), "model".to_string()]
            .into_iter()
            .collect()
    );
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

#[tokio::test]
async fn native_compact_posts_to_responses_compact_and_returns_marker() {
    use rho_sdk::provider::ModelProvider;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 8192];
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
        assert!(request.starts_with("POST /responses/compact HTTP/1.1\r\n"));
        assert!(request.contains("authorization: Bearer access-token\r\n"));
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert!(body.get("stream").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("instructions").is_none());
        assert!(body.get("store").is_none());
        assert!(body.get("reasoning").is_none());
        assert_eq!(body["model"], "grok-4.5");
        assert_eq!(body["input"][0]["role"], "system");
        assert_eq!(body["input"][0]["content"], "system");
        assert_eq!(body["input"][1]["role"], "user");
        let keys: std::collections::BTreeSet<_> =
            body.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            ["input".to_string(), "model".to_string()]
                .into_iter()
                .collect()
        );

        let response_body = r#"{"id":"cmp_1","object":"response.compaction","output":[{"type":"compaction","id":"cmp_1","encrypted_content":"blob"}],"usage":{"input_tokens":12,"output_tokens":3,"total_tokens":15}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response_body}",
            response_body.len()
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
    let messages = [
        Message::System("system".into()),
        Message::user_text("hello"),
        Message::assistant_text("world"),
    ];
    let response = provider
        .native_compact(ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        })
        .expect("xAI exposes native compact")
        .await;
    let (result, failed_attempts) = response.into_parts();
    assert!(failed_attempts.is_empty());
    let output = result.expect("compact succeeded");
    let replacement = output.messages();
    assert!(matches!(&replacement[0], Message::System(text) if text == "system"));
    let Message::EnrichedAssistant(marker) = replacement.last().unwrap() else {
        panic!("expected compaction marker");
    };
    assert_eq!(marker.provider_context[0].data["encrypted_content"], "blob");
    assert!(marker.portable_fallback().is_some_and(|text| {
        text.contains("xAI server-side") && !text.contains("Retained recent user messages")
    }));
    server.await.unwrap();
}

#[tokio::test]
async fn native_compact_unauthorized_without_refresh_fails() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc as StdArc,
    };

    use rho_sdk::provider::ModelProvider;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let compact_hits = StdArc::new(AtomicUsize::new(0));
    let server_hits = StdArc::clone(&compact_hits);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            let count = stream.read(&mut chunk).await.unwrap();
            if count == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..count]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let _ = server_hits.fetch_add(1, Ordering::SeqCst);
        let response =
            "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        stream.write_all(response.as_bytes()).await.unwrap();
        let _ = stream.shutdown().await;
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
        .native_compact(ModelRequest {
            messages: &[Message::user_text("hello")],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        })
        .expect("xAI exposes native compact")
        .await;
    let (result, failed_attempts) = response.into_parts();
    assert!(result.is_err());
    assert_eq!(compact_hits.load(Ordering::SeqCst), 1);
    // No-refresh 401 is the final response; it is not a prior failed attempt.
    assert!(failed_attempts.is_empty());
    server.await.unwrap();
}
