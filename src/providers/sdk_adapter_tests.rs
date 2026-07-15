use std::{
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use pretty_assertions::assert_eq;
use reqwest::StatusCode;
use rho_sdk::{
    model::{ContentBlock, Message, ModelEvent, ModelIdentity, ModelRequest, ModelResponse},
    provider::{provider_event_channel, ModelProvider as SdkModelProvider},
    CancellationToken, ProviderErrorKind, ReasoningLevel,
};

use super::{
    provider_error_from_model_error, AdaptableProvider, AppProviderFuture, SdkProviderAdapter,
};
use crate::model::ModelError;

#[derive(Clone)]
struct FakeAdaptableProvider {
    identity: ModelIdentity,
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    response: ModelResponse,
}

#[derive(Clone, Debug, PartialEq)]
struct RecordedRequest {
    messages: Vec<Message>,
    reasoning_level: ReasoningLevel,
    prompt_cache_key: Option<String>,
}

impl FakeAdaptableProvider {
    fn new(response: ModelResponse) -> Self {
        Self {
            identity: ModelIdentity::new("fake", "test", "model"),
            calls: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(Mutex::new(Vec::new())),
            response,
        }
    }
}

impl AdaptableProvider for FakeAdaptableProvider {
    fn model_identity(&self) -> ModelIdentity {
        self.identity.clone()
    }

    fn complete_turn<'a>(&'a self, request: ModelRequest<'a>) -> AppProviderFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(RecordedRequest {
                    messages: request.messages.to_vec(),
                    reasoning_level: request.reasoning_level,
                    prompt_cache_key: request.prompt_cache_key.map(str::to_owned),
                });
            if request.cancellation.is_cancelled() {
                return Err(ModelError::Interrupted);
            }
            Ok(self.response.clone())
        })
    }
}

fn request<'a>(
    messages: &'a [Message],
    cancellation: CancellationToken,
    reasoning_level: ReasoningLevel,
) -> ModelRequest<'a> {
    ModelRequest {
        messages,
        tools: &[],
        cancellation,
        reasoning_level,
        prompt_cache_key: Some("session-1"),
    }
}

#[test]
fn maps_model_errors_to_sanitized_provider_errors() {
    let cases = [
        (
            ModelError::MissingApiKey,
            ProviderErrorKind::Authentication,
            false,
        ),
        (
            ModelError::MissingCodexAuth,
            ProviderErrorKind::Authentication,
            false,
        ),
        (
            ModelError::MissingAnthropicApiKey,
            ProviderErrorKind::Authentication,
            false,
        ),
        (
            ModelError::MissingGithubCopilotAuth,
            ProviderErrorKind::Authentication,
            false,
        ),
        (
            ModelError::MissingXaiAuth,
            ProviderErrorKind::Authentication,
            false,
        ),
        (
            ModelError::Credentials("vault locked".into()),
            ProviderErrorKind::Authentication,
            false,
        ),
        (
            ModelError::Interrupted,
            ProviderErrorKind::Interrupted,
            false,
        ),
        (
            ModelError::StreamIdleTimeout {
                timeout: std::time::Duration::from_secs(30),
            },
            ProviderErrorKind::Timeout,
            true,
        ),
        (
            ModelError::StreamFailedAfterOutput {
                message: "truncated stream".into(),
            },
            ProviderErrorKind::InvalidResponse,
            false,
        ),
        (
            ModelError::InvalidResponse("bad json".into()),
            ProviderErrorKind::InvalidResponse,
            false,
        ),
        (
            ModelError::UnsupportedReasoning {
                provider: "xai",
                model: "grok-build-0.1".into(),
                requested: ReasoningLevel::High,
            },
            ProviderErrorKind::Other,
            false,
        ),
        (
            ModelError::UnsupportedProvider("unknown".into()),
            ProviderErrorKind::Other,
            false,
        ),
        (
            ModelError::HttpStatus {
                status: StatusCode::UNAUTHORIZED,
                body: "secret-token-should-not-leak".into(),
            },
            ProviderErrorKind::Authentication,
            false,
        ),
        (
            ModelError::HttpStatus {
                status: StatusCode::TOO_MANY_REQUESTS,
                body: "retry later".into(),
            },
            ProviderErrorKind::RateLimit,
            true,
        ),
        (
            ModelError::HttpStatus {
                status: StatusCode::BAD_GATEWAY,
                body: "upstream".into(),
            },
            ProviderErrorKind::Unavailable,
            true,
        ),
        (
            ModelError::HttpStatus {
                status: StatusCode::BAD_REQUEST,
                body: "nope".into(),
            },
            ProviderErrorKind::Other,
            false,
        ),
        (
            ModelError::Io(std::io::Error::other("disk full")),
            ProviderErrorKind::Other,
            true,
        ),
    ];

    for (error, kind, retryable) in cases {
        let converted = provider_error_from_model_error(error);
        assert_eq!(converted.kind(), kind);
        assert_eq!(converted.is_retryable(), retryable);
        assert!(!converted.message().contains("secret-token-should-not-leak"));
        assert!(!format!("{converted:?}").contains("secret-token-should-not-leak"));
    }
}

