use super::*;
use crate::model::{ContentBlock, ModelResponse, ProviderReportedErrorKind};
use serde_json::json;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;

use super::codex_ws_test_support::{
    body, immediate, read_request_frame, send_completion, tokens, ws_server, ws_server_connections,
};

/// Answers one turn, stalls on the next, then answers again on a reconnect.
///
/// Models a turn abandoned mid-flight: the socket is left with an unanswered
/// request, so the next turn must not reuse it.
async fn ws_server_stalls_then_accepts_reconnect() -> (String, Arc<StdMutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let frames = Arc::new(StdMutex::new(Vec::new()));
    let server_frames = Arc::clone(&frames);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let frame = read_request_frame(&mut socket).await;
        server_frames.lock().unwrap().push(frame);
        send_completion(&mut socket, 1).await;
        let frame = read_request_frame(&mut socket).await;
        server_frames.lock().unwrap().push(frame);

        let (stream, _) = listener.accept().await.unwrap();
        let mut reconnected = accept_async(stream).await.unwrap();
        let frame = read_request_frame(&mut reconnected).await;
        server_frames.lock().unwrap().push(frame);
        send_completion(&mut reconnected, 2).await;
        std::future::pending::<()>().await;
    });
    (format!("ws://{addr}/responses"), frames)
}

