use super::codex_ws_test_support::{
    body, immediate, read_request_frame, send_completion, tokens, ws_server,
};
use super::*;
use crate::model::{ContentBlock, ModelResponse};
use serde_json::json;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;

async fn send_created(socket: &mut WebSocketStream<TcpStream>, response_id: &str) {
    socket
        .send(Message::Text(
            json!({"type":"response.created","response":{"id": response_id, "status":"in_progress"}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
}

async fn send_steered_incomplete(
    socket: &mut WebSocketStream<TcpStream>,
    response_id: &str,
    text: &str,
) {
    socket
        .send(Message::Text(
            json!({
                "type":"response.incomplete",
                "response":{
                    "id": response_id,
                    "status":"incomplete",
                    "incomplete_details":{"reason":"steered"},
                    "output_text": text
                }
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
}

async fn ws_server_completes_steer_before_ack() -> (String, Arc<StdMutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let frames = Arc::new(StdMutex::new(Vec::new()));
    let server_frames = Arc::clone(&frames);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let create = read_request_frame(&mut socket).await;
        server_frames.lock().unwrap().push(create);
        send_created(&mut socket, "resp_1").await;
        socket
            .send(Message::Text(
                json!({"type":"response.output_text.delta","delta":"partial"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let steer = read_request_frame(&mut socket).await;
        server_frames.lock().unwrap().push(steer);
        send_steered_incomplete(&mut socket, "resp_1", "partial").await;
        send_created(&mut socket, "resp_2").await;
        send_completion(&mut socket, 2).await;
    });
    (format!("ws://{addr}/responses"), frames)
}

fn steer_receiver(
    text: &str,
) -> (
    Option<rho_sdk::provider::ProviderSteeringReceiver>,
    tokio::sync::mpsc::UnboundedSender<rho_sdk::provider::ProviderSteeringRequest>,
) {
    let (tx, rx) = rho_sdk::provider::provider_steering_channel();
    let _ = text;
    (Some(rx), tx)
}

// Covers: a steered completion proves consumption even when its explicit ack
// is delayed, so the next request reuses the continuation without response.create.
// Owner: openai websocket steering
#[tokio::test]
async fn steer_reuses_auto_continuation_when_completion_precedes_ack() {
    let (url, frames) = ws_server_completes_steer_before_ack().await;
    let transport = CodexWsTransport::new_with_url(url);
    let (mut steering, offer) = steer_receiver("S1");
    let first_output = std::sync::Arc::new(tokio::sync::Notify::new());
    let notify = std::sync::Arc::clone(&first_output);
    let mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)> =
        Some(&mut move |event| {
            if matches!(event, ModelEvent::OutputDelta(_)) {
                notify.notify_one();
            }
            Ok(())
        });
    let mut steer_outcomes = None;
    let first_turn = {
        let body = body(vec![json!({"role":"user","content":"one"})]);
        let tokens = tokens();
        let turn =
            transport.send_responses_turn_steerable(body, &tokens, &mut on_event, &mut steering);
        tokio::pin!(turn);
        let mut offered = false;
        loop {
            tokio::select! {
                () = first_output.notified(), if !offered => {
                    let (request, outcomes) =
                        rho_sdk::provider::ProviderSteeringRequest::test_unclaimed(vec![
                            ContentBlock::Text("S1".into()),
                        ]);
                    offer.send(request).unwrap();
                    steer_outcomes = Some(outcomes);
                    offered = true;
                }
                result = &mut turn => break result.unwrap(),
            }
        }
    };
    let CodexWsTurn::Completed(response) = first_turn else {
        panic!("expected steered original completion");
    };
    assert!(response.steered);
    assert_eq!(
        response.response,
        ModelResponse::Assistant(vec![ContentBlock::Text("partial".into())])
    );
    let outcome = steer_outcomes
        .as_mut()
        .expect("steer was offered")
        .try_recv()
        .ok();
    assert!(
        matches!(
            outcome,
            Some((_, rho_sdk::provider::ProviderSteeringOutcome::Accepted))
        ),
        "{outcome:?}"
    );

    let mut on_event = None;
    let mut steering = None;
    let continuation = immediate(transport.send_responses_turn_steerable(
        body(vec![
            json!({"role":"user","content":"one"}),
            json!({"role":"assistant","content":"partial"}),
            json!({"role":"user","content":[{"type":"input_text","text":"S1"}]}),
        ]),
        &tokens(),
        &mut on_event,
        &mut steering,
    ))
    .await
    .unwrap();
    let CodexWsTurn::Completed(response) = continuation else {
        panic!("expected continuation completion");
    };
    assert!(!response.steered);
    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["type"], "response.create");
    assert_eq!(frames[1]["type"], "response.steer");
    assert_eq!(frames[1]["previous_response_id"], "resp_1");
}

// Covers: an accepted steer is released when the original ends before any
// representable output, and steer.failed releases without delivering
// Owner: openai websocket steering
#[tokio::test]
async fn steer_is_released_when_unforwardable_or_failed() {
    let (url, frames) = ws_server(1).await;
    let transport = CodexWsTransport::new_with_url(url);
    let (mut steering, offer) = steer_receiver("S1");
    let (request, mut outcomes) =
        rho_sdk::provider::ProviderSteeringRequest::test_unclaimed(vec![ContentBlock::Text(
            "S1".into(),
        )]);
    offer.send(request).unwrap();
    let mut on_event = None;
    let turn = immediate(transport.send_responses_turn_steerable(
        body(vec![json!({"role":"user","content":"one"})]),
        &tokens(),
        &mut on_event,
        &mut steering,
    ))
    .await
    .unwrap();
    assert!(matches!(turn, CodexWsTurn::Completed(_)));
    let outcome = outcomes.try_recv().ok();
    assert!(
        matches!(
            outcome,
            Some((_, rho_sdk::provider::ProviderSteeringOutcome::Released))
        ),
        "{outcome:?}"
    );
    assert_eq!(frames.lock().unwrap().len(), 1);
    assert_eq!(frames.lock().unwrap()[0]["type"], "response.create");
}

async fn ws_server_acknowledges_first_of_two_steers() -> (String, Arc<StdMutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let frames = Arc::new(StdMutex::new(Vec::new()));
    let server_frames = Arc::clone(&frames);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let create = read_request_frame(&mut socket).await;
        server_frames.lock().unwrap().push(create);
        send_created(&mut socket, "resp_1").await;
        socket
            .send(Message::Text(
                json!({"type":"response.output_text.delta","delta":"partial"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        for sequence in 1..=2 {
            let steer = read_request_frame(&mut socket).await;
            server_frames.lock().unwrap().push(steer);
            if sequence == 1 {
                socket
                    .send(Message::Text(
                        json!({
                            "type":"response.steer.accepted",
                            "sequence_number": sequence,
                            "steer":{"id":format!("steer_{sequence}"),"previous_response_id":"resp_1"}
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
            }
        }
        send_steered_incomplete(&mut socket, "resp_1", "partial").await;
    });
    (format!("ws://{addr}/responses"), frames)
}

// Covers: acknowledgement of one steer releases the delivery slot, while that
// steer—not a later unacknowledged send—accounts for the steered completion.
// Owner: openai websocket steering
#[tokio::test]
async fn queued_steer_sends_after_prior_acknowledgement() {
    let (url, frames) = ws_server_acknowledges_first_of_two_steers().await;
    let transport = CodexWsTransport::new_with_url(url);
    let (mut steering, offer) = steer_receiver("unused");
    let first_output = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&first_output);
    let mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)> =
        Some(&mut move |event| {
            if matches!(event, ModelEvent::OutputDelta(_)) {
                notify.notify_one();
            }
            Ok(())
        });
    let body = body(vec![json!({"role":"user","content":"one"})]);
    let tokens = tokens();
    let mut steer_outcomes = Vec::new();
    let turn = transport.send_responses_turn_steerable(body, &tokens, &mut on_event, &mut steering);
    tokio::pin!(turn);
    let result = loop {
        tokio::select! {
            () = first_output.notified() => {
                for text in ["S1", "S2"] {
                    let (request, outcomes) =
                        rho_sdk::provider::ProviderSteeringRequest::test_unclaimed(vec![
                            ContentBlock::Text(text.into()),
                        ]);
                    offer.send(request).unwrap();
                    steer_outcomes.push(outcomes);
                }
            }
            result = &mut turn => break result.unwrap(),
        }
    };
    assert!(matches!(result, CodexWsTurn::Completed(response) if response.steered));
    let outcomes = steer_outcomes
        .iter_mut()
        .map(|outcomes| outcomes.try_recv().expect("steer settled").1)
        .collect::<Vec<_>>();
    assert_eq!(
        outcomes,
        vec![
            rho_sdk::provider::ProviderSteeringOutcome::Accepted,
            rho_sdk::provider::ProviderSteeringOutcome::Released,
        ]
    );
    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[1]["type"], "response.steer");
    assert_eq!(frames[2]["type"], "response.steer");
}

async fn ws_server_accepts_steer_during_continuation() -> (String, Arc<StdMutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let frames = Arc::new(StdMutex::new(Vec::new()));
    let server_frames = Arc::clone(&frames);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let create = read_request_frame(&mut socket).await;
        server_frames.lock().unwrap().push(create);
        send_created(&mut socket, "resp_1").await;
        socket
            .send(Message::Text(
                json!({"type":"response.output_text.delta","delta":"partial"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        for (response_id, text) in [("resp_1", "partial"), ("resp_2", "continued")] {
            let steer = read_request_frame(&mut socket).await;
            server_frames.lock().unwrap().push(steer);
            socket
                .send(Message::Text(
                    json!({
                        "type":"response.steer.accepted",
                        "steer":{"id":format!("steer_{response_id}"),"previous_response_id":response_id}
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            send_steered_incomplete(&mut socket, response_id, text).await;
            if response_id == "resp_1" {
                send_created(&mut socket, "resp_2").await;
                socket
                    .send(Message::Text(
                        json!({"type":"response.output_text.delta","delta":"continued"})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
            }
        }
        send_created(&mut socket, "resp_3").await;
        send_completion(&mut socket, 3).await;
    });
    (format!("ws://{addr}/responses"), frames)
}

// Covers: steering an auto-continuation creates another reusable continuation
// rather than losing it and sending a conflicting response.create.
// Owner: openai websocket steering
#[tokio::test]
async fn steer_during_reused_continuation_is_reused_again() {
    let (url, frames) = ws_server_accepts_steer_during_continuation().await;
    let transport = CodexWsTransport::new_with_url(url);
    let (mut steering, offer) = steer_receiver("unused");
    let first_output = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&first_output);
    let mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)> =
        Some(&mut move |event| {
            if matches!(event, ModelEvent::OutputDelta(_)) {
                notify.notify_one();
            }
            Ok(())
        });
    let first = {
        let body = body(vec![json!({"role":"user","content":"one"})]);
        let tokens = tokens();
        let turn =
            transport.send_responses_turn_steerable(body, &tokens, &mut on_event, &mut steering);
        tokio::pin!(turn);
        loop {
            tokio::select! {
                () = first_output.notified() => {
                    let (request, _outcomes) =
                        rho_sdk::provider::ProviderSteeringRequest::test_unclaimed(vec![
                            ContentBlock::Text("S1".into()),
                        ]);
                    offer.send(request).unwrap();
                }
                result = &mut turn => break result.unwrap(),
            }
        }
    };
    assert!(matches!(first, CodexWsTurn::Completed(response) if response.steered));

    let second_output = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&second_output);
    let mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)> =
        Some(&mut move |event| {
            if matches!(event, ModelEvent::OutputDelta(_)) {
                notify.notify_one();
            }
            Ok(())
        });
    let second = {
        let body = body(vec![
            json!({"role":"user","content":"one"}),
            json!({"role":"assistant","content":"partial"}),
            json!({"role":"user","content":[{"type":"input_text","text":"S1"}]}),
        ]);
        let tokens = tokens();
        let turn =
            transport.send_responses_turn_steerable(body, &tokens, &mut on_event, &mut steering);
        tokio::pin!(turn);
        loop {
            tokio::select! {
                () = second_output.notified() => {
                    let (request, _outcomes) =
                        rho_sdk::provider::ProviderSteeringRequest::test_unclaimed(vec![
                            ContentBlock::Text("S2".into()),
                        ]);
                    offer.send(request).unwrap();
                }
                result = &mut turn => break result.unwrap(),
            }
        }
    };
    assert!(matches!(second, CodexWsTurn::Completed(response) if response.steered));

    let mut on_event = None;
    let mut no_steering = None;
    let third = immediate(transport.send_responses_turn_steerable(
        body(vec![
            json!({"role":"user","content":"one"}),
            json!({"role":"assistant","content":"partial"}),
            json!({"role":"user","content":[{"type":"input_text","text":"S1"}]}),
            json!({"role":"assistant","content":"continued"}),
            json!({"role":"user","content":[{"type":"input_text","text":"S2"}]}),
        ]),
        &tokens(),
        &mut on_event,
        &mut no_steering,
    ))
    .await
    .unwrap();
    assert!(matches!(third, CodexWsTurn::Completed(response) if !response.steered));
    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0]["type"], "response.create");
    assert_eq!(frames[1]["type"], "response.steer");
    assert_eq!(frames[2]["type"], "response.steer");
}