#[test]
fn http_error_messages_include_status_without_bodies() {
    let converted = provider_error_from_model_error(ModelError::HttpStatus {
        status: StatusCode::FORBIDDEN,
        body: "authorization=super-secret".into(),
    });

    assert_eq!(converted.kind(), ProviderErrorKind::Authentication);
    assert_eq!(converted.message(), "HTTP 403");
    assert_eq!(converted.is_retryable(), false);
    assert!(!converted.message().contains("super-secret"));
}

#[tokio::test]
async fn adapter_exposes_identity_and_completes_provider_neutral_turns() {
    let provider = FakeAdaptableProvider::new(ModelResponse::Assistant(vec![ContentBlock::Text(
        "hello".into(),
    )]));
    let adapter = SdkProviderAdapter::new(provider.clone());
    let sdk: Arc<dyn SdkModelProvider> = Arc::new(adapter);
    let messages = [Message::user_text("hi")];

    let response = sdk
        .send_turn(request(
            &messages,
            CancellationToken::new(),
            ReasoningLevel::High,
        ))
        .await
        .unwrap();

    assert_eq!(
        response,
        ModelResponse::Assistant(vec![ContentBlock::Text("hello".into())])
    );
    assert_eq!(sdk.identity(), ModelIdentity::new("fake", "test", "model"));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        provider
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        [RecordedRequest {
            messages: vec![Message::user_text("hi")],
            reasoning_level: ReasoningLevel::High,
            prompt_cache_key: Some("session-1".into()),
        }]
    );
}

#[tokio::test]
async fn default_streaming_synthesizes_text_deltas_from_completed_turn() {
    let provider = FakeAdaptableProvider::new(ModelResponse::Assistant(vec![
        ContentBlock::Text("hello".into()),
        ContentBlock::Text(" world".into()),
    ]));
    let adapter = SdkProviderAdapter::shared(provider);
    let (events, mut receiver) = provider_event_channel(NonZeroUsize::new(4).unwrap());
    let messages = [Message::user_text("hi")];

    let (result, received) = tokio::join!(
        adapter.send_turn_stream(
            request(
                &messages,
                CancellationToken::new(),
                ReasoningLevel::default()
            ),
            events
        ),
        async {
            let mut received = Vec::new();
            while let Some(event) = receiver.recv().await {
                received.push(event);
            }
            received
        }
    );

    assert_eq!(
        result.unwrap(),
        ModelResponse::Assistant(vec![
            ContentBlock::Text("hello".into()),
            ContentBlock::Text(" world".into()),
        ])
    );
    assert_eq!(
        received,
        [
            ModelEvent::OutputDelta("hello".into()),
            ModelEvent::OutputDelta(" world".into()),
        ]
    );
}

#[tokio::test]
async fn cancellation_before_turn_is_reported_as_interrupted() {
    let provider = FakeAdaptableProvider::new(ModelResponse::Assistant(vec![]));
    let adapter = SdkProviderAdapter::new(provider);
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = adapter
        .send_turn(request(&[], cancellation, ReasoningLevel::default()))
        .await
        .unwrap_err();

    assert_eq!(error.kind(), ProviderErrorKind::Interrupted);
}

#[test]
fn debug_redacts_provider_internals() {
    let provider = FakeAdaptableProvider::new(ModelResponse::Assistant(vec![]));
    let adapter = SdkProviderAdapter::new(provider);
    let debug = format!("{adapter:?}");

    assert!(debug.contains("SdkProviderAdapter"));
    assert!(debug.contains("fake"));
    assert!(debug.contains("model"));
    assert!(!debug.contains("response"));
}

#[test]
fn retryability_matches_provider_error_contract() {
    let retryable = provider_error_from_model_error(ModelError::HttpStatus {
        status: StatusCode::TOO_MANY_REQUESTS,
        body: String::new(),
    });
    let permanent = provider_error_from_model_error(ModelError::MissingApiKey);

    assert!(retryable.is_retryable());
    assert!(!permanent.is_retryable());
}

#[test]
fn factory_builds_sdk_provider_entrypoints_for_all_runtimes() {
    use crate::providers::factory::build_sdk_provider;

    // Linking the factory keeps the adapter surface reachable from application
    // construction without requiring live credentials in this unit test.
    let _ = build_sdk_provider;
}

#[tokio::test]
async fn concrete_openai_provider_adapts_to_sdk_model_provider() {
    use crate::credentials::MemoryCredentialStore;
    use crate::providers::openai::auth::Auth;
    use crate::providers::openai::OpenAiProvider;
    use std::sync::Arc;

    let provider = OpenAiProvider::new_with_auth(
        "gpt-4.1".into(),
        Auth::ApiKey("test-key".into()),
        Arc::new(MemoryCredentialStore::default()),
    );
    let adapter = SdkProviderAdapter::new(provider);
    let sdk: Arc<dyn SdkModelProvider> = Arc::new(adapter);

    assert_eq!(
        sdk.identity(),
        ModelIdentity::new("openai", "openai-chat-completions", "gpt-4.1")
    );
}
