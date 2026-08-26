use std::time::Duration;

use rho_sdk::SecretString;
use url::Url;

use super::{ProviderBuildOptions, ProviderBuilder, ProviderCredential};
use crate::{providers::openai_compatible::CompatibleAuth, reasoning::ReasoningLevel};

struct RestoreCustomProviders;
impl Drop for RestoreCustomProviders {
    fn drop(&mut self) {
        crate::provider::reset_custom_openai_compatible_providers_for_tests();
    }
}

#[test]
fn options_reject_invalid_states_and_accept_typed_overrides() {
    assert!(ProviderBuildOptions::new("", "model", ReasoningLevel::Off).is_err());
    assert!(ProviderBuildOptions::new("openai", "", ReasoningLevel::Off).is_err());
    assert!(ProviderBuildOptions::new("unknown", "model", ReasoningLevel::Off).is_err());

    let options = ProviderBuildOptions::new("openai", "model", ReasoningLevel::Low)
        .unwrap()
        .with_auth("codex")
        .unwrap()
        .endpoint(Url::parse("https://example.test/v1").unwrap())
        .unwrap()
        .request_timeout(Duration::from_secs(30))
        .unwrap();

    assert_eq!(options.provider(), "openai-codex");
    assert_eq!(options.auth(), "codex");
    assert_eq!(options.model(), "model");
    assert!(
        ProviderBuildOptions::new("openai", "model", ReasoningLevel::Off)
            .unwrap()
            .endpoint(Url::parse("file:///tmp/provider").unwrap())
            .is_err()
    );
    assert!(
        ProviderBuildOptions::new("openai", "model", ReasoningLevel::Off)
            .unwrap()
            .request_timeout(Duration::ZERO)
            .is_err()
    );
}

#[test]
fn credentials_are_redacted_and_mismatches_fail_before_execution() {
    let secret = "sk-provider-secret";
    let credential = ProviderCredential::AnthropicApiKey(SecretString::new(secret));
    let debug = format!("{credential:?}");
    assert!(debug.contains("anthropic-api-key"));
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(secret));

    let options = ProviderBuildOptions::new("openai", "gpt-test", ReasoningLevel::Off).unwrap();
    let error = match ProviderBuilder::new(options, credential).build() {
        Ok(_) => panic!("mismatched credential unexpectedly built a provider"),
        Err(error) => error,
    };
    assert!(error
        .to_string()
        .contains("credential kind does not match provider"));
    assert!(!format!("{error:?}").contains(secret));

    let options = ProviderBuildOptions::new("ollama", "local-model", ReasoningLevel::Off).unwrap();
    let error = match ProviderBuilder::new(
        options,
        ProviderCredential::OpenAiCompatible(CompatibleAuth::ApiKey(secret.into())),
    )
    .build()
    {
        Ok(_) => panic!("authenticated Ollama credential unexpectedly built a provider"),
        Err(error) => error,
    };
    assert!(error
        .to_string()
        .contains("credential kind does not match provider"));
    assert!(!format!("{error:?}").contains(secret));
}

// Covers: custom-host construction must not depend on a still-live thread scope
// Owner: provider builder
#[test]
fn custom_host_build_survives_dropped_thread_scope() {
    let _lock = crate::provider::custom_provider_registry_test_lock();
    crate::provider::reset_custom_openai_compatible_providers_for_tests();
    let _restore = RestoreCustomProviders;

    let names = crate::provider::intern_custom_openai_compatible_providers(["composer"]).unwrap();
    let options = {
        let _scope = crate::provider::CustomProviderThreadScope::enter(names);
        ProviderBuildOptions::new("composer", "local-model", ReasoningLevel::Off)
            .unwrap()
            .endpoint(Url::parse("http://127.0.0.1:8787/v1").unwrap())
            .unwrap()
    };
    assert!(
        crate::provider::provider_descriptor("composer").is_none(),
        "scope drop must hide the name from process-wide lookup"
    );
    ProviderBuilder::new(
        options,
        ProviderCredential::OpenAiCompatible(CompatibleAuth::None),
    )
    .build()
    .expect("interned custom host must still build after the constructing scope drops");
}

