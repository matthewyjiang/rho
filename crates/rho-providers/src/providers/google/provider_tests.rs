use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::{
    model::{ContentBlock, Message, ModelEvent, ToolCall, ToolSpec},
    reasoning::ReasoningLevel,
};

fn sample_tool() -> ToolSpec {
    ToolSpec {
        name: "edit_file".into(),
        description: "test tool".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        }),
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
        reqwest::Client::new(),
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
        reqwest::Client::new(),
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

#[test]
fn reasoning_uses_levels_for_gemini_3_and_bounded_budgets_for_gemini_2_5() {
    assert_eq!(
        thinking_config("gemini-3-flash", ReasoningLevel::High).unwrap(),
        Some(ThinkingConfig {
            thinking_budget: None,
            thinking_level: Some(ThinkingLevel::High),
            include_thoughts: true,
        })
    );
    assert_eq!(
        thinking_config("gemini-2.5-flash", ReasoningLevel::Max).unwrap(),
        Some(ThinkingConfig {
            thinking_budget: Some(24_576),
            thinking_level: None,
            include_thoughts: true,
        })
    );
    assert!(matches!(
        thinking_config("gemini-3-pro", ReasoningLevel::Off),
        Err(ModelError::UnsupportedReasoning { .. })
    ));
    assert!(matches!(
        thinking_config("gemini-3-pro-preview", ReasoningLevel::Medium),
        Err(ModelError::UnsupportedReasoning { .. })
    ));
    assert_eq!(
        thinking_config("gemini-3.1-pro-preview", ReasoningLevel::Medium).unwrap(),
        Some(ThinkingConfig {
            thinking_budget: None,
            thinking_level: Some(ThinkingLevel::Medium),
            include_thoughts: true,
        })
    );
    assert_eq!(
        thinking_config("gemini-2.0-flash", ReasoningLevel::Off).unwrap(),
        None
    );
    assert!(matches!(
        thinking_config("gemini-2.0-flash", ReasoningLevel::Medium),
        Err(ModelError::UnsupportedReasoning { .. })
    ));
}
