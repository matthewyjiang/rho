use super::*;
use crate::model::ContentBlock;
use pretty_assertions::assert_eq;
use rho_tools::tool::ToolSpec;
use serde_json::json;

use super::super::auth::Auth;
use super::super::codex_request::ResponsesProfile;

fn api_key_profile(model: &str) -> ResponsesProfile {
    ResponsesProfile::from_auth(&Auth::ApiKey("key".into()), model)
}

fn codex_profile(model: &str) -> ResponsesProfile {
    ResponsesProfile::from_auth(
        &Auth::Codex {
            tokens: crate::credentials::CodexTokens {
                access_token: "token".into(),
                refresh_token: None,
                id_token: None,
                account_id: None,
            },
            source: super::super::auth::CodexAuthSource::Env,
        },
        model,
    )
}

#[test]
fn compact_request_body_is_unary_without_trigger() {
    let profile = api_key_profile("gpt-5.4");
    let body = build_compact_request_body(
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

#[test]
fn compact_request_body_keeps_codex_responses_lite_shape_without_tools() {
    let profile = codex_profile("gpt-5.6-sol");
    let body = build_compact_request_body(
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
    assert!(body
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .all(|item| item.get("type").and_then(Value::as_str) != Some("additional_tools")));
    assert_eq!(
        body["reasoning"],
        json!({"effort": "medium", "summary": "auto", "context": "all_turns"})
    );
}

#[test]
fn extract_compaction_item_requires_exactly_one_valid_item() {
    assert!(extract_compaction_item(&[]).is_err());
    assert!(extract_compaction_item(&[json!({"type": "message"})]).is_err());
    assert!(extract_compaction_item(&[json!({
        "type": "compaction",
        "encrypted_content": ""
    })])
    .is_err());
    assert!(extract_compaction_item(&[
        json!({"type": "compaction", "encrypted_content": "a"}),
        json!({"type": "compaction", "encrypted_content": "b"}),
    ])
    .is_err());

    let item = extract_compaction_item(&[
        json!({"type": "reasoning", "encrypted_content": "r"}),
        json!({"type": "compaction", "encrypted_content": "blob"}),
    ])
    .unwrap();
    assert_eq!(item["encrypted_content"], "blob");
}

#[test]
fn replacement_uses_server_output_users_and_compaction_marker() {
    let identity = ModelIdentity::new("openai", "openai-responses", "gpt-5.4");
    let retained_system_messages = vec![Message::System("system".into())];
    let output = vec![
        json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "recent user"}]
        }),
        json!({
            "type": "compaction",
            "id": "cmp_1",
            "encrypted_content": "blob"
        }),
    ];
    let replacement =
        replacement_from_compact_output(identity.clone(), &retained_system_messages, &output)
            .unwrap();

    assert!(matches!(replacement[0], Message::System(_)));
    assert!(matches!(
        &replacement[1],
        Message::User(blocks) if matches!(
            blocks.as_slice(),
            [ContentBlock::Text(text)] if text == "recent user"
        )
    ));
    let Message::EnrichedAssistant(marker) = replacement.last().unwrap() else {
        panic!("expected compaction marker");
    };
    assert_eq!(marker.provenance.as_ref(), Some(&identity));
    assert!(marker.content.is_empty());
    assert!(marker
        .portable_fallback()
        .is_some_and(|text| text.contains("server-side")));
    let native_context = marker
        .provider_context
        .iter()
        .find(|block| block.kind == COMPACTION_OUTPUT_ITEM_KIND)
        .expect("compaction context");
    assert_eq!(native_context.data["encrypted_content"], "blob");
    assert!(!replacement
        .iter()
        .any(|message| matches!(message, Message::Assistant(_))));
}

#[test]
fn parse_compact_response_reads_usage() {
    let identity = ModelIdentity::new("openai-codex", "openai-responses", "gpt-5.4");
    let body = json!({
        "id": "resp_compact",
        "object": "response.compaction",
        "output": [
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "hi"}]
            },
            {
                "type": "compaction",
                "encrypted_content": "blob"
            }
        ],
        "usage": {
            "input_tokens": 100,
            "output_tokens": 5,
            "total_tokens": 105
        }
    });
    let (messages, usage) = parse_compact_response(identity, &[], &body).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(usage.input_tokens, Some(100));
    assert_eq!(usage.output_tokens, Some(5));
    assert_eq!(usage.total_tokens, Some(105));
}

#[test]
fn parse_compact_response_malformed_output_is_invalid() {
    let identity = ModelIdentity::new("openai", "openai-responses", "gpt-5.4");
    let body = json!({
        "id": "resp_compact",
        "output": {
            "not": "an array"
        }
    });
    let error = parse_compact_response(identity, &[Message::System("sys".into())], &body)
        .expect_err("malformed compact output must fail");
    assert!(matches!(error, ModelError::InvalidResponse(_)));
}

#[tokio::test]
async fn compact_with_http_malformed_retry_response_preserves_failed_attempts() {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
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

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let store = MemoryCredentialStore::default();
    let refreshed = Mutex::new(None);
    let auth = Auth::Codex {
        tokens: CodexTokens {
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            id_token: None,
            account_id: None,
        },
        source: CodexAuthSource::Env,
    };
    let profile = ResponsesProfile::from_auth(&auth, "gpt-5.4");
    let refresh_url = format!("{base}/oauth/token");
    let http = ResponsesHttpTransport::new(&client, &base, &profile, &store, &refreshed)
        .with_codex_refresh_url(&refresh_url);
    let codex_ws = CodexWsTransport::new(&base);
    let messages = [
        Message::System("system".into()),
        Message::user_text("hello"),
        Message::assistant_text("world"),
    ];
    let response = compact_with_http(
        &auth,
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
