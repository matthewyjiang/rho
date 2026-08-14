use std::time::Duration;

use rho_sdk::SecretString;
use url::Url;

use super::{ProviderBuildOptions, ProviderBuilder, ProviderCredential};
use crate::{providers::openai_compatible::CompatibleAuth, reasoning::ReasoningLevel};

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
    struct RestoreCustomProviders;
    impl Drop for RestoreCustomProviders {
        fn drop(&mut self) {
            crate::provider::reset_custom_openai_compatible_providers_for_tests();
        }
    }
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

// Covers: catalog anthropic npm must send x-api-key to /messages, not Bearer chat
// Owner: provider builder
#[tokio::test]
async fn opencode_go_anthropic_npm_posts_messages_with_x_api_key() {
    use crate::model::models_dev::{
        with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests, ModelMetadata,
    };
    use rho_sdk::model::{Message, ModelRequest};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 8192];
        let bytes = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..bytes]);
        assert!(request.starts_with("POST /messages HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("x-api-key: go-secret"));
        assert!(!request
            .to_ascii_lowercase()
            .contains("authorization: bearer"));
        let json = request
            .rsplit("\r\n\r\n")
            .next()
            .expect("messages request body");
        assert!(
            json.contains(r#""type":"enabled""#) || json.contains(r#""type": "enabled""#),
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
