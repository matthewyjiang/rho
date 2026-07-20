use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::model::ReasoningCapabilities;

#[test]
fn model_conversion_filters_methods_and_strips_resource_prefix() {
    let response: ModelsResponse = serde_json::from_str(r#"{"models":[{"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro","inputTokenLimit":1048576,"outputTokenLimit":65536,"thinking":true,"supportedGenerationMethods":["generateContent"]},{"name":"models/embedding-001","supportedGenerationMethods":["embedContent"]},{"name":"models/gemini-3.1-flash-image","thinking":true,"supportedGenerationMethods":["generateContent"]}]}"#).unwrap();
    let models = response
        .models
        .into_iter()
        .filter(Model::supports_generate_content)
        .filter(|model| is_text_chat_model(model.id()))
        .map(|model| model.into_provider_model("google"))
        .collect::<Vec<_>>();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].model, "gemini-2.5-pro");
    assert_eq!(models[0].context_window, Some(1_048_576));
    assert_eq!(models[0].max_output_tokens, Some(65_536));
    assert!(matches!(
        models[0].reasoning_capabilities,
        ReasoningCapabilities::Levels(_)
    ));
}

#[test]
fn non_thinking_models_are_not_configurable() {
    let model: Model = serde_json::from_str(
        r#"{"name":"models/gemma-test","thinking":false,"supportedGenerationMethods":["generateContent"]}"#,
    )
    .unwrap();

    assert_eq!(
        model.into_provider_model("google").reasoning_capabilities,
        ReasoningCapabilities::NotConfigurable
    );
}

#[test]
fn deprecation_filter_matches_exact_model_ids() {
    let response: ModelsResponse = serde_json::from_str(
        r#"{"models":[{"name":"models/gemini-active","supportedGenerationMethods":["generateContent"]},{"name":"models/gemini-retired","supportedGenerationMethods":["generateContent"]}]}"#,
    )
    .unwrap();
    let mut models = response
        .models
        .into_iter()
        .map(|model| model.into_provider_model("google"))
        .collect::<Vec<_>>();
    let deprecated = HashSet::from(["gemini-retired".to_string()]);
    hide_deprecated_models(&mut models, &deprecated);
    assert_eq!(
        models
            .iter()
            .map(|model| model.model.as_str())
            .collect::<Vec<_>>(),
        ["gemini-active"]
    );
}

#[tokio::test]
async fn fetch_paginates_deduplicates_and_filters_non_chat_models() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        // Two pages from Google's model-list API.
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 8192];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..read]);
                let Some(headers_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..headers_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap_or_default();
                if request.len() >= headers_end + 4 + content_length {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap();
            let (status, body) = if request.starts_with("GET /v1beta/models HTTP/1.1") {
                (
                    "200 OK",
                    r#"{"models":[{"name":"models/gemini-z","thinking":true,"supportedGenerationMethods":["generateContent"]}],"nextPageToken":"next"}"#,
                )
            } else if request.starts_with("GET /v1beta/models?pageToken=next HTTP/1.1") {
                (
                    "200 OK",
                    r#"{"models":[{"name":"models/gemini-a","thinking":true,"supportedGenerationMethods":["generateContent"]},{"name":"models/gemini-z","thinking":true,"supportedGenerationMethods":["generateContent"]},{"name":"models/gemini-retired","thinking":true,"supportedGenerationMethods":["generateContent"]},{"name":"models/gemini-3.1-flash-image","thinking":true,"supportedGenerationMethods":["generateContent"]}]}"#,
                )
            } else {
                ("404 Not Found", r#"{"error":{"status":"NOT_FOUND"}}"#)
            };
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            requests.push(request);
        }
        requests
    });

    let models = fetch_from(
        "google",
        "test-key".into(),
        &format!("http://{address}/v1beta/models"),
    )
    .await
    .unwrap();
    let requests = server.await.unwrap();

    assert_eq!(
        models
            .iter()
            .map(|model| model.model.as_str())
            .collect::<Vec<_>>(),
        ["gemini-a", "gemini-retired", "gemini-z"]
    );
    assert!(requests.iter().any(|request| {
        request.starts_with("GET /v1beta/models HTTP/1.1")
            && request
                .to_ascii_lowercase()
                .contains("x-goog-api-key: test-key")
    }));
    assert!(requests
        .iter()
        .any(|request| request.starts_with("GET /v1beta/models?pageToken=next HTTP/1.1")));
    assert!(requests
        .iter()
        .all(|request| !request.contains("generateContent")));
}
