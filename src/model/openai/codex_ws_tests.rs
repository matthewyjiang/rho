use super::*;
use crate::model::ContentBlock;
use serde_json::json;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;

fn body(input: Vec<Value>) -> Value {
    json!({
        "model": "gpt-5-codex",
        "instructions": "system",
        "input": input,
        "store": false,
        "stream": true,
        "prompt_cache_key": "rho:session",
        "tools": [{"type":"function","name":"read","parameters":{"type":"object"}}],
        "tool_choice": "auto",
        "reasoning": {"effort":"low","summary":"auto"},
    })
}

fn candidate(input: Vec<Value>) -> CodexContinuationCandidate {
    CodexContinuationCandidate::from_responses_body(&body(input)).unwrap()
}

fn tokens() -> CodexTokens {
    CodexTokens {
        access_token: "token".into(),
        refresh_token: None,
        id_token: None,
        account_id: Some("account".into()),
    }
}

async fn ws_server(expected_messages: usize) -> (String, Arc<StdMutex<Vec<Value>>>) {
    ws_server_connections(vec![expected_messages]).await
}

async fn ws_server_connections(
    expected_messages_by_connection: Vec<usize>,
) -> (String, Arc<StdMutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let frames = Arc::new(StdMutex::new(Vec::new()));
    let server_frames = Arc::clone(&frames);
    tokio::spawn(async move {
        let mut response_index = 0;
        for expected_messages in expected_messages_by_connection {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            for _ in 0..expected_messages {
                response_index += 1;
                let message = socket.next().await.unwrap().unwrap();
                let text = message.into_text().unwrap();
                let frame: Value = serde_json::from_str(&text).unwrap();
                server_frames.lock().unwrap().push(frame);
                let response_id = format!("resp_{response_index}");
                socket
                    .send(Message::Text(
                        json!({"type":"response.output_text.delta","delta":format!("ok{response_index}")})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                socket
                    .send(Message::Text(
                        json!({
                            "type":"response.completed",
                            "response":{
                                "id": response_id,
                                "output_text": format!("ok{response_index}"),
                                "output":[],
                                "usage":{"input_tokens": 10, "output_tokens": 2}
                            }
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
            }
        }
    });
    (format!("ws://{addr}/responses"), frames)
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

async fn ws_server_stalls_after_request() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let _request = socket.next().await.unwrap().unwrap();
        std::future::pending::<()>().await;
    });
    format!("ws://{addr}/responses")
}

#[test]
fn builds_delta_body_when_next_input_extends_previous_input() {
    let first = candidate(vec![json!({"role":"user","content":"one"})]);
    let second = candidate(vec![
        json!({"role":"user","content":"one"}),
        json!({"role":"assistant","content":"two"}),
        json!({"role":"user","content":"three"}),
    ]);
    let mut state = CodexContinuationState::default();
    state.record_success(&first, Some("resp_1".into()));

    let plan = state.plan_delta(&second);

    let CodexContinuationPlan::Delta {
        previous_response_id,
        input,
        body,
    } = plan
    else {
        panic!("expected delta plan");
    };
    assert_eq!(previous_response_id, "resp_1");
    assert_eq!(
        input,
        vec![
            json!({"role":"assistant","content":"two"}),
            json!({"role":"user","content":"three"}),
        ]
    );
    assert_eq!(body["previous_response_id"], "resp_1");
    assert_eq!(body["input"], json!(input));
    assert_eq!(body["model"], "gpt-5-codex");
    assert_eq!(body["prompt_cache_key"], "rho:session");
    assert_eq!(body["tools"][0]["name"], "read");
    assert_eq!(body["reasoning"], json!({"effort":"low","summary":"auto"}));
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
}

#[tokio::test]
async fn responses_lite_websocket_request_sets_lite_client_metadata() {
    let (url, frames) = ws_server(1).await;
    let transport = CodexWsTransport::new_with_url(url);
    let mut on_event = None;

    transport
        .send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            CodexRequestMode::ResponsesLite,
            &mut on_event,
        )
        .await
        .unwrap();

    let frames = frames.lock().unwrap();
    assert_eq!(
        frames[0]["client_metadata"]["ws_request_header_x_openai_internal_codex_responses_lite"],
        "true"
    );
}

#[tokio::test]
async fn responses_lite_websocket_requests_do_not_use_incomplete_continuation_state() {
    let (url, frames) = ws_server(2).await;
    let transport = CodexWsTransport::new_with_url(url);
    let mut on_event = None;

    transport
        .send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            CodexRequestMode::ResponsesLite,
            &mut on_event,
        )
        .await
        .unwrap();
    transport
        .send_responses_turn(
            body(vec![
                json!({"role":"user","content":"one"}),
                json!({"role":"assistant","content":"two"}),
                json!({"role":"user","content":"three"}),
            ]),
            &tokens(),
            CodexRequestMode::ResponsesLite,
            &mut on_event,
        )
        .await
        .unwrap();

    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 2);
    assert!(frames[1].get("previous_response_id").is_none());
    assert_eq!(
        frames[1]["input"],
        json!([
            {"role":"user","content":"one"},
            {"role":"assistant","content":"two"},
            {"role":"user","content":"three"}
        ])
    );
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
            CodexRequestMode::Standard,
            &mut on_event,
        )
        .await
        .unwrap();

    assert!(matches!(turn, CodexWsTurn::Completed(_)));
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
            CodexRequestMode::Standard,
            &mut on_event,
        )
        .await
        .unwrap();
    let turn = transport
        .send_responses_turn(
            body(vec![
                json!({"role":"user","content":"one"}),
                json!({"role":"assistant","content":"two"}),
                json!({"role":"user","content":"three"}),
            ]),
            &tokens(),
            CodexRequestMode::Standard,
            &mut on_event,
        )
        .await
        .unwrap();

    let CodexWsTurn::Completed(ModelResponse::Assistant(blocks)) = turn else {
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
        json!([
            {"role":"assistant","content":"two"},
            {"role":"user","content":"three"}
        ])
    );
}

