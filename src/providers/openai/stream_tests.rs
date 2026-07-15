use std::sync::Arc;

use super::{
    auth::{Auth, CodexAuthSource},
    codex_ws::CodexWsTransport,
    OpenAiProvider,
};
use crate::{
    cancellation::RunCancellation,
    credentials::{CodexTokens, MemoryCredentialStore},
    model::{ContentBlock, Message, ModelError, ModelEvent, ModelRequest, ModelResponse},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_tungstenite::{accept_async, tungstenite::Message as WsMessage};

#[tokio::test]
async fn chat_completion_stream_accepts_data_without_space_after_colon() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let bytes = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..bytes]);
        assert!(request.starts_with("POST /chat/completions HTTP/1.1"));

        let body = concat!(
            "data:{\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data:[DONE]\n\n"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let mut provider = OpenAiProvider::new_with_auth(
        "gpt-4.1".into(),
        Auth::ApiKey("test-key".into()),
        Arc::new(MemoryCredentialStore::default()),
        None,
        None,
    );
    provider.api_base = api_base;
    provider.client = reqwest::Client::new();

    let mut events = Vec::new();
    let response = provider
        .stream_turn(
            ModelRequest {
                messages: &[Message::user_text("hello")],
                tools: &[],
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: None,
            },
            &mut |event| {
                events.push(event);
                Ok(())
            },
        )
        .await
        .unwrap();

    assert!(matches!(
        response,
        ModelResponse::Assistant(blocks)
            if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "hello")
    ));
    assert!(matches!(
        events.as_slice(),
        [ModelEvent::OutputDelta(delta)] if delta == "hello"
    ));
}

#[tokio::test]
async fn cancelling_codex_stream_resets_websocket_before_next_turn() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}/responses", listener.local_addr().unwrap());
    let first_request_received = Arc::new(tokio::sync::Notify::new());
    let server_first_request_received = Arc::clone(&first_request_received);
    tokio::spawn(async move {
        let (first_stream, _) = listener.accept().await.unwrap();
        let mut first_socket = accept_async(first_stream).await.unwrap();
        first_socket.next().await.unwrap().unwrap();
        server_first_request_received.notify_one();

        let (second_stream, _) = listener.accept().await.unwrap();
        let mut second_socket = accept_async(second_stream).await.unwrap();
        second_socket.next().await.unwrap().unwrap();
        second_socket
            .send(WsMessage::Text(
                json!({"type":"response.output_text.delta","delta":"fresh"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        second_socket
            .send(WsMessage::Text(
                json!({
                    "type":"response.completed",
                    "response":{
                        "id":"resp_fresh",
                        "output_text":"fresh",
                        "output":[],
                        "usage":{"input_tokens":1,"output_tokens":1}
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
    });

    let tokens = CodexTokens {
        access_token: "token".into(),
        refresh_token: None,
        id_token: None,
        account_id: None,
    };
    let mut provider = OpenAiProvider::new_with_auth(
        "gpt-5-codex".into(),
        Auth::Codex {
            tokens,
            source: CodexAuthSource::Env,
        },
        Arc::new(MemoryCredentialStore::default()),
        None,
        None,
    );
    provider.codex_ws = CodexWsTransport::new_with_url(ws_url);

    let cancellation = RunCancellation::default();
    let cancel_after_request = {
        let cancellation = cancellation.clone();
        async move {
            first_request_received.notified().await;
            cancellation.cancel();
        }
    };
    let first_messages = [Message::user_text("first")];
    let mut on_first_event = |_| Ok(());
    let first_turn = provider.stream_turn(
        ModelRequest {
            messages: &first_messages,
            tools: &[],
            cancellation,
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
        &mut on_first_event,
    );
    let (result, ()) = tokio::join!(first_turn, cancel_after_request);
    assert!(matches!(result, Err(ModelError::Interrupted)));

    let response = provider
        .stream_turn(
            ModelRequest {
                messages: &[Message::user_text("second")],
                tools: &[],
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: None,
            },
            &mut |_| Ok(()),
        )
        .await
        .unwrap();
    assert!(matches!(
        response,
        ModelResponse::Assistant(blocks)
            if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "fresh")
    ));
}

#[test]
fn rejects_out_of_range_tool_call_index() {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let err = crate::protocol::openai_chat::handle_openai_stream_line(
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":4000000000}]}}]}"#,
        &mut text,
        &mut tool_calls,
        &mut |_| Ok(()),
    )
    .unwrap_err();

    assert!(matches!(
        err,
        crate::model::ModelError::InvalidResponse(message)
            if message == "stream block index 4000000000 out of range"
    ));
    assert!(tool_calls.is_empty());
}