async fn ws_server_empty_completion(emit_delta: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let _request = socket.next().await.unwrap().unwrap();
        if emit_delta {
            socket
                .send(Message::Text(
                    json!({"type":"response.output_text.delta","delta":"partial"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        }
        socket
            .send(Message::Text(
                json!({
                    "type":"response.completed",
                    "response":{"id":"resp_empty","output":[]}
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
    });
    format!("ws://{addr}/responses")
}

async fn ws_server_waits_for_delta_callback() -> (String, Arc<tokio::sync::Notify>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let delta_observed = Arc::new(tokio::sync::Notify::new());
    let server_delta_observed = Arc::clone(&delta_observed);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let _request = socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(
                json!({"type":"response.output_text.delta","delta":"first"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        server_delta_observed.notified().await;
        socket
            .send(Message::Text(
                json!({
                    "type":"response.completed",
                    "response":{
                        "id":"resp_streaming",
                        "output_text":"first",
                        "output":[],
                        "usage":{"input_tokens":10,"output_tokens":1}
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
    });
    (format!("ws://{addr}/responses"), delta_observed)
}

async fn ws_server_closes_after_delta() -> (String, Arc<StdMutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let frames = Arc::new(StdMutex::new(Vec::new()));
    let server_frames = Arc::clone(&frames);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let message = socket.next().await.unwrap().unwrap();
        let text = message.into_text().unwrap();
        let frame: Value = serde_json::from_str(&text).unwrap();
        server_frames.lock().unwrap().push(frame);
        socket
            .send(Message::Text(
                json!({"type":"response.output_text.delta","delta":"partial"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
    });
    (format!("ws://{addr}/responses"), frames)
}

async fn ws_server_stalls_after_event(events: Vec<Value>) -> (String, Arc<StdMutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let frames = Arc::new(StdMutex::new(Vec::new()));
    let server_frames = Arc::clone(&frames);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let message = socket.next().await.unwrap().unwrap();
        let frame = serde_json::from_str(&message.into_text().unwrap()).unwrap();
        server_frames.lock().unwrap().push(frame);
        for event in events {
            socket
                .send(Message::Text(event.to_string().into()))
                .await
                .unwrap();
        }
        std::future::pending::<()>().await;
    });
    (format!("ws://{addr}/responses"), frames)
}

#[tokio::test]
async fn first_websocket_request_sends_full_input_without_previous_response_id() {
    let (url, frames) = ws_server(1).await;
    let transport = CodexWsTransport::new_with_url(url);
    let mut on_event = None;

    let turn = transport
        .send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            &mut on_event,
        )
        .await
        .unwrap();

    let CodexWsTurn::Completed(response) = turn else {
        panic!("expected websocket completion");
    };
    assert_eq!(response.service_tier.as_deref(), Some("default"));
    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["type"], "response.create");
    assert!(frames[0].get("previous_response_id").is_none());
    assert_eq!(frames[0]["input"], json!([{"role":"user","content":"one"}]));
}

#[tokio::test]
async fn compatible_websocket_request_sends_delta_with_previous_response_id() {
    let (url, frames) = ws_server(2).await;
    let transport = CodexWsTransport::new_with_url(url);
    let mut on_event = None;

    transport
        .send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            &mut on_event,
        )
        .await
        .unwrap();
    let turn = transport
        .send_responses_turn(
            body(vec![
                json!({"role":"user","content":"one"}),
                json!({"role":"assistant","content":"ok1"}),
                json!({"role":"user","content":"three"}),
            ]),
            &tokens(),
            &mut on_event,
        )
        .await
        .unwrap();

    let CodexWsTurn::Completed(CodexSseResponse {
        response: ModelResponse::Assistant(blocks),
        ..
    }) = turn
    else {
        panic!("expected websocket completion");
    };
    assert!(matches!(
        blocks.as_slice(),
        [ContentBlock::Text(text)] if text == "ok2"
    ));
    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1]["previous_response_id"], "resp_1");
    assert_eq!(
        frames[1]["input"],
        json!([{"role":"user","content":"three"}])
    );
}

#[tokio::test]
async fn abandoned_turn_does_not_leave_continuation_state_for_the_next_turn() {
    let (url, frames) = ws_server_stalls_then_accepts_reconnect().await;
    let transport = CodexWsTransport::new_with_url(url);
    let mut on_event = None;

    transport
        .send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            &mut on_event,
        )
        .await
        .unwrap();

    // Drop the second turn part way through, the way cancellation does.
    let abandoned_tokens = tokens();
    let abandoned = transport.send_responses_turn(
        body(vec![
            json!({"role":"user","content":"one"}),
            json!({"role":"assistant","content":"ok1"}),
            json!({"role":"user","content":"two"}),
        ]),
        &abandoned_tokens,
        &mut on_event,
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), abandoned)
            .await
            .is_err(),
        "the stalled turn should still be waiting when it is dropped"
    );

    let turn = immediate(transport.send_responses_turn(
        body(vec![json!({"role":"user","content":"three"})]),
        &tokens(),
        &mut on_event,
    ))
    .await
    .unwrap();

    let CodexWsTurn::Completed(CodexSseResponse {
        response: ModelResponse::Assistant(blocks),
        ..
    }) = turn
    else {
        panic!("expected websocket completion");
    };
    assert!(matches!(
        blocks.as_slice(),
        [ContentBlock::Text(text)] if text == "ok2"
    ));
    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 3);
    // The turn after the abandoned one reconnects and sends full input, because
    // the continuation the abandoned turn might have left is not trustworthy.
    assert_eq!(frames[2].get("previous_response_id"), None);
    assert_eq!(
        frames[2]["input"],
        json!([{"role":"user","content":"three"}])
    );
}

#[tokio::test]
async fn websocket_connection_failure_reports_that_no_model_request_was_submitted() {
    let transport = CodexWsTransport::new_with_url("not a websocket url".into());
    let mut on_event = None;

    let outcome = transport
        .send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            &mut on_event,
        )
        .await
        .unwrap();

    assert!(matches!(
        outcome,
        CodexWsTurn::FullSseFallback {
            request_submitted: false,
            ..
        }
    ));
}

#[test]
fn terminal_failure_uses_error_type_when_code_is_null() {
    for (event, expected_type, expected_kind) in [
        (
            json!({
                "type":"error",
                "error":{
                    "type":"invalid_request_error",
                    "code":null,
                    "message":"invalid request"
                }
            }),
            "invalid_request_error",
            ProviderReportedErrorKind::InvalidResponse,
        ),
        (
            json!({
                "type":"response.failed",
                "response":{
                    "error":{
                        "type":"server_error",
                        "code":null,
                        "message":"server failed"
                    }
                }
            }),
            "server_error",
            ProviderReportedErrorKind::Unavailable,
        ),
        (
            json!({
                "type":"error",
                "error":{
                    "type":"server_is_overloaded",
                    "code":null,
                    "message":"Our servers are currently overloaded. Please try again later."
                }
            }),
            "server_is_overloaded",
            ProviderReportedErrorKind::Unavailable,
        ),
        // Classification prefers `code` over `type`. A permanent invalid-request
        // family with a specific code must stay permanent, not fall through a
        // retryable catch-all keyed only on the code string.
        (
            json!({
                "type":"error",
                "error":{
                    "type":"invalid_request_error",
                    "code":"context_length_exceeded",
                    "message":"Context length exceeded."
                }
            }),
            "context_length_exceeded",
            ProviderReportedErrorKind::InvalidResponse,
        ),
    ] {
        // Route the event through the shared check inside `handle_codex_sse_value`
        // and the websocket `classify_model_error` mapping, the same path a live
        // stream takes.
        let mut state = CodexSseState::default();
        let mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)> =
            None;
        let error =
            handle_codex_sse_value(&event, &mut state, &mut on_event, CodexTransport::WebSocket)
                .expect_err("terminal protocol event must fail the stream");
        assert!(matches!(
            classify_model_error(error, /*events_emitted*/ false),
            CodexWsFailure::Model(ModelError::ProviderReported {
                kind,
                error_type,
                ..
            }) if kind == expected_kind && error_type == expected_type
        ));
    }
}

// Covers: an `error` event carrying the code at the event level (no nested
// `error` object) still routes a stale continuation to the full-SSE fallback
// before any caller-visible output.
// Owner: providers stream parse
#[test]
fn top_level_previous_response_not_found_falls_back_to_sse() {
    let event = json!({
        "type": "error",
        "code": "previous_response_not_found",
        "message": "Previous response not found.",
    });
    let mut state = CodexSseState::default();
    let mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)> = None;
    let error =
        handle_codex_sse_value(&event, &mut state, &mut on_event, CodexTransport::WebSocket)
            .expect_err("terminal protocol event must fail the stream");

    assert!(matches!(
        classify_model_error(error, /*events_emitted*/ false),
        CodexWsFailure::Transport {
            events_emitted: false,
            ..
        }
    ));
}