// Covers: PreferModelsDevNpm must construct the adapter named by catalog npm
// Owner: provider builder
#[test]
fn opencode_go_catalog_npm_selects_adapter_identity() {
    use crate::model::models_dev::{
        with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests, ModelMetadata,
    };
    use rho_sdk::model::ModelIdentity;

    let cases = [
        (
            "kimi-k2.7-code",
            Some("@ai-sdk/openai-compatible"),
            "openai-chat-completions",
        ),
        ("grok-4.5", Some("@ai-sdk/openai"), "openai-responses"),
        (
            "minimax-m3",
            Some("@ai-sdk/anthropic"),
            "anthropic-messages",
        ),
        ("unknown-go-model", None, "openai-chat-completions"),
    ];

    for (model, sdk_package, api) in cases {
        let cache = tempfile::tempdir().unwrap();
        with_models_dev_cache_dir_for_tests(cache.path().to_path_buf(), || {
            if let Some(sdk_package) = sdk_package {
                // Hydrate keeps sdk-only rows with incomplete reasoning
                // metadata; construction must still honor them.
                write_cached_model_metadata_for_tests(
                    "opencode-go",
                    model,
                    &ModelMetadata {
                        sdk_package: Some(sdk_package.into()),
                        reasoning_metadata_complete: false,
                        ..ModelMetadata::default()
                    },
                );
            }
            let provider = ProviderBuilder::new(
                ProviderBuildOptions::new("opencode-go", model, ReasoningLevel::Off).unwrap(),
                ProviderCredential::OpenAiCompatible(CompatibleAuth::ApiKey("go-secret".into())),
            )
            .build()
            .unwrap();
            assert_eq!(
                provider.identity(),
                ModelIdentity::new("opencode-go", api, model)
            );
        });
    }
}

// Covers: MiniMax always constructs Anthropic Messages from the declared API,
// including a cold cache and a catalog row that names a different SDK package
// Owner: provider builder
#[test]
fn minimax_declared_api_is_anthropic_messages() {
    use crate::model::models_dev::{
        with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests, ModelMetadata,
    };
    use rho_sdk::model::ModelIdentity;

    let cases = [
        ("MiniMax-M3", None),
        ("MiniMax-M3", Some("@ai-sdk/openai-compatible")),
        ("MiniMax-M2.7", Some("@ai-sdk/anthropic")),
    ];

    for (model, sdk_package) in cases {
        let cache = tempfile::tempdir().unwrap();
        with_models_dev_cache_dir_for_tests(cache.path().to_path_buf(), || {
            if let Some(sdk_package) = sdk_package {
                write_cached_model_metadata_for_tests(
                    "minimax",
                    model,
                    &ModelMetadata {
                        sdk_package: Some(sdk_package.into()),
                        reasoning_metadata_complete: false,
                        ..ModelMetadata::default()
                    },
                );
            }
            let provider = ProviderBuilder::new(
                ProviderBuildOptions::new("minimax", model, ReasoningLevel::Off).unwrap(),
                ProviderCredential::OpenAiCompatible(CompatibleAuth::ApiKey(
                    "minimax-secret".into(),
                )),
            )
            .build()
            .unwrap();
            assert_eq!(
                provider.identity(),
                ModelIdentity::new("minimax", "anthropic-messages", model)
            );
        });
    }
}

