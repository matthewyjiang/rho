use super::codex_ws_test_support::{immediate, read_request_frame, request_body, tokens};
use super::*;
use crate::model::{Message as ModelMessage, ModelResponse, ToolResult};
use pretty_assertions::assert_eq;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;

// Covers: SDK-style context projection changes between tool steps must not
// replay history or lose fresh context, including after a previous delta.
// Owner: openai websocket continuation wire contract
#[tokio::test]
async fn tool_loop_projects_fresh_context_without_replaying_history() {
    let contexts: [&[&str]; 5] = [
        &["first"],
        &["changed", "another projection"],
        &["changed", "another projection"],
        &[],
        &["restored"],
    ];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/responses", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let mut frames = Vec::new();
        for index in 0..contexts.len() {
            let frame = read_request_frame(&mut socket).await;
            frames.push(frame);
            let item = json!({
                "type": "function_call",
                "call_id": format!("call_{index}"),
                "name": "read",
                "arguments": "{}",
            });
            for payload in [
                json!({"type": "response.output_item.done", "output_index": 0, "item": item}),
                json!({
                    "type": "response.completed",
                    "response": {"id": format!("resp_{index}"), "output": [item]},
                }),
            ] {
                socket
                    .send(Message::Text(payload.to_string().into()))
                    .await
                    .unwrap();
            }
        }
        frames
    });
    let transport = CodexWsTransport::new_with_url(url);
    let mut history = vec![ModelMessage::user_text("read it")];
    let mut expected_frames = Vec::new();
    for (index, context) in contexts.into_iter().enumerate() {
        // The SDK appends the current context to a copy of durable history; it
        // does not insert the previous request's context into that history.
        let mut projected = history.clone();
        projected.extend(context.iter().copied().map(ModelMessage::model_context));
        let body = request_body(&projected);
        let mut expected = body.clone();
        expected["type"] = json!("response.create");
        if index > 0 {
            // Old context is retained on the server but absent from this
            // transcript. Even after a delta, only the next result and current
            // projection belong in the new frame.
            expected["previous_response_id"] = json!(format!("resp_{}", index - 1));
            expected["input"] = json!(request_body(&projected[history.len() - 1..])["input"]);
        }
        expected_frames.push(expected);
        let mut on_event = None;
        let turn = immediate(transport.send_responses_turn(body, &tokens(), &mut on_event))
            .await
            .unwrap();
        let CodexWsTurn::Completed(response) = turn else {
            panic!("expected websocket tool response");
        };
        let ModelResponse::Assistant(blocks) = response.response;
        history.push(ModelMessage::Assistant(blocks));
        history.push(ModelMessage::ToolResult(ToolResult {
            id: format!("call_{index}"),
            ok: true,
            content: format!("contents {index}"),
        }));
    }
    let frames = immediate(server).await.unwrap();
    assert_eq!(frames, expected_frames);
}
