use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message, WebSocketStream};

use crate::credentials::CodexTokens;

pub(super) fn body(input: Vec<Value>) -> Value {
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

pub(super) fn tokens() -> CodexTokens {
    CodexTokens {
        access_token: "token".into(),
        refresh_token: None,
        id_token: None,
        account_id: Some("account".into()),
    }
}

pub(super) async fn ws_server(expected_messages: usize) -> (String, Arc<Mutex<Vec<Value>>>) {
    ws_server_connections(vec![expected_messages]).await
}

pub(super) async fn ws_server_connections(
    expected_messages_by_connection: Vec<usize>,
) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let frames = Arc::new(Mutex::new(Vec::new()));
    let server_frames = Arc::clone(&frames);
    tokio::spawn(async move {
        let mut response_index = 0;
        for expected_messages in expected_messages_by_connection {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            for _ in 0..expected_messages {
                response_index += 1;
                let frame = read_request_frame(&mut socket).await;
                server_frames.lock().unwrap().push(frame);
                send_completion(&mut socket, response_index).await;
            }
        }
    });
    (format!("ws://{addr}/responses"), frames)
}

pub(super) async fn read_request_frame(socket: &mut WebSocketStream<TcpStream>) -> Value {
    let message = socket.next().await.unwrap().unwrap();
    serde_json::from_str(&message.into_text().unwrap()).unwrap()
}

pub(super) async fn send_completion(
    socket: &mut WebSocketStream<TcpStream>,
    response_index: usize,
) {
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
                    "id": format!("resp_{response_index}"),
                    "service_tier":"default",
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

pub(super) async fn immediate<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(1), future)
        .await
        .expect("terminal websocket event should return without waiting for the idle timeout")
}