// Covers: catalog anthropic npm must send x-api-key to /messages, not Bearer chat
// Owner: provider builder
#[tokio::test]
async fn opencode_go_anthropic_npm_posts_messages_with_x_api_key() {
    use crate::model::models_dev::{
        with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests, ModelMetadata,
    };
    use rho_sdk::model::{Message, ModelRequest};
    use tokio::{io::AsyncWriteExt, net::TcpListener};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let raw = read_complete_http_request(&mut stream).await;
        let header_end = raw
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("http header terminator");
        let headers = String::from_utf8_lossy(&raw[..header_end]);
        assert!(headers.starts_with("POST /messages HTTP/1.1"));
        assert!(headers
            .to_ascii_lowercase()
            .contains("x-api-key: go-secret"));
        assert!(!headers
            .to_ascii_lowercase()
            .contains("authorization: bearer"));
        let content_length = headers.lines().find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|value| value.trim().parse::<usize>().unwrap())
        });
        let body = match content_length {
            Some(len) => &raw[header_end + 4..header_end + 4 + len],
            None => &raw[header_end + 4..],
        };
        let json: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(
            json.pointer("/thinking/type")
                .and_then(|value| value.as_str()),
            Some("enabled"),
            "hosted Messages Max must send thinking.type=enabled, got {json}"
        );
        let body = r#"{"content":[{"type":"text","text":"hello"}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir_for_tests(cache.path().to_path_buf(), || {
        write_cached_model_metadata_for_tests(
            "opencode-go",
            "minimax-m3",
            &ModelMetadata {
                sdk_package: Some("@ai-sdk/anthropic".into()),
                reasoning_metadata_complete: false,
                ..ModelMetadata::default()
            },
        );
    });

    let provider = with_models_dev_cache_dir_for_tests(cache.path().to_path_buf(), || {
        ProviderBuilder::new(
            ProviderBuildOptions::new("opencode-go", "minimax-m3", ReasoningLevel::Off)
                .unwrap()
                .endpoint(Url::parse(&api_base).unwrap())
                .unwrap(),
            ProviderCredential::OpenAiCompatible(CompatibleAuth::ApiKey("go-secret".into())),
        )
        .build()
        .unwrap()
    });

    let messages = [Message::user_text("hello")];
    provider
        .send_turn(ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Max,
            prompt_cache_key: None,
        })
        .await
        .unwrap();
    server.await.unwrap();
}

fn install_custom_composer_responses() -> RestoreCustomProviders {
    crate::provider::reset_custom_openai_compatible_providers_for_tests();
    crate::provider::install_custom_openai_compatible_providers([
        crate::provider::CustomProviderSpec::new("composer", None)
            .with_api(crate::provider::OpenAiCompatibleApi::Responses),
    ])
    .unwrap();
    RestoreCustomProviders
}

const RESPONSES_SSE_OK: &str = concat!(
    "data:{\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
    "data:{\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"output_text\":\"hello\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
);

fn http_request_headers_and_json(raw: &[u8]) -> (String, serde_json::Value) {
    let header_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("http header terminator");
    let headers = String::from_utf8_lossy(&raw[..header_end]).into_owned();
    let content_length = headers.lines().find_map(|line| {
        line.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .map(|value| value.trim().parse::<usize>().unwrap())
    });
    let body = match content_length {
        Some(len) => &raw[header_end + 4..header_end + 4 + len],
        None => &raw[header_end + 4..],
    };
    let json: serde_json::Value = serde_json::from_slice(body).unwrap();
    (headers, json)
}

async fn serve_one_responses_sse(listener: tokio::net::TcpListener) -> Vec<u8> {
    use tokio::io::AsyncWriteExt;

    let (mut stream, _) = listener.accept().await.unwrap();
    let raw = read_complete_http_request(&mut stream).await;
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{RESPONSES_SSE_OK}",
        RESPONSES_SSE_OK.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
    raw
}

// Covers: custom host with api=responses and a key must hit Responses, not chat
// Owner: provider builder
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn custom_responses_api_key_posts_responses_with_bearer() {
    use rho_sdk::model::{Message, ModelIdentity, ModelRequest};
    use tokio::net::TcpListener;

    let _lock = crate::provider::custom_provider_registry_test_lock();
    let _restore = install_custom_composer_responses();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(serve_one_responses_sse(listener));

    let provider = ProviderBuilder::new(
        ProviderBuildOptions::new("composer", "local-model", ReasoningLevel::Off)
            .unwrap()
            .with_auth("composer-api-key")
            .unwrap()
            .endpoint(Url::parse(&api_base).unwrap())
            .unwrap(),
        ProviderCredential::OpenAiCompatible(CompatibleAuth::ApiKey("secret".into())),
    )
    .build()
    .unwrap();
    assert_eq!(
        provider.identity(),
        ModelIdentity::new("composer", "openai-responses", "local-model")
    );

    let messages = [Message::user_text("hello")];
    provider
        .send_turn(ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Off,
            prompt_cache_key: None,
        })
        .await
        .unwrap();
    let raw = server.await.unwrap();
    let (headers, _) = http_request_headers_and_json(&raw);
    assert!(
        headers.starts_with("POST /responses HTTP/1.1"),
        "expected POST /responses, got {headers}"
    );
    assert!(headers
        .to_ascii_lowercase()
        .contains("authorization: bearer secret"));
    assert!(!headers.contains("/chat/completions"));
}

