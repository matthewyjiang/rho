use std::sync::Arc;

use super::{auth::Auth, OpenAiProvider};
use crate::{
    credentials::MemoryCredentialStore,
    model::{ContentBlock, Message, ModelEvent, ModelProvider, ModelRequest, ModelResponse},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

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
        .send_turn_stream(
            ModelRequest {
                messages: vec![Message::user_text("hello")],
                tools: Vec::new(),
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
