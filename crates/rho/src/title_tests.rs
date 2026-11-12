use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{generate_title, sanitize_title, title_model_from_config};
use crate::config::Config;

// Covers: title-model prose is rejected rather than displayed as a truncated title.
// Owner: title sanitizer
#[test]
fn sanitize_title_cleans_labels_and_rejects_prose() {
    let at_character_limit = "a".repeat(80);
    let over_character_limit = "a".repeat(81);
    for (input, expected) in [
        ("  \"Review the auth path\".  ", Some("Review the auth path")),
        ("\"Implement resume picker.\"", Some("Implement resume picker")),
        ("\n\n# Draft\n", Some("Draft")),
        ("   ", None),
        ("Fix one two three four five six", Some("Fix one two three four five six")),
        ("Fix one two three four five six seven", None),
        ("Unable to inspect or modify the repository because no shell or file tools are available", None),
        ("Implement resume picker\nI will inspect the repository first", None),
        (at_character_limit.as_str(), Some(at_character_limit.as_str())),
        (over_character_limit.as_str(), None),
    ] {
        assert_eq!(sanitize_title(input).as_deref(), expected, "input: {input:?}");
    }
}

/// Answers one Chat Completions request with `title`, streamed, and yields
/// the request line it answered.
async fn serve_one_title(listener: tokio::net::TcpListener, title: &str) -> String {
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut request = Vec::new();
    let mut buffer = [0; 16_384];
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
        let length = headers
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .map_or(0, |value| value.trim().parse::<usize>().unwrap());
        if request.len() >= header_end + 4 + length {
            break;
        }
    }
    let chunk = |delta: serde_json::Value, finish: Option<&str>| {
        let chunk = serde_json::json!({
            "id": "title",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "local-model",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        });
        format!("data: {chunk}\n\n")
    };
    let body = [
        chunk(
            serde_json::json!({"role": "assistant", "content": title}),
            None,
        ),
        chunk(serde_json::json!({}), Some("stop")),
        "data: [DONE]\n\n".to_owned(),
    ]
    .concat();
    let reply = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
    );
    stream.write_all(reply.as_bytes()).await.unwrap();
    String::from_utf8_lossy(&request)
        .lines()
        .next()
        .unwrap()
        .to_owned()
}

// Covers: a session whose model is a custom provider from `[providers]` gets
// a title; the title model was built without the config's providers, so
// every custom-provider session went untitled.
// Owner: session title generation
#[tokio::test]
async fn a_custom_provider_from_config_names_the_session() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = Config {
        provider: "titlehost".into(),
        model: "local-model".into(),
        auth: rho_providers::provider::KEYLESS_AUTH.into(),
        ..Config::default()
    };
    config
        .providers
        .set_endpoint(
            "titlehost",
            &format!("http://{}/v1", listener.local_addr().unwrap()),
        )
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();

    let server = tokio::spawn(serve_one_title(listener, "Fix flaky login test"));

    let title = generate_title(
        title_model_from_config(&config),
        super::first_turn_input("fix the flaky login test", "Fixed it."),
        rho_sdk::SessionId::new(),
        workspace.path().to_path_buf(),
        std::sync::Arc::new(rho_providers::credentials::MemoryCredentialStore::default()),
        rho_sdk::ProviderRequestUsageRecording::default(),
        rho_sdk::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(
        (title.as_str(), server.await.unwrap().as_str()),
        ("Fix flaky login test", "POST /v1/chat/completions HTTP/1.1")
    );
}
