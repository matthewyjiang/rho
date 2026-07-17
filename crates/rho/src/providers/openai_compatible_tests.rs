use super::*;
use crate::{
    model::{ContentBlock, Message},
    tool::Tool,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[tokio::test]
async fn moonshot_posts_chat_completions_with_bearer_auth() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 8192];
        let bytes = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..bytes]);
        assert!(request.starts_with("POST /chat/completions HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer moonshot-secret"));
        assert!(request.contains("\"model\":\"kimi-k3\""));
        let request_body = request.split("\r\n\r\n").nth(1).unwrap();
        let body: serde_json::Value = serde_json::from_str(request_body).unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
        let schema = &body["tools"][0]["function"]["parameters"];
        assert_eq!(schema["type"], "object");
        assert!(schema.get("anyOf").is_none());

        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hello"}}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let provider = OpenAiCompatibleProvider::new(
        reqwest::Client::new(),
        "moonshot",
        "kimi-k3".into(),
        OpenAiCompatibleDialect::Moonshot,
        CompatibleAuth::ApiKey("moonshot-secret".into()),
        api_base,
    );
    let tool = crate::tools::edit_file::EditFile.spec();
    let response = provider
        .complete_turn(ModelRequest {
            messages: &[Message::user_text("hello")],
            tools: &[tool],
            cancellation: Default::default(),
            reasoning_level: crate::reasoning::ReasoningLevel::Max,
            prompt_cache_key: None,
        })
        .await
        .unwrap();
    assert!(matches!(
        response,
        ModelResponse::Assistant(blocks)
            if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "hello")
    ));
    server.await.unwrap();
}

#[test]
fn identities_keep_custom_provider_names() {
    let moonshot = OpenAiCompatibleProvider::new(
        reqwest::Client::new(),
        "moonshot",
        "kimi-k3".into(),
        OpenAiCompatibleDialect::Moonshot,
        CompatibleAuth::ApiKey("secret".into()),
        "https://api.moonshot.ai/v1".into(),
    );
    assert_eq!(moonshot.model_identity().provider, "moonshot");
    assert_eq!(moonshot.model_identity().api, "openai-chat-completions");
}
