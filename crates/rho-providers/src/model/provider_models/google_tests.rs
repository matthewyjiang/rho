use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

#[test]
fn model_conversion_filters_methods_and_strips_resource_prefix() {
    let response: ModelsResponse = serde_json::from_str(r#"{"models":[{"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro","inputTokenLimit":1048576,"outputTokenLimit":65536,"supportedGenerationMethods":["generateContent"]},{"name":"models/embedding-001","supportedGenerationMethods":["embedContent"]}]}"#).unwrap();
    let models = response
        .models
        .into_iter()
        .filter(Model::supports_generate_content)
        .map(|model| model.into_provider_model("google"))
        .collect::<Vec<_>>();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].model, "gemini-2.5-pro");
    assert_eq!(models[0].context_window, Some(1_048_576));
    assert_eq!(models[0].max_output_tokens, Some(65_536));
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

#[tokio::test]
async fn fetch_paginates_sorts_deduplicates_and_uses_api_key_header() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for body in [
            r#"{"models":[{"name":"models/gemini-z","supportedGenerationMethods":["generateContent"]}],"nextPageToken":"next"}"#,
            r#"{"models":[{"name":"models/gemini-a","supportedGenerationMethods":["generateContent"]},{"name":"models/gemini-z","supportedGenerationMethods":["generateContent"]}]}"#,
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0_u8; 2048];
            let read = socket.read(&mut request).await.unwrap();
            request.truncate(read);
            socket.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            requests.push(String::from_utf8(request).unwrap());
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
        ["gemini-a", "gemini-z"]
    );
    assert!(requests[0].starts_with("GET /v1beta/models HTTP/1.1"));
    assert!(requests[1].starts_with("GET /v1beta/models?pageToken=next HTTP/1.1"));
    assert!(requests.iter().all(|request| request
        .to_ascii_lowercase()
        .contains("x-goog-api-key: test-key")));
}
