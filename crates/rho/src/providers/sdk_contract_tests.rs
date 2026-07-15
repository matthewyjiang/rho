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

use super::provider_error_from_model_error;
use crate::model::ModelError;

#[derive(Clone)]
struct FakeProvider {
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

impl FakeProvider {
    fn new(response: ModelResponse) -> Self {
        Self {
            identity: ModelIdentity::new("fake", "test", "model"),
            calls: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(Mutex::new(Vec::new())),
            response,
        }
    }

    fn model_identity(&self) -> ModelIdentity {
        self.identity.clone()
    }

    async fn complete_turn(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ModelError> {
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
    }

    async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let response = self.complete_turn(request).await?;
        let ModelResponse::Assistant(blocks) = &response;
        for block in blocks {
            if let ContentBlock::Text(text) = block {
                on_event(ModelEvent::OutputDelta(text.clone()))?;
            }
        }
        Ok(response)
    }
}

crate::impl_sdk_model_provider!(FakeProvider);

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
async fn providers_implement_sdk_contract_directly() {
    let provider = FakeProvider::new(ModelResponse::Assistant(vec![ContentBlock::Text(
        "hello".into(),
    )]));
    let sdk: Arc<dyn SdkModelProvider> = Arc::new(provider.clone());
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
async fn callback_stream_bridge_forwards_events_in_order() {
    let provider = FakeProvider::new(ModelResponse::Assistant(vec![
        ContentBlock::Text("hello".into()),
        ContentBlock::Text(" world".into()),
    ]));
    let (events, mut receiver) = provider_event_channel(NonZeroUsize::new(4).unwrap());
    let messages = [Message::user_text("hi")];

    let (result, received) = tokio::join!(
        provider.send_turn_stream(
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
    let provider = FakeProvider::new(ModelResponse::Assistant(vec![]));
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = provider
        .send_turn(request(&[], cancellation, ReasoningLevel::default()))
        .await
        .unwrap_err();

    assert_eq!(error.kind(), ProviderErrorKind::Interrupted);
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

#[tokio::test]
async fn concrete_openai_provider_implements_sdk_model_provider() {
    use crate::credentials::MemoryCredentialStore;
    use crate::providers::openai::auth::Auth;
    use crate::providers::openai::OpenAiProvider;
    use std::sync::Arc;

    let provider = OpenAiProvider::new_with_auth(
        "gpt-4.1".into(),
        Auth::ApiKey("test-key".into()),
        Arc::new(MemoryCredentialStore::default()),
    );
    let sdk: Arc<dyn SdkModelProvider> = Arc::new(provider);

    assert_eq!(
        sdk.identity(),
        ModelIdentity::new("openai", "openai-chat-completions", "gpt-4.1")
    );
}
