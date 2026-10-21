use std::{num::NonZeroUsize, time::Instant};

use pretty_assertions::assert_eq;

use crate::{
    model::{
        Message, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage, ToolSpec,
    },
    CancellationToken, ProviderErrorKind,
};

use super::{
    provider_event_channel, ModelProvider, ProviderRequestEvent, ScriptedProvider, ScriptedTurn,
};

fn identity() -> ModelIdentity {
    ModelIdentity::new("scripted", "test", "model")
}

fn request<'a>(
    messages: &'a [Message],
    tools: &'a [ToolSpec],
    cancellation: CancellationToken,
) -> ModelRequest<'a> {
    ModelRequest {
        messages,
        tools,
        cancellation,
        reasoning_level: crate::ReasoningLevel::default(),
        prompt_cache_key: Some("session-1"),
    }
}

#[tokio::test]
async fn semantic_and_request_event_receivers_remain_compatible() {
    let (events, mut receiver) = provider_event_channel(NonZeroUsize::new(2).unwrap());
    events
        .send_request_attempt_failed(ProviderErrorKind::Unavailable, ModelUsage::default())
        .await
        .unwrap();
    let expected = ModelEvent::OutputDelta("retry succeeded".into());
    events.send(expected.clone()).await.unwrap();
    drop(events);

    assert_eq!(receiver.recv().await, Some(expected));
    assert_eq!(receiver.recv().await, None);
    assert_eq!(
        receiver.recv_request_event().await,
        Some(ProviderRequestEvent::RequestAttemptFailed {
            kind: ProviderErrorKind::Unavailable,
            usage: ModelUsage::default(),
        })
    );
    assert_eq!(receiver.recv_request_event().await, None);
}

// Covers: a provider observation timestamp survives channel delivery, and a
// synthesized event carries none.
// Owner: SDK provider channel
#[tokio::test]
async fn observed_event_timestamp_survives_channel_delivery() {
    let (events, mut receiver) = provider_event_channel(NonZeroUsize::new(2).unwrap());
    let observed_at = Instant::now();
    let observed = ModelEvent::OutputDelta("hello".into());
    let synthesized = ModelEvent::OutputDelta("synthesized".into());

    events
        .send_observed(observed.clone(), observed_at)
        .await
        .unwrap();
    events.send_unobserved(synthesized.clone()).await.unwrap();

    assert_eq!(
        receiver.recv_timed_stream_event().await,
        Some((
            super::ProviderStreamEvent::Model(observed),
            Some(observed_at)
        ))
    );
    assert_eq!(
        receiver.recv_timed_stream_event().await,
        Some((super::ProviderStreamEvent::Model(synthesized), None))
    );
}

// Covers: generic provider channels must not reinterpret reserved provider context.
// Owner: SDK provider channel
#[tokio::test]
async fn public_receiver_preserves_generation_output_token_carrier() {
    let (events, mut receiver) = provider_event_channel(NonZeroUsize::new(2).unwrap());
    let expected_carrier =
        ModelEvent::GenerationOutputTokens(crate::model::GenerationOutputTokens::Reported(12));
    events.send(expected_carrier.clone()).await.unwrap();
    let expected_output = ModelEvent::OutputDelta("done".into());
    events.send(expected_output.clone()).await.unwrap();
    drop(events);

    assert_eq!(receiver.recv().await, Some(expected_carrier));
    assert_eq!(receiver.recv().await, Some(expected_output));
    assert_eq!(receiver.recv().await, None);
}

#[tokio::test]
async fn cancellation_stops_a_scripted_request_before_consuming_a_turn() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![]))],
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = provider
        .send_turn(request(&[], &[], cancellation))
        .await
        .unwrap_err();

    assert_eq!(error.kind(), ProviderErrorKind::Interrupted);
    assert!(provider.recorded_requests().is_empty());
}

#[tokio::test]
async fn exhausted_script_is_reported_as_invalid_response() {
    let provider = ScriptedProvider::new(identity(), []);

    let error = provider
        .send_turn(request(&[], &[], CancellationToken::new()))
        .await
        .unwrap_err();

    assert_eq!(error.kind(), ProviderErrorKind::InvalidResponse);
}
