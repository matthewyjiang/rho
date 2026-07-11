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
                            "type":"response.output_item.done",
                            "item":{
                                "id": format!("msg_{response_index}"),
                                "type":"message",
                                "role":"assistant",
                                "content":[{"type":"output_text","text":format!("ok{response_index}")}]
                            }
                        })
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

async fn ws_server_streams_delta_before_completion() -> (String, Arc<std::sync::atomic::AtomicBool>)
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let server_completed = Arc::clone(&completed);
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
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        server_completed.store(true, std::sync::atomic::Ordering::SeqCst);
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
    (format!("ws://{addr}/responses"), completed)
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

async fn ws_server_sends_keep_alive_frames() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let _request = socket.next().await.unwrap().unwrap();
        loop {
            socket.send(Message::Ping(Vec::new().into())).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });
    format!("ws://{addr}/responses")
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
                json!({"role":"assistant","content":"ok1"}),
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
            {"role":"assistant","content":"ok1"},
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
                json!({"role":"assistant","content":"ok1"}),
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
        json!([{"role":"user","content":"three"}])
    );
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
async fn websocket_keep_alive_frames_do_not_reset_the_idle_timeout() {
    let url = ws_server_sends_keep_alive_frames().await;
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
async fn websocket_emits_delta_before_response_completes() {
    let (url, completed) = ws_server_streams_delta_before_completion().await;
    let transport = CodexWsTransport::new_with_url(url);
    let callback_ran_before_completion = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let callback_observation = Arc::clone(&callback_ran_before_completion);
    let mut collect_event = |event| {
        if matches!(event, ModelEvent::OutputDelta(_)) {
            callback_observation.store(
                !completed.load(std::sync::atomic::Ordering::SeqCst),
                std::sync::atomic::Ordering::SeqCst,
            );
        }
        Ok(())
    };
    let mut on_event =
        Some(&mut collect_event as &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>);

    transport
        .send_responses_turn(
            body(vec![json!({"role":"user","content":"one"})]),
            &tokens(),
            CodexRequestMode::Standard,
            &mut on_event,
        )
        .await
        .unwrap();

    assert!(callback_ran_before_completion.load(std::sync::atomic::Ordering::SeqCst));
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
        let mut on_event =
            Some(&mut collect_event as &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>);

        transport
            .send_responses_turn(
                body(vec![json!({"role":"user","content":"one"})]),
                &tokens(),
                CodexRequestMode::Standard,
                &mut on_event,
            )
            .await
            .unwrap_err()
    };

    assert_eq!(deltas, ["partial"]);
    assert!(error
        .to_string()
        .contains("Codex WebSocket failed after streaming output"));
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
