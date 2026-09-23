use super::*;
use crate::model::{Message, ToolSpec};
use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use super::super::auth::Auth;
use super::super::codex_request::{codex_test_auth, NativeCompactionWire, ResponsesProfile};

fn api_key_profile(model: &str) -> ResponsesProfile {
    ResponsesProfile::from_auth(&Auth::ApiKey("key".into()), model)
}

fn codex_profile(model: &str) -> ResponsesProfile {
    ResponsesProfile::from_auth(&codex_test_auth(), model)
}

#[tokio::test]
async fn compact_request_body_is_unary_without_trigger() {
    let profile = api_key_profile("gpt-5.4");
    let body = build_responses_compact_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        ModelRequest {
            messages: &[
                Message::System("be helpful".into()),
                Message::user_text("hello"),
            ],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: Some("session-1"),
        },
    )
    .unwrap();

    let input = body["input"].as_array().unwrap();
    assert!(input
        .iter()
        .all(|item| item.get("type").and_then(Value::as_str) != Some("compaction_trigger")));
    assert!(body.get("stream").is_none());
    assert_eq!(body["store"], false);
    assert_eq!(body["prompt_cache_key"], "session-1");
    assert!(body.get("tools").is_none());
    assert!(body.get("additional_tools").is_none());
    assert!(body.get("tool_choice").is_none());
    assert!(body.get("parallel_tool_calls").is_none());
}

// Covers: Codex compaction must stream a create body ending in
// compaction_trigger; the backend 404s /responses/compact.
// Owner: OpenAI native compaction wire
#[tokio::test]
async fn codex_compaction_body_streams_trailing_trigger_without_tools() {
    let profile = codex_profile("gpt-5.6-sol");
    assert_eq!(
        profile.contract().native_compaction(),
        NativeCompactionWire::CompactionTrigger
    );
    let body = build_compaction_trigger_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        ModelRequest {
            messages: &[
                Message::System("be careful".into()),
                Message::user_text("hello"),
            ],
            tools: &[ToolSpec {
                name: "bash".into(),
                description: "run a command".into(),
                input_schema: json!({"type": "object"}),
            }],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
    )
    .unwrap();

    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
    assert!(body.get("parallel_tool_calls").is_none());
    assert_eq!(body["instructions"], "be careful");
    let input = body["input"]
        .as_array()
        .expect("compact request must serialize input as an array");
    assert_eq!(input.last(), Some(&json!({"type": "compaction_trigger"})));
}

// Covers: compact replacement history preserves the effort used to create the
// encrypted artifact, so a later effort change stays out of the cached prefix.
// Owner: OpenAI native compaction
#[test]
fn astra_compaction_preserves_reasoning_baseline() {
    let profile = codex_profile("gpt-6-astra");
    let identity = profile.identity().clone();
    let compact = build_responses_compact_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        ModelRequest {
            messages: &[Message::user_text("before compact")],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: crate::reasoning::ReasoningLevel::Low,
            prompt_cache_key: None,
        },
    )
    .unwrap();
    let context = compact_assistant_context(&identity, &compact);
    let (mut replacement, _) = crate::protocol::openai_responses::parse_compact_response(
        identity,
        &[],
        &json!({
            "output": [{
                "type": "compaction",
                "encrypted_content": "opaque"
            }]
        }),
        PORTABLE_HANDOFF_NOTICE,
        CompactUserRetention::KeepServerUsers,
        &context,
    )
    .unwrap();
    replacement.push(Message::user_text("after compact"));

    let created = super::super::codex_request::build_responses_create_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        ModelRequest {
            messages: &replacement,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: crate::reasoning::ReasoningLevel::High,
            prompt_cache_key: None,
        },
        None,
        /*hosted_web_search*/ true,
        /*async_tools*/ &Default::default(),
    )
    .unwrap();

    assert_eq!(created.body["reasoning"]["effort"], "low");
    assert!(created.body["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item.get("type").and_then(Value::as_str) == Some("configuration_update")
                && item["reasoning"]["effort"] == "high"
        }));
}