#[test]
fn falls_back_to_full_request_without_previous_response_id() {
    let state = CodexContinuationState::default();
    let plan = state.plan_delta(&candidate(vec![json!({"role":"user","content":"one"})]));

    assert_eq!(
        plan,
        CodexContinuationPlan::Full {
            reason: CodexContinuationFullReason::MissingPreviousResponse
        }
    );
}

#[test]
fn resets_when_history_is_rewritten_by_compaction() {
    let first = candidate(vec![
        json!({"role":"user","content":"old"}),
        json!({"role":"assistant","content":"answer"}),
    ]);
    let compacted_body = body(vec![
        json!({"role":"user","content":"summary of old conversation"}),
        json!({"role":"user","content":"new"}),
    ]);
    let compacted = CodexContinuationCandidate::from_responses_body(&compacted_body).unwrap();
    let mut state = CodexContinuationState::default();
    state.record_success(&first, Some("resp_1".into()));

    let plan = state.plan_request(&compacted, compacted_body.clone());

    assert!(!plan.planned_delta);
    assert_eq!(
        plan.reset_reason,
        Some(CodexContinuationResetReason::HistoryRewritten)
    );
    assert_eq!(plan.body, compacted_body);
    assert_eq!(
        state.plan_delta(&compacted),
        CodexContinuationPlan::Full {
            reason: CodexContinuationFullReason::MissingPreviousResponse
        }
    );
}

#[test]
fn resets_when_tools_change() {
    let first = candidate(vec![json!({"role":"user","content":"one"})]);
    let mut changed_body = body(vec![
        json!({"role":"user","content":"one"}),
        json!({"role":"user","content":"two"}),
    ]);
    changed_body["tools"] = json!([{ "type":"function", "name":"write" }]);
    let changed = CodexContinuationCandidate::from_responses_body(&changed_body).unwrap();
    let mut state = CodexContinuationState::default();
    state.record_success(&first, Some("resp_1".into()));

    let plan = state.plan_request(&changed, changed_body);

    assert_eq!(
        plan.reset_reason,
        Some(CodexContinuationResetReason::ToolsChanged)
    );
    assert_eq!(
        state.plan_delta(&changed),
        CodexContinuationPlan::Full {
            reason: CodexContinuationFullReason::MissingPreviousResponse
        }
    );
}

