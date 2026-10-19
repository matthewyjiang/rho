use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::{
    model::{ContentBlock, Message, ModelEvent, ToolCall, ToolSpec},
    reasoning::ReasoningLevel,
};

fn sample_tool() -> ToolSpec {
    ToolSpec {
        name: "edit".into(),
        description: "test tool".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        }),
    }
}

// Covers: catalog-advertised levels (what pickers offer) gate thinkingLevel for
// Gemini ids the model-id table predates or reads too broadly; Gemini 2.5 keeps
// its budget table.
// Owner: Gemini thinking-config policy
#[test]
fn thinking_follows_catalog_levels() {
    use crate::model::{provider_models::thinking_policy, ReasoningLevelSet};
    use crate::protocol::gemini_generate_content::{ThinkingConfig, ThinkingLevel};

    let catalog = ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
    ]));
    let level = |thinking_level| {
        Ok(Some(ThinkingConfig {
            thinking_budget: None,
            thinking_level: Some(thinking_level),
            include_thoughts: true,
        }))
    };
    for (model, catalog, requested, expected) in [
        // The name table predates gemini-4; the catalog unlocks its levels.
        (
            "gemini-4-argon",
            catalog.clone(),
            ReasoningLevel::High,
            level(ThinkingLevel::High),
        ),
        // Gemini 3+ always thinks: Off is rejected, not sent as the default.
        (
            "gemini-4-argon",
            catalog.clone(),
            ReasoningLevel::Off,
            Err(UnsupportedLevel),
        ),
        (
            "gemini-3.7-flash",
            catalog.clone(),
            ReasoningLevel::Low,
            level(ThinkingLevel::Low),
        ),
        // The name table allows minimal for every 3.x flash; the catalog narrows it.
        (
            "gemini-3.7-flash",
            catalog.clone(),
            ReasoningLevel::Minimal,
            Err(UnsupportedLevel),
        ),
        // Without catalog levels the name table still gates.
        (
            "gemini-3.7-flash",
            ReasoningCapabilities::Unknown,
            ReasoningLevel::Minimal,
            level(ThinkingLevel::Minimal),
        ),
        (
            "gemini-4-argon",
            ReasoningCapabilities::Unknown,
            ReasoningLevel::Off,
            Ok(None),
        ),
        // Gemini 2.5 keeps its budget table whatever the catalog lists.
        (
            "gemini-2.5-flash",
            catalog.clone(),
            ReasoningLevel::Off,
            Ok(Some(ThinkingConfig {
                thinking_budget: Some(0),
                thinking_level: None,
                include_thoughts: false,
            })),
        ),
    ] {
        let thinking = GeminiThinking::resolve(thinking_policy(model), catalog);
        pretty_assertions::assert_eq!(
            thinking.config(requested),
            expected,
            "{model} {requested:?}"
        );
    }
}

#[tokio::test]
async fn complete_turn_uses_google_header_and_generate_content_endpoint() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = socket.read(&mut buffer).await.unwrap();
            request.extend_from_slice(&buffer[..read]);
            let Some(headers_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..headers_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .unwrap_or_default();
            if request.len() >= headers_end + 4 + content_length {
                break;
            }
        }
        let response = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"hello"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1,"totalTokenCount":2}}"#;
        socket.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    let provider = GoogleProvider::new_with_transport(
        "gemini-2.5-pro".into(),
        "secret-key".into(),
        crate::reqwest_client(),
        format!("http://{address}/v1beta"),
    );
    let messages = [Message::user_text("hi")];
    let tools = [sample_tool()];
    let response = provider
        .complete_turn(ModelRequest {
            messages: &messages,
            tools: &tools,
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Medium,
            prompt_cache_key: None,
        })
        .await
        .unwrap();
    let request = server.await.unwrap();

    assert_eq!(
        response,
        ModelResponse::Assistant(vec![ContentBlock::Text("hello".into())])
    );
    assert!(request.starts_with("POST /v1beta/models/gemini-2.5-pro:generateContent HTTP/1.1"));
    assert!(request
        .to_ascii_lowercase()
        .contains("x-goog-api-key: secret-key"));
    assert!(!request.contains("secret-key\""));
    assert!(request.contains("\"parametersJsonSchema\""));
    assert!(request.contains("\"additionalProperties\":false"));
}

#[tokio::test]
async fn stream_turn_parses_sse_and_uses_stream_endpoint() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = socket.read(&mut buffer).await.unwrap();
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let body = concat!(
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hel\"}]}}],\"usageMetadata\":{\"promptTokenCount\":1,\"candidatesTokenCount\":1,\"totalTokenCount\":2}}\n\n",
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"lo\"},{\"functionCall\":{\"id\":\"call-1\",\"name\":\"bash\",\"args\":{\"command\":\"pwd\"}}}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}}\n\n"
        );
        let headers = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        socket.write_all(headers.as_bytes()).await.unwrap();
        let split = body.len() / 2;
        socket.write_all(&body.as_bytes()[..split]).await.unwrap();
        tokio::task::yield_now().await;
        socket.write_all(&body.as_bytes()[split..]).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    let provider = GoogleProvider::new_with_transport(
        "gemini-2.5-flash".into(),
        "secret-key".into(),
        crate::reqwest_client(),
        format!("http://{address}/v1beta"),
    );
    let messages = [Message::user_text("hi")];
    let mut events = Vec::new();
    let response = provider
        .stream_turn(
            ModelRequest {
                messages: &messages,
                tools: &[],
                cancellation: Default::default(),
                reasoning_level: ReasoningLevel::Medium,
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
    let request = server.await.unwrap();

    assert_eq!(
        response,
        ModelResponse::Assistant(vec![
            ContentBlock::Text("hello".into()),
            ContentBlock::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command":"pwd"}),
            }),
        ])
    );
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                ModelEvent::Usage(usage) => usage.total_tokens,
                _ => None,
            })
            .sum::<u64>(),
        5
    );
    assert!(request.starts_with(
        "POST /v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse HTTP/1.1"
    ));
    assert!(request
        .to_ascii_lowercase()
        .contains("accept: text/event-stream"));
}