// Covers: custom host with api=responses and no key must hit Responses without Authorization
// Owner: provider builder
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn custom_responses_keyless_posts_responses_without_authorization() {
    use rho_sdk::model::{Message, ModelRequest};
    use tokio::net::TcpListener;

    let _lock = crate::provider::custom_provider_registry_test_lock();
    let _restore = install_custom_composer_responses();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(serve_one_responses_sse(listener));

    let provider = ProviderBuilder::new(
        ProviderBuildOptions::new("composer", "local-model", ReasoningLevel::Off)
            .unwrap()
            .endpoint(Url::parse(&api_base).unwrap())
            .unwrap(),
        ProviderCredential::OpenAiCompatible(CompatibleAuth::None),
    )
    .build()
    .unwrap();

    let messages = [Message::user_text("hello")];
    provider
        .send_turn(ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Off,
            prompt_cache_key: None,
        })
        .await
        .unwrap();
    let raw = server.await.unwrap();
    let (headers, _) = http_request_headers_and_json(&raw);
    assert!(
        headers.starts_with("POST /responses HTTP/1.1"),
        "expected POST /responses, got {headers}"
    );
    assert!(!headers.to_ascii_lowercase().contains("authorization:"));
}

// Covers: a web_search ToolSpec must serialize as type=function, not type=web_search
// Owner: provider builder
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn custom_responses_does_not_inject_hosted_web_search() {
    use rho_sdk::model::{Message, ModelRequest, ToolSpec};
    use serde_json::json;
    use tokio::net::TcpListener;

    let _lock = crate::provider::custom_provider_registry_test_lock();
    let _restore = install_custom_composer_responses();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(serve_one_responses_sse(listener));

    let provider = ProviderBuilder::new(
        ProviderBuildOptions::new("composer", "local-model", ReasoningLevel::Off)
            .unwrap()
            .endpoint(Url::parse(&api_base).unwrap())
            .unwrap(),
        ProviderCredential::OpenAiCompatible(CompatibleAuth::None),
    )
    .build()
    .unwrap();

    let messages = [Message::user_text("search")];
    let tools = [ToolSpec {
        name: "web_search".into(),
        description: "search the web".into(),
        input_schema: json!({"type": "object", "properties": {}}),
    }];
    provider
        .send_turn(ModelRequest {
            messages: &messages,
            tools: &tools,
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Off,
            prompt_cache_key: None,
        })
        .await
        .unwrap();
    let raw = server.await.unwrap();
    let (_, json) = http_request_headers_and_json(&raw);
    let tools = json
        .get("tools")
        .and_then(|value| value.as_array())
        .expect("tools array");
    let web_search = tools
        .iter()
        .find(|tool| tool.get("name").and_then(|value| value.as_str()) == Some("web_search"))
        .expect("web_search tool");
    assert_eq!(
        web_search.get("type").and_then(|value| value.as_str()),
        Some("function"),
        "hosted web_search must not be injected, got {json}"
    );
}

async fn read_complete_http_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    use tokio::io::AsyncReadExt;

    let mut buf = vec![0; 16_384];
    let mut request = Vec::new();
    loop {
        let bytes = stream.read(&mut buf).await.unwrap();
        if bytes == 0 {
            break;
        }
        request.extend_from_slice(&buf[..bytes]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end + 4]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap_or(0))
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    request
}
