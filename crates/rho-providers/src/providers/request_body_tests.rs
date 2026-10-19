use std::io::Read;

use flate2::read::GzDecoder;
use pretty_assertions::assert_eq;
use reqwest::header::{HeaderValue, ACCEPT, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::encoding::{encode_request, supports_gzip, MIN_COMPRESSED_BYTES};
use super::{EndpointCompression, RequestCompression};

// Covers: a shared JSON dialect must not enable gzip on an incompatible endpoint.
// Owner: provider HTTP request encoding; existing provider fixtures use local overrides.
#[test]
fn compression_support_is_scoped_to_verified_endpoints() {
    for (url, expected) in [
        ("https://api.anthropic.com/v1/messages", true),
        ("https://api.anthropic.com/v1/messages?beta=true", true),
        ("https://api.anthropic.com/v1/messages/count_tokens", false),
        ("http://api.anthropic.com/v1/messages", false),
        ("https://api.anthropic.com:8443/v1/messages", false),
        ("https://api.anthropic.com.example/v1/messages", false),
        ("https://proxy.example/v1/messages", false),
        ("https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent", true),
        ("https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse", true),
        ("https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:countTokens", false),
        ("https://generativelanguage.googleapis.com/v1beta/models/:generateContent", false),
        ("https://generativelanguage.googleapis.com/unknown/models/gemini-2.5-flash:generateContent", false),
        ("https://api.openai.com/v1/responses", false),
        ("https://chatgpt.com/backend-api/codex/responses", false),
        ("https://api.x.ai/v1/responses", true),
        ("https://api.x.ai/v1/responses/compact", true),
        ("https://api.x.ai/v1/chat/completions", true),
        ("https://api.x.ai/v1/models", false),
        ("https://api.githubcopilot.com/chat/completions", false),
        ("https://openrouter.ai/api/v1/chat/completions", false),
        ("http://localhost:11434/v1/chat/completions", false),
        ("https://ollama.com/v1/chat/completions", false),
        ("https://api.moonshot.ai/v1/chat/completions", false),
        ("https://api.kimi.com/coding/v1/chat/completions", false),
        ("https://inference.poolside.ai/v1/chat/completions", false),
        ("https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1/chat/completions", false),
        ("https://api.meta.ai/v1/chat/completions", false),
        ("https://opencode.ai/inference/go/v1/messages", false),
        ("https://api.minimax.io/anthropic/v1/messages", false),
        ("https://api.typesafe.ai/v1/chat/completions", false),
        ("https://api.typesafe.ai/v1/systemone", true),
        ("http://node0:8317/v1/responses", false),
    ] {
        assert_eq!(supports_gzip(&url.parse().unwrap()), expected, "{url}");
    }
}

// Covers: each encoding must round-trip the full serialized conversation and
// retain SSE/auth headers, a configured encoding must apply to the user's own
// host, and small or incompressible requests must not grow on the wire.
// Owner: provider HTTP request encoding; no existing test exercises binary bodies.
#[test]
fn encoded_body_round_trips_and_preserves_request_semantics() {
    let conversation = serde_json::to_vec(&json!({
        "messages": [{"role": "user", "content": "read src/control.rs — 状態 🦀\n".repeat(2000)}],
        "tools": [{"name": "read_file", "input_schema": {"type": "object"}}],
        "stream": true
    }))
    .unwrap();
    let mut incompressible = Vec::new();
    for index in 0..(MIN_COMPRESSED_BYTES / 32 + 1) {
        incompressible.extend_from_slice(&Sha256::digest(index.to_le_bytes()));
    }
    let endpoints = [
        (
            "https://api.anthropic.com/v1/messages",
            EndpointCompression::Verified,
            RequestCompression::Gzip,
        ),
        (
            "http://node0:8317/v1/responses",
            EndpointCompression::Configured(RequestCompression::Zstd),
            RequestCompression::Zstd,
        ),
        (
            "https://proxy.example/v1/chat/completions",
            EndpointCompression::Configured(RequestCompression::Gzip),
            RequestCompression::Gzip,
        ),
    ];
    for (url, policy, encoding) in endpoints {
        for (bytes, compressed) in [
            (vec![b'a'; MIN_COMPRESSED_BYTES - 1], false),
            (vec![b'a'; MIN_COMPRESSED_BYTES], true),
            (conversation.clone(), true),
            (incompressible.clone(), false),
        ] {
            let mut request = crate::reqwest_client()
                .post(url)
                .header(CONTENT_TYPE, "application/json")
                .header(CONTENT_LENGTH, bytes.len())
                .header(ACCEPT, "text/event-stream")
                .header("x-api-key", "test-secret")
                .body(bytes.clone())
                .build()
                .unwrap();
            let original_headers = request.headers().clone();
            encode_request(&mut request, policy).unwrap();
            let wire_body = request.body().unwrap().as_bytes().unwrap();
            let mut expected_headers = original_headers;
            if compressed {
                assert_eq!(decode(encoding, wire_body), bytes, "{url}");
                assert!(wire_body.len() < bytes.len());
                expected_headers.remove(CONTENT_LENGTH);
                expected_headers.insert(
                    CONTENT_ENCODING,
                    HeaderValue::from_static(encoding.as_str()),
                );
            } else {
                assert_eq!(wire_body, bytes, "{url}");
            }
            assert_eq!(request.headers(), &expected_headers, "{url}");
            // A rebuilt/retried encoded request must not be compressed twice.
            let once = wire_body.to_vec();
            encode_request(&mut request, policy).unwrap();
            assert_eq!(request.body().unwrap().as_bytes().unwrap(), once);
        }
    }
}

fn decode(encoding: RequestCompression, body: &[u8]) -> Vec<u8> {
    match encoding {
        RequestCompression::Gzip => {
            let mut decoded = Vec::new();
            GzDecoder::new(body).read_to_end(&mut decoded).unwrap();
            decoded
        }
        RequestCompression::Zstd => zstd::decode_all(body).unwrap(),
    }
}

// Covers: endpoint overrides and pre-encoded or streaming bodies retain their
// original framing instead of acquiring an invalid gzip label.
// Owner: provider HTTP request encoding.
#[test]
fn ineligible_requests_are_unchanged() {
    let client = crate::reqwest_client();
    let body = vec![b'a'; MIN_COMPRESSED_BYTES * 2];
    for builder in [
        client
            .post("https://proxy.example/v1/messages")
            .json(&json!({"text": "a".repeat(MIN_COMPRESSED_BYTES)})),
        client
            .post("https://api.anthropic.com/v1/messages")
            .header(CONTENT_ENCODING, "br")
            .header(CONTENT_TYPE, "application/json")
            .body(body.clone()),
        client
            .post("https://api.anthropic.com/v1/messages")
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(body),
        client
            .post("https://api.anthropic.com/v1/messages")
            .header(CONTENT_TYPE, "application/json")
            .body(reqwest::Body::wrap_stream(futures_util::stream::once(
                async { Ok::<_, std::io::Error>(vec![b'a'; MIN_COMPRESSED_BYTES]) },
            ))),
    ] {
        let mut request = builder.build().unwrap();
        let headers = request.headers().clone();
        let bytes = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .map(<[u8]>::to_vec);
        encode_request(&mut request, EndpointCompression::Verified).unwrap();
        assert_eq!(request.headers(), &headers);
        assert_eq!(
            request
                .body()
                .and_then(reqwest::Body::as_bytes)
                .map(<[u8]>::to_vec),
            bytes
        );
    }
}