/// Reads one HTTP/1.1 request (headers plus `content-length` body).
async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut buf = vec![0; 16_384];
    let mut request = Vec::new();
    loop {
        let bytes = stream.read(&mut buf).await.unwrap();
        if bytes == 0 {
            break;
        }
        request.extend_from_slice(&buf[..bytes]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = &request[..header_end + 4];
        let headers_text = String::from_utf8_lossy(headers);
        let content_length = headers_text
            .lines()
            .find_map(|line| {
                let lower = line.to_ascii_lowercase();
                lower
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap_or(0))
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    request
}

#[tokio::test]
async fn compact_with_http_malformed_retry_response_preserves_failed_attempts() {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    use tokio::{io::AsyncWriteExt, net::TcpListener};

    use super::super::{auth::CodexAuthSource, codex_ws::CodexWsTransport};
    use crate::credentials::{CodexTokens, MemoryCredentialStore};
    use crate::providers::responses_http::ResponsesHttpTransport;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let compact_hits = Arc::new(AtomicUsize::new(0));
    let server_compact_hits = Arc::clone(&compact_hits);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let raw = read_http_request(&mut stream).await;
            let raw = String::from_utf8_lossy(&raw);
            let (headers, _body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_ref(), ""));
            let path = headers
                .lines()
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            let response = if path.contains("oauth/token") {
                let body = r#"{"access_token":"access-2","refresh_token":"refresh-2"}"#;
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
            } else {
                let n = server_compact_hits.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        .into()
                } else {
                    // Successful HTTP status with a stream that has no compaction item.
                    let body =
                        "data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n";
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                }
            };
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });

    let client = crate::reqwest_client_builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let auth = Auth::codex(
        CodexTokens {
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            id_token: None,
            account_id: None,
        },
        CodexAuthSource::Env,
        std::sync::Arc::new(MemoryCredentialStore::default()),
    );
    let profile = ResponsesProfile::from_auth(&auth, "gpt-5.4");
    let refresh_url = format!("{base}/oauth/token");
    let http = ResponsesHttpTransport::new(&client, &base);
    let codex_ws = CodexWsTransport::new(&base);
    let messages = [
        Message::System("system".into()),
        Message::user_text("hello"),
        Message::assistant_text("world"),
    ];
    let response = compact_with_http(
        CompactHttp {
            auth: Some(&auth),
            profile: &profile,
            reasoning_profile: &OpenAiReasoningProfile::unknown(),
            http: &http,
            client: &client,
            refresh_url: &refresh_url,
            codex_ws: &codex_ws,
        },
        ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
    )
    .await;

    let (result, failed_attempts) = response.into_parts();
    assert!(
        result.is_err(),
        "compaction stream without an item must fail"
    );
    assert_eq!(failed_attempts.len(), 1);
    assert_eq!(
        failed_attempts[0].kind,
        rho_sdk::ProviderErrorKind::Authentication
    );
}

// Covers: Codex trigger compaction parses the streamed compaction item and
// keeps system plus recent user turns, since the server returns no users.
// Owner: OpenAI native compaction wire
#[tokio::test]
async fn codex_trigger_compaction_streams_item_and_retains_users() {
    use std::time::Duration;

    use tokio::{io::AsyncWriteExt, net::TcpListener, sync::oneshot};

    use super::super::codex_ws::CodexWsTransport;
    use crate::model::ContentBlock;
    use crate::providers::responses_http::ResponsesHttpTransport;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (request_tx, request_rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let raw = read_http_request(&mut stream).await;
        let raw = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        let path = head.split_whitespace().nth(1).unwrap().to_string();
        let body: Value = serde_json::from_str(body).unwrap();
        request_tx.send((path, body)).unwrap();
        let compaction = json!({"id": "cmp_1", "type": "compaction", "encrypted_content": "blob"});
        let events = [
            json!({"type": "response.created", "response": {"id": "resp_1"}}),
            json!({"type": "response.output_item.done", "output_index": 0, "item": compaction}),
            json!({"type": "response.completed", "response": {
                "id": "resp_1",
                "status": "completed",
                "output": [],
                "usage": {"input_tokens": 51, "output_tokens": 42, "total_tokens": 93}
            }}),
        ];
        let sse = events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
            sse.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
    });

    let client = crate::reqwest_client_builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let auth = codex_test_auth();
    let profile = ResponsesProfile::from_auth(&auth, "gpt-6-astra");
    let http = ResponsesHttpTransport::new(&client, &base);
    let codex_ws = CodexWsTransport::new(&base);
    let messages = [
        Message::System("system".into()),
        Message::user_text("first ask"),
        Message::assistant_text("did it"),
        Message::user_text("second ask"),
        Message::assistant_text("did that too"),
    ];
    let response = compact_with_http(
        CompactHttp {
            auth: Some(&auth),
            profile: &profile,
            reasoning_profile: &OpenAiReasoningProfile::unknown(),
            http: &http,
            client: &client,
            refresh_url: "http://127.0.0.1:9/unused",
            codex_ws: &codex_ws,
        },
        ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
    )
    .await;

    let (path, body) = request_rx.await.unwrap();
    assert_eq!(path, "/responses");
    assert_eq!(
        body["input"].as_array().unwrap().last(),
        Some(&json!({"type": "compaction_trigger"}))
    );

    let (result, failed_attempts) = response.into_parts();
    assert!(failed_attempts.is_empty());
    let output = result.expect("trigger compaction succeeds");
    let messages = output.messages();
    assert_eq!(
        messages[..3],
        [
            Message::System("system".into()),
            Message::User(vec![ContentBlock::Text("first ask".into())]),
            Message::User(vec![ContentBlock::Text("second ask".into())]),
        ]
    );
    let [.., Message::EnrichedAssistant(marker)] = messages else {
        panic!("expected compaction marker last, got {messages:?}");
    };
    assert_eq!(
        marker.provider_context[0].data,
        json!({"id": "cmp_1", "type": "compaction", "encrypted_content": "blob"})
    );
    assert_eq!(messages.len(), 4);
    assert_eq!(output.usage().output_tokens, Some(42));
    assert_eq!(output.usage().total_tokens, Some(93));
}