#[tokio::test]
async fn continuation_error_before_output_returns_immediate_full_sse_fallback() {
    let (url, frames) = ws_server_stalls_after_event(vec![json!({
        "type":"error",
        "error":{
            "type":"invalid_request_error",
            "code":"previous_response_not_found",
            "message":"Previous response not found.",
            "param":"previous_response_id"
        },
        "status":400
    })])
    .await;
    let transport = CodexWsTransport::new_with_url(url);
    let first_body = body(vec![json!({"role":"user","content":"one"})]);
    let candidate = CodexContinuationCandidate::from_responses_body(&first_body).unwrap();
    let continuation_response = CodexContinuationResponse::from_response(
        &ModelResponse::Assistant(vec![ContentBlock::Text("ok1".into())]),
        Some("resp_1".into()),
        vec![json!({
            "id":"msg_1",
            "type":"message",
            "role":"assistant",
            "content":[{"type":"output_text","text":"ok1"}]
        })],
    );
    transport
        .state
        .lock()
        .await
        .continuation
        .record_success(candidate, continuation_response);
    let mut on_event = None;

    let outcome = immediate(transport.send_responses_turn(
        body(vec![
            json!({"role":"user","content":"one"}),
            json!({"role":"assistant","content":"ok1"}),
            json!({"role":"user","content":"two"}),
        ]),
        &tokens(),
        &mut on_event,
    ))
    .await
    .unwrap();

    assert!(matches!(
        outcome,
        CodexWsTurn::FullSseFallback {
            request_submitted: true,
            ..
        }
    ));
    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["previous_response_id"], "resp_1");
    assert_eq!(frames[0]["input"], json!([{"role":"user","content":"two"}]));
}

#[tokio::test]
async fn response_failed_after_delta_returns_immediately_without_replay() {
    let (url, frames) = ws_server_stalls_after_event(vec![
        json!({"type":"response.output_text.delta","delta":"partial"}),
        json!({
            "type":"response.failed",
            "response":{
                "id":"resp_failed",
                "status":"failed",
                "error":{"code":"server_error","message":"generation failed"}
            }
        }),
    ])
    .await;
    let transport = CodexWsTransport::new_with_url(url);
    let mut deltas = Vec::new();
    let error = {
        let mut collect_event = |event| {
            if let ModelEvent::OutputDelta(delta) = event {
                deltas.push(delta);
            }
            Ok(())
        };
        let mut on_event = Some(
            &mut collect_event as &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        );

        immediate(transport.send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            &mut on_event,
        ))
        .await
        .unwrap_err()
    };

    assert_eq!(deltas, ["partial"]);
    assert!(matches!(
        error,
        ModelError::StreamFailedAfterOutput { message }
            if message.contains("server_error") && message.contains("generation failed")
    ));
    assert_eq!(frames.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn silent_response_incomplete_returns_immediate_model_error() {
    let (url, frames) = ws_server_stalls_after_event(vec![json!({
        "type":"response.incomplete",
        "response":{
            "id":"resp_incomplete",
            "status":"incomplete",
            "incomplete_details":{"reason":"max_output_tokens"}
        }
    })])
    .await;
    let transport = CodexWsTransport::new_with_url(url);

    let error = immediate(transport.send_responses_turn_silent(
        body(vec![json!({"role":"user","content":"one"})]),
        &tokens(),
    ))
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        ModelError::ProviderReported {
            kind: ProviderReportedErrorKind::InvalidResponse,
            error_type,
            message,
        } if error_type == "response_incomplete" && message.contains("max_output_tokens")
    ));
    assert_eq!(frames.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn websocket_error_resets_continuation_and_returns_full_sse_fallback() {
    let (url, frames) = ws_server_connections(vec![1, 1]).await;
    let transport = CodexWsTransport::new_with_url(url);
    let mut on_event = None;
    transport
        .send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            &mut on_event,
        )
        .await
        .unwrap();

    let sent = body(vec![
        json!({"role":"user","content":"one"}),
        json!({"role":"user","content":"two"}),
    ]);
    let outcome = transport
        .send_responses_turn(sent.clone(), &tokens(), &mut on_event)
        .await
        .unwrap();

    // The failed turn framed a delta, so the retained full body comes back intact.
    assert_eq!(
        outcome,
        CodexWsTurn::FullSseFallback {
            request_submitted: true,
            body: sent,
        }
    );

    transport
        .send_responses_turn(
            body(vec![
                json!({"role":"user","content":"one"}),
                json!({"role":"user","content":"two"}),
            ]),
            &tokens(),
            &mut on_event,
        )
        .await
        .unwrap();
    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 2);
    assert!(frames[1].get("previous_response_id").is_none());
}