#[test]
fn resets_when_model_changes() {
    assert_reset_reason_for_body_change(
        |body| body["model"] = json!("gpt-5-codex-alt"),
        CodexContinuationResetReason::ModelChanged,
    );
}

#[test]
fn resets_when_reasoning_changes() {
    assert_reset_reason_for_body_change(
        |body| body["reasoning"] = json!({"effort":"high","summary":"auto"}),
        CodexContinuationResetReason::ReasoningChanged,
    );
}

#[test]
fn resets_when_prompt_cache_key_changes() {
    assert_reset_reason_for_body_change(
        |body| body["prompt_cache_key"] = json!("rho:other"),
        CodexContinuationResetReason::PromptCacheKeyChanged,
    );
}

#[test]
fn resets_when_tool_choice_changes() {
    assert_reset_reason_for_body_change(
        |body| body["tool_choice"] = json!("none"),
        CodexContinuationResetReason::ToolChoiceChanged,
    );
}

fn assert_reset_reason_for_body_change(
    mutate: impl FnOnce(&mut Value),
    expected: CodexContinuationResetReason,
) {
    let first = candidate(vec![json!({"role":"user","content":"one"})]);
    let mut changed_body = body(vec![
        json!({"role":"user","content":"one"}),
        json!({"role":"user","content":"two"}),
    ]);
    mutate(&mut changed_body);
    let changed = CodexContinuationCandidate::from_responses_body(&changed_body).unwrap();
    let mut state = CodexContinuationState::default();
    state.record_success(&first, Some("resp_1".into()));

    let plan = state.plan_request(&changed, changed_body);

    assert_eq!(plan.reset_reason, Some(expected));
}

#[tokio::test]
async fn stalled_websocket_falls_back_instead_of_blocking_the_turn() {
    let url = ws_server_stalls_after_request().await;
    let mut transport = CodexWsTransport::new_with_url(url);
    transport.idle_timeout = std::time::Duration::from_millis(10);
    let mut on_event = None;

    let outcome = transport
        .send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            CodexRequestMode::Standard,
            &mut on_event,
        )
        .await
        .unwrap();

    assert!(matches!(outcome, CodexWsTurn::FullSseFallback));
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
            CodexRequestMode::Standard,
            &mut on_event,
        )
        .await
        .unwrap();

    let outcome = transport
        .send_responses_turn(
            body(vec![
                json!({"role":"user","content":"one"}),
                json!({"role":"user","content":"two"}),
            ]),
            &tokens(),
            CodexRequestMode::Standard,
            &mut on_event,
        )
        .await
        .unwrap();

    assert!(matches!(outcome, CodexWsTurn::FullSseFallback));

    transport
        .send_responses_turn(
            body(vec![
                json!({"role":"user","content":"one"}),
                json!({"role":"user","content":"two"}),
            ]),
            &tokens(),
            CodexRequestMode::Standard,
            &mut on_event,
        )
        .await
        .unwrap();
    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 2);
    assert!(frames[1].get("previous_response_id").is_none());
}

#[tokio::test]
async fn websocket_fallback_does_not_emit_partial_events() {
    let (url, frames) = ws_server_closes_after_delta().await;
    let transport = CodexWsTransport::new_with_url(url);
    let mut deltas = Vec::new();
    {
        let mut collect_event = |event| {
            if let ModelEvent::OutputDelta(delta) = event {
                deltas.push(delta);
            }
            Ok(())
        };
        let mut on_event =
            Some(&mut collect_event as &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>);

        let outcome = transport
            .send_responses_turn(
                body(vec![json!({"role":"user","content":"one"})]),
                &tokens(),
                CodexRequestMode::Standard,
                &mut on_event,
            )
            .await
            .unwrap();

        assert!(matches!(outcome, CodexWsTurn::FullSseFallback));
    }

    assert!(deltas.is_empty());
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
