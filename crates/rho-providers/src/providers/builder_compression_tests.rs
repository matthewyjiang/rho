use std::num::NonZeroUsize;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{Message, ModelRequest},
    provider::provider_event_channel,
};
use tokio::{io::AsyncWriteExt, net::TcpListener};
use url::Url;

use super::{
    tests::read_complete_http_request, ProviderBuildOptions, ProviderBuilder, ProviderCredential,
    RequestCompression,
};
use crate::{
    provider::{CustomProviderSpec, OpenAiCompatibleApi},
    providers::openai_compatible::CompatibleAuth,
    reasoning::ReasoningLevel,
};

// Covers: a custom host's configured encoding must reach the wire on each
// transport a custom host can construct, not only in the encoder.
// Owner: provider builder through the SDK streaming API and HTTP wire.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn configured_compression_reaches_every_custom_host_api() {
    let _lock = crate::provider::custom_provider_registry_test_lock();
    let prompt = "summarize src/control.rs\n".repeat(2_000);
    for (api, path) in [
        (OpenAiCompatibleApi::Responses, "/responses"),
        (OpenAiCompatibleApi::ChatCompletions, "/chat/completions"),
        // Not a config value, but SDK callers can declare it on a custom host.
        (OpenAiCompatibleApi::AnthropicMessages, "/messages"),
    ] {
        crate::provider::reset_custom_openai_compatible_providers_for_tests();
        crate::provider::install_custom_openai_compatible_providers([CustomProviderSpec::new(
            "composer", None,
        )
        .with_api(api)])
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let provider = ProviderBuilder::new(
            ProviderBuildOptions::new("composer", "local-model", ReasoningLevel::Off)
                .unwrap()
                .with_auth("composer-api-key")
                .unwrap()
                .endpoint(endpoint)
                .unwrap()
                .request_compression(RequestCompression::Zstd)
                .unwrap(),
            ProviderCredential::OpenAiCompatible(CompatibleAuth::ApiKey("secret".into())),
        )
        .build()
        .unwrap();

        let messages = [Message::user_text(prompt.clone())];
        // Keep the receiver alive so stream cancellation cannot mask the request.
        let (sender, _receiver) = provider_event_channel(NonZeroUsize::MIN);
        let request = provider.send_turn_stream(
            ModelRequest {
                messages: &messages,
                tools: &[],
                cancellation: Default::default(),
                reasoning_level: ReasoningLevel::Off,
                prompt_cache_key: None,
            },
            sender,
        );
        let server = async {
            let (mut stream, _) = listener.accept().await.unwrap();
            let raw = read_complete_http_request(&mut stream).await;
            // Stop at the HTTP boundary: response parsing is not under test.
            stream
                .write_all(
                    b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            raw
        };
        let (result, raw) = tokio::join!(request, server);
        assert!(result.is_err());

        let header_end = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8(raw[..header_end].to_vec()).unwrap();
        let mut lines = head.lines();
        assert_eq!(lines.next(), Some(format!("POST {path} HTTP/1.1").as_str()));
        let content_encoding = lines.find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
                .map(|(_, value)| value.trim().to_owned())
        });
        assert_eq!(content_encoding.as_deref(), Some("zstd"), "{path}");
        let body: serde_json::Value =
            serde_json::from_slice(&zstd::decode_all(&raw[header_end + 4..]).unwrap()).unwrap();
        assert!(
            body.to_string().contains("summarize src/control.rs"),
            "{path}"
        );
    }
    crate::provider::reset_custom_openai_compatible_providers_for_tests();
}
