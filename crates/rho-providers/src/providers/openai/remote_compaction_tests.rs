use super::*;
use crate::model::{Message, ToolSpec};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::auth::Auth;
use super::super::codex_request::{codex_test_auth, ResponsesProfile};

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

#[tokio::test]
async fn compact_request_body_uses_codex_standard_shape_for_gpt56_models() {
    let profile = codex_profile("gpt-5.6-sol");
    let body = build_responses_compact_body(
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

    assert!(body.get("stream").is_none());
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
    assert!(body.get("parallel_tool_calls").is_none());
    assert_eq!(body["instructions"], "be careful");
    let input = body["input"]
        .as_array()
        .expect("compact request must serialize input as an array");
    assert!(input
        .iter()
        .all(|item| item.get("type").and_then(Value::as_str) != Some("additional_tools")));
    assert!(body
        .get("reasoning")
        .and_then(|value| value.get("context"))
        .is_none());
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

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::super::{
        auth::CodexAuthSource, codex_ws::CodexWsTransport, responses_http::ResponsesHttpTransport,
    };
    use crate::credentials::{CodexTokens, MemoryCredentialStore};

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
        let mut buf = vec![0; 16_384];
        let mut request = Vec::new();
        loop {
            let bytes = stream.read(&mut buf).await.unwrap();
            if bytes == 0 {
                break;
            }
            request.extend_from_slice(&buf[..bytes]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
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
                    // Successful HTTP status with malformed compact JSON body.
                    let body = r#"{"id":"resp","output":{"not":"array"}}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
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
    let http = ResponsesHttpTransport::new(&client, &base).with_codex_refresh_url(&refresh_url);
    let codex_ws = CodexWsTransport::new(&base);
    let messages = [
        Message::System("system".into()),
        Message::user_text("hello"),
        Message::assistant_text("world"),
    ];
    let response = compact_with_http(
        Some(&auth),
        &profile,
        &OpenAiReasoningProfile::unknown(),
        &http,
        &codex_ws,
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
    assert!(result.is_err(), "malformed compact body must fail");
    assert_eq!(failed_attempts.len(), 1);
    assert_eq!(
        failed_attempts[0].kind,
        rho_sdk::ProviderErrorKind::Authentication
    );
}