// Covers: an SSE fallback must return the caller's exact body, including for a
// turn whose frame carried that body rather than a delta.
// Owner: Codex WebSocket transport.
#[tokio::test]
async fn full_sse_fallback_returns_the_unframed_request_body() {
    let transport = CodexWsTransport::new_with_url(ws_server_empty_completion(false).await);
    let mut on_event = None;
    let sent = body(vec![json!({"role":"user","content":"one"})]);

    let outcome = transport
        .send_responses_turn(sent.clone(), &tokens(), &mut on_event)
        .await
        .unwrap();

    assert_eq!(
        outcome,
        CodexWsTurn::FullSseFallback {
            request_submitted: true,
            body: sent,
        }
    );
}

#[tokio::test]
async fn empty_websocket_completion_before_output_falls_back_to_sse() {
    let transport = CodexWsTransport::new_with_url(ws_server_empty_completion(false).await);
    let mut on_event = None;

    let outcome = transport
        .send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            &mut on_event,
        )
        .await
        .unwrap();

    assert!(matches!(
        outcome,
        CodexWsTurn::FullSseFallback {
            request_submitted: true,
            ..
        }
    ));
}

#[tokio::test]
async fn empty_websocket_completion_after_output_uses_streamed_output() {
    let transport = CodexWsTransport::new_with_url(ws_server_empty_completion(true).await);
    let mut collect_event = |_event| Ok(());
    let mut on_event =
        Some(&mut collect_event as &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send));

    let outcome = transport
        .send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            &mut on_event,
        )
        .await
        .unwrap();

    assert!(matches!(
        outcome,
        CodexWsTurn::Completed(CodexSseResponse {
            response: ModelResponse::Assistant(blocks),
            ..
        })
            if blocks == vec![ContentBlock::Text("partial".into())]
    ));
}

#[tokio::test]
async fn websocket_emits_delta_before_response_completes() {
    let (url, delta_observed) = ws_server_waits_for_delta_callback().await;
    let transport = CodexWsTransport::new_with_url(url);
    let callback_delta_observed = Arc::clone(&delta_observed);
    let mut collect_event = |event| {
        if matches!(event, ModelEvent::OutputDelta(_)) {
            callback_delta_observed.notify_one();
        }
        Ok(())
    };
    let mut on_event =
        Some(&mut collect_event as &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send));

    transport
        .send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            &mut on_event,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn websocket_failure_after_delta_does_not_replay_request() {
    let (url, frames) = ws_server_closes_after_delta().await;
    let transport = CodexWsTransport::new_with_url(url);
    let mut deltas = Vec::new();
    let error = {
        let mut collect_event = |event| {
            if let ModelEvent::OutputDelta(delta) = event {
                deltas.push(delta);
            }
            Ok(())
        };
        let mut on_event = Some(
            &mut collect_event as &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        );

        transport
            .send_responses_turn(
                body(vec![json!({"role":"user","content":"one"})]),
                &tokens(),
                &mut on_event,
            )
            .await
            .unwrap_err()
    };

    assert_eq!(deltas, ["partial"]);
    assert!(matches!(
        error,
        ModelError::StreamFailedAfterOutput { message }
            if message.contains("websocket")
    ));
    assert_eq!(frames.lock().unwrap().len(), 1);
}

#[test]
fn derives_websocket_url_from_codex_api_base() {
    assert_eq!(
        codex_ws_url("https://chatgpt.com/backend-api/codex"),
        "wss://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        codex_ws_url("http://127.0.0.1:1234/codex/"),
        "ws://127.0.0.1:1234/codex/responses"
    );
}
